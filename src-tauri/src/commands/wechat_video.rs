//! Local audio extraction and durable ASR observation for WeChat media receipts.
use crate::wechat::video::{
    self, MediaKind, VideoRequest, VideoResult, TRANSCRIPT_LIMIT, VIDEO_LIMIT, VOICE_LIMIT,
};
use crate::wechat::clip_video::{ClipArtifactSource, ClipRange, ClipSource};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    OnceLock,
};
#[path = "wechat_voice_decoder.rs"]
mod voice_decoder;

const ENGINE: &str = crate::wechat::subtitles::WHISPER_ENGINE;
const PRESET: &str = "asr_zh_balanced";
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

pub(crate) fn configure(app: tauri::AppHandle) {
    let _ = APP.set(app);
}

pub(crate) async fn handle(request: VideoRequest) -> VideoResult {
    match request {
        VideoRequest::Start { id, bytes } => start(MediaKind::Video, &id, &bytes)
            .await
            .unwrap_or_else(VideoResult::Failed),
        VideoRequest::StartVoice { id, bytes } => start(MediaKind::Voice, &id, &bytes)
            .await
            .unwrap_or_else(VideoResult::Failed),
        VideoRequest::Observe { id } => observe(MediaKind::Video, &id)
            .await
            .unwrap_or(VideoResult::Unknown),
        VideoRequest::ObserveVoice { id } => observe(MediaKind::Voice, &id)
            .await
            .unwrap_or(VideoResult::Unknown),
    }
}

fn media_folder(kind: MediaKind, id: &str) -> Result<PathBuf, String> {
    if !video::receipt_id(id) {
        return Err("原音视频收件编号无效。".into());
    }
    Ok(crate::adapters::homebase_home_dir()
        .join(".homebase")
        .join("wechat")
        .join("workspace")
        .join(match kind {
            MediaKind::Video => "video-transcripts",
            MediaKind::Voice => "voice-transcripts",
        })
        .join(id))
}

fn ordinary_directory(path: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    meta.is_dir() && !meta.file_type().is_symlink()
}

/// 将当前微信会话原片、已登记字幕、三张封面和发布草稿制作成一个本机 ZIP。
#[tauri::command]
pub async fn capability_video_publish_package(
    source: ClipSource,
    segments: Vec<ClipRange>,
    cover_at_ms: u64,
    title: String,
    description: String,
) -> Result<Value, String> {
    let app = APP.get().ok_or("本机发布包环境尚未就绪。")?;
    let runtime = super::distribution::bootstrap(app).map_err(|_| "本机发布包环境暂不可用。")?;
    super::wechat::bridge().await?
        .publish_package_original(&runtime.ffmpeg_path, source, &segments, cover_at_ms, &title, &description).await
}

