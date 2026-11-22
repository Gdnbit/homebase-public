//! Transactional encrypted vault + metadata. All paths are explicit for isolation.
use super::sops::SopsAdapter;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

pub type Metadata = BTreeMap<String, Entry>;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub revision: u64,
    pub expires_at: Option<String>,
    pub display_name: Option<String>,
    pub service_id: Option<String>,
    pub rotate_url: Option<String>,
    pub expiry_policy: String,
    pub updated_at: Option<String>,
    pub rotations: Vec<Rotation>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rotation {
    pub id: String,
    pub changed_at: String,
    pub validation: String,
    pub revoked_at: Option<String>,
    #[serde(default)]
    pub rolled_back_at: Option<String>,
    #[serde(default)]
    pub backup_file: String,
    #[serde(default)]
    pub previous_expiry: Option<String>,
    #[serde(default)]
    pub previous_policy: String,
}

#[derive(Deserialize)]
pub struct SaveRequest {
    pub short_name: String,
    pub expected_revision: u64,
    pub expected_vault_version: String,
    pub value: Option<Zeroizing<String>>,
    pub expires_at: Option<String>,
    pub display_name: Option<String>,
    pub service_id: Option<String>,
    pub rotate_url: Option<String>,
    pub expiry_policy: Option<String>,
    #[serde(default = "save_operation")]
    pub operation: String,
    #[serde(default)]
    pub candidate_verified: bool,
    pub rotation_id: Option<String>,
}
fn save_operation() -> String {
    "save".into()
}

#[derive(Serialize, Deserialize)]
pub struct SaveResult {
    pub revision: u64,
    pub vault_version: String,
    pub backup_path: String,
    pub verified: bool,
}

pub struct ListedValue {
    pub name: String,
    pub state: String,
    pub masked: String,
}

pub struct Snapshot {
    pub values: Vec<ListedValue>,
    pub metadata: Metadata,
    pub version: String,
}

#[async_trait]
pub trait Cipher: Send + Sync {
    async fn set(&self, path: &Path, key: &str, value: &str) -> Result<(), String>;
    async fn key(&self, path: &Path, key: &str) -> Result<Zeroizing<String>, String>;
    async fn all(&self, path: &Path) -> Result<Zeroizing<String>, String>;
}

#[async_trait]
impl Cipher for SopsAdapter {
    async fn set(&self, path: &Path, key: &str, value: &str) -> Result<(), String> {
        self.set_key(path, key, value)
            .await
            .map_err(|_| "加密写入失败，原文件未修改".into())
    }
    async fn key(&self, path: &Path, key: &str) -> Result<Zeroizing<String>, String> {
        let secure = self
            .decrypt_key_unchecked(path, key)
            .await
            .map_err(|_| "无法验证暂存密钥，原文件未修改".to_string())?;
        Ok(Zeroizing::new(secure.as_str().to_string()))
    }
    async fn all(&self, path: &Path) -> Result<Zeroizing<String>, String> {
        let secure = self
            .decrypt_file(path)
            .await
            .map_err(|_| "密钥库解密失败，请检查 SOPS 与身份文件".to_string())?;
        Ok(Zeroizing::new(secure.as_str().to_string()))
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Image {
    yaml: Vec<u8>,
    meta: Option<Vec<u8>>,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    before: Image,
    after: Image,
}

pub struct Vault<C: Cipher> {
    pub yaml: PathBuf,
    pub meta: PathBuf,
    pub cipher: C,
    #[cfg(test)]
    fail_after_meta: std::sync::atomic::AtomicU8,
}

impl<C: Cipher> Vault<C> {
    pub fn new(yaml: PathBuf, meta: PathBuf, cipher: C) -> Self {
        Self {
            yaml,
            meta,
            cipher,
            #[cfg(test)]
            fail_after_meta: std::sync::atomic::AtomicU8::new(0),
        }
    }

    async fn guard(&self) -> Result<fs::File, String> {
        let yaml = self.yaml.clone();
        tokio::task::spawn_blocking(move || crate::secret_policy::lock(&yaml, true))
            .await
            .map_err(|_| "无法取得密钥库锁".to_string())?
    }

    fn image(&self) -> Result<Image, String> {
        let yaml = fs::read(&self.yaml).map_err(|_| "密钥库不存在或无法读取".to_string())?;
        let meta = match fs::read(&self.meta) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("无法读取元数据，已拒绝修改".into()),
        };
        Ok(Image { yaml, meta })
    }

    fn recover(&self) -> Result<(), String> {
        let path = crate::secret_policy::journal_path(&self.yaml);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("事务日志无法读取，已停止操作".into()),
        };
        let journal: Journal = serde_json::from_slice(&bytes)
            .map_err(|_| "事务日志损坏，请从备份人工恢复；原文件保留".to_string())?;
        let current = self.image()?;
        if (current.yaml != journal.before.yaml && current.yaml != journal.after.yaml)
            || (current.meta != journal.before.meta && current.meta != journal.after.meta)
        {
            return Err("事务中断后发现外部修改，已停止恢复以保护数据；请核对备份".into());
        }
        self.restore(&journal.after, &journal.before, &journal.after)?;
        fs::remove_file(path).map_err(|_| "事务已恢复，但日志未能清理，请检查目录权限".to_string())
    }

