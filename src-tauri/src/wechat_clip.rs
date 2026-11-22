//! A bounded local edit of one original MP4 admitted by the WeChat bridge.
use super::*;
use regex::Regex;
use std::{process::Stdio, sync::LazyLock};
use tokio::io::AsyncReadExt;

pub(crate) const OUTPUT_LIMIT: usize = 19 * 1024 * 1024;
pub(crate) const TRIM_ARTIFACT_COMMAND: &str = "capability_trim_clip_artifact";
const FRAMES_PER_SECOND: u64 = 30;
static RECENT_VIDEO_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"刚(?:刚|才)?发(?:送|给你)?的?(?:[这那一][个条段])?(?:视频|原片)")
        .expect("static recent video reference pattern")
});
static PREVIOUS_VIDEO_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^第\s*[二2]\s*(?:段\s*(?:视频|原片)|[个条]\s*视频)")
        .expect("static previous video reference pattern")
});
static PREVIOUS_CLIP_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:照旧|[按照]老办法|",
        r"(?:按照|按|照|沿用)(?:我们|咱们)?(?:之前|以前|上次|上回|平时)的?(?:方式|做法|办法|流程))$"
    ))
    .expect("static previous clip method pattern")
});

pub(crate) fn trim_failure_message(code: &str) -> Option<&'static str> {
    Some(match code {
        "CLIP_SOURCE_UNAVAILABLE" => "无法读取已登记的上一版成片。请检查微信接收是否开启，并核对成片是否仍属于当前会话。",
        "CLIP_SOURCE_CHANGED" => "上一版成片与登记的文件内容不一致。请先核对原文件。",
        "CLIP_RANGE_INVALID" => "所选片段超出上一版成片的时长或剪辑范围限制。请按成片时间重新指定。",
        "CLIP_RENDER_FAILED" => "本机未能生成新的剪辑成片。请核对原任务结果和本机剪辑环境。",
        _ => return None,
    })
}

pub(crate) fn is_clip_command(command: &str) -> bool {
    matches!(command, "capability_clip_video" | "capability_assemble_video" | TRIM_ARTIFACT_COMMAND)
}

pub(crate) fn uses_original_mp4(command: &str) -> bool {
    matches!(command, "capability_clip_video" | "capability_assemble_video" | super::video_cover::COMMAND | super::publish_package::COMMAND)
}

pub(crate) fn is_clip_output(path: &Path) -> bool {
    let Some(directory) = path.parent() else { return false };
    path.is_absolute()
        && path.file_name().and_then(|name| name.to_str()) == Some("clip.mp4")
        && directory.parent().and_then(Path::file_name).and_then(|name| name.to_str()) == Some("clips")
        && directory.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_prefix("clip-"))
            .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipSource {
    pub receipt_id: String,
    pub session_id: String,
    pub sha256: String,
}

