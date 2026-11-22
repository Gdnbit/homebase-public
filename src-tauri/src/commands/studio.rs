//! Studio 命令（Phase D · 2026-05-06 启动）
//!
//! AI 写作工坊 · 双流：
//! - **新建对话流**：空白开始 → Claude CLI 生成 → 落 `C:\HomeBase\Wiki\_raw\studio-<ts>-<slug>.md`
//! - **改现有 wiki 流**（D4）：从 Knowledge 跳入 → Tiptap 编辑 + 侧栏 AI
//!
//! 文件约定：
//! - 对话：`<vault>\_raw\studio-<ts>-<slug>.chat.jsonl`（JSON Lines · 每行一条 ChatMessage）
//! - 文章：`<vault>\_raw\studio-<ts>-<slug>.md`（可选 · AI 生成后落盘）
//!
//! 密钥：
//! - OpenRouter 凭据仅由后端解析；已有 SOPS 条目优先，缺项可复用原 DPAPI 引用
//! - 凭据仅注入必要子进程内存，不作为前端状态、预览或命令行参数返回
//!
//! 本模块范围：
//! - **D2.1（本次）**：数据结构 + `studio_list_sessions` 扫描
//! - **D2.2+**：`studio_chat_send` 流式 / jsonl 持久化 / 资料源 / 文章落盘 / 多轮 context

use crate::adapters::sops::SecureString;
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::AppHandle;
use crate::task_events::TrackedEmitter;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command as TokioCommand};
use tokio::sync::Mutex;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// =====================================================================
// 常量
// =====================================================================

/// Tauri 事件名 · Studio 模块专用（D2.2 启用）
const EVENT_NAME: &str = "studio:event";

/// 默认 vault 根目录 · 与 gdnbit / wiki 模块一致

/// `_raw/` 子目录名 · USER 的 raw/inbox 原始沉淀区
const RAW_DIR: &str = "_raw";

/// Studio 产物文件名前缀 · 便于从 _raw 一堆文件里筛出 Studio 相关
const SESSION_PREFIX: &str = "studio-";

/// 对话 jsonl 后缀（含文件扩展）
const CHAT_SUFFIX: &str = ".chat.jsonl";

/// 文章 md 后缀
#[allow(dead_code)] // D2.5 启用
const ARTICLE_SUFFIX: &str = ".md";

// =====================================================================
// OpenRouter 配置 · D2.2 修复版
// =====================================================================
//
// 用户 Claude Code v2.x 默认模型 `claude-sonnet-4-6` 在直连 Anthropic 模式下不可用 ·
// 走 OpenRouter · 后端凭据引用解析器保留 SOPS 优先策略，spawn 时仅注入进程环境。
//
// 参考：https://openrouter.ai/docs/guides/coding-agents/claude-code-integration

/// OpenRouter Anthropic-Skin endpoint（注：官方文档无尾 `/v1`）
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";

/// 锁定模型版本 · 避免后续 alias 漂移再炸
///
/// **OPUS fallback 设计**：USER 的 OpenRouter 账号当前没 Opus tier 权限 ·
/// Claude Code 内部某些路径（plan mode / sub-agent）会调 OPUS → 不可用就 fail。
/// 暂把 OPUS 也指向 Sonnet 4.6 · 保证全流程不炸。USER 充值 Opus 后改回。
const OPENROUTER_SONNET_MODEL: &str = "anthropic/claude-sonnet-4.6";
const OPENROUTER_HAIKU_MODEL: &str = "anthropic/claude-haiku-4.5";
const OPENROUTER_OPUS_MODEL: &str = "anthropic/claude-sonnet-4.6"; // fallback to Sonnet

// =====================================================================
// 数据结构（对应前端 TypeScript 类型 · src/lib/tauri-api.ts）
// =====================================================================

/// 会话元信息 · 左栏列表用 · 不含完整消息内容
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioSessionMeta {
    /// 文件名主干，作为唯一 ID · 如 "studio-20260506-0320-react-hooks"
    pub session_id: String,
    /// 标题 · 来源优先级：
    /// 1. 第一条 user 消息的 content 前 30 字（去换行符）
    /// 2. session_id 本身
    pub title: String,
    /// 创建时间（Unix 毫秒）· 来源：文件创建时间 or 第一条消息 ts
    pub created_ms: i64,
    /// 最后活动时间（Unix 毫秒）· 来源：文件最后修改时间
    pub updated_ms: i64,
    /// 消息总数（包括 user / assistant / tool）
    pub message_count: usize,
    /// 对应生成文章的绝对路径（`.md` 存在时填，不存在则 None）
    pub article_path: Option<String>,
    /// `.chat.jsonl` 绝对路径（前端不常用 · 仅 debug / 导出场景）
    pub chat_path: String,
}

/// **D2.7**：assistant 消息的有序子部分 · text + 工具调用穿插
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TurnPart {
    Text {
        text: String,
    },
    Tool {
        tool_use_id: String,
        name: String,
        input_summary: String,
        result_preview: Option<String>,
        is_error: bool,
        finished: bool,
    },
}

/// **D2.7**：任务统计（claude --output-format stream-json 的 result 事件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatStats {
    pub total_cost_usd: f64,
    pub duration_ms: i64,
    pub num_turns: u32,
}

/// 对话消息 · jsonl 每一行反序列化为一条
/// 前端发送 + 后端持久化 · 同一结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// 消息时间戳（Unix 毫秒）
    pub ts: i64,
    /// 角色：user / assistant / tool / system
    pub role: String,
    /// 消息正文（Markdown 允许）· assistant 类型也填合并后的纯文本 · 兼容旧版
    pub content: String,
    /// 模型名（assistant 消息时填 · 如 "claude-sonnet-4.6"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// token 消耗（assistant 消息时可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<usize>,
    /// **D2.7**：assistant 消息的有序子部分（text + 工具调用穿插）·
    /// 存在时优先于 content 渲染 · 缺省时 fallback content
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<TurnPart>>,
    /// **D2.7**：任务统计（assistant 完成时可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<ChatStats>,
    /// **E1**：user 消息发送前生成的 checkpoint id（= task_id）·
    /// 使用 “撚回这条” 按钮时 · unzip `<id>.zip` + truncate jsonl 到 该条 ts 之前
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// tool 调用详情（旧字段 · 兼容性保留 · D3 配图可能用）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
}

// =====================================================================
// 内部工具
// =====================================================================

/// 取 `_raw/` 目录绝对路径
///
/// 策略：
/// 1. 显式测试根优先；正常模式读 `HB_VAULT_PATH`（临时覆盖）
/// 2. 正常模式未覆盖时沿用 `C:\HomeBase\Wiki`
/// 3. 追加 `_raw/`
///
/// **TODO D2.x**：改为读 `~/.homebase/gdnbit.json` 的 `vault_path`（与 gdnbit / wiki 模块共享）
fn raw_dir() -> Result<PathBuf, String> {
    let path = crate::legacy_isolation::studio_root().join(RAW_DIR);
    crate::legacy_isolation::ensure_test_path(&path)?;
    Ok(path)
}

