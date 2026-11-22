//! Read-only model installation manifest probe for the Quick Panel model center.

use super::distribution::{self, DistributionRuntime};
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_JOB_BYTES: u64 = 2 * 1024 * 1024;
const MODEL_DOWNLOAD_EVENT: &str = "model-download";
const DEFAULT_MANAGER: &str = r"C:\HomeBase\AIHub\tools\model-manager\model_manager.py";
const DEFAULT_CATALOG: &str = r"C:\HomeBase\AIHub\capabilities\model-download-catalog.json";
const DEFAULT_PYTHON: &str = r"C:\HomeBase\Tools\Python313\python.exe";

#[derive(Debug, Serialize)]
pub struct ModelInstallProbe {
    pub id: String,
    pub state: String,
    pub local_path: Option<String>,
    pub path_exists: Option<bool>,
    pub detail: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelManifestProbe {
    pub source_path: String,
    pub manifest_state: String,
    pub schema_version: Option<u64>,
    pub updated_at: Option<String>,
    pub models: Vec<ModelInstallProbe>,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn model_center_manifest(
    app: AppHandle,
    manifest_path: String,
) -> Result<ModelManifestProbe, String> {
    let runtime = distribution::bootstrap(&app)?;
    let requested = validate_manifest_path(&manifest_path)?;
    let path = if requested.is_file() || nearest_existing_ancestor(&requested).is_some() {
        requested
    } else {
        distribution::model_manifest_path(&runtime)
    };
    if !path.exists() {
        return Ok(empty_probe(
            path,
            "missing",
            Some("安装清单不存在。".to_string()),
        ));
    }

    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|error| format!("读取安装清单元数据失败: {error}"))?;
    if !metadata.is_file() {
        return Ok(empty_probe(
            path,
            "invalid",
            Some("安装清单路径不是文件。".to_string()),
        ));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Ok(empty_probe(
            path,
            "invalid",
            Some("安装清单超过 2 MB 安全上限。".to_string()),
        ));
    }

    let content = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("读取安装清单失败: {error}"))?;
    Ok(parse_manifest(&path, &content))
}

