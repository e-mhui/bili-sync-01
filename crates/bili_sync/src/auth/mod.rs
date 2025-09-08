use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::bilibili::Credential;

#[derive(Debug, Serialize, Deserialize)]
pub struct QRCodeInfo {
    pub url: String,
    pub qrcode_key: String,
}

#[derive(Debug, Clone)]
pub enum LoginStatus {
    Pending,
    Scanned,
    Confirmed(LoginResult),
    Expired,
    Error(String),
}

#[derive(Debug, Clone)]
pub struct LoginResult {
    pub credential: Credential,
    pub user_info: UserInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct UserInfo {
    pub user_id: String,
    pub username: String,
    pub avatar_url: String,
}

#[derive(Debug)]
struct QRSession {
    qrcode_key: String,
    created_at: Instant,
    #[allow(dead_code)]
    status: LoginStatus,
}

/// 扫码登录服务
pub struct QRLoginService {
    client: Client,
    sessions: RwLock<HashMap<String, QRSession>>,
}

impl QRLoginService {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .cookie_store(true)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// 生成二维码
    pub async fn generate_qr_code(&self) -> Result<(String, QRCodeInfo)> {
        tracing::info!("开始调用B站API生成二维码");

        // 首先访问B站主页获取 buvid3
        tracing::info!("访问B站主页获取设备标识...");
        let _ = self.client
            .get("https://www.bilibili.com")
            .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .send()
            .await
            .map_err(|e| {
                tracing::warn!("访问B站主页失败，继续尝试生成二维码: {}", e);
                e
            });

        // 调用B站API生成二维码
        let response = self
            .client
            .get("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
            .header("Referer", "https://www.bilibili.com")
            .header("Origin", "https://www.bilibili.com")
            .send()
            .await
            .map_err(|e| {
                tracing::error!("请求B站API失败: {}", e);
                e
            })?;

        let status = response.status();
        tracing::info!("B站API响应状态: {}", status);

        let data: serde_json::Value = response.json().await.map_err(|e| {
            tracing::error!("解析B站API响应失败: {}", e);
            e
        })?;

        tracing::info!("B站API响应数据: {}", data);

        if data["code"].as_i64() != Some(0) {
            let error_msg = format!(
                "B站API返回错误: code={}, message={}",
                data["code"].as_i64().unwrap_or(-1),
                data["message"].as_str().unwrap_or("Unknown error")
            );
            tracing::error!("{}", error_msg);
            return Err(anyhow::anyhow!(error_msg));
        }

        let qr_info = QRCodeInfo {
            url: data["data"]["url"].as_str().unwrap().to_string(),
            qrcode_key: data["data"]["qrcode_key"].as_str().unwrap().to_string(),
        };

        // 生成会话ID并存储
        let session_id = Uuid::new_v4().to_string();
        let session = QRSession {
            qrcode_key: qr_info.qrcode_key.clone(),
            created_at: Instant::now(),
            status: LoginStatus::Pending,
        };

        self.sessions.write().await.insert(session_id.clone(), session);

        // 打印当前存储的所有会话
        {
            let sessions = self.sessions.read().await;
            tracing::info!("当前会话数量: {}", sessions.len());
            for (id, _) in sessions.iter() {
                tracing::info!("存储的会话ID: {}", id);
            }
        }

        Ok((session_id, qr_info))
    }

    /// 轮询登录状态
    pub async fn poll_login_status(&self, session_id: &str) -> Result<LoginStatus> {
        tracing::info!("轮询登录状态: session_id={}", session_id);

        // 打印当前存储的所有会话
        {
            let sessions = self.sessions.read().await;
            tracing::info!("轮询时会话数量: {}", sessions.len());
            for (id, _) in sessions.iter() {
                tracing::info!("存储的会话ID: {}", id);
            }
        }

        let sessions = self.sessions.read().await;
        let session = sessions.get(session_id).ok_or_else(|| {
            tracing::error!("找不到会话: session_id={}", session_id);
            anyhow::anyhow!("Session not found")
        })?;

        // 检查会话是否过期 (3分钟)
        if session.created_at.elapsed().as_secs() > 180 {
            return Ok(LoginStatus::Expired);
        }

        // 调用B站API检查状态
        let response = self
            .client
            .get("https://passport.bilibili.com/x/passport-login/web/qrcode/poll")
            .query(&[("qrcode_key", &session.qrcode_key)])
            .header("Referer", "https://www.bilibili.com")
            .header("Origin", "https://www.bilibili.com")
            .send()
            .await?;

        // 先提取headers
        let headers = response.headers().clone();
        let data: serde_json::Value = response.json().await?;

        match data["data"]["code"].as_i64() {
            Some(0) => {
                // 登录成功，提取凭证
                let cookies = self.extract_cookies_from_headers(&headers)?;

                // 如果响应头中没有 buvid3，尝试从用户信息接口获取
                let mut buvid3 = cookies.get("buvid3").cloned().unwrap_or_default();

                let user_info = self.get_user_info(&cookies).await?;

                // 如果还是没有 buvid3，尝试从之前访问主页时获取的cookie中查找
                if buvid3.is_empty() {
                    tracing::warn!("登录响应中未找到 buvid3");
                    // 从当前配置中获取（如果有的话）
                    let current_config = crate::config::reload_config();
                    if let Some(current_cred) = current_config.credential.load().as_ref() {
                        if !current_cred.buvid3.is_empty() {
                            buvid3 = current_cred.buvid3.clone();
                            tracing::info!("使用现有配置中的 buvid3");
                        }
                    }

                    // 如果还是没有，尝试生成一个新的 buvid3
                    if buvid3.is_empty() {
                        // 访问 B站的 buvid3 生成接口
                        match self.generate_buvid3().await {
                            Ok(new_buvid3) => {
                                buvid3 = new_buvid3;
                                tracing::info!("成功生成新的 buvid3");
                            }
                            Err(e) => {
                                tracing::warn!("生成 buvid3 失败: {}，将使用空值", e);
                            }
                        }
                    }
                }

                let credential = Credential {
                    sessdata: cookies.get("SESSDATA").unwrap().clone(),
                    bili_jct: cookies.get("bili_jct").unwrap().clone(),
                    buvid3: buvid3.clone(),
                    dedeuserid: cookies.get("DedeUserID").unwrap().clone(),
                    ac_time_value: data["data"]["refresh_token"].as_str().unwrap_or("").to_string(),
                    buvid4: buvid3, // 暂时使用buvid3作为buvid4
                    dedeuserid_ckmd5: cookies.get("DedeUserID__ckMd5").cloned(),
                };

                let login_result = LoginResult { credential, user_info };

                Ok(LoginStatus::Confirmed(login_result))
            }
            Some(86038) => Ok(LoginStatus::Expired),
            Some(86090) => Ok(LoginStatus::Scanned),
            Some(86101) => Ok(LoginStatus::Pending),
            _ => Ok(LoginStatus::Error(
                data["data"]["message"].as_str().unwrap_or("Unknown error").to_string(),
            )),
        }
    }

    /// 从响应头中提取Cookie
    fn extract_cookies_from_headers(&self, headers: &reqwest::header::HeaderMap) -> Result<HashMap<String, String>> {
        let mut cookies = HashMap::new();

        // 从响应头中提取Set-Cookie
        for header_value in headers.get_all("set-cookie").iter() {
            if let Ok(cookie_str) = header_value.to_str() {
                // 解析cookie
                for cookie_part in cookie_str.split(';') {
                    if let Some((key, value)) = cookie_part.split_once('=') {
                        let key = key.trim();
                        let value = value.trim();

                        if ["SESSDATA", "bili_jct", "DedeUserID", "DedeUserID__ckMd5", "buvid3"].contains(&key) {
                            cookies.insert(key.to_string(), value.to_string());
                        }
                    }
                }
            }
        }

        // 确保必要的cookie都存在
        let required_cookies = ["SESSDATA", "bili_jct", "DedeUserID"];
        for &required in &required_cookies {
            if !cookies.contains_key(required) {
                return Err(anyhow::anyhow!("Missing required cookie: {}", required));
            }
        }

        Ok(cookies)
    }

    /// 获取用户信息
    async fn get_user_info(&self, cookies: &HashMap<String, String>) -> Result<UserInfo> {
        // 构建cookie字符串
        let cookie_str = cookies
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join("; ");

        let response = self
            .client
            .get("https://api.bilibili.com/x/web-interface/nav")
            .header("Cookie", cookie_str)
            .send()
            .await?;

        let data: serde_json::Value = response.json().await?;

        if data["code"].as_i64() != Some(0) {
            return Err(anyhow::anyhow!(
                "Failed to get user info: {}",
                data["message"].as_str().unwrap_or("Unknown error")
            ));
        }

        let user_data = &data["data"];
        Ok(UserInfo {
            user_id: user_data["mid"].as_i64().unwrap_or(0).to_string(),
            username: user_data["uname"].as_str().unwrap_or("").to_string(),
            avatar_url: user_data["face"].as_str().unwrap_or("").to_string(),
        })
    }

    /// 清理过期会话
    #[allow(dead_code)]
    pub async fn cleanup_expired_sessions(&self) {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| {
            session.created_at.elapsed().as_secs() < 300 // 5分钟
        });
    }

