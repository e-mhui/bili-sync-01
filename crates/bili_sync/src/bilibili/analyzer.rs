use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::bilibili::error::BiliError;

pub struct PageAnalyzer {
    info: serde_json::Value,
}

#[derive(
    Debug, strum::FromRepr, strum::EnumString, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Clone, Copy,
)]
pub enum VideoQuality {
    Quality360p = 16,
    Quality480p = 32,
    Quality720p = 64,
    Quality1080p = 80,
    Quality1080pPLUS = 112,
    Quality1080p60 = 116,
    Quality4k = 120,
    QualityHdr = 125,
    QualityDolby = 126,
    Quality8k = 127,
}

#[derive(Debug, Clone, Copy, strum::FromRepr, strum::EnumString, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioQuality {
    Quality64k = 30216,
    Quality132k = 30232,
    QualityDolby = 30250,
    QualityHiRES = 30251,
    Quality192k = 30280,
}

impl Ord for AudioQuality {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_sort_key().cmp(&other.as_sort_key())
    }
}

impl PartialOrd for AudioQuality {
    fn partial_cmp(&self, other: &AudioQuality) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AudioQuality {
    pub fn as_sort_key(&self) -> isize {
        match self {
            // 这可以让 Dolby 和 Hi-RES 排在 192k 之后，且 Dolby 和 Hi-RES 之间的顺序不变
            Self::QualityHiRES | Self::QualityDolby => (*self as isize) + 40,
            _ => *self as isize,
        }
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(
    Debug,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    Clone,
    Copy,
)]
pub enum VideoCodecs {
    #[strum(serialize = "HEV")]
    HEV,
    #[strum(serialize = "AVC")]
    AVC,
    #[strum(serialize = "AV1")]
    AV1,
}

impl TryFrom<u64> for VideoCodecs {
    type Error = anyhow::Error;

    fn try_from(value: u64) -> std::result::Result<Self, Self::Error> {
        // https://socialsisteryi.github.io/bilibili-API-collect/docs/video/videostream_url.html#%E8%A7%86%E9%A2%91%E7%BC%96%E7%A0%81%E4%BB%A3%E7%A0%81
        match value {
            7 => Ok(Self::AVC),
            12 => Ok(Self::HEV),
            13 => Ok(Self::AV1),
            _ => bail!("invalid video codecs id: {}", value),
        }
    }
}

// 视频流的筛选偏好
#[derive(Serialize, Deserialize, Clone)]
pub struct FilterOption {
    pub video_max_quality: VideoQuality,
    pub video_min_quality: VideoQuality,
    pub audio_max_quality: AudioQuality,
    pub audio_min_quality: AudioQuality,
    pub codecs: Vec<VideoCodecs>,
    pub no_dolby_video: bool,
    pub no_dolby_audio: bool,
    pub no_hdr: bool,
    pub no_hires: bool,
}

impl Default for FilterOption {
    fn default() -> Self {
        Self {
            video_max_quality: VideoQuality::Quality8k,
            video_min_quality: VideoQuality::Quality360p,
            audio_max_quality: AudioQuality::QualityHiRES,
            audio_min_quality: AudioQuality::Quality64k,
            codecs: vec![VideoCodecs::AVC, VideoCodecs::HEV, VideoCodecs::AV1],
            no_dolby_video: false,
            no_dolby_audio: false,
            no_hdr: false,
            no_hires: false,
        }
    }
}

// 上游项目中的五种流类型，不过目测应该只有 Flv、DashVideo、DashAudio 三种会被用到
#[derive(Debug, PartialEq, PartialOrd)]
pub enum Stream {
    Flv(String),
    Html5Mp4(String),
    EpisodeTryMp4(String),
    DashVideo {
        url: String,
        backup_url: Vec<String>,
        quality: VideoQuality,
        codecs: VideoCodecs,
    },
    DashAudio {
        url: String,
        backup_url: Vec<String>,
        quality: AudioQuality,
    },
}

// 通用的获取流链接的方法，交由 Downloader 使用
impl Stream {
    pub fn urls(&self) -> Vec<&str> {
        match self {
            Self::Flv(url) | Self::Html5Mp4(url) | Self::EpisodeTryMp4(url) => vec![url],
            Self::DashVideo { url, backup_url, .. } | Self::DashAudio { url, backup_url, .. } => {
                let mut urls = std::iter::once(url.as_str())
                    .chain(backup_url.iter().map(|s| s.as_str()))
                    .collect();
                let current_config = crate::config::reload_config();
                if !current_config.cdn_sorting {
                    urls
                } else {
                    urls.sort_by_key(|u| {
                        if u.contains("upos-") {
                            0 // 服务商 cdn
                        } else if u.contains("cn-") {
                            1 // 自建 cdn
                        } else if u.contains("mcdn") {
                            2 // mcdn
                        } else {
                            3 // pcdn 或者其它
                        }
                    });
                    urls
                }
            }
        }
    }
}

/// 用于获取视频流的最佳筛选结果，有两种可能：
/// 1. 单个混合流，作为 Mixed 返回
/// 2. 视频、音频分离，作为 VideoAudio 返回，其中音频流可能不存在（对于无声视频，如 BV1J7411H7KQ）
#[derive(Debug)]
pub enum BestStream {
    VideoAudio { video: Stream, audio: Option<Stream> },
    Mixed(Stream),
}

impl PageAnalyzer {
    pub fn new(info: serde_json::Value) -> Self {
        Self { info }
    }

    fn is_flv_stream(&self) -> bool {
        self.info.get("durl").is_some() && self.info["format"].as_str().is_some_and(|f| f.starts_with("flv"))
    }

    fn is_html5_mp4_stream(&self) -> bool {
        self.info.get("durl").is_some()
            && self.info["format"].as_str().is_some_and(|f| f.starts_with("mp4"))
            && self.info["is_html5"].as_bool().is_some_and(|b| b)
    }

    fn is_episode_try_mp4_stream(&self) -> bool {
        self.info.get("durl").is_some()
            && self.info["format"].as_str().is_some_and(|f| f.starts_with("mp4"))
            && self.info["is_html5"].as_bool().is_none_or(|b| !b)
    }

    /// 获取所有的视频、音频流，并根据条件筛选
    fn streams(&mut self, filter_option: &FilterOption) -> Result<Vec<Stream>> {
        if self.is_flv_stream() {
            return Ok(vec![Stream::Flv(
                self.info["durl"][0]["url"]
                    .as_str()
                    .context("invalid flv stream")?
                    .to_string(),
            )]);
        }
        if self.is_html5_mp4_stream() {
            return Ok(vec![Stream::Html5Mp4(
                self.info["durl"][0]["url"]
                    .as_str()
                    .context("invalid html5 mp4 stream")?
                    .to_string(),
            )]);
        }
        if self.is_episode_try_mp4_stream() {
            return Ok(vec![Stream::EpisodeTryMp4(
                self.info["durl"][0]["url"]
                    .as_str()
                    .context("invalid episode try mp4 stream")?
                    .to_string(),
            )]);
        }
        
        // 优化的B站API响应调试日志
        tracing::info!("=== B站视频流分析 ===");
        if let Some(video_array) = self.info.pointer("/dash/video").and_then(|v| v.as_array()) {
            tracing::info!("API返回视频流数量: {}", video_array.len());
            let mut available_qualities = Vec::new();
            for video in video_array.iter() {
                if let Some(quality_id) = video["id"].as_u64() {
                    if let Some(quality) = VideoQuality::from_repr(quality_id as usize) {
                        available_qualities.push(format!("{:?}({})", quality, quality_id));
                    }
                }
            }
            if !available_qualities.is_empty() {
                tracing::info!("可用质量: [{}]", available_qualities.join(", "));
            } else {
                tracing::warn!("未找到有效的视频质量信息");
            }
        } else {
            tracing::warn!("API响应中未找到dash/video数组");
        }
        
        tracing::info!("筛选条件: {}({}) - {}({}), 编码: {:?}", 
            format!("{:?}", filter_option.video_max_quality), filter_option.video_max_quality as u32,
            format!("{:?}", filter_option.video_min_quality), filter_option.video_min_quality as u32,
            filter_option.codecs);
        tracing::info!("=== 开始筛选 ===");
        
        let mut streams: Vec<Stream> = Vec::new();
        let mut filtered_count = 0;
        for (idx, video) in self
            .info
            .pointer_mut("/dash/video")
            .and_then(|v| v.as_array_mut())
            .ok_or(BiliError::RiskControlOccurred)?
            .iter_mut()
            .enumerate()
        {
            let (Some(url), Some(quality_id), Some(codecs_id)) = (
                video["baseUrl"].as_str(),
                video["id"].as_u64(),
                video["codecid"].as_u64(),
            ) else {
                tracing::info!("流 {}: 跳过 - 缺少必要字段", idx + 1);
                continue;
            };
            
            let quality = match VideoQuality::from_repr(quality_id as usize) {
                Some(q) => q,
                None => {
                    tracing::info!("流 {}: 跳过 - 无效的质量ID {}", idx + 1, quality_id);
                    continue;
                }
            };
            
            let codecs = match codecs_id.try_into() {
                Ok(c) => c,
                Err(_) => {
                    tracing::info!("流 {}: 跳过 - 无效的编码ID {}", idx + 1, codecs_id);
                    continue;
                }
            };
            
            // 筛选条件检查
            let mut passed = true;
            let mut filter_reason = String::new();
            
            if !filter_option.codecs.contains(&codecs) {
                passed = false;
                filter_reason = format!("编码{:?}不符", codecs);
            } else if quality < filter_option.video_min_quality {
                passed = false;
                filter_reason = format!("质量过低(<{:?})", filter_option.video_min_quality);
            } else if quality > filter_option.video_max_quality {
                passed = false;
                filter_reason = format!("质量过高(>{:?})", filter_option.video_max_quality);
            } else if quality == VideoQuality::QualityHdr && filter_option.no_hdr {
                passed = false;
                filter_reason = "HDR被禁用".to_string();
            } else if quality == VideoQuality::QualityDolby && filter_option.no_dolby_video {
                passed = false;
                filter_reason = "杜比视界被禁用".to_string();
            }
            
            if !passed {
                filtered_count += 1;
                tracing::debug!("过滤: {:?}({}) - {}", quality, quality_id, filter_reason);
                continue;
            }
            
            tracing::info!("✓ 接受: {:?}({}) {:?}", quality, quality_id, codecs);
            
            streams.push(Stream::DashVideo {
                url: url.to_string(),
                backup_url: serde_json::from_value(video["backupUrl"].take()).unwrap_or_default(),
                quality,
                codecs,
            });
        }
        
        let video_stream_count = streams.iter().filter(|s| matches!(s, Stream::DashVideo { .. })).count();
        tracing::info!("=== 筛选结果: {}个通过, {}个过滤 ===", video_stream_count, filtered_count);
        
        if video_stream_count == 0 {
            // 分析筛选失败的原因
            let max_quality_requested = filter_option.video_max_quality as u32;
            if max_quality_requested >= 120 {
                tracing::error!("❌ 无可用视频流！请求4K+质量({})可能需要：", max_quality_requested);
                tracing::error!("   1. 大会员权限");
                tracing::error!("   2. 视频本身支持该质量");
                tracing::error!("   3. 正确的登录状态");
                tracing::error!("   建议：降低质量要求或检查会员状态");
            } else {
                tracing::error!("❌ 无可用视频流！可能原因：");
                tracing::error!("   1. 网络问题或API访问失败");
                tracing::error!("   2. 视频不存在或已删除");
                tracing::error!("   3. 编码格式不匹配");
                tracing::error!("   建议：检查网络连接和编码设置");
            }
        }
        if let Some(audios) = self.info.pointer_mut("/dash/audio").and_then(|a| a.as_array_mut()) {
            for audio in audios.iter_mut() {
                let (Some(url), Some(quality)) = (audio["baseUrl"].as_str(), audio["id"].as_u64()) else {
                    continue;
                };
                let quality = AudioQuality::from_repr(quality as usize).context("invalid audio stream quality")?;
                if quality < filter_option.audio_min_quality || quality > filter_option.audio_max_quality {
                    continue;
                }
                streams.push(Stream::DashAudio {
                    url: url.to_string(),
                    backup_url: serde_json::from_value(audio["backupUrl"].take()).unwrap_or_default(),
                    quality,
                });
            }
        }
        if !filter_option.no_hires {
            if let Some(flac) = self.info.pointer_mut("/dash/flac/audio") {
                let (Some(url), Some(quality)) = (flac["baseUrl"].as_str(), flac["id"].as_u64()) else {
                    bail!("invalid flac stream");
                };
                let quality = AudioQuality::from_repr(quality as usize).context("invalid flac stream quality")?;
                if quality >= filter_option.audio_min_quality && quality <= filter_option.audio_max_quality {
                    streams.push(Stream::DashAudio {
                        url: url.to_string(),
                        backup_url: serde_json::from_value(flac["backupUrl"].take()).unwrap_or_default(),
                        quality,
                    });
                }
            }
        }
        if !filter_option.no_dolby_audio {
            if let Some(dolby_audio) = self
                .info
                .pointer_mut("/dash/dolby/audio/0")
                .and_then(|a| a.as_object_mut())
            {
                let (Some(url), Some(quality)) = (dolby_audio["baseUrl"].as_str(), dolby_audio["id"].as_u64()) else {
                    bail!("invalid dolby audio stream");
                };
                let quality =
                    AudioQuality::from_repr(quality as usize).context("invalid dolby audio stream quality")?;
                if quality >= filter_option.audio_min_quality && quality <= filter_option.audio_max_quality {
                    streams.push(Stream::DashAudio {
                        url: url.to_string(),
                        backup_url: serde_json::from_value(dolby_audio["backupUrl"].take()).unwrap_or_default(),
                        quality,
                    });
                }
            }
        }
        Ok(streams)
    }

    pub fn best_stream(&mut self, filter_option: &FilterOption) -> Result<BestStream> {
        let streams = self.streams(filter_option)?;
        if self.is_flv_stream() || self.is_html5_mp4_stream() || self.is_episode_try_mp4_stream() {
            // 按照 streams 中的假设，符合这三种情况的流只有一个，直接取
            return Ok(BestStream::Mixed(
                streams.into_iter().next().context("no stream found")?,
            ));
        }
        let (videos, audios): (Vec<Stream>, Vec<Stream>) =
            streams.into_iter().partition(|s| matches!(s, Stream::DashVideo { .. }));
            
        tracing::info!("=== 最佳流选择 ===");
        if videos.is_empty() {
            tracing::error!("错误: 没有可用的视频流！");
            return Err(anyhow!("no video stream found"));
        }
        
        tracing::info!("候选流: {}", videos.iter()
            .filter_map(|s| match s {
                Stream::DashVideo { quality, codecs, .. } => 
                    Some(format!("{:?}({}):{:?}", quality, *quality as u32, codecs)),
                _ => None
            })
            .collect::<Vec<_>>()
            .join(", ")
        );
        
        let selected_video = videos
            .into_iter()
            .max_by(|a, b| match (a, b) {
                (
                    Stream::DashVideo {
                        quality: a_quality,
                        codecs: a_codecs,
                        ..
                    },
                    Stream::DashVideo {
                        quality: b_quality,
                        codecs: b_codecs,
                        ..
                    },
                ) => {
                    // 优先按质量选择
                    if a_quality != b_quality {
                        return a_quality.cmp(b_quality);
                    }
                    // 质量相同时，按编码偏好选择
                    let a_pos = filter_option.codecs.iter().position(|c| c == a_codecs);
                    let b_pos = filter_option.codecs.iter().position(|c| c == b_codecs);
                    b_pos.cmp(&a_pos) // 优先选择更靠前的编码
                }
                _ => unreachable!(),
            })
            .context("no video stream found")?;
            
        if let Stream::DashVideo { quality, codecs, .. } = &selected_video {
            tracing::info!("✓ 最终选择: {:?}({}) {:?}", quality, *quality as u32, codecs);
        }
        
        Ok(BestStream::VideoAudio {
            video: selected_video,
            audio: audios.into_iter().max_by(|a, b| match (a, b) {
                (Stream::DashAudio { quality: a_quality, .. }, Stream::DashAudio { quality: b_quality, .. }) => {
                    a_quality.cmp(b_quality)
                }
                _ => unreachable!(),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quality_order() {
        assert!([
            VideoQuality::Quality360p,
            VideoQuality::Quality480p,
            VideoQuality::Quality720p,
            VideoQuality::Quality1080p,
            VideoQuality::Quality1080pPLUS,
            VideoQuality::Quality1080p60,
            VideoQuality::Quality4k,
            VideoQuality::QualityHdr,
            VideoQuality::QualityDolby,
            VideoQuality::Quality8k
        ]
        .is_sorted());
        assert!([
            AudioQuality::Quality64k,
            AudioQuality::Quality132k,
            AudioQuality::Quality192k,
            AudioQuality::QualityDolby,
            AudioQuality::QualityHiRES,
        ]
        .is_sorted());
    }

    #[test]
    fn test_url_sort() {
        let stream = Stream::DashVideo {
            url: "https://xy116x207x155x163xy240ey95dy1010y700yy8dxy.mcdn.bilivideo.cn:4483".to_owned(),
            backup_url: vec![
                "https://upos-sz-mirrorcos.bilivideo.com".to_owned(),
                "https://cn-tj-cu-01-11.bilivideo.com".to_owned(),
                "https://xxx.v1d.szbdys.com".to_owned(),
            ],
            quality: VideoQuality::Quality1080p,
            codecs: VideoCodecs::AVC,
        };
        assert_eq!(
            stream.urls(),
            vec![
                "https://upos-sz-mirrorcos.bilivideo.com",
                "https://cn-tj-cu-01-11.bilivideo.com",
                "https://xy116x207x155x163xy240ey95dy1010y700yy8dxy.mcdn.bilivideo.cn:4483",
                "https://xxx.v1d.szbdys.com"
            ]
        );
    }
}
