//! Adapter 层 — 调用现有 CLI/工具的 Rust 抽象（v3 §3.2 + §10）
//!
//! DRY 原则（v3 §2.1）：不重写 SOPS 加解密 / agent-env / git 命令。
//!
//! 共享契约（Day 3 定义）：
//! - 所有子进程调用走 `spawn_with_timeout` → 结构化 `CommandOutput`（stdout/stderr/exit/duration）
//! - 错误按 `AdapterError` 分类（spawn fail / non-zero exit / timeout / utf8 decode / io）
//! - 默认 timeout 30s（长命令可传自定义），agent-env 例外不 timeout（long-running agent）

pub mod agent_env;
pub mod audit;
pub mod git;
pub mod mcp_pool;
pub mod mcp_reconcile;
pub mod powershell;
pub mod secrets_acl;
pub mod secrets_ipc;
pub mod secrets_store;
pub mod shared_context;
pub mod skills_pool;
pub mod snapshot;
pub mod sops;
pub mod sops_probe;
pub mod secret_vault;
pub mod secret_health;
pub mod dpapi;
pub mod service_credentials;

/// Home 目录定位 · 测试可通过 `HOMEBASE_TEST_HOME` env 覆盖（防污染真实 ~/.codex 等）
///
/// 生产路径：dirs::home_dir() · 跨平台稳。
/// 测试路径：set HOMEBASE_TEST_HOME=<tmpdir> · 所有 shared_context / mcp_pool / mcp_reconcile 都走它。
pub fn homebase_home_dir() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("HOMEBASE_TEST_HOME") {
        return std::path::PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

mod process_runner;
pub use process_runner::*;