    /// 生成 buvid3
    async fn generate_buvid3(&self) -> Result<String> {
        tracing::info!("尝试生成 buvid3...");

        // 方法1：访问 B站的 buvid3 生成接口
        let response = self
            .client
            .get("https://api.bilibili.com/x/frontend/finger/spi")
            .header("Referer", "https://www.bilibili.com")
            .header("Origin", "https://www.bilibili.com")
            .send()
            .await?;

        let data: serde_json::Value = response.json().await?;

        if data["code"].as_i64() == Some(0) {
            if let Some(buvid3) = data["data"]["b_3"].as_str() {
                tracing::info!("从 spi 接口获取到 buvid3: {}", buvid3);
                return Ok(buvid3.to_string());
            }
        }

        // 方法2：如果上面失败，尝试生成一个随机的 buvid3
        // B站的 buvid3 格式通常是：大写字母和数字的组合，长度约35个字符
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let charset: &[u8] = b"0123456789ABCDEF";
        let buvid3: String = (0..35)
            .map(|_| {
                let idx = rng.gen_range(0..charset.len());
                charset[idx] as char
            })
            .collect();

        tracing::info!("生成随机 buvid3: {}", buvid3);
        Ok(buvid3)
    }
}

impl Default for QRLoginService {
    fn default() -> Self {
        Self::new()
    }
}
