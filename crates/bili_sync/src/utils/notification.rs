use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::config::NotificationConfig;

// Server酱API请求结构
#[derive(Serialize)]
struct ServerChanRequest {
    title: String,
    desp: String,
}

// Server酱API响应结构
#[derive(Deserialize)]
struct ServerChanResponse {
    code: i32,
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    data: Option<serde_json::Value>,
}

// 推送通知客户端
pub struct NotificationClient {
    client: Client,
    config: NotificationConfig,
}

// 扫描结果数据结构
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct NewVideoInfo {
    pub title: String,
    pub bvid: String,
    pub upper_name: String,
    pub source_type: String,
    pub source_name: String,
}

#[derive(Debug, Clone)]
pub struct SourceScanResult {
    pub source_type: String,
    pub source_name: String,
    pub new_videos: Vec<NewVideoInfo>,
}

#[derive(Debug, Clone)]
pub struct ScanSummary {
    pub total_sources: usize,
    pub total_new_videos: usize,
    pub scan_duration: Duration,
    pub source_results: Vec<SourceScanResult>,
}

impl NotificationClient {
    pub fn new(config: NotificationConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.notification_timeout))
            .build()
            .expect("Failed to create HTTP client");
            
        Self { client, config }
    }

    pub async fn send_scan_completion(&self, summary: &ScanSummary) -> Result<()> {
        if !self.config.enable_scan_notifications {
            debug!("推送通知已禁用，跳过发送");
            return Ok(());
        }

        if summary.total_new_videos < self.config.notification_min_videos {
            debug!("新增视频数量({})未达到推送阈值({})", 
                   summary.total_new_videos, self.config.notification_min_videos);
            return Ok(());
        }

        let Some(ref key) = self.config.serverchan_key else {
            warn!("未配置Server酱密钥，无法发送推送");
            return Ok(());
        };

        let (title, content) = self.format_scan_message(summary);
        
        for attempt in 1..=self.config.notification_retry_count {
            match self.send_to_serverchan(key, &title, &content).await {
                Ok(_) => {
                    info!("扫描完成推送发送成功");
                    return Ok(());
                }
                Err(e) => {
                    warn!("推送发送失败 (尝试 {}/{}): {}", 
                          attempt, self.config.notification_retry_count, e);
                    
                    if attempt < self.config.notification_retry_count {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }

        error!("推送发送失败，已达到最大重试次数");
        Ok(()) // 不返回错误，避免影响主要功能
    }

    async fn send_to_serverchan(&self, key: &str, title: &str, content: &str) -> Result<()> {
        let url = format!("https://sctapi.ftqq.com/{}.send", key);
        let request = ServerChanRequest {
            title: title.to_string(),
            desp: content.to_string(),
        };

        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        let response_text = response.text().await?;
        let server_response: ServerChanResponse = serde_json::from_str(&response_text)
            .map_err(|e| anyhow!("解析响应失败: {}, 响应内容: {}", e, response_text))?;

        if server_response.code == 0 {
            Ok(())
        } else {
            Err(anyhow!("Server酱返回错误: {}", server_response.message))
        }
    }

    fn format_scan_message(&self, summary: &ScanSummary) -> (String, String) {
        let title = "Bili Sync 扫描完成".to_string();
        
        let mut content = format!(
            "📊 **扫描摘要**\n\n- 扫描视频源: {}个\n- 新增视频: {}个\n- 扫描耗时: {:.1}分钟\n\n",
            summary.total_sources,
            summary.total_new_videos,
            summary.scan_duration.as_secs_f64() / 60.0
        );

        if summary.total_new_videos > 0 {
            content.push_str("📹 **新增视频详情**\n\n");
            
            for source_result in &summary.source_results {
                if !source_result.new_videos.is_empty() {
                    let icon = match source_result.source_type.as_str() {
                        "收藏夹" => "🎬",
                        "合集" => "📁",
                        "UP主投稿" => "🎯",
                        "稍后再看" => "⏰",
                        "番剧" => "📺",
                        _ => "📄",
                    };
                    
                    content.push_str(&format!(
                        "{} **{}** - {} ({}个新视频):\n",
                        icon,
                        source_result.source_type,
                        source_result.source_name,
                        source_result.new_videos.len()
                    ));
                    
                    for video in &source_result.new_videos {
                        content.push_str(&format!(
                            "- [{}](https://www.bilibili.com/video/{}) ({})\n",
                            video.title, video.bvid, video.bvid
                        ));
                    }
                    content.push('\n');
                }
            }
        }

        (title, content)
    }

    pub async fn test_notification(&self) -> Result<()> {
        let Some(ref key) = self.config.serverchan_key else {
            return Err(anyhow!("未配置Server酱密钥"));
        };

        let title = "Bili Sync 测试推送";
        let content = "这是一条测试推送消息，如果您收到此消息，说明推送配置正确。\n\n🎉 推送功能工作正常！";

        self.send_to_serverchan(key, title, &content).await
    }

    pub async fn send_custom_test(&self, message: &str) -> Result<()> {
        let Some(ref key) = self.config.serverchan_key else {
            return Err(anyhow!("未配置Server酱密钥"));
        };

        let title = "Bili Sync 自定义测试推送";
        let content = format!("🧪 **自定义测试消息**\n\n{}", message);

        self.send_to_serverchan(key, title, &content).await
    }
}

// 便捷函数
pub async fn send_scan_notification(summary: ScanSummary) -> Result<()> {
    let config = crate::config::reload_config().notification;
    let client = NotificationClient::new(config);
    client.send_scan_completion(&summary).await
}

#[allow(dead_code)]
pub async fn test_notification() -> Result<()> {
    let config = crate::config::reload_config().notification;
    let client = NotificationClient::new(config);
    client.test_notification().await
}

#[allow(dead_code)]
pub async fn send_custom_test_notification(message: &str) -> Result<()> {
    let config = crate::config::reload_config().notification;
    let client = NotificationClient::new(config);
    client.send_custom_test(message).await
}