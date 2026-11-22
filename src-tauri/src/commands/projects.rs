//! Projects 模块命令（v3 §6.5）— 6 项目 git status + AGENTS.md 检测

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: String,
    pub has_agents_md: bool,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub last_commit: Option<String>,
}

#[tauri::command]
pub async fn projects_list() -> Result<Vec<ProjectInfo>, String> {
    // TODO Phase 13.2: 列 C:\HomeBase\Projects\* 子目录 + git status
    tracing::debug!("projects_list 调用 (stub)");
    Ok(vec![])
}
