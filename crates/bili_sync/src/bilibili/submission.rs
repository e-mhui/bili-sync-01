use anyhow::{anyhow, Context, Result};
use arc_swap::access::Access;
use async_stream::try_stream;
use futures::Stream;
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::bilibili::credential::encoded_query;
use crate::bilibili::favorite_list::Upper;
use crate::bilibili::{BiliClient, Validate, VideoInfo, MIXIN_KEY};
use crate::config::SubmissionRiskControlConfig;
pub struct Submission<'a> {
    client: &'a BiliClient,
    upper_id: String,
    upper_name: Option<String>,
}

impl<'a> Submission<'a> {
    pub fn new(client: &'a BiliClient, upper_id: String) -> Self {
        Self { 
            client, 
            upper_id, 
            upper_name: None,
        }
    }

    pub fn with_name(client: &'a BiliClient, upper_id: String, upper_name: String) -> Self {
        Self { 
            client, 
            upper_id, 
            upper_name: Some(upper_name),
        }
    }

    fn display_name(&self) -> &str {
        self.upper_name.as_deref().unwrap_or(&self.upper_id)
    }

    pub async fn get_info(&self) -> Result<Upper<String>> {
        let mut res = self
            .client
            .request(Method::GET, "https://api.bilibili.com/x/web-interface/card")
            .await
            .query(&[("mid", self.upper_id.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?
            .validate()?;
        Ok(serde_json::from_value(res["data"]["card"].take())?)
    }

    async fn get_videos(&self, page: i32) -> Result<Value> {
        self.client
            .request(Method::GET, "https://api.bilibili.com/x/space/wbi/arc/search")
            .await
            .query(&encoded_query(
                vec![
                    ("mid", self.upper_id.as_str()),
                    ("order", "pubdate"),
                    ("order_avoided", "true"),
                    ("platform", "web"),
                    ("web_location", "1550101"),
                    ("pn", page.to_string().as_str()),
                    ("ps", "30"),
                ],
                MIXIN_KEY.load().as_deref(),
            ))
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?
            .validate()
    }

    pub fn into_video_stream(self) -> impl Stream<Item = Result<VideoInfo>> + 'a {
        try_stream! {
            let mut page: usize = 1;
            let mut request_count = 0;
            let mut total_video_count: Option<i64> = None;
            let mut is_large_submission = false;

            let current_config = crate::config::reload_config();
            let config = &current_config.submission_risk_control;

            loop {
                // 在第一次请求后实施延迟策略
                if request_count > 0 {
                    let delay = Self::calculate_adaptive_delay(
                        request_count,
                        is_large_submission,
                        config,
                    );

                    if delay.as_millis() > 0 {
                        debug!(
                            "UP主 {} 第 {} 次请求前延迟 {}ms（大量视频UP主: {}）",
                            self.display_name(),
                            request_count + 1,
                            delay.as_millis(),
                            is_large_submission
                        );
                        tokio::time::sleep(delay).await;
                    }

                    // 分批处理：每处理batch_size页后额外延迟
                    if is_large_submission && config.enable_batch_processing && page > 1 && (page - 1) % config.batch_size == 0 {
                        let batch_delay = Duration::from_secs(config.batch_delay_seconds);
                        info!(
                            "UP主 {} 分批处理：完成第 {} 批（{}页），延迟 {}秒",
                            self.display_name(),
                            (page - 1) / config.batch_size,
                            config.batch_size,
                            config.batch_delay_seconds
                        );
                        tokio::time::sleep(batch_delay).await;
                    }
                }

                let mut videos = self
                    .get_videos(page as i32)
                    .await
                    .map_err(|e| {
                        // 使用现有的错误分类系统检测风控
                        let classified_error = crate::error::ErrorClassifier::classify_error(&e);
                        if classified_error.error_type == crate::error::ErrorType::RiskControl {
                            warn!(
                                "UP主 {} 第 {} 页获取触发风控: {}",
                                self.display_name(), page, classified_error.message
                            );
                            return crate::error::DownloadAbortError().into();
                        }

                        // 其他错误继续抛出
                        e.context(format!("failed to get videos of upper {} page {}", self.display_name(), page))
                    })?;

                request_count += 1;

                // 在第一次请求时检测是否为大量视频UP主并确定处理策略
                if page == 1 {
                    if let Some(count) = videos["data"]["page"]["count"].as_i64() {
                        total_video_count = Some(count);
                        is_large_submission = count > config.large_submission_threshold as i64;

                        if is_large_submission {
                            if config.enable_batch_processing {
                                info!(
                                    "检测到大量视频UP主 {} ({}个视频)，启用分批处理策略（批次大小：{}页，间隔：{}秒）",
                                    self.display_name(), count, config.batch_size, config.batch_delay_seconds
                                );
                            } else {
                                info!(
                                    "检测到大量视频UP主 {} ({}个视频)，启用保守请求策略",
                                    self.display_name(), count
                                );
                            }
                        } else {
                            debug!(
                                "UP主 {} 有 {} 个视频，使用标准请求策略",
                                self.display_name(), count
                            );
                        }
                    }
                }

                let vlist = &mut videos["data"]["list"]["vlist"];
                if vlist.as_array().is_none_or(|v| v.is_empty()) {
                    if page == 1 {
                        Err(anyhow!("no medias found in upper {} page {}", self.display_name(), page))?;
                    } else {
                        // 非第一页没有视频表示已经到达末尾
                        break;
                    }
                }

                let videos_info: Vec<VideoInfo> = serde_json::from_value(vlist.take())
                    .with_context(|| format!("failed to parse videos of upper {} page {}", self.display_name(), page))?;

                for video_info in videos_info {
                    yield video_info;
                }

                let count = &videos["data"]["page"]["count"];
                if let Some(v) = count.as_i64() {
                    if v > (page * 30) as i64 {
                        page += 1;
                        continue;
                    }
                } else {
                    Err(anyhow!("count is not an i64"))?;
                }
                break;
            }

            // 扫描完成后记录统计信息
            if let Some(total_count) = total_video_count {
                if is_large_submission && config.enable_batch_processing {
                    let total_batches = (page - 1).div_ceil(config.batch_size);
                    info!(
                        "UP主 {} 分批扫描完成：共 {} 个视频，分 {} 批处理（每批{}页），发起 {} 次API请求",
                        self.display_name(), total_count, total_batches, config.batch_size, request_count
                    );
                } else {
                    info!(
                        "UP主 {} 扫描完成：共 {} 个视频，发起 {} 次API请求",
                        self.display_name(), total_count, request_count
                    );
                }
            }
        }
    }

    /// 计算自适应延迟时间
    fn calculate_adaptive_delay(
        request_count: usize,
        is_large_submission: bool,
        config: &SubmissionRiskControlConfig,
    ) -> Duration {
        let base_delay = Duration::from_millis(config.base_request_delay);

        // 大量视频UP主使用额外倍数
        let large_submission_multiplier = if is_large_submission {
            config.large_submission_delay_multiplier
        } else {
            1
        };

        // 渐进式延迟：请求次数越多，延迟越长
        let progressive_multiplier = if config.enable_progressive_delay {
            (request_count as u64 / 5).min(config.max_delay_multiplier - 1) + 1
        } else {
            1
        };

        let total_multiplier = large_submission_multiplier * progressive_multiplier;
        base_delay * total_multiplier as u32
    }
}
