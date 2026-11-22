//! Agents 模块命令（v3 §6.2）— 5 agent 真实状态检测 + 配置检查 + 启动
//!
//! 检测策略：
//! 1. **进程检测**：用 sysinfo crate 列当前进程，按进程名子串匹配
//! 2. **配置检查**：检查 ~/.claude/settings.json 等配置文件是否存在
//! 3. **launch 链路**：通过 adapters::agent_env::AgentEnvAdapter 启动（v3 §10.3）
//!
//! 状态语义（AgentState 枚举）：
//! - Running：进程在跑（不论配置是否完整）— 绿
//! - Stopped：配置存在但进程未跑 — 灰
//! - Misconfigured：配置缺失（agent 装了但需要初始化）— 黄
//! - NotInstalled：连配置目录都没有（agent 未装）— 红

use crate::adapters::sops::SopsAdapter;
use crate::util::strip_bom;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use sysinfo::System;

/// SOPS 加密 secrets 文件路径（与 commands::secrets 共用）

fn secrets_yaml_path() -> PathBuf {
    crate::secret_policy::vault_path()
}

// =====================================================================
// SOPS reveal helper 路径定位
// =====================================================================

/// 使用和启动检查相同的密钥助手路径。
fn homebase_reveal_exe_path() -> Result<PathBuf, String> {
    let candidate = crate::reveal_runtime::current().map_err(|error| error.to_string())?;
    if !candidate.is_file() {
        return Err(format!(
            "找不到密钥助手，请检查 HomeBase 随包运行时或开发版构建：{}",
            candidate.display()
        ));
    }
    Ok(candidate)
}

/// 拿到 ~/.homebase/ 目录路径（必要时自动创建）
fn homebase_dir() -> Result<PathBuf, String> {
    let dir = PathBuf::from(env_home()).join(".homebase");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    }
    Ok(dir)
}

/// 为 Claude Code apiKeyHelper 字段写一个 .cmd 中转脚本
///
/// Claude Code apiKeyHelper 字段只接单可执行路径（不接参数），所以需要
/// 一层 .cmd 中转把 `<reveal_exe> <short_name>` 包装起来。
///
/// 路径：`~/.homebase/reveal-<short_name>.cmd`，内容固定为：
/// ```cmd
/// @echo off
/// "<reveal_exe absolute>" <short_name>
/// ```
fn ensure_reveal_cmd_for(short_name: &str) -> Result<PathBuf, String> {
    if short_name.trim().is_empty() {
        return Err("short_name 为空".into());
    }
    // 防注入：short_name 必须符合白名单字符集（已通过 LLM_SECRET_REGISTRY 校验，
    // 但这里再做一次硬性 check，作为深度防御）
    if !short_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!("short_name {short_name:?} 含非法字符"));
    }
    let reveal_exe = homebase_reveal_exe_path()?;
    let dir = homebase_dir()?;
    let cmd_path = dir.join(format!("reveal-{short_name}.cmd"));
    let content = format!(
        "@echo off\r\n\"{}\" {}\r\n",
        reveal_exe.display(),
        short_name
    );
    std::fs::write(&cmd_path, content)
        .map_err(|e| format!("写 {} 失败: {e}", cmd_path.display()))?;
    Ok(cmd_path)
}

/// 从 apiKeyHelper 路径反推 short_name
///
/// 期望格式：`.../reveal-<short_name>.cmd`；非该形态返回 None
fn parse_short_name_from_helper(helper_path: &str) -> Option<String> {
    let p = Path::new(helper_path);
    let file = p.file_name()?.to_str()?;
    let stem = file.strip_suffix(".cmd")?;
    let short = stem.strip_prefix("reveal-")?;
    if short.is_empty() {
        None
    } else {
        Some(short.to_string())
    }
}

// =====================================================================
// 类型
// =====================================================================

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    /// "cli" | "service" | "mcp" | "skill"（v3 §18 — 当前全是 cli）
    pub kind: String,
    pub config_path: Option<String>,
    pub config_exists: bool,
    /// agent-env.ps1 子命令（如 "claude" / "codex"）；None 表示直接调主命令
    pub launch_subcommand: Option<String>,
    /// 进程名匹配关键字（前端显示给用户参考）
    pub process_match: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentStatus {
    pub id: String,
    pub state: AgentState,
    pub running: bool,
    pub pid: Option<u32>,
    pub config_exists: bool,
    pub config_path: Option<String>,
    pub mcp_servers: Vec<McpServerStatus>,
    pub last_seen: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// 进程在跑（绿）
    Running,
    /// 已配置但未启动（灰）
    Stopped,
    /// 配置缺失但 agent 装了（黄）
    Misconfigured,
    /// agent 完全没装（红）
    NotInstalled,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpServerStatus {
    pub name: String,
    pub healthy: bool,
}

// =====================================================================
// 5 agent 静态元数据（来自 v3 §6.2 + secrets-management.md + key-naming-convention.md）
// =====================================================================

struct AgentMeta {
    id: &'static str,
    name: &'static str,
    /// 进程名子串（小写匹配）
    process_match: &'static [&'static str],
    /// Win 侧相对 USERPROFILE 的配置路径；如 ".claude\\settings.json"
    config_relpath: Option<&'static str>,
    /// 启动主命令：
    /// - 若 `wsl_distro = None`：通过 agent-env.ps1 子命令启动（`agent-env claude`）
    /// - 若 `wsl_distro = Some(...)`：直接作为 WSL 内命令（`wsl -d Ubuntu -- hermes`）
    launch_subcommand: Option<&'static str>,
    /// WSL 发行版名（如 "Ubuntu"）；None 表示 Win 原生 agent
    wsl_distro: Option<&'static str>,
    /// WSL 内绝对 Linux 路径（如 "/home/homebase/.hermes/config.yaml"）；
    /// agent_info 会把它转成 `\\wsl.localhost\<distro>\...` UNC 形式供 Win 侧打开
    wsl_config_path: Option<&'static str>,
}

const AGENTS: &[AgentMeta] = &[
    AgentMeta {
        id: "claude-code",
        name: "Claude Code",
        process_match: &["claude"],
        config_relpath: Some(".claude\\settings.json"),
        launch_subcommand: Some("claude"),
        wsl_distro: None,
        wsl_config_path: None,
    },
    AgentMeta {
        id: "codex",
        name: "Codex CLI",
        process_match: &["codex"],
        config_relpath: Some(".codex\\config.toml"),
        launch_subcommand: Some("codex"),
        wsl_distro: None,
        wsl_config_path: None,
    },
    AgentMeta {
        id: "gemini-cli",
        name: "Gemini CLI",
        process_match: &["gemini"],
        config_relpath: Some(".gemini\\settings.json"),
        launch_subcommand: Some("gemini"),
        wsl_distro: None,
        wsl_config_path: None,
    },
    AgentMeta {
        id: "openclaw",
        name: "OpenClaw",
        process_match: &["openclaw"],
        config_relpath: None, // OpenClaw 配置在项目级 .openclaw/，没有全局 home 配置
        launch_subcommand: Some("openclaw"), // agent-env.ps1 `& openclaw` → C:\Users\homebase\AppData\Roaming\npm-global\openclaw.cmd
        wsl_distro: None,
        wsl_config_path: None,
    },
    AgentMeta {
        id: "hermes",
        name: "Hermes Agent",
        process_match: &["hermes"],
        config_relpath: None,              // Win 侧无配置，改走 wsl_config_path
        launch_subcommand: Some("hermes"), // wsl -d Ubuntu -- hermes
        wsl_distro: Some("Ubuntu"),
        wsl_config_path: Some("/home/homebase/.hermes/config.yaml"),
    },
];

fn env_home() -> String {
    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\homebase".to_string())
}

fn agent_info(meta: &AgentMeta) -> AgentInfo {
    let home = env_home();
    // 优先级：wsl_config_path（WSL UNC）> config_relpath（Win 本地）> None
    let config_path =
        if let (Some(distro), Some(wsl_path)) = (meta.wsl_distro, meta.wsl_config_path) {
            // Linux 绝对路径转 Win UNC：/home/homebase/.hermes/config.yaml
            //   → \\wsl.localhost\Ubuntu\home\homebase\.hermes\config.yaml
            let win_path = wsl_path.replace('/', "\\");
            Some(format!("\\\\wsl.localhost\\{distro}{win_path}"))
        } else {
            meta.config_relpath.map(|rel| format!("{home}\\{rel}"))
        };
    let config_exists = config_path
        .as_ref()
        .map(|p| Path::new(p).exists())
        .unwrap_or(false);

    AgentInfo {
        id: meta.id.to_string(),
        name: meta.name.to_string(),
        kind: "cli".to_string(),
        config_path,
        config_exists,
        launch_subcommand: meta.launch_subcommand.map(|s| s.to_string()),
        process_match: meta.process_match.iter().map(|s| s.to_string()).collect(),
    }
}

/// 用 sysinfo 找匹配进程；返回首个命中的 PID。
///
/// OpenClaw 是 npm shim，真实进程常是 `node ...openclaw.mjs`；
/// Hermes 跑在 WSL 内，Windows 侧常表现为 `wsl.exe ... hermes`。
/// 因此不能只看 process name，也必须扫 command line。
fn find_running_pid(sys: &System, process_keywords: &[&str]) -> Option<u32> {
    sys.processes().iter().find_map(|(pid, proc)| {
        let name_lower = proc.name().to_string_lossy().to_lowercase();
        let cmd_lower = proc
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if process_keywords
            .iter()
            .any(|kw| name_lower.contains(*kw) || cmd_lower.contains(*kw))
        {
            Some(pid.as_u32())
        } else {
            None
        }
    })
}

