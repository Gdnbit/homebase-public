//! Secrets Store · S3 · 2026-05-06
//!
//! 高层 facade · 包装 SOPS 加密文件（`C:\HomeBase\Secrets\secrets.yaml`）。
//!
//! 职责：
//! - **list**：列已有 secret 名（不解密 · 直接读 yaml 顶层 key）
//! - **get**：解密单个 secret · 返 `SecureString`（自动 zeroize）
//! - **set**：加密回写单个 secret（先 backup · 再 sops --set）
//!
//! 不负责：
//! - **delete**：暂不支持 · USER 用 `sops <yaml>` 自己编辑删（v3.7+ 才有 unset）
//! - **ACL**：见 `secrets_acl` 模块
//!
//! 路径与 key 名：
//! - 默认 yaml 路径与 secret_policy 的统一解析一致
//! - 测试可通过 env `HOMEBASE_SECRETS_YAML` 覆盖
//! - secret 名 = yaml 顶层 key 字面量（`openrouter_key` / `gemini_key` 等 USER 自定）

use super::sops::{list_top_keys, SecureString, SopsAdapter};
use super::AdapterResult;
use std::path::PathBuf;

/// 解析 secrets.yaml 路径，复用桌面端与 helper 的统一优先级。
pub fn secrets_yaml_path() -> PathBuf {
    crate::secret_policy::vault_path()
}

/// 列出 secret 名（不解密）
///
/// 返回排序好的 yaml 顶层 key 列表 · 已剔除 SOPS metadata 段。
/// 文件不存在返空 vec（前端 UI 提示「请创建 secrets.yaml」）。
pub fn list_secret_names() -> AdapterResult<Vec<String>> {
    let path = secrets_yaml_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    list_top_keys(&path)
}

/// 解密单 secret · 返 SecureString（drop 时 zeroize）
pub async fn get_secret_value(name: &str) -> Result<SecureString, String> {
    let path = secrets_yaml_path();
    if !path.exists() {
        return Err(format!(
            "SOPS yaml 不存在: {} · 请先创建 secrets.yaml",
            path.display()
        ));
    }
    let adapter = SopsAdapter::new();
    adapter
        .decrypt_key(&path, name)
        .await
        .map_err(|e| format!("解密 {name} 失败: {e}"))
}

/// 加密回写单 secret · 先 backup
///
/// 旧非版本化写入入口已停用；使用密钥管理事务接口。
pub async fn set_secret_value(_name: &str, _value: &str) -> Result<(), String> {
    Err("旧写入入口已停用；请通过带版本检查的密钥管理保存流程操作".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_path_env_override() {
        let prev = std::env::var("HOMEBASE_SECRETS_YAML").ok();
        std::env::set_var("HOMEBASE_SECRETS_YAML", r"C:\tmp\custom-secrets.yaml");
        let p = secrets_yaml_path();
        assert_eq!(p, PathBuf::from(r"C:\tmp\custom-secrets.yaml"));
        match prev {
            Some(v) => std::env::set_var("HOMEBASE_SECRETS_YAML", v),
            None => std::env::remove_var("HOMEBASE_SECRETS_YAML"),
        }
    }

    #[test]
    fn list_returns_empty_when_file_missing() {
        let prev = std::env::var("HOMEBASE_SECRETS_YAML").ok();
        std::env::set_var(
            "HOMEBASE_SECRETS_YAML",
            r"C:\definitely\does\not\exist\secrets.yaml",
        );
        let names = list_secret_names().expect("list should not error on missing");
        assert!(names.is_empty());
        match prev {
            Some(v) => std::env::set_var("HOMEBASE_SECRETS_YAML", v),
            None => std::env::remove_var("HOMEBASE_SECRETS_YAML"),
        }
    }

    #[test]
    fn list_parses_yaml_top_keys() {
        // 写一个假的 SOPS-shaped yaml 文件
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let yaml_content = r#"openrouter_key: ENC[AES256_GCM,data:abc]
anthropic_key: ENC[AES256_GCM,data:def]
github_token: ENC[AES256_GCM,data:ghi]
sops:
    age:
        - recipient: age1xxx
"#;
        std::fs::write(tmp.path(), yaml_content).unwrap();

        let prev = std::env::var("HOMEBASE_SECRETS_YAML").ok();
        std::env::set_var("HOMEBASE_SECRETS_YAML", tmp.path());

        let names = list_secret_names().expect("parse should succeed");
        assert_eq!(
            names,
            vec!["anthropic_key", "github_token", "openrouter_key"]
        );
        // sops metadata 段被剔除
        assert!(!names.iter().any(|n| n == "sops"));

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_SECRETS_YAML", v),
            None => std::env::remove_var("HOMEBASE_SECRETS_YAML"),
        }
    }
}
