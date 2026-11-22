#[path = "../../../src-tauri/src/control_protocol.rs"]
pub mod control_protocol;
#[path = "../../../src-tauri/src/control_state.rs"]
pub mod control_state;
#[path = "../../../src-tauri/src/control_bootstrap.rs"]
pub mod control_bootstrap;
#[path = "../../../src-tauri/src/query.rs"]
pub mod query;
#[path = "../../../src-tauri/src/distribution_status.rs"]
pub mod distribution_status;
#[path = "../../../src-tauri/src/task_event_store.rs"]
pub mod task_event_store;
#[path = "../../../src-tauri/src/inbox_storage.rs"]
pub mod inbox_storage;
#[path = "../../../src-tauri/src/inbox_document.rs"]
pub mod inbox_document;
#[path = "../../../src-tauri/src/inbox_receipts.rs"]
pub mod inbox_receipts;
#[path = "../../../src-tauri/src/inbox_workspace.rs"]
pub mod inbox_workspace;
#[path = "../../../src-tauri/src/development_workspace.rs"]
pub mod development_workspace;
#[path = "../../../src-tauri/src/legacy_isolation.rs"]
pub mod legacy_isolation;
#[path = "../../../src-tauri/src/model_capabilities.rs"]
pub mod model_capabilities;
#[path = "../../../src-tauri/src/model_capability_io.rs"]
pub mod model_capability_io;
#[path = "../../../src-tauri/src/wechat.rs"]
pub mod wechat;
#[path = "../../../src-tauri/src/wechat_crypto.rs"]
pub mod wechat_crypto;
#[path = "../../../src-tauri/src/assistant.rs"]
pub mod assistant;
#[path = "../../../src-tauri/src/assistant_state_store.rs"]
pub mod assistant_state_store;

/// In-process fixtures with a format-1 save callback. Production uses the
/// commit adapter directly so a segmented projection can be returned.
pub mod test_support {
    use crate::assistant::{archive, Commit, Control, Engine, Model, State};
    use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

    pub type Save = Arc<dyn Fn(State) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

    pub async fn legacy_engine(
        state: State,
        save: Save,
        model: Model,
        control: Control,
        workspace: PathBuf,
    ) -> Result<Arc<Engine>, String> {
        archive::ensure_legacy(&state)?;
        let commit: Commit = Arc::new(move |state, _| {
            let save = save.clone();
            Box::pin(async move {
                archive::ensure_legacy(&state)?;
                save(state.clone()).await?;
                Ok(state)
            })
        });
        let documents = Arc::new(crate::assistant_documents::DocumentStore::new(workspace.join(".documents")));
        Engine::new_with_commit(state, commit, model, control, workspace, documents).await
    }
}
#[path = "../../../src-tauri/src/assistant_history.rs"]
pub mod assistant_history;
#[path = "../../../src-tauri/src/capability_discovery.rs"]
pub mod capability_discovery;
#[path = "../../../src-tauri/src/capability_catalog_projection.rs"]
pub mod capability_catalog_projection;
#[path = "../../../src-tauri/src/assistant_input_presentation.rs"]
pub mod assistant_input_presentation;
#[path = "../../../src-tauri/src/capability_facts.rs"]
pub mod capability_facts;
#[path = "../../../src-tauri/src/assistant_desktop.rs"]
pub mod assistant_desktop;
#[path = "../../../src-tauri/src/native_presentation.rs"]
pub mod native_presentation;
#[path = "../../../src-tauri/src/document_runtime.rs"]
pub mod document_runtime;
#[cfg(windows)]
#[path = "../../../src-tauri/src/process_lifetime.rs"]
mod process_lifetime;
#[path = "../../../src-tauri/src/assistant_documents.rs"]
pub mod assistant_documents;
#[path = "../../../src-tauri/src/document_service.rs"]
pub mod document_service;
#[path = "../../../src-tauri/src/assistant_protocol.rs"]
pub mod assistant_protocol;
#[path = "../../../src-tauri/src/assistant_runtime.rs"]
pub mod assistant_runtime;
#[path = "../../../src-tauri/src/assistant_local_host.rs"]
pub mod assistant_local_host;
#[path = "../../../src-tauri/src/assistant_local.rs"]
pub mod assistant_local;
#[path = "../../../src-tauri/src/assistant_personal.rs"]
pub mod assistant_personal;
#[path = "../../../src-tauri/src/native_agent.rs"]
pub mod native_agent;
