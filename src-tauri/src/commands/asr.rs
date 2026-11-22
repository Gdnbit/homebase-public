//! Canonical ASR task control plane for the Quick Panel.

use super::distribution::{self, DistributionRuntime};
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

const EVENT_NAME: &str = "asr-job";
const DEFAULT_RUNNER: &str = r"C:\HomeBase\AIHub\tools\local-asr\runner\asr_task_runner.py";
const DEFAULT_OUTPUT_ROOT: &str = r"C:\HomeBase\Wiki\raw\audio";
const DEFAULT_PYTHON: &str = r"C:\HomeBase\Tools\Python313\python.exe";
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ARTIFACT_FILES: usize = 128;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrEnvironmentPayload {
    pub source_path: Option<String>,
    pub output_root: Option<String>,
    pub preset_id: Option<String>,
    pub engine_ids: Option<Vec<String>>,
    pub runner_path: Option<String>,
    pub python_path: Option<String>,
    #[serde(default)]
    pub allow_system_drive: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrTaskStartPayload {
    pub source_path: String,
    pub output_root: Option<String>,
    pub task_name: Option<String>,
    pub preset_id: String,
    pub engine_ids: Vec<String>,
    pub language: Option<String>,
    pub fusion_mode: Option<String>,
    pub preprocess_profile: Option<String>,
    pub runner_path: Option<String>,
    pub python_path: Option<String>,
    #[serde(default)]
    pub allow_system_drive: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrTaskOrganizePayload {
    pub task_dir: String,
    pub output_root: Option<String>,
    pub fusion_mode: Option<String>,
    pub fusion_model: Option<String>,
    pub runner_path: Option<String>,
    pub python_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrEnvironmentReport {
    pub available: bool,
    pub checked_at: String,
    pub python_path: String,
    pub runner_path: String,
    pub output_root: String,
    pub report: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrTaskStartResult {
    pub job_id: String,
    pub task_dir: String,
    pub job_state_path: String,
    pub command_preview: String,
    pub preflight: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrJobRecord {
    pub schema_version: u32,
    pub job_id: String,
    pub status: String,
    pub source_path: String,
    pub output_root: String,
    pub task_dir: String,
    pub preset_id: String,
    pub engine_ids: Vec<String>,
    pub pid: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrTaskView {
    pub task_dir: String,
    pub task_meta: Value,
    pub job: Option<AsrJobRecord>,
    pub artifacts: Vec<AsrArtifactSummary>,
    pub preprocess_meta: Option<Value>,
    pub run_matrix: Option<Value>,
    pub fusion_meta: Option<Value>,
    pub summary_meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrArtifactSummary {
    pub relative_path: String,
    pub absolute_path: String,
    pub category: String,
    pub engine_id: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrArtifactContent {
    pub relative_path: String,
    pub absolute_path: String,
    pub content: String,
    pub size_bytes: u64,
}

struct RunningAsrTask {
    child: Child,
    record: AsrJobRecord,
    job_path: PathBuf,
}

type AsrTaskMap = Arc<Mutex<HashMap<String, RunningAsrTask>>>;

fn task_map() -> &'static AsrTaskMap {
    static MAP: OnceLock<AsrTaskMap> = OnceLock::new();
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

fn now_iso() -> String {
    Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

fn configured_path(value: Option<String>, env_name: &str, fallback: &str) -> PathBuf {
    value
        .filter(|item| !item.trim().is_empty())
        .or_else(|| std::env::var(env_name).ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(fallback))
}

fn sanitize_task_name(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut separator = false;
    for character in value.trim().chars() {
        let invalid = character.is_control()
            || matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )
            || character.is_whitespace();
        if invalid {
            if !result.is_empty() {
                separator = true;
            }
            continue;
        }
        if separator && !result.ends_with('-') {
            result.push('-');
        }
        separator = false;
        result.push(character);
        if result.chars().count() >= 80 {
            break;
        }
    }
    result.trim_matches([' ', '.', '-', '_']).to_string()
}

fn redact_message(value: impl AsRef<str>) -> String {
    let mut result = value
        .as_ref()
        .replace('\r', " ")
        .replace('\n', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    for marker in [
        "Bearer ",
        "Token ",
        "api_key=",
        "api-key=",
        "Authorization:",
    ] {
        let marker_lower = marker.to_ascii_lowercase();
        let mut cursor = 0;
        loop {
            let lower = result.to_ascii_lowercase();
            let Some(relative_index) = lower[cursor..].find(&marker_lower) else {
                break;
            };
            let index = cursor + relative_index;
            let start = index + marker.len();
            let end = result[start..]
                .find(char::is_whitespace)
                .map(|offset| start + offset)
                .unwrap_or(result.len());
            result.replace_range(start..end, "[redacted]");
            cursor = start + "[redacted]".len();
        }
    }
    result.chars().take(800).collect()
}

fn validate_absolute_file(path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label} 必须使用绝对路径。"));
    }
    if !path.is_file() {
        return Err(format!("{label} 不存在或不是文件：{}", path.display()));
    }
    fs::canonicalize(path).map_err(|error| format!("解析 {label} 失败：{error}"))
}

fn validate_output_root(path: &Path, allow_system_drive: bool) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("输出根目录必须使用绝对路径。".to_string());
    }
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    let normalized = path.to_string_lossy().replace('/', "\\");
    if !allow_system_drive
        && normalized
            .to_ascii_uppercase()
            .starts_with(&format!("{}\\", system_drive.to_ascii_uppercase()))
    {
        return Err(format!("默认禁止把 ASR 任务写入系统盘 {system_drive}。"));
    }
    fs::create_dir_all(path).map_err(|error| format!("创建输出目录失败：{error}"))?;
    let probe = path.join(format!(".homebase-asr-write-{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&probe, b"ok").map_err(|error| format!("输出目录不可写：{error}"))?;
    fs::remove_file(&probe).map_err(|error| format!("清理写入探针失败：{error}"))?;
    fs::canonicalize(path).map_err(|error| format!("解析输出目录失败：{error}"))
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建状态目录失败：{error}"))?;
    }
    let data =
        serde_json::to_vec_pretty(value).map_err(|error| format!("序列化状态失败：{error}"))?;
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|item| item.to_str())
            .unwrap_or("state"),
        uuid::Uuid::new_v4()
    ));
    fs::write(&temporary, data).map_err(|error| format!("写入临时状态失败：{error}"))?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("替换状态文件失败：{error}"));
    }
    Ok(())
}

fn read_json_file(path: &Path) -> Result<Value, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("读取状态元数据失败：{error}"))?;
    if metadata.len() > MAX_JSON_BYTES {
        return Err(format!(
            "状态文件超过 {} MB 上限。",
            MAX_JSON_BYTES / 1024 / 1024
        ));
    }
    let content = fs::read_to_string(path).map_err(|error| format!("读取状态失败：{error}"))?;
    serde_json::from_str(&content).map_err(|error| format!("状态 JSON 无效：{error}"))
}

