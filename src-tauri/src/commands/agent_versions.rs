//! Agent 版本检测（S5 · 2026-05-06）
//!
//! 给 Agents 页面提供"当前版本 / 最新版本 / 是否过时"信息。
//!
//! 5 agent 检测策略：
//! - **claude-code**：npm package `@anthropic-ai/claude-code`
//! - **codex**：npm package `@openai/codex`
//! - **gemini-cli**：npm package `@google/gemini-cli`
//! - **openclaw**：npm package `openclaw`（USER fork · 但发布到了 npm）
//! - **hermes**：USER 自研 fork · 不走 npm · 返回 `{ kind: "fork" }`
//!
//! 实现：
//! - `npm ls -g --depth=0 --json` 一次拿所有已安装包的本地版本（1 次 spawn）
//! - 若 npm ls 没找到（standalone binary 安装方式）· fallback `<bin> --version` 解析
//!   - Windows 用 `cmd /C <bin> --version`（PATHEXT 自动解析 .cmd/.exe）
//!   - 其他系统直接 spawn
//! - `npm view <pkg> version` 拿 registry 最新版本（4 次并行 spawn）
//! - 结果缓存 1 小时（OnceLock<RwLock<...>>）· 减少 npm 调用
//!
//! 前端调用：`invoke<AgentVersionsResult[]>("agent_versions_check")`

use crate::adapters::{spawn_with_timeout, AdapterResult, CommandOutput, DEFAULT_TIMEOUT};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

const CACHE_TTL: Duration = Duration::from_secs(3600);

/// 4 个走 npm 的 agent 映射（hermes 单独处理）
///
/// `(agent_id, npm_package, binary_name_for_fallback)`
const NPM_PACKAGES: &[(&str, &str, &str)] = &[
    ("claude-code", "@anthropic-ai/claude-code", "claude"),
    ("codex", "@openai/codex", "codex"),
    ("gemini-cli", "@google/gemini-cli", "gemini"),
    ("openclaw", "openclaw", "openclaw"),
];

/// 缓存条目（version + 时间戳）
#[derive(Debug, Clone)]
struct CachedVersion {
    /// 上次检查时间（Unix 秒）
    checked_at: u64,
    /// npm registry 最新版本
    latest: String,
}

fn cache() -> &'static RwLock<HashMap<String, CachedVersion>> {
    static C: OnceLock<RwLock<HashMap<String, CachedVersion>>> = OnceLock::new();
    C.get_or_init(|| RwLock::new(HashMap::new()))
}

/// 单 agent 版本信息（前端展示）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentVersionInfo {
    pub agent_id: String,
    /// `npm` / `fork` / `unknown`（hermes = fork）
    pub kind: String,
    /// npm 包名（kind=npm 时有效）
    pub package: Option<String>,
    /// 当前已装版本（None = 未装 或 检测失败）
    pub current: Option<String>,
    /// npm registry 最新版本（None = 取失败）
    pub latest: Option<String>,
    /// 当前 < 最新（取不到任一就 false）
    pub update_available: bool,
    /// 给 USER 的更新命令（kind=npm 时有效）
    pub update_command: Option<String>,
    /// 错误信息（kind=fork 用作"无 npm 上游"提示）
    pub message: Option<String>,
}

