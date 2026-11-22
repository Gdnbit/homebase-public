//! Transcript jobs for a native WeChat MP4, bound to the original receipt.
use super::*;

pub(crate) const VIDEO_LIMIT: usize = 10 * 1024 * 1024;
pub(crate) const VIDEO_SECONDS: usize = 120;
pub(crate) const VOICE_LIMIT: usize = 2 * 1024 * 1024;
pub(crate) const VOICE_SECONDS: usize = 60;
pub(crate) const TRANSCRIPT_LIMIT: usize = 256 * 1024;
const COMMAND: &str = "wechat_video_transcript";
const RECENT_CLIP_WINDOW: chrono::TimeDelta = chrono::TimeDelta::minutes(15);
const RECENT_CLIP_MISSING: &str = "当前对话最近没有可选的原视频。把 MP4 发给我，或引用你想剪的那一段。";
const PREVIOUS_CLIP_MISSING: &str = "当前对话最近没有倒数第二段原视频可选。请引用想剪的那段，或重新发送原片。";

fn clip_action(receipt: &Record, text: &str) -> Result<Action, String> {
    if text.len() > 8000 {
        return Err("请把剪辑要求缩短到 8000 字节以内。".into());
    }
    if receipt.args["original_sha256"].as_str().is_none() {
        return Err(if receipt.status == "running" && receipt.args["phase"] == "receiving" {
            "这段原片还在接收，等我收完后再叫我剪。"
        } else {
            "这段原片没有接收成功或缺少校验记录，请在当前对话重新发一次 MP4。"
        }.into());
    }
    Ok(Action::Call("assistant".into(), json!({"text":text,"video_clip_receipt":receipt.id})))
}
pub(super) const MAX_REPOSITORY_REFS: usize = 5;
pub(super) const REPOSITORY_ADAPTER_REQUIRED: &str = "这个仓库没有找到可自动封装的唯一 npm 命令行入口，需要先适配。";

pub(crate) enum VideoRequest {
    Start {
        id: String,
        bytes: Zeroizing<Vec<u8>>,
    },
    Observe {
        id: String,
    },
    StartVoice {
        id: String,
        bytes: Zeroizing<Vec<u8>>,
    },
    ObserveVoice {
        id: String,
    },
}

pub(crate) enum VideoResult {
    Running,
    Unknown,
    Failed(String),
    Ready { preview: String, artifact: Value, repositories: RepositoryHints },
}

#[derive(Clone, Default)]
pub(crate) struct RepositoryHints {
    pub(super) links: Vec<String>,
    pub(super) names: Vec<String>,
}

impl RepositoryHints {
    pub(crate) fn from_transcript(text: &str) -> Self {
        let links = repository_refs(text);
        let names = repository_search::mentioned_names(text, &links);
        Self { links, names }
    }
}

pub(super) fn repository_ordinal(index: &str) -> Option<usize> {
    match index {
        "一" => Some(1),
        "二" => Some(2),
        "三" => Some(3),
        "四" => Some(4),
        "五" => Some(5),
        _ => index.parse().ok(),
    }.filter(|number| (1..=MAX_REPOSITORY_REFS).contains(number))
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum MediaCompletion {
    Automatic,
    ExplicitQuery,
}

pub(crate) type VideoInbox =
    Arc<dyn Fn(VideoRequest) -> Pin<Box<dyn Future<Output = VideoResult> + Send>> + Send + Sync>;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum MediaKind {
    Video,
    Voice,
}

impl MediaKind {
    fn transcript_folder(self) -> &'static str {
        match self {
            Self::Video => "video-transcripts",
            Self::Voice => "voice-transcripts",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Video => "视频",
            Self::Voice => "语音",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Video => COMMAND,
            Self::Voice => voice::COMMAND,
        }
    }
}

pub(super) struct MediaAttachment {
    kind: MediaKind,
    media: cdn::CdnReference,
    cipher_bytes: Option<usize>,
    plain_bytes: Option<usize>,
    md5: Option<String>,
    item_id: Option<String>,
}

pub(super) fn inbound(message: &Value) -> Option<Result<MediaAttachment, String>> {
    let items = message["item_list"].as_array()?;
    let item = items.iter().find(|item| is_video_item(item))?;
    if items
        .iter()
        .filter(|item| matches!(item["type"].as_u64(), Some(2..=5)))
        .count()
        != 1
        || items.iter().any(|item| {
            item["type"] == 1
                && item["text_item"]["text"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
        })
    {
        return Some(Err("请单独发送一段 MP4 视频，大小不超过 10 MiB。".into()));
    }
    let video = if item["type"] == 5 {
        parse_video(&item["video_item"])
    } else {
        parse_file(&item["file_item"])
    };
    Some(video.map(|mut video| {
        video.item_id = identity::reference_id(&item["msg_id"]);
        video
    }))
}

fn is_video_item(item: &Value) -> bool {
    item["type"] == 5
        || item["type"] == 4
            && item["file_item"]["file_name"]
                .as_str()
                .and_then(|name| name.rsplit_once('.'))
                .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("mp4"))
}

fn parse_video(value: &Value) -> Result<MediaAttachment, String> {
    let cipher_bytes = match value.get("video_size") {
        None | Some(Value::Null) => None,
        Some(size) => Some(
            size.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n >= 16 && *n <= VIDEO_LIMIT + 16 && *n % 16 == 0)
                .ok_or("微信视频大小无效或超过 10 MiB，未下载。")?,
        ),
    };
    Ok(MediaAttachment {
        kind: MediaKind::Video,
        media: cdn::reference(&value["media"], "微信视频")?,
        cipher_bytes,
        plain_bytes: None,
        md5: parse_md5(value.get("video_md5"))?,
        item_id: None,
    })
}

fn parse_file(value: &Value) -> Result<MediaAttachment, String> {
    let name = value["file_name"].as_str().unwrap_or_default();
    if name.len() > 255
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
        || name.rsplit_once('.').is_none_or(|(stem, _)| stem.trim().is_empty())
    {
        return Err("微信 MP4 文件名无效，未下载。".into());
    }
    // FileItem.len counts plaintext bytes. The encrypted CDN transfer is padded.
    let plain_bytes = match value.get("len") {
        None | Some(Value::Null) => None,
        Some(size) => Some(
            size.as_str()
                .filter(|s| s.bytes().all(|byte| byte.is_ascii_digit()))
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| (1..=VIDEO_LIMIT).contains(n))
                .ok_or("微信 MP4 文件长度无效或超过 10 MiB，未下载。")?,
        ),
    };
    Ok(MediaAttachment {
        kind: MediaKind::Video,
        media: cdn::reference(&value["media"], "微信 MP4 文件")?,
        cipher_bytes: None,
        plain_bytes,
        md5: parse_md5(value.get("md5"))?,
        item_id: None,
    })
}