// =====================================================================
// Tauri commands
// =====================================================================

/// 列出 5 个内置 agent + 静态元数据 + 配置文件存在性
#[tauri::command]
pub async fn agents_list() -> Result<Vec<AgentInfo>, String> {
    tracing::debug!("agents_list 调用");
    Ok(AGENTS.iter().map(agent_info).collect())
}

/// 单个 agent 实时状态：进程检测 + 配置检查 → AgentState 语义化结果
// Retain only matching PIDs, never process command lines. Concurrent per-agent
// queries share one scan; the short TTL does not cache config or failures.
type AgentPidSnapshot = Option<(Instant, Vec<Option<u32>>)>;

fn cached_agent_pids(
    snapshot: &Mutex<AgentPidSnapshot>,
    scan: impl FnOnce() -> Result<Vec<Option<u32>>, String>,
) -> Result<Vec<Option<u32>>, String> {
    let mut snapshot = snapshot.lock()
        .map_err(|_| "QUERY_FAILED: 代理进程快照锁不可用".to_string())?;
    if let Some((at, pids)) = snapshot.as_ref() {
        if at.elapsed() < Duration::from_secs(1) {
            return Ok(pids.clone());
        }
    }
    let pids = scan()?;
    *snapshot = Some((Instant::now(), pids.clone()));
    Ok(pids)
}

fn running_agent_pids() -> Result<Vec<Option<u32>>, String> {
    static SNAPSHOT: OnceLock<Mutex<AgentPidSnapshot>> = OnceLock::new();
    cached_agent_pids(SNAPSHOT.get_or_init(|| Mutex::new(None)), || {
        let mut sys = System::new();
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
        if sys.processes().is_empty() {
            return Err("QUERY_FAILED: 进程清单不可用，无法判定代理是否停止".into());
        }
        Ok(AGENTS.iter().map(|meta| find_running_pid(&sys, meta.process_match)).collect())
    })
}

#[tauri::command]
pub async fn agents_status(id: String) -> Result<AgentStatus, String> {
    tracing::debug!("agents_status 调用: {}", id);
    let Some(meta) = AGENTS.iter().find(|m| m.id == id) else {
        return Err(format!("未知 agent id: {id}"));
    };
    let info = agent_info(meta);

    let pids = tokio::task::spawn_blocking(running_agent_pids).await
        .map_err(|_| "QUERY_FAILED: 代理进程扫描任务失败".to_string())??;
    if let Some(path) = info.config_path.as_deref() {
        if let Err(error) = std::fs::metadata(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err("QUERY_FAILED: 代理配置元数据不可读取，不能判定为未配置".into());
            }
        }
    }
    let pid = pids[AGENTS.iter().position(|agent| agent.id == id).expect("validated agent")];
    let running = pid.is_some();

    // 状态判定
    let state = match (running, info.config_exists, meta.config_relpath.is_some()) {
        (true, _, _) => AgentState::Running,
        (false, true, _) => AgentState::Stopped,
        // 配置 relpath 是 Some 但文件不存在 → 装了但未配
        (false, false, true) => AgentState::Misconfigured,
        // 配置 relpath 是 None（如 OpenClaw / Hermes）→ 看进程；进程没跑就当 stopped
        (false, false, false) => AgentState::Stopped,
    };

    Ok(AgentStatus {
        id: id.clone(),
        state,
        running,
        pid,
        config_exists: info.config_exists,
        config_path: info.config_path,
        mcp_servers: vec![], // Phase 13.2 加 MCP 健康检查
        last_seen: Some(chrono::Utc::now().to_rfc3339()),
        error: None,
    })
}

// =====================================================================
// agents_start — 一键启动（在新 PowerShell 窗口跑 agent-env <subcommand>）
// =====================================================================
//
// 设计要点：
// - 不用 AgentEnvAdapter（那是 daemon 模式，捕获 stdout）；agent 是交互式 REPL，
//   必须给用户一个**真实终端窗口**。
// - Windows 用 `cmd /c start "" powershell -NoExit -Command "agent-env <name>"`：
//   start 会脱离 cmd 进程组、新建窗口；HomeBase 这边 spawn 立即返回。
// - 启动失败（agent-env 不在 PATH 等）会在新窗口显示，HomeBase 通过下次 poll
//   检测 PID 是否出现间接判断；这里只保证 spawn 本身不出错。

#[tauri::command]
pub async fn agents_start(id: String) -> Result<(), String> {
    tracing::debug!("agents_start 调用: {}", id);
    let Some(meta) = AGENTS.iter().find(|m| m.id == id) else {
        return Err(format!("未知 agent id: {id}"));
    };
    let Some(subcommand) = meta.launch_subcommand else {
        return Err(format!(
            "agent {id} 不支持一键启动（无 launch_subcommand，请手动启动）"
        ));
    };

    // 根据 wsl_distro 决定走哪条路径：
    // - WSL agent: powershell -Command "wsl -d <distro> -- bash -lc '<subcommand>'"
    //   bash -lc 启动 login shell，会 source ~/.profile / ~/.bashrc，把 ~/.local/bin
    //   等加进 PATH（否则 wsl --command 默认 non-login shell，PATH 不全，
    //   会出现 `bash: hermes: command not found`）
    // - Win agent: powershell -Command "agent-env <subcommand>"
    let ps_command = if let Some(distro) = meta.wsl_distro {
        format!("wsl -d {distro} -- bash -lc '{subcommand}'")
    } else {
        format!("& 'C:\\HomeBase\\Tools\\bin\\agent-env.ps1' {subcommand}")
    };

    let mut cmd = std::process::Command::new("cmd");
    cmd.args([
        "/c",
        "start",
        "",
        "powershell",
        "-NoExit",
        "-NoProfile",
        "-Command",
        &ps_command,
    ]);
    // 隐藏中间 cmd.exe 闪烁；start 启动的目标 powershell 窗口不受影响
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let result = cmd.spawn();

    match result {
        Ok(_) => {
            tracing::info!("agents_start: spawn cmd /c start powershell -Command \"{ps_command}\"");
            Ok(())
        }
        Err(e) => Err(format!("启动失败: {e}")),
    }
}

// =====================================================================
// agents_open_config — 用 OS 默认编辑器打开 agent 配置文件
// =====================================================================

#[tauri::command]
pub async fn agents_open_config(id: String) -> Result<(), String> {
    tracing::debug!("agents_open_config 调用: {}", id);
    let Some(meta) = AGENTS.iter().find(|m| m.id == id) else {
        return Err(format!("未知 agent id: {id}"));
    };
    let info = agent_info(meta);
    let Some(path) = info.config_path else {
        return Err(format!("agent {id} 无独立配置文件（如 OpenClaw / Hermes）"));
    };
    if !Path::new(&path).exists() {
        return Err(format!("配置文件不存在：{path}（请先初始化 agent）"));
    }

    // cmd /c start "" "<path>" — OS 选 default handler 打开
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", &path]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.spawn().map_err(|e| format!("打开失败: {e}"))?;
    tracing::info!("agents_open_config: opened {path}");
    Ok(())
}

// =====================================================================
// agents_get_model / agents_set_model / agents_list_models
// （Phase 1 · 详情面板 · 模型 tab）
// =====================================================================
//
// 适配 3 个 backend：
//   - claude-code : ~/.claude/settings.json 顶层 `model` 字段（JSON）
//   - codex       : ~/.codex/config.toml 顶层 `model` 字段（TOML）
//   - gemini-cli  : ~/.gemini/settings.json 顶层 `model` 字段（JSON）
//
// openclaw / hermes 暂不支持（无标准模型字段 / WSL 跨边界），返回 unsupported

