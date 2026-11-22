//! MCP 池 → 4 agent 配置文件 reconcile（S2 · 2026-05-06）
//!
//! 严格不变量：**只动以 `homebase-` 前缀开头的 server entry**。
//! USER 在 `~/.claude/settings.json` 已自配 7 个 server（context7/figma/github/...）·
//! 这些一字不动 · 完全不碰。
//!
//! 配置文件路径与格式（2026-05 实测）：
//! - `claude-code`：`~/.claude/settings.json` · 顶层 `mcpServers: { <id>: { command, args, env? } }`
//! - `codex`     ：`~/.codex/config.toml` · `[mcp_servers.<id>]` table per server
//! - `gemini-cli`：`~/.gemini/settings.json` · 顶层 `mcpServers: { ... }`（同 Claude）
//! - `openclaw`  ：`~/.openclaw/settings.json` · 同 Claude 格式（用户 fork）
//!
//! 不存在的 agent config dir 一律跳过（USER 没装就不乱创建）。

use crate::adapters::mcp_pool::{McpServerDef, SharedMcpPool, AGENT_IDS, HB_MANAGED_PREFIX};
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use toml_edit::{value as toml_value, Array as TomlArray, DocumentMut, Item, Table};

#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentReconcileResult {
    pub agent_id: String,
    /// 路径（即便 skip 也返回 · 给 UI 显示）
    pub path: String,
    /// `applied` / `skipped` / `error`
    pub status: String,
    /// 应用了几个 server（applied 时有效）
    pub applied_count: usize,
    /// 错误消息（error 时有效）
    pub message: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ReconcileReport {
    pub agents: Vec<AgentReconcileResult>,
}

/// 入口：把整个 pool 推到 4 个 agent
pub fn reconcile_all(pool: &SharedMcpPool) -> ReconcileReport {
    let mut report = ReconcileReport::default();
    for agent_id in AGENT_IDS {
        let assigned_ids = pool.assignments.get(*agent_id).cloned().unwrap_or_default();
        let assigned: Vec<&McpServerDef> = pool
            .servers
            .iter()
            .filter(|s| assigned_ids.contains(&s.id))
            .collect();

        let result = match *agent_id {
            "claude-code" => sync_json_agent(
                "claude-code",
                claude_code_config_path(),
                claude_code_dir(),
                &assigned,
            ),
            "codex" => sync_codex(&assigned),
            "gemini-cli" => {
                sync_json_agent("gemini-cli", gemini_config_path(), gemini_dir(), &assigned)
            }
            "openclaw" => sync_json_agent(
                "openclaw",
                openclaw_config_path(),
                openclaw_dir(),
                &assigned,
            ),
            _ => unreachable!("未知 agent_id: {}", agent_id),
        };
        report.agents.push(result);
    }
    report
}

// =====================================================================
// 路径定位
// =====================================================================

fn home() -> PathBuf {
    super::homebase_home_dir()
}

fn claude_code_dir() -> PathBuf {
    home().join(".claude")
}
fn claude_code_config_path() -> PathBuf {
    claude_code_dir().join("settings.json")
}

fn codex_dir() -> PathBuf {
    home().join(".codex")
}
fn codex_config_path() -> PathBuf {
    codex_dir().join("config.toml")
}

fn gemini_dir() -> PathBuf {
    home().join(".gemini")
}
fn gemini_config_path() -> PathBuf {
    gemini_dir().join("settings.json")
}

fn openclaw_dir() -> PathBuf {
    home().join(".openclaw")
}
fn openclaw_config_path() -> PathBuf {
    openclaw_dir().join("settings.json")
}

// =====================================================================
// JSON-based agents（Claude Code / Gemini CLI / OpenClaw）
// =====================================================================

