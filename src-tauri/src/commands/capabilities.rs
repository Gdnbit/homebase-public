//! Reviewed drivers expose data-only contracts. Repository code executes only in the OS sandbox.
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command};
use serde::{Deserialize, Serialize};
use tauri::Emitter;
static APP:std::sync::OnceLock<tauri::AppHandle>=std::sync::OnceLock::new();
pub fn register(app:tauri::AppHandle){let _=APP.set(app);}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoAnalyzeRequest {
    pub repository_url: String,
    pub revision: Option<String>,
    pub request_id: String,
    pub refresh: Option<bool>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoAcceptance {
    pub input: Value,
    pub contains: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoInstallRequest {
    pub project_id: String,
    pub revision: u64,
    pub request_id: String,
    pub examples: Option<Vec<RepoAcceptance>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoRunRequest {
    pub capability_id: String,
    pub input: Value,
    pub request_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoCancelRequest {
    pub project_id: Option<String>,
    pub run_id: Option<String>,
    pub operation_id: String,
}

async fn repository_adapter(action: &str, request: Value) -> Result<Value, String> {
    use crate::assistant_local::BWRAP_SHA256;
    let source=PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().ok_or("项目目录无效")?.to_path_buf();
    let directory=std::env::current_exe().ok().and_then(|p|p.parent().map(|p|p.join("distribution/capability-pipeline"))).filter(|p|p.join("repo-capabilities.mjs").is_file()).unwrap_or_else(||source.join("scripts"));
    let script=directory.join("repo-capabilities.mjs");
    if !script.is_file(){return Err("仓库能力运行包缺失，未运行其他脚本".into());}
    let context = if action == "search" {
        json!({})
    } else {
        let runtime=std::env::current_exe().ok().and_then(|p|p.parent().map(|p|p.join("distribution/agent-runtime"))).filter(|p|p.join("tools/bwrap").is_file()).unwrap_or_else(||source.join("agent-runtime"));
        let executable = runtime
            .join("tools/bwrap");
        let home=crate::adapters::homebase_home_dir();
        json!({"data_root":home.join(".homebase/capabilities/projects"),"output_root":home.join(".homebase/wechat/workspace/repo-capabilities"),"sandbox":{"kind":"wsl-bubblewrap","executable":executable,"sha256":BWRAP_SHA256,"distro":"Ubuntu","wsl_command":"wsl.exe"}})
    };
    let payload=serde_json::to_vec(&json!({"context":context,"request":request})).map_err(|_|"能力参数无法编码")?;
    if payload.len()>256*1024{return Err("能力请求超过大小上限，未执行".into());}
    let mut command=Command::new("node");command.arg(script).arg(action).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    for key in ["SystemRoot","WINDIR","COMSPEC","PATH","PATHEXT"] {if let Some(v)=std::env::var_os(key){command.env(key,v);}}
    #[cfg(windows)] command.creation_flags(0x08000000);
    let mut child=command.spawn().map_err(|_|"无法启动能力运行包，请检查Node安装")?;
    let mut input=child.stdin.take().ok_or("能力运行输入不可用")?;
    input.write_all(&payload).await.map_err(|_|"能力请求未能发送")?;input.write_all(b"\n").await.map_err(|_|"能力请求未能发送")?;
    let output=tokio::time::timeout(Duration::from_secs(600),child.wait_with_output()).await.map_err(|_|"仓库能力等待超时；原结果未知，先查询原项目或运行编号，不重新提交")?.map_err(|_|"能力进程结果无法确认，请查询原任务")?;
    drop(input);
    if output.stdout.len()>2*1024*1024{return Err("能力响应过大，不能将截断内容作为结果".into());}
    let value:Value=serde_json::from_slice(&output.stdout).map_err(|_|"能力运行包响应无效，原结果未确认")?;
    if !output.status.success()&&value["status"]!="failed" {return Err("能力运行进程中断，不能判定成功或自动重跑".into());}
    if matches!(action,"analyze"|"install"|"run"|"cancel"|"reconcile") {if let Some(app)=APP.get(){let _=app.emit("capabilities:changed",json!({"action":action}));}}
    Ok(value)
}

#[tauri::command]
pub async fn repo_projects_list(query:Option<String>,offset:Option<u32>,limit:Option<u32>)->Result<Value,String>{repository_adapter("projects",json!({"query":query.unwrap_or_default(),"offset":offset.unwrap_or(0),"limit":limit.unwrap_or(6)})).await}
/// 根据仓库名称或 owner/repo 查询最多五个公开 GitHub 候选，只发送明确给出的查询。
#[tauri::command]
pub async fn repo_search_public(query: String) -> Result<Value, String> {
    repository_adapter("search", json!({"query":query})).await
}
#[tauri::command]
pub async fn repo_project_analyze(request:RepoAnalyzeRequest)->Result<Value,String>{repository_adapter("analyze",serde_json::to_value(request).map_err(|_|"分析参数无效")?).await}
#[tauri::command]
pub async fn repo_project_get(project_id:String)->Result<Value,String>{repository_adapter("get",json!({"project_id":project_id})).await}
#[tauri::command]
pub async fn repo_project_install(request:RepoInstallRequest)->Result<Value,String>{repository_adapter("install",serde_json::to_value(request).map_err(|_|"安装参数无效")?).await}
#[tauri::command]
pub async fn repo_capabilities_list(query:Option<String>,offset:Option<u32>,limit:Option<u32>)->Result<Value,String>{repository_adapter("capabilities",json!({"query":query.unwrap_or_default(),"offset":offset.unwrap_or(0),"limit":limit.unwrap_or(6)})).await}
pub(super) async fn repo_capability_index_entries() -> Result<Vec<Value>, String> {
    let mut snapshot = repository_adapter("capability-index", json!({})).await?;
    if snapshot["status"] == "succeeded" {
        if let Some(Value::Array(entries)) = snapshot.as_object_mut().and_then(|v| v.remove("entries")) {
            if entries.len() <= 512 {
                return Ok(entries);
            }
        }
    }
    Err("开源能力目录暂不可用；原注册内容未修改".into())
}
#[tauri::command]
pub async fn repo_capability_describe(capability_id:String)->Result<Value,String>{repository_adapter("describe",json!({"capability_id":capability_id})).await}
#[tauri::command]
pub async fn repo_capability_run(request:RepoRunRequest)->Result<Value,String>{repository_adapter("run",serde_json::to_value(request).map_err(|_|"运行参数无效")?).await}
#[tauri::command]
pub async fn repo_capability_result(run_id:String)->Result<Value,String>{repository_adapter("result",json!({"run_id":run_id})).await}
#[tauri::command]
pub async fn repo_operation_cancel(request:RepoCancelRequest)->Result<Value,String>{repository_adapter("cancel",serde_json::to_value(request).map_err(|_|"取消参数无效")?).await}
#[tauri::command]
pub async fn repo_operation_reconcile(operation_id:String)->Result<Value,String>{repository_adapter("reconcile",json!({"operation_id":operation_id})).await}

async fn adapter(action: &str, request: Value) -> Result<Value, String> {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("项目目录无效")?
        .join("scripts/capability-runtime.mjs");
    if !script.is_file() {
        return Err("能力适配器脚本缺失".into());
    }
    let mut command = Command::new("node");
    command.arg(script).arg(action).env_clear();
    for key in ["SystemRoot", "WINDIR", "COMSPEC", "PATH", "PATHEXT"] {
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
        .map_err(|_| "无法启动 Node 能力适配器，请检查 Node.js 安装")?;
    let bytes = serde_json::to_vec(&request).map_err(|_| "能力参数不可编码")?;
    if bytes.len() > 16000 {
        return Err("能力参数过大".into());
    }
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&bytes)
            .await
            .map_err(|_| "能力参数传入失败")?;
    }
    let output = tokio::time::timeout(Duration::from_secs(1200), child.wait_with_output())
        .await
        .map_err(|_| "能力适配器等待超时；请核对隔离目录与控制任务，不自动重试")?
        .map_err(|_| "能力适配器无法获取结果")?;
    if output.stdout.len() > 1024 * 1024 {
        return Err("能力响应超过上限".into());
    }
    let result: Value =
        serde_json::from_slice(&output.stdout).map_err(|_| "能力适配器没有返回有效 JSON")?;
    // Keep safe structured business failures visible instead of an empty result.
    if !output.status.success() && result["status"] != "failed" {
        return Err("能力适配器异常退出".into());
    }
    Ok(result)
}

#[tauri::command]
pub async fn capabilities_catalog(root_path: Option<String>) -> Result<Value, String> {
    adapter("catalog", json!({"root_path": root_path})).await
}
#[tauri::command]
pub async fn capability_install(root_path: String) -> Result<Value, String> {
    adapter("install", json!({"root_path": root_path})).await
}
#[tauri::command]
pub async fn capability_render(root_path: String, input: Value) -> Result<Value, String> {
    adapter("render", json!({"root_path": root_path, "input": input})).await
}
#[tauri::command]
pub async fn capability_result(root_path: String, run_id: String) -> Result<Value, String> {
    adapter("result", json!({"root_path": root_path, "run_id": run_id})).await
}
