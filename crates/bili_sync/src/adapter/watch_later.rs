use std::path::Path;
use std::pin::Pin;

use anyhow::{Context, Result};
use bili_sync_entity::*;
use chrono::Utc;
use futures::Stream;
use sea_orm::entity::prelude::*;
use sea_orm::sea_query::{OnConflict, SimpleExpr};
use sea_orm::ActiveValue::Set;
use sea_orm::{DatabaseConnection, Unchanged};

use crate::adapter::{VideoSource, VideoSourceEnum, _ActiveModel};
use crate::bilibili::{BiliClient, VideoInfo, WatchLater};

impl VideoSource for watch_later::Model {
    fn filter_expr(&self) -> SimpleExpr {
        video::Column::WatchLaterId.eq(self.id)
    }

    fn set_relation_id(&self, video_model: &mut video::ActiveModel) {
        video_model.watch_later_id = Set(Some(self.id));
    }

    fn path(&self) -> &Path {
        Path::new(self.path.as_str())
    }

    fn get_latest_row_at(&self) -> String {
        self.latest_row_at.clone()
    }

    fn update_latest_row_at(&self, datetime: String) -> _ActiveModel {
        _ActiveModel::WatchLater(watch_later::ActiveModel {
            id: Unchanged(self.id),
            latest_row_at: Set(datetime),
            ..Default::default()
        })
    }

    fn should_take(&self, _release_datetime: &chrono::DateTime<Utc>, _latest_row_at: &chrono::DateTime<Utc>) -> bool {
        // 修改稍后观看源，每次都全量拉取所有视频，不管时间戳
        true
    }

    fn log_refresh_video_start(&self) {
        info!("开始扫描稍后再看..");
    }

    fn log_refresh_video_end(&self, count: usize) {
        if count > 0 {
            info!("扫描稍后再看完成，获取到 {} 条新视频", count);
        } else {
            info!("稍后再看无新视频");
        }
    }

    fn log_fetch_video_start(&self) {
        debug!("开始填充稍后再看视频详情..");
    }

    fn log_fetch_video_end(&self) {
        debug!("填充稍后再看视频详情完成");
    }

    fn log_download_video_start(&self) {
        debug!("开始下载稍后再看视频..");
    }

    fn log_download_video_end(&self) {
        debug!("下载稍后再看视频完成");
    }

    fn scan_deleted_videos(&self) -> bool {
        self.scan_deleted_videos
    }

    fn source_type_display(&self) -> String {
        "稍后再看".to_string()
    }

    fn source_name_display(&self) -> String {
        "稍后再看".to_string()
    }
}

// 稍后观看源的初始化现在通过Web API完成，不再需要这个函数

pub(super) async fn watch_later_from<'a>(
    path: &Path,
    bili_client: &'a BiliClient,
    connection: &DatabaseConnection,
) -> Result<(
    VideoSourceEnum,
    Pin<Box<dyn Stream<Item = Result<VideoInfo>> + 'a + Send>>,
)> {
    // 检查是否在内存模式，如果是则使用内存数据库连接
    let optimized_conn = crate::utils::global_memory_optimizer::get_optimized_connection().await;
    let db_conn = if let Some(ref conn) = optimized_conn {
        debug!("使用内存数据库连接初始化稍后观看源");
        conn.as_ref()
    } else {
        connection
    };

    let watch_later = WatchLater::new(bili_client);
    watch_later::Entity::insert(watch_later::ActiveModel {
        id: Set(1),
        path: Set(path.to_string_lossy().to_string()),
        created_at: Set(crate::utils::time_format::now_standard_string()),
        latest_row_at: Set("1970-01-01 00:00:00".to_string()),
        enabled: Set(true),
        scan_deleted_videos: Set(false),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::column(watch_later::Column::Id)
            .update_column(watch_later::Column::Path)
            .to_owned(),
    )
    .exec(db_conn)
    .await?;
    Ok((
        watch_later::Entity::find()
            .filter(watch_later::Column::Id.eq(1))
            .one(db_conn)
            .await?
            .context("watch_later not found")?
            .into(),
        Box::pin(watch_later.into_video_stream()),
    ))
}
