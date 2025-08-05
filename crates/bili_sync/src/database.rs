use anyhow::Result;
use bili_sync_migration::{Migrator, MigratorTrait};
use sea_orm::{ConnectOptions, Database, DatabaseConnection};
use tracing::debug;

use crate::config::CONFIG_DIR;

fn database_url() -> String {
    // 确保配置目录存在
    if !CONFIG_DIR.exists() {
        std::fs::create_dir_all(&*CONFIG_DIR).expect("创建配置目录失败");
    }
    format!("sqlite://{}?mode=rwc", CONFIG_DIR.join("data.sqlite").to_string_lossy())
}

async fn database_connection() -> Result<DatabaseConnection> {
    let mut option = ConnectOptions::new(database_url());
    option
        .max_connections(20) // 降低最大连接数，避免过多连接
        .min_connections(2) // 最小连接数
        .acquire_timeout(std::time::Duration::from_secs(30)) // 缩短超时时间
        .idle_timeout(std::time::Duration::from_secs(300)) // 空闲连接超时5分钟
        .max_lifetime(std::time::Duration::from_secs(3600)) // 连接最大生命周期1小时
        .sqlx_logging(false); // 禁用sqlx查询日志，避免过多的日志输出

    let connection = Database::connect(option).await?;

    // 确保 WAL 模式已启用并应用额外的性能优化
    use sea_orm::ConnectionTrait;
    connection.execute_unprepared("PRAGMA journal_mode = WAL;").await?;
    connection.execute_unprepared("PRAGMA synchronous = NORMAL;").await?;
    connection.execute_unprepared("PRAGMA cache_size = 10000;").await?; // 增加缓存大小
    connection.execute_unprepared("PRAGMA temp_store = memory;").await?;
    connection.execute_unprepared("PRAGMA mmap_size = 268435456;").await?; // 256MB
    connection
        .execute_unprepared("PRAGMA wal_autocheckpoint = 1000;")
        .await?;
    connection.execute_unprepared("PRAGMA busy_timeout = 30000;").await?; // 30秒忙等超时
    connection.execute_unprepared("PRAGMA optimize;").await?; // 启用查询优化器

    debug!("SQLite WAL 模式已启用，性能优化参数已应用");

    Ok(connection)
}

async fn migrate_database() -> Result<()> {
    // 检查数据库文件是否存在，不存在则会在连接时自动创建
    let db_path = CONFIG_DIR.join("data.sqlite");
    if !db_path.exists() {
        debug!("数据库文件不存在，将创建新的数据库");
    } else {
        debug!("检测到现有数据库文件，将在必要时应用迁移");
    }

    // 注意此处使用内部构造的 DatabaseConnection，而不是通过 database_connection() 获取
    // 这是因为使用多个连接的 Connection 会导致奇怪的迁移顺序问题，而使用默认的连接选项不会
    let connection = Database::connect(database_url()).await?;

    // 确保所有迁移都应用
    Ok(Migrator::up(&connection, None).await?)
}

/// 进行数据库迁移并获取数据库连接，供外部使用
pub async fn setup_database() -> DatabaseConnection {
    migrate_database().await.expect("数据库迁移失败");
    let connection = database_connection().await.expect("获取数据库连接失败");

    // 执行番剧缓存相关的数据库迁移
    if let Err(e) = crate::utils::bangumi_cache::ensure_cache_columns(&connection).await {
        tracing::warn!("番剧缓存数据库迁移失败: {}", e);
    }

    connection
}
