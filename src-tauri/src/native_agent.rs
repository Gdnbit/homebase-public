//! Owned stdio App Server connection. UI reads never start/resume a model turn.
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{oneshot, Mutex},
};

pub type ToolHost =
    Arc<dyn Fn(Value) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>> + Send + Sync>;

#[derive(Default)]
pub struct Projection {
    pub threads: BTreeMap<String, Value>,
}
impl Projection {
    pub fn event(&mut self, method: &str, p: &Value) {
        let Some(id) = p["threadId"]
            .as_str()
            .or_else(|| p["thread"]["id"].as_str())
        else {
            return;
        };
        let v = self.threads.entry(id.into()).or_insert_with(
            || json!({"id":id,"state":"idle","items":{},"item_order":[],"error":null}),
        );
        let item_id = p["item"]["id"].as_str().or_else(|| p["itemId"].as_str());
        if let Some(item) = item_id {
            if !v["item_order"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == item)
            {
                v["item_order"].as_array_mut().unwrap().push(json!(item));
            }
            if v["item_order"].as_array().unwrap().len() > 128 {
                let old = v["item_order"].as_array_mut().unwrap().remove(0);
                if let Some(old) = old.as_str() {
                    v["items"].as_object_mut().unwrap().remove(old);
                }
                v["truncated"] = json!(true);
            }
        }
        match method {
            "turn/started" => {
                if v["completed_turn_id"] == p["turn"]["id"] && !p["turn"]["id"].is_null() {
                    return;
                }
                v["state"] = json!("inProgress");
                v["active_turn_id"] = p["turn"]["id"].clone();
                v["error"] = Value::Null;
            }
            "turn/completed" => {
                if v["active_turn_id"]
                    .as_str()
                    .is_some_and(|id| p["turn"]["id"].as_str().is_some_and(|done| id != done))
                {
                    return;
                }
                v["state"] = p["turn"]["status"].clone();
                v["error"] = p["turn"]["error"].clone();
                v["active_turn_id"] = Value::Null;
                v["completed_turn_id"] = p["turn"]["id"].clone();
            }
            "item/started" | "item/completed" => {
                if let Some(item) = p["item"]["id"].as_str() {
                    v["items"][item] = p["item"].clone();
                }
            }
            "item/agentMessage/delta" => {
                if let Some(item) = p["itemId"].as_str() {
                    if v["items"][item].is_null() {
                        v["items"][item] = json!({"id":item,"type":"agentMessage","text":""});
                    }
                    let text = format!(
                        "{}{}",
                        v["items"][item]["text"].as_str().unwrap_or(""),
                        p["delta"].as_str().unwrap_or("")
                    );
                    v["items"][item]["text"] = json!(text);
                }
            }
            "error" => {
                v["error"] = p["error"].clone();
                if p["willRetry"] != true {
                    v["state"] = json!("failed");
                }
            }
            _ => {}
        }
    }
    pub fn hydrate(&mut self, thread: Value) {
        if let Some(id) = thread["id"].as_str() {
            let id = id.to_string();
            let existing = self.threads.remove(&id);
            if self.threads.len() >= 32 {
                let candidate = self
                    .threads
                    .iter()
                    .find(|(_, v)| v["state"] != "inProgress")
                    .map(|(k, _)| k.clone());
                if let Some(key) = candidate {
                    self.threads.remove(&key);
                }
            }
            let mut v = json!({"id":id,"state":"idle","items":{},"item_order":[],"active_turn_id":null,"error":null});
            if let Some(e) = existing {
                for key in [
                    "state",
                    "items",
                    "item_order",
                    "active_turn_id",
                    "error",
                    "completed_turn_id",
                    "truncated",
                ] {
                    if let Some(value) = e.get(key) {
                        v[key] = value.clone();
                    }
                }
            }
            self.threads.insert(id, v);
        }
    }
}