/// 通用 JSON agent reconcile：
/// 1. agent dir 不存在 → skip（USER 没装）
/// 2. config 文件不存在 + assigned 非空 → 创建 minimal `{"mcpServers": {...}}`
/// 3. config 文件存在 → 解析 → 删 `homebase-` 前缀的旧 entry → 加新 → 写回
fn sync_json_agent(
    agent_id: &str,
    cfg_path: PathBuf,
    agent_dir: PathBuf,
    assigned: &[&McpServerDef],
) -> AgentReconcileResult {
    if !agent_dir.exists() {
        return AgentReconcileResult {
            agent_id: agent_id.into(),
            path: cfg_path.to_string_lossy().into_owned(),
            status: "skipped".into(),
            applied_count: 0,
            message: Some(format!("{} 未安装（目录不存在）", agent_id)),
        };
    }

    // 读现有 config 或建新
    let mut root: Value = if cfg_path.exists() {
        match std::fs::read_to_string(&cfg_path) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(e) => return error_result(agent_id, &cfg_path, format!("解析 JSON 失败: {e}")),
            },
            Err(e) => return error_result(agent_id, &cfg_path, format!("读取失败: {e}")),
        }
    } else {
        json!({})
    };

    // 取 mcpServers 对象（不存在则建空）
    let obj = root
        .as_object_mut()
        .unwrap_or_else(|| unreachable!("root not object · serde_json::Value::Object 上面已保证"));
    let mcp = obj
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(Map::new()));
    let mcp_obj = match mcp.as_object_mut() {
        Some(o) => o,
        None => return error_result(agent_id, &cfg_path, "mcpServers 字段不是 object".into()),
    };

    // 删旧的 homebase-* entry
    mcp_obj.retain(|k, _| !k.starts_with(HB_MANAGED_PREFIX));

    // 加新的
    for s in assigned {
        let mut entry = Map::new();
        // Claude Code / Gemini 都不强制要 type 字段（默认 stdio）· 不写减少噪音
        entry.insert("command".into(), json!(s.command));
        if !s.args.is_empty() {
            entry.insert("args".into(), json!(s.args));
        }
        let env_with_agent = effective_env(s, agent_id);
        if !env_with_agent.is_empty() {
            entry.insert("env".into(), json!(env_with_agent));
        }
        mcp_obj.insert(s.id.clone(), Value::Object(entry));
    }

    // 写回
    let snapshot = match super::snapshot::create_snapshot(
        &format!("before mcp_reconcile::{agent_id}"),
        &[cfg_path.clone()],
    ) {
        Ok(s) => s,
        Err(e) => return error_result(agent_id, &cfg_path, format!("snapshot: {e}")),
    };
    if let Err(e) = std::fs::create_dir_all(&agent_dir) {
        return error_result(agent_id, &cfg_path, format!("create_dir: {e}"));
    }
    let pretty = match serde_json::to_string_pretty(&root) {
        Ok(s) => s,
        Err(e) => return error_result(agent_id, &cfg_path, format!("serialize: {e}")),
    };
    if let Err(e) = std::fs::write(&cfg_path, pretty) {
        return error_result(agent_id, &cfg_path, format!("write: {e}"));
    }
    super::audit::record(
        "mcp.agent_config_applied",
        cfg_path.to_string_lossy(),
        json!({
            "agent_id": agent_id,
            "applied_count": assigned.len(),
            "snapshot_id": snapshot.snapshot_id
        }),
    );

    AgentReconcileResult {
        agent_id: agent_id.into(),
        path: cfg_path.to_string_lossy().into_owned(),
        status: "applied".into(),
        applied_count: assigned.len(),
        message: None,
    }
}

// =====================================================================
// Codex (TOML)
// =====================================================================

