//! Agent 历史对话（Phase D · 2026-05-04）
//!
//! 读 agent 本地存的对话历史，在 Agent 详情 HistoryTab 展示。
//!
//! **数据源**(Phase H · 2026-05-05 实地调研 + 社区官方文档交叉确认)：
//! - `claude-code` → `~/.claude/projects/<cwd-slug>/<session_id>.jsonl`
//!   完整对话(user/assistant/system/tool)；cwd 路径编码：`C:\Users\homebase` → `C--Users-homebase`
//! - `codex` → `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<sid>.jsonl` (Phase H 改)
//!   完整对话；首行 session_meta 含 cwd/originator；消息用 type=response_item 包装。
//!   旧 `~/.codex/history.jsonl` 只有纯 user prompt 流水，已弃用为主源。
//! - `gemini-cli` → `~/.gemini/tmp/<hash>/logs.json` (Phase H 新增)
//!   JSON array，每条 `{sessionId, messageId, type: "user", message, timestamp}`；
//!   **只有 user prompts**，无 assistant 回复(官方限制)。
//! - `openclaw` → 本适配器尚未实现其版本相关的持久会话读取；不能据此判断产品不存会话。
//! - `hermes` → `~/.hermes/state.db` (SQLite + FTS5)。schema 已调研，但需要
//!   `rusqlite` 依赖才能读取。当前未装 Hermes，暂留技术债(未来要加)。
//!
//! **隐私**：历史只在本机读取，不离开本机。content 原样返回，前端做脱敏。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[path = "agents_history_cache.rs"]
mod summary_cache;
use summary_cache::{
    open_history, open_matching_history, read_summaries, SummaryCache, HISTORY_READ_FAILED,
};

fn env_home() -> String {
    crate::adapters::homebase_home_dir().to_string_lossy().into_owned()
}

// =====================================================================
// 对外 schema
// =====================================================================

#[derive(Debug, Serialize, Clone)]
pub struct HistoryEntry {
    pub agent_id: String,
    pub session_id: String,
    /// cwd（Claude 有，Codex 无 → None）
    pub cwd: Option<String>,
    /// 第一条 user prompt 预览（前 120 字）
    pub first_user_prompt: Option<String>,
    /// 会话开始时间（ISO8601 UTC）
    pub started_at: String,
    /// 会话最后活动时间
    pub last_active_at: String,
    /// 消息数（user + assistant + system）
    pub message_count: u32,
    /// 绝对路径（用于 "在资源管理器中显示"）
    pub file_path: String,
}

#[derive(Debug, Serialize)]
pub struct HistoryListResult {
    pub agent_id: String,
    /// 是否支持
    pub supported: bool,
    /// 不支持的原因（hint）
    pub unsupported_reason: Option<String>,
    /// 会话列表（按 last_active_at 倒序）
    pub entries: Vec<HistoryEntry>,
    /// 数据源根（例如 ~/.claude/projects）
    pub data_root: String,
}