/// 从 `.chat.jsonl` 路径扫出会话元信息
/// 读取策略：
/// - 逐行校验与计数，只保留第一条消息的标题和时间
fn scan_session_file(path: &Path) -> Result<StudioSessionMeta, String> {
    // session_id = 文件名去掉 .chat.jsonl
    let file_name = path.file_name().and_then(|n| n.to_str()).ok_or("QUERY_INVALID: 会话文件名无效")?;
    let session_id = file_name.strip_suffix(CHAT_SUFFIX).ok_or("QUERY_INVALID: 会话文件后缀无效")?.to_string();

    use crate::query::{visit_json_lines, JsonLinesError};
    let file = std::fs::File::open(path).map_err(|_| "QUERY_FAILED: Studio 会话文件不可读取")?;
    let mut title = session_id.clone();
    let mut created_ms = 0;
    let mut message_count = 0;
    visit_json_lines(std::io::BufReader::new(file), |message: ChatMessage| {
        if message_count == 0 {
            let first_line: String = message.content.lines().next().unwrap_or("").chars().take(30).collect();
            if !first_line.trim().is_empty() {
                title = first_line;
            }
            created_ms = message.ts;
        }
        message_count += 1;
    }).map_err(|error| match error {
        JsonLinesError::Read(_) => "QUERY_FAILED: Studio 会话文件不可读取",
        JsonLinesError::Invalid => "QUERY_INVALID: Studio 会话记录格式无效或尚未写完，请重试",
    })?;

    // updated_ms 从文件 metadata 拿（比最后一条消息 ts 更准 · 处理重命名/复制场景）
    let updated_ms = path
        .metadata()
        .map_err(|_| "QUERY_FAILED: Studio 会话元数据不可读取")?
        .modified().map_err(|_| "QUERY_FAILED: Studio 会话修改时间不可读取")?
        .duration_since(std::time::UNIX_EPOCH).map_err(|_| "QUERY_INVALID: Studio 会话修改时间无效")?
        .as_millis() as i64;

    // 检查同目录下对应 .md 文章是否存在
    let article_path = path.with_file_name(format!("{}.md", session_id));
    let article_path = if article_path.exists() {
        Some(article_path.to_string_lossy().to_string())
    } else {
        None
    };

    Ok(StudioSessionMeta {
        session_id,
        title,
        created_ms,
        updated_ms,
        message_count,
        article_path,
        chat_path: path.to_string_lossy().to_string(),
    })
}

// =====================================================================
// D2.7 · jsonl 持久化 helpers
// =====================================================================

/// 取 session 对应的 chat.jsonl 绝对路径
fn storage_key(value: &str) -> Result<(), String> {
    let stem = value.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$")
        || ["COM", "LPT"].iter().any(|prefix| stem.strip_prefix(*prefix)
            .is_some_and(|suffix| suffix.chars().count() == 1 && suffix.chars().all(char::is_numeric)));
    if value.is_empty() || matches!(value, "." | "..") || value.ends_with(['.', ' '])
        || device || value.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Err("QUERY_INVALID: 会话或快照编号必须是单一文件名，未读取或写入其他目录".into());
    }
    Ok(())
}

fn chat_jsonl_path(session_id: &str) -> Result<PathBuf, String> {
    storage_key(session_id)?;
    let path = raw_dir()?.join(format!("{}{}", session_id, CHAT_SUFFIX));
    crate::legacy_isolation::ensure_test_path(&path)?;
    Ok(path)
}

/// Append 一条 ChatMessage 到 session 的 chat.jsonl
///
/// - 不存在则创建（含父目录）· 已存在则 append
/// - 单行 JSON · trailing `\n`
/// - OpenOptions::append 在 Windows 上 atomic 单行 · 不需手工锁
async fn append_message_jsonl(
    session_id: &str,
    message: &ChatMessage,
) -> Result<(), String> {
    use tokio::fs::OpenOptions;
    use tokio::io::AsyncWriteExt;

    let path = chat_jsonl_path(session_id)?;
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("创建 _raw 目录失败: {}", e))?;
        }
    }

    let mut line = serde_json::to_string(message)
        .map_err(|e| format!("序列化 ChatMessage 失败: {}", e))?;
    line.push('\n');

    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| format!("打开 jsonl 失败: {} ({})", path.display(), e))?;
    f.write_all(line.as_bytes())
        .await
        .map_err(|e| format!("写 jsonl 失败: {}", e))?;
    f.flush().await.ok();
    Ok(())
}

/// 读 session 的 chat.jsonl · 返回所有 ChatMessage（按文件中顺序）
///
/// - 文件不存在 → 返回空 Vec（不是错误 · 视为新会话）
/// - parse 失败的行 → log warn 跳过 · 不中断
async fn read_messages_jsonl(session_id: &str) -> Result<Vec<ChatMessage>, String> {
    let path = chat_jsonl_path(session_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("读 jsonl 失败: {} ({})", path.display(), e))?;

    let mut messages = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<ChatMessage>(trimmed) {
            Ok(msg) => messages.push(msg),
            Err(e) => {
                tracing::warn!(
                    "jsonl line {} parse 失败 · 跳过: {} (line head: {})",
                    i + 1,
                    e,
                    &trimmed.chars().take(80).collect::<String>()
                );
            }
        }
    }
    Ok(messages)
}

// =====================================================================
// E2 · MCP image generation helpers
// =====================================================================

