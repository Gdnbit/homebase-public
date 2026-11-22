//! Protected source input. No command executes work merely by reading a draft.
use crate::inbox_workspace::{InboxWorkspaceDraft, InboxWorkspaceReply, WorkspaceStore};
use serde::Deserialize;

fn store() -> &'static WorkspaceStore {
    static STORE: std::sync::OnceLock<WorkspaceStore> = std::sync::OnceLock::new();
    STORE.get_or_init(|| {
        WorkspaceStore::new(crate::adapters::homebase_home_dir().join(".homebase/inbox"))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxWorkspaceSaveRequest {
    pub revision: u64,
    pub draft: InboxWorkspaceDraft,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InboxWorkspacePrepareRequest {
    pub revision: u64,
    pub submission_id: String,
    pub snapshot: InboxWorkspaceDraft,
}

/// Load source input and observe the original intent; never publish or resume.
#[tauri::command]
pub async fn inbox_workspace_get() -> Result<InboxWorkspaceReply, String> {
    store().get(super::inbox::submission_store())
}
/// CAS-save source fields. Does not query prior note contents or execute work.
#[tauri::command]
pub async fn inbox_workspace_save(
    request: InboxWorkspaceSaveRequest,
) -> Result<InboxWorkspaceReply, String> {
    store().save(request.revision, request.draft).await
}
/// Persist an immutable original snapshot before any business publication.
#[tauri::command]
pub async fn inbox_workspace_prepare(
    request: InboxWorkspacePrepareRequest,
) -> Result<InboxWorkspaceReply, String> {
    store()
        .prepare(
            super::inbox::submission_store(),
            request.revision,
            &request.submission_id,
            request.snapshot,
        )
        .await
}
/// Explicitly submit only the already protected original intent.
#[tauri::command]
pub async fn inbox_workspace_submit(submission_id: String) -> Result<InboxWorkspaceReply, String> {
    store()
        .submit(super::inbox::submission_store(), &submission_id)
        .await
}
/// Settle an observed original result without publishing a prepared operation.
#[tauri::command]
pub async fn inbox_workspace_resolve(submission_id: String) -> Result<InboxWorkspaceReply, String> {
    store()
        .resolve(super::inbox::submission_store(), &submission_id)
        .await
}
/// Cancel an unpublished original reservation without deleting notes or input.
#[tauri::command]
pub async fn inbox_workspace_cancel(submission_id: String) -> Result<InboxWorkspaceReply, String> {
    store()
        .cancel(super::inbox::submission_store(), &submission_id)
        .await
}