#[derive(Debug, Serialize)]
pub struct HistorySession {
    pub agent_id: String,
    pub session_id: String,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    pub messages: Vec<HistoryMessage>,
    pub stats: SessionStats,
    pub file_path: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct HistoryMessage {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    /// user / assistant / system
    pub role: String,
    /// 扁平化后的文本（tool_use / tool_result 会标记 [tool]）
    pub content: String,
    pub timestamp: String,
    /// claude-code 有（如 "anthropic/claude-4.6-sonnet-20260217"）
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

#[derive(Debug, Serialize, Default)]
pub struct SessionStats {
    pub message_count: u32,
    pub user_count: u32,
    pub assistant_count: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub total_cache_write_tokens: u64,
    pub duration_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct AgentBucket {
    pub agent_id: String,
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    pub entry_count: u32,
    pub data_root: String,
}

#[derive(Debug, Serialize)]
pub struct HistoryAllResult {
    pub entries: Vec<HistoryEntry>,
    pub buckets: Vec<AgentBucket>,
}

// =====================================================================
// Tauri commands
// =====================================================================

#[tauri::command]
pub async fn agents_history_list(id: String) -> Result<HistoryListResult, String> {
    tracing::debug!("agents_history_list 调用: {}", id);

    tokio::task::spawn_blocking(move || match id.as_str() {
        "claude-code" => list_claude_code(),
        "codex" => list_codex(),
        "gemini-cli" => list_gemini_cli(),
        "openclaw" => Ok(unsupported(
            &id,
            "当前适配器未接入 OpenClaw 会话存储；这不表示 OpenClaw 不支持持久会话。请使用其原生入口，或显式交接到 HomeBase。",
        )),
        "hermes" => list_hermes(),
        _ => Err(format!("未知 agent id: {id}")),
    })
    .await
    .map_err(|_| "QUERY_FAILED: 历史列表读取失败")?
}

#[tauri::command]
pub async fn agents_history_read(
    id: String,
    session_id: String,
) -> Result<HistorySession, String> {
    tracing::debug!("agents_history_read 调用: {} / {}", id, session_id);

    tokio::task::spawn_blocking(move || match id.as_str() {
        "claude-code" => read_claude_code(&session_id),
        "codex" => read_codex(&session_id),
        "gemini-cli" => read_gemini_cli(&session_id),
        "openclaw" => Err("OpenClaw 不持久化对话历史".to_string()),
        "hermes" => Err("Hermes 历史需 rusqlite 依赖，尚未接入(见 agents_history.rs 顶部注释)".to_string()),
        _ => Err(format!("未知 agent id: {id}")),
    })
    .await
    .map_err(|_| "QUERY_FAILED: 历史详情读取失败")?
}

/// Phase H · 跨 agent 历史聚合(all agents 合并列表)
///
/// 用于 `/history` 页面的总览视图。每条都带 agent_id，方便前端显示图标。
/// 按 last_active_at 倒序；unsupported 的 agent 被静默跳过(不进聚合)。
#[tauri::command]
pub async fn agents_history_all() -> Result<HistoryAllResult, String> {
    tracing::debug!("agents_history_all 调用");

    let mut buckets: Vec<AgentBucket> = Vec::new();
    let mut all_entries: Vec<HistoryEntry> = Vec::new();

    for id in ["claude-code", "codex", "gemini-cli", "openclaw", "hermes"] {
        let result = agents_history_list(id.to_string()).await;
        match result {
            Ok(r) => {
                // 进聚合列表(仅 supported 且有条目的)
                if r.supported {
                    all_entries.extend(r.entries.iter().cloned());
                }
                buckets.push(AgentBucket {
                    agent_id: id.to_string(),
                    supported: r.supported,
                    unsupported_reason: r.unsupported_reason.clone(),
                    entry_count: r.entries.len() as u32,
                    data_root: r.data_root.clone(),
                });
            }
            Err(e) => {
                // 单个 agent 出错不阻塞整体
                tracing::warn!("agents_history_all: {id} 读取失败: {e}");
                buckets.push(AgentBucket {
                    agent_id: id.to_string(),
                    supported: false,
                    unsupported_reason: Some(format!("读取失败：{e}")),
                    entry_count: 0,
                    data_root: String::new(),
                });
            }
        }
    }

    // 按 last_active_at 倒序
    all_entries.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));

    Ok(HistoryAllResult {
        entries: all_entries,
        buckets,
    })
}

fn unsupported(id: &str, reason: &str) -> HistoryListResult {
    HistoryListResult {
        agent_id: id.to_string(),
        supported: false,
        unsupported_reason: Some(reason.to_string()),
        entries: vec![],
        data_root: String::new(),
    }
}

// =====================================================================
// Claude Code parser
// =====================================================================

fn claude_projects_root() -> PathBuf {
    PathBuf::from(env_home()).join(".claude").join("projects")
}

/// 把目录名反编码成 cwd（尽力；只能猜）
///
/// `C--Users-homebase` → `C:\Users\homebase`
/// `C--WINDOWS-system32` → `C:\WINDOWS\system32`
fn decode_cwd_slug(slug: &str) -> Option<String> {
    // slug 以 "<盘符>--" 开头
    let bytes = slug.as_bytes();
    if bytes.len() < 3 || bytes[1] != b'-' || bytes[2] != b'-' {
        return None;
    }
    let drive = bytes[0] as char;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    // 把后面的 "-" 当 "\"（无法区分原路径里真有 - 的情况，先妥协）
    let rest = &slug[3..];
    let path = rest.replace('-', "\\");
    Some(format!("{}:{}{}", drive.to_ascii_uppercase(), "\\", path))
}

fn next_jsonl_record(
    reader: &mut impl BufRead,
    line: &mut String,
) -> Result<Option<serde_json::Value>, String> {
    loop {
        line.clear();
        if reader.read_line(line).map_err(|_| HISTORY_READ_FAILED)? == 0 {
            return Ok(None);
        }
        if !line.trim().is_empty() {
            return serde_json::from_str(line)
                .map(Some)
                .map_err(|_| "QUERY_INVALID: 历史会话记录格式无效或尚未写完，请重试".into());
        }
    }
}

fn list_claude_code() -> Result<HistoryListResult, String> {
    let root = claude_projects_root();
    let data_root = root.display().to_string();
    let mut paths = Vec::new();
    // root 下第一层是 cwd-slug 目录
    let cwd_dirs = crate::query::entries(&root, true)?;

    for cwd_entry in cwd_dirs {
        let cwd_path = cwd_entry.path();
        if !cwd_path.is_dir() {
            continue;
        }

        // 只扫 cwd_path 下的 .jsonl 文件（不递归 — subagents 暂不展示）
        for file in crate::query::entries(&cwd_path, false)? {
            let p = file.path();
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            // 只收 UUID v4 形式的会话文件（避免把临时文件当 session）
            if !looks_like_uuid(stem) {
                continue;
            }
            paths.push(p);
        }
    }

    static CACHE: OnceLock<Mutex<SummaryCache>> = OnceLock::new();
    let mut entries = read_summaries(CACHE.get_or_init(Mutex::default), &paths, |path, file| {
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or("QUERY_INVALID: Claude 历史会话缺少编号")?;
        let cwd = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|slug| decode_cwd_slug(&slug.to_string_lossy()));
        Ok(vec![summarize_claude_session(
            path,
            file,
            session_id,
            cwd.as_deref(),
        )?])
    })?;
    entries.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));

    Ok(HistoryListResult {
        agent_id: "claude-code".into(),
        supported: true,
        unsupported_reason: None,
        entries,
        data_root,
    })
}

fn looks_like_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(i, &b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        _ => b.is_ascii_hexdigit(),
    })
}

