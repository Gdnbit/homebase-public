//! Immutable, scoped PNG attachments. Drafts and turns retain references only.
use crate::model_capabilities::vision::{normalize_attachment, MAX_IMAGE_BYTES};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const LIMIT: usize = 256;
const SUFFIX: &str = ".png.dpapi";
const CHECKPOINT_KEY: &str = "homebase:vision-input:v1";

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageReference {
    pub scope: String,
    pub name: String,
    pub sha256: String,
    pub bytes: usize,
}

fn desktop_scope(scope: &str) -> bool {
    scope == "new" || uuid::Uuid::parse_str(scope).is_ok_and(|id| id.to_string() == scope)
}

fn png_name(name: &str) -> bool {
    name.len() <= 180
        && name.to_ascii_lowercase().ends_with(".png")
        && !name.chars().any(|c| c.is_control() || "/\\:".contains(c))
}

impl ImageReference {
    pub(crate) fn validate(&self, scope: &str) -> Result<(), String> {
        if self.scope != scope
            || !desktop_scope(scope)
            || !png_name(&self.name)
            || !(1..=MAX_IMAGE_BYTES).contains(&self.bytes)
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("图片引用与当前桌面草稿不匹配，未读取或发送图片".into());
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VisionCheckpoint {
    pub image: ImageReference,
    pub attempted: bool,
}

fn read_checkpoint(
    stored: Option<&Value>,
    inline: Option<ImageReference>,
    attempted: bool,
) -> Result<Option<VisionCheckpoint>, String> {
    let Some(stored) = stored else {
        return Ok(inline.map(|image| VisionCheckpoint { image, attempted }));
    };
    let checkpoint: VisionCheckpoint =
        serde_json::from_value(stored.clone()).map_err(|_| "本轮图片检查点无法确认，保留原记录")?;
    if inline.is_some_and(|image| image != checkpoint.image) {
        return Err("本轮图片检查点与原引用不一致，保留原记录".into());
    }
    Ok(Some(checkpoint))
}

pub(crate) fn archived_image(turn: &Value) -> Result<Option<ImageReference>, String> {
    let inline = turn
        .get("image")
        .filter(|v| !v.is_null())
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|_| "本轮图片引用无法确认")?;
    Ok(
        read_checkpoint(turn["runtime_models"].get(CHECKPOINT_KEY), inline, true)?
            .map(|checkpoint| checkpoint.image),
    )
}

impl super::Turn {
    pub(super) fn vision_checkpoint(&self) -> Result<Option<VisionCheckpoint>, String> {
        read_checkpoint(
            self.runtime_models.get(CHECKPOINT_KEY),
            self.image.clone(),
            self.runtime_uncertain_model || self.rounds > 0 || super::terminal(&self.state),
        )
    }

    pub(super) fn protect_new_image(&mut self, image: ImageReference) {
        self.write_vision_checkpoint(VisionCheckpoint {
            image,
            attempted: false,
        });
        // Older builds honor this guard before they can turn queued visual work
        // into a text-only model call. Only an understood ready checkpoint can run.
        self.runtime_uncertain_model = true;
    }

    pub(super) fn write_vision_checkpoint(&mut self, checkpoint: VisionCheckpoint) {
        self.runtime_models
            .insert(CHECKPOINT_KEY.into(), json!(checkpoint));
        self.image = None;
    }

    pub(super) fn has_inline_image_only(&self) -> bool {
        self.image.is_some() && !self.runtime_models.contains_key(CHECKPOINT_KEY)
    }

    pub(super) fn migrate_inline_image(&mut self) {
        if let Some(image) = self.image.take() {
            let attempted =
                self.runtime_uncertain_model || self.rounds > 0 || super::terminal(&self.state);
            if attempted {
                self.write_vision_checkpoint(VisionCheckpoint { image, attempted });
            } else {
                self.protect_new_image(image);
            }
        }
    }
}

pub(crate) struct ImageStore {
    root: PathBuf,
    gate: Mutex<()>,
}

impl ImageStore {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            gate: Mutex::new(()),
        }
    }

    fn path(&self, reference: &ImageReference) -> PathBuf {
        let scope = hex_digest(reference.scope.as_bytes());
        self.root
            .join(format!("{}-{}{SUFFIX}", scope, reference.sha256))
    }

    pub(crate) fn load(&self, reference: &ImageReference) -> Result<Zeroizing<Vec<u8>>, String> {
        self.archive_source(reference).map_err(|failure| {
            if failure.starts_with("ARCHIVE_CHANGED:") {
                "本机图片校验失败，未发送".into()
            } else {
                "无法读取已保存的本机图片，未发送".into()
            }
        })
    }

    pub(crate) fn archive_source(
        &self,
        reference: &ImageReference,
    ) -> Result<Zeroizing<Vec<u8>>, String> {
        reference.validate(&reference.scope)?;
        let cipher = crate::assistant_state_store::read_bounded(
            &self.path(reference),
            MAX_IMAGE_BYTES + 4096,
        )?
        .ok_or("ARCHIVE_MISSING: 找不到原图片密文")?;
        let pixels = crate::wechat_crypto::crypt(&cipher, false)
            .map_err(|_| "ARCHIVE_CHANGED: 图片密文无法解密")?;
        if pixels.len() != reference.bytes || hex_digest(&pixels) != reference.sha256 {
            return Err("ARCHIVE_CHANGED: 图片内容与原引用不符".into());
        }
        Ok(pixels)
    }

    pub(crate) async fn stage(
        &self,
        scope: String,
        name: String,
        base64: &str,
    ) -> Result<ImageReference, String> {
        if !desktop_scope(&scope) || !png_name(&name) {
            return Err("请选择当前桌面草稿可用的本机 PNG 文件".into());
        }
        let pixels = Zeroizing::new(normalize_attachment(base64)?);
        let reference = ImageReference {
            scope,
            name,
            bytes: pixels.len(),
            sha256: hex_digest(&pixels),
        };
        let path = self.path(&reference);
        let _guard = self.gate.lock().await;
        fs::create_dir_all(&self.root).map_err(|_| "无法创建加密图片目录")?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".lock"))
            .map_err(|_| "无法锁定加密图片目录")?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| "另一窗口正在保存图片，请稍后重试")?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                self.load(&reference)?;
                return Ok(reference);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("无法核对已保存图片，原文件保留".into()),
        }
        let mut count = 0;
        for item in fs::read_dir(&self.root).map_err(|_| "无法读取加密图片目录")? {
            let item = item.map_err(|_| "无法读取加密图片目录")?;
            if item.file_name().to_string_lossy().ends_with(SUFFIX) {
                count += 1;
            }
            if count >= LIMIT {
                return Err("本机图片已达到保存上限，原图片保留".into());
            }
        }
        let cipher = crate::wechat_crypto::crypt(&pixels, true).map_err(|_| "无法加密本机图片")?;
        crate::control_protocol::publish_private(&path, &cipher)
            .await
            .map_err(|_| "无法安全保存本机图片")?;
        Ok(reference)
    }
}

fn hex_digest(data: &[u8]) -> String {
    format!("{:x}", Sha256::digest(data))
}

impl super::Engine {
    pub(crate) async fn stage_image(
        &self,
        scope: String,
        name: String,
        base64: &str,
    ) -> Result<ImageReference, String> {
        self.desktop_scope(&scope).await?;
        self.images.stage(scope, name, base64).await
    }
}
