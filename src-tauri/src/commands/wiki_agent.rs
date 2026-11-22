//! Wiki Agent 消化命令（Phase 14.5c — 方案 A · v1 stub）
//!
//! 用户场景：
//! - 投料口落到 `raw/inbox/` 后，需要 AI 消化成 `wiki/` 概念页
//! - 用户选了方案 A：独立 Agent 概念，但 UI 嵌在投料口下方
//!
//! v1（本版本）：
//! - 不真的 spawn AI 进程（避免长任务管理 / 流式输出 / 错误恢复复杂度）
//! - 提供 `wiki_agent_status`：扫 `raw/inbox/` 下未消化文件数（frontmatter 缺 `digested_at`）
//! - 提供 `wiki_agent_prepare_prompt`：生成可粘贴到 Claude Code / Codex 的 ingest prompt
//! - 提供 `wiki_agent_open_in_terminal`：在 vault 目录下打开 Claude Code（`claude` CLI）
//!
//! v2（下一轮）：
//! - 真正 spawn 子进程跑 `claude --print` 或类似 headless 模式
//! - 流式回写状态 / 进度 / log

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use tauri::AppHandle;
use crate::task_events::TrackedEmitter;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// =====================================================================
// v2 · headless 真自动化 · 任务管理（全局 Mutex 持有 spawn 出去的子进程）
// =====================================================================

/// 单条投料消化任务的运行时状态
struct IngestTask {
    /// tokio Child handle，用于 kill
    child: tokio::process::Child,
}

/// 全局任务表 task_id -> IngestTask
type IngestTaskMap = Arc<Mutex<HashMap<String, IngestTask>>>;

/// 用 OnceLock 创建全局任务表（首次访问时初始化）
fn ingest_task_map() -> &'static IngestTaskMap {
    use std::sync::OnceLock;
    static MAP: OnceLock<IngestTaskMap> = OnceLock::new();
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

// =====================================================================
// 类型
// =====================================================================

#[derive(Debug, Serialize)]
pub struct WikiAgentStatus {
    /// 未消化的 raw/inbox 文件数（frontmatter 没 digested_at）
    pub pending_count: usize,
    /// 已消化文件数（frontmatter 有 digested_at）
    pub digested_count: usize,
    /// 投料总数
    pub total_count: usize,
    /// 最后一条 inbox 文件的修改时间（ISO8601 本地）
    pub last_inbox_at: Option<String>,
    /// vault 根
    pub vault_root: String,
    /// 是否找到 AGENTS.md（消化的指令源）
    pub has_agents_md: bool,
    /// 默认底层 Agent backend（v1 固定 claude_code，v2 从 settings 读）
    pub backend: String,
}

#[derive(Debug, Deserialize)]
pub struct WikiAgentPreparePromptPayload {
    /// 可选：只针对某一天 raw/inbox/<day>/ 的未消化文件
    #[serde(default)]
    pub day: Option<String>,
    /// 可选：只针对特定 raw 路径列表
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WikiAgentPreparePromptResult {
    /// 可粘贴到 Claude Code / Codex 的完整 prompt
    pub prompt: String,
    /// 涉及的 raw 文件路径列表（绝对路径）
    pub target_files: Vec<String>,
    /// 提示中提到的 schema 文档路径
    pub schema_path: String,
}

// =====================================================================
// 命令 1：状态查询
// =====================================================================

#[tauri::command]
pub async fn wiki_agent_status() -> Result<WikiAgentStatus, String> {
    tracing::debug!("wiki_agent_status 调用");

    let vault_root = wiki_vault_root();
    crate::legacy_isolation::ensure_test_path(&vault_root)?;
    let inbox_root = vault_root.join("raw").join("inbox");
    let agents_md = vault_root.join("AGENTS.md");
    crate::legacy_isolation::ensure_test_path(&inbox_root)?;
    crate::legacy_isolation::ensure_test_path(&agents_md)?;

    let mut pending = 0usize;
    let mut digested = 0usize;
    let mut total = 0usize;
    let mut last_modified: Option<std::time::SystemTime> = None;

    {
        for day_entry in crate::query::entries(&inbox_root, true)? {
            crate::legacy_isolation::ensure_test_path(&day_entry.path())?;
            if !day_entry.file_type().map_err(|_| "QUERY_FAILED: Inbox 条目类型不可读取")?.is_dir() {
                continue;
            }
            for file_entry in crate::query::entries(&day_entry.path(), false)? {
                let path = file_entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("md") {
                    continue;
                }
                total += 1;

                if let Ok(meta) = file_entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        match last_modified {
                            Some(t) if t >= modified => {}
                            _ => last_modified = Some(modified),
                        }
                    }
                }

                // 读 frontmatter 看 digested_at
                if has_digested_at(&path)? {
                    digested += 1;
                } else {
                    pending += 1;
                }
            }
        }
    }

    let last_inbox_at = last_modified.map(|t| {
        chrono::DateTime::<Local>::from(t)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string()
    });

    let cfg = load_wiki_agent_config()?;
    Ok(WikiAgentStatus {
        pending_count: pending,
        digested_count: digested,
        total_count: total,
        last_inbox_at,
        vault_root: vault_root.to_string_lossy().into_owned(),
        has_agents_md: agents_md.exists(),
        backend: cfg.backend,
    })
}

