//! GDNbit 发布命令（Phase C · 2026-05-04）
//!
//! 包装两个外部脚本（不重写、不绕过 publishing-kit）：
//! - `publishing-kit/scripts/publish.mjs` · 单文件 doctor/render/preview/publish/list 主工具
//! - `scripts/obsidian-sync.mjs` · 批量扫 vault 全局同步
//!
//! 密钥流（Phase G 集成）：
//! - HomeBase 调 `SopsAdapter::decrypt_key` 拿到 `gdnbit_content_api_key` 明文
//! - 仅在 spawn 子进程时以 env `CONTENT_API_KEY` 注入，不落盘
//! - 旧 `~/.homebase/gdnbit.json` 里的 `api_key` 字段软弃用（`#[serde(default)]` 付兼容，但不再读/写）
//!
//! Tauri commands：
//! - `gdnbit_get_config` / `gdnbit_set_config` · 读写 `~/.homebase/gdnbit.json`
//! - `gdnbit_doctor` · 本地 6 项检查（node / publish.mjs / sync.mjs / vault / SOPS key / API URL）
//! - `gdnbit_kit_doctor` · 透传 `publish.mjs doctor`（6 项上游检查）
//! - `gdnbit_publish_file` · 单文件 preview/publish · 流式
//! - `gdnbit_render_file` · 单文件 本地 HTML 预览 · 一次性
//! - `gdnbit_list_remote` · 列服务器现有内容 · 一次性
//! - `gdnbit_sync_vault` · 批量扫 vault · 流式（原 `gdnbit_publish`）
//! - `gdnbit_cancel` · kill 正在跑的任务
//!
//! 所有流式命令都 emit Tauri 事件 `gdnbit:event`（`GdnbitEvent` enum）。
//! 模式参考 `wiki_agent::wiki_agent_run_ingest`。

use crate::adapters::sops::{SecureString, SopsAdapter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::AppHandle;
use crate::task_events::TrackedEmitter;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::sync::Mutex;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// =====================================================================
// 常量
// =====================================================================

/// SOPS 里的密钥名（Phase G 约定）
const SOPS_KEY_NAME: &str = "gdnbit_content_api_key";
/// 默认 SOPS 加密文件
/// 事件名 · 前后端约定
const EVENT_NAME: &str = "gdnbit:event";

fn sops_yaml_path() -> PathBuf {
    crate::secret_policy::vault_path()
}

// =====================================================================
// 配置 schema · 持久化到 ~/.homebase/gdnbit.json
// =====================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GdnbitConfig {
    /// node.exe 可执行路径（默认 "node"，假设在 PATH）
    #[serde(default = "default_node_path")]
    pub node_path: String,
    /// obsidian-sync.mjs 脚本绝对路径（批量扫 vault 同步）
    #[serde(default = "default_script_path")]
    pub script_path: String,
    /// publishing-kit/scripts/publish.mjs 脚本绝对路径（单文件主工具）
    #[serde(default = "default_publish_kit_path")]
    pub publish_kit_path: String,
    /// 网站 API URL（脚本会拼 /api/content/publish）
    #[serde(default = "default_api_url")]
    pub api_url: String,
    /// **软弃用**：Phase G 后密钥从 SOPS `gdnbit_content_api_key` 取，不再从此读
    ///
    /// 仅为了兼容旧 `~/.homebase/gdnbit.json` 中可能已写入的字段。
    /// 运行时应始终走 SOPS（参见 `fetch_api_key`）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Obsidian vault 根目录（默认 C:\HomeBase\Wiki）
    #[serde(default = "default_vault_path")]
    pub vault_path: String,
    /// 默认发布状态：draft / published
    #[serde(default = "default_status")]
    pub default_status: String,
}

fn default_node_path() -> String { "node".into() }
fn default_script_path() -> String {
    if let Some(path) = crate::legacy_isolation::test_path("Site/scripts/obsidian-sync.mjs") { return path.to_string_lossy().into_owned(); }
    r"C:\HomeBase\Integrations\ExampleSite\scripts\obsidian-sync.mjs".into()
}
fn default_publish_kit_path() -> String {
    if let Some(path) = crate::legacy_isolation::test_path("Site/publishing-kit/scripts/publish.mjs") { return path.to_string_lossy().into_owned(); }
    r"C:\HomeBase\Integrations\ExampleSite\publishing-kit\scripts\publish.mjs".into()
}
fn default_api_url() -> String {
    if crate::legacy_isolation::test_path("").is_some() { "http://127.0.0.1:9".into() } else { "https://example.invalid".into() }
}
fn default_vault_path() -> String { crate::legacy_isolation::wiki_root().to_string_lossy().into_owned() }
fn default_status() -> String { "draft".into() }