impl ClipSource {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !video::receipt_id(&self.receipt_id)
            || uuid::Uuid::parse_str(&self.session_id).is_err()
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("原视频收件或摘要无法核对。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipArtifactSource {
    pub session_id: String,
    pub turn_id: String,
    pub job_id: String,
    pub sha256: String,
}

impl ClipArtifactSource {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if [&self.session_id, &self.turn_id, &self.job_id].iter().any(|id| uuid::Uuid::parse_str(id).is_err())
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err("上一版成片的任务归属或摘要无法核对。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClipRange {
    pub start_ms: u64,
    pub duration_ms: u64,
}

pub(crate) fn validate_ranges(ranges: &[ClipRange]) -> Result<(), String> {
    if ranges.iter().any(|range| {
        !(200..=30_000).contains(&range.duration_ms)
            || range
                .start_ms
                .checked_add(range.duration_ms)
                .is_none_or(|end| end > 120_000)
    }) {
        return Err("每段须位于原视频前 120 秒内，长度为 0.2 至 30 秒。".into());
    }
    if ranges.iter().map(|range| range.duration_ms).sum::<u64>() > 30_000 {
        return Err("所有选中片段的总长度不得超过 30 秒。".into());
    }
    Ok(())
}

fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

pub(super) fn is_edit_request(text: &str) -> bool {
    let text = text.trim();
    if text.starts_with('/')
        || crate::assistant::explicit_discussion(text)
        || [
            "不要剪",
            "别剪",
            "不用剪",
            "先别",
            "怎么剪",
            "如何剪",
            "为什么",
            "解释",
            "分析",
            "剪辑手法",
            "剪辑思路",
            "当参考",
            "作为参考",
            "作参考",
            "做参考",
            "参考这段视频",
            "参考这条视频",
            "参考这个视频",
            "参考这段原片",
            "怎么样",
            "剪辑好了",
            "剪好了吗",
        ]
        .iter()
        .any(|term| text.contains(term))
    {
        return false;
    }
    let request = [
        "帮我",
        "请",
        "麻烦",
        "拜托",
        "能帮我",
        "可以帮我",
        "能不能帮我",
        "能把",
        "可以把",
        "能不能把",
        "可不可以帮我",
    ]
    .iter()
    .any(|p| text.starts_with(p))
        || requests_previous_clip_method(text);
    (request
        || (!text.contains(['?', '？'])
            && [
                "按",
                "照",
                "把",
                "刚发",
                "刚刚",
                "刚才",
                "上一个",
                "上一条",
                "倒数",
                "这段",
                "这个",
                "这条",
                "剪辑",
                "剪片",
                "剪个片",
                "剪一下",
                "截取",
                "裁剪",
                "只保留",
                "拼接",
                "合并",
            ]
            .iter()
            .any(|p| text.starts_with(p))))
        && [
            "剪辑",
            "剪片",
            "剪个片",
            "剪成",
            "剪出",
            "剪一下",
            "截取",
            "裁剪",
            "只保留",
            "剪掉",
            "拼接",
            "合并",
            "拼成",
            "拼在一起",
            "再接",
        ]
        .iter()
        .any(|term| text.contains(term))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecentClip {
    Latest,
    Previous,
    Unique,
    ImplicitUnique,
}

pub(super) fn recent_clip_intent(text: &str) -> Option<RecentClip> {
    if !(is_edit_request(text) || super::video_cover::is_cover_request(text) || super::publish_package::is_package_request(text))
        || [
            "不是刚发", "别用刚发", "不要用刚发", "前一个",
        ]
            .iter()
            .any(|term| text.contains(term))
    {
        return None;
    }
    let latest = RECENT_VIDEO_REFERENCE.is_match(text)
        || ["上一个视频", "上一段视频", "上一条视频"]
            .iter()
            .any(|term| text.contains(term));
    let unique = [
        "这段视频",
        "这个视频",
        "这条视频",
        "这段原片",
        "这个原片",
        "这条原片",
    ]
    .iter()
    .any(|term| text.contains(term));
    if let Some((_, ordinal)) = text.split_once("倒数") {
        return (!latest
            && !unique
            && !ordinal.contains("倒数")
            && PREVIOUS_VIDEO_REFERENCE.is_match(ordinal))
        .then_some(RecentClip::Previous);
    }
    if latest {
        return Some(RecentClip::Latest);
    }
    if unique {
        return Some(RecentClip::Unique);
    }
    requests_previous_clip_method(text).then_some(RecentClip::ImplicitUnique)
}

fn requests_previous_clip_method(text: &str) -> bool {
    // Only infer an original for a complete request with no other object.
    const ACTIONS: &[&str] = &[
        "剪辑一下",
        "剪辑",
        "剪一下",
        "剪片",
        "剪个片",
        "完成一下剪辑",
        "完成剪辑",
    ];
    let text = text
        .trim()
        .trim_end_matches(['。', '！', '!', '？', '?'])
        .trim_end();
    let request = [
        "可不可以帮我", "能不能帮我", "可以帮我", "能帮我", "麻烦你", "麻烦", "请你",
        "帮我", "请", "拜托",
    ]
    .into_iter()
    .find_map(|prefix| text.strip_prefix(prefix))
    .unwrap_or(text)
    .trim_start();
    let request = ["就行", "吧", "吗"]
        .into_iter()
        .find_map(|suffix| request.strip_suffix(suffix))
        .unwrap_or(request)
        .trim_end();
    let request = request.strip_prefix('就').unwrap_or(request).trim_start();
    if request == "沿用上次的剪辑流程" {
        return true;
    }
    ACTIONS
        .iter()
        .filter_map(|&action| request.strip_suffix(action))
        .any(|method| {
            let method = method.trim_end();
            let method = method.strip_suffix("帮我").unwrap_or(method);
            let method =
                method.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '，' | ','));
            PREVIOUS_CLIP_METHOD.is_match(method)
        })
        || ACTIONS
            .iter()
            .filter_map(|&action| request.strip_prefix(action))
            .any(|method| {
                let method = method
                    .trim_start()
                    .trim_start_matches(['，', ','])
                    .trim_start();
                let method = method.strip_prefix("还是").unwrap_or(method).trim_start();
                PREVIOUS_CLIP_METHOD.is_match(method)
            })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipRevision {
    Original,
    Rendered,
    Clarify,
}

fn clip_revision_text(text: &str) -> &str {
    let text = text.trim().trim_end_matches(['。', '！', '!', '？', '?']).trim_end();
    let text = ["帮我", "请", "麻烦你", "麻烦"]
        .into_iter().find_map(|prefix| text.strip_prefix(prefix)).unwrap_or(text).trim_start();
    text.strip_prefix("这次").unwrap_or(text).trim_start()
}

pub(super) fn timeline_answer(text: &str) -> Option<&'static str> {
    if text.trim_end().ends_with(['？', '?']) {
        return None;
    }
    match clip_revision_text(text) {
        "按原片" | "按原片时间" | "按原片时间剪" => Some("原片"),
        "按成片" | "按成片时间" | "按成片时间剪" | "按上一版成片" => Some("上一版成片的"),
        _ => None,
    }
}

pub(crate) fn ambiguous_clip_range(text: &str) -> Option<&str> {
    let text = clip_revision_text(text);
    let range = ["只留", "只保留", "保留", "截取"]
        .into_iter()
        .find_map(|prefix| text.strip_prefix(prefix))?
        .trim_start();
    let duration = range.strip_prefix('前')?.strip_suffix('秒')?.trim();
    let digits = |c: char| c.is_ascii_digit() || "零一二两三四五六七八九十半".contains(c);
    let mut components = duration.split(['.', '点']);
    let valid_digits = |part: &str| !part.is_empty() && part.chars().all(digits);
    (duration.chars().count() <= 12 && valid_digits(components.next()?)
        && components.next().is_none_or(valid_digits) && components.next().is_none()).then_some(range)
}

pub(crate) fn clip_revision_intent(text: &str) -> Option<ClipRevision> {
    let text = clip_revision_text(text);
    if ["只留原片", "只保留原片", "保留原片", "截取原片", "用原片", "从原片",
        "只留原视频", "只保留原视频", "保留原视频", "截取原视频", "用原视频", "从原视频"]
        .into_iter()
        .filter_map(|prefix| text.strip_prefix(prefix))
        .map(str::trim_start)
        .any(|range| range.starts_with('前') || range.starts_with('第') || range.starts_with(|c: char| c.is_ascii_digit()))
    {
        return Some(ClipRevision::Original);
    }
    let request = ["只留", "只保留", "保留", "截取", "用", "从", "把"]
        .into_iter().find_map(|prefix| text.strip_prefix(prefix));
    let rendered = request.and_then(|text| ["上一版成片", "上一版视频", "这一版成片", "这版成片", "这份成片", "成片"]
        .into_iter().find_map(|prefix| text.strip_prefix(prefix)));
    if rendered.is_some_and(|range| {
        let range = range.trim_start_matches(['的', '，', ',', ' ']);
        range.starts_with(['前', '第']) || range.starts_with(|c: char| c.is_ascii_digit())
    }) {
        return Some(ClipRevision::Rendered);
    }
    if matches!(text, "再短一点" | "再剪短一点" | "剪短一点" | "能再短一点吗")
        || ["只留前", "只保留前", "保留前", "截取前"].into_iter().any(|prefix| text.starts_with(prefix) && text.ends_with('秒'))
    {
        return Some(ClipRevision::Clarify);
    }
    None
}

impl Bridge {
    pub(crate) async fn trim_clip_artifact(
        &self,
        ffmpeg: &Path,
        source: ClipArtifactSource,
        start_ms: u64,
        duration_ms: u64,
    ) -> Result<Value, String> {
        source.validate().map_err(|error| format!("CLIP_SOURCE_UNAVAILABLE: {error}"))?;
        let range = ClipRange { start_ms, duration_ms };
        validate_ranges(&[range]).map_err(|error| format!("CLIP_RANGE_INVALID: {error}"))?;
        let (account, generation) = {
            let state = self.state.lock().await;
            if !state.stored.enabled {
                return Err("CLIP_SOURCE_UNAVAILABLE: 微信接收已停用，未剪辑上一版成片。".into());
            }
            (state.stored.account(), self.generation.load(Ordering::SeqCst))
        };
        let resolved = (self.agent.as_ref().ok_or("CLIP_SOURCE_UNAVAILABLE: 共享助理尚未就绪。")?)(
            json!({"operation":"resolve_clip_artifact","source_key":account.logical_key(),"clip":source}),
        ).await.map_err(|error| format!("CLIP_SOURCE_UNAVAILABLE: {error}"))?;
        if start_ms + duration_ms > resolved["duration_ms"].as_u64().ok_or("CLIP_SOURCE_UNAVAILABLE: 无法核对上一版成片的时长。")? {
            return Err("CLIP_RANGE_INVALID: 所选范围超出上一版成片的时长。".into());
        }
        let root = self.workspace.join("clips");
        let path = Path::new(resolved["path"].as_str().ok_or("CLIP_SOURCE_UNAVAILABLE: 上一版成片文件无法核对。")?);
        if !is_clip_output(path) || path.parent().and_then(Path::parent) != Some(root.as_path()) {
            return Err("CLIP_SOURCE_UNAVAILABLE: 上一版成片不在已登记的剪辑目录内。".into());
        }
        let state = self.state.lock().await;
        if !self.inbound_account_matches(&state, &account, generation) {
            return Err("CLIP_SOURCE_UNAVAILABLE: 微信绑定已变化，未读取上一版成片。".into());
        }
        let bytes = artifact_bytes_limited(path, &root, OUTPUT_LIMIT)
            .map_err(|_| "CLIP_SOURCE_UNAVAILABLE: 上一版成片已移除或无法读取，请核对原结果。")?;
        drop(state);
        if resolved["bytes"] != bytes.len() || !video::is_mp4(&bytes)
            || format!("{:x}", Sha256::digest(&bytes)) != source.sha256
        {
            return Err("CLIP_SOURCE_CHANGED: 上一版成片与登记摘要不一致，未生成新版本。".into());
        }
        render_clip(ffmpeg, &self.workspace, &bytes, &[range]).await
            .map_err(|error| format!("CLIP_RENDER_FAILED: {error}"))
    }