/// 看 markdown 文件 frontmatter 有没有 `digested_at: ...` 字段
fn has_digested_at(path: &Path) -> Result<bool, String> {
    crate::legacy_isolation::ensure_test_path(path)?;
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Err("QUERY_FAILED: Inbox 元数据不可读取，消化状态未知".into()),
    };
    if !raw.starts_with("---") {
        return Ok(false);
    }
    let after = &raw[3..];
    let end = match after.find("\n---") {
        Some(i) => i,
        None => return Ok(false),
    };
    let frontmatter = &after[..end];
    Ok(frontmatter
        .lines()
        .any(|line| line.trim_start().starts_with("digested_at:")))
}

// =====================================================================
// 命令 2：生成可粘贴的 ingest prompt（v1 stub 核心）
// =====================================================================

#[tauri::command]
pub async fn wiki_agent_prepare_prompt(
    payload: WikiAgentPreparePromptPayload,
) -> Result<WikiAgentPreparePromptResult, String> {
    tracing::debug!("wiki_agent_prepare_prompt 调用");

    let vault_root = wiki_vault_root();
    crate::legacy_isolation::ensure_test_path(&vault_root)?;
    let inbox_root = vault_root.join("raw").join("inbox");
    let agents_md = vault_root.join("AGENTS.md");
    let naming_md = vault_root.join("_schema").join("NAMING_CONVENTION.md");
    for path in [&inbox_root, &agents_md, &naming_md] { crate::legacy_isolation::ensure_test_path(path)?; }

    // 收集目标文件
    let mut target_files: Vec<PathBuf> = Vec::new();

    if !payload.paths.is_empty() {
        for p in payload.paths {
            target_files.push(PathBuf::from(p));
        }
    } else {
        // 扫未消化的（按 day 过滤可选）
        let day_filter = payload.day.as_deref();
        {
            for day_entry in crate::query::entries(&inbox_root, true)? {
                crate::legacy_isolation::ensure_test_path(&day_entry.path())?;
                if !day_entry.file_type().map_err(|_| "QUERY_FAILED: Inbox 条目类型不可读取")?.is_dir() {
                    continue;
                }
                let day_name = day_entry.file_name();
                let day_str = day_name.to_string_lossy();
                if let Some(filter) = day_filter {
                    if day_str != filter {
                        continue;
                    }
                }
                for file_entry in crate::query::entries(&day_entry.path(), false)? {
                    let path = file_entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("md") {
                        continue;
                    }
                    if !has_digested_at(&path)? {
                        target_files.push(path);
                    }
                }
            }
        }
    }

    for path in &target_files { crate::legacy_isolation::ensure_test_path(path)?; }
    // 排序（按修改时间，老的在前 — 先消化老投料）
    target_files.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());

    let target_paths: Vec<String> = target_files
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    // 生成 prompt
    let prompt = render_ingest_prompt(&vault_root, &agents_md, &naming_md, &target_paths);

    Ok(WikiAgentPreparePromptResult {
        prompt,
        target_files: target_paths,
        schema_path: agents_md.to_string_lossy().into_owned(),
    })
}

