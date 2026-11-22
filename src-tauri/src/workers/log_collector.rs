//! LogCollector — 收集 agent stdout/stderr
//!
//! Phase 13.3：聚合到 Logs 模块（v3 §6.8）+ "一键复制给 AI" 按钮。

#[allow(dead_code)]
pub struct LogCollector;

impl LogCollector {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LogCollector {
    fn default() -> Self {
        Self::new()
    }
}