/// MCP server `homebase-mcp-image` 可执行路径
///
/// 策略：跟主进程 exe 同目录找 `homebase-mcp-image.exe`
/// - dev 模式：`<repo>/src-tauri/target/debug/homebase-mcp-image.exe`
/// - release 模式：`<install-dir>/homebase-mcp-image.exe`
fn mcp_image_exe_path() -> Option<PathBuf> {
    let main_exe = std::env::current_exe().ok()?;
    let parent = main_exe.parent()?;
    let candidate = parent.join(if cfg!(target_os = "windows") {
        "homebase-mcp-image.exe"
    } else {
        "homebase-mcp-image"
    });
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

/// 写 Studio 的 MCP 配置文件 · 路径 `~/.homebase/studio.mcp.json`
///
/// - 不含密钥 · env 用 `${OPENROUTER_API_KEY}` 占位符（Claude Code 自动从父进程 env 替换）
/// - 已存在则覆盖（mcp-image exe 路径可能因升级/重装变化）
/// - 失败 → 返回 Err · 上层决定是否阻塞 chat
fn ensure_studio_mcp_config() -> Result<PathBuf, String> {
    let home =
        std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME"))
            .map_err(|_| "无法定位 user home（USERPROFILE / HOME 都没设）".to_string())?;
    let cfg_dir = PathBuf::from(home).join(".homebase");
    std::fs::create_dir_all(&cfg_dir).map_err(|e| format!("创建 ~/.homebase 失败: {}", e))?;
    let cfg_path = cfg_dir.join("studio.mcp.json");

    let exe = mcp_image_exe_path().ok_or_else(|| {
        "homebase-mcp-image 可执行文件不存在（同 homebase 主进程目录找不到）".to_string()
    })?;

    // Claude Code 支持 ${VAR} 语法从父进程 env 替换 · 不在配置文件落盘 secret
    let cfg = serde_json::json!({
        "mcpServers": {
            "homebase-image": {
                "type": "stdio",
                "command": exe.to_string_lossy(),
                "args": [],
                "env": {
                    "OPENROUTER_API_KEY": "${OPENROUTER_API_KEY}",
                    "HB_IMAGE_MODEL": "${HB_IMAGE_MODEL}",
                    "HB_IMAGE_OUTPUT_DIR": "${HB_IMAGE_OUTPUT_DIR}"
                }
            }
        }
    });
    let json_str =
        serde_json::to_string_pretty(&cfg).map_err(|e| format!("序列化 mcp 配置失败: {}", e))?;
    std::fs::write(&cfg_path, json_str).map_err(|e| format!("写 mcp 配置失败: {}", e))?;
    Ok(cfg_path)
}

// =====================================================================
// E1 · Checkpoint 持久化 helpers（zip 快照 _raw · 一键撤回 AI 改动）
// =====================================================================

const CHECKPOINTS_DIR: &str = ".studio-checkpoints";
const ATTACHMENTS_DIR: &str = "attachments";
const MAX_CHECKPOINTS: usize = 50;

/// checkpoints 目录绝对路径
fn checkpoints_dir() -> Result<PathBuf, String> {
    let path = raw_dir()?.join(CHECKPOINTS_DIR);
    crate::legacy_isolation::ensure_test_path(&path)?;
    Ok(path)
}

/// **E1**：创建 checkpoint · zip _raw 内容（排除 .studio-checkpoints + attachments）
///
/// - 阻塞 IO 走 spawn_blocking · 不卡 async runtime
/// - _raw 不存在 → 返回 Err（应当先有 _raw 目录才有 chat）
/// - zip 体积通常 < 5MB · 几百 ms 完成
async fn create_checkpoint(task_id: &str) -> Result<PathBuf, String> {
    storage_key(task_id)?;
    let raw = raw_dir()?;
    if !raw.exists() {
        // _raw 还没建 · 跳过 checkpoint（首轮新会话 · 没东西可备份）
        return Err(format!("_raw 不存在 · 跳过快照: {}", raw.display()));
    }

    let cp_dir = checkpoints_dir()?;
    tokio::fs::create_dir_all(&cp_dir)
        .await
        .map_err(|e| format!("创建 checkpoints 目录失败: {}", e))?;

    let zip_path = cp_dir.join(format!("{}.zip", task_id));

    let raw_clone = raw.clone();
    let zip_path_clone = zip_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        use std::io::Write;
        let f = std::fs::File::create(&zip_path_clone)
            .map_err(|e| format!("创建 zip 失败: {}", e))?;
        let mut writer = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        for entry in walkdir::WalkDir::new(&raw_clone)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let rel = match path.strip_prefix(&raw_clone) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let rel_str = rel.to_string_lossy();
            // 排除自身 + 大附件目录
            if rel_str.starts_with(CHECKPOINTS_DIR) || rel_str.starts_with(ATTACHMENTS_DIR) {
                continue;
            }
            if path.is_file() {
                let rel_path_str: String = rel.to_string_lossy().replace('\\', "/");
                writer
                    .start_file::<_, ()>(rel_path_str, opts)
                    .map_err(|e| format!("zip start_file 失败: {}", e))?;
                let bytes = std::fs::read(path)
                    .map_err(|e| format!("读文件失败 {}: {}", path.display(), e))?;
                writer
                    .write_all(&bytes)
                    .map_err(|e| format!("zip write 失败: {}", e))?;
            }
        }
        writer
            .finish()
            .map_err(|e| format!("zip finish 失败: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("checkpoint zip task panic: {}", e))??;

    tracing::debug!("checkpoint 创建: {}", zip_path.display());
    Ok(zip_path)
}

/// **E1**：还原 checkpoint · unzip 覆盖 _raw（保留 attachments / .studio-checkpoints 不动）
async fn restore_checkpoint(task_id: &str) -> Result<(), String> {
    storage_key(task_id)?;
    let raw = raw_dir()?;
    let cp_path = checkpoints_dir()?.join(format!("{}.zip", task_id));
    crate::legacy_isolation::ensure_test_path(&cp_path)?;
    if !cp_path.exists() {
        return Err(format!("checkpoint 不存在: {}", cp_path.display()));
    }

    let raw_clone = raw.clone();
    let cp_clone = cp_path.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let f = std::fs::File::open(&cp_clone)
            .map_err(|e| format!("打开 zip 失败: {}", e))?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("zip 解析失败: {}", e))?;
        for i in 0..archive.len() {
            let mut zf = archive
                .by_index(i)
                .map_err(|e| format!("zip 读 entry 失败: {}", e))?;
            let out_path = match zf.enclosed_name() {
                Some(p) => raw_clone.join(p),
                None => continue,
            };
            if zf.is_dir() {
                std::fs::create_dir_all(&out_path)
                    .map_err(|e| format!("mkdir 失败 {}: {}", out_path.display(), e))?;
            } else {
                if let Some(parent) = out_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("mkdir parent 失败: {}", e))?;
                }
                let mut out_f = std::fs::File::create(&out_path)
                    .map_err(|e| format!("写文件失败 {}: {}", out_path.display(), e))?;
                std::io::copy(&mut zf, &mut out_f)
                    .map_err(|e| format!("zip copy 失败: {}", e))?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("restore task panic: {}", e))??;

    tracing::debug!("checkpoint 还原完成: {}", cp_path.display());
    Ok(())
}

/// **E1**：清理老 checkpoints · 保留最近 max_keep 个（按 mtime 升序删除老的）
async fn cleanup_old_checkpoints(max_keep: usize) {
    let cp_dir = match checkpoints_dir() {
        Ok(path) => path,
        Err(error) => { tracing::warn!("未清理无法核对的快照路径: {error}"); return; }
    };
    if !cp_dir.exists() {
        return;
    }
    let res = tokio::task::spawn_blocking(move || {
        let rd = match std::fs::read_dir(&cp_dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };
        let mut entries: Vec<(PathBuf, std::time::SystemTime)> = rd
            .filter_map(|e| {
                let e = e.ok()?;
                let path = e.path();
                if path.extension()? != "zip" {
                    return None;
                }
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((path, mtime))
            })
            .collect();
        if entries.len() <= max_keep {
            return;
        }
        entries.sort_by_key(|(_, t)| *t);
        let to_delete = entries.len() - max_keep;
        for (path, _) in entries.iter().take(to_delete) {
            if let Err(e) = std::fs::remove_file(path) {
                tracing::warn!("清理 checkpoint 失败: {} ({})", path.display(), e);
            }
        }
    })
    .await;
    if let Err(e) = res {
        tracing::warn!("cleanup checkpoints task 失败: {}", e);
    }
}

/// **E1**：truncate session jsonl · 删掉 ts >= cutoff_ts 的所有 messages
async fn truncate_jsonl_at_or_after(session_id: &str, cutoff_ts: i64) -> Result<(), String> {
    let path = chat_jsonl_path(session_id)?;
    if !path.exists() {
        return Ok(());
    }
    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("读 jsonl 失败: {}", e))?;
    let mut kept = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<ChatMessage>(trimmed) {
            Ok(m) if m.ts < cutoff_ts => kept.push(trimmed.to_string()),
            _ => {} // ts >= cutoff_ts 或 parse 失败 · 都不留
        }
    }
    let mut new_content = kept.join("\n");
    if !new_content.is_empty() {
        new_content.push('\n');
    }
    tokio::fs::write(&path, new_content)
        .await
        .map_err(|e| format!("重写 jsonl 失败: {}", e))?;
    Ok(())
}

// =====================================================================
// Tauri 命令
// =====================================================================

/// 列出所有 Studio 会话 · 按最后活动时间降序
///
/// 扫描路径：`<vault>/_raw/studio-*.chat.jsonl`
/// - 不存在 → 返回空 Vec（不是错误）
/// - 解析失败的文件 → 明确报错，不返回不完整的列表
/// - 按 `updated_ms` 降序（最新在前）
#[tauri::command]
pub async fn studio_list_sessions() -> Result<Vec<StudioSessionMeta>, String> {
    tokio::task::spawn_blocking(list_studio_sessions)
        .await
        .map_err(|_| "QUERY_FAILED: Studio 会话列表读取任务未完成")?
}

fn list_studio_sessions() -> Result<Vec<StudioSessionMeta>, String> {
    tracing::debug!("studio_list_sessions 调用");

    let dir = raw_dir()?;
    if !dir.exists() {
        tracing::debug!("_raw 目录不存在（正常 · USER 还没建过会话）: {}", dir.display());
        return Ok(Vec::new());
    }

    let entries = crate::query::entries(&dir, true)?;
    for entry in &entries { crate::legacy_isolation::ensure_test_path(&entry.path())?; }

    let mut sessions: Vec<StudioSessionMeta> = entries
        .into_iter()
        .map(|entry| entry.path())
        .filter(|p| {
            // 只要 studio-*.chat.jsonl
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(SESSION_PREFIX) && n.ends_with(CHAT_SUFFIX))
                    .unwrap_or(false)
        })
        .map(|p| scan_session_file(&p))
        .collect::<Result<Vec<_>, _>>()?;

    sessions.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));

    tracing::debug!("studio_list_sessions 返回 {} 条会话", sessions.len());
    Ok(sessions)
}

