//! Secrets 模块命令 — 替代 secrets-cli WPF（v3 §6.3）
//!
//! 安全约束（v3 §12.2）：
//! - SOPS 解密结果仅内存（adapters::sops::SecureString，Drop 自动 zeroize）
//! - 解密 → 用完立即 drop；不入 SQLite，不入 log
//! - HTTP 检测仅显式执行；不自动读取剪贴板
//!
//! 11 keys 元数据来源：`tools/key-naming-convention.md`（阶段 12 权威映射表）

use crate::adapters::sops::SopsAdapter;
use crate::adapters::secret_vault::{Entry, Rotation, SaveRequest, SaveResult, Vault};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;


fn secrets_yaml_path() -> PathBuf {
    crate::secret_policy::vault_path()
}

fn secrets_meta_path() -> PathBuf {
    crate::secret_policy::metadata_path(&secrets_yaml_path())
}

// =====================================================================
// 类型定义
// =====================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SecretInfo {
    pub short_name: String,
    pub mcp_long_name: Option<String>,
    pub used_by: Vec<String>,
    pub category: SecretCategory,
    pub last_health: Option<HealthStatus>,
    pub expires_at: Option<String>,
    /// 用户自定义显示名（来自 secrets-meta.json 的 display_name 字段）；
    /// 前端优先用此名展示，缺失时 fallback 到 service-registry 启发式或 short_name
    pub display_name: Option<String>,
    /// 掩码值，例如 "sk-or-v...****...xyz"；如果 secrets.yaml 未含此 key 则为 "(missing)"
    pub masked_value: String,
    pub has_health_check: bool,
    pub rotate_url: Option<String>,
    pub configured: bool,
    pub value_state: String,
    pub revision: u64,
    pub vault_version: String,
    pub service_id: Option<String>,
    pub expiry_policy: String,
    pub rotations: Vec<Rotation>,
}

fn vault() -> Vault<SopsAdapter> {
    let yaml = secrets_yaml_path();
    let adapter = SopsAdapter::for_vault(&yaml);
    Vault::new(yaml, secrets_meta_path(), adapter)
}

fn service_meta<'a>(name: &str, entry: Option<&'a Entry>) -> Option<KeyMeta> {
    meta_for(entry.and_then(|e| e.service_id.as_deref()).unwrap_or(name))
}

#[tauri::command]
pub async fn secrets_save(request: SaveRequest) -> Result<SaveResult, String> {
    if let Some(service) = request.service_id.as_deref().filter(|s| !s.is_empty()) {
        if meta_for(service).is_none() { return Err("未知服务配置，请选择内置服务或自定义链接".into()); }
    }
    vault().save(request).await
}

#[derive(Serialize)]
pub struct LifecycleAlert {
    pub short_name: String,
    pub display_name: String,
    pub expires_at: Option<String>,
    pub expiry_policy: String,
    pub pending_revocation: bool,
}