async fn start(kind: MediaKind, id: &str, bytes: &[u8]) -> Result<VideoResult, String> {
    let label = kind.label();
    match kind {
        MediaKind::Video if bytes.len() > VIDEO_LIMIT || !video::is_mp4(bytes) => {
            return Err("仅能在本机处理符合大小上限的 MP4 视频。".into())
        }
        MediaKind::Voice if bytes.len() > VOICE_LIMIT || !video::is_silk(bytes) => {
            return Err("仅能在本机处理符合大小上限的 SILK 语音。".into())
        }
        _ => (),
    }
    let app = APP.get().ok_or("本地转写环境尚未就绪。")?;
    let directory = media_folder(kind, id)?;
    let parent = directory.parent().ok_or("原转写工作目录无效。")?;
    if parent.exists() && !ordinary_directory(parent) {
        return Err("本机转写目录无法安全使用。".into());
    }
    let runtime = super::distribution::bootstrap(app).map_err(|_| "本机转写环境无法初始化。")?;
    crate::control_protocol::private_directory(parent)
        .await
        .map_err(|_| "无法保护本机转写目录，未保存文件。")?;
    if !ordinary_directory(parent) || fs2::available_space(parent).unwrap_or(0) < 40 * 1024 * 1024 {
        return Err("转写工作目录不可用或剩余空间不足，未保存文件。".into());
    }
    match std::fs::create_dir(&directory) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Ok(VideoResult::Unknown)
        }
        Err(_) => return Err("无法创建独立的音视频收件目录。".into()),
    }
    let wave = directory.join("audio.wav");
    match kind {
        MediaKind::Video => {
            let source = directory.join("original.mp4");
            crate::control_protocol::publish_private(&source, bytes)
                .await
                .map_err(|_| "本机保存视频失败，未开始转写。")?;
            video::extract_audio(&runtime.ffmpeg_path, &source, &wave).await?;
        }
        MediaKind::Voice => {
            let (node, agent_runtime) = if runtime.packaged {
                let folder = runtime.resource_root.join("agent-runtime");
                (folder.join("node.exe"), folder)
            } else {
                let source_tree = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .ok_or("本机开发运行包路径无效。")?;
                (PathBuf::from("node"), source_tree.join("agent-runtime"))
            };
            let audio = voice_decoder::decode(&node, &agent_runtime, &directory, bytes).await?;
            crate::control_protocol::publish_private(&wave, &audio)
                .await
                .map_err(|_| "本机保存语音采样失败，未开始转写。")?;
        }
    }
    let output = directory.join("transcripts");
    crate::control_protocol::private_directory(&output)
        .await
        .map_err(|_| "无法保护转写结果目录，未启动转写。")?;
    let payload = super::asr::AsrTaskStartPayload {
        source_path: wave.to_string_lossy().into(),
        output_root: Some(output.to_string_lossy().into()),
        task_name: Some(format!("微信{label}")),
        preset_id: PRESET.into(),
        engine_ids: vec![ENGINE.into()],
        language: Some("zh".into()),
        fusion_mode: Some("manual".into()),
        preprocess_profile: None,
        runner_path: None,
        python_path: None,
        allow_system_drive: true,
    };
    let claimed = AtomicBool::new(false);
    let result = super::asr::start_prepared(
        app.clone(),
        payload,
        async {
            claimed.store(true, Ordering::SeqCst);
            Ok(true)
        },
        true,
        true,
    )
    .await;
    match result {
        Ok(Some(_)) => Ok(VideoResult::Running),
        Ok(None) => Err("本地转写未启动。".into()),
        Err(_) if claimed.load(Ordering::SeqCst) => Ok(VideoResult::Unknown),
        Err(_) => Err("本机语音转写环境暂不可用，请检查本地 Whisper 模型。".into()),
    }
}

async fn observe(kind: MediaKind, id: &str) -> Result<VideoResult, String> {
    let label = kind.label();
    let folder = media_folder(kind, id)?;
    let parent = folder.parent().ok_or("原转写工作目录无效。")?;
    let output = folder.join("transcripts");
    if !ordinary_directory(parent) || !ordinary_directory(&folder) || !ordinary_directory(&output) {
        return Ok(VideoResult::Unknown);
    }
    let rows = super::asr::asr_task_reconcile(output.to_string_lossy().into()).await?;
    let [job] = rows.as_slice() else {
        return Ok(VideoResult::Unknown);
    };
    let wave = folder.join("audio.wav");
    if job["presetId"] != PRESET
        || job["engineIds"] != json!([ENGINE])
        || !job["sourcePath"]
            .as_str()
            .is_some_and(|path| crate::task_recovery::same_file(Path::new(path), &wave))
    {
        return Ok(VideoResult::Unknown);
    }
    let location = PathBuf::from(job["taskDir"].as_str().ok_or("原转写结果目录缺失。")?);
    let status = job["status"].as_str().unwrap_or("unknown");
    match status {
        "failed" | "partial" | "interrupted" | "verification_failed" => {
            return Ok(VideoResult::Failed(format!(
                "本地{label}转写失败，原收件已保留。"
            )))
        }
        "cancelled" | "canceled" => {
            return Ok(VideoResult::Failed(format!("原{label}转写已取消。")))
        }
        _ => {}
    }
    if job["runtime_state"] == "managed" {
        return Ok(VideoResult::Running);
    }
    let meta = crate::wechat::artifact_bytes_limited(
        &location.join("task.meta.json"),
        &output,
        256 * 1024,
    )?;
    let meta: Value = serde_json::from_slice(&meta).map_err(|_| "原转写状态无效。")?;
    match meta["status"].as_str().unwrap_or("unknown") {
        "success" | "completed" | "succeeded" => (),
        "failed" | "partial" | "interrupted" => {
            return Ok(VideoResult::Failed(format!("原{label}转写未完整完成。")))
        }
        _ => return Ok(VideoResult::Unknown),
    }
    let original = location.join("exports").join("transcript.txt");
    if std::fs::metadata(&original).is_ok_and(|m| m.is_file() && m.len() == 0) {
        return Ok(VideoResult::Failed(format!(
            "{label}没有识别到可转写的语音。"
        )));
    }
    let bytes = crate::wechat::artifact_bytes_limited(&original, &output, TRANSCRIPT_LIMIT)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| "原转写文字格式无法核对。")?
        .trim();
    if text.is_empty() {
        return Ok(VideoResult::Failed(format!(
            "{label}没有识别到可转写的语音。"
        )));
    }
    let preview: String = text.chars().take(1200).collect();
    let preview = if preview.len() < text.len() {
        format!("{preview}\n全文见文字稿附件。")
    } else {
        preview
    };
    let mut artifact = json!({"path":original.to_string_lossy(),"sha256":format!("{:x}", Sha256::digest(&bytes))});
    if kind == MediaKind::Video {
        use crate::wechat::subtitles;
        if let Some((kind, digest)) = subtitles::observe_timed_source(&location, &output) {
            artifact["timed_subtitle_kind"] = json!(kind);
            artifact["timed_subtitle_sha256"] = json!(digest);
        }
    }
    Ok(VideoResult::Ready {
        preview,
        artifact,
        repositories: if kind == MediaKind::Video { video::RepositoryHints::from_transcript(text) } else { Default::default() },
    })
}

