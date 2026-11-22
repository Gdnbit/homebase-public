//! SQLite types & Tauri command DTOs（v3 §7 + §21 schema）
//!
//! Phase 13.1 MVP：基础类型；Phase 13.2 加 Tasks/Skills/Chat 完整 DTO。

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EventRecord {
    pub id: i64,
    pub event_type: String,
    pub actor: Option<String>,
    pub target: Option<String>,
    pub details: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NotificationRecord {
    pub id: i64,
    pub severity: String, // "info" | "warning" | "error"
    pub title: String,
    pub body: String,
    pub read: bool,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AgentSession {
    pub id: i64,
    pub agent_name: String,
    pub pid: Option<i32>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_code: Option<i32>,
    pub log_path: Option<String>,
}