/// Metadata only: background reminders never decrypt the vault.
#[tauri::command]
pub async fn secrets_lifecycle_status() -> Result<Vec<LifecycleAlert>, String> {
    let (metadata, _) = vault().metadata().await?;
    Ok(metadata.into_iter().filter(|(_, e)| e.expires_at.is_some() || e.rotations.iter().any(|r| r.revoked_at.is_none() && r.rolled_back_at.is_none()))
        .map(|(short_name, entry)| LifecycleAlert {
            display_name: entry.display_name.clone().unwrap_or_else(|| short_name.clone()),
            short_name, expires_at: entry.expires_at,
            expiry_policy: if entry.expiry_policy.is_empty() { "warn".into() } else { entry.expiry_policy },
            pending_revocation: entry.rotations.iter().any(|r| r.revoked_at.is_none() && r.rolled_back_at.is_none()),
        }).collect())
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecretCategory {
    /// Core 5：MCP 已映射，日常使用（OpenRouter / GitHub / Firecrawl / Tavily / Figma）
    Core,
    /// Future-ready：占位映射，待 SOPS 加入后激活（Anthropic / OpenAI / Gemini / DeepSeek / Obsidian / TryAllAI）
    FutureReady,
    /// GDNbit 项目专用（5 把：content_api / mongodb / nextauth / jwt / admin_secret）
    GdnbitProject,
    /// 未知（出现在 secrets.yaml 但不在 meta 表里 — 提示用户在 key-naming-convention.md 注册）
    Unknown,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HealthStatus {
    /// "valid" | "unauthorized" | "forbidden" | "not_found" | "network_error" | "no_check" | "unknown"
    pub status: String,
    pub checked_at: String,
    pub response_ms: Option<u32>,
    pub error: Option<String>,
}

pub type HealthResult = crate::adapters::secret_health::ProbeResult;

// =====================================================================
// 11 keys 静态元数据（来自 tools/key-naming-convention.md，2026-05-03 阶段 12）
// =====================================================================

#[derive(Debug, Clone, Copy)]
struct KeyMeta {
    long_name: Option<&'static str>,
    consumers: &'static [&'static str],
    category: SecretCategory,
    rotate_url: Option<&'static str>,
    health: Option<HealthSpec>,
}

#[derive(Debug, Clone, Copy)]
struct HealthSpec {
    method: HttpMethod,
    url: &'static str,
    /// header 名（如 "Authorization"、"X-API-Key"）
    auth_header: &'static str,
    /// 模板字符串：{key} 会被替换为实际 key 值（如 "Bearer {key}"、"{key}"）
    auth_format: &'static str,
}

#[derive(Debug, Clone, Copy)]
enum HttpMethod {
    Get,
}

fn meta_for(short_name: &str) -> Option<KeyMeta> {
    use HttpMethod::*;
    use SecretCategory::*;
    match short_name {
        // ==================== Core 5 ====================
        "openrouter_key" => Some(KeyMeta {
            long_name: Some("OPENROUTER_API_KEY"),
            consumers: &["Codex", "OpenClaw"],
            category: Core,
            rotate_url: Some("https://openrouter.ai/keys"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://openrouter.ai/api/v1/key",
                auth_header: "Authorization",
                auth_format: "Bearer {key}",
            }),
        }),
        "github_token" => Some(KeyMeta {
            long_name: Some("GITHUB_PERSONAL_ACCESS_TOKEN"),
            consumers: &["github-mcp-server (Docker)"],
            category: Core,
            rotate_url: Some("https://github.com/settings/tokens"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.github.com/user",
                auth_header: "Authorization",
                auth_format: "Bearer {key}",
            }),
        }),
        "firecrawl_token" => Some(KeyMeta {
            long_name: Some("FIRECRAWL_API_KEY"),
            consumers: &["firecrawl-mcp"],
            category: Core,
            rotate_url: Some("https://www.firecrawl.dev/app/api-keys"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.firecrawl.dev/v2/team/credit-usage",
                auth_header: "Authorization",
                auth_format: "Bearer {key}",
            }),
        }),
        "tavily_token" => Some(KeyMeta {
            long_name: Some("TAVILY_API_KEY"),
            consumers: &["tavily-mcp"],
            category: Core,
            rotate_url: Some("https://app.tavily.com/home"),
            // Read-only usage endpoint; never spend search credits for a key check.
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.tavily.com/usage",
                auth_header: "Authorization",
                auth_format: "Bearer {key}",
            }),
        }),
        "figma_token" => Some(KeyMeta {
            long_name: Some("FIGMA_ACCESS_TOKEN"),
            consumers: &["figma-developer-mcp"],
            category: Core,
            rotate_url: Some("https://www.figma.com/settings"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.figma.com/v1/me",
                auth_header: "X-Figma-Token",
                auth_format: "{key}",
            }),
        }),
        // ==================== Future-ready（含 1 已占位）====================
        "tryallai_key" => Some(KeyMeta {
            long_name: Some("TRYALLAI_API_KEY"),
            consumers: &["备用 LLM 网关"],
            category: FutureReady,
            rotate_url: None,
            health: None, // endpoint 未确定
        }),
        "anthropic_key" => Some(KeyMeta {
            long_name: Some("ANTHROPIC_API_KEY"),
            consumers: &["直调 Anthropic API（非 Claude Code）"],
            category: FutureReady,
            rotate_url: Some("https://console.anthropic.com/settings/keys"),
            health: None, // Anthropic 无简单鉴权探测端点（需带 model 参数 POST）
        }),
        "openai_key" => Some(KeyMeta {
            long_name: Some("OPENAI_API_KEY"),
            consumers: &["直调 OpenAI"],
            category: FutureReady,
            rotate_url: Some("https://platform.openai.com/api-keys"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.openai.com/v1/models",
                auth_header: "Authorization",
                auth_format: "Bearer {key}",
            }),
        }),
        "deepgram_key" => Some(KeyMeta {
            long_name: Some("DEEPGRAM_API_KEY"),
            consumers: &["ASR: Deepgram Nova-3", "Realtime captions"],
            category: FutureReady,
            rotate_url: Some("https://console.deepgram.com/"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.deepgram.com/v1/projects",
                auth_header: "Authorization",
                auth_format: "Token {key}",
            }),
        }),
        "assemblyai_key" => Some(KeyMeta {
            long_name: Some("ASSEMBLYAI_API_KEY"),
            consumers: &["ASR: AssemblyAI Universal"],
            category: FutureReady,
            rotate_url: Some("https://www.assemblyai.com/dashboard/signup"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.assemblyai.com/v2/transcript",
                auth_header: "Authorization",
                auth_format: "{key}",
            }),
        }),
        "elevenlabs_key" => Some(KeyMeta {
            long_name: Some("ELEVENLABS_API_KEY"),
            consumers: &["ASR: ElevenLabs Scribe", "TTS: ElevenLabs"],
            category: FutureReady,
            rotate_url: Some("https://elevenlabs.io/app/settings/api-keys"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.elevenlabs.io/v1/user/subscription",
                auth_header: "xi-api-key",
                auth_format: "{key}",
            }),
        }),
        "hf_token" => Some(KeyMeta {
            long_name: Some("HF_TOKEN"),
            consumers: &["Hugging Face downloads", "pyannote diarization"],
            category: FutureReady,
            rotate_url: Some("https://huggingface.co/settings/tokens"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://huggingface.co/api/whoami-v2",
                auth_header: "Authorization",
                auth_format: "Bearer {key}",
            }),
        }),
        "gemini_key" => Some(KeyMeta {
            long_name: Some("GEMINI_API_KEY"),
            consumers: &["Gemini CLI / Gemini MCP"],
            category: FutureReady,
            rotate_url: Some("https://aistudio.google.com/apikey"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://generativelanguage.googleapis.com/v1beta/models",
                auth_header: "X-Goog-Api-Key",
                auth_format: "{key}",
            }),
        }),
        "deepseek_key" => Some(KeyMeta {
            long_name: Some("DEEPSEEK_API_KEY"),
            consumers: &["DeepSeek V4 直调"],
            category: FutureReady,
            rotate_url: Some("https://platform.deepseek.com/api_keys"),
            health: Some(HealthSpec {
                method: Get,
                url: "https://api.deepseek.com/v1/models",
                auth_header: "Authorization",
                auth_format: "Bearer {key}",
            }),
        }),
        "obsidian_api_key_raw" => Some(KeyMeta {
            long_name: Some("OBSIDIAN_API_KEY"),
            consumers: &["mcp-obsidian REST 版"],
            category: FutureReady,
            rotate_url: None,
            health: None, // 本地 plugin，需要 Obsidian 启动 + plugin 监听端口
        }),
        // ==================== GDNbit 项目（5 把）====================
        "gdnbit_content_api_key" => Some(KeyMeta {
            long_name: None, // 项目内直接读 SOPS 短名，不需要长名映射
            consumers: &["publishing-kit", "GDNbit Web"],
            category: GdnbitProject,
            rotate_url: Some("https://example.invalid/admin"),
            // No confirmed read-only auth endpoint: use manual verification.
            health: None,
        }),
        "gdnbit_mongodb_password" => Some(KeyMeta {
            long_name: None,
            consumers: &["MongoDB gdnbit 用户", "crontab 备份脚本"],
            category: GdnbitProject,
            rotate_url: None,
            health: None, // 数据库密码不能通过 HTTP 测
        }),
        "gdnbit_nextauth_secret" => Some(KeyMeta {
            long_name: None,
            consumers: &["NextAuth.js session 加密"],
            category: GdnbitProject,
            rotate_url: None,
            health: None, // 加密 secret 不能通过 HTTP 测
        }),
        "gdnbit_jwt_secret" => Some(KeyMeta {
            long_name: None,
            consumers: &["JWT 签名"],
            category: GdnbitProject,
            rotate_url: None,
            health: None,
        }),
        "gdnbit_admin_secret_key" => Some(KeyMeta {
            long_name: None,
            consumers: &["内部管理 API 校验"],
            category: GdnbitProject,
            rotate_url: Some("https://example.invalid/admin"),
            health: None, // 内部管理 API endpoint 需要专属 path
        }),
        _ => None,
    }
}