fn parse_md5(value: Option<&Value>) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Ok(None),
        Some(Value::String(value))
            if value.len() == 32 && value.bytes().all(|n| n.is_ascii_hexdigit()) =>
        {
            Ok(Some(value.to_ascii_lowercase()))
        }
        _ => Err("微信视频校验值无效，未下载。".into()),
    }
}

pub(super) fn voice_attachment(item: &Value) -> Result<MediaAttachment, String> {
    let value = &item["voice_item"];
    if value.get("playtime").is_some_and(|val| {
        !val.is_null()
            && !val
                .as_u64()
                .is_some_and(|ms| (1..=(VOICE_SECONDS as u64 * 1000)).contains(&ms))
    }) {
        return Err("微信语音时长无效或超过 60 秒。".into());
    }
    Ok(MediaAttachment {
        kind: MediaKind::Voice,
        media: cdn::reference(&value["media"], "微信语音")?,
        cipher_bytes: None,
        plain_bytes: None,
        md5: None,
        item_id: identity::reference_id(&item["msg_id"]),
    })
}

pub(crate) fn is_silk(bytes: &[u8]) -> bool {
    bytes.starts_with(b"#!SILK_V3") || bytes.starts_with(b"\x02#!SILK_V3")
}

pub(crate) fn is_mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && bytes.get(4..8) == Some(b"ftyp")
}

pub(crate) async fn extract_audio(ffmpeg: &Path, source: &Path, wave: &Path) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    let mut process = tokio::process::Command::new(ffmpeg);
    process
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "error",
            "-n",
            "-protocol_whitelist",
            "file,pipe",
            "-f",
            "mov",
            "-enable_drefs",
            "0",
            "-use_absolute_path",
            "0",
            "-threads",
            "2",
            "-i",
        ])
        .arg(source)
        .args(["-map", "0:a:0", "-vn", "-sn", "-dn", "-t"])
        .arg(VIDEO_SECONDS.to_string())
        .args(["-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le", "-f", "wav"])
        .arg(wave)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    process.creation_flags(0x08000000);
    let mut child = process.spawn().map_err(|_| "本机视频解码器无法启动。")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("本机视频解码器诊断通道不可用。")?;
    let mut diagnostic = Vec::new();
    let exit = tokio::time::timeout(Duration::from_secs(35), async {
        stderr.take(12 * 1024).read_to_end(&mut diagnostic).await?;
        child.wait().await
    })
    .await
    .map_err(|_| "视频音轨读取超时。")?
    .map_err(|_| "本机视频解码器结果无法核对。")?;
    if !exit.success() {
        let diagnostic = String::from_utf8_lossy(&diagnostic);
        return Err(if diagnostic.contains("matches no streams") {
            "这段视频没有可转写的音轨。"
        } else {
            "未能读取视频音轨，请检查 MP4 是否完整。"
        }
        .into());
    }
    let size = std::fs::metadata(wave)
        .map_err(|_| "未生成可读音轨。")?
        .len();
    if size <= 78 || size > (VIDEO_SECONDS * 32_000 + 4096) as u64 {
        return Err("这段视频没有可转写的音轨，或音轨超出输入上限。".into());
    }
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
#[ignore = "explicit pinned FFmpeg and synthetic media fixtures required"]
async fn wechat_video_real_decoder_extracts_audio_and_distinguishes_missing_track() {
    let ffmpeg = PathBuf::from(std::env::var_os("HOMEBASE_VIDEO_FFMPEG").unwrap());
    let source = PathBuf::from(std::env::var_os("HOMEBASE_VIDEO_FIXTURE").unwrap());
    let silent = PathBuf::from(std::env::var_os("HOMEBASE_VIDEO_NO_TRACK_FIXTURE").unwrap());
    assert!(ffmpeg.is_absolute() && source.is_absolute() && silent.is_absolute());
    let directory = tempfile::tempdir().unwrap();
    let wave = directory.path().join("decoded.wav");
    extract_audio(&ffmpeg, &source, &wave).await.unwrap();
    let bytes = std::fs::read(wave).unwrap();
    assert!(bytes.starts_with(b"RIFF"));
    assert_eq!(&bytes[8..12], b"WAVE");
    assert!(bytes.len() > 32_000);
    let missing = directory.path().join("no-audio.wav");
    assert!(extract_audio(&ffmpeg, &silent, &missing)
        .await
        .unwrap_err()
        .contains("没有可转写的音轨"));
    assert!(!missing.exists());
}

