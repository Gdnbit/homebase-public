//! 共享 Skills Tauri 命令（S4 · 2026-05-06）
//!
//! 暴露 3 个命令给前端：
//! - `shared_skills_list` · 列 ~/.homebase/shared/skills/
//! - `shared_skills_apply` · 复制到 claude / openclaw 的 skills/ 目录（带 homebase- 前缀）
//! - `shared_skills_open_dir` · 打开 ~/.homebase/shared/skills/ 在文件管理器（让 USER 用 VSCode/Obsidian 编辑）

use crate::adapters::skills_pool::{self, SkillInfo, SkillsReconcileReport};
use std::path::PathBuf;

#[tauri::command]
pub async fn shared_skills_list() -> Result<Vec<SkillInfo>, String> {
    tracing::debug!("shared_skills_list 调用");
    skills_pool::list_skills()
}

#[tauri::command]
pub async fn shared_skills_apply() -> Result<SkillsReconcileReport, String> {
    tracing::debug!("shared_skills_apply 调用");
    Ok(skills_pool::reconcile_all())
}

/// 用系统默认文件管理器打开 ~/.homebase/shared/skills/
/// （让 USER 用 VSCode/Obsidian 直接编辑 SKILL.md · UI 阶段先不做编辑器）
#[tauri::command]
pub async fn shared_skills_open_dir() -> Result<String, String> {
    let root: PathBuf = skills_pool::shared_skills_root();
    std::fs::create_dir_all(&root).map_err(|e| format!("create_dir: {e}"))?;
    let path_str = root.to_string_lossy().into_owned();

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        use std::os::windows::process::CommandExt;
        Command::new("explorer.exe")
            .arg(&path_str)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("spawn explorer: {e}"))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        // macOS / Linux 兜底
        use std::process::Command;
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        Command::new(opener)
            .arg(&path_str)
            .spawn()
            .map_err(|e| format!("spawn {opener}: {e}"))?;
    }

    Ok(path_str)
}