/// 11 keys 已知短名清单（与 meta_for match 分支保持一致）
/// 注意：未来加新 key 必须同步 (meta_for + KNOWN_SHORT_NAMES)
const KNOWN_SHORT_NAMES: &[&str] = &[
    "openrouter_key",
    "github_token",
    "firecrawl_token",
    "tavily_token",
    "figma_token",
    "tryallai_key",
    "anthropic_key",
    "openai_key",
    "deepgram_key",
    "assemblyai_key",
    "elevenlabs_key",
    "hf_token",
    "gemini_key",
    "deepseek_key",
    "obsidian_api_key_raw",
    "gdnbit_content_api_key",
    "gdnbit_mongodb_password",
    "gdnbit_nextauth_secret",
    "gdnbit_jwt_secret",
    "gdnbit_admin_secret_key",
];

// =====================================================================
// 工具函数
// =====================================================================


/// 校验短名格式（v3 §6.3 命名规范 + 防 sops --extract 注入）
///
/// 规则：
/// - 仅 lowercase + 数字 + underscore
/// - 字母开头
/// - 3-64 字符
fn validate_short_name(short_name: &str) -> Result<(), String> {
    if short_name.len() < 3 || short_name.len() > 64 {
        return Err(format!(
            "短名长度须在 3-64 字符之间，当前 {}",
            short_name.len()
        ));
    }
    let mut chars = short_name.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err("短名必须以小写字母开头".into());
    }
    for c in chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' {
            return Err(format!(
                "短名只能含小写字母 / 数字 / 下划线（含非法字符 '{c}'）"
            ));
        }
    }
    Ok(())
}

