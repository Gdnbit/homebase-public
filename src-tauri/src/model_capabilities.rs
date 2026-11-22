//! Model/channel/key routing. DTOs contain references and public model facts, never key material.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use crate::capability_facts::PriceInfo;
#[path = "model_vision.rs"]
pub(crate) mod vision;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSource {
    #[default]
    Default,
    Sops,
    ApiKeyManager,
}

pub fn parse_route_pins(
    bytes: &[u8],
) -> Result<std::collections::BTreeMap<String, ModelCapability>, String> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("所选模型元数据超过上限，未覆盖".into());
    }
    let pins: std::collections::BTreeMap<String, ModelCapability> =
        serde_json::from_slice(bytes).map_err(|_| "所选模型元数据无效，未覆盖")?;
    if pins.len() > 100
        || pins.iter().any(|(id, m)| {
            !valid_id(&m.model_id)
                || id != &m.route_id
                || *id != format!("{}::{}", m.provider_id, m.model_id)
                || !PROVIDERS.iter().any(|p| p.0 == m.provider_id)
                || !matches!(m.protocol.as_str(), "chat" | "responses")
                || endpoint(m, &Value::Null).is_err()
                || m.pricing.as_ref().is_some_and(|p|m.provider_id!="openrouter"||!p.valid_for(id))
        })
    {
        return Err("所选模型元数据路由校验失败，未使用或覆盖".into());
    }
    Ok(pins)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CapabilityCredential {
    pub provider_id: String,
    pub provider_name: String,
    pub credential_ref: String,
    pub source: String,
    pub state: String,
    pub message: String,
    pub remote_state: String,
    pub expires_at: Option<String>,
    pub expiry_warning: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelCapability {
    pub route_id: String,
    pub model_id: String,
    pub name: String,
    pub publisher: String,
    pub canonical_id: String,
    pub provider_id: String,
    pub functions: Vec<String>,
    pub protocol: String,
    pub source: String,
    pub source_url: String,
    pub account_state: String,
    pub input_schema: Value,
    pub input_example: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<PriceInfo>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct CapabilityFunction {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelCatalogSource {
    pub provider_id: String,
    pub state: String,
    pub message: String,
    pub fetched_at: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ServiceCapability {
    pub id: String,
    pub name: String,
    pub function: String,
    pub credential_ref: String,
    pub state: String,
    pub message: String,
}

/// Public service configuration metadata, not permission or a remote probe.
pub fn service_configuration_state(
    configured: bool,
    value_state: &str,
    expires_at: Option<&str>,
    now: i64,
) -> &'static str {
    if configured != (value_state == "filled") {
        return "unknown";
    }
    match value_state {
        "empty" | "missing" => "not_configured",
        "invalid" => "invalid",
        "filled" => match expires_at.map(chrono::DateTime::parse_from_rfc3339) {
            Some(Err(_)) => "invalid",
            Some(Ok(date)) if date.timestamp() <= now => "expired",
            _ => "configured",
        },
        _ => "unknown",
    }
}

#[cfg(test)]
mod service_status_tests {
    use super::service_configuration_state as state;
    const NOW: i64 = 1_800_000_000;
    #[test]
    fn known_positive_and_unconfigured_states() {
        assert_eq!(state(true, "filled", None, NOW), "configured");
        for v in ["empty", "missing"] {
            assert_eq!(state(false, v, None, NOW), "not_configured");
        }
    }
    #[test]
    fn invalid_and_unknown_values_are_not_reported_unconfigured() {
        assert_eq!(state(false, "invalid", None, NOW), "invalid");
        assert_eq!(state(false, "unrecognized_future_state", None, NOW), "unknown");
    }
    #[test]
    fn contradictory_metadata_is_unknown() {
        assert_eq!(state(false, "filled", None, NOW), "unknown");
        assert_eq!(state(true, "empty", None, NOW), "unknown");
    }
    #[test]
    fn expiry_parse_error_is_not_the_same_as_a_valid_expired_date() {
        assert_eq!(state(true, "filled", Some("invalid-date"), NOW), "invalid");
        let date = chrono::DateTime::from_timestamp(NOW, 0).unwrap().to_rfc3339();
        assert_eq!(state(true, "filled", Some(&date), NOW), "expired");
        assert_eq!(state(true, "filled", Some(&date), NOW - 1), "configured");
    }
}
#[derive(Serialize)]
pub struct ModelCapabilityCatalog {
    pub credentials: Vec<CapabilityCredential>,
    pub functions: Vec<CapabilityFunction>,
    pub models: Vec<ModelCapability>,
    pub services: Vec<ServiceCapability>,
    pub sources: Vec<ModelCatalogSource>,
    pub issues: Vec<String>,
    pub total_models: usize,
    pub next_offset: Option<usize>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilityRequest {
    /// 从 model_capabilities_catalog 的 models[].route_id 选择，绑定调用渠道而非模型品牌。
    pub route_id: String,
    /// 依照该 route.input_schema；不接受密钥、HTTP 地址、认证头或其他 model 覆盖。
    pub input: Value,
    /// 二进制音频或较大 JSON 结果必须指定专用输出目录；只创建新的 UUID 子目录。
    pub output_root: Option<String>,
    /// default 保持原优先级；sops/api_key_manager 为显式选择，失败不换来源。
    #[serde(default)]
    pub credential_source: CredentialSource,
}
#[derive(Serialize)]
pub struct ModelCapabilityPlan {
    pub route: ModelCapability,
    pub credential: CapabilityCredential,
    pub method: String,
    pub endpoint: String,
    pub request: Value,
    pub callable: bool,
    pub message: String,
}
#[derive(Serialize)]
pub struct ModelCapabilityArtifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub mime_type: String,
}
#[derive(Serialize)]
pub struct ModelCapabilityResult {
    pub provider_id: String,
    pub model_id: String,
    pub credential_ref: String,
    /// succeeded / running / queued / failed / unknown；queued/running 不等于生成完成。
    pub status: String,
    pub task_id: Option<String>,
    /// 厂商结果。任务结果含 URL 时需另外按用户范围核实；大结果见 artifact。
    pub output: Value,
    pub artifact: Option<ModelCapabilityArtifact>,
    pub message: String,
}

pub const PROVIDERS: &[(&str, &str, &str)] = &[
    ("openrouter", "OpenRouter", "openrouter_key"),
    ("openai", "OpenAI 直连", "openai_key"),
    ("fal", "fal.ai", "fal_key"),
    ("dashscope", "阿里云百炼 · 北京", "dashscope_key"),
    ("ark", "火山方舟", "ark_key"),
    ("minimax", "MiniMax · 中国区", "minimax_key"),
    ("elevenlabs", "ElevenLabs", "elevenlabs_key"),
    ("assemblyai", "AssemblyAI", "assemblyai_key"),
    ("deepgram", "Deepgram", "deepgram_key"),
    ("anthropic", "Anthropic 直连", "anthropic_key"),
    ("deepseek", "DeepSeek 直连", "deepseek_key"),
    ("gemini", "Gemini 直连", "gemini_key"),
    ("doubao", "豆包语音（独立凭据）", "doubao_speech_key"),
    ("tryallai", "TryAllAI 网关", "tryallai_key"),
];
pub fn binding_choice(
    source_known: bool,
    sops_state: Option<&str>,
    external: Option<(&str, &str)>,
    alias: &str,
    expired: bool,
    policy: &str,
) -> (String, String, String, String) {
    let (source, state, reference, message) =
        if let Some(state) = sops_state.filter(|s| *s != "missing") {
            let blocked = state != "filled" || (expired && policy == "block");
            (
                "sops",
                if blocked { "blocked" } else { "configured" },
                format!("sops:{alias}"),
                if blocked {
                    "现有 SOPS 条目不可用或已被到期策略阻断；不切换到其他来源"
                } else {
                    "已配置；远端认证、余额及模型权限未验证"
                },
            )
        } else if !source_known {
            (
                "unknown",
                "unknown",
                format!("sops:{alias}"),
                "SOPS 优先来源未知，不能推断其他来源可用",
            )
        } else if let Some((state, reference)) = external {
            (
                "dpapi_reference",
                if state == "linked" {
                    "configured"
                } else if state == "not_configured" {
                    "not_configured"
                } else {
                    "blocked"
                },
                reference.into(),
                if state == "linked" {
                    "本机引用可用；远端认证、余额及模型权限未验证"
                } else if state == "not_configured" {
                    "尚未配置该服务的独立凭据"
                } else {
                    "独立凭据被阻断；请检查密钥页的引用状态"
                },
            )
        } else {
            (
                "sops",
                "not_configured",
                format!("sops:{alias}"),
                "尚未配置该渠道密钥",
            )
        };
    (source.into(), state.into(), reference, message.into())
}
pub fn functions() -> Vec<CapabilityFunction> {
    [
        ("text", "文本与推理"),
        ("image", "图片生成"),
        ("video", "视频生成"),
        ("tts", "语音合成"),
        ("asr", "语音转写"),
        ("embedding", "向量检索"),
        ("vision", "图片理解"),
        ("video_understanding", "视频理解"),
        ("audio_understanding", "音频理解"),
        ("audio_generation", "音频生成"),
        ("music", "音乐生成"),
        ("sound", "音效生成"),
        ("3d", "三维生成"),
        ("other", "其他模型"),
    ]
    .into_iter()
    .map(|(id, name)| CapabilityFunction {
        id: id.into(),
        name: name.into(),
    })
    .collect()
}
fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":true})
}
pub fn input_contract(protocol: &str) -> (Value, Value) {
    let prompt = json!({"type":"string","minLength":1,"maxLength":20000});
    match protocol {
        "chat" => (
            object(
                json!({"messages":{"type":"array","minItems":1,"items":{"type":"object","properties":{"role":{"enum":["system","user","assistant"]},"content":{}},"required":["role","content"]}}}),
                &["messages"],
            ),
            json!({"messages":[{"role":"user","content":"请填写本次任务"}]}),
        ),
        "responses" => (
            object(json!({"input":prompt}), &["input"]),
            json!({"input":"请填写本次任务"}),
        ),
        "embeddings" => (
            object(
                json!({"input":{"anyOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]}}),
                &["input"],
            ),
            json!({"input":"待向量化的文字"}),
        ),
        "ark_video" | "minimax_video" => (
            object(
                json!({"content":{"type":"array","minItems":1,"items":{"type":"object"}}}),
                &["content"],
            ),
            json!({"content":[{"type":"text","text":"描述需要生成的视频"}]}),
        ),
        "dashscope_image" | "dashscope_video" => (
            object(
                json!({"input":{"type":"object","properties":{"prompt":prompt},"required":["prompt"]}}),
                &["input"],
            ),
            json!({"input":{"prompt":"描述需要生成的内容"}}),
        ),
        "minimax_tts" => (
            object(
                json!({"text":prompt,"voice_setting":{"type":"object","properties":{"voice_id":{"type":"string","minLength":1}},"required":["voice_id"]}}),
                &["text", "voice_setting"],
            ),
            json!({"text":"待朗读的文字","voice_setting":{"voice_id":"从官方音色列表选择"}}),
        ),
        "speech_openai" => (
            object(
                json!({"input":prompt,"voice":{"type":"string","minLength":1}}),
                &["input", "voice"],
            ),
            json!({"input":"待朗读的文字","voice":"alloy"}),
        ),
        "speech_elevenlabs" => (
            object(
                json!({"text":prompt,"voice_id":{"type":"string","pattern":"^[A-Za-z0-9_-]{1,120}$"}}),
                &["text", "voice_id"],
            ),
            json!({"text":"待朗读的文字","voice_id":"VOICE_ID_FROM_PROVIDER"}),
        ),
        "assemblyai_asr" => (
            object(
                json!({"audio_url":{"type":"string","pattern":"^https://"}}),
                &["audio_url"],
            ),
            json!({"audio_url":"https://example.com/authorized-audio.wav"}),
        ),
        "deepgram_asr" => (
            object(
                json!({"url":{"type":"string","pattern":"^https://"}}),
                &["url"],
            ),
            json!({"url":"https://example.com/authorized-audio.wav"}),
        ),
        _ => (
            object(json!({"prompt":prompt}), &["prompt"]),
            json!({"prompt":"请填写本次任务"}),
        ),
    }
}
pub fn model(
    provider: &str,
    id: &str,
    name: &str,
    publisher: &str,
    groups: &[&str],
    protocol: &str,
    url: &str,
    source: &str,
) -> ModelCapability {
    let (input_schema, input_example) = input_contract(protocol);
    let canonical_id = if provider == "openrouter" {
        id.to_string()
    } else {
        format!("{publisher}/{id}")
    };
    let mut route = ModelCapability {
        route_id: format!("{provider}::{id}"),
        model_id: id.into(),
        name: name.into(),
        publisher: publisher.into(),
        canonical_id,
        provider_id: provider.into(),
        functions: groups.iter().map(|s| s.to_string()).collect(),
        protocol: protocol.into(),
        source: source.into(),
        source_url: url.into(),
        account_state: "not_verified".into(),
        input_schema,
        input_example,
        pricing: None,
    };
    if vision::supported(&route) {
        let mut chat = route.input_schema;
        chat["not"] = json!({"required":["vision"]});
        route.input_schema = json!({"anyOf":[chat,vision::schema()]});
    }
    route
}
/// Reviewed examples, not an assertion of this account's entitlement or an exhaustive provider list.
pub fn seed_models() -> Vec<ModelCapability> {
    let mut rows = vec![
        model(
            "openrouter",
            "openrouter/auto",
            "OpenRouter 自动路由",
            "openrouter",
            &["text"],
            "chat",
            "https://openrouter.ai/docs",
            "official_reference",
        ),
        model(
            "deepseek",
            "deepseek-flash",
            "DeepSeek Flash",
            "deepseek",
            &["text"],
            "chat",
            "https://api-docs.deepseek.com/",
            "official_reference",
        ),
        model(
            "deepseek",
            "deepseek-v4-pro",
            "DeepSeek V4 Pro",
            "deepseek",
            &["text"],
            "chat",
            "https://api-docs.deepseek.com/",
            "official_reference",
        ),
        model(
            "openai",
            "text-embedding-3-small",
            "Text Embedding 3 Small",
            "openai",
            &["embedding"],
            "embeddings",
            "https://developers.openai.com/api/docs/models/all",
            "official_reference",
        ),
        model(
            "minimax",
            "MiniMax-M3.1-Flash",
            "MiniMax M3.1 Flash",
            "minimax",
            &["text"],
            "chat",
            "https://platform.minimax.cn/docs/guides/models-intro",
            "official_reference",
        ),
        model(
            "minimax",
            "MiniMax-M3",
            "MiniMax M3",
            "minimax",
            &["text"],
            "chat",
            "https://platform.minimax.cn/docs/guides/models-intro",
            "official_reference",
        ),
        model(
            "minimax",
            "MiniMax-H3",
            "MiniMax H3",
            "minimax",
            &["video"],
            "minimax_video",
            "https://platform.minimax.cn/docs/api-reference/video-generation-v2-create",
            "official_reference",
        ),
        model(
            "minimax",
            "speech-2.8-hd",
            "Speech 2.8 HD",
            "minimax",
            &["tts"],
            "minimax_tts",
            "https://platform.minimax.cn/docs/api-reference/speech-t2a-http",
            "official_reference",
        ),
        model(
            "minimax",
            "speech-2.8-turbo",
            "Speech 2.8 Turbo",
            "minimax",
            &["tts"],
            "minimax_tts",
            "https://platform.minimax.cn/docs/api-reference/speech-t2a-http",
            "official_reference",
        ),
        model(
            "minimax",
            "image-01",
            "MiniMax Image 01",
            "minimax",
            &["image"],
            "minimax_image",
            "https://platform.minimax.cn/docs/guides/image-generation",
            "official_reference",
        ),
        model(
            "dashscope",
            "wan2.7-t2v",
            "通义万相 Wan 2.7",
            "qwen",
            &["video"],
            "dashscope_video",
            "https://help.aliyun.com/en/model-studio/text-to-video-api-reference",
            "official_reference",
        ),
        model(
            "ark",
            "doubao-seedance-2-5-260628",
            "Seedance 2.5",
            "bytedance",
            &["video"],
            "ark_video",
            "https://docs.volcengine.com/docs/ark/model-list",
            "official_reference",
        ),
        model(
            "elevenlabs",
            "eleven_multilingual_v2",
            "Eleven Multilingual v2",
            "elevenlabs",
            &["tts"],
            "speech_elevenlabs",
            "https://elevenlabs.io/docs/api-reference/models/list",
            "official_reference",
        ),
        model(
            "elevenlabs",
            "eleven_flash_v2_5",
            "Eleven Flash v2.5",
            "elevenlabs",
            &["tts"],
            "speech_elevenlabs",
            "https://elevenlabs.io/docs/api-reference/models/list",
            "official_reference",
        ),
        model(
            "assemblyai",
            "universal-3-pro",
            "Universal 3 Pro",
            "assemblyai",
            &["asr"],
            "assemblyai_asr",
            "https://www.assemblyai.com/docs/pre-recorded-audio/select-the-speech-model",
            "official_reference",
        ),
        model(
            "assemblyai",
            "universal-2",
            "Universal 2",
            "assemblyai",
            &["asr"],
            "assemblyai_asr",
            "https://www.assemblyai.com/docs/pre-recorded-audio/select-the-speech-model",
            "official_reference",
        ),
        model(
            "deepgram",
            "nova-3",
            "Nova 3",
            "deepgram",
            &["asr"],
            "deepgram_asr",
            "https://developers.deepgram.com/docs/models-languages-overview",
            "official_reference",
        ),
        model(
            "fal",
            "fal-ai/flux/schnell",
            "FLUX.1 Schnell",
            "black-forest-labs",
            &["image"],
            "fal_queue",
            "https://fal.ai/models/fal-ai/flux/schnell",
            "official_reference",
        ),
    ];
    // No guessed Ark deployment IDs, gateway hosts or Gemini/Anthropic protocol substitutions.
    rows.sort_by(|a, b| a.route_id.cmp(&b.route_id));
    rows
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 240
        && !id.contains("..")
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.:~".contains(&b))
        && !id.contains(' ')
}
pub fn endpoint(route: &ModelCapability, input: &Value) -> Result<String, String> {
    if !valid_id(&route.model_id) {
        return Err("模型编号无效".into());
    }
    let p = route.provider_id.as_str();
    let path = match route.protocol.as_str() {
        "chat" => match p {
            "openrouter" => "https://openrouter.ai/api/v1/chat/completions",
            "dashscope" => "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            "ark" => "https://ark.cn-beijing.volces.com/api/v3/chat/completions",
            "minimax" => "https://api.minimax.cn/v1/text/chatcompletion_v2",
            "deepseek" => "https://api.deepseek.com/chat/completions",
            _ => return Err("未接入该渠道的对话适配器".into()),
        },
        "responses" if p == "openai" => "https://api.openai.com/v1/responses",
        "image_openai" if p == "openai" => "https://api.openai.com/v1/images/generations",
        "speech_openai" if p == "openai" => "https://api.openai.com/v1/audio/speech",
        "embeddings" => match p {
            "openrouter" => "https://openrouter.ai/api/v1/embeddings",
            "openai" => "https://api.openai.com/v1/embeddings",
            "dashscope" => "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings",
            _ => return Err("未接入该渠道的向量适配器".into()),
        },
        "minimax_image" if p == "minimax" => "https://api.minimax.cn/v1/image_generation",
        "minimax_tts" if p == "minimax" => "https://api.minimax.cn/v1/t2a_v2",
        "minimax_video" if p == "minimax" => "https://api.minimax.cn/v2/video_generation",
        "ark_video" if p == "ark" => {
            "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks"
        }
        "dashscope_video" if p == "dashscope" => {
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/video-generation/video-synthesis"
        }
        "dashscope_image" if p == "dashscope" => {
            "https://dashscope.aliyuncs.com/api/v1/services/aigc/text2image/image-synthesis"
        }
        "fal_queue" if p == "fal" => {
            return Ok(format!("https://queue.fal.run/{}", route.model_id))
        }
        "assemblyai_asr" if p == "assemblyai" => "https://api.assemblyai.com/v2/transcript",
        "deepgram_asr" if p == "deepgram" => {
            return Ok(format!(
                "https://api.deepgram.com/v1/listen?model={}",
                route.model_id
            ))
        }
        "speech_elevenlabs" if p == "elevenlabs" => {
            let voice = input["voice_id"].as_str().unwrap_or("");
            if voice.is_empty()
                || voice.len() > 120
                || !voice
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            {
                return Err("请选择有效的 ElevenLabs voice_id".into());
            }
            return Ok(format!(
                "https://api.elevenlabs.io/v1/text-to-speech/{voice}"
            ));
        }
        _ => return Err("该模型的调用协议尚未接入；不能用其他渠道的 Key 替代".into()),
    };
    Ok(path.into())
}
fn forbidden_input(value: &Value, depth: usize) -> bool {
    if depth > 32 {
        return true;
    }
    match value {
        Value::Object(map) => map.iter().any(|(k, v)| {
            [
                "api_key",
                "apikey",
                "authorization",
                "token",
                "password",
                "secret",
                "base_url",
                "headers",
            ]
            .contains(&k.to_ascii_lowercase().as_str())
                || forbidden_input(v, depth + 1)
        }),
        Value::Array(a) => a.iter().any(|v| forbidden_input(v, depth + 1)),
        _ => false,
    }
}
pub fn prepare(route: &ModelCapability, input: &Value) -> Result<(String, Value), String> {
    if input.get("vision").is_some() {
        return vision::prepare(route, input);
    }
    if !input.is_object() || input.to_string().len() > 65536 || forbidden_input(input, 0) {
        return Err("调用参数必须为 JSON 对象，且不得包含密钥、认证头、端点或过深内容".into());
    }
    if input.get("stream").is_some_and(|v| v != false) {
        return Err("本接口仅支持非流式调用；请设置 stream=false".into());
    }
    if route.protocol == "speech_openai" && input.get("response_format").is_some_and(|v| v != "mp3")
    {
        return Err("当前音频文件适配器仅支持 MP3，请设置 response_format=mp3".into());
    }
    if route.protocol == "speech_elevenlabs" && input.get("output_format").is_some() {
        return Err(
            "当前 ElevenLabs 适配器使用默认 MP3，不接受 JSON 内的 output_format 参数".into(),
        );
    }
    if input
        .get("model")
        .is_some_and(|v| v.as_str() != Some(route.model_id.as_str()))
        || input
            .get("model_id")
            .is_some_and(|v| v.as_str() != Some(route.model_id.as_str()))
    {
        return Err("参数模型与所选渠道模型不一致，已拒绝错配 Key".into());
    }
    for required in route.input_schema["required"]
        .as_array()
        .or_else(|| route.input_schema["anyOf"][0]["required"].as_array())
        .into_iter()
        .flatten()
    {
        let key = required.as_str().unwrap();
        if input.get(key).is_none_or(|v| {
            v.is_null()
                || v.as_str().is_some_and(|s| s.trim().is_empty())
                || v.as_array().is_some_and(Vec::is_empty)
        }) {
            return Err(format!("缺少有效调用参数：{key}"));
        }
    }
    let url = endpoint(route, input)?;
    let mut body = input.clone();
    let map = body.as_object_mut().unwrap();
    match route.protocol.as_str() {
        "fal_queue" | "deepgram_asr" => {
            map.remove("model");
            map.remove("model_id");
        }
        "assemblyai_asr" => {
            map.insert("speech_models".into(), json!([route.model_id]));
        }
        "speech_elevenlabs" => {
            map.remove("voice_id");
            map.insert("model_id".into(), json!(route.model_id));
        }
        _ => {
            map.insert("model".into(), json!(route.model_id));
        }
    }
    if route.protocol == "chat" || route.protocol == "responses" {
        map.insert("stream".into(), json!(false));
    }
    Ok((url, body))
}
pub fn task_endpoint(route: &ModelCapability, id: &str, result: bool) -> Result<String, String> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err("任务编号无效".into());
    }
    match route.protocol.as_str() {
        "fal_queue" => {
            let base = route
                .model_id
                .split('/')
                .take(2)
                .collect::<Vec<_>>()
                .join("/");
            Ok(format!(
                "https://queue.fal.run/{base}/requests/{id}{}",
                if result { "" } else { "/status" }
            ))
        }
        "ark_video" => Ok(format!(
            "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks/{id}"
        )),
        "minimax_video" => Ok(format!(
            "https://api.minimax.cn/v2/query/video_generation/{id}"
        )),
        "dashscope_image" | "dashscope_video" => {
            Ok(format!("https://dashscope.aliyuncs.com/api/v1/tasks/{id}"))
        }
        "assemblyai_asr" => Ok(format!("https://api.assemblyai.com/v2/transcript/{id}")),
        _ => Err("该调用不使用异步任务查询".into()),
    }
}
pub fn outcome(
    route: &ModelCapability,
    value: &Value,
    is_submit: bool,
) -> (String, Option<String>) {
    let nested = value
        .get("task")
        .or_else(|| value.get("output"))
        .unwrap_or(value);
    if value.get("error").is_some_and(|v| !v.is_null())
        || value["base_resp"]["status_code"]
            .as_i64()
            .is_some_and(|v| v != 0)
        || value["ok"] == false
        || value["success"] == false
    {
        return ("failed".into(), None);
    }
    let id = [
        nested.get("task_id"),
        nested.get("id"),
        value.get("task_id"),
        value.get("request_id"),
    ]
    .into_iter()
    .flatten()
    .find_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string));
    let raw = nested["task_status"]
        .as_str()
        .or_else(|| nested["status"].as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let nonempty = |v: &Value| match v {
        Value::Null => false,
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
        Value::String(v) => !v.is_empty(),
        _ => true,
    };
    let payload = match route.protocol.as_str() {
        "chat" => value["choices"].as_array().is_some_and(|a| !a.is_empty()),
        "responses" => value["output"].as_array().is_some_and(|a| !a.is_empty()),
        "embeddings" | "image_openai" => value["data"].as_array().is_some_and(|a| !a.is_empty()),
        "minimax_image" => {
            nonempty(&value["data"]["image_urls"]) || nonempty(&value["data"]["image_base64"])
        }
        "minimax_tts" => nonempty(&value["data"]["audio"]),
        "deepgram_asr" => nonempty(&value["results"]["channels"]),
        "fal_queue" => ["images", "image", "video", "audio", "text", "transcription"]
            .iter()
            .any(|k| nonempty(&value[k])),
        "ark_video" => nonempty(&nested["content"]["video_url"]),
        "minimax_video" => nonempty(&nested["content"]["url"]),
        "dashscope_video" => nonempty(&nested["video_url"]),
        "dashscope_image" => nonempty(&nested["results"]),
        "assemblyai_asr" => nested["text"].is_string(),
        _ => false,
    };
    let state = match raw.as_str() {
        "success" | "succeeded" | "completed"
            if payload || (!is_submit && route.protocol == "fal_queue") =>
        {
            "succeeded"
        }
        "success" | "succeeded" | "completed" => "unknown",
        "failed" | "error" | "cancelled" | "canceled" => "failed",
        "pending" | "queued" | "in_queue" | "created" => "queued",
        "running" | "processing" | "in_progress" => "running",
        _ if is_submit
            && !payload
            && id.is_some()
            && [
                "fal_queue",
                "ark_video",
                "minimax_video",
                "dashscope_image",
                "dashscope_video",
                "assemblyai_asr",
            ]
            .contains(&route.protocol.as_str()) =>
        {
            "queued"
        }
        _ if !is_submit && task_endpoint(route, "synthetic", false).is_ok() => "unknown",
        _ if payload => "succeeded",
        _ => "unknown",
    };
    (state.into(), id)
}
pub fn merge_models(
    existing: &mut Vec<ModelCapability>,
    provider: &str,
    mut incoming: Vec<ModelCapability>,
) {
    existing.retain(|m| m.provider_id != provider);
    let mut seen = BTreeSet::new();
    incoming.retain(|m| valid_id(&m.model_id) && seen.insert(m.route_id.clone()));
    existing.extend(incoming);
    existing.sort_by(|a, b| a.name.cmp(&b.name).then(a.route_id.cmp(&b.route_id)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selected_routes_and_source_choices_are_validated_and_backward_compatible() {
        let request: ModelCapabilityRequest =
            serde_json::from_value(json!({"route_id":"openrouter::fixture:free","input":{}}))
                .unwrap();
        assert_eq!(request.credential_source, CredentialSource::Default);
        assert!(serde_json::from_value::<ModelCapabilityRequest>(
            json!({"route_id":"x","input":{},"credential_source":"arbitrary-file"})
        )
        .is_err());
        let route = seed_models().into_iter().find(|m|m.protocol=="chat").unwrap();
        let good = json!({route.route_id.clone():route});
        let loaded = parse_route_pins(&serde_json::to_vec(&good).unwrap()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(parse_route_pins(b"{broken").is_err());
        let mut bad = good.clone();
        let key = bad.as_object().unwrap().keys().next().unwrap().clone();
        bad[&key]["provider_id"] = json!("arbitrary");
        assert!(parse_route_pins(&serde_json::to_vec(&bad).unwrap()).is_err());
        let mut wrong_protocol = good.clone();
        wrong_protocol[&key]["protocol"] = json!("fal_queue");
        assert!(parse_route_pins(&serde_json::to_vec(&wrong_protocol).unwrap()).is_err());
    }
    #[test]
    fn provider_not_publisher_selects_endpoint_and_key() {
        let route = model(
            "openrouter",
            "openai/example",
            "Example",
            "openai",
            &["text"],
            "chat",
            "",
            "api_catalog",
        );
        let (url, body) = prepare(
            &route,
            &json!({"messages":[{"role":"user","content":"fixture"}]}),
        )
        .unwrap();
        assert!(url.starts_with("https://openrouter.ai/"));
        assert_eq!(body["model"], "openai/example");
        assert!(prepare(&route, &json!({"model":"other","messages":[{}]})).is_err());
        assert!(prepare(&route, &json!({"messages":[{}],"api_key":"synthetic-only"})).is_err());
    }
    #[test]
    fn explicit_protocols_do_not_accept_cross_provider_or_path_injection() {
        let mut route = seed_models()
            .into_iter()
            .find(|r| r.provider_id == "fal")
            .unwrap();
        assert!(task_endpoint(&route, "../private", false).is_err());
        assert!(task_endpoint(&route, "a", false)
            .unwrap()
            .ends_with("fal-ai/flux/requests/a/status"));
        route.provider_id = "openai".into();
        assert!(endpoint(&route, &json!({})).is_err());
    }
    #[test]
    fn async_submission_and_unknown_never_become_completed() {
        let route = seed_models()
            .into_iter()
            .find(|r| r.protocol == "minimax_video")
            .unwrap();
        assert_eq!(
            outcome(&route, &json!({"task_id":"fixture"}), true).0,
            "queued"
        );
        assert_eq!(outcome(&route, &json!({}), false).0, "unknown");
        assert_eq!(
            outcome(&route, &json!({"task":{"status":"failed"}}), false).0,
            "failed"
        );
        assert_eq!(outcome(&route,&json!({"task":{"status":"succeeded","content":{"url":"https://example.com/fixture.mp4"}}}),false).0,"succeeded");
    }
    #[test]
    fn seeds_are_explicit_not_entitlements_and_refresh_replaces_retired_models() {
        let mut models = seed_models();
        for m in &models {
            assert_eq!(m.account_state, "not_verified");
            assert!(prepare(m, &m.input_example).is_ok(), "{}", m.route_id);
        }
        merge_models(&mut models, "openai", vec![]);
        assert!(!models.iter().any(|m| m.provider_id == "openai"));
    }
    #[test]
    fn empty_or_unknown_provider_payload_is_never_success() {
        let route = model(
            "openrouter",
            "publisher/test",
            "fixture",
            "publisher",
            &["text"],
            "chat",
            "",
            "api_catalog",
        );
        assert_eq!(
            outcome(&route, &json!({"request_id":"fixture"}), true).0,
            "unknown"
        );
        assert_eq!(outcome(&route, &json!({"choices":[]}), true).0, "unknown");
        assert_eq!(
            outcome(&route, &json!({"status":"completed","choices":[]}), true).0,
            "unknown"
        );
        assert_eq!(
            outcome(
                &route,
                &json!({"choices":[{"message":{"content":"fixture"}}]}),
                true
            )
            .0,
            "succeeded"
        );
        let fal = model(
            "fal",
            "fixture/video",
            "fixture",
            "fixture",
            &["video"],
            "fal_queue",
            "",
            "fixture",
        );
        assert_eq!(
            outcome(
                &fal,
                &json!({"request_id":"id","video":{"url":"https://example.com/fixture.mp4"}}),
                true
            )
            .0,
            "succeeded"
        );
    }
    #[test]
    fn sops_precedence_and_missing_independent_file_never_misclassify_real_binding() {
        let missing = Some(("not_configured", "api-key-manager:elevenlabs"));
        let active = binding_choice(
            true,
            Some("filled"),
            missing,
            "elevenlabs_key",
            false,
            "warn",
        );
        assert_eq!(active.0, "sops");
        assert_eq!(active.1, "configured");
        assert_eq!(active.2, "sops:elevenlabs_key");
        let blocked = binding_choice(
            true,
            Some("empty"),
            Some(("linked", "api-key-manager:openrouter")),
            "openrouter_key",
            false,
            "warn",
        );
        assert_eq!(blocked.1, "blocked");
        assert_eq!(blocked.0, "sops");
        assert_eq!(
            binding_choice(
                false,
                None,
                Some(("linked", "api-key-manager:fal")),
                "fal_key",
                false,
                "warn"
            )
            .1,
            "unknown"
        );
        assert_eq!(
            binding_choice(
                true,
                None,
                Some(("linked", "api-key-manager:fal")),
                "fal_key",
                false,
                "warn"
            )
            .0,
            "dpapi_reference"
        );
        assert_eq!(
            binding_choice(true, Some("filled"), None, "openai_key", true, "block").1,
            "blocked"
        );
    }
}