pub(crate) fn receipt_id(id: &str) -> bool {
    id.strip_prefix("wx-").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

impl Record {
    pub(super) fn is_video(&self) -> bool {
        self.command == COMMAND
    }

    pub(super) fn local_media_kind(&self) -> Option<MediaKind> {
        if self.is_video() {
            Some(MediaKind::Video)
        } else if self.command == voice::COMMAND && self.args["phase"].is_string() {
            Some(MediaKind::Voice)
        } else {
            None
        }
    }

    pub(super) fn pending_media_completion(&self) -> bool {
        self.delivery == "pending"
            && matches!(self.status.as_str(), "completed" | "failed" | "unknown")
            && self.local_media_kind().is_some()
            && self.args[COMPLETION_REPLY] == true
            && self.result.as_ref().is_some_and(|value| value["text"].is_string())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VideoQuestion {
    pub receipt_id: String,
    pub scope: String,
    pub sha256: String,
    path: PathBuf,
    workspace: PathBuf,
}

impl VideoQuestion {
    pub(crate) fn excerpt(&self) -> Result<crate::assistant::TextAttachment, String> {
        let bytes = verified_transcript(MediaKind::Video, &self.receipt_id, &self.workspace, &self.path, &self.sha256)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| "视频文字稿格式无法核对。")?
            .trim();
        if text.is_empty() {
            return Err("没有可用于提问的视频语音文字。".into());
        }
        let mut end = text.len().min(6000);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let description = if end < text.len() {
            "已截断"
        } else {
            "本次转写全文"
        };
        Ok(crate::assistant::TextAttachment {
            name: "原视频语音文字稿.txt".into(),
            content: format!(
                "视频第一条音轨，最多前 {VIDEO_SECONDS} 秒。{description}。\n{}",
                &text[..end]
            ),
            document: None,
        })
    }
}

pub(super) fn verified_transcript(
    kind: MediaKind,
    id: &str,
    workspace: &Path,
    path: &Path,
    expected: &str,
) -> Result<Zeroizing<Vec<u8>>, String> {
    verified_media_export(kind, id, workspace, path, expected, Path::new("exports/transcript.txt"))
}

pub(super) fn verified_subtitles(id: &str, workspace: &Path, artifact: &Value) -> Result<Zeroizing<Vec<u8>>, String> {
    let expected = artifact["timed_subtitle_sha256"].as_str().ok_or("原转写回执没有已核验的时间字幕。")?;
    let transcript = artifact["path"].as_str().map(Path::new).ok_or("原文字稿路径无法核对。")?;
    if !transcript.ends_with("exports/transcript.txt") {
        return Err("原文字稿路径无法核对。".into());
    }
    let kind = artifact.get("timed_subtitle_kind").map_or(Some("export"), Value::as_str)
        .ok_or("原视频字幕来源无效。")?;
    let suffix = super::subtitles::source_suffix(kind).ok_or("原视频字幕来源无效。")?;
    let task = transcript.parent().and_then(Path::parent).ok_or("原文字稿路径无法核对。")?;
    verified_media_export(MediaKind::Video, id, workspace, &task.join(suffix), expected, suffix)
}

fn verified_media_export(
    kind: MediaKind,
    id: &str,
    workspace: &Path,
    path: &Path,
    expected: &str,
    suffix: &Path,
) -> Result<Zeroizing<Vec<u8>>, String> {
    if !receipt_id(id) {
        return Err(format!("原{}收件编号无法核对。", kind.label()));
    }
    let relative_folder = Path::new(kind.transcript_folder()).join(id).join("transcripts");
    let folder = workspace.join(&relative_folder);
    // ASR records absolute paths after canonicalizing its output root (verbatim on Windows).
    let relative = match path.strip_prefix(&folder) {
        Ok(relative) => relative,
        Err(_) => {
            let canonical = workspace
                .canonicalize()
                .map_err(|_| format!("原{}工作目录不存在。", kind.label()))?;
            path.strip_prefix(canonical.join(&relative_folder))
                .map_err(|_| format!("文字稿不属于原{}转写目录。", kind.label()))?
        }
    };
    let mut parts = relative.components();
    if !matches!(parts.next(), Some(std::path::Component::Normal(_))) || parts.as_path() != suffix {
        return Err(format!("文字稿不属于原{}转写目录。", kind.label()));
    }
    let bytes = Zeroizing::new(artifact_bytes_limited(
        &folder.join(relative),
        workspace,
        TRANSCRIPT_LIMIT,
    )?);
    if expected != format!("{:x}", Sha256::digest(&*bytes)) {
        return Err(if suffix.extension().and_then(|s| s.to_str()) == Some("srt") { "原视频字幕摘要不一致。".into() } else { format!("{}文字稿摘要不一致。", kind.label()) });
    }
    Ok(bytes)
}

pub(crate) fn repository_refs(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for token in text.split(|c: char| c.is_whitespace() || matches!(c,
        '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | '`' | ',' | ';' |
        '（' | '）' | '【' | '】' | '「' | '」' | '“' | '”' | '《' | '》' |
        '，' | '。' | '、' | '；' | '：' | '！' | '？')) {
        let rest = match token.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
            Some(_) => continue,
            None => token,
        };
        let Some((host, path)) = rest.split_once('/') else { continue };
        if !host.eq_ignore_ascii_case("github.com") {
            continue;
        }
        let Some(url) = github_repository_url(path) else { continue };
        if !found.iter().any(|previous| previous.eq_ignore_ascii_case(&url)) {
            found.push(url);
            if found.len() == MAX_REPOSITORY_REFS {
                break;
            }
        }
    }
    found
}

fn github_repository_url(path: &str) -> Option<String> {
    let path = path.strip_suffix('/').unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    // Match the path grammar used by repository() in repo-capabilities-analysis.mjs.
    let valid_part = |part: &str| {
        (1..=100).contains(&part.len()) && !part.contains("..")
            && part.as_bytes()[0].is_ascii_alphanumeric()
            && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    (valid_part(owner) && valid_part(name)).then(|| format!("https://github.com/{owner}/{name}"))
}

fn analyze_repository(repository: String, request_id: &str) -> Action {
    Action::Call(
        "repo_project_analyze".into(),
        json!({"request":{"repository_url":repository,"request_id":request_id}}),
    )
}

pub(super) fn analyze_repository_link(text: &str, request_id: &str) -> Option<Result<Action, String>> {
    let text = text.trim().trim_end_matches(['。', '！', '!']);
    let address = ["/分析仓库", "帮我分析仓库", "分析仓库"]
        .into_iter()
        .find_map(|prefix| text.strip_prefix(prefix).filter(|rest| {
            rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || matches!(c, ':' | '：'))
        }))
        .map(|rest| {
            let rest = rest.trim_start();
            rest.strip_prefix([':', '：']).unwrap_or(rest).trim()
        })
        .or_else(|| text.strip_prefix('把')?.strip_suffix("加入能力库").map(str::trim)
            .filter(|address| address.contains("://")))?;
    let canonical = address.split_once("://").and_then(|(scheme, rest)| {
        let (host, path) = rest.split_once('/')?;
        if scheme.eq_ignore_ascii_case("https") && ["github.com", "deepwiki.com", "gitingest.com", "gitmcp.io"]
            .iter().any(|allowed| host.eq_ignore_ascii_case(allowed)) {
            github_repository_url(path)
        } else {
            None
        }
    });
    Some(canonical.map(|url| analyze_repository(url, request_id)).ok_or_else(|| {
        "请发送一个公开仓库主页，例如「分析仓库 https://github.com/markedjs/marked」。链接中请去掉查询参数或账号信息。".into()
    }))
}

fn append_repository_refs(text: &mut String, refs: Vec<String>) {
    if !refs.is_empty() {
        text.push_str("\n\n文字稿里提到的 GitHub 仓库链接：\n");
        for (index, repository) in refs.iter().enumerate() {
            text.push_str(&format!("{}. {repository}\n", index + 1));
        }
        text.push_str("可引用原视频回复「分析第 1 个项目」，或发 /分析项目 原收件编号 1。");
    }
}

impl Bridge {
    pub(super) async fn repository_install_request(
        &self, message: &Value, account: &Stored, generation: u64, text: &str, request_id: &str,
    ) -> Option<Result<Action, String>> {
        let text = text.trim();
        let usage = "请引用仓库分析的回复说「安装这个项目」，或发送 /安装项目 原分析任务编号。";
        let (target, examples) = if let Some(rest) = text.strip_prefix("/安装项目")
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            if text.len() > 16_000 {
                return Some(Err("安装请求超过 16 KB，未准备安装。".into()));
            }
            let rest = rest.trim();
            let (target, payload) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if !receipt_id(target) {
                return Some(Err(usage.into()));
            }
            let examples = if payload.trim().is_empty() {
                None
            } else {
                let Ok(examples) = serde_json::from_str::<Vec<Value>>(payload) else {
                    return Some(Err("验收示例需要 JSON 数组，每条包含 input.stdin 和 contains。".into()));
                };
                if let Err(error) = crate::assistant::repository_install::validate_examples(&examples) {
                    return Some(Err(error));
                }
                Some(examples)
            };
            (target.to_owned(), examples)
        } else {
            if !matches!(text.trim_end_matches(['。', '！', '!']), "安装这个项目" | "安装这项能力" | "安装这个仓库") {
                return None;
            }
            match self.quoted_task(account, message, true).await {
                Some(Ok(Action::Query(id, _, false))) if receipt_id(&id) => (id, None),
                _ => return Some(Err(usage.into())),
            }
        };
        Some(self.repository_install_action(&target, generation, request_id, examples).await)
    }

    async fn repository_install_action(
        &self, receipt: &str, generation: u64, request_id: &str, examples: Option<Vec<Value>>,
    ) -> Result<Action, String> {
        let (job, repository) = {
            let state = self.state.lock().await;
            let record = state.stored.records.iter().find(|record| {
                record.id == receipt && record.command == "repo_project_analyze"
            }).ok_or("请引用仓库分析的回复；视频或其他任务结果不能用来确认仓库安装。")?;
            let job = record.job_id.clone().ok_or("原仓库分析尚未受理，请先查询原任务。")?;
            let repository = record.args["request"]["repository_url"].as_str()
                .ok_or("原仓库地址无法核对。")?.to_owned();
            (job, repository)
        };
        let observed = self.observe_query(generation, self.request(
            "jobs.get", None, json!({}), request_id, Some(&job)
        )).await.ok_or("微信连接已变化，未准备安装。")?
            .map_err(|_| "原仓库分析结果暂时读不到，请先查询原任务。")?;
        if observed["id"] != job || observed["command"] != "repo_project_analyze"
            || observed["request_id"] != receipt
        {
            return Err("原仓库分析回执无法核对，未准备安装。".into());
        }
        if matches!(observed["status"].as_str(), Some("running" | "queued")) {
            return Err("仓库还在分析，请等结果出来后再引用回复。".into());
        }
        if observed["status"] != "succeeded" || observed["result"]["status"] != "succeeded" {
            return Err("目前还没有可确认的仓库分析结果，请先查询原任务。".into());
        }
        let project = &observed["result"]["project"];
        if project["repository"].as_str().is_some_and(|url| !url.eq_ignore_ascii_case(&repository)) {
            return Err("仓库分析返回的地址和原仓库不一致，未准备安装。".into());
        }
        match project["phase"].as_str() {
            Some("registered") => return Err("这个仓库已经加入能力库，可以直接查看和使用。".into()),
            Some("needs_adapter") => return Err(REPOSITORY_ADAPTER_REQUIRED.into()),
            Some("analyzed") => (),
            _ => return Err("原项目还未到可安装阶段，请先查询分析结果。".into()),
        }
        let intent = crate::assistant::repository_install::Intent {
            repository,
            project_id: project["id"].as_str().unwrap_or_default().into(),
            revision: project["revision"].as_u64().unwrap_or_default(),
            analysis_receipt: receipt.into(),
            examples,
        };
        intent.validate()?;
        Ok(Action::Call("assistant".into(), json!({"repository_install":intent})))
    }

    pub(super) async fn video_repository_analysis(
        &self,
        message: &Value,
        account: &Stored,
        generation: u64,
        text: &str,
        request_id: &str,
    ) -> Option<Result<Action, String>> {
        let text = text.trim();
        let usage = "请引用视频或搜索回复说「分析第 1 个项目」，也可发送 /分析项目 原任务编号 序号。";
        let (target, index) = if let Some(rest) = text.strip_prefix("/分析项目")
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        {
            let fields: Vec<_> = rest.split_whitespace().take(3).collect();
            let [id, index] = fields.as_slice() else { return Some(Err(usage.into())) };
            if !receipt_id(id) {
                return Some(Err(usage.into()));
            }
            ((*id).to_owned(), *index)
        } else {
            let index = text.trim_end_matches(['。', '！', '!'])
                .strip_prefix("分析第")?.strip_suffix("个项目")?.trim();
            let target = match self.quoted_task(account, message, true).await {
                Some(Ok(Action::Query(id, _, false))) if receipt_id(&id) => id,
                Some(Err(error)) => return Some(Err(error)),
                _ => return Some(Err(usage.into())),
            };
            (target, index)
        };
        let Some(ordinal) = repository_ordinal(index) else {
            return Some(Err("请使用原回复里列出的项目序号。".into()));
        };
        enum Reference { Search(Option<String>), Video(String, Option<Value>) }
        let source = account.logical_key();
        let saved = {
            let state = self.state.lock().await;
            state.stored.records.iter().find(|record| {
                record.id == target && (record.command == repository_search::COMMAND
                    || record.is_video() && record.args["source"] == source)
            }).map(|record| if record.command == repository_search::COMMAND {
                Reference::Search(record.job_id.clone())
            } else {
                Reference::Video(record.status.clone(), record.result.clone())
            })
        };
        let repository = match saved {
            Some(Reference::Search(Some(job))) => match self.search_candidate(&target, &job, ordinal, generation).await {
                Ok(url) => url,
                Err(error) => return Some(Err(error)),
            },
            Some(Reference::Search(None)) => return Some(Err("原搜索还未受理，请稍后按原编号查询。".into())),
            Some(Reference::Video(status, result)) => {
                let Some(result) = result.filter(|_| status == "completed") else {
                    return Some(Err("原视频文字稿还未核对完成，请先查询原收件。".into()));
                };
                let repositories = saved_repository_refs(&result)
                    .or_else(|| legacy_repository_hints(&target, &self.workspace, &result["artifact"]).map(|hints| hints.links));
                let Some(url) = repositories.and_then(|refs| refs.into_iter().nth(ordinal - 1)) else {
                    return Some(Err("原视频结果里暂时找不到这个项目序号，请重新查询原收件。".into()));
                };
                url
            }
            None => return Some(Err("没有找到当前微信来源的原视频或原搜索。".into())),
        };
        if repository_refs(&repository).first() != Some(&repository) {
            return Some(Err("原仓库主页地址无法核对。".into()));
        }
        Some(Ok(analyze_repository(repository, request_id)))
    }

    pub(super) async fn video_question(
        &self,
        message: &Value,
        account: &Stored,
        text: &str,
    ) -> Option<Result<Action, String>> {
        let text = text.trim();
        let explicit = text
            .strip_prefix("/问视频")
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
        let (target, question) = match explicit {
            Some(rest) => {
                let Some((id, question)) = rest.trim().split_once(char::is_whitespace) else {
                    return Some(Err(
                        "请发送 /问视频 收件编号 你的问题，或引用原视频后提问。".into(),
                    ));
                };
                if !receipt_id(id) {
                    return Some(Err("请使用微信返回的原视频收件编号。".into()));
                }
                (Some(id), question.trim())
            }
            None if text.is_empty() || text.starts_with('/') => return None,
            None => (None, text),
        };
        let refs = message["item_list"]
            .as_array()?
            .iter()
            .filter(|item| item["type"] == 1)
            .filter_map(|item| item.get("ref_msg").filter(|v| v.is_object()))
            .collect::<Vec<_>>();
        if target.is_none() && refs.is_empty() {
            return None;
        }
        let source = account.logical_key();
        let state = self.state.lock().await;
        let records = state
            .stored
            .records
            .iter()
            .filter(|item| item.is_video() && item.args["source"] == source);
        let mut chosen = None;
        if let Some(id) = target {
            chosen = records.clone().find(|item| item.id == id);
        } else {
            let mut mentions_video = false;
            let mut other_target = false;
            for reference in refs {
                let item = &reference["message_item"];
                mentions_video |= is_video_item(item);
                for key in [&reference["svr_id"], &item["msg_id"]]
                    .into_iter()
                    .filter_map(identity::reference_id)
                {
                    let legacy = identity::legacy_receipt_id(account, &key);
                    let stable = identity::source_key(&source, &key);
                    for record in state.stored.records.iter().filter(|record| {
                        identity::matches_task_reference(record, &source, &legacy, &stable)
                    }) {
                        let video = if record.is_video() {
                            Some(record)
                        } else {
                            record.numbered_query().and_then(|(id, _)| {
                                records.clone().find(|video| video.id == id)
                            })
                        };
                        let Some(record) = video else {
                            other_target = true;
                            continue;
                        };
                        mentions_video = true;
                        if chosen.is_some_and(|other: &Record| other.id != record.id) {
                            return Some(Err(
                                "引用对应多段视频，请用 /问视频 和原收件编号选定一段。".into(),
                            ));
                        }
                        chosen = Some(record);
                    }
                }
            }
            if !mentions_video {
                return None;
            }
            if chosen.is_some() && other_target {
                return Some(Err("这条引用对应多个任务，请用 /问视频 和原收件编号选定一段。".into()));
            }
        }
        let Some(receipt) = chosen else {
            return Some(Err("没有找到引用的视频收件。请引用当前对话中的原视频，或发送 /问视频 原收件编号 你的问题。".into()));
        };
        if receipt.args["scope"].as_str().is_none() {
            return Some(Err(
                "这段视频没有可核对的原会话。仍可用 /结果 原收件编号取回文字稿。".into(),
            ));
        }
        if explicit.is_none() && (clip_video::is_edit_request(question) || super::video_cover::is_cover_request(question) || super::publish_package::is_package_request(question)) {
            return Some(clip_action(receipt, question));
        }
        if receipt.status != "completed" {
            return Some(Err(
                "这段视频目前没有已核实的文字稿，请用 /结果 核对原收件编号。".into(),
            ));
        }
        if question.is_empty() || question.len() > 8000 {
            return Some(Err("请为这段视频输入一个不超过 8000 字节的问题。".into()));
        }
        Some(Ok(Action::Call(
            "assistant".into(),
            json!({"text":question,"video_receipt":receipt.id}),
        )))
    }

    pub(super) async fn read_video_scope(&self, source: &str) -> Result<Option<String>, String> {
        let Some(agent) = &self.agent else { return Ok(None); };
        let value = agent(json!({"operation":"current_intake_scope","source_key":source})).await?;
        Ok(value["session_id"].as_str().map(str::to_owned))
    }

    pub(super) async fn recent_video_clip(
        &self,
        message: &Value,
        account: &Stored,
        generation: u64,
        text: &str,
        received_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<Result<Action, String>> {
        let choice = clip_video::recent_clip_intent(text)?;
        if message["item_list"].as_array()?.iter().any(|item| item["ref_msg"].is_object()) {
            return None;
        }
        self.agent.as_ref()?;
        let source = account.logical_key();
        let missing = if choice == clip_video::RecentClip::Previous {
            PREVIOUS_CLIP_MISSING
        } else {
            RECENT_CLIP_MISSING
        };
        let scope = match self.read_video_scope(&source).await {
            Ok(Some(scope)) => scope,
            Ok(None) if choice == clip_video::RecentClip::ImplicitUnique => return None,
            Ok(None) => return Some(Err(missing.into())),
            Err(_) => return Some(Err("我暂时查不到当前对话的原视频，请稍后再试。".into())),
        };
        let state = self.state.lock().await;
        if !self.inbound_account_matches(&state, account, generation) {
            return None;
        }
        // Keep an unscoped failed receipt as a barrier. It must never make an
        // older, unrelated file look like the one that was just sent.
        let mut recent = state.stored.records.iter().rev().filter(|record| {
            (record.is_video() || record.args["video_rejected"] == true)
                && record.args["source"] == source
                && (record.args["scope"] == scope || record.args["scope"].is_null())
                && chrono::DateTime::parse_from_rfc3339(&record.created_at).is_ok_and(|created| {
                    let age = received_at.signed_duration_since(created);
                    (chrono::TimeDelta::zero()..=RECENT_CLIP_WINDOW).contains(&age)
                })
        });
        let Some(receipt) = recent.next() else {
            return if choice == clip_video::RecentClip::ImplicitUnique {
                None
            } else {
                Some(Err(missing.into()))
            };
        };
        if matches!(choice, clip_video::RecentClip::Unique | clip_video::RecentClip::ImplicitUnique)
            && recent.next().is_some()
        {
            return Some(Err("我在当前对话收到了不止一段视频。引用你想剪的那一段，我就接着做。".into()));
        }
        let uncertain_scope = || {
            Some(Err("我还不能确认刚收到的原片属于这次对话，请重发你想剪的 MP4。".into()))
        };
        if receipt.args["scope"] != scope {
            return uncertain_scope();
        }
        let receipt = if choice == clip_video::RecentClip::Previous {
            let Some(previous) = recent.next() else {
                return Some(Err(missing.into()));
            };
            if previous.args["scope"] != scope {
                return uncertain_scope();
            }
            previous
        } else {
            receipt
        };
        Some(clip_action(receipt, text.trim()))
    }

    pub(super) async fn implicit_clip_followup(
        &self, account: &Stored, generation: u64, text: &str,
    ) -> Option<Action> {
        let intent = clip_video::clip_revision_intent(text)?;
        let agent = self.agent.as_ref()?;
        let read = agent(json!({"operation":"resolve_current_clip","source_key":account.logical_key()}));
        let result = self.observe_query(generation, read).await?;
        let prompt = || Action::Reply("我暂时无法确定要修改的版本。请引用那份成片或对应的任务回复。".into());
        let value = match result {
            Ok(value) => value,
            Err(_) => return Some(prompt()),
        };
        if value.get("target") == Some(&Value::Null) {
            return (intent != clip_video::ClipRevision::Clarify).then(prompt);
        }
        let Some(turn) = value["target"].as_str().filter(|id| uuid::Uuid::parse_str(id).is_ok()) else {
            return Some(prompt());
        };
        let Ok(file) = serde_json::from_value::<file_delivery::FileSelection>(value["file"].clone()) else {
            return Some(prompt());
        };
        Some(Action::AssistantFollowup { turn: turn.into(), file: Some(file), text: text.into() })
    }

    pub(super) fn clip_timeline_followup(
        stored: &Stored,
        source: &str,
        timeline: &str,
        received_at: chrono::DateTime<chrono::Utc>,
    ) -> Option<Action> {
        let prompt = stored.records.last()?;
        if prompt.command != "message" || prompt.status != "completed" || prompt.delivery != "accepted" {
            return None;
        }
        let age = received_at.signed_duration_since(chrono::DateTime::parse_from_rfc3339(&prompt.created_at).ok()?);
        if !(chrono::TimeDelta::zero()..=RECENT_CLIP_WINDOW).contains(&age) {
            return None;
        }
        let original = prompt.args.get("quoted_assistant_v1")?;
        if original["source"] != source {
            return None;
        }
        let turn = original["turn"].as_str().filter(|id| uuid::Uuid::parse_str(id).is_ok())?;
        let range = clip_video::ambiguous_clip_range(original["text"].as_str()?)?;
        let file = serde_json::from_value(prompt.args.get("clip_clarification_file_v1")?.clone()).ok()?;
        Some(Action::AssistantFollowup { turn: turn.into(), file: Some(file), text: format!("只留{timeline}{range}") })
    }

    pub(super) async fn video_clip_request(&self, record: &Record, receipt_id: &str) -> Result<Option<Value>, String> {
        let state = self.state.lock().await;
        if !state.stored.enabled || record.status != "received" { return Ok(None); }
        let source = state.stored.logical_key();
        let receipt = state.stored.records.iter().find(|item| item.id == receipt_id && item.is_video() && item.args["source"] == source)
            .ok_or("这段原视频不属于当前微信绑定，未开始剪辑。")?;
        let clip = clip_video::ClipSource {
            receipt_id: receipt.id.clone(),
            session_id: receipt.args["scope"].as_str().ok_or("原视频没有可核对的接收会话。")?.into(),
            sha256: receipt.args["original_sha256"].as_str().ok_or("原视频缺少进入本机时的摘要。")?.into(),
        };
        clip.validate()?;
        Ok(Some(json!({"operation":"submit_video_clip","source_key":source,
            "request_id":record.id,"text":record.args["text"],"video":clip})))
    }

    pub(super) async fn video_question_request(
        &self,
        record: &Record,
        receipt_id: &str,
    ) -> Result<Option<Value>, String> {
        let state = self.state.lock().await;
        if !state.stored.enabled || record.status != "received" {
            return Ok(None);
        }
        let source = state.stored.logical_key();
        let receipt = state
            .stored
            .records
            .iter()
            .find(|item| {
                item.id == receipt_id
                    && item.is_video()
                    && item.status == "completed"
                    && item.args["source"] == source
            })
            .ok_or("原视频收件已无法核对，没有发起模型请求。")?;
        let scope = receipt.args["scope"]
            .as_str()
            .ok_or("原视频收件缺少会话归属。")?;
        let artifact = &receipt.result.as_ref().ok_or("视频收件缺少转写结果。")?["artifact"];
        let path = artifact["path"].as_str().ok_or("原视频文字稿尚不可用。")?;
        let sha256 = artifact["sha256"]
            .as_str()
            .ok_or("原视频文字稿缺少摘要。")?;
        Ok(Some(
            json!({"operation":"submit_video_question","source_key":source,
            "request_id":record.id,"text":record.args["text"],
            "video":{"receipt_id":receipt_id,"scope":scope,"sha256":sha256,"path":path,"workspace":self.workspace}}),
        ))
    }

    pub(super) async fn receive_media(
        &self,
        account: &Stored,
        generation: u64,
        identity: &InboundIdentity,
        context: &str,
        media: &MediaAttachment,
    ) -> Result<(), String> {
        let _submit = self.submit_lock.lock().await;
        let id = identity.receipt.as_str();
        let label = media.kind.label();
        let inbox = self.video_inbox.get().ok_or("本机转写尚未就绪。")?;
        let source = account.logical_key();
        let busy = {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation)
                || !matches!(
                    self.existing_inbound(&mut state, identity).await?,
                    InboundMatch::New
                )
            {
                return Ok(());
            }
            if state.stored.records.len() >= 5000 {
                return Err("微信账本已满，请归档后继续。".into());
            }
            state
                .stored
                .records
                .iter()
                .filter(|record| record.local_media_kind().is_some() && record.status == "running")
                .count()
                >= 2
        };
        let scope = match self.agent.as_ref().filter(|_| !busy) {
            Some(agent) => agent(json!({"operation":"intake_scope", "source_key":source}))
                .await
                .ok()
                .and_then(|value| value["session_id"].as_str().map(str::to_owned)),
            None => None,
        };
        let record = {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation)
                || !matches!(
                    self.existing_inbound(&mut state, identity).await?,
                    InboundMatch::New
                )
            {
                return Ok(());
            }
            if state.stored.records.len() >= 5000 {
                return Err("微信账本已满，请归档后继续。".into());
            }
            let mut next = state.stored.clone();
            next.reply_context_pending = false;
            let item_id = media.item_id.as_deref();
            let record = Record {
                id: id.into(),
                inbound_keys: identity.keys.clone(),
                command: media.kind.command().into(),
                status: "running".into(),
                args: json!({"source":source,"phase":"receiving","scope":scope,
                    "quote_id":item_id.map(|id| identity::legacy_receipt_id(account, id)),
                    "quote_source":item_id.map(|id| identity::source_key(&source, id))}),
                context: context.into(),
                delivery: "pending".into(),
                client_id: format!("hb-{}", uuid::Uuid::new_v4()),
                created_at: chrono::Utc::now().to_rfc3339(),
                ..Record::default()
            }.with_persisted_inbound_keys();
            next.records.push(record.clone());
            (self.save)(next.clone()).await?;
            state.stored = next;
            record
        };
        if busy {
            return self
                .complete_media(
                    account,
                    generation,
                    &record,
                    VideoResult::Failed(format!(
                        "目前有两条音视频正在处理。完成后可以重新发送这段{label}。"
                    )),
                    MediaCompletion::Automatic,
                )
                .await;
        }
        let canceled = self.cancel.notified();
        tokio::pin!(canceled);
        canceled.as_mut().enable();
        if !self.inbound_account_matches(&*self.state.lock().await, account, generation) {
            return Ok(());
        }
        let (download_limit, download_label) = match media.kind {
            MediaKind::Video => (VIDEO_LIMIT, "微信视频"),
            MediaKind::Voice => (VOICE_LIMIT, "微信语音"),
        };
        let result = tokio::select! {
            biased;
            _ = &mut canceled => return Ok(()),
            r = self.api.download_cdn(&media.media, download_limit, media.cipher_bytes, download_label) => r,
        };
        let bytes = match result {
            Ok(bytes)
                if (match media.kind {
                    MediaKind::Video => is_mp4(&bytes),
                    MediaKind::Voice => is_silk(&bytes),
                }) && media.plain_bytes.is_none_or(|size| size == bytes.len())
                    && media
                    .md5
                    .as_ref()
                    .is_none_or(|value| *value == format!("{:x}", md5::Md5::digest(&*bytes))) =>
            {
                bytes
            }
            Ok(_) => {
                return self
                    .complete_media(
                        account,
                        generation,
                        &record,
                        VideoResult::Failed(
                            match media.kind {
                                MediaKind::Video => {
                                    "视频内容与 MP4 格式、文件长度或原校验值不符，未开始转写。"
                                }
                                MediaKind::Voice => {
                                    "微信语音内容与受支持的 SILK 格式不符，未开始转写。"
                                }
                            }
                            .into(),
                        ),
                        MediaCompletion::Automatic,
                    )
                    .await
            }
            Err(message) => {
                return self
                    .complete_media(
                        account,
                        generation,
                        &record,
                        VideoResult::Failed(message),
                        MediaCompletion::Automatic,
                    )
                    .await
            }
        };
        {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation) {
                return Ok(());
            }
            let mut next = state.stored.clone();
            let item = next
                .records
                .iter_mut()
                .find(|item| item.id == id)
                .ok_or("音视频收件记录缺失")?;
            item.args["phase"] = json!("submitted");
            if media.kind == MediaKind::Video {
                item.args["original_sha256"] = json!(format!("{:x}", Sha256::digest(&*bytes)));
            }
            (self.save)(next.clone()).await?;
            state.stored = next;
        }
        let request = match media.kind {
            MediaKind::Video => VideoRequest::Start {
                id: id.into(),
                bytes,
            },
            MediaKind::Voice => VideoRequest::StartVoice {
                id: id.into(),
                bytes,
            },
        };
        let job = tokio::select! {
            biased;
            _ = &mut canceled => return Ok(()),
            result = inbox(request) => result,
        };
        if matches!(job, VideoResult::Running)
            && self.inbound_account_matches(&*self.state.lock().await, account, generation)
        {
            let text = format!("{label}已收到，正在本机转写。\n用 /状态 {id} 查看原收件。");
            // The receive claim is already durable, so duplicate messages do not repost the acknowledgement.
            let _ = self
                .api
                .text(
                    account,
                    context,
                    &format!("{}-ack", record.client_id),
                    &text,
                )
                .await;
        }
        self.complete_media(
            account,
            generation,
            &record,
            job,
            MediaCompletion::Automatic,
        )
        .await
    }

    pub(super) async fn update_media(
        &self,
        record: &Record,
        generation: u64,
    ) -> Result<(), String> {
        let canceled = self.cancel.notified();
        tokio::pin!(canceled);
        canceled.as_mut().enable();
        let _submit = tokio::select! {
            biased;
            _ = &mut canceled => return Ok(()),
            guard = self.submit_lock.lock() => guard,
        };
        let (account, current) = {
            let state = self.state.lock().await;
            let account = state.stored.account();
            if !self.inbound_account_matches(&state, &account, generation) {
                return Ok(());
            }
            let Some(current) = state.stored.records.iter().find(|item| {
                item.id == record.id
                    && item.local_media_kind().is_some()
                    && (item.status == "running" || item.pending_media_completion())
                    && item.args["source"] == account.logical_key()
            }) else {
                return Ok(());
            };
            (account, current.clone())
        };
        if current.pending_media_completion() {
            return self.send_media_completion(&account, generation, &current).await;
        }
        let result = tokio::select! {
            biased;
            _ = &mut canceled => return Ok(()),
            result = self.observe_media(&current) => result,
        };
        self.complete_media(
            &account,
            generation,
            &current,
            result,
            MediaCompletion::Automatic,
        )
        .await
    }

    async fn observe_media(&self, record: &Record) -> VideoResult {
        if record.args["phase"] == "submitted" {
            if let Some(inbox) = self.video_inbox.get() {
                let id = record.id.clone();
                return match record.local_media_kind() {
                    Some(MediaKind::Video) => inbox(VideoRequest::Observe { id }).await,
                    Some(MediaKind::Voice) => inbox(VideoRequest::ObserveVoice { id }).await,
                    None => VideoResult::Unknown,
                };
            }
            return VideoResult::Unknown;
        }
        let expired =
            chrono::DateTime::parse_from_rfc3339(&record.created_at).is_ok_and(|created| {
                chrono::Utc::now()
                    .signed_duration_since(created)
                    .num_seconds()
                    > 30
            });
        if expired {
            VideoResult::Unknown
        } else {
            VideoResult::Running
        }
    }

    async fn complete_media(
        &self,
        account: &Stored,
        generation: u64,
        record: &Record,
        result: VideoResult,
        completion: MediaCompletion,
    ) -> Result<(), String> {
        let Some(kind) = record.local_media_kind() else {
            return Ok(());
        };
        let label = kind.label();
        let (status, text, artifact, transcript, repositories) = match result {
            VideoResult::Running => return Ok(()),
            VideoResult::Unknown if completion == MediaCompletion::ExplicitQuery => return Ok(()),
            VideoResult::Unknown => (
                "unknown",
                format!("{label}处理回执暂未核实，已保留原收件。"),
                None,
                None,
                None,
            ),
            VideoResult::Failed(message) => ("failed", message, None, None, None),
            VideoResult::Ready { preview, artifact, repositories } => (
                "completed",
                media_preview(kind, &preview),
                Some(artifact),
                (kind == MediaKind::Voice).then_some(preview),
                (kind == MediaKind::Video).then_some(repositories),
            ),
        };
        let mut stored_result = json!({"text":text,"artifact":artifact});
        if let Some(transcript) = transcript {
            stored_result["transcript"] = json!(transcript);
        }
        if let Some(repositories) = repositories {
            stored_result["repository_refs"] = json!(repositories.links);
            stored_result["project_names"] = json!(repositories.names);
        }
        let pending = {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation) {
                return Ok(());
            }
            let mut next = state.stored.clone();
            let item = next
                .records
                .iter_mut()
                .find(|item| {
                    item.id == record.id
                        && item.command == record.command
                        && item.args["source"] == record.args["source"]
                })
                .ok_or("音视频收件记录缺失")?;
            if item.status != "running"
                && !(completion == MediaCompletion::ExplicitQuery && item.status == "unknown")
            {
                return Ok(());
            }
            item.status = status.into();
            item.result = Some(stored_result);
            let post = completion == MediaCompletion::Automatic
                && !state.stored.reply_context_pending
                && !item.reply_context_retired();
            if completion == MediaCompletion::Automatic {
                if post {
                    item.args[COMPLETION_REPLY] = json!(true);
                    item.delivery = "pending".into();
                    item.delivery_error = None;
                } else if item.reply_context_retired() {
                    item.mark_old_reply_unsent();
                } else {
                    item.delivery = "not_sent".into();
                }
            } else if item.delivery == "pending" {
                item.delivery = "not_sent".into();
            }
            let pending = post.then(|| item.clone());
            (self.save)(next.clone()).await?;
            state.stored = next;
            pending
        };
        if let Some(pending) = pending {
            self.send_media_completion(account, generation, &pending).await?;
        }
        Ok(())
    }

    async fn send_media_completion(
        &self,
        account: &Stored,
        generation: u64,
        record: &Record,
    ) -> Result<(), String> {
        let kind = record.local_media_kind().ok_or("原音视频收件类型无法核对")?;
        let result = record.result.as_ref().ok_or("原音视频收件缺少转写结果")?;
        let text = result["text"].as_str().ok_or("原音视频收件缺少结果摘要")?;
        let mut response = text.to_owned();
        if let Some(refs) = saved_repository_refs(result) {
            append_repository_refs(&mut response, refs);
        }
        if let Some(names) = repository_search::saved_names(result) {
            repository_search::append_names(&mut response, names);
        }
        response.push_str(&format!("\n\n可以用 /结果 {} 核对原收件。", record.id));
        if kind == MediaKind::Voice && record.status == "completed" && record.args["scope"].is_string() {
            response.push(' ');
            response.push_str(voice::TASK_HINT);
        }
        if record.status == "completed" && record.args["scope"].is_string() {
            response.push_str(&format!("\n想了解内容，可以引用原{}问我。", kind.label()));
        }
        let files = result.get("artifact").filter(|value| !value.is_null())
            .map(std::slice::from_ref).unwrap_or_default();
        self.reply_files_with_claim(&record.id, &response, files, Some((account, generation)), true, None, None).await
    }

    pub(super) async fn query_media(
        &self,
        id: &str,
        target: &Record,
        files: bool,
        origin: (&Stored, u64),
    ) -> Result<(), String> {
        let kind = target
            .local_media_kind()
            .ok_or("原音视频收件类型无法核对")?;
        let label = kind.label();
        let (account, generation) = origin;
        if target.args["source"] != account.logical_key() {
            return self
                .reply_files_for(
                    id,
                    &format!("没有找到当前微信来源的{label}收件。"),
                    &[],
                    Some(origin),
                )
                .await;
        }
        let (mut message, artifact, mut links, mut names) = if target.status == "running" || target.status == "unknown" {
            let Some(result) = self.observe_query(generation, self.observe_media(target)).await else {
                return Ok(());
            };
            let response = match &result {
                VideoResult::Running => (format!("正在处理原{label}，请稍后再看结果。"), None, None, None),
                VideoResult::Unknown => (format!("暂时查不到原{label}的处理回执。"), None, None, None),
                VideoResult::Failed(text) => (text.clone(), None, None, None),
                VideoResult::Ready { preview, artifact, repositories } =>
                    (media_preview(kind, preview), Some(artifact.clone()), Some(repositories.links.clone()), Some(repositories.names.clone())),
            };
            // The query can settle a late result without reposting the original notification.
            self.complete_media(
                account,
                generation,
                target,
                result,
                MediaCompletion::ExplicitQuery,
            )
            .await?;
            response
        } else {
            let stored = target.result.as_ref().ok_or("原音视频收件缺少转写结果")?;
            (
                stored["text"]
                    .as_str()
                    .unwrap_or("原收件没有可取回的文字结果。")
                    .into(),
                stored
                    .get("artifact")
                    .filter(|value| !value.is_null())
                    .cloned(),
                saved_repository_refs(stored),
                repository_search::saved_names(stored),
            )
        };
        if kind == MediaKind::Video && files {
            if let Some(artifact) = artifact.as_ref() {
                if links.is_none() || names.is_none() {
                    if let Some(hints) = legacy_repository_hints(&target.id, &self.workspace, artifact) {
                        links = links.or(Some(hints.links));
                        names = names.or(Some(hints.names));
                    } else {
                        message.push_str("\n暂时无法核对原文字稿的仓库信息。");
                    }
                }
                if let Some(refs) = links { append_repository_refs(&mut message, refs); }
                if let Some(names) = names { repository_search::append_names(&mut message, names); }
            }
        }
        let message = if kind == MediaKind::Voice && !files && artifact.is_some() {
            "语音转写已保存。发送 /结果 和原收件编号可以查看。".into()
        } else {
            message
        };
        self.reply_files_for(
            id,
            &message,
            if files { artifact.as_slice() } else { &[] },
            Some(origin),
        )
        .await
    }
}

fn saved_repository_refs(result: &Value) -> Option<Vec<String>> {
    let refs = result["repository_refs"].as_array()?;
    Some(refs.iter().filter_map(Value::as_str).take(MAX_REPOSITORY_REFS).map(str::to_owned).collect())
}

pub(super) fn legacy_repository_hints(id: &str, workspace: &Path, artifact: &Value) -> Option<RepositoryHints> {
    let path = Path::new(artifact["path"].as_str()?);
    let bytes = verified_transcript(MediaKind::Video, id, workspace, path, artifact["sha256"].as_str()?).ok()?;
    Some(RepositoryHints::from_transcript(std::str::from_utf8(&bytes).ok()?))
}

fn media_preview(kind: MediaKind, preview: &str) -> String {
    match kind {
        MediaKind::Video => format!("视频语音已转写（最多前 {VIDEO_SECONDS} 秒）。\n{preview}"),
        MediaKind::Voice => format!("本机已完成微信语音转写。\n{preview}"),
    }
}

#[cfg(test)]
mod repository_ref_tests {
    use super::repository_refs;

    #[test]
    fn links_are_canonical_unique_ordered_and_bounded() {
        let text = concat!(
            "<https://GitHub.com/Owner/Name.git/> 与（https://github.com/owner/name）。\n",
            "http://github.com/test/insecure https://github.com:443/test/port\n",
            "https://a:b@github.com/test/credential https://github.com.evil.invalid/u/p\n",
            "https://wrapped.invalid/?url=https://github.com/test/redirect\n",
            "https://github.com/test/query?key=sensitive https://github.com/test/page#section\n",
            "https://github.com/test/tree/tree/main https://github.com/test/../child\n",
            "GitHub.com/a/one、https://github.com/b/two、https://github.com/c/three、",
            "https://github.com/d/four、https://github.com/e/five。",
        );
        assert_eq!(repository_refs(text), [
            "https://github.com/Owner/Name",
            "https://github.com/a/one",
            "https://github.com/b/two",
            "https://github.com/c/three",
            "https://github.com/d/four",
        ]);
    }
}
