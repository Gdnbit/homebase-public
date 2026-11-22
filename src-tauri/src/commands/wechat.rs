//! Desktop binding commands deliberately blocked from AI/CLI secret export.
use crate::wechat::{Api, Bridge, Control, PairingView, Save, Status, Stored};
use std::sync::Arc;
use tokio::sync::OnceCell;
static BRIDGE: OnceCell<Arc<Bridge>> = OnceCell::const_new();

pub(crate) fn rollout_activity() -> Option<crate::wechat::RolloutActivity> {
    BRIDGE.get().and_then(|bridge| bridge.rollout_activity())
}

pub(super) async fn bridge() -> Result<&'static Arc<Bridge>, String> {
    BRIDGE
        .get_or_try_init(|| async {
            let root = crate::adapters::homebase_home_dir()
                .join(".homebase")
                .join("wechat");
            std::fs::create_dir_all(&root).map_err(|_| "微信配置目录创建失败")?;
            let file = root.join("channel.dpapi");
            let stored: Stored = match std::fs::read(&file) {
                Ok(bytes) => {
                    if bytes.len() > 32 * 1024 * 1024 {
                        return Err("微信加密账本过大，未加载或覆盖".into());
                    }
                    let plain = crate::wechat_crypto::crypt(&bytes, false)?;
                    serde_json::from_slice(&plain)
                        .map_err(|_| "微信加密账本无法解析，未回退空配置")?
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Stored::default(),
                Err(_) => return Err("微信账本读取失败，原配置未覆盖".into()),
            };
            let save: Save = Arc::new(move |state| {
                let file = file.clone();
                Box::pin(async move {
                    let plain = zeroize::Zeroizing::new(
                        serde_json::to_vec(&state).map_err(|_| "微信账本编码失败")?,
                    );
                    if plain.len() > 24 * 1024 * 1024 {
                        return Err("微信账本达到容量上限，未覆盖".into());
                    }
                    let cipher = crate::wechat_crypto::crypt(&plain, true)?;
                    crate::control_protocol::publish_private(&file, &cipher)
                        .await
                        .map_err(|_| "微信加密账本保存失败".into())
                })
            });
            let control: Control =
                Arc::new(|request| Box::pin(crate::control_plane::channel_request(request)));
            let bridge = Bridge::new_with_pdf(
                stored,
                Api::new()?,
                save,
                control,
                root.join("workspace"),
                Some(Arc::new(|request| Box::pin(super::assistant::wechat(request)))),
                Some(Arc::new(|request| Box::pin(super::assistant::wechat_pdf(request)))),
            );
            bridge.configure_video(Arc::new(|request| Box::pin(super::wechat_video::handle(request))));
            Ok(bridge)
        })
        .await
}

pub async fn resume() {
    match bridge().await {
        Ok(b) => {
            if b.status().await.enabled {
                let _ = b.start().await;
            }
        }
        Err(_) => tracing::warn!("微信通道初始化失败，请在微信接入页检查；未修改原配置"),
    }
}

#[tauri::command]
pub async fn wechat_status() -> Result<Status, String> {
    Ok(bridge().await?.status().await)
}
#[tauri::command]
pub async fn wechat_start() -> Result<(), String> {
    bridge().await?.start().await
}
#[tauri::command]
pub async fn wechat_stop() -> Result<(), String> {
    bridge().await?.stop().await
}
#[tauri::command]
pub async fn wechat_self_test_message(text: String, request_id: String) -> Result<serde_json::Value, String> {
    bridge().await?.self_test_message(&text, &request_id).await
}
#[tauri::command]
pub async fn wechat_login_begin() -> Result<(), String> {
    bridge().await?.begin().await
}
#[tauri::command]
pub async fn wechat_pairing() -> Result<PairingView, String> {
    Ok(bridge().await?.pairing().await)
}
#[tauri::command]
pub async fn wechat_login_pin(pin: String) -> Result<(), String> {
    bridge().await?.pin(pin).await
}
#[tauri::command]
pub async fn wechat_login_cancel() -> Result<(), String> {
    bridge().await?.cancel_pairing().await
}
#[tauri::command]
pub async fn wechat_forget() -> Result<(), String> {
    bridge().await?.forget().await
}
