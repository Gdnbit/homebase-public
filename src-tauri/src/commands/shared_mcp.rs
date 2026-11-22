//! 共享 MCP 池 Tauri 命令（S2 · 2026-05-06）
//!
//! 暴露 4 个命令给前端：
//! - `shared_mcp_get` · 读 pool（含 servers + assignments）
//! - `shared_mcp_save` · 写 pool · 不自动 reconcile（让前端控制时机）
//! - `shared_mcp_apply` · 触发 reconcile · 把 pool 推到 4 agent 配置文件
//! - `shared_mcp_save_and_apply` · 写 + 立即 reconcile（"Apply" 按钮）

use crate::adapters::mcp_pool::{self, SharedMcpPool, AGENT_IDS};
use crate::adapters::mcp_reconcile::{self, ReconcileReport};

#[tauri::command]
pub async fn shared_mcp_get() -> Result<SharedMcpPool, String> {
    tracing::debug!("shared_mcp_get 调用");
    mcp_pool::load()
}

#[tauri::command]
pub async fn shared_mcp_save(pool: SharedMcpPool) -> Result<(), String> {
    tracing::debug!("shared_mcp_save 调用 · servers={}", pool.servers.len());
    mcp_pool::save(&pool)
}

#[tauri::command]
pub async fn shared_mcp_apply() -> Result<ReconcileReport, String> {
    tracing::debug!("shared_mcp_apply 调用");
    let pool = mcp_pool::load()?;
    Ok(mcp_reconcile::reconcile_all(&pool))
}

#[tauri::command]
pub async fn shared_mcp_save_and_apply(
    pool: SharedMcpPool,
) -> Result<ReconcileReport, String> {
    tracing::debug!(
        "shared_mcp_save_and_apply 调用 · servers={}",
        pool.servers.len()
    );
    mcp_pool::save(&pool)?;
    let pool_after = mcp_pool::load()?; // 走一遍 normalize
    Ok(mcp_reconcile::reconcile_all(&pool_after))
}

/// 给前端提供 agent 列表（避免前端硬编码）
#[tauri::command]
pub async fn shared_mcp_agent_ids() -> Result<Vec<String>, String> {
    Ok(AGENT_IDS.iter().map(|s| (*s).to_string()).collect())
}
