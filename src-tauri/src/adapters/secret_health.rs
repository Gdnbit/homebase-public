//! Credential probes are explicit user actions. No redirect may carry a credential.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeResult {
    pub success: bool,
    pub status: String,
    pub response_ms: u32,
    pub http_code: Option<u16>,
    pub error: Option<String>,
}

pub async fn probe(url: &str, post: bool, header: &str, auth: &str) -> Result<ProbeResult, String> {
    let start = Instant::now();
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .user_agent("HomeBase/0.1 credential-check")
        .redirect(reqwest::redirect::Policy::none());
    let parsed = reqwest::Url::parse(url).map_err(|_| "检测地址无效")?;
    if matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|_| "无法建立检测客户端")?;
    let request = if post {
        client.post(url).json(&serde_json::json!({"query":"ping"}))
    } else {
        client.get(url)
    };
    let request = request.header("Accept", "application/json");
    let request = if header.is_empty() {
        request
    } else {
        request.header(header, auth)
    };
    match request.send().await {
        Ok(response) => {
            let code = response.status().as_u16();
            let success = (200..300).contains(&code);
            let status = match code {
                200..=299 => "valid",
                401 => "unauthorized",
                403 => "forbidden",
                404 => "not_found",
                422 => "invalid_request",
                300..=399 => "redirect_refused",
                _ => "unknown",
            };
            Ok(ProbeResult {
                success,
                status: status.into(),
                response_ms: start.elapsed().as_millis() as u32,
                http_code: Some(code),
                error: (!success).then(|| format!("服务返回 HTTP {code}；不能据此确认密钥有效")),
            })
        }
        Err(_) => Ok(ProbeResult {
            success: false,
            status: "network_error".into(),
            response_ms: start.elapsed().as_millis() as u32,
            http_code: None,
            error: Some("连接失败或超时；敏感请求信息未记录".into()),
        }),
    }
}