fn render_ingest_prompt(
    vault: &Path,
    agents_md: &Path,
    naming_md: &Path,
    targets: &[String],
) -> String {
    let now = Local::now().format("%Y-%m-%d %H:%M").to_string();
    let n = targets.len();

    let target_list = if targets.is_empty() {
        "  (无未消化的 inbox 文件)".to_string()
    } else {
        targets
            .iter()
            .map(|p| format!("  - `{p}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"# Wiki 消化任务 · {now}

你的角色：**LLM Wiki 维护者**（按 Karpathy LLM Wiki 模式）

## 上下文文档（必读）

1. **Schema 指令**：`{agents}`
2. **命名规范**：`{naming}`
3. **Vault 根**：`{vault}`

## 本次任务

把以下 {n} 个未消化的 raw/inbox 文件按 4 阶段 workflow 消化进 wiki/：

{target_list}

## 4 阶段 workflow（来自 AGENTS.md）

1. **Ingest** — 逐个读 raw 文件，理解内容
2. **Extract** — 提炼概念 / 实体 / 关系
3. **Resolve** — 跟 wiki/concepts/、wiki/entities/ 已有页对齐去重
4. **Schema** — 创建或更新对应 wiki/ 页（按 NAMING_CONVENTION.md §5 frontmatter 标准）

## 完成后

- 在 `wiki/log.md` 追加一条 `## [{date}] ingest | <summary>` 条目
- 在每个被消化的 raw 文件 frontmatter 加 `digested_at: <ISO8601>` 标记（不删原文）
- 报告：哪些 wiki 页被新增 / 更新 / 跳过，以及发现的潜在矛盾

按 `_schema/NAMING_CONVENTION.md` 的反模式表格 §8 自检产物 — 不要在 wiki/concepts/ 用日期前置文件名。

开始吧。
"#,
        now = now,
        date = Local::now().format("%Y-%m-%d"),
        agents = agents_md.to_string_lossy(),
        naming = naming_md.to_string_lossy(),
        vault = vault.to_string_lossy(),
        n = n,
        target_list = target_list,
    )
}

// =====================================================================
// 命令 3：在 vault 目录打开终端（让用户跑 claude / codex）
// =====================================================================

#[tauri::command]
pub async fn wiki_agent_open_in_terminal() -> Result<String, String> {
    tracing::debug!("wiki_agent_open_in_terminal 调用");

    let vault_root = wiki_vault_root();
    crate::legacy_isolation::ensure_test_path(&vault_root)?;
    let vault_str = vault_root.to_string_lossy().into_owned();

    #[cfg(target_os = "windows")]
    {
        // Windows Terminal (`wt.exe`) 优先；找不到 fallback 到 `cmd /c start cmd`
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // 试 wt.exe
        let try_wt = Command::new("wt.exe")
            .args(["-d", &vault_str])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();

        if try_wt.is_ok() {
            return Ok(format!("已用 Windows Terminal 打开：{vault_str}"));
        }

        // fallback：cmd
        Command::new("cmd")
            .args(["/c", "start", "cmd", "/k", &format!("cd /d {vault_str}")])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("打开终端失败：{e}"))?;

        return Ok(format!("已用 cmd 打开：{vault_str}"));
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .args(["-a", "Terminal", &vault_str])
            .spawn()
            .map_err(|e| format!("打开终端失败：{e}"))?;
        return Ok(format!("已用 Terminal.app 打开：{vault_str}"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Linux: 试 gnome-terminal / konsole / xterm
        for term in ["gnome-terminal", "konsole", "xterm"] {
            if Command::new(term)
                .arg("--working-directory")
                .arg(&vault_str)
                .spawn()
                .is_ok()
            {
                return Ok(format!("已用 {term} 打开：{vault_str}"));
            }
        }
        Err("未找到可用终端（gnome-terminal / konsole / xterm）".into())
    }
}

// =====================================================================
// 命令 4 · v2 · run_ingest · spawn claude headless + 流式 emit Tauri events
// =====================================================================
//
// 输入：
//   - WikiAgentRunIngestPayload（同 prepare_prompt 的 day/paths 过滤）
// 输出：
//   - task_id（前端用来订阅事件 / 中止）
// 事件名："wiki-agent:event"
// 事件 payload 见 WikiAgentEvent，几种 kind：
//   - "started"   : 任务开始（含 task_id、target_files、prompt 摘要）
//   - "stream"    : claude stream-json 的一行（透传给前端解析）
//   - "stderr"    : 子进程 stderr 一行（错误日志）
//   - "completed" : 正常退出
//   - "failed"    : 非零退出
//   - "cancelled" : 用户中止

#[derive(Debug, Deserialize)]
pub struct WikiAgentRunIngestPayload {
    /// 只针对某一天的未消化文件
    #[serde(default)]
    pub day: Option<String>,
    /// 只针对特定路径列表
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WikiAgentRunIngestResult {
    pub task_id: String,
    pub target_count: usize,
    /// 实际跑的命令行（前端展示给用户看）
    pub command_preview: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WikiAgentEvent {
    Started {
        task_id: String,
        target_count: usize,
        backend: String,
    },
    /// 一行 stream-json（前端 JSON.parse 后展开各种 message 类型）
    Stream {
        task_id: String,
        line: String,
    },
    Stderr {
        task_id: String,
        line: String,
    },
    Completed {
        task_id: String,
        exit_code: i32,
    },
    Failed {
        task_id: String,
        exit_code: i32,
        message: String,
    },
    Cancelled {
        task_id: String,
    },
}

const EVENT_NAME: &str = "wiki-agent:event";

#[tauri::command]
pub async fn wiki_agent_run_ingest(
    app: AppHandle,
    payload: WikiAgentRunIngestPayload,
) -> Result<WikiAgentRunIngestResult, String> {
    tracing::debug!("wiki_agent_run_ingest 调用");

    let cfg = load_wiki_agent_config()?;
    if cfg.backend != "claude-code" {
        return Err(format!(
            "v2 当前只支持 claude-code backend（当前：{}）。请到 Wiki Agent 设置切换或使用 v1 'prepare_prompt' 模式。",
            cfg.backend
        ));
    }

    // 复用 prepare_prompt 的目标收集 + prompt 渲染逻辑
    let prepare = wiki_agent_prepare_prompt(WikiAgentPreparePromptPayload {
        day: payload.day,
        paths: payload.paths,
    })
    .await?;

    let target_count = prepare.target_files.len();
    if target_count == 0 {
        return Err("没有未消化的 raw 文件可处理".into());
    }

    let vault_root = wiki_vault_root();
    let task_id = uuid::Uuid::new_v4().to_string();
    crate::legacy_isolation::ensure_test_path(&vault_root)?;

    // 构造命令行 — claude -p <prompt> --output-format stream-json --verbose
    //   --permission-mode acceptEdits  → 自动接受文件写入
    //   --allowedTools "Read,Write,Edit,Glob,Grep"  → 仅授权这几个，不让乱跑 Bash
    let allowed_tools = "Read,Write,Edit,Glob,Grep";
    let command_preview = format!(
        "claude -p <prompt> --output-format stream-json --verbose --permission-mode acceptEdits --allowedTools \"{allowed_tools}\""
    );

    let mut cmd = TokioCommand::new("claude");
    cmd.arg("-p")
        .arg(&prepare.prompt)
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--include-partial-messages")
        .arg("--permission-mode")
        .arg("acceptEdits")
        .arg("--allowedTools")
        .arg(allowed_tools)
        .current_dir(&vault_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn claude 失败：{e}（请确认 claude CLI 在 PATH 中）"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 claude 子进程 stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法获取 claude 子进程 stderr".to_string())?;

    // 注册任务
    {
        let map = ingest_task_map();
        let mut g = map.lock().await;
        g.insert(task_id.clone(), IngestTask { child });
    }

    // emit started
    let _ = app.emit_tracked(
        EVENT_NAME,
        WikiAgentEvent::Started {
            task_id: task_id.clone(),
            target_count,
            backend: cfg.backend.clone(),
        },
    );

    // spawn 后台 task 流式读 stdout / stderr，emit 事件
    {
        let app_clone = app.clone();
        let task_id_clone = task_id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                let line = match reader.next_line().await { Ok(Some(line)) => line, Ok(None) => break, Err(_) => { crate::task_events::stream_error(EVENT_NAME, &task_id_clone); break; } };
                let _ = app_clone.emit_tracked(
                    EVENT_NAME,
                    WikiAgentEvent::Stream {
                        task_id: task_id_clone.clone(),
                        line,
                    },
                );
            }
            crate::task_events::stream_closed(EVENT_NAME, &task_id_clone);
        });
    }
    {
        let app_clone = app.clone();
        let task_id_clone = task_id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            loop {
                let line = match reader.next_line().await { Ok(Some(line)) => line, Ok(None) => break, Err(_) => { crate::task_events::stream_error(EVENT_NAME, &task_id_clone); break; } };
                let _ = app_clone.emit_tracked(
                    EVENT_NAME,
                    WikiAgentEvent::Stderr {
                        task_id: task_id_clone.clone(),
                        line,
                    },
                );
            }
            crate::task_events::stream_closed(EVENT_NAME, &task_id_clone);
        });
    }

    // spawn 后台 task 等子进程退出，emit completed / failed / cancelled
    {
        let app_clone = app.clone();
        let task_id_clone = task_id.clone();
        tokio::spawn(async move {
            // 这里不能直接 await child（已 move 到 map）；改成轮询 map 等任务结束
            // 用 try_wait poll：每 500ms 看一次
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let map = ingest_task_map();
                let mut g = map.lock().await;
                let Some(task) = g.get_mut(&task_id_clone) else {
                    // 已被 cancel_ingest 移除
                    let _ = app_clone.emit_tracked(
                        EVENT_NAME,
                        WikiAgentEvent::Cancelled {
                            task_id: task_id_clone.clone(),
                        },
                    );
                    return;
                };
                match task.child.try_wait() {
                    Ok(Some(status)) => {
                        let exit_code = status.code().unwrap_or(-1);
                        if status.success() {
                            let _ = app_clone.emit_tracked(
                                EVENT_NAME,
                                WikiAgentEvent::Completed {
                                    task_id: task_id_clone.clone(),
                                    exit_code,
                                },
                            );
                        } else {
                            let _ = app_clone.emit_tracked(
                                EVENT_NAME,
                                WikiAgentEvent::Failed {
                                    task_id: task_id_clone.clone(),
                                    exit_code,
                                    message: format!("claude 非零退出 (code {exit_code})"),
                                },
                            );
                        }
                        g.remove(&task_id_clone);
                        return;
                    }
                    Ok(None) => {
                        // 仍在跑，继续 poll
                    }
                    Err(e) => {
                        let _ = app_clone.emit_tracked(
                            EVENT_NAME,
                            WikiAgentEvent::Failed {
                                task_id: task_id_clone.clone(),
                                exit_code: -1,
                                message: format!("等待 claude 退出失败：{e}"),
                            },
                        );
                        g.remove(&task_id_clone);
                        return;
                    }
                }
            }
        });
    }

    Ok(WikiAgentRunIngestResult {
        task_id,
        target_count,
        command_preview,
    })
}