/// 把 key 值掩码：前 7 + ...****... + 后 4
/// 短 key（< 16 字符）一律 "********"（避免 "abc...xyz" 给攻击者太多线索）
#[cfg(test)]
fn mask_value(v: &str) -> String {
    let total = v.chars().count();
    if total < 16 {
        return "********".to_string();
    }
    let prefix: String = v.chars().take(7).collect();
    let suffix: String = v
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}...****...{suffix}")
}


fn category_rank(c: &SecretCategory) -> u8 {
    match c {
        SecretCategory::Core => 0,
        SecretCategory::FutureReady => 1,
        SecretCategory::GdnbitProject => 2,
        SecretCategory::Unknown => 3,
    }
}

// =====================================================================
// Tauri commands
// =====================================================================

/// 列出全部 keys（合并 secrets.yaml 实际内容 + meta 注册表）
///
/// 行为：
/// - secrets.yaml 里有 → 真实 masked + 元数据
/// - meta 表有但 yaml 没有 → masked = "(missing)"，提示该 key 还没装
/// - yaml 有但 meta 表没有 → category = Unknown
#[tauri::command]
pub async fn secrets_list() -> Result<Vec<SecretInfo>, String> {
    tracing::debug!("secrets_list 调用");
    let snapshot = vault().snapshot().await?;
    let meta_map = snapshot.metadata;

    let mut result = Vec::new();
    let mut yaml_short_set = std::collections::HashSet::new();

    // 第一遍：yaml 里实际存在的 keys
    for value in &snapshot.values {
        let short = &value.name;
        yaml_short_set.insert(short.clone());
        let user_meta = meta_map.get(short);
        let meta = service_meta(short, user_meta);
        result.push(SecretInfo {
            short_name: short.clone(),
            mcp_long_name: meta_for(short).and_then(|m| m.long_name.map(|s| s.to_string())),
            used_by: meta_for(short)
                .map(|m| m.consumers.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default(),
            category: meta.map(|m| m.category).unwrap_or(SecretCategory::Unknown),
            last_health: None,
            expires_at: user_meta.and_then(|m| m.expires_at.clone()),
            display_name: user_meta.and_then(|m| m.display_name.clone()),
            masked_value: value.masked.clone(),
            has_health_check: meta.map(|m| m.health.is_some()).unwrap_or(false),
            rotate_url: user_meta.and_then(|m| m.rotate_url.clone()).or_else(|| meta.and_then(|m| m.rotate_url.map(str::to_string))),
            configured: value.state == "filled",
            value_state: value.state.clone(),
            revision: user_meta.map(|m| m.revision).unwrap_or(0),
            vault_version: snapshot.version.clone(),
            service_id: user_meta.and_then(|m| m.service_id.clone()),
            expiry_policy: user_meta.map(|m| m.expiry_policy.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "warn".into()),
            rotations: user_meta.map(|m| m.rotations.clone()).unwrap_or_default(),
        });
    }

    // 第二遍：meta 表里有但 yaml 没有的 — 显示 "(missing)" 提示用户
    let all_names: std::collections::BTreeSet<&str> = KNOWN_SHORT_NAMES.iter().copied().chain(meta_map.keys().map(String::as_str)).collect();
    for known_short in all_names {
        if yaml_short_set.contains(known_short) {
            continue;
        }
        let user_meta = meta_map.get(known_short);
        let meta = service_meta(known_short, user_meta);
        result.push(SecretInfo {
            short_name: known_short.to_string(),
            mcp_long_name: meta_for(known_short).and_then(|m| m.long_name.map(str::to_string)),
            used_by: meta_for(known_short).map(|m| m.consumers.iter().map(|s| s.to_string()).collect()).unwrap_or_default(),
            category: meta.map(|m| m.category).unwrap_or(SecretCategory::Unknown),
            last_health: None,
            expires_at: user_meta.and_then(|m| m.expires_at.clone()),
            display_name: user_meta.and_then(|m| m.display_name.clone()),
            masked_value: "(missing)".to_string(),
            has_health_check: meta.is_some_and(|m| m.health.is_some()),
            rotate_url: user_meta.and_then(|m| m.rotate_url.clone()).or_else(|| meta.and_then(|m| m.rotate_url.map(str::to_string))),
            configured: false,
            value_state: "missing".into(),
            revision: user_meta.map(|m| m.revision).unwrap_or(0),
            vault_version: snapshot.version.clone(),
            service_id: user_meta.and_then(|m| m.service_id.clone()),
            expiry_policy: user_meta.map(|m| m.expiry_policy.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "warn".into()),
            rotations: user_meta.map(|m| m.rotations.clone()).unwrap_or_default(),
        });
    }

    // 排序：Core 在前 → Future-ready → GDNbit → Unknown；同 category 按 short_name
    result.sort_by(|a, b| {
        category_rank(&a.category)
            .cmp(&category_rank(&b.category))
            .then_with(|| a.short_name.cmp(&b.short_name))
    });

    Ok(result)
}

/// 调真 API 验证 key 有效性
///
/// 流程：
/// 1. 用 SopsAdapter 单 key 解密（`--extract '["short_name"]'`）
/// 2. 按 meta 表 HealthSpec 发起 HTTP 请求（带 8s timeout）
/// 3. 解读 status code → "valid" / "unauthorized" / "forbidden" / "network_error"
/// 4. SecureString 在函数结束时 drop → zeroize
fn no_check(message: &str) -> HealthResult {
    HealthResult { success: false, status: "no_check".into(), response_ms: 0, http_code: None, error: Some(message.into()) }
}

async fn probe_value(spec: HealthSpec, value: &str) -> Result<HealthResult, String> {
    let auth = zeroize::Zeroizing::new(spec.auth_format.replace("{key}", value));
    crate::adapters::secret_health::probe(spec.url, match spec.method { HttpMethod::Get => false }, spec.auth_header, &auth).await
}

#[tauri::command]
pub async fn secrets_test(short_name: String) -> Result<HealthResult, String> {
    if validate_short_name(&short_name).is_err() { return Ok(no_check("无效或未知的密钥短名")); }
    if meta_for(&short_name).is_some_and(|m| m.health.is_none()) { return Ok(no_check("该服务没有受支持的自动检测接口")); }
    let (metadata, _) = vault().metadata().await?;
    let Some(spec) = service_meta(&short_name, metadata.get(&short_name)).and_then(|m| m.health) else {
        return Ok(no_check("该服务没有受支持的自动检测接口"));
    };
    let secure = SopsAdapter::new().decrypt_key(&secrets_yaml_path(), &short_name).await
        .map_err(|_| "当前密钥不可读取或已被到期策略阻止".to_string())?;
    probe_value(spec, secure.as_str()).await
}

#[tauri::command]
pub async fn secrets_test_candidate(service_id: String, value: String) -> Result<HealthResult, String> {
    let value = zeroize::Zeroizing::new(value);
    if value.trim().is_empty() || value.len() > 65536 { return Err("候选密钥不能为空或过长".into()); }
    let Some(spec) = meta_for(&service_id).and_then(|m| m.health) else {
        return Ok(no_check("此服务无自动检测接口，请在服务商处人工验证候选密钥"));
    };
    probe_value(spec, &value).await
}

/// 返回该 key 的轮换页面 URL（前端打开浏览器）
#[tauri::command]
pub async fn secrets_rotate_url(short_name: String) -> Result<Option<String>, String> {
    validate_short_name(&short_name)?;
    let (metadata, _) = vault().metadata().await?;
    Ok(rotation_url(&short_name, metadata.get(&short_name)))
}

fn rotation_url(name: &str, entry: Option<&Entry>) -> Option<String> {
    entry.and_then(|e| e.rotate_url.clone()).or_else(|| service_meta(name, entry).and_then(|m| m.rotate_url.map(str::to_string)))
}

// =====================================================================
// secrets_reveal — 解密单 key 返回明文（前端 30s 自动 hide）
// =====================================================================

#[tauri::command]
pub async fn secrets_reveal(short_name: String) -> Result<String, String> {
    tracing::debug!("secrets_reveal 调用: {}", short_name);
    validate_short_name(&short_name)?;
    let adapter = SopsAdapter::new();
    let yaml_path = secrets_yaml_path();
    let secure = adapter
        .decrypt_key(&yaml_path, &short_name)
        .await
        .map_err(|e| format!("sops 解密失败: {e}"))?;
    let plain = secure.as_str().to_string();
    drop(secure);
    if plain.is_empty() {
        return Err(format!("key {short_name} 解密结果为空"));
    }
    Ok(plain)
}

// =====================================================================
// secrets_set — 加密回写（backup + sops --set + round-trip verify）
// =====================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct SetResult {
    pub success: bool,
    pub backup_path: Option<String>,
    pub verified: bool,
    pub value_length: usize,
    pub error: Option<String>,
}

