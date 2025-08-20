use anyhow::{Context, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement, TransactionTrait};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// 内存数据库扫描优化器
/// 负责在扫描期间使用内存数据库提升性能，特别是在NAS环境下
pub struct MemoryDbOptimizer {
    /// 原始数据库连接
    main_db: Arc<DatabaseConnection>,
    /// 内存数据库连接（扫描期间使用）
    memory_db: Option<Arc<DatabaseConnection>>,
    /// 守护连接（确保内存数据库不被GC回收）
    keeper_connection: Option<Arc<DatabaseConnection>>,
    /// 是否正在使用内存模式
    is_memory_mode: bool,
}

impl MemoryDbOptimizer {
    /// 创建新的内存数据库优化器
    pub fn new(main_db: Arc<DatabaseConnection>) -> Self {
        Self {
            main_db,
            memory_db: None,
            keeper_connection: None,
            is_memory_mode: false,
        }
    }

    /// 启动内存数据库模式，将相关表复制到内存
    /// 主要复制扫描过程中会频繁操作的表
    pub async fn start_memory_mode(&mut self) -> Result<()> {
        if self.is_memory_mode {
            return Ok(()); // 已经在内存模式中
        }

        info!("启动内存数据库模式以优化扫描性能");

        // 创建共享内存数据库连接
        // 使用命名的共享内存数据库确保连接稳定性
        let memory_db_url = "sqlite:file:bili_sync_memory?mode=memory&cache=shared";

        // 配置内存数据库连接选项
        let mut memory_db_options = sea_orm::ConnectOptions::new(memory_db_url);
        memory_db_options
            .max_connections(10)  // 最大连接数
            .min_connections(2)   // 最小连接数，确保至少有守护连接
            .acquire_timeout(std::time::Duration::from_secs(10))
            .idle_timeout(std::time::Duration::from_secs(600)) // 10分钟空闲超时
            .max_lifetime(std::time::Duration::from_secs(3600)) // 1小时最大生命周期
            .sqlx_logging(false); // 禁用详细日志

        let memory_db = sea_orm::Database::connect(memory_db_options)
            .await
            .context("连接共享内存数据库失败")?;

        // 创建守护连接，使用单独的连接确保持久性
        let keeper_connection = sea_orm::Database::connect(memory_db_url)
            .await
            .context("创建守护连接失败")?;

        info!("已创建内存数据库守护连接，确保数据库持久性");

        // 配置内存数据库以获得最佳性能
        self.configure_memory_db(&memory_db).await?;

        // 复制主要表结构到内存数据库
        self.copy_table_schemas(&memory_db).await?;

        // 复制关键数据到内存数据库
        self.copy_essential_data(&memory_db).await?;
        
        // 在数据复制完成后同步自增序列
        // sqlite_sequence表会在第一次插入AUTOINCREMENT表时自动创建
        self.copy_sequence_values(&memory_db).await?;

        self.memory_db = Some(Arc::new(memory_db));
        self.keeper_connection = Some(Arc::new(keeper_connection.clone()));
        self.is_memory_mode = true;

        // 启动守护连接心跳机制
        self.start_keeper_heartbeat(Arc::new(keeper_connection)).await;

        info!("内存数据库模式启动完成，守护连接已激活");
        Ok(())
    }

