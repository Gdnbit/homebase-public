//! Render one admitted MP4 frame into a set of three PNG covers.
use super::{artifact_bytes_limited, clip_video, Bridge};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

pub(crate) const COMMAND: &str = "capability_video_cover";
const PNG_LIMIT: usize = 10 * 1024 * 1024;
const COVERS: [(&str, u32, u32); 3] = [
    ("cover-wide.png", 1280, 720),
    ("cover-portrait.png", 1080, 1440),
    ("cover-vertical.png", 1080, 1920),
];

pub(super) fn is_cover_request(text: &str) -> bool {
    let text = text.trim();
    !text.starts_with('/')
        && !crate::assistant::explicit_discussion(text)
        && ![
            "不要",
            "别做",
            "先别",
            "不用",
            "怎么",
            "如何",
            "做好了",
            "怎么样",
            "拿封面",
            "找封面",
            "提到",
            "聊聊",
        ]
        .iter()
        .any(|term| text.contains(term))
        && [
            "帮我", "请", "麻烦", "把", "从", "刚发", "刚刚", "刚才", "这段", "这个", "这条", "用",
            "取帧", "截", "做", "生成", "制作",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
        && text.contains("封面")
        && ["做", "生成", "制作", "截", "取帧"]
            .iter()
            .any(|term| text.contains(term))
}

impl Bridge {
    pub(crate) async fn cover_original(
        &self,
        ffmpeg: &Path,
        source: clip_video::ClipSource,
        at_ms: u64,
        title: &str,
    ) -> Result<Value, String> {
        let title = cover_title(at_ms, title)?;
        let bytes = self
            .read_original_video(&source)
            .await
            .map_err(|error| format!("COVER_SOURCE_UNAVAILABLE: {error}"))?;
        render(ffmpeg, &self.workspace, &bytes, at_ms, &title).await
    }
}

pub(super) fn cover_title(at_ms: u64, title: &str) -> Result<String, String> {
    if at_ms >= 120_000 {
        return Err("COVER_INPUT_INVALID: 封面时间点须在原视频前 120 秒内。".into());
    }
    if title.trim().is_empty() || title.chars().any(char::is_control) || title.chars().count() > 32
    {
        return Err("COVER_INPUT_INVALID: 请使用 1 至 32 个字的单行封面标题。".into());
    }
    let mut rendered = String::new();
    for (index, letter) in title.trim().chars().enumerate() {
        if index == 16 {
            rendered.push('\n');
        }
        rendered.push(letter);
    }
    Ok(rendered)
}

pub(crate) fn failure_message(code: &str) -> Option<&'static str> {
    Some(match code {
        "COVER_INPUT_INVALID" => "请提供原视频前 120 秒内的时间点和 1 至 32 个字的单行标题。",
        "COVER_SOURCE_UNAVAILABLE" => "无法核对当前会话内的原片，请引用或重发原视频。",
        "COVER_BUSY" => "已有封面在生成，请稍后再试。",
        "COVER_FONT_UNAVAILABLE" => "无法使用 Windows 微软雅黑字体，请核对本机字体。",
        "COVER_RENDER_FAILED" => "未生成完整的三张封面，请核对原片、时间点或本机渲染环境。",
        _ => return None,
    })
}

