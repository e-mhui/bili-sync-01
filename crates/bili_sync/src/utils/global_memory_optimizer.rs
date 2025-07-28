use anyhow::Result;
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use once_cell::sync::Lazy;

use crate::utils::memory_db::MemoryDbOptimizer;

/// 全局内存数据库优化器实例
pub static GLOBAL_MEMORY_OPTIMIZER: Lazy<Arc<RwLock<GlobalMemoryOptimizer>>> = 
    Lazy::new(|| Arc::new(RwLock::new(GlobalMemoryOptimizer::new())));

/// 全局内存数据库优化器
/// 负责在程序启动时评估并启动内存优化，确保所有数据库操作都能享受优化
pub struct GlobalMemoryOptimizer {
    /// 内存数据库优化器实例
    optimizer: Option<MemoryDbOptimizer>,
    /// 是否启用了内存优化模式
    is_memory_mode_enabled: bool,
    /// 是否已经初始化
    is_initialized: bool,
}

impl GlobalMemoryOptimizer {
    /// 创建新的全局内存优化器
    pub fn new() -> Self {
        Self {
            optimizer: None,
            is_memory_mode_enabled: false,
            is_initialized: false,
        }
    }

    /// 初始化全局内存优化器
    /// 这应该在程序启动时就调用，确保所有后续的数据库操作都能享受优化
    pub async fn initialize(&mut self, db: Arc<DatabaseConnection>) -> Result<()> {
        if self.is_initialized {
            info!("全局内存优化器已经初始化，跳过重复初始化");
            return Ok(());
        }

        info!("开始初始化全局内存数据库优化器");

        // 创建内存数据库优化器
        let mut optimizer = MemoryDbOptimizer::new(db.clone());

        // 检查是否应该使用内存模式
        if optimizer.should_use_memory_mode().await? {
            info!("检测到需要内存优化，在程序启动阶段启用内存数据库优化");
            
            // 启动内存模式
            match optimizer.start_memory_mode().await {
                Ok(_) => {
                    self.optimizer = Some(optimizer);
                    self.is_memory_mode_enabled = true;
                    info!("全局内存数据库优化模式启动成功，所有数据库操作将使用内存优化");
                }
                Err(e) => {
                    warn!("启动全局内存数据库优化失败，使用常规模式: {}", e);
                    self.optimizer = Some(MemoryDbOptimizer::new(db));
                    self.is_memory_mode_enabled = false;
                }
            }
        } else {
            info!("根据配置或环境评估，不需要内存优化，使用常规模式");
            self.optimizer = Some(MemoryDbOptimizer::new(db));
            self.is_memory_mode_enabled = false;
        }

        self.is_initialized = true;
        info!("全局内存优化器初始化完成");
        Ok(())
    }

    /// 获取当前的数据库连接
    /// 如果启用了内存优化，返回内存数据库连接；否则返回主数据库连接
    pub fn get_connection(&self) -> Option<Arc<DatabaseConnection>> {
        if let Some(ref optimizer) = self.optimizer {
            Some(optimizer.get_active_connection())
        } else {
            None
        }
    }

    /// 检查是否在内存优化模式中
    pub fn is_memory_mode_enabled(&self) -> bool {
        self.is_memory_mode_enabled
    }

    /// 动态重配置内存优化器（检测配置变化并重新配置）
    /// 这允许在运行时根据配置变化切换内存优化模式
    pub async fn reconfigure_if_needed(&mut self, db: Arc<DatabaseConnection>) -> Result<bool> {
        if !self.is_initialized {
            // 如果未初始化，直接初始化
            self.initialize(db).await?;
            return Ok(true);
        }

        // 获取最新配置
        let config = crate::config::reload_config();
        let should_use_memory = config.enable_memory_optimization;

        // 检测配置变化或内存数据库是否需要重建
        let needs_reconfigure = should_use_memory != self.is_memory_mode_enabled || 
                               (self.is_memory_mode_enabled && self.needs_memory_rebuild().await);

        if needs_reconfigure {
            if should_use_memory != self.is_memory_mode_enabled {
                info!("检测到内存优化配置变化：{} -> {}", 
                    self.is_memory_mode_enabled, should_use_memory);
            } else {
                info!("检测到内存数据库需要重建");
            }

            if should_use_memory {
                // 需要切换到内存模式或重建内存数据库
                self.switch_to_memory_mode(db).await?;
            } else {
                // 需要切换到常规模式
                self.switch_to_normal_mode().await?;
            }
            return Ok(true); // 发生了切换
        }
        
        Ok(false) // 无需切换
    }

    /// 检查内存数据库是否需要重建
    async fn needs_memory_rebuild(&self) -> bool {
        if let Some(ref optimizer) = self.optimizer {
            // 使用专门的验证方法
            match optimizer.verify_memory_db_tables().await {
                Ok(is_valid) => !is_valid, // 如果无效则需要重建
                Err(e) => {
                    warn!("内存数据库验证失败: {}，需要重建", e);
                    true
                }
            }
        } else {
            false
        }
    }

