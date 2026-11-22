//! Tasks 模块命令（v3 §21.1）⭐ 新
//!
//! 替代散落的 TODO 注释 / progress.txt / 草稿 md。
//! 与 Knowledge / Projects / Chat Hub 互通（context 注入）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub status: String,   // "pending" | "in_progress" | "done" | "cancelled"
    pub priority: String, // "high" | "medium" | "low"
    pub project_id: Option<i64>,
    pub knowledge_doc_id: Option<String>,
    pub parent_task_id: Option<i64>,
    pub estimated_minutes: Option<i32>,
    pub actual_minutes: Option<i32>,
    pub due_date: Option<String>,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[tauri::command]
pub async fn tasks_list() -> Result<Vec<TaskInfo>, String> {
    // TODO Phase 13.2: SQLite SELECT FROM tasks
    tracing::debug!("tasks_list 调用 (stub)");
    Ok(vec![])
}