#[derive(Debug, Serialize)]
pub struct AgentModelInfo {
    pub agent_id: String,
    /// 当前模型；config 文件不存在或字段缺失时 None
    pub current: Option<String>,
    /// 该 backend 支持的预设模型选项（hard-coded 表）
    pub presets: Vec<ModelPreset>,
    /// 是否支持模型修改（false = openclaw/hermes 等 disabled）
    pub supported: bool,
    /// 不支持时的原因（hint）
    pub unsupported_reason: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ModelPreset {
    /// 实际写入 config 的字符串值
    pub value: String,
    /// UI 显示名（如 "Sonnet 4.5"）
    pub label: String,
    /// 一句话特征（如 "默认 · 平衡 · 推荐"）
    pub hint: String,
}

/// Claude Code 的 alias 预设（来源：code.claude.com/docs/en/model-config）
///
/// 写入 `~/.claude/settings.json` 顶层 `"model"` 字段。
/// alias 方式比具体 ID 鲁棒（Anthropic 发新版时会自动迁移）。
fn claude_code_presets() -> Vec<ModelPreset> {
    vec![
        ModelPreset {
            value: "default".into(),
            label: "Default".into(),
            hint: "跟随订阅 plan（Max→Opus 4.7 / Pro→Sonnet 4.6）".into(),
        },
        ModelPreset {
            value: "opusplan".into(),
            label: "Opus + Plan".into(),
            hint: "★ plan 模式 Opus 思考 / execute 自动切 Sonnet · 省 token".into(),
        },
        ModelPreset {
            value: "opus".into(),
            label: "Opus".into(),
            hint: "旗舰 · 最强 reasoning · 慢 / 贵".into(),
        },
        ModelPreset {
            value: "sonnet".into(),
            label: "Sonnet".into(),
            hint: "平衡 · 日常推荐".into(),
        },
        ModelPreset {
            value: "haiku".into(),
            label: "Haiku".into(),
            hint: "最快 · 适合短任务 / 自动化".into(),
        },
        ModelPreset {
            value: "opus[1m]".into(),
            label: "Opus (1M ctx)".into(),
            hint: "1M token 上下文（按 extra usage 计费）".into(),
        },
        ModelPreset {
            value: "sonnet[1m]".into(),
            label: "Sonnet (1M ctx)".into(),
            hint: "1M token 上下文 · 大型仓库分析".into(),
        },
        ModelPreset {
            value: "best".into(),
            label: "Best".into(),
            hint: "= Opus，跟 alias 绑定".into(),
        },
    ]
}

fn codex_presets() -> Vec<ModelPreset> {
    vec![
        ModelPreset {
            value: "gpt-5".into(),
            label: "GPT-5".into(),
            hint: "默认 · 通用".into(),
        },
        ModelPreset {
            value: "gpt-5-codex".into(),
            label: "GPT-5 Codex".into(),
            hint: "代码专精 · 推荐".into(),
        },
        ModelPreset {
            value: "o3".into(),
            label: "o3".into(),
            hint: "强推理 · 慢".into(),
        },
        ModelPreset {
            value: "o4-mini".into(),
            label: "o4-mini".into(),
            hint: "快 / 便宜".into(),
        },
    ]
}

fn gemini_presets() -> Vec<ModelPreset> {
    vec![
        ModelPreset {
            value: "gemini-2.5-pro".into(),
            label: "Gemini 2.5 Pro".into(),
            hint: "默认 · 长上下文 · 推荐".into(),
        },
        ModelPreset {
            value: "gemini-2.5-flash".into(),
            label: "Gemini 2.5 Flash".into(),
            hint: "快 / 便宜".into(),
        },
    ]
}

#[tauri::command]
pub async fn agents_get_model(id: String) -> Result<AgentModelInfo, String> {
    tracing::debug!("agents_get_model 调用: {}", id);

    let (supported, presets, unsupported_reason): (bool, Vec<ModelPreset>, Option<String>) =
        match id.as_str() {
            "claude-code" => (true, claude_code_presets(), None),
            "codex" => (true, codex_presets(), None),
            "gemini-cli" => (true, gemini_presets(), None),
            "openclaw" => (
                false,
                vec![],
                Some("OpenClaw 无标准模型字段，模型由项目级配置决定".into()),
            ),
            "hermes" => (
                false,
                vec![],
                Some("Hermes 在 WSL 内 · 跨边界改 yaml 风险大 · v1 跳过".into()),
            ),
            _ => return Err(format!("未知 agent id: {id}")),
        };

    let current = if supported {
        read_agent_model(&id).unwrap_or(None)
    } else {
        None
    };

    Ok(AgentModelInfo {
        agent_id: id,
        current,
        presets,
        supported,
        unsupported_reason,
    })
}

#[tauri::command]
pub async fn agents_set_model(id: String, model: String) -> Result<AgentModelInfo, String> {
    tracing::debug!("agents_set_model 调用: {} → {}", id, model);

    match id.as_str() {
        "claude-code" | "codex" | "gemini-cli" => {}
        "openclaw" | "hermes" => {
            return Err(format!("agent {id} 不支持模型修改"));
        }
        _ => return Err(format!("未知 agent id: {id}")),
    }

    write_agent_model(&id, &model)?;
    agents_get_model(id).await
}

/// 读对应 backend 配置文件中的 model 字段
fn read_agent_model(id: &str) -> Result<Option<String>, String> {
    let meta = AGENTS
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("未知 agent id: {id}"))?;
    let info = agent_info(meta);
    let Some(path) = info.config_path else {
        return Ok(None);
    };
    if !Path::new(&path).exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败：{e}"))?;

    match id {
        "claude-code" | "gemini-cli" => {
            // JSON：{"model": "..."}
            let v: serde_json::Value = serde_json::from_str(strip_bom(&raw))
                .map_err(|e| format!("解析 JSON 失败：{e}"))?;
            Ok(v.get("model")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string()))
        }
        "codex" => {
            // TOML：model = "..."
            let v: toml::Value =
                toml::from_str(strip_bom(&raw)).map_err(|e| format!("解析 TOML 失败：{e}"))?;
            Ok(v.get("model")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string()))
        }
        _ => Ok(None),
    }
}

/// 把 model 字段写回 backend 配置文件（保留其他字段）
fn write_agent_model(id: &str, model: &str) -> Result<(), String> {
    let meta = AGENTS
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| format!("未知 agent id: {id}"))?;
    let info = agent_info(meta);
    let path = info
        .config_path
        .ok_or_else(|| format!("agent {id} 无配置文件路径"))?;

    // 配置文件不存在 → 创建父目录并初始化最小配置
    let path_obj = Path::new(&path);
    if let Some(parent) = path_obj.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建配置目录 {} 失败：{e}", parent.display()))?;
    }

    match id {
        "claude-code" | "gemini-cli" => {
            // 读现有 JSON（不存在则 {}），改 model，写回（pretty）
            let mut v: serde_json::Value = if path_obj.exists() {
                let raw =
                    std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败：{e}"))?;
                serde_json::from_str(strip_bom(&raw)).map_err(|e| format!("解析 JSON 失败：{e}"))?
            } else {
                serde_json::json!({})
            };
            if !v.is_object() {
                return Err(format!("配置 {path} 顶层不是对象"));
            }
            v["model"] = serde_json::Value::String(model.to_string());
            let pretty =
                serde_json::to_string_pretty(&v).map_err(|e| format!("序列化 JSON 失败：{e}"))?;
            std::fs::write(&path, pretty).map_err(|e| format!("写入 {path} 失败：{e}"))?;
        }
        "codex" => {
            // 读现有 TOML（不存在则空），改 model，写回
            let mut v: toml::Value = if path_obj.exists() {
                let raw =
                    std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败：{e}"))?;
                toml::from_str(strip_bom(&raw)).map_err(|e| format!("解析 TOML 失败：{e}"))?
            } else {
                toml::Value::Table(toml::map::Map::new())
            };
            let table = v
                .as_table_mut()
                .ok_or_else(|| format!("配置 {path} 顶层不是 table"))?;
            table.insert("model".into(), toml::Value::String(model.to_string()));
            let serialized =
                toml::to_string_pretty(&v).map_err(|e| format!("序列化 TOML 失败：{e}"))?;
            std::fs::write(&path, serialized).map_err(|e| format!("写入 {path} 失败：{e}"))?;
        }
        _ => return Err(format!("agent {id} 不支持模型修改")),
    }

    tracing::info!("agents_set_model: {id} → {model} 写入 {path}");
    Ok(())
}