fn validate_manifest_path(value: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(value.trim());
    if !path.is_absolute() {
        return Err("安装清单必须使用绝对路径。".to_string());
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !file_name.eq_ignore_ascii_case("installed-models.json") {
        return Err("只允许读取 installed-models.json。".to_string());
    }
    Ok(path)
}

fn empty_probe(path: PathBuf, state: &str, error: Option<String>) -> ModelManifestProbe {
    ModelManifestProbe {
        source_path: path.to_string_lossy().to_string(),
        manifest_state: state.to_string(),
        schema_version: None,
        updated_at: None,
        models: Vec::new(),
        error,
    }
}

fn parse_manifest(path: &Path, content: &str) -> ModelManifestProbe {
    let root: Value = match serde_json::from_str(content) {
        Ok(value) => value,
        Err(error) => {
            return empty_probe(
                path.to_path_buf(),
                "invalid",
                Some(format!("安装清单 JSON 无效: {error}")),
            )
        }
    };
    let Some(models) = root.get("models").and_then(Value::as_object) else {
        return empty_probe(
            path.to_path_buf(),
            "invalid",
            Some("安装清单缺少 models 对象。".to_string()),
        );
    };

    let mut probes = models
        .iter()
        .map(|(id, value)| parse_model_record(id, value))
        .collect::<Vec<_>>();
    probes.sort_by(|left, right| left.id.cmp(&right.id));

    ModelManifestProbe {
        source_path: path.to_string_lossy().to_string(),
        manifest_state: "loaded".to_string(),
        schema_version: root.get("schema_version").and_then(Value::as_u64),
        updated_at: root
            .get("updated_at")
            .and_then(Value::as_str)
            .map(str::to_string),
        models: probes,
        error: None,
    }
}

fn parse_model_record(id: &str, value: &Value) -> ModelInstallProbe {
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let local_path = value
        .get("target_dir")
        .and_then(Value::as_str)
        .filter(|item| !item.trim().is_empty())
        .or_else(|| value.get("local_path").and_then(Value::as_str))
        .map(str::to_string);
    let path_exists = local_path.as_ref().map(|item| Path::new(item).exists());
    let smoke_test_passed = value
        .pointer("/verification/smoke_test/status")
        .and_then(Value::as_str)
        .map(|item| item.eq_ignore_ascii_case("passed"))
        .unwrap_or(false);

    let (state, detail) = if mode.eq_ignore_ascii_case("probe")
        || status.eq_ignore_ascii_case("probed")
    {
        (
            "probed",
            Some("仅验证了下载来源；尚未证明模型权重完整可运行。".to_string()),
        )
    } else if status.eq_ignore_ascii_case("corrupt") {
        ("corrupt", Some("清单将该安装标记为损坏。".to_string()))
    } else if status.eq_ignore_ascii_case("downloaded") || mode.eq_ignore_ascii_case("snapshot") {
        if path_exists == Some(true) && smoke_test_passed {
            (
                "installed",
                Some("权重目录存在，且最小推理验证已通过。".to_string()),
            )
        } else if path_exists == Some(true) {
            (
                "partial",
                Some("权重目录存在，但最小推理验证尚未通过。".to_string()),
            )
        } else {
            ("partial", Some("清单存在，但权重目录不可用。".to_string()))
        }
    } else {
        (
            "partial",
            Some(format!("无法识别安装状态: mode={mode}, status={status}")),
        )
    };

    ModelInstallProbe {
        id: id.to_string(),
        state: state.to_string(),
        local_path,
        path_exists,
        detail,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadStartPayload {
    pub model_id: String,
    pub manager_path: Option<String>,
    pub python_path: Option<String>,
    pub catalog_path: Option<String>,
    pub manifest_path: Option<String>,
    pub jobs_root: Option<String>,
    pub model_root: Option<String>,
    #[serde(default = "default_true")]
    pub allow_large: bool,
    #[serde(default)]
    pub allow_system_drive: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadStartResult {
    pub job_id: String,
    pub state_path: String,
    pub target_root: String,
    pub command_preview: String,
}

struct RunningModelDownload {
    child: Child,
    state_path: PathBuf,
    cancel_path: PathBuf,
}

type ModelDownloadMap = Arc<Mutex<HashMap<String, RunningModelDownload>>>;

fn model_download_map() -> &'static ModelDownloadMap {
    static MAP: OnceLock<ModelDownloadMap> = OnceLock::new();
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn now_iso() -> String {
    Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|candidate| candidate.exists())
        .map(Path::to_path_buf)
}

fn configured_data_path(value: Option<String>, env_name: &str, fallback: &Path) -> PathBuf {
    if let Some(explicit) = value.filter(|item| !item.trim().is_empty()) {
        let path = PathBuf::from(explicit);
        if path.is_absolute() && (path.exists() || nearest_existing_ancestor(&path).is_some()) {
            return path;
        }
    }
    std::env::var_os(env_name)
        .filter(|item| !item.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn apply_distribution_environment(
    command: &mut Command,
    runtime: &DistributionRuntime,
    catalog: &Path,
    manifest: &Path,
    jobs_root: &Path,
    model_root: &Path,
) {
    command
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("HOMEBASE_AIHUB_ROOT", &runtime.aihub_root)
        .env("HOMEBASE_MODEL_CATALOG", catalog)
        .env("HOMEBASE_MODEL_MANIFEST", manifest)
        .env("HOMEBASE_MODEL_JOBS", jobs_root)
        .env("HOMEBASE_MODEL_ROOT", model_root)
        .env("HOMEBASE_MODEL_CACHE", &runtime.cache_root)
        .env("HF_HOME", runtime.cache_root.join("huggingface"));
}

fn require_absolute_file(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} 必须使用绝对路径。"));
    }
    if !path.is_file() {
        return Err(format!("{label} 不存在或不是文件：{}", path.display()));
    }
    fs::canonicalize(path).map_err(|error| format!("解析 {label} 失败：{error}"))
}

fn require_absolute_dir(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} 必须使用绝对路径。"));
    }
    fs::create_dir_all(path).map_err(|error| format!("创建 {label} 失败：{error}"))?;
    fs::canonicalize(path).map_err(|error| format!("解析 {label} 失败：{error}"))
}

fn atomic_write_value(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建模型任务目录失败：{error}"))?;
    }
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|item| item.to_str())
            .unwrap_or("job"),
        uuid::Uuid::new_v4()
    ));
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| format!("序列化模型任务失败：{error}"))?;
    fs::write(&temporary, bytes).map_err(|error| format!("写入模型任务临时状态失败：{error}"))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("替换模型任务状态失败：{error}"));
    }
    Ok(())
}