/// 剪辑当前微信会话已接收并核验的原 MP4。新建片段，保留原片。start_ms 与 duration_ms 用毫秒。
#[tauri::command]
pub async fn capability_clip_video(source: ClipSource, start_ms: u64, duration_ms: u64) -> Result<Value, String> {
    let app = APP.get().ok_or("本机剪辑环境尚未就绪。")?;
    let runtime = super::distribution::bootstrap(app).map_err(|_| "本机剪辑环境暂不可用。")?;
    super::wechat::bridge().await?.clip_original(&runtime.ffmpeg_path, source, start_ms, duration_ms).await
}

/// 从同一微信原 MP4 取多个范围，按参数顺序拼接成新的本机 MP4。
#[tauri::command]
pub async fn capability_assemble_video(
    source: ClipSource,
    segments: Vec<ClipRange>,
    include_subtitles: Option<bool>,
) -> Result<Value, String> {
    let app = APP.get().ok_or("本机剪辑环境尚未就绪。")?;
    let runtime = super::distribution::bootstrap(app).map_err(|_| "本机剪辑环境暂不可用。")?;
    let bridge = super::wechat::bridge().await?;
    if include_subtitles.unwrap_or(false) {
        bridge.assemble_subtitled_original(&runtime.ffmpeg_path, source, &segments).await
    } else {
        bridge.assemble_original(&runtime.ffmpeg_path, source, &segments).await
    }
}

/// 按上一版成片的时间轴截取一段，保留现有成片。source 只能引用当前微信会话已登记的本机剪辑产物。
#[tauri::command]
pub async fn capability_trim_clip_artifact(source: ClipArtifactSource, start_ms: u64, duration_ms: u64) -> Result<Value, String> {
    let app = APP.get().ok_or("CLIP_RENDER_FAILED: 本机剪辑环境尚未就绪。")?;
    let runtime = super::distribution::bootstrap(app).map_err(|_| "CLIP_RENDER_FAILED: 本机剪辑环境暂不可用。")?;
    super::wechat::bridge().await.map_err(|error| format!("CLIP_SOURCE_UNAVAILABLE: {error}"))?
        .trim_clip_artifact(&runtime.ffmpeg_path, source, start_ms, duration_ms).await
}

/// 在当前微信会话的已核验原 MP4 选择时间点，生成三种固定尺寸的 PNG 封面。
#[tauri::command]
pub async fn capability_video_cover(source: ClipSource, at_ms: u64, title: String) -> Result<Value, String> {
    let app = APP.get().ok_or("COVER_RENDER_FAILED: 本机封面环境尚未就绪。")?;
    let runtime = super::distribution::bootstrap(app).map_err(|_| "COVER_RENDER_FAILED: 本机封面环境暂不可用。")?;
    super::wechat::bridge().await.map_err(|error| format!("COVER_SOURCE_UNAVAILABLE: {error}"))?
        .cover_original(&runtime.ffmpeg_path, source, at_ms, &title).await
}
