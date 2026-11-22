//! Fixed-host provider transport and public catalog parsing; no credential-source or Tauri access.
use crate::model_capabilities::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Duration};
#[path = "model_reliability.rs"]
pub mod reliability;
#[path = "model_dialogue_stream.rs"]
pub(crate) mod dialogue_stream;

pub(crate) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|_| "无法初始化模型网络客户端".into())
}
pub(crate) fn model_transport_error(error: reqwest::Error) -> String {
    reliability::transport_error(&error).encode()
}
#[cfg(test)]
#[test]
fn model_transport_diagnostics_do_not_echo_private_request_data() {
    let error = reqwest::Client::new()
        .get("https://example.invalid/private?token=synthetic-secret")
        .header("authorization", "synthetic-secret\n")
        .build()
        .unwrap_err();
    let message = model_transport_error(error);
    assert!(message.contains("build"));
    assert!(!message.contains("synthetic-secret"));
    assert!(!message.contains("example.invalid"));
}
pub(crate) async fn bytes(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err(reliability::Failure::http(response.status().as_u16()).encode());
    }
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(reliability::Failure::unknown(reliability::Kind::Limit).encode());
    }
    let mut result = Vec::new();
    while let Some(part) = response
        .chunk()
        .await
        .map_err(|error| reliability::Failure::unknown(if error.is_timeout(){reliability::Kind::Timeout}else{reliability::Kind::Read}).encode())?
    {
        if result.len() + part.len() > max {
            return Err(reliability::Failure::unknown(reliability::Kind::Limit).encode());
        }
        result.extend_from_slice(&part);
    }
    Ok(result)
}
pub(crate) fn authorized(
    request: reqwest::RequestBuilder,
    provider: &str,
    key: &str,
) -> reqwest::RequestBuilder {
    match provider {
        "fal" => request.header("Authorization", format!("Key {key}")),
        "elevenlabs" => request.header("xi-api-key", key),
        "assemblyai" => request.header("Authorization", key),
        "deepgram" => request.header("Authorization", format!("Token {key}")),
        _ => request.bearer_auth(key),
    }
}