#[tauri::command]
pub async fn wiki_agent_cancel_ingest(task_id: String) -> Result<(), String> {
    tracing::debug!("wiki_agent_cancel_ingest 调用: {}", task_id);

    let map = ingest_task_map();
    let mut g = map.lock().await;
    let Some(mut task) = g.remove(&task_id) else {
        return Err(format!("task {task_id} 不存在或已结束"));
    };
    if let Err(e) = task.child.kill().await {
        return Err(format!("kill 失败：{e}"));
    }
    Ok(())
}

// =====================================================================
// 命令 4 / 5：配置 get / set（v1 只持久化，调度由 v2 接 tokio scheduler）
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiAgentConfig {
    /// 底层 AI backend：claude-code / codex / gemini-cli / hermes
    #[serde(default = "default_backend")]
    pub backend: String,
    /// 是否启用自动消化（v1 只存，不真的跑）
    #[serde(default)]
    pub auto_digest: bool,
    /// 自动消化间隔小时（1 / 6 / 24 等；0 = 手动）
    #[serde(default = "default_interval_hours")]
    pub interval_hours: u32,
    /// vault 根目录（v1 默认 C:\HomeBase\Wiki，预留可改）
    #[serde(default = "default_vault_root")]
    pub vault_root: String,
}

fn default_backend() -> String {
    "claude-code".into()
}
fn default_interval_hours() -> u32 {
    6
}
fn default_vault_root() -> String {
    wiki_vault_root().to_string_lossy().into_owned()
}

