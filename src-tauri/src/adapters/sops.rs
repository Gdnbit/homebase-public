//! SOPS 适配器 — 调 C:\HomeBase\Tools\sops\bin\sops.exe（v3 §10.1 Secrets.list 来源）
//!
//! 安全约束（v3 §12.2）：
//! - 解密结果仅内存 + Drop 时 zeroize
//! - `SecureString` 的 `Debug` 返回 `"[redacted]"`（防止意外 `{:?}` 打印 key）
//! - 不实现 `Display`（强制调用方显式 `.as_str()` 表达意图）
//!
//! 2026-05-03 增量（Phase 13.1 对齐桌面端 WPF）：
//! - `set_key` — sops set --value-stdin 加密回写暂存文件，密钥不进入命令行
//! - `backup_file` — 写入前强制备份到 C:\HomeBase\Secrets\backup\

use super::{AdapterError, AdapterResult, DEFAULT_TIMEOUT};
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

const SOPS_EXE: &str = "C:\\HomeBase\\Tools\\sops\\bin\\sops.exe";

fn conventional_age_identity_path(yaml_path: &Path) -> Option<PathBuf> {
    let sibling = yaml_path.parent()?.join(".age-keys").join("primary.txt");
    if sibling.is_file() {
        return Some(sibling);
    }
    let home = dirs::home_dir()?;
    [
        home.join(".config")
            .join("sops")
            .join("age")
            .join("keys.txt"),
        home.join("AppData")
            .join("Roaming")
            .join("sops")
            .join("age")
            .join("keys.txt"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn sops_child_environment(yaml_path: &Path) -> Vec<(String, String)> {
    const IDENTITY_ENV_NAMES: &[&str] = &[
        "SOPS_AGE_KEY_FILE",
        "SOPS_AGE_KEY",
        "SOPS_AGE_KEY_CMD",
        "SOPS_AGE_SSH_PRIVATE_KEY_FILE",
        "SOPS_AGE_SSH_PRIVATE_KEY_CMD",
    ];
    if IDENTITY_ENV_NAMES
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return Vec::new();
    }
    let explicit = std::env::var_os("HOMEBASE_SOPS_AGE_KEY_FILE")
        .map(PathBuf::from)
        .filter(|path| path.is_file());
    explicit
        .or_else(|| conventional_age_identity_path(yaml_path))
        .map(|path| {
            vec![(
                "SOPS_AGE_KEY_FILE".to_string(),
                path.to_string_lossy().into_owned(),
            )]
        })
        .unwrap_or_default()
}

/// SOPS 适配器
pub struct SopsAdapter {
    sops_exe: String,
    identity: Option<PathBuf>,
    config: Option<PathBuf>,
    identity_base: Option<PathBuf>,
}

impl SopsAdapter {
    pub fn new() -> Self {
        Self {
            sops_exe: std::env::var("HOMEBASE_SOPS_EXE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| SOPS_EXE.to_string()),
            identity: None,
            identity_base: None,
            config: std::env::var_os("HOMEBASE_SOPS_CONFIG").filter(|s| !s.is_empty()).map(PathBuf::from),
        }
    }

    /// 允许自定义 sops.exe 路径（测试/替换方便）
    pub fn with_exe(path: impl Into<String>) -> Self {
        Self {
            sops_exe: path.into(),
            identity: None,
            identity_base: None,
            config: None,
        }
    }

    /// Explicit identity for isolated vaults; never inherits a user's age identity.
    pub fn with_identity(exe: impl Into<String>, identity: PathBuf, config: PathBuf) -> Self {
        Self { sops_exe: exe.into(), identity: Some(identity), config: Some(config), identity_base: None }
    }

    /// Staged files and rotation backups use the original vault's identity location.
    pub fn for_vault(yaml: &Path) -> Self {
        let mut adapter = Self::new();
        adapter.identity_base = Some(yaml.to_path_buf());
        adapter
    }

    fn child_environment(&self, yaml_path: &Path) -> Vec<(String, String)> {
        if let Some(identity) = &self.identity {
            let home = identity.parent().unwrap_or(Path::new(".")).to_string_lossy().into_owned();
            let mut env = vec![("SOPS_AGE_KEY_FILE".into(), identity.to_string_lossy().into_owned())];
            for name in ["SystemRoot", "WINDIR", "PATH"] { if let Some(value) = std::env::var_os(name) { env.push((name.into(), value.to_string_lossy().into_owned())); } }
            for name in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "TEMP", "TMP"] { env.push((name.into(), home.clone())); }
            env.push(("HTTP_PROXY".into(), "http://127.0.0.1:1".into()));
            env.push(("HTTPS_PROXY".into(), "http://127.0.0.1:1".into()));
            env
        } else {
        sops_child_environment(self.identity_base.as_deref().unwrap_or(yaml_path))
        }
    }

    async fn run(&self, yaml_path: &Path, args: &[&str], input: Option<&[u8]>) -> AdapterResult<super::CommandOutput> {
        let child_env = self.child_environment(yaml_path);
        let remove: &[&str] = if self.identity.is_some() { &["SOPS_AGE_KEY", "SOPS_AGE_KEY_CMD", "SOPS_AGE_SSH_PRIVATE_KEY_FILE", "SOPS_AGE_SSH_PRIVATE_KEY_CMD", "HOMEBASE_SOPS_AGE_KEY_FILE"] } else { &[] };
        let mut actual_args: Vec<&str> = Vec::new();
        let config = self.config.as_ref().map(|p| p.to_string_lossy().into_owned());
        if let Some(config) = &config { actual_args.extend(["--config", config.as_str()]); }
        actual_args.extend_from_slice(args);
        let mut out = super::spawn_with_timeout_env_input_remove(&self.sops_exe, actual_args, self.identity.as_ref().and_then(|p| p.parent()), &child_env, remove, self.identity.is_some(), input, DEFAULT_TIMEOUT).await?;
        if !out.is_success() { out.stdout.zeroize(); out.stderr.zeroize(); return Err(AdapterError::NonZeroExit { program: "sops".into(), code: out.exit_code, stderr: "SOPS 操作失败，敏感输出已隐藏".into() }); }
        Ok(out)
    }

    /// 解密整个 secrets.yaml 到 SecureString（自动 zeroize）
    pub async fn decrypt_file(&self, yaml_path: &Path) -> AdapterResult<SecureString> {
        let path_str = yaml_path.to_string_lossy().to_string();
        let out = self.run(yaml_path, &["--decrypt", &path_str], None).await?;
        Ok(SecureString::new(out.stdout))
    }

    /// 解密单个 top-level key（`--extract '["<key>"]'`）
    ///
    /// 适合 Secrets.test 场景：只取一个 key 验证，避免把整个文件读入内存
    pub async fn decrypt_key(
        &self,
        yaml_path: &Path,
        key_name: &str,
    ) -> AdapterResult<SecureString> {
        let policy_path = yaml_path.to_path_buf(); let policy_name = key_name.to_string();
        let _guard = tokio::task::spawn_blocking(move || crate::secret_policy::check_access(&policy_path, &policy_name)).await
            .map_err(|_| AdapterError::Io { program: "secret_policy".into(), source: std::io::Error::new(std::io::ErrorKind::Other, "无法执行密钥策略检查") })?
            .map_err(|message| AdapterError::Io {
            program: "secret_policy".into(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, message),
        })?;
        self.decrypt_key_unchecked(yaml_path, key_name).await
    }

    /// Internal staging verification only. Consumers must use decrypt_key.
    pub(crate) async fn decrypt_key_unchecked(&self, yaml_path: &Path, key_name: &str) -> AdapterResult<SecureString> {
        crate::secret_policy::require_encrypted_string(yaml_path, key_name).map_err(|message| AdapterError::Io { program: "secret type".into(), source: std::io::Error::new(std::io::ErrorKind::InvalidData, message) })?;
        let path_str = yaml_path.to_string_lossy().to_string();
        let extract_expr = serde_json::to_string(&[key_name]).expect("string array");
        let out = self.run(yaml_path, &["--decrypt", "--output-type", "json", "--extract", &extract_expr, &path_str], None).await?;
        let raw = zeroize::Zeroizing::new(out.stdout);
        // SOPS emits a scalar string verbatim even with JSON output selected.
        // Its authenticated encrypted type was checked above; do not trim bytes.
        let mut value = raw;
        if value.trim().is_empty() { return Err(AdapterError::Io { program: "sops extract".into(), source: std::io::Error::new(std::io::ErrorKind::InvalidData, "密钥值为空") }); }
        Ok(SecureString::new(std::mem::take(&mut *value)))
    }

    /// Encrypt one value via `sops set --value-stdin`; plaintext is never argv.
    ///
    /// 绕开 secrets.cmd `>30 字符截断 bug`（参数以 JSON-encoded 整体传递，不拆分）。
    ///
    /// 安全约束：
    /// - JSON value is sent only through a bounded child stdin pipe.
    /// - 调用方应尽快 drop 明文变量（本函数不持有长生命周期拷贝）
    /// - **调用方必须在调用前 backup_file，并在调用后 verify round-trip**
    pub async fn set_key(
        &self,
        yaml_path: &Path,
        key_name: &str,
        value: &str,
    ) -> AdapterResult<()> {
        crate::secret_policy::require_encryption_scope(yaml_path, key_name).map_err(|message| AdapterError::Io { program: "SOPS encryption scope".into(), source: std::io::Error::new(std::io::ErrorKind::InvalidInput, message) })?;
        let path_str = yaml_path.to_string_lossy().to_string();
        // sops --set 语法：'[index_path] json_value'
        // JSON 编码 value 避免引号 / 特殊字符歧义
        let json_value = zeroize::Zeroizing::new(serde_json::to_string(value).map_err(|e| super::AdapterError::Io {
            program: self.sops_exe.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("JSON encode: {e}"),
            ),
        })?);
        let index = serde_json::to_string(&[key_name]).expect("string array is serializable");
        let out = self.run(yaml_path, &["set", "--value-stdin", &path_str, &index], Some(json_value.as_bytes())).await?;
        if !out.is_success() {
            return Err(AdapterError::NonZeroExit { program: "sops set".into(), code: out.exit_code, stderr: "加密写入失败（敏感输出已隐藏）".into() });
        }
        // sops --set 成功时通常 stdout 为空 / stderr 有少量 info；exit 0 即成功
        tracing::debug!(
            "sops --set 成功: key={} stdout_len={}",
            key_name,
            out.stdout.len()
        );
        Ok(())
    }

    /// 备份 secrets.yaml 到 C:\HomeBase\Secrets\backup\secrets-<yyyyMMddHHmmss>.yaml
    ///
    /// 写入前必须调用；返回备份文件路径（便于失败回滚）
    pub fn backup_file(&self, yaml_path: &Path) -> AdapterResult<PathBuf> {
        let parent = yaml_path.parent().ok_or_else(|| super::AdapterError::Io {
            program: "backup_file".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "yaml_path 无父目录"),
        })?;
        let backup_dir = parent.join("backup");
        std::fs::create_dir_all(&backup_dir).map_err(|e| super::AdapterError::Io {
            program: "backup_file".to_string(),
            source: e,
        })?;
        let ts = chrono::Utc::now().format("%Y%m%d%H%M%S%.9f");
        let fname = format!("secrets-{}-{}.yaml", ts, uuid::Uuid::new_v4());
        let backup_path = backup_dir.join(fname);
        std::fs::copy(yaml_path, &backup_path).map_err(|e| super::AdapterError::Io {
            program: "backup_file".to_string(),
            source: e,
        })?;
        tracing::info!("secrets.yaml 已备份至 {:?}", backup_path);
        Ok(backup_path)
    }
}

