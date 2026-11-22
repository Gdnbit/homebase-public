//! Typed, payload-free model diagnostics and bounded inference-only retries.
use crate::control_protocol::ControlError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const PREFIX: &str = "HB_MODEL_FAILURE:";
pub const TOTAL_SECONDS: u64 = 120;
const MAX_ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Build,
    Connect,
    ConnectTimeout,
    Timeout,
    Send,
    Http,
    Provider,
    Read,
    InvalidResponse,
    Limit,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Acceptance {
    NotSubmitted,
    Rejected,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Detail {
    Dns,
    Tls,
    Refused,
    Reset,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub kind: Kind,
    pub detail: Option<Detail>,
    pub http_status: Option<u16>,
    pub provider_status: Option<u16>,
    pub elapsed_ms: u64,
}
impl From<&Failure> for Attempt {
    fn from(f: &Failure) -> Self {
        Self {
            kind: f.kind.clone(),
            detail: f.detail.clone(),
            http_status: f.http_status,
            provider_status: f.provider_status,
            elapsed_ms: f.elapsed_ms,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    #[serde(default)]
    pub previous: Vec<Attempt>,
    pub kind: Kind,
    pub acceptance: Acceptance,
    pub detail: Option<Detail>,
    pub http_status: Option<u16>,
    pub provider_status: Option<u16>,
    pub retry_after_ms: Option<u64>,
    pub attempts: u32,
    pub elapsed_ms: u64,
}
impl Failure {
    pub fn http(status: u16) -> Self {
        Self {
            previous: vec![],
            kind: Kind::Http,
            acceptance: Acceptance::Rejected,
            detail: None,
            http_status: Some(status),
            provider_status: None,
            retry_after_ms: None,
            attempts: 1,
            elapsed_ms: 0,
        }
    }
    pub fn unknown(kind: Kind) -> Self {
        Self {
            previous: vec![],
            kind,
            acceptance: Acceptance::Unknown,
            detail: None,
            http_status: None,
            provider_status: None,
            retry_after_ms: None,
            attempts: 1,
            elapsed_ms: 0,
        }
    }
    pub fn safe_to_resume(&self) -> bool {
        self.acceptance != Acceptance::Unknown
    }
    pub fn provider(status: u16) -> Self {
        Self {
            kind: Kind::Provider,
            http_status: Some(200),
            provider_status: Some(status),
            ..Self::http(status)
        }
    }
    pub fn transient(&self) -> bool {
        matches!(self.kind, Kind::Connect | Kind::ConnectTimeout)
            && self.detail != Some(Detail::Tls)
            || self.kind == Kind::Http
                && matches!(self.http_status, Some(408 | 429 | 500 | 502 | 503 | 504))
    }
    pub fn encode(&self) -> String {
        format!(
            "{PREFIX}{}",
            serde_json::to_string(self).expect("safe model failure")
        )
    }
    pub fn decode(text: &str) -> Option<Self> {
        let v: Self = serde_json::from_str(text.strip_prefix(PREFIX)?).ok()?;
        if !(1..=MAX_ATTEMPTS).contains(&v.attempts)
            || v.previous.len() > 2
            || v.previous.iter().any(|a| {
                a.elapsed_ms > 600_000
                    || a.http_status.is_some_and(|s| !(100..=599).contains(&s))
                    || a.provider_status.is_some_and(|s| !(400..=599).contains(&s))
            })
            || v.elapsed_ms > 600_000
            || v.retry_after_ms.is_some_and(|n| n > 86_400_000)
            || v.http_status.is_some_and(|s| !(100..=599).contains(&s))
            || v.provider_status.is_some_and(|s| !(400..=599).contains(&s))
        {
            return None;
        }
        // Never let a malformed/fabricated diagnostic downgrade an uncertain send.
        let valid = match v.kind {
            Kind::Build | Kind::Connect | Kind::ConnectTimeout => {
                v.acceptance == Acceptance::NotSubmitted && v.http_status.is_none()
            }
            Kind::Http => {
                v.acceptance == Acceptance::Rejected
                    && v.http_status.is_some_and(|s| s >= 400)
                    && v.provider_status.is_none()
            }
            Kind::Provider => {
                v.acceptance == Acceptance::Rejected
                    && v.http_status == Some(200)
                    && v.provider_status.is_some()
            }
            _ => v.acceptance == Acceptance::Unknown && v.http_status.is_none(),
        };
        valid.then_some(v)
    }
    pub fn code(&self) -> &'static str {
        match self.kind {
            Kind::Build => "MODEL_REQUEST_INVALID",
            Kind::Connect => "MODEL_CONNECT_FAILED",
            Kind::ConnectTimeout => "MODEL_CONNECT_TIMEOUT",
            Kind::Timeout => "MODEL_RESPONSE_TIMEOUT",
            Kind::Send => "MODEL_TRANSPORT_UNKNOWN",
            Kind::Read => "MODEL_RESPONSE_INTERRUPTED",
            Kind::InvalidResponse => "MODEL_RESPONSE_INVALID",
            Kind::Limit => "MODEL_RESPONSE_LIMIT",
            Kind::Http | Kind::Provider => match self.provider_status.or(self.http_status) {
                Some(401 | 403) => "MODEL_AUTH_REJECTED",
                Some(402) => "MODEL_QUOTA_REJECTED",
                Some(429) => "MODEL_RATE_LIMITED",
                Some(500..=599) => "MODEL_PROVIDER_UNAVAILABLE",
                _ => "MODEL_HTTP_REJECTED",
            },
        }
    }
    pub fn message(&self) -> String {
        let cause = match self.kind {
            Kind::Build => "请求格式无效，未发送",
            Kind::Connect => "模型连接失败，尚未提交推理",
            Kind::ConnectTimeout => "建立模型连接超时，尚未提交推理",
            Kind::Timeout => "等待模型响应超时，可能已被受理",
            Kind::Send => "模型传输中断，是否受理未知",
            Kind::Read => "模型响应读取中断，结果不完整",
            Kind::InvalidResponse => "模型响应无效，不能用作执行依据",
            Kind::Limit => "模型响应超过大小上限，未把截断内容当作结果",
            Kind::Provider => "模型服务在响应正文中明确返回错误",
            Kind::Http => "模型服务明确拒绝或返回失败状态",
        };
        format!(
            "{}{}；已尝试{}次。{}",
            cause,
            self.http_status
                .map(|s| format!("（HTTP {s}）"))
                .unwrap_or_default(),
            self.attempts,
            if self.safe_to_resume() {
                "已完成步骤保留，可继续原任务；不会重做已有操作。"
            } else {
                "不会自动重试或重复执行，请先核对原任务。"
            }
        )
    }
    pub fn control_error(&self) -> ControlError {
        let mut e = ControlError::new(self.code(), &self.message());
        e.details = Some(json!(self));
        e
    }
}
pub fn transport_error(error: &reqwest::Error) -> Failure {
    if error.is_builder() {
        return Failure {
            kind: Kind::Build,
            acceptance: Acceptance::NotSubmitted,
            ..Failure::unknown(Kind::Build)
        };
    }
    if error.is_connect() {
        let mut failure = Failure {
            kind: if error.is_timeout() {
                Kind::ConnectTimeout
            } else {
                Kind::Connect
            },
            acceptance: Acceptance::NotSubmitted,
            ..Failure::unknown(Kind::Connect)
        };
        let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
        while let Some(value) = cause {
            let text = value.to_string().to_ascii_lowercase();
            failure.detail = if text.contains("certificate") || text.contains("tls") {
                Some(Detail::Tls)
            } else if text.contains("dns") || text.contains("resolve") {
                Some(Detail::Dns)
            } else if text.contains("refused") {
                Some(Detail::Refused)
            } else if text.contains("reset") {
                Some(Detail::Reset)
            } else {
                None
            };
            if failure.detail.is_some() {
                break;
            }
            cause = value.source();
        }
        return failure;
    }
    Failure::unknown(if error.is_timeout() {
        Kind::Timeout
    } else {
        Kind::Send
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub attempts: u32,
    pub elapsed_ms: u64,
    pub retries: Vec<Failure>,
}
impl Report {
    pub fn checked(value: &Value) -> Option<Self> {
        let report: Self = serde_json::from_value(value.clone()).ok()?;
        if !(1..=MAX_ATTEMPTS).contains(&report.attempts)
            || report.retries.len() > 2
            || report.elapsed_ms > 600_000
            || report
                .retries
                .iter()
                .any(|f| Failure::decode(&f.encode()).is_none())
        {
            return None;
        }
        Some(report)
    }
}
fn retry_after(value: Option<&reqwest::header::HeaderValue>) -> Option<u64> {
    let text = value?.to_str().ok()?;
    if let Ok(seconds) = text.parse::<u64>() {
        return Some(seconds.min(86_400) * 1000);
    }
    let date = chrono::DateTime::parse_from_rfc2822(text).ok()?;
    Some(
        (date.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .num_milliseconds()
            .clamp(0, 86_400_000) as u64,
    )
}
fn delay(failure: &Failure, remaining: Duration) -> Option<Duration> {
    let jitter = (uuid::Uuid::new_v4().as_u128() % 250) as u64;
    let backoff = Duration::from_millis(
        failure
            .retry_after_ms
            .unwrap_or((1u64 << (failure.attempts - 1)) * 1000 + jitter),
    );
    (backoff + Duration::from_millis(50) < remaining).then_some(backoff)
}
/// Retry only an explicitly opted-in, stateless text/function inference request.
/// A send/body timeout or connection reset after submission is never replayed.
pub async fn send(
    request: reqwest::RequestBuilder,
    allow_retry: bool,
) -> Result<(reqwest::Response, Report), String> {
    send_limited(
        request,
        allow_retry,
        Duration::from_secs(TOTAL_SECONDS),
        Duration::from_secs(90),
    )
    .await
}
async fn send_limited(
    request: reqwest::RequestBuilder,
    allow_retry: bool,
    budget: Duration,
    attempt_timeout: Duration,
) -> Result<(reqwest::Response, Report), String> {
    let started = Instant::now();
    let mut retries: Vec<Failure> = vec![];
    for attempt in 1..=MAX_ATTEMPTS {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining < Duration::from_millis(50) {
            if let Some(mut failure) = retries.last().cloned() {
                failure.elapsed_ms = started.elapsed().as_millis() as u64;
                return Err(failure.encode());
            }
        }
        let builder = request.try_clone().ok_or_else(|| {
            Failure {
                kind: Kind::Build,
                acceptance: Acceptance::NotSubmitted,
                ..Failure::unknown(Kind::Build)
            }
            .encode()
        })?;
        let sent = builder.timeout(remaining.min(attempt_timeout)).send().await;
        let (mut failure, server_allows) = match sent {
            Ok(response) if response.status().is_success() => {
                return Ok((
                    response,
                    Report {
                        attempts: attempt,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        retries,
                    },
                ))
            }
            Ok(response) => {
                let mut f = Failure::http(response.status().as_u16());
                f.retry_after_ms = retry_after(response.headers().get("retry-after"));
                let allowed = response
                    .headers()
                    .get("x-should-retry")
                    .and_then(|v| v.to_str().ok())
                    != Some("false");
                (f, allowed)
            }
            Err(error) => (transport_error(&error), true),
        };
        failure.attempts = attempt;
        failure.elapsed_ms = started.elapsed().as_millis() as u64;
        failure.previous = retries.iter().map(Attempt::from).collect();
        if allow_retry && server_allows && failure.transient() && attempt < MAX_ATTEMPTS {
            if let Some(wait) = delay(&failure, budget.saturating_sub(started.elapsed())) {
                retries.push(failure);
                tokio::time::sleep(wait).await;
                continue;
            }
        }
        return Err(failure.encode());
    }
    unreachable!()
}
pub fn retry_eligible(
    provider: &str,
    protocol: &str,
    route: &str,
    input: &Value,
    has_output_root: bool,
) -> bool {
    provider == "openrouter"
        && protocol == "chat"
        && route.ends_with(":free")
        && !has_output_root
        // Unknown extensions may execute server-side tools or create persistent resources.
        && input.as_object().is_some_and(|m|m.keys().all(|k|matches!(k.as_str(),"model"|"messages"|"tools"|"tool_choice"|"max_tokens"|"max_completion_tokens"|"provider"|"stream"|"modalities"|"temperature"|"top_p"|"top_k"|"stop"|"seed"|"response_format"|"reasoning"|"parallel_tool_calls"|"frequency_penalty"|"presence_penalty")))
        && input["provider"]["allow_fallbacks"] == false
        && input["provider"]["max_price"]
            .as_object()
            .is_some_and(|p| !p.is_empty() && p.values().all(|v| v.as_f64() == Some(0.0)))
        && input["stream"] != true
        && (input["modalities"].is_null() || input["modalities"] == json!(["text"]))
        && input.get("tools").map_or(true, |v| {
            v.as_array()
                .is_some_and(|a| a.iter().all(|t| t["type"] == "function"))
        })
}
pub fn text_inference(functions: &[String]) -> bool {
    functions.iter().any(|f| f == "text")
        && !functions.iter().any(|f| {
            matches!(
                f.as_str(),
                "image" | "video" | "audio_generation" | "tts" | "music" | "sound_effects" | "3d"
            )
        })
}

#[cfg(test)]
#[path = "model_reliability_tests.rs"]
mod tests;