    /// 切换到内存优化模式
    async fn switch_to_memory_mode(&mut self, db: Arc<DatabaseConnection>) -> Result<()> {
        info!("开始切换到内存优化模式");

        // 如果当前有优化器实例，先清理
        if let Some(mut current_optimizer) = self.optimizer.take() {
            if self.is_memory_mode_enabled {
                // 先停止当前的内存模式
                current_optimizer.stop_memory_mode().await?;
            }
        }

        // 创建新的内存数据库优化器并启动内存模式
        let mut new_optimizer = MemoryDbOptimizer::new(db.clone());
        match new_optimizer.start_memory_mode().await {
            Ok(_) => {
                self.optimizer = Some(new_optimizer);
                self.is_memory_mode_enabled = true;
                info!("成功切换到内存优化模式");
                Ok(())
            }
            Err(e) => {
                warn!("切换到内存优化模式失败，回退到常规模式: {}", e);
                self.optimizer = Some(MemoryDbOptimizer::new(db));
                self.is_memory_mode_enabled = false;
                Err(e)
            }
        }
    }

    /// 切换到常规模式
    async fn switch_to_normal_mode(&mut self) -> Result<()> {
        info!("开始切换到常规模式");

        if let Some(mut current_optimizer) = self.optimizer.take() {
            if self.is_memory_mode_enabled {
                // 将内存数据库的变更写回主数据库
                current_optimizer.stop_memory_mode().await?;
                info!("内存数据库变更已写回主数据库");
            }
            
            // 创建常规模式的优化器
            let main_db = current_optimizer.get_active_connection();
            self.optimizer = Some(MemoryDbOptimizer::new(main_db));
        }

        self.is_memory_mode_enabled = false;
        info!("成功切换到常规模式");
        Ok(())
    }

    /// 完成扫描，将内存数据库的变更写回主数据库
    /// 这通常在程序关闭或扫描完成时调用
    pub async fn finalize(&mut self) -> Result<()> {
        if !self.is_initialized {
            return Ok(());
        }

        info!("开始完成全局内存优化器，同步数据变更");

        if let Some(mut optimizer) = self.optimizer.take() {
            if self.is_memory_mode_enabled {
                match optimizer.stop_memory_mode().await {
                    Ok(_) => {
                        info!("全局内存数据库变更已成功写回主数据库");
                    }
                    Err(e) => {
                        error!("写回全局内存数据库变更失败: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        self.is_memory_mode_enabled = false;
        self.is_initialized = false;
        info!("全局内存优化器已完成并清理");
        Ok(())
    }

    /// 同步变更但保持内存模式
    pub async fn sync_changes_without_stopping(&mut self) -> Result<()> {
        if !self.is_initialized || !self.is_memory_mode_enabled {
            return Ok(());
        }
        
        info!("开始同步全局内存优化器的变更（保持内存模式）");
        
        if let Some(ref optimizer) = self.optimizer {
            // 调用底层的同步方法
            optimizer.sync_to_main_db_keep_memory().await?;
            info!("全局内存优化器数据同步完成");
        }
        
        Ok(())
    }

}

impl Default for GlobalMemoryOptimizer {
    fn default() -> Self {
        Self::new()
    }
}


/// 便捷函数：初始化全局内存优化器
pub async fn initialize_global_memory_optimizer(db: Arc<DatabaseConnection>) -> Result<()> {
    let mut optimizer = GLOBAL_MEMORY_OPTIMIZER.write().await;
    optimizer.initialize(db).await
}

/// 便捷函数：获取全局优化的数据库连接
pub async fn get_optimized_connection() -> Option<Arc<DatabaseConnection>> {
    let optimizer = GLOBAL_MEMORY_OPTIMIZER.read().await;
    optimizer.get_connection()
}

/// 便捷函数：检查是否启用了内存优化
pub async fn is_memory_optimization_enabled() -> bool {
    let optimizer = GLOBAL_MEMORY_OPTIMIZER.read().await;
    optimizer.is_memory_mode_enabled()
}

/// 便捷函数：重配置全局内存优化器（检测配置变化）
pub async fn reconfigure_global_memory_optimizer(db: Arc<DatabaseConnection>) -> Result<bool> {
    let mut optimizer = GLOBAL_MEMORY_OPTIMIZER.write().await;
    optimizer.reconfigure_if_needed(db).await
}

/// 便捷函数：完成全局内存优化器
pub async fn finalize_global_memory_optimizer() -> Result<()> {
    let mut optimizer = GLOBAL_MEMORY_OPTIMIZER.write().await;
    optimizer.finalize().await
}

/// 便捷函数：手动触发同步到主数据库（不停止内存模式）
pub async fn sync_to_main_db() -> Result<()> {
    let mut optimizer = GLOBAL_MEMORY_OPTIMIZER.write().await;
    optimizer.sync_changes_without_stopping().await
}

