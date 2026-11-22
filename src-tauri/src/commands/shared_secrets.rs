//! Shared Secrets Tauri 命令（S3）· 2026-05-06
//!
//! 与现有 `commands/secrets.rs`（带 metadata / health check 的完整管理 UI）共存：
//! - 旧 `secrets_*`：Secrets 页面用 · 含 health / rotate / set-meta · 仅 USER 直接管理
//! - 新 `shared_secrets_*`：跨 4 agent 的 ACL 池 / IPC endpoint / consent 流程
//!
//! UI 入口：`/agents` 页面新加 SecretsSection
//! - 列出 IPC server 状态（endpoint · 是否启动）
//! - 列出 secrets.yaml 已有的 secret 名（不解密）
//! - ACL 矩阵：4 agent × N secret · 每格 ✓ allowed / · never asked
//! - 点 ✓ → revoke
//! - 「打开 SOPS 编辑器」 → spawn `sops <yaml>` 让 USER 自己增删改

use serde::{Deserialize, Serialize};

use crate::adapters::secrets_acl::{self, AclFile, AclEntry};
use crate::adapters::secrets_ipc::{self, ConsentAction, IpcEndpoint};
use crate::adapters::secrets_store;

/// 4 个走 ACL 的 agent（hermes 暂不支持 · 与 mcp/skills 一致）
const ACL_AGENT_IDS: &[&str] = &["claude-code", "codex", "gemini-cli", "openclaw"];

/// 列已有 secret 名（不解密）
#[tauri::command]
pub async fn shared_secrets_list_names() -> Result<Vec<String>, String> {
    secrets_store::list_secret_names().map_err(|e| format!("list secret names: {e}"))
}

/// IPC server 端点信息（前端可显示「监听中:51234」）
///
/// 文件不存在表示 server 未启动（理论不该发生 · setup 阶段就启）
#[tauri::command]
pub fn shared_secrets_ipc_endpoint() -> Result<Option<IpcEndpoint>, String> {
    let path = secrets_ipc::endpoint_file_path();
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读 ipc endpoint 文件: {e}"))?;
    let ep: IpcEndpoint = serde_json::from_str(&text)
        .map_err(|e| format!("解析 ipc endpoint: {e}"))?;
    Ok(Some(ep))
}

/// 列 ACL 条目（带矩阵展开 · 方便前端渲染）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclListResult {
    /// 全部 entry · key=`agent:secret` · entry 含 granted_at / purpose
    pub entries: std::collections::BTreeMap<String, AclEntry>,
    /// 矩阵预计算：行=agent · 列=secret · cell=是否允许
    pub matrix: AclMatrix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AclMatrix {
    pub agents: Vec<String>,
    pub secrets: Vec<String>,
    /// `cells[agent_idx][secret_idx]` = bool
    pub cells: Vec<Vec<bool>>,
}

#[tauri::command]
pub async fn shared_secrets_acl_list() -> Result<AclListResult, String> {
    let acl = secrets_acl::load_checked()?;
    let secrets = secrets_store::list_secret_names()
        .map_err(|e| format!("list secret names: {e}"))?;
    let agents: Vec<String> = ACL_AGENT_IDS.iter().map(|s| s.to_string()).collect();
    let cells: Vec<Vec<bool>> = agents
        .iter()
        .map(|a| {
            secrets
                .iter()
                .map(|s| acl.is_allowed(a, s))
                .collect()
        })
        .collect();
    Ok(AclListResult {
        entries: acl.entries,
        matrix: AclMatrix {
            agents,
            secrets,
            cells,
        },
    })
}

/// 撤销 ACL · revoke = false 表示原本就没记录
#[tauri::command]
pub async fn shared_secrets_acl_revoke(
    agent_id: String,
    secret_name: String,
) -> Result<bool, String> {
    let mut acl = secrets_acl::load();
    let removed = acl.revoke(&agent_id, &secret_name);
    if removed {
        secrets_acl::save(&acl).map_err(|e| format!("写 acl: {e}"))?;
    }
    Ok(removed)
}

/// USER 在弹窗的决定 · 通过这个命令送达 IPC server 的等待中 oneshot
///
/// `action`: `"allow_once"` / `"allow_persistent"` / `"deny"`
#[tauri::command]
pub async fn shared_secrets_consent_resolve(
    request_id: String,
    action: String,
) -> Result<(), String> {
    let parsed: ConsentAction = match action.as_str() {
        "allow_once" => ConsentAction::AllowOnce,
        "allow_persistent" => ConsentAction::AllowPersistent,
        "deny" => ConsentAction::Deny,
        _ => return Err(format!("unknown action: {action}")),
    };
    secrets_ipc::resolve_pending(&request_id, parsed).await
}

/// 打开 SOPS 编辑器（让 USER 直接编辑 yaml · 添加/删除 secret）
///
/// Windows 下用 `cmd /C start` 让默认编辑器异步打开 · 不阻塞 HomeBase。
/// SOPS 会调 $EDITOR · 默认 vim · USER 可设 `SOPS_EDITOR` 改成 notepad / VS Code。
#[tauri::command]
pub async fn shared_secrets_open_yaml() -> Result<String, String> {
    let path = secrets_store::secrets_yaml_path();
    if !path.exists() {
        return Err(format!(
            "secrets.yaml 不存在: {} · 请先按文档初始化 SOPS",
            path.display()
        ));
    }
    let path_str = path.to_string_lossy().to_string();
    // sops <yaml> 走 stdio · 必须分离一个 console 窗口给它（不然 GUI app 没法承载交互编辑）
    // Windows: `cmd /C start cmd /k sops <path>` 弹一个新 cmd 窗口
    #[cfg(target_os = "windows")]
    {
        let res = std::process::Command::new("cmd")
            .args(["/C", "start", "cmd", "/k", "sops", &path_str])
            .spawn()
            .map_err(|e| format!("spawn sops: {e}"))?;
        let _ = res; // 不等
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("sops")
            .arg(&path_str)
            .spawn()
            .map_err(|e| format!("spawn sops: {e}"))?;
    }
    Ok(path_str)
}

/// 当前 ACL 支持的 agent id 列表（前端做矩阵列头用）
#[tauri::command]
pub fn shared_secrets_agent_ids() -> Vec<String> {
    ACL_AGENT_IDS.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_ids_returns_four_official() {
        let ids = shared_secrets_agent_ids();
        assert_eq!(ids.len(), 4);
        assert!(ids.iter().any(|x| x == "claude-code"));
        assert!(ids.iter().any(|x| x == "codex"));
        assert!(ids.iter().any(|x| x == "gemini-cli"));
        assert!(ids.iter().any(|x| x == "openclaw"));
        // hermes 不在
        assert!(!ids.iter().any(|x| x == "hermes"));
    }
}