fn read_optional_json(path: &Path) -> Result<Option<Value>, String> {
    match fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("QUERY_FAILED: ASR 可选产物元数据不可读取".into()),
        Ok(_) => read_json_file(path).map(Some),
    }
}

fn normalized_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn canonical_task_under_root(task_dir: &Path, output_root: &Path) -> Result<PathBuf, String> {
    if !task_dir.is_absolute() || !output_root.is_absolute() {
        return Err("任务目录和输出根目录必须使用绝对路径。".to_string());
    }
    let canonical_root = fs::canonicalize(output_root)
        .map_err(|error| format!("解析 ASR 输出根目录失败：{error}"))?;
    let canonical_task =
        fs::canonicalize(task_dir).map_err(|error| format!("解析 ASR 任务包失败：{error}"))?;
    if canonical_task == canonical_root || !canonical_task.starts_with(&canonical_root) {
        return Err("ASR 任务包必须位于当前配置的输出根目录内。".to_string());
    }
    Ok(canonical_task)
}

fn is_allowed_artifact(relative: &Path) -> bool {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return false;
    }
    let parts = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let file_name = parts.last().map(String::as_str).unwrap_or_default();
    match parts.first().map(String::as_str) {
        Some("runs") if parts.len() == 3 => matches!(
            file_name,
            "run.meta.json"
                | "transcript.md"
                | "transcript.json"
                | "raw.json"
                | "segments.json"
                | "speaker-turns.json"
                | "subtitles.srt"
        ),
        Some("compare") if parts.len() == 2 => matches!(
            file_name,
            "run-matrix.json" | "alignment-notes.md" | "fusion.meta.json"
        ),
        Some("final") if parts.len() == 2 => matches!(
            file_name,
            "transcript.md"
                | "multi-engine-fused-transcript.md"
                | "original-scene-reconstruction.md"
                | "meeting-summary.md"
                | "title-summary.json"
        ),
        Some("exports") if parts.len() == 2 => matches!(
            file_name,
            "transcript.txt" | "transcript.md" | "transcript.srt" | "transcript.vtt"
        ),
        Some("preprocess") if parts.len() == 2 => {
            matches!(file_name, "audio-profile.json" | "preprocessing.meta.json")
        }
        _ => false,
    }
}