// =====================================================================
// Endpoint 配置（Phase F · 2026-05-05 重构 — SOPS 引用化）
// =====================================================================
//
// Claude Code 的 endpoint 由环境变量控制（非 model 字段）：
//   - ANTHROPIC_BASE_URL          API 端点（OpenRouter: https://openrouter.ai/api/v1）
//   - ANTHROPIC_AUTH_TOKEN        Bearer token（OpenRouter key 或 Anthropic key）
//   - ANTHROPIC_DEFAULT_OPUS_MODEL    alias → 具体 model ID 映射
//   - ANTHROPIC_DEFAULT_SONNET_MODEL
//   - ANTHROPIC_DEFAULT_HAIKU_MODEL
//
// HomeBase 写到 ~/.claude/settings.json 的 `env` 块（Claude Code 启动时自动注入子进程）。
// **不污染 system env vars**，rotate 时只改一处。
//
// **Phase F 安全模型**（2026-05-05）：
// - schema 不再持有明文 token，仅持 secret_short_name 引用 SOPS
// - set / test 路径：reveal SOPS → 临时持有 → 写 settings.json / HTTP 请求 → drop（zeroize）
// - read 路径：读 settings.json 现有 token → 反查 SOPS 看是哪个 secret →
//   命中 = secret_short_name=Some(...); 没命中 = external_token_detected=true（提示用户导入）
// - 磁盘上 settings.json 仍是明文（Claude Code 子进程读取的硬约束）；
//   能保证的是 UI 永不展示 / 接收明文 + 单点 SOPS 单源轮换
//
// 详 ../../../../C:/Users/homebase/tools/key-naming-convention.md
// §Claude Code via OpenRouter 桥接

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct EndpointConfig {
    /// "anthropic-direct" / "openrouter" / "custom" / "unset"
    pub provider: String,
    pub base_url: Option<String>,
    /// SOPS secret 引用（短名）— 前端只见到这个；明文不进 schema
    pub secret_short_name: Option<String>,
    /// 写入 settings.json env 块时使用的变量名
    /// "ANTHROPIC_AUTH_TOKEN"（OpenRouter）/ "ANTHROPIC_API_KEY"（Anthropic 直连）
    pub auth_token_var: String,
    pub default_opus: Option<String>,
    pub default_sonnet: Option<String>,
    pub default_haiku: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentEndpointInfo {
    pub agent_id: String,
    pub supported: bool,
    pub unsupported_reason: Option<String>,
    pub current: EndpointConfig,
    /// settings.json 中检测到的 token 是 SOPS 之外的（用户手动粘贴 / 旧值残留）。
    /// 前端应展示警告横幅引导用户改成下拉选择 SOPS secret。
    pub external_token_detected: bool,
    /// 当前 settings.json 中 token 的 masked 预览（external 时展示 "abc...****...xyz"，
    /// SOPS 命中时统一显示 "(via SOPS)"，无值时为空字符串）。
    pub current_token_masked: String,
    /// 内嵌的预设清单（OpenRouter Anthropic 14 个 model ID）
    pub presets: EndpointPresets,
    /// 可用的 LLM secret 列表（前端 dropdown 用 — 已 filter 掉非 LLM 类）
    pub available_secrets: Vec<AvailableSecret>,
}

/// 单个 LLM-capable secret 的前端展示元数据（明文不在此 struct）
#[derive(Debug, Serialize, Clone)]
pub struct AvailableSecret {
    pub short_name: String,
    /// 用户友好显示名（如 "OpenRouter（推荐）"）
    pub display_label: String,
    /// 推荐 provider："openrouter" / "anthropic-direct" / "custom"
    pub suggested_provider: String,
    /// 推荐写入的 env 变量名
    pub suggested_token_var: String,
    pub suggested_base_url: Option<String>,
    /// 是否在 SOPS 中实际存在（false = 注册了但 yaml 里没值）
    pub present: bool,
}

/// LLM-capable secrets 白名单（与 secrets::meta_for 中 Core/FutureReady 的 LLM 类对齐）
///
/// 字段：(short_name, display_label, suggested_provider, suggested_token_var, suggested_base_url)
const LLM_SECRET_REGISTRY: &[(&str, &str, &str, &str, Option<&str>)] = &[
    (
        "openrouter_key",
        "OpenRouter（推荐）",
        "openrouter",
        "ANTHROPIC_AUTH_TOKEN",
        Some("https://openrouter.ai/api/v1"),
    ),
    (
        "anthropic_key",
        "Anthropic 直连",
        "anthropic-direct",
        "ANTHROPIC_API_KEY",
        Some("https://api.anthropic.com"),
    ),
    (
        "openai_key",
        "OpenAI（作为 gateway）",
        "custom",
        "ANTHROPIC_AUTH_TOKEN",
        None,
    ),
    (
        "gemini_key",
        "Gemini（作为 gateway）",
        "custom",
        "ANTHROPIC_AUTH_TOKEN",
        None,
    ),
    (
        "deepseek_key",
        "DeepSeek（作为 gateway）",
        "custom",
        "ANTHROPIC_AUTH_TOKEN",
        None,
    ),
    (
        "tryallai_key",
        "TryAllAI（备用）",
        "custom",
        "ANTHROPIC_AUTH_TOKEN",
        None,
    ),
];

#[derive(Debug, Serialize)]
pub struct EndpointPresets {
    pub openrouter_anthropic: Vec<ModelIdEntry>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ModelIdEntry {
    /// "anthropic/claude-opus-4.7"
    pub id: String,
    /// UI 显示名（如 "Opus 4.7（最新）"）
    pub label: String,
    /// "1M" / "200K"
    pub context: String,
    /// "$5 / $25 per M"
    pub pricing: String,
    /// "opus" / "sonnet" / "haiku"
    pub tier: String,
}

#[derive(Debug, Serialize)]
pub struct EndpointTestResult {
    pub ok: bool,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<String>,
    /// 用于显示返回的具体内容（前 200 字）
    pub body_preview: Option<String>,
}

/// OpenRouter Anthropic 模型 ID 内嵌快照（2026-05-04 实测）
///
/// 来源：`Invoke-RestMethod 'https://openrouter.ai/api/v1/models'`
fn openrouter_anthropic_presets() -> Vec<ModelIdEntry> {
    use ModelIdEntry as E;
    vec![
        // Opus
        E {
            id: "anthropic/claude-opus-4.7".into(),
            label: "Opus 4.7（最新）".into(),
            context: "1M".into(),
            pricing: "$5 / $25".into(),
            tier: "opus".into(),
        },
        E {
            id: "anthropic/claude-opus-4.6".into(),
            label: "Opus 4.6".into(),
            context: "1M".into(),
            pricing: "$5 / $25".into(),
            tier: "opus".into(),
        },
        E {
            id: "anthropic/claude-opus-4.6-fast".into(),
            label: "Opus 4.6 Fast（贵 6x）".into(),
            context: "1M".into(),
            pricing: "$30 / $150".into(),
            tier: "opus".into(),
        },
        E {
            id: "anthropic/claude-opus-4.5".into(),
            label: "Opus 4.5".into(),
            context: "200K".into(),
            pricing: "$5 / $25".into(),
            tier: "opus".into(),
        },
        E {
            id: "anthropic/claude-opus-4.1".into(),
            label: "Opus 4.1（旧版）".into(),
            context: "200K".into(),
            pricing: "$15 / $75".into(),
            tier: "opus".into(),
        },
        E {
            id: "anthropic/claude-opus-4".into(),
            label: "Opus 4（旧版）".into(),
            context: "200K".into(),
            pricing: "$15 / $75".into(),
            tier: "opus".into(),
        },
        // Sonnet
        E {
            id: "anthropic/claude-sonnet-4.6".into(),
            label: "Sonnet 4.6（最新）".into(),
            context: "1M".into(),
            pricing: "$3 / $15".into(),
            tier: "sonnet".into(),
        },
        E {
            id: "anthropic/claude-sonnet-4.5".into(),
            label: "Sonnet 4.5".into(),
            context: "1M".into(),
            pricing: "$3 / $15".into(),
            tier: "sonnet".into(),
        },
        E {
            id: "anthropic/claude-sonnet-4".into(),
            label: "Sonnet 4".into(),
            context: "1M".into(),
            pricing: "$3 / $15".into(),
            tier: "sonnet".into(),
        },
        E {
            id: "anthropic/claude-3.7-sonnet".into(),
            label: "Sonnet 3.7".into(),
            context: "200K".into(),
            pricing: "$3 / $15".into(),
            tier: "sonnet".into(),
        },
        E {
            id: "anthropic/claude-3.7-sonnet:thinking".into(),
            label: "Sonnet 3.7 Thinking".into(),
            context: "200K".into(),
            pricing: "$3 / $15".into(),
            tier: "sonnet".into(),
        },
        // Haiku
        E {
            id: "anthropic/claude-haiku-4.5".into(),
            label: "Haiku 4.5（最新）".into(),
            context: "200K".into(),
            pricing: "$1 / $5".into(),
            tier: "haiku".into(),
        },
        E {
            id: "anthropic/claude-3.5-haiku".into(),
            label: "Haiku 3.5".into(),
            context: "200K".into(),
            pricing: "$0.8 / $4".into(),
            tier: "haiku".into(),
        },
        E {
            id: "anthropic/claude-3-haiku".into(),
            label: "Haiku 3（最便宜）".into(),
            context: "200K".into(),
            pricing: "$0.25 / $1.25".into(),
            tier: "haiku".into(),
        },
    ]
}

/// 推断当前 endpoint 走的 provider
fn infer_provider(base_url: &Option<String>) -> &'static str {
    match base_url.as_deref() {
        None => "unset",
        Some(u) if u.contains("openrouter.ai") => "openrouter",
        Some(u) if u.contains("api.anthropic.com") => "anthropic-direct",
        Some(_) => "custom",
    }
}

/// 把明文 token 做 short masked（用于 external_token_detected 时给前端展示）
fn short_mask(value: &str) -> String {
    let total = value.chars().count();
    if total < 16 {
        return "********".to_string();
    }
    let prefix: String = value.chars().take(7).collect();
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}...****...{suffix}")
}

/// 解密 SOPS yaml，按 token 字符串反查 short_name；找不到返回 None
///
/// **明文仅在此函数内短暂存在**，函数返回前 SecureString drop → zeroize。
async fn lookup_secret_by_value(token: &str) -> Result<Option<String>, String> {
    let adapter = SopsAdapter::new();
    let yaml_path = secrets_yaml_path();
    let secure = adapter
        .decrypt_file(&yaml_path)
        .await
        .map_err(|e| format!("sops decrypt 失败: {e}"))?;
    let parsed: serde_yaml::Value =
        serde_yaml::from_str(secure.as_str()).map_err(|e| format!("YAML 解析失败: {e}"))?;
    // SecureString 在此 drop → zeroize

    let mapping = match parsed {
        serde_yaml::Value::Mapping(m) => m,
        _ => return Ok(None),
    };
    for (k, v) in mapping {
        let Some(short) = k.as_str() else { continue };
        if short == "sops" {
            continue;
        }
        let Some(value) = v.as_str() else { continue };
        if value == token {
            return Ok(Some(short.to_string()));
        }
    }
    Ok(None)
}