// =====================================================================
// D2.2 · 流式对话基础设施
// =====================================================================

/// Studio 流式事件 · 与前端 `studio:event` listener 对齐
///
/// **D2.7 工具调用可视化**：解析 `claude --output-format stream-json` 的 NDJSON ·
/// emit 4 个结构化变体（AssistantText / ToolUse / ToolResult / ResultStats）·
/// 前端按 kind 分发渲染。Stream 变体保留作 fallback（parse 失败时的 raw 行）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StudioEvent {
    /// spawn 成功 · prompt 已发送给 Claude
    Started {
        task_id: String,
        session_id: String,
        cmd_preview: String,
    },
    /// 一段完整 assistant 文本（来自 stream-json 的 message.content[*].text）
    AssistantText { task_id: String, text: String },
    /// 工具调用开始（Read / Edit / Write 等）
    ToolUse {
        task_id: String,
        tool_use_id: String,
        name: String,
        /// 人类可读的输入摘要 · 如 file_path / pattern / query
        input_summary: String,
    },
    /// 工具调用结果（前 300 字预览 · 完整内容前端不展示避免冗长）
    ToolResult {
        task_id: String,
        tool_use_id: String,
        content_preview: String,
        is_error: bool,
    },
    /// 任务统计（claude 自报）· 在 result type 事件里
    ResultStats {
        task_id: String,
        total_cost_usd: f64,
        duration_ms: i64,
        num_turns: u32,
    },
    /// stdout 一行 · parse JSON 失败时的 raw 行（debug / fallback）
    Stream { task_id: String, line: String },
    /// stderr 一行 · 通常是 Claude 调试或错误
    Stderr { task_id: String, line: String },
    /// 进程正常退出 · exit_code = 0
    Completed {
        task_id: String,
        session_id: String,
        exit_code: i32,
    },
    /// 进程非零退出
    Failed {
        task_id: String,
        exit_code: i32,
        message: String,
    },
    /// USER 主动 cancel
    Cancelled { task_id: String },
}

/// 单个流式任务（cancel 时需要 kill 这个 child）
struct StudioTask {
    child: Child,
}

type StudioTaskMap = Arc<Mutex<HashMap<String, StudioTask>>>;

/// 全局任务表 · 模块内 OnceLock 单例
fn task_map() -> &'static StudioTaskMap {
    use std::sync::OnceLock;
    static MAP: OnceLock<StudioTaskMap> = OnceLock::new();
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

/// chat_send 入参 · 前端构造
///
/// **D2.2 设计决策（2026-05-06）**：
/// HomeBase Studio 永远走 OpenRouter · 不切换 Anthropic 直连。理由：
/// - Anthropic 对非纯净网络（中国大陆 / VPN）封号风险高 · 不值
/// - OpenRouter 中转 IP 是 OR 服务器 · Anthropic 看不到 USER 的 IP
/// - SOPS openrouter_key 已配 · 余额按量付费 · 单次对话成本 < 1 美分
/// - 简化架构 · 不需要 provider switcher
#[derive(Debug, Clone, Deserialize)]
pub struct StudioChatSendPayload {
    /// 会话 ID · 不填表示新建（后端生成 timestamp-based id）
    #[serde(default)]
    pub session_id: Option<String>,
    /// USER 输入的本轮消息（必填）
    pub user_message: String,
    /// **D2.6 多轮**：历史 messages（不含本轮 user_message）·
    /// 后端拼接到 prompt 头 · 让 claude 获得上下文。None / 空数组 → 当作首轮。
    #[serde(default)]
    pub history: Option<Vec<ChatMessage>>,
    /// D2.4 启用：资料源文件绝对路径列表 · D2.2 留 None
    #[serde(default)]
    pub source_paths: Option<Vec<String>>,
}

/// chat_send 返回 · 前端用 task_id 订阅 + 用 session_id 关联会话
#[derive(Debug, Clone, Serialize)]
pub struct StudioChatSendResult {
    pub task_id: String,
    pub session_id: String,
    pub cmd_preview: String,
}

// =====================================================================
// 工具
// =====================================================================

/// 生成新 session_id · 格式 `studio-YYYYMMDD-HHMM-<rand>`
/// `<rand>` = 6 位 hex 摘要 · 防同分钟内多会话冲突
fn generate_session_id() -> String {
    use chrono::Local;
    let now = Local::now();
    let ts = now.format("%Y%m%d-%H%M");
    // 简单随机：UUID v4 取前 6 位 hex
    let rand = uuid::Uuid::new_v4().to_string();
    let rand_short: String = rand.chars().filter(|c| c.is_ascii_alphanumeric()).take(6).collect();
    format!("{}{}-{}", SESSION_PREFIX, ts, rand_short)
}