    pub(crate) async fn clip_original(
        &self,
        ffmpeg: &Path,
        source: ClipSource,
        start_ms: u64,
        duration_ms: u64,
    ) -> Result<Value, String> {
        let range = ClipRange {
            start_ms,
            duration_ms,
        };
        self.edit_original(ffmpeg, source, &[range]).await
    }

    pub(crate) async fn assemble_original(
        &self,
        ffmpeg: &Path,
        source: ClipSource,
        segments: &[ClipRange],
    ) -> Result<Value, String> {
        if !(2..=6).contains(&segments.len()) {
            return Err("请提供 2 至 6 个需要拼接的片段。".into());
        }
        self.edit_original(ffmpeg, source, segments).await
    }

    pub(crate) async fn assemble_subtitled_original(
        &self,
        ffmpeg: &Path,
        source: ClipSource,
        segments: &[ClipRange],
    ) -> Result<Value, String> {
        if !(2..=6).contains(&segments.len()) {
            return Err("请提供 2 至 6 个需要拼接的片段。".into());
        }
        validate_ranges(segments)?;
        let bytes = self.read_original_video(&source).await?;
        let timed = self.read_original_subtitles(&source).await?;
        render_clip_with_subtitles(ffmpeg, &self.workspace, &bytes, segments, Some(&timed), false).await
    }