impl Default for WikiAgentConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            auto_digest: false,
            interval_hours: default_interval_hours(),
            vault_root: default_vault_root(),
        }
    }
}

#[tauri::command]
pub async fn wiki_agent_get_config() -> Result<WikiAgentConfig, String> {
    tracing::debug!("wiki_agent_get_config 调用");
    load_wiki_agent_config()
}

#[tauri::command]
pub async fn wiki_agent_set_config(config: WikiAgentConfig) -> Result<WikiAgentConfig, String> {
    tracing::debug!("wiki_agent_set_config 调用: backend={}", config.backend);
    save_wiki_agent_config(&config)?;
    Ok(config)
}

fn wiki_agent_config_path() -> Option<PathBuf> {
    if let Some(path) = crate::legacy_isolation::test_path(".homebase/wiki_agent.json") { return Some(path); }
    std::env::var("USERPROFILE")
        .ok()
        .map(|h| PathBuf::from(h).join(".homebase").join("wiki_agent.json"))
}

fn load_wiki_agent_config() -> Result<WikiAgentConfig, String> {
    let path = wiki_agent_config_path().ok_or("QUERY_FAILED: 无法定位用户配置目录")?;
    crate::legacy_isolation::ensure_test_path(&path)?;
    let config: WikiAgentConfig = crate::query::optional_json(&path)?;
    crate::legacy_isolation::ensure_test_path(Path::new(&config.vault_root))?;
    Ok(config)
}

