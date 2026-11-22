//! Frozen native send identity. No SDK, account, or execution state is stored here.
use super::{DesktopEntry, DesktopStore};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeMessage {
    pub provider: String,
    pub thread_id: String,
    pub text: String,
    pub request_id: String,
}
impl NativeMessage {
    pub fn scope(&self) -> String {
        format!("native:{}:{}", self.provider, self.thread_id)
    }
    pub fn validate(&self) -> Result<(), String> {
        DesktopStore::validate_scope(&self.scope())?;
        if self.text.trim().is_empty() || self.text.len() > 8000 {
            return Err("原生消息须为1–8000字节；长草稿保留，请缩短或分段发送".into());
        }
        if self.request_id.is_empty()
            || self.request_id.len() > 120
            || self.request_id.chars().any(char::is_control)
        {
            return Err("原生发送需要稳定且有效的request_id".into());
        }
        Ok(())
    }
}

impl DesktopStore {
    pub async fn native_prepare(
        &self,
        revision: u64,
        request: NativeMessage,
    ) -> Result<DesktopEntry, String> {
        request.validate()?;
        let _gate = self.gate.lock().await;
        let _lock = self.lock()?;
        let mut entry = self.get(&request.scope())?;
        if entry.pending.is_some() {
            return Err("原生命名空间存在不相容的内置请求，原文件保留".into());
        }
        if let Some(old) = &entry.native_pending {
            if old == &request {
                return Ok(entry);
            }
            return Err("原生请求尚待核对，不能更换编号、Agent、会话或正文；请先查询原请求".into());
        }
        if entry.revision != revision {
            return Err("原生输入已被另一窗口更新，未执行发送".into());
        }
        // Fail explicitly rather than silently dropping unsupported input fields.
        if !entry.body.files.is_empty() || !entry.body.route.is_empty() || entry.body.mode != "auto"
        {
            return Err("原生发送当前只支持文字；未忽略或发送未适配的附件/模型选择".into());
        }
        entry.body.text = request.text.clone();
        entry.native_pending = Some(request);
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }

    /// Only call after the original provider durably acknowledged this request.
    /// Acknowledgement is acceptance, not proof the native turn completed.
    pub async fn native_ack(&self, request: &NativeMessage) -> Result<DesktopEntry, String> {
        request.validate()?;
        let _gate = self.gate.lock().await;
        let _lock = self.lock()?;
        let mut entry = self.get(&request.scope())?;
        let Some(old) = &entry.native_pending else {
            return Ok(entry);
        };
        if old != request {
            return Err("已找到原ACK，但当前正在核对另一条请求，未清理其回执或新稿".into());
        }
        if entry.body.text == request.text {
            entry.body.text.clear();
        }
        entry.native_pending = None;
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::assistant_desktop::DesktopBody;
    fn request(provider: &str) -> NativeMessage {
        NativeMessage {
            provider: provider.into(),
            thread_id: "opaque_thread_01".into(),
            text: "只处理这条合成消息".into(),
            request_id: "stable-original-01".into(),
        }
    }
    #[tokio::test]
    async fn native_frozen_request_survives_restart_and_never_replaces_unknown_identity() {
        let root = tempfile::tempdir().unwrap();
        for provider in ["codex", "claude-code", "gemini-cli"] {
            let store = DesktopStore::new(root.path().to_owned());
            let req = request(provider);
            let saved = store.native_prepare(0, req.clone()).await.unwrap();
            let restarted = DesktopStore::new(root.path().to_owned());
            assert!(restarted.get(&req.scope()).unwrap().native_pending.as_ref() == Some(&req));
            assert_eq!(
                restarted
                    .native_prepare(0, req.clone())
                    .await
                    .unwrap()
                    .revision,
                saved.revision
            );
            let mut other = req.clone();
            other.request_id.push_str("-new");
            assert!(restarted
                .native_prepare(saved.revision, other)
                .await
                .is_err());
            let mut changed = req.clone();
            changed.text.push_str("改动");
            assert!(restarted
                .native_prepare(saved.revision, changed)
                .await
                .is_err());
            let meta = restarted
                .index()
                .unwrap()
                .entries
                .into_iter()
                .find(|e| e.scope == req.scope())
                .unwrap();
            assert_eq!(meta.pending_state.as_deref(), Some("prepared"));
        }
    }
    #[tokio::test]
    async fn native_prepare_failure_and_stale_revision_do_not_publish_intent() {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().to_owned());
        let req = request("codex");
        let entry = store
            .save(
                &req.scope(),
                0,
                DesktopBody {
                    image: None,
                    text: "保留原稿".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let before = std::fs::read(store.path(&req.scope()).unwrap()).unwrap();
        assert!(store.native_prepare(0, req.clone()).await.is_err());
        store.fail_writes(true);
        assert!(store
            .native_prepare(entry.revision, req.clone())
            .await
            .is_err());
        assert!(store.get(&req.scope()).unwrap().native_pending.is_none());
        assert_eq!(
            std::fs::read(store.path(&req.scope()).unwrap()).unwrap(),
            before
        );
        store.fail_writes(false);
        let mut too_long = req;
        too_long.text = "中文".repeat(2000);
        assert!(store
            .native_prepare(entry.revision, too_long)
            .await
            .is_err());
        assert_eq!(
            std::fs::read(store.path("native:codex:opaque_thread_01").unwrap()).unwrap(),
            before
        );
    }
    #[tokio::test]
    async fn native_ack_atomic_cleanup_preserves_newer_body_and_other_pending_request() {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().to_owned());
        let req = request("claude-code");
        let prepared = store.native_prepare(0, req.clone()).await.unwrap();
        let newer = "下一条未发送输入";
        store
            .save(
                &req.scope(),
                prepared.revision,
                DesktopBody {
                    image: None,
                    text: newer.into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        store.fail_writes(true);
        assert!(store.native_ack(&req).await.is_err());
        assert!(store.get(&req.scope()).unwrap().native_pending.is_some());
        store.fail_writes(false);
        let accepted = store.native_ack(&req).await.unwrap();
        assert_eq!(accepted.body.text, newer);
        assert!(accepted.native_pending.is_none());
        assert_eq!(
            store.native_ack(&req).await.unwrap().revision,
            accepted.revision
        );
        let mut next = req.clone();
        next.text = newer.into();
        next.request_id = "next-original".into();
        store
            .native_prepare(accepted.revision, next.clone())
            .await
            .unwrap();
        assert!(store.native_ack(&req).await.is_err());
        assert!(store.get(&req.scope()).unwrap().native_pending.as_ref() == Some(&next));
        let cleared = store.native_ack(&next).await.unwrap();
        assert!(cleared.body.text.is_empty());
    }
    #[tokio::test]
    async fn native_scope_mismatch_fails_closed_and_pre_receipt_format_remains_readable() {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().to_owned());
        let req = request("codex");
        let entry = store.native_prepare(0, req.clone()).await.unwrap();
        let path = store.path(&req.scope()).unwrap();
        let mut value = serde_json::to_value(&entry).unwrap();
        value["native_pending"]["provider"] = serde_json::json!("claude-code");
        let invalid =
            crate::wechat_crypto::crypt(&serde_json::to_vec(&value).unwrap(), true).unwrap();
        std::fs::write(&path, &invalid).unwrap();
        assert!(store.get(&req.scope()).is_err());
        assert_eq!(store.index().unwrap().unreadable, 1);
        assert_eq!(std::fs::read(&path).unwrap().as_slice(), invalid.as_slice());
        let mut legacy = serde_json::to_value(&entry).unwrap();
        legacy.as_object_mut().unwrap().remove("native_pending");
        let old = crate::wechat_crypto::crypt(&serde_json::to_vec(&legacy).unwrap(), true).unwrap();
        std::fs::write(&path, &old).unwrap();
        let loaded = store.get(&req.scope()).unwrap();
        assert_eq!(loaded.body.text, req.text);
        assert!(loaded.native_pending.is_none());
        assert_eq!(std::fs::read(&path).unwrap().as_slice(), old.as_slice());
    }
}