/// Codex reconcile：
/// 1. ~/.codex 不存在 → skip
/// 2. 用 toml_edit 保留注释/格式
/// 3. 删除所有 `mcp_servers.<homebase-*>` table · 加新的
fn sync_codex(assigned: &[&McpServerDef]) -> AgentReconcileResult {
    let agent_id = "codex";
    let cfg_path = codex_config_path();
    let agent_dir = codex_dir();

    if !agent_dir.exists() {
        return AgentReconcileResult {
            agent_id: agent_id.into(),
            path: cfg_path.to_string_lossy().into_owned(),
            status: "skipped".into(),
            applied_count: 0,
            message: Some("codex 未安装（~/.codex 不存在）".into()),
        };
    }

    // 读现有 toml 或建新
    let mut doc: DocumentMut = if cfg_path.exists() {
        match std::fs::read_to_string(&cfg_path) {
            Ok(s) => match s.parse::<DocumentMut>() {
                Ok(d) => d,
                Err(e) => return error_result(agent_id, &cfg_path, format!("解析 TOML 失败: {e}")),
            },
            Err(e) => return error_result(agent_id, &cfg_path, format!("读取失败: {e}")),
        }
    } else {
        DocumentMut::new()
    };

    // 取 [mcp_servers] table（不存在则创建为 dotted table 不影响其他段）
    let mcp_root = doc.as_table_mut().entry("mcp_servers").or_insert_with(|| {
        let mut t = Table::new();
        t.set_implicit(true); // 让 [mcp_servers.<id>] 输出 · 不输出空 [mcp_servers]
        Item::Table(t)
    });
    let mcp_table = match mcp_root.as_table_mut() {
        Some(t) => t,
        None => return error_result(agent_id, &cfg_path, "mcp_servers 不是 table".into()),
    };

    // 删旧 homebase-* table
    let stale: Vec<String> = mcp_table
        .iter()
        .filter_map(|(k, _)| {
            if k.starts_with(HB_MANAGED_PREFIX) {
                Some(k.to_string())
            } else {
                None
            }
        })
        .collect();
    for k in stale {
        mcp_table.remove(&k);
    }

    // 加新
    for s in assigned {
        let mut t = Table::new();
        t.insert("command", toml_value(&s.command));
        if !s.args.is_empty() {
            let mut arr = TomlArray::new();
            for a in &s.args {
                arr.push(a.clone());
            }
            t.insert("args", Item::Value(arr.into()));
        }
        let env_with_agent = effective_env(s, "codex");
        if !env_with_agent.is_empty() {
            let mut env_table = Table::new();
            for (k, v) in &env_with_agent {
                env_table.insert(k, toml_value(v));
            }
            t.insert("env", Item::Table(env_table));
        }
        mcp_table.insert(&s.id, Item::Table(t));
    }

    if let Err(e) = std::fs::create_dir_all(&agent_dir) {
        return error_result(agent_id, &cfg_path, format!("create_dir: {e}"));
    }
    let snapshot = match super::snapshot::create_snapshot(
        "before mcp_reconcile::codex",
        &[cfg_path.clone()],
    ) {
        Ok(s) => s,
        Err(e) => return error_result(agent_id, &cfg_path, format!("snapshot: {e}")),
    };
    if let Err(e) = std::fs::write(&cfg_path, doc.to_string()) {
        return error_result(agent_id, &cfg_path, format!("write: {e}"));
    }
    super::audit::record(
        "mcp.agent_config_applied",
        cfg_path.to_string_lossy(),
        json!({
            "agent_id": agent_id,
            "applied_count": assigned.len(),
            "snapshot_id": snapshot.snapshot_id
        }),
    );

    AgentReconcileResult {
        agent_id: agent_id.into(),
        path: cfg_path.to_string_lossy().into_owned(),
        status: "applied".into(),
        applied_count: assigned.len(),
        message: None,
    }
}

// =====================================================================
// helpers
// =====================================================================

/// 给某 server 生成“写入 agent 配置时的有效 env”· 自动注入 HB_AGENT_ID = 当前 agent_id
///
/// 仅对 homebase- 前缀 server 有效（反正 reconcile 其他不动）·
/// 所以这里不检查前缀 · 加上不会圣。USER 自己可在 pool 里覆写该 env · 但主动覆写会被这里反覆·
/// 混乱场景制衡：众者间 effectively pool->env 被覆写。
fn effective_env(s: &McpServerDef, agent_id: &str) -> std::collections::BTreeMap<String, String> {
    let mut e = s.env.clone();
    e.insert("HB_AGENT_ID".to_string(), agent_id.to_string());
    e
}

