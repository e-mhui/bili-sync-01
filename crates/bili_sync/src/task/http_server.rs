use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Path, Request};
use axum::http::{header, Uri};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{middleware, Extension, Router, ServiceExt};
use reqwest::StatusCode;
use rust_embed::Embed;
use sea_orm::DatabaseConnection;
use utoipa::OpenApi;
use utoipa_swagger_ui::{Config, SwaggerUi};

use crate::api::auth;
use crate::api::handler::{
    add_video_source,
    batch_update_config_internal,
    check_initial_setup,
    delete_video,
    delete_video_source,
    get_bangumi_seasons,
    get_config,
    get_config_history,
    // 新增配置管理API
    get_config_item,
    get_hot_reload_status,
    get_logs,
    get_queue_status,
    get_submission_videos,
    get_subscribed_collections,
    get_task_control_status,
    get_user_collections,
    get_user_favorites,
    get_user_favorites_by_uid,
    get_user_followings,
    get_video,
    get_video_bvid,
    get_video_play_info,
    get_video_sources,
    get_videos,
    pause_scanning_endpoint,
    proxy_image,
    proxy_video_stream,
    reload_config,
    reload_config_new_internal,
    reset_all_videos,
    reset_specific_tasks,
    reset_video,
    reset_video_source_path,
    resume_scanning_endpoint,
    search_bilibili,
    setup_auth_token,
    update_config,
    update_config_item_internal,
    update_credential,
    generate_qr_code,
    poll_qr_status,
    get_current_user,
    get_dashboard_data,
    clear_credential,
    update_video_source_enabled,
    update_video_source_scan_deleted,
    update_video_status,
    validate_config,
    validate_favorite,
    ApiDoc,
};
use crate::api::request::{BatchUpdateConfigRequest, UpdateConfigItemRequest};
use crate::api::video_stream::stream_video;
use crate::api::wrapper::ApiResponse;
use crate::api::ws;
// CONFIG导入已移除 - 现在使用动态配置

#[derive(Embed)]
#[folder = "../../web/build"]
struct Asset;

pub async fn http_server(database_connection: Arc<DatabaseConnection>) -> Result<()> {
    let app = Router::new()
        .route("/api/video-sources", get(get_video_sources))
        .route("/api/video-sources", post(add_video_source))
        .route(
            "/api/video-sources/{source_type}/{id}/enabled",
            put(update_video_source_enabled),
        )
        .route(
            "/api/video-sources/{source_type}/{id}/scan-deleted",
            put(update_video_source_scan_deleted),
        )
        .route(
            "/api/video-sources/{source_type}/{id}/reset-path",
            post(reset_video_source_path),
        )
        .route("/api/video-sources/{source_type}/{id}", delete(delete_video_source))
        .route("/api/videos", get(get_videos))
        .route("/api/videos/{id}", get(get_video))
        .route("/api/videos/{id}", delete(delete_video))
        .route("/api/videos/{id}/reset", post(reset_video))
        .route("/api/videos/{id}/update-status", post(update_video_status))
        .route("/api/videos/reset-all", post(reset_all_videos))
        .route("/api/videos/reset-specific-tasks", post(reset_specific_tasks))
        .route("/api/dashboard", get(get_dashboard_data))
        .route("/api/reload-config", post(reload_config))
        .route("/api/config", get(get_config))
        .route("/api/config", put(update_config))
        // 新的配置管理API路由
        .route("/api/config/item/{key}", get(get_config_item))
        .route(
            "/api/config/item/{key}",
            put(
                |Path(key): Path<String>,
                 Extension(db): Extension<Arc<DatabaseConnection>>,
                 axum::Json(request): axum::Json<UpdateConfigItemRequest>| async move {
                    update_config_item_internal(db, key, request).await.map(ApiResponse::ok)
                },
            ),
        )
        .route(
            "/api/config/batch",
            post(
                |Extension(db): Extension<Arc<DatabaseConnection>>,
                 axum::Json(request): axum::Json<BatchUpdateConfigRequest>| async move {
                    batch_update_config_internal(db, request).await.map(ApiResponse::ok)
                },
            ),
        )
        .route(
            "/api/config/reload",
            post(|Extension(db): Extension<Arc<DatabaseConnection>>| async move {
                reload_config_new_internal(db).await.map(ApiResponse::ok)
            }),
        )
        .route("/api/config/history", get(get_config_history))
        .route("/api/config/validate", post(validate_config))
        .route("/api/config/hot-reload/status", get(get_hot_reload_status))
        // 初始设置API路由
        .route("/api/setup/check", get(check_initial_setup))
        .route("/api/setup/auth-token", post(setup_auth_token))
        .route("/api/credential", put(update_credential))
        // 扫码登录API路由
        .route("/api/auth/qr/generate", post(generate_qr_code))
        .route("/api/auth/qr/poll", get(poll_qr_status))
        .route("/api/auth/current-user", get(get_current_user))
        .route("/api/auth/clear-credential", post(clear_credential))
        .route("/api/bangumi/seasons/{season_id}", get(get_bangumi_seasons))
        .route("/api/search", get(search_bilibili))
        .route("/api/user/favorites", get(get_user_favorites))
        .route("/api/user/{uid}/favorites", get(get_user_favorites_by_uid))
        .route("/api/favorite/{fid}/validate", get(validate_favorite))
        .route("/api/user/collections/{mid}", get(get_user_collections))
        .route("/api/user/followings", get(get_user_followings))
        .route("/api/user/subscribed-collections", get(get_subscribed_collections))
        .route("/api/submission/{up_id}/videos", get(get_submission_videos))
        .route("/api/logs", get(get_logs))
        .route("/api/queue-status", get(get_queue_status))
        .route("/api/proxy/image", get(proxy_image))
        .route("/api/task-control/status", get(get_task_control_status))
        .route("/api/task-control/pause", post(pause_scanning_endpoint))
        .route("/api/task-control/resume", post(resume_scanning_endpoint))
        // 视频流API
        .route("/api/videos/stream/{video_id}", get(stream_video))
        // 新增在线播放API
        .route("/api/videos/{video_id}/play-info", get(get_video_play_info))
        .route("/api/videos/{video_id}/bvid", get(get_video_bvid))
        .route("/api/videos/proxy-stream", get(proxy_video_stream))
        // 先应用认证中间件
        .layer(Extension(database_connection))
        .layer(middleware::from_fn(auth::auth))
        // WebSocket API需要在认证中间件之后
        .merge(ws::router())
        .merge(
            SwaggerUi::new("/swagger-ui/")
                .url("/api-docs/openapi.json", ApiDoc::openapi())
                .config(
                    Config::default()
                        .try_it_out_enabled(true)
                        .persist_authorization(true)
                        .validator_url("none"),
                ),
        )
        .fallback_service(get(frontend_files));
    // 使用动态配置而非静态CONFIG
    let config = crate::config::reload_config();
    let listener = tokio::net::TcpListener::bind(&config.bind_address)
        .await
        .context("bind address failed")?;
    info!("开始运行管理页: http://{}", config.bind_address);
    Ok(axum::serve(listener, ServiceExt::<Request>::into_make_service(app)).await?)
}

async fn frontend_files(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }

    match Asset::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => {
            // 对于SPA路由，如果请求的路径不是文件（没有扩展名），则返回index.html
            if !path.contains('.') {
                match Asset::get("index.html") {
                    Some(content) => {
                        let mime = mime_guess::from_path("index.html").first_or_octet_stream();
                        ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
                    }
                    None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
                }
            } else {
                (StatusCode::NOT_FOUND, "404 Not Found").into_response()
            }
        }
    }
}