/// 扫 jsonl 把第一条 user prompt + 首末时间戳 + 消息总数凑出来
fn summarize_claude_session(
    path: &Path,
    file: &File,
    session_id: &str,
    cwd: Option<&str>,
) -> Result<HistoryEntry, String> {
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut first_user_prompt: Option<String> = None;
    let mut started_at: Option<String> = None;
    let mut last_active_at: Option<String> = None;
    let mut message_count: u32 = 0;

    while let Some(v) = next_jsonl_record(&mut reader, &mut line)? {
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        // 更新首末时间
        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
            if started_at.is_none() {
                started_at = Some(ts.to_string());
            }
            last_active_at = Some(ts.to_string());
        }

        // 消息数只计 user / assistant
        if ty == "user" || ty == "assistant" {
            message_count += 1;
        }

        // 抓第一条 user 文本
        if first_user_prompt.is_none() && ty == "user" {
            if let Some(text) = extract_claude_user_text(&v) {
                let preview: String = text.chars().take(120).collect();
                first_user_prompt = Some(preview);
            }
        }
    }

    Ok(HistoryEntry {
        agent_id: "claude-code".into(),
        session_id: session_id.into(),
        cwd: cwd.map(|s| s.to_string()),
        first_user_prompt,
        started_at: started_at.unwrap_or_default(),
        last_active_at: last_active_at.unwrap_or_default(),
        message_count,
        file_path: path.display().to_string(),
    })
}

fn extract_claude_user_text(v: &serde_json::Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
    match content {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let mut buf = String::new();
            for block in arr {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        if !buf.is_empty() {
                            buf.push('\n');
                        }
                        buf.push_str(t);
                    }
                }
            }
            if buf.is_empty() { None } else { Some(buf) }
        }
        _ => None,
    }
}

fn read_claude_code(session_id: &str) -> Result<HistorySession, String> {
    if !looks_like_uuid(session_id) {
        return Err(format!("非法 session_id: {session_id}"));
    }
    // 在 projects 下任意 cwd 目录里找 <session_id>.jsonl
    let root = claude_projects_root();
    let Ok(cwd_dirs) = std::fs::read_dir(&root) else {
        return Err(format!("无法读取 {}", root.display()));
    };

    let mut file_path: Option<PathBuf> = None;
    let mut cwd_decoded: Option<String> = None;
    for cwd_entry in cwd_dirs {
        let cwd_entry = cwd_entry.map_err(|_| "QUERY_FAILED: 历史会话目录条目不可读取")?;
        let p = cwd_entry.path().join(format!("{session_id}.jsonl"));
        if p.exists() {
            file_path = Some(p);
            cwd_decoded = decode_cwd_slug(
                cwd_entry.file_name().to_string_lossy().as_ref(),
            );
            break;
        }
    }
    let file_path = file_path.ok_or_else(|| format!("找不到 session {session_id}"))?;

    let mut reader = BufReader::new(open_history(&file_path)?);
    let mut line = String::new();

    let mut messages: Vec<HistoryMessage> = Vec::new();
    let mut stats = SessionStats::default();
    let mut git_branch: Option<String> = None;
    let mut version: Option<String> = None;
    let mut first_ts: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut last_ts: Option<chrono::DateTime<chrono::Utc>> = None;

    while let Some(v) = next_jsonl_record(&mut reader, &mut line)? {
        if git_branch.is_none() {
            git_branch = v
                .get("gitBranch")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
        }
        if version.is_none() {
            version = v
                .get("version")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
        }

        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        // 只收 user / assistant / system（其他跳过）
        if ty != "user" && ty != "assistant" && ty != "system" {
            continue;
        }

        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&ts) {
            let utc = dt.with_timezone(&chrono::Utc);
            if first_ts.is_none() {
                first_ts = Some(utc);
            }
            last_ts = Some(utc);
        }

        let role = v
            .get("message")
            .and_then(|m| m.get("role"))
            .and_then(|r| r.as_str())
            .unwrap_or(ty)
            .to_string();

        let content = extract_claude_message_content(&v, ty);

        let model = v
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());

        let usage = v.get("message").and_then(|m| m.get("usage"));
        let input_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|x| x.as_u64());
        let output_tokens = usage
            .and_then(|u| u.get("output_tokens"))
            .and_then(|x| x.as_u64());
        let cache_read_tokens = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|x| x.as_u64());
        let cache_write_tokens = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|x| x.as_u64());

        if ty == "user" {
            stats.user_count += 1;
        }
        if ty == "assistant" {
            stats.assistant_count += 1;
        }
        stats.message_count += 1;
        stats.total_input_tokens += input_tokens.unwrap_or(0);
        stats.total_output_tokens += output_tokens.unwrap_or(0);
        stats.total_cache_read_tokens += cache_read_tokens.unwrap_or(0);
        stats.total_cache_write_tokens += cache_write_tokens.unwrap_or(0);

        messages.push(HistoryMessage {
            uuid: v
                .get("uuid")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            parent_uuid: v
                .get("parentUuid")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            role,
            content,
            timestamp: ts,
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
        });
    }

    if let (Some(f), Some(l)) = (first_ts, last_ts) {
        stats.duration_seconds = (l - f).num_seconds().max(0) as u64;
    }

    Ok(HistorySession {
        agent_id: "claude-code".into(),
        session_id: session_id.into(),
        cwd: cwd_decoded,
        git_branch,
        version,
        messages,
        stats,
        file_path: file_path.display().to_string(),
    })
}