/// 读 ~/.claude/settings.json 的 env 块 + 顶层 model 字段，组装 EndpointConfig
///
/// 同时反查 SOPS 看现有 token 是否对应已注册的 secret：
/// - 命中 → secret_short_name=Some("openrouter_key"), external_token_detected=false
/// - 没命中 + token 存在 → secret_short_name=None, external_token_detected=true
/// - 没 token → secret_short_name=None, external_token_detected=false
async fn read_claude_endpoint() -> Result<(EndpointConfig, bool, String), String> {
    use serde_json::Value;
    let path = format!("{}/.claude/settings.json", env_home());
    let path_obj = Path::new(&path);

    if !path_obj.exists() {
        return Ok((EndpointConfig::default(), false, String::new()));
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败：{e}"))?;
    let v: Value =
        serde_json::from_str(strip_bom(&raw)).map_err(|e| format!("解析 JSON 失败：{e}"))?;

    let env = v.get("env").and_then(|x| x.as_object());
    let get_str = |k: &str| -> Option<String> {
        env.and_then(|m| m.get(k))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
    };

    let base_url = get_str("ANTHROPIC_BASE_URL");
    let provider = infer_provider(&base_url).to_string();

    // ---- Phase G 优先级 ----
    // 1. settings.json 顶层 apiKeyHelper 存在 → SOPS helper 模式（最优）
    // 2. env.ANTHROPIC_AUTH_TOKEN / API_KEY 存在 → legacy / external token（向后兼容）
    // 3. 都没 → OAuth / 未配置
    let helper_short = v
        .get("apiKeyHelper")
        .and_then(|x| x.as_str())
        .and_then(parse_short_name_from_helper);

    let env_token = get_str("ANTHROPIC_AUTH_TOKEN").or_else(|| get_str("ANTHROPIC_API_KEY"));

    let auth_token_var = if v
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_API_KEY"))
        .is_some()
    {
        "ANTHROPIC_API_KEY".to_string()
    } else if v
        .get("env")
        .and_then(|e| e.get("ANTHROPIC_AUTH_TOKEN"))
        .is_some()
    {
        "ANTHROPIC_AUTH_TOKEN".to_string()
    } else {
        match provider.as_str() {
            "anthropic-direct" => "ANTHROPIC_API_KEY".to_string(),
            _ => "ANTHROPIC_AUTH_TOKEN".to_string(),
        }
    };

    let (secret_short_name, external_detected, masked) = match (&helper_short, &env_token) {
        // helper 优先：标记 SOPS 模式，不反查（helper 自己保证 token 来自 SOPS）
        (Some(short), _) => (
            Some(short.clone()),
            false,
            format!("(via apiKeyHelper · SOPS:{short})"),
        ),
        // 没有 helper 但有 env token → 走 legacy 反查 SOPS
        (None, Some(t)) if !t.trim().is_empty() => match lookup_secret_by_value(t).await {
            Ok(Some(short)) => (
                Some(short),
                false,
                "(via env · SOPS-managed legacy)".to_string(),
            ),
            Ok(None) => (None, true, short_mask(t)),
            Err(e) => {
                tracing::warn!("SOPS 反查失败（视作 external）: {e}");
                (None, true, short_mask(t))
            }
        },
        // 啥都没 → OAuth / 未配置
        _ => (None, false, String::new()),
    };

    Ok((
        EndpointConfig {
            provider,
            base_url,
            secret_short_name,
            auth_token_var,
            default_opus: get_str("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            default_sonnet: get_str("ANTHROPIC_DEFAULT_SONNET_MODEL"),
            default_haiku: get_str("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        },
        external_detected,
        masked,
    ))
}

// =====================================================================
// Phase G-4+5 · Gemini CLI / OpenClaw / Hermes — 走 agent-env.ps1 wrapper 注入
//
// 这 3 个 agent 没有原生 helper script 字段，HomeBase 只能：
// 1. 存用户偏好（哪个 SOPS short_name 映射到哪个 env var）到 ~/.homebase/agent-endpoints.json
// 2. UI 暴露读写
// 3. 实际生效仍由 C:\HomeBase\Tools\bin\agent-env.ps1 完成（已在 SOPS-managed wrapper 模式）
// =====================================================================

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
struct SimpleEndpointEntry {
    secret_short_name: Option<String>,
    auth_token_var: String,
    base_url: Option<String>,
}

fn agent_endpoints_json_path() -> Result<PathBuf, String> {
    let dir = homebase_dir()?;
    Ok(dir.join("agent-endpoints.json"))
}

/// 从 ~/.homebase/agent-endpoints.json 读单个 agent endpoint
fn read_simple_endpoint(agent_id: &str) -> Result<(EndpointConfig, bool, String), String> {
    let path = agent_endpoints_json_path()?;
    if !path.exists() {
        // 文件不存在 → 返回该 agent 的 default 推荐
        return Ok((default_simple_endpoint(agent_id), false, String::new()));
    }
    let raw =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    let map: std::collections::HashMap<String, SimpleEndpointEntry> =
        serde_json::from_str(strip_bom(&raw)).map_err(|e| format!("解析 JSON 失败: {e}"))?;
    let entry = map.get(agent_id).cloned().unwrap_or_default();
    let mut cfg = EndpointConfig {
        provider: infer_provider(&entry.base_url).to_string(),
        base_url: entry.base_url,
        secret_short_name: entry.secret_short_name.clone(),
        auth_token_var: if entry.auth_token_var.is_empty() {
            default_simple_endpoint(agent_id).auth_token_var
        } else {
            entry.auth_token_var
        },
        ..Default::default()
    };
    if cfg.provider == "unset" {
        cfg.provider = default_simple_endpoint(agent_id).provider;
    }
    let masked = if entry.secret_short_name.is_some() {
        format!(
            "(via agent-env.ps1 wrapper · SOPS:{})",
            cfg.secret_short_name.as_deref().unwrap_or("")
        )
    } else {
        String::new()
    };
    Ok((cfg, false, masked))
}

/// 写 ~/.homebase/agent-endpoints.json 的单个 agent 条目
fn write_simple_endpoint(agent_id: &str, cfg: &EndpointConfig) -> Result<(), String> {
    let path = agent_endpoints_json_path()?;
    let mut map: std::collections::HashMap<String, SimpleEndpointEntry> = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
        serde_json::from_str(strip_bom(&raw)).map_err(|e| format!("解析 JSON 失败: {e}"))?
    } else {
        std::collections::HashMap::new()
    };
    let entry = SimpleEndpointEntry {
        secret_short_name: cfg.secret_short_name.clone(),
        auth_token_var: cfg.auth_token_var.clone(),
        base_url: cfg.base_url.clone(),
    };
    map.insert(agent_id.to_string(), entry);
    let pretty = serde_json::to_string_pretty(&map).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, pretty).map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    tracing::info!(
        "write_simple_endpoint: {} secret={:?} 写入 {}",
        agent_id,
        cfg.secret_short_name,
        path.display()
    );
    Ok(())
}

/// 各 agent 默认推荐的 endpoint（首次访问 / 文件缺失时返回）
fn default_simple_endpoint(agent_id: &str) -> EndpointConfig {
    match agent_id {
        "gemini-cli" => EndpointConfig {
            provider: "custom".to_string(),
            secret_short_name: Some("gemini_key".to_string()),
            auth_token_var: "GEMINI_API_KEY".to_string(),
            base_url: None, // Gemini 默认 endpoint
            ..Default::default()
        },
        "openclaw" => EndpointConfig {
            provider: "openrouter".to_string(),
            secret_short_name: Some("openrouter_key".to_string()),
            auth_token_var: "ANTHROPIC_AUTH_TOKEN".to_string(),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
            ..Default::default()
        },
        "hermes" => EndpointConfig {
            provider: "openrouter".to_string(),
            secret_short_name: Some("openrouter_key".to_string()),
            auth_token_var: "ANTHROPIC_AUTH_TOKEN".to_string(),
            base_url: Some("https://openrouter.ai/api/v1".to_string()),
            ..Default::default()
        },
        _ => EndpointConfig::default(),
    }
}

// =====================================================================
// Phase G-3 · Codex CLI auth.command 接入
//
// Codex 配置位于 ~/.codex/config.toml；HomeBase 接管一个固定 provider id
// `homebase-sops` 段 + 顶层 model_provider 字段。其它 user-managed
// 段（[model_providers.openrouter] env-var 旧式 / [projects.<...>] 等）
// 完全保留不动。
// =====================================================================

/// HomeBase 在 Codex config.toml 里管理的 provider id（固定）
const CODEX_HOMEBASE_PROVIDER: &str = "homebase-sops";

async fn read_codex_endpoint() -> Result<(EndpointConfig, bool, String), String> {
    let path = format!("{}/.codex/config.toml", env_home());
    let path_obj = Path::new(&path);
    if !path_obj.exists() {
        return Ok((EndpointConfig::default(), false, String::new()));
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败：{e}"))?;
    let doc: toml_edit::DocumentMut = strip_bom(&raw)
        .parse()
        .map_err(|e| format!("解析 {path} 失败：{e}"))?;

    // 顶层 model_provider —— 决定走 OAuth (默认/openai) 还是 SOPS helper (homebase-sops)
    let active_provider = doc
        .get("model_provider")
        .and_then(|x| x.as_str())
        .unwrap_or("openai");

    // 默认值（OAuth 模式或 provider 未管理）
    let mut cfg = EndpointConfig {
        provider: "custom".to_string(), // Codex 上下文无强 provider 语义；前端用作展示
        ..Default::default()
    };
    cfg.auth_token_var = "(N/A · Codex 走 auth.command)".to_string();

    if active_provider != CODEX_HOMEBASE_PROVIDER {
        return Ok((cfg, false, String::new()));
    }

    // 解析 [model_providers.homebase-sops] 段
    let providers = doc.get("model_providers").and_then(|x| x.as_table());
    let hb = providers.and_then(|t| t.get(CODEX_HOMEBASE_PROVIDER).and_then(|x| x.as_table()));
    let Some(hb) = hb else {
        // 顶层指向 homebase-sops 但段不存在 — 异常状态，按 OAuth 处理
        tracing::warn!(
            "Codex config.toml: model_provider='{}' 但段 [model_providers.{}] 缺失",
            CODEX_HOMEBASE_PROVIDER,
            CODEX_HOMEBASE_PROVIDER
        );
        return Ok((cfg, false, String::new()));
    };

    cfg.base_url = hb
        .get("base_url")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    cfg.provider = infer_provider(&cfg.base_url).to_string();

    // auth.command + auth.args[0] 反推 short_name
    let auth = hb.get("auth").and_then(|x| x.as_table());
    let helper_short = auth
        .and_then(|t| t.get("args"))
        .and_then(|a| a.as_array())
        .and_then(|a| a.get(0))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // 安全检查：确认 command 指向我们自己的 reveal exe（防误识别用户手配的其他 helper）
    let command_is_homebase = auth
        .and_then(|t| t.get("command"))
        .and_then(|x| x.as_str())
        .map(|s| s.contains("homebase-reveal"))
        .unwrap_or(false);

    let masked = match (&helper_short, command_is_homebase) {
        (Some(short), true) => {
            cfg.secret_short_name = Some(short.clone());
            format!("(via auth.command · SOPS:{short})")
        }
        (Some(_), false) => "(via 第三方 helper · 非 HomeBase 管理)".to_string(),
        (None, _) => String::new(),
    };

    Ok((cfg, false, masked))
}

fn write_codex_endpoint(cfg: &EndpointConfig) -> Result<(), String> {
    use toml_edit::{value, Array, DocumentMut, Item, Table};
    let path = format!("{}/.codex/config.toml", env_home());
    let path_obj = Path::new(&path);

    if let Some(parent) = path_obj.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建配置目录 {} 失败：{e}", parent.display()))?;
    }

    let mut doc: DocumentMut = if path_obj.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败：{e}"))?;
        strip_bom(&raw)
            .parse()
            .map_err(|e| format!("解析 {path} 失败：{e}"))?
    } else {
        DocumentMut::new()
    };

    if let Some(short) = cfg.secret_short_name.as_deref() {
        // ---- SOPS 模式 ----
        let reveal_exe = homebase_reveal_exe_path()?;

        // base_url：优先 cfg.base_url，否则按白名单推导
        let base_url = cfg
            .base_url
            .clone()
            .or_else(|| {
                LLM_SECRET_REGISTRY
                    .iter()
                    .find(|(s, _, _, _, _)| *s == short)
                    .and_then(|(_, _, _, _, base)| base.map(|s| s.to_string()))
            })
            .ok_or_else(|| {
                "secret 未对应 LLM_SECRET_REGISTRY 中的 base_url 且未传 base_url".to_string()
            })?;

        // 1. 顶层 model_provider
        doc["model_provider"] = value(CODEX_HOMEBASE_PROVIDER);

        // 2. [model_providers.homebase-sops]
        if doc.get("model_providers").is_none() {
            doc["model_providers"] = Item::Table(Table::new());
        }
        let mut hb_table = Table::new();
        hb_table["name"] = value("HomeBase SOPS (auto-managed)");
        hb_table["base_url"] = value(&base_url);
        // wire_api: OpenRouter / 自定义 gateway 默认 chat（兼容性最好）；
        //           OpenAI/Anthropic 直连可用 responses，但保守默认 chat
        hb_table["wire_api"] = value("chat");

        // 3. [model_providers.homebase-sops.auth] 子表
        let mut auth_table = Table::new();
        auth_table["command"] = value(reveal_exe.to_string_lossy().replace('\\', "/"));
        let mut args_arr = Array::new();
        args_arr.push(short);
        auth_table["args"] = value(args_arr);
        auth_table["timeout_ms"] = value(5000_i64);
        auth_table["refresh_interval_ms"] = value(0_i64);
        hb_table.insert("auth", Item::Table(auth_table));

        // 标记是 HomeBase 管理（让用户人手编辑时明白）
        hb_table.decor_mut().set_prefix(
            "\n# === HomeBase Phase G · SOPS provider (auto-managed) ===\n# 不要手动编辑此段；改用 HomeBase EndpointSection。\n# 切回 OAuth：删除顶层 model_provider 行（或 EndpointSection 切到 OAuth 模式）。\n",
        );

        doc["model_providers"][CODEX_HOMEBASE_PROVIDER] = Item::Table(hb_table);
    } else {
        // ---- OAuth 模式 —— 删 model_provider 顶层 + 留 model_providers 段不动 ----
        doc.remove("model_provider");
        // 注意：故意不删 [model_providers.homebase-sops] 段，方便用户随时切回
    }

    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入 {path} 失败：{e}"))?;

    tracing::info!(
        "write_codex_endpoint: mode={} provider={} base_url={:?} 写入 {path}",
        if cfg.secret_short_name.is_some() {
            "sops-helper"
        } else {
            "oauth"
        },
        cfg.provider,
        cfg.base_url,
    );
    Ok(())
}

/// 列 LLM-capable secrets 元数据（前端 dropdown 用）
///
/// 解密一次 SOPS yaml 拿到所有现存 short_name 集合，标 present 字段；
/// 不返回任何明文。
async fn list_llm_secrets() -> Result<Vec<AvailableSecret>, String> {
    let adapter = SopsAdapter::new();
    let yaml_path = secrets_yaml_path();
    let present_set: std::collections::HashSet<String> =
        match adapter.decrypt_file(&yaml_path).await {
            Ok(secure) => {
                let parsed: serde_yaml::Value = serde_yaml::from_str(secure.as_str())
                    .map_err(|e| format!("YAML 解析失败: {e}"))?;
                // SecureString 在此 drop
                match parsed {
                    serde_yaml::Value::Mapping(m) => m
                        .into_iter()
                        .filter_map(|(k, _)| k.as_str().map(|s| s.to_string()))
                        .collect(),
                    _ => std::collections::HashSet::new(),
                }
            }
            Err(e) => {
                tracing::warn!(
                    "list_llm_secrets: SOPS 解密失败 {e}（仍返回注册表，全部 present=false）"
                );
                std::collections::HashSet::new()
            }
        };

    Ok(LLM_SECRET_REGISTRY
        .iter()
        .map(|(short, label, provider, var, base)| AvailableSecret {
            short_name: short.to_string(),
            display_label: label.to_string(),
            suggested_provider: provider.to_string(),
            suggested_token_var: var.to_string(),
            suggested_base_url: base.map(|s| s.to_string()),
            present: present_set.contains(*short),
        })
        .collect())
}

/// 写 ~/.claude/settings.json — Phase G apiKeyHelper 模式
///
/// 安全核心：**settings.json 上完全不写 token 明文**。token 通过 apiKeyHelper
/// 字段引用 ~/.homebase/reveal-<short>.cmd 中转脚本，Claude Code 启动时才 spawn
/// helper 解密。
///
/// 三种写入路径：
/// - `cfg.secret_short_name = Some(short)`：SOPS helper 模式
///   - 顶层 `apiKeyHelper` ← 中转脚本绝对路径
///   - env 块清掉 ANTHROPIC_AUTH_TOKEN / ANTHROPIC_API_KEY
/// - `cfg.secret_short_name = None`：OAuth 模式
///   - 删除 `apiKeyHelper`
///   - env 块清掉 ANTHROPIC_AUTH_TOKEN / ANTHROPIC_API_KEY
///   - Claude Code 自己走 keychain 登录（`/login` 命令）
fn write_claude_endpoint(cfg: &EndpointConfig) -> Result<(), String> {
    use serde_json::{json, Value};
    let path = format!("{}/.claude/settings.json", env_home());
    let path_obj = Path::new(&path);

    if let Some(parent) = path_obj.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建配置目录 {} 失败：{e}", parent.display()))?;
    }

    let mut v: Value = if path_obj.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| format!("读取 {path} 失败：{e}"))?;
        serde_json::from_str(strip_bom(&raw)).map_err(|e| format!("解析 JSON 失败：{e}"))?
    } else {
        json!({})
    };
    if !v.is_object() {
        return Err(format!("配置 {path} 顶层不是对象"));
    }
    let v_obj = v.as_object_mut().expect("checked above");

    // ---- 1. apiKeyHelper 字段 ----
    if let Some(short) = cfg.secret_short_name.as_deref() {
        let cmd_path = ensure_reveal_cmd_for(short)?;
        v_obj.insert(
            "apiKeyHelper".to_string(),
            Value::String(cmd_path.to_string_lossy().to_string()),
        );
    } else {
        v_obj.remove("apiKeyHelper");
    }

    // ---- 2. env 块 ----
    // 取原有 env 块（保留非 ANTHROPIC_ 前缀字段）→ 清掉旧 ANTHROPIC_ 字段 → 重写
    let mut env_map = v_obj
        .get("env")
        .and_then(|x| x.as_object())
        .cloned()
        .unwrap_or_default();
    env_map.retain(|k, _| !k.starts_with("ANTHROPIC_"));

    fn set_opt(m: &mut serde_json::Map<String, Value>, k: &str, v: &Option<String>) {
        if let Some(s) = v {
            if !s.is_empty() {
                m.insert(k.to_string(), Value::String(s.clone()));
            }
        }
    }

    set_opt(&mut env_map, "ANTHROPIC_BASE_URL", &cfg.base_url);
    set_opt(
        &mut env_map,
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        &cfg.default_opus,
    );
    set_opt(
        &mut env_map,
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        &cfg.default_sonnet,
    );
    set_opt(
        &mut env_map,
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        &cfg.default_haiku,
    );
    // ⚠️ 故意不写 ANTHROPIC_AUTH_TOKEN / ANTHROPIC_API_KEY — 由 apiKeyHelper 提供

    v_obj.insert("env".to_string(), Value::Object(env_map));

    let pretty = serde_json::to_string_pretty(&v).map_err(|e| format!("序列化 JSON 失败：{e}"))?;
    std::fs::write(&path, pretty).map_err(|e| format!("写入 {path} 失败：{e}"))?;

    tracing::info!(
        "write_claude_endpoint: mode={} provider={} base_url={:?} 写入 {path}",
        if cfg.secret_short_name.is_some() {
            "sops-helper"
        } else {
            "oauth"
        },
        cfg.provider,
        cfg.base_url,
    );
    Ok(())
}

