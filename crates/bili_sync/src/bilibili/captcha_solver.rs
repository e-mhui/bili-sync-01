use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::config::AutoSolveConfig;
use super::{CaptchaInfo, CaptchaResult, GeetestInfo};

/// 验证码识别服务类型
#[derive(Debug, Clone)]
pub enum CaptchaService {
    TwoCaptcha,
    AntiCaptcha,
    CapSolver,
    YunMa,
}

impl From<&str> for CaptchaService {
    fn from(s: &str) -> Self {
        match s {
            "2captcha" => CaptchaService::TwoCaptcha,
            "anticaptcha" => CaptchaService::AntiCaptcha,
            "capsolver" => CaptchaService::CapSolver,
            "yunma" => CaptchaService::YunMa,
            _ => CaptchaService::TwoCaptcha, // 默认使用2captcha
        }
    }
}

/// 验证码识别器
pub struct CaptchaSolver {
    config: AutoSolveConfig,
    client: reqwest::Client,
}

impl CaptchaSolver {
    pub fn new(config: AutoSolveConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.solve_timeout + 30))
            .user_agent("bili-sync/2.7.6")
            .build()
            .unwrap_or_default();

        Self { config, client }
    }

    /// 解决极验验证码
    pub async fn solve_geetest(&self, geetest_info: &GeetestInfo, page_url: &str) -> Result<CaptchaResult> {
        let service = CaptchaService::from(self.config.service.as_str());
        
        let mut last_error = None;
        
        for attempt in 1..=self.config.max_retries {
            tracing::info!("验证码识别尝试 {}/{}", attempt, self.config.max_retries);
            
            let result = match service {
                CaptchaService::TwoCaptcha => self.solve_with_2captcha(geetest_info, page_url).await,
                CaptchaService::AntiCaptcha => self.solve_with_anticaptcha(geetest_info, page_url).await,
                CaptchaService::CapSolver => self.solve_with_capsolver(geetest_info, page_url).await,
                CaptchaService::YunMa => self.solve_with_yunma(geetest_info, page_url).await,
            };
            
            match result {
                Ok(captcha_result) => {
                    tracing::info!("验证码识别成功，尝试次数: {}", attempt);
                    return Ok(captcha_result);
                }
                Err(e) => {
                    last_error = Some(e);
                    tracing::warn!("验证码识别失败，尝试次数: {}, 错误: {}", attempt, last_error.as_ref().unwrap());
                    
                    if attempt < self.config.max_retries {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                    }
                }
            }
        }
        
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("验证码识别失败")))
    }

    /// 使用2Captcha服务
    async fn solve_with_2captcha(&self, geetest_info: &GeetestInfo, page_url: &str) -> Result<CaptchaResult> {
        tracing::info!("使用2Captcha服务解决GeeTest验证码");
        
        // 1. 提交验证码任务
        let submit_data = json!({
            "method": "geetest",
            "gt": geetest_info.gt,
            "challenge": geetest_info.challenge,
            "pageurl": page_url,
            "key": self.config.api_key,
            "json": 1
        });

        let submit_response: serde_json::Value = self.client
            .post("http://2captcha.com/in.php")
            .form(&submit_data)
            .send()
            .await?
            .json()
            .await?;

        if submit_response["status"].as_i64() != Some(1) {
            anyhow::bail!("2Captcha提交失败: {}", submit_response["error_text"].as_str().unwrap_or("未知错误"));
        }

        let captcha_id = submit_response["request"].as_str()
            .ok_or_else(|| anyhow::anyhow!("无法获取验证码ID"))?;

        tracing::info!("验证码任务已提交，ID: {}", captcha_id);

        // 2. 等待并获取结果
        let start_time = std::time::Instant::now();
        let timeout = Duration::from_secs(self.config.solve_timeout);

        loop {
            if start_time.elapsed() > timeout {
                anyhow::bail!("验证码识别超时");
            }

            tokio::time::sleep(Duration::from_secs(5)).await;

            let result_response: serde_json::Value = self.client
                .get("http://2captcha.com/res.php")
                .query(&[
                    ("key", self.config.api_key.as_str()),
                    ("action", "get"),
                    ("id", captcha_id),
                    ("json", "1"),
                ])
                .send()
                .await?
                .json()
                .await?;

            if result_response["status"].as_i64() == Some(1) {
                let request = result_response["request"].as_str()
                    .ok_or_else(|| anyhow::anyhow!("无法获取验证结果"))?;

                // 解析结果：challenge:validate:seccode
                let parts: Vec<&str> = request.split(':').collect();
                if parts.len() != 3 {
                    anyhow::bail!("验证结果格式错误");
                }

                return Ok(CaptchaResult {
                    challenge: parts[0].to_string(),
                    validate: parts[1].to_string(),
                    seccode: format!("{}|jordan", parts[1]), // seccode格式通常是 validate|jordan
                    token: geetest_info.challenge.clone(),
                });
            } else if result_response["error_text"].as_str() != Some("CAPCHA_NOT_READY") {
                anyhow::bail!("2Captcha识别失败: {}", result_response["error_text"].as_str().unwrap_or("未知错误"));
            }
        }
    }

    /// 使用AntiCaptcha服务
    async fn solve_with_anticaptcha(&self, geetest_info: &GeetestInfo, page_url: &str) -> Result<CaptchaResult> {
        tracing::info!("使用AntiCaptcha服务解决GeeTest验证码");
        
        // 1. 创建任务
        let create_task_data = json!({
            "clientKey": self.config.api_key,
            "task": {
                "type": "GeeTestTaskProxyless",
                "websiteURL": page_url,
                "gt": geetest_info.gt,
                "challenge": geetest_info.challenge
            }
        });

        let create_response: serde_json::Value = self.client
            .post("https://api.anti-captcha.com/createTask")
            .json(&create_task_data)
            .send()
            .await?
            .json()
            .await?;

        if create_response["errorId"].as_i64() != Some(0) {
            anyhow::bail!("AntiCaptcha创建任务失败: {}", create_response["errorDescription"].as_str().unwrap_or("未知错误"));
        }

        let task_id = create_response["taskId"].as_i64()
            .ok_or_else(|| anyhow::anyhow!("无法获取任务ID"))?;

        tracing::info!("验证码任务已创建，ID: {}", task_id);

        // 2. 获取结果
        let start_time = std::time::Instant::now();
        let timeout = Duration::from_secs(self.config.solve_timeout);

        loop {
            if start_time.elapsed() > timeout {
                anyhow::bail!("验证码识别超时");
            }

            tokio::time::sleep(Duration::from_secs(5)).await;

            let result_data = json!({
                "clientKey": self.config.api_key,
                "taskId": task_id
            });

            let result_response: serde_json::Value = self.client
                .post("https://api.anti-captcha.com/getTaskResult")
                .json(&result_data)
                .send()
                .await?
                .json()
                .await?;

            if result_response["errorId"].as_i64() != Some(0) {
                anyhow::bail!("AntiCaptcha获取结果失败: {}", result_response["errorDescription"].as_str().unwrap_or("未知错误"));
            }

            if result_response["status"].as_str() == Some("ready") {
                let solution = &result_response["solution"];
                
                return Ok(CaptchaResult {
                    challenge: solution["challenge"].as_str().unwrap_or("").to_string(),
                    validate: solution["validate"].as_str().unwrap_or("").to_string(),
                    seccode: solution["seccode"].as_str().unwrap_or("").to_string(),
                    token: geetest_info.challenge.clone(),
                });
            }
        }
    }

    /// 使用CapSolver服务
    async fn solve_with_capsolver(&self, geetest_info: &GeetestInfo, page_url: &str) -> Result<CaptchaResult> {
        tracing::info!("使用CapSolver服务解决GeeTest验证码");
        
        // CapSolver API实现类似，这里暂时返回错误
        anyhow::bail!("CapSolver服务暂未实现");
    }

    /// 使用云码服务
    async fn solve_with_yunma(&self, geetest_info: &GeetestInfo, page_url: &str) -> Result<CaptchaResult> {
        tracing::info!("使用云码服务解决GeeTest验证码");
        
        // 云码API实现，这里暂时返回错误
        anyhow::bail!("云码服务暂未实现");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_service_from_str() {
        assert!(matches!(CaptchaService::from("2captcha"), CaptchaService::TwoCaptcha));
        assert!(matches!(CaptchaService::from("anticaptcha"), CaptchaService::AntiCaptcha));
        assert!(matches!(CaptchaService::from("capsolver"), CaptchaService::CapSolver));
        assert!(matches!(CaptchaService::from("yunma"), CaptchaService::YunMa));
        assert!(matches!(CaptchaService::from("unknown"), CaptchaService::TwoCaptcha));
    }
}