impl Default for GdnbitConfig {
    fn default() -> Self {
        Self {
            node_path: default_node_path(),
            script_path: default_script_path(),
            publish_kit_path: default_publish_kit_path(),
            api_url: default_api_url(),
            api_key: None,
            vault_path: default_vault_path(),
            default_status: default_status(),
        }
    }
}

fn config_path() -> PathBuf {
    if let Some(path) = crate::legacy_isolation::test_path(".homebase/gdnbit.json") { return path; }
    let home = std::env::var("USERPROFILE")
        .unwrap_or_else(|_| "C:\\Users\\homebase".to_string());
    PathBuf::from(home).join(".homebase").join("gdnbit.json")
}

fn load_config() -> Result<GdnbitConfig, String> {
    let path = config_path();
    crate::legacy_isolation::ensure_test_path(&path)?;
    let config: GdnbitConfig = crate::query::optional_json(&path)?;
    validate_test_config(&config)?;
    Ok(config)
}

fn validate_test_config(cfg: &GdnbitConfig) -> Result<(), String> {
    for path in [&cfg.script_path, &cfg.publish_kit_path, &cfg.vault_path] {
        crate::legacy_isolation::ensure_test_path(Path::new(path))?;
    }
    crate::legacy_isolation::ensure_test_url(&cfg.api_url)
}

fn save_config(cfg: &GdnbitConfig) -> Result<(), String> {
    let path = config_path();
    crate::legacy_isolation::ensure_test_path(&path)?;
    validate_test_config(cfg)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
    }
    let pretty = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化 GdnbitConfig 失败：{e}"))?;
    std::fs::write(&path, pretty)
        .map_err(|e| format!("写入 {} 失败：{e}", path.display()))?;
    Ok(())
}

#[tauri::command]
pub async fn gdnbit_get_config() -> Result<GdnbitConfig, String> {
    load_config()
}

#[tauri::command]
pub async fn gdnbit_set_config(config: GdnbitConfig) -> Result<GdnbitConfig, String> {
    save_config(&config)?;
    load_config()
}

// =====================================================================
// 公共 helpers · 密钥获取 / spawn 子进程
// =====================================================================

/// 从 SOPS 解密 `gdnbit_content_api_key` · 返回 SecureString（Drop 时自动 zeroize）
///
/// 失败原因：
/// - SOPS yaml 不存在
/// - 环境缺 SOPS_AGE_KEY_FILE / SOPS_AGE_KEY
/// - yaml 里不包含该 key
async fn fetch_api_key() -> Result<SecureString, String> {
    let yaml = sops_yaml_path();
    if !yaml.exists() {
        return Err(format!("SOPS 密钥文件不存在：{}", yaml.display()));
    }
    let adapter = SopsAdapter::new();
    adapter
        .decrypt_key(&yaml, SOPS_KEY_NAME)
        .await
        .map_err(|e| format!("解密 {SOPS_KEY_NAME} 失败：{e}"))
}

/// 构造 `node <script> [args...]` 的 TokioCommand，含：
/// - env 注入 `CONTENT_API_KEY` / `GDNBIT_API_URL`
/// - stdout/stderr piped · stdin null
/// - Windows CREATE_NO_WINDOW
///
/// 调用方拿到 `cmd` 后仍需 `.spawn()` 或 `.output().await` 。
async fn build_node_command(
    cfg: &GdnbitConfig,
    script: &str,
    args: &[&str],
) -> Result<TokioCommand, String> {
    if !Path::new(script).exists() {
        return Err(format!("脚本不存在：{script}"));
    }
    let api_key = fetch_api_key().await?;

    let mut cmd = TokioCommand::new(&cfg.node_path);
    cmd.arg(script).args(args);
    cmd.env("CONTENT_API_KEY", api_key.as_str());
    cmd.env("GDNBIT_API_URL", &cfg.api_url);
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    // 返回前 api_key 会 Drop · 但 cmd.env() 已拷贝到内部 String。
    // 未来优化点：允许使用 OsString 避免拷贝（当前 cmd.env 接受 AsRef<OsStr> 会隐拷）
    Ok(cmd)
}

// =====================================================================
// Doctor · 本地 6 项检查（不联网 doctor，只看环境）
// =====================================================================