#[tauri::command]
pub async fn agents_get_endpoint(id: String) -> Result<AgentEndpointInfo, String> {
    tracing::debug!("agents_get_endpoint 调用: {}", id);

    let (current, external_token_detected, current_token_masked) = match id.as_str() {
        "claude-code" => read_claude_endpoint().await?,
        "codex" => read_codex_endpoint().await?,
        "gemini-cli" | "openclaw" | "hermes" => read_simple_endpoint(&id)?,
        _ => {
            return Ok(AgentEndpointInfo {
                agent_id: id,
                supported: false,
                unsupported_reason: Some("未知 agent".into()),
                current: EndpointConfig::default(),
                external_token_detected: false,
                current_token_masked: String::new(),
                presets: EndpointPresets {
                    openrouter_anthropic: vec![],
                },
                available_secrets: vec![],
            });
        }
    };

    let available_secrets = list_llm_secrets().await.unwrap_or_else(|e| {
        tracing::warn!("list_llm_secrets 失败（返回空列表）: {e}");
        vec![]
    });
    Ok(AgentEndpointInfo {
        agent_id: id,
        supported: true,
        unsupported_reason: None,
        current,
        external_token_detected,
        current_token_masked,
        presets: EndpointPresets {
            openrouter_anthropic: openrouter_anthropic_presets(),
        },
        available_secrets,
    })
}

