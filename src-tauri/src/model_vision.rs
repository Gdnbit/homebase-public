//! Explicit one-image input for catalog-verified OpenRouter chat routes.
use super::{endpoint, ModelCapability};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Cursor;

pub(crate) const MAX_IMAGE_BYTES: usize = 384 * 1024;
const MAX_ENCODED_BYTES: usize = MAX_IMAGE_BYTES.div_ceil(3) * 4;
pub(crate) const MAX_PROMPT_BYTES: usize = 4000;
const MAX_EDGE: u32 = 2048;
const MAX_PIXELS: u32 = 2 * 1024 * 1024;
const UNSUPPORTED: &str = "VISION_UNSUPPORTED: 所选渠道没有经过目录确认的图片输入适配";
const INVALID_INPUT: &str =
    "VISION_INVALID_INPUT: 图片理解只接受 vision 对象内的 prompt 和 png_base64";
const INVALID_PNG: &str = "VISION_INVALID_PNG: 图片不是可完整解码的 PNG，未发起请求";
const IMAGE_LIMIT: &str =
    "VISION_IMAGE_LIMIT: 请使用至多 384 KiB 的静态 PNG，每边至多 2048 像素，总像素至多 2097152";
const PROMPT_LIMIT: &str = "VISION_PROMPT_LIMIT: 图片问题须为 1 至 4000 UTF-8 字节";
const ENCODE_FAILED: &str = "VISION_ENCODE_FAILED: 无法准备 PNG 图片，未发起请求";

pub(crate) fn public_error(error: &str) -> Option<(&'static str, &'static str)> {
    [
        UNSUPPORTED,
        INVALID_INPUT,
        INVALID_PNG,
        IMAGE_LIMIT,
        PROMPT_LIMIT,
        ENCODE_FAILED,
    ]
    .into_iter()
    .find(|message| *message == error)
    .and_then(|message| message.split_once(": "))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request<'a> {
    prompt: &'a str,
    png_base64: &'a str,
}

pub(crate) fn supported(route: &ModelCapability) -> bool {
    route.provider_id == "openrouter"
        && route.protocol == "chat"
        && route.source == "api_catalog"
        && !route.model_id.starts_with("openrouter/")
        && !route.model_id.starts_with('~')
        && ["text", "vision"]
            .iter()
            .all(|name| route.functions.iter().any(|item| item == name))
}

pub(super) fn schema() -> Value {
    json!({
        "type":"object","required":["vision"],"additionalProperties":false,
        "properties":{"vision":{
            "type":"object","required":["prompt","png_base64"],"additionalProperties":false,
            "properties":{
                "prompt":{"type":"string","minLength":1,"maxLength":MAX_PROMPT_BYTES},
                "png_base64":{"type":"string","minLength":1,"maxLength":MAX_ENCODED_BYTES,
                    "contentEncoding":"base64","contentMediaType":"image/png"}
            }
        }}
    })
}

fn normalize_png(original: &[u8], max_bytes: usize, max_edge: u32, max_pixels: u32) -> Result<Vec<u8>, String> {
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(original),
        png::Limits {
            bytes: max_pixels as usize * 8,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|_| INVALID_PNG)?;
    let info = reader.info();
    if info.width > max_edge
        || info.height > max_edge
        || info.width * info.height > max_pixels
        || info.animation_control.is_some()
    {
        return Err(IMAGE_LIMIT.into());
    }
    let mut pixels = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut pixels).map_err(|_| INVALID_PNG)?;
    reader.finish().map_err(|_| INVALID_PNG)?;
    // Only the decoded frame is sent. Text, EXIF, profiles and trailing bytes
    // from the source file are never included.
    let mut normalized = Vec::new();
    let mut encoder = png::Encoder::new(&mut normalized, frame.width, frame.height);
    encoder.set_color(frame.color_type);
    encoder.set_depth(frame.bit_depth);
    let mut writer = encoder.write_header().map_err(|_| ENCODE_FAILED)?;
    writer
        .write_image_data(&pixels[..frame.buffer_size()])
        .map_err(|_| ENCODE_FAILED)?;
    writer.finish().map_err(|_| ENCODE_FAILED)?;
    if normalized.len() > max_bytes {
        return Err(IMAGE_LIMIT.into());
    }
    Ok(normalized)
}

