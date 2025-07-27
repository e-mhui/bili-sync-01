use anyhow::{Context, Result};
use sea_orm::{DatabaseConnection, Statement, ConnectionTrait, DatabaseBackend, TransactionTrait};
use tracing::{debug, info, warn};
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

        // 创建内存数据库连接
        let memory_db_url = "sqlite::memory:";
        let memory_db = sea_orm::Database::connect(memory_db_url)
            .await
            .context("连接内存数据库失败")?;

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
            "PRAGMA locking_mode = EXCLUSIVE", // 独占锁定模式
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

    /// 将内存数据库的变更同步到主数据库
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

        info!("开始同步 {} 个表的变更到主数据库", tables_to_sync.len());

        for table_name in tables_to_sync {
            // 忽略可能不存在或为空的表
            if let Err(e) = self.sync_table_changes(table_name, memory_db, &txn).await {
                debug!("表 {} 同步失败（可能无变更）: {}", table_name, e);
            } else {
                debug!("{}表变更同步完成", table_name);
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
        // 从内存数据库读取表数据
        let select_sql = format!("SELECT * FROM {}", table_name);
        let rows = memory_db
            .query_all(Statement::from_string(DatabaseBackend::Sqlite, &select_sql))
            .await?;

        if rows.is_empty() {
            return Ok(());
        }

        // 获取表的列名
        let columns = self.get_table_columns(table_name).await?;
        let column_names = columns.join(", ");
        let placeholders: Vec<String> = columns.iter().map(|_| "?".to_string()).collect();
        let placeholders_str = placeholders.join(", ");

        // 使用INSERT OR REPLACE进行批量更新
        let upsert_sql = format!(
            "INSERT OR REPLACE INTO {} ({}) VALUES ({})",
            table_name, column_names, placeholders_str
        );

        // 批量插入/更新数据
        for row in rows {
            let mut values = Vec::new();
            for column in &columns {
                let value = self.extract_value_from_row(&row, column)?;
                values.push(value);
            }

            txn.execute(Statement::from_sql_and_values(
                DatabaseBackend::Sqlite,
                &upsert_sql,
                values,
            ))
            .await?;
        }

        Ok(())
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

}

impl Drop for MemoryDbOptimizer {
    fn drop(&mut self) {
        if self.is_memory_mode {
            warn!("内存数据库优化器被丢弃时仍在内存模式中，可能有未保存的变更");
        }
    }
}