use crate::config::CONFIG_DIR;
use chrono::{Local, TimeZone};
use once_cell::sync::Lazy;
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

// 全局启动时间，用于生成日志文件名
pub static STARTUP_TIME: Lazy<String> = Lazy::new(|| Local::now().format("%Y-%m-%d-%H-%M-%S").to_string());

// 日志条目结构
#[derive(Debug)]
struct LogEntry {
    timestamp: String,
    level: String,
    message: String,
    target: String,
}

// 日志文件写入器
pub struct FileLogWriter {
    all_writer: Arc<Mutex<BufWriter<File>>>,
    debug_writer: Arc<Mutex<BufWriter<File>>>,
    info_writer: Arc<Mutex<BufWriter<File>>>,
    warn_writer: Arc<Mutex<BufWriter<File>>>,
    error_writer: Arc<Mutex<BufWriter<File>>>,
    // 日志缓冲区
    log_buffer: Arc<Mutex<VecDeque<LogEntry>>>,
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

        let all_path = log_dir.join(format!("logs-all-{}.csv", startup_time));
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
            log_buffer: Arc::new(Mutex::new(VecDeque::new())),
        })
    }

    fn create_log_file(path: &Path) -> anyhow::Result<BufWriter<File>> {
        let file = OpenOptions::new().create(true).write(true).truncate(true).open(path)?;

        let mut buf_writer = BufWriter::with_capacity(64 * 1024, file); // 64KB缓冲区

        // 写入CSV头，使用UTF-8 BOM以支持Excel正确识别中文
        buf_writer.write_all(&[0xEF, 0xBB, 0xBF])?; // UTF-8 BOM
        writeln!(buf_writer, "时间,级别,消息,来源")?;
        buf_writer.flush()?; // 仅刷新缓冲区，不强制同步到磁盘

        Ok(buf_writer)
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
                                let modified_datetime = Local
                                    .timestamp_opt(modified_timestamp, 0)
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

        // 创建日志条目
        let entry = LogEntry {
            timestamp: timestamp.to_string(),
            level: level.to_string(),
            message: message.to_string(),
            target: target_str.to_string(),
        };

        // 添加到缓冲区，非阻塞操作
        if let Ok(mut buffer) = self.log_buffer.lock() {
            buffer.push_back(entry);
            // 当缓冲区达到1000条时自动刷新到文件
            if buffer.len() >= 1000 {
                // 先取出所有日志，避免死锁
                let entries: Vec<LogEntry> = buffer.drain(..).collect();
                drop(buffer); // 释放锁
                
                // 写入文件
                self.write_entries_to_files(entries);
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

    // 内部方法：直接写入日志条目到文件
    fn write_entries_to_files(&self, entries: Vec<LogEntry>) {
        for entry in entries {
            let escaped_message = Self::escape_csv(&entry.message);
            let escaped_target = Self::escape_csv(&entry.target);
            let log_line = format!(
                "{},{},{},{}\n",
                entry.timestamp, entry.level, escaped_message, escaped_target
            );

            // 写入全部日志文件（不包含debug级别）
            if entry.level.to_lowercase() != "debug" {
                if let Ok(mut writer) = self.all_writer.lock() {
                    let _ = writer.write_all(log_line.as_bytes());
                    let _ = writer.flush(); // 立即刷新
                }
            }

            // 根据级别写入对应文件
            let level_writer = match entry.level.to_lowercase().as_str() {
                "debug" => &self.debug_writer,
                "info" => &self.info_writer,
                "warn" => &self.warn_writer,
                "error" => &self.error_writer,
                _ => continue,
            };

            if let Ok(mut writer) = level_writer.lock() {
                let _ = writer.write_all(log_line.as_bytes());
                let _ = writer.flush(); // 立即刷新
            }
        }
    }

    // 手动刷新日志到文件
    pub fn flush(&self) {
        Self::flush_logs(
            &self.log_buffer,
            &self.all_writer,
            &self.debug_writer,
            &self.info_writer,
            &self.warn_writer,
            &self.error_writer,
        );
    }

    // 刷新日志到文件
    fn flush_logs(
        log_buffer: &Arc<Mutex<VecDeque<LogEntry>>>,
        all_writer: &Arc<Mutex<BufWriter<File>>>,
        debug_writer: &Arc<Mutex<BufWriter<File>>>,
        info_writer: &Arc<Mutex<BufWriter<File>>>,
        warn_writer: &Arc<Mutex<BufWriter<File>>>,
        error_writer: &Arc<Mutex<BufWriter<File>>>,
    ) {
        // 获取待处理的日志
        let entries = {
            if let Ok(mut buffer) = log_buffer.lock() {
                let entries: Vec<LogEntry> = buffer.drain(..).collect();
                entries
            } else {
                Vec::new()
            }
        };

        // 批量写入日志（如果有的话）
        for entry in entries {
            let escaped_message = Self::escape_csv(&entry.message);
            let escaped_target = Self::escape_csv(&entry.target);
            let log_line = format!(
                "{},{},{},{}\n",
                entry.timestamp, entry.level, escaped_message, escaped_target
            );

            // 写入全部日志文件（不包含debug级别）
            if entry.level.to_lowercase() != "debug" {
                if let Ok(mut writer) = all_writer.lock() {
                    let _ = writer.write_all(log_line.as_bytes());
                }
            }

            // 根据级别写入对应文件
            let level_writer = match entry.level.to_lowercase().as_str() {
                "debug" => debug_writer,
                "info" => info_writer,
                "warn" => warn_writer,
                "error" => error_writer,
                _ => continue,
            };

            if let Ok(mut writer) = level_writer.lock() {
                let _ = writer.write_all(log_line.as_bytes());
            }
        }

        // 刷新所有写入器的缓冲区并强制同步到磁盘
        if let Ok(mut writer) = all_writer.lock() {
            let _ = writer.flush();
            let _ = writer.get_mut().sync_all(); // 强制同步到磁盘
        }
        if let Ok(mut writer) = debug_writer.lock() {
            let _ = writer.flush();
            let _ = writer.get_mut().sync_all();
        }
        if let Ok(mut writer) = info_writer.lock() {
            let _ = writer.flush();
            let _ = writer.get_mut().sync_all();
        }
        if let Ok(mut writer) = warn_writer.lock() {
            let _ = writer.flush();
            let _ = writer.get_mut().sync_all();
        }
        if let Ok(mut writer) = error_writer.lock() {
            let _ = writer.flush();
            let _ = writer.get_mut().sync_all();
        }
    }

    // 优雅停止
    pub fn shutdown(&self) {
        // 最后一次刷新所有缓冲的日志
        self.flush();
    }
}

// 全局文件日志写入器
pub static FILE_LOG_WRITER: Lazy<Option<FileLogWriter>> = Lazy::new(|| match FileLogWriter::new() {
    Ok(writer) => Some(writer),
    Err(e) => {
        tracing::error!("文件日志系统初始化失败: {}", e);
        None
    }
});

// 手动刷新所有缓冲的日志到文件
pub fn flush_file_logger() {
    if let Some(ref writer) = *FILE_LOG_WRITER {
        writer.flush();
    }
}

// 在程序退出时调用，确保所有日志都被写入
pub fn shutdown_file_logger() {
    if let Some(ref writer) = *FILE_LOG_WRITER {
        writer.shutdown();
    }
}

