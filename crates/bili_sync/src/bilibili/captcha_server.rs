use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use tower_http::cors::CorsLayer;

use super::{CaptchaInfo, CaptchaResult};

/// 全局验证锁，防止多个验证服务器同时启动
static VERIFICATION_LOCK: Lazy<Mutex<Option<CaptchaResult>>> = Lazy::new(|| Mutex::new(None));

/// 验证码Web服务器状态
#[derive(Clone)]
pub struct CaptchaServerState {
    /// 当前验证码信息
    captcha_info: Arc<Mutex<Option<CaptchaInfo>>>,
    /// 验证结果发送器
    result_sender: Arc<Mutex<Option<oneshot::Sender<CaptchaResult>>>>,
}

impl CaptchaServerState {
    pub fn new() -> Self {
        Self {
            captcha_info: Arc::new(Mutex::new(None)),
            result_sender: Arc::new(Mutex::new(None)),
        }
    }
}

/// 验证码Web服务器
pub struct CaptchaWebServer {
    port: u16,
    state: CaptchaServerState,
}

impl CaptchaWebServer {
    /// 创建新的验证码服务器
    pub fn new(port: u16) -> Self {
        Self {
            port,
            state: CaptchaServerState::new(),
        }
    }

    /// 启动服务器并等待验证结果
    pub async fn start_and_wait_for_verification(&self, captcha_info: CaptchaInfo) -> Result<CaptchaResult> {
        // 获取全局验证锁，防止多个验证进程同时启动
        let mut global_lock = VERIFICATION_LOCK.lock().await;
        
        // 检查是否已有其他进程在验证中
        if global_lock.is_some() {
            tracing::info!("检测到已有验证进程在运行，等待其完成...");
            // 等待其他进程完成验证
            drop(global_lock); // 释放锁以便其他进程可以完成
            
            // 轮询等待验证完成
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let lock = VERIFICATION_LOCK.lock().await;
                if let Some(ref result) = *lock {
                    tracing::info!("其他验证进程已完成，使用其验证结果");
                    return Ok(result.clone());
                }
                // 如果锁为None，说明验证已清除或失败，继续当前验证
                if lock.is_none() {
                    break;
                }
            }
            global_lock = VERIFICATION_LOCK.lock().await;
        }
        
        tracing::info!("启动验证码Web服务器，端口: {}", self.port);

        // 设置验证码信息
        {
            let mut info = self.state.captcha_info.lock().await;
            *info = Some(captcha_info);
        }

        // 创建oneshot channel用于接收验证结果
        let (tx, rx) = oneshot::channel();
        {
            let mut sender = self.state.result_sender.lock().await;
            *sender = Some(tx);
        }

        // 创建路由
        let app = Router::new()
            .route("/", get(serve_captcha_page))
            .route("/captcha", get(serve_captcha_page))
            .route("/api/captcha-info", get(get_captcha_info))
            .route("/api/submit-result", post(submit_captcha_result))
            .route("/api/status", get(get_status))
            .layer(CorsLayer::permissive())
            .with_state(self.state.clone());

        // 启动服务器
        let listener = tokio::net::TcpListener::bind(&format!("127.0.0.1:{}", self.port)).await?;
        
        tracing::info!("验证码服务器已启动: http://127.0.0.1:{}", self.port);

        // 自动打开浏览器
        let url = format!("http://127.0.0.1:{}/captcha", self.port);
        if let Err(e) = webbrowser::open(&url) {
            tracing::warn!("无法自动打开浏览器: {}", e);
            tracing::info!("请手动打开浏览器访问: {}", url);
        } else {
            tracing::info!("已自动打开浏览器: {}", url);
        }