/// Tauri 命令：一次性检查 5 agent 的版本（带缓存）
#[tauri::command]
pub async fn agent_versions_check() -> Result<Vec<AgentVersionInfo>, String> {
    tracing::debug!("agent_versions_check 调用");

    // 1. 先用 `npm ls -g --depth=0 --json` 拿所有已装包的当前版本（一次 spawn 拿全部）
    let installed = match list_installed_global().await {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!("npm ls -g 失败 · 当前版本全部置 None: {e}");
            HashMap::new()
        }
    };

    // 2. 对 4 npm agent 并行查 latest（带缓存）+ 仅当 npm ls 没找到时 · 并发 fallback `<bin> --version`
    type FetchTuple = (String, Option<String>, Option<String>, Option<String>);
    let mut handles: Vec<tokio::task::JoinHandle<FetchTuple>> = Vec::new();
    for (agent_id, pkg, bin) in NPM_PACKAGES {
        let agent_id = agent_id.to_string();
        let pkg = pkg.to_string();
        let bin = bin.to_string();
        let needs_fallback = !installed.contains_key(&pkg);
        handles.push(tokio::spawn(async move {
            // a) 并发拿 latest
            let latest_fut = npm_view_latest(&pkg);
            // b) 并发跑 binary fallback（如果 npm ls 没找到）
            let fallback_fut = async {
                if needs_fallback {
                    binary_version_fallback(&bin).await.ok()
                } else {
                    None
                }
            };
            let (latest_res, fallback_current) = tokio::join!(latest_fut, fallback_fut);
            match latest_res {
                Ok(v) => (agent_id, Some(v), None, fallback_current),
                Err(e) => (agent_id, None, Some(e), fallback_current),
            }
        }));
    }

    let mut results: Vec<AgentVersionInfo> = Vec::new();
    let pkg_map: HashMap<&str, &str> = NPM_PACKAGES.iter().map(|(a, p, _)| (*a, *p)).collect();

    for h in handles {
        let (agent_id, latest, err, fallback_current) = match h.await {
            Ok(t) => t,
            Err(e) => {
                let (id, lat, msg) =
                    continue_with_join_error(e, &mut results, &installed, &pkg_map);
                (id, lat, msg, None)
            }
        };
        let pkg = pkg_map.get(agent_id.as_str()).copied().unwrap_or("?");
        // 优先 npm ls · 没有就用 binary fallback 结果
        let (current, install_kind) = match installed.get(pkg).cloned() {
            Some(v) => (Some(v), "npm"),
            None => match fallback_current {
                Some(v) => (Some(v), "standalone"),
                None => (None, "npm"),
            },
        };
        let update_available = match (&current, &latest) {
            (Some(c), Some(l)) => version_less(c, l),
            _ => false,
        };
        // standalone 装的就别给 npm 命令 · 容易误导
        let update_command = if install_kind == "npm" {
            Some(format!("npm i -g {pkg}@latest"))
        } else {
            Some(format!(
                "# 检测到 standalone 安装 · 请按官方更新方式重装 {pkg}"
            ))
        };
        results.push(AgentVersionInfo {
            agent_id,
            kind: install_kind.into(),
            package: Some(pkg.to_string()),
            current: current.clone(),
            latest,
            update_available,
            update_command,
            message: if current.is_none() {
                Some(format!("当前版本未知：未检测到可读取的安装版本，或版本查询失败。{}", err.unwrap_or_default()))
            } else { err },
        });
    }

    // 3. hermes
    results.push(AgentVersionInfo {
        agent_id: "hermes".into(),
        kind: "fork".into(),
        package: None,
        current: None,
        latest: None,
        update_available: false,
        update_command: None,
        message: Some("USER fork · 不走 npm · 手动 git pull".into()),
    });

    // 保持原顺序（claude-code → codex → gemini-cli → openclaw → hermes）
    results.sort_by_key(|r| {
        let order = ["claude-code", "codex", "gemini-cli", "openclaw", "hermes"];
        order.iter().position(|x| *x == r.agent_id).unwrap_or(99)
    });

    Ok(results)
}

/// 容错：JoinError（极少触发 · 比如 panic）· push 一个空结果
fn continue_with_join_error(
    e: tokio::task::JoinError,
    _results: &mut Vec<AgentVersionInfo>,
    _installed: &HashMap<String, String>,
    _pkg_map: &HashMap<&str, &str>,
) -> (String, Option<String>, Option<String>) {
    tracing::warn!("agent_versions_check JoinError: {e}");
    ("unknown".into(), None, Some(format!("join error: {e}")))
}

