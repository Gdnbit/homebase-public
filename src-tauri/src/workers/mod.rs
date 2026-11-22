//! Background workers（v3 §3.2）
//!
//! - HealthChecker：周期检测 secrets/agents/disks 健康
//! - FileWatcher：监听 C:\HomeBase\Wiki + C:\HomeBase\Skills + INDEX.md 变化
//! - LogCollector：收集 agent stdout/stderr

pub mod file_watcher;
pub mod health_checker;
pub mod log_collector;
