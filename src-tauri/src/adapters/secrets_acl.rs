//! Secrets ACL · S3 · 2026-05-06
//!
//! 管理 `~/.homebase/shared/secrets-acl.json` · 记录 USER 对 (agent_id, secret_name) 的永久授权。
//!
//! 设计：
//! - **粒度**：复合 key `{agent_id}:{secret_name}` 唯一标识
//! - **状态**：仅记 "永久允许"（allow=true）。"deny" / "allow_once" 不写盘 · 下次还问
//! - **审计**：记 granted_at + purpose（USER 当时看到的描述 · 备查）
//! - **撤销**：UI revoke = 删 entry · 下次再访问会重新弹窗
//!
//! 文件格式（JSON · 易看易编辑）：
//! ```json
//! {
//!   "version": 1,
//!   "entries": {
//!     "claude-code:OPENROUTER_API_KEY": {
//!       "granted_at": "2026-05-06T15:00:00+08:00",
//!       "purpose": "Studio agent invokes OpenRouter API"
//!     }
//!   }
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::homebase_home_dir;

/// ACL 文件路径 · `~/.homebase/shared/secrets-acl.json`
pub fn acl_file_path() -> PathBuf {
    homebase_home_dir().join(".homebase").join("shared").join("secrets-acl.json")
}

/// 单条永久授权
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AclEntry {
    /// 授予时刻（ISO 8601 with offset）
    pub granted_at: String,
    /// USER 同意时看到的 purpose 描述（agent 提交的）
    #[serde(default)]
    pub purpose: String,
}

/// ACL 文件根结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclFile {
    pub version: u32,
    /// key = `{agent_id}:{secret_name}`
    #[serde(default)]
    pub entries: BTreeMap<String, AclEntry>,
}

impl Default for AclFile {
    fn default() -> Self {
        Self {
            version: 1,
            entries: BTreeMap::new(),
        }
    }
}

impl AclFile {
    /// 复合 key
    pub fn make_key(agent_id: &str, secret_name: &str) -> String {
        format!("{agent_id}:{secret_name}")
    }

    /// 是否已永久授权
    pub fn is_allowed(&self, agent_id: &str, secret_name: &str) -> bool {
        self.entries
            .contains_key(&Self::make_key(agent_id, secret_name))
    }

    /// 授予永久允许
    pub fn grant(&mut self, agent_id: &str, secret_name: &str, purpose: impl Into<String>) {
        let key = Self::make_key(agent_id, secret_name);
        let entry = AclEntry {
            granted_at: chrono::Local::now().to_rfc3339(),
            purpose: purpose.into(),
        };
        self.entries.insert(key, entry);
    }

    /// 撤销
    pub fn revoke(&mut self, agent_id: &str, secret_name: &str) -> bool {
        self.entries
            .remove(&Self::make_key(agent_id, secret_name))
            .is_some()
    }
}

/// 加载 ACL · 文件不存在或解析失败时返回默认空
pub fn load_checked() -> Result<AclFile, String> {
    crate::query::optional_json(&acl_file_path())
}

pub fn load() -> AclFile {
    let path = acl_file_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return AclFile::default(),
    };
    match serde_json::from_str::<AclFile>(&text) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                "secrets-acl.json 解析失败 · 用空 ACL 兜底 · 不覆盖原文件: {e}"
            );
            AclFile::default()
        }
    }
}

/// 保存 ACL · 自动创建父目录
pub fn save(acl: &AclFile) -> std::io::Result<()> {
    let path = acl_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(acl)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::shared_context::tests::TEST_ENV_LOCK;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _g = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());
        f();
        match prev {
            Some(p) => std::env::set_var("HOMEBASE_TEST_HOME", p),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }

    #[test]
    fn make_key_format() {
        assert_eq!(
            AclFile::make_key("claude-code", "OPENROUTER_API_KEY"),
            "claude-code:OPENROUTER_API_KEY"
        );
    }

    #[test]
    fn grant_revoke_roundtrip() {
        let mut acl = AclFile::default();
        assert!(!acl.is_allowed("codex", "OPENAI_API_KEY"));
        acl.grant("codex", "OPENAI_API_KEY", "调 OpenAI");
        assert!(acl.is_allowed("codex", "OPENAI_API_KEY"));
        let revoked = acl.revoke("codex", "OPENAI_API_KEY");
        assert!(revoked);
        assert!(!acl.is_allowed("codex", "OPENAI_API_KEY"));
        // 再次撤销返回 false
        assert!(!acl.revoke("codex", "OPENAI_API_KEY"));
    }

    #[test]
    fn load_missing_returns_default() {
        with_temp_home(|| {
            let acl = load();
            assert_eq!(acl.version, 1);
            assert!(acl.entries.is_empty());
        });
    }

    #[test]
    fn save_then_load_roundtrip() {
        with_temp_home(|| {
            let mut acl = AclFile::default();
            acl.grant("claude-code", "OPENROUTER_API_KEY", "studio chat");
            acl.grant("gemini-cli", "GEMINI_API_KEY", "code review");
            save(&acl).unwrap();

            let loaded = load();
            assert_eq!(loaded.entries.len(), 2);
            assert!(loaded.is_allowed("claude-code", "OPENROUTER_API_KEY"));
            assert!(loaded.is_allowed("gemini-cli", "GEMINI_API_KEY"));
            // purpose 保留
            let key = AclFile::make_key("claude-code", "OPENROUTER_API_KEY");
            assert_eq!(loaded.entries[&key].purpose, "studio chat");
        });
    }

    #[test]
    fn corrupted_file_yields_empty() {
        with_temp_home(|| {
            let path = acl_file_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "{ not valid json").unwrap();
            let loaded = load();
            // 损坏不抛异常 · 返回空
            assert!(loaded.entries.is_empty());
            // 重要：不会主动覆盖 USER 的损坏文件 · USER 自己排查
            let on_disk = std::fs::read_to_string(&path).unwrap();
            assert!(on_disk.starts_with("{ not valid"));
        });
    }
}
