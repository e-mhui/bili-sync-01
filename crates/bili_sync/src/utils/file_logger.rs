use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;
use chrono::{Local, TimeZone};
use crate::config::CONFIG_DIR;

// 全局启动时间，用于生成日志文件名
pub static STARTUP_TIME: Lazy<String> = Lazy::new(|| {
    Local::now().format("%Y-%m-%d-%H-%M-%S").to_string()
});

// 日志文件写入器
pub struct FileLogWriter {
    all_writer: Arc<Mutex<File>>,
    debug_writer: Arc<Mutex<File>>,
    info_writer: Arc<Mutex<File>>,
    warn_writer: Arc<Mutex<File>>,
    error_writer: Arc<Mutex<File>>,
}

impl FileLogWriter {
    pub fn new() -> anyhow::Result<Self> {
        // 创建日志目录
        let log_dir = CONFIG_DIR.join("logs");
        fs::create_dir_all(&log_dir)?;
        
        // 清理超过30天的旧日志
        Self::cleanup_old_logs(&log_dir)?;
        
        // 创建各级别的日志文件
        let startup_time = &*STARTUP_TIME;
        
        let all_path = log_dir.join(format!("logs-全部-{}.csv", startup_time));
        let debug_path = log_dir.join(format!("logs-debug-{}.csv", startup_time));
        let info_path = log_dir.join(format!("logs-info-{}.csv", startup_time));
        let warn_path = log_dir.join(format!("logs-warn-{}.csv", startup_time));
        let error_path = log_dir.join(format!("logs-error-{}.csv", startup_time));
        
        // 创建文件并写入CSV头
        let all_writer = Self::create_log_file(&all_path)?;
        let debug_writer = Self::create_log_file(&debug_path)?;
        let info_writer = Self::create_log_file(&info_path)?;
        let warn_writer = Self::create_log_file(&warn_path)?;
        let error_writer = Self::create_log_file(&error_path)?;
        
        Ok(Self {
            all_writer: Arc::new(Mutex::new(all_writer)),
            debug_writer: Arc::new(Mutex::new(debug_writer)),
            info_writer: Arc::new(Mutex::new(info_writer)),
            warn_writer: Arc::new(Mutex::new(warn_writer)),
            error_writer: Arc::new(Mutex::new(error_writer)),
        })
    }
    
    fn create_log_file(path: &Path) -> anyhow::Result<File> {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        
        // 写入CSV头，使用UTF-8 BOM以支持Excel正确识别中文
        file.write_all(&[0xEF, 0xBB, 0xBF])?; // UTF-8 BOM
        writeln!(file, "时间,级别,消息,来源")?;
        file.sync_all()?; // 确保立即写入磁盘
        
        Ok(file)
    }
    
    fn cleanup_old_logs(log_dir: &Path) -> anyhow::Result<()> {
        let thirty_days_ago = Local::now() - chrono::Duration::days(30);
        
        if let Ok(entries) = fs::read_dir(log_dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        if let Ok(modified) = metadata.modified() {
                            if let Ok(modified_datetime) = modified.duration_since(std::time::UNIX_EPOCH) {
                                let modified_timestamp = modified_datetime.as_secs() as i64;
                                // 使用 timestamp_opt 方法来创建本地时间
                                let modified_datetime = Local.timestamp_opt(modified_timestamp, 0)
                                    .single()
                                    .unwrap_or_else(|| Local::now());
                                
                                if modified_datetime < thirty_days_ago {
                                    // 删除超过30天的日志文件
                                    let _ = fs::remove_file(entry.path());
                                }
                            }
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    pub fn write_log(&self, timestamp: &str, level: &str, message: &str, target: Option<&str>) {
        let target_str = target.unwrap_or("");
        
        // 转义CSV特殊字符
        let escaped_message = Self::escape_csv(message);
        let escaped_target = Self::escape_csv(target_str);
        
        let log_line = format!("{},{},{},{}\n", timestamp, level, escaped_message, escaped_target);
        
        // 写入全部日志文件（不包含debug级别）
        if level.to_lowercase() != "debug" {
            if let Ok(mut writer) = self.all_writer.lock() {
                if let Err(e) = writer.write_all(log_line.as_bytes()) {
                    eprintln!("写入全部日志文件失败: {}", e);
                }
                if let Err(e) = writer.sync_all() {
                    eprintln!("同步全部日志文件失败: {}", e);
                }
            }
        }
        
        // 根据级别写入对应文件
        let level_writer = match level.to_lowercase().as_str() {
            "debug" => &self.debug_writer,
            "info" => &self.info_writer,
            "warn" => &self.warn_writer,
            "error" => &self.error_writer,
            _ => return,
        };
        
        if let Ok(mut writer) = level_writer.lock() {
            if let Err(e) = writer.write_all(log_line.as_bytes()) {
                eprintln!("写入{}日志文件失败: {}", level, e);
            }
            if let Err(e) = writer.sync_all() {
                eprintln!("同步{}日志文件失败: {}", level, e);
            }
        }
    }
    
    fn escape_csv(field: &str) -> String {
        if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
            format!("\"{}\"", field.replace('"', "\"\""))
        } else {
            field.to_string()
        }
    }
}

// 全局文件日志写入器
pub static FILE_LOG_WRITER: Lazy<Option<FileLogWriter>> = Lazy::new(|| {
    match FileLogWriter::new() {
        Ok(writer) => {
            tracing::info!("文件日志系统初始化成功，日志目录: {}/logs", CONFIG_DIR.display());
            Some(writer)
        }
        Err(e) => {
            tracing::error!("文件日志系统初始化失败: {}", e);
            None
        }
    }
});

// 获取当前会话的日志文件列表
pub fn get_current_session_logs() -> Vec<PathBuf> {
    let log_dir = CONFIG_DIR.join("logs");
    let startup_time = &*STARTUP_TIME;
    
    let patterns = vec![
        format!("logs-全部-{}.csv", startup_time),
        format!("logs-debug-{}.csv", startup_time),
        format!("logs-info-{}.csv", startup_time),
        format!("logs-warn-{}.csv", startup_time),
        format!("logs-error-{}.csv", startup_time),
    ];
    
    patterns.into_iter()
        .map(|name| log_dir.join(name))
        .filter(|path| path.exists())
        .collect()
}