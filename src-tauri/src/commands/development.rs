//! Local coding workflow adapter. Credentials stay in the existing Autoloop worker.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentRequest {
    pub request_id: String,
    pub workspace: String,
    pub brief: String,
    pub workflow: Option<String>,
    pub checks: Option<Vec<Vec<String>>>,
    pub protected_files: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentAnswer {
    pub task_id: String,
    pub card_id: u64,
    pub option: u32,
}

async fn adapter(action: &str, request: Value) -> Result<Value, String> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("开发适配器目录无效")?
        .join("scripts");
    let executable = std::env::current_exe().map_err(|_| "无法定位当前程序")?;
    let packaged = executable
        .parent()
        .ok_or("无法定位当前程序目录")?
        .join("distribution/capability-pipeline");
    let scripts = if packaged.join("development-capability.mjs").is_file() {
        packaged
    } else {
        source
    };
    let script = scripts.join("development-capability.mjs");
    if !script.is_file() {
        return Err("开发能力运行包缺失，未启动任务".into());
    }
    let bundled_node = executable
        .parent()
        .unwrap()
        .join("distribution/agent-runtime/node.exe");
    let node = if bundled_node.is_file() {
        bundled_node
    } else {
        PathBuf::from("node")
    };
    let payload =
        serde_json::to_vec(&json!({"home":crate::adapters::homebase_home_dir(),"request":request}))
            .map_err(|_| "开发参数无法编码")?;
    if payload.len() > 64000 {
        return Err("开发参数超过大小限制".into());
    }
    let mut command = Command::new(node);
    command.arg(script).arg(action).env_clear();
    for key in [
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATH",
        "PATHEXT",
        "USERPROFILE",
        "HOME",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|_| "无法启动开发适配器，请检查本机 Node")?;
    if let Some(mut input) = child.stdin.take() {
        input
            .write_all(&payload)
            .await
            .map_err(|_| "开发请求发送未确认，请保留原编号")?;
    }
    let output = tokio::time::timeout(Duration::from_secs(90), child.wait_with_output())
        .await
        .map_err(|_| "开发受理等待超时；只查询原任务，不能重新创建请求")?
        .map_err(|_| "开发响应未确认；只查询原任务")?;
    if output.stdout.len() > 2 * 1024 * 1024 {
        return Err("开发结果超过响应上限".into());
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "开发响应无效，请查询原任务")?;
    if !output.status.success() || value["status"] == "failed" {
        let code = value["code"]
            .as_str()
            .filter(|s| {
                s.starts_with("DEVELOPMENT_")
                    && s.chars().all(|c| c.is_ascii_uppercase() || c == '_')
            })
            .unwrap_or("DEVELOPMENT_FAILED");
        return Err(format!(
            "{code}: 开发调用未完成；保留原编号核对参数、依赖和任务，不自动重跑"
        ));
    }
    Ok(value)
}

/// 查询开发能力依赖、流程与最近任务；不启动模型。费用未知不代表免费。
#[tauri::command]
pub async fn development_catalog() -> Result<Value, String> {
    adapter("catalog", json!({})).await
}

/// 提交开发任务。固定request_id；workspace为已有本机绝对目录，brief为需求，checks为宿主验收argv数组，protected_files为不可改动验收文件。受理后按task_id查询，只有state=verified代表宿主验收通过。
#[tauri::command]
pub async fn development_run(request: DevelopmentRequest) -> Result<Value, String> {
    adapter(
        "start",
        serde_json::to_value(request).map_err(|_| "开发参数无效")?,
    )
    .await
}

/// 按原task_id查询开发阶段、待回答问题及宿主验收；不重新执行。
#[tauri::command]
pub async fn development_result(task_id: String) -> Result<Value, String> {
    adapter("get", json!({"task_id":task_id})).await
}

/// 回答原开发任务的待确认卡片；option从1开始，卡片必须属于task_id。
#[tauri::command]
pub async fn development_answer(request: DevelopmentAnswer) -> Result<Value, String> {
    adapter(
        "answer",
        serde_json::to_value(request).map_err(|_| "回答参数无效")?,
    )
    .await
}
