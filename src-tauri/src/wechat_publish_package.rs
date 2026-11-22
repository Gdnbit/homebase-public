//! Produce one recoverable, local delivery from a verified original video.
use super::{artifact_bytes_limited, clip_video, subtitles, video_cover, Bridge};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Write},
    path::Path,
};

pub(crate) const COMMAND: &str = "capability_video_publish_package";
pub(super) const FILENAME: &str = "short-video-publish.zip";
const LIMIT: usize = 20 * 1024 * 1024;
const ROOT: &str = "publish-packages";
const COVER_NAMES: [&str; 3] = ["cover-wide.png", "cover-portrait.png", "cover-vertical.png"];

pub(super) fn is_package_request(text: &str) -> bool {
    let text = text.trim();
    text.contains("发布包")
        && !crate::assistant::explicit_discussion(text)
        && ![
            "不要",
            "别做",
            "先别",
            "怎么",
            "如何",
            "做好了",
            "发给我",
            "取回",
            "查一下",
            "找一下",
        ]
        .iter()
        .any(|value| text.contains(value))
        && ["做", "生成", "整理", "打包", "制作", "导出"]
            .iter()
            .any(|value| text.contains(value))
        && [
            "帮我", "请", "麻烦", "把", "从", "用", "给", "做", "生成", "制作", "打包", "导出",
        ]
        .iter()
        .any(|value| text.starts_with(value))
}

impl Bridge {
    pub(crate) async fn publish_package_original(
        &self,
        ffmpeg: &Path,
        source: clip_video::ClipSource,
        ranges: &[clip_video::ClipRange],
        cover_at_ms: u64,
        title: &str,
        description: &str,
    ) -> Result<Value, String> {
        if !(1..=6).contains(&ranges.len()) {
            return Err("发布包需要 1 至 6 个片段。".into());
        }
        clip_video::validate_ranges(ranges)?;
        let cover_title = video_cover::cover_title(cover_at_ms, title)?;
        let description = description.trim();
        if description.is_empty()
            || description.len() > 4000
            || description
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err("请提供 4000 字节以内的发布简介。".into());
        }

        static SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        let _slot = SLOT
            .try_acquire()
            .map_err(|_| "已有发布包在生成，请稍后再试。")?;
        let original = self.read_original_video(&source).await?;
        let timed = self.read_original_subtitles(&source).await?;
        let root = self.workspace.join(ROOT);
        if let Err(error) = std::fs::create_dir(&root) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err("无法创建发布包目录。".into());
            }
        }
        let metadata = std::fs::symlink_metadata(&root).map_err(|_| "无法核对发布包目录。")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("发布包路径必须是普通目录。".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("发布包路径包含目录链接。".into());
            }
        }
        crate::control_protocol::private_directory(&root)
            .await
            .map_err(|_| "无法保护发布包目录。")?;
        let scratch = tempfile::Builder::new()
            .prefix("package-work-")
            .tempdir_in(&root)
            .map_err(|_| "无法创建发布包工作目录。")?;
        let clip = clip_video::render_clip_with_subtitles(
            ffmpeg,
            scratch.path(),
            &original,
            ranges,
            Some(&timed),
            true,
        )
        .await?;
        let covers =
            video_cover::render(ffmpeg, scratch.path(), &original, cover_at_ms, &cover_title)
                .await?;
        let draft = format!(
            "# {}\n\n{}\n\n![封面](cover-vertical.png)\n",
            title.trim(),
            description
        );
        let mut writer = ZipContents::new();
        writer.artifact(scratch.path(), "video-vertical.mp4", &clip["artifact"])?;
        writer.artifact(scratch.path(), subtitles::NAME, &clip["subtitles"])?;
        let cover_files = covers["artifacts"]
            .as_array()
            .filter(|rows| rows.len() == COVER_NAMES.len())
            .ok_or("三张封面未完整生成。")?;
        for (name, receipt) in COVER_NAMES.into_iter().zip(cover_files) {
            writer.artifact(scratch.path(), name, receipt)?;
        }
        writer.entry("post.md", draft.as_bytes())?;
        let manifest = json!({
            "version":1,"kind":"homebase_short_video_package",
            "source":{"wechat_receipt_id":source.receipt_id,"sha256":source.sha256,"subtitle_sha256":digest(&timed)},
            "segments":ranges,"cover_at_ms":cover_at_ms,
            "video":{"width":720,"height":1280,"duration_ms":clip["duration_ms"]},
            "files":writer.entries
        });
        let manifest = serde_json::to_vec_pretty(&manifest).map_err(|_| "无法保存发布包清单。")?;
        writer.entry("manifest.json", &manifest)?;
        let data = writer
            .zip
            .finish()
            .map_err(|_| "无法完成 ZIP 发布包。")?
            .into_inner();
        if data.len() > LIMIT {
            return Err("发布包超过 20 MiB，请缩短所选片段。".into());
        }
        let staged = scratch.path().join("archive");
        std::fs::create_dir(&staged).map_err(|_| "无法创建发布包产物目录。")?;
        crate::control_protocol::publish_private(&staged.join(FILENAME), &data)
            .await
            .map_err(|_| "无法保存 ZIP 发布包。")?;
        let published = root.join(format!("package-{}", uuid::Uuid::new_v4()));
        std::fs::rename(staged, &published).map_err(|_| "无法登记 ZIP 发布包。")?;
        Ok(
            json!({"status":"succeeded","artifact":{"path":published.join(FILENAME),"bytes":data.len(),"sha256":digest(&data),"mime_type":"application/zip"}}),
        )
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