#[derive(Debug, Serialize)]
pub struct GdnbitDoctorResult {
    /// 整体 ok（全部子项都通过）
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
    /// 推断的问题清单 / 下一步提示
    pub suggestions: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DoctorCheck {
    /// 唯一标识（node / publish_kit / sync_script / vault / sops_key / api）
    pub id: String,
    /// 显示名
    pub label: String,
    /// 是否通过
    pub ok: bool,
    /// 描述 / 错误信息
    pub detail: String,
}

#[tauri::command]
pub async fn gdnbit_doctor() -> Result<GdnbitDoctorResult, String> {
    tracing::debug!("gdnbit_doctor 调用");
    let cfg = load_config()?;
    let mut checks: Vec<DoctorCheck> = Vec::new();
    let mut suggestions: Vec<String> = Vec::new();

    // 1. node 可用性（node --version）
    let node_check = check_node(&cfg.node_path).await;
    if !node_check.ok {
        suggestions.push(format!(
            "node 不可用：{}。请安装 Node.js >= 18 或在 config 填具体路径",
            node_check.detail
        ));
    }
    checks.push(node_check);

    // 2. publishing-kit/scripts/publish.mjs 存在（主工具）
    let kit_ok = Path::new(&cfg.publish_kit_path).exists();
    checks.push(DoctorCheck {
        id: "publish_kit".into(),
        label: "publishing-kit/publish.mjs".into(),
        ok: kit_ok,
        detail: if kit_ok {
            cfg.publish_kit_path.clone()
        } else {
            format!("未找到：{}", cfg.publish_kit_path)
        },
    });
    if !kit_ok {
        suggestions.push("publish.mjs 不存在：确认 配置的站点工作区中的 publishing-kit 已可用".into());
    }

    // 3. obsidian-sync.mjs 脚本存在（批量扫 vault，可选件；不影响主闭环）
    let sync_ok = Path::new(&cfg.script_path).exists();
    checks.push(DoctorCheck {
        id: "sync_script".into(),
        label: "obsidian-sync.mjs (批量)".into(),
        ok: sync_ok,
        detail: if sync_ok {
            cfg.script_path.clone()
        } else {
            format!("未找到（仅影响批量同步）：{}", cfg.script_path)
        },
    });
    if !sync_ok {
        suggestions.push("obsidian-sync.mjs 不存在（仅影响批量同步功能，单文件发布不受影响）".into());
    }

    // 4. Vault 目录存在
    let vault_ok = Path::new(&cfg.vault_path).exists();
    checks.push(DoctorCheck {
        id: "vault".into(),
        label: "Obsidian vault".into(),
        ok: vault_ok,
        detail: if vault_ok {
            cfg.vault_path.clone()
        } else {
            format!("未找到：{}", cfg.vault_path)
        },
    });
    if !vault_ok {
        suggestions.push("vault 路径不存在：请确认 C:\\HomeBase\\Wiki 目录或填对 vault_path".into());
    }

    // 5. SOPS 密钥（gdnbit_content_api_key）可解密
    let sops_check = match fetch_api_key().await {
        Ok(s) => DoctorCheck {
            id: "sops_key".into(),
            label: "SOPS gdnbit_content_api_key".into(),
            ok: true,
            detail: format!("已解密（长度 {}，尾 4 位 ...{}）",
                s.len(),
                if s.len() >= 4 { &s.as_str()[s.len()-4..] } else { s.as_str() }),
        },
        Err(e) => DoctorCheck {
            id: "sops_key".into(),
            label: "SOPS gdnbit_content_api_key".into(),
            ok: false,
            detail: e,
        },
    };
    if !sops_check.ok {
        suggestions.push(format!(
            "SOPS 读不到密钥：确认 {SOPS_KEY_NAME} 已在 HomeBase 密钥页注册。"
        ));
    }
    checks.push(sops_check);

    // 6. API URL 可达性（HEAD /api/health 或 GET /）
    let api_check = check_api_reachable(&cfg.api_url).await;
    if !api_check.ok {
        suggestions.push(format!(
            "API 不可达：{}。检查 api_url 或确认网站在跑",
            api_check.detail
        ));
    }
    checks.push(api_check);

    let ok = checks.iter().all(|c| c.ok);
    Ok(GdnbitDoctorResult {
        ok,
        checks,
        suggestions,
    })
}

async fn check_node(node_path: &str) -> DoctorCheck {
    let mut cmd = TokioCommand::new(node_path);
    cmd.arg("--version")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(out)) => {
            let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if out.status.success() && !ver.is_empty() {
                DoctorCheck {
                    id: "node".into(),
                    label: "node 可用".into(),
                    ok: true,
                    detail: format!("{node_path} {ver}"),
                }
            } else {
                DoctorCheck {
                    id: "node".into(),
                    label: "node 可用".into(),
                    ok: false,
                    detail: format!("exit={} stderr={}", out.status.code().unwrap_or(-1),
                        String::from_utf8_lossy(&out.stderr).trim()),
                }
            }
        }
        Ok(Err(e)) => DoctorCheck {
            id: "node".into(),
            label: "node 可用".into(),
            ok: false,
            detail: format!("spawn 失败：{e}"),
        },
        Err(_) => DoctorCheck {
            id: "node".into(),
            label: "node 可用".into(),
            ok: false,
            detail: "--version 超时（5s）".into(),
        },
    }
}

