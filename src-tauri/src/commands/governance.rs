//! P0 governance commands: audit, snapshots, machine manifest, capability matrix.

use crate::adapters::{
    audit, mcp_pool, powershell::PowerShellAdapter, secrets_acl, skills_pool, sops::SopsAdapter,
};
use chrono::Local;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;


#[tauri::command]
pub async fn governance_audit_recent(
    limit: Option<usize>,
) -> Result<Vec<audit::AuditEvent>, String> {
    tokio::task::spawn_blocking(move || audit::list_recent(limit.unwrap_or(100)))
        .await
        .map_err(|_| "QUERY_FAILED: 审计记录读取任务未完成")?
}

#[tauri::command]
pub async fn governance_snapshots_list(
    limit: Option<usize>,
) -> Result<Vec<crate::adapters::snapshot::SnapshotManifest>, String> {
    crate::adapters::snapshot::list_snapshots(limit.unwrap_or(50))
}

#[tauri::command]
pub async fn governance_snapshot_rollback(
    snapshot_id: String,
) -> Result<crate::adapters::snapshot::SnapshotManifest, String> {
    let manifest = crate::adapters::snapshot::rollback_snapshot(&snapshot_id)?;
    audit::record(
        "config.rollback_completed",
        snapshot_id,
        serde_json::json!({
            "restored_files": manifest.files.len()
        }),
    );
    Ok(manifest)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineManifest {
    pub schema_version: u32,
    pub generated_at: String,
    pub homebase_home: String,
    pub os: OsManifest,
    pub disks: Vec<DiskManifest>,
    pub directories: Vec<DirectoryManifest>,
    pub runtimes: Vec<RuntimeManifest>,
    pub agents: Vec<AgentManifest>,
    pub ai_tools: Vec<AiToolManifest>,
    pub secrets_policy: String,
    pub docs_index: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsManifest {
    pub name: String,
    pub arch: String,
    pub hostname: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskManifest {
    pub drive: String,
    pub used_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectoryManifest {
    pub path: String,
    pub role: String,
    pub exists: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeManifest {
    pub id: String,
    pub command: String,
    pub installed: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentManifest {
    pub id: String,
    pub config_path: String,
    pub config_exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiToolManifest {
    pub id: String,
    pub display_name: String,
    pub install_path: String,
    pub installed: bool,
    pub control_level: String,
}

#[tauri::command]
pub async fn governance_machine_manifest_export() -> Result<MachineManifest, String> {
    let manifest = build_machine_manifest().await?;
    let path = crate::adapters::homebase_home_dir()
        .join(".homebase")
        .join("machine-manifest.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir {parent:?}: {e}"))?;
    }
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize machine manifest: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("write {path:?}: {e}"))?;
    audit::record(
        "machine.manifest_exported",
        path.to_string_lossy(),
        serde_json::json!({
            "runtimes": manifest.runtimes.len(),
            "agents": manifest.agents.len(),
            "ai_tools": manifest.ai_tools.len()
        }),
    );
    Ok(manifest)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigOwnership {
    pub generated_at: String,
    pub entries: Vec<ConfigOwnershipEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigOwnershipEntry {
    pub path: String,
    pub owner: String,
    pub managed_by: String,
    pub write_policy: String,
    pub plaintext_secret_allowed: bool,
    pub backup_required: bool,
    pub exists: bool,
    pub notes: String,
}

#[tauri::command]
pub async fn governance_config_ownership() -> Result<ConfigOwnership, String> {
    let home = crate::adapters::homebase_home_dir();
    let entries = vec![
        ownership(
            home.join(".homebase").join("shared").join("mcp-pool.json"),
            "homebase",
            "HomeBase Shared MCP Pool",
            "homebase-only",
            false,
            true,
            "统一 MCP 池的单一事实源；保存和分发前自动生成快照。",
        ),
        ownership(
            home.join(".homebase").join("shared").join("skills"),
            "homebase",
            "HomeBase Shared Skills Pool",
            "homebase-or-user-edit-shared-source",
            false,
            true,
            "统一 skills 池；agent 目录只接收复制结果，不作为事实源。",
        ),
        ownership(
            home.join(".homebase").join("secrets-acl.json"),
            "homebase",
            "HomeBase Secrets Consent",
            "homebase-only",
            false,
            true,
            "记录 agent 对 secret 的授权，不保存 secret 明文。",
        ),
        ownership(
            home.join(".claude").join("settings.json"),
            "shared",
            "HomeBase endpoint/MCP + Claude Code",
            "scoped-merge",
            false,
            true,
            "HomeBase 只写 apiKeyHelper/env/MCP 中 homebase-* 管理段；ccswitch 可切 profile，但不应写系统级明文密钥。",
        ),
        ownership(
            home.join(".claude").join("profiles"),
            "third-party",
            "ccswitch",
            "third-party-profile-switch",
            false,
            true,
            "ccswitch 的合理边界是 profile 切换；HomeBase 负责审计、密钥引用和 MCP/skills 池。",
        ),
        ownership(
            home.join(".codex").join("config.toml"),
            "shared",
            "HomeBase + Codex CLI",
            "scoped-merge",
            false,
            true,
            "HomeBase 可写 auth.command/MCP 管理段；日常编码仍由 IDE/CLI 承担。",
        ),
        ownership(
            home.join(".gemini").join("settings.json"),
            "shared",
            "HomeBase + Gemini CLI",
            "scoped-merge",
            false,
            true,
            "仅按 agent 能力写兼容配置；不强行注入不支持的 MCP/skills。",
        ),
        ownership(
            PathBuf::from(r"C:\HomeBase\Secrets\secrets.yaml"),
            "homebase",
            "SOPS age",
            "encrypted-only",
            false,
            true,
            "唯一允许保存 LLM/API 密钥的位置；前端只展示引用名和掩码。",
        ),
    ];

    Ok(ConfigOwnership {
        generated_at: Local::now().to_rfc3339(),
        entries,
    })
}

fn ownership(
    path: PathBuf,
    owner: &str,
    managed_by: &str,
    write_policy: &str,
    plaintext_secret_allowed: bool,
    backup_required: bool,
    notes: &str,
) -> ConfigOwnershipEntry {
    ConfigOwnershipEntry {
        exists: path.exists(),
        path: path.to_string_lossy().into_owned(),
        owner: owner.into(),
        managed_by: managed_by.into(),
        write_policy: write_policy.into(),
        plaintext_secret_allowed,
        backup_required,
        notes: notes.into(),
    }
}

async fn build_machine_manifest() -> Result<MachineManifest, String> {
    let home = crate::adapters::homebase_home_dir();
    let ps = PowerShellAdapter::new();
    let disks = match ps.get_psdrives().await {
        Ok(drives) => drives
            .into_iter()
            .map(|d| DiskManifest {
                drive: format!("{}:", d.name.to_uppercase()),
                used_bytes: d.used,
                free_bytes: d.free,
                role: drive_role(&d.name),
            })
            .collect(),
        Err(e) => {
            tracing::warn!("machine manifest disk probe failed: {}", e);
            Vec::new()
        }
    };

    let directories = known_directories();
    let runtimes = probe_runtimes().await;
    let agents = vec![
        agent_manifest("claude-code", home.join(".claude").join("settings.json")),
        agent_manifest("codex", home.join(".codex").join("config.toml")),
        agent_manifest("gemini-cli", home.join(".gemini").join("settings.json")),
        agent_manifest("openclaw", home.join(".openclaw").join("settings.json")),
        agent_manifest(
            "hermes",
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\homebase\.hermes"),
        ),
    ];
    let ai_tools = known_ai_tools();

    Ok(MachineManifest {
        schema_version: 1,
        generated_at: Local::now().to_rfc3339(),
        homebase_home: home.to_string_lossy().into_owned(),
        os: OsManifest {
            name: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            hostname: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into()),
            username: std::env::var("USERNAME").unwrap_or_else(|_| "unknown".into()),
        },
        disks,
        directories,
        runtimes,
        agents,
        ai_tools,
        secrets_policy: "sops-age-helper-no-plaintext-env".into(),
        docs_index: r"C:\Users\homebase\INDEX.md".into(),
    })
}

fn drive_role(name: &str) -> String {
    match name.to_ascii_uppercase().as_str() {
        "C" => "system".into(),
        "D" => "legacy-apps".into(),
        "E" => "works-and-knowledge".into(),
        "F" => "apps-runtimes-secrets-bulk".into(),
        _ => "unknown".into(),
    }
}

fn known_directories() -> Vec<DirectoryManifest> {
    [
        (r"C:\HomeBase\Projects", "projects", true),
        (r"C:\HomeBase\Wiki", "knowledge", true),
        (r"C:\HomeBase\Tools", "apps", true),
        (r"C:\HomeBase\Runtimes", "runtimes", true),
        (r"C:\HomeBase\Secrets", "secrets", true),
        (r"C:\Users\homebase\.homebase", "homebase-config", true),
        (r"C:\Users\homebase\.claude", "claude-config", false),
        (r"C:\Users\homebase\.codex", "codex-config", false),
        (r"C:\Users\homebase\.gemini", "gemini-config", false),
        (r"C:\Users\homebase\.openclaw", "openclaw-config", false),
    ]
    .into_iter()
    .map(|(path, role, required)| DirectoryManifest {
        path: path.into(),
        role: role.into(),
        exists: PathBuf::from(path).exists(),
        required,
    })
    .collect()
}

async fn probe_runtimes() -> Vec<RuntimeManifest> {
    let probes = [
        ("git", "git", vec!["--version"]),
        ("node", "node", vec!["--version"]),
        ("npm", "npm.cmd", vec!["--version"]),
        ("python", "python", vec!["--version"]),
        ("uv", "uv", vec!["--version"]),
        ("cargo", "cargo", vec!["--version"]),
        ("rustc", "rustc", vec!["--version"]),
        ("go", "go", vec!["version"]),
        ("docker", "docker", vec!["--version"]),
        ("wsl", "wsl", vec!["--version"]),
        ("sops", "sops", vec!["--version"]),
        ("age", "age", vec!["--version"]),
    ];
    let mut out = Vec::new();
    for (id, command, args) in probes {
        let result = crate::adapters::spawn_with_timeout(
            command,
            args.iter().copied(),
            None,
            Duration::from_secs(5),
        )
        .await;
        match result {
            Ok(output) if output.exit_code == 0 => out.push(RuntimeManifest {
                id: id.into(),
                command: command.into(),
                installed: true,
                version: output.stdout.lines().next().map(|s| s.trim().to_string()),
            }),
            _ => out.push(RuntimeManifest {
                id: id.into(),
                command: command.into(),
                installed: false,
                version: None,
            }),
        }
    }
    out
}

fn agent_manifest(id: &str, path: PathBuf) -> AgentManifest {
    AgentManifest {
        id: id.into(),
        config_exists: path.exists(),
        config_path: path.to_string_lossy().into_owned(),
    }
}

fn known_ai_tools() -> Vec<AiToolManifest> {
    [
        (
            "ccswitch",
            "CC Switch",
            r"C:\Users\homebase\AppData\Local\Programs\CC Switch\cc-switch.exe",
            "inspect-and-launch",
        ),
        (
            "cursor",
            "Cursor",
            r"D:\Program Files\cursor\Cursor.exe",
            "launch-and-index",
        ),
        (
            "windsurf",
            "Windsurf",
            r"C:\Users\homebase\AppData\Local\Programs\Windsurf\Windsurf.exe",
            "launch-and-index",
        ),
        (
            "vscode",
            "Visual Studio Code",
            r"C:\Users\homebase\AppData\Local\Programs\Microsoft VS Code\Code.exe",
            "visible-only",
        ),
        (
            "marvis",
            "Marvis",
            r"C:\Users\homebase\AppData\Local\Programs\Marvis\Marvis.exe",
            "visible-only",
        ),
    ]
    .into_iter()
    .map(
        |(id, display_name, install_path, control_level)| AiToolManifest {
            id: id.into(),
            display_name: display_name.into(),
            install_path: install_path.into(),
            installed: PathBuf::from(install_path).exists(),
            control_level: control_level.into(),
        },
    )
    .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityMatrix {
    pub generated_at: String,
    pub rows: Vec<CapabilityRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityRow {
    pub actor_id: String,
    pub actor_kind: String,
    pub mcp_servers: Vec<String>,
    pub skills: Vec<String>,
    pub allowed_secrets: Vec<String>,
    pub config_paths: Vec<String>,
    pub risk: String,
}

#[tauri::command]
pub async fn governance_capability_matrix() -> Result<CapabilityMatrix, String> {
    let pool = mcp_pool::load()?;
    let skills = skills_pool::list_skills()
        .map_err(|_| "QUERY_FAILED: Skills 清单读取失败，能力矩阵不完整".to_string())?
        .into_iter()
        .filter(|s| s.has_skill_md)
        .map(|s| format!("homebase-{}", s.id))
        .collect::<Vec<_>>();
    let acl = secrets_acl::load_checked()?;
    let mut secrets_by_agent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for key in acl.entries.keys() {
        if let Some((agent_id, secret_name)) = key.split_once(':') {
            secrets_by_agent
                .entry(agent_id.into())
                .or_default()
                .push(secret_name.into());
        }
    }

    let home = crate::adapters::homebase_home_dir();
    let config_paths: BTreeMap<&str, Vec<String>> = BTreeMap::from([
        (
            "claude-code",
            vec![home
                .join(".claude")
                .join("settings.json")
                .to_string_lossy()
                .into()],
        ),
        (
            "codex",
            vec![home
                .join(".codex")
                .join("config.toml")
                .to_string_lossy()
                .into()],
        ),
        (
            "gemini-cli",
            vec![home
                .join(".gemini")
                .join("settings.json")
                .to_string_lossy()
                .into()],
        ),
        (
            "openclaw",
            vec![home
                .join(".openclaw")
                .join("settings.json")
                .to_string_lossy()
                .into()],
        ),
        (
            "hermes",
            vec![r"\\wsl.localhost\Ubuntu\home\homebase\.hermes".into()],
        ),
    ]);

    let mut rows = Vec::new();
    for agent_id in mcp_pool::AGENT_IDS {
        rows.push(CapabilityRow {
            actor_id: (*agent_id).into(),
            actor_kind: "cli-agent".into(),
            mcp_servers: pool.assignments.get(*agent_id).cloned().unwrap_or_default(),
            skills: if *agent_id == "claude-code" || *agent_id == "openclaw" {
                skills.clone()
            } else {
                Vec::new()
            },
            allowed_secrets: secrets_by_agent.remove(*agent_id).unwrap_or_default(),
            config_paths: config_paths.get(*agent_id).cloned().unwrap_or_default(),
            risk: if *agent_id == "claude-code" || *agent_id == "openclaw" {
                "high".into()
            } else {
                "medium".into()
            },
        });
    }
    rows.push(CapabilityRow {
        actor_id: "ccswitch".into(),
        actor_kind: "third-party-switcher".into(),
        mcp_servers: Vec::new(),
        skills: Vec::new(),
        allowed_secrets: Vec::new(),
        config_paths: vec![
            home.join(".claude")
                .join("profiles")
                .to_string_lossy()
                .into(),
            r"C:\Users\homebase\AppData\Local\Programs\CC Switch".into(),
        ],
        risk: "medium".into(),
    });

    Ok(CapabilityMatrix {
        generated_at: Local::now().to_rfc3339(),
        rows,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceRegistry {
    pub generated_at: String,
    pub interfaces: Vec<InterfaceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceEntry {
    pub id: String,
    pub display_name: String,
    pub kind: String,
    pub base_url: Option<String>,
    pub owner: String,
    pub auth_source: String,
    pub discovery_methods: Vec<String>,
    pub risk: String,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceProbePayload {
    pub kind: String,
    pub base_url: String,
    pub secret_short_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterfaceProbeResult {
    pub ok: bool,
    pub attempted_urls: Vec<String>,
    pub status_code: Option<u16>,
    pub latency_ms: u128,
    pub models: Vec<String>,
    pub error: Option<String>,
    pub body_preview: Option<String>,
}

#[tauri::command]
pub async fn governance_interface_registry() -> Result<InterfaceRegistry, String> {
    Ok(InterfaceRegistry {
        generated_at: Local::now().to_rfc3339(),
        interfaces: vec![
            InterfaceEntry {
                id: "openrouter-anthropic".into(),
                display_name: "OpenRouter Anthropic-compatible".into(),
                kind: "anthropic-compatible".into(),
                base_url: Some("https://openrouter.ai/api/v1".into()),
                owner: "HomeBase endpoint config".into(),
                auth_source: "SOPS:openrouter_key via apiKeyHelper".into(),
                discovery_methods: vec!["GET /models".into(), "GET /v1/models".into()],
                risk: "medium".into(),
                notes: "Claude Code 当前通过 ANTHROPIC_BASE_URL 指向该接口；模型列表可通过网关探测。".into(),
            },
            InterfaceEntry {
                id: "anthropic-direct".into(),
                display_name: "Anthropic Direct".into(),
                kind: "anthropic".into(),
                base_url: Some("https://api.anthropic.com".into()),
                owner: "Provider".into(),
                auth_source: "SOPS:anthropic_key when configured".into(),
                discovery_methods: vec!["GET /v1/models".into()],
                risk: "medium".into(),
                notes: "官方接口适合作为基线；不建议用系统环境变量持久化 token。".into(),
            },
            InterfaceEntry {
                id: "openai-compatible-custom".into(),
                display_name: "OpenAI-compatible / sub2api / LiteLLM".into(),
                kind: "openai-compatible".into(),
                base_url: None,
                owner: "User supplied".into(),
                auth_source: "SOPS secret reference only".into(),
                discovery_methods: vec!["GET /v1/models".into(), "GET /models".into()],
                risk: "high".into(),
                notes: "接入新订阅或代理前先探测 schema、模型列表、鉴权方式和错误格式，再进入 agent 配置。".into(),
            },
            InterfaceEntry {
                id: "mcp-stdio".into(),
                display_name: "MCP stdio server".into(),
                kind: "mcp".into(),
                base_url: None,
                owner: "HomeBase Shared MCP Pool".into(),
                auth_source: "server env placeholders + runtime consent".into(),
                discovery_methods: vec!["initialize".into(), "tools/list".into()],
                risk: "high".into(),
                notes: "HTTP 接口之外的 PC 能力扩展入口；后续手动 MCP Console 应基于该注册表。".into(),
            },
        ],
    })
}

#[tauri::command]
pub async fn governance_interface_probe(
    payload: InterfaceProbePayload,
) -> Result<InterfaceProbeResult, String> {
    let base_url = payload.base_url.trim().trim_end_matches('/').to_string();
    if !base_url.starts_with("https://") && !base_url.starts_with("http://localhost") {
        return Err(
            "base_url 只允许 https:// 或 http://localhost，用于避免误探测不安全接口".into(),
        );
    }
    let attempted_urls = model_probe_urls(&payload.kind, &base_url);
    let mut headers = HeaderMap::new();
    if let Some(short_name) = payload
        .secret_short_name
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let token = reveal_secret(short_name).await?;
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|e| format!("invalid authorization header: {e}"))?;
        headers.insert(AUTHORIZATION, value);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .default_headers(headers)
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let started = std::time::Instant::now();
    let mut last_error = None;
    let mut last_status = None;
    let mut last_preview = None;

    for url in &attempted_urls {
        match client.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                last_status = Some(status.as_u16());
                let text = resp.text().await.unwrap_or_default();
                last_preview = Some(text.chars().take(300).collect::<String>());
                if status.is_success() {
                    let models = parse_model_ids(&text);
                    audit::record(
                        "interface.probed",
                        url,
                        serde_json::json!({
                            "kind": payload.kind,
                            "models": models.len(),
                            "status": status.as_u16()
                        }),
                    );
                    return Ok(InterfaceProbeResult {
                        ok: true,
                        attempted_urls,
                        status_code: last_status,
                        latency_ms: started.elapsed().as_millis(),
                        models,
                        error: None,
                        body_preview: last_preview,
                    });
                }
                last_error = Some(format!("HTTP {}", status.as_u16()));
            }
            Err(e) => {
                last_error = Some(e.to_string());
            }
        }
    }

    Ok(InterfaceProbeResult {
        ok: false,
        attempted_urls,
        status_code: last_status,
        latency_ms: started.elapsed().as_millis(),
        models: Vec::new(),
        error: last_error,
        body_preview: last_preview,
    })
}

fn model_probe_urls(kind: &str, base_url: &str) -> Vec<String> {
    let mut urls = Vec::new();
    if base_url.ends_with("/v1") || base_url.ends_with("/api/v1") {
        urls.push(format!("{base_url}/models"));
    } else if kind == "anthropic" {
        urls.push(format!("{base_url}/v1/models"));
    } else {
        urls.push(format!("{base_url}/v1/models"));
        urls.push(format!("{base_url}/models"));
    }
    urls
}

async fn reveal_secret(short_name: &str) -> Result<String, String> {
    if !short_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("secret_short_name {short_name:?} 含非法字符"));
    }
    let secure = SopsAdapter::new()
        .decrypt_key(&crate::secret_policy::vault_path(), short_name)
        .await
        .map_err(|e| format!("sops 解密 secret {short_name} 失败: {e}"))?;
    let token = secure.as_str().to_string();
    drop(secure);
    if token.trim().is_empty() {
        return Err(format!("secret {short_name} 解密结果为空"));
    }
    Ok(token)
}

fn parse_model_ids(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(items) = value.get("data").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut ids = items
        .iter()
        .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::shared_context::tests::TEST_ENV_LOCK;

    #[tokio::test]
    async fn capability_matrix_has_core_agents() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());

        let matrix = governance_capability_matrix().await.unwrap();
        assert!(matrix.rows.iter().any(|r| r.actor_id == "claude-code"));
        assert!(matrix.rows.iter().any(|r| r.actor_id == "codex"));
        assert!(matrix.rows.iter().any(|r| r.actor_id == "ccswitch"));

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }

    #[test]
    fn parse_model_ids_reads_openai_shape() {
        let ids = parse_model_ids(r#"{"data":[{"id":"b"},{"id":"a"},{"id":"a"}]}"#);
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn config_ownership_marks_secrets_plaintext_disallowed() {
        let entry = ownership(
            PathBuf::from(r"C:\HomeBase\Secrets\secrets.yaml"),
            "homebase",
            "SOPS age",
            "encrypted-only",
            false,
            true,
            "test",
        );
        assert!(!entry.plaintext_secret_allowed);
        assert!(entry.backup_required);
    }
}
