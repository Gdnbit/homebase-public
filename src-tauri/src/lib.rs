//! HomeBase 控制面板 — Tauri 后端入口
//!
//! 架构（v3 §3.2）：Commands / Events / Workers / Adapters / SQLite

mod adapters;
mod commands;
mod models;
mod util;
mod query;
mod legacy_isolation;
#[cfg(test)]
mod legacy_isolation_tests;
mod task_recovery;
mod workers;
mod secret_policy;
mod control_protocol;
mod control_state;
mod control_plane;
mod task_event_store;
mod inbox_storage;
mod inbox_document;
mod inbox_receipts;
mod inbox_workspace;
mod task_events;
mod control_bootstrap;
mod model_capabilities;
mod model_capability_io;
mod wechat;
mod wechat_crypto;
mod assistant;
mod assistant_state_store;
mod assistant_history;
mod capability_discovery;
mod capability_catalog_projection;
mod assistant_input_presentation;
mod capability_facts;
mod assistant_desktop;
mod development_workspace;
mod native_presentation;
mod document_runtime;
#[cfg(windows)]
mod process_lifetime;
mod reveal_runtime;
mod assistant_documents;
mod document_service;
mod assistant_protocol;
mod assistant_runtime;
mod assistant_local_host;
mod assistant_local;
mod assistant_personal;
mod native_agent;

use std::sync::Arc;

