use chrono::{DateTime, NaiveDateTime, Utc};
use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::IntoActiveModel;

use crate::bilibili::{PageInfo, VideoInfo};

impl VideoInfo {
    /// 在检测视频更新时，通过该方法将 VideoInfo 转换为简单的 ActiveModel，此处仅填充一些简单信息，后续会使用详情覆盖
    pub fn into_simple_model(self) -> bili_sync_entity::video::ActiveModel {
        let default = bili_sync_entity::video::ActiveModel {
            id: NotSet,
            created_at: NotSet,
            // 此处不使用 ActiveModel::default() 是为了让其它字段有默认值
            ..bili_sync_entity::video::Model::default().into_active_model()
        };
        match self {
            VideoInfo::Collection {
                bvid,
                cover,
                ctime,
                pubtime,
            } => bili_sync_entity::video::ActiveModel {
                bvid: Set(bvid),
                cover: Set(cover),
                ctime: Set(ctime.naive_utc()),
                pubtime: Set(pubtime.naive_utc()),
                category: Set(2), // 视频合集里的内容类型肯定是视频
                valid: Set(true),
                ..default
            },
            VideoInfo::Favorite {
                title,
                vtype,
                bvid,
                intro,
                cover,
                upper,
                ctime,
                fav_time,
                pubtime,
                attr,
            } => bili_sync_entity::video::ActiveModel {
                bvid: Set(bvid),
                name: Set(title),
                category: Set(vtype),
                intro: Set(intro),
                cover: Set(cover),
                ctime: Set(ctime.naive_utc()),
                pubtime: Set(pubtime.naive_utc()),
                favtime: Set(fav_time.naive_utc()),
                download_status: Set(0),
                valid: Set(attr == 0),
                upper_id: Set(upper.mid),
                upper_name: Set(upper.name),
                upper_face: Set(upper.face),
                ..default
            },
            VideoInfo::WatchLater {
                title,
                bvid,
                intro,
                cover,
                upper,
                ctime,
                fav_time,
                pubtime,
                state,
            } => bili_sync_entity::video::ActiveModel {
                bvid: Set(bvid),
                name: Set(title),
                category: Set(2), // 稍后再看里的内容类型肯定是视频
                intro: Set(intro),
                cover: Set(cover),
                ctime: Set(ctime.naive_utc()),
                pubtime: Set(pubtime.naive_utc()),
                favtime: Set(fav_time.naive_utc()),
                download_status: Set(0),
                valid: Set(state == 0),
                upper_id: Set(upper.mid),
                upper_name: Set(upper.name),
                upper_face: Set(upper.face),
                ..default
            },
            VideoInfo::Submission {
                title,
                bvid,
                intro,
                cover,
                ctime,
            } => bili_sync_entity::video::ActiveModel {
                bvid: Set(bvid),
                name: Set(title),
                intro: Set(intro),
                cover: Set(cover),
                ctime: Set(ctime.naive_utc()),
                category: Set(2), // 投稿视频的内容类型肯定是视频
                valid: Set(true),
                ..default
            },
            VideoInfo::Bangumi {
                title,
                bvid,
                season_id,
                ep_id,
                cover,
                intro,
                pubtime,
                show_title,
                season_number,
                episode_number,
                share_copy,
                show_season_type,
                actors,
                ..
            } => {
                // 对于番剧，智能选择最详细的标题作为name
                // 对于番剧影视类型(show_season_type=2)，不使用share_copy避免文件名过长
                // 优先级：番剧影视类型(show_title > title)，常规番剧(share_copy > show_title > title)
                tracing::debug!(
                    "处理番剧转换: title={}, share_copy={:?}, show_title={:?}, show_season_type={:?}",
                    title,
                    share_copy,
                    show_title,
                    show_season_type
                );
                let intelligent_name = if show_season_type == Some(2) {
                    // 番剧影视类型，使用简化命名，直接使用title（如"日配"、"中配"）
                    &title
                } else {
                    // 常规番剧类型，使用详细命名
                    share_copy
                        .as_ref()
                        .filter(|s| !s.is_empty() && s.len() > title.len()) // 只有当share_copy更详细时才使用
                        .map(|s| s.as_str())
                        .or(show_title.as_deref())
                        .unwrap_or(&title)
                };
                tracing::debug!("选择的intelligent_name: {}", intelligent_name);

                // 记录actors字段信息
                if actors.is_some() {
                    tracing::debug!("convert.rs - 准备保存的演员信息: {:?}", actors);
                }

                bili_sync_entity::video::ActiveModel {
                    bvid: Set(bvid),
                    name: Set(intelligent_name.to_string()),
                    intro: Set(intro),
                    cover: Set(cover),
                    pubtime: Set(pubtime.naive_utc()),
                    favtime: Set(pubtime.naive_utc()),
                    category: Set(1), // 番剧类型
                    valid: Set(true),
                    season_id: Set(Some(season_id)),
                    ep_id: Set(Some(ep_id)),
                    season_number: Set(season_number),
                    episode_number: Set(episode_number),
                    share_copy: Set(share_copy),
                    show_season_type: Set(show_season_type),
                    actors: Set(actors),
                    ..default
                }
            }
            _ => unreachable!(),
        }
    }

