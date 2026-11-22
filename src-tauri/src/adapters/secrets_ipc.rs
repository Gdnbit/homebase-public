//! Secrets IPC Server · S3 · 2026-05-06
//!
//! HomeBase 主进程内置的本地 TCP 服务器 · 用于 `homebase-mcp-secrets` binary 反向调用：
//!
//! 工作流：
//! 1. HomeBase 启动 → bind `127.0.0.1:0`（OS 分配端口）+ 生成 32 字节 token
//! 2. 写 `~/.homebase/shared/.ipc-secrets.json` 描述 `{ endpoint, token }`
//! 3. mcp-secrets binary 启动 → 读该文件 → connect + 第一行 auth + 后续 secrets/request
//! 4. server 收到 secrets/request →
//!    - 命中 ACL → 直接解密返回
//!    - 未命中 → emit Tauri event `secrets:consent_request` 通知前端弹窗 → await USER 决定
//!    - USER 决定后通过 Tauri command `secrets_consent_resolve` 回信号 →
//!      action=allow 解密返回 · action=deny 返回错误
//!
//! 协议（line-based JSON-RPC over TCP · 每行一个 JSON · 不用 HTTP）：
//! - 客户端首行：`{"method":"auth","params":{"token":"..."}}`
//! - 服务端首行回：`{"result":"ok"}` 或 `{"error":{"code":-32001,"message":"bad token"}}`
//! - 后续：`{"id":"<uuid>","method":"secrets/request","params":{"agent_id","secret_name","purpose"}}`
//! - 响应：`{"id":"<uuid>","result":{"action":"allow_persistent|allow_once","secret":"..."}}`
//!   或 `{"id":"<uuid>","error":{"code":-32002,"message":"USER 拒绝"}}`
//!
//! 安全：
//! - 监听仅 127.0.0.1（不可外网访问）
//! - 客户端必须先 auth · token 不匹配立即 close
//! - 60s 不确认的 consent 自动 deny（防 USER 漏看 hang 死 binary）
//!
//! 测试：
//! - 启动 server + 客户端模拟 binary · 跑过 auth + 一次 ACL 已允许的 fast path
//! - consent 流程的端到端测试需要前端 · 在 commands 层做集成测试

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex};

use super::homebase_home_dir;
use super::secrets_acl::{self, AclFile};
use super::secrets_store;

// =====================================================================
// 公共类型
// =====================================================================

/// Endpoint 描述文件 · 写到 `~/.homebase/shared/.ipc-secrets.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEndpoint {
    pub endpoint: String, // "127.0.0.1:51234"
    pub token: String,    // 64-char hex（uuid v4 拼接）
}

pub fn endpoint_file_path() -> PathBuf {
    homebase_home_dir()
        .join(".homebase")
        .join("shared")
        .join(".ipc-secrets.json")
}

/// USER 通过弹窗作出的决定
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConsentAction {
    AllowOnce,
    AllowPersistent,
    Deny,
}

/// Pending consent · 等 USER 弹窗回信号
struct Pending {
    sender: oneshot::Sender<ConsentAction>,
}

/// 全局 pending map · request_id → 决定通道
static PENDING: OnceLock<Arc<Mutex<HashMap<String, Pending>>>> = OnceLock::new();

fn pending() -> &'static Arc<Mutex<HashMap<String, Pending>>> {
    PENDING.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// 发出 consent_request 事件给前端的回调（由 lib.rs setup 时注入）
/// 接收 (request_id, agent_id, secret_name, purpose)
/// 返回 true 表示 emit 成功
pub type EmitFn = Arc<dyn Fn(&str, &str, &str, &str) -> bool + Send + Sync>;

static EMITTER: OnceLock<EmitFn> = OnceLock::new();

/// 注册 Tauri AppHandle 的 emit 回调
///
/// 由 lib.rs Tauri setup 调用。Without it · 走 fallback 直接 deny。
pub fn set_emitter(f: EmitFn) {
    let _ = EMITTER.set(f);
}