fn read_job_value(path: &Path) -> Result<Value, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("读取模型任务元数据失败：{error}"))?;
    if metadata.len() > MAX_JOB_BYTES {
        return Err("模型任务状态超过 2 MB 安全上限。".to_string());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("读取模型任务状态失败：{error}"))?;
    let value: Value = serde_json::from_str(&content).map_err(|_| "QUERY_INVALID: 模型任务状态 JSON 无效")?;
    if !value.is_object() || value.get("status").and_then(Value::as_str).filter(|s| !s.is_empty()).is_none() {
        return Err("QUERY_INVALID: 模型任务状态缺少有效 status".into());
    }
    Ok(value)
}

fn set_job_terminal(path: &Path, status: &str, error: Option<String>) -> Result<Value, String> {
    let mut value = if path.exists() {
        read_job_value(path)?
    } else {
        serde_json::json!({"schema_version":1,"revision":0})
    };
    let object = value
        .as_object_mut()
        .ok_or_else(|| "模型任务状态根节点不是对象。".to_string())?;
    object.insert("status".into(), Value::String(status.to_string()));
    object.insert("phase".into(), Value::String(status.to_string()));
    object.insert("updated_at".into(), Value::String(now_iso()));
    object.insert("completed_at".into(), Value::String(now_iso()));
    let revision = object.get("revision").and_then(Value::as_u64).unwrap_or(0) + 1;
    object.insert("revision".into(), Value::Number(revision.into()));
    if let Some(error) = error {
        object.insert("error".into(), Value::String(error));
    }
    atomic_write_value(path, &value)?;
    Ok(value)
}

#[cfg(test)]
fn active_model_download_status(status: &str) -> bool {
    matches!(status, "queued" | "running")
}

