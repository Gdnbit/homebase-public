//! User-initiated microphone staging; no arbitrary source/output paths accepted.
use crate::assistant_desktop::{DesktopEntry, DesktopReply, VoiceJob, VoicePhase, VoiceRecord};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn folder(recording_id: &str) -> Result<PathBuf, String> {
    let id = uuid::Uuid::parse_str(recording_id).map_err(|_| "录音编号无效")?;
    Ok(crate::adapters::homebase_home_dir()
        .join(".homebase")
        .join("assistant")
        .join("recordings")
        .join(id.to_string()))
}

#[derive(Serialize)]
pub struct VoiceStage {
    pub recording_id: String,
    pub source_path: String,
    pub output_root: String,
    pub audio_sha256: String,
}

#[tauri::command]
pub async fn assistant_voice_stage(
    audio_base64: String,
    mime_type: String,
) -> Result<VoiceStage, String> {
    if audio_base64.len() > 14 * 1024 * 1024 {
        return Err("录音超过 10 MiB 上限，请缩短录音".into());
    }
    let bytes = STANDARD.decode(audio_base64).map_err(|_| "录音编码无效")?;
    if bytes.is_empty() || bytes.len() > 10 * 1024 * 1024 {
        return Err("录音为空或超过 10 MiB 上限".into());
    }
    let extension = match mime_type.split(';').next().unwrap_or("") {
        "audio/webm" if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) => "webm",
        "audio/ogg" if bytes.starts_with(b"OggS") => "ogg",
        "audio/wav" if bytes.starts_with(b"RIFF") => "wav",
        _ => return Err("录音格式不支持，请使用 WebM、Ogg 或 WAV".into()),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let dir = folder(&id)?;
    crate::control_protocol::private_directory(&dir)
        .await
        .map_err(|_| "录音目录无法设为私有，未写入录音")?;
    let source = dir.join(format!("input.{extension}"));
    crate::control_protocol::publish_private(&source, &bytes)
        .await
        .map_err(|_| "录音暂存失败")?;
    Ok(VoiceStage {
        recording_id: id,
        source_path: source.to_string_lossy().into(),
        output_root: dir.join("transcripts").to_string_lossy().into(),
        audio_sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}

#[tauri::command]
pub async fn assistant_voice_discard(recording_id: String) -> Result<(), String> {
    let dir = folder(&recording_id)?;
    // Remove only the owned raw recording; keep ASR receipts/results for recovery.
    for extension in ["webm", "ogg", "wav"] {
        let path = dir.join(format!("input.{extension}"));
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("录音清理失败，可稍后重试".into()),
        }
    }
    Ok(())
}

fn voice(entry: &DesktopEntry, id: &str) -> Result<VoiceRecord, String> {
    entry
        .voices
        .iter()
        .find(|v| v.recording_id == id)
        .cloned()
        .ok_or("录音不属于当前输入，未读取其他会话".into())
}
fn output(id: &str) -> Result<String, String> {
    Ok(folder(id)?.join("transcripts").to_string_lossy().into())
}
fn payload(id: &str) -> Result<super::asr::AsrTaskStartPayload, String> {
    let dir = folder(id)?;
    let source = ["webm", "ogg", "wav"]
        .into_iter()
        .map(|ext| dir.join(format!("input.{ext}")))
        .find(|p| p.is_file())
        .ok_or("原录音文件不可读，未重新提交")?;
    Ok(super::asr::AsrTaskStartPayload {
        source_path: source.to_string_lossy().into(),
        output_root: Some(output(id)?),
        task_name: Some(format!("助理语音-{}", &id[..8])),
        preset_id: "asr_zh_balanced".into(),
        engine_ids: vec!["faster_whisper_large_v3_cpu".into()],
        language: Some("zh".into()),
        fusion_mode: Some("manual".into()),
        preprocess_profile: None,
        runner_path: None,
        python_path: None,
        allow_system_drive: true,
    })
}
fn reply(entry: DesktopEntry) -> DesktopReply {
    DesktopReply::entry("loaded", entry)
}
fn failure(code: &str, message: String) -> DesktopReply {
    DesktopReply::failure(code, message)
}

#[tauri::command]
pub async fn assistant_voice_begin(
    scope: String,
    audio_base64: String,
    mime_type: String,
) -> Result<DesktopReply, String> {
    super::assistant_desktop::validate_scope(&scope).await?;
    let store = super::assistant_desktop::store();
    if store.get(&scope)?.voices.iter().any(|v| v.phase.active()) {
        return Ok(failure(
            "VOICE_PENDING",
            "已有待核对录音，请先查看或明确停止等待".into(),
        ));
    }
    let stage = assistant_voice_stage(audio_base64, mime_type).await?;
    crate::control_protocol::private_directory(&folder(&stage.recording_id)?.join("transcripts"))
        .await
        .map_err(|_| "无法保护转写输出目录，未启动任务")?;
    Ok(
        match store
            .voice_begin(&scope, stage.recording_id.clone(), stage.audio_sha256)
            .await
        {
            Ok(entry) => reply(entry),
            Err(message) => {
                let _ = assistant_voice_discard(stage.recording_id).await;
                failure("VOICE_SAVE_FAILED", message)
            }
        },
    )
}

/// Read the original owned ASR journal. No path or job identifier comes from the client.
async fn observe(scope: &str, id: &str) -> Result<DesktopReply, String> {
    let store = super::assistant_desktop::store();
    let entry = store.get(scope)?;
    let mut record = voice(&entry, id)?;
    if matches!(
        record.phase,
        VoicePhase::Staged
            | VoicePhase::Ready
            | VoicePhase::Applied
            | VoicePhase::Dismissed
            | VoicePhase::Cancelled
            | VoicePhase::Failed
    ) {
        return Ok(reply(entry));
    }
    let root = output(id)?;
    let jobs = super::asr::asr_task_reconcile(root.clone()).await?;
    if record.job.is_none() {
        let source_dir =
            std::fs::canonicalize(folder(id)?).map_err(|_| "原录音目录不可读，回执保留")?;
        let owned: Vec<_> = jobs
            .iter()
            .filter(|j| {
                j["sourcePath"].as_str().is_some_and(|s| {
                    let path = std::path::Path::new(s);
                    path.file_name().is_some_and(|n| {
                        ["input.webm", "input.ogg", "input.wav"]
                            .iter()
                            .any(|name| n == *name)
                    }) && path
                        .parent()
                        .and_then(|p| std::fs::canonicalize(p).ok())
                        .is_some_and(|p| p == source_dir)
                })
            })
            .collect();
        if owned.len() != 1 {
            let message = if owned.is_empty() {
                "尚未找到原转写回执；不代表未受理，未重新提交"
            } else {
                "同一录音存在多个任务记录，需要核对，未自动选择或重提"
            };
            return Ok(reply(
                store
                    .voice_update(scope, id, None, VoicePhase::Unknown, None, message.into())
                    .await?,
            ));
        }
        let job = VoiceJob {
            job_id: owned[0]["jobId"].as_str().ok_or("ASR回执编号缺失")?.into(),
            task_dir: owned[0]["taskDir"]
                .as_str()
                .ok_or("ASR回执目录缺失")?
                .into(),
        };
        let entry = store
            .voice_update(
                scope,
                id,
                Some(job),
                VoicePhase::Accepted,
                None,
                String::new(),
            )
            .await?;
        record = voice(&entry, id)?;
    }
    let job = record.job.clone().ok_or("原转写回执缺失")?;
    if let Some(journal) = jobs.iter().find(|row| row["jobId"] == job.job_id) {
        let state = journal["status"].as_str().unwrap_or("unknown");
        if ["cancelled", "canceled", "failed"].contains(&state) {
            return Ok(reply(
                store
                    .voice_update(
                        scope,
                        id,
                        None,
                        if state == "failed" {
                            VoicePhase::Failed
                        } else {
                            VoicePhase::Cancelled
                        },
                        None,
                        if state == "failed" {
                            "原转写日志确认失败，未插入草稿"
                        } else {
                            "原转写日志确认已取消"
                        }
                        .into(),
                    )
                    .await?,
            ));
        }
    }
    let view = match super::asr::asr_task_get(
        job.task_dir.clone(),
        Some(root.clone()),
        Some(job.job_id.clone()),
    )
    .await
    {
        Ok(view) => view,
        Err(_) => {
            let entry = store
                .voice_update(
                    scope,
                    id,
                    None,
                    record.phase,
                    None,
                    "暂不能读取原任务进度，回执保留；可重试查询，不会重新提交".into(),
                )
                .await?;
            let read_failed = voice(&entry, id)?.phase == record.phase;
            let mut value = reply(entry);
            if read_failed {
                value.status = "partial".into();
            }
            return Ok(value);
        }
    };
    let journal_status = view
        .job
        .as_ref()
        .map(|v| v.status.as_str())
        .unwrap_or("unknown");
    let meta_status = view.task_meta["status"].as_str().unwrap_or("unknown");
    let terminal = |s: &str| {
        [
            "success",
            "completed",
            "succeeded",
            "failed",
            "partial",
            "canceled",
            "cancelled",
            "interrupted",
        ]
        .contains(&s)
    };
    let status = if terminal(journal_status) {
        journal_status
    } else if terminal(meta_status) {
        meta_status
    } else {
        journal_status
    };
    let managed = jobs
        .iter()
        .any(|row| row["jobId"] == job.job_id && row["runtime_state"] == "managed");
    if ["success", "completed", "succeeded"].contains(&status) {
        let file = view
            .artifacts
            .iter()
            .find(|a| {
                a.relative_path
                    .replace('\\', "/")
                    .ends_with("/transcript.txt")
                    || a.relative_path == "transcript.txt"
            })
            .ok_or("转写完成但没有正文文件，原回执保留")?;
        let text = super::asr::asr_task_read_artifact(
            job.task_dir,
            file.relative_path.clone(),
            Some(root),
        )
        .await?;
        let content = text.content.trim();
        if content.is_empty() || content.len() > 8000 {
            return Ok(reply(
                store
                    .voice_update(
                        scope,
                        id,
                        None,
                        VoicePhase::Failed,
                        None,
                        if content.is_empty() {
                            "未识别到语音内容；原结果保留"
                        } else {
                            "转写超过输入框容量；请在语音任务查看全文，未截断或自动插入"
                        }
                        .into(),
                    )
                    .await?,
            ));
        }
        return Ok(reply(
            store
                .voice_update(
                    scope,
                    id,
                    None,
                    VoicePhase::Ready,
                    Some(content.into()),
                    String::new(),
                )
                .await?,
        ));
    }
    let (phase, message) = match status {
        "cancelled" | "canceled" => (VoicePhase::Cancelled, "原转写任务已取消"),
        "failed" | "partial" => (
            VoicePhase::Failed,
            "本地转写失败或仅部分完成，请查看原任务，未插入草稿",
        ),
        "running" | "queued" if managed => (VoicePhase::Accepted, "正在本地转写"),
        _ => (
            VoicePhase::Unknown,
            "原任务执行状态未确认；只保留并查询原编号，不重新提交",
        ),
    };
    Ok(reply(
        store
            .voice_update(scope, id, None, phase, None, message.into())
            .await?,
    ))
}

#[tauri::command]
pub async fn assistant_voice_get(
    scope: String,
    recording_id: String,
) -> Result<DesktopReply, String> {
    super::assistant_desktop::validate_scope(&scope).await?;
    observe(&scope, &recording_id).await
}

#[tauri::command]
pub async fn assistant_voice_submit(
    app: tauri::AppHandle,
    scope: String,
    recording_id: String,
) -> Result<DesktopReply, String> {
    super::assistant_desktop::validate_scope(&scope).await?;
    let store = super::assistant_desktop::store();
    let record = voice(&store.get(&scope)?, &recording_id)?;
    if record.phase != VoicePhase::Staged {
        return observe(&scope, &recording_id).await;
    }
    let payload = payload(&recording_id)?;
    let bytes = std::fs::read(&payload.source_path).map_err(|_| "原录音不可读，未提交")?;
    if bytes.len() > 10 * 1024 * 1024
        || format!("{:x}", Sha256::digest(&bytes)) != record.audio_sha256
    {
        return Ok(failure(
            "VOICE_SOURCE_CHANGED",
            "原录音已变化，未提交其他内容".into(),
        ));
    }
    // Probe failures happen before the durable submission boundary, so an explicit
    // retry can fix the environment without ever replaying accepted work.
    match super::asr::start_prepared(
        app,
        payload,
        store.voice_prepare(&scope, &recording_id),
        true,
        false,
    )
    .await
    {
        Ok(None) => observe(&scope, &recording_id).await,
        Ok(Some(job)) => Ok(
            match store
                .voice_update(
                    &scope,
                    &recording_id,
                    Some(VoiceJob {
                        job_id: job.job_id,
                        task_dir: job.task_dir,
                    }),
                    VoicePhase::Accepted,
                    None,
                    String::new(),
                )
                .await
            {
                Ok(entry) => reply(entry),
                Err(message) => failure(
                    "VOICE_RECEIPT_UNKNOWN",
                    format!("原转写可能已受理，请查询原录音，不要重新提交：{message}"),
                ),
            },
        ),
        Err(message) => {
            let entry = store
                .voice_submit_failure(&scope, &recording_id, message)
                .await?;
            let current = voice(&entry, &recording_id)?;
            if current.phase == VoicePhase::Staged {
                let mut value = failure("VOICE_NOT_STARTED", current.message);
                value.entry = Some(entry);
                return Ok(value);
            }
            Ok(reply(entry))
        }
    }
}

#[tauri::command]
pub async fn assistant_voice_apply(
    scope: String,
    recording_id: String,
    revision: u64,
) -> Result<DesktopReply, String> {
    super::assistant_desktop::validate_scope(&scope).await?;
    Ok(
        match super::assistant_desktop::store()
            .voice_apply(&scope, &recording_id, revision)
            .await
        {
            Ok(entry) => {
                let mut value = reply(entry);
                if assistant_voice_discard(recording_id).await.is_err() {
                    value.status = "partial".into();
                    value.warning =
                        Some("转写已插入，但原录音清理尚未确认；不要重复生成或追加".into());
                }
                value
            }
            Err(message) => failure("VOICE_APPLY_FAILED", message),
        },
    )
}

#[tauri::command]
pub async fn assistant_voice_dismiss(
    scope: String,
    recording_id: String,
) -> Result<DesktopReply, String> {
    super::assistant_desktop::validate_scope(&scope).await?;
    Ok(reply(
        super::assistant_desktop::store()
            .voice_dismiss(&scope, &recording_id)
            .await?,
    ))
}

#[tauri::command]
pub async fn assistant_voice_cancel(
    app: tauri::AppHandle,
    scope: String,
    recording_id: String,
) -> Result<DesktopReply, String> {
    super::assistant_desktop::validate_scope(&scope).await?;
    let store = super::assistant_desktop::store();
    let record = voice(&store.get(&scope)?, &recording_id)?;
    if matches!(
        record.phase,
        VoicePhase::Applied | VoicePhase::Dismissed | VoicePhase::Cancelled | VoicePhase::Failed
    ) {
        return Ok(reply(store.get(&scope)?));
    }
    let discard_after_cancel = matches!(record.phase, VoicePhase::Staged | VoicePhase::Ready);
    if !discard_after_cancel {
        let _ = observe(&scope, &recording_id).await;
    }
    let entry = store.voice_request_cancel(&scope, &recording_id).await?;
    let record = voice(&entry, &recording_id)?;
    if record.phase == VoicePhase::Cancelled && discard_after_cancel {
        let _ = assistant_voice_discard(recording_id).await;
        return Ok(reply(entry));
    }
    if record.phase == VoicePhase::CancelRequested {
        if let Some(job) = record.job {
            let _ = super::asr::asr_task_cancel(app, job.job_id).await;
            return observe(&scope, &recording_id).await;
        }
    }
    Ok(reply(entry))
}