    fn restore(&self, image: &Image, before: &Image, after: &Image) -> Result<(), String> {
        let current = self.image()?;
        if (current.yaml != before.yaml && current.yaml != after.yaml)
            || (current.meta != before.meta && current.meta != after.meta)
        {
            return Err("提交期间发现外部修改，拒绝覆盖；事务及备份保留".into());
        }
        match &image.meta {
            Some(bytes) => atomic_write(&self.meta, bytes)?,
            None if self.meta.exists() => {
                fs::remove_file(&self.meta).map_err(|_| "无法恢复原元数据".to_string())?
            }
            None => (),
        }
        #[cfg(test)]
        match self
            .fail_after_meta
            .swap(0, std::sync::atomic::Ordering::SeqCst)
        {
            1 => return Err("模拟元数据提交后写入失败".into()),
            2 => {
                fs::write(&self.yaml, b"external change during commit").unwrap();
                return Err("模拟提交期间外部修改".into());
            }
            _ => (),
        }
        let current = self.image()?;
        if (current.yaml != before.yaml && current.yaml != after.yaml) || current.meta != image.meta
        {
            return Err("提交期间发现外部修改，拒绝覆盖；事务及备份保留".into());
        }
        atomic_write(&self.yaml, &image.yaml)
    }

    pub async fn metadata(&self) -> Result<(Metadata, String), String> {
        let _guard = self.guard().await?;
        self.recover()?;
        let image = self.image()?;
        Ok((parse_metadata(&image.meta)?, version(&image)))
    }

    pub async fn snapshot(&self) -> Result<Snapshot, String> {
        let _guard = self.guard().await?;
        self.recover()?;
        let before = self.image()?;
        let metadata = parse_metadata(&before.meta)?;
        let plain = self.cipher.all(&self.yaml).await?;
        let mut parsed: serde_yaml::Value =
            serde_yaml::from_str(&plain).map_err(|_| "解密后的密钥库格式无效".to_string())?;
        let mut values = Vec::new();
        if let serde_yaml::Value::Mapping(map) = &parsed {
            for (name, value) in map {
                let Some(name) = name.as_str() else {
                    continue;
                };
                if ["sops", "homebase_initialized"].contains(&name) {
                    continue;
                }
                let (state, masked) = match value.as_str() {
                    Some(value) if value.trim().is_empty() => ("empty", "(empty)".into()),
                    Some(value) => ("filled", mask(value)),
                    None => ("invalid", "(invalid)".into()),
                };
                values.push(ListedValue {
                    name: name.into(),
                    state: state.into(),
                    masked,
                });
            }
        } else {
            wipe_yaml(&mut parsed);
            return Err("密钥库顶层必须是键值表".into());
        }
        wipe_yaml(&mut parsed);
        if self.image()? != before {
            return Err("读取期间密钥库被外部修改，请刷新".into());
        }
        Ok(Snapshot {
            values,
            metadata,
            version: version(&before),
        })
    }