fn extract_claude_message_content(v: &serde_json::Value, ty: &str) -> String {
    // system 类型 content 字段是 string（带 <command-name> 等）
    if ty == "system" {
        return v
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
    }
    let Some(content) = v.get("message").and_then(|m| m.get("content")) else {
        return String::new();
    };
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let mut buf = String::new();
            for block in arr {
                let bty = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let piece: String = match bty {
                    "text" => block
                        .get("text")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    "tool_use" => {
                        let name = block
                            .get("name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        let id = block.get("id").and_then(|x| x.as_str()).unwrap_or("");
                        format!("[tool_use: {name} · {id}]")
                    }
                    "tool_result" => {
                        let id = block
                            .get("tool_use_id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        let text = block
                            .get("content")
                            .map(|c| match c {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .unwrap_or_default();
                        let preview: String = text.chars().take(400).collect();
                        format!("[tool_result: {id}]\n{preview}")
                    }
                    "thinking" => {
                        let t = block
                            .get("thinking")
                            .and_then(|x| x.as_str())
                            .unwrap_or("");
                        format!("[thinking]\n{t}")
                    }
                    _ => format!("[{bty}]"),
                };
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(&piece);
            }
            buf
        }
        _ => String::new(),
    }
}

// =====================================================================
// Codex parser (Phase H · sessions/**/*.jsonl 主源)
//
// 数据源：~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl
// 每个文件一个 session，内容含完整对话(user/assistant/tool)。
//
// JSONL 行类型：
//   - session_meta · 一个文件只有一次，含 id/cwd/cli_version/originator
//   - turn_context · turn 切换记录(忽略)
//   - response_item · 主消息(payload.type=message, payload.role=user|assistant|developer)
//                     content 是 array of {type, text}，type ∈ input_text/output_text/
//                     function_call/function_call_output 等
//   - event_msg · 事件流(task_started/task_complete/token_count/error 等)
//
// 旧的 ~/.codex/history.jsonl 只有纯 user prompt 流水，已弃用。
// =====================================================================

fn codex_sessions_root() -> PathBuf {
    PathBuf::from(env_home()).join(".codex").join("sessions")
}

fn list_codex() -> Result<HistoryListResult, String> {
    let root = codex_sessions_root();
    let data_root = root.display().to_string();
    let mut paths = Vec::new();
    if root.try_exists().map_err(|_| "QUERY_FAILED: Codex 历史目录不可读取")? {
        walk_codex_rollouts(&root, &mut paths)?;
    }

    static CACHE: OnceLock<Mutex<SummaryCache>> = OnceLock::new();
    let mut entries = read_summaries(CACHE.get_or_init(Mutex::default), &paths, |path, file| {
        Ok(vec![summarize_codex_session(path, file)?])
    })?;
    entries.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));

    Ok(HistoryListResult {
        agent_id: "codex".into(),
        supported: true,
        unsupported_reason: None,
        entries,
        data_root,
    })
}

/// 递归扫 sessions/ 下所有 rollout-*.jsonl
fn walk_codex_rollouts(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in crate::query::entries(dir, false)? {
        let p = entry.path();
        if p.is_dir() {
            walk_codex_rollouts(&p, out)?;
        } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if name.starts_with("rollout-") && name.ends_with(".jsonl") {
                out.push(p);
            }
        }
    }
    Ok(())
}

/// 读 rollout 文件抽出会话摘要(session_id/cwd/起止时间/首条 user prompt/消息数)
fn summarize_codex_session(path: &Path, file: &File) -> Result<HistoryEntry, String> {
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut session_id: Option<String> = None;
    let mut cwd: Option<String> = None;
    let mut started_at: Option<String> = None;
    let mut last_active_at: Option<String> = None;
    let mut preview = crate::native_presentation::CodexPreview::default();
    let mut message_count: u32 = 0;

    while let Some(v) = next_jsonl_record(&mut reader, &mut line)? {
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        preview.observe(&v);
        let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
        if !ts.is_empty() {
            if started_at.is_none() {
                started_at = Some(ts.to_string());
            }
            last_active_at = Some(ts.to_string());
        }

        match ty {
            "session_meta" => {
                if let Some(payload) = v.get("payload") {
                    session_id = payload
                        .get("id")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    cwd = payload
                        .get("cwd")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                }
            }
            "response_item" => {
                let Some(payload) = v.get("payload") else {
                    continue;
                };
                let pty = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if pty != "message" {
                    continue;
                }
                let role = payload
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("");
                if role == "user" || role == "assistant" {
                    message_count += 1;
                }
            }
            _ => {}
        }
    }

    Ok(HistoryEntry {
        agent_id: "codex".into(),
        session_id: session_id.ok_or("QUERY_INVALID: 历史会话缺少编号")?,
        cwd,
        first_user_prompt: preview.finish(),
        started_at: started_at.unwrap_or_default(),
        last_active_at: last_active_at.unwrap_or_default(),
        message_count,
        file_path: path.display().to_string(),
    })
}