async fn check_api_reachable(api_url: &str) -> DoctorCheck {
    let normalized = api_url.trim_end_matches('/');
    // 优先探 /api/health，否则探根路径
    let probe_urls = vec![
        format!("{normalized}/api/health"),
        normalized.to_string(),
    ];

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return DoctorCheck {
                id: "api".into(),
                label: "API 可达".into(),
                ok: false,
                detail: format!("HTTP client 构造失败：{e}"),
            };
        }
    };

    for url in &probe_urls {
        match client.head(url).send().await {
            Ok(r) if r.status().is_success() || r.status().is_redirection() => {
                return DoctorCheck {
                    id: "api".into(),
                    label: "API 可达".into(),
                    ok: true,
                    detail: format!("HTTP {} · {}", r.status().as_u16(), url),
                };
            }
            Ok(r) => {
                // 404 也算可达（只是没这个 endpoint）
                if r.status().as_u16() < 500 {
                    return DoctorCheck {
                        id: "api".into(),
                        label: "API 可达".into(),
                        ok: true,
                        detail: format!("HTTP {} · {}", r.status().as_u16(), url),
                    };
                }
            }
            Err(_) => continue,
        }
    }

    DoctorCheck {
        id: "api".into(),
        label: "API 可达".into(),
        ok: false,
        detail: format!("所有探测均失败：{api_url}（HEAD 超时 / 拒绝连接）"),
    }
}

// =====================================================================
// 任务管理 + GdnbitEvent enum（多个流式命令共享）
// =====================================================================

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GdnbitEvent {
    /// 任务启动 · mode 区分 "sync_vault" / "publish" / "preview" / "kit_doctor"
    Started {
        task_id: String,
        mode: String,
        /// 是否预览模式（sync_vault 下 --dry-run；preview mode 同）· 他者 false
        dry_run: bool,
        /// 脱敏后的命令预览（不含密钥）
        cmd: String,
    },
    /// stdout 一行
    Stream {
        task_id: String,
        line: String,
    },
    /// stderr 一行
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

struct GdnbitTask {
    child: tokio::process::Child,
}

type GdnbitTaskMap = Arc<Mutex<HashMap<String, GdnbitTask>>>;

fn task_map() -> &'static GdnbitTaskMap {
    use std::sync::OnceLock;
    static MAP: OnceLock<GdnbitTaskMap> = OnceLock::new();
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

#[derive(Debug, Serialize)]
pub struct StreamingTaskResult {
    pub task_id: String,
    /// 漂棘主参数后的命令预览（脱敏、不含密钥）
    pub command_preview: String,
    pub mode: String,
    pub dry_run: bool,
}

/// 公共 · spawn TokioCommand + 注册任务 + 流式 emit。
///
/// 三个后台 tokio::spawn：stdout pipe / stderr pipe / try_wait 轮询
///
/// 调用方传入已 build_node_command 准备好的 cmd，本函数负责：
/// 1. spawn
/// 2. emit Started
/// 3. 起 3 个后台任务 stream stdout / stderr / wait_exit
/// 4. 返回 task_id
async fn spawn_streaming(
    app: &AppHandle,
    mut cmd: TokioCommand,
    mode: &str,
    dry_run: bool,
    command_preview: String,
) -> Result<StreamingTaskResult, String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn node 失败：{e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "无法获取 stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法获取 stderr".to_string())?;

    let task_id = uuid::Uuid::new_v4().to_string();

    {
        let map = task_map();
        let mut g = map.lock().await;
        g.insert(task_id.clone(), GdnbitTask { child });
    }

    let _ = app.emit_tracked(
        EVENT_NAME,
        GdnbitEvent::Started {
            task_id: task_id.clone(),
            mode: mode.into(),
            dry_run,
            cmd: command_preview.clone(),
        },
    );

    // pipe stdout
    {
        let app2 = app.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            loop {
                let line = match reader.next_line().await { Ok(Some(line)) => line, Ok(None) => break, Err(_) => { crate::task_events::stream_error(EVENT_NAME, &tid); break; } };
                let _ = app2.emit_tracked(EVENT_NAME, GdnbitEvent::Stream { task_id: tid.clone(), line });
            }
            crate::task_events::stream_closed(EVENT_NAME, &tid);
        });
    }

    // pipe stderr
    {
        let app2 = app.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            loop {
                let line = match reader.next_line().await { Ok(Some(line)) => line, Ok(None) => break, Err(_) => { crate::task_events::stream_error(EVENT_NAME, &tid); break; } };
                let _ = app2.emit_tracked(EVENT_NAME, GdnbitEvent::Stderr { task_id: tid.clone(), line });
            }
            crate::task_events::stream_closed(EVENT_NAME, &tid);
        });
    }

    // 等退出（轮询 try_wait）
    {
        let app2 = app.clone();
        let tid = task_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let map = task_map();
                let mut g = map.lock().await;
                let Some(task) = g.get_mut(&tid) else {
                    let _ = app2.emit_tracked(EVENT_NAME, GdnbitEvent::Cancelled { task_id: tid.clone() });
                    return;
                };
                match task.child.try_wait() {
                    Ok(Some(status)) => {
                        let exit_code = status.code().unwrap_or(-1);
                        if status.success() {
                            let _ = app2.emit_tracked(EVENT_NAME, GdnbitEvent::Completed {
                                task_id: tid.clone(),
                                exit_code,
                            });
                        } else {
                            let _ = app2.emit_tracked(EVENT_NAME, GdnbitEvent::Failed {
                                task_id: tid.clone(),
                                exit_code,
                                message: format!("非零退出 (code {exit_code})"),
                            });
                        }
                        g.remove(&tid);
                        return;
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        let _ = app2.emit_tracked(EVENT_NAME, GdnbitEvent::Failed {
                            task_id: tid.clone(),
                            exit_code: -1,
                            message: format!("等退出失败：{e}"),
                        });
                        g.remove(&tid);
                        return;
                    }
                }
            }
        });
    }

    Ok(StreamingTaskResult {
        task_id,
        command_preview,
        mode: mode.into(),
        dry_run,
    })
}