use tauri::{Emitter, Manager};
use tauri_plugin_sql::{Migration, MigrationKind};
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 日志（v3 §14 技术债：MVP 用 tracing；Phase 14 评估 OpenTelemetry）
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("homebase=debug,info")),
        )
        .init();

    tracing::info!("HomeBase 启动 (v0.1.0)");

    // SQLite migrations（v3 §7 + §21）
    let migrations = vec![Migration {
        version: 1,
        description: "initial schema (v3 §7 + §21 含 Tasks/Skills/Chat Hub/Extensions)",
        sql: include_str!("../migrations/0001_initial.sql"),
        kind: MigrationKind::Up,
    }];

    tracing::info!("启动阶段：载入应用上下文");
    let mut context = tauri::generate_context!();
    tracing::info!("启动阶段：应用上下文已就绪");
    let test_profile = std::env::var_os("HOMEBASE_TEST_HOME").map(std::path::PathBuf::from);
    if let Some(home) = &test_profile {
        use sha2::{Digest, Sha256};
        assert!(home.is_absolute(), "HOMEBASE_TEST_HOME must be absolute");
        let hash = format!("{:x}", Sha256::digest(home.to_string_lossy().as_bytes()));
        // Known Folder paths on Windows ignore APPDATA overrides. A distinct app ID
        // isolates plugin SQL/store paths; WebView cookies/localStorage get their own directory.
        context.config_mut().identifier = format!("io.github.gdnbit.homebase.test.{}", &hash[..16]);
    }
    // Keep the original configs, but create windows after service setup. A slow
    // renderer must not prevent the authenticated CLI from starting.
    let startup_windows = std::mem::take(&mut context.config_mut().app.windows);

    tracing::info!("启动阶段：初始化原生运行时与插件");
    tauri::Builder::default()
        // Tauri 官方插件
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_sql::Builder::default()
                .add_migrations("sqlite:homebase.db", migrations)
                .build(),
        )
        // 启动钩子
        .setup(move |app| {
            tracing::info!("启动阶段：进入应用初始化");
            commands::assistant::register(app.handle().clone());
            commands::capabilities::register(app.handle().clone());
            tracing::info!("HomeBase setup hook 运行");
            match commands::distribution::bootstrap(app.handle()) {
                Ok(runtime) => tracing::info!(
                    "发行运行时已就绪 · packaged={} data={} resources={}",
                    runtime.packaged,
                    runtime.data_root.display(),
                    runtime.resource_root.display()
                ),
                Err(error) => tracing::error!("发行运行时初始化失败：{error}"),
            }
            match adapters::mcp_pool::load() {
                Ok(shared_mcp) => tracing::info!("共享 MCP 已只读检查 · servers={} assignments={}", shared_mcp.servers.len(), shared_mcp.assignments.len()),
                Err(_) => tracing::warn!("共享 MCP 配置读取失败，请在对应页面核对；未保存默认值或覆盖原配置"),
            }
            // Personal reminders run while the app is open, even before the assistant page is visited.
            tauri::async_runtime::spawn(commands::assistant::resume_personal());
            // S3 · Secrets IPC 启动 + emitter 注册（共享 secrets · 跨 4 agent runtime 询问）
            let app_handle_for_emit = app.handle().clone();
            adapters::secrets_ipc::set_emitter(Arc::new(
                move |req_id: &str, agent_id: &str, secret_name: &str, purpose: &str| {
                    let payload = serde_json::json!({
                        "request_id": req_id,
                        "agent_id": agent_id,
                        "secret_name": secret_name,
                        "purpose": purpose,
                    });
                    match app_handle_for_emit.emit("secrets:consent_request", payload) {
                        Ok(()) => true,
                        Err(e) => {
                            tracing::warn!("emit secrets:consent_request 失败: {e}");
                            false
                        }
                    }
                },
            ));
            tauri::async_runtime::spawn(async {
                match adapters::secrets_ipc::start().await {
                    Ok(ep) => tracing::info!(
                        "Secrets IPC 已启动 · {} (token len={})",
                        ep.endpoint,
                        ep.token.len()
                    ),
                    Err(e) => tracing::error!("启动 Secrets IPC 失败: {e}"),
                }
            });

            if std::env::var("HOMEBASE_CONTROL_DISABLED").as_deref() != Ok("1") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = control_plane::run(handle).await {
                        tracing::error!("本机控制服务不可用 [{}]: {}", error.code, error.message);
                    }
                });
            } else {
                control_plane::disabled();
            }

            let window_app = app.handle().clone();
            std::thread::Builder::new()
                .name("homebase-window-init".into())
                .spawn(move || {
                    for window in startup_windows.iter().filter(|window| window.create) {
                        tracing::info!(label = %window.label, "启动阶段：创建应用窗口");
                        let created = (|| -> tauri::Result<()> {
                            let mut builder = tauri::WebviewWindowBuilder::from_config(&window_app, window)?;
                            if let Some(home) = &test_profile {
                                builder = builder.data_directory(home.join("webview").join(&window.label));
                            }
                            builder.build()?;
                            Ok(())
                        })();
                        if let Err(error) = created {
                            tracing::error!(label = %window.label, %error, "应用窗口创建失败，退出应用；未重试或覆盖原数据");
                            window_app.exit(1);
                            return;
                        }
                        tracing::info!(label = %window.label, "启动阶段：应用窗口已就绪");
                    }
                    #[cfg(debug_assertions)]
                    if std::env::var_os("HOMEBASE_OPEN_DEVTOOLS").as_deref()
                        == Some(std::ffi::OsStr::new("1"))
                    {
                        if let Some(window) = window_app.get_webview_window("main") {
                            window.open_devtools();
                        }
                    }
                })?;
            tracing::info!("启动阶段：服务已注册，窗口初始化已安排");

            Ok(())
        })
        // Tauri commands（v3 §8）
        .invoke_handler(tauri::generate_handler![
            commands::ai_control::ai_control_bootstrap,
            commands::service_credentials::service_credentials_status,
            // commands::secrets
            commands::secrets::secrets_list,
            commands::secrets::secrets_test,
            commands::secrets::secrets_rotate_url,
            commands::secrets::secrets_reveal,
            commands::secrets::secrets_set,
            commands::secrets::secrets_set_meta,
            commands::secrets::secrets_save,
            commands::secrets::secrets_lifecycle_status,
            commands::secrets::secrets_test_candidate,
            // commands::agents
            commands::agents::agents_list,
            commands::agents::agents_status,
            commands::agents::agents_start,
            commands::agents::agents_open_config,
            commands::agents::agents_get_model,
            commands::agents::agents_set_model,
            commands::agents::agents_get_endpoint,
            commands::agents::agents_set_endpoint,
            commands::agents::agents_test_endpoint,
            // commands::openrouter（2026-06-07 · API 模型目录）
            commands::openrouter::openrouter_models_catalog,
            // commands::agents_history（Phase D · 2026-05-04 — 历史对话 parser；Phase H · 跨 agent 聚合）
            commands::agents_history::agents_history_list,
            commands::agents_history::agents_history_read,
            commands::agents_history::agents_history_all,
            // commands::system
            commands::system::system_disks,
            commands::system::system_info,
            commands::development::development_catalog,
            commands::development::development_run,
            commands::development::development_result,
            commands::development::development_answer,
            commands::development_workspace::development_workspace_get,
            commands::development_workspace::development_workspace_save,
            commands::development_workspace::development_workspace_prepare,
            commands::development_workspace::development_workspace_new,
            commands::capabilities::capabilities_catalog,
            commands::capabilities::repo_projects_list,
            commands::capabilities::repo_project_analyze,
            commands::capabilities::repo_search_public,
            commands::capabilities::repo_project_get,
            commands::capabilities::repo_project_install,
            commands::capabilities::repo_capabilities_list,
            commands::capabilities::repo_capability_describe,
            commands::capabilities::repo_capability_run,
            commands::capabilities::repo_capability_result,
            commands::capabilities::repo_operation_cancel,
            commands::capabilities::repo_operation_reconcile,
            commands::capabilities::capability_install,
            commands::capabilities::capability_render,
            commands::capabilities::capability_result,
            commands::model_capabilities::model_capabilities_catalog,
            commands::model_capabilities::model_capabilities_refresh,
            commands::model_capabilities::model_capability_resolve,
            commands::model_capabilities::model_capability_call,
            commands::model_capabilities::model_capability_task,
            commands::wechat::wechat_status,
            commands::wechat::wechat_start,
            commands::wechat::wechat_self_test_message,
            commands::wechat::wechat_stop,
            commands::wechat::wechat_login_begin,
            commands::wechat::wechat_pairing,
            commands::wechat::wechat_login_pin,
            commands::wechat::wechat_login_cancel,
            commands::wechat::wechat_forget,
            commands::wechat_video::capability_clip_video,
            commands::wechat_video::capability_assemble_video,
            commands::wechat_video::capability_video_publish_package,
            commands::wechat_video::capability_trim_clip_artifact,
            commands::wechat_video::capability_video_cover,
            commands::assistant::assistant_config_get,
            commands::assistant::assistant_local_config_get,
            commands::assistant::assistant_local_config_save,
            commands::assistant::assistant_local_execute,
            commands::assistant::assistant_local_progress,
            commands::assistant::assistant_local_cancel,
            commands::assistant::assistant_config_save,
            commands::assistant::assistant_personal_get,
            commands::assistant::assistant_followup_save,
            commands::assistant::assistant_matters_get,
            commands::assistant::assistant_matter_save,
            commands::assistant::assistant_matter_review_ack,
            commands::assistant::assistant_followup_cancel,
            commands::assistant::assistant_memory_save,
            commands::assistant::assistant_memory_correct,
            commands::assistant::assistant_memory_search,
            commands::assistant::assistant_turn_steer,
            commands::assistant::assistant_memory_forget,
            commands::assistant::assistant_reminder_save,
            commands::assistant::assistant_reminder_ack,
            commands::assistant::assistant_resume,
            commands::assistant::assistant_queue_resume,
            commands::assistant::assistant_sessions_list,
            commands::assistant::assistant_sessions_query,
            commands::assistant::assistant_context_get,
            commands::capability_index::capability_index_query,
            commands::assistant::assistant_usage_get,
            commands::conversations::conversation_providers,
            commands::conversations::conversation_native,
            commands::capability_index::capability_index_get,
            commands::assistant::assistant_session_create,
            commands::assistant::assistant_session_get,
            commands::assistant::assistant_session_window_get,
            commands::assistant::assistant_history_query,
            commands::assistant::assistant_history_read,
            commands::assistant::assistant_turn_snapshot,
            commands::assistant::assistant_archive_backup,
            commands::assistant::assistant_archive_backup_get,
            commands::assistant::assistant_archive_backup_files,
            commands::assistant::assistant_archive_backup_file_recover,
            commands::assistant::assistant_archive_backup_draft_get,
            commands::assistant::assistant_send,
            commands::assistant::assistant_send_receipt_get,
            commands::assistant_desktop::assistant_desktop_index,
            commands::assistant_desktop::assistant_desktop_get,
            commands::assistant_desktop::assistant_desktop_save,
            commands::assistant_desktop::assistant_desktop_image_stage,
            commands::assistant_desktop::assistant_desktop_send,
            commands::assistant_desktop::assistant_desktop_resolve,
            commands::assistant_desktop::assistant_native_send,
            commands::assistant_desktop::assistant_native_resolve,
            commands::assistant_chat::assistant_voice_stage,
            commands::assistant_chat::assistant_voice_discard,
            commands::assistant_chat::assistant_voice_begin,
            commands::assistant_chat::assistant_voice_get,
            commands::assistant_chat::assistant_voice_submit,
            commands::assistant_chat::assistant_voice_apply,
            commands::assistant_chat::assistant_voice_dismiss,
            commands::assistant_chat::assistant_voice_cancel,
            commands::assistant_documents::assistant_document_import,
            commands::assistant_documents::assistant_document_import_status,
            commands::assistant_documents::assistant_document_import_commit,
            commands::assistant_documents::assistant_document_index,
            commands::assistant_documents::assistant_document_get,
            commands::assistant_documents::assistant_document_extract,
            commands::assistant_documents::assistant_document_status,
            commands::assistant_documents::assistant_document_result,
            commands::assistant_documents::assistant_document_excerpt,
            commands::assistant_documents::assistant_document_cancel,
            commands::assistant::assistant_turn_get,
            commands::assistant::assistant_turn_inspect,
            commands::assistant::assistant_turn_step_get,
            commands::assistant::assistant_cancel,
            commands::assistant::assistant_confirm,
            commands::assistant::assistant_artifact_open,
            commands::assistant::assistant_artifact_preview,
            commands::assistant::assistant_artifact_image_preview,
            commands::assistant::assistant_artifact_export,
            commands::task_events::task_events_get,
            commands::system::system_metrics,
            commands::system::system_gpu,
            commands::system::system_preflight,
            commands::system::system_path_list,
            commands::system::system_processes,
            // commands::distribution（安装版/便携版运行目录与首次运行状态）
            commands::distribution::distribution_info,
            // commands::asr（Quick Panel · 本地/云端统一任务控制面）
            commands::asr::asr_environment_probe,
            commands::asr::asr_task_start,
            commands::asr::asr_task_get,
            commands::asr::asr_task_read_artifact,
            commands::asr::asr_task_organize,
            commands::asr::asr_task_list,
            commands::asr::asr_task_reconcile,
            commands::asr::asr_task_cancel,
            // commands::model_center（Quick Panel · 本地模型安装清单只读探测）
            commands::model_center::model_center_manifest,
            commands::model_center::model_download_start,
            commands::model_center::model_download_get,
            commands::model_center::model_download_list,
            commands::model_center::model_download_reconcile,
            commands::model_center::model_download_cancel,
            // commands::docs
            commands::docs::docs_list,
            commands::docs::docs_read,
            commands::docs::obsidian_health,
            // commands::wiki（Phase 14.5 — LLM Wiki + Typora）
            commands::wiki::wiki_list,
            commands::wiki::wiki_dirs,
            commands::wiki::wiki_read,
            commands::wiki::docs_open_in_typora,
            // commands::topology（Phase 14.5b — AI 知识库磁盘架构图）
            commands::topology::topology_load,
            commands::topology::topology_list_dir,
            commands::topology::topology_open_dir,
            commands::topology::topology_parse_refs,
            // commands::inbox（Phase 14.5c — 投料口：raw 层低门槛输入）
            commands::inbox::inbox_submit,
            commands::inbox::inbox_attach,
            commands::inbox::inbox_open_obsidian,
            commands::inbox::inbox_list_recent,
            commands::inbox::inbox_submission_get,
            commands::inbox::inbox_submission_cancel,
            commands::inbox_workspace::inbox_workspace_get,
            commands::inbox_workspace::inbox_workspace_save,
            commands::inbox_workspace::inbox_workspace_prepare,
            commands::inbox_workspace::inbox_workspace_submit,
            commands::inbox_workspace::inbox_workspace_resolve,
            commands::inbox_workspace::inbox_workspace_cancel,
            // commands::wiki_agent（Phase 14.5c — 方案 A · v1 stub）
            commands::wiki_agent::wiki_agent_status,
            commands::wiki_agent::wiki_agent_prepare_prompt,
            commands::wiki_agent::wiki_agent_open_in_terminal,
            commands::wiki_agent::wiki_agent_get_config,
            commands::wiki_agent::wiki_agent_set_config,
            commands::wiki_agent::wiki_agent_run_ingest,
            commands::wiki_agent::wiki_agent_cancel_ingest,
            // commands::gdnbit（Phase C · 2026-05-04/2026-05-06 — Wiki↔网站发布打通 · 增强版）
            commands::gdnbit::gdnbit_get_config,
            commands::gdnbit::gdnbit_set_config,
            commands::gdnbit::gdnbit_doctor,
            commands::gdnbit::gdnbit_kit_doctor,
            commands::gdnbit::gdnbit_sync_vault,
            commands::gdnbit::gdnbit_publish_file,
            commands::gdnbit::gdnbit_render_file,
            commands::gdnbit::gdnbit_list_remote,
            commands::gdnbit::gdnbit_ai_optimize,
            commands::gdnbit::gdnbit_cancel,
            // commands::studio（Phase D · 2026-05-06 — AI 写作工坊）
            commands::studio::studio_list_sessions,
            commands::studio::studio_load_session,
            commands::studio::studio_chat_send,
            commands::studio::studio_persist_message,
            commands::studio::studio_restore_checkpoint,
            commands::studio::studio_read_attachment_b64,
            commands::studio::studio_list_image_progress,
            commands::studio::studio_cancel,
            // commands::projects
            commands::projects::projects_list,
            // commands::extensions（v3 §23 改名 Plugin → Extension）
            commands::extensions::extensions_list,
            // commands::tasks（v3 §21.1 新增）
            commands::tasks::tasks_list,
            // commands::skills（v3 §21.2 新增）
            commands::skills::skills_list,
            // commands::shared_mcp（S2 · 2026-05-06 · 共享 MCP 池 → 4 agent reconcile）
            commands::shared_mcp::shared_mcp_get,
            commands::shared_mcp::shared_mcp_save,
            commands::shared_mcp::shared_mcp_apply,
            commands::shared_mcp::shared_mcp_save_and_apply,
            commands::shared_mcp::shared_mcp_agent_ids,
            // commands::agent_versions（S5 · 2026-05-06 · 5 agent 版本 / 更新检测）
            commands::agent_versions::agent_versions_check,
            // commands::shared_skills（S4 · 2026-05-06 · skill 共享池 → 复制到 claude/openclaw）
            commands::shared_skills::shared_skills_list,
            commands::shared_skills::shared_skills_apply,
            commands::shared_skills::shared_skills_open_dir,
            // commands::shared_secrets（S3 · 2026-05-06 · 共享 secrets · ACL · IPC consent）
            commands::shared_secrets::shared_secrets_list_names,
            commands::shared_secrets::shared_secrets_ipc_endpoint,
            commands::shared_secrets::shared_secrets_acl_list,
            commands::shared_secrets::shared_secrets_acl_revoke,
            commands::shared_secrets::shared_secrets_consent_resolve,
            commands::shared_secrets::shared_secrets_open_yaml,
            commands::shared_secrets::shared_secrets_agent_ids,
            // commands::governance · Agent OS P0 governance
            commands::governance::governance_audit_recent,
            commands::governance::governance_snapshots_list,
            commands::governance::governance_snapshot_rollback,
            commands::governance::governance_machine_manifest_export,
            commands::governance::governance_capability_matrix,
            commands::governance::governance_config_ownership,
            commands::governance::governance_interface_registry,
            commands::governance::governance_interface_probe,
        ])
        .build(context)
        .expect("error while building tauri application")
        .run(|_, event| {
            if matches!(event, tauri::RunEvent::Exit) { commands::conversations::shutdown(); }
        });
}