#[tauri::command]
pub async fn model_download_start(
    app: AppHandle,
    payload: ModelDownloadStartPayload,
) -> Result<ModelDownloadStartResult, String> {
    if payload.model_id.trim().is_empty() {
        return Err("模型 ID 不能为空。".to_string());
    }
    let runtime = distribution::bootstrap(&app)?;
    let manager = require_absolute_file(
        &distribution::resolve_runtime_file(
            &runtime,
            payload.manager_path.as_deref(),
            "HOMEBASE_MODEL_MANAGER",
            &distribution::model_manager_path(&runtime),
            Path::new(DEFAULT_MANAGER),
        ),
        "模型管理器",
    )?;
    let python = require_absolute_file(
        &distribution::resolve_runtime_file(
            &runtime,
            payload.python_path.as_deref(),
            "HOMEBASE_MODEL_PYTHON",
            &runtime.python_path,
            Path::new(DEFAULT_PYTHON),
        ),
        "Python 运行时",
    )?;
    let catalog = require_absolute_file(
        &distribution::resolve_runtime_file(
            &runtime,
            payload.catalog_path.as_deref(),
            "HOMEBASE_MODEL_CATALOG",
            &distribution::model_catalog_path(&runtime),
            Path::new(DEFAULT_CATALOG),
        ),
        "模型目录",
    )?;
    let manifest = configured_data_path(
        payload.manifest_path,
        "HOMEBASE_MODEL_MANIFEST",
        &distribution::model_manifest_path(&runtime),
    );
    if !manifest.is_absolute() {
        return Err("模型安装清单必须使用绝对路径。".to_string());
    }
    let jobs_root = require_absolute_dir(
        &configured_data_path(
            payload.jobs_root,
            "HOMEBASE_MODEL_JOBS",
            &distribution::model_jobs_path(&runtime),
        ),
        "模型任务目录",
    )?;
    let model_root = require_absolute_dir(
        &configured_data_path(
            payload.model_root,
            "HOMEBASE_MODEL_ROOT",
            &runtime.model_root,
        ),
        "模型权重根目录",
    )?;
    let job_id = uuid::Uuid::new_v4().to_string();
    let state_path = jobs_root.join(format!("{job_id}.json"));
    let cancel_path = jobs_root.join(format!("{job_id}.cancel"));

    let mut command = Command::new(&python);
    apply_distribution_environment(
        &mut command,
        &runtime,
        &catalog,
        &manifest,
        &jobs_root,
        &model_root,
    );
    command
        .arg(&manager)
        .arg("download")
        .arg(&payload.model_id)
        .arg("--catalog")
        .arg(&catalog)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--jobs-root")
        .arg(&jobs_root)
        .arg("--model-root")
        .arg(&model_root)
        .arg("--job-id")
        .arg(&job_id)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(false);
    if payload.allow_large {
        command.arg("--allow-large");
    }
    if payload.allow_system_drive {
        command.arg("--allow-system-drive");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动模型下载失败：{error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法接管模型下载 stdout。".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法接管模型下载 stderr。".to_string())?;
    model_download_map().lock().await.insert(
        job_id.clone(),
        RunningModelDownload {
            child,
            state_path: state_path.clone(),
            cancel_path: cancel_path.clone(),
        },
    );
    let _ = app.emit(
        MODEL_DOWNLOAD_EVENT,
        serde_json::json!({"kind":"started","jobId":job_id,"modelId":payload.model_id}),
    );

    {
        let app = app.clone();
        let job_id = job_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let payload = serde_json::from_str::<Value>(&line)
                    .unwrap_or_else(|_| serde_json::json!({"event":"stream","message":line.chars().take(800).collect::<String>()}));
                let _ = app.emit(
                    MODEL_DOWNLOAD_EVENT,
                    serde_json::json!({"kind":"progress","jobId":job_id,"payload":payload}),
                );
            }
        });
    }
    {
        let app = app.clone();
        let job_id = job_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = app.emit(
                    MODEL_DOWNLOAD_EVENT,
                    serde_json::json!({"kind":"diagnostic","jobId":job_id,"message":line.chars().take(800).collect::<String>()}),
                );
            }
        });
    }
    {
        let app = app.clone();
        let job_id = job_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(400)).await;
                let mut tasks = model_download_map().lock().await;
                let Some(task) = tasks.get_mut(&job_id) else {
                    return;
                };
                match task.child.try_wait() {
                    Ok(Some(status)) => {
                        let state = read_job_value(&task.state_path).ok();
                        let reported = state
                            .as_ref()
                            .and_then(|value| value.get("status"))
                            .and_then(Value::as_str);
                        let (job_status, issue) = crate::query::finished_status(reported, status.success());
                        let _ = app.emit(
                            MODEL_DOWNLOAD_EVENT,
                            serde_json::json!({"kind":"finished","jobId":job_id,"status":job_status,"exitCode":status.code(),"error":issue}),
                        );
                        tasks.remove(&job_id);
                        return;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ =
                            set_job_terminal(&task.state_path, "failed", Some(error.to_string()));
                        let _ = app.emit(
                            MODEL_DOWNLOAD_EVENT,
                            serde_json::json!({"kind":"finished","jobId":job_id,"status":"failed","message":error.to_string()}),
                        );
                        tasks.remove(&job_id);
                        return;
                    }
                }
            }
        });
    }

    Ok(ModelDownloadStartResult {
        job_id,
        state_path: state_path.to_string_lossy().to_string(),
        target_root: model_root.to_string_lossy().to_string(),
        command_preview: format!(
            "{} model_manager.py download {} --model-root <模型目录> --allow-large",
            python.display(),
            payload.model_id
        ),
    })
}

#[tauri::command]
pub async fn model_download_get(jobs_root: String, job_id: String) -> Result<Value, String> {
    if !crate::task_recovery::valid_job_id(&job_id) { return Err("QUERY_INVALID: 模型任务编号无效".into()); }
    let root = PathBuf::from(jobs_root);
    if !root.is_absolute() {
        return Err("模型任务目录必须使用绝对路径。".to_string());
    }
    read_job_value(&root.join(format!("{job_id}.json")))
}

