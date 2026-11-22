//! Command policy, idempotent operation tracking, and bounded control results.
use crate::control_protocol::{ControlError, Request};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;

#[path = "control_receipt.rs"]
mod receipt;

const MAX_LEDGER_BYTES: u64 = 32 * 1024 * 1024;
const MAX_MODEL_DIAGNOSTIC_BYTES: u64 = 2048;
// Allow for the terminal timestamp and JSON field syntax beyond the payload.
const TERMINAL_METADATA_HEADROOM: u64 = 128;

pub type Runner = Arc<
    dyn Fn(
            String,
            Map<String, Value>,
        ) -> Pin<Box<dyn Future<Output = Result<Value, ControlError>> + Send>>
        + Send
        + Sync,
>;
#[derive(Clone, Deserialize)]
struct Command {
    name: String,
    status: String,
    requires: Vec<String>,
    reason: Option<String>,
    parameters: Vec<Parameter>,
}
#[derive(Clone, Deserialize)]
struct Parameter {
    name: String,
    rust_name: String,
    optional: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub request_id: String,
    pub command: String,
    pub status: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub fingerprint: String,
    pub requires: Vec<String>,
    #[serde(skip)]
    pub result: Option<Value>,
    #[serde(skip)]
    pub error: Option<ControlError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    protected_result: Option<String>,
    // Persist only the closed, payload-free model diagnostic. Generic errors/results stay private in memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_failure: Option<crate::model_capability_io::reliability::Failure>,
    #[serde(skip)]
    terminal_reserve: u64,
}
impl Job {
    fn metadata(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap();
        let fields = value.as_object_mut().unwrap();
        fields.remove("fingerprint");
        fields.remove("protected_result");
        let protected = self.status == "succeeded" && self.protected_result.is_some();
        value["result_available"] = json!(self.result.is_some() || self.error.is_some() || protected);
        value["result_storage"] = json!(if protected {
            "protected"
        } else if self.result.is_some() || self.error.is_some() {
            "memory"
        } else {
            "unavailable"
        });
        value
    }
}
struct State {
    jobs: HashMap<String, Job>,
    requests: HashMap<String, String>,
    reserved_terminal_bytes: u64,
    last_compacted_length: Option<u64>,
}
pub struct Controller {
    catalog: Value,
    commands: HashMap<String, Command>,
    ledger: PathBuf,
    state: Mutex<State>,
    runner: Runner,
    healthy: AtomicBool,
}
impl Controller {
    pub fn healthy(&self) -> bool { self.healthy.load(Ordering::SeqCst) }
    /// Internal lookup of the same operation identity accepted by `submit`.
    /// The caller must first authorize the original assistant turn and source.
    pub(crate) async fn assistant_receipt(&self, request: &Request) -> Result<Value, ControlError> {
        if !self.healthy() {
            return Err(ControlError::new("STATE_IO", "控制账本暂不可核对"));
        }
        let command = request.command.as_deref().ok_or_else(|| ControlError::new("BAD_REQUEST", "缺少原命令"))?;
        let id = request.request_id.as_deref().ok_or_else(|| ControlError::new("BAD_REQUEST", "缺少原请求编号"))?;
        let definition = self.commands.get(command).ok_or_else(|| ControlError::new("UNKNOWN_COMMAND", "原命令不可核对"))?;
        require_grants(&definition.requires, &request.grants)?;
        let state = self.state.lock().await;
        let Some(job_id) = state.requests.get(id) else { return Ok(json!({"state":"not_found"})); };
        let job = state.jobs.get(job_id).ok_or_else(|| ControlError::new("STATE_CORRUPT", "原任务编号不在控制账本"))?;
        require_grants(&job.requires, &request.grants)?;
        if job.request_id != id || job.command != command || job.fingerprint != fingerprint(command, &request.args) {
            return Err(ControlError::new("REQUEST_CONFLICT", "原请求、命令或参数与账本不一致"));
        }
        Ok(json!({"state":"found","job":job.metadata()}))
    }
    pub fn new(
        catalog: Value,
        directory: &Path,
        runner: Runner,
    ) -> Result<Arc<Self>, ControlError> {
        let definitions: Vec<Command> = serde_json::from_value(catalog["commands"].clone())
            .map_err(|_| ControlError::new("CATALOG_INVALID", "控制命令目录无效"))?;
        let ledger = directory.join("operations.jsonl");
        let mut state = State {
            jobs: HashMap::new(),
            requests: HashMap::new(),
            reserved_terminal_bytes: 0,
            last_compacted_length: None,
        };
        match std::fs::metadata(&ledger) {
            Ok(metadata) if metadata.len() > MAX_LEDGER_BYTES => {
                return Err(ControlError::new(
                    "STATE_LIMIT",
                    "操作账本超过维护上限，请备份后人工归档，禁止自动重跑旧任务",
                ))
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(ControlError::new("STATE_IO", "无法读取操作账本"))
            }
            _ => (),
        }
        match std::fs::read_to_string(&ledger) {
            Ok(text) => {
                for line in text.lines() {
                    // A partial/truncated journal is deliberately not ignored.
                    let mut job: Job = serde_json::from_str(line).map_err(|_| {
                        ControlError::new("STATE_CORRUPT", "操作账本损坏，已拒绝接管，避免重复执行")
                    })?;
                    if job.status == "running" {
                        job.status = "interrupted_unknown".into();
                    }
                    if job.status=="failed" {job.error=job.model_failure.as_ref().filter(|f|crate::model_capability_io::reliability::Failure::decode(&f.encode()).is_some()).map(|f|f.control_error());}
                    state
                        .requests
                        .insert(job.request_id.clone(), job.id.clone());
                    state.jobs.insert(job.id.clone(), job);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(ControlError::new("STATE_IO", "无法读取操作账本")),
        }
        Ok(Arc::new(Self {
            catalog,
            commands: definitions
                .into_iter()
                .map(|command| (command.name.clone(), command))
                .collect(),
            ledger,
            state: Mutex::new(state),
            runner,
            healthy: AtomicBool::new(true),
        }))
    }

    pub async fn handle(self: Arc<Self>, request: Request) -> Result<Value, ControlError> {
        if request
            .grants
            .iter()
            .any(|grant| !["write", "private", "external"].contains(&grant.as_str()))
        {
            return Err(ControlError::new("BAD_GRANT", "未知操作范围"));
        }
        match request.method.as_str() {
            "hello" => Ok(
                json!({"protocol":1,"catalog_hash":self.catalog["catalog_hash"],"pid":std::process::id(),"service":"homebase-control","state_healthy":self.healthy.load(Ordering::SeqCst),"operations_persistent":true,"results_persistent":false,"protected_result_commands":receipt::commands()}),
            ),
            "catalog" => Ok(self.catalog.clone()),
            "rollout.status" => self.rollout_status(&request).await,
            "jobs.list" => {
                let state = self.state.lock().await;
                let mut jobs: Vec<_> = state.jobs.values().collect();
                jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                Ok(json!(jobs
                    .into_iter()
                    .take(100)
                    .map(Job::metadata)
                    .collect::<Vec<_>>()))
            }
            "jobs.get" => {
                let state = self.state.lock().await;
                let job = state
                    .jobs
                    .get(request.job_id.as_deref().unwrap_or(""))
                    .ok_or_else(|| {
                        ControlError::new(
                            "JOB_NOT_FOUND",
                            "未找到操作；进程中断后请核对业务任务，禁止盲目重试",
                        )
                    })?;
                require_grants(&job.requires, &request.grants)?;
                if job.protected_result.is_some() {
                    if let Some(command) = self.commands.get(&job.command) {
                        require_grants(&command.requires, &request.grants)?;
                    }
                }
                let mut value = job.metadata();
                if (job.result.is_none() || matches!(job.command.as_str(), crate::wechat::video_cover::COMMAND | crate::wechat::publish_package::COMMAND)
                    || (job.command == "capability_assemble_video" && job.result.as_ref().is_some_and(|result| result.get("subtitles").is_some())))
                    && job.status == "succeeded"
                {
                    if let Some(ciphertext) = &job.protected_result {
                        let opened = if receipt::verifies_artifact_files(&job.command) {
                            let (original, sealed) = (job.clone(), ciphertext.clone());
                            drop(state);
                            tokio::task::spawn_blocking(move || receipt::open(&original, &sealed))
                                .await
                                .map_err(|_| ControlError::new("RESULT_UNAVAILABLE", "无法核对原任务的产物。"))
                                .and_then(|result| result)
                        } else {
                            receipt::open(job, ciphertext)
                        };
                        match opened {
                            Ok(result) => {
                                value["result"] = result;
                                value["error"] = Value::Null;
                            }
                            Err(error) => {
                                value["result_available"] = json!(false);
                                value["result_storage"] = json!("unavailable");
                                value["result"] = Value::Null;
                                value["error"] = serde_json::to_value(error).unwrap();
                            }
                        }
                        return Ok(value);
                    }
                }
                value["result"] = job.result.clone().unwrap_or(Value::Null);
                value["error"] = serde_json::to_value(&job.error).unwrap();
                Ok(value)
            }
            "call" => self.submit(request).await,
            _ => Err(ControlError::new("UNKNOWN_METHOD", "未知控制协议方法")),
        }
    }

    async fn rollout_status(&self, request: &Request) -> Result<Value, ControlError> {
        if !request.grants.iter().any(|scope| scope == "private") {
            return Err(ControlError::new("SCOPE_REQUIRED", "更新预检需要已获任务授权的 private 范围"));
        }
        if request.command.is_some() || request.request_id.is_some() || request.job_id.is_some()
            || !request.args.as_object().is_some_and(Map::is_empty)
        {
            return Err(ControlError::new("BAD_ARGUMENT", "更新预检不接受业务操作参数"));
        }
        let control = {
            let state = self.state.try_lock()
                .map_err(|_| ControlError::new("ROLLOUT_ACTIVITY_BUSY", "控制账本正在更新，请稍后再核对"))?;
            let mut active = 0;
            let mut to_review = 0;
            for job in state.jobs.values() {
                match job.status.as_str() {
                    "queued" | "running" => active += 1,
                    "succeeded" | "failed" | "timed_out" | "canceled" => {}
                    _ => to_review += 1,
                }
            }
            json!({"healthy":self.healthy(),"jobs":state.jobs.len(),"active":active,"to_review":to_review})
        };
        Ok(json!({
            "schema_version":1, "pid":std::process::id(), "safe_to_update":false,
            "control":control, "wechat":null
        }))
    }

    async fn submit(self: &Arc<Self>, request: Request) -> Result<Value, ControlError> {
        if request.catalog_hash.as_deref() != self.catalog["catalog_hash"].as_str() {
            return Err(ControlError::new(
                "CATALOG_MISMATCH",
                "CLI 与运行中的应用版本不一致，请重启更新后的 HomeBase",
            ));
        }
        let name = request
            .command
            .as_deref()
            .ok_or_else(|| ControlError::new("BAD_REQUEST", "缺少命令名称"))?;
        let command = self
            .commands
            .get(name)
            .ok_or_else(|| ControlError::new("UNKNOWN_COMMAND", "命令不在已注册目录中"))?;
        if command.status != "available" {
            return Err(ControlError::new(
                if command.status == "blocked" {
                    "HUMAN_REQUIRED"
                } else {
                    "NOT_IMPLEMENTED"
                },
                command.reason.as_deref().unwrap_or("此命令不可调用"),
            ));
        }
        require_grants(&command.requires, &request.grants)?;
        let args = request
            .args
            .as_object()
            .ok_or_else(|| ControlError::new("BAD_ARGUMENT", "参数必须是 JSON 对象"))?;
        for parameter in &command.parameters {
            if parameter.name != parameter.rust_name
                && args.contains_key(&parameter.name)
                && args.contains_key(&parameter.rust_name)
            {
                return Err(ControlError::new(
                    "BAD_ARGUMENT",
                    "同一参数不能同时使用两种命名形式",
                ));
            }
            if !parameter.optional
                && !args.contains_key(&parameter.name)
                && !args.contains_key(&parameter.rust_name)
            {
                return Err(ControlError::new(
                    "BAD_ARGUMENT",
                    &format!("缺少参数 {}", parameter.name),
                ));
            }
        }
        for name in args.keys() {
            if !command
                .parameters
                .iter()
                .any(|parameter| parameter.name == *name || parameter.rust_name == *name)
            {
                return Err(ControlError::new(
                    "BAD_ARGUMENT",
                    "参数包含未知字段，请先 describe 命令",
                ));
            }
        }
        if name == "secrets_save"
            && args
                .get("request")
                .and_then(|r| r.get("value"))
                .is_some_and(|value| !value.is_null())
        {
            return Err(ControlError::new(
                "HUMAN_REQUIRED",
                "CLI 支持密钥元数据管理；密钥正文通过人工密钥管理入口写入",
            ));
        }
        let request_id = request
            .request_id
            .as_deref()
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 128
                    && value
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            })
            .ok_or_else(|| {
                ControlError::new(
                    "BAD_REQUEST",
                    "需要 1–128 位字母数字下划线或短横线的幂等请求编号",
                )
            })?;
        let fingerprint = fingerprint(name, &request.args);
        let mut state = self.state.lock().await;
        if let Some(id) = state.requests.get(request_id) {
            let job = &state.jobs[id];
            if job.fingerprint != fingerprint {
                return Err(ControlError::new(
                    "REQUEST_CONFLICT",
                    "相同请求编号已用于不同操作，未重复执行",
                ));
            }
            return Ok(json!({"job":job.metadata(),"reused":true}));
        }
        let running = state
            .jobs
            .values()
            .filter(|job| job.status == "running")
            .count();
        let priority =
            !command.requires.iter().any(|scope| scope == "write") || name.contains("_cancel");
        if running >= if priority { 8 } else { 4 } {
            return Err(ControlError::new(
                "BUSY",
                "控制操作并发已满，请查询已有任务；查询和取消使用预留容量",
            ));
        }
        let mut job = Job {
            id: uuid::Uuid::new_v4().to_string(),
            request_id: request_id.into(),
            command: name.into(),
            status: "running".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            fingerprint,
            requires: command.requires.clone(),
            result: None,
            error: None,
            protected_result: None,
            model_failure: None,
            terminal_reserve: 0,
        };
        let bytes = ledger_line(&job);
        job.terminal_reserve = bytes.len() as u64
            + TERMINAL_METADATA_HEADROOM
            + MAX_MODEL_DIAGNOSTIC_BYTES.max(receipt::encoded_limit(name));
        let required = bytes.len() as u64 + state.reserved_terminal_bytes + job.terminal_reserve;
        let mut length = self.ledger_length()?;
        if length.saturating_add(required) > MAX_LEDGER_BYTES
            && state.last_compacted_length != Some(length)
        {
            length = self.compact_ledger(&state, length).await?;
            state.last_compacted_length = Some(length);
        }
        // Both rows must fit before dispatch. Other in-flight operations keep their own space.
        if length.saturating_add(required) > MAX_LEDGER_BYTES {
            return Err(ledger_limit());
        }
        self.append(&bytes, length)?;
        let response = json!({"job":job.metadata(),"reused":false});
        state.reserved_terminal_bytes += job.terminal_reserve;
        state.requests.insert(request_id.into(), job.id.clone());
        state.jobs.insert(job.id.clone(), job.clone());
        drop(state);
        let controller = self.clone();
        let mut args = args.clone();
        // The operation ID is already durable. Legacy callers do not have to
        // generate a business identity inside a retryable model/tool step.
        if name == "inbox_submit" {
            if let Some(payload) = args.get_mut("payload").and_then(Value::as_object_mut) {
                if payload.get("submission_id").is_none_or(Value::is_null) {
                    payload.insert("submission_id".into(),Value::String(job.id.clone()));
                }
            }
        }
        tokio::spawn(async move {
            let runner = controller.runner.clone();
            let name = job.command.clone();
            // Panics are recorded; a lost client connection never cancels execution.
            let result = tokio::spawn(async move { runner(name, args).await })
                .await
                .unwrap_or_else(|_| {
                    Err(ControlError::new(
                        "COMMAND_PANIC",
                        "操作异常中断，请核对业务状态，禁止自动重跑",
                    ))
                });
            controller.finish(&job.id, result).await;
        });
        Ok(response)
    }

