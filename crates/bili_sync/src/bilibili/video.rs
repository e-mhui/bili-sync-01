use anyhow::{anyhow, bail, ensure, Result};
use futures::stream::FuturesUnordered;
use futures::TryStreamExt;
use prost::Message;
use reqwest::Method;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::bilibili::analyzer::PageAnalyzer;
use crate::bilibili::client::BiliClient;
use crate::bilibili::credential::encoded_query;
use crate::bilibili::danmaku::{DanmakuElem, DanmakuWriter, DmSegMobileReply};
use crate::bilibili::subtitle::{SubTitle, SubTitleBody, SubTitleInfo, SubTitlesInfo};
use crate::bilibili::{Validate, VideoInfo, MIXIN_KEY};

static MASK_CODE: u64 = 2251799813685247;
static XOR_CODE: u64 = 23442827791579;
static BASE: u64 = 58;
static DATA: &[char] = &[
    'F', 'c', 'w', 'A', 'P', 'N', 'K', 'T', 'M', 'u', 'g', '3', 'G', 'V', '5', 'L', 'j', '7', 'E', 'J', 'n', 'H', 'p',
    'W', 's', 'x', '4', 't', 'b', '8', 'h', 'a', 'Y', 'e', 'v', 'i', 'q', 'B', 'z', '6', 'r', 'k', 'C', 'y', '1', '2',
    'm', 'U', 'S', 'D', 'Q', 'X', '9', 'R', 'd', 'o', 'Z', 'f',
];

pub struct Video<'a> {
    client: &'a BiliClient,
    pub aid: String,
    pub bvid: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct Tag {
    pub tag_name: String,
}

impl serde::Serialize for Tag {
    fn serialize<S>(&self, serializer: S) -> core::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.tag_name)
    }
}
#[derive(Debug, serde::Deserialize, Default)]
pub struct PageInfo {
    pub cid: i64,
    pub page: i32,
    #[serde(rename = "part")]
    pub name: String,
    pub duration: u32,
    pub first_frame: Option<String>,
    pub dimension: Option<Dimension>,
}

#[derive(Debug, serde::Deserialize, Default)]
pub struct Dimension {
    pub width: u32,
    pub height: u32,
    pub rotate: u32,
}

impl<'a> Video<'a> {
    pub fn new(client: &'a BiliClient, bvid: String) -> Self {
        let aid = bvid_to_aid(&bvid).to_string();
        Self { client, aid, bvid }
    }

    /// 创建一个使用特定 aid 的 Video 实例，用于番剧等特殊情况
    pub fn new_with_aid(client: &'a BiliClient, bvid: String, aid: String) -> Self {
        Self { client, aid, bvid }
    }

    /// 直接调用视频信息接口获取详细的视频信息，视频信息中包含了视频的分页信息
    pub async fn get_view_info(&self) -> Result<VideoInfo> {
        let mut res = self
            .client
            .request(Method::GET, "https://api.bilibili.com/x/web-interface/view")
            .await
            .query(&[("aid", &self.aid), ("bvid", &self.bvid)])
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?
            .validate()?;
        Ok(serde_json::from_value(res["data"].take())?)
    }

    #[allow(unused)]
    pub async fn get_pages(&self) -> Result<Vec<PageInfo>> {
        let mut res = self
            .client
            .request(Method::GET, "https://api.bilibili.com/x/player/pagelist")
            .await
            .query(&[("aid", &self.aid), ("bvid", &self.bvid)])
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?
            .validate()?;
        Ok(serde_json::from_value(res["data"].take())?)
    }

    pub async fn get_tags(&self) -> Result<Vec<Tag>> {
        let mut res = self
            .client
            .request(Method::GET, "https://api.bilibili.com/x/web-interface/view/detail/tag")
            .await
            .query(&[("aid", &self.aid), ("bvid", &self.bvid)])
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?
            .validate()?;
        Ok(serde_json::from_value(res["data"].take())?)
    }