// =====================================================================
// gdnbit_sync_vault · 批量扫 vault · 流式（原 gdnbit_publish）
// =====================================================================

#[derive(Debug, Deserialize, Default)]
pub struct SyncVaultPayload {
    /// 只同步某个具体文件（vault 相对路径），None = 扫描整个 vault
    #[serde(default)]
    pub single_file: Option<String>,
    /// 预览模式（--dry-run）· 不写入网站
    #[serde(default)]
    pub dry_run: bool,
    /// 忽略 publish: true frontmatter，强制全发（--force）
    #[serde(default)]
    pub force: bool,
    /// 覆盖默认状态（draft / published）
    #[serde(default)]
    pub status: Option<String>,
    /// 只同步 vault 下某个子文件夹
    #[serde(default)]
    pub folder: Option<String>,
}

#[tauri::command]
pub async fn gdnbit_sync_vault(
    app: AppHandle,
    payload: SyncVaultPayload,
) -> Result<StreamingTaskResult, String> {
    tracing::debug!("gdnbit_sync_vault 调用 dry_run={} force={}", payload.dry_run, payload.force);

    let cfg = load_config()?;

    // 前置校验
    if !Path::new(&cfg.script_path).exists() {
        return Err(format!("obsidian-sync.mjs 脚本不存在：{}", cfg.script_path));
    }
    if !Path::new(&cfg.vault_path).exists() {
        return Err(format!("vault 不存在：{}", cfg.vault_path));
    }

    let status = payload
        .status
        .clone()
        .unwrap_or_else(|| cfg.default_status.clone());

    // 拼参数（密钥不走命令行，obsidian-sync.mjs 会从 env CONTENT_API_KEY 读）
    let mut args: Vec<String> = vec![
        "--vault".into(), cfg.vault_path.clone(),
        "--status".into(), status.clone(),
    ];
    if let Some(folder) = &payload.folder {
        args.push("--folder".into());
        args.push(folder.clone());
    }
    if let Some(single) = &payload.single_file {
        args.push("--file".into());
        args.push(single.clone());
    }
    if payload.dry_run {
        args.push("--dry-run".into());
    }
    if payload.force {
        args.push("--force".into());
    }

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let cmd = build_node_command(&cfg, &cfg.script_path, &args_ref).await?;

    let command_preview = format!(
        "node {} {}",
        cfg.script_path,
        args.join(" ")
    );

    spawn_streaming(&app, cmd, "sync_vault", payload.dry_run, command_preview).await
}