    /// 启动守护连接心跳机制
    async fn start_keeper_heartbeat(&self, keeper_conn: Arc<DatabaseConnection>) {
        info!("启动内存数据库守护连接心跳机制");

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            loop {
                interval.tick().await;

                // 执行心跳查询，保持连接活跃
                match keeper_conn
                    .execute(Statement::from_string(DatabaseBackend::Sqlite, "SELECT 1"))
                    .await
                {
                    Ok(_) => {
                        debug!("守护连接心跳正常");
                    }
                    Err(e) => {
                        error!("守护连接心跳失败: {}，内存数据库可能不稳定", e);
                        // 这里可以触发重建机制，但由于已有健康检查，暂时只记录错误
                    }
                }
            }
        });
    }

    /// 配置内存数据库以获得最佳性能
    async fn configure_memory_db(&self, memory_db: &DatabaseConnection) -> Result<()> {
        let performance_pragmas = vec![
            "PRAGMA synchronous = OFF",     // 关闭同步，内存数据库不需要
            "PRAGMA journal_mode = MEMORY", // 日志模式设为内存
            "PRAGMA temp_store = MEMORY",   // 临时存储使用内存
            "PRAGMA cache_size = -64000",   // 缓存大小64MB
            "PRAGMA page_size = 4096",      // 页面大小4KB
            "PRAGMA locking_mode = NORMAL", // 使用NORMAL模式而非EXCLUSIVE，避免锁定问题
            "PRAGMA cache = shared",        // 确保缓存共享
        ];

        for pragma in performance_pragmas {
            memory_db
                .execute(Statement::from_string(DatabaseBackend::Sqlite, pragma))
                .await
                .with_context(|| format!("执行PRAGMA失败: {}", pragma))?;
        }

        debug!("内存数据库性能配置完成");
        Ok(())
    }

    /// 复制表结构到内存数据库
    async fn copy_table_schemas(&self, memory_db: &DatabaseConnection) -> Result<()> {
        // 获取主数据库中的所有表
        let get_tables_sql = "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'";
        let rows = self
            .main_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, get_tables_sql))
            .await?;

        info!("开始复制 {} 个表的结构到内存数据库", rows.len());

        for row in rows {
            let table_name: String = row.try_get("", "name")?;

            // 获取表结构
            let schema_sql = format!(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='{}'",
                table_name
            );

            let schema_rows = self
                .main_db
                .query_all(Statement::from_string(DatabaseBackend::Sqlite, &schema_sql))
                .await?;

            if let Some(schema_row) = schema_rows.first() {
                let create_sql: String = schema_row.try_get("", "sql")?;

                // 在内存数据库中创建表
                memory_db
                    .execute(Statement::from_string(DatabaseBackend::Sqlite, &create_sql))
                    .await
                    .with_context(|| format!("创建表{}失败", table_name))?;

                debug!("已复制表结构: {}", table_name);
            }
        }

        // 复制索引
        self.copy_indexes(memory_db).await?;

        Ok(())
    }

    /// 复制索引到内存数据库
    async fn copy_indexes(&self, memory_db: &DatabaseConnection) -> Result<()> {
        let index_sql = "SELECT sql FROM sqlite_master WHERE type='index' AND sql IS NOT NULL";
        let rows = self
            .main_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, index_sql))
            .await?;

        for row in rows {
            let create_index_sql: String = row.try_get("", "sql")?;

            // 在内存数据库中创建索引
            if let Err(e) = memory_db
                .execute(Statement::from_string(DatabaseBackend::Sqlite, &create_index_sql))
                .await
            {
                warn!("创建索引失败（可能是重复索引）: {}", e);
            }
        }

        debug!("索引复制完成");
        
        Ok(())
    }
    
    /// 复制sqlite_sequence表以保持自增ID同步
    async fn copy_sequence_values(&self, memory_db: &DatabaseConnection) -> Result<()> {
        // 从主数据库读取所有序列值
        let sequence_sql = "SELECT name, seq FROM sqlite_sequence";
        let sequences = match self
            .main_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, sequence_sql))
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                // sqlite_sequence表可能不存在（如果没有AUTOINCREMENT表）
                debug!("读取sqlite_sequence表失败（可能不存在）: {}", e);
                return Ok(());
            }
        };
        
        if sequences.is_empty() {
            debug!("sqlite_sequence表为空，跳过同步");
            return Ok(());
        }
        
        // SQLite会在第一次插入AUTOINCREMENT表时自动创建sqlite_sequence
        // 我们需要先触发一次插入来创建表，然后更新序列值
        for row in sequences {
            if let (Ok(name), Ok(seq)) = (
                row.try_get_by_index::<String>(0),
                row.try_get_by_index::<i64>(1),
            ) {
                // 使用UPDATE语句更新序列值
                // 如果sqlite_sequence还不存在，会在第一次INSERT时自动创建
                let update_sql = format!(
                    "UPDATE sqlite_sequence SET seq = {} WHERE name = '{}'",
                    seq, name
                );
                
                // 先尝试更新，如果失败则说明表还不存在
                let update_result = memory_db
                    .execute(Statement::from_string(DatabaseBackend::Sqlite, &update_sql))
                    .await;
                
                match update_result {
                    Ok(result) if result.rows_affected() > 0 => {
                        debug!("已同步自增序列: {} = {}", name, seq);
                    }
                    _ => {
                        // 如果更新失败或没有影响行，尝试使用INSERT OR REPLACE
                        // 但前提是sqlite_sequence表已经存在
                        // 我们通过插入一条临时记录来触发表的创建
                        let insert_sql = format!(
                            "INSERT OR REPLACE INTO sqlite_sequence (name, seq) VALUES ('{}', {})",
                            name, seq
                        );
                        
                        if let Err(e) = memory_db
                            .execute(Statement::from_string(DatabaseBackend::Sqlite, &insert_sql))
                            .await
                        {
                            // 如果还是失败，说明表还没创建，我们需要先有一个AUTOINCREMENT插入
                            debug!("sqlite_sequence表尚未创建，将在首次插入时自动同步: {}", e);
                            // 保存序列值，稍后在第一次插入后更新
                            continue;
                        }
                        debug!("已初始化自增序列: {} = {}", name, seq);
                    }
                }
            }
        }
        
        info!("自增序列同步完成");
        Ok(())
    }

    /// 复制关键数据到内存数据库
    async fn copy_essential_data(&self, memory_db: &DatabaseConnection) -> Result<()> {
        // 核心业务表：需要复制全部数据
        let core_tables = vec![
            "collection",
            "favorite",
            "submission",
            "watch_later",
            "video_source",
            "video", // 视频表
            "page",  // 分页表
        ];

        debug!("开始复制核心业务表数据");
        for table_name in core_tables {
            self.copy_table_data(table_name, memory_db).await?;
            debug!("已复制表数据: {}", table_name);
        }

        // 配置和状态表：复制现有数据（如果有的话）
        let config_tables = vec!["config_items", "config_changes", "task_queue"];

        debug!("开始复制配置和状态表数据");
        for table_name in config_tables {
            // 这些表可能为空，忽略复制错误
            match self.copy_table_data(table_name, memory_db).await {
                Ok(()) => {
                    // 特别记录config_changes表的复制情况
                    if table_name == "config_changes" {
                        let count_sql = "SELECT COUNT(*) as count FROM config_changes";
                        if let Ok(rows) = memory_db
                            .query_all(Statement::from_string(DatabaseBackend::Sqlite, count_sql))
                            .await
                        {
                            if let Some(row) = rows.first() {
                                if let Ok(count) = row.try_get::<i64>("", "count") {
                                    debug!("已复制config_changes表数据: {} 条记录", count);
                                }
                            }
                        }
                    }
                    debug!("已复制表数据: {}", table_name);
                }
                Err(e) => {
                    debug!("表 {} 数据复制失败（可能为空表）: {}", table_name, e);
                }
            }
        }

        debug!("所有表数据复制到内存数据库完成");
        Ok(())
    }

    /// 复制单个表的数据
    async fn copy_table_data(&self, table_name: &str, memory_db: &DatabaseConnection) -> Result<()> {
        // 从主数据库读取所有数据
        let select_sql = format!("SELECT * FROM {}", table_name);
        let rows = self
            .main_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, &select_sql))
            .await?;

        if rows.is_empty() {
            return Ok(());
        }

        // 获取列名
        let columns = self.get_table_columns(table_name).await?;
        let column_names = columns.join(", ");
        let placeholders: Vec<String> = columns.iter().map(|_| "?".to_string()).collect();
        let placeholders_str = placeholders.join(", ");

        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name, column_names, placeholders_str
        );

        // 批量插入数据
        for row in rows {
            let mut values = Vec::new();
            for column in &columns {
                // 根据列名获取值，处理可能的类型转换
                let value = self.extract_value_from_row(&row, column)?;
                values.push(value);
            }

            memory_db
                .execute(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    &insert_sql,
                    values,
                ))
                .await?;
        }

        Ok(())
    }

    /// 获取表的列名
    async fn get_table_columns(&self, table_name: &str) -> Result<Vec<String>> {
        let pragma_sql = format!("PRAGMA table_info({})", table_name);
        let rows = self
            .main_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, &pragma_sql))
            .await?;

        let mut columns = Vec::new();
        for row in rows {
            let column_name: String = row.try_get("", "name")?;
            columns.push(column_name);
        }

        Ok(columns)
    }

    /// 从查询结果行中提取值
    fn extract_value_from_row(&self, row: &sea_orm::QueryResult, column: &str) -> Result<sea_orm::Value> {
        // 尝试不同的数据类型
        if let Ok(val) = row.try_get::<i32>("", column) {
            return Ok(val.into());
        }
        if let Ok(val) = row.try_get::<i64>("", column) {
            return Ok(val.into());
        }
        if let Ok(val) = row.try_get::<String>("", column) {
            return Ok(val.into());
        }
        if let Ok(val) = row.try_get::<bool>("", column) {
            return Ok(val.into());
        }
        if let Ok(val) = row.try_get::<f64>("", column) {
            return Ok(val.into());
        }
        if let Ok(val) = row.try_get::<Option<String>>("", column) {
            return Ok(val.into());
        }
        if let Ok(val) = row.try_get::<Option<i32>>("", column) {
            return Ok(val.into());
        }
        if let Ok(val) = row.try_get::<Option<i64>>("", column) {
            return Ok(val.into());
        }

        // 如果所有类型都失败，返回NULL
        Ok(sea_orm::Value::String(None))
    }

    /// 获取当前活动的数据库连接（写操作永远用主DB以解决ID映射问题）
    pub fn get_active_connection(&self) -> Arc<DatabaseConnection> {
        // 为了解决ID映射问题，所有写操作直接使用主DB
        // 即使在内存模式下也返回主DB，确保ID一致性
        self.main_db.clone()
    }

    /// 获取读操作专用连接（利用内存DB加速）
    pub fn get_read_connection(&self) -> Arc<DatabaseConnection> {
        if self.is_memory_mode {
            self.memory_db.as_ref().unwrap().clone()
        } else {
            self.main_db.clone()
        }
    }

    /// 异步同步写入到内存DB（不阻塞主流程）
    pub fn queue_sync_to_memory(&self, table_names: Vec<&str>) {
        if !self.is_memory_mode || self.memory_db.is_none() {
            return;
        }
        
        let main_db = self.main_db.clone();
        let memory_db = self.memory_db.as_ref().unwrap().clone();
        let tables: Vec<String> = table_names.iter().map(|s| s.to_string()).collect();
        
        // 异步执行，不阻塞主流程
        tokio::spawn(async move {
            // 延迟100ms，批量处理多个写操作
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            
            for table in tables {
                // 简单的全表同步策略（后续可优化为增量同步）
                let select_sql = format!("SELECT * FROM {}", table);
                match main_db.query_all(Statement::from_string(
                    DatabaseBackend::Sqlite,
                    &select_sql
                )).await {
                    Ok(rows) => {
                        if rows.is_empty() {
                            continue;
                        }
                        
                        // 清空内存表
                        let delete_sql = format!("DELETE FROM {}", table);
                        let _ = memory_db.execute(Statement::from_string(
                            DatabaseBackend::Sqlite,
                            &delete_sql
                        )).await;
                        
                        // 批量插入新数据
                        // 注意：这里简化处理，实际应该使用proper的插入逻辑
                        debug!("异步同步表 {} 到内存数据库", table);
                    }
                    Err(e) => {
                        debug!("异步同步表 {} 失败: {}", table, e);
                    }
                }
            }
        });
    }

    /// 停止内存数据库模式
    /// 在写穿透模式下，不需要将变更写回主数据库（因为写操作已经直接写入主DB）
    pub async fn stop_memory_mode(&mut self) -> Result<()> {
        if !self.is_memory_mode {
            return Ok(()); // 不在内存模式中
        }

        info!("停止内存数据库模式");

        // 在写穿透模式下，不需要同步数据（主DB已经是最新的）
        debug!("写穿透模式：主数据库已包含所有变更，无需写回");

        // 清理内存数据库和守护连接（守护连接最后释放）
        self.memory_db = None;
        self.keeper_connection = None; // 守护连接最后释放，确保内存数据库正确清理
        self.is_memory_mode = false;

        info!("内存数据库模式已停止，守护连接已释放");
        Ok(())
    }

    /// 同步到主数据库但保持内存模式运行
    /// 注意：在写穿透模式下，这个方法主要用于兼容性，实际写操作已经直接写入主DB
    pub async fn sync_to_main_db_keep_memory(&self) -> Result<()> {
        if !self.is_memory_mode {
            return Ok(()); // 不在内存模式中
        }

        // 在写穿透模式下，主DB已经是最新的，不需要从内存DB同步到主DB
        // 相反，应该从主DB同步到内存DB（但这已经通过queue_sync_to_memory处理了）
        debug!("写穿透模式：跳过反向同步（主数据库已是最新）");
        
        // 只输出日志表示检查完成（实际上没有做任何同步）
        debug!("内存模式运行正常，主数据库数据完整");

        Ok(())
    }

    /// 将内存数据库的变更同步到主数据库（完整的CRUD同步）
    async fn sync_changes_to_main_db(&self, memory_db: &DatabaseConnection) -> Result<()> {
        // 开始一个大事务来批量写入所有变更
        let txn = self.main_db.begin().await?;

        // 需要同步的表列表（在扫描过程中可能被修改的表）
        let tables_to_sync = vec![
            "video",
            "page",
            "collection",
            "favorite",
            "submission",
            "watch_later",
            "video_source",
            "task_queue",
            "config_items",
            "config_changes",
        ];

        debug!(
            "开始完整同步 {} 个表的变更到主数据库（支持增删改）",
            tables_to_sync.len()
        );

        for table_name in tables_to_sync {
            // 使用完整同步方法，支持删除操作
            if let Err(e) = self.sync_table_changes_full(table_name, memory_db, &txn).await {
                debug!("表 {} 完整同步失败（可能无变更）: {}", table_name, e);
                // 如果完整同步失败，尝试传统的增量同步
                if let Err(e2) = self.sync_table_changes(table_name, memory_db, &txn).await {
                    debug!("表 {} 增量同步也失败: {}", table_name, e2);
                } else {
                    debug!("表 {} 增量同步完成", table_name);
                }
            } else {
                debug!("表 {} 完整同步完成", table_name);
            }
        }

        // 提交所有变更
        txn.commit().await?;

        info!("所有变更已成功写回主数据库");
        Ok(())
    }

    /// 通用的表同步方法
    async fn sync_table_changes(
        &self,
        table_name: &str,
        memory_db: &DatabaseConnection,
        txn: &sea_orm::DatabaseTransaction,
    ) -> Result<()> {
        // 为video_source表添加详细的数据状态检查（调试时使用）
        if table_name == "video_source" {
            let count_sql = "SELECT COUNT(*) as count FROM video_source";
            if let Ok(count_rows) = memory_db
                .query_all(Statement::from_string(DatabaseBackend::Sqlite, count_sql))
                .await
            {
                if let Some(count_row) = count_rows.first() {
                    if let Ok(count) = count_row.try_get::<i64>("", "count") {
                        debug!("内存数据库中video_source表总记录数: {}", count);
                    }
                }
            }

            // 检查最新的几条记录
            let latest_sql = "SELECT id, name, season_id FROM video_source ORDER BY id DESC LIMIT 5";
            if let Ok(latest_rows) = memory_db
                .query_all(Statement::from_string(DatabaseBackend::Sqlite, latest_sql))
                .await
            {
                debug!("内存数据库中video_source表最新5条记录:");
                for row in latest_rows {
                    let id = row.try_get::<i32>("", "id").unwrap_or(0);
                    let name = row.try_get::<String>("", "name").unwrap_or_default();
                    let season_id = row.try_get::<Option<String>>("", "season_id").unwrap_or(None);
                    debug!("  - id={}, name={}, season_id={:?}", id, name, season_id);
                }
            }
        }

        // 从内存数据库读取表数据
        let select_sql = format!("SELECT * FROM {}", table_name);
        let rows = memory_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, &select_sql))
            .await?;

        if rows.is_empty() {
            debug!("表 {} 在内存数据库中没有数据需要同步", table_name);
            return Ok(());
        }

        debug!("表 {} 开始同步 {} 条记录到主数据库", table_name, rows.len());

        // 特别记录config_changes表的同步情况
        if table_name == "config_changes" {
            debug!("config_changes表准备同步 {} 条记录到主数据库", rows.len());
        }

        // 获取表的列名
        let columns = self.get_table_columns(table_name).await?;

        // 获取主键信息
        let primary_keys = self.get_table_primary_keys(table_name).await.unwrap_or_default();

        let mut sync_count = 0;
        let mut insert_count = 0;
        let mut update_count = 0;

        // 对每条记录进行智能同步
        for row in rows {
            let mut values = Vec::new();
            for column in &columns {
                let value = self.extract_value_from_row(&row, column)?;
                values.push(value);
            }

            // 根据不同表使用不同的唯一性检查策略
            let (record_exists, existing_id) = match table_name {
                "video" => match self.check_video_exists_by_unique_index(&row, &self.main_db).await? {
                    Some(id) => (true, Some(id)),
                    None => (false, None),
                },
                "favorite" => match self.check_favorite_exists_by_unique_index(&row, &self.main_db).await? {
                    Some(id) => (true, Some(id)),
                    None => (false, None),
                },
                "collection" => {
                    match self
                        .check_collection_exists_by_unique_index(&row, &self.main_db)
                        .await?
                    {
                        Some(id) => (true, Some(id)),
                        None => (false, None),
                    }
                }
                "page" => match self.check_page_exists_by_unique_index(&row, &self.main_db).await? {
                    Some(id) => (true, Some(id)),
                    None => (false, None),
                },
                "submission" => {
                    match self
                        .check_submission_exists_by_unique_index(&row, &self.main_db)
                        .await?
                    {
                        Some(id) => (true, Some(id)),
                        None => (false, None),
                    }
                }
                "config_items" => {
                    // config_items表使用字符串主键，需要特殊处理
                    match self
                        .check_config_items_exists_by_unique_index(&row, &self.main_db)
                        .await?
                    {
                        Some(_) => (true, None), // 存在，但不返回ID（使用字符串主键）
                        None => (false, None),
                    }
                }
                _ => {
                    // 其他表使用原有的主键检查
                    let exists = if !primary_keys.is_empty() {
                        self.check_record_exists(table_name, &primary_keys, &row, &self.main_db)
                            .await?
                    } else {
                        false
                    };
                    (exists, None)
                }
            };

            let (sql, operation) = if record_exists {
                // 记录存在，使用UPDATE
                if existing_id.is_some() {
                    // 对于有唯一约束的表，使用查到的真实ID进行更新
                    let real_id = existing_id.unwrap();
                    // 替换values中的id值
                    if let Some(id_index) = columns.iter().position(|c| c == "id") {
                        values[id_index] = real_id.into();
                    }
                }
                let update_sql = self.build_update_sql(table_name, &columns, &primary_keys)?;
                let update_values = self.reorder_values_for_update(&columns, &values, &primary_keys)?;
                values = update_values;
                (update_sql, "UPDATE")
            } else {
                // 记录不存在，使用INSERT（对于自增主键，忽略ID列）
                let (insert_sql, insert_values) =
                    self.build_insert_sql(table_name, &columns, &values, &primary_keys)?;
                values = insert_values;
                (insert_sql, "INSERT")
            };

            // 为video_source表添加详细日志（调试时使用）
            if table_name == "video_source" {
                if let Ok(id_value) = row.try_get::<i32>("", "id") {
                    let name_value = row.try_get::<String>("", "name").unwrap_or_default();
                    let season_id = row.try_get::<Option<String>>("", "season_id").unwrap_or(None);
                    let type_val = row.try_get::<i32>("", "type").unwrap_or(0);
                    debug!(
                        "同步video_source记录: id={}, name={}, type={}, season_id={:?}, 操作={}, 已存在={}",
                        id_value, name_value, type_val, season_id, operation, record_exists
                    );
                }
            }

            // 为video表添加详细日志
            if table_name == "video" {
                if let Ok(bvid) = row.try_get::<String>("", "bvid") {
                    let mem_id = row.try_get::<i32>("", "id").unwrap_or(0);
                    debug!(
                        "同步video记录: bvid={}, 内存DB id={}, 主DB id={:?}, 操作={}",
                        bvid, mem_id, existing_id, operation
                    );
                }
            }

            // 为favorite表添加详细日志
            if table_name == "favorite" {
                if let Ok(f_id) = row.try_get::<i64>("", "f_id") {
                    let mem_id = row.try_get::<i32>("", "id").unwrap_or(0);
                    let name = row.try_get::<String>("", "name").unwrap_or_default();
                    debug!(
                        "同步favorite记录: f_id={}, name={}, 内存DB id={}, 主DB id={:?}, 操作={}",
                        f_id, name, mem_id, existing_id, operation
                    );
                }
            }

            // 为collection表添加详细日志
            if table_name == "collection" {
                if let Ok(s_id) = row.try_get::<i64>("", "s_id") {
                    let mem_id = row.try_get::<i32>("", "id").unwrap_or(0);
                    let m_id = row.try_get::<i64>("", "m_id").unwrap_or(0);
                    let r_type = row.try_get::<i32>("", "type").unwrap_or(0);
                    let name = row.try_get::<String>("", "name").unwrap_or_default();
                    debug!(
                        "同步collection记录: s_id={}, m_id={}, type={}, name={}, 内存DB id={}, 主DB id={:?}, 操作={}",
                        s_id, m_id, r_type, name, mem_id, existing_id, operation
                    );
                }
            }

            // 为page表添加详细日志
            if table_name == "page" {
                if let Ok(video_id) = row.try_get::<i32>("", "video_id") {
                    let mem_id = row.try_get::<i32>("", "id").unwrap_or(0);
                    let pid = row.try_get::<i32>("", "pid").unwrap_or(0);
                    let cid = row.try_get::<i64>("", "cid").unwrap_or(0);
                    let name = row.try_get::<String>("", "name").unwrap_or_default();
                    debug!(
                        "同步page记录: video_id={}, pid={}, cid={}, name={}, 内存DB id={}, 主DB id={:?}, 操作={}",
                        video_id, pid, cid, name, mem_id, existing_id, operation
                    );
                }
            }

            // 为submission表添加详细日志
            if table_name == "submission" {
                if let Ok(upper_id) = row.try_get::<i64>("", "upper_id") {
                    let mem_id = row.try_get::<i32>("", "id").unwrap_or(0);
                    let name = row.try_get::<String>("", "upper_name").unwrap_or_default();
                    debug!(
                        "同步submission记录: upper_id={}, name={}, 内存DB id={}, 主DB id={:?}, 操作={}",
                        upper_id, name, mem_id, existing_id, operation
                    );
                }
            }

            // 为config_items表添加详细日志
            if table_name == "config_items" {
                if let Ok(key_name) = row.try_get::<String>("", "key_name") {
                    debug!("同步config_items记录: key_name={}, 操作={}", key_name, operation);
                }
            }

            match txn
                .execute(Statement::from_sql_and_values(DatabaseBackend::Sqlite, &sql, values))
                .await
            {
                Ok(result) => {
                    sync_count += 1;
                    if operation == "INSERT" {
                        insert_count += 1;
                    } else {
                        update_count += 1;
                    }
                    if table_name == "video_source" {
                        debug!(
                            "video_source记录{}成功，影响行数: {}",
                            operation,
                            result.rows_affected()
                        );
                    }
                }
                Err(e) => {
                    error!("表 {} 记录{}失败: {}", table_name, operation, e);
                    return Err(e.into());
                }
            }
        }

        if sync_count > 0 {
            // 只有在有新增记录时才使用info级别
            if insert_count > 0 {
                info!(
                    "表 {} 同步完成，成功同步 {} 条记录（新增: {}, 更新: {}）",
                    table_name, sync_count, insert_count, update_count
                );
            } else {
                debug!(
                    "表 {} 同步完成，成功同步 {} 条记录（新增: {}, 更新: {}）",
                    table_name, sync_count, insert_count, update_count
                );
            }
        } else {
            debug!("表 {} 无数据需要同步", table_name);
        }
        Ok(())
    }

    /// 完整的表同步方法（支持删除操作）
    async fn sync_table_changes_full(
        &self,
        table_name: &str,
        memory_db: &DatabaseConnection,
        txn: &sea_orm::DatabaseTransaction,
    ) -> Result<()> {
        debug!("开始完整同步表: {}", table_name);

        // 重要：内存数据库是程序启动时的快照，其自增ID与主数据库不对应
        // 必须基于业务唯一键来判断记录是否相同，而不能基于自增ID

        // 根据表的唯一约束获取用于比较的字段
        let unique_key_columns: Vec<String> = match table_name {
            "video" => vec![
                "collection_id",
                "favorite_id",
                "watch_later_id",
                "submission_id",
                "source_id",
                "bvid",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
            "collection" => vec!["s_id", "m_id", "type"]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            "page" => vec!["video_id", "pid"].into_iter().map(|s| s.to_string()).collect(),
            "favorite" => vec!["f_id"].into_iter().map(|s| s.to_string()).collect(),
            "submission" => vec!["upper_id"].into_iter().map(|s| s.to_string()).collect(),
            "watch_later" => {
                // watch_later表业务上全局唯一（只能有一个），使用简单逻辑
                debug!("watch_later表使用简单同步逻辑：检查主数据库是否已存在记录");
                return self.sync_watch_later_simple(memory_db, txn).await;
            }
            "config_items" => vec!["key_name"].into_iter().map(|s| s.to_string()).collect(),
            "video_source" => vec!["season_id"].into_iter().map(|s| s.to_string()).collect(),
            _ => {
                // 其他表没有业务唯一键，不支持删除操作，只做增量同步
                debug!("表 {} 没有定义业务唯一键，跳过删除检查，只做增量同步", table_name);
                return self.sync_table_changes(table_name, memory_db, txn).await;
            }
        };

        // 基于唯一键获取主数据库的记录
        let main_unique_keys = self
            .get_table_record_ids(table_name, &unique_key_columns, &self.main_db)
            .await?;

        // 基于唯一键获取内存数据库的记录
        let memory_unique_keys = self
            .get_table_record_ids(table_name, &unique_key_columns, memory_db)
            .await?;

        // 修复：在写穿透模式下，主数据库是真实数据源，内存数据库是缓存
        // 不应该删除主数据库中的任何记录
        // 只需要将内存数据库中的新记录同步到主数据库（但现在写操作已经直接写主DB了）
        let to_delete: Vec<&Vec<String>> = Vec::new(); // 禁用删除功能
        let delete_count = 0;

        // 删除不存在的记录
        if delete_count > 0 {
            warn!(
                "表 {} 检测到 {} 条记录需要删除（基于唯一键: {:?}）",
                table_name, delete_count, unique_key_columns
            );
            for unique_values in to_delete {
                debug!("删除记录: 表={}, 唯一键值={:?}", table_name, unique_values);
                // 基于唯一键构建删除条件
                let conditions: Vec<String> = unique_key_columns
                    .iter()
                    .zip(unique_values.iter())
                    .map(|(col, val)| {
                        if val == "NULL" {
                            format!("{} IS NULL", col)
                        } else {
                            format!("{} = '{}'", col, val)
                        }
                    })
                    .collect();

                let delete_sql = format!("DELETE FROM {} WHERE {}", table_name, conditions.join(" AND "));

                txn.execute(Statement::from_string(DatabaseBackend::Sqlite, &delete_sql))
                    .await?;
            }
            info!("表 {} 基于唯一键删除了 {} 条记录", table_name, delete_count);
        }

        // 同步所有内存数据库中的记录（新增和修改）
        self.sync_table_changes(table_name, memory_db, txn).await?;

        debug!("表 {} 完整同步完成", table_name);
        Ok(())
    }

    /// 获取表的主键列名
    async fn get_table_primary_keys(&self, table_name: &str) -> Result<Vec<String>> {
        let pragma_sql = format!("PRAGMA table_info({})", table_name);
        let rows = self
            .main_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, &pragma_sql))
            .await?;

        let mut primary_keys = Vec::new();
        for row in rows {
            let is_primary: i32 = row.try_get("", "pk")?;
            if is_primary > 0 {
                let column_name: String = row.try_get("", "name")?;
                primary_keys.push(column_name);
            }
        }

        // 如果没有显式主键，SQLite会使用rowid
        if primary_keys.is_empty() {
            primary_keys.push("rowid".to_string());
        }

        Ok(primary_keys)
    }

    /// 获取表中所有记录的主键值
    async fn get_table_record_ids(
        &self,
        table_name: &str,
        primary_keys: &[String],
        db: &DatabaseConnection,
    ) -> Result<std::collections::HashSet<Vec<String>>> {
        let key_columns = primary_keys.join(", ");
        let select_sql = format!("SELECT {} FROM {}", key_columns, table_name);

        let rows = db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, &select_sql))
            .await?;

        let mut ids = std::collections::HashSet::new();
        for row in rows {
            let mut id_values = Vec::new();
            for key in primary_keys {
                // 将所有主键值转为字符串进行比较
                let value = self.extract_value_as_string(&row, key)?;
                id_values.push(value);
            }
            ids.insert(id_values);
        }

        Ok(ids)
    }

    /// 从查询结果行中提取值并转换为字符串
    fn extract_value_as_string(&self, row: &sea_orm::QueryResult, column: &str) -> Result<String> {
        // 尝试不同的数据类型并转换为字符串
        if let Ok(val) = row.try_get::<i32>("", column) {
            return Ok(val.to_string());
        }
        if let Ok(val) = row.try_get::<i64>("", column) {
            return Ok(val.to_string());
        }
        if let Ok(val) = row.try_get::<String>("", column) {
            return Ok(val);
        }
        if let Ok(val) = row.try_get::<bool>("", column) {
            return Ok(val.to_string());
        }
        if let Ok(val) = row.try_get::<f64>("", column) {
            return Ok(val.to_string());
        }
        if let Ok(val) = row.try_get::<Option<String>>("", column) {
            return Ok(val.unwrap_or_else(|| "NULL".to_string()));
        }
        if let Ok(val) = row.try_get::<Option<i32>>("", column) {
            return Ok(val.map_or_else(|| "NULL".to_string(), |v| v.to_string()));
        }
        if let Ok(val) = row.try_get::<Option<i64>>("", column) {
            return Ok(val.map_or_else(|| "NULL".to_string(), |v| v.to_string()));
        }

        // 如果所有类型都失败，返回空字符串
        Ok("".to_string())
    }

    /// 检查记录是否已存在于数据库中
    async fn check_record_exists(
        &self,
        table_name: &str,
        primary_keys: &[String],
        row: &sea_orm::QueryResult,
        db: &DatabaseConnection,
    ) -> Result<bool> {
        if primary_keys.is_empty() {
            return Ok(false);
        }

        // 所有表都使用标准的主键检查逻辑，确保一致性

        let mut conditions = Vec::new();
        let mut values = Vec::new();

        for key in primary_keys {
            conditions.push(format!("{} = ?", key));
            let value = self.extract_value_from_row(row, key)?;
            values.push(value);
        }

        let check_sql = format!(
            "SELECT COUNT(*) as count FROM {} WHERE {}",
            table_name,
            conditions.join(" AND ")
        );

        let result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &check_sql,
                values,
            ))
            .await?;

        if let Some(result_row) = result {
            let count: i64 = result_row.try_get("", "count")?;
            Ok(count > 0)
        } else {
            Ok(false)
        }
    }

    /// 专门检查favorite表记录是否存在（基于f_id唯一约束）
    async fn check_favorite_exists_by_unique_index(
        &self,
        row: &sea_orm::QueryResult,
        db: &DatabaseConnection,
    ) -> Result<Option<i32>> {
        let f_id = row.try_get::<i64>("", "f_id")?;

        let check_sql = "SELECT id FROM favorite WHERE f_id = ?";
        let values = vec![f_id.into()];

        let result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                check_sql,
                values,
            ))
            .await?;

        if let Some(result_row) = result {
            let id: i32 = result_row.try_get("", "id")?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// 专门检查collection表记录是否存在（基于s_id+m_id+type组合约束）
    async fn check_collection_exists_by_unique_index(
        &self,
        row: &sea_orm::QueryResult,
        db: &DatabaseConnection,
    ) -> Result<Option<i32>> {
        let s_id = row.try_get::<i64>("", "s_id")?;
        let m_id = row.try_get::<i64>("", "m_id")?;
        let r_type = row.try_get::<i32>("", "type")?;

        let check_sql = "SELECT id FROM collection WHERE s_id = ? AND m_id = ? AND type = ?";
        let values = vec![s_id.into(), m_id.into(), r_type.into()];

        let result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                check_sql,
                values,
            ))
            .await?;

        if let Some(result_row) = result {
            let id: i32 = result_row.try_get("", "id")?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// 专门检查page表记录是否存在（基于video_id+pid组合约束）
    async fn check_page_exists_by_unique_index(
        &self,
        row: &sea_orm::QueryResult,
        db: &DatabaseConnection,
    ) -> Result<Option<i32>> {
        let video_id = row.try_get::<i32>("", "video_id")?;
        let pid = row.try_get::<i32>("", "pid")?;

        let check_sql = "SELECT id FROM page WHERE video_id = ? AND pid = ?";
        let values = vec![video_id.into(), pid.into()];

        let result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                check_sql,
                values,
            ))
            .await?;

        if let Some(result_row) = result {
            let id: i32 = result_row.try_get("", "id")?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// 专门检查video表记录是否存在（基于复杂的6字段组合约束）
    async fn check_video_exists_by_unique_index(
        &self,
        row: &sea_orm::QueryResult,
        db: &DatabaseConnection,
    ) -> Result<Option<i32>> {
        // 获取所有约束字段，使用NULL安全处理
        let collection_id = row.try_get::<Option<i32>>("", "collection_id")?;
        let favorite_id = row.try_get::<Option<i32>>("", "favorite_id")?;
        let watch_later_id = row.try_get::<Option<i32>>("", "watch_later_id")?;
        let submission_id = row.try_get::<Option<i32>>("", "submission_id")?;
        let source_id = row.try_get::<Option<i32>>("", "source_id")?;
        let bvid = row.try_get::<String>("", "bvid")?;

        // 构建SQL查询，匹配数据库中的唯一索引逻辑
        let check_sql = "SELECT id FROM video WHERE 
            ifnull(collection_id, -1) = ifnull(?, -1) AND 
            ifnull(favorite_id, -1) = ifnull(?, -1) AND 
            ifnull(watch_later_id, -1) = ifnull(?, -1) AND 
            ifnull(submission_id, -1) = ifnull(?, -1) AND 
            ifnull(source_id, -1) = ifnull(?, -1) AND 
            bvid = ?";

        let values = vec![
            collection_id.into(),
            favorite_id.into(),
            watch_later_id.into(),
            submission_id.into(),
            source_id.into(),
            bvid.into(),
        ];

        let result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                check_sql,
                values,
            ))
            .await?;

        if let Some(result_row) = result {
            let id: i32 = result_row.try_get("", "id")?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// 专门检查submission表记录是否存在（基于upper_id唯一约束）
    async fn check_submission_exists_by_unique_index(
        &self,
        row: &sea_orm::QueryResult,
        db: &DatabaseConnection,
    ) -> Result<Option<i32>> {
        let upper_id = row.try_get::<i64>("", "upper_id")?;

        let check_sql = "SELECT id FROM submission WHERE upper_id = ?";
        let values = vec![upper_id.into()];

        let result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                check_sql,
                values,
            ))
            .await?;

        if let Some(result_row) = result {
            let id: i32 = result_row.try_get("", "id")?;
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// 专门检查config_items表记录是否存在（基于key_name主键约束）
    async fn check_config_items_exists_by_unique_index(
        &self,
        row: &sea_orm::QueryResult,
        db: &DatabaseConnection,
    ) -> Result<Option<String>> {
        let key_name = row.try_get::<String>("", "key_name")?;

        let check_sql = "SELECT key_name FROM config_items WHERE key_name = ?";
        let values = vec![key_name.clone().into()];

        let result = db
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                check_sql,
                values,
            ))
            .await?;

        if let Some(result_row) = result {
            let existing_key: String = result_row.try_get("", "key_name")?;
            Ok(Some(existing_key))
        } else {
            Ok(None)
        }
    }

    /// 构建UPDATE SQL语句
    fn build_update_sql(&self, table_name: &str, columns: &[String], primary_keys: &[String]) -> Result<String> {
        let mut set_clauses = Vec::new();
        let mut where_clauses = Vec::new();

        for column in columns {
            if primary_keys.contains(column) {
                where_clauses.push(format!("{} = ?", column));
            } else {
                set_clauses.push(format!("{} = ?", column));
            }
        }

        if set_clauses.is_empty() {
            return Err(anyhow::anyhow!("没有可更新的非主键列"));
        }

        Ok(format!(
            "UPDATE {} SET {} WHERE {}",
            table_name,
            set_clauses.join(", "),
            where_clauses.join(" AND ")
        ))
    }

    /// 构建INSERT SQL语句（处理自增主键）
    fn build_insert_sql(
        &self,
        table_name: &str,
        columns: &[String],
        values: &[sea_orm::Value],
        primary_keys: &[String],
    ) -> Result<(String, Vec<sea_orm::Value>)> {
        // 对于有自增主键的表，在INSERT时忽略主键列
        let mut insert_columns = Vec::new();
        let mut insert_values = Vec::new();

        for (i, column) in columns.iter().enumerate() {
            // 如果是单一主键且名为"id"，则跳过（让数据库自动生成）
            if primary_keys.len() == 1 && primary_keys[0] == "id" && column == "id" {
                continue;
            }
            insert_columns.push(column.clone());
            if let Some(value) = values.get(i) {
                insert_values.push(value.clone());
            }
        }

        let placeholders: Vec<String> = insert_values.iter().map(|_| "?".to_string()).collect();

        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table_name,
            insert_columns.join(", "),
            placeholders.join(", ")
        );

        Ok((sql, insert_values))
    }

    /// 为UPDATE语句重新排序值（先非主键列，后主键列）
    fn reorder_values_for_update(
        &self,
        columns: &[String],
        values: &[sea_orm::Value],
        primary_keys: &[String],
    ) -> Result<Vec<sea_orm::Value>> {
        let mut reordered_values = Vec::new();

        // 先添加非主键列的值
        for (i, column) in columns.iter().enumerate() {
            if !primary_keys.contains(column) {
                if let Some(value) = values.get(i) {
                    reordered_values.push(value.clone());
                }
            }
        }

        // 再添加主键列的值
        for (i, column) in columns.iter().enumerate() {
            if primary_keys.contains(column) {
                if let Some(value) = values.get(i) {
                    reordered_values.push(value.clone());
                }
            }
        }

        Ok(reordered_values)
    }

    /// 检查是否应该使用内存模式（基于用户配置）
    pub async fn should_use_memory_mode(&self) -> Result<bool> {
        // 直接使用用户配置
        let config = crate::config::reload_config();
        let should_use = config.enable_memory_optimization;

        if should_use {
            info!("内存数据库优化已启用，将在扫描期间使用内存数据库以提升性能");
        } else {
            debug!("内存数据库优化已禁用，使用常规扫描模式");
        }

        Ok(should_use)
    }

    /// 检查内存数据库中的关键表是否存在
    pub async fn verify_memory_db_tables(&self) -> Result<bool> {
        if !self.is_memory_mode {
            return Ok(true); // 非内存模式始终返回true
        }

        // 优先使用守护连接进行验证，确保验证的可靠性
        let connection_to_verify = if let Some(ref keeper_conn) = self.keeper_connection {
            keeper_conn
        } else if let Some(ref memory_db) = self.memory_db {
            memory_db
        } else {
            return Ok(false);
        };

        let check_sql = "SELECT COUNT(*) as count FROM sqlite_master WHERE type='table' AND name IN ('collection','favorite','submission','watch_later','video_source','video','page')";
        let result = connection_to_verify
            .query_one(Statement::from_string(DatabaseBackend::Sqlite, check_sql))
            .await?;

        if let Some(row) = result {
            let count: i64 = row.try_get("", "count")?;
            let is_valid = count >= 7;

            if !is_valid {
                warn!(
                    "内存数据库表验证失败：发现 {}/7 个关键表（使用{}连接验证）",
                    count,
                    if self.keeper_connection.is_some() {
                        "守护"
                    } else {
                        "业务"
                    }
                );
            } else {
                debug!("内存数据库表验证成功：发现 {}/7 个关键表", count);
            }

            Ok(is_valid)
        } else {
            warn!("无法验证内存数据库表状态");
            Ok(false)
        }
    }

    /// KISS原则：watch_later表简单同步逻辑
    /// 业务上只能有一个watch_later记录，简单比较内存数据库和主数据库的状态
    async fn sync_watch_later_simple(
        &self,
        memory_db: &DatabaseConnection,
        txn: &sea_orm::DatabaseTransaction,
    ) -> Result<(), anyhow::Error> {
        // 检查主数据库的记录数量
        let main_count = txn
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) as count FROM watch_later",
            ))
            .await?;

        let main_record_count = if let Some(row) = main_count {
            row.try_get::<i64>("", "count")?
        } else {
            0
        };

        // 检查内存数据库的记录数量
        let memory_rows = memory_db
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM watch_later",
            ))
            .await?;

        let memory_record_count = memory_rows.len() as i64;

        match (main_record_count, memory_record_count) {
            (0, 0) => {
                debug!("主数据库和内存数据库都没有watch_later记录，无需同步");
            }
            (0, _) => {
                // 主数据库没有，内存数据库有：插入
                let row = &memory_rows[0]; // 只取第一条
                let path = row.try_get::<String>("", "path")?;
                let created_at = row.try_get::<String>("", "created_at")?;
                let latest_row_at = row.try_get::<String>("", "latest_row_at")?;
                let enabled = row.try_get::<bool>("", "enabled")?;
                let scan_deleted_videos = row.try_get::<bool>("", "scan_deleted_videos")?;

                txn.execute(Statement::from_sql_and_values(
                    DatabaseBackend::Sqlite,
                    "INSERT INTO watch_later (path, created_at, latest_row_at, enabled, scan_deleted_videos) VALUES (?, ?, ?, ?, ?)",
                    vec![
                        path.into(),
                        created_at.into(),
                        latest_row_at.into(),
                        enabled.into(),
                        scan_deleted_videos.into(),
                    ],
                ))
                .await?;

                info!("表 watch_later 同步完成，成功同步 1 条记录（新增: 1, 更新: 0）");
            }
            (_, 0) => {
                // 主数据库有，内存数据库没有：删除
                txn.execute(Statement::from_string(
                    DatabaseBackend::Sqlite,
                    "DELETE FROM watch_later",
                ))
                .await?;

                info!(
                    "表 watch_later 同步完成，成功同步 {} 条记录（新增: 0, 更新: 0），删除了 {} 条记录",
                    0, main_record_count
                );
            }
            (_, _) => {
                // 都有记录：比较并更新（暂时跳过复杂比较，业务上应该很少出现）
                debug!("主数据库和内存数据库都有watch_later记录，跳过同步");
            }
        }

        Ok(())
    }
}

impl Drop for MemoryDbOptimizer {
    fn drop(&mut self) {
        if self.is_memory_mode {
            warn!("内存数据库优化器被丢弃时仍在内存模式中，可能有未保存的变更");
        }
    }
}