pub struct Client {
    _child: Mutex<Child>,
    input: Mutex<ChildStdin>,
    pending: Mutex<BTreeMap<u64, oneshot::Sender<Result<Value, String>>>>,
    pub projection: Mutex<Projection>,
    next: AtomicU64,
    pub alive: AtomicBool,
}
impl Client {
    pub async fn spawn(program: PathBuf, host: ToolHost) -> Result<Arc<Self>, String> {
        let mut command = Command::new(program);
        if let Some(root) = std::env::var_os("HOMEBASE_TEST_HOME") {
            // A native smoke must never inherit the real Agent's auth, history, MCP or Skills.
            let home = PathBuf::from(root).join(".codex");
            std::fs::create_dir_all(&home).map_err(|_| "隔离 Agent 目录不可创建")?;
            command.env_clear();
            for key in ["SystemRoot", "WINDIR", "COMSPEC", "PATH", "PATHEXT"] {
                if let Some(v) = std::env::var_os(key) {
                    command.env(key, v);
                }
            }
            command
                .env("CODEX_HOME", &home)
                .env("USERPROFILE", &home)
                .env("HOME", &home)
                .env("APPDATA", home.join("AppData/Roaming"))
                .env("LOCALAPPDATA", home.join("AppData/Local"))
                .env("TEMP", &home)
                .env("TMP", &home);
        }
        command
            .args(["app-server", "-c", "mcp_servers={}"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|_| "Codex App Server 无法启动；未运行 shell 或其他替代程序")?;
        let output = child.stdout.take().ok_or("原生 Agent 输出管道缺失")?;
        let input = child.stdin.take().ok_or("原生 Agent 输入管道缺失")?;
        let client = Arc::new(Self {
            _child: Mutex::new(child),
            input: Mutex::new(input),
            pending: Mutex::new(BTreeMap::new()),
            projection: Mutex::new(Projection::default()),
            next: AtomicU64::new(1),
            alive: AtomicBool::new(true),
        });
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            let mut reader = BufReader::new(output).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if line.len() > 8 * 1024 * 1024 {
                    break;
                }
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(client) = weak.upgrade() else { break };
                if let Some(method) = message["method"].as_str() {
                    if message.get("id").is_some() {
                        let host = host.clone();
                        let id = message["id"].clone();
                        let params = message["params"].clone();
                        let method = method.to_string();
                        tokio::spawn(async move {
                            let response = if method == "item/tool/call" {
                                match host(params).await {
                                    Ok(v) => {
                                        json!({"success":true,"contentItems":[{"type":"inputText","text":v.to_string()}]})
                                    }
                                    Err(e) => {
                                        json!({"success":false,"contentItems":[{"type":"inputText","text":e}]})
                                    }
                                }
                            } else {
                                json!({"decision":"decline","answers":{},"permissions":{},"scope":"turn"})
                            };
                            let _ = client.write(json!({"id":id,"result":response})).await;
                        });
                    } else {
                        client
                            .projection
                            .lock()
                            .await
                            .event(method, &message["params"]);
                    }
                } else if let Some(id) = message["id"].as_u64() {
                    if let Some(waiter) = client.pending.lock().await.remove(&id) {
                        // Do not relay server diagnostics containing account/config details.
                        let result = if message.get("error").is_some() {
                            Err(format!(
                                "原生 Agent 拒绝请求（{}）；请检查会话所属厂商、登录状态或协议版本",
                                message["error"]["code"]
                            ))
                        } else {
                            Ok(message["result"].clone())
                        };
                        let _ = waiter.send(result);
                    }
                }
            }
            if let Some(client) = weak.upgrade() {
                client.alive.store(false, Ordering::SeqCst);
                for (_, waiter) in std::mem::take(&mut *client.pending.lock().await) {
                    let _ = waiter.send(Err(
                        "原生连接已断开，受理结果未知；请查询原会话，不要重发".into(),
                    ));
                }
                for v in client.projection.lock().await.threads.values_mut() {
                    if v["state"] == "inProgress" {
                        v["state"] = json!("unknown");
                        v["error"] = json!({"message":"连接中断，原任务结果未知；未自动重跑"});
                    }
                }
            }
        });
        client.call("initialize",json!({"clientInfo":{"name":"homebase","title":"HomeBase","version":"0.1.0"},"capabilities":{"experimentalApi":true}})).await?;
        client
            .write(json!({"method":"initialized","params":{}}))
            .await?;
        Ok(client)
    }
    async fn write(&self, value: Value) -> Result<(), String> {
        let mut input = self.input.lock().await;
        input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .map_err(|_| "原生 Agent 连接中断，受理结果未知".to_string())
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err("原生 Agent 连接已断开；未重新提交任务".into());
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        if let Err(e) = self
            .write(json!({"id":id,"method":method,"params":params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(Duration::from_secs(45), rx).await {
            Ok(Ok(v)) => v,
            _ => {
                self.pending.lock().await.remove(&id);
                Err("原生 Agent 响应超时，受理结果未知；请查询原会话，不要重发".into())
            }
        }
    }
    pub fn stop(&self) {
        if let Ok(mut child) = self._child.try_lock() {
            let _ = child.start_kill();
        }
    }
}

/// Find a native binary without evaluating shell shims or reading auth files.
pub fn codex_binary() -> Option<PathBuf> {
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for path in [directory.join("node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe"),directory.join("codex.exe")]{if path.is_file(){return Some(path);}}
        #[cfg(not(windows))]
        if directory.join("codex").is_file() {
            return Some(directory.join("codex"));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hydration_does_not_replay_or_reset_active_turn() {
        let mut p = Projection::default();
        p.event("turn/started", &json!({"threadId":"a","turn":{"id":"t"}}));
        p.event(
            "item/completed",
            &json!({"threadId":"a","item":{"id":"m","type":"agentMessage","text":"结果"}}),
        );
        p.hydrate(json!({"id":"a","turns":[]}));
        p.hydrate(json!({"id":"a","turns":[]}));
        assert_eq!(p.threads["a"]["state"], "inProgress");
        assert_eq!(p.threads["a"]["items"].as_object().unwrap().len(), 1);
        p.event(
            "turn/completed",
            &json!({"threadId":"b","turn":{"status":"failed"}}),
        );
        assert_eq!(p.threads["a"]["state"], "inProgress");
    }
    #[test]
    fn event_order_is_explicit_bounded_and_old_turns_cannot_reset_new_work() {
        let mut p = Projection::default();
        p.event(
            "turn/started",
            &json!({"threadId":"a","turn":{"id":"first"}}),
        );
        p.event(
            "turn/completed",
            &json!({"threadId":"a","turn":{"id":"first","status":"completed"}}),
        );
        p.event(
            "turn/started",
            &json!({"threadId":"a","turn":{"id":"first"}}),
        );
        assert_eq!(p.threads["a"]["state"], "completed");
        p.event(
            "turn/started",
            &json!({"threadId":"a","turn":{"id":"second"}}),
        );
        p.event(
            "turn/completed",
            &json!({"threadId":"a","turn":{"id":"first","status":"completed"}}),
        );
        assert_eq!(p.threads["a"]["state"], "inProgress");
        for id in ["z", "a"] {
            p.event(
                "item/completed",
                &json!({"threadId":"a","item":{"id":id,"type":"agentMessage","text":id}}),
            );
        }
        assert_eq!(p.threads["a"]["item_order"], json!(["z", "a"]));
        for id in 0..150 {
            p.event("item/completed",&json!({"threadId":"a","item":{"id":format!("m{id}"),"type":"agentMessage","text":"bounded"}}));
        }
        assert_eq!(p.threads["a"]["items"].as_object().unwrap().len(), 128);
        assert_eq!(p.threads["a"]["truncated"], true);
        p.hydrate(json!({"id":"a","turns":[{"id":"large-history","items":[]}]}));
        assert!(p.threads["a"].get("turns").is_none());
    }
}