fn read_codex(session_id: &str) -> Result<HistorySession, String> {
    let root = codex_sessions_root();
    if !root.exists() {
        return Err(format!("Codex sessions 目录不存在：{}", root.display()));
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    walk_codex_rollouts(&root, &mut paths)?;

    // rollout 文件名形如 rollout-<ts>-<uuid>.jsonl，通过文件名包含 session_id 定位
    let path = paths
        .into_iter()
        .find(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.contains(session_id))
                .unwrap_or(false)
        })
        .ok_or_else(|| format!("找不到 Codex session {session_id}"))?;

    let mut reader = BufReader::new(open_history(&path)?);
    let mut line = String::new();

    let mut messages: Vec<HistoryMessage> = Vec::new();
    let mut stats = SessionStats::default();
    let mut cwd: Option<String> = None;
    let mut version: Option<String> = None;
    let mut first_ts: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut last_ts: Option<chrono::DateTime<chrono::Utc>> = None;

    while let Some(v) = next_jsonl_record(&mut reader, &mut line)? {
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let ts = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&ts) {
            let utc = dt.with_timezone(&chrono::Utc);
            if first_ts.is_none() {
                first_ts = Some(utc);
            }
            last_ts = Some(utc);
        }

        match ty {
            "session_meta" => {
                if let Some(payload) = v.get("payload") {
                    cwd = payload
                        .get("cwd")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    version = payload
                        .get("cli_version")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                }
            }
            "event_msg" => {
                // 抽 token_count 累计
                let Some(payload) = v.get("payload") else {
                    continue;
                };
                let pty = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if pty == "token_count" {
                    stats.total_input_tokens += payload
                        .get("input_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    stats.total_output_tokens += payload
                        .get("output_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    stats.total_cache_read_tokens += payload
                        .get("cache_read_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                }
            }
            "response_item" => {
                let Some(payload) = v.get("payload") else {
                    continue;
                };
                let pty = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if pty != "message" {
                    continue; // 跳过 function_call / function_call_output 顶层包装
                }
                let role = payload
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_string();
                let content = extract_codex_message_content(payload);

                match role.as_str() {
                    "user" => stats.user_count += 1,
                    "assistant" => stats.assistant_count += 1,
                    _ => {}
                }
                stats.message_count += 1;

                messages.push(HistoryMessage {
                    uuid: format!("{}:{}", session_id, messages.len()),
                    parent_uuid: None,
                    role,
                    content,
                    timestamp: ts,
                    model: None,
                    input_tokens: None,
                    output_tokens: None,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                });
            }
            _ => {}
        }
    }

    if let (Some(f), Some(l)) = (first_ts, last_ts) {
        stats.duration_seconds = (l - f).num_seconds().max(0) as u64;
    }

    Ok(HistorySession {
        agent_id: "codex".into(),
        session_id: session_id.into(),
        cwd,
        git_branch: None,
        version,
        messages,
        stats,
        file_path: path.display().to_string(),
    })
}

/// 扁平化 Codex response_item.content 数组为单段文本(类似 Claude 的处理)
fn extract_codex_message_content(payload: &serde_json::Value) -> String {
    let Some(content) = payload.get("content").and_then(|c| c.as_array()) else {
        return String::new();
    };
    let mut buf = String::new();
    for block in content {
        let bty = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let piece: String = match bty {
            "input_text" | "output_text" => block
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            "function_call" => {
                let name = block.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let args = block
                    .get("arguments")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let preview: String = args.chars().take(200).collect();
                format!("[function_call: {name}]\n{preview}")
            }
            "function_call_output" => {
                let preview: String = block
                    .get("output")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(400)
                    .collect();
                format!("[function_call_output]\n{preview}")
            }
            _ => format!("[{bty}]"),
        };
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(&piece);
    }
    buf
}

// =====================================================================
// Gemini CLI parser (Phase H · ~/.gemini/tmp/<hash>/logs.json)
//
// 数据源极简：JSON array of {sessionId, messageId, type: "user", message, timestamp}
// 注：官方限制 logs.json **只记 user prompts，不记 assistant 回复**。
// hash 目录是 40 char hex(可能是 user_id+project 的派生 key，不是 cwd)
// =====================================================================

fn gemini_tmp_root() -> PathBuf {
    PathBuf::from(env_home()).join(".gemini").join("tmp")
}

#[derive(Debug, Deserialize)]
struct GeminiLogLine {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "messageId", default)]
    message_id: u32,
    #[serde(default, rename = "type")]
    log_type: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    timestamp: String,
}

// An ignored JSON field can bypass the JSON parser's UTF-8 check. Validate the
// source as it is buffered, including ignored fields and split code points.
struct Utf8HistoryReader<R> {
    reader: R,
    tail: [u8; 4],
    tail_len: usize,
}

impl<R: Read> Read for Utf8HistoryReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let invalid = || io::Error::from(io::ErrorKind::InvalidData);
        let size = self.reader.read(buffer)?;
        let mut input = &buffer[..size];
        if input.is_empty() && self.tail_len != 0 {
            return Err(invalid());
        }

        while self.tail_len != 0 && !input.is_empty() {
            self.tail[self.tail_len] = input[0];
            self.tail_len += 1;
            input = &input[1..];
            match std::str::from_utf8(&self.tail[..self.tail_len]) {
                Ok(_) => self.tail_len = 0,
                Err(error) if error.error_len().is_none() => {}
                Err(_) => return Err(invalid()),
            }
        }
        if let Err(error) = std::str::from_utf8(input) {
            if error.error_len().is_some() {
                return Err(invalid());
            }
            let tail = &input[error.valid_up_to()..];
            self.tail[..tail.len()].copy_from_slice(tail);
            self.tail_len = tail.len();
        }
        Ok(size)
    }
}