/// 当前时间戳 Unix 毫秒
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 构造 Claude CLI Command · oneshot 模式 + 流式 stdout
///
/// **D2.2 双层 prompt 设计**（USER 选项 C）：
/// - 顶部硬编码：站点定位 / Claude 职责 / 产物落地 / 风格调性（稳定信息 · 不每次读文件）
/// - 中部 agentic：指引 Claude 用 Read 工具读 README 获取最新详细信息（永远跟实际同步）
/// - 底部工具边界 + USER 消息
///
/// **--allowedTools Read,Glob,Grep,Edit,Write**：Studio 工坊范围比 ai_optimize 广 · 允许新建文件
/// **--permission-mode acceptEdits**：oneshot 模式下避免交互卡住
///
/// **D2.4 升级**：append 资料源路径列表
/// **D2.6 升级**：拼接历史消息（多轮 context）
///
/// **provider**：永远 OpenRouter · 注入 4 个核心 env + 3 个模型别名
/// （见 StudioChatSendPayload 顶注的设计决策）
///
/// **不变量**：caller `studio_chat_send` 保证 `openrouter_key` 已成功解密 ·
/// 解密失败的请求在 caller 直接 return Err · 不会进入本函数。
fn build_claude_command(
    user_message: &str,
    history: &[ChatMessage],
    openrouter_key: &SecureString,
) -> Result<(TokioCommand, String), String> {
    // **S1 · 共享上下文池注入**（每次 build 时动态 · 含当前时间 + 精准路径 + USER 调性）
    // 单一数据源：~/.homebase/shared/context.toml （adapters::shared_context::render_header）
    // 让所有 agent prompt（studio / gdnbit / wiki_agent / 未来）共用同一上下文头 · 消除重复
    let context_header = crate::adapters::shared_context::render_header();

    // System prompt · 网站协作 agent 定位
    // 设计原则：
    // - 顶部由 HomeBase 动态注入时间 + 路径（见 context_header）
    // - 中部：任务类型矩阵 · 让 claude 知道能做啥（不只是写文章）
    // - agentic 引导：让 claude 自己用 Read 工具读细节（按需深入）
    // - 工具边界 + USER 风格偏好
    let system_prompt = r#"# 你的角色

你是 **GDNbit 网站协作 Agent** · USER 的网站维护与内容创作搭档。
不只是被动等 USER 指令的对话 bot · 你主动用工具读项目当前状态 · 给可执行建议。

# 你能做的任务（不只是写文章）

1. **写新文章** · 按 USER 题目产出草稿 → 落 `C:\HomeBase\Wiki\_raw\studio-<ts>-<slug>.md`
2. **改既有文章** · 修订 `content/blog/` 或 `content/projects/` 下的 MDX
   - 改之前先 Read 看现状 · 别盲改
   - 涉及发布文章 · 必须先用 1-2 句告诉 USER 改什么 · USER 不反对再 Edit
3. **选题与策略** · 看 `content/blog/` 历史选题 · 给新选题建议 · 关键词 · 内链建议
4. **知识库整合** · 把 `C:\HomeBase\Wiki/<某主题>/` 下零散笔记汇成一篇正式文章草稿
5. **工程协助** · 读 已配置的站点 源码 · 解 Next.js / 构建 / MDX 问题
   - 不主动跑 shell（Bash 未启用）· 用 Read/Glob/Grep 排查 · 给修改建议
6. **README / 文档维护** · 根据当前代码状态更新 README

# 行为准则

- **agentic** · 不要等 USER 喂全部信息 · 主动 Read 项目文件确认现状
- **简述意图** · 大改 / 多文件操作前 · 先 1-2 句告诉 USER 你要做什么
- **直接给答案** · 不重复 USER 的问题 · 不绕弯子
- **工程性建议** · 涉及文章质量 / 选题 / SEO · 给具体可执行建议 · 不空泛
- **不主动改已发布** · 只读 `content/` 是默认安全 · 改要 USER 明确说"改这篇"
- **代码可运行** · 不写伪代码 · 写完整片段

# USER 风格偏好（严格遵守）

- **中文交流**（注释/变量名英文 · 文章正文中文）
- **简短精简** · 禁排比 · 禁诗化抒情 · 禁"一个人，两种语言"这种文青腔
- **冷静符号化** · 中英混排 / 字重对撞 / Off-White / minimal.gallery / magazine 式 typography
- **不用营销腔** · 不写"赋能"、"打造"、"践行"这类词
- **不用 emoji**（除非原文已有 / 文章主题相关）
- **Karpathy LLM Wiki 风格**：清晰 / 工程化 / 直接给答案

# 按需深入（agentic 引导）

USER 提到具体话题时 · 你应该主动用 Read 工具读：
- `C:\HomeBase\Integrations\ExampleSite\README.md`（网站架构 / 文档导航）
- `C:\HomeBase\Wiki\README.md`（笔记调性 · Karpathy 4 阶段范式）
- `C:\HomeBase\Wiki\_schema\STYLE_GUIDE.md`（如存在 · 风格规范细则）
- USER 提到的任何具体路径

# 工具与边界

- 可用：**Read / Glob / Grep / Edit / Write** + **mcp__homebase-image__generate_image**（生图）
- 不可用：Bash（未启用）· WebFetch / WebSearch（未启用）
- workdir = `C:\HomeBase\Wiki` · 写相对路径默认在 vault · 写网站绝对路径用 `C:\HomeBase\Integrations\ExampleSite\...`
- 不修改 USER 的已发布文章 · 除非 USER 明确指示

# 配图能力（MCP 工具 · 多模型路由）

工具名：`mcp__homebase-image__generate_image`
- **何时主动调**：写新文章需要 hero image 时 · 长文某些章节需插图说明时 · USER 说“加图 / 配图”时
- **何时不调**：只改 frontmatter / 只改格式 / 短回复 / USER 明确说不要图 / 已发布文章修订（除非 USER 说改图）

## 模型路由（必读 · 按 prompt 内容选 model 参数）

| 场景特征 | model 参数（OR slug） | 说明 |
|---|---|---|
| **prompt 含中文文字 / 海报 / 信息图 / 图表** | `openai/gpt-5.4-image-2` | GPT Image 2 · 文字渲染 ≈100% 准确 · 中英文都强 |
| **prompt 含人物 / 人像 / 摄影 / 真实场景 / 多人合影** | `google/gemini-3-pro-image-preview` | Nano Banana Pro · 皮肤毛孔 / 多人一致性 / 写实之王 |
| **prompt 含插画 / 艺术 / 抽象 / 风格化 / 油画 / 水彩** | `black-forest-labs/flux.2-pro` | FLUX.2 Pro · prompt 跟随强 · 风格稳定 |
| **其他**（一般 hero image / 概念图 / 默认） | `google/gemini-3.1-flash-image-preview` | Nano Banana 2 · 性价比首选 · Pro 级质量 + Flash 速度 |

**规则**：
- 路由是**强制**的 · 不要懒省事固定一个模型
- 选完模型后 · 在 input_summary（自然描述里）**显式说明为什么选这个模型**（一句话）· 如 "用 GPT Image 2 因为含中文标题"
- 只能选这 4 个 slug 之一 · 不要瞎填别的

## 通用规则

- **prompt 要求**：中文或英文 · 至少 30 字 · 包含：风格 / 主体 / 色调 / 构图 / 光线 · USER 调性偏好 Off-White / minimal.gallery 风格 · 中性冷静
- **filename_hint**：设有意义的英文短语 · 例 `karpathy-llm-arch` `transformer-attention-flow`
- 调用后工具返回 markdown 语法 · 直接 Edit 插入文章

# 输出格式

- Markdown 中文 · 长回答合理分段（## 二级标题）
- 列表用 `-` · 强调用 `**`
- 涉及文件 / 路径 · 用 `code 体`"#;

    // 拼接历史（如有）· 格式：### USER / ### ASSISTANT 交替
    let history_block = if history.is_empty() {
        String::new()
    } else {
        let mut buf = String::from("\n\n---\n\n# 历史对话\n\n");
        for msg in history {
            let role_label = match msg.role.as_str() {
                "user" => "### USER",
                "assistant" => "### ASSISTANT",
                _ => continue, // tool / system 不拼进历史 · 避免混乱语义
            };
            buf.push_str(role_label);
            buf.push_str("\n\n");
            buf.push_str(&msg.content);
            buf.push_str("\n\n");
        }
        buf
    };

    let full_prompt = format!(
        "{}{}{}\n\n---\n\n# 本轮 USER 消息\n\n{}",
        context_header, system_prompt, history_block, user_message
    );

    // **E2**：尝试准备 MCP 生图配置 · 失败仅 warn · chat 仍可跑（只是没生图能力）
    let mcp_cfg_path = match ensure_studio_mcp_config() {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("ensure_studio_mcp_config 失败 · 生图不可用: {}", e);
            None
        }
    };
    let has_image_tool = mcp_cfg_path.is_some();

    let mut cmd = TokioCommand::new("claude");
    cmd.arg("-p")
        .arg(&full_prompt)
        .arg("--permission-mode")
        .arg("acceptEdits")
        .arg("--allowedTools")
        .arg(if has_image_tool {
            "Read,Glob,Grep,Edit,Write,mcp__homebase-image__generate_image"
        } else {
            "Read,Glob,Grep,Edit,Write"
        })
        // **D2.7 工具调用可视化**：stream-json 输出结构化 NDJSON
        // 每行一个 JSON 事件 · 区分 text / tool_use / tool_result / result
        // 必须配合 --verbose（stream-json 在 -p 模式要求）
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    if let Some(p) = &mcp_cfg_path {
        cmd.arg("--mcp-config").arg(p);
    }
    cmd.kill_on_drop(true);

    // OpenRouter env 注入 · 4 核心 + 3 模型别名（永远走 OR · 见 StudioChatSendPayload 注释）
    let key_str = openrouter_key.as_str();
    cmd.env("ANTHROPIC_BASE_URL", OPENROUTER_BASE_URL);
    cmd.env("ANTHROPIC_AUTH_TOKEN", key_str);
    cmd.env("OPENROUTER_API_KEY", key_str);
    cmd.env("ANTHROPIC_API_KEY", ""); // 显式置空 · 防 Anthropic 凭据冲突
    cmd.env("ANTHROPIC_DEFAULT_SONNET_MODEL", OPENROUTER_SONNET_MODEL);
    cmd.env("ANTHROPIC_DEFAULT_HAIKU_MODEL", OPENROUTER_HAIKU_MODEL);
    cmd.env("ANTHROPIC_DEFAULT_OPUS_MODEL", OPENROUTER_OPUS_MODEL);

    // **E2**：mcp-image binary 从这些 env 取配置（通过 .mcp.json 的 ${VAR} 占位符转发）
    if has_image_tool {
        // 生图落到 _raw/attachments/ · 与 _raw 同父文件夹 · 便于 Inbox 后续处理
        let attachments = crate::legacy_isolation::wiki_root().join("_raw").join("attachments");
        crate::legacy_isolation::ensure_test_path(&attachments)?;
        cmd.env("HB_IMAGE_MODEL", "openai/gpt-5.4-image-2");
        cmd.env("HB_IMAGE_OUTPUT_DIR", attachments.to_string_lossy().to_string());
    }

    // 工作目录设为 vault 根 · USER 提相对路径时 Claude 容易找
    let work_dir = crate::legacy_isolation::wiki_root();
    crate::legacy_isolation::ensure_test_path(&work_dir)?;
    if work_dir.exists() {
        cmd.current_dir(&work_dir);
    }

    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW

    let preview = format!(
        "claude -p <{} chars> --permission-mode acceptEdits {} [provider=openrouter]",
        full_prompt.chars().count(),
        if has_image_tool { "+mcp(image)" } else { "" }
    );

    Ok((cmd, preview))
}

