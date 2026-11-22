//! One encrypted source editor with CAS and an explicit original-intent outbox.
use crate::{
    inbox_document::InboxSubmitPayload,
    inbox_receipts::{InboxSubmissionState, InboxSubmissionView, ReceiptStore},
};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const SCHEMA: u32 = 1;
const MAX_PLAIN: usize = 16 * 1024 * 1024;
const MAX_CIPHER: usize = 32 * 1024 * 1024;
pub fn public_error(error: &str) -> Option<(&'static str, &'static str)> {
    let (code, supplied) = error.split_once(": ").unwrap_or((error, ""));
    let pair = match code {
        "INBOX_WORKSPACE_READ" => (
            "INBOX_WORKSPACE_READ",
            "资料草稿无法完整读取或解密，原文件保留，不能当作空草稿。",
        ),
        "INBOX_WORKSPACE_SAVE" => (
            "INBOX_WORKSPACE_SAVE",
            "资料草稿或原意图尚未确认保存，当前输入保留，不执行收录。",
        ),
        "INBOX_WORKSPACE_CONFLICT" => (
            "INBOX_WORKSPACE_CONFLICT",
            "资料已被另一窗口更新，未覆盖已保存版本；当前输入保留，请核对。",
        ),
        "INBOX_WORKSPACE_PENDING" => (
            "INBOX_WORKSPACE_PENDING",
            "已有原收录需要核对，不能另建编号重复投递；新输入仍可保存。",
        ),
        "INBOX_WORKSPACE_ID" => (
            "INBOX_WORKSPACE_ID",
            "收录编号与原意图不一致，未执行或清理。",
        ),
        "INBOX_WORKSPACE_NOT_READY" => (
            "INBOX_WORKSPACE_NOT_READY",
            "原收录准备尚未确认；请核对或完成原准备，未发布笔记。",
        ),
        "INBOX_WORKSPACE_LIMIT" => (
            "INBOX_WORKSPACE_LIMIT",
            "资料草稿超过安全上限，未截断或覆盖原内容。",
        ),
        "INBOX_WORKSPACE_INVALID" => (
            "INBOX_WORKSPACE_INVALID",
            "资料草稿字段或附件名称无效，原稿保持，未保存或执行。",
        ),
        _ => return None,
    };
    if supplied.is_empty() || supplied == pair.1 {
        Some(pair)
    } else {
        None
    }
}
fn error(code: &str) -> String {
    let (c, m) = public_error(code).unwrap();
    format!("{c}: {m}")
}

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InboxWorkspaceAttachment {
    pub rel: String,
    pub name: String,
    pub size: u64,
}
#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InboxWorkspaceDraft {
    pub text: String,
    pub title: String,
    pub tags_input: String,
    pub source_hint: String,
    pub attachments: Vec<InboxWorkspaceAttachment>,
    pub attachment_unconfirmed: bool,
}
impl InboxWorkspaceDraft {
    fn payload(&self, id: &str) -> InboxSubmitPayload {
        InboxSubmitPayload {
            text: self.text.clone(),
            title: if self.title.trim().is_empty() {
                None
            } else {
                Some(self.title.trim().into())
            },
            extra_tags: self
                .tags_input
                .split(|c: char| c.is_whitespace() || c == ',' || c == '，')
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect(),
            source_hint: if self.source_hint.is_empty() {
                None
            } else {
                Some(self.source_hint.clone())
            },
            attachments: self.attachments.iter().map(|a| a.rel.clone()).collect(),
            submission_id: Some(id.into()),
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self
            .attachments
            .iter()
            .any(|a| a.name.chars().any(char::is_control) || a.rel.chars().any(char::is_control))
        {
            return Err(error("INBOX_WORKSPACE_INVALID"));
        }
        if self.tags_input.len() > 64 * 1024
            || self
                .attachments
                .iter()
                .any(|a| a.name.len() > 1024 || a.size > 9_007_199_254_740_991)
        {
            return Err(error("INBOX_WORKSPACE_LIMIT"));
        }
        crate::inbox_receipts::validate_bounds(&self.payload(""))
    }
    fn settled(&mut self, old: &Self) {
        let unchanged = self == old;
        if self.text == old.text {
            self.text.clear();
        }
        if self.title == old.title {
            self.title.clear();
        }
        if self.tags_input == old.tags_input {
            self.tags_input.clear();
        }
        self.attachments.retain(|a| !old.attachments.contains(a));
        if unchanged {
            self.attachment_unconfirmed = false;
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboxWorkspaceStage {
    Reserved,
    Ready,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InboxWorkspacePending {
    pub submission_id: String,
    pub snapshot: InboxWorkspaceDraft,
    pub stage: InboxWorkspaceStage,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InboxWorkspaceEntry {
    pub schema_version: u32,
    pub revision: u64,
    pub draft: InboxWorkspaceDraft,
    pub pending: Option<InboxWorkspacePending>,
    pub last_submission_id: Option<String>,
}
impl Default for InboxWorkspaceEntry {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA,
            revision: 0,
            draft: Default::default(),
            pending: None,
            last_submission_id: None,
        }
    }
}
impl InboxWorkspaceEntry {
    fn changed(&mut self) -> Result<(), String> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| error("INBOX_WORKSPACE_LIMIT"))?;
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboxWorkspaceObservation {
    NotNeeded,
    NotChecked,
    Loaded,
    Failed,
}
#[derive(Clone, Debug, Serialize)]
pub struct InboxWorkspaceReply {
    pub entry: InboxWorkspaceEntry,
    pub submission: Option<InboxSubmissionView>,
    /// Saves do not reread a prior note. Null submission is not an empty result.
    pub observation: InboxWorkspaceObservation,
    pub observation_error: Option<String>,
}

pub struct WorkspaceStore {
    directory: PathBuf,
    gate: Mutex<()>,
    #[cfg(test)]
    fail_publish: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_ready_publish: std::sync::atomic::AtomicBool,
}
impl WorkspaceStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            gate: Mutex::new(()),
            #[cfg(test)]
            fail_publish: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_ready_publish: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn path(&self) -> PathBuf {
        self.directory.join("source.workspace.dpapi")
    }
    fn load(&self) -> Result<InboxWorkspaceEntry, String> {
        let Some(cipher) = crate::assistant_state_store::read_bounded(&self.path(), MAX_CIPHER)
            .map_err(|_| error("INBOX_WORKSPACE_READ"))?
        else {
            return Ok(Default::default());
        };
        let plain = crate::wechat_crypto::crypt(&cipher, false)
            .map_err(|_| error("INBOX_WORKSPACE_READ"))?;
        if plain.len() > MAX_PLAIN {
            return Err(error("INBOX_WORKSPACE_LIMIT"));
        }
        let e: InboxWorkspaceEntry =
            serde_json::from_slice(&plain).map_err(|_| error("INBOX_WORKSPACE_READ"))?;
        if e.schema_version != SCHEMA
            || e.revision == 0
            || e.last_submission_id
                .as_ref()
                .is_some_and(|id| !crate::inbox_receipts::valid_id(id))
        {
            return Err(error("INBOX_WORKSPACE_READ"));
        }
        e.draft.validate()?;
        if let Some(p) = &e.pending {
            p.snapshot.validate()?;
            crate::inbox_receipts::validate_payload(&p.snapshot.payload(&p.submission_id))?;
        }
        Ok(e)
    }
    async fn lock(&self) -> Result<fs::File, String> {
        crate::assistant_state_store::safe(&self.directory)
            .map_err(|_| error("INBOX_WORKSPACE_SAVE"))?;
        crate::legacy_isolation::ensure_test_path(&self.directory)?;
        crate::control_protocol::private_directory(&self.directory)
            .await
            .map_err(|_| error("INBOX_WORKSPACE_SAVE"))?;
        let path = self.directory.join("source.writer.lock");
        crate::assistant_state_store::safe(&path).map_err(|_| error("INBOX_WORKSPACE_SAVE"))?;
        let file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .map_err(|_| error("INBOX_WORKSPACE_SAVE"))?;
        crate::assistant_state_store::ordinary(&file).map_err(|_| error("INBOX_WORKSPACE_SAVE"))?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| error("INBOX_WORKSPACE_CONFLICT"))?;
        Ok(file)
    }
    async fn publish(&self, e: &InboxWorkspaceEntry) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_publish.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(error("INBOX_WORKSPACE_SAVE"));
        }
        #[cfg(test)]
        if self
            .fail_ready_publish
            .load(std::sync::atomic::Ordering::SeqCst)
            && e.pending
                .as_ref()
                .is_some_and(|p| p.stage == InboxWorkspaceStage::Ready)
        {
            return Err(error("INBOX_WORKSPACE_SAVE"));
        }
        let plain =
            Zeroizing::new(serde_json::to_vec(e).map_err(|_| error("INBOX_WORKSPACE_SAVE"))?);
        if plain.len() > MAX_PLAIN {
            return Err(error("INBOX_WORKSPACE_LIMIT"));
        }
        let cipher =
            crate::wechat_crypto::crypt(&plain, true).map_err(|_| error("INBOX_WORKSPACE_SAVE"))?;
        crate::control_protocol::publish_private(&self.path(), &cipher)
            .await
            .map_err(|_| error("INBOX_WORKSPACE_SAVE"))
    }
    fn reply(
        &self,
        e: InboxWorkspaceEntry,
        receipts: &ReceiptStore,
    ) -> Result<InboxWorkspaceReply, String> {
        let id = e
            .pending
            .as_ref()
            .map(|p| p.submission_id.as_str())
            .or(e.last_submission_id.as_deref());
        let (observation, submission, observation_error) = match id {
            None => (InboxWorkspaceObservation::NotNeeded, None, None),
            Some(id) => match e.pending.as_ref().map_or_else(
                || receipts.get(id),
                |p| receipts.get_for(id, &p.snapshot.payload(id)),
            ) {
                Ok(v) => (InboxWorkspaceObservation::Loaded, Some(v), None),
                Err(e) => (InboxWorkspaceObservation::Failed, None, Some(e)),
            },
        };
        Ok(InboxWorkspaceReply {
            entry: e,
            submission,
            observation,
            observation_error,
        })
    }
    fn saved(e: InboxWorkspaceEntry) -> InboxWorkspaceReply {
        let observation = if e.pending.is_some() || e.last_submission_id.is_some() {
            InboxWorkspaceObservation::NotChecked
        } else {
            InboxWorkspaceObservation::NotNeeded
        };
        InboxWorkspaceReply {
            entry: e,
            submission: None,
            observation,
            observation_error: None,
        }
    }
    pub fn get(&self, receipts: &ReceiptStore) -> Result<InboxWorkspaceReply, String> {
        self.reply(self.load()?, receipts)
    }
    pub async fn save(
        &self,
        revision: u64,
        draft: InboxWorkspaceDraft,
    ) -> Result<InboxWorkspaceReply, String> {
        draft.validate()?;
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut e = self.load()?;
        if e.draft == draft {
            return Ok(Self::saved(e));
        }
        if e.revision != revision {
            return Err(error("INBOX_WORKSPACE_CONFLICT"));
        }
        e.draft = draft;
        e.changed()?;
        self.publish(&e).await?;
        Ok(Self::saved(e))
    }
    pub async fn prepare(
        &self,
        receipts: &ReceiptStore,
        revision: u64,
        id: &str,
        snapshot: InboxWorkspaceDraft,
    ) -> Result<InboxWorkspaceReply, String> {
        snapshot.validate()?;
        if !crate::inbox_receipts::valid_id(id) {
            return Err(error("INBOX_WORKSPACE_ID"));
        }
        crate::inbox_receipts::validate_payload(&snapshot.payload(id))?;
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut e = self.load()?;
        if let Some(p) = &e.pending {
            if p.submission_id != id {
                return Err(error("INBOX_WORKSPACE_PENDING"));
            }
            if p.snapshot != snapshot {
                return Err(error("INBOX_WORKSPACE_ID"));
            }
            if p.stage == InboxWorkspaceStage::Ready {
                return self.reply(e, receipts);
            }
        } else {
            if e.revision != revision {
                return Err(error("INBOX_WORKSPACE_CONFLICT"));
            }
            receipts.get_for(id, &snapshot.payload(id))?;
            // Persist the original source and identity before preparing a receipt.
            // A reserved source alone cannot authorize publication.
            e.draft = snapshot.clone();
            e.pending = Some(InboxWorkspacePending {
                submission_id: id.into(),
                snapshot: snapshot.clone(),
                stage: InboxWorkspaceStage::Reserved,
            });
            e.changed()?;
            self.publish(&e).await?;
        }
        let view = receipts.prepare(id, snapshot.payload(id)).await?;
        e.pending
            .as_mut()
            .ok_or_else(|| error("INBOX_WORKSPACE_ID"))?
            .stage = InboxWorkspaceStage::Ready;
        e.changed()?;
        self.publish(&e).await?;
        Ok(InboxWorkspaceReply {
            entry: e,
            submission: Some(view),
            observation: InboxWorkspaceObservation::Loaded,
            observation_error: None,
        })
    }
    async fn settle(
        &self,
        mut e: InboxWorkspaceEntry,
        view: &InboxSubmissionView,
    ) -> Result<InboxWorkspaceReply, String> {
        if matches!(
            view.state,
            InboxSubmissionState::Completed | InboxSubmissionState::Canceled
        ) {
            let pending = e
                .pending
                .take()
                .ok_or_else(|| error("INBOX_WORKSPACE_ID"))?;
            if pending.submission_id != view.submission_id {
                return Err(error("INBOX_WORKSPACE_ID"));
            }
            if view.state == InboxSubmissionState::Completed {
                e.draft.settled(&pending.snapshot);
            }
            e.last_submission_id = Some(view.submission_id.clone());
            e.changed()?;
            self.publish(&e).await?;
        }
        Ok(InboxWorkspaceReply {
            entry: e,
            submission: Some(view.clone()),
            observation: InboxWorkspaceObservation::Loaded,
            observation_error: None,
        })
    }
    pub async fn submit(
        &self,
        receipts: &ReceiptStore,
        id: &str,
    ) -> Result<InboxWorkspaceReply, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let e = self.load()?;
        let p = e
            .pending
            .as_ref()
            .filter(|p| p.submission_id == id)
            .ok_or_else(|| error("INBOX_WORKSPACE_ID"))?;
        if p.stage != InboxWorkspaceStage::Ready {
            return Err(error("INBOX_WORKSPACE_NOT_READY"));
        }
        let view = receipts.commit_for(id, &p.snapshot.payload(id)).await?;
        self.settle(e, &view).await
    }
    pub async fn resolve(
        &self,
        receipts: &ReceiptStore,
        id: &str,
    ) -> Result<InboxWorkspaceReply, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let e = self.load()?;
        if e.pending.is_none() && e.last_submission_id.as_deref() == Some(id) {
            return self.reply(e, receipts);
        }
        let p = e
            .pending
            .as_ref()
            .filter(|p| p.submission_id == id)
            .ok_or_else(|| error("INBOX_WORKSPACE_ID"))?;
        let observed = receipts.get_for(id, &p.snapshot.payload(id))?;
        if observed.state == InboxSubmissionState::NotFound {
            return self.reply(e, receipts);
        }
        let view = receipts.resolve_for(id, &p.snapshot.payload(id)).await?;
        self.settle(e, &view).await
    }
    /// Cancel only an unpublished original. A reserved pointer with no matching
    /// receipt may be dropped without touching an unrelated business receipt.
    pub async fn cancel(
        &self,
        receipts: &ReceiptStore,
        id: &str,
    ) -> Result<InboxWorkspaceReply, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut e = self.load()?;
        let p = e
            .pending
            .as_ref()
            .filter(|p| p.submission_id == id)
            .ok_or_else(|| error("INBOX_WORKSPACE_ID"))?;
        let payload = p.snapshot.payload(id);
        let observed = receipts.get_for(id, &payload);
        let unmatched = matches!(&observed,Ok(v) if v.state==InboxSubmissionState::NotFound)
            || matches!(&observed,Err(e) if e.starts_with("INBOX_SUBMISSION_CONFLICT:"));
        if p.stage == InboxWorkspaceStage::Reserved && unmatched {
            e.pending = None;
            e.changed()?;
            self.publish(&e).await?;
            return Ok(Self::saved(e));
        }
        observed?;
        let view = receipts.cancel_for(id, &payload).await?;
        self.settle(e, &view).await
    }
}

#[cfg(test)]
#[path = "inbox_workspace_tests.rs"]
mod tests;