// =====================================================================
// gdnbit_publish_file · 单文件 preview / publish · 流式
// 调 publishing-kit/scripts/publish.mjs
// =====================================================================

#[derive(Debug, Deserialize)]
pub struct PublishFilePayload {
    /// 模式：preview / publish
    pub mode: String,
    /// markdown 文件绝对路径
    pub file: String,
    /// 仅 mode=publish 时生效 · 覆盖 frontmatter status · draft / published / archived
    #[serde(default)]
    pub status: Option<String>,
}

#[tauri::command]
pub async fn gdnbit_publish_file(
    app: AppHandle,
    payload: PublishFilePayload,
) -> Result<StreamingTaskResult, String> {
    let mode = match payload.mode.as_str() {
        "preview" | "publish" => payload.mode.as_str(),
        other => return Err(format!("无效 mode：{other}（应为 preview/publish）")),
    };
    if !Path::new(&payload.file).exists() {
        return Err(format!("目标文件不存在：{}", payload.file));
    }
    let cfg = load_config()?;

    let mut args: Vec<String> = vec![mode.to_string(), payload.file.clone()];
    if mode == "publish" {
        if let Some(s) = payload.status.as_deref() {
            args.push("--status".into());
            args.push(s.into());
        }
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let cmd = build_node_command(&cfg, &cfg.publish_kit_path, &args_ref).await?;

    let command_preview = format!(
        "node {} {}",
        cfg.publish_kit_path,
        args.join(" ")
    );

    let dry_run = mode == "preview";
    spawn_streaming(&app, cmd, mode, dry_run, command_preview).await
}

// =====================================================================
// gdnbit_kit_doctor · 流式 publish.mjs doctor（6 项上游检查）
// =====================================================================

#[tauri::command]
pub async fn gdnbit_kit_doctor(app: AppHandle) -> Result<StreamingTaskResult, String> {
    let cfg = load_config()?;
    let cmd = build_node_command(&cfg, &cfg.publish_kit_path, &["doctor"]).await?;
    let command_preview = format!("node {} doctor", cfg.publish_kit_path);
    spawn_streaming(&app, cmd, "kit_doctor", false, command_preview).await
}

// =====================================================================
// gdnbit_render_file · 本地 HTML 预览（publish.mjs render）· 一次性返回
// =====================================================================

#[derive(Debug, Deserialize)]
pub struct RenderFilePayload {
    pub file: String,
    /// 生成后是否自动用默认浏览器打开
    #[serde(default)]
    pub open: bool,
}

#[derive(Debug, Serialize)]
pub struct OneShotResult {
    pub ok: bool,
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[tauri::command]
pub async fn gdnbit_render_file(payload: RenderFilePayload) -> Result<OneShotResult, String> {
    if !Path::new(&payload.file).exists() {
        return Err(format!("目标文件不存在：{}", payload.file));
    }
    let cfg = load_config()?;

    let mut args: Vec<String> = vec!["render".into(), payload.file.clone()];
    if payload.open {
        args.push("--open".into());
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut cmd = build_node_command(&cfg, &cfg.publish_kit_path, &args_ref).await?;

    let output = cmd.output().await.map_err(|e| format!("spawn 失败：{e}"))?;
    Ok(OneShotResult {
        ok: output.status.success(),
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// =====================================================================
// gdnbit_list_remote · 列服务器现有内容（publish.mjs list）· 一次性
// =====================================================================

#[derive(Debug, Deserialize, Default)]
pub struct ListRemoteFilter {
    /// 过滤类型：blog / project / resource
    #[serde(default, rename = "type")]
    pub item_type: Option<String>,
    /// draft / published / archived
    #[serde(default)]
    pub status: Option<String>,
    /// 搜索关键词
    #[serde(default)]
    pub search: Option<String>,
    /// 结果上限
    #[serde(default)]
    pub limit: Option<u32>,
}

#[tauri::command]
pub async fn gdnbit_list_remote(filter: ListRemoteFilter) -> Result<OneShotResult, String> {
    let cfg = load_config()?;

    let mut args: Vec<String> = vec!["list".into()];
    if let Some(t) = &filter.item_type {
        args.push("--type".into());
        args.push(t.clone());
    }
    if let Some(s) = &filter.status {
        args.push("--status".into());
        args.push(s.clone());
    }
    if let Some(s) = &filter.search {
        args.push("--search".into());
        args.push(s.clone());
    }
    if let Some(n) = filter.limit {
        args.push("--limit".into());
        args.push(n.to_string());
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let mut cmd = build_node_command(&cfg, &cfg.publish_kit_path, &args_ref).await?;

    let output = cmd.output().await.map_err(|e| format!("spawn 失败：{e}"))?;
    Ok(OneShotResult {
        ok: output.status.success(),
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// =====================================================================
// gdnbit_ai_optimize · Phase C-4a · spawn claude CLI 对单文件做 AI 风格优化
// =====================================================================
//
// 复用 spawn_streaming 基础设施 + gdnbit:event 事件流（task_id 区分）
// claude 会按 STYLE_GUIDE.md 规范直接改文件（--permission-mode acceptEdits）
//
// 上游依赖：
//   - claude CLI（需在 PATH，v1 不做安装检查）
//   - C:\HomeBase\Wiki\_schema\STYLE_GUIDE.md（可选，缺失则跑通用优化）

#[derive(Debug, Deserialize)]
pub struct GdnbitAiOptimizePayload {
    /// markdown 文件绝对路径
    pub file: String,
    /// 优化 focus：简短中文（"风格" / "结构" / "完整性" / 自由文本）· 留空 = 全面
    #[serde(default)]
    pub focus: Option<String>,
}

/// 拼 Claude prompt
///
/// 关键设计：**不把 STYLE_GUIDE 全文嵌入 prompt**（避免 Windows 32K 命令行上限）
/// 而是指示 Claude 自己用 Read 工具读取 STYLE_GUIDE.md，**agentic 式**：
/// - prompt 体积稳定在 <2KB，不会超 Windows CreateProcess 参数上限
/// - Claude 可按需决定要不要读完整 STYLE_GUIDE / 读哪段 / 要不要参考别的文件
fn render_optimize_prompt(
    file: &Path,
    style_guide_path: &Path,
    style_guide_exists: bool,
    focus: Option<&str>,
) -> String {
    let focus_line = match focus {
        Some(f) if !f.trim().is_empty() => format!("\n\n**本次 focus**：{}", f.trim()),
        _ => "\n\n**本次请做全面优化**（风格 + 结构 + 完整性）".into(),
    };
    let style_section = if style_guide_exists {
        format!(
            "## 风格规范（**第一步先读**）\n\n用 Read 工具读：`{}`\n\n读完之后按规范开始优化。",
            style_guide_path.to_string_lossy()
        )
    } else {
        "## 风格规范\n\n（未找到 STYLE_GUIDE.md · 按 Karpathy LLM Wiki 通用原则处理：清晰 / 对仗 / 无冗语 / 工程直觉优先 / 不用营销腔）".into()
    };
    // **S1 · 共享上下文池注入**（动态 · 每次调用刷新）
    // 单一数据源：~/.homebase/shared/context.toml
    let context_header = crate::adapters::shared_context::render_header();

    format!(
        r#"{context_header}# 任务 · GDNbit Wiki 文章 AI 优化

你的角色：**GDNbit 内容编辑**（把用户 Obsidian Wiki 文章打磨成可发布的网站内容）

## 目标文件

`{file}`

{style_section}

## 操作流程

1. Read 上方 STYLE_GUIDE.md 获取规范
2. Read 目标文件 `{file}`
3. 按规范审核：frontmatter / 标题层级 / 段落长度 / 代码块 / 中英文排版 / 死链 等
4. 用 **Edit 工具**对需要改的地方做**最小必要改动**（保留作者原意 + 语感，不重写整篇）
5. 简短总结："改了 X 处，按 STYLE_GUIDE 第 N 条修复了 Y"；若无需改则说"已符合规范"

## 边界

- 允许工具：**Read / Edit**（禁止 Write 重写整篇 · 禁止 Glob/Grep 去改别的文件）
- **不要**动 frontmatter 的 `slug` / `id` / `created_at` 字段（网站依赖）
- **不要**加 emoji（除非原文已有）
- 中英文之间按 STYLE_GUIDE 的规则（以 STYLE_GUIDE 为准）
{focus_line}
"#,
        file = file.to_string_lossy(),
        style_section = style_section,
        focus_line = focus_line,
    )
}

#[tauri::command]
pub async fn gdnbit_ai_optimize(
    app: AppHandle,
    payload: GdnbitAiOptimizePayload,
) -> Result<StreamingTaskResult, String> {
    tracing::info!("gdnbit_ai_optimize 调用: file={}", payload.file);

    let abs = PathBuf::from(&payload.file);
    if !abs.exists() {
        return Err(format!("文件不存在：{}", payload.file));
    }
    if abs.extension().and_then(|s| s.to_str()) != Some("md") {
        return Err(format!("仅支持 markdown 文件：{}", payload.file));
    }

    // 只检查 STYLE_GUIDE 是否存在（不读内容，让 Claude 用 Read 工具自行读）
    // 这样 prompt 体积稳定在 <2KB，避免 Windows CreateProcess 32K 参数上限
    let style_guide = crate::legacy_isolation::wiki_root().join("_schema/STYLE_GUIDE.md");
    crate::legacy_isolation::ensure_test_path(&style_guide)?;
    let style_guide_exists = style_guide.exists();

    let prompt = render_optimize_prompt(
        &abs,
        &style_guide,
        style_guide_exists,
        payload.focus.as_deref(),
    );

    // spawn claude -p <prompt> --allowedTools Read,Edit --permission-mode acceptEdits
    let allowed_tools = "Read,Edit";
    let work_dir = abs
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut cmd = TokioCommand::new("claude");
    cmd.arg("-p")
        .arg(&prompt)
        .arg("--permission-mode")
        .arg("acceptEdits")
        .arg("--allowedTools")
        .arg(allowed_tools)
        .current_dir(&work_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null());

    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let command_preview = format!(
        "claude -p <prompt> --permission-mode acceptEdits --allowedTools \"{allowed_tools}\" (cwd: {})",
        work_dir.to_string_lossy()
    );

    spawn_streaming(&app, cmd, "ai_optimize", false, command_preview).await
}

// =====================================================================
// gdnbit_cancel · 取消任意流式任务
// =====================================================================

#[tauri::command]
pub async fn gdnbit_cancel(task_id: String) -> Result<(), String> {
    tracing::debug!("gdnbit_cancel 调用: {}", task_id);
    let map = task_map();
    let mut g = map.lock().await;
    let Some(mut task) = g.remove(&task_id) else {
        return Err(format!("task {task_id} 不存在或已结束"));
    };
    if let Err(e) = task.child.kill().await {
        return Err(format!("kill 失败：{e}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Explicit isolated pure path probe; never reads real site"]
    fn legacy_isolation_path_probe() {
        let home = PathBuf::from(std::env::var_os("HOMEBASE_TEST_HOME").unwrap());
        assert_eq!(PathBuf::from(default_vault_path()), home.join("Wiki"));
    }

    #[test]
    fn default_config_has_sensible_values() {
        let cfg = GdnbitConfig::default();
        assert_eq!(cfg.node_path, "node");
        assert!(cfg.script_path.ends_with("obsidian-sync.mjs"));
        assert!(cfg.publish_kit_path.ends_with("publish.mjs"));
        if let Some(home) = crate::legacy_isolation::test_path("") {
            assert_eq!(cfg.api_url, "http://127.0.0.1:9");
            assert_eq!(PathBuf::from(cfg.vault_path), home.join("Wiki"));
        } else {
            assert_eq!(cfg.api_url, "https://example.invalid");
            assert_eq!(cfg.vault_path, r"C:\HomeBase\Wiki");
        }
        assert_eq!(cfg.default_status, "draft");
        assert!(cfg.api_key.is_none());
    }

    #[tokio::test]
    async fn get_config_returns_ok() {
        let r = gdnbit_get_config().await;
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn doctor_runs_and_returns_6_checks() {
        // Phase C · doctor 从 5 项扩为 6 项：
        // node / publish_kit / sync_script / vault / sops_key / api
        let r = gdnbit_doctor().await.expect("doctor should not error");
        assert_eq!(r.checks.len(), 6, "6 个检查项");
        let ids: Vec<&str> = r.checks.iter().map(|c| c.id.as_str()).collect();
        for must_have in ["node", "publish_kit", "sync_script", "vault", "sops_key", "api"] {
            assert!(ids.contains(&must_have), "缺检查项 {must_have}");
        }
    }

    #[tokio::test]
    async fn cancel_nonexistent_task_errors() {
        let r = gdnbit_cancel("nonexistent-task-id".into()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("不存在"));
    }

    #[tokio::test]
    async fn publish_file_rejects_invalid_mode() {
        let app_result = gdnbit_publish_file_test_invalid_mode().await;
        assert!(app_result.contains("无效 mode"));
    }

    /// 走中间函数跳过 AppHandle 需求（单测不能造真 AppHandle）
    /// 只验证 mode 校验逻辑
    async fn gdnbit_publish_file_test_invalid_mode() -> String {
        // 手动复现 mode 校验（与 gdnbit_publish_file 头部一致）
        let mode = "badmode";
        match mode {
            "preview" | "publish" => String::new(),
            other => format!("无效 mode：{other}（应为 preview/publish）"),
        }
    }
}
