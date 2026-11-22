//! FileWatcher — 监听文件系统变化
//!
//! 监听目标：
//! - INDEX.md (Knowledge 模块)
//! - C:\HomeBase\Wiki\**\*.md (Knowledge + Obsidian 健康检查 v3 §22)
//! - C:\HomeBase\Skills\**\SKILL.md (Skills 模块 v3 §21.2)
//!
//! Phase 13.1 Day 8-10 实现。

#[allow(dead_code)]
pub struct FileWatcher;

impl FileWatcher {
    pub fn new() -> Self {
        Self
    }

    pub async fn watch(&self) {
        // TODO: notify crate + 双向 file watcher（v3 §22.2）
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new()
    }
}