    pub async fn get_danmaku_writer(&self, page: &'a PageInfo, token: CancellationToken) -> Result<DanmakuWriter> {
        let segment_count = page.duration.div_ceil(360);
        debug!("开始获取弹幕，共{}个分段", segment_count);

        // 串行获取弹幕分段，避免并发过多
        let mut all_danmaku: Vec<DanmakuElem> = Vec::new();

        for i in 1..=segment_count {
            if token.is_cancelled() {
                bail!("Danmaku download cancelled");
            }
            match self
                .get_danmaku_segment_with_retry(page, i as i64, 3, token.clone())
                .await
            {
                Ok(mut segment_danmaku) => {
                    debug!("成功获取弹幕分段 {}/{}", i, segment_count);
                    all_danmaku.append(&mut segment_danmaku);
                }
                Err(e) => {
                    warn!("获取弹幕分段 {}/{} 失败: {:#}", i, segment_count, e);
                    // 继续处理其他分段，不因单个分段失败而整体失败
                }
            }
        }

        // 按时间排序
        all_danmaku.sort_by_key(|d| d.progress);
        debug!("弹幕获取完成，共{}条弹幕", all_danmaku.len());

        Ok(DanmakuWriter::new(
            page,
            all_danmaku.into_iter().map(|x| x.into()).collect(),
        ))
    }