/// 从 SOPS 解密 OpenRouter API key
///
/// **OpenRouter-only 设计**：解密失败直接 return Err · 不 fallback ·
/// 让 USER 看到清晰错误信息（避免 silent degrade 到不可用的 Anthropic 直连）。
async fn fetch_openrouter_key() -> Result<SecureString, String> {
    crate::adapters::service_credentials::resolve_api_key("openrouter").await
}

/// 解析 stream-json 单行 JSON · 按 type 分发 emit 不同结构化事件
///
/// Claude Code stream-json 输出 schema（NDJSON · 每行一个事件）：
/// - `{"type": "system", "subtype": "init", ...}` → 忽略（init 元数据）
/// - `{"type": "assistant", "message": {"content": [{"type":"text"|"tool_use", ...}]}}` → text / tool_use
/// - `{"type": "user", "message": {"content": [{"type": "tool_result", ...}]}}` → tool_result
/// - `{"type": "result", "total_cost_usd": ..., "duration_ms": ..., "num_turns": ...}` → 统计
fn emit_stream_json_events(app: &AppHandle, task_id: &str, v: &serde_json::Value) {
    let evt_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    match evt_type {
        "assistant" => {
            // message.content 数组 · 可能含 text 块或 tool_use 块
            if let Some(content) = v.pointer("/message/content").and_then(|x| x.as_array()) {
                for block in content {
                    let block_type = block.get("type").and_then(|x| x.as_str()).unwrap_or("");
                    match block_type {
                        "text" => {
                            let text =
                                block.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if !text.is_empty() {
                                let _ = app.emit_tracked(
                                    EVENT_NAME,
                                    StudioEvent::AssistantText {
                                        task_id: task_id.to_string(),
                                        text: text.to_string(),
                                    },
                                );
                            }
                        }
                        "tool_use" => {
                            let tool_use_id = block
                                .get("id")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = block
                                .get("name")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            let input =
                                block.get("input").cloned().unwrap_or(serde_json::Value::Null);
                            let summary = summarize_tool_input(&name, &input);
                            let _ = app.emit_tracked(
                                EVENT_NAME,
                                StudioEvent::ToolUse {
                                    task_id: task_id.to_string(),
                                    tool_use_id,
                                    name,
                                    input_summary: summary,
                                },
                            );
                        }
                        _ => {} // thinking / 未知 block · 暂忽略
                    }
                }
            }
        }
        "user" => {
            // tool_result 在 user message 里（Claude 的 SDK 结构）
            if let Some(content) = v.pointer("/message/content").and_then(|x| x.as_array()) {
                for block in content {
                    if block.get("type").and_then(|x| x.as_str()) == Some("tool_result") {
                        let tool_use_id = block
                            .get("tool_use_id")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string();
                        let content_str = block
                            .get("content")
                            .map(|c| match c {
                                serde_json::Value::String(s) => s.clone(),
                                serde_json::Value::Array(arr) => arr
                                    .iter()
                                    .filter_map(|item| {
                                        item.get("text").and_then(|t| t.as_str())
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => c.to_string(),
                            })
                            .unwrap_or_default();
                        let is_error = block
                            .get("is_error")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false);
                        let preview = truncate_preview(&content_str, 300);
                        let _ = app.emit_tracked(
                            EVENT_NAME,
                            StudioEvent::ToolResult {
                                task_id: task_id.to_string(),
                                tool_use_id,
                                content_preview: preview,
                                is_error,
                            },
                        );
                    }
                }
            }
        }
        "result" => {
            let total_cost = v.get("total_cost_usd").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let duration_ms = v.get("duration_ms").and_then(|x| x.as_i64()).unwrap_or(0);
            let num_turns = v.get("num_turns").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
            let _ = app.emit_tracked(
                EVENT_NAME,
                StudioEvent::ResultStats {
                    task_id: task_id.to_string(),
                    total_cost_usd: total_cost,
                    duration_ms,
                    num_turns,
                },
            );
        }
        _ => {} // system/init 等 · 忽略
    }
}

/// 把工具输入压缩成一行摘要（前端 ToolCard 显示）
fn summarize_tool_input(name: &str, input: &serde_json::Value) -> String {
    match name {
        "Read" | "Write" | "Edit" => input
            .get("file_path")
            .and_then(|x| x.as_str())
            .unwrap_or("?")
            .to_string(),
        "Glob" => format!(
            "pattern: {}",
            input.get("pattern").and_then(|x| x.as_str()).unwrap_or("?")
        ),
        "Grep" => format!(
            "query: {}",
            input
                .get("pattern")
                .or_else(|| input.get("query"))
                .and_then(|x| x.as_str())
                .unwrap_or("?")
        ),
        _ => {
            // 未知工具 · 截前 80 字符 JSON 表示
            let s = input.to_string();
            truncate_preview(&s, 80)
        }
    }
}

/// 字符级截断 · 加省略号
fn truncate_preview(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_string()
    } else {
        let mut buf: String = s.chars().take(max_chars).collect();
        buf.push('…');
        buf
    }
}