pub(crate) fn discover(
    provider: &str,
    raw: &Value,
    url: &str,
) -> Result<Vec<ModelCapability>, String> {
    let array = if provider == "fal" {
        raw["models"].as_array()
    } else if provider == "elevenlabs" {
        raw.as_array()
    } else {
        raw["data"].as_array()
    }
    .ok_or("厂商模型目录结构不匹配，已保留上次目录")?;
    let mut result = Vec::new();
    let observed_at = chrono::Utc::now().to_rfc3339();
    for item in array {
        let id = item[if provider == "fal" {
            "endpoint_id"
        } else if provider == "elevenlabs" {
            "model_id"
        } else {
            "id"
        }]
        .as_str()
        .ok_or("模型目录缺少编号；未将损坏结果解释为空目录")?;
        if !valid_id(id) {
            return Err("模型目录包含无效编号，已保留上次目录".into());
        }
        let lower = id.to_ascii_lowercase();
        let mut groups = Vec::new();
        let mut protocol = "unsupported";
        match provider {
            "openrouter" => {
                let output = item["architecture"]["output_modalities"].as_array();
                let input = item["architecture"]["input_modalities"].as_array();
                let has = |list: Option<&Vec<Value>>, s: &str| {
                    list.is_some_and(|v| v.iter().any(|x| x == s))
                };
                if has(output, "text") {
                    groups.push("text");
                    protocol = "chat";
                }
                if has(output, "image") {
                    groups.push("image");
                    protocol = "chat";
                }
                if has(output, "embeddings") {
                    groups.push("embedding");
                    protocol = "embeddings";
                }
                if has(input, "image") && has(output, "text") {
                    groups.push("vision");
                }
                if has(input, "video") && has(output, "text") {
                    groups.push("video_understanding");
                }
                if has(input, "audio") && has(output, "text") {
                    groups.push("audio_understanding");
                }
                if has(output, "audio") {
                    groups.push("audio_generation");
                    protocol = "unsupported";
                }
                if has(output, "video") {
                    groups.push("video");
                    protocol = "unsupported";
                }
            }
            "elevenlabs" => {
                if item["can_do_text_to_speech"] == true {
                    groups.push("tts");
                    protocol = "speech_elevenlabs";
                }
            }
            "fal" => {
                let category = item["metadata"]["category"].as_str().unwrap_or("");
                if category.ends_with("to-image") {
                    groups.push("image");
                    protocol = "fal_queue";
                }
                if category.ends_with("to-video") {
                    groups.push("video");
                    protocol = "fal_queue";
                }
                if category == "text-to-speech" {
                    groups.push("tts");
                    protocol = "fal_queue";
                }
                if category == "speech-to-text" {
                    groups.push("asr");
                    protocol = "fal_queue";
                }
                if category.ends_with("to-3d") {
                    groups.push("3d");
                    protocol = "fal_queue";
                }
                if category == "text-to-music" {
                    groups.push("music");
                    protocol = "fal_queue";
                }
                if ["text-to-audio", "sound-effects"].contains(&category) {
                    groups.push(if category == "sound-effects" {
                        "sound"
                    } else {
                        "audio_generation"
                    });
                    protocol = "fal_queue";
                }
            }
            "openai" => {
                if lower.starts_with("text-embedding-") {
                    groups.push("embedding");
                    protocol = "embeddings";
                } else if lower.starts_with("gpt-image-") {
                    groups.push("image");
                    protocol = "image_openai";
                } else if lower.contains("tts") {
                    groups.push("tts");
                    protocol = "speech_openai";
                } else if lower.contains("transcribe") || lower.starts_with("whisper") {
                    groups.push("asr");
                } else if lower.starts_with("sora") {
                    groups.push("video");
                } else if lower.starts_with("gpt-")
                    && !lower.contains("audio")
                    && !lower.contains("transcribe")
                    && !lower.contains("realtime")
                {
                    groups.push("text");
                    protocol = "responses";
                }
            }
            "dashscope" => {
                if lower.starts_with("text-embedding-") {
                    groups.push("embedding");
                    protocol = "embeddings";
                } else if lower.contains("t2v") {
                    groups.push("video");
                    protocol = "dashscope_video";
                } else if lower.starts_with("qwen-image") || lower.starts_with("wanx") {
                    groups.push("image");
                } else if lower.contains("tts") {
                    groups.push("tts");
                } else if lower.contains("asr") {
                    groups.push("asr");
                } else if lower.starts_with("qwen")
                    && !lower.contains("image")
                    && !lower.contains("tts")
                    && !lower.contains("audio")
                    && !lower.contains("asr")
                    && !lower.contains("omni")
                {
                    groups.push("text");
                    protocol = "chat";
                    if lower.contains("-vl") {
                        groups.push("vision");
                    }
                }
            }
            _ => {}
        }
        if groups.is_empty() {
            groups.push("other");
        }
        let name = item["name"]
            .as_str()
            .or_else(|| item["metadata"]["display_name"].as_str())
            .unwrap_or(id);
        let publisher = if provider == "openrouter" {
            id.split('/').next().unwrap_or(provider)
        } else {
            provider
        };
        let mut m = model(
            provider,
            id,
            name,
            publisher,
            &groups,
            protocol,
            url,
            "api_catalog",
        );
        m.source_url = match provider {
            "openrouter" => format!("https://openrouter.ai/{id}"),
            "openai" => format!("https://developers.openai.com/api/docs/models/{id}"),
            "elevenlabs" => {
                "https://elevenlabs.io/docs/api-reference/text-to-speech/convert".into()
            }
            "dashscope" => "https://help.aliyun.com/zh/model-studio/models".into(),
            _ => url.into(),
        };
        m.account_state = if provider == "openrouter" && !url.contains("/models/user") {
            "public_catalog_not_entitlement"
        } else {
            "listed_not_inference_verified"
        }
        .into();
        if provider == "openrouter" {
            m.pricing = item.get("pricing").map(|value|crate::capability_facts::PriceInfo::from_openrouter(id,value,&observed_at).with_catalog_source(url)).transpose()?;
        }
        if provider == "fal" {
            m.source_url = format!("https://fal.ai/models/{id}/api");
            let category = item["metadata"]["category"].as_str().unwrap_or("");
            if category.starts_with("image-to-") {
                m.input_schema["properties"]["image_url"] =
                    json!({"type":"string","pattern":"^https://"});
                m.input_schema["required"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!("image_url"));
                m.input_example["image_url"] = json!("https://example.com/authorized-image.png");
            }
        }
        result.push(m);
    }
    Ok(result)
}