    pub async fn save(&self, mut request: SaveRequest) -> Result<SaveResult, String> {
        validate_request(&request)?;
        let _guard = self.guard().await?;
        self.recover()?;
        let before = self.image()?;
        let mut metadata = parse_metadata(&before.meta)?;
        let backup_id = format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.9f"),
            uuid::Uuid::new_v4()
        );
        let backup_name = format!("secrets-{backup_id}.yaml");
        let entry = metadata.entry(request.short_name.clone()).or_default();
        if entry.revision != request.expected_revision
            || version(&before) != request.expected_vault_version
        {
            return Err("密钥库已变化，未写入任何数据。请刷新后核对并重试，草稿已保留".into());
        }
        if request.operation == "rotate"
            && entry
                .rotations
                .iter()
                .any(|r| r.revoked_at.is_none() && r.rolled_back_at.is_none())
        {
            return Err("上次轮换尚未确认旧密钥停用，请先完成该步骤".into());
        }
        let now = chrono::Utc::now().to_rfc3339();
        let previous_expiry = entry.expires_at.clone();
        let previous_policy = entry.expiry_policy.clone();
        let mut new_value = request.value.take();
        if request.operation == "confirm_revocation" {
            let id = request.rotation_id.as_deref().ok_or("缺少轮换记录 ID")?;
            let rotation = entry
                .rotations
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("轮换记录已变化，请刷新")?;
            if rotation.revoked_at.is_some() || rotation.rolled_back_at.is_some() {
                return Err("该轮换已结束，请刷新".into());
            }
            rotation.revoked_at = Some(now.clone());
        } else if request.operation == "rollback_rotation" {
            let id = request.rotation_id.as_deref().ok_or("缺少轮换记录 ID")?;
            let rotation = entry
                .rotations
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("轮换记录不存在")?;
            if rotation.revoked_at.is_some() || rotation.rolled_back_at.is_some() {
                return Err("旧密钥已确认停用或轮换已撤回，不能回退".into());
            }
            if !rotation.backup_file.starts_with("secrets-")
                || !rotation.backup_file.ends_with(".yaml")
                || rotation.backup_file.contains(['/', '\\'])
            {
                return Err("轮换备份路径无效".into());
            }
            let backup = self
                .yaml
                .parent()
                .ok_or("密钥库目录无效")?
                .join("backup")
                .join(&rotation.backup_file);
            new_value = Some(self.cipher.key(&backup, &request.short_name).await?);
            entry.expires_at = rotation.previous_expiry.clone();
            entry.expiry_policy = rotation.previous_policy.clone();
            rotation.rolled_back_at = Some(now.clone());
        } else {
            patch(&mut entry.expires_at, &request.expires_at);
            patch(&mut entry.display_name, &request.display_name);
            patch(&mut entry.service_id, &request.service_id);
            patch(&mut entry.rotate_url, &request.rotate_url);
            if let Some(policy) = &request.expiry_policy {
                entry.expiry_policy = policy.clone();
            }
        }
        let mut after_yaml = before.yaml.clone();
        if let Some(value) = &new_value {
            let previous = self.cipher.key(&self.yaml, &request.short_name).await;
            let had_value = previous.as_ref().is_ok_and(|v| !v.trim().is_empty());
            if had_value && !["rotate", "rollback_rotation"].contains(&request.operation.as_str()) {
                return Err("已有密钥必须通过轮换流程替换，未修改当前值".into());
            }
            if request.operation == "rotate" {
                let previous = previous.map_err(|_| "无法读取旧密钥，拒绝轮换".to_string())?;
                if previous.trim().is_empty() {
                    return Err("当前没有已填密钥，请使用保存".into());
                }
                if previous.as_str() == value.as_str() {
                    return Err("新密钥与当前值相同，未执行轮换".into());
                }
            }
            let parent = self.yaml.parent().ok_or("密钥库目录无效")?;
            let mut staged = tempfile::Builder::new()
                .prefix(".homebase-stage-")
                .suffix(".yaml")
                .tempfile_in(parent)
                .map_err(|_| "无法创建加密暂存文件".to_string())?;
            staged
                .write_all(&before.yaml)
                .map_err(|_| "无法准备加密暂存文件".to_string())?;
            staged
                .as_file()
                .sync_all()
                .map_err(|_| "无法同步暂存文件".to_string())?;
            self.cipher
                .set(staged.path(), &request.short_name, value)
                .await?;
            let check = self.cipher.key(staged.path(), &request.short_name).await?;
            if check.as_str() != value.as_str() {
                return Err("写后逐字节校验失败，当前密钥及元数据均未修改".into());
            }
            after_yaml = fs::read(staged.path()).map_err(|_| "无法读取加密暂存结果".to_string())?;
            if request.operation == "rotate" {
                entry.rotations.push(Rotation {
                    id: uuid::Uuid::new_v4().to_string(),
                    changed_at: now.clone(),
                    validation: "user_confirmed".into(),
                    revoked_at: None,
                    rolled_back_at: None,
                    backup_file: backup_name.clone(),
                    previous_expiry,
                    previous_policy,
                });
            }
        }
        entry.revision = entry.revision.checked_add(1).ok_or("版本号溢出")?;
        entry.updated_at = Some(now);
        let revision = entry.revision;
        let after = Image {
            yaml: after_yaml,
            meta: Some(encode_metadata(&metadata, &before.meta)?),
        };
        if self.image()? != before {
            return Err("提交前检测到外部修改，未覆盖数据，请刷新".into());
        }
        let backup_dir = self.yaml.parent().ok_or("密钥库目录无效")?.join("backup");
        fs::create_dir_all(&backup_dir).map_err(|_| "无法创建备份目录，已拒绝写入".to_string())?;
        let backup = backup_dir.join(&backup_name);
        write_new(&backup, &before.yaml)?;
        if let Some(meta) = &before.meta {
            write_new(
                &backup_dir.join(format!("secrets-{backup_id}.meta.json")),
                meta,
            )?;
        }
        let journal_path = crate::secret_policy::journal_path(&self.yaml);
        atomic_write(
            &journal_path,
            &serde_json::to_vec(&Journal {
                before: before.clone(),
                after: after.clone(),
            })
            .map_err(|_| "无法准备事务日志")?,
        )?;
        if let Err(error) = self.restore(&after, &before, &after) {
            return match self.restore(&before, &before, &after) {
                Ok(()) => {
                    fs::remove_file(&journal_path)
                        .map_err(|_| "已回滚，但事务日志清理失败，请刷新".to_string())?;
                    Err(format!("保存失败，已恢复原密钥与元数据：{error}"))
                }
                Err(_) => {
                    Err("保存与回滚均未完成；已保留事务日志和备份，禁止继续使用，请刷新恢复".into())
                }
            };
        }
        fs::remove_file(journal_path)
            .map_err(|_| "数据已提交，日志清理失败，请刷新确认，不要重复提交".to_string())?;
        Ok(SaveResult {
            revision,
            vault_version: version(&after),
            backup_path: backup.to_string_lossy().into_owned(),
            verified: new_value.is_some(),
        })
    }
}