struct ZipContents {
    zip: zip::ZipWriter<Cursor<Vec<u8>>>,
    entries: Vec<Value>,
}

impl ZipContents {
    fn new() -> Self {
        Self {
            zip: zip::ZipWriter::new(Cursor::new(Vec::new())),
            entries: Vec::new(),
        }
    }

    fn artifact(&mut self, scratch: &Path, name: &str, receipt: &Value) -> Result<(), String> {
        let path = receipt["path"]
            .as_str()
            .map(Path::new)
            .ok_or("缺少发布包产物。")?;
        let bytes = artifact_bytes_limited(path, scratch, LIMIT)?;
        if receipt["bytes"] != bytes.len() || receipt["sha256"] != digest(&bytes) {
            return Err("发布包产物内容或摘要已变化。".into());
        }
        self.entry(name, &bytes)
    }

    fn entry(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let used: u64 = self
            .entries
            .iter()
            .filter_map(|entry| entry["bytes"].as_u64())
            .sum();
        if used + bytes.len() as u64 > LIMIT as u64 {
            return Err("发布包超过 20 MiB，请缩短所选片段。".into());
        }
        self.zip
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .map_err(|_| "无法添加发布包文件。")?;
        self.zip
            .write_all(bytes)
            .map_err(|_| "无法写入发布包文件。")?;
        self.entries
            .push(json!({"name":name,"bytes":bytes.len(),"sha256":digest(bytes)}));
        Ok(())
    }
}

pub(crate) fn valid_receipt(result: &Value, verify: bool) -> bool {
    let artifact = &result["artifact"];
    let Some(path) = artifact["path"].as_str().map(Path::new) else {
        return false;
    };
    let Some(directory) = path.parent() else {
        return false;
    };
    let Some(root) = directory.parent() else {
        return false;
    };
    let Some(size) = artifact["bytes"].as_u64() else {
        return false;
    };
    let sha = artifact["sha256"].as_str().unwrap_or_default();
    result.as_object().is_some_and(|object| object.len() == 2)
        && result["status"] == "succeeded"
        && artifact.as_object().is_some_and(|object| object.len() == 4)
        && artifact["mime_type"] == "application/zip"
        && (1..=LIMIT as u64).contains(&size)
        && sha.len() == 64
        && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && path.is_absolute()
        && path.file_name().and_then(|name| name.to_str()) == Some(FILENAME)
        && root.file_name().and_then(|name| name.to_str()) == Some(ROOT)
        && directory
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("package-"))
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        && (!verify
            || artifact_bytes_limited(path, root, LIMIT).is_ok_and(|data| {
                data.len() as u64 == size && data.starts_with(b"PK\x03\x04") && digest(&data) == sha
            }))
}
