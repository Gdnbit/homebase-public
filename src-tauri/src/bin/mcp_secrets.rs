//! homebase-mcp-secrets · MCP stdio server · 给 4 agent 提供共享 secret 拿取能力（S3）
//!
//! 设计目标（与 `homebase-mcp-image` 一致）：
//! - **self-contained binary**：不依赖 lib · 不依赖 Tauri · 仅 std + serde_json + tokio
//! - **stdin/stdout JSON-RPC 2.0**：MCP 协议 2025-06-18 规范
//! - **反向 IPC 调 HomeBase**：connect `~/.homebase/shared/.ipc-secrets.json` 描述的端点
//!
//! 调用契约（被 agent 通过 mcp 配置启动）：
//! - env `HB_AGENT_ID`：agent 标识（claude-code / codex / gemini-cli / openclaw）
//!   - 由 reconcile 写入 mcp 配置时附带 · binary 用它给 IPC server 标记是哪个 agent 在请求
//!   - 缺省回退 `unknown`（HomeBase 端会拒绝非 4 个白名单的 agent_id · 或者按 unknown 处理）
//!
//! Tool exposed: `get_secret(name: string, purpose: string)`
//! - `name`：secrets.yaml 顶层 key（如 `openrouter_key`）
//! - `purpose`：向 USER 解释为什么要 · 弹窗里会显示
//! - 返回 `content[0].text` = secret 值（agent 拿了直接用）
//!
//! 错误码：
//! - -32001 endpoint 文件不存在 → HomeBase 没运行
//! - -32002 USER 拒绝
//! - -32003 HomeBase UI 未就绪
//! - -32005 60s timeout
//! - -32006 SOPS 解密失败

use std::io::{BufRead as _, BufReader as IoBufReader, Write as _};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::TcpStream;

const PROTOCOL_VERSION: &str = "2025-06-18";
const SERVER_NAME: &str = "homebase-mcp-secrets";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize, Serialize, Clone)]
struct IpcEndpoint {
    endpoint: String,
    token: String,
}

fn endpoint_file_path() -> std::path::PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".homebase")
        .join("shared")
        .join(".ipc-secrets.json")
}

fn load_endpoint() -> Result<IpcEndpoint, String> {
    let path = endpoint_file_path();
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读 .ipc-secrets.json ({}): {e}", path.display()))?;
    serde_json::from_str::<IpcEndpoint>(&text).map_err(|e| format!("parse endpoint json: {e}"))
}

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = IoBufReader::new(stdin.lock());
    let mut line = String::new();

    // mcp-secrets 是同步 stdio 循环 · 但单次 tool/call 走 tokio runtime 调 IPC
    let rt = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[mcp-secrets] 创建 tokio runtime 失败: {e}");
            return;
        }
    };

    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("[mcp-secrets] read stdin: {e}");
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
                eprintln!("[mcp-secrets] json parse: {e} · line: {trimmed}");
                continue;
            }
        };

        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned();
        let is_notification = id.is_none();

        let response = handle_request(method, &req, &rt);
        if is_notification {
            continue;
        }
        let resp_obj = match response {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, msg)) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": msg }
            }),
        };
        let line_out = serde_json::to_string(&resp_obj).unwrap_or_default();
        let mut h = stdout.lock();
        let _ = h.write_all(line_out.as_bytes());
        let _ = h.write_all(b"\n");
        let _ = h.flush();
    }
}

fn handle_request(
    method: &str,
    req: &Value,
    rt: &tokio::runtime::Runtime,
) -> Result<Value, (i32, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
            "capabilities": { "tools": {} }
        })),
        "notifications/initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": [
                {
                    "name": "get_secret",
                    "description": "获取一个共享 secret（如 OpenRouter / Anthropic / GitHub Token）。HomeBase 会向 USER 弹窗确认 · 同意后返回明文值；USER 可选『仅这一次』或『永久允许』。请在 purpose 字段简洁说明用途（10-30 字 · USER 会看到）。",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "secret 名 = secrets.yaml 顶层 key · 如 openrouter_key / anthropic_key / github_token"
                            },
                            "purpose": {
                                "type": "string",
                                "description": "用途描述 · 中文 · 10-30 字 · USER 弹窗里会看到 · 例：调用 OpenRouter 生图 / 推送 GitHub release"
                            }
                        },
                        "required": ["name", "purpose"]
                    }
                }
            ]
        })),
        "tools/call" => rt.block_on(handle_tool_call(req)),
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}