/// 跑 `<bin> --version` · 抓 stdout 第一段 `\d+\.\d+\.\d+` 当版本号
///
/// Windows 走 `cmd /C` 让 PATHEXT 自动解析 .cmd / .exe / .bat
async fn run_version_command(program: &str, args: &[&str]) -> AdapterResult<CommandOutput> {
    // Callers supply only the fixed programs/packages in NPM_PACKAGES, never user input.
    #[cfg(target_os = "windows")]
    {
        let mut shell_args = vec!["/D", "/C", program];
        shell_args.extend_from_slice(args);
        spawn_with_timeout("cmd.exe", shell_args, None, DEFAULT_TIMEOUT).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        spawn_with_timeout(program, args, None, DEFAULT_TIMEOUT).await
    }
}

async fn binary_version_fallback(bin: &str) -> Result<String, String> {
    let out = run_version_command(bin, &["--version"])
        .await
        .map_err(|e| format!("spawn {bin} --version: {e}"))?;
    if !out.is_success() {
        return Err(format!("{bin} --version exit {:?}", out.exit_code));
    }
    // 有些 CLI 把 version 写到 stderr · 都扫一遍
    extract_first_triple(&out.stdout)
        .or_else(|| extract_first_triple(&out.stderr))
        .ok_or_else(|| format!("无法从 {bin} --version 输出抓到版本号"))
}

/// 在字符串里找第一个 `<u32>.<u32>.<u32>`（不引 regex crate）
fn extract_first_triple(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // 必须是数字开头
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut dots = 0;
        let mut j = i;
        while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.') {
            if bytes[j] == b'.' {
                dots += 1;
                if dots > 2 {
                    break;
                }
            }
            j += 1;
        }
        // 必须正好 2 个点 · 且最后一段是数字
        if dots == 2 && j > start && bytes[j - 1].is_ascii_digit() {
            let candidate = &s[start..j];
            // 三段都必须是数字
            if candidate
                .split('.')
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            {
                return Some(candidate.to_string());
            }
        }
        i = j.max(start + 1);
    }
    None
}

/// 跑 `npm ls -g --depth=0 --json` · 解出 `{ "<pkg>": "version" }` 映射
async fn list_installed_global() -> Result<HashMap<String, String>, String> {
    let out = run_version_command("npm", &["ls", "-g", "--depth=0", "--json"])
        .await
        .map_err(|e| format!("spawn npm: {e}"))?;
    // npm ls 即便有警告也会 exit 1 · 但 stdout 仍然有 JSON · 不靠 exit code
    let stdout = out.stdout;

    #[derive(Deserialize)]
    struct NpmLsRoot {
        #[serde(default)]
        dependencies: HashMap<String, NpmLsDep>,
    }
    #[derive(Deserialize)]
    struct NpmLsDep {
        version: Option<String>,
    }

    let root: NpmLsRoot =
        serde_json::from_str(&stdout).map_err(|e| format!("parse npm ls JSON: {e}"))?;
    Ok(root
        .dependencies
        .into_iter()
        .filter_map(|(k, v)| v.version.map(|ver| (k, ver)))
        .collect())
}

/// 跑 `npm view <pkg> version` · 用 1h 缓存
async fn npm_view_latest(pkg: &str) -> Result<String, String> {
    // 命中缓存
    if let Some(cached) = read_cached(pkg) {
        return Ok(cached);
    }

    let out = run_version_command("npm", &["view", pkg, "version"])
        .await
        .map_err(|e| format!("spawn npm view: {e}"))?;
    if !out.is_success() {
        let stderr = out.stderr;
        return Err(format!("npm view {pkg} 失败: {stderr}"));
    }
    let version = out.stdout.trim().to_string();
    if version.is_empty() {
        return Err(format!("npm view {pkg} 返回空"));
    }

    // 写入缓存
    let now = unix_secs_now();
    if let Ok(mut c) = cache().write() {
        c.insert(
            pkg.to_string(),
            CachedVersion {
                checked_at: now,
                latest: version.clone(),
            },
        );
    }

    Ok(version)
}