#[tauri::command]
pub async fn secrets_set(short_name: String, value: String) -> Result<SetResult, String> {
    let _value = zeroize::Zeroizing::new(value);
    let _ = short_name;
    Err("旧保存接口缺少版本保护，已停用；请刷新密钥页面后使用新版保存流程".into())
}

// =====================================================================
// secrets_set_meta — 写入 secrets-meta.json 的 expires_at（不动 yaml）
// =====================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct SetMetaResult {
    pub success: bool,
    pub error: Option<String>,
}

/// 注册或更新 secrets-meta.json 中某个 key 的元数据（expires_at / display_name）
///
/// **Merge 语义**：
/// - 参数 = `None`  → 不动该字段（保留旧值）
/// - 参数 = `Some("")` → 清空（删除该字段）
/// - 参数 = `Some("xxx")` → 设值
///
/// - 文件不存在时自动创建
/// - 不修改 secrets.yaml；仅元数据
#[tauri::command]
pub async fn secrets_set_meta(short_name: String, expires_at: Option<String>, display_name: Option<String>) -> Result<SetMetaResult, String> {
    let _ = (short_name, expires_at, display_name);
    Err("旧元数据接口缺少事务保护，已停用；请刷新密钥页面".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_long_value() {
        let v = "sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789";
        let m = mask_value(v);
        assert!(m.starts_with("sk-or-v"), "prefix wrong: {m}");
        assert!(m.ends_with("6789"), "suffix wrong: {m}");
        assert!(m.contains("****"), "should contain ****: {m}");
    }

    #[test]
    fn mask_short_value_redacts_fully() {
        assert_eq!(mask_value("short"), "********");
        assert_eq!(mask_value("123456789012345"), "********"); // 15 chars
    }

    #[test]
    fn meta_table_aligns_with_known_shorts() {
        for k in KNOWN_SHORT_NAMES {
            assert!(meta_for(k).is_some(), "missing meta for {k}");
        }
    }

    #[test]
    fn unknown_key_returns_none() {
        assert!(meta_for("__never_a_real_key__").is_none());
    }

    #[test]
    fn validate_short_name_accepts_known() {
        for k in KNOWN_SHORT_NAMES {
            assert!(validate_short_name(k).is_ok(), "should accept: {k}");
        }
    }

    #[test]
    fn validate_short_name_accepts_new_user_name() {
        assert!(validate_short_name("my_new_api_key_2026").is_ok());
        assert!(validate_short_name("abc").is_ok());
    }

    #[test]
    fn validate_short_name_rejects_uppercase() {
        assert!(validate_short_name("OpenRouter_Key").is_err());
    }

    #[test]
    fn validate_short_name_rejects_starts_with_digit() {
        assert!(validate_short_name("1abc_key").is_err());
    }

    #[test]
    fn validate_short_name_rejects_special_chars() {
        for bad in ["my-key", "my.key", "my key", "my$key", "my/key", "my\"key"] {
            assert!(validate_short_name(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn validate_short_name_rejects_too_short_or_too_long() {
        assert!(validate_short_name("ab").is_err()); // 2 chars
        assert!(validate_short_name(&"a".repeat(65)).is_err()); // 65 chars
    }

    #[test]
    fn category_rank_orders_core_first() {
        assert!(category_rank(&SecretCategory::Core) < category_rank(&SecretCategory::FutureReady));
        assert!(
            category_rank(&SecretCategory::FutureReady)
                < category_rank(&SecretCategory::GdnbitProject)
        );
    }

    #[test]
    fn secrets_rotate_url_returns_correct_url() {
        let url = rotation_url("openrouter_key", None);
        assert_eq!(url.as_deref(), Some("https://openrouter.ai/keys"));
    }

    #[test]
    fn secrets_rotate_url_for_unknown_returns_none() {
        let url = rotation_url("__unknown__", None);
        assert_eq!(url, None);
    }

    #[tokio::test]
    async fn secrets_test_unknown_key_returns_no_check() {
        let r = secrets_test("__unknown__".to_string()).await.expect("ok");
        assert!(!r.success);
        assert_eq!(r.status, "no_check");
    }

    #[tokio::test]
    async fn secrets_test_no_health_returns_no_check() {
        // gdnbit_mongodb_password 无 health spec
        let r = secrets_test("gdnbit_mongodb_password".to_string())
            .await
            .expect("ok");
        assert!(!r.success);
        assert_eq!(r.status, "no_check");
    }
}