/// 公共 spawn + 流式 emit
///
/// **E1**：task_id 不在内部生成 · 由调用方传入（便于 spawn 前创建 checkpoint 复用同一个 id）
async fn spawn_streaming(
    app: &AppHandle,
    mut cmd: TokioCommand,
    session_id: String,
    cmd_preview: String,
    task_id: String,
) -> Result<StudioChatSendResult, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn claude 失败: {}（确认 claude CLI 在 PATH）", e))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法获取 stderr".to_string())?;

    {
        let map = task_map();
        let mut g = map.lock().await;
        g.insert(task_id.clone(), StudioTask { child });
    }

    let _ = app.emit_tracked(
        EVENT_NAME,
        StudioEvent::Started {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            cmd_preview: cmd_preview.clone(),
        },
    );

    // 共享 stdout/stderr buffers · reader push · wait 任务读
    let stdout_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // pipe stdout · 解析 stream-json NDJSON · emit 结构化事件 · parse 失败 fallback Stream
    {
        let app2 = app.clone();
        let tid = task_id.clone();
        let sid_log = session_id.clone();
        let buf = stdout_buf.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                let line = match reader.next_line().await { Ok(Some(line)) => line, Ok(None) => break, Err(_) => { crate::task_events::stream_error(EVENT_NAME, &tid); break; } };
                tracing::trace!("studio claude stdout [{}]: {}", sid_log, line);
                {
                    let mut g = buf.lock().await;
                    g.push(line.clone());
                }
                // 尝试 parse stream-json · 失败 fallback 当 raw text
                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(v) => {
                        emit_stream_json_events(&app2, &tid, &v);
                    }
                    Err(_) => {
                        let _ = app2.emit_tracked(
                            EVENT_NAME,
                            StudioEvent::Stream { task_id: tid.clone(), line },
                        );
                    }
                }
            }
            crate::task_events::stream_closed(EVENT_NAME, &tid);
            tracing::debug!("studio claude stdout [{}]: <EOF>", sid_log);
        });
    }

    // pipe stderr · 同时 tracing log + 累计 buffer
    {
        let app2 = app.clone();
        let tid = task_id.clone();
        let sid_log = session_id.clone();
        let buf = stderr_buf.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            loop {
                let line = match reader.next_line().await { Ok(Some(line)) => line, Ok(None) => break, Err(_) => { crate::task_events::stream_error(EVENT_NAME, &tid); break; } };
                tracing::warn!("studio claude stderr [{}]: {}", sid_log, line);
                {
                    let mut g = buf.lock().await;
                    g.push(line.clone());
                }
                let _ = app2.emit_tracked(EVENT_NAME, StudioEvent::Stderr { task_id: tid.clone(), line });
            }
            crate::task_events::stream_closed(EVENT_NAME, &tid);
            tracing::debug!("studio claude stderr [{}]: <EOF>", sid_log);
        });
    }

    // 等退出
    {
        let app2 = app.clone();
        let tid = task_id.clone();
        let sid = session_id.clone();
        let buf_err = stderr_buf.clone();
        let buf_out = stdout_buf.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let map = task_map();
                let mut g = map.lock().await;
                let Some(task) = g.get_mut(&tid) else {
                    let _ = app2.emit_tracked(EVENT_NAME, StudioEvent::Cancelled { task_id: tid.clone() });
                    return;
                };
                match task.child.try_wait() {
                    Ok(Some(status)) => {
                        let exit_code = status.code().unwrap_or(-1);
                        if status.success() {
                            let _ = app2.emit_tracked(EVENT_NAME, StudioEvent::Completed {
                                task_id: tid.clone(),
                                session_id: sid.clone(),
                                exit_code,
                            });
                        } else {
                            // 释放 task_map 锁 · 否则下面 sleep 会卡死后续 chat
                            g.remove(&tid);
                            drop(g);
                            // 等 pipe drain · 避免 race（reader tokio task 还没 push 完）
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            // 取 stdout + stderr 最后 8 行附在 Failed 消息里
                            let stderr_tail = {
                                let g_buf = buf_err.lock().await;
                                let n = g_buf.len();
                                let start = n.saturating_sub(8);
                                g_buf[start..].join("\n")
                            };
                            let stdout_tail = {
                                let g_buf = buf_out.lock().await;
                                let n = g_buf.len();
                                let start = n.saturating_sub(8);
                                g_buf[start..].join("\n")
                            };
                            let mut parts = vec![format!("claude 非零退出 (code {exit_code})")];
                            if !stderr_tail.is_empty() {
                                parts.push(format!("--- stderr ---\n{}", stderr_tail));
                            }
                            if !stdout_tail.is_empty() {
                                parts.push(format!("--- stdout ---\n{}", stdout_tail));
                            }
                            if stderr_tail.is_empty() && stdout_tail.is_empty() {
                                parts.push(
                                    "(claude 静默退出 · 无 stdout/stderr 输出 · 可能是 TTY 检测 / signal kill)".into(),
                                );
                            }
                            let msg = parts.join("\n");
                            tracing::warn!("studio claude failed [{}]: {}", sid, msg);
                            let _ = app2.emit_tracked(EVENT_NAME, StudioEvent::Failed {
                                task_id: tid.clone(),
                                exit_code,
                                message: msg,
                            });
                            return;
                        }
                        g.remove(&tid);
                        return;
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        let _ = app2.emit_tracked(EVENT_NAME, StudioEvent::Failed {
                            task_id: tid.clone(),
                            exit_code: -1,
                            message: format!("等退出失败: {}", e),
                        });
                        g.remove(&tid);
                        return;
                    }
                }
            }
        });
    }

    Ok(StudioChatSendResult {
        task_id,
        session_id,
        cmd_preview,
    })
}

// =====================================================================
// D2.2 · Tauri 命令
// =====================================================================

/// 发送一条用户消息 · 调 Claude CLI 流式生成 assistant 回复
///
/// **D2.2 范围**：
/// - 单轮（不带历史 context）· 多轮 D2.6 实现
/// - 不持久化（不写 jsonl）· D2.3 实现
/// - 不接资料源（payload.source_paths 暂忽略）· D2.4 启用
///
/// **流程**：
/// 1. 解析 payload（无 session_id 时新建一个）
/// 2. 构造 Claude prompt（system + user）
/// 3. spawn `claude -p <prompt>` · 流式 stdout
/// 4. 返回 task_id + session_id · 前端 listen("studio:event") 接 stream
#[tauri::command]
pub async fn studio_chat_send(
    app: AppHandle,
    payload: StudioChatSendPayload,
) -> Result<StudioChatSendResult, String> {
    if payload.user_message.trim().is_empty() {
        return Err("user_message 不能为空".into());
    }

    let session_id = payload
        .session_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(generate_session_id);

    tracing::debug!(
        "studio_chat_send: session_id={} user_chars={}",
        session_id,
        payload.user_message.chars().count()
    );

    // 只解析 OpenRouter；现有 SOPS 失败不得换用 DPAPI 或其他服务绕过策略。
    let or_key = fetch_openrouter_key().await?;

    // 历史 messages（D2.6 多轮）· None / 空数组 视为首轮
    let history = payload.history.as_deref().unwrap_or(&[]);
    tracing::debug!("studio_chat_send: history={} 条", history.len());

    // **E1 Checkpoint**：spawn 前生成 task_id + 打快照
    // 快照失败不阻塞 chat（首轮 _raw 不存在是正常场景）· 仅 log warn
    let task_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = create_checkpoint(&task_id).await {
        tracing::warn!("create_checkpoint 跳过（撚回不可用）: {}", e);
    }
    cleanup_old_checkpoints(MAX_CHECKPOINTS).await;

    let (cmd, preview) = build_claude_command(&payload.user_message, history, &or_key)?;
    spawn_streaming(&app, cmd, session_id, preview, task_id).await
}

/// **D2.7**：从 jsonl 加载会话历史 messages · 用于切换会话恢复
#[tauri::command]
pub async fn studio_load_session(session_id: String) -> Result<Vec<ChatMessage>, String> {
    tracing::debug!("studio_load_session: session_id={}", session_id);
    let messages = read_messages_jsonl(&session_id).await?;
    tracing::debug!("studio_load_session: 读到 {} 条 messages", messages.len());
    Ok(messages)
}

/// **D2.7**：append 一条 ChatMessage 到 session 的 jsonl
///
/// 前端在两个时机调：
/// 1. 用户发送后立即写 user message（保证即使 spawn 失败也有记录）
/// 2. 任务 completed 后写 assistant message（含 parts + stats）
#[tauri::command]
pub async fn studio_persist_message(
    session_id: String,
    message: ChatMessage,
) -> Result<(), String> {
    tracing::debug!(
        "studio_persist_message: session_id={} role={} content_chars={}",
        session_id,
        message.role,
        message.content.chars().count()
    );
    append_message_jsonl(&session_id, &message).await
}