#[cfg(test)]
#[test]
fn catalog_prices_are_provider_bound_and_pinned_without_refreshing_observation() {
    let rows = discover("openrouter", &json!({"data":[{"id":"fixture/text","name":"Fixture","architecture":{"output_modalities":["text"]},"pricing":{"prompt":"0.0000001","completion":"0","request":"0"}}]}), "https://openrouter.ai/api/v1/models").unwrap();
    let row = &rows[0];
    let price = row.pricing.as_ref().unwrap();
    assert!(price.valid_for(&row.route_id));
    assert_eq!(price.rates[0].per_million_tokens.as_deref(), Some("0.1"));
    let bytes = serde_json::to_vec(&json!({row.route_id.clone():row})).unwrap();
    let pinned = parse_route_pins(&bytes).unwrap();
    assert_eq!(pinned[&row.route_id].pricing.as_ref(), Some(price));
    let mut legacy = serde_json::to_value(row).unwrap();
    legacy.as_object_mut().unwrap().remove("pricing");
    let old = parse_route_pins(&serde_json::to_vec(&json!({row.route_id.clone():legacy})).unwrap()).unwrap();
    assert!(old[&row.route_id].pricing.is_none());
    let mut wrong = serde_json::to_value(row).unwrap();
    wrong["pricing"]["route_id"] = json!("openrouter::other");
    assert!(parse_route_pins(&serde_json::to_vec(&json!({row.route_id.clone():wrong})).unwrap()).is_err());
    let other = discover("openai", &json!({"data":[{"id":"gpt-fixture","pricing":{"prompt":"0"}}]}), "https://api.openai.com/v1/models").unwrap();
    assert!(other[0].pricing.is_none());
    let user_url="https://openrouter.ai/api/v1/models/user?output_modalities=all";
    let user=discover("openrouter",&json!({"data":[{"id":"fixture/text","architecture":{"output_modalities":["text"]},"pricing":{"prompt":"0","completion":"0","request":"0"}}]}),user_url).unwrap();
    assert_eq!(user[0].pricing.as_ref().unwrap().source_url,user_url);
    assert_eq!(user[0].account_state,"listed_not_inference_verified");
}

