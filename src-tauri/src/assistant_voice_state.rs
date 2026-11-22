//! Durable voice receipts share the same encrypted, revisioned client workspace.
use super::{DesktopEntry, DesktopStore};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoicePhase {
    Staged,
    Submitting,
    Accepted,
    Ready,
    Applied,
    CancelRequested,
    Cancelled,
    Failed,
    Unknown,
    Dismissed,
}
impl VoicePhase {
    pub fn active(self) -> bool {
        !matches!(
            self,
            Self::Applied | Self::Dismissed | Self::Cancelled | Self::Failed
        )
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceJob {
    pub job_id: String,
    pub task_dir: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceRecord {
    pub recording_id: String,
    pub audio_sha256: String,
    pub phase: VoicePhase,
    pub job: Option<VoiceJob>,
    pub transcript: Option<String>,
    pub message: String,
}
pub(super) fn validate(voices: &[VoiceRecord]) -> Result<(), String> {
    if voices.len() > 512 {
        return Err("录音回执达到维护上限，旧记录未删除".into());
    }
    let mut ids = std::collections::HashSet::new();
    for voice in voices {
        if !uuid::Uuid::parse_str(&voice.recording_id)
            .is_ok_and(|id| id.to_string() == voice.recording_id)
            || !ids.insert(&voice.recording_id)
            || voice.audio_sha256.len() != 64
            || !voice.audio_sha256.bytes().all(|c| c.is_ascii_hexdigit())
            || voice.message.len() > 2000
            || voice
                .transcript
                .as_ref()
                .is_some_and(|t| t.is_empty() || t.len() > 8000)
            || (voice.phase == VoicePhase::Ready && voice.transcript.is_none())
            || voice.job.as_ref().is_some_and(|j| {
                uuid::Uuid::parse_str(&j.job_id).is_err()
                    || j.task_dir.is_empty()
                    || j.task_dir.len() > 4096
            })
        {
            return Err("录音回执格式无效，原状态未改动".into());
        }
    }
    Ok(())
}
impl DesktopStore {
    pub async fn voice_begin(
        &self,
        scope: &str,
        recording_id: String,
        audio_sha256: String,
    ) -> Result<DesktopEntry, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock()?;
        let mut entry = self.get(scope)?;
        if entry.voices.iter().any(|v| v.phase.active()) {
            return Err("已有待核对录音，请先查看或明确停止等待；原回执保留".into());
        }
        entry.voices.push(VoiceRecord {
            recording_id,
            audio_sha256,
            phase: VoicePhase::Staged,
            job: None,
            transcript: None,
            message: String::new(),
        });
        validate(&entry.voices)?;
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }
    /// true is the single durable transition allowed to start ASR. All repeats only query.
    pub async fn voice_prepare(&self, scope: &str, id: &str) -> Result<bool, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock()?;
        let mut entry = self.get(scope)?;
        let voice = entry
            .voices
            .iter_mut()
            .find(|v| v.recording_id == id)
            .ok_or("录音回执不存在")?;
        if voice.phase != VoicePhase::Staged {
            return Ok(false);
        }
        voice.phase = VoicePhase::Submitting;
        voice.message.clear();
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(true)
    }
    pub async fn voice_update(
        &self,
        scope: &str,
        id: &str,
        job: Option<VoiceJob>,
        phase: VoicePhase,
        transcript: Option<String>,
        message: String,
    ) -> Result<DesktopEntry, String> {
        self.change_voice(scope, id, |voice| {
            if let Some(job) = job {
                if voice.job.as_ref().is_some_and(|old| old != &job) {
                    return Err("原录音已关联另一任务，未替换回执".into());
                }
                voice.job = Some(job);
            }
            // Observations may complete out of order. Retain terminal evidence
            // and never allow a status reply to reopen the submission boundary.
            let can_update = match voice.phase {
                VoicePhase::Applied | VoicePhase::Dismissed => false,
                VoicePhase::Ready | VoicePhase::Cancelled | VoicePhase::Failed => {
                    phase == VoicePhase::Dismissed
                }
                VoicePhase::CancelRequested => matches!(
                    phase,
                    VoicePhase::Cancelled
                        | VoicePhase::Failed
                        | VoicePhase::Dismissed
                        | VoicePhase::Ready
                ),
                current => phase != VoicePhase::Staged || current == VoicePhase::Staged,
            };
            if can_update {
                voice.phase = phase;
                voice.transcript = transcript;
                voice.message = message.chars().take(400).collect();
            }
            Ok(())
        })
        .await
    }

    pub async fn voice_submit_failure(
        &self,
        scope: &str,
        id: &str,
        message: String,
    ) -> Result<DesktopEntry, String> {
        self.change_voice(scope, id, |voice| {
            match voice.phase {
                VoicePhase::Staged => {
                    voice.message =
                        format!("本地转写尚未启动，录音已保存，可检查环境后重试：{message}")
                            .chars()
                            .take(400)
                            .collect();
                }
                VoicePhase::Submitting => {
                    voice.phase = VoicePhase::Unknown;
                    voice.message =
                        "转写受理未确认，原录音和提交意图已保存；只查询原任务，未重复提交".into();
                }
                _ => {}
            }
            Ok(())
        })
        .await
    }

    pub async fn voice_request_cancel(
        &self,
        scope: &str,
        id: &str,
    ) -> Result<DesktopEntry, String> {
        self.change_voice(scope, id, |voice| {
            let (phase, message) = match voice.phase {
                VoicePhase::Staged => (VoicePhase::Cancelled, "已取消本次语音输入"),
                VoicePhase::Ready => (
                    VoicePhase::Cancelled,
                    "转写已经完成；已取消插入草稿，原结果保留",
                ),
                VoicePhase::Submitting
                | VoicePhase::Accepted
                | VoicePhase::Unknown
                | VoicePhase::CancelRequested => (
                    VoicePhase::CancelRequested,
                    "已停止等待，取消尚未确认；原回执保留",
                ),
                VoicePhase::Applied
                | VoicePhase::Dismissed
                | VoicePhase::Cancelled
                | VoicePhase::Failed => return Ok(()),
            };
            voice.phase = phase;
            voice.transcript = None;
            voice.message = message.into();
            Ok(())
        })
        .await
    }

    async fn change_voice(
        &self,
        scope: &str,
        id: &str,
        change: impl FnOnce(&mut VoiceRecord) -> Result<(), String>,
    ) -> Result<DesktopEntry, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock()?;
        let mut entry = self.get(scope)?;
        let voice = entry
            .voices
            .iter_mut()
            .find(|v| v.recording_id == id)
            .ok_or("录音回执不存在")?;
        let before = voice.clone();
        change(voice)?;
        if before == *voice {
            return Ok(entry);
        }
        validate(&entry.voices)?;
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }
    pub async fn voice_apply(
        &self,
        scope: &str,
        id: &str,
        revision: u64,
    ) -> Result<DesktopEntry, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock()?;
        let mut entry = self.get(scope)?;
        let voice = entry
            .voices
            .iter_mut()
            .find(|v| v.recording_id == id)
            .ok_or("录音回执不存在")?;
        if voice.phase == VoicePhase::Applied {
            return Ok(entry);
        }
        if entry.revision != revision {
            return Err("输入已被另一窗口更新，请核对新稿后再插入，转写保留".into());
        }
        if voice.phase != VoicePhase::Ready {
            return Err("转写尚未完整就绪，未插入草稿".into());
        }
        let text = voice.transcript.as_ref().ok_or("转写正文不存在")?;
        if entry.body.text.len() + usize::from(!entry.body.text.is_empty()) + text.len()
            > super::MAX_DRAFT_BYTES
        {
            return Err(
                "当前草稿加上转写超过64 KiB，请先缩短草稿；转写仍保留，未截断或插入".into(),
            );
        }
        if !entry.body.text.is_empty() {
            entry.body.text.push('\n');
        }
        entry.body.text.push_str(text);
        voice.phase = VoicePhase::Applied;
        voice.transcript = None;
        voice.message.clear();
        entry.body.validate(&entry.scope)?;
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }
    pub async fn voice_dismiss(&self, scope: &str, id: &str) -> Result<DesktopEntry, String> {
        self.voice_update(
            scope,
            id,
            None,
            VoicePhase::Dismissed,
            None,
            "已停止显示；不代表原任务取消，回执仍保留".into(),
        )
        .await
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, DesktopStore, String) {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().join("desktop"));
        (root, store, uuid::Uuid::new_v4().to_string())
    }
    #[tokio::test]
    async fn voice_stale_observations_keep_the_first_persisted_result() {
        let (root, store, id) = setup();
        store
            .voice_begin("new", id.clone(), "a".repeat(64))
            .await
            .unwrap();
        assert!(store.voice_prepare("new", &id).await.unwrap());
        let ready = store
            .voice_update(
                "new",
                &id,
                None,
                VoicePhase::Ready,
                Some("原转写结果".into()),
                String::new(),
            )
            .await
            .unwrap();
        let restarted = DesktopStore::new(root.path().join("desktop"));
        for phase in [
            VoicePhase::Unknown,
            VoicePhase::Accepted,
            VoicePhase::Failed,
            VoicePhase::Cancelled,
            VoicePhase::Ready,
        ] {
            let updated = restarted
                .voice_update(
                    "new",
                    &id,
                    None,
                    phase,
                    (phase == VoicePhase::Ready).then(|| "另一条迟到结果".into()),
                    "迟到的观察".into(),
                )
                .await
                .unwrap();
            assert_eq!(
                updated.revision, ready.revision,
                "late {phase:?} changed the draft"
            );
            assert_eq!(updated.voices[0].phase, VoicePhase::Ready);
            assert_eq!(updated.voices[0].transcript.as_deref(), Some("原转写结果"));
        }
        let applied = restarted
            .voice_apply("new", &id, ready.revision)
            .await
            .unwrap();
        assert_eq!(applied.body.text, "原转写结果");
    }
    #[tokio::test]
    async fn voice_stale_observations_cannot_reopen_terminal_failure_or_cancel() {
        for terminal in [VoicePhase::Cancelled, VoicePhase::Failed] {
            let (_root, store, id) = setup();
            store
                .voice_begin("new", id.clone(), "b".repeat(64))
                .await
                .unwrap();
            let finished = store
                .voice_update("new", &id, None, terminal, None, "确认结束".into())
                .await
                .unwrap();
            let late = store
                .voice_update(
                    "new",
                    &id,
                    None,
                    VoicePhase::Ready,
                    Some("迟到的文稿".into()),
                    String::new(),
                )
                .await
                .unwrap();
            assert_eq!(late.revision, finished.revision);
            assert_eq!(late.voices[0].phase, terminal);
            assert_eq!(late.voices[0].message, "确认结束");
            assert!(late.voices[0].transcript.is_none());
            assert_eq!(
                store.voice_dismiss("new", &id).await.unwrap().voices[0].phase,
                VoicePhase::Dismissed
            );
        }
    }
    #[tokio::test]
    async fn voice_stale_preflight_cannot_reopen_the_submission_boundary() {
        let (_root, store, id) = setup();
        store
            .voice_begin("new", id.clone(), "c".repeat(64))
            .await
            .unwrap();
        assert!(store.voice_prepare("new", &id).await.unwrap());
        let prepared = store.get("new").unwrap();
        let stale = store
            .voice_update(
                "new",
                &id,
                None,
                VoicePhase::Staged,
                None,
                "迟到的预检错误".into(),
            )
            .await
            .unwrap();
        assert_eq!(stale.revision, prepared.revision);
        assert_eq!(stale.voices[0].phase, VoicePhase::Submitting);
        assert!(!store.voice_prepare("new", &id).await.unwrap());
    }
    #[tokio::test]
    async fn voice_concurrent_prepare_and_cancel_keep_one_original_submission() {
        let (_root, store, id) = setup();
        store
            .voice_begin("new", id.clone(), "d".repeat(64))
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            store.voice_prepare("new", &id),
            store.voice_prepare("new", &id)
        );
        assert_ne!(a.unwrap(), b.unwrap());
        let cancelled = store.voice_request_cancel("new", &id).await.unwrap();
        let late = store
            .voice_update("new", &id, None, VoicePhase::Accepted, None, String::new())
            .await
            .unwrap();
        assert_eq!(late.voices[0].phase, VoicePhase::CancelRequested);
        assert_eq!(late.voices[0].message, cancelled.voices[0].message);
        assert!(!store.voice_prepare("new", &id).await.unwrap());
        assert!(late.revision >= cancelled.revision);
        let ready = store
            .voice_update(
                "new",
                &id,
                None,
                VoicePhase::Ready,
                Some("已完成但未插入".into()),
                String::new(),
            )
            .await
            .unwrap();
        assert_eq!(ready.voices[0].phase, VoicePhase::Ready);
        let closed = store.voice_request_cancel("new", &id).await.unwrap();
        assert_eq!(closed.voices[0].phase, VoicePhase::Cancelled);
        assert!(closed.voices[0].transcript.is_none());
    }
    #[tokio::test]
    async fn voice_submission_failure_keeps_a_known_acceptance() {
        let (_root, store, id) = setup();
        store
            .voice_begin("new", id.clone(), "d".repeat(64))
            .await
            .unwrap();
        let preflight = store
            .voice_submit_failure("new", &id, "预检失败".into())
            .await
            .unwrap();
        assert_eq!(preflight.voices[0].phase, VoicePhase::Staged);
        assert!(preflight.voices[0].message.contains("预检失败"));
        assert!(store.voice_prepare("new", &id).await.unwrap());
        let job = VoiceJob {
            job_id: uuid::Uuid::new_v4().to_string(),
            task_dir: "X:/isolated/voice".into(),
        };
        let received = store
            .voice_update(
                "new",
                &id,
                Some(job),
                VoicePhase::Accepted,
                None,
                String::new(),
            )
            .await
            .unwrap();
        let stale = store
            .voice_submit_failure("new", &id, "迟到的失败".into())
            .await
            .unwrap();
        assert_eq!(stale.revision, received.revision);
        assert_eq!(stale.voices[0].phase, VoicePhase::Accepted);
        assert!(stale.voices[0].job == received.voices[0].job);
    }
    #[tokio::test]
    async fn voice_failed_atomic_insert_and_capacity_do_not_consume_the_receipt() {
        let (_root, store, id) = setup();
        store
            .voice_begin("new", id.clone(), "e".repeat(64))
            .await
            .unwrap();
        let ready = store
            .voice_update(
                "new",
                &id,
                None,
                VoicePhase::Ready,
                Some("保留文字".into()),
                String::new(),
            )
            .await
            .unwrap();
        store.fail_writes(true);
        assert!(store.voice_apply("new", &id, ready.revision).await.is_err());
        let retained = store.get("new").unwrap();
        assert!(retained.body.text.is_empty());
        assert_eq!(retained.voices[0].phase, VoicePhase::Ready);
        store.fail_writes(false);
        let full = store
            .save(
                "new",
                retained.revision,
                super::super::DesktopBody {
                    image: None,
                    text: "a".repeat(super::super::MAX_DRAFT_BYTES),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(store
            .voice_apply("new", &id, full.revision)
            .await
            .err()
            .unwrap()
            .contains("64 KiB"));
        assert_eq!(store.get("new").unwrap().voices[0].phase, VoicePhase::Ready);
        let long = "长中文稿".repeat(1000);
        let saved = store
            .save(
                "new",
                full.revision,
                super::super::DesktopBody {
                    image: None,
                    text: long.clone(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let inserted = store.voice_apply("new", &id, saved.revision).await.unwrap();
        assert_eq!(inserted.body.text, format!("{long}\n保留文字"));
        assert_eq!(inserted.voices[0].phase, VoicePhase::Applied);
    }
    #[tokio::test]
    async fn voice_native_namespaces_stay_separate_from_desktop_and_old_format_still_reads() {
        let (root, store, id) = setup();
        let scope = format!("native:claude-code:{}", uuid::Uuid::new_v4());
        let native = store.voice_begin(&scope, id, "f".repeat(64)).await.unwrap();
        assert_eq!(native.voices.len(), 1);
        assert!(store.get("new").unwrap().voices.is_empty());
        assert!(store.get("native:codex:../not-a-thread").is_err());
        let opaque = "native:codex:thread_synthetic_opaque_01";
        let opaque_body = super::super::DesktopBody {
            image: None,
            text: "不透明原生会话标识".into(),
            ..Default::default()
        };
        store.save(opaque, 0, opaque_body.clone()).await.unwrap();
        assert_eq!(store.get(opaque).unwrap().body.text, opaque_body.text);
        assert!(store
            .save(
                opaque,
                1,
                super::super::DesktopBody {
                    image: None,
                    route: "unsupported-model".into(),
                    ..opaque_body
                }
            )
            .await
            .is_err());
        let saved = store
            .save(
                "new",
                0,
                super::super::DesktopBody {
                    image: None,
                    text: "兼容旧草稿".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let mut old = serde_json::to_value(saved).unwrap();
        old.as_object_mut().unwrap().remove("voices");
        let cipher = crate::wechat_crypto::crypt(&serde_json::to_vec(&old).unwrap(), true).unwrap();
        std::fs::write(store.path("new").unwrap(), cipher).unwrap();
        let fresh = DesktopStore::new(root.path().join("desktop"));
        assert_eq!(fresh.get("new").unwrap().body.text, "兼容旧草稿");
        assert!(fresh.get("new").unwrap().voices.is_empty());
    }
    #[tokio::test]
    async fn voice_intent_restart_and_once_only_apply_share_the_draft_transaction() {
        let (root, store, id) = setup();
        store
            .voice_begin("new", id.clone(), "a".repeat(64))
            .await
            .unwrap();
        assert!(store.voice_prepare("new", &id).await.unwrap());
        assert!(!store.voice_prepare("new", &id).await.unwrap());
        let restarted = DesktopStore::new(root.path().join("desktop"));
        assert_eq!(
            restarted.get("new").unwrap().voices[0].phase,
            VoicePhase::Submitting
        );
        let ready = restarted
            .voice_update(
                "new",
                &id,
                None,
                VoicePhase::Ready,
                Some("合成语音".into()),
                String::new(),
            )
            .await
            .unwrap();
        assert!(restarted.voice_apply("new", &id, 0).await.is_err());
        let applied = restarted
            .voice_apply("new", &id, ready.revision)
            .await
            .unwrap();
        assert_eq!(applied.body.text, "合成语音");
        let same = restarted.voice_apply("new", &id, 0).await.unwrap();
        assert_eq!(same.revision, applied.revision);
        assert_eq!(same.body.text, applied.body.text);
        assert!(same.voices[0].transcript.is_none());
    }
    #[tokio::test]
    async fn voice_write_failure_blocks_start_and_dismissed_receipts_ignore_late_results() {
        let (_root, store, id) = setup();
        store
            .voice_begin("new", id.clone(), "b".repeat(64))
            .await
            .unwrap();
        store.fail_writes(true);
        assert!(store.voice_prepare("new", &id).await.is_err());
        assert_eq!(
            store.get("new").unwrap().voices[0].phase,
            VoicePhase::Staged
        );
        store.fail_writes(false);
        store.voice_dismiss("new", &id).await.unwrap();
        let late = store
            .voice_update(
                "new",
                &id,
                None,
                VoicePhase::Ready,
                Some("迟到结果".into()),
                String::new(),
            )
            .await
            .unwrap();
        assert_eq!(late.voices[0].phase, VoicePhase::Dismissed);
        assert!(late.voices[0].transcript.is_none());
        assert!(store.voice_apply("new", &id, late.revision).await.is_err());
        let new_id = uuid::Uuid::new_v4().to_string();
        let next = store
            .voice_begin("new", new_id, "c".repeat(64))
            .await
            .unwrap();
        assert_eq!(next.voices.len(), 2);
    }
}