    pub(super) async fn read_original_subtitles(&self, source: &ClipSource) -> Result<Zeroizing<Vec<u8>>, String> {
        let artifact = {
            let state = self.state.lock().await;
            let account = state.stored.logical_key();
            state.stored.records.iter().find(|item| state.stored.enabled
                && item.id == source.receipt_id && item.is_video() && item.status == "completed"
                && item.args["source"] == account && item.args["scope"] == source.session_id
                && item.args["original_sha256"] == source.sha256)
                .and_then(|item| item.result.as_ref()?.get("artifact")).cloned()
                .ok_or("未找到当前原片已完成的转写，等待原收件转写完成后再剪辑。")?
        };
        let timed = video::verified_subtitles(&source.receipt_id, &self.workspace, &artifact)?;
        if !super::subtitles::is_timed_srt(&timed) {
            return Err("原转写字幕格式无效，请核对原收件。".into());
        }
        Ok(timed)
    }

    async fn edit_original(
        &self,
        ffmpeg: &Path,
        source: ClipSource,
        segments: &[ClipRange],
    ) -> Result<Value, String> {
        validate_ranges(segments)?;
        let bytes = self.read_original_video(&source).await?;
        render_clip(ffmpeg, &self.workspace, &bytes, segments).await
    }

    pub(super) async fn read_original_video(&self, source: &ClipSource) -> Result<Vec<u8>, String> {
        source.validate()?;
        let account_source = {
            let state = self.state.lock().await;
            let account_source = state.stored.logical_key();
            if !state.stored.enabled
                || !state.stored.records.iter().any(|item| {
                    item.id == source.receipt_id
                        && item.is_video()
                        && item.args["source"] == account_source
                        && item.args["scope"] == source.session_id
                        && item.args["original_sha256"] == source.sha256
                })
            {
                return Err("当前微信绑定无法核对这段原视频。".into());
            }
            account_source
        };
        let current_scope = (self.agent.as_ref().ok_or("共享助理尚未就绪。")?)(
            json!({"operation":"current_intake_scope","source_key":account_source}),
        )
        .await?;
        if current_scope["session_id"] != source.session_id {
            return Err("微信会话已切换，请在当前会话重新发原视频。".into());
        }
        let source_path = self
            .workspace
            .join("video-transcripts")
            .join(&source.receipt_id)
            .join("original.mp4");
        let bytes = artifact_bytes_limited(&source_path, &self.workspace, video::VIDEO_LIMIT)
            .map_err(|_| "原片尚未保存或已不可读取，请核对当前收件。")?;
        if !video::is_mp4(&bytes) || format!("{:x}", Sha256::digest(&bytes)) != source.sha256 {
            return Err("微信原视频与收件摘要不一致。".into());
        }
        Ok(bytes)
    }
}

async fn render_clip(
    ffmpeg: &Path,
    workspace: &Path,
    bytes: &[u8],
    segments: &[ClipRange],
) -> Result<Value, String> {
    render_clip_with_subtitles(ffmpeg, workspace, bytes, segments, None, false).await
}

pub(super) async fn render_clip_with_subtitles(
    ffmpeg: &Path,
    workspace: &Path,
    bytes: &[u8],
    segments: &[ClipRange],
    timed_subtitles: Option<&[u8]>,
    vertical: bool,
) -> Result<Value, String> {
    static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let _slot = SLOTS
        .try_acquire()
        .map_err(|_| "目前有两段视频正在剪辑，请稍后再试。")?;
    let root = workspace.join("clips");
    if let Err(error) = std::fs::create_dir(&root) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err("无法创建本机剪辑目录。".into());
        }
    }
    let info = std::fs::symlink_metadata(&root).map_err(|_| "无法核对本机剪辑目录。")?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err("本机剪辑路径必须是普通目录。".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if info.file_attributes() & 0x400 != 0 {
            return Err("本机剪辑目录含链接，未写入文件。".into());
        }
    }
    crate::control_protocol::private_directory(&root)
        .await
        .map_err(|_| "无法保护本机剪辑目录。")?;
    let scratch = tempfile::Builder::new()
        .prefix("clip-work-")
        .tempdir_in(&root)
        .map_err(|_| "无法创建本机剪辑工作目录。")?;
    let input = scratch.path().join("source.mp4");
    let output = scratch.path().join("clip.mp4");
    crate::control_protocol::publish_private(&input, bytes)
        .await
        .map_err(|_| "无法暂存已核验的剪辑输入。")?;
    let frame_counts = tokio::time::timeout(
        Duration::from_secs(70),
        encode_selection(ffmpeg, &input, &output, segments, vertical),
    )
    .await
    .map_err(|_| "本机剪辑超时，未登记成片。")??;
    let edited = artifact_bytes_limited(&output, &root, OUTPUT_LIMIT)?;
    if !video::is_mp4(&edited) {
        return Err("剪辑产物格式无法核对。".into());
    }
    let timed = timed_subtitles.map(|data| super::subtitles::rebase(data, segments, &frame_counts, FRAMES_PER_SECOND)).transpose()?;
    let directory = root.join(format!("clip-{}", uuid::Uuid::new_v4()));
    let publish = scratch.path().join("results");
    std::fs::create_dir(&publish).map_err(|_| "无法创建独立的剪辑结果目录。")?;
    let file = directory.join("clip.mp4");
    crate::control_protocol::publish_private(&publish.join("clip.mp4"), &edited)
        .await
        .map_err(|_| "未能保存剪辑成片。")?;
    let mut result = json!({
        "status":"succeeded","duration_ms":duration_ms(frame_counts.iter().sum()),
        "artifact":{"path":file,"bytes":edited.len(),"sha256":format!("{:x}",Sha256::digest(&edited)),"mime_type":"video/mp4"}
    });
    if let Some(timed) = timed {
        use super::subtitles;
        crate::control_protocol::publish_private(&publish.join(subtitles::NAME), timed.as_bytes())
            .await.map_err(|_| "未能保存与成片对齐的字幕。")?;
        result["subtitles"] = json!({"path":directory.join(subtitles::NAME),"bytes":timed.len(),"sha256":format!("{:x}",Sha256::digest(timed.as_bytes())),"mime_type":subtitles::MIME});
    }
    std::fs::rename(publish, directory).map_err(|_| "无法登记剪辑产物。")?;
    match segments {
        [range] => result["start_ms"] = json!(range.start_ms),
        _ => result["segments"] = json!(segments),
    }
    Ok(result)
}