    /// 填充视频详情时调用，该方法会将视频详情附加到原有的 Model 上
    /// 特殊地，如果在检测视频更新时记录了 favtime，那么 favtime 会维持原样，否则会使用 pubtime 填充
    pub fn into_detail_model(self, base_model: bili_sync_entity::video::Model) -> bili_sync_entity::video::ActiveModel {
        match self {
            VideoInfo::Detail {
                title,
                bvid,
                intro,
                cover,
                upper,
                ctime,
                pubtime,
                state,
                show_title,
                staff,
                ..
            } => bili_sync_entity::video::ActiveModel {
                bvid: Set(bvid),
                // 如果原始model的name字段包含"第"并且看起来像番剧的show_title格式，则保留原来的name
                // 否则优先使用show_title，如果show_title为空则使用title
                name: if base_model.name.contains("第")
                    && (base_model.name.contains("话") || base_model.name.contains("集"))
                {
                    NotSet
                } else {
                    Set(show_title.unwrap_or(title))
                },
                category: Set(2),
                intro: Set(intro),
                cover: Set(cover),
                ctime: Set(ctime.naive_utc()),
                pubtime: Set(pubtime.naive_utc()),
                favtime: if base_model.favtime != NaiveDateTime::default() {
                    NotSet // 之前设置了 favtime，不覆盖
                } else {
                    Set(pubtime.naive_utc()) // 未设置过 favtime，使用 pubtime 填充
                },
                download_status: Set(0),
                valid: Set(state == 0),
                upper_id: Set(upper.mid),
                upper_name: Set(upper.name),
                upper_face: Set(upper.face),
                // 保存staff信息到数据库
                staff_info: Set(staff.map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))),
                ..base_model.into_active_model()
            },
            _ => unreachable!(),
        }
    }

    /// 获取视频的发布时间，用于对时间做筛选检查新视频
    pub fn release_datetime(&self) -> &DateTime<Utc> {
        match self {
            VideoInfo::Collection { pubtime: time, .. }
            | VideoInfo::Favorite { fav_time: time, .. }
            | VideoInfo::WatchLater { fav_time: time, .. }
            | VideoInfo::Submission { ctime: time, .. }
            | VideoInfo::Bangumi { pubtime: time, .. } => time,
            _ => unreachable!(),
        }
    }
}

impl PageInfo {
    pub fn into_active_model(
        self,
        video_model: &bili_sync_entity::video::Model,
    ) -> bili_sync_entity::page::ActiveModel {
        let (width, height) = match &self.dimension {
            Some(d) => {
                if d.rotate == 0 {
                    (Some(d.width), Some(d.height))
                } else {
                    (Some(d.height), Some(d.width))
                }
            }
            None => (None, None),
        };
        bili_sync_entity::page::ActiveModel {
            video_id: Set(video_model.id),
            cid: Set(self.cid),
            pid: Set(self.page),
            name: Set(self.name),
            width: Set(width),
            height: Set(height),
            duration: Set(self.duration),
            image: Set(self.first_frame),
            download_status: Set(0),
            ..Default::default()
        }
    }
}
