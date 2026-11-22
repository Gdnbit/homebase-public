//! Private native-to-host RPC. Context is supplied by the trusted adapter, never the model.
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, process::Stdio, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::ChildStdin,
    sync::{oneshot, Mutex},
};

type Reply = oneshot::Sender<Result<Value, String>>;
pub type LocalReply = oneshot::Receiver<Result<Value, String>>;
pub struct LocalHost {
    writer: Mutex<ChildStdin>,
    pending: Mutex<BTreeMap<String, Reply>>,
    progress: Mutex<BTreeMap<String, Value>>,
    healthy: std::sync::atomic::AtomicBool,
}
impl LocalHost {
    pub async fn start(runtime: &Path, profile: &Path) -> Result<Arc<Self>, String> {
        let script = runtime.join("local/host.mjs");
        if !script.is_file() {
            return Err("本机工具运行包缺失，未执行宿主命令".into());
        }
        tokio::fs::create_dir_all(profile)
            .await
            .map_err(|_| "本机工具隔离配置目录不可创建")?;
        let bundled = runtime.join("node.exe");
        let mut command = tokio::process::Command::new(if bundled.is_file() {
            bundled
        } else {
            "node".into()
        });
        command
            .arg(script)
            .current_dir(profile)
            .env_clear()
            .env("HOME", profile)
            .env("USERPROFILE", profile)
            .env("TEMP", profile)
            .env("TMP", profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for name in ["SystemRoot", "WINDIR", "PATH", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().map_err(|_| "本机工具宿主启动失败")?;
        let writer = child.stdin.take().ok_or("本机工具输入不可用")?;
        let stdout = child.stdout.take().ok_or("本机工具输出不可用")?;
        let host = Arc::new(Self {
            writer: Mutex::new(writer),
            pending: Mutex::new(BTreeMap::new()),
            progress: Mutex::new(BTreeMap::new()),
            healthy: true.into(),
        });
        let weak = Arc::downgrade(&host);
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut frame = Vec::new();
                let read = (&mut reader)
                    .take(4 * 1024 * 1024 + 1)
                    .read_until(b'\n', &mut frame)
                    .await;
                if !matches!(read, Ok(n) if n > 0 && n <= 4 * 1024 * 1024) {
                    break;
                }
                let Ok(message) = serde_json::from_slice::<Value>(&frame) else {
                    break;
                };
                let Some(host) = weak.upgrade() else {
                    break;
                };
                let Some(id) = message["id"].as_str() else {
                    break;
                };
                match message["kind"].as_str() {
                    Some("result" | "protocol_error") => {
                        if let Some(reply) = host.pending.lock().await.remove(id) {
                            let result = if message["kind"] == "result" {
                                Ok(message["result"].clone())
                            } else {
                                Err("本机工具拒绝不一致的请求，未重复执行".into())
                            };
                            let _ = reply.send(result);
                        }
                        host.progress.lock().await.remove(id);
                    }
                    Some("progress") => {
                        let mut progress = host.progress.lock().await;
                        if host.pending.lock().await.contains_key(id) {
                            let item = progress
                                .entry(id.into())
                                .or_insert_with(|| json!({"state":"running","output":""}));
                            let event = &message["event"];
                            if let Some(text) = event["text"].as_str() {
                                let previous = item["output"].as_str().unwrap_or("");
                                let combined = format!("{previous}{text}");
                                item["output"] = json!(combined
                                    .chars()
                                    .rev()
                                    .take(16000)
                                    .collect::<String>()
                                    .chars()
                                    .rev()
                                    .collect::<String>());
                            }
                            item["event"] = event.clone();
                        }
                    }
                    Some("accepted" | "cancel_ack" | "status") => (),
                    _ => break,
                }
            }
            let _ = child.kill().await;
            if let Some(host) = weak.upgrade() {
                host.healthy
                    .store(false, std::sync::atomic::Ordering::SeqCst);
                for (_, reply) in std::mem::take(&mut *host.pending.lock().await) {
                    let _ = reply.send(Err(
                        "本机工具宿主断开，执行结果未知；未自动重新执行，请检查原任务和产物".into(),
                    ));
                }
                host.progress.lock().await.clear();
            }
        });
        Ok(host)
    }
    pub fn healthy(&self) -> bool {
        self.healthy.load(std::sync::atomic::Ordering::SeqCst)
    }
    async fn send(&self, message: Value) -> Result<(), String> {
        let mut bytes =
            zeroize::Zeroizing::new(serde_json::to_vec(&message).map_err(|_| "本机请求无法编码")?);
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("本机工具参数超过上限".into());
        }
        bytes.push(b'\n');
        self.writer
            .lock()
            .await
            .write_all(&bytes)
            .await
            .map_err(|_| "本机工具宿主已断开；未重试".to_string())
    }
    pub async fn execute(
        &self,
        id: &str,
        operation: &str,
        context: Value,
        request: Value,
    ) -> Result<Value, String> {
        let reply = self.submit(id, operation, context, request).await?;
        self.result(id, reply).await
    }
    pub async fn busy(&self) -> bool {
        !self.pending.lock().await.is_empty()
    }
    pub async fn reset_connections(&self) -> Result<(), String> {
        let id = format!("config-{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        if let Err(error) = self.send(json!({"id":id,"kind":"reset_connections"})).await {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        let value = self.result(&id, rx).await?;
        if value["status"] != "succeeded" {
            return Err("MCP旧连接未确认关闭，请重启后使用新授权".into());
        }
        Ok(())
    }
    pub async fn disconnect_server(&self, server: &str) -> Result<(), String> {
        let id = format!("disconnect-{}", uuid::Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id.clone(), tx);
        if let Err(error) = self
            .send(json!({"id":id,"kind":"disconnect_server","server_id":server}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        let value = self.result(&id, rx).await?;
        if value["status"] != "succeeded" {
            return Err("MCP旧连接关闭未确认".into());
        }
        Ok(())
    }
    pub async fn submit(
        &self,
        id: &str,
        operation: &str,
        context: Value,
        request: Value,
    ) -> Result<LocalReply, String> {
        if !self.healthy() {
            return Err("本机工具宿主不可用，请检查原任务；未重试".into());
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(id) {
                return Err("同一操作正在执行，请查询原任务".into());
            }
            if pending.len() >= 8 {
                return Err("本机工具正在忙，请查询已有任务".into());
            }
            pending.insert(id.into(), tx);
        }
        if let Err(error) = self.send(json!({"kind":"execute","id":id,"operation":operation,"context":context,"request":request})).await {
            self.pending.lock().await.remove(id); return Err(error);
        }
        Ok(rx)
    }
    pub async fn result(&self, id: &str, rx: LocalReply) -> Result<Value, String> {
        let result = tokio::time::timeout(Duration::from_secs(360), rx).await;
        // The reader releases the slot only after an actual final frame or disconnect.
        // A local wait timeout must not make an unconfirmed execution look idle to policy changes.
        match result {
            Ok(Ok(value)) => value,
            _ => {
                let _ = self.cancel(id).await;
                Err("本机工具等待超时或断开；已请求停止，结果未知，未自动重试".into())
            }
        }
    }
    pub async fn cancel(&self, id: &str) -> Result<(), String> {
        self.send(json!({"kind":"cancel","id":id})).await
    }
    pub async fn progress(&self, id: &str) -> Value {
        if let Some(value) = self.progress.lock().await.get(id).cloned() {
            return value;
        }
        if self.pending.lock().await.contains_key(id) {
            return json!({"state":"running","output":"","message":"本机工具正在处理，暂未产生可展示输出"});
        }
        json!({"state":"not_running_here","message":"此宿主没有可确认的实时进程；请查询持久任务结果，不据此重执行"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires an explicitly selected isolated local-tool lab and runtime"]
    async fn native_local_host_files_and_cancel() {
        let lab = std::path::PathBuf::from(
            std::env::var_os("HOMEBASE_LOCAL_TEST_LAB").expect("isolated lab"),
        );
        assert!(lab.is_absolute());
        let root = lab.join(format!("rust-host-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let runtime =
            std::path::PathBuf::from(std::env::var_os("HOMEBASE_LOCAL_RUNTIME").expect("runtime"));
        let host = LocalHost::start(&runtime, &root.join("profile"))
            .await
            .unwrap();
        let context = json!({"workspaces":[{"id":"test","path":root,"access":"write"}],"default_workspace":"test","sandbox":{"kind":"wsl-bubblewrap","distro":"Ubuntu","executable":std::env::var("HOMEBASE_BWRAP_EXE").unwrap(),"sha256":std::env::var("HOMEBASE_BWRAP_SHA256").unwrap()}});
        let result = host
            .execute(
                "write-1",
                "files",
                context.clone(),
                json!({"action":"write","path":"proof.txt","content":"隔离宿主"}),
            )
            .await
            .unwrap();
        assert_eq!(result["status"], "succeeded");
        let denied = host
            .execute(
                "denied-1",
                "files",
                context.clone(),
                json!({"action":"read","path":"../outside"}),
            )
            .await
            .unwrap();
        assert_eq!(denied["code"], "PATH_INVALID");
        let child = host.clone();
        let running = tokio::spawn(async move {
            child.execute("cancel-1", "terminal", context, json!({"command":"echo READY; sleep 20; echo BAD > late.txt","timeout_ms":30000})).await
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if host.progress("cancel-1").await["output"]
                .as_str()
                .unwrap_or("")
                .contains("READY")
            {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        host.cancel("cancel-1").await.unwrap();
        assert_eq!(running.await.unwrap().unwrap()["status"], "canceled");
        assert!(!root.join("late.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(root.join("proof.txt"))
                .await
                .unwrap(),
            "隔离宿主"
        );
    }
}
