//! Desktop/CLI adapter. Model keys remain inside the existing model capability backend.
use crate::{
    assistant::{
        Ack, Commit, Control, Engine, Model, SendRequest, SessionMeta, SessionView, Settings, State,
        TurnView,
    },
    assistant_protocol::{self, ModelRequest},
    model_capabilities::ModelCapabilityRequest,
};
use serde::Serialize;
use crate::assistant::{ContextProjection, FollowupInput, MatterInput, SteerInput, UsageSummary};
use crate::assistant_state_store::{ArchiveBackup, ArchiveDraftView, ArchiveFilePage, ArchiveRecoveredFile};
use crate::assistant::window::{TimelineRequest,SessionWindow};
use crate::assistant::session_catalog::{SessionPage, SessionQuery};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tauri::Emitter;
use tokio::sync::OnceCell;
static SERVICE: OnceCell<Arc<Engine>> = OnceCell::const_new();
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
#[path = "assistant_local_adapter.rs"]
pub(super) mod local;
#[path = "assistant_artifact_export.rs"]
mod artifact_export;
#[cfg(windows)]
#[path = "assistant_artifact_open.rs"]
mod artifact_open;
pub fn register(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

pub async fn resume_personal() {
    if let Err(error) = service().await {
        tracing::error!("个人助理状态初始化失败，未覆盖原数据：{error}");
        if let Some(app) = APP.get() {
            let _ = app.emit("assistant:service-error", "个人助理状态初始化失败，请打开助理页面查看；原数据未覆盖");
        }
    }
}

pub(crate) fn service() -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<&'static Arc<Engine>,String>>+Send>> {
    // A recovered worker can call the same initialized host. Erase the recursive
    // future type at the service boundary; initialization never awaits a worker.
    Box::pin(async { SERVICE
        .get_or_try_init(|| async {
            let root = crate::adapters::homebase_home_dir()
                .join(".homebase")
                .join("assistant");
            std::fs::create_dir_all(&root).map_err(|_| "助理状态目录不可创建")?;
            let file = root.join("state.dpapi");
            let history_root=root.join("history-v1");
            let mode=std::env::var_os("HOMEBASE_ASSISTANT_SEGMENTS");
            let test_home=std::env::var_os("HOMEBASE_TEST_HOME");
            let initialize=crate::assistant_state_store::initialization_allowed(mode.as_deref(),test_home.as_deref())?;
            let context_mode=std::env::var_os("HOMEBASE_ASSISTANT_CONTEXT_SEGMENTS");
            let initialize_context=crate::assistant_state_store::initialization_allowed(context_mode.as_deref(),test_home.as_deref())?;
            let task_mode=std::env::var_os("HOMEBASE_ASSISTANT_TASK_ARCHIVE");
            let initialize_tasks=crate::assistant_state_store::initialization_allowed(task_mode.as_deref(),test_home.as_deref())?;
            let capacity_mode=std::env::var_os("HOMEBASE_ASSISTANT_CAPACITY");
            let initialize_capacity=crate::assistant_state_store::initialization_allowed(capacity_mode.as_deref(),test_home.as_deref())?;
            let history_present=match std::fs::symlink_metadata(&history_root) {Ok(_)=>true,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>false,Err(_)=>return Err("历史存储状态无法核对，未回退旧快照或覆盖原文件".into())};
            let segmented=initialize||initialize_context||initialize_tasks||initialize_capacity||history_present;
            let history=if segmented {let mut store=crate::assistant_state_store::Store::new(history_root,file.clone())?;if initialize_context{store.enable_context_segments();}if initialize_capacity{store.enable_capacity_segments();}Some(Arc::new(tokio::sync::Mutex::new(store)))}else{None};
            let mut state:State=if let Some(history)=&history {
                let snapshot=history.lock().await.load_index()?;
                if let Some(snapshot)=snapshot {
                    crate::assistant::archive::load(history.clone(),snapshot).await?
                }else{
                    if !initialize&&!initialize_context&&!initialize_tasks&&!initialize_capacity{return Err("历史索引缺失，未回退为空或旧快照".into());}
                    match crate::assistant_state_store::read_legacy(&file)? {Some(plain)=>serde_json::from_slice(&plain).map_err(|_|"助理状态无法解析，原记录保留")?,None=>State::default()}
                }
            }else{
                match crate::assistant_state_store::read_legacy(&file)? {Some(plain)=>serde_json::from_slice(&plain).map_err(|_|"助理状态无法解析，未回退或覆盖为空配置")?,None=>State::default()}
            };
            // Only the current setting is projected. Stored turns keep their original engine.
            state.settings.runtime = "pi".into();
            if initialize_tasks{crate::assistant::archive::enable_tasks(&mut state);}
            if initialize_capacity{crate::assistant::archive::enable_capacity(&mut state);}
            let save: Commit = Arc::new(move |state, protected| {
                let file = file.clone();
                let history=history.clone();
                Box::pin(async move {
                    let changed=state.changed.clone();
                    let state = if let Some(history)=history {
                        crate::assistant::archive::commit(history,state,protected).await?
                    }else{
                    crate::assistant::archive::ensure_legacy(&state)?;
                    let plain = zeroize::Zeroizing::new(
                        serde_json::to_vec(&state).map_err(|_| "助理状态编码失败")?,
                    );
                    let cipher = crate::wechat_crypto::crypt(&plain, true)?;
                    crate::control_protocol::publish_private(&file, &cipher)
                        .await
                        .map_err(|_| "助理状态加密保存失败，未执行后续动作")?;
                        state
                    };
                    if let Some(app) = APP.get() {
                        let _ = app.emit("assistant:changed", changed);
                    }
                    Ok(state)
                })
            });
            let model: Model = Arc::new(|request: ModelRequest| {
                Box::pin(async move {
                    let route = super::model_capabilities::find_route(&request.route_id)?;
                    let input = if let Some(vision) = &request.vision {
                        if request.free_only && !route.model_id.ends_with(":free") {
                            return Err("免费模式下，请为图片选择明确免费的视觉模型".into());
                        }
                        json!({"vision":vision})
                    } else {
                        assistant_protocol::body(&route.protocol, &route.provider_id, &request)?
                    };
                    let result =
                        super::model_capabilities::model_capability_call_bounded(ModelCapabilityRequest {
                            route_id: request.route_id,
                            input,
                            output_root: None,
                            credential_source: request.credential_source,
                        }, request.vision.as_ref().map(|_| request.max_tokens))
                        .await?;
                    if result.status != "succeeded" {
                        return Err(format!(
                            "助理模型没有成功返回：{}；不会自动切换渠道或重试",
                            result.message
                        ));
                    }
                    assistant_protocol::parse(&route.protocol, &result.output).map_err(|_|crate::model_capability_io::reliability::Failure::unknown(crate::model_capability_io::reliability::Kind::InvalidResponse).encode())
                })
            });
            let control: Control = Arc::new(|request| Box::pin(async move {
                if request.method=="assistant.local_read" {
                    if !request.grants.iter().any(|g|g=="private"){return Err(crate::control_protocol::ControlError::new("SCOPE_REQUIRED","本机读取需要私人范围"));}
                    let operation=request.args["operation"].as_str().ok_or_else(||crate::control_protocol::ControlError::new("BAD_ARGUMENT","缺少本机操作"))?;
                    let input=&request.args["request"];
                    if !crate::assistant_local::readonly_operation(operation,input){return Err(crate::control_protocol::ControlError::new("READ_ONLY_REQUIRED","内部读取入口不能执行修改动作"));}
                    let id=request.args["operationId"].as_str().ok_or_else(||crate::control_protocol::ControlError::new("BAD_ARGUMENT","缺少操作编号"))?;
                    let revision=request.args["policyRevision"].as_u64().ok_or_else(||crate::control_protocol::ControlError::new("BAD_ARGUMENT","缺少授权版本"))?;
                    return Ok(match local::execute(operation.into(),input.clone(),id.into(),revision).await {Ok(v)=>v,Err(message)=>crate::assistant_local::operation_failure(message)});
                }
                if request.method=="assistant.local_progress" {
                    if !request.grants.iter().any(|g|g=="private") {return Err(crate::control_protocol::ControlError::new("SCOPE_REQUIRED","查询实时输出需要私人读取授权"));}
                    let id=request.args["operationId"].as_str().ok_or_else(||crate::control_protocol::ControlError::new("BAD_ARGUMENT","缺少本机操作编号"))?;
                    return local::progress(id.into()).await.map_err(|_|crate::control_protocol::ControlError::new("LOCAL_PROGRESS_UNAVAILABLE","本机实时输出暂不可用"));
                }
                if request.method == "assistant.control_receipt" {
                    return crate::control_plane::assistant_job_receipt(&request).await;
                }
                crate::control_plane::channel_request(request).await
            }));
            // Reuse the already verified owned runtime; every generated file is in a fresh run directory.
            let workspace = assistant_workspace();
            let documents=Arc::new(crate::assistant_documents::DocumentStore::new(root.join("documents")));
            let engine = Engine::new_with_commit(state, save, model, control, workspace, documents).await?;
            engine.configure_dialogue_stream(
                Arc::new(|request, delta| Box::pin(super::model_capabilities::assistant_chat_stream(request, delta))),
                Arc::new(|session_id, turn_id| {
                    if let Some(app) = APP.get() {
                        let _ = app.emit("assistant:preview", json!({"session_id":session_id,"turn_id":turn_id}));
                    }
                }),
            )?;
            Ok(engine)
        })
        .await })
}