/// 从 SOPS 拿 token 明文（仅返回 String 给调用者短暂持有）
///
/// SecureString 在函数内 drop → zeroize；调用者负责拿到后尽快用完。
async fn reveal_secret_for(short_name: &str) -> Result<String, String> {
    let adapter = SopsAdapter::new();
    let yaml_path = secrets_yaml_path();
    let secure = adapter
        .decrypt_key(&yaml_path, short_name)
        .await
        .map_err(|e| format!("sops 解密 secret {short_name} 失败: {e}"))?;
    let plain = secure.as_str().to_string();
    drop(secure);
    if plain.trim().is_empty() {
        return Err(format!(
            "secret {short_name} 解密结果为空（检查 SOPS 中是否已写入值）"
        ));
    }
    Ok(plain)
}

#[tauri::command]
pub async fn agents_set_endpoint(
    id: String,
    config: EndpointConfig,
) -> Result<AgentEndpointInfo, String> {
    tracing::debug!(
        "agents_set_endpoint 调用: {} provider={} secret={:?}",
        id,
        config.provider,
        config.secret_short_name
    );
    if !matches!(
        id.as_str(),
        "claude-code" | "codex" | "gemini-cli" | "openclaw" | "hermes"
    ) {
        return Err(format!("agent {id} 不支持 endpoint 配置"));
    }

    // 共享校验：secret_short_name = None 是合法（OAuth 模式）
    if let Some(short) = config.secret_short_name.as_deref() {
        if short.trim().is_empty() {
            return Err("secret_short_name 为空（如要走 OAuth 模式，请传 null）".into());
        }
        // 白名单校验
        if !LLM_SECRET_REGISTRY
            .iter()
            .any(|(s, _, _, _, _)| *s == short)
        {
            return Err(format!(
                "secret {short} 不是 LLM-capable（不在白名单）。可选：{}",
                LLM_SECRET_REGISTRY
                    .iter()
                    .map(|(s, _, _, _, _)| *s)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        // 早期校验：reveal helper 必须存在
        homebase_reveal_exe_path()?;
    }

    match id.as_str() {
        "claude-code" => write_claude_endpoint(&config)?,
        "codex" => write_codex_endpoint(&config)?,
        "gemini-cli" | "openclaw" | "hermes" => write_simple_endpoint(&id, &config)?,
        _ => unreachable!("checked above"),
    }
    agents_get_endpoint(id).await
}

#[tauri::command]
pub async fn agents_test_endpoint(
    id: String,
    config: EndpointConfig,
) -> Result<EndpointTestResult, String> {
    tracing::debug!(
        "agents_test_endpoint 调用: {} provider={} secret={:?}",
        id,
        config.provider,
        config.secret_short_name
    );
    if id != "claude-code" && id != "codex" {
        return Err(format!(
            "agent {id} 不支持 endpoint 测试（Phase G-4/G-5 后扩展）"
        ));
    }

    let base_url = config
        .base_url
        .clone()
        .ok_or_else(|| "base_url 未设置".to_string())?;
    let short = config
        .secret_short_name
        .as_ref()
        .ok_or_else(|| "secret_short_name 未设置".to_string())?;
    if short.trim().is_empty() {
        return Err("secret_short_name 为空".into());
    }
    let token = reveal_secret_for(short).await?;

    // 协议分发：根据 base_url 推断
    // - anthropic.com → Anthropic /v1/messages
    // - 其他（OpenRouter / OpenAI / 自定义 OpenAI 兼容）→ OpenAI /v1/chat/completions
    let is_anthropic_protocol = base_url.contains("anthropic.com");
    let normalized = base_url.trim_end_matches('/');

    let (url, body): (String, serde_json::Value) = if is_anthropic_protocol {
        let model = config
            .default_haiku
            .clone()
            .or_else(|| config.default_sonnet.clone())
            .or_else(|| config.default_opus.clone())
            .unwrap_or_else(|| "claude-haiku-4-5".to_string());
        let u = if normalized.ends_with("/v1") {
            format!("{normalized}/messages")
        } else {
            format!("{normalized}/v1/messages")
        };
        let b = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}]
        });
        (u, b)
    } else {
        // OpenAI chat.completions 协议（OpenRouter 默认 + OpenAI direct）
        let model = config
            .default_haiku
            .clone()
            .or_else(|| config.default_sonnet.clone())
            .or_else(|| config.default_opus.clone())
            .unwrap_or_else(|| {
                // 用 OpenRouter 上最便宜的轻量模型探测
                "openai/gpt-4o-mini".to_string()
            });
        let u = if normalized.ends_with("/v1") {
            format!("{normalized}/chat/completions")
        } else {
            format!("{normalized}/v1/chat/completions")
        };
        let b = serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}]
        });
        (u, b)
    };

    let started = std::time::Instant::now();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client 构造失败：{e}"))?;

    let mut req = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json");
    if is_anthropic_protocol {
        req = req.header("anthropic-version", "2023-06-01");
    }
    let req_result = req.json(&body).send().await;

    drop(token); // 明示 drop 明文变量
    let latency_ms = started.elapsed().as_millis() as u64;

    match req_result {
        Ok(resp) => {
            let status = resp.status();
            let preview = match resp.text().await {
                Ok(text) => Some(text.chars().take(400).collect::<String>()),
                Err(_) => None,
            };
            Ok(EndpointTestResult {
                ok: status.is_success(),
                status_code: Some(status.as_u16()),
                latency_ms,
                error: if status.is_success() {
                    None
                } else {
                    Some(format!("HTTP {}", status.as_u16()))
                },
                body_preview: preview,
            })
        }
        Err(e) => Ok(EndpointTestResult {
            ok: false,
            status_code: None,
            latency_ms,
            error: Some(format!("{e}")),
            body_preview: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn performance_pid_snapshot_deduplicates_and_expires() {
        use super::*;
        let snapshot = Mutex::new(None);
        assert_eq!(cached_agent_pids(&snapshot, || Ok(vec![Some(7), None])).unwrap(), vec![Some(7), None]);
        assert_eq!(cached_agent_pids(&snapshot, || panic!("duplicate scan")).unwrap(), vec![Some(7), None]);
        snapshot.lock().unwrap().as_mut().unwrap().0 = Instant::now() - Duration::from_secs(2);
        assert_eq!(cached_agent_pids(&snapshot, || Ok(vec![None, Some(8)])).unwrap(), vec![None, Some(8)]);
    }

    #[test]
    fn performance_pid_snapshot_does_not_cache_errors() {
        use super::*;
        let snapshot = Mutex::new(None);
        assert!(cached_agent_pids(&snapshot, || Err("synthetic unavailable".into())).is_err());
        assert!(snapshot.lock().unwrap().is_none());
        assert_eq!(cached_agent_pids(&snapshot, || Ok(vec![Some(9)])).unwrap(), vec![Some(9)]);
    }

    #[test]
    fn performance_pid_snapshot_serializes_concurrent_scans() {
        use super::*;
        use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
        let snapshot = Arc::new(Mutex::new(None));
        let scans = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..5 {
                let snapshot = snapshot.clone();
                let scans = scans.clone();
                scope.spawn(move || {
                    cached_agent_pids(&snapshot, || {
                        scans.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(10));
                        Ok(vec![Some(9)])
                    }).unwrap();
                });
            }
        });
        assert_eq!(scans.load(Ordering::SeqCst), 1);
    }
    use super::*;

    #[tokio::test]
    async fn agents_list_returns_5_agents() {
        let list = agents_list().await.expect("ok");
        assert_eq!(list.len(), 5);
        let ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.contains(&"claude-code"));
        assert!(ids.contains(&"codex"));
        assert!(ids.contains(&"gemini-cli"));
        assert!(ids.contains(&"openclaw"));
        assert!(ids.contains(&"hermes"));
    }

    #[tokio::test]
    async fn agents_status_unknown_id_errors() {
        let res = agents_status("__not_a_real_agent__".to_string()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn agents_status_returns_state_for_known_id() {
        // 不 assert 具体状态（可能在跑或未跑），只验证命令能跑通且 state 字段被填
        let st = agents_status("claude-code".to_string()).await.expect("ok");
        assert_eq!(st.id, "claude-code");
        assert!(matches!(
            st.state,
            AgentState::Running
                | AgentState::Stopped
                | AgentState::Misconfigured
                | AgentState::NotInstalled
        ));
    }

    #[tokio::test]
    async fn agents_start_unknown_id_errors() {
        let res = agents_start("__nope__".to_string()).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("未知 agent"));
    }

    #[tokio::test]
    async fn all_agents_have_launch_subcommand() {
        // 保证所有已知 agent 都可被一键启动（无 launch_subcommand 的 agent 应当剔除或显式标注）
        let list = agents_list().await.expect("ok");
        for a in &list {
            assert!(
                a.launch_subcommand.is_some(),
                "agent {} 缺 launch_subcommand",
                a.id
            );
        }
    }

    #[tokio::test]
    async fn agents_open_config_unknown_id_errors() {
        let res = agents_open_config("__nope__".to_string()).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("未知 agent"));
    }

    #[tokio::test]
    async fn agents_open_config_without_path_errors() {
        // openclaw / hermes 都是 config_relpath = None
        let res = agents_open_config("openclaw".to_string()).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("无独立配置文件"));
    }

    #[tokio::test]
    async fn agents_get_model_unknown_id_errors() {
        let res = agents_get_model("__nope__".to_string()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn agents_get_model_supported_returns_presets() {
        let info = agents_get_model("claude-code".to_string())
            .await
            .expect("ok");
        assert!(info.supported);
        assert_eq!(info.agent_id, "claude-code");
        assert!(info.presets.iter().any(|p| p.value == "sonnet"));
        assert!(info.unsupported_reason.is_none());
    }

    #[tokio::test]
    async fn agents_get_model_unsupported_for_openclaw() {
        let info = agents_get_model("openclaw".to_string()).await.expect("ok");
        assert!(!info.supported);
        assert!(info.presets.is_empty());
        assert!(info.unsupported_reason.is_some());
        assert!(info.current.is_none());
    }

    #[tokio::test]
    async fn agents_get_model_unsupported_for_hermes() {
        let info = agents_get_model("hermes".to_string()).await.expect("ok");
        assert!(!info.supported);
    }

    #[tokio::test]
    async fn agents_set_model_rejects_unsupported() {
        let res = agents_set_model("openclaw".to_string(), "anything".into()).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("不支持模型修改"));

        let res = agents_set_model("hermes".to_string(), "anything".into()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn agents_set_model_unknown_id_errors() {
        let res = agents_set_model("__nope__".to_string(), "x".into()).await;
        assert!(res.is_err());
    }

    // ---- Endpoint Config 测试（Phase B · 2026-05-04）-------------------

    #[test]
    fn infer_provider_handles_known_urls() {
        assert_eq!(infer_provider(&None), "unset");
        assert_eq!(
            infer_provider(&Some("https://api.anthropic.com".into())),
            "anthropic-direct"
        );
        assert_eq!(
            infer_provider(&Some("https://openrouter.ai/api/v1".into())),
            "openrouter"
        );
        assert_eq!(
            infer_provider(&Some("https://my-gateway.example.com/v1".into())),
            "custom"
        );
    }

    #[test]
    fn openrouter_presets_cover_all_tiers() {
        let presets = openrouter_anthropic_presets();
        assert!(presets.len() >= 14, "至少 14 条 model ID");
        let opus = presets.iter().filter(|p| p.tier == "opus").count();
        let sonnet = presets.iter().filter(|p| p.tier == "sonnet").count();
        let haiku = presets.iter().filter(|p| p.tier == "haiku").count();
        assert!(opus >= 3, "opus tier ≥3 条");
        assert!(sonnet >= 3, "sonnet tier ≥3 条");
        assert!(haiku >= 3, "haiku tier ≥3 条");
        // 含当前最新版本
        assert!(presets.iter().any(|p| p.id == "anthropic/claude-opus-4.7"));
        assert!(presets
            .iter()
            .any(|p| p.id == "anthropic/claude-sonnet-4.6"));
        assert!(presets.iter().any(|p| p.id == "anthropic/claude-haiku-4.5"));
    }

    #[tokio::test]
    async fn agents_get_endpoint_supported_for_all_5_agents() {
        // Phase G-4+5 后 5 agent 全部 supported
        for id in ["claude-code", "codex", "gemini-cli", "openclaw", "hermes"] {
            let info = agents_get_endpoint(id.to_string()).await.expect("ok");
            assert!(info.supported, "{id} 应该 supported");
            assert!(info.unsupported_reason.is_none());
            assert!(!info.available_secrets.is_empty());
        }
    }

    #[tokio::test]
    async fn agents_get_endpoint_rejects_unknown_agent() {
        let info = agents_get_endpoint("made-up-agent".to_string())
            .await
            .expect("ok");
        assert!(!info.supported);
        assert!(info.unsupported_reason.is_some());
    }

    #[tokio::test]
    async fn agents_set_endpoint_rejects_empty_secret_string() {
        // Phase G：None 是合法（OAuth 模式）；但显式空串 "" 仍非法
        let cfg = EndpointConfig {
            provider: "openrouter".into(),
            base_url: Some("https://openrouter.ai/api/v1".into()),
            secret_short_name: Some("   ".into()), // 空白
            auth_token_var: "ANTHROPIC_AUTH_TOKEN".into(),
            ..Default::default()
        };
        let res = agents_set_endpoint("claude-code".to_string(), cfg).await;
        assert!(res.is_err());
        let msg = res.unwrap_err();
        assert!(msg.contains("OAuth") || msg.contains("空"));
    }

    #[test]
    fn parse_short_name_from_helper_works() {
        assert_eq!(
            parse_short_name_from_helper(r"C:\Users\u\.homebase\reveal-openrouter_key.cmd"),
            Some("openrouter_key".to_string())
        );
        assert_eq!(
            parse_short_name_from_helper(r"/home/u/.homebase/reveal-anthropic_key.cmd"),
            Some("anthropic_key".to_string())
        );
        // 不匹配的形态 → None（不会误识别 OAuth 模式下用户手动配的其他 helper）
        assert_eq!(
            parse_short_name_from_helper("/usr/bin/some-other-helper"),
            None
        );
        assert_eq!(parse_short_name_from_helper("reveal-.cmd"), None);
        assert_eq!(parse_short_name_from_helper(""), None);
    }

    #[tokio::test]
    async fn agents_set_endpoint_rejects_non_llm_secret() {
        let cfg = EndpointConfig {
            provider: "custom".into(),
            base_url: Some("https://example.com/v1".into()),
            secret_short_name: Some("github_token".into()), // 不在 LLM 白名单
            auth_token_var: "ANTHROPIC_AUTH_TOKEN".into(),
            ..Default::default()
        };
        let res = agents_set_endpoint("claude-code".to_string(), cfg).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("LLM-capable"));
    }

    #[tokio::test]
    async fn agents_test_endpoint_rejects_missing_secret() {
        let cfg = EndpointConfig {
            provider: "openrouter".into(),
            base_url: Some("https://openrouter.ai/api/v1".into()),
            secret_short_name: None,
            auth_token_var: "ANTHROPIC_AUTH_TOKEN".into(),
            ..Default::default()
        };
        let res = agents_test_endpoint("claude-code".to_string(), cfg).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("secret_short_name"));
    }

    #[tokio::test]
    async fn agents_test_endpoint_rejects_missing_base_url() {
        let cfg = EndpointConfig {
            provider: "custom".into(),
            base_url: None,
            secret_short_name: Some("openrouter_key".into()),
            auth_token_var: "ANTHROPIC_AUTH_TOKEN".into(),
            ..Default::default()
        };
        let res = agents_test_endpoint("claude-code".to_string(), cfg).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("base_url"));
    }

    #[test]
    fn llm_registry_contains_expected_keys() {
        // 必须包含 openrouter_key / anthropic_key（最常用 2 个）
        let keys: Vec<&str> = LLM_SECRET_REGISTRY
            .iter()
            .map(|(s, _, _, _, _)| *s)
            .collect();
        assert!(keys.contains(&"openrouter_key"));
        assert!(keys.contains(&"anthropic_key"));
        // 不包含非 LLM key（github_token 等不应在白名单里）
        assert!(!keys.contains(&"github_token"));
        assert!(!keys.contains(&"firecrawl_token"));
    }

    #[test]
    fn short_mask_handles_edge_cases() {
        assert_eq!(short_mask("short"), "********"); // < 16 字符
        let masked = short_mask("sk-or-v1-1234567890abcdefghij");
        assert!(masked.starts_with("sk-or-v"));
        assert!(masked.contains("****"));
        assert!(masked.ends_with("ghij"));
    }
}