pub(super) fn ffmpeg_process(ffmpeg: &Path) -> tokio::process::Command {
    let mut process = tokio::process::Command::new(ffmpeg);
    process.env_clear();
    if let Some(root) = std::env::var_os("SystemRoot") {
        process.env("SystemRoot", root);
    }
    process
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-xerror",
            "-n",
            "-nostats",
            "-progress",
            "pipe:1",
            "-max_alloc",
            "67108864",
            "-filter_threads",
            "1",
            "-protocol_whitelist",
            "file,pipe",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(0x08000000);
    process
}

pub(super) async fn frame_count(mut process: tokio::process::Command) -> Result<u64, String> {
    let mut child = process.spawn().map_err(|_| "本机剪辑程序无法启动。")?;
    #[cfg(windows)]
    let _lifetime = match crate::process_lifetime::ProcessLifetime::attach(&child) {
        Ok(job) => Some(job),
        Err(_) => {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill().await;
                return Err("无法保护本机剪辑进程，已停止生成成片。".into());
            }
            None
        }
    };
    #[cfg(all(test, windows))]
    recovery_tests::record_encoder(&child);
    let stdout = child.stdout.take().ok_or("本机剪辑结果通道不可用。")?;
    let mut progress = String::new();
    stdout
        .take(48 * 1024)
        .read_to_string(&mut progress)
        .await
        .map_err(|_| "暂时无法读取本机剪辑进度。")?;
    let exit = child
        .wait()
        .await
        .map_err(|_| "暂时无法核对剪辑程序的退出状态。")?;
    let frames = progress
        .lines()
        .filter_map(|line| line.strip_prefix("frame=")?.trim().parse::<u64>().ok())
        .last()
        .unwrap_or(0);
    if !exit.success() || progress.lines().last() != Some("progress=end") || frames == 0 {
        return Err("原视频无法生成完整的所选片段，请核对时间范围和视频格式。".into());
    }
    Ok(frames)
}

fn duration_ms(frames: u64) -> u64 {
    frames.saturating_mul(1000) / FRAMES_PER_SECOND
}

async fn encode_segment(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    range: ClipRange,
    intermediate: bool,
    vertical: bool,
) -> Result<u64, String> {
    let mut process = ffmpeg_process(ffmpeg);
    #[cfg(all(test, windows))]
    if recovery_tests::slow_encoding() {
        process.arg("-re");
    }
    let filter = if vertical {
        "scale=720:1280:force_original_aspect_ratio=decrease:force_divisible_by=2:reset_sar=1,pad=720:1280:(ow-iw)/2:(oh-ih)/2"
    } else {
        "scale='min(1280,iw)':'min(1280,ih)':force_original_aspect_ratio=decrease:force_divisible_by=2"
    };
    process.args(["-f", "mov", "-enable_drefs", "0", "-use_absolute_path", "0", "-threads", "2", "-i"])
        .arg(input).args(["-ss", &seconds(range.start_ms), "-t", &seconds(range.duration_ms), "-map", "0:V:0", "-map", "0:a:0?",
            "-sn", "-dn", "-map_metadata", "-1", "-map_chapters", "-1", "-vf", filter,
            "-r", "30", "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "veryfast", "-threads", "2", "-b:v", "2M", "-maxrate", "4M", "-bufsize", "8M"]);
    if intermediate {
        // PCM across joins avoids encoder delay on every audio segment. Optional
        // mapping leaves sources without audio silent without synthesizing a track.
        process.args([
            "-bf",
            "0",
            "-c:a",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-af",
            "aresample=48000:async=1:first_pts=0,apad",
            "-f",
            "mov",
        ]);
    } else {
        process.args(["-c:a", "aac", "-movflags", "+faststart", "-f", "mp4"]);
    }
    #[cfg(all(test, windows))]
    if recovery_tests::slow_encoding() {
        process.args(["-flush_packets", "1"]);
    }
    process.args(["-fs", &OUTPUT_LIMIT.to_string()]).arg(output);
    let frames = frame_count(process).await?;
    if duration_ms(frames).abs_diff(range.duration_ms) > 50 {
        return Err("视频长度不足，未登记不完整的剪辑。请核对时间范围。".into());
    }
    Ok(frames)
}

async fn encode_selection(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    segments: &[ClipRange],
    vertical: bool,
) -> Result<Vec<u64>, String> {
    if let [range] = segments {
        return Ok(vec![encode_segment(ffmpeg, input, output, *range, false, vertical).await?]);
    }
    let scratch = output.parent().ok_or("剪辑工作目录不可用。")?;
    let mut playlist = String::from("ffconcat version 1.0\n");
    let mut frame_counts = Vec::with_capacity(segments.len());
    for (index, range) in segments.iter().enumerate() {
        let filename = format!("part-{index}.mov");
        let frames = encode_segment(ffmpeg, input, &scratch.join(&filename), *range, true, vertical).await?;
        frame_counts.push(frames);
        let length = frames as f64 / FRAMES_PER_SECOND as f64;
        playlist.push_str(&format!("file {filename}\nduration {length:.6}\n"));
    }
    let list = scratch.join("parts.ffconcat");
    crate::control_protocol::publish_private(&list, playlist.as_bytes())
        .await
        .map_err(|_| "无法保存本机片段顺序。")?;
    let mut process = ffmpeg_process(ffmpeg);
    process
        .args(["-f", "concat", "-safe", "1", "-i"])
        .arg(list)
        .args([
            "-map",
            "0:V:0",
            "-map",
            "0:a:0?",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-af",
            "aresample=48000:async=1:first_pts=0",
            "-fs",
            &OUTPUT_LIMIT.to_string(),
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
        ])
        .arg(output);
    let frames = frame_count(process).await?;
    if frames != frame_counts.iter().sum::<u64>() {
        return Err("拼接成片少于或多于已剪出的画面，未登记结果。".into());
    }
    Ok(frame_counts)
}

#[cfg(all(test, windows))]
#[path = "wechat_clip_recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
mod tests {
    use super::{
        is_edit_request, recent_clip_intent, render_clip, validate_ranges, ClipRange, RecentClip,
    };
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn assembly_ranges_are_checked_as_one_action() {
        let check = |start_ms, duration_ms| ClipRange {
            start_ms,
            duration_ms,
        };
        assert!(validate_ranges(&[check(119_800, 200), check(10, 400)]).is_ok());
        assert!(validate_ranges(&[check(119_801, 200)]).is_err());
        assert!(validate_ranges(&[check(u64::MAX, 300)]).is_err());
        assert!(validate_ranges(&[check(0, 199)]).is_err());
        assert!(validate_ranges(&[check(0, 20_000), check(40_000, 10_001)]).is_err());
    }

    #[tokio::test]
    #[ignore = "explicit pinned FFmpeg for isolated synthetic media required"]
    async fn assembly_without_an_audio_track_remains_silent_and_single_cut_stays_compatible() {
        let ffmpeg = PathBuf::from(std::env::var_os("HOMEBASE_VIDEO_FFMPEG").unwrap());
        assert!(ffmpeg.is_absolute());
        let lab = tempfile::tempdir().unwrap();
        let path = lab.path().join("silent.mp4");
        assert!(std::process::Command::new(&ffmpeg)
            .args([
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=red:s=160x120:d=2:r=30",
                "-c:v",
                "libx264",
                "-an"
            ])
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success());
        let bytes = std::fs::read(path).unwrap();
        let a = ClipRange {
            start_ms: 900,
            duration_ms: 400,
        };
        let b = ClipRange {
            start_ms: 0,
            duration_ms: 300,
        };
        let stitched = render_clip(&ffmpeg, lab.path(), &bytes, &[a, b])
            .await
            .unwrap();
        assert_eq!(stitched["segments"], json!([a, b]));
        assert_eq!(stitched["duration_ms"], 700);
        let single = render_clip(&ffmpeg, lab.path(), &bytes, &[b])
            .await
            .unwrap();
        assert!(single.get("segments").is_none());
        assert_eq!(single["start_ms"], 0);
        assert_eq!(single["duration_ms"], 300);
        for result in [stitched, single] {
            let movie = result["artifact"]["path"].as_str().unwrap();
            assert!(std::process::Command::new(&ffmpeg)
                .args([
                    "-loglevel",
                    "error",
                    "-xerror",
                    "-i",
                    movie,
                    "-map",
                    "0:V:0",
                    "-f",
                    "null",
                    "-"
                ])
                .output()
                .unwrap()
                .status
                .success());
            assert!(!std::process::Command::new(&ffmpeg)
                .args([
                    "-loglevel",
                    "error",
                    "-i",
                    movie,
                    "-map",
                    "0:a:0",
                    "-f",
                    "null",
                    "-"
                ])
                .output()
                .unwrap()
                .status
                .success());
        }
    }

    #[test]
    fn only_natural_edit_requests_leave_the_transcript_route() {
        for text in ["帮我按之前的方式剪辑", "请截取前两秒", "照上次的方式剪一下"]
        {
            assert!(is_edit_request(text), "{text}");
        }
        for text in [
            "/问视频 wx-1 怎么剪辑",
            "这段是怎么剪的吗",
            "请解释这段视频的剪辑手法",
            "请不要剪辑",
            "请把这段视频只作参考，照它的节奏剪另一段。",
            "请参考这段视频的剪辑，帮我总结一下。",
            "要剪辑吗？",
        ] {
            assert!(!is_edit_request(text), "{text}");
        }
    }

    #[test]
    fn recent_clip_distinguishes_explicit_and_implicit_sources() {
        for text in [
            "把我刚发的视频按之前的方式剪一下",
            "能帮我把刚才发给你的视频剪一下吗？",
            "刚发的视频按上次的方式剪一下",
            "请截取上一段视频的前两秒",
            "把刚发的视频两段按顺序拼接",
            "把刚发的视频第 5 秒到第 7 秒、再接第 1 秒到第 3 秒",
            "能不能帮我把刚发那段视频照老办法剪一下？",
            "麻烦把刚刚发给你的那段原片剪一下",
        ] {
            assert_eq!(recent_clip_intent(text), Some(RecentClip::Latest), "{text}");
        }
        assert_eq!(
            recent_clip_intent("帮我剪一下这个视频"),
            Some(RecentClip::Unique)
        );
        for text in [
            "帮我按之前的方式剪辑",
            "帮我按照之前的方式完成一下剪辑",
            "照旧剪一下吧！",
            "能帮我照上次的方式剪一下吗？",
            "帮我剪一下，还是按上次的做法。",
            "剪片还是照上回的办法就行",
            "可以帮我沿用上次的剪辑流程吗？",
            "麻烦按平时的方式剪一下",
            "就照咱们上回的做法剪一下吧。",
            "照旧，剪一下。",
            "麻烦按上次的方式，帮我剪一下。",
        ] {
            assert_eq!(recent_clip_intent(text), Some(RecentClip::ImplicitUnique), "{text}");
        }
        for text in [
            "帮我按之前的方式剪辑桌面上的视频",
            "照旧，剪一下电脑里的视频",
            "照旧，先聊聊怎么剪一下",
            "帮我剪辑桌面上的视频",
            "按上次的方式剪一下昨天发来的素材",
            "帮我剪一下昨天的素材，还是按上次的做法",
            "剪一下，别按上次的做法",
            "帮我剪一下，还是按上次的做法，再上传到群里",
            "先讨论帮我剪一下，还是按上次的做法",
            "按之前的方式剪辑是什么意思？",
            "请解释怎么剪辑刚发的视频",
            "请不要剪辑刚发的视频",
            "刚发的视频剪辑好了吗？",
            "帮我看看刚发的视频剪辑怎么样？",
            "帮我剪辑刚发的视频的前一个",
            "请解释刚发那段视频是怎么剪的",
            "刚发那段视频不用剪",
            "就照咱们上回的做法剪一下电脑里的视频",
            "把刚发那段素材照老办法剪一下",
            "请把刚刚发给你的那段原片作为参考，剪一下电脑里的视频",
            "就照刚发的视频的剪辑节奏，剪一下电脑里的视频",
            "请截取刚发的倒数第三段视频的前两秒",
            "请把刚发的视频和倒数第二段视频合并",
            "请把倒数第二段视频和倒数第三段视频合并",
        ] {
            assert_eq!(recent_clip_intent(text), None, "{text}");
        }
        for text in [
            "把倒数第二段视频照旧剪一下。",
            "请剪一下倒数第 2 段原片",
            "倒数第二条视频剪一下",
        ] {
            assert_eq!(recent_clip_intent(text), Some(RecentClip::Previous), "{text}");
        }
    }
}