/// **E2**：读 attachments/ 下的图片文件 · 返回 base64 data URL（前端 img src 用）
///
/// 安全：path 必须在 `<vault>/_raw/attachments/` 下 · 否则拒绝
#[tauri::command]
pub async fn studio_read_attachment_b64(path: String) -> Result<String, String> {
    use base64::Engine;
    let p = PathBuf::from(&path);
    crate::legacy_isolation::ensure_test_path(&p)?;
    let raw = raw_dir()?;
    let allowed = raw.join("attachments");
    crate::legacy_isolation::ensure_test_path(&allowed)?;
    if !allowed.exists() {
        return Err(format!("attachments 目录不存在: {}", allowed.display()));
    }
    let canonical = p
        .canonicalize()
        .map_err(|e| format!("canonical {}: {}", path, e))?;
    let allowed_canonical = allowed
        .canonicalize()
        .map_err(|e| format!("canonical attachments dir: {}", e))?;
    if !canonical.starts_with(&allowed_canonical) {
        return Err(format!(
            "禁止：path 不在 {} 下",
            allowed_canonical.display()
        ));
    }
    let bytes = tokio::fs::read(&canonical)
        .await
        .map_err(|e| format!("读图失败: {}", e))?;
    if bytes.len() > 10 * 1024 * 1024 {
        return Err(format!("图片过大 ({} bytes) · 拒绝读 base64", bytes.len()));
    }
    let mime = match canonical.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/png",
    };
    let b64 = base64::prelude::BASE64_STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, b64))
}

// =====================================================================
// E2.6 · 生图阶段进度（前端 polling · 配合 mcp-image binary）
// =====================================================================

/// 生图阶段进度（mcp-image binary 写到 ~/.homebase/img-progress/<pid>.json）
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ImageProgress {
    /// mcp-image 子进程 pid（文件名）
    pub pid: u32,
    /// 阶段：preparing / sending / waiting / decoding / saving / done / error
    pub stage: String,
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
    pub elapsed_ms: u64,
    pub model: String,
    pub prompt_preview: String,
    /// done 阶段 = 输出文件绝对路径
    pub output_path: Option<String>,
    /// error 阶段 = 错误信息
    pub error: Option<String>,
}

#[cfg(test)]
mod query_safety_tests {
    use super::*;
    #[test]
    fn legacy_isolation_store_keys_reject_directory_and_stream_escape_without_io() {
        for key in ["", ".", "..", "../other", r"..\other", r"F:\outside", "file:stream", "name ", "name.", "line\nbreak", "CON", "con.txt", "NUL", "COM1", "LPT9", "file*", "file?"] {
            assert!(storage_key(key).is_err(), "{key}");
        }
        for key in ["studio-20261117-fixture", "旧文稿🙂", "8af17fb3-97b5-4aa1-9ec0-a5dc430672fc"] {
            assert!(storage_key(key).is_ok());
        }
    }
    #[test]
    #[ignore = "Explicit isolated pure path probe; never lists real sessions"]
    fn legacy_isolation_path_probe() {
        let home = PathBuf::from(std::env::var_os("HOMEBASE_TEST_HOME").unwrap());
        assert_eq!(raw_dir().unwrap(), home.join("Wiki/_raw"));
    }
    #[tokio::test]
    async fn query_safety_progress_query_preserves_corrupt_and_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("1.json");
        std::fs::write(&path, "broken synthetic json").unwrap();
        assert!(list_image_progress_in(dir.path().into()).await.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken synthetic json");
        let progress = ImageProgress { pid: 1, stage: "waiting".into(), started_at_ms: 1, updated_at_ms: 1, elapsed_ms: 0, model: "synthetic".into(), prompt_preview: "fixture".into(), output_path: None, error: None };
        let bytes = serde_json::to_vec(&progress).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let result = list_image_progress_in(dir.path().into()).await.unwrap();
        assert_eq!(result[0].stage, "unknown");
        assert!(result[0].error.as_ref().unwrap().contains("未知"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

/// 进度目录 · ~/.homebase/img-progress/
fn img_progress_dir() -> Option<PathBuf> {
    if let Some(path) = crate::legacy_isolation::test_path(".homebase/img-progress") { return Some(path); }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    Some(PathBuf::from(home).join(".homebase").join("img-progress"))
}

/// **E2.6**：列出当前所有 in-progress 生图任务（按 started_at_ms 倒序 · 最新在前）
///
/// 只读查询；过期进度显式标为未知，不删除现有进度文件。
#[tauri::command]
pub async fn studio_list_image_progress() -> Result<Vec<ImageProgress>, String> {
    let dir = match img_progress_dir() {
        Some(d) => d,
        None => return Err("QUERY_FAILED: 无法定位生图进度目录".into()),
    };
    list_image_progress_in(dir).await
}

async fn list_image_progress_in(dir: PathBuf) -> Result<Vec<ImageProgress>, String> {
    crate::legacy_isolation::ensure_test_path(&dir)?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let entries = match tokio::fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return Err("QUERY_FAILED: 生图进度目录不可读取".into()),
    };
    let mut out: Vec<ImageProgress> = Vec::new();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    const STALE_MS: u64 = 5 * 60 * 1000; // 5 分钟

    let mut entries = entries;
    while let Some(entry) = entries.next_entry().await.map_err(|_| "QUERY_FAILED: 生图进度目录条目不可读取")? {
        let path = entry.path();
        crate::legacy_isolation::ensure_test_path(&path)?;
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => return Err("QUERY_FAILED: 生图进度文件不可读取".into()),
        };
        let mut progress: ImageProgress = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(_) => {
                return Err("QUERY_INVALID: 生图进度文件格式无效或尚未写完，未删除文件".into());
            }
        };
        // 僵尸清理（应当极少发生 · mcp-image 完成时自己 remove · 这里是 crash 兜底）
        if now_ms.saturating_sub(progress.updated_at_ms) > STALE_MS {
            progress.stage = "unknown".into();
            progress.error = Some("进度超过 5 分钟未更新，实际任务状态未知，请核对输出；进度文件已保留".into());
        }
        out.push(progress);
    }
    out.sort_by(|a, b| b.started_at_ms.cmp(&a.started_at_ms));
    Ok(out)
}

/// **E1**：一键撚回 AI 改动 · unzip checkpoint + truncate jsonl
///
/// 参数：
/// - `session_id`：当前会话 id
/// - `task_id`：= user message 的 checkpoint_id
/// - `cutoff_ts`：= user message 的 ts（该条及之后全删）
///
/// 返回：截断后剩余的 messages（前端直接 setMessages 替换）
///
/// 流程：
/// 1. unzip `<task_id>.zip` 覆盖 `_raw/`（排除 attachments + .studio-checkpoints）
/// 2. 重写 jsonl · 只保留 ts < cutoff_ts 的行
/// 3. 重新读 jsonl 返回
#[tauri::command]
pub async fn studio_restore_checkpoint(
    session_id: String,
    task_id: String,
    cutoff_ts: i64,
) -> Result<Vec<ChatMessage>, String> {
    tracing::debug!(
        "studio_restore_checkpoint: session={} task={} cutoff_ts={}",
        session_id,
        task_id,
        cutoff_ts
    );

    // 1. unzip 覆盖 _raw
    restore_checkpoint(&task_id).await?;

    // 2. 截断 jsonl（删除 ts >= cutoff_ts 的 messages）
    truncate_jsonl_at_or_after(&session_id, cutoff_ts).await?;

    // 3. 读取截断后的 messages 返回
    let messages = read_messages_jsonl(&session_id).await?;
    tracing::debug!(
        "studio_restore_checkpoint: 还原完成 · 剩余 {} 条 messages",
        messages.len()
    );
    Ok(messages)
}

/// 取消正在跑的 chat 任务 · kill child + emit Cancelled
#[tauri::command]
pub async fn studio_cancel(task_id: String) -> Result<(), String> {
    tracing::debug!("studio_cancel: task_id={}", task_id);
    let map = task_map();
    let mut g = map.lock().await;
    if let Some(mut task) = g.remove(&task_id) {
        let _ = task.child.kill().await;
    }
    Ok(())
}