async fn handle_tool_call(req: &Value) -> Result<Value, (i32, String)> {
    let params = req
        .get("params")
        .ok_or_else(|| (-32602, "missing params".to_string()))?;
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name != "get_secret" {
        return Err((-32601, format!("Unknown tool: {name}")));
    }
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let secret_name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (-32602, "missing argument: name".to_string()))?
        .to_string();
    let purpose = args
        .get("purpose")
        .and_then(|v| v.as_str())
        .ok_or_else(|| (-32602, "missing argument: purpose".to_string()))?
        .to_string();
    let agent_id = std::env::var("HB_AGENT_ID").unwrap_or_else(|_| "unknown".to_string());

    eprintln!(
        "[mcp-secrets] get_secret · agent={} secret_name={} purpose={}",
        agent_id, secret_name, purpose
    );

    // 走 IPC
    let ep = load_endpoint().map_err(|e| (-32001, e))?;
    let secret = request_secret_via_ipc(&ep, &agent_id, &secret_name, &purpose)
        .await
        .map_err(|(c, m)| (c, m))?;

    Ok(json!({
        "content": [
            { "type": "text", "text": secret }
        ]
    }))
}

/// 实际 TCP 调用：connect → auth → secrets/request → 拿 secret
async fn request_secret_via_ipc(
    ep: &IpcEndpoint,
    agent_id: &str,
    secret_name: &str,
    purpose: &str,
) -> Result<String, (i32, String)> {
    let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&ep.endpoint))
        .await
        .map_err(|_| (-32001, "连 HomeBase IPC 超时(5s)".to_string()))?
        .map_err(|e| (-32001, format!("连 HomeBase IPC: {e}")))?;

    let (read, mut write) = stream.into_split();
    let mut reader = TokioBufReader::new(read);

    // auth
    let auth = json!({ "method": "auth", "params": { "token": ep.token } });
    write
        .write_all(auth.to_string().as_bytes())
        .await
        .map_err(|e| (-32001, format!("write auth: {e}")))?;
    write.write_all(b"\n").await.ok();
    write
        .flush()
        .await
        .map_err(|e| (-32001, format!("flush auth: {e}")))?;

    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| (-32001, format!("read auth resp: {e}")))?;
    if !line.contains("\"ok\"") {
        return Err((-32001, format!("auth 失败: {}", line.trim())));
    }

    // 真正的 secrets/request
    let req_id = uuid_simple();
    let req = json!({
        "id": req_id,
        "method": "secrets/request",
        "params": {
            "agent_id": agent_id,
            "secret_name": secret_name,
            "purpose": purpose,
        }
    });
    write
        .write_all(req.to_string().as_bytes())
        .await
        .map_err(|e| (-32001, format!("write request: {e}")))?;
    write.write_all(b"\n").await.ok();
    write
        .flush()
        .await
        .map_err(|e| (-32001, format!("flush request: {e}")))?;

    line.clear();
    // 60s+5s 余量（HomeBase 60s timeout）
    tokio::time::timeout(Duration::from_secs(65), reader.read_line(&mut line))
        .await
        .map_err(|_| (-32005, "等待 HomeBase 响应超时".to_string()))?
        .map_err(|e| (-32001, format!("read response: {e}")))?;

    let resp: Value = serde_json::from_str(line.trim())
        .map_err(|e| (-32001, format!("parse response json: {e}")))?;
    if let Some(err) = resp.get("error") {
        let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-32000) as i32;
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("(no message)")
            .to_string();
        return Err((code, msg));
    }
    let secret = resp
        .pointer("/result/secret")
        .and_then(|s| s.as_str())
        .ok_or_else(|| (-32001, "响应缺 result.secret".to_string()))?;
    Ok(secret.to_string())
}

/// 32-char hex uuid · 不引外部依赖 · 用 std + 时间 + tid 拼一个伪 uuid 即够（IPC 内部 id · 不需要密码学强度）
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    // 16 字节熵 · 时间戳 + pid 高位
    let combined: u128 = nanos ^ (pid << 96);
    format!("{:032x}", combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_path_uses_userprofile() {
        // 不深入测试 · 验证函数返回非空路径即可
        let p = endpoint_file_path();
        assert!(p.ends_with(".ipc-secrets.json"));
    }

    #[test]
    fn load_endpoint_errors_when_missing() {
        // 把 USERPROFILE 指到一个肯定不存在的目录
        let prev = std::env::var("USERPROFILE").ok();
        std::env::set_var("USERPROFILE", r"C:\definitely\not\exist\xyz123");
        let res = load_endpoint();
        assert!(res.is_err());
        match prev {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
    }

    #[test]
    fn uuid_simple_returns_32_hex() {
        let u = uuid_simple();
        assert_eq!(u.len(), 32);
        assert!(u.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