        // 启动服务器（在后台）
        let server_handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                tracing::error!("验证码服务器运行失败: {}", e);
            }
        });

        // 等待验证结果（带超时）
        let result = tokio::time::timeout(Duration::from_secs(300), rx).await;
        
        // 停止服务器
        server_handle.abort();

        let final_result = match result {
            Ok(Ok(captcha_result)) => {
                tracing::info!("验证码验证成功，已获取结果");
                
                // 将结果存储到全局锁中供其他进程使用
                *global_lock = Some(captcha_result.clone());
                
                // 设置一个定时器清理全局锁，避免长期占用
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    let mut lock = VERIFICATION_LOCK.lock().await;
                    *lock = None;
                    tracing::debug!("已清理全局验证锁");
                });
                
                Ok(captcha_result)
            }
            Ok(Err(_)) => {
                // 验证失败，清理全局锁
                *global_lock = None;
                Err(anyhow::anyhow!("验证码结果通道已关闭"))
            }
            Err(_) => {
                // 验证超时，清理全局锁
                *global_lock = None;
                Err(anyhow::anyhow!("验证码验证超时（5分钟）"))
            }
        };
        
        // 释放锁
        drop(global_lock);
        
        final_result
    }
}

/// 验证结果提交参数
#[derive(Debug, Deserialize)]
struct SubmitCaptchaParams {
    geetest_challenge: String,
    geetest_validate: String,
    geetest_seccode: String,
}

/// API响应
#[derive(Debug, Serialize)]
struct ApiResponse {
    success: bool,
    data: Option<serde_json::Value>,
    message: String,
}

impl ApiResponse {
    fn success<T: Serialize>(data: T) -> Self {
        Self {
            success: true,
            data: Some(serde_json::to_value(data).unwrap()),
            message: "操作成功".to_string(),
        }
    }

    fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            message,
        }
    }
}

/// 验证码页面
async fn serve_captcha_page() -> impl IntoResponse {
    Html(include_str!("captcha_page.html"))
}

/// 获取验证码信息API
async fn get_captcha_info(State(state): State<CaptchaServerState>) -> impl IntoResponse {
    let captcha_info = state.captcha_info.lock().await;
    
    if let Some(ref info) = *captcha_info {
        Json(ApiResponse::success(info))
    } else {
        Json(ApiResponse::error("验证码信息未找到".to_string()))
    }
}

/// 提交验证结果API
async fn submit_captcha_result(
    State(state): State<CaptchaServerState>,
    Query(params): Query<SubmitCaptchaParams>,
) -> impl IntoResponse {
    tracing::info!("收到验证码结果提交");

    // 获取原始验证码信息
    let captcha_info = {
        let info_guard = state.captcha_info.lock().await;
        match info_guard.clone() {
            Some(info) => info,
            None => {
                return Json(ApiResponse::error("验证码信息未找到".to_string()));
            }
        }
    };

    // 构造验证结果
    let captcha_result = CaptchaResult {
        challenge: params.geetest_challenge,
        token: captcha_info.token.clone(),
        validate: params.geetest_validate,
        seccode: params.geetest_seccode,
    };

    // 发送结果
    let mut sender_guard = state.result_sender.lock().await;
    if let Some(sender) = sender_guard.take() {
        if sender.send(captcha_result).is_ok() {
            tracing::info!("验证码结果已成功发送");
            Json(ApiResponse::success("验证完成，窗口将自动关闭"))
        } else {
            tracing::error!("验证码结果发送失败");
            Json(ApiResponse::error("结果发送失败".to_string()))
        }
    } else {
        Json(ApiResponse::error("验证已完成或已超时".to_string()))
    }
}

/// 获取服务器状态API
async fn get_status(State(state): State<CaptchaServerState>) -> impl IntoResponse {
    let info = state.captcha_info.lock().await;
    let sender = state.result_sender.lock().await;
    
    #[derive(Serialize)]
    struct StatusInfo {
        has_captcha: bool,
        waiting_for_result: bool,
    }
    
    let status = StatusInfo {
        has_captcha: info.is_some(),
        waiting_for_result: sender.is_some(),
    };
    
    Json(ApiResponse::success(status))
}