    fn ledger_length(&self) -> Result<u64, ControlError> {
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(ControlError::new(
                "STATE_IO",
                "操作账本异常，已停止接受新操作，请核对后恢复",
            ));
        }
        match std::fs::metadata(&self.ledger) {
            Ok(metadata) => Ok(metadata.len()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(_) => Err(ControlError::new("STATE_IO", "操作账本无法核对")),
        }
    }

    async fn compact_ledger(&self, state: &State, length: u64) -> Result<u64, ControlError> {
        use std::io::Read;
        let invalid = || ControlError::new("STATE_CORRUPT", "操作账本身份不一致，未进行整理");
        let io = || ControlError::new("STATE_IO", "无法整理操作账本，未执行新操作");
        let mut input = String::new();
        std::fs::File::open(&self.ledger)
            .map_err(|_| io())?
            .take(MAX_LEDGER_BYTES + 1)
            .read_to_string(&mut input)
            .map_err(|_| io())?;
        if input.len() as u64 != length || length > MAX_LEDGER_BYTES {
            return Err(io());
        }
        let mut latest = HashMap::with_capacity(state.jobs.len());
        for (index, line) in input.lines().enumerate() {
            let row: Job = serde_json::from_str(line).map_err(|_| invalid())?;
            let job = state.jobs.get(&row.id).ok_or_else(invalid)?;
            if row.request_id != job.request_id
                || row.command != job.command
                || row.fingerprint != job.fingerprint
                || row.requires != job.requires
                || state.requests.get(&row.request_id) != Some(&row.id)
            {
                return Err(invalid());
            }
            latest.insert(row.id, (index, line));
        }
        if latest.len() != state.jobs.len() {
            return Err(invalid());
        }
        let mut latest: Vec<_> = latest.into_values().collect();
        latest.sort_unstable_by_key(|row| row.0);
        let mut compacted = Vec::new();
        for (_, line) in latest {
            compacted.extend_from_slice(line.as_bytes());
            compacted.push(b'\n');
        }
        if compacted.len() >= input.len() {
            return Ok(length);
        }
        crate::control_protocol::publish_private(&self.ledger, &compacted)
            .await
            .map_err(|_| io())?;
        Ok(compacted.len() as u64)
    }

    fn append(&self, bytes: &[u8], length: u64) -> Result<(), ControlError> {
        if length.saturating_add(bytes.len() as u64) > MAX_LEDGER_BYTES {
            return Err(ledger_limit());
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.ledger)
            .map_err(|_| ControlError::new("STATE_IO", "操作账本无法写入，未执行新操作"))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| {
                self.healthy.store(false, Ordering::SeqCst);
                ControlError::new("STATE_IO", "操作账本未确认持久化，请核对后处理")
            })
    }
    async fn finish(&self, id: &str, result: Result<Value, ControlError>) {
        let mut state = self.state.lock().await;
        let Some(reserve) = state.jobs.get(id).map(|job| job.terminal_reserve) else {
            return;
        };
        state.reserved_terminal_bytes -= reserve;
        let remaining_reserves = state.reserved_terminal_bytes;
        let Some(job) = state.jobs.get_mut(id) else {
            return;
        };
        job.terminal_reserve = 0;
        job.finished_at = Some(chrono::Utc::now().to_rfc3339());
        match result {
            Ok(mut value) => {
                redact(&mut value);
                if serde_json::to_vec(&value).map_or(true, |bytes| bytes.len() > 2 * 1024 * 1024) {
                    job.status = "failed".into();
                    job.error = Some(ControlError::new(
                        "RESULT_LIMIT",
                        "结果超过 2MiB，请通过业务任务的文件结果入口读取",
                    ));
                } else {
                    job.status = "succeeded".into();
                    job.protected_result = receipt::seal(job, &value);
                    if (matches!(job.command.as_str(), crate::wechat::video_cover::COMMAND | crate::wechat::publish_package::COMMAND)
                        || (job.command == "capability_assemble_video" && value.get("subtitles").is_some()))
                        && job.protected_result.is_none() {
                        job.status = "uncertain".into();
                        job.error = Some(ControlError::new("RESULT_UNAVAILABLE", "产物回执无法保护，请核对原任务。"));
                    } else {
                        job.result = Some(value);
                    }
                }
            }
            Err(error) => {
                job.status = "failed".into();
                job.model_failure=error.details.as_ref().and_then(|v|serde_json::from_value::<crate::model_capability_io::reliability::Failure>(v.clone()).ok()).filter(|f|crate::model_capability_io::reliability::Failure::decode(&f.encode()).is_some());
                if job.model_failure.as_ref().is_some_and(|failure| {
                    serde_json::to_vec(failure).map_or(true, |bytes| bytes.len() as u64 > MAX_MODEL_DIAGNOSTIC_BYTES)
                }) {
                    job.model_failure = None;
                }
                job.error = Some(error);
            }
        }
        let bytes = ledger_line(job);
        let written = self.ledger_length().and_then(|length| {
            if bytes.len() as u64 > reserve
                || length.saturating_add(remaining_reserves).saturating_add(bytes.len() as u64) > MAX_LEDGER_BYTES
            {
                return Err(ledger_limit());
            }
            self.append(&bytes, length)
        });
        if written.is_err() {
            job.status = "uncertain".into();
            job.protected_result = None;
            job.error = Some(ControlError::new(
                "STATE_IO",
                "操作已返回但结果状态无法持久化；请核对，禁止自动重跑",
            ));
        }
        let mut completed: Vec<_> = state
            .jobs
            .values()
            .filter(|job| job.status != "running" && (job.result.is_some() || job.error.is_some()))
            .map(|job| {
                (
                    job.created_at.clone(),
                    job.id.clone(),
                    serde_json::to_vec(&job.result).map_or(0, |bytes| bytes.len()),
                )
            })
            .collect();
        completed.sort();
        let mut bytes: usize = completed.iter().map(|job| job.2).sum();
        let mut count = completed.len();
        for (_, id, size) in completed {
            if count <= 128 && bytes <= 32 * 1024 * 1024 {
                break;
            }
            if let Some(job) = state.jobs.get_mut(&id) {
                job.result = None;
                job.error = None;
            }
            bytes = bytes.saturating_sub(size);
            count -= 1;
        }
    }
}

fn ledger_line(job: &Job) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(job).expect("control ledger record is serializable");
    bytes.push(b'\n');
    bytes
}

fn ledger_limit() -> ControlError {
    ControlError::new("STATE_LIMIT", "操作账本空间不足，请核对已有操作并备份后维护")
}

fn require_grants(required: &[String], actual: &[String]) -> Result<(), ControlError> {
    if required.iter().any(|grant| !actual.contains(grant)) {
        return Err(ControlError::new(
            "SCOPE_REQUIRED",
            &format!("该命令需要已获任务授权的操作范围：{}", required.join(", ")),
        ));
    }
    Ok(())
}
fn fingerprint(command: &str, args: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(&(command, canonical(args))).unwrap()))
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}
pub fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
                if [
                    "apikey",
                    "token",
                    "accesstoken",
                    "refreshtoken",
                    "password",
                    "authorization",
                    "privatekey",
                    "secret",
                    "secretvalue",
                ]
                .contains(&normalized.as_str())
                {
                    *value = json!("[redacted]");
                } else if normalized == "env" {
                    if let Some(environment) = value.as_object_mut() {
                        for value in environment.values_mut() {
                            if !value
                                .as_str()
                                .is_some_and(|text| text.starts_with("${") && text.ends_with('}'))
                            {
                                *value = json!("[redacted]");
                            }
                        }
                    }
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(array) => array.iter_mut().for_each(redact),
        _ => (),
    }
}