#[derive(Serialize)]
pub struct AssistantConfig {
    pub settings: Settings,
    pub workspace_root: String,
    pub model_state: String,
    pub message: String,
}
#[tauri::command]
pub async fn assistant_config_get() -> Result<AssistantConfig, String> {
    let e = service().await?;
    let settings = e.settings().await;
    let enabled = settings.enabled;
    let free_only = settings.free_only;
    Ok(AssistantConfig {
        settings,
        workspace_root: e.workspace.to_string_lossy().into(),
        model_state: if enabled {
            "configured_not_remote_verified"
        } else {
            "disabled"
        }
        .into(),
        message: if enabled && free_only {
            "仅免费模式：使用所选凭据引用与明确免费路由，零价格上限；限流、认证或免费端点不可用时停止，不切换付费模型"
        } else if enabled {
            "使用已选模型渠道；发送消息会产生正常 API 费用，实际工具能力及余额以调用结果为准"
        } else {
            "模型暂未启用，不会发起真实推理；微信明确指令保持可用"
        }
        .into(),
    })
}

/// Read-only projection; never resumes a task, changes policy or reads key values.
#[tauri::command]
pub async fn assistant_context_get(session_id: Option<String>, query: Option<String>) -> Result<ContextProjection, String> {
    let value=service().await?.context(session_id.as_deref(),query.as_deref().unwrap_or("")).await?;
    serde_json::from_value(value).map_err(|_|"上下文摘要契约不匹配，未返回不完整内容".into())
}

