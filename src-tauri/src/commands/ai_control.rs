use serde::Serialize;
use std::path::PathBuf;

#[derive(Serialize)]
pub struct AiControlBootstrap {
    pub files: crate::control_bootstrap::BootstrapFiles,
    pub service: crate::control_plane::ServiceStatus,
    pub total: usize,
    pub available: usize,
    pub blocked: usize,
    pub unavailable: usize,
}

/// AI 启动提示词、受验证的操作文件位置及本机控制状态；不返回认证信息。
#[tauri::command]
pub fn ai_control_bootstrap() -> Result<AiControlBootstrap, String> {
    let catalog: serde_json::Value =
        serde_json::from_str(crate::control_plane::CATALOG).map_err(|_| "控制目录不可读取")?;
    let entries = catalog["commands"].as_array().ok_or("控制目录无效")?;
    let mut candidates = Vec::new();
    for path in [
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(PathBuf::from)),
        std::env::current_dir().ok(),
    ]
    .into_iter()
    .flatten()
    {
        candidates.extend(path.ancestors().take(8).map(PathBuf::from));
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf(),
    );
    let explicit = std::env::var_os("HOMEBASE_CONTROL_PROJECT_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    Ok(AiControlBootstrap {
        files: crate::control_bootstrap::discover(
            explicit,
            &candidates,
            catalog["catalog_hash"].as_str().unwrap_or(""),
        ),
        service: crate::control_plane::service_status(),
        total: entries.len(),
        available: entries
            .iter()
            .filter(|entry| entry["status"] == "available")
            .count(),
        blocked: entries
            .iter()
            .filter(|entry| entry["status"] == "blocked")
            .count(),
        unavailable: entries
            .iter()
            .filter(|entry| entry["status"] == "unavailable")
            .count(),
    })
}