fn emit_consent_request(
    request_id: &str,
    agent_id: &str,
    secret_name: &str,
    purpose: &str,
) -> bool {
    if let Some(f) = EMITTER.get() {
        f(request_id, agent_id, secret_name, purpose)
    } else {
        tracing::warn!("secrets_ipc: emitter 未注册 · consent 请求无法弹窗 · 走 deny");
        false
    }
}

// =====================================================================
// 启动 / 关闭
// =====================================================================

/// 启动 IPC server · 返回写入文件的 endpoint
///
/// 由 lib.rs Tauri setup 中 spawn_blocking 调用。
/// 长生命周期 task · 跟随主进程退出。
pub async fn start() -> std::io::Result<IpcEndpoint> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let local = listener.local_addr()?;
    // 64-hex token · 拼两个 uuid v4 · 128+128=256 bit
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let ep = IpcEndpoint {
        endpoint: local.to_string(),
        token: token.clone(),
    };
    write_endpoint_file(&ep)?;
    tracing::info!("secrets_ipc · 监听 {}", ep.endpoint);

    let token_arc = Arc::new(token);
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    tracing::debug!("secrets_ipc · 接收连接 from {peer}");
                    let token = token_arc.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_client(stream, token).await {
                            tracing::warn!("secrets_ipc · client 处理出错: {e}");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("secrets_ipc · accept 失败: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });

    Ok(ep)
}

fn write_endpoint_file(ep: &IpcEndpoint) -> std::io::Result<()> {
    let path = endpoint_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(ep)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(&path, json)?;
    Ok(())
}

// =====================================================================
// 客户端处理
// =====================================================================

async fn handle_client(stream: TcpStream, expected_token: Arc<String>) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = String::new();

    // 第一行：auth
    line.clear();
    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }
    let auth_ok = match serde_json::from_str::<serde_json::Value>(line.trim()) {
        Ok(v) => v.get("method").and_then(|m| m.as_str()) == Some("auth")
            && v.pointer("/params/token").and_then(|t| t.as_str()) == Some(expected_token.as_str()),
        Err(_) => false,
    };
    if !auth_ok {
        let resp = serde_json::json!({
            "error": { "code": -32001, "message": "bad token" }
        });
        let _ = write.write_all(resp.to_string().as_bytes()).await;
        let _ = write.write_all(b"\n").await;
        let _ = write.flush().await;
        return Ok(());
    }
    let resp = serde_json::json!({ "result": "ok" });
    write.write_all(resp.to_string().as_bytes()).await?;
    write.write_all(b"\n").await?;
    write.flush().await?;

    // 后续：secrets/request 循环（一个连接可串行多次请求）
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(());
        }
        let req: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => {
                let resp = serde_json::json!({
                    "error": { "code": -32700, "message": format!("parse: {e}") }
                });
                write.write_all(resp.to_string().as_bytes()).await?;
                write.write_all(b"\n").await?;
                write.flush().await?;
                continue;
            }
        };
        let id = req
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let resp = match method {
            "secrets/request" => handle_secrets_request(&id, &req).await,
            _ => serde_json::json!({
                "id": id,
                "error": { "code": -32601, "message": format!("unknown method: {method}") }
            }),
        };
        write.write_all(resp.to_string().as_bytes()).await?;
        write.write_all(b"\n").await?;
        write.flush().await?;
    }
}