#[tauri::command]
pub async fn assistant_usage_get() -> Result<UsageSummary, String> { Ok(service().await?.usage().await) }
#[tauri::command]
pub async fn assistant_config_save(settings: Settings) -> Result<AssistantConfig, String> {
    if settings.runtime != "pi" {
        return Err("当前只支持 Pi；旧引擎仅保留历史记录".into());
    }
    if settings.enabled {
        let route = super::model_capabilities::find_route(&settings.route_id)?;
        let request = ModelRequest {
            vision: None,
            route_id: settings.route_id.clone(),
            messages: vec![json!({"role":"user","content":"配置校验，不发出请求"})],
            max_tokens: settings.max_tokens,
            credential_source: settings.credential_source,
            free_only: settings.free_only,
            definitions: None,
        };
        let input = assistant_protocol::body(&route.protocol, &route.provider_id, &request)?;
        let plan = super::model_capabilities::model_capability_resolve(ModelCapabilityRequest {
            route_id: settings.route_id.clone(),
            input,
            output_root: None,
            credential_source: settings.credential_source,
        })
        .await?;
        if !plan.callable {
            return Err("该渠道凭据不可用，未启用模型；请在密钥页面检查".into());
        }
        super::model_capabilities::remember_route(route).await?;
    }
    service().await?.save_settings(settings).await?;
    assistant_config_get().await
}