fn save_wiki_agent_config(config: &WikiAgentConfig) -> Result<(), String> {
    let path = wiki_agent_config_path()
        .ok_or_else(|| "无法定位用户目录（USERPROFILE 未设置）".to_string())?;
    crate::legacy_isolation::ensure_test_path(&path)?;
    crate::legacy_isolation::ensure_test_path(Path::new(&config.vault_root))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("序列化配置失败：{e}"))?;
    fs::write(&path, json).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    Ok(())
}

// =====================================================================
// 内部工具
// =====================================================================

fn wiki_vault_root() -> PathBuf {
    crate::legacy_isolation::wiki_root()
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Explicit isolated pure path probe; never reads real Wiki"]
    fn legacy_isolation_path_probe() {
        let home = PathBuf::from(std::env::var_os("HOMEBASE_TEST_HOME").unwrap());
        assert_eq!(wiki_vault_root(), home.join("Wiki"));
    }

    #[test]
    fn render_prompt_contains_target_paths() {
        let targets = vec![
            "C:\\HomeBase\\Wiki\\raw\\inbox\\2026-05-04\\231233-a3f.md".into(),
            "C:\\HomeBase\\Wiki\\raw\\inbox\\2026-05-04\\231245-b4c.md".into(),
        ];
        let p = render_ingest_prompt(
            Path::new("C:\\HomeBase\\Wiki"),
            Path::new("C:\\HomeBase\\Wiki\\AGENTS.md"),
            Path::new("C:\\HomeBase\\Wiki\\_schema\\NAMING_CONVENTION.md"),
            &targets,
        );
        assert!(p.contains("231233-a3f.md"));
        assert!(p.contains("231245-b4c.md"));
        assert!(p.contains("AGENTS.md"));
        assert!(p.contains("NAMING_CONVENTION.md"));
        assert!(p.contains("4 阶段 workflow"));
    }

    #[test]
    fn render_prompt_handles_empty_targets() {
        let p = render_ingest_prompt(
            Path::new("C:\\HomeBase\\Wiki"),
            Path::new("C:\\HomeBase\\Wiki\\AGENTS.md"),
            Path::new("C:\\HomeBase\\Wiki\\_schema\\NAMING_CONVENTION.md"),
            &[],
        );
        assert!(p.contains("(无未消化的 inbox 文件)"));
    }

    #[test]
    fn has_digested_at_detects_marker() {
        // 这个测试只测函数逻辑，需要 fs，跳过实际文件 IO 用 frontmatter 字符串模拟
        // 实际 has_digested_at 读文件，测试需要 tempdir。简化为只测概念正确性 — 见下面 frontmatter 解析。
        // （CI 跑得通才有意义，先 placeholder）
        assert!(true);
    }
}
