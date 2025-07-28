use anyhow::{Context, Result};
use sea_orm::{DatabaseConnection, Statement, ConnectionTrait, DatabaseBackend, TransactionTrait};
use tracing::{debug, info, warn, error};
use std::sync::Arc;

/// 内存数据库扫描优化器
/// 负责在扫描期间使用内存数据库提升性能，特别是在NAS环境下
pub struct MemoryDbOptimizer {
    /// 原始数据库连接
    main_db: Arc<DatabaseConnection>,
    /// 内存数据库连接（扫描期间使用）
    memory_db: Option<Arc<DatabaseConnection>>,
    /// 是否正在使用内存模式
    is_memory_mode: bool,
}

impl MemoryDbOptimizer {
    /// 创建新的内存数据库优化器
    pub fn new(main_db: Arc<DatabaseConnection>) -> Self {
        Self {
            main_db,
            memory_db: None,
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
        let memory_db = sea_orm::Database::connect(memory_db_url)
            .await
            .context("连接共享内存数据库失败")?;

        // 配置内存数据库以获得最佳性能
        self.configure_memory_db(&memory_db).await?;

        // 复制主要表结构到内存数据库
        self.copy_table_schemas(&memory_db).await?;

        // 复制关键数据到内存数据库
        self.copy_essential_data(&memory_db).await?;

        self.memory_db = Some(Arc::new(memory_db));
        self.is_memory_mode = true;

        info!("内存数据库模式启动完成");
        Ok(())
    }

    /// 配置内存数据库以获得最佳性能
    async fn configure_memory_db(&self, memory_db: &DatabaseConnection) -> Result<()> {
        let performance_pragmas = vec![
            "PRAGMA synchronous = OFF",        // 关闭同步，内存数据库不需要
            "PRAGMA journal_mode = MEMORY",    // 日志模式设为内存
            "PRAGMA temp_store = MEMORY",      // 临时存储使用内存
            "PRAGMA cache_size = -64000",      // 缓存大小64MB
            "PRAGMA page_size = 4096",         // 页面大小4KB
            "PRAGMA locking_mode = NORMAL",    // 使用NORMAL模式而非EXCLUSIVE，避免锁定问题
            "PRAGMA cache = shared",           // 确保缓存共享
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
        let rows = self.main_db
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
            
            let schema_rows = self.main_db
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
        let rows = self.main_db
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

    /// 复制关键数据到内存数据库
    async fn copy_essential_data(&self, memory_db: &DatabaseConnection) -> Result<()> {
        // 核心业务表：需要复制全部数据
        let core_tables = vec![
            "collection",
            "favorite", 
            "submission",
            "watch_later",
            "video_source",
            "video",      // 视频表
            "page",       // 分页表
        ];

        info!("开始复制核心业务表数据");
        for table_name in core_tables {
            self.copy_table_data(table_name, memory_db).await?;
            debug!("已复制表数据: {}", table_name);
        }

        // 配置和状态表：复制现有数据（如果有的话）
        let config_tables = vec![
            "config_items",
            "config_changes", 
            "task_queue",
        ];

        info!("开始复制配置和状态表数据");
        for table_name in config_tables {
            // 这些表可能为空，忽略复制错误
            if let Err(e) = self.copy_table_data(table_name, memory_db).await {
                debug!("表 {} 数据复制失败（可能为空表）: {}", table_name, e);
            } else {
                debug!("已复制表数据: {}", table_name);
            }
        }

        info!("所有表数据复制到内存数据库完成");
        Ok(())
    }

    /// 复制单个表的数据
    async fn copy_table_data(&self, table_name: &str, memory_db: &DatabaseConnection) -> Result<()> {
        // 从主数据库读取所有数据
        let select_sql = format!("SELECT * FROM {}", table_name);
        let rows = self.main_db
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
        let rows = self.main_db
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


    /// 获取当前活动的数据库连接（内存模式时返回内存数据库）
    pub fn get_active_connection(&self) -> Arc<DatabaseConnection> {
        if self.is_memory_mode {
            self.memory_db.as_ref().unwrap().clone()
        } else {
            self.main_db.clone()
        }
    }

    /// 停止内存数据库模式，将变更写回主数据库
    pub async fn stop_memory_mode(&mut self) -> Result<()> {
        if !self.is_memory_mode {
            return Ok(()); // 不在内存模式中
        }

        info!("开始将内存数据库变更写回主数据库");

        if let Some(memory_db) = &self.memory_db {
            // 分析和写回变更
            self.sync_changes_to_main_db(memory_db).await?;
        }

        // 清理内存数据库
        self.memory_db = None;
        self.is_memory_mode = false;

        info!("内存数据库模式已停止，变更已写回主数据库");
        Ok(())
    }

    /// 同步到主数据库但保持内存模式运行
    pub async fn sync_to_main_db_keep_memory(&self) -> Result<()> {
        if !self.is_memory_mode {
            return Ok(()); // 不在内存模式中
        }

        info!("开始同步内存数据库变更到主数据库（保持内存模式）");

        if let Some(memory_db) = &self.memory_db {
            self.sync_changes_to_main_db(memory_db).await?;
            info!("数据同步完成，内存模式继续运行");
        }

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

        debug!("开始完整同步 {} 个表的变更到主数据库（支持增删改）", tables_to_sync.len());

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
            if let Ok(count_rows) = memory_db.query_all(Statement::from_string(DatabaseBackend::Sqlite, count_sql)).await {
                if let Some(count_row) = count_rows.first() {
                    if let Ok(count) = count_row.try_get::<i64>("", "count") {
                        debug!("内存数据库中video_source表总记录数: {}", count);
                    }
                }
            }
            
            // 检查最新的几条记录
            let latest_sql = "SELECT id, name, season_id FROM video_source ORDER BY id DESC LIMIT 5";
            if let Ok(latest_rows) = memory_db.query_all(Statement::from_string(DatabaseBackend::Sqlite, latest_sql)).await {
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

            // 检查记录是否已存在于主数据库中
            let record_exists = if !primary_keys.is_empty() {
                self.check_record_exists(table_name, &primary_keys, &row, &self.main_db).await?
            } else {
                false
            };

            let (sql, operation) = if record_exists {
                // 记录存在，使用UPDATE
                let update_sql = self.build_update_sql(table_name, &columns, &primary_keys)?;
                let update_values = self.reorder_values_for_update(&columns, &values, &primary_keys)?;
                values = update_values;
                (update_sql, "UPDATE")
            } else {
                // 记录不存在，使用INSERT（对于自增主键，忽略ID列）
                let (insert_sql, insert_values) = self.build_insert_sql(table_name, &columns, &values, &primary_keys)?;
                values = insert_values;
                (insert_sql, "INSERT")
            };

            // 为video_source表添加详细日志（调试时使用）
            if table_name == "video_source" {
                if let Ok(id_value) = row.try_get::<i32>("", "id") {
                    let name_value = row.try_get::<String>("", "name").unwrap_or_default();
                    let season_id = row.try_get::<Option<String>>("", "season_id").unwrap_or(None);
                    let type_val = row.try_get::<i32>("", "type").unwrap_or(0);
                    debug!("同步video_source记录: id={}, name={}, type={}, season_id={:?}, 操作={}, 已存在={}", 
                        id_value, name_value, type_val, season_id, operation, record_exists);
                }
            }

            match txn.execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &sql,
                values,
            ))
            .await {
                Ok(result) => {
                    sync_count += 1;
                    if operation == "INSERT" {
                        insert_count += 1;
                    } else {
                        update_count += 1;
                    }
                    if table_name == "video_source" {
                        debug!("video_source记录{}成功，影响行数: {}", operation, result.rows_affected());
                    }
                }
                Err(e) => {
                    error!("表 {} 记录{}失败: {}", table_name, operation, e);
                    return Err(e.into());
                }
            }
        }

        if sync_count > 0 {
            info!("表 {} 同步完成，成功同步 {} 条记录（新增: {}, 更新: {}）", 
                table_name, sync_count, insert_count, update_count);
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

        // 获取表的主键列名
        let primary_keys = self.get_table_primary_keys(table_name).await?;
        if primary_keys.is_empty() {
            warn!("表 {} 没有主键，使用传统同步方法", table_name);
            return self.sync_table_changes(table_name, memory_db, txn).await;
        }

        // 获取主数据库中当前的所有记录ID
        let main_ids = self.get_table_record_ids(table_name, &primary_keys, &self.main_db).await?;
        
        // 获取内存数据库中的所有记录ID
        let memory_ids = self.get_table_record_ids(table_name, &primary_keys, memory_db).await?;

        // 找出需要删除的记录（在主数据库中但不在内存数据库中）
        let to_delete: Vec<_> = main_ids.difference(&memory_ids).collect();
        let delete_count = to_delete.len();
        
        // 删除不存在的记录
        for id_values in to_delete {
            self.delete_record_by_primary_key(table_name, &primary_keys, id_values, txn).await?;
        }

        if delete_count > 0 {
            debug!("表 {} 删除了 {} 条记录", table_name, delete_count);
        }

        // 同步所有内存数据库中的记录（新增和修改）
        self.sync_table_changes(table_name, memory_db, txn).await?;

        debug!("表 {} 完整同步完成", table_name);
        Ok(())
    }

    /// 获取表的主键列名
    async fn get_table_primary_keys(&self, table_name: &str) -> Result<Vec<String>> {
        let pragma_sql = format!("PRAGMA table_info({})", table_name);
        let rows = self.main_db
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

    /// 根据主键删除记录
    async fn delete_record_by_primary_key(
        &self,
        table_name: &str,
        primary_keys: &[String],
        id_values: &[String],
        txn: &sea_orm::DatabaseTransaction,
    ) -> Result<()> {
        if primary_keys.len() != id_values.len() {
            return Err(anyhow::anyhow!("主键数量与值数量不匹配"));
        }

        let conditions: Vec<String> = primary_keys
            .iter()
            .zip(id_values.iter())
            .map(|(key, value)| format!("{} = '{}'", key, value))
            .collect();

        let delete_sql = format!(
            "DELETE FROM {} WHERE {}",
            table_name,
            conditions.join(" AND ")
        );

        txn.execute(Statement::from_string(DatabaseBackend::Sqlite, &delete_sql))
            .await?;

        debug!("删除记录: {} WHERE {}", table_name, conditions.join(" AND "));
        Ok(())
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


    /// 构建UPDATE SQL语句
    fn build_update_sql(
        &self,
        table_name: &str,
        columns: &[String],
        primary_keys: &[String],
    ) -> Result<String> {
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

        if let Some(ref memory_db) = self.memory_db {
            let check_sql = "SELECT COUNT(*) as count FROM sqlite_master WHERE type='table' AND name IN ('collection','favorite','submission','watch_later','video_source','video','page')";
            let result = memory_db
                .query_one(Statement::from_string(DatabaseBackend::Sqlite, check_sql))
                .await?;

            if let Some(row) = result {
                let count: i64 = row.try_get("", "count")?;
                let is_valid = count >= 7;
                
                if !is_valid {
                    warn!("内存数据库表验证失败：发现 {}/7 个关键表", count);
                } else {
                    debug!("内存数据库表验证成功：发现 {}/7 个关键表", count);
                }
                
                Ok(is_valid)
            } else {
                warn!("无法验证内存数据库表状态");
                Ok(false)
            }
        } else {
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