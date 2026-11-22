//! 共享 MCP 池（S2 · 2026-05-06）
//!
//! 单一数据源 `~/.homebase/shared/mcp-pool.json` · HomeBase 启动时 reconcile 到 4 个 agent
//! 各自的默认配置文件中 · 让 USER 在终端直接跑 agent（非 HomeBase 启动）也能用这些 MCP server。
//!
//! 关键设计：
//! - 每个被 HomeBase 管理的 server 加 `homebase-` 前缀（id 命名约定）
//! - reconcile 时 **只动** 带这前缀的 entry · USER 自加的 MCP server 一字不动
//! - assignments 矩阵控制每个 agent 拿到哪些 server
//!
//! Tauri 命令暴露在 `commands::shared_mcp` 模块。

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Agent ID（4 官方 agent · Hermes 暂不支持）
pub const AGENT_IDS: &[&str] = &["claude-code", "codex", "gemini-cli", "openclaw"];

/// HomeBase 管理标记（写入 agent config 时所有 server id 都加这个前缀）
pub const HB_MANAGED_PREFIX: &str = "homebase-";

/// `~/.homebase/shared/mcp-pool.json`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMcpPool {
    /// MCP server 定义列表（不含密钥 · env 可用 ${VAR} 占位符）
    #[serde(default)]
    pub servers: Vec<McpServerDef>,
    /// 分配矩阵 · agent_id → 启用的 server.id 列表
    #[serde(default)]
    pub assignments: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerDef {
    /// Server 唯一 id（建议带 `homebase-` 前缀 · reconcile 强制加前缀）
    pub id: String,
    /// 显示名（UI 用）
    pub name: String,
    /// 一句话说明（UI + agent 看）
    #[serde(default)]
    pub description: String,
    /// 传输类型 · 目前仅支持 stdio（Phase S2 范围）
    #[serde(default = "default_transport")]
    pub transport: String,
    /// 子进程命令绝对路径
    pub command: String,
    /// 命令行参数
    #[serde(default)]
    pub args: Vec<String>,
    /// 环境变量（值可用 `${VAR}` 占位符 · agent 启动时从父进程 env 替换）
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn default_transport() -> String {
    "stdio".into()
}

impl Default for SharedMcpPool {
    fn default() -> Self {
        // 默认池：
        // - homebase-image · 仅分配给 claude-code + openclaw（生图能力）
        // - homebase-secrets · 分配给全 4 agent（runtime secret 询问 · S3）
        // - homebase-local-asr · 分配给全 4 agent（与 Quick Panel 共用任务契约）
        let mut assignments = BTreeMap::new();
        for id in AGENT_IDS {
            assignments.insert((*id).to_string(), Vec::new());
        }
        let mut servers: Vec<McpServerDef> = Vec::new();

        if let Some(image_def) = default_image_server() {
            assignments
                .get_mut("claude-code")
                .unwrap()
                .push(image_def.id.clone());
            assignments
                .get_mut("openclaw")
                .unwrap()
                .push(image_def.id.clone());
            servers.push(image_def);
        }
        if let Some(secrets_def) = default_secrets_server() {
            for id in AGENT_IDS {
                assignments
                    .get_mut(*id)
                    .unwrap()
                    .push(secrets_def.id.clone());
            }
            servers.push(secrets_def);
        }
        if let Some(asr_def) = default_asr_server() {
            for id in AGENT_IDS {
                assignments.get_mut(*id).unwrap().push(asr_def.id.clone());
            }
            servers.push(asr_def);
        }

        Self {
            servers,
            assignments,
        }
    }
}

/// 默认的 homebase-image server（mcp-image binary 跟主进程同目录）
fn default_image_server() -> Option<McpServerDef> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mcp_image = if cfg!(target_os = "windows") {
        dir.join("homebase-mcp-image.exe")
    } else {
        dir.join("homebase-mcp-image")
    };
    if !mcp_image.exists() {
        return None;
    }
    let mut env = BTreeMap::new();
    env.insert("OPENROUTER_API_KEY".into(), "${OPENROUTER_API_KEY}".into());
    env.insert("HB_IMAGE_MODEL".into(), "${HB_IMAGE_MODEL}".into());
    env.insert(
        "HB_IMAGE_OUTPUT_DIR".into(),
        "${HB_IMAGE_OUTPUT_DIR}".into(),
    );
    Some(McpServerDef {
        id: "homebase-image".into(),
        name: "HomeBase 生图".into(),
        description: "多模型路由生图（GPT Image 2 / Nano Banana Pro / FLUX.2 Pro / Nano Banana 2）"
            .into(),
        transport: "stdio".into(),
        command: mcp_image.to_string_lossy().into_owned(),
        args: Vec::new(),
        env,
    })
}

/// 默认的 homebase-secrets server（S3 · 跟主进程同目录的独立 binary）
///
/// HB_AGENT_ID 不在这里写 · 由 reconcile 时按 agent 注入。
fn default_secrets_server() -> Option<McpServerDef> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mcp_secrets = if cfg!(target_os = "windows") {
        dir.join("homebase-mcp-secrets.exe")
    } else {
        dir.join("homebase-mcp-secrets")
    };
    if !mcp_secrets.exists() {
        return None;
    }
    Some(McpServerDef {
        id: "homebase-secrets".into(),
        name: "HomeBase 共享 Secrets".into(),
        description:
            "通过 USER 弹窗授权拿取共享 secret（OpenRouter / GitHub / ...）· 永久允许写 ACL".into(),
        transport: "stdio".into(),
        command: mcp_secrets.to_string_lossy().into_owned(),
        args: Vec::new(),
        env: BTreeMap::new(), // HB_AGENT_ID 由 reconcile 注入
    })
}

fn default_asr_server() -> Option<McpServerDef> {
    let python = std::env::var("HOMEBASE_ASR_MCP_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\HomeBase\Tools\Python313\python.exe"));
    let aihub_root = std::env::var("HOMEBASE_AIHUB_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\HomeBase\AIHub"));
    let server = aihub_root
        .join("mcp")
        .join("servers")
        .join("local-asr")
        .join("server.py");
    if !python.is_file() || !server.is_file() {
        return None;
    }
    let mut env = BTreeMap::new();
    env.insert(
        "HOMEBASE_AIHUB_ROOT".into(),
        aihub_root.to_string_lossy().into_owned(),
    );
    env.insert(
        "HOMEBASE_ASR_PYTHON".into(),
        python.to_string_lossy().into_owned(),
    );
    env.insert(
        "HOMEBASE_ASR_OUTPUT_ROOT".into(),
        std::env::var("HOMEBASE_ASR_OUTPUT_ROOT")
            .unwrap_or_else(|_| r"C:\HomeBase\Wiki\raw\audio".to_string()),
    );
    Some(McpServerDef {
        id: "homebase-local-asr".into(),
        name: "HomeBase 本地语音识别".into(),
        description: "本地/云端/混合 ASR 预检、后台转写、结果读取与融合提示词".into(),
        transport: "stdio".into(),
        command: python.to_string_lossy().into_owned(),
        args: vec![server.to_string_lossy().into_owned()],
        env,
    })
}

// =====================================================================
// 文件读写
// =====================================================================

pub fn pool_path() -> PathBuf {
    super::homebase_home_dir()
        .join(".homebase")
        .join("shared")
        .join("mcp-pool.json")
}

pub fn load() -> Result<SharedMcpPool, String> {
    load_at(&pool_path())
}

fn load_at(path: &std::path::Path) -> Result<SharedMcpPool, String> {
    // 读取及启动检查不保存默认值/迁移；持久化仅由显式 save/apply 完成。
    let pool = crate::query::optional_json::<SharedMcpPool>(path)?;
    Ok(normalize(merge_default_servers(pool)))
}

fn parse_pool_text(raw: &str) -> Result<SharedMcpPool, serde_json::Error> {
    serde_json::from_str(crate::util::strip_bom(raw))
}

pub fn save(pool: &SharedMcpPool) -> Result<(), String> {
    let path = pool_path();
    let snapshot = super::snapshot::create_snapshot("before mcp_pool::save", &[path.clone()])?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create_dir {dir:?}: {e}"))?;
    }
    let s = serde_json::to_string_pretty(pool).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(&path, s).map_err(|e| format!("write {path:?}: {e}"))?;
    super::audit::record(
        "mcp.pool_saved",
        path.to_string_lossy(),
        json!({
            "snapshot_id": snapshot.snapshot_id,
            "servers": pool.servers.len(),
            "assignments": pool.assignments.len()
        }),
    );
    Ok(())
}

/// 校验 + 标准化 pool（保证不变量）
///
/// - 所有 server.id 强制加 `homebase-` 前缀（保护：reconcile 不动用户自加 server）
/// - assignments 中引用的 id 必须存在于 servers · 否则丢弃
/// - 4 个 agent_id 必须都有 entry（缺则补空数组）
fn normalize(mut pool: SharedMcpPool) -> SharedMcpPool {
    // 1. 强制 id 前缀
    for s in pool.servers.iter_mut() {
        if !s.id.starts_with(HB_MANAGED_PREFIX) {
            s.id = format!("{HB_MANAGED_PREFIX}{}", s.id);
        }
    }
    let valid_ids: std::collections::HashSet<String> =
        pool.servers.iter().map(|s| s.id.clone()).collect();

    // 2. assignments 清洗
    for id in AGENT_IDS {
        pool.assignments
            .entry((*id).to_string())
            .or_insert_with(Vec::new);
    }
    for (_agent, ids) in pool.assignments.iter_mut() {
        ids.retain(|id| valid_ids.contains(id));
    }
    pool
}