pub(crate) fn validate_output_root(root: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(root);
    if !root.is_absolute() || root.parent().is_none() {
        return Err("请指定专用的绝对输出目录，不使用磁盘根目录".into());
    }
    if root.to_string_lossy().starts_with("\\\\") {
        return Err("输出目录不能使用网络共享".into());
    }
    for ancestor in root.ancestors() {
        if std::fs::symlink_metadata(ancestor).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("输出目录不能包含链接".into());
        }
    }
    if root.exists() && !root.is_dir() {
        return Err("输出路径不是文件夹".into());
    }
    Ok(root)
}
fn output_file(
    root: &str,
    content: &[u8],
    extension: &str,
    mime: &str,
) -> Result<ModelCapabilityArtifact, String> {
    let root = validate_output_root(root)?;
    let directory = root.join(format!("homebase-model-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).map_err(|_| "输出根目录不可创建")?;
    std::fs::create_dir(&directory).map_err(|_| "独立结果目录不可创建")?;
    let file = directory.join(format!("result.{extension}"));
    std::fs::write(&file, content).map_err(|_| "模型结果不可保存；请核对原任务，不自动重跑")?;
    Ok(ModelCapabilityArtifact {
        path: file.to_string_lossy().into(),
        bytes: content.len() as u64,
        sha256: format!("{:x}", Sha256::digest(content)),
        mime_type: mime.into(),
    })
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponseMode {
    Submit,
    Poll,
    SubmitPng,
}

impl ResponseMode {
    pub(crate) fn for_submission(route: &ModelCapability, body: &Value, root: Option<&str>) -> Self {
        if root.is_some()
            && route.provider_id == "openai"
            && route.protocol == "image_openai"
            && route.model_id.starts_with("gpt-image-")
            && body.get("output_format").is_none_or(|value| value == "png")
            && body.get("n").is_none_or(|value| value.as_u64() == Some(1))
            && body.get("stream").is_none_or(|value| value == false)
        {
            Self::SubmitPng
        } else {
            Self::Submit
        }
    }
}

pub(crate) async fn response_result(
    route: &ModelCapability,
    key_ref: String,
    response: reqwest::Response,
    root: Option<&str>,
    mode: ResponseMode,
) -> Result<ModelCapabilityResult, String> {
    let binary = ["speech_openai", "speech_elevenlabs"].contains(&route.protocol.as_str());
    let audio_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .starts_with("audio/");
    let body = bytes(response, 32 * 1024 * 1024).await?;
    if binary {
        if !audio_type || body.is_empty() {
            return Err("服务商未返回有效音频响应，不能将空内容或 JSON 当作音频成功".into());
        }
        let artifact = output_file(
            root.ok_or("二进制音频必须指定 output_root")?,
            &body,
            "mp3",
            "audio/mpeg",
        )?;
        return Ok(ModelCapabilityResult {
            provider_id: route.provider_id.clone(),
            model_id: route.model_id.clone(),
            credential_ref: key_ref,
            status: "succeeded".into(),
            task_id: None,
            output: json!({"format":"mp3","remote_inference_completed":true}),
            artifact: Some(artifact),
            message: "音频已保存；请按文件路径试听验收".into(),
        });
    }
    let value: Value = serde_json::from_slice(&body)
        .map_err(|_| reliability::Failure::unknown(reliability::Kind::InvalidResponse).encode())?;
    if let Some(error)=value.get("error") {
        let code=error["code"].as_u64().filter(|s|(400..=599).contains(s));
        return Err(code.map(|s|reliability::Failure::provider(s as u16)).unwrap_or_else(||reliability::Failure::unknown(reliability::Kind::InvalidResponse)).encode());
    }
    let (status, task_id) = outcome(route, &value, mode != ResponseMode::Poll);
    if mode == ResponseMode::SubmitPng {
        if status == "failed" {
            return Ok(ModelCapabilityResult {
                provider_id: route.provider_id.clone(), model_id: route.model_id.clone(),
                credential_ref: key_ref, status, task_id: None,
                output: json!({}), artifact: None, message: "原生图请求返回失败。".into(),
            });
        }
        let invalid = || reliability::Failure::unknown(reliability::Kind::InvalidResponse).encode();
        if status != "succeeded" || value.get("output_format").is_some_and(|format| format != "png") {
            return Err(invalid());
        }
        let [image] = value["data"].as_array().map(Vec::as_slice).ok_or_else(invalid)? else {
            return Err(invalid());
        };
        let encoded = image["b64_json"].as_str().ok_or_else(invalid)?;
        let png = vision::normalize_generated_png(encoded).map_err(|_| invalid())?;
        let artifact = output_file(root.ok_or_else(invalid)?, &png, "png", "image/png")?;
        let usage: serde_json::Map<String, Value> = ["input_tokens", "output_tokens", "total_tokens"]
            .into_iter().filter_map(|key| value["usage"][key].as_u64().map(|count| (key.into(), json!(count))))
            .collect();
        return Ok(ModelCapabilityResult {
            provider_id: route.provider_id.clone(), model_id: route.model_id.clone(),
            credential_ref: key_ref, status, task_id: None,
            output: json!({"format":"png","usage":usage}), artifact: Some(artifact),
            message: "图片已校验并保存为 PNG。".into(),
        });
    }
    let artifact = if body.len() > 256 * 1024 {
        Some(output_file(
            root.ok_or("较大结果必须指定 output_root；结果未截断，请核对原任务")?,
            &body,
            "json",
            "application/json",
        )?)
    } else {
        None
    };
    let output = if artifact.is_some() {
        json!({"result_in_artifact":true})
    } else {
        value
    };
    Ok(ModelCapabilityResult{provider_id:route.provider_id.clone(),model_id:route.model_id.clone(),credential_ref:key_ref,status,task_id,output,artifact,message:"异步任务需 model_capability_task 查询；未知/失败不代表成功，生成内容按结果字段或产物验收".into()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_modalities_drive_groups_not_brand_or_input_video() {
        let rows=discover("openrouter",&json!({"data":[{"id":"publisher/fixture","architecture":{"input_modalities":["text","video","image"],"output_modalities":["text"]}}]}),"fixture").unwrap();
        assert!(rows[0].functions.contains(&"video_understanding".into()));
        assert!(!rows[0].functions.contains(&"video".into()));
        assert_eq!(rows[0].provider_id, "openrouter");
    }
    #[test]
    fn malformed_catalog_does_not_become_empty_and_unknown_models_are_not_guessed() {
        assert!(discover("openai", &json!({"error":"synthetic"}), "fixture").is_err());
        let rows = discover(
            "openai",
            &json!({"data":[{"id":"new-unknown-fixture"}]}),
            "fixture",
        )
        .unwrap();
        assert_eq!(rows[0].protocol, "unsupported");
        assert_eq!(rows[0].functions, vec!["other"]);
        let aliases=discover("openrouter",&json!({"data":[{"id":"~openai/fixture-latest","architecture":{"output_modalities":["text"],"input_modalities":["text"]}}]}),"fixture").unwrap();
        assert_eq!(aliases[0].model_id, "~openai/fixture-latest");
        assert!(prepare(&aliases[0], &aliases[0].input_example).is_ok());
    }
    #[test]
    fn output_files_are_new_and_do_not_overwrite_existing_data() {
        let dir = tempfile::tempdir().unwrap();
        let existing = dir.path().join("existing.txt");
        std::fs::write(&existing, b"original").unwrap();
        let first = output_file(
            dir.path().to_str().unwrap(),
            b"synthetic",
            "json",
            "application/json",
        )
        .unwrap();
        let second = output_file(
            dir.path().to_str().unwrap(),
            b"synthetic",
            "json",
            "application/json",
        )
        .unwrap();
        assert_ne!(first.path, second.path);
        assert_eq!(std::fs::read(existing).unwrap(), b"original");
    }
    pub(crate) fn fixture_response(
        status: &str,
        content_type: &str,
        body: &[u8],
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let status = status.to_string();
        let content_type = content_type.to_string();
        let body = body.to_vec();
        let handle = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = vec![];
            let mut part = [0u8; 8192];
            loop {
                let read = socket.read(&mut part).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&part[..read]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|s| s.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            write!(socket,"HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
            socket.write_all(&body).unwrap();
            String::from_utf8(request).unwrap()
        });
        (url, handle)
    }
    #[tokio::test]
    async fn successful_http_with_provider_error_preserves_both_statuses_without_echoing_body() {
        let (url,worker)=fixture_response("200 OK","application/json",br#"{"error":{"code":429,"message":"synthetic-secret-do-not-echo"}}"#);
        let response=client().unwrap().post(url).send().await.unwrap();
        let route=discover("openrouter",&json!({"data":[{"id":"fixture/text","architecture":{"output_modalities":["text"]}}]}),"fixture").unwrap().remove(0);
        let error=response_result(&route,"fixture".into(),response,None,ResponseMode::Submit).await.err().unwrap();
        let f=reliability::Failure::decode(&error).unwrap();assert_eq!(f.http_status,Some(200));assert_eq!(f.provider_status,Some(429));assert!(f.safe_to_resume());assert!(!f.transient());assert!(!error.contains("synthetic-secret"));worker.join().unwrap();
    }
    #[tokio::test]
    async fn synthetic_transport_uses_selected_provider_header_and_model_without_real_credentials()
    {
        for (provider, protocol, header) in [
            (
                "openrouter",
                "chat",
                "authorization: bearer fixture-only-key",
            ),
            ("fal", "fal_queue", "authorization: key fixture-only-key"),
            (
                "deepgram",
                "deepgram_asr",
                "authorization: token fixture-only-key",
            ),
            (
                "elevenlabs",
                "speech_elevenlabs",
                "xi-api-key: fixture-only-key",
            ),
        ] {
            let body = if provider == "fal" {
                json!({"request_id":"fixture-task"})
            } else if provider == "deepgram" {
                json!({"results":{"channels":[{"alternatives":[{"transcript":"synthetic"}]}]}})
            } else {
                json!({"choices":[{"message":{"content":"synthetic"}}]})
            };
            let (url, server) =
                fixture_response("200 OK", "application/json", body.to_string().as_bytes());
            let response = authorized(client().unwrap().post(url), provider, "fixture-only-key")
                .json(&json!({"model":"selected-fixture-model"}))
                .send()
                .await
                .unwrap();
            let bytes = bytes(response, 1024).await.unwrap();
            assert!(!bytes.is_empty());
            let sent = server.join().unwrap().to_ascii_lowercase();
            assert!(sent.contains(header));
            assert!(sent.contains("selected-fixture-model"));
            let route = model(
                provider,
                "fixture-model",
                "fixture",
                provider,
                &["text"],
                protocol,
                "",
                "fixture",
            );
            if provider == "fal" {
                assert_eq!(outcome(&route, &body, true).0, "queued");
            }
        }
    }
    #[tokio::test]
    async fn failures_do_not_echo_provider_body_and_json_is_not_saved_as_audio() {
        let (url, server) = fixture_response(
            "401 Unauthorized",
            "application/json",
            br#"{"error":"fixture-only-key"}"#,
        );
        let response = client().unwrap().get(url).send().await.unwrap();
        let error = bytes(response, 1024).await.err().unwrap();
        assert!(error.contains("401"));
        assert!(!error.contains("fixture-only-key"));
        server.join().unwrap();
        let route = model(
            "elevenlabs",
            "fixture",
            "fixture",
            "elevenlabs",
            &["tts"],
            "speech_elevenlabs",
            "",
            "fixture",
        );
        let (url, server) = fixture_response("200 OK", "application/json", b"{}");
        let response = client().unwrap().get(url).send().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        assert!(response_result(
            &route,
            "sops:fixture".into(),
            response,
            Some(directory.path().to_str().unwrap()),
            ResponseMode::Submit
        )
        .await
        .is_err());
        server.join().unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    #[tokio::test]
    async fn synthetic_audio_is_saved_in_a_new_isolated_directory_with_a_hash() {
        let (url, server) =
            fixture_response("200 OK", "audio/mpeg", b"ID3synthetic-transport-fixture");
        let response = client().unwrap().get(url).send().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let route = model(
            "elevenlabs",
            "fixture",
            "fixture",
            "elevenlabs",
            &["tts"],
            "speech_elevenlabs",
            "",
            "fixture",
        );
        let result = response_result(
            &route,
            "sops:fixture".into(),
            response,
            Some(directory.path().to_str().unwrap()),
            ResponseMode::Submit,
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(result.status, "succeeded");
        let artifact = result.artifact.unwrap();
        assert!(PathBuf::from(&artifact.path).starts_with(directory.path()));
        assert_eq!(
            artifact.sha256,
            format!("{:x}", Sha256::digest(b"ID3synthetic-transport-fixture"))
        );
    }

    #[tokio::test]
    async fn single_png_mode_rejects_missing_invalid_or_conflicting_images_without_writing_or_retrying() {
        let route = discover("openai", &json!({"data":[{"id":"gpt-image-1.5"}]}), "https://api.openai.com/v1/models").unwrap().remove(0);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_str().unwrap();
        for body in [json!({"n":2}), json!({"output_format":"jpeg"}), json!({"stream":true})] {
            assert!(ResponseMode::for_submission(&route, &body, Some(root)) == ResponseMode::Submit);
        }
        assert!(ResponseMode::for_submission(&route, &json!({}), None) == ResponseMode::Submit);
        let mode = ResponseMode::for_submission(&route, &json!({}), Some(root));
        assert!(mode == ResponseMode::SubmitPng);
        for body in [
            json!({"data":[]}),
            json!({"data":[{"url":"https://synthetic-private.invalid/raw"}]}),
            json!({"data":[{"b64_json":"SG9tZUJhc2U="}]}),
            json!({"data":[{"b64_json":"x"},{"b64_json":"y"}]}),
            json!({"output_format":"webp","data":[{"b64_json":"x"}]}),
        ] {
            let (url, server) = fixture_response("200 OK", "application/json", body.to_string().as_bytes());
            let response = client().unwrap().post(url).send().await.unwrap();
            let error = response_result(&route, "fixture".into(), response, Some(root), mode).await.err().unwrap();
            assert!(!reliability::Failure::decode(&error).unwrap().safe_to_resume());
            assert!(!error.contains("synthetic-private"));
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
            server.join().unwrap();
        }
    }
}

#[cfg(test)]
pub(crate) use tests::fixture_response;
