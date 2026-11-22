//! Skills 模块命令（v3 §21.2）⭐ 新
//!
//! 跨 agent 开放标准 SKILL.md 资产管理（不重新发明 marketplace）。
//! Phase 13.2: 扫 C:\HomeBase\Skills + Monaco 编辑器。

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SkillInfo {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub file_path: String,
    pub source: String,        // "claude" | "cursor" | "cline" | "aider" | "general" | "custom"
    pub target_agents: Vec<String>,
    pub tags: Vec<String>,
    pub last_modified: Option<String>,
}

#[tauri::command]
pub async fn skills_list() -> Result<Vec<SkillInfo>, String> {
    // TODO Phase 13.2: 扫 C:\HomeBase\Skills\* + ~/.claude/skills/ 解析 SKILL.md frontmatter
    tracing::debug!("skills_list 调用 (stub)");
    Ok(vec![])
}