fn error_result(agent_id: &str, path: &PathBuf, msg: String) -> AgentReconcileResult {
    AgentReconcileResult {
        agent_id: agent_id.into(),
        path: path.to_string_lossy().into_owned(),
        status: "error".into(),
        applied_count: 0,
        message: Some(msg),
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::mcp_pool::McpServerDef;
    use std::collections::BTreeMap;

    fn fake_server(id: &str) -> McpServerDef {
        let mut env = BTreeMap::new();
        env.insert("FOO".into(), "bar".into());
        McpServerDef {
            id: format!("homebase-{id}"),
            name: id.into(),
            description: String::new(),
            transport: "stdio".into(),
            command: "/usr/bin/cmd".into(),
            args: vec!["--flag".into()],
            env,
        }
    }

    fn with_test_home<F: FnOnce()>(tmp: &tempfile::TempDir, f: F) {
        // 跟 shared_context 共用同一把 env 锁 · 防止并行竞争
        let _guard = crate::adapters::shared_context::tests::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());
        f();
        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }

    #[test]
    fn json_agent_preserves_user_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".claude");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("settings.json");

        // 模拟 USER 已有的配置
        std::fs::write(
            &cfg,
            r#"{
  "autoUpdatesChannel": "latest",
  "mcpServers": {
    "context7": { "command": "cmd", "args": ["/c", "npx", "context7"] },
    "homebase-stale": { "command": "old" }
  }
}"#,
        )
        .unwrap();

        let s = fake_server("image");
        let assigned: Vec<&McpServerDef> = vec![&s];
        let res = sync_json_agent("claude-code", cfg.clone(), dir.clone(), &assigned);
        assert_eq!(res.status, "applied", "result: {:?}", res);
        assert_eq!(res.applied_count, 1);

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let mcp = after["mcpServers"].as_object().unwrap();

        // USER 自加的还在
        assert!(mcp.contains_key("context7"), "context7 should be preserved");
        // 旧 homebase-stale 删了
        assert!(
            !mcp.contains_key("homebase-stale"),
            "stale homebase entry should be removed"
        );
        // 新 homebase-image 加了
        assert!(mcp.contains_key("homebase-image"));
        // 顶层别的字段保留
        assert_eq!(after["autoUpdatesChannel"], "latest");
    }

    #[test]
    fn json_agent_skips_when_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".not-exist");
        let cfg = dir.join("settings.json");

        let res = sync_json_agent("claude-code", cfg, dir, &[]);
        assert_eq!(res.status, "skipped");
    }

    #[test]
    fn codex_preserves_user_segments() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.toml");

        std::fs::write(
            &cfg,
            r#"# user comment
model = "gpt-5.5"

[model_providers.openrouter]
name = "openrouter"
"#,
        )
        .unwrap();

        with_test_home(&tmp, || {
            let s = fake_server("image");
            let assigned: Vec<&McpServerDef> = vec![&s];
            let res = sync_codex(&assigned);
            assert_eq!(res.status, "applied", "{:?}", res);

            let after = std::fs::read_to_string(&cfg).unwrap();
            assert!(after.contains("# user comment"));
            assert!(after.contains("gpt-5.5"));
            assert!(after.contains("[model_providers.openrouter]"));
            // toml_edit 对含 - 的 key 会 quote · 接受两种输出
            let has_unquoted = after.contains("[mcp_servers.homebase-image]");
            let has_quoted = after.contains("[mcp_servers.\"homebase-image\"]");
            assert!(
                has_unquoted || has_quoted,
                "预期 homebase-image table·got:\n{after}"
            );
            assert!(after.contains("/usr/bin/cmd"));
        });
    }

    #[test]
    fn reconcile_all_skips_missing_dirs() {
        // 全部 4 agent dir 都不存在·reconcile_all 应该 4 个都 skip·不要 panic
        let tmp = tempfile::tempdir().unwrap();
        with_test_home(&tmp, || {
            let pool = SharedMcpPool::default();
            let report = reconcile_all(&pool);
            assert_eq!(report.agents.len(), 4);
            for r in &report.agents {
                assert_eq!(
                    r.status, "skipped",
                    "agent {} 应该 skipped·got {:?}",
                    r.agent_id, r
                );
            }
        });
    }
}