pub(super) fn prepare(route: &ModelCapability, input: &Value) -> Result<(String, Value), String> {
    if !supported(route) {
        return Err(UNSUPPORTED.into());
    }
    if input.as_object().is_none_or(|input| input.len() != 1) {
        return Err(INVALID_INPUT.into());
    }
    let request = Request::deserialize(&input["vision"]).map_err(|_| INVALID_INPUT)?;
    if request.prompt.trim().is_empty()
        || request.prompt.len() > MAX_PROMPT_BYTES
        || request.prompt.contains('\0')
    {
        return Err(PROMPT_LIMIT.into());
    }
    let image = normalize_attachment(request.png_base64)?;
    let mut body = json!({
        "model": route.model_id,
        "messages": [{"role":"user","content":[
            {"type":"text","text":request.prompt},
            {"type":"image_url","image_url":{"url":format!("data:image/png;base64,{}",STANDARD.encode(image))}}
        ]}],
        "max_tokens":1200,"modalities":["text"],"stream":false,
        "provider":{"require_parameters":true,"allow_fallbacks":false}
    });
    if route.model_id.ends_with(":free") {
        body["provider"]["max_price"] = json!({"prompt":0,"completion":0,"request":0,"image":0});
    }
    Ok((endpoint(route, input)?, body))
}

pub(crate) fn normalize_attachment(encoded: &str) -> Result<Vec<u8>, String> {
    normalize_base64_png(encoded, MAX_IMAGE_BYTES, MAX_EDGE, MAX_PIXELS)
}

pub(crate) fn normalize_generated_png(encoded: &str) -> Result<Vec<u8>, String> {
    normalize_base64_png(encoded, 8 * 1024 * 1024, 4096, 8_294_400)
}

fn normalize_base64_png(encoded: &str, max_bytes: usize, max_edge: u32, max_pixels: u32) -> Result<Vec<u8>, String> {
    if encoded.is_empty() || encoded.len() > max_bytes.div_ceil(3) * 4 {
        return Err(IMAGE_LIMIT.into());
    }
    let image = STANDARD
        .decode(encoded)
        .map_err(|_| INVALID_PNG)?;
    if image.len() > max_bytes {
        return Err(IMAGE_LIMIT.into());
    }
    normalize_png(&image, max_bytes, max_edge, max_pixels)
}