    /// 带重试机制的弹幕分段获取
    async fn get_danmaku_segment_with_retry(
        &self,
        page: &PageInfo,
        segment_idx: i64,
        max_retries: usize,
        token: CancellationToken,
    ) -> Result<Vec<DanmakuElem>> {
        let mut last_error = None;

        for attempt in 1..=max_retries {
            if token.is_cancelled() {
                bail!("Danmaku download cancelled");
            }
            match self.get_danmaku_segment(page, segment_idx, token.clone()).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        let delay = std::time::Duration::from_millis(1000 * attempt as u64);
                        debug!(
                            "弹幕分段{}获取失败，{}ms后重试({}/{}): {:#}",
                            segment_idx,
                            delay.as_millis(),
                            attempt,
                            max_retries,
                            last_error.as_ref().unwrap()
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        Err(last_error.unwrap())
    }

    async fn get_danmaku_segment(
        &self,
        page: &PageInfo,
        segment_idx: i64,
        token: CancellationToken,
    ) -> Result<Vec<DanmakuElem>> {
        debug!(
            "请求弹幕片段: type=1, oid={}, pid={}, segment_index={}",
            page.cid, self.aid, segment_idx
        );

        let url = format!(
            "http://api.bilibili.com/x/v2/dm/web/seg.so?type=1&oid={}&pid={}&segment_index={}",
            page.cid, self.aid, segment_idx
        );

        let res = tokio::select! {
            biased;
            _ = token.cancelled() => return Err(anyhow!("Download cancelled")),
            res = self.client.get(&url, token.clone()) => res,
        }?;

        if !res.status().is_success() {
            bail!("弹幕API请求失败，状态码: {}", res.status());
        }

        let headers = res.headers().clone();
        let content_type = headers.get("content-type");
        ensure!(
            content_type.is_some_and(|v| v == "application/octet-stream"),
            "unexpected content type: {:?}, body: {:?}",
            content_type,
            res.text().await
        );
        Ok(DmSegMobileReply::decode(res.bytes().await?)?.elems)
    }

    /// 带质量回退的页面分析器获取
    pub async fn get_page_analyzer_with_fallback(&self, page: &PageInfo) -> Result<PageAnalyzer> {
        // 质量回退列表：从最高到最低，恢复原始顺序
        let quality_levels = ["127", "126", "125", "120", "116", "112", "80", "64", "32", "16"];

        for (attempt, qn) in quality_levels.iter().enumerate() {
            tracing::debug!(
                "尝试获取视频流 (尝试 {}/{}): qn={}",
                attempt + 1,
                quality_levels.len(),
                qn
            );

            match self.get_page_analyzer_with_quality(page, qn).await {
                Ok(analyzer) => {
                    tracing::debug!("✓ 成功获取视频流: qn={}", qn);
                    return Ok(analyzer);
                }
                Err(e) => {
                    // 检查是否为充电专享视频错误（包括试看视频），如果是则不输出详细的质量级别失败日志
                    let (is_charging_video_error, is_trial_video) = {
                        if let Some(bili_err) = e.downcast_ref::<crate::bilibili::BiliError>() {
                            match bili_err {
                                crate::bilibili::BiliError::RequestFailed(87007 | 87008, msg) => {
                                    (true, msg.contains("试看视频"))
                                },
                                _ => (false, false)
                            }
                        } else {
                            (false, false)
                        }
                    };

                    if !is_charging_video_error {
                        tracing::warn!("× 质量 qn={} 获取失败: {}", qn, e);
                    } else if attempt == 0 && is_trial_video {
                        // 只在第一次尝试时记录试看视频信息
                        tracing::info!("检测到试看视频，需要充电才能观看完整版");
                    }

                    if attempt == quality_levels.len() - 1 {
                        // 最后一次尝试也失败了
                        if is_charging_video_error {
                            if !is_trial_video {
                                tracing::info!("视频需要充电才能观看");
                            }
                        } else {
                            tracing::error!("所有质量级别都获取失败");
                        }
                        return Err(e);
                    }
                    // 继续尝试下一个质量级别
                    continue;
                }
            }
        }

        // 理论上不会到达这里
        Err(anyhow!("无法获取任何质量的视频流"))
    }

    /// 使用指定质量获取页面分析器
    async fn get_page_analyzer_with_quality(&self, page: &PageInfo, qn: &str) -> Result<PageAnalyzer> {
        // 修复字符串生命周期问题
        let cid_string = page.cid.to_string();

        // 恢复原始API参数配置，基于工作版本的设置
        let params = vec![
            ("avid", self.aid.as_str()),
            ("cid", cid_string.as_str()),
            ("qn", qn), // 使用指定的质量参数
            ("otype", "json"),
            ("fnval", "4048"), // 恢复原始fnval值
            ("fourk", "1"),    // 启用4K支持
        ];

        tracing::debug!("API参数: {:?}", params);

        let request_url = "https://api.bilibili.com/x/player/wbi/playurl";

        let res = self
            .client
            .request(Method::GET, request_url)
            .await
            .query(&encoded_query(params, MIXIN_KEY.load().as_deref()))
            .header("Referer", "https://www.bilibili.com/")
            .header("Origin", "https://www.bilibili.com")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        // 添加详细的API响应日志
        tracing::debug!("API完整响应: {}", serde_json::to_string_pretty(&res).unwrap_or_else(|_| "无法序列化".to_string()));
        
        // 记录关键字段
        if let Some(code) = res["code"].as_i64() {
            tracing::debug!("API返回code: {}", code);
        }
        if let Some(message) = res["message"].as_str() {
            tracing::debug!("API返回message: {}", message);
        }
        
        // 检查data字段是否存在
        if res["data"].is_null() {
            tracing::debug!("API返回的data字段为null");
        } else if let Some(dash) = res["data"]["dash"].as_object() {
            tracing::debug!("dash对象存在，视频流数量: {}", 
                dash.get("video").and_then(|v| v.as_array()).map(|v| v.len()).unwrap_or(0));
            tracing::debug!("dash对象存在，音频流数量: {}", 
                dash.get("audio").and_then(|v| v.as_array()).map(|v| v.len()).unwrap_or(0));
        } else {
            tracing::debug!("API返回的data.dash字段不存在或不是对象");
        }

        // 检查API响应中的错误信息
        if let Some(code) = res["code"].as_i64() {
            if code != 0 {
                let message = res["message"].as_str().unwrap_or("未知错误");
                return Err(crate::bilibili::BiliError::RequestFailed(code, message.to_string()).into());
            }
        }

        // 检查是否有可用的视频流 (只接受dash格式，durl是试看片段)
        let has_dash_video = res["data"]["dash"]["video"].as_array().is_some_and(|v| !v.is_empty());
        let has_durl_only = res["data"]["durl"].as_array().is_some_and(|v| !v.is_empty()) && !has_dash_video;
        
        if has_durl_only {
            // 只在debug级别记录试看视频详情，避免日志过多
            tracing::debug!("试看视频data字段: {}", 
                serde_json::to_string_pretty(&res["data"]).unwrap_or_else(|_| "无法序列化".to_string()));
            // 返回充电视频错误，触发自动删除
            return Err(crate::bilibili::BiliError::RequestFailed(87008, "试看视频，需要充电才能观看完整版".to_string()).into());
        }
        
        if !has_dash_video {
            tracing::error!("视频流为空，完整的data字段: {}", 
                serde_json::to_string_pretty(&res["data"]).unwrap_or_else(|_| "无法序列化".to_string()));
            return Err(crate::bilibili::BiliError::VideoStreamEmpty("API返回的视频流为空".to_string()).into());
        }

        // 记录成功获取的质量信息
        if let Some(quality) = res["data"]["quality"].as_u64() {
            tracing::debug!("API返回的实际质量: {}", quality);
        }
        if let Some(accept_quality) = res["data"]["accept_quality"].as_array() {
            let qualities: Vec<u64> = accept_quality.iter().filter_map(|v| v.as_u64()).collect();
            tracing::debug!("可用质量列表: {:?}", qualities);
        }

        let mut validated_res = res.validate()?;
        Ok(PageAnalyzer::new(validated_res["data"].take()))
    }

    pub async fn get_page_analyzer(&self, page: &PageInfo) -> Result<PageAnalyzer> {
        // 修复字符串生命周期问题
        let cid_string = page.cid.to_string();

        // 恢复原始API参数配置，基于工作版本的设置
        let params = vec![
            ("avid", self.aid.as_str()),
            ("cid", cid_string.as_str()),
            ("qn", "127"), // 恢复原始qn=127请求8K质量
            ("otype", "json"),
            ("fnval", "4048"), // 恢复原始fnval值
            ("fourk", "1"),    // 启用4K支持
        ];

        tracing::debug!("=== API参数调试 ===");
        tracing::debug!("视频: {} (aid: {})", self.bvid, self.aid);
        tracing::debug!("分页: cid: {}", page.cid);
        tracing::debug!("请求参数: {:?}", params);

        let request_url = "https://api.bilibili.com/x/player/wbi/playurl";
        tracing::debug!("请求URL: {}", request_url);

        let res = self
            .client
            .request(Method::GET, request_url)
            .await
            .query(&encoded_query(params, MIXIN_KEY.load().as_deref()))
            .header("Referer", "https://www.bilibili.com/")
            .header("Origin", "https://www.bilibili.com")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        // 增强的API响应调试信息
        tracing::debug!("=== API响应调试 ===");
        if let Some(code) = res["code"].as_i64() {
            tracing::debug!("响应代码: {}", code);
        }
        if let Some(message) = res["message"].as_str() {
            tracing::debug!("响应消息: {}", message);
        }

        // 记录视频质量信息
        if let Some(quality) = res["data"]["quality"].as_u64() {
            tracing::debug!("API返回的当前质量: {}", quality);
        }
        if let Some(accept_quality) = res["data"]["accept_quality"].as_array() {
            let qualities: Vec<u64> = accept_quality.iter().filter_map(|v| v.as_u64()).collect();
            tracing::debug!("API返回的可用质量列表: {:?}", qualities);
        }

        // 检查是否存在VIP要求
        if let Some(vip_status) = res["data"]["vip_status"].as_i64() {
            tracing::debug!("VIP状态要求: {}", vip_status);
        }
        if let Some(vip_type) = res["data"]["vip_type"].as_i64() {
            tracing::debug!("VIP类型: {}", vip_type);
        }

        tracing::debug!("=== API响应调试结束 ===");

        let mut validated_res = res.validate()?;
        Ok(PageAnalyzer::new(validated_res["data"].take()))
    }

    /// 带质量回退的番剧页面分析器获取
    pub async fn get_bangumi_page_analyzer_with_fallback(&self, page: &PageInfo, ep_id: &str) -> Result<PageAnalyzer> {
        // 质量回退列表：从最高到最低，恢复原始顺序
        let quality_levels = ["127", "126", "125", "120", "116", "112", "80", "64", "32", "16"];

        for (attempt, qn) in quality_levels.iter().enumerate() {
            tracing::debug!(
                "尝试获取番剧视频流 (尝试 {}/{}): qn={}",
                attempt + 1,
                quality_levels.len(),
                qn
            );

            match self.get_bangumi_page_analyzer_with_quality(page, ep_id, qn).await {
                Ok(analyzer) => {
                    tracing::debug!("✓ 成功获取番剧视频流: qn={}", qn);
                    return Ok(analyzer);
                }
                Err(e) => {
                    // 检查是否为充电专享视频错误，如果是则不输出详细的质量级别失败日志
                    let is_charging_video_error = {
                        if let Some(bili_err) = e.downcast_ref::<crate::bilibili::BiliError>() {
                            matches!(bili_err, crate::bilibili::BiliError::RequestFailed(87007 | 87008, _))
                        } else {
                            false
                        }
                    };

                    if !is_charging_video_error {
                        tracing::warn!("× 番剧质量 qn={} 获取失败: {}", qn, e);
                    } else {
                        tracing::debug!("× 番剧质量 qn={} 获取失败: 充电专享视频", qn);
                    }

                    if attempt == quality_levels.len() - 1 {
                        // 最后一次尝试也失败了
                        if is_charging_video_error {
                            tracing::debug!("所有番剧质量级别都获取失败: 充电专享视频");
                        } else {
                            tracing::error!("所有番剧质量级别都获取失败");
                        }
                        return Err(e);
                    }
                    // 继续尝试下一个质量级别
                    continue;
                }
            }
        }

        // 理论上不会到达这里
        Err(anyhow!("无法获取任何质量的番剧视频流"))
    }

    /// 使用指定质量获取番剧页面分析器
    async fn get_bangumi_page_analyzer_with_quality(
        &self,
        page: &PageInfo,
        ep_id: &str,
        qn: &str,
    ) -> Result<PageAnalyzer> {
        // 修复字符串生命周期问题
        let cid_string = page.cid.to_string();

        // 恢复原始番剧API参数配置
        let params = [
            ("ep_id", ep_id),
            ("cid", cid_string.as_str()),
            ("qn", qn), // 使用指定的质量参数
            ("otype", "json"),
            ("fnval", "4048"), // 恢复原始fnval值
            ("fourk", "1"),    // 启用4K支持
        ];

        tracing::debug!("番剧API参数: {:?}", params);

        let request_url = "https://api.bilibili.com/pgc/player/web/playurl";

        let res = self
            .client
            .request(Method::GET, request_url)
            .await
            .query(&params)
            .header("Referer", "https://www.bilibili.com/")
            .header("Origin", "https://www.bilibili.com")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        // 添加详细的番剧API响应日志
        tracing::debug!("番剧API完整响应: {}", serde_json::to_string_pretty(&res).unwrap_or_else(|_| "无法序列化".to_string()));
        
        // 记录关键字段
        if let Some(code) = res["code"].as_i64() {
            tracing::debug!("番剧API返回code: {}", code);
        }
        if let Some(message) = res["message"].as_str() {
            tracing::debug!("番剧API返回message: {}", message);
        }
        
        // 检查result字段是否存在
        if res["result"].is_null() {
            tracing::debug!("番剧API返回的result字段为null");
        } else if let Some(dash) = res["result"]["dash"].as_object() {
            tracing::debug!("番剧dash对象存在，视频流数量: {}", 
                dash.get("video").and_then(|v| v.as_array()).map(|v| v.len()).unwrap_or(0));
            tracing::debug!("番剧dash对象存在，音频流数量: {}", 
                dash.get("audio").and_then(|v| v.as_array()).map(|v| v.len()).unwrap_or(0));
        } else {
            tracing::debug!("番剧API返回的result.dash字段不存在或不是对象");
        }

        // 检查番剧API响应中的错误信息
        if let Some(code) = res["code"].as_i64() {
            if code != 0 {
                let message = res["message"].as_str().unwrap_or("未知错误");
                return Err(crate::bilibili::BiliError::RequestFailed(code, message.to_string()).into());
            }
        }

        // 检查是否有可用的番剧视频流
        if res["result"]["dash"]["video"].as_array().is_none_or(|v| v.is_empty()) {
            tracing::error!("番剧视频流为空，完整的result字段: {}", 
                serde_json::to_string_pretty(&res["result"]).unwrap_or_else(|_| "无法序列化".to_string()));
            return Err(crate::bilibili::BiliError::VideoStreamEmpty("番剧API返回的视频流为空".to_string()).into());
        }

        // 记录成功获取的番剧质量信息
        if let Some(quality) = res["result"]["quality"].as_u64() {
            tracing::debug!("番剧API返回的实际质量: {}", quality);
        }
        if let Some(accept_quality) = res["result"]["accept_quality"].as_array() {
            let qualities: Vec<u64> = accept_quality.iter().filter_map(|v| v.as_u64()).collect();
            tracing::debug!("番剧可用质量列表: {:?}", qualities);
        }

        let mut validated_res = res.validate()?;
        Ok(PageAnalyzer::new(validated_res["result"].take()))
    }

    /// 专门为番剧获取播放地址分析器
    pub async fn get_bangumi_page_analyzer(&self, page: &PageInfo, ep_id: &str) -> Result<PageAnalyzer> {
        // 修复字符串生命周期问题
        let cid_string = page.cid.to_string();

        // 恢复原始番剧API参数配置
        let params = [
            ("ep_id", ep_id),
            ("cid", cid_string.as_str()),
            ("qn", "127"), // 恢复原始qn=127请求8K质量
            ("otype", "json"),
            ("fnval", "4048"), // 恢复原始fnval值
            ("fourk", "1"),    // 启用4K支持
        ];

        tracing::debug!("=== 番剧API参数调试 ===");
        tracing::debug!("番剧EP: {}", ep_id);
        tracing::debug!("分页: cid: {}", page.cid);
        tracing::debug!("请求参数: {:?}", params);

        let request_url = "https://api.bilibili.com/pgc/player/web/playurl";
        
        // 构建完整的URL用于日志显示
        let mut full_url = format!("{}?", request_url);
        for (i, (key, value)) in params.iter().enumerate() {
            if i > 0 {
                full_url.push('&');
            }
            full_url.push_str(&format!("{}={}", key, value));
        }
        
        tracing::debug!("==================== 番剧API请求 ====================");
        tracing::debug!("完整请求URL: {}", full_url);
        tracing::debug!("==================================================");

        let res = self
            .client
            .request(Method::GET, request_url)
            .await
            .query(&params)
            .header("Referer", "https://www.bilibili.com/")
            .header("Origin", "https://www.bilibili.com")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36")
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;

        // 增强的番剧API响应调试信息
        tracing::debug!("=== 番剧API响应调试 ===");
        if let Some(code) = res["code"].as_i64() {
            tracing::debug!("响应代码: {}", code);
        }
        if let Some(message) = res["message"].as_str() {
            tracing::debug!("响应消息: {}", message);
        }

        // 记录番剧视频质量信息
        if let Some(quality) = res["result"]["quality"].as_u64() {
            tracing::debug!("番剧API返回的当前质量: {}", quality);
        }
        if let Some(accept_quality) = res["result"]["accept_quality"].as_array() {
            let qualities: Vec<u64> = accept_quality.iter().filter_map(|v| v.as_u64()).collect();
            tracing::debug!("番剧API返回的可用质量列表: {:?}", qualities);
        }

        // 检查番剧会员要求
        if let Some(vip_status) = res["result"]["vip_status"].as_i64() {
            tracing::debug!("番剧VIP状态要求: {}", vip_status);
        }
        if let Some(vip_type) = res["result"]["vip_type"].as_i64() {
            tracing::debug!("番剧VIP类型: {}", vip_type);
        }

        tracing::debug!("=== 番剧API响应调试结束 ===");

        let mut validated_res = res.validate()?;
        Ok(PageAnalyzer::new(validated_res["result"].take()))
    }

    pub async fn get_subtitles(&self, page: &PageInfo) -> Result<Vec<SubTitle>> {
        let res = self
            .client
            .request(Method::GET, "https://api.bilibili.com/x/player/wbi/v2")
            .await
            .query(&encoded_query(
                vec![("cid", &page.cid.to_string()), ("bvid", &self.bvid), ("aid", &self.aid)],
                MIXIN_KEY.load().as_deref(),
            ))
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?
            .validate()?;

        // 检查字幕数据是否存在
        let subtitle_data = &res["data"]["subtitle"];
        if subtitle_data.is_null() {
            debug!("视频没有字幕数据");
            return Ok(Vec::new());
        }

        // 接口返回的信息，包含了一系列的字幕，每个字幕包含了字幕的语言和 json 下载地址
        let subtitles_info: SubTitlesInfo = serde_json::from_value(subtitle_data.clone())?;
        let tasks = subtitles_info
            .subtitles
            .into_iter()
            .filter(|v| !v.is_ai_sub())
            .map(|v| self.get_subtitle(v))
            .collect::<FuturesUnordered<_>>();
        tasks.try_collect().await
    }

    async fn get_subtitle(&self, info: SubTitleInfo) -> Result<SubTitle> {
        let mut res = self
            .client
            .client // 这里可以直接使用 inner_client，因为该请求不需要鉴权
            .request(Method::GET, format!("https:{}", &info.subtitle_url).as_str(), None)
            .send()
            .await?
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        let body: SubTitleBody = serde_json::from_value(res["body"].take())?;
        Ok(SubTitle { lan: info.lan, body })
    }
}

pub fn bvid_to_aid(bvid: &str) -> u64 {
    let mut bvid = bvid.chars().collect::<Vec<_>>();
    (bvid[3], bvid[9]) = (bvid[9], bvid[3]);
    (bvid[4], bvid[7]) = (bvid[7], bvid[4]);
    let mut tmp = 0u64;
    for char in bvid.into_iter().skip(3) {
        let idx = DATA.iter().position(|&x| x == char).expect("invalid bvid");
        tmp = tmp * BASE + idx as u64;
    }
    (tmp & MASK_CODE) ^ XOR_CODE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bvid_to_aid() {
        assert_eq!(bvid_to_aid("BV1Tr421n746"), 1401752220u64);
        assert_eq!(bvid_to_aid("BV1sH4y1s7fe"), 1051892992u64);
    }
}
