//! Bounded server-sent events for the supported desktop model routes.
use super::reliability::{Failure, Kind};
use crate::assistant_protocol::{self, ModelReply, ModelRequest};
use crate::model_capabilities::ModelCapability;
use serde_json::{json, Value};

#[path = "model_chat_stream.rs"]
mod chat;
#[path = "model_responses_stream.rs"]
mod responses;
#[cfg(test)]
pub(crate) use responses::test_frames;

const MAX_CHARS: usize = 12_000;
const MAX_EVENT: usize = 128 * 1024;
const MAX_WIRE: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Protocol {
    Chat,
    ChatTools,
    Responses,
    ResponsesTools,
}

impl Protocol {
    pub(crate) fn for_route(route: &ModelCapability, request: &ModelRequest) -> Option<Self> {
        if request.vision.is_some() || !route.functions.iter().any(|function| function == "text") {
            return None;
        }
        let has_tools = !request.definitions.as_ref()?.is_empty();
        match (route.provider_id.as_str(), route.protocol.as_str(), has_tools) {
            ("openrouter" | "deepseek", "chat", false) => Some(Self::Chat),
            ("openai", "responses", false) => Some(Self::Responses),
            ("deepseek", "chat", true)
                if matches!(route.model_id.as_str(), "deepseek-flash" | "deepseek-v4-pro") => Some(Self::ChatTools),
            ("openai", "responses", true)
                if matches!(route.model_id.as_str(), "gpt-4o-mini" | "gpt-5-mini" | "gpt-5.4-mini") => Some(Self::ResponsesTools),
            _ => None,
        }
    }

    pub(crate) fn enable(self, provider: &str, body: &mut Value) {
        body["stream"] = json!(true);
        match self {
            Self::Chat if provider == "deepseek" => {
                body["stream_options"] = json!({"include_usage":true});
                body["thinking"] = json!({"type":"disabled"});
            }
            Self::ChatTools => {
                body["stream_options"] = json!({"include_usage":true});
            }
            Self::Responses => {
                if let Some(request) = body.as_object_mut() {
                    request.remove("include");
                }
            }
            Self::Chat | Self::ResponsesTools => {}
        }
    }

    pub(crate) async fn read(
        self,
        mut response: reqwest::Response,
        mut publish: impl FnMut(&str) + Send,
    ) -> Result<ModelReply, String> {
        if !response.status().is_success() {
            return Err(Failure::http(response.status().as_u16()).encode());
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        {
            return Err(invalid());
        }
        let mut sse = Sse::default();
        let mut chat = chat::Response::new(matches!(self, Self::ChatTools));
        let mut responses = responses::Response::new(matches!(self, Self::ResponsesTools));
        while let Some(chunk) = response.chunk().await.map_err(|err| {
            Failure::unknown(if err.is_timeout() {
                Kind::Timeout
            } else {
                Kind::Read
            })
            .encode()
        })? {
            let result = sse.push(&chunk, |name, data| match self {
                Self::Chat | Self::ChatTools => chat.event(data, &mut publish),
                Self::Responses | Self::ResponsesTools => responses.event(name, data, &mut publish),
            })?;
            if let Some(reply) = result {
                return Ok(reply);
            }
        }
        Err(invalid())
    }
}

fn invalid() -> String {
    Failure::unknown(Kind::InvalidResponse).encode()
}

fn limit() -> String {
    Failure::unknown(Kind::Limit).encode()
}

#[derive(Default)]
struct Sse {
    line: Vec<u8>,
    name: Vec<u8>,
    data: Vec<u8>,
    received: usize,
}

impl Sse {
    fn push(
        &mut self,
        bytes: &[u8],
        mut event: impl FnMut(&[u8], &[u8]) -> Result<Option<ModelReply>, String>,
    ) -> Result<Option<ModelReply>, String> {
        self.received += bytes.len();
        if self.received > MAX_WIRE {
            return Err(limit());
        }
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            let complete = part.ends_with(b"\n");
            let part = part.strip_suffix(b"\n").unwrap_or(part);
            if self.line.len() + part.len() > MAX_EVENT {
                return Err(limit());
            }
            self.line.extend_from_slice(part);
            if !complete {
                continue;
            }
            let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    if let Some(reply) = event(&self.name, &self.data)? {
                        return Ok(Some(reply));
                    }
                }
                self.data.clear();
                self.name.clear();
            } else if let Some(name) = line.strip_prefix(b"event:") {
                self.name.clear();
                self.name
                    .extend_from_slice(name.strip_prefix(b" ").unwrap_or(name));
            } else if let Some(data) = line.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if self.data.len() + data.len() + 1 > MAX_EVENT {
                    return Err(limit());
                }
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(data);
            }
            self.line.clear();
        }
        Ok(None)
    }
}

#[derive(Default)]
struct Text {
    value: String,
    chars: usize,
}

impl Text {
    fn append(&mut self, delta: &str) -> Result<(), String> {
        self.chars += delta.chars().count();
        if self.value.len() + delta.len() > assistant_protocol::MAX_REPLY_BYTES
            || self.chars > MAX_CHARS
        {
            return Err(limit());
        }
        self.value.push_str(delta);
        Ok(())
    }

    fn reply(&self, model: &Value, usage: &Value) -> Result<ModelReply, String> {
        assistant_protocol::parse(
            "chat",
            &json!({"model":model,"usage":usage,"choices":[
                {"finish_reason":"stop","message":{"role":"assistant","content":self.value}}
            ]}),
        )
        .map_err(|_| invalid())
    }
}