pub(super) async fn render(
    ffmpeg: &Path,
    workspace: &Path,
    bytes: &[u8],
    at_ms: u64,
    title: &str,
) -> Result<Value, String> {
    static SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let _slot = SLOT
        .try_acquire()
        .map_err(|_| "COVER_BUSY: 已有封面在生成，请稍后再试。")?;
    let root = workspace.join("video-covers");
    if let Err(error) = std::fs::create_dir(&root) {
        if error.kind() != std::io::ErrorKind::AlreadyExists {
            return Err("COVER_RENDER_FAILED: 无法创建封面目录。".into());
        }
    }
    let info =
        std::fs::symlink_metadata(&root).map_err(|_| "COVER_RENDER_FAILED: 无法核对封面目录。")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if info.file_attributes() & 0x400 != 0 {
            return Err("COVER_RENDER_FAILED: 封面目录含链接。".into());
        }
    }
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err("COVER_RENDER_FAILED: 封面路径必须是普通目录。".into());
    }
    crate::control_protocol::private_directory(&root)
        .await
        .map_err(|_| "COVER_RENDER_FAILED: 无法保护封面目录。")?;
    let scratch = tempfile::Builder::new()
        .prefix("cover-work-")
        .tempdir_in(&root)
        .map_err(|_| "COVER_RENDER_FAILED: 无法创建封面工作目录。")?;
    let output = scratch.path().join("results");
    std::fs::create_dir(&output).map_err(|_| "COVER_RENDER_FAILED: 无法创建结果目录。")?;
    crate::control_protocol::publish_private(&scratch.path().join("source.mp4"), bytes)
        .await
        .map_err(|_| "COVER_RENDER_FAILED: 无法暂存已核验的原片。")?;
    std::fs::write(scratch.path().join("title.txt"), title)
        .map_err(|_| "COVER_RENDER_FAILED: 无法暂存封面标题。")?;
    let font = std::env::var_os("SystemRoot")
        .map(|root| {
            std::path::PathBuf::from(root)
                .join("Fonts")
                .join("msyh.ttc")
        })
        .filter(|path| path.is_absolute() && path.is_file())
        .ok_or("COVER_FONT_UNAVAILABLE: 缺少 Windows 微软雅黑字体，无法生成中文封面。")?;
    std::fs::copy(font, scratch.path().join("font.ttc"))
        .map_err(|_| "COVER_FONT_UNAVAILABLE: 无法读取 Windows 微软雅黑字体。")?;

    let mut filters = String::from("[0:V:0]split=3[frame0][frame1][frame2]");
    for (index, (_, w, h)) in COVERS.iter().enumerate() {
        let box_top = h * 73 / 100;
        let box_width = w * 88 / 100;
        let box_height = h * 20 / 100;
        let box_left = w * 6 / 100;
        let title_x = box_left + 30;
        let title_y = box_top + 23;
        filters.push_str(&format!(
            ";[frame{index}]scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=0x162638,setsar=1,\
             drawbox=x={box_left}:y={box_top}:w={box_width}:h={box_height}:color=0x121D2E@0.94:t=fill,\
             drawtext=fontfile=font.ttc:textfile=title.txt:expansion=none:fontsize=40:line_spacing=8:\
             fontcolor=white:x={title_x}:y={title_y}:fix_bounds=1[cover{index}]"
        ));
    }
    let mut process = clip_video::ffmpeg_process(ffmpeg);
    process
        .current_dir(scratch.path())
        .args(["-filter_complex_threads", "1", "-threads", "1", "-ss"])
        .arg(format!("{}.{:03}", at_ms / 1000, at_ms % 1000))
        .args(["-f", "mp4", "-i", "source.mp4", "-filter_complex"])
        .arg(filters);
    for (index, (name, _, _)) in COVERS.iter().enumerate() {
        process
            .arg("-map")
            .arg(format!("[cover{index}]"))
            .args([
                "-frames:v",
                "1",
                "-threads",
                "1",
                "-c:v",
                "png",
                "-pix_fmt",
                "rgb24",
                "-f",
                "image2",
                "-update",
                "1",
            ])
            .arg(format!("results/{name}"));
    }
    tokio::time::timeout(Duration::from_secs(45), clip_video::frame_count(process))
        .await
        .map_err(|_| "COVER_RENDER_FAILED: 封面渲染超时。")?
        .map_err(|_| "COVER_RENDER_FAILED: 无法取得所选帧，请核对原片和时间点。")?;

    let directory = root.join(format!("cover-{}", uuid::Uuid::new_v4()));
    let mut artifacts = Vec::with_capacity(COVERS.len());
    for (name, w, h) in COVERS {
        let bytes = artifact_bytes_limited(&output.join(name), &root, PNG_LIMIT)
            .map_err(|_| "COVER_RENDER_FAILED: 未生成完整的三种封面。")?;
        if !matches_png(&bytes, w, h) {
            return Err("COVER_RENDER_FAILED: 封面 PNG 格式或尺寸不一致。".into());
        }
        artifacts.push(json!({"path":directory.join(name),"bytes":bytes.len(),"sha256":format!("{:x}", Sha256::digest(&bytes)),"mime_type":"image/png","width":w,"height":h}));
    }
    std::fs::rename(output, directory).map_err(|_| "COVER_RENDER_FAILED: 无法登记封面文件。")?;
    Ok(json!({"status":"succeeded","at_ms":at_ms,"artifacts":artifacts}))
}

fn matches_png(bytes: &[u8], width: u32, height: u32) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR")
        && bytes.get(16..20) == Some(width.to_be_bytes().as_slice())
        && bytes.get(20..24) == Some(height.to_be_bytes().as_slice())
        && bytes.ends_with(b"\0\0\0\0IEND\xaeB`\x82")
}

pub(crate) fn valid_receipt(result: &Value, verify_files: bool) -> bool {
    let Some(artifacts) = result["artifacts"].as_array() else {
        return false;
    };
    if artifacts.len() != COVERS.len()
        || result.as_object().map(|m| m.len()) != Some(3)
        || result["status"] != "succeeded"
        || !result["at_ms"].as_u64().is_some_and(|ms| ms < 120_000)
    {
        return false;
    }
    let directory = artifacts[0]["path"]
        .as_str()
        .map(Path::new)
        .and_then(Path::parent);
    let Some(directory) = directory else {
        return false;
    };
    let Some(root) = directory.parent() else {
        return false;
    };
    if !root.is_absolute()
        || root.file_name().and_then(|s| s.to_str()) != Some("video-covers")
        || !directory
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("cover-"))
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
    {
        return false;
    }
    artifacts
        .iter()
        .zip(COVERS)
        .all(|(artifact, (name, w, h))| {
            let path = directory.join(name);
            let sha = artifact["sha256"].as_str().unwrap_or_default();
            let size = artifact["bytes"].as_u64().unwrap_or_default();
            artifact.as_object().map(|m| m.len()) == Some(6)
                && artifact["path"].as_str().map(Path::new) == Some(path.as_path())
                && artifact["width"] == w
                && artifact["height"] == h
                && artifact["mime_type"] == "image/png"
                && (1..=PNG_LIMIT as u64).contains(&size)
                && sha.len() == 64
                && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                && (!verify_files
                    || artifact_bytes_limited(&path, root, PNG_LIMIT).is_ok_and(|bytes| {
                        bytes.len() as u64 == size
                            && matches_png(&bytes, w, h)
                            && format!("{:x}", Sha256::digest(&bytes)) == sha
                    }))
        })
}

#[cfg(all(test, windows))]
#[path = "wechat_video_cover_tests.rs"]
mod tests;
