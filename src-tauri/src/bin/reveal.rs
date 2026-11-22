//! homebase-reveal — 独立小型 helper binary，给 5 个 agent CLI 当 SOPS reveal 通道
//!
//! 设计目标（Phase G）：
//! - 与桌面端共享有超时的进程执行器和到期策略，不依赖 Tauri
//! - **stdout-only 协议**：参照 Claude Code apiKeyHelper / Codex auth.command 的契约
//!     - 成功 → 解密后 token 原样写到 stdout，exit 0
//!     - 失败 → stderr 写错误信息，非零 exit code
//! - **静默**：无 banner / 不打 log / 不写文件
//!
//! 调用形态：
//! ```text
//!   homebase-reveal.exe openrouter_key                              # 默认 yaml
//!   set HOMEBASE_SECRETS_FILE=D:\custom.yaml & homebase-reveal.exe openrouter_key
//! ```
//!
//! 共享调用契约（Phase G 各 agent 文档保持一致）：
//! - Claude Code: settings.json 的 `apiKeyHelper` 指向 .cmd 中转脚本（apiKeyHelper 不接参数）
//! - Codex:      config.toml 的 `model_providers.<id>.auth.command` + `args = ["<short>"]`
//! - Gemini/OpenClaw/Hermes: 由 HomeBase 的 agent-env wrapper 调用后注入 env

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use zeroize::Zeroize;
#[path = "../adapters/process_runner.rs"]
mod process_runner;
#[path = "../secret_policy.rs"]
mod secret_policy;

const SOPS_EXE: &str = r"C:\HomeBase\Tools\sops\bin\sops.exe";

#[tokio::main]
async fn main() {
    let short_name = match std::env::args().nth(1) {
        Some(s) if !s.trim().is_empty() => s,
        _ => {
            eprintln!("homebase-reveal: missing argument");
            eprintln!("usage: homebase-reveal <secret_short_name>");
            eprintln!("  env HOMEBASE_SECRETS_FILE overrides the configured vault path");
            std::process::exit(2);
        }
    };

    let yaml_path = secret_policy::vault_path().to_string_lossy().into_owned();

    let _guard = match secret_policy::check_access(Path::new(&yaml_path), &short_name) {
        Ok(guard) => guard,
        Err(error) => { eprintln!("homebase-reveal: {error}"); std::process::exit(6); }
    };
    if let Err(error) = secret_policy::require_encrypted_string(Path::new(&yaml_path), &short_name) { eprintln!("homebase-reveal: {error}"); std::process::exit(6); }

    // SOPS --extract 需要 JSON-style key path, e.g. ["openrouter_key"]
    let extract_expr = serde_json::to_string(&[&short_name]).expect("string array");

    let sops_exe = std::env::var("HOMEBASE_SOPS_EXE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| SOPS_EXE.to_string());
    let mut command = Command::new(&sops_exe);
    command
        .args(["--decrypt", "--extract", &extract_expr, &yaml_path])
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped());
    apply_conventional_age_identity(&mut command, Path::new(&yaml_path));
    let child_env: Vec<(String, String)> = command.get_envs().filter_map(|(key, value)| {
        value.map(|value| (key.to_string_lossy().into_owned(), value.to_string_lossy().into_owned()))
    }).collect();
    let config = std::env::var("HOMEBASE_SOPS_CONFIG").ok().filter(|s| !s.is_empty());
    let mut args = Vec::new();
    if let Some(config) = &config { args.extend(["--config", config.as_str()]); }
    args.extend(["--decrypt", "--output-type", "json", "--extract", &extract_expr, &yaml_path]);
    let result = process_runner::spawn_with_timeout_env(
        &sops_exe, args,
        None, &child_env, std::time::Duration::from_secs(30),
    ).await;
    let mut out = match result {
        Ok(output) => output,
        Err(_) => { eprintln!("homebase-reveal: SOPS 调用失败或超时，敏感输出已隐藏"); std::process::exit(3); }
    };
    if !out.is_success() {
        out.stdout.zeroize(); out.stderr.zeroize();
        eprintln!("homebase-reveal: SOPS 未能读取密钥，敏感输出已隐藏");
        std::process::exit(4);
    }
    let raw = zeroize::Zeroizing::new(out.stdout);
    let plain = raw;
    if plain.trim().is_empty() { eprintln!("homebase-reveal: 密钥为空"); std::process::exit(4); }

    // 写到 stdout（不附加换行 — Claude Code/Codex 直接消费）
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if let Err(e) = handle.write_all(plain.as_bytes()) {
        eprintln!("homebase-reveal: write stdout failed: {}", e);
        std::process::exit(5);
    }
}

fn apply_conventional_age_identity(command: &mut Command, yaml_path: &Path) {
    const IDENTITY_ENV_NAMES: &[&str] = &[
        "SOPS_AGE_KEY_FILE",
        "SOPS_AGE_KEY",
        "SOPS_AGE_KEY_CMD",
        "SOPS_AGE_SSH_PRIVATE_KEY_FILE",
        "SOPS_AGE_SSH_PRIVATE_KEY_CMD",
    ];
    if IDENTITY_ENV_NAMES
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    {
        return;
    }
    let explicit = std::env::var_os("HOMEBASE_SOPS_AGE_KEY_FILE")
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file());
    let sibling = yaml_path
        .parent()
        .map(|parent| parent.join(".age-keys").join("primary.txt"))
        .filter(|path| path.is_file());
    if let Some(identity) = explicit.or(sibling) {
        command.env("SOPS_AGE_KEY_FILE", identity);
    }
}
