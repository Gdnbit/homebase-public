//! Extensions 模块命令（v3 §9 + §23 — 原 Plugin 改名）
//!
//! 三者边界（v3 §23.1）：
//! - **Skill**：agent prompt + script 资产，跨 agent（由 commands::skills 管理）
//! - **Plugin**：业界统称，不在 HomeBase 内部使用
//! - **Extension**：HomeBase 自身 UI 模块单元（TOML manifest + UI + 可选 backend）

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ExtensionInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub kind: String, // "module" | "action" | "integration"
    pub enabled: bool,
    pub manifest_path: String,
    pub installed_at: Option<String>,
}

/// 列出已安装的 extensions（扫 extensions/ 目录）
#[tauri::command]
pub async fn extensions_list() -> Result<Vec<ExtensionInfo>, String> {
    // TODO Phase 13.2: 扫 extensions/_builtin/* + extensions/* 解析 extension.toml
    tracing::debug!("extensions_list 调用 (stub)");
    Ok(vec![])
}
