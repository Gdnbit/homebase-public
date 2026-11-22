//! Protected developer input and original request references. Never runs work.
use crate::development_workspace::{
    DevelopmentDraft, DevelopmentSnapshot, DevelopmentWorkspaceEntry, DevelopmentWorkspaceStore,
};
use serde::Deserialize;
fn store() -> &'static DevelopmentWorkspaceStore {
    static STORE: std::sync::OnceLock<DevelopmentWorkspaceStore> = std::sync::OnceLock::new();
    STORE.get_or_init(|| {
        DevelopmentWorkspaceStore::new(
            crate::adapters::homebase_home_dir()
                .join(".homebase/capabilities/development-workspace"),
        )
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentWorkspaceSaveRequest {
    pub revision: u64,
    pub draft: DevelopmentDraft,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentWorkspacePrepareRequest {
    pub revision: u64,
    pub draft: DevelopmentDraft,
    pub original: DevelopmentSnapshot,
}
/// Load private developer input and original IDs without invoking an executor.
#[tauri::command]
pub async fn development_workspace_get() -> Result<DevelopmentWorkspaceEntry, String> {
    store().get()
}
/// Save all raw editor fields with CAS; invalid JSON is retained for editing.
#[tauri::command]
pub async fn development_workspace_save(
    request: DevelopmentWorkspaceSaveRequest,
) -> Result<DevelopmentWorkspaceEntry, String> {
    store().save(request.revision, request.draft).await
}
/// Protect the immutable original request before any development_run call.
#[tauri::command]
pub async fn development_workspace_prepare(
    request: DevelopmentWorkspacePrepareRequest,
) -> Result<DevelopmentWorkspaceEntry, String> {
    store()
        .prepare(request.revision, request.draft, request.original)
        .await
}
/// Explicitly archive the original reference and unlock the editor; never cancel.
#[tauri::command]
pub async fn development_workspace_new(
    revision: u64,
    request_id: Option<String>,
) -> Result<DevelopmentWorkspaceEntry, String> {
    store().fresh(revision, request_id).await
}