#[tauri::command]
pub async fn model_download_list(
    jobs_root: String,
    limit: Option<usize>,
) -> Result<Vec<Value>, String> {
    let root = PathBuf::from(jobs_root);
    if !root.is_absolute() { return Err("QUERY_INVALID: 模型任务目录必须使用绝对路径".into()); }
    let mut paths = crate::query::entries(&root, true)?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|item| item.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    Ok(paths
        .into_iter()
        .take(limit.unwrap_or(100).clamp(1, 500))
        .map(|path| read_job_value(&path))
        .collect::<Result<Vec<_>, _>>()?)
}

/// 只读恢复下载监控：保留原始状态，不把未托管任务改写为中断。
#[tauri::command]
pub async fn model_download_reconcile(jobs_root: String) -> Result<Vec<Value>, String> {
    let root = PathBuf::from(jobs_root);
    if !root.is_absolute() { return Err("QUERY_INVALID: 模型任务目录必须使用绝对路径".into()); }
    let active: HashMap<String, PathBuf> = model_download_map().lock().await.iter()
        .map(|(id, task)| (id.clone(), task.state_path.clone())).collect();
    let mut result = Vec::new();
    for entry in crate::query::entries(&root, true)? {
        let path = entry.path();
        if path.extension().and_then(|item| item.to_str()) != Some("json") {
            continue;
        }
        let value = read_job_value(&path)?;
        let job_id = value
            .get("job_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !crate::task_recovery::valid_job_id(job_id)
            || path.file_stem().and_then(|name| name.to_str()) != Some(job_id) {
            return Err("QUERY_INVALID: 模型任务编号与状态文件不一致".into());
        }
        let owned = active.get(job_id).is_some_and(|owned| crate::task_recovery::same_file(owned, &path));
        result.push(crate::task_recovery::observe(value, owned)?);
    }
    Ok(result)
}

#[tauri::command]
pub async fn model_download_cancel(app: AppHandle, job_id: String) -> Result<(), String> {
    let mut tasks = model_download_map().lock().await;
    let Some(mut task) = tasks.remove(&job_id) else {
        return Err(format!("模型下载任务 {job_id} 不存在或已结束。"));
    };
    fs::write(&task.cancel_path, now_iso())
        .map_err(|error| format!("写入下载取消标记失败：{error}"))?;
    task.child
        .kill()
        .await
        .map_err(|error| format!("终止模型下载进程失败：{error}"))?;
    set_job_terminal(&task.state_path, "cancelled", None)?;
    let _ = app.emit(
        MODEL_DOWNLOAD_EVENT,
        serde_json::json!({"kind":"finished","jobId":job_id,"status":"cancelled"}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn task_recovery_model_rejoins_only_owned_file_in_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let id = "task-recovery-owned-model";
        let path = dir.path().join(format!("{id}.json"));
        let value = serde_json::json!({"job_id":id,"status":"running","revision":4});
        std::fs::write(&path, value.to_string()).unwrap();
        std::fs::write(other.path().join(format!("{id}.json")), value.to_string()).unwrap();
        let before = crate::task_recovery::snapshot(dir.path());
        let child = tokio::process::Command::new(std::env::current_exe().unwrap()).arg("--list").env_clear()
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        super::model_download_map().lock().await.insert(id.into(), super::RunningModelDownload { child, state_path:path, cancel_path:dir.path().join("never-created") });
        let owned = super::model_download_reconcile(dir.path().to_string_lossy().into()).await.unwrap();
        let foreign = super::model_download_reconcile(other.path().to_string_lossy().into()).await.unwrap();
        let mut task = super::model_download_map().lock().await.remove(id).unwrap();
        let _ = task.child.wait().await;
        assert_eq!(owned[0]["runtime_state"], "managed");
        assert_eq!(foreign[0]["runtime_state"], "unmanaged");
        assert_eq!(before, crate::task_recovery::snapshot(dir.path()));
    }
    #[tokio::test]
    async fn task_recovery_model_repeated_observation_never_writes_or_assumes_pid_liveness() {
        let dir = tempfile::tempdir().unwrap();
        for (index, status) in ["queued", "running", "completed", "failed", "cancelled", "interrupted", "verification_failed", "future-state"].iter().enumerate() {
            let id = format!("synthetic-{index}");
            let value = serde_json::json!({"job_id":id,"status":status,"phase":"synthetic","pid":std::process::id(),"revision":12,"updated_at":"original","completed_at":null,"extra":{"keep":true}});
            std::fs::write(dir.path().join(format!("{id}.json")), serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        }
        let before = crate::task_recovery::snapshot(dir.path());
        for _ in 0..8 {
            let rows = super::model_download_reconcile(dir.path().to_string_lossy().into()).await.unwrap();
            assert_eq!(rows.len(), 8);
            for row in rows {
                assert_eq!(row["revision"], 12);
                assert_eq!(row["updated_at"], "original");
                if matches!(row["status"].as_str().unwrap(), "queued" | "running") { assert_eq!(row["runtime_state"], "unmanaged"); }
                else if row["status"] == "future-state" { assert_eq!(row["runtime_state"], "unknown"); }
                else { assert_eq!(row["runtime_state"], "terminal"); }
            }
        }
        assert_eq!(before, crate::task_recovery::snapshot(dir.path()));
    }

    #[tokio::test]
    async fn task_recovery_model_errors_do_not_rewrite_earlier_records() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("good.json"), r#"{"job_id":"good","status":"running","revision":1}"#).unwrap();
        std::fs::write(dir.path().join("bad.json"), "{unfinished").unwrap();
        let before = crate::task_recovery::snapshot(dir.path());
        assert!(super::model_download_reconcile(dir.path().to_string_lossy().into()).await.is_err());
        assert!(super::model_download_get(dir.path().to_string_lossy().into(), "../outside".into()).await.is_err());
        assert_eq!(before, crate::task_recovery::snapshot(dir.path()));
        let missing = dir.path().join("missing");
        assert!(super::model_download_reconcile(missing.to_string_lossy().into()).await.unwrap().is_empty());
        assert!(!missing.exists());
    }
    use super::*;
    #[tokio::test]
    async fn query_safety_model_list_distinguishes_empty_and_invalid_records() {
        let dir = tempfile::tempdir().unwrap();
        assert!(model_download_list("relative".into(), None).await.is_err());
        assert!(model_download_list(dir.path().to_string_lossy().into(), None).await.unwrap().is_empty());
        let path = dir.path().join("broken.json");
        fs::write(&path, "null").unwrap();
        assert!(model_download_list(dir.path().to_string_lossy().into(), None).await.is_err());
        assert!(model_download_list(path.to_string_lossy().into(), None).await.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "null");
    }

    #[test]
    fn probe_record_is_not_reported_as_installed() {
        let parsed = parse_manifest(
            Path::new(r"C:\HomeBase\AIHub\models\installed-models.json"),
            r#"{
              "schema_version": 1,
              "models": {
                "sensevoice-small": {
                  "mode": "probe",
                  "status": "probed",
                  "local_path": "C:\\HomeBase\\Cache\\README.md"
                }
              }
            }"#,
        );
        assert_eq!(parsed.manifest_state, "loaded");
        assert_eq!(parsed.models[0].state, "probed");
    }

    #[test]
    fn malformed_manifest_is_reported_without_panicking() {
        let parsed = parse_manifest(Path::new(r"D:\installed-models.json"), "not-json");
        assert_eq!(parsed.manifest_state, "invalid");
        assert!(parsed.error.is_some());
    }

    #[test]
    fn downloaded_snapshot_needs_smoke_test_before_installed_state() {
        let parsed = parse_manifest(
            Path::new(r"C:\HomeBase\Models\installed-models.json"),
            r#"{
              "schema_version": 1,
              "models": {
                "sample": {
                  "mode": "snapshot",
                  "status": "downloaded_unverified",
                  "target_dir": "Z:\\missing-model",
                  "verification": {"smoke_test": {"status": "failed"}}
                }
              }
            }"#,
        );
        assert_ne!(parsed.models[0].state, "installed");
    }

    #[test]
    fn active_download_states_are_bounded() {
        assert!(active_model_download_status("queued"));
        assert!(active_model_download_status("running"));
        assert!(!active_model_download_status("verification_failed"));
        assert!(!active_model_download_status("completed"));
    }
}
