use anyhow::Result;
use sea_orm::DatabaseConnection;
use std::sync::Arc;
use tracing::{info, warn, error};

use crate::utils::memory_db::MemoryDbOptimizer;
use crate::adapter::VideoSourceEnum;

/// 扫描管理器，负责优化扫描性能，特别是在NAS环境下
pub struct ScanManager {
    /// 内存数据库优化器
    memory_optimizer: Option<MemoryDbOptimizer>,
    /// 是否启用了内存优化模式
    memory_mode_enabled: bool,
}

impl ScanManager {
    /// 创建新的扫描管理器
    pub fn new() -> Self {
        Self {
            memory_optimizer: None,
            memory_mode_enabled: false,
        }
    }

    /// 准备扫描，根据环境和数据量决定是否使用内存优化
    pub async fn prepare_scan(&mut self, db: Arc<DatabaseConnection>) -> Result<()> {
        info!("准备开始扫描，评估是否需要内存优化");

        // 创建内存数据库优化器
        let mut optimizer = MemoryDbOptimizer::new(db.clone());

        // 检查是否应该使用内存模式
        if optimizer.should_use_memory_mode().await? {
            info!("检测到大数据量或NAS环境，启用内存数据库优化");
            
            // 启动内存模式
            match optimizer.start_memory_mode().await {
                Ok(_) => {
                    self.memory_optimizer = Some(optimizer);
                    self.memory_mode_enabled = true;
                    info!("内存数据库优化模式启动成功");
                }
                Err(e) => {
                    warn!("启动内存数据库优化失败，使用常规模式: {}", e);
                    self.memory_optimizer = Some(MemoryDbOptimizer::new(db));
                    self.memory_mode_enabled = false;
                }
            }
        } else {
            info!("数据量较小或环境适合，使用常规扫描模式");
            self.memory_optimizer = Some(MemoryDbOptimizer::new(db));
            self.memory_mode_enabled = false;
        }

        Ok(())
    }

    /// 获取用于扫描的数据库连接
    pub fn get_scan_connection(&self) -> Arc<DatabaseConnection> {
        if let Some(ref optimizer) = self.memory_optimizer {
            optimizer.get_active_connection()
        } else {
            // 如果没有optimizer，说明是fallback模式，需要从某处获取原始连接
            // 这里我们需要返回一个默认连接，但实际上需要从外部传入
            panic!("扫描管理器未初始化，无法获取数据库连接");
        }
    }

    /// 完成扫描，将内存数据库的变更写回主数据库
    pub async fn finalize_scan(&mut self) -> Result<()> {
        info!("扫描完成，开始处理数据同步");

        if let Some(mut optimizer) = self.memory_optimizer.take() {
            if self.memory_mode_enabled {
                match optimizer.stop_memory_mode().await {
                    Ok(_) => {
                        info!("内存数据库变更已成功写回主数据库");
                    }
                    Err(e) => {
                        error!("写回内存数据库变更失败: {}", e);
                        return Err(e);
                    }
                }
            }
        }

        self.memory_mode_enabled = false;
        info!("扫描清理完成");
        Ok(())
    }

    /// 检查是否在内存优化模式中
    pub fn is_memory_mode(&self) -> bool {
        self.memory_mode_enabled
    }

    /// 获取性能统计信息
    pub fn get_performance_stats(&self) -> ScanPerformanceStats {
        ScanPerformanceStats {
            memory_mode_enabled: self.memory_mode_enabled,
            // 可以在未来添加更多统计信息
        }
    }
}

impl Default for ScanManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScanManager {
    fn drop(&mut self) {
        if self.memory_mode_enabled {
            warn!("扫描管理器被丢弃时仍在内存模式中，可能有未保存的变更");
        }
    }
}

/// 扫描性能统计信息
#[derive(Debug, Clone)]
pub struct ScanPerformanceStats {
    /// 是否启用了内存模式
    pub memory_mode_enabled: bool,
}

/// 扫描上下文，包含扫描期间的所有必要信息
pub struct ScanContext {
    /// 扫描管理器
    pub manager: ScanManager,
    /// 当前扫描的视频源列表
    pub video_sources: Vec<VideoSourceEnum>,
    /// 扫描开始时间
    pub start_time: std::time::Instant,
}

impl ScanContext {
    /// 创建新的扫描上下文
    pub async fn new(
        db: Arc<DatabaseConnection>,
        video_sources: Vec<VideoSourceEnum>,
    ) -> Result<Self> {
        let mut manager = ScanManager::new();
        manager.prepare_scan(db).await?;

        Ok(Self {
            manager,
            video_sources,
            start_time: std::time::Instant::now(),
        })
    }

    /// 创建fallback扫描上下文（常规模式）
    pub async fn new_fallback(db: Arc<DatabaseConnection>) -> Result<Self> {
        let mut manager = ScanManager::new();
        // 设置fallback模式的optimizer
        manager.memory_optimizer = Some(crate::utils::memory_db::MemoryDbOptimizer::new(db));
        manager.memory_mode_enabled = false;

        Ok(Self {
            manager,
            video_sources: Vec::new(), // 空的视频源列表
            start_time: std::time::Instant::now(),
        })
    }

    /// 获取扫描数据库连接
    pub fn get_connection(&self) -> Arc<DatabaseConnection> {
        self.manager.get_scan_connection()
    }

    /// 完成扫描上下文
    pub async fn finalize(mut self) -> Result<ScanResult> {
        let duration = self.start_time.elapsed();
        let performance_stats = self.manager.get_performance_stats();
        
        // 完成扫描并同步数据
        self.manager.finalize_scan().await?;

        Ok(ScanResult {
            duration,
            performance_stats,
            total_sources: self.video_sources.len(),
        })
    }
}

/// 扫描结果
#[derive(Debug)]
pub struct ScanResult {
    /// 扫描总耗时
    pub duration: std::time::Duration,
    /// 性能统计信息
    pub performance_stats: ScanPerformanceStats,
    /// 扫描的视频源总数
    pub total_sources: usize,
}

impl ScanResult {
    /// 记录扫描结果日志
    pub fn log_result(&self) {
        let mode = if self.performance_stats.memory_mode_enabled {
            "内存优化模式"
        } else {
            "常规模式"
        };

        info!(
            "扫描完成 - 模式: {}, 耗时: {:.2}秒, 视频源数量: {}",
            mode,
            self.duration.as_secs_f64(),
            self.total_sources
        );

        if self.performance_stats.memory_mode_enabled {
            info!("内存优化模式有效减少了数据库I/O开销，提升了扫描性能");
        }
    }
}