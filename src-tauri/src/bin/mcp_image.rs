//! homebase-mcp-image — MCP stdio server，给 Studio 的 Claude Code 提供生图能力
//!
//! 设计目标（Phase E2）：
//! - **self-contained binary**：编译进 HomeBase 安装目录，不依赖 npm / Node
//! - **stdin/stdout JSON-RPC 2.0**：MCP 协议 2025-06-18 规范
//! - **走 OpenRouter Image API**：与 chat 共用 `OPENROUTER_API_KEY`
//! - **生成图片落 attachments/**：默认 cwd 下，env `HB_IMAGE_OUTPUT_DIR` 可覆盖
//! - **阶段进度文件**：~/.homebase/img-progress/<pid>.json · 前端 polling · 完成或错误后删除
//!
//! 调用契约（被 Claude Code 通过 .mcp.json 启动）：
//! - env `OPENROUTER_API_KEY`：必需，从 SOPS 解密后由父进程注入
//! - env `HB_IMAGE_MODEL`：可选 默认·工具参数 model 会覆盖该 env
//! - env `HB_IMAGE_OUTPUT_DIR`：可选，默认 `attachments`（相对 cwd）
//!
//! Tool exposed: `generate_image(prompt: string, model?: string, filename_hint?: string)`
//! - 返回 `content[0].text` 包含 文件绝对路径 + Markdown image 语法
//! - Claude 可直接复制 markdown 插入文章

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// 默认模型 · Nano Banana 2 · 质量高 + 价格低 + 速度快 · 一般配图首选
/// · system prompt 会指导 Claude 按场景覆写 · 只有在太智选不出才走默认
const DEFAULT_MODEL: &str = "google/gemini-3.1-flash-image-preview";
const DEFAULT_OUTPUT_DIR: &str = "attachments";
const OR_API_BASE: &str = "https://openrouter.ai/api/v1/chat/completions";
const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "homebase-mcp-image";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// =====================================================================
// 阶段进度 helpers · 写 ~/.homebase/img-progress/<pid>.json · 供前端 polling
// =====================================================================

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 进度目录 · ~/.homebase/img-progress/
fn progress_dir() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    let dir = PathBuf::from(home).join(".homebase").join("img-progress");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// 进度文件路径 · 以 pid 命名
fn progress_file() -> Option<PathBuf> {
    let pid = std::process::id();
    Some(progress_dir()?.join(format!("{}.json", pid)))
}

#[derive(Clone)]
struct ProgressCtx {
    started_at_ms: u64,
    model: String,
    prompt_preview: String,
}

fn write_progress(ctx: &ProgressCtx, stage: &str, output_path: Option<&str>, error: Option<&str>) {
    if let Some(path) = progress_file() {
        let now = now_ms();
        let v = json!({
            "pid": std::process::id(),
            "stage": stage,
            "started_at_ms": ctx.started_at_ms,
            "updated_at_ms": now,
            "elapsed_ms": now.saturating_sub(ctx.started_at_ms),
            "model": ctx.model,
            "prompt_preview": ctx.prompt_preview,
            "output_path": output_path,
            "error": error,
        });
        let s = serde_json::to_string(&v).unwrap_or_default();
        let _ = std::fs::write(&path, s);
    }
}

fn clear_progress() {
    if let Some(path) = progress_file() {
        let _ = std::fs::remove_file(&path);
    }
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF · client closed pipe
            Ok(_) => {}
            Err(e) => {
                eprintln!("[mcp-image] read stdin failed: {}", e);
                break;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[mcp-image] parse json failed: {} (line: {})", e, trimmed);
                continue;
            }
        };

        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned();
        // notification (无 id) · 无需响应
        let is_notification = id.is_none();

        let response = handle_request(method, &req);

        if !is_notification {
            let resp_obj = match response {
                Ok(result) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result,
                }),
                Err((code, msg)) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": code, "message": msg },
                }),
            };
            let line_out = serde_json::to_string(&resp_obj).unwrap_or_default();
            let mut h = stdout.lock();
            let _ = h.write_all(line_out.as_bytes());
            let _ = h.write_all(b"\n");
            let _ = h.flush();
        }
    }
}

fn handle_request(method: &str, req: &Value) -> Result<Value, (i32, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
            },
            "capabilities": {
                "tools": {}
            }
        })),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": [
                {
                    "name": "generate_image",
                    "description": "生成一张图片并保存到本地。· 可选多模型·默认 google/gemini-3.1-flash-image-preview (Nano Banana 2 · 价低质高·一般配图首选)· 路由策略：含中文文字/海报/信息图→openai/gpt-5.4-image-2·含人物/人像/摄影/多人→google/gemini-3-pro-image-preview·含插画/艺术/抽象→black-forest-labs/flux.2-pro。返回 markdown 语法·直接复制插入文章。",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "prompt": {
                                "type": "string",
                                "description": "图片描述 · 中文或英文 · 至少 30 字 · 包含风格 / 主体 / 色调 / 构图 / 光线"
                            },
                            "model": {
                                "type": "string",
                                "description": "可选 · OpenRouter 模型 slug · 仅限 4 个选项：【openai/gpt-5.4-image-2】(文字/海报)、【google/gemini-3-pro-image-preview】(人像/多人)、【google/gemini-3.1-flash-image-preview】(默认)、【black-forest-labs/flux.2-pro】(插画)。不传走 env HB_IMAGE_MODEL 默认。",
                                "enum": [
                                    "openai/gpt-5.4-image-2",
                                    "google/gemini-3-pro-image-preview",
                                    "google/gemini-3.1-flash-image-preview",
                                    "black-forest-labs/flux.2-pro"
                                ]
                            },
                            "filename_hint": {
                                "type": "string",
                                "description": "可选 · 文件名前缀 · 不带扩展名 · 例如 \"sunset-mountains\" · 不传则用 prompt 哈希"
                            }
                        },
                        "required": ["prompt"]
                    }
                }
            ]
        })),
        "tools/call" => handle_tool_call(req),
        // 未知方法 · 标准 JSON-RPC -32601
        _ => Err((-32601, format!("Method not found: {}", method))),
    }
}

fn handle_tool_call(req: &Value) -> Result<Value, (i32, String)> {
    let params = req
        .get("params")
        .ok_or_else(|| (-32602, "missing params".into()))?;
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name != "generate_image" {
        return Err((-32601, format!("Unknown tool: {}", name)));
    }

    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let prompt = args
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (-32602, "missing prompt".into()))?
        .to_string();
    let filename_hint = args
        .get("filename_hint")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    // **多模型路由**：Claude 传入的 model 优先 · 其次 env · 最后默认
    let model = args
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| std::env::var("HB_IMAGE_MODEL").ok())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    // 进度上下文
    let ctx = ProgressCtx {
        started_at_ms: now_ms(),
        model: model.clone(),
        prompt_preview: prompt.chars().take(80).collect::<String>(),
    };
    write_progress(&ctx, "preparing", None, None);

    // 必需 env：OR key
    let or_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            let msg = "OPENROUTER_API_KEY env not set".to_string();
            write_progress(&ctx, "error", None, Some(&msg));
            // 即刻清 · 前端拿不到最后一帧也 OK · ToolCard 会看到 is_error=true
            clear_progress();
            return Err((-32603, msg));
        }
    };
    let output_dir =
        std::env::var("HB_IMAGE_OUTPUT_DIR").unwrap_or_else(|_| DEFAULT_OUTPUT_DIR.to_string());

    eprintln!(
        "[mcp-image] generate_image · model={} prompt_chars={} hint={:?} output_dir={}",
        model,
        prompt.chars().count(),
        filename_hint,
        output_dir
    );

    // 调 OR Image API（chat.completions + modalities=["image","text"]）
    let body = json!({
        "model": model,
        "messages": [
            { "role": "user", "content": prompt }
        ],
        "modalities": ["image", "text"]
    });

    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("client build: {}", e);
            write_progress(&ctx, "error", None, Some(&msg));
            clear_progress();
            return Err((-32000, msg));
        }
    };

    write_progress(&ctx, "sending", None, None);

    let send_result = client
        .post(OR_API_BASE)
        .bearer_auth(&or_key)
        .header("HTTP-Referer", "https://homebase.local")
        .header("X-Title", "HomeBase Studio")
        .json(&body)
        .send();

    // **同步 reqwest · send() 阅发完返回 Response（包含 headers + body 还在路上）
    // 实际在 .json() 阅读 body 才是本质的“等响应”。为阶段进度体验
    // · 我们 send 后立即设 waiting。
    write_progress(&ctx, "waiting", None, None);

    let resp = match send_result {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("OR request: {}", e);
            write_progress(&ctx, "error", None, Some(&msg));
            clear_progress();
            return Err((-32000, msg));
        }
    };

    let status = resp.status();
    let resp_json: Value = match resp.json() {
        Ok(j) => j,
        Err(e) => {
            let msg = format!("OR parse: {}", e);
            write_progress(&ctx, "error", None, Some(&msg));
            clear_progress();
            return Err((-32000, msg));
        }
    };
    if !status.is_success() {
        let err_msg = resp_json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("(no error message)")
            .to_string();
        let combined = format!("OR API {} · {}", status, err_msg);
        write_progress(&ctx, "error", None, Some(&combined));
        clear_progress();
        return Err((-32000, combined));
    }

    // 抽 image data URL（OR 文档约定 message.images[0].imageUrl.url）
    let image_url = match resp_json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("images"))
        .and_then(|imgs| imgs.get(0))
        .and_then(|img| img.get("imageUrl"))
        .and_then(|u| u.get("url"))
        .and_then(|s| s.as_str())
    {
        Some(s) => s.to_string(),
        None => {
            let msg = format!(
                "OR response missing message.images[0].imageUrl.url · raw: {}",
                resp_json.to_string().chars().take(200).collect::<String>()
            );
            write_progress(&ctx, "error", None, Some(&msg));
            clear_progress();
            return Err((-32000, msg));
        }
    };

    write_progress(&ctx, "decoding", None, None);

    // 解 base64 data URL
    let (mime, b64) = match parse_data_url(&image_url) {
        Some(t) => t,
        None => {
            let msg = "image is not a data URL".to_string();
            write_progress(&ctx, "error", None, Some(&msg));
            clear_progress();
            return Err((-32000, msg));
        }
    };
    let ext = mime_to_ext(mime);

    use base64::Engine;
    let bytes = match base64::prelude::BASE64_STANDARD.decode(b64) {
        Ok(b) => b,
        Err(e) => {
            let msg = format!("base64 decode: {}", e);
            write_progress(&ctx, "error", None, Some(&msg));
            clear_progress();
            return Err((-32000, msg));
        }
    };

    write_progress(&ctx, "saving", None, None);

    // 落盘
    let dir = std::path::PathBuf::from(&output_dir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        let msg = format!("mkdir {}: {}", dir.display(), e);
        write_progress(&ctx, "error", None, Some(&msg));
        clear_progress();
        return Err((-32000, msg));
    }

    let stem = filename_hint
        .as_deref()
        .map(sanitize_filename)
        .unwrap_or_else(|| short_hash(&prompt));
    let filename = format!("{}.{}", stem, ext);
    let path = dir.join(&filename);
    if let Err(e) = std::fs::write(&path, &bytes) {
        let msg = format!("write file: {}", e);
        write_progress(&ctx, "error", None, Some(&msg));
        clear_progress();
        return Err((-32000, msg));
    }

    let abs = path.canonicalize().unwrap_or(path);
    let mut abs_str = abs.to_string_lossy().to_string();
    // strip Windows extended-length prefix `\\?\` for cleaner markdown
    if let Some(stripped) = abs_str.strip_prefix(r"\\?\") {
        abs_str = stripped.to_string();
    }

    eprintln!(
        "[mcp-image] saved {} · {} bytes",
        abs_str,
        bytes.len()
    );

    write_progress(&ctx, "done", Some(&abs_str), None);
    // 给前端 polling 200ms 机会拿到 done 帧 · 然后清理
    std::thread::sleep(std::time::Duration::from_millis(200));
    clear_progress();

    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": format!(
                    "Image generated and saved.\n\nPath: {}\nSize: {} bytes\nModel: {}\n\nMarkdown to insert:\n![]({})",
                    abs_str,
                    bytes.len(),
                    model,
                    abs_str.replace('\\', "/")
                )
            }
        ]
    }))
}

/// 解 `data:image/png;base64,XXXX` 格式的 URL · 返回 (mime, b64_payload)
fn parse_data_url(url: &str) -> Option<(&str, &str)> {
    let s = url.strip_prefix("data:")?;
    let (header, b64) = s.split_once(',')?;
    let mime = header.split(';').next()?;
    Some((mime, b64))
}

fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    }
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

fn short_hash(input: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    input.hash(&mut h);
    let n = h.finish();
    format!("img-{:08x}", n & 0xFFFF_FFFF)
}