impl Default for SopsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// 列出加密 yaml 的顶层 key 名（不解密 · 不需要 age key）
///
/// SOPS 文件结构：顶层 key 是明文 · value 才加密 · 还有一个 `sops` metadata 段我们要剔除。
/// 这个函数不依赖 SopsAdapter / sops.exe · 纯文件解析。
pub fn list_top_keys(yaml_path: &Path) -> AdapterResult<Vec<String>> {
    let text = std::fs::read_to_string(yaml_path).map_err(|e| AdapterError::Io {
        program: "list_top_keys".to_string(),
        source: e,
    })?;
    let parsed: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| AdapterError::Io {
        program: "list_top_keys".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, format!("yaml parse: {e}")),
    })?;
    let mapping = match parsed.as_mapping() {
        Some(m) => m,
        None => return Ok(Vec::new()),
    };
    let mut keys: Vec<String> = mapping
        .iter()
        .filter_map(|(k, _v)| k.as_str().map(|s| s.to_string()))
        .filter(|k| k != "sops") // 剔除 SOPS metadata 段
        .collect();
    keys.sort();
    Ok(keys)
}

/// 自动 zeroize 的字符串容器（v3 §12.2）
///
/// 安全属性：
/// - `Drop` 时调用 `zeroize::Zeroize::zeroize` 覆写内存为 0
/// - `Debug` 返回 `"[redacted]"`，不泄漏内容
/// - 不实现 `Display`，强制调用方用 `.as_str()` 显式取内容
pub struct SecureString(String);

