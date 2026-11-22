//! Shared by the app and reveal helper. No decrypted secret is read here.
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub fn lock_path(yaml: &Path) -> PathBuf {
    yaml.with_extension("homebase.lock")
}
pub fn journal_path(yaml: &Path) -> PathBuf {
    yaml.with_extension("homebase-transaction.json")
}
pub fn vault_path() -> PathBuf {
    std::env::var_os("HOMEBASE_SECRETS_FILE")
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var_os("HOMEBASE_SECRETS_YAML").filter(|s| !s.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\HomeBase\Secrets\secrets.yaml"))
}

pub fn lock(yaml: &Path, exclusive: bool) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path(yaml))
        .map_err(|_| "无法锁定密钥库，请检查目录权限".to_string())?;
    let start = Instant::now();
    loop {
        let result = if exclusive {
            FileExt::try_lock_exclusive(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        if result.is_ok() {
            return Ok(file);
        }
        if start.elapsed() >= Duration::from_secs(3) {
            return Err("密钥库正在被其他操作使用，请稍后重试".into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn metadata_path(yaml: &Path) -> PathBuf {
    let configured = vault_path();
    if configured == yaml {
        if let Some(path) = std::env::var_os("HOMEBASE_SECRETS_META").filter(|s| !s.is_empty()) {
            return PathBuf::from(path);
        }
    }
    yaml.with_file_name("secrets-meta.json")
}

pub fn validate_expiry(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Ok(());
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| "到期时间必须是含时区的有效 ISO 8601 时间".into())
}

pub fn require_encrypted_string(yaml: &Path, name: &str) -> Result<(), String> {
    let raw = std::fs::read_to_string(yaml).map_err(|_| "无法检查密钥类型")?;
    let parsed: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|_| "密钥库格式无效")?;
    let Some(value) = parsed.get(name).and_then(|v| v.as_str()) else {
        return Err("密钥不存在或不是字符串".into());
    };
    if !value.starts_with("ENC[") || !value.ends_with(",type:str]") {
        return Err("密钥必须是 SOPS 加密的字符串，已拒绝提供".into());
    }
    Ok(())
}

pub fn require_encryption_scope(yaml: &Path, name: &str) -> Result<(), String> {
    let raw = std::fs::read_to_string(yaml).map_err(|_| "无法检查加密范围")?;
    let parsed: serde_yaml::Value = serde_yaml::from_str(&raw).map_err(|_| "密钥库格式无效")?;
    let meta = parsed.get("sops").ok_or("缺少 SOPS 元数据")?;
    if meta
        .get("unencrypted_suffix")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty() && name.ends_with(s))
    {
        return Err("该短名位于不加密范围，已拒绝写入明文".into());
    }
    if meta
        .get("encrypted_suffix")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty() && !name.ends_with(s))
    {
        return Err("该短名不在加密范围，已拒绝写入".into());
    }
    for (key, must_match) in [("encrypted_regex", true), ("unencrypted_regex", false)] {
        if let Some(pattern) = meta
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            let regex = regex::Regex::new(pattern)
                .map_err(|_| "无法安全判断库的自定义加密规则，已拒绝写入")?;
            if regex.is_match(name) != must_match {
                return Err("该短名不在加密范围，已拒绝写入".into());
            }
        }
    }
    if ["encrypted_comment_regex", "unencrypted_comment_regex"]
        .iter()
        .any(|key| {
            meta.get(*key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        })
    {
        return Err("库使用注释加密规则，无法安全确定该值是否加密，已拒绝自动写入".into());
    }
    Ok(())
}

pub fn enforce_entry(
    entry: &serde_json::Value,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    if !entry.is_object() {
        return Err("该密钥元数据格式无效，已停止提供".into());
    }
    if entry
        .get("expiry_policy")
        .is_some_and(|v| !v.is_null() && !v.is_string())
    {
        return Err("密钥到期策略无效，已停止提供".into());
    }
    if entry
        .get("expires_at")
        .is_some_and(|v| !v.is_null() && !v.is_string())
    {
        return Err("密钥到期日期格式无效，已停止提供".into());
    }
    match entry
        .get("expiry_policy")
        .and_then(|v| v.as_str())
        .unwrap_or("")
    {
        "" | "warn" => return Ok(()),
        "block" => (),
        _ => return Err("密钥到期策略无效，已停止提供，请先修正".into()),
    }
    let Some(value) = entry
        .get("expires_at")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return Ok(());
    };
    let expiry = chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| "密钥到期时间无效，已停止提供，请先修正日期".to_string())?;
    if expiry <= now {
        return Err("密钥已到期，已按设置停止提供；请轮换或更新日期".into());
    }
    Ok(())
}

/// Returned file holds a shared OS lock for the consumer's entire decryption.
pub fn check_access(yaml: &Path, name: &str) -> Result<File, String> {
    let guard = lock(yaml, false)?;
    if journal_path(yaml).exists() {
        return Err("密钥库有未完成事务，请先在 HomeBase 刷新以恢复".into());
    }
    let path = metadata_path(yaml);
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let parsed: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(raw.trim_start_matches('\u{feff}'))
                    .map_err(|_| "密钥元数据损坏，已拒绝使用，原文件保留".to_string())?;
            if let Some(entry) = parsed.get(name) {
                enforce_entry(entry, chrono::Utc::now())?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err("无法读取密钥到期策略，已拒绝使用".into()),
    }
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expiry_policy_preserves_legacy_warn_and_blocks_only_when_requested() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-08T00:00:00Z")
            .unwrap()
            .to_utc();
        assert!(enforce_entry(
            &serde_json::json!({"expires_at":"2020-01-01T00:00:00Z"}),
            now
        )
        .is_ok());
        assert!(enforce_entry(
            &serde_json::json!({"expiry_policy":"block","expires_at":"2020-01-01T00:00:00Z"}),
            now
        )
        .is_err());
        assert!(enforce_entry(
            &serde_json::json!({"expiry_policy":"block","expires_at":"invalid"}),
            now
        )
        .is_err());
        assert!(enforce_entry(
            &serde_json::json!({"expiry_policy":"block","expires_at":"2027-01-01T00:00:00Z"}),
            now
        )
        .is_ok());
    }

    #[test]
    fn invalid_policy_types_fail_closed_and_expiry_boundary_is_exact() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-08T00:00:00Z")
            .unwrap()
            .to_utc();
        for entry in [
            serde_json::json!(null),
            serde_json::json!({"expiry_policy":false}),
            serde_json::json!({"expires_at":42}),
            serde_json::json!({"expiry_policy":"unknown"}),
            serde_json::json!({"expiry_policy":"block","expires_at":"2026-11-08T00:00:00Z"}),
        ] {
            assert!(enforce_entry(&entry, now).is_err());
        }
        assert!(enforce_entry(
            &serde_json::json!({"expiry_policy":"block","expires_at":"2026-11-08T00:00:00.001Z"}),
            now
        )
        .is_ok());
        assert!(validate_expiry("2026-11-08").is_err());
        assert!(validate_expiry("2026-02-29T00:00:00Z").is_err());
    }
}