fn visit_gemini_log(file: &File, record: impl FnMut(GeminiLogLine)) -> Result<(), String> {
    struct LogVisitor<F>(F);

    impl<'de, F: FnMut(GeminiLogLine)> serde::de::Visitor<'de> for LogVisitor<F> {
        type Value = ();

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a Gemini history array")
        }

        fn visit_seq<S>(self, mut sequence: S) -> Result<(), S::Error>
        where
            S: serde::de::SeqAccess<'de>,
        {
            let LogVisitor(mut record) = self;
            while let Some(line) = sequence.next_element()? {
                record(line);
            }
            Ok(())
        }
    }

    let validated = Utf8HistoryReader {
        reader: file,
        tail: [0; 4],
        tail_len: 0,
    };
    let mut parser = serde_json::Deserializer::from_reader(BufReader::new(validated));
    serde::Deserializer::deserialize_seq(&mut parser, LogVisitor(record))
        .and_then(|()| parser.end())
        .map_err(|error| {
            if error.is_io() {
                "QUERY_FAILED: Gemini 历史文件不可读取".into()
            } else {
                "QUERY_INVALID: Gemini 历史格式无效".into()
            }
        })
}

fn gemini_log_paths(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for directory in crate::query::entries(root, true)? {
        let path = directory.path();
        if !path.is_dir() {
            continue;
        }
        let file = path.join("logs.json");
        if file.try_exists().map_err(|_| HISTORY_READ_FAILED)? {
            paths.push(file);
        }
    }
    Ok(paths)
}

fn summarize_gemini_log(path: &Path, file: &File) -> Result<Vec<HistoryEntry>, String> {
    use std::collections::btree_map::Entry;

    let mut buckets = BTreeMap::<String, HistoryEntry>::new();
    visit_gemini_log(file, |line| match buckets.entry(line.session_id) {
        Entry::Vacant(slot) => {
            let session_id = slot.key().clone();
            slot.insert(HistoryEntry {
                agent_id: "gemini-cli".into(),
                session_id,
                cwd: None,
                first_user_prompt: Some(line.message.chars().take(120).collect()),
                started_at: line.timestamp.clone(),
                last_active_at: line.timestamp,
                message_count: 1,
                file_path: path.display().to_string(),
            });
        }
        Entry::Occupied(mut slot) => {
            let summary = slot.get_mut();
            if line.timestamp < summary.started_at {
                summary.started_at = line.timestamp.clone();
            }
            if line.timestamp > summary.last_active_at {
                summary.last_active_at = line.timestamp;
            }
            summary.message_count = summary.message_count.saturating_add(1);
        }
    })?;
    Ok(buckets.into_values().collect())
}

static GEMINI_HISTORY_CACHE: OnceLock<Mutex<SummaryCache>> = OnceLock::new();

fn list_gemini_cli() -> Result<HistoryListResult, String> {
    list_gemini_logs(&gemini_tmp_root())
}

fn list_gemini_logs(root: &Path) -> Result<HistoryListResult, String> {
    use std::collections::btree_map::Entry;

    let data_root = root.display().to_string();
    let paths = gemini_log_paths(root)?;
    let summaries = read_summaries(
        GEMINI_HISTORY_CACHE.get_or_init(Mutex::default),
        &paths,
        summarize_gemini_log,
    )?;
    let mut buckets = BTreeMap::<String, HistoryEntry>::new();
    for summary in summaries {
        match buckets.entry(summary.session_id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(summary);
            }
            Entry::Occupied(mut slot) => {
                let existing = slot.get_mut();
                if summary.started_at < existing.started_at {
                    existing.started_at = summary.started_at;
                }
                if summary.last_active_at > existing.last_active_at {
                    existing.last_active_at = summary.last_active_at;
                }
                existing.message_count = existing.message_count.saturating_add(summary.message_count);
            }
        }
    }
    let mut entries: Vec<_> = buckets.into_values().collect();
    entries.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));

    Ok(HistoryListResult {
        agent_id: "gemini-cli".into(),
        supported: true,
        unsupported_reason: None,
        entries,
        data_root,
    })
}

fn read_gemini_cli(session_id: &str) -> Result<HistorySession, String> {
    read_gemini_logs(&gemini_tmp_root(), session_id)
}

fn read_gemini_logs(root: &Path, session_id: &str) -> Result<HistorySession, String> {
    if !root.exists() {
        return Err(format!(
            "Gemini CLI 历史目录不存在：{}",
            root.display()
        ));
    }

    let mut messages: Vec<HistoryMessage> = Vec::new();
    let mut file_path = String::new();
    let cache = GEMINI_HISTORY_CACHE.get_or_init(Mutex::default);
    for logs_path in gemini_log_paths(root)? {
        let Some(file) = open_matching_history(cache, &logs_path, session_id)? else {
            continue;
        };
        visit_gemini_log(&file, |line| {
            if line.session_id != session_id {
                return;
            }
            if file_path.is_empty() {
                file_path = logs_path.display().to_string();
            }
            let role = if line.log_type.is_empty() {
                "user".to_string()
            } else {
                line.log_type
            };
            messages.push(HistoryMessage {
                uuid: format!("{}:{}", line.session_id, line.message_id),
                parent_uuid: None,
                role,
                content: line.message,
                timestamp: line.timestamp,
                model: None,
                input_tokens: None,
                output_tokens: None,
                cache_read_tokens: None,
                cache_write_tokens: None,
            });
        })?;
    }

    if messages.is_empty() {
        return Err(format!("session {session_id} 无记录"));
    }

    let mut stats = SessionStats {
        message_count: messages.len() as u32,
        user_count: messages.len() as u32,
        ..Default::default()
    };
    let first_ts = chrono::DateTime::parse_from_rfc3339(&messages.first().unwrap().timestamp).ok();
    let last_ts = chrono::DateTime::parse_from_rfc3339(&messages.last().unwrap().timestamp).ok();
    if let (Some(f), Some(l)) = (first_ts, last_ts) {
        stats.duration_seconds = (l - f).num_seconds().max(0) as u64;
    }

    Ok(HistorySession {
        agent_id: "gemini-cli".into(),
        session_id: session_id.into(),
        cwd: None,
        git_branch: None,
        version: None,
        messages,
        stats,
        file_path,
    })
}