impl SecureString {
    pub fn new(s: String) -> Self {
        Self(s)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SecureString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SecureString").field(&"[redacted]").finish()
    }
}

impl Drop for SecureString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SECRETS_YAML: &str = r"C:\HomeBase\Secrets\secrets.yaml";

    /// 检查 age key 是否在环境里（SOPS 解密前置）
    fn has_age_key() -> bool {
        std::env::var("SOPS_AGE_KEY_FILE").is_ok() || std::env::var("SOPS_AGE_KEY").is_ok()
    }

    #[test]
    fn secure_string_debug_is_redacted() {
        let s = SecureString::new("super-secret-api-key".to_string());
        let debug = format!("{:?}", s);
        assert!(
            !debug.contains("super-secret"),
            "Debug leaked key: {}",
            debug
        );
        assert!(
            debug.contains("redacted"),
            "Debug should contain 'redacted': {}",
            debug
        );
    }

    #[test]
    fn secure_string_len_and_empty() {
        let s = SecureString::new("abc".to_string());
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
        let e = SecureString::new(String::new());
        assert!(e.is_empty());
    }

    #[test]
    fn conventional_identity_is_resolved_next_to_secrets_file() {
        let temp = tempfile::tempdir().unwrap();
        let yaml = temp.path().join("secrets.yaml");
        let identity = temp.path().join(".age-keys").join("primary.txt");
        std::fs::create_dir_all(identity.parent().unwrap()).unwrap();
        std::fs::write(&identity, "test identity placeholder").unwrap();
        assert_eq!(conventional_age_identity_path(&yaml), Some(identity));
    }

    /// 解密整个 secrets.yaml
    /// 需要环境有 SOPS_AGE_KEY_FILE 或 SOPS_AGE_KEY；否则跳过
    #[tokio::test]
    async fn decrypt_file_works_with_age_key() {
        if !has_age_key() {
            eprintln!(
                "[SKIP] decrypt_file_works_with_age_key: no SOPS_AGE_KEY_FILE/SOPS_AGE_KEY in env"
            );
            return;
        }
        let yaml = PathBuf::from(SECRETS_YAML);
        if !yaml.exists() {
            eprintln!("[SKIP] decrypt_file: {} not found", SECRETS_YAML);
            return;
        }
        let adapter = SopsAdapter::new();
        let decrypted = adapter.decrypt_file(&yaml).await.expect("decrypt succeeds");
        assert!(!decrypted.is_empty(), "decrypted yaml should have content");
        // 合理性：yaml 至少应该 > 100 字节
        assert!(
            decrypted.len() > 100,
            "decrypted yaml unexpectedly short: {} bytes",
            decrypted.len()
        );
    }

    /// decrypt_key fail on missing key — 即使没 age key 也能测（sops 会报 "no match" 前还要解密，所以实际上仍需 age key）
    #[tokio::test]
    async fn decrypt_key_with_nonexistent_key_errors() {
        if !has_age_key() {
            eprintln!("[SKIP] decrypt_key_with_nonexistent_key_errors: no age key");
            return;
        }
        let yaml = PathBuf::from(SECRETS_YAML);
        if !yaml.exists() {
            eprintln!("[SKIP] decrypt_key: {} not found", SECRETS_YAML);
            return;
        }
        let adapter = SopsAdapter::new();
        let res = adapter
            .decrypt_key(&yaml, "___definitely_not_a_real_key___")
            .await;
        // 期待 NonZeroExit（sops 找不到该 key 会报错退出）
        assert!(
            matches!(res, Err(AdapterError::NonZeroExit { .. })),
            "expected NonZeroExit, got {:?}",
            res.map(|_| "OK").unwrap_or("Err")
        );
    }
}