fn patch(target: &mut Option<String>, value: &Option<String>) {
    if let Some(value) = value {
        *target = (!value.is_empty()).then(|| value.clone());
    }
}
pub fn valid_name(name: &str) -> bool {
    (3..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_lowercase()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
fn validate_request(r: &SaveRequest) -> Result<(), String> {
    if !valid_name(&r.short_name) {
        return Err("短名须为 3–64 位小写字母、数字或下划线，并以字母开头".into());
    }
    if !["save", "rotate", "confirm_revocation", "rollback_rotation"]
        .contains(&r.operation.as_str())
    {
        return Err("未知保存操作".into());
    }
    if let Some(value) = &r.value {
        if value.trim().is_empty() || value.len() > 65536 {
            return Err("密钥值不能为空或超过 64 KiB".into());
        }
    }
    if r.operation == "rotate" && (r.value.is_none() || !r.candidate_verified) {
        return Err("请先验证新密钥，并明确确认它可用，再切换".into());
    }
    if r.operation == "rotate" && r.expires_at.is_none() {
        return Err("轮换时请明确填写或清空新密钥到期日期，不能静默继承旧日期".into());
    }
    if r.operation == "rollback_rotation" && (r.value.is_some() || !r.candidate_verified) {
        return Err("回退前请确认旧密钥仍然可用且未在服务商处停用".into());
    }
    if r.operation == "confirm_revocation" && r.value.is_some() {
        return Err("停用确认不能修改密钥值".into());
    }
    if let Some(value) = &r.expires_at {
        crate::secret_policy::validate_expiry(value)?;
    }
    if let Some(value) = &r.expiry_policy {
        if !["warn", "block"].contains(&value.as_str()) {
            return Err("无效的到期策略".into());
        }
    }
    if r.display_name
        .as_ref()
        .is_some_and(|s| s.chars().count() > 120)
    {
        return Err("显示名称不能超过 120 字".into());
    }
    if let Some(value) = &r.rotate_url {
        if !value.is_empty() {
            let url = reqwest::Url::parse(value).map_err(|_| "轮换链接无效")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("轮换链接必须是无登录凭据的 HTTPS 地址".into());
            }
        }
    }
    Ok(())
}
fn parse_metadata(bytes: &Option<Vec<u8>>) -> Result<Metadata, String> {
    let Some(bytes) = bytes else {
        return Ok(Metadata::new());
    };
    let raw = std::str::from_utf8(bytes).map_err(|_| "元数据编码无效，原文件保留")?;
    let metadata: Metadata = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|_| "元数据损坏，已拒绝修改，原文件保留".to_string())?;
    if metadata
        .values()
        .any(|e| !["", "warn", "block"].contains(&e.expiry_policy.as_str()))
    {
        return Err("元数据包含未知到期策略，原文件保留".into());
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    #[derive(Clone, Default)]
    struct FakeCipher {
        corrupt: Arc<AtomicBool>,
        fail: Arc<AtomicBool>,
    }
    #[async_trait]
    impl Cipher for FakeCipher {
        async fn set(&self, path: &Path, key: &str, value: &str) -> Result<(), String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("模拟加密失败".into());
            }
            let mut map: serde_json::Value =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            map[key] = serde_json::Value::String(if self.corrupt.load(Ordering::SeqCst) {
                "x".repeat(value.len())
            } else {
                value.into()
            });
            fs::write(path, serde_json::to_vec(&map).unwrap()).unwrap();
            Ok(())
        }
        async fn key(&self, path: &Path, key: &str) -> Result<Zeroizing<String>, String> {
            let bytes = fs::read(path).map_err(|_| "fixture file missing")?;
            let map: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| "fixture invalid")?;
            map[key]
                .as_str()
                .map(|s| Zeroizing::new(s.into()))
                .ok_or_else(|| "fixture key absent".into())
        }
        async fn all(&self, path: &Path) -> Result<Zeroizing<String>, String> {
            Ok(Zeroizing::new(fs::read_to_string(path).unwrap()))
        }
    }
    fn fixture() -> (tempfile::TempDir, Vault<FakeCipher>) {
        let dir = tempfile::tempdir().unwrap();
        let yaml = dir.path().join("secrets.yaml");
        let meta = dir.path().join("secrets-meta.json");
        fs::write(&yaml, br#"{"sample_key":"old-value","empty_key":"  ","invalid_key":42,"other_key":"keep-value"}"#).unwrap();
        fs::write(&meta, b"{}").unwrap();
        (dir, Vault::new(yaml, meta, FakeCipher::default()))
    }
    async fn request(vault: &Vault<FakeCipher>, value: serde_json::Value) -> SaveRequest {
        let snapshot = vault.snapshot().await.unwrap();
        let mut base = serde_json::json!({"short_name":"sample_key","expected_revision": snapshot.metadata.get("sample_key").map(|e| e.revision).unwrap_or(0),"expected_vault_version":snapshot.version});
        base.as_object_mut()
            .unwrap()
            .extend(value.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }
    #[tokio::test]
    async fn empty_invalid_and_filled_are_distinct() {
        let (_dir, vault) = fixture();
        let snapshot = vault.snapshot().await.unwrap();
        assert_eq!(
            snapshot
                .values
                .iter()
                .find(|v| v.name == "empty_key")
                .unwrap()
                .state,
            "empty"
        );
        assert_eq!(
            snapshot
                .values
                .iter()
                .find(|v| v.name == "invalid_key")
                .unwrap()
                .state,
            "invalid"
        );
        assert_eq!(
            snapshot
                .values
                .iter()
                .find(|v| v.name == "sample_key")
                .unwrap()
                .state,
            "filled"
        );
    }
    #[tokio::test]
    async fn save_new_value_and_metadata_together() {
        let (_dir, vault) = fixture();
        let r = request(&vault, serde_json::json!({"short_name":"new_key","value":"new-value","expires_at":"2027-01-01T00:00:00Z","display_name":"测试"})).await;
        let result = vault.save(r).await.unwrap();
        assert!(result.verified);
        let s = vault.snapshot().await.unwrap();
        assert_eq!(s.metadata["new_key"].revision, 1);
        assert_eq!(s.metadata["new_key"].display_name.as_deref(), Some("测试"));
        assert_eq!(
            vault
                .cipher
                .key(&vault.yaml, "other_key")
                .await
                .unwrap()
                .as_str(),
            "keep-value"
        );
        assert!(Path::new(&result.backup_path).is_file());
    }
    #[tokio::test]
    async fn same_length_corruption_is_rejected_without_modifying_originals() {
        let (_dir, vault) = fixture();
        let before = vault.image().unwrap();
        let r = request(&vault, serde_json::json!({"short_name":"new_key","value":"new-value","display_name":"not committed"})).await;
        vault.cipher.corrupt.store(true, Ordering::SeqCst);
        assert!(vault.save(r).await.err().unwrap().contains("逐字节"));
        assert!(vault.image().unwrap() == before);
    }
    #[tokio::test]
    async fn encryption_failure_preserves_originals() {
        let (_dir, vault) = fixture();
        let before = vault.image().unwrap();
        let r = request(
            &vault,
            serde_json::json!({"short_name":"new_key","value":"new-value"}),
        )
        .await;
        vault.cipher.fail.store(true, Ordering::SeqCst);
        assert!(vault.save(r).await.is_err());
        assert!(vault.image().unwrap() == before);
    }
    #[tokio::test]
    async fn existing_value_cannot_be_silently_overwritten() {
        let (_dir, vault) = fixture();
        let before = vault.image().unwrap();
        let r = request(&vault, serde_json::json!({"value":"new-value"})).await;
        assert!(vault.save(r).await.is_err());
        assert!(vault.image().unwrap() == before);
    }
    #[tokio::test]
    async fn invalid_date_and_blank_values_are_rejected_before_writes() {
        let (_dir, vault) = fixture();
        let before = vault.image().unwrap();
        for patch in [
            serde_json::json!({"expires_at":"invalid"}),
            serde_json::json!({"value":"   "}),
            serde_json::json!({"rotate_url":"http://unsafe.invalid"}),
            serde_json::json!({"expiry_policy":"invalid"}),
        ] {
            assert!(vault.save(request(&vault, patch).await).await.is_err());
            assert!(vault.image().unwrap() == before);
        }
    }
    #[tokio::test]
    async fn corrupt_metadata_is_never_replaced_by_an_empty_map() {
        let (_dir, vault) = fixture();
        let r = request(&vault, serde_json::json!({"display_name":"new"})).await;
        fs::write(&vault.meta, b"{broken").unwrap();
        let before = vault.image().unwrap();
        assert!(vault.save(r).await.is_err());
        assert!(vault.image().unwrap() == before);
        assert!(vault.snapshot().await.is_err());
    }
    #[tokio::test]
    async fn stale_revision_and_external_updates_are_rejected() {
        let (_dir, vault) = fixture();
        let r = request(&vault, serde_json::json!({"display_name":"stale"})).await;
        vault
            .save(request(&vault, serde_json::json!({"display_name":"fresh"})).await)
            .await
            .unwrap();
        let before = vault.image().unwrap();
        assert!(vault.save(r).await.is_err());
        assert!(vault.image().unwrap() == before);
        let r = request(&vault, serde_json::json!({"display_name":"stale again"})).await;
        fs::write(&vault.meta, b"{\"external\":{}}").unwrap();
        let before = vault.image().unwrap();
        assert!(vault.save(r).await.is_err());
        assert!(vault.image().unwrap() == before);
    }
    #[tokio::test]
    async fn concurrent_saves_cannot_lose_updates() {
        let (_dir, vault) = fixture();
        let a = request(&vault, serde_json::json!({"display_name":"a"})).await;
        let b = request(&vault, serde_json::json!({"display_name":"b"})).await;
        let (a, b) = tokio::join!(vault.save(a), vault.save(b));
        assert_ne!(a.is_ok(), b.is_ok());
        assert_eq!(
            vault.snapshot().await.unwrap().metadata["sample_key"].revision,
            1
        );
    }
    #[tokio::test]
    async fn commit_failure_rolls_back_both_files() {
        let (_dir, vault) = fixture();
        let before = vault.image().unwrap();
        let r = request(
            &vault,
            serde_json::json!({"short_name":"new_key","value":"new-value","display_name":"no"}),
        )
        .await;
        vault.fail_after_meta.store(1, Ordering::SeqCst);
        assert!(vault.save(r).await.err().unwrap().contains("已恢复"));
        assert!(vault.image().unwrap() == before);
        assert!(!crate::secret_policy::journal_path(&vault.yaml).exists());
    }
    #[tokio::test]
    async fn rollback_never_clobbers_an_external_change_during_commit() {
        let (_dir, vault) = fixture();
        let r = request(&vault, serde_json::json!({"display_name":"new metadata"})).await;
        vault.fail_after_meta.store(2, Ordering::SeqCst);
        assert!(vault.save(r).await.is_err());
        assert_eq!(
            fs::read(&vault.yaml).unwrap(),
            b"external change during commit"
        );
        assert!(crate::secret_policy::journal_path(&vault.yaml).exists());
    }
    #[tokio::test]
    async fn interrupted_transaction_recovers_consistent_pair() {
        let (_dir, vault) = fixture();
        let before = vault.image().unwrap();
        let after = Image {
            yaml: br#"{"sample_key":"new-value"}"#.to_vec(),
            meta: Some(br#"{"sample_key":{"revision":1}}"#.to_vec()),
        };
        let journal = Journal {
            before: before.clone(),
            after: after.clone(),
        };
        atomic_write(
            &crate::secret_policy::journal_path(&vault.yaml),
            &serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        atomic_write(&vault.meta, after.meta.as_ref().unwrap()).unwrap();
        vault.metadata().await.unwrap();
        assert!(vault.image().unwrap() == after);
    }
    #[tokio::test]
    async fn recovery_refuses_unknown_external_changes() {
        let (_dir, vault) = fixture();
        let before = vault.image().unwrap();
        let after = Image {
            yaml: br#"{"sample_key":"new-value"}"#.to_vec(),
            meta: Some(b"{}".to_vec()),
        };
        atomic_write(
            &crate::secret_policy::journal_path(&vault.yaml),
            &serde_json::to_vec(&Journal { before, after }).unwrap(),
        )
        .unwrap();
        fs::write(&vault.yaml, b"external unknown content").unwrap();
        let before = vault.image().unwrap();
        assert!(vault.metadata().await.is_err());
        assert!(vault.image().unwrap() == before);
    }
    #[tokio::test]
    async fn backups_are_unique_and_metadata_encoding_is_preserved() {
        let (_dir, vault) = fixture();
        fs::write(
            &vault.meta,
            b"\xef\xbb\xbf{\r\n  \"sample_key\": {\"custom\": 42}\r\n}\r\n",
        )
        .unwrap();
        let a = vault
            .save(request(&vault, serde_json::json!({"display_name":"a"})).await)
            .await
            .unwrap();
        let b = vault
            .save(request(&vault, serde_json::json!({"display_name":"b"})).await)
            .await
            .unwrap();
        assert_ne!(a.backup_path, b.backup_path);
        assert!(Path::new(&a.backup_path).exists());
        let bytes = fs::read(&vault.meta).unwrap();
        assert!(bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        assert!(bytes.windows(2).any(|w| w == b"\r\n"));
        assert_eq!(
            vault.snapshot().await.unwrap().metadata["sample_key"].extra["custom"],
            42
        );
    }
    #[tokio::test]
    async fn rotation_requires_validation_and_supports_safe_rollback() {
        let (_dir, vault) = fixture();
        assert!(vault
            .save(
                request(
                    &vault,
                    serde_json::json!({"operation":"rotate","value":"new-value","expires_at":""})
                )
                .await
            )
            .await
            .is_err());
        vault.save(request(&vault, serde_json::json!({"operation":"rotate","value":"new-value","expires_at":"2027-01-01T00:00:00Z","candidate_verified":true})).await).await.unwrap();
        let s = vault.snapshot().await.unwrap();
        let id = s.metadata["sample_key"].rotations[0].id.clone();
        vault.save(request(&vault, serde_json::json!({"operation":"rollback_rotation","rotation_id":id,"candidate_verified":true})).await).await.unwrap();
        assert_eq!(
            vault
                .cipher
                .key(&vault.yaml, "sample_key")
                .await
                .unwrap()
                .as_str(),
            "old-value"
        );
        assert_eq!(
            vault
                .cipher
                .key(&vault.yaml, "other_key")
                .await
                .unwrap()
                .as_str(),
            "keep-value"
        );
        assert!(
            vault.snapshot().await.unwrap().metadata["sample_key"].rotations[0]
                .rolled_back_at
                .is_some()
        );
    }
    #[tokio::test]
    async fn revoked_rotation_cannot_be_rolled_back() {
        let (_dir, vault) = fixture();
        vault.save(request(&vault, serde_json::json!({"operation":"rotate","value":"new-value","expires_at":"","candidate_verified":true})).await).await.unwrap();
        let id = vault.snapshot().await.unwrap().metadata["sample_key"].rotations[0]
            .id
            .clone();
        vault
            .save(
                request(
                    &vault,
                    serde_json::json!({"operation":"confirm_revocation","rotation_id":id}),
                )
                .await,
            )
            .await
            .unwrap();
        assert!(vault.save(request(&vault, serde_json::json!({"operation":"rollback_rotation","rotation_id":id,"candidate_verified":true})).await).await.is_err());
        assert_eq!(
            vault
                .cipher
                .key(&vault.yaml, "sample_key")
                .await
                .unwrap()
                .as_str(),
            "new-value"
        );
    }
}
fn encode_metadata(metadata: &Metadata, previous: &Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut text = serde_json::to_string_pretty(metadata).map_err(|_| "元数据无法序列化")?;
    text.push('\n');
    if let Some(old) = previous {
        if old.windows(2).any(|w| w == b"\r\n") {
            text = text.replace('\n', "\r\n");
        }
        if old.starts_with(&[0xef, 0xbb, 0xbf]) {
            text.insert(0, '\u{feff}');
        }
    }
    Ok(text.into_bytes())
}
fn version(image: &Image) -> String {
    let mut hash = Sha256::new();
    hash.update((image.yaml.len() as u64).to_le_bytes());
    hash.update(&image.yaml);
    if let Some(bytes) = &image.meta {
        hash.update([1]);
        hash.update(bytes);
    } else {
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("文件目录无效")?)
        .map_err(|_| "无法创建原子写入临时文件")?;
    if let Ok(metadata) = fs::metadata(path) {
        file.as_file()
            .set_permissions(metadata.permissions())
            .map_err(|_| "无法保留文件权限")?;
    }
    file.write_all(bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| "文件写入或同步失败")?;
    #[cfg(windows)]
    if path.exists() {
        use std::os::windows::ffi::OsStrExt;
        let staged_path = file.into_temp_path(); // ReplaceFile requires closed replacement handles.
        let target: Vec<u16> = path
            .canonicalize()
            .map_err(|_| "无法解析原文件")?
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let source: Vec<u16> = staged_path
            .canonicalize()
            .map_err(|_| "无法解析暂存文件")?
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        // Flags 0 preserves/merges the original ACL; never ignore ACL failures.
        let replaced = unsafe {
            windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
                target.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if replaced == 0 {
            return Err(format!(
                "无法原子替换或保留原文件权限（系统错误码 {:?}），事务及备份保留",
                std::io::Error::last_os_error().raw_os_error()
            ));
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    if path.exists() {
        file.persist(path)
            .map_err(|_| "无法原子替换文件，原文件保留")?;
        return Ok(());
    }
    file.persist_noclobber(path)
        .map_err(|_| "无法原子提交文件，原文件保留")?;
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "无法创建唯一备份，已拒绝写入")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "备份写入失败，已拒绝写入".into())
}
fn mask(value: &str) -> String {
    if value.chars().count() < 16 {
        return "********".into();
    }
    format!(
        "{}…****…{}",
        value.chars().take(7).collect::<String>(),
        value
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<String>()
    )
}
fn wipe_yaml(value: &mut serde_yaml::Value) {
    match value {
        serde_yaml::Value::String(s) => s.zeroize(),
        serde_yaml::Value::Sequence(s) => s.iter_mut().for_each(wipe_yaml),
        serde_yaml::Value::Mapping(m) => m.values_mut().for_each(wipe_yaml),
        _ => (),
    }
}