fn read_cached(pkg: &str) -> Option<String> {
    let now = unix_secs_now();
    let c = cache().read().ok()?;
    let entry = c.get(pkg)?;
    if now.saturating_sub(entry.checked_at) <= CACHE_TTL.as_secs() {
        Some(entry.latest.clone())
    } else {
        None
    }
}

fn unix_secs_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 简单 semver 比较：a < b · 不引外部 crate · 按 dot-separated 数字比
///
/// 仅做"是否过时"判断 · 对 prerelease 后缀（-alpha 等）忽略后段直接比数字三元组
fn version_less(a: &str, b: &str) -> bool {
    let pa = parse_triple(a);
    let pb = parse_triple(b);
    pa < pb
}

fn parse_triple(s: &str) -> (u64, u64, u64) {
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let parts: Vec<u64> = core
        .split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    (
        parts.first().copied().unwrap_or(0),
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[tokio::test]
    async fn npm_version_resolves_windows_command_shim() {
        let out = run_version_command("npm", &["--version"])
            .await
            .expect("npm is a development prerequisite");
        assert!(out.is_success());
        assert!(extract_first_triple(&out.stdout).is_some());
    }

    #[test]
    fn version_less_basic() {
        assert!(version_less("0.128.0", "0.130.0"));
        assert!(version_less("1.0.0", "1.0.1"));
        assert!(version_less("1.0.0", "2.0.0"));
        assert!(!version_less("2.0.0", "1.99.99"));
        assert!(!version_less("1.0.0", "1.0.0"));
    }

    #[test]
    fn version_less_prerelease_ignored() {
        // pre-release 后缀直接被砍 · 同 triple 视为相等
        assert!(!version_less("1.0.0-alpha", "1.0.0"));
        assert!(version_less("1.0.0-alpha", "1.0.1"));
    }

    #[test]
    fn parse_triple_default_zero() {
        assert_eq!(parse_triple("1"), (1, 0, 0));
        assert_eq!(parse_triple("1.2"), (1, 2, 0));
        assert_eq!(parse_triple("1.2.3"), (1, 2, 3));
        assert_eq!(parse_triple("1.2.3.4"), (1, 2, 3)); // 第 4 段忽略
        assert_eq!(parse_triple("abc"), (0, 0, 0));
    }

    #[test]
    fn extract_first_triple_basic() {
        // 各 agent 的真实 --version 输出格式
        assert_eq!(
            extract_first_triple("2.1.28 (Claude Code)"),
            Some("2.1.28".into())
        );
        assert_eq!(
            extract_first_triple("codex-cli 0.42.0"),
            Some("0.42.0".into())
        );
        assert_eq!(extract_first_triple("0.7.0"), Some("0.7.0".into()));
        assert_eq!(
            extract_first_triple("openclaw v1.2.3\n"),
            Some("1.2.3".into())
        );
    }

    #[test]
    fn extract_first_triple_skips_partial() {
        // 只有 1 段或 2 段不算
        assert_eq!(extract_first_triple("version 1"), None);
        assert_eq!(extract_first_triple("build 1.2 hash abc"), None);
        // 必须是数字三元组 · 不能是 1.2.3-alpha 直接抓出来 (但抓到 1.2.3 即可)
        assert_eq!(extract_first_triple("1.2.3-alpha"), Some("1.2.3".into()));
    }

    #[test]
    fn extract_first_triple_ignores_garbage() {
        assert_eq!(extract_first_triple(""), None);
        assert_eq!(extract_first_triple("no version here"), None);
        assert_eq!(extract_first_triple("..."), None);
        assert_eq!(extract_first_triple("1.2."), None);
    }

    #[test]
    fn extract_first_triple_picks_first() {
        assert_eq!(
            extract_first_triple("app 1.0.0 with deps 2.3.4"),
            Some("1.0.0".into())
        );
    }
}