fn artifact_category(relative: &Path) -> String {
    relative
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn collect_task_artifacts(task_dir: &Path) -> Result<Vec<AsrArtifactSummary>, String> {
    if !task_dir.is_absolute() || !task_dir.is_dir() {
        return Err("ASR 任务包不存在或不是绝对目录。".to_string());
    }
    let mut files = Vec::new();
    for top_level in ["preprocess", "runs", "compare", "final", "exports"] {
        let root = task_dir.join(top_level);
        if !root.is_dir() {
            continue;
        }
        let mut pending = vec![root];
        while let Some(directory) = pending.pop() {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let Ok(relative) = path.strip_prefix(task_dir) else {
                    continue;
                };
                if !is_allowed_artifact(relative) {
                    continue;
                }
                let Ok(metadata) = fs::metadata(&path) else {
                    continue;
                };
                let parts = relative
                    .components()
                    .filter_map(|component| match component {
                        std::path::Component::Normal(value) => {
                            Some(value.to_string_lossy().to_string())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                files.push(AsrArtifactSummary {
                    relative_path: normalized_relative_path(relative),
                    absolute_path: path.to_string_lossy().to_string(),
                    category: artifact_category(relative),
                    engine_id: if parts.first().map(String::as_str) == Some("runs") {
                        parts.get(1).cloned()
                    } else {
                        None
                    },
                    size_bytes: metadata.len(),
                });
                if files.len() >= MAX_ARTIFACT_FILES {
                    break;
                }
            }
            if files.len() >= MAX_ARTIFACT_FILES {
                break;
            }
        }
        if files.len() >= MAX_ARTIFACT_FILES {
            break;
        }
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn read_task_artifact(task_dir: &Path, relative_path: &str) -> Result<AsrArtifactContent, String> {
    if !task_dir.is_absolute() || !task_dir.is_dir() {
        return Err("ASR 任务包不存在或不是绝对目录。".to_string());
    }
    let relative = Path::new(relative_path);
    if !is_allowed_artifact(relative) {
        return Err("只能读取任务包内登记的 ASR 文本产物。".to_string());
    }
    let canonical_root =
        fs::canonicalize(task_dir).map_err(|error| format!("解析任务包失败：{error}"))?;
    let path = task_dir.join(relative);
    let canonical_path =
        fs::canonicalize(&path).map_err(|error| format!("读取结果文件失败：{error}"))?;
    if !canonical_path.starts_with(&canonical_root) {
        return Err("结果文件必须位于当前 ASR 任务包内。".to_string());
    }
    let metadata =
        fs::metadata(&canonical_path).map_err(|error| format!("读取结果元数据失败：{error}"))?;
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "结果文件超过 {} MB 读取上限。",
            MAX_ARTIFACT_BYTES / 1024 / 1024
        ));
    }
    let content = fs::read_to_string(&canonical_path)
        .map_err(|error| format!("结果文件不是有效 UTF-8 文本：{error}"))?;
    Ok(AsrArtifactContent {
        relative_path: normalized_relative_path(relative),
        absolute_path: canonical_path.to_string_lossy().to_string(),
        content,
        size_bytes: metadata.len(),
    })
}

fn build_task_view(
    task_dir: &Path,
    job: Option<AsrJobRecord>,
    include_details: bool,
) -> Result<AsrTaskView, String> {
    let task_meta = read_json_file(&task_dir.join("task.meta.json"))?;
    let artifacts = if include_details {
        collect_task_artifacts(task_dir)?
    } else {
        Vec::new()
    };
    Ok(AsrTaskView {
        task_dir: task_dir.to_string_lossy().to_string(),
        task_meta,
        job,
        artifacts,
        preprocess_meta: include_details
            .then(|| {
                read_optional_json(&task_dir.join("preprocess").join("preprocessing.meta.json"))
            })
            .transpose()?.flatten(),
        run_matrix: include_details
            .then(|| read_optional_json(&task_dir.join("compare").join("run-matrix.json")))
            .transpose()?.flatten(),
        fusion_meta: include_details
            .then(|| read_optional_json(&task_dir.join("compare").join("fusion.meta.json")))
            .transpose()?.flatten(),
        summary_meta: include_details
            .then(|| read_optional_json(&task_dir.join("final").join("title-summary.json")))
            .transpose()?.flatten(),
    })
}

fn read_job(path: &Path) -> Result<AsrJobRecord, String> {
    let value = read_json_file(path)?;
    serde_json::from_value(value).map_err(|error| format!("任务监督状态无效：{error}"))
}

fn task_status(task_dir: &Path) -> Option<String> {
    read_json_file(&task_dir.join("task.meta.json"))
        .ok()
        .and_then(|value| {
            value
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn update_task_meta_status(task_dir: &Path, status: &str) -> Result<(), String> {
    let path = task_dir.join("task.meta.json");
    if !path.exists() {
        return Ok(());
    }
    let mut value = read_json_file(&path)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "task.meta.json 根节点不是对象。".to_string())?;
    object.insert("status".to_string(), Value::String(status.to_string()));
    object.insert("updated_at".to_string(), Value::String(now_iso()));
    let revision = object.get("revision").and_then(Value::as_u64).unwrap_or(0) + 1;
    object.insert("revision".to_string(), Value::Number(revision.into()));
    if matches!(status, "cancelled" | "interrupted") {
        object.insert("completed_at".to_string(), Value::String(now_iso()));
    }
    atomic_write_json(&path, &value)
}

fn emit_event(app: &AppHandle, payload: Value) {
    let _ = app.emit(EVENT_NAME, payload);
}

fn apply_distribution_environment(
    command: &mut Command,
    runtime: &DistributionRuntime,
    output_root: &Path,
) {
    command
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("HOMEBASE_AIHUB_ROOT", &runtime.aihub_root)
        .env("HOMEBASE_ASR_PRESETS", distribution::asr_preset_path(runtime))
        .env("HOMEBASE_ASR_OUTPUT_ROOT", output_root)
        .env("HOMEBASE_MODEL_ROOT", &runtime.model_root)
        .env(
            "HOMEBASE_FASTER_WHISPER_MODEL",
            runtime
                .model_root
                .join("asr")
                .join("faster-whisper-large-v3"),
        )
        .env("HOMEBASE_FFMPEG", &runtime.ffmpeg_path)
        .env("HOMEBASE_REVEAL_EXE", &runtime.reveal_path)
        .env("HOMEBASE_SOPS_EXE", &runtime.sops_path)
        .env("HOMEBASE_SECRETS_FILE", &runtime.secrets_path)
        .env("HOMEBASE_SECRETS_META", &runtime.secrets_meta_path);
}

fn apply_offline_model_environment(command: &mut Command, offline: bool) {
    if offline {
        command.env("HF_HUB_OFFLINE", "1").env("TRANSFORMERS_OFFLINE", "1")
            .env_remove("HF_TOKEN").env_remove("HUGGING_FACE_HUB_TOKEN");
    }
}

async fn run_preflight(
    runtime: &DistributionRuntime,
    python: &Path,
    runner: &Path,
    source: Option<&Path>,
    output_root: &Path,
    preset_id: &str,
    engine_ids: &[String],
    allow_system_drive: bool,
    offline: bool,
) -> Result<Value, String> {
    let mut command = Command::new(python);
    apply_distribution_environment(&mut command, runtime, output_root);
    apply_offline_model_environment(&mut command, offline);
    command
        .arg(runner)
        .arg("preflight")
        .arg("--output-root")
        .arg(output_root)
        .arg("--preset-id")
        .arg(preset_id)
        .arg("--engines")
        .arg(engine_ids.join(","));
    if let Some(source) = source {
        command.arg("--source").arg(source);
    }
    if allow_system_drive {
        command.arg("--allow-system-drive");
    }
    let output = tokio::time::timeout(Duration::from_secs(45), command.output())
        .await
        .map_err(|_| "ASR 预检超时。".to_string())?
        .map_err(|error| format!("启动 ASR 预检失败：{error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last_line = stdout.lines().rev().find(|line| !line.trim().is_empty());
    let event: Value = last_line
        .ok_or_else(|| "ASR 预检未返回结构化结果。".to_string())
        .and_then(|line| {
            serde_json::from_str(line).map_err(|error| format!("ASR 预检结果无效：{error}"))
        })?;
    if event.get("event").and_then(Value::as_str) == Some("failed") {
        return Err(event
            .get("message")
            .and_then(Value::as_str)
            .map(redact_message)
            .unwrap_or_else(|| "ASR 预检失败。".to_string()));
    }
    Ok(event.get("report").cloned().unwrap_or(event))
}

#[tauri::command]
pub async fn asr_environment_probe(
    app: AppHandle,
    payload: AsrEnvironmentPayload,
) -> Result<AsrEnvironmentReport, String> {
    let runtime = distribution::bootstrap(&app)?;
    let runner = distribution::resolve_runtime_file(
        &runtime,
        payload.runner_path.as_deref(),
        "HOMEBASE_ASR_RUNNER",
        &distribution::asr_runner_path(&runtime),
        Path::new(DEFAULT_RUNNER),
    );
    let python = distribution::resolve_runtime_file(
        &runtime,
        payload.python_path.as_deref(),
        "HOMEBASE_ASR_PYTHON",
        &runtime.python_path,
        Path::new(DEFAULT_PYTHON),
    );
    let output_root_value = payload
        .output_root
        .or_else(|| std::env::var("HOMEBASE_ASR_OUTPUT_ROOT").ok())
        .unwrap_or_else(|| runtime.output_root.to_string_lossy().into_owned());
    let output_root = PathBuf::from(&output_root_value);
    let preset_id = payload
        .preset_id
        .unwrap_or_else(|| "asr_zh_balanced".to_string());
    let engine_ids = payload.engine_ids.unwrap_or_else(|| {
        vec![
            "faster_whisper_large_v3_cpu".to_string(),
            "sensevoice_small".to_string(),
        ]
    });
    let checked_at = now_iso();

    let basic_error = validate_absolute_file(&runner, "ASR Runner")
        .and_then(|_| validate_absolute_file(&python, "Python 运行时"))
        .and_then(|_| validate_output_root(&output_root, payload.allow_system_drive));
    if let Err(error) = basic_error {
        return Ok(AsrEnvironmentReport {
            available: false,
            checked_at,
            python_path: python.to_string_lossy().to_string(),
            runner_path: runner.to_string_lossy().to_string(),
            output_root: output_root_value,
            report: None,
            error: Some(error),
        });
    }

    let source = payload.source_path.as_deref().map(Path::new);
    match run_preflight(
        &runtime,
        &python,
        &runner,
        source,
        &output_root,
        &preset_id,
        &engine_ids,
        payload.allow_system_drive,
        false,
    )
    .await
    {
        Ok(report) => Ok(AsrEnvironmentReport {
            available: report
                .get("ready_engine_count")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 0
                && report
                    .get("blockers")
                    .and_then(Value::as_array)
                    .map_or(true, Vec::is_empty),
            checked_at,
            python_path: python.to_string_lossy().to_string(),
            runner_path: runner.to_string_lossy().to_string(),
            output_root: output_root.to_string_lossy().to_string(),
            report: Some(report),
            error: None,
        }),
        Err(error) => Ok(AsrEnvironmentReport {
            available: false,
            checked_at,
            python_path: python.to_string_lossy().to_string(),
            runner_path: runner.to_string_lossy().to_string(),
            output_root: output_root.to_string_lossy().to_string(),
            report: None,
            error: Some(error),
        }),
    }
}

#[tauri::command]
pub async fn asr_task_start(
    app: AppHandle,
    payload: AsrTaskStartPayload,
) -> Result<AsrTaskStartResult, String> {
    start_prepared(app, payload, async { Ok(true) }, false, false)
        .await?
        .ok_or_else(|| "ASR提交未获准，未启动任务".into())
}

pub(crate) async fn start_prepared<F>(
    app: AppHandle,
    payload: AsrTaskStartPayload,
    prepare: F,
    detached_stdio: bool,
    offline: bool,
) -> Result<Option<AsrTaskStartResult>, String>
where
    F: std::future::Future<Output = Result<bool, String>> + Send,
{
    if payload.engine_ids.is_empty() {
        return Err("至少选择一个 ASR 引擎。".to_string());
    }
    let fusion_mode = payload.fusion_mode.as_deref().unwrap_or("manual");
    if !matches!(fusion_mode, "manual" | "llm" | "auto") {
        return Err("融合模式只允许 manual、llm 或 auto。".to_string());
    }
    let runtime = distribution::bootstrap(&app)?;
    let source = validate_absolute_file(Path::new(&payload.source_path), "录音文件")?;
    let runner = validate_absolute_file(
        &distribution::resolve_runtime_file(
            &runtime,
            payload.runner_path.as_deref(),
            "HOMEBASE_ASR_RUNNER",
            &distribution::asr_runner_path(&runtime),
            Path::new(DEFAULT_RUNNER),
        ),
        "ASR Runner",
    )?;
    let python = validate_absolute_file(
        &distribution::resolve_runtime_file(
            &runtime,
            payload.python_path.as_deref(),
            "HOMEBASE_ASR_PYTHON",
            &runtime.python_path,
            Path::new(DEFAULT_PYTHON),
        ),
        "Python 运行时",
    )?;
    let output_root_value = payload
        .output_root
        .clone()
        .or_else(|| std::env::var("HOMEBASE_ASR_OUTPUT_ROOT").ok())
        .unwrap_or_else(|| runtime.output_root.to_string_lossy().into_owned());
    let output_root =
        validate_output_root(Path::new(&output_root_value), payload.allow_system_drive)?;
    let preflight = run_preflight(
        &runtime,
        &python,
        &runner,
        Some(&source),
        &output_root,
        &payload.preset_id,
        &payload.engine_ids,
        payload.allow_system_drive,
        offline,
    )
    .await?;
    let ready_count = preflight
        .get("ready_engine_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let has_blocker = preflight
        .get("blockers")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty());
    if ready_count == 0 || has_blocker {
        return Err("当前组合没有可执行引擎，请按预检修复建议处理。".to_string());
    }

    // One preflight, then the caller's durable intent, before journal/spawn.
    if !prepare.await? {
        return Ok(None);
    }
    let job_id = uuid::Uuid::new_v4().to_string();
    let fallback_name = source
        .file_stem()
        .and_then(|item| item.to_str())
        .unwrap_or("audio");
    let task_name = sanitize_task_name(payload.task_name.as_deref().unwrap_or(fallback_name));
    let safe_name = if task_name.is_empty() {
        "audio"
    } else {
        task_name.as_str()
    };
    let task_dir = output_root.join(format!(
        "{}_{}_{}",
        Local::now().format("%Y-%m-%d"),
        safe_name,
        &job_id[..6]
    ));
    if task_dir.exists() {
        return Err(format!("任务目录已存在：{}", task_dir.display()));
    }
    let job_path = output_root.join("_jobs").join(format!("{job_id}.json"));
    let created_at = now_iso();
    let mut record = AsrJobRecord {
        schema_version: 1,
        job_id: job_id.clone(),
        status: "queued".to_string(),
        source_path: source.to_string_lossy().to_string(),
        output_root: output_root.to_string_lossy().to_string(),
        task_dir: task_dir.to_string_lossy().to_string(),
        preset_id: payload.preset_id.clone(),
        engine_ids: payload.engine_ids.clone(),
        pid: None,
        created_at: created_at.clone(),
        updated_at: created_at,
        completed_at: None,
        exit_code: None,
        error: None,
    };
    atomic_write_json(&job_path, &record)?;

    let preprocess_profile = payload.preprocess_profile.as_deref().unwrap_or("adaptive");
    let language = payload.language.as_deref().unwrap_or("zh");
    let mut command = Command::new(&python);
    apply_distribution_environment(&mut command, &runtime, &output_root);
    apply_offline_model_environment(&mut command, offline);
    command
        .arg(&runner)
        .arg("run")
        .arg("--source")
        .arg(&source)
        .arg("--output-root")
        .arg(&output_root)
        .arg("--task-dir")
        .arg(&task_dir)
        .arg("--task-name")
        .arg(safe_name)
        .arg("--preset-id")
        .arg(&payload.preset_id)
        .arg("--engines")
        .arg(payload.engine_ids.join(","))
        .arg("--preprocess-profile")
        .arg(preprocess_profile)
        .arg("--language")
        .arg(language)
        .arg("--fusion-mode")
        .arg(fusion_mode)
        // Voice uses the runner's durable metadata, so it must not inherit a
        // pipe whose reader disappears when the host exits. Other ASR callers
        // retain their existing live event transport.
        .stdout(if detached_stdio {
            std::process::Stdio::null()
        } else {
            std::process::Stdio::piped()
        })
        .stderr(if detached_stdio {
            std::process::Stdio::null()
        } else {
            std::process::Stdio::piped()
        })
        .kill_on_drop(false);
    if payload.allow_system_drive {
        command.arg("--allow-system-drive");
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动 ASR 任务失败：{error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    record.status = "running".to_string();
    record.pid = child.id();
    record.updated_at = now_iso();
    atomic_write_json(&job_path, &record)?;

    task_map().lock().await.insert(
        job_id.clone(),
        RunningAsrTask {
            child,
            record: record.clone(),
            job_path: job_path.clone(),
        },
    );
    emit_event(
        &app,
        json!({"kind":"started","jobId":job_id,"taskDir":task_dir,"status":"running"}),
    );

    if let Some(stdout) = stdout {
        let app = app.clone();
        let job_id = job_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let payload = serde_json::from_str::<Value>(&line)
                    .unwrap_or_else(|_| json!({"event":"stream","message":redact_message(&line)}));
                emit_event(
                    &app,
                    json!({"kind":"progress","jobId":job_id,"payload":payload}),
                );
            }
        });
    }
    if let Some(stderr) = stderr {
        let app = app.clone();
        let job_id = job_id.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                emit_event(
                    &app,
                    json!({"kind":"diagnostic","jobId":job_id,"message":redact_message(&line)}),
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
                let mut tasks = task_map().lock().await;
                let Some(task) = tasks.get_mut(&job_id) else {
                    return;
                };
                match task.child.try_wait() {
                    Ok(Some(status)) => {
                        let mut record = task.record.clone();
                        let task_dir = PathBuf::from(&record.task_dir);
                        let reported = task_status(&task_dir);
                        let (task_state, issue) = crate::query::finished_status(reported.as_deref(), status.success());
                        record.status = task_state.to_string();
                        if let Some(issue) = issue { record.error = Some(issue.into()); }
                        record.exit_code = status.code();
                        record.pid = None;
                        record.updated_at = now_iso();
                        record.completed_at = Some(now_iso());
                        if !status.success() && record.status == "failed" {
                            record.error = Some(format!(
                                "ASR Runner 非零退出：{}",
                                status.code().unwrap_or(-1)
                            ));
                        }
                        let _ = atomic_write_json(&task.job_path, &record);
                        emit_event(
                            &app,
                            json!({"kind":"finished","jobId":job_id,"taskDir":record.task_dir,"status":record.status,"exitCode":record.exit_code}),
                        );
                        tasks.remove(&job_id);
                        return;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let mut record = task.record.clone();
                        record.status = "failed".to_string();
                        record.pid = None;
                        record.updated_at = now_iso();
                        record.completed_at = Some(now_iso());
                        record.error = Some(redact_message(error.to_string()));
                        let _ = atomic_write_json(&task.job_path, &record);
                        emit_event(
                            &app,
                            json!({"kind":"finished","jobId":job_id,"taskDir":record.task_dir,"status":"failed","message":record.error}),
                        );
                        tasks.remove(&job_id);
                        return;
                    }
                }
            }
        });
    }

    let command_preview = format!(
        "{} asr_task_runner.py run --source <录音> --preset-id {} --engines {} --fusion-mode {}",
        python.display(),
        payload.preset_id,
        payload.engine_ids.join(","),
        fusion_mode
    );
    Ok(Some(AsrTaskStartResult {
        job_id,
        task_dir: task_dir.to_string_lossy().to_string(),
        job_state_path: job_path.to_string_lossy().to_string(),
        command_preview,
        preflight,
    }))
}

#[tauri::command]
pub async fn asr_task_get(
    task_dir: String,
    output_root: Option<String>,
    job_id: Option<String>,
) -> Result<AsrTaskView, String> {
    let output_root = configured_path(output_root, "HOMEBASE_ASR_OUTPUT_ROOT", DEFAULT_OUTPUT_ROOT);
    let task_dir = canonical_task_under_root(Path::new(&task_dir), &output_root)?;
    let job = match job_id {
        Some(job_id) => {
            if !crate::task_recovery::valid_job_id(&job_id) { return Err("QUERY_INVALID: ASR 任务编号无效".into()); }
            let output_root = task_dir
                .parent()
                .ok_or_else(|| "任务目录没有父目录。".to_string())?;
            let path = output_root.join("_jobs").join(format!("{job_id}.json"));
            if path.exists() {
                Some(read_job(&path)?)
            } else {
                None
            }
        }
        None => None,
    };
    build_task_view(&task_dir, job, true)
}

#[tauri::command]
pub async fn asr_task_read_artifact(
    task_dir: String,
    relative_path: String,
    output_root: Option<String>,
) -> Result<AsrArtifactContent, String> {
    let output_root = configured_path(output_root, "HOMEBASE_ASR_OUTPUT_ROOT", DEFAULT_OUTPUT_ROOT);
    let task_dir = canonical_task_under_root(Path::new(&task_dir), &output_root)?;
    read_task_artifact(&task_dir, &relative_path)
}

#[tauri::command]
pub async fn asr_task_organize(
    app: AppHandle,
    payload: AsrTaskOrganizePayload,
) -> Result<AsrTaskView, String> {
    let runtime = distribution::bootstrap(&app)?;
    let output_root = configured_path(
        payload.output_root,
        "HOMEBASE_ASR_OUTPUT_ROOT",
        DEFAULT_OUTPUT_ROOT,
    );
    let canonical_task_dir = canonical_task_under_root(Path::new(&payload.task_dir), &output_root)?;
    if !canonical_task_dir.join("task.meta.json").is_file()
        || !canonical_task_dir
            .join("compare")
            .join("run-matrix.json")
            .is_file()
    {
        return Err(
            "请选择包含 task.meta.json 和 compare/run-matrix.json 的 ASR 任务包。".to_string(),
        );
    }
    if task_map()
        .lock()
        .await
        .values()
        .any(|task| Path::new(&task.record.task_dir) == canonical_task_dir)
    {
        return Err("当前任务仍在运行，不能同时整理结果。".to_string());
    }
    let fusion_mode = payload.fusion_mode.as_deref().unwrap_or("auto");
    if !matches!(fusion_mode, "manual" | "llm" | "auto") {
        return Err("融合模式只允许 manual、llm 或 auto。".to_string());
    }
    let runner = validate_absolute_file(
        &distribution::resolve_runtime_file(
            &runtime,
            payload.runner_path.as_deref(),
            "HOMEBASE_ASR_RUNNER",
            &distribution::asr_runner_path(&runtime),
            Path::new(DEFAULT_RUNNER),
        ),
        "ASR Runner",
    )?;
    let python = validate_absolute_file(
        &distribution::resolve_runtime_file(
            &runtime,
            payload.python_path.as_deref(),
            "HOMEBASE_ASR_PYTHON",
            &runtime.python_path,
            Path::new(DEFAULT_PYTHON),
        ),
        "Python 运行时",
    )?;
    let mut command = Command::new(&python);
    apply_distribution_environment(&mut command, &runtime, &output_root);
    command
        .arg(&runner)
        .arg("organize")
        .arg("--task-dir")
        .arg(&canonical_task_dir)
        .arg("--fusion-mode")
        .arg(fusion_mode)
        .kill_on_drop(true);
    if let Some(model) = payload
        .fusion_model
        .filter(|value| !value.trim().is_empty())
    {
        command.arg("--fusion-model").arg(model);
    }
    let output = tokio::time::timeout(Duration::from_secs(300), command.output())
        .await
        .map_err(|_| "ASR 语义整理超过 5 分钟，已停止等待。".to_string())?
        .map_err(|error| format!("启动 ASR 语义整理失败：{error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let event = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| serde_json::from_str::<Value>(line).ok());
    if !output.status.success() {
        let message = event
            .as_ref()
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .map(redact_message)
            .unwrap_or_else(|| redact_message(String::from_utf8_lossy(&output.stderr)));
        return Err(if message.is_empty() {
            "ASR 语义整理失败；各引擎原稿未受影响。".to_string()
        } else {
            message
        });
    }
    build_task_view(&canonical_task_dir, None, true)
}

#[tauri::command]
pub async fn asr_task_list(
    output_root: String,
    limit: Option<usize>,
) -> Result<Vec<AsrTaskView>, String> {
    let root = PathBuf::from(output_root);
    if !root.is_absolute() || !root.is_dir() {
        return Err("ASR 输出根目录不存在或不是绝对目录。".to_string());
    }
    let mut task_dirs = crate::query::entries(&root, false)?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && !path
                    .file_name()
                    .and_then(|item| item.to_str())
                    .unwrap_or_default()
                    .starts_with('_')
                && path.join("task.meta.json").is_file()
        })
        .collect::<Vec<_>>();
    task_dirs.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    let cap = limit.unwrap_or(100).clamp(1, 500);
    let jobs = read_jobs(&root)?;
    let mut result = Vec::new();
    for task_dir in task_dirs.into_iter().take(cap) {
        let task_dir_string = task_dir.to_string_lossy().to_string();
        let job = jobs
            .iter()
            .find(|item| item.task_dir == task_dir_string)
            .cloned();
        result.push(build_task_view(&task_dir, job, false)?);
    }
    Ok(result)
}

fn read_jobs(output_root: &Path) -> Result<Vec<AsrJobRecord>, String> {
    let jobs_dir = output_root.join("_jobs");
    let mut result = Vec::new();
    for entry in crate::query::entries(&jobs_dir, true)? {
        let path = entry.path();
        if path.extension().and_then(|item| item.to_str()) == Some("json") {
            let job = read_job(&path)?;
            if !crate::task_recovery::valid_job_id(&job.job_id)
                || path.file_stem().and_then(|name| name.to_str()) != Some(job.job_id.as_str()) {
                return Err("QUERY_INVALID: ASR 任务编号与状态文件不一致".into());
            }
            result.push(job);
        }
    }
    result.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(result)
}

/// 只读恢复转写监控：原始状态保留；无法确认归属时返回未知观察。
#[tauri::command]
pub async fn asr_task_reconcile(output_root: String) -> Result<Vec<Value>, String> {
    let root = PathBuf::from(output_root);
    if !root.is_absolute() { return Err("QUERY_INVALID: ASR 输出根目录必须是绝对目录".into()); }
    match fs::metadata(&root) {
        Ok(metadata) if metadata.is_dir() => {},
        Ok(_) => return Err("QUERY_INVALID: ASR 输出根路径不是目录".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("QUERY_FAILED: ASR 输出根目录不可读取".into()),
    }
    let active: HashMap<String, PathBuf> = task_map().lock().await.iter()
        .map(|(id, task)| (id.clone(), task.job_path.clone())).collect();
    read_jobs(&root)?.into_iter().map(|job| {
        if !crate::task_recovery::valid_job_id(&job.job_id) {
            return Err("QUERY_INVALID: ASR 任务编号无效".into());
        }
        let path = root.join("_jobs").join(format!("{}.json", job.job_id));
        let owned = active.get(&job.job_id).is_some_and(|owned| crate::task_recovery::same_file(owned, &path));
        crate::task_recovery::observe(serde_json::to_value(job).map_err(|_| "QUERY_INVALID: ASR 任务记录无效")?, owned)
    }).collect()
}

#[tauri::command]
pub async fn asr_task_cancel(app: AppHandle, job_id: String) -> Result<(), String> {
    let mut tasks = task_map().lock().await;
    let Some(task) = tasks.get_mut(&job_id) else {
        return Err(format!("ASR 任务 {job_id} 不存在或已结束。"));
    };
    let task_dir = PathBuf::from(&task.record.task_dir);
    fs::create_dir_all(task_dir.join("execution"))
        .map_err(|error| format!("创建取消标记目录失败：{error}"))?;
    fs::write(
        task_dir.join("execution").join("cancel.requested"),
        now_iso(),
    )
    .map_err(|error| format!("写入取消标记失败：{error}"))?;
    task.child
        .kill()
        .await
        .map_err(|error| format!("终止 ASR 进程失败：{error}"))?;
    task.record.status = "cancelled".to_string();
    task.record.pid = None;
    task.record.updated_at = now_iso();
    task.record.completed_at = Some(now_iso());
    atomic_write_json(&task.job_path, &task.record)?;
    update_task_meta_status(&task_dir, "cancelled")?;
    let task_dir_string = task.record.task_dir.clone();
    tasks.remove(&job_id);
    emit_event(
        &app,
        json!({"kind":"finished","jobId":job_id,"taskDir":task_dir_string,"status":"cancelled"}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn task_recovery_asr_rejoins_only_owned_file_in_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("_jobs")).unwrap();
        std::fs::create_dir(other.path().join("_jobs")).unwrap();
        let id = "task-recovery-owned-asr";
        let record = recovery_record(dir.path(), id, "running");
        let path = dir.path().join("_jobs").join(format!("{id}.json"));
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        std::fs::write(other.path().join("_jobs").join(format!("{id}.json")), serde_json::to_vec(&record).unwrap()).unwrap();
        let before = crate::task_recovery::snapshot(dir.path());
        let child = tokio::process::Command::new(std::env::current_exe().unwrap()).arg("--list").env_clear()
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        super::task_map().lock().await.insert(id.into(), super::RunningAsrTask { child, record, job_path:path });
        let owned = super::asr_task_reconcile(dir.path().to_string_lossy().into()).await.unwrap();
        let foreign = super::asr_task_reconcile(other.path().to_string_lossy().into()).await.unwrap();
        let mut task = super::task_map().lock().await.remove(id).unwrap();
        let _ = task.child.wait().await;
        assert_eq!(owned[0]["runtime_state"], "managed");
        assert_eq!(foreign[0]["runtime_state"], "unmanaged");
        assert_eq!(before, crate::task_recovery::snapshot(dir.path()));
    }
    fn recovery_record(root: &std::path::Path, id: &str, status: &str) -> super::AsrJobRecord {
        super::AsrJobRecord { schema_version:1, job_id:id.into(), status:status.into(), source_path:"synthetic.wav".into(), output_root:root.to_string_lossy().into(), task_dir:root.join("task").to_string_lossy().into(), preset_id:"synthetic".into(), engine_ids:vec![], pid:Some(std::process::id()), created_at:"original".into(), updated_at:"original".into(), completed_at:None, exit_code:None, error:None }
    }

    #[tokio::test]
    async fn task_recovery_asr_repeated_observation_keeps_all_files_and_times() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("_jobs")).unwrap();
        std::fs::create_dir(dir.path().join("task")).unwrap();
        std::fs::write(dir.path().join("task/task.meta.json"), r#"{"status":"running","preserve":"synthetic"}"#).unwrap();
        for (index, status) in ["queued", "preflight", "preprocessing", "running", "fusing", "completed", "failed", "cancelled", "interrupted", "future-state"].iter().enumerate() {
            let id = format!("synthetic-{index}");
            let job = recovery_record(dir.path(), &id, status);
            std::fs::write(dir.path().join("_jobs").join(format!("{id}.json")), serde_json::to_vec_pretty(&job).unwrap()).unwrap();
        }
        let before = crate::task_recovery::snapshot(dir.path());
        for _ in 0..8 {
            let rows = super::asr_task_reconcile(dir.path().to_string_lossy().into()).await.unwrap();
            assert_eq!(rows.len(), 10);
            for row in rows {
                assert_eq!(row["updatedAt"], "original");
                assert_eq!(row["pid"], std::process::id());
                let status = row["status"].as_str().unwrap();
                if ["queued", "preflight", "preprocessing", "running", "fusing"].contains(&status) { assert_eq!(row["runtime_state"], "unmanaged"); }
            }
        }
        assert_eq!(before, crate::task_recovery::snapshot(dir.path()));
    }

    #[tokio::test]
    async fn task_recovery_asr_corrupt_mismatched_and_external_records_never_write() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-created");
        assert!(super::asr_task_reconcile(missing.to_string_lossy().into()).await.unwrap().is_empty());
        assert!(!missing.exists());
        std::fs::create_dir(dir.path().join("_jobs")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sentinel = outside.path().join("task.meta.json");
        std::fs::write(&sentinel, r#"{"status":"running","keep":true}"#).unwrap();
        let mut job = recovery_record(dir.path(), "good", "running");
        job.task_dir = outside.path().to_string_lossy().into();
        std::fs::write(dir.path().join("_jobs/good.json"), serde_json::to_vec(&job).unwrap()).unwrap();
        let before = crate::task_recovery::snapshot(dir.path());
        let external = crate::task_recovery::snapshot(outside.path());
        super::asr_task_reconcile(dir.path().to_string_lossy().into()).await.unwrap();
        assert_eq!(external, crate::task_recovery::snapshot(outside.path()));
        assert_eq!(before, crate::task_recovery::snapshot(dir.path()));
        std::fs::write(dir.path().join("_jobs/broken.json"), "{unfinished").unwrap();
        let corrupt = crate::task_recovery::snapshot(dir.path());
        assert!(super::asr_task_reconcile(dir.path().to_string_lossy().into()).await.is_err());
        assert_eq!(corrupt, crate::task_recovery::snapshot(dir.path()));
        std::fs::remove_file(dir.path().join("_jobs/broken.json")).unwrap();
        std::fs::rename(dir.path().join("_jobs/good.json"), dir.path().join("_jobs/wrong.json")).unwrap();
        let mismatch = crate::task_recovery::snapshot(dir.path());
        assert!(super::asr_task_reconcile(dir.path().to_string_lossy().into()).await.is_err());
        assert_eq!(mismatch, crate::task_recovery::snapshot(dir.path()));
        assert_eq!(external, crate::task_recovery::snapshot(outside.path()));
    }
    use super::*;
    #[tokio::test]
    async fn query_safety_task_list_and_optional_artifacts_report_corruption() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_optional_json(&dir.path().join("absent.json")).unwrap().is_none());
        let jobs = dir.path().join("_jobs");
        fs::create_dir(&jobs).unwrap();
        let path = jobs.join("broken.json");
        fs::write(&path, "synthetic invalid json").unwrap();
        assert!(read_optional_json(&path).is_err());
        assert!(asr_task_list(dir.path().to_string_lossy().into(), None).await.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "synthetic invalid json");
    }
    use tempfile::tempdir;

    #[test]
    fn task_name_keeps_chinese_and_removes_windows_illegal_characters() {
        assert_eq!(
            sanitize_task_name("采购:会议/复盘?  \"A\""),
            "采购-会议-复盘-A"
        );
    }

    #[test]
    fn redaction_removes_header_values() {
        let value = redact_message("Authorization: Bearer sk-example-secret-value request failed");
        assert!(!value.contains("sk-example"));
        assert!(value.contains("[redacted]"));
    }

    #[test]
    fn job_record_round_trips_without_credentials() {
        let record = AsrJobRecord {
            schema_version: 1,
            job_id: "job".into(),
            status: "running".into(),
            source_path: r"D:\audio.mp3".into(),
            output_root: r"C:\HomeBase\Wiki\raw\audio".into(),
            task_dir: r"C:\HomeBase\Wiki\raw\audio\task".into(),
            preset_id: "asr_full_audit".into(),
            engine_ids: vec!["deepgram_nova_3".into()],
            pid: Some(42),
            created_at: now_iso(),
            updated_at: now_iso(),
            completed_at: None,
            exit_code: None,
            error: None,
        };
        let serialized = serde_json::to_string(&record).unwrap();
        assert!(!serialized.to_ascii_lowercase().contains("api_key"));
        assert!(!serialized.to_ascii_lowercase().contains("authorization"));
    }

    #[test]
    fn artifact_inventory_only_exposes_canonical_result_files() {
        let temp = tempdir().unwrap();
        let task_dir = temp.path();
        fs::create_dir_all(task_dir.join("runs").join("engine_a")).unwrap();
        fs::create_dir_all(task_dir.join("compare")).unwrap();
        fs::create_dir_all(task_dir.join("final")).unwrap();
        fs::create_dir_all(task_dir.join("source")).unwrap();
        fs::write(
            task_dir.join("runs").join("engine_a").join("transcript.md"),
            "engine",
        )
        .unwrap();
        fs::write(task_dir.join("compare").join("run-matrix.json"), "{}").unwrap();
        fs::write(task_dir.join("final").join("transcript.md"), "final").unwrap();
        fs::write(task_dir.join("source").join("private.wav"), b"audio").unwrap();

        let inventory = collect_task_artifacts(task_dir).unwrap();
        let paths = inventory
            .iter()
            .map(|item| item.relative_path.as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"runs/engine_a/transcript.md"));
        assert!(paths.contains(&"compare/run-matrix.json"));
        assert!(paths.contains(&"final/transcript.md"));
        assert!(!paths.iter().any(|path| path.starts_with("source/")));
    }

    #[test]
    fn artifact_reader_rejects_path_escape() {
        let temp = tempdir().unwrap();
        let task_dir = temp.path().join("task");
        fs::create_dir_all(task_dir.join("final")).unwrap();
        fs::write(task_dir.join("final").join("transcript.md"), "ok").unwrap();
        fs::write(temp.path().join("outside.md"), "private").unwrap();

        let error = read_task_artifact(&task_dir, "../outside.md").unwrap_err();
        assert!(error.contains("任务包"));
    }

    #[test]
    fn task_access_is_bound_to_the_configured_output_root() {
        let temp = tempdir().unwrap();
        let output_root = temp.path().join("audio");
        let task_dir = output_root.join("task-a");
        let outside = temp.path().join("outside-task");
        fs::create_dir_all(&task_dir).unwrap();
        fs::create_dir_all(&outside).unwrap();

        let resolved = canonical_task_under_root(&task_dir, &output_root).unwrap();
        assert_eq!(resolved, fs::canonicalize(&task_dir).unwrap());

        let error = canonical_task_under_root(&outside, &output_root).unwrap_err();
        assert!(error.contains("输出根目录"));
    }
}