pub(crate) fn has_complete_text(response: &Value) -> bool {
    let choice = &response["choices"][0];
    let message = &choice["message"];
    choice["finish_reason"] == "stop"
        && message["content"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty())
        && message
            .get("tool_calls")
            .is_none_or(|calls| calls.is_null() || calls.as_array().is_some_and(Vec::is_empty))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn route() -> ModelCapability {
        super::super::model(
            "openrouter",
            "fixture/multimodal:free",
            "Fixture",
            "fixture",
            &["text", "vision"],
            "chat",
            "https://openrouter.ai/docs",
            "api_catalog",
        )
    }

    fn synthetic_png(width: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder
            .add_text_chunk("Synthetic".into(), "exclude this metadata".into())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[17, 34, 51].repeat(width as usize))
            .unwrap();
        writer.finish().unwrap();
        bytes
    }

    fn input(image: &[u8]) -> Value {
        json!({"vision":{"prompt":"描述这张合成图片","png_base64":STANDARD.encode(image)}})
    }

    #[test]
    fn only_explicit_catalog_routes_and_safe_images_pass() {
        let route = route();
        let request = input(&synthetic_png(1));
        assert_eq!(
            route.input_schema["anyOf"][1]["required"],
            json!(["vision"])
        );
        assert!(super::super::prepare(&route, &request).is_ok());
        assert!(super::super::prepare(
            &route,
            &json!({"messages":[{"role":"user","content":"普通文字"}]})
        )
        .is_ok());
        assert!(super::super::prepare(&route, &json!({"stream":false})).is_err());
        let mut mixed = request.clone();
        mixed["messages"] = json!([]);
        assert!(super::super::prepare(&route, &mixed).is_err());
        let mut unverified = route.clone();
        unverified.functions.retain(|function| function != "vision");
        let error = super::super::prepare(&unverified, &request).unwrap_err();
        assert_eq!(public_error(&error).unwrap().0, "VISION_UNSUPPORTED");
        assert!(public_error(&format!("{error}: private data")).is_none());
        let mut alias = route.clone();
        alias.model_id = "openrouter/auto".into();
        assert!(super::super::prepare(&alias, &request).is_err());
        let mut different_provider = route.clone();
        different_provider.provider_id = "dashscope".into();
        assert!(super::super::prepare(&different_provider, &request).is_err());
        assert!(prepare(&route, &input(b"<svg>spoofed image</svg>")).is_err());
        assert!(prepare(&route, &input(&synthetic_png(MAX_EDGE + 1))).is_err());
        let mut truncated = synthetic_png(1);
        truncated.truncate(38);
        assert!(prepare(&route, &input(&truncated)).is_err());
        assert!(prepare(&route, &input(&vec![0; MAX_IMAGE_BYTES + 1])).is_err());
        let response =
            json!({"choices":[{"finish_reason":"stop","message":{"content":"合成像素"}}]});
        assert!(has_complete_text(&response));
        let mut incomplete = response.clone();
        incomplete["choices"][0]["finish_reason"] = json!("length");
        assert!(!has_complete_text(&incomplete));
        let mut tool_call = response;
        tool_call["choices"][0]["message"]["tool_calls"] = json!([{"id":"synthetic"}]);
        assert!(!has_complete_text(&tool_call));
    }

    #[tokio::test]
    async fn loopback_receives_pixels_without_source_metadata_or_provider_fallback() {
        let route = route();
        let (endpoint, expected) =
            super::super::prepare(&route, &input(&synthetic_png(1))).unwrap();
        assert_eq!(endpoint, "https://openrouter.ai/api/v1/chat/completions");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = tokio::io::BufReader::new(stream);
            use tokio::io::AsyncBufReadExt;
            let mut count = None;
            loop {
                let mut line = String::new();
                assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(length) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    count = Some(length.trim().parse::<usize>().unwrap());
                }
            }
            let mut body = vec![0; count.unwrap()];
            stream.read_exact(&mut body).await.unwrap();
            let wire: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(wire["model"], "fixture/multimodal:free");
            assert_eq!(wire["provider"]["allow_fallbacks"], false);
            assert_eq!(wire["provider"]["max_price"]["image"], 0);
            assert_eq!(wire["modalities"], json!(["text"]));
            let data = wire["messages"][0]["content"][1]["image_url"]["url"]
                .as_str()
                .unwrap();
            let png = STANDARD
                .decode(data.strip_prefix("data:image/png;base64,").unwrap())
                .unwrap();
            assert!(!png
                .windows("exclude this metadata".len())
                .any(|part| part == b"exclude this metadata"));
            let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
            let mut pixels = vec![0; reader.output_buffer_size()];
            let frame = reader.next_frame(&mut pixels).unwrap();
            assert_eq!(&pixels[..frame.buffer_size()], &[17, 34, 51]);
            let reply =
                br#"{"choices":[{"finish_reason":"stop","message":{"content":"fixture"}}]}"#;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        reply.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(reply).await.unwrap();
        });
        let response = crate::model_capability_io::client()
            .unwrap()
            .post(format!("http://{address}/api/v1/chat/completions"))
            .json(&expected)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert!(has_complete_text(&response));
        peer.await.unwrap();
    }
}