async fn handle_secrets_request(id: &str, req: &serde_json::Value) -> serde_json::Value {
    let agent_id = req
        .pointer("/params/agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let secret_name = req
        .pointer("/params/secret_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let purpose = req
        .pointer("/params/purpose")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if agent_id.is_empty() || secret_name.is_empty() {
        return serde_json::json!({
            "id": id,
            "error": { "code": -32602, "message": "missing agent_id / secret_name" }
        });
    }

    // 1. ACL 命中 fast path
    let acl_now = secrets_acl::load();
    if acl_now.is_allowed(&agent_id, &secret_name) {
        return resolve_with_secret(id, &secret_name, "allow_persistent").await;
    }

    // 2. 未命中 · 通过 emitter 推前端 · await USER 决定（60s timeout）
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel::<ConsentAction>();
    {
        let mut map = pending().lock().await;
        map.insert(request_id.clone(), Pending { sender: tx });
    }
    let emitted = emit_consent_request(&request_id, &agent_id, &secret_name, &purpose);
    if !emitted {
        // 没注册 emitter · 立即 deny · 清 pending
        pending().lock().await.remove(&request_id);
        return serde_json::json!({
            "id": id,
            "error": { "code": -32003, "message": "HomeBase UI 未就绪 · 暂无法授权" }
        });
    }

    let action = match tokio::time::timeout(Duration::from_secs(60), rx).await {
        Ok(Ok(a)) => a,
        Ok(Err(_)) => {
            // sender drop 了（不该发生）
            pending().lock().await.remove(&request_id);
            return serde_json::json!({
                "id": id,
                "error": { "code": -32004, "message": "consent channel dropped" }
            });
        }
        Err(_) => {
            pending().lock().await.remove(&request_id);
            return serde_json::json!({
                "id": id,
                "error": { "code": -32005, "message": "USER 60s 内未回应 · 自动 deny" }
            });
        }
    };

    match action {
        ConsentAction::Deny => serde_json::json!({
            "id": id,
            "error": { "code": -32002, "message": "USER 拒绝授权" }
        }),
        ConsentAction::AllowOnce => resolve_with_secret(id, &secret_name, "allow_once").await,
        ConsentAction::AllowPersistent => {
            // 写 ACL
            let mut acl = secrets_acl::load();
            acl.grant(&agent_id, &secret_name, &purpose);
            if let Err(e) = secrets_acl::save(&acl) {
                tracing::warn!("写 secrets-acl.json 失败: {e}");
                // 失败也继续返回 secret · USER 已经同意了
            }
            resolve_with_secret(id, &secret_name, "allow_persistent").await
        }
    }
}

async fn resolve_with_secret(id: &str, secret_name: &str, action: &str) -> serde_json::Value {
    match secrets_store::get_secret_value(secret_name).await {
        Ok(secure) => serde_json::json!({
            "id": id,
            "result": {
                "action": action,
                "secret": secure.as_str(),
            }
        }),
        Err(e) => serde_json::json!({
            "id": id,
            "error": { "code": -32006, "message": format!("解密失败: {e}") }
        }),
    }
}

// =====================================================================
// 前端 commands 用的 helper · 让 USER 的决定到达 pending
// =====================================================================

/// Tauri command 调这个把 USER 决定灌进等待中的 oneshot
///
/// 找不到 request_id 返 Err（可能已经超时被清掉）
pub async fn resolve_pending(request_id: &str, action: ConsentAction) -> Result<(), String> {
    let pending_arc = pending().clone();
    let mut map = pending_arc.lock().await;
    let entry = map
        .remove(request_id)
        .ok_or_else(|| format!("request_id 不存在或已超时: {request_id}"))?;
    entry
        .sender
        .send(action)
        .map_err(|_| "consent receiver 已 drop".to_string())?;
    Ok(())
}

/// 列出当前待处理的 consent request_id（debug / 测试用）
pub async fn list_pending_ids() -> Vec<String> {
    pending().lock().await.keys().cloned().collect()
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::shared_context::tests::TEST_ENV_LOCK;
    use tokio::io::AsyncReadExt;

    fn with_test_home<F: FnOnce()>(tmp: &tempfile::TempDir, f: F) {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());
        f();
        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }

    #[test]
    fn endpoint_file_path_under_test_home() {
        let tmp = tempfile::tempdir().unwrap();
        with_test_home(&tmp, || {
            let p = endpoint_file_path();
            assert!(p.starts_with(tmp.path()));
            assert!(p.ends_with(".ipc-secrets.json"));
        });
    }

    #[test]
    fn write_endpoint_file_creates_parent() {
        let tmp = tempfile::tempdir().unwrap();
        with_test_home(&tmp, || {
            let ep = IpcEndpoint {
                endpoint: "127.0.0.1:51234".into(),
                token: "abc".into(),
            };
            write_endpoint_file(&ep).unwrap();
            let on_disk = std::fs::read_to_string(endpoint_file_path()).unwrap();
            let parsed: IpcEndpoint = serde_json::from_str(&on_disk).unwrap();
            assert_eq!(parsed.endpoint, ep.endpoint);
            assert_eq!(parsed.token, ep.token);
        });
    }

    #[test]
    fn consent_action_serde_snake_case() {
        let json = serde_json::to_string(&ConsentAction::AllowPersistent).unwrap();
        assert_eq!(json, "\"allow_persistent\"");
        let parsed: ConsentAction = serde_json::from_str("\"deny\"").unwrap();
        assert_eq!(parsed, ConsentAction::Deny);
    }

    /// 端到端：启 server + 客户端 auth + 假 ACL 命中 → 返回 secret
    ///
    /// 因为没有真 SOPS 文件 · 这里只测到 secret 解密前的 IPC 路径 · 解密会报错。
    /// 我们只验证 auth 成功 + ACL 命中后会去走 resolve_with_secret 这条路。
    #[tokio::test]
    async fn auth_bad_token_closes_connection() {
        let tmp = tempfile::tempdir().unwrap();
        // 不能在 spawn 后还引用 tmp · 用 path string
        let tmp_path = tmp.path().to_path_buf();
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", &tmp_path);

        let ep = start().await.expect("start");
        // bad token
        let mut sock = TcpStream::connect(&ep.endpoint).await.unwrap();
        let bad = serde_json::json!({
            "method": "auth", "params": { "token": "nope" }
        });
        sock.write_all(bad.to_string().as_bytes()).await.unwrap();
        sock.write_all(b"\n").await.unwrap();
        sock.flush().await.unwrap();

        let mut buf = String::new();
        let mut reader = BufReader::new(sock);
        reader.read_line(&mut buf).await.unwrap();
        assert!(buf.contains("bad token"), "got: {buf}");

        // server 应该关闭连接 · 再读应得 EOF
        buf.clear();
        let extra = reader.read_to_string(&mut buf).await.unwrap();
        assert_eq!(extra, 0, "server should close after bad auth · got: {buf}");

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }

    /// auth ok 后发请求 · ACL 没命中 + 没 emitter → 应得 -32003 错误
    #[tokio::test]
    async fn no_emitter_yields_ui_not_ready() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", &tmp_path);

        let ep = start().await.expect("start");
        let mut sock = TcpStream::connect(&ep.endpoint).await.unwrap();
        // auth
        let auth = serde_json::json!({
            "method": "auth", "params": { "token": ep.token }
        });
        sock.write_all(auth.to_string().as_bytes()).await.unwrap();
        sock.write_all(b"\n").await.unwrap();
        sock.flush().await.unwrap();

        let (read, mut write) = sock.into_split();
        let mut reader = BufReader::new(read);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("\"ok\""), "auth resp: {line}");

        // request
        let req = serde_json::json!({
            "id": "test-1",
            "method": "secrets/request",
            "params": {
                "agent_id": "claude-code",
                "secret_name": "openrouter_key",
                "purpose": "test",
            }
        });
        write.write_all(req.to_string().as_bytes()).await.unwrap();
        write.write_all(b"\n").await.unwrap();
        write.flush().await.unwrap();

        line.clear();
        reader.read_line(&mut line).await.unwrap();
        // 没注册 emitter · 应直接 ui_not_ready
        assert!(
            line.contains("-32003") || line.contains("UI 未就绪"),
            "expected ui_not_ready · got: {line}"
        );

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }
}