#[tauri::command]
pub async fn assistant_personal_get() -> Result<Value, String> {
    Ok(service().await?.personal().await)
}
#[tauri::command]
pub async fn assistant_matters_get() -> Result<Value, String> { Ok(service().await?.matters().await) }
#[tauri::command]
pub async fn assistant_matter_save(request: MatterInput) -> Result<Value, String> { service().await?.matter_save(request).await }
#[tauri::command]
pub async fn assistant_matter_review_ack(matter_id: String, revision: u64) -> Result<(), String> { service().await?.matter_review_ack(&matter_id,revision).await }
#[tauri::command]
pub async fn assistant_followup_save(request: FollowupInput) -> Result<Value, String> {
    service().await?.followup_save(request, "desktop", "desktop").await
}
#[tauri::command]
pub async fn assistant_followup_cancel(followup_id: String, revision: u64) -> Result<(), String> {
    service().await?.followup_cancel(&followup_id, revision).await
}
#[tauri::command]
pub async fn assistant_memory_save(
    request: crate::assistant_personal::MemoryInput,
) -> Result<Value, String> {
    service().await?.memory_save(request).await
}
#[tauri::command]
pub async fn assistant_turn_steer(request: SteerInput) -> Result<Value,String> {
    service().await?.steer(request).await
}
#[tauri::command]
pub async fn assistant_memory_search(query: String, offset: Option<usize>) -> Result<Value,String> {
    service().await?.memory_search(&query,offset.unwrap_or(0)).await
}
#[tauri::command]
pub async fn assistant_memory_correct(request: crate::assistant_personal::MemoryCorrectionInput) -> Result<Value,String> {
    service().await?.memory_correct(request).await
}
#[tauri::command]
pub async fn assistant_memory_forget(memory_id: String, revision: u64) -> Result<(), String> {
    service().await?.memory_forget(&memory_id, revision).await
}
#[tauri::command]
pub async fn assistant_reminder_save(
    request: crate::assistant_personal::ReminderInput,
) -> Result<Value, String> {
    service()
        .await?
        .reminder_save(request, "desktop", "desktop")
        .await
}
#[tauri::command]
pub async fn assistant_reminder_ack(reminder_id: String, cancel: bool) -> Result<(), String> {
    service().await?.reminder_ack(&reminder_id, cancel).await
}
#[tauri::command]
pub async fn assistant_resume(turn_id: String, expected_generation: Option<u64>) -> Result<Ack, String> {
    service().await?.resume_once(&turn_id, expected_generation).await
}
#[tauri::command]
pub async fn assistant_queue_resume(session_id: String, expected_generation: Option<u64>) -> Result<Value, String> {
    service().await?.resume_queue_once(&session_id, expected_generation).await
}
#[tauri::command]
pub async fn assistant_sessions_list() -> Result<Vec<SessionMeta>, String> {
    Ok(service().await?.sessions().await)
}
#[tauri::command]
pub async fn assistant_sessions_query(request: SessionQuery) -> Result<SessionPage, String> {
    service().await?.sessions_query(request).await
}
#[tauri::command]
pub async fn assistant_session_create(request_id: Option<String>) -> Result<SessionMeta, String> {
    service().await?.create_session_requested(request_id.as_deref()).await
}
#[tauri::command]
pub async fn assistant_session_get(session_id: String) -> Result<SessionView, String> {
    service().await?.session(&session_id).await
}
#[tauri::command]
pub async fn assistant_history_query(request: crate::assistant_history::HistoryQuery) -> Result<crate::assistant_history::HistoryPage,String> {
    service().await?.history_query(request).await
}
#[tauri::command]
pub async fn assistant_history_read(request: crate::assistant_history::HistoryRead) -> Result<crate::assistant_history::HistoryChunk,String> {
    service().await?.history_read(request).await
}
#[tauri::command]
pub async fn assistant_turn_snapshot(session_id: String, turn_id: String) -> Result<crate::assistant_history::HistoryPage, String> {
    service().await?.history_turn_snapshot(&session_id, &turn_id).await
}
fn history_store() -> Result<crate::assistant_state_store::Store, String> {
    let directory = crate::adapters::homebase_home_dir().join(".homebase/assistant");
    crate::assistant_state_store::Store::new(directory.join("history-v1"), directory.join("state.dpapi"))
}
fn assistant_workspace() -> std::path::PathBuf {
    crate::adapters::homebase_home_dir().join(".homebase/wechat/workspace")
}
#[tauri::command]
pub async fn assistant_archive_backup(backup_id: String) -> Result<ArchiveBackup, String> {
    history_store()?.backup(backup_id, assistant_workspace()).await
}
#[tauri::command]
pub async fn assistant_archive_backup_get(backup_id: String) -> Result<ArchiveBackup, String> {
    let store = history_store()?;
    tokio::task::spawn_blocking(move || store.backup_status(backup_id))
        .await.map_err(|_| "ARCHIVE_IO: 备份状态暂不可读".to_owned())?
}
#[tauri::command]
pub async fn assistant_archive_backup_files(backup_id: String, offset: Option<usize>) -> Result<ArchiveFilePage, String> {
    let store = history_store()?;
    tokio::task::spawn_blocking(move || store.backup_files(backup_id, offset.unwrap_or(0)))
        .await.map_err(|_| "ARCHIVE_IO: 备份文件清单暂不可读".to_owned())?
}
#[tauri::command]
pub async fn assistant_archive_backup_file_recover(backup_id: String, file_id: String) -> Result<ArchiveRecoveredFile, String> {
    let store = history_store()?;
    tokio::task::spawn_blocking(move || store.backup_file_recover(backup_id, file_id))
        .await.map_err(|_| "ARCHIVE_IO: 备份文件恢复暂不可确认".to_owned())?
}
#[tauri::command]
pub async fn assistant_archive_backup_draft_get(backup_id: String, file_id: String) -> Result<ArchiveDraftView, String> {
    let store = history_store()?;
    tokio::task::spawn_blocking(move || store.backup_draft_get(backup_id, file_id))
        .await.map_err(|_| "ARCHIVE_IO: 备份草稿暂不可读".to_owned())?
}
#[tauri::command]
pub async fn assistant_session_window_get(request: TimelineRequest) -> Result<SessionWindow, String> {
    service().await?.session_window(request).await
}
#[tauri::command]
pub async fn assistant_send(request: SendRequest) -> Result<Ack, String> {
    service().await?.send_checked(request, validate_route).await
}
#[tauri::command]
pub async fn assistant_send_receipt_get(request: SendRequest) -> Result<crate::assistant::SendReceiptLookup, String> {
    service().await?.receipt_lookup(&request).await
}
pub(crate) fn validate_route(request: &SendRequest) -> Result<(), String> {
    if let Some(route_id) = &request.route_id {
        let route = super::model_capabilities::find_route(route_id)?;
        if !["chat", "responses"].contains(&route.protocol.as_str()) {
            return Err("该模型渠道不支持助理对话".into());
        }
        if request.image.is_some() && !crate::model_capabilities::vision::supported(&route) {
            return Err("请选择目录已确认支持图片输入的 OpenRouter 渠道".into());
        }
    } else if request.image.is_some() {
        return Err("请为图片明确选择视觉模型".into());
    }
    Ok(())
}
#[tauri::command]
pub async fn assistant_turn_get(turn_id: String, include_details: Option<bool>) -> Result<TurnView, String> {
    let engine = service().await?;
    if include_details == Some(false) {
        engine.observe_turn(&turn_id).await
    } else {
        engine.turn_with_progress(&turn_id).await
    }
}
#[tauri::command]
pub async fn assistant_turn_inspect(session_id: String, turn_id: String) -> Result<TurnView, String> {
    service().await?.inspect_turn_steps(&session_id, &turn_id).await
}
#[tauri::command]
pub async fn assistant_turn_step_get(
    session_id: String,
    turn_id: String,
    step_id: String,
) -> Result<crate::assistant::StepView, String> {
    service().await?.read_turn_step(&session_id, &turn_id, &step_id).await
}
#[tauri::command]
pub async fn assistant_cancel(turn_id: String) -> Result<(), String> {
    service().await?.cancel(&turn_id).await
}
#[tauri::command]
pub async fn assistant_confirm(
    turn_id: String,
    confirmation_id: String,
    approve: bool,
) -> Result<Ack, String> {
    service()
        .await?
        .confirm(&turn_id, &confirmation_id, approve)
        .await
}
#[tauri::command]
pub async fn assistant_artifact_preview(artifact_id: String) -> Result<Value, String> {
    let (artifact,bytes) = service().await?.artifact_content(&artifact_id,true).await?;
    let extension = std::path::Path::new(&artifact.name).extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    let content = String::from_utf8(bytes).map_err(|_| "结果不是 UTF-8 文本，未显示乱码")?;
    Ok(serde_json::json!({"name":artifact.name,"content":content,"sha256":artifact.sha256,"markdown":extension=="md"||extension=="markdown"}))
}
#[tauri::command]
pub async fn assistant_artifact_image_preview(
    artifact_id: String,
) -> Result<tauri::ipc::Response, String> {
    let bytes = service().await?.artifact_image_content(&artifact_id).await?;
    Ok(tauri::ipc::Response::new(bytes))
}
#[tauri::command]
pub async fn assistant_artifact_open(artifact_id: String) -> Result<(), String> {
    let artifact = service().await?.artifact(&artifact_id).await?;
    #[cfg(windows)]
    {
        tauri::async_runtime::spawn_blocking(move || {
            artifact_open::open(std::path::Path::new(&artifact.path))
        })
        .await
        .map_err(|_| "ARTIFACT_OPEN_UNCONFIRMED: Windows 打开结果未确认，请检查系统应用窗口。")?
    }
    #[cfg(not(windows))]
    {
        let _ = artifact;
        Err("当前平台暂不支持打开结果，请使用返回的文件路径".into())
    }
}
#[tauri::command]
pub async fn assistant_artifact_export(
    app: tauri::AppHandle,
    artifact_id: String,
) -> Result<Option<artifact_export::ExportReceipt>, String> {
    artifact_export::run(app, artifact_id).await
}
pub(crate) async fn wechat(request: Value) -> Result<Value, String> {
    service().await?.channel(request).await
}
pub(crate) async fn wechat_pdf(request: crate::wechat::PdfRequest) -> Result<Option<String>, String> {
    service().await?.channel_pdf(request).await
}
#[tauri::command]
pub async fn assistant_local_config_get() -> Result<Value, String> { local::config_get().await }
#[tauri::command]
pub async fn assistant_local_config_save(config: crate::assistant_local::LocalConfig) -> Result<Value, String> { local::save(config).await }
#[tauri::command]
pub async fn assistant_local_execute(operation: String, request: Value, operation_id: String, policy_revision: u64) -> Result<Value, String> {
    // These errors are static, trusted adapter reasons, never raw child stderr or credentials.
    Ok(match local::execute(operation,request,operation_id,policy_revision).await {
        Ok(value)=>value,
        Err(message)=>crate::assistant_local::operation_failure(message),
    })
}
#[tauri::command]
pub async fn assistant_local_progress(operation_id: String) -> Result<Value, String> { local::progress(operation_id).await }
#[tauri::command]
pub async fn assistant_local_cancel(operation_id: String) -> Result<Value, String> { local::cancel(operation_id).await }