// =====================================================================
// Hermes parser (Phase H · ~/.hermes/state.db SQLite + FTS5)
//
// schema 已调研(官方 docs/developer-guide/session-storage.md)：
//   sessions(id, source, user_id, model, title, started_at, last_active_at, ...)
//   messages(session_id, role, content, timestamp, ...)
//   messages_fts FTS5 全文索引
//   messages_fts_trigram FTS5 trigram(支持 CJK 子串搜索)
//
// 当前未集成 rusqlite 依赖，故 Hermes parser 只做"探测式"实现：
//   - state.db 不存在 → 提示用户安装 Hermes Agent
//   - state.db 存在 → 提示当前版本未集成 rusqlite(技术债)
// =====================================================================

fn hermes_state_db() -> PathBuf {
    PathBuf::from(env_home()).join(".hermes").join("state.db")
}

fn list_hermes() -> Result<HistoryListResult, String> {
    let path = hermes_state_db();
    let data_root = path.display().to_string();
    let reason = if !path.exists() {
        "未找到 ~/.hermes/state.db — 请先安装 Hermes Agent (https://github.com/NousResearch/hermes-agent)"
    } else {
        "Hermes state.db 已检测到，但 HomeBase 当前版本未集成 rusqlite(技术债 · Phase H+1 加 SQLite 依赖后接入)"
    };
    Ok(HistoryListResult {
        agent_id: "hermes".into(),
        supported: false,
        unsupported_reason: Some(reason.to_string()),
        entries: vec![],
        data_root,
    })
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_safety_corrupt_history_is_not_an_empty_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.jsonl");
        std::fs::write(&path, "broken synthetic json").unwrap();
        assert!(
            summarize_claude_session(&path, &File::open(&path).unwrap(), "synthetic", None)
                .is_err()
        );
        assert!(summarize_codex_session(&path, &File::open(&path).unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken synthetic json");
        assert!(walk_codex_rollouts(&path, &mut vec![]).is_err());
    }

    #[test]
    fn decode_cwd_slug_handles_known_forms() {
        assert_eq!(
            decode_cwd_slug("C--Users-homebase"),
            Some(r"C:\Users\homebase".to_string())
        );
        assert_eq!(
            decode_cwd_slug("C--WINDOWS-system32"),
            Some(r"C:\WINDOWS\system32".to_string())
        );
        assert_eq!(decode_cwd_slug("xyz"), None);
        assert_eq!(decode_cwd_slug("3--bad"), None);
    }

    #[test]
    fn looks_like_uuid_validates_basic_shapes() {
        assert!(looks_like_uuid("5fa9709f-e65f-489d-afe2-260659c9f25f"));
        assert!(!looks_like_uuid("not-a-uuid"));
        assert!(!looks_like_uuid("5fa9709fe65f489dafe2260659c9f25f")); // 无 dash
        assert!(!looks_like_uuid("")); // 空
    }

    #[test]
    fn codex_paired_user_event_preserves_prompt_looking_human_preview() {
        let root = tempfile::tempdir().unwrap();
        for (index, text) in [
            "# AGENTS.md instructions for synthetic",
            "<INSTRUCTIONS>\nyou are...",
            "<PROJECT_INSTRUCTIONS>",
            "<user_instructions>",
            "如何在 codex 里登录我的账号？",
            "write a function...",
        ].iter().enumerate() {
            let records = [
                serde_json::json!({"type":"session_meta","payload":{"id":"synthetic-session"}}),
                serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}),
                serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":text}}),
            ];
            let raw = records.iter().map(serde_json::Value::to_string).collect::<Vec<_>>().join("\n");
            let path = root.path().join(format!("paired-{index}.jsonl"));
            std::fs::write(&path, &raw).unwrap();
            let summary = summarize_codex_session(&path, &File::open(&path).unwrap()).unwrap();
            assert_eq!(summary.first_user_prompt.as_deref(), text.lines().next());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        }
    }

    #[test]
    fn codex_preview_matches_structured_event_across_input_blocks() {
        // 模拟 codex response_item.payload
        let payload: serde_json::Value = serde_json::json!({
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": "你好"},
                {"type": "input_text", "text": "Codex"},
                {"type": "function_call", "name": "shell"},
            ]
        });
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("blocks.jsonl");
        let records = [
            serde_json::json!({"type":"session_meta","payload":{"id":"synthetic-blocks"}}),
            serde_json::json!({"type":"response_item","payload":payload}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"你好\nCodex"}}),
        ];
        let raw = records.iter().map(serde_json::Value::to_string).collect::<Vec<_>>().join("\n");
        std::fs::write(&path, &raw).unwrap();
        let summary = summarize_codex_session(&path, &File::open(&path).unwrap()).unwrap();
        assert_eq!(summary.first_user_prompt.as_deref(), Some("你好"));
        assert_eq!(summary.message_count, 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    }

    #[test]
    fn gemini_list_and_detail_keep_cross_file_order_and_refresh_after_edit() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        let unused = root.path().join("unused");
        for dir in [&left, &right, &unused] {
            std::fs::create_dir(dir).unwrap();
        }
        let left = left.join("logs.json");
        let right = right.join("logs.json");
        let unused = unused.join("logs.json");
        std::fs::write(root.path().join("ignored-file.json"), "{}").unwrap();
        let logs = |id, message, timestamp, mid| {
            serde_json::json!([{
                "sessionId": id, "message": message, "timestamp": timestamp, "messageId": mid,
            }])
            .to_string()
        };
        let left_record = logs("shared", "第一份", "2026-11-18T10:00:00Z", 1);
        let right_record = logs("shared", "完整第二份", "2026-11-18T09:00:00Z", 2);
        std::fs::write(&left, &left_record).unwrap();
        std::fs::write(&right, &right_record).unwrap();
        std::fs::write(&unused, logs("another", "隔离记录", "2026-11-18T08:00:00Z", 1)).unwrap();

        let first = list_gemini_logs(root.path()).unwrap();
        let again = list_gemini_logs(root.path()).unwrap();
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&again).unwrap()
        );
        let shared = &first.entries[0];
        assert_eq!(shared.session_id, "shared");
        assert_eq!(shared.started_at, "2026-11-18T09:00:00Z");
        assert_eq!(shared.last_active_at, "2026-11-18T10:00:00Z");
        assert_eq!(shared.message_count, 2);
        let detail = read_gemini_logs(root.path(), "shared").unwrap();
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(shared.file_path, detail.file_path);
        assert_eq!(
            shared.first_user_prompt.as_deref(),
            Some(detail.messages[0].content.as_str())
        );

        std::fs::write(&unused, "[ {").unwrap();
        assert!(read_gemini_logs(root.path(), "shared")
            .unwrap_err()
            .starts_with("QUERY_INVALID:"));
        assert!(list_gemini_logs(root.path())
            .unwrap_err()
            .starts_with("QUERY_INVALID:"));
        std::fs::remove_file(&unused).unwrap();
        std::fs::write(&left, logs("shared", "修改后", "2026-11-18T11:00:00Z", 1)).unwrap();
        let updated = list_gemini_logs(root.path()).unwrap();
        assert_eq!(updated.entries.len(), 1);
        assert_eq!(updated.entries[0].last_active_at, "2026-11-18T11:00:00Z");
        let detail = read_gemini_logs(root.path(), "shared").unwrap();
        assert_eq!(
            detail.messages.iter().find(|message| message.uuid == "shared:1").unwrap().content,
            "修改后"
        );
        assert_eq!(std::fs::read_to_string(&right).unwrap(), right_record);
    }

    #[test]
    fn gemini_history_validates_utf8_across_chunks_including_ignored_fields() {
        let mut reader = Utf8HistoryReader {
            reader: "甲😀乙".as_bytes(),
            tail: [0; 4],
            tail_len: 0,
        };
        let mut result = Vec::new();
        let mut byte = [0];
        while reader.read(&mut byte).unwrap() != 0 {
            result.push(byte[0]);
        }
        assert_eq!(result, "甲😀乙".as_bytes());

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs.json");
        let mut bad = br#"[{"sessionId":"id","message":"valid","ignored":""#.to_vec();
        bad.extend_from_slice(&[0xff]);
        bad.extend_from_slice(br#""}]"#);
        std::fs::write(&path, bad).unwrap();
        let file = File::open(&path).unwrap();
        let error = summarize_gemini_log(&path, &file).unwrap_err();
        assert!(error.starts_with("QUERY_FAILED:"));
    }

    #[tokio::test]
    async fn agents_history_list_supported_agents_smoke() {
        // claude-code/codex/gemini-cli 都是 supported(即使 entries 为空也不报错)
        for id in ["claude-code", "codex", "gemini-cli"] {
            let r = agents_history_list(id.to_string()).await.expect("ok");
            assert!(r.supported, "{id} 应为 supported");
        }
    }

    #[tokio::test]
    async fn agents_history_list_openclaw_and_hermes_unsupported() {
        // OpenClaw: IM gateway 不持久化
        // Hermes: 需要 rusqlite 依赖(暂留技术债)
        for id in ["openclaw", "hermes"] {
            let r = agents_history_list(id.to_string()).await.expect("ok");
            assert!(!r.supported, "{id} 应为 unsupported");
            assert!(r.entries.is_empty());
            assert!(r.unsupported_reason.is_some());
        }
    }

    #[tokio::test]
    async fn agents_history_list_unknown_id_errors() {
        let r = agents_history_list("__nope__".into()).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn agents_history_read_rejects_invalid_session_id() {
        let r = agents_history_read("claude-code".into(), "not-a-uuid".into()).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn agents_history_read_unsupported_agents() {
        // OpenClaw 和 Hermes read 操作都应直接返回 Err
        let r = agents_history_read("openclaw".into(), "any".into()).await;
        assert!(r.is_err());
        let r = agents_history_read("hermes".into(), "any".into()).await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn agents_history_all_returns_5_buckets_sorted() {
        // Phase H · 跨 agent 聚合命令必返回 5 个 bucket
        let r = agents_history_all().await.expect("ok");
        assert_eq!(r.buckets.len(), 5, "必须返回 5 个 agent 桶");
        let ids: Vec<&str> = r.buckets.iter().map(|b| b.agent_id.as_str()).collect();
        for must_have in ["claude-code", "codex", "gemini-cli", "openclaw", "hermes"] {
            assert!(ids.contains(&must_have), "缺 {must_have}");
        }
        // 聚合 entries 必须按 last_active_at 倒序
        for w in r.entries.windows(2) {
            assert!(
                w[0].last_active_at >= w[1].last_active_at,
                "entries 未按 last_active_at 倒序"
            );
        }
    }
}