/// 老版本用户的 `mcp-pool.json` 可能只含早期默认 server（例如只有
/// `homebase-image`），后续新增的默认项（如 `homebase-secrets`）不能因为文件
/// 已存在就永远缺失。这里做非破坏性迁移：
/// - 只补缺失的默认 server；
/// - 只给缺失该 server 的默认 agent assignment 追加；
/// - 不删除、不覆盖用户已有 server 或矩阵选择。
fn merge_default_servers(mut pool: SharedMcpPool) -> SharedMcpPool {
    let defaults = SharedMcpPool::default();
    for server in defaults.servers {
        if !pool.servers.iter().any(|s| s.id == server.id) {
            pool.servers.push(server);
        }
    }

    for agent_id in AGENT_IDS {
        let current = pool
            .assignments
            .entry((*agent_id).to_string())
            .or_insert_with(Vec::new);
        if let Some(default_ids) = defaults.assignments.get(*agent_id) {
            for server_id in default_ids {
                if !current.contains(server_id) {
                    current.push(server_id.clone());
                }
            }
        }
    }

    pool
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_safety_mcp_load_does_not_write_or_hide_invalid_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-pool.json");
        assert!(load_at(&path).is_ok());
        assert!(!path.exists());
        std::fs::write(&path, "broken synthetic config").unwrap();
        assert!(load_at(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken synthetic config");
        std::fs::write(&path, "{}").unwrap();
        assert!(load_at(&path).is_ok());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }
    use std::collections::BTreeMap;

    #[test]
    fn default_has_4_agents_in_assignments() {
        let pool = SharedMcpPool::default();
        for id in AGENT_IDS {
            assert!(
                pool.assignments.contains_key(*id),
                "assignments 缺 agent: {id}"
            );
        }
    }

    #[test]
    fn normalize_forces_prefix() {
        let mut pool = SharedMcpPool {
            servers: vec![McpServerDef {
                id: "image".into(), // 没前缀
                name: "x".into(),
                description: String::new(),
                transport: "stdio".into(),
                command: "/x".into(),
                args: vec![],
                env: BTreeMap::new(),
            }],
            assignments: BTreeMap::new(),
        };
        pool.assignments
            .insert("claude-code".into(), vec!["image".into()]);
        // 模拟 deserialize 后 normalize · 但 assignments 现在引用未加前缀 id · normalize 后会被丢
        let n = normalize(pool);
        assert_eq!(n.servers[0].id, "homebase-image");
        // assignments 引用 "image" · normalize 后 servers 是 "homebase-image" · 不匹配 · 被清掉
        assert!(n.assignments["claude-code"].is_empty());
    }

    #[test]
    fn normalize_keeps_valid_assignments() {
        let mut pool = SharedMcpPool {
            servers: vec![McpServerDef {
                id: "homebase-image".into(),
                name: "x".into(),
                description: String::new(),
                transport: "stdio".into(),
                command: "/x".into(),
                args: vec![],
                env: BTreeMap::new(),
            }],
            assignments: BTreeMap::new(),
        };
        pool.assignments
            .insert("claude-code".into(), vec!["homebase-image".into()]);
        let n = normalize(pool);
        assert_eq!(n.assignments["claude-code"], vec!["homebase-image"]);
    }

    #[test]
    fn merge_default_servers_adds_missing_defaults_without_dropping_user_choices() {
        let mut old = SharedMcpPool {
            servers: vec![McpServerDef {
                id: "homebase-user-kept".into(),
                name: "kept".into(),
                description: String::new(),
                transport: "stdio".into(),
                command: "/kept".into(),
                args: vec![],
                env: BTreeMap::new(),
            }],
            assignments: BTreeMap::new(),
        };
        old.assignments
            .insert("codex".into(), vec!["homebase-user-kept".into()]);

        let merged = merge_default_servers(old);
        assert!(merged.servers.iter().any(|s| s.id == "homebase-user-kept"));
        assert!(merged.assignments["codex"].contains(&"homebase-user-kept".into()));
        for id in AGENT_IDS {
            assert!(merged.assignments.contains_key(*id));
        }
    }

    #[test]
    fn parse_pool_accepts_utf8_bom() {
        let parsed = parse_pool_text("\u{feff}{\"servers\":[],\"assignments\":{}}")
            .expect("BOM-prefixed user config should parse");
        assert!(parsed.servers.is_empty());
    }
}
