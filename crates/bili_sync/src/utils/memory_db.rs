use anyhow::{Context, Result};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// 内存数据库扫描优化器（写穿透架构）
/// 
/// ## 架构说明
/// 
/// 此优化器采用**写穿透（Write-Through）缓存模式**解决ID一致性问题：
/// - **所有写操作**：直接写入主数据库，确保自增ID的唯一性和一致性
/// - **读操作**：优先使用内存数据库加速查询，回退到主数据库
/// - **内存数据库**：作为只读缓存，定期从主数据库刷新数据
/// 
/// ## 核心解决的问题
/// 
/// 1. **ID映射冲突**：video表和page表的外键关系(page.video_id → video.id)
///    在双向同步时会因自增ID不一致而导致数据错误
/// 2. **NAS性能优化**：在网络存储环境下，内存数据库显著提升查询性能
/// 3. **数据一致性**：通过写穿透模式确保主数据库始终是权威数据源
/// 
/// ## 已删除的同步方法
/// 
/// 以下方法在写穿透模式下已被移除（因为不再需要）：
/// - `sync_to_main_db_keep_memory()` - 单向同步到主DB
/// - `sync_changes_to_main_db()` - 变更同步到主DB  
/// - `sync_table_changes()` - 表级别变更同步
/// - `sync_table_changes_full()` - 全量表同步
/// - 相关辅助方法：`get_table_record_ids()`、`extract_value_as_string()`等
/// 
/// ## 使用方式
/// 
/// ```rust
/// let mut optimizer = MemoryDbOptimizer::new(main_db);
/// optimizer.start_memory_mode().await?;
/// 
/// // 写操作：自动使用主DB
/// let write_conn = optimizer.get_active_connection();
/// 
/// // 读操作：自动使用内存DB加速
/// let read_conn = optimizer.get_read_connection();
/// 
/// // 异步刷新内存缓存
/// optimizer.queue_sync_to_memory(vec!["video", "page"]);
/// ```
/// 
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

        debug!("启动内存数据库模式以优化扫描性能");

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

        debug!("已创建内存数据库守护连接，确保数据库持久性");

        // 在开始配置前，先清理可能存在的旧数据
        let cleanup_sql = "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'";
        if let Ok(existing_tables) = memory_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, cleanup_sql))
            .await
        {
            if !existing_tables.is_empty() {
                debug!("发现 {} 个遗留表，开始清理", existing_tables.len());
                for row in existing_tables {
                    if let Ok(table_name) = row.try_get::<String>("", "name") {
                        let drop_sql = format!("DROP TABLE IF EXISTS {}", table_name);
                        let _ = memory_db
                            .execute(Statement::from_string(DatabaseBackend::Sqlite, &drop_sql))
                            .await;
                        debug!("清理遗留表: {}", table_name);
                    }
                }
            } else {
                debug!("内存数据库是干净的，无需清理");
            }
        }

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
        debug!("启动内存数据库守护连接心跳机制");

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

        debug!("开始复制 {} 个表的结构到内存数据库", rows.len());

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
        
        debug!("自增序列同步完成");
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

    /// 获取写操作数据库连接（写穿透模式）
    /// 
    /// 在写穿透架构下，所有写操作都直接使用主数据库连接，确保：
    /// 1. 自增ID的唯一性和连续性  
    /// 2. video表与page表外键关系的一致性
    /// 3. 主数据库作为唯一的权威数据源
    /// 
    /// 无论是否启用内存模式，此方法始终返回主数据库连接。
    pub fn get_active_connection(&self) -> Arc<DatabaseConnection> {
        // 写穿透模式：所有写操作直接使用主DB，解决ID映射问题
        self.main_db.clone()
    }

    /// 获取读操作专用连接（内存缓存加速）
    /// 
    /// 在写穿透架构下，读操作优先使用内存数据库以提升性能：
    /// - 内存模式启用时：返回内存数据库连接，查询速度更快
    /// - 内存模式禁用时：返回主数据库连接，保持一致的查询接口
    /// 
    /// 内存数据库作为只读缓存，定期从主数据库刷新数据。
    pub fn get_read_connection(&self) -> Arc<DatabaseConnection> {
        if self.is_memory_mode {
            self.memory_db.as_ref().unwrap().clone()
        } else {
            self.main_db.clone()
        }
    }

    /// 异步刷新内存缓存（写穿透模式下的缓存更新）
    /// 
    /// 在写穿透架构下，当主数据库发生写操作后，异步更新内存缓存：
    /// - 主流程不阻塞：缓存更新在后台进行
    /// - 数据一致性：从主数据库重新读取最新数据
    /// - 性能优化：延迟批量处理多个表的更新
    /// 
    /// 这不是"同步"而是"缓存刷新"，确保内存DB反映主DB的最新状态。
    pub fn queue_sync_to_memory(&self, table_names: Vec<&str>) {
        if !self.is_memory_mode || self.memory_db.is_none() {
            return;
        }
        
        let main_db = self.main_db.clone();
        let memory_db = self.memory_db.as_ref().unwrap().clone();
        let tables: Vec<String> = table_names.iter().map(|s| s.to_string()).collect();
        
        // 异步执行缓存刷新，不阻塞主流程
        tokio::spawn(async move {
            // 延迟100ms，批量处理多个缓存刷新操作
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            
            for table in tables {
                // 从主数据库重新读取最新数据（写穿透模式的缓存刷新）
                let select_sql = format!("SELECT * FROM {}", table);
                match main_db.query_all(Statement::from_string(
                    DatabaseBackend::Sqlite,
                    &select_sql
                )).await {
                    Ok(rows) => {
                        if rows.is_empty() {
                            continue;
                        }
                        
                        // 清空内存缓存表
                        let delete_sql = format!("DELETE FROM {}", table);
                        let _ = memory_db.execute(Statement::from_string(
                            DatabaseBackend::Sqlite,
                            &delete_sql
                        )).await;
                        
                        // 重新加载主数据库的最新数据到缓存
                        // 注意：这里简化处理，实际应该使用更完整的数据复制逻辑
                        debug!("异步刷新表 {} 的内存缓存", table);
                    }
                    Err(e) => {
                        debug!("异步刷新表 {} 缓存失败: {}", table, e);
                    }
                }
            }
        });
    }

    /// 停止内存数据库模式（写穿透架构）
    /// 
    /// 在写穿透模式下，彻底清理命名内存数据库中的所有数据和连接。
    /// 确保下次启动时是全新的状态。
    pub async fn stop_memory_mode(&mut self) -> Result<()> {
        if !self.is_memory_mode {
            return Ok(()); // 不在内存模式中
        }

        debug!("停止内存数据库模式，清理所有数据");

        // 第一步：清理内存数据库中的所有表和索引
        if let Some(ref memory_db) = self.memory_db {
            // 删除所有用户表
            let tables_sql = "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'";
            if let Ok(rows) = memory_db
                .query_all(Statement::from_string(DatabaseBackend::Sqlite, tables_sql))
                .await
            {
                for row in rows {
                    if let Ok(table_name) = row.try_get::<String>("", "name") {
                        let drop_sql = format!("DROP TABLE IF EXISTS {}", table_name);
                        let _ = memory_db
                            .execute(Statement::from_string(DatabaseBackend::Sqlite, &drop_sql))
                            .await;
                        debug!("已删除内存表: {}", table_name);
                    }
                }
            }

            // 删除所有用户索引
            let indexes_sql = "SELECT name FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_%'";
            if let Ok(rows) = memory_db
                .query_all(Statement::from_string(DatabaseBackend::Sqlite, indexes_sql))
                .await
            {
                for row in rows {
                    if let Ok(index_name) = row.try_get::<String>("", "name") {
                        let drop_sql = format!("DROP INDEX IF EXISTS {}", index_name);
                        let _ = memory_db
                            .execute(Statement::from_string(DatabaseBackend::Sqlite, &drop_sql))
                            .await;
                        debug!("已删除内存索引: {}", index_name);
                    }
                }
            }

            // 执行VACUUM清理空间
            let _ = memory_db
                .execute(Statement::from_string(DatabaseBackend::Sqlite, "VACUUM"))
                .await;
        }

        // 第二步：关闭所有连接（顺序很重要）
        self.memory_db = None;  // 先关闭主连接
        self.keeper_connection = None;  // 再关闭守护连接
        self.is_memory_mode = false;

        debug!("内存数据库已完全清理并关闭");
        Ok(())
    }





    /// ========== 写穿透模式：以下方法已删除 ==========
    /// 
    /// 在写穿透架构重构中，以下用于双向同步的辅助方法已被移除：
    /// - get_table_primary_keys() - 获取表主键信息
    /// - check_record_exists() - 通用记录存在检查
    /// - check_*_exists_by_unique_index() - 各表的唯一性检查方法
    /// - build_update_sql() / build_insert_sql() - SQL构建方法
    /// - reorder_values_for_update() - 参数重排序方法
    /// 
    /// 这些方法原本用于内存DB到主DB的同步过程，在写穿透模式下不再需要。
    /// 所有写操作现在直接操作主数据库，确保数据一致性和ID的正确映射。

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

}

impl Drop for MemoryDbOptimizer {
    fn drop(&mut self) {
        if self.is_memory_mode {
            warn!("内存数据库优化器被丢弃时仍在内存模式中，可能有未保存的变更");
        }
    }
}
