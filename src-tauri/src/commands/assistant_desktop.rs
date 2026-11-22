//! Private client workspace. Does not alter shared model settings or resume work.
use crate::assistant::{ImageReference, SendRequest};
use crate::assistant_desktop::{DesktopBody, DesktopReply, DesktopStore, NativeMessage};
use serde::Deserialize;
use std::sync::OnceLock;

pub(crate) fn store() -> &'static DesktopStore {
    static STORE: OnceLock<DesktopStore> = OnceLock::new();
    STORE.get_or_init(|| {
        DesktopStore::new(crate::adapters::homebase_home_dir().join(".homebase/assistant/desktop"))
    })
}
pub(crate) async fn validate_scope(scope: &str) -> Result<(), String> {
    DesktopStore::validate_scope(scope)?;
    // Native scopes contain only local client input. They grant no native thread
    // read/send authority; the original provider adapter validates those actions.
    if scope.starts_with("native:") {
        return Ok(());
    }
    super::assistant::service()
        .await?
        .desktop_scope(scope)
        .await
}
async fn validate_draft_scope(scope:&str)->Result<(),String>{
    DesktopStore::validate_scope(scope)?;
    if scope.starts_with("handoff:"){return Ok(());}
    validate_scope(scope).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopSaveRequest {
    pub scope: String,
    pub revision: u64,
    pub body: DesktopBody,
}

#[tauri::command]
pub async fn assistant_desktop_index() -> Result<DesktopReply, String> {
    Ok(match store().index() {
        Ok(index) => DesktopReply::index(index),
        Err(message) => DesktopReply::failure("DRAFT_INDEX_FAILED", message),
    })
}

#[tauri::command]
pub async fn assistant_desktop_get(scope: String) -> Result<DesktopReply, String> {
    if let Err(message) = validate_draft_scope(&scope).await {
        return Ok(DesktopReply::failure("SCOPE_INVALID", message));
    }
    Ok(match store().get(&scope) {
        Ok(entry) => DesktopReply::entry("loaded", entry),
        Err(message) => DesktopReply::failure("DRAFT_READ_FAILED", message),
    })
}

#[tauri::command]
pub async fn assistant_desktop_save(request: DesktopSaveRequest) -> Result<DesktopReply, String> {
    if let Err(message) = validate_draft_scope(&request.scope).await {
        return Ok(DesktopReply::failure("SCOPE_INVALID", message));
    }
    Ok(
        match store()
            .save(&request.scope, request.revision, request.body)
            .await
        {
            Ok(entry) => DesktopReply::entry("saved", entry),
            Err(message) => DesktopReply::failure("DRAFT_SAVE_FAILED", message),
        },
    )
}

/// 加密保存当前桌面草稿的 PNG，返回归属引用，不发起模型调用。
#[tauri::command]
pub async fn assistant_desktop_image_stage(
    scope: String,
    name: String,
    png_base64: String,
) -> Result<ImageReference, String> {
    super::assistant::service().await?.stage_image(scope, name, &png_base64).await
}

#[tauri::command]
pub async fn assistant_desktop_send(
    request: SendRequest,
    revision: u64,
) -> Result<DesktopReply, String> {
    // Resolve an existing receipt before consulting a changing route catalog. A
    // removed model must not hide the already accepted original task.
    let engine = super::assistant::service().await?;
    if let Ok(None) = engine.desktop_receipt(&request).await {
        if let Err(message) = super::assistant::validate_route(&request) {
            return Ok(DesktopReply::failure("MODEL_ROUTE_INVALID", message));
        }
    }
    Ok(store().transmit(engine, revision, request).await)
}

#[tauri::command]
pub async fn assistant_desktop_resolve(
    scope: String,
    request: Option<SendRequest>,
) -> Result<DesktopReply, String> {
    let engine = super::assistant::service().await?;
    if let Err(message) = engine.desktop_scope(&scope).await {
        return Ok(DesktopReply::failure("SCOPE_INVALID", message));
    }
    if let Some(request) = request {
        if request.session_id.as_deref().unwrap_or("new") != scope {
            return Ok(DesktopReply::failure(
                "SCOPE_INVALID",
                "原请求与草稿会话不一致".into(),
            ));
        }
        match engine.desktop_receipt(&request).await {
            Ok(Some(ack)) => return Ok(store().accept(&scope, &request, ack).await),
            Err(message) => return Ok(DesktopReply::failure("RECEIPT_CONFLICT", message)),
            Ok(None) => {}
        }
    }
    Ok(store().recover(engine, &scope).await)
}

async fn native_reply(request: &NativeMessage, result: serde_json::Value) -> DesktopReply {
    let accepted = result["state"] == "accepted";
    let mut reply = if accepted {
        match store().native_ack(request).await {
            Ok(entry) => DesktopReply::entry("accepted", entry),
            Err(message) => {
                let mut reply = DesktopReply::failure(
                    "NATIVE_ACK_CLEANUP_FAILED",
                    "原生消息已受理，但本机清理未确认；请查询原请求，不要换编号重发".into(),
                );
                reply.status = "partial".into();
                reply.entry = store().get(&request.scope()).ok();
                reply.warning = Some(message);
                reply
            }
        }
    } else {
        match store().get(&request.scope()) {
            Ok(entry) => DesktopReply::entry("unknown", entry),
            Err(message) => DesktopReply::failure("DRAFT_READ_FAILED", message),
        }
    };
    if !accepted {
        reply.message = "原生请求受理结果未知，原编号保留；查询原会话，不会自动重发".into();
    }
    reply.native_result = Some(result);
    reply
}

#[tauri::command]
pub async fn assistant_native_send(
    request: NativeMessage,
    revision: u64,
) -> Result<DesktopReply, String> {
    if let Err(message) = request.validate() {
        return Ok(DesktopReply::failure("NATIVE_REQUEST_INVALID", message));
    }
    let existing = match super::conversations::send_receipt(&request).await {
        Ok(receipt) => receipt,
        Err(message) => return Ok(DesktopReply::failure("NATIVE_RECEIPT_FAILED", message)),
    };
    if let Some(result) = existing.as_ref().filter(|r| r["state"] == "accepted") {
        return Ok(native_reply(&request, result.clone()).await);
    }
    // This must finish before the original native adapter may start/resume/send.
    if let Err(message) = store().native_prepare(revision, request.clone()).await {
        let mut reply = DesktopReply::failure("NATIVE_PREPARE_FAILED", message);
        if let Some(result) = existing {
            reply.status = "unknown".into();
            reply.warning = Some(reply.message);
            reply.message =
                "原生已有未知回执，但本机保护保存未确认；不能当作未执行，请保留原编号核对".into();
            reply.native_result = Some(result);
        }
        return Ok(reply);
    }
    if let Some(result) = existing {
        return Ok(native_reply(&request, result).await);
    }
    let result =
        super::conversations::conversation_native(super::conversations::send_request(&request))
            .await;
    // Only the original durable provider receipt can settle an uncertain response.
    match super::conversations::send_receipt(&request).await {
        Ok(Some(receipt)) => Ok(native_reply(&request, receipt).await),
        receipt => {
            let mut reply = DesktopReply::entry("unknown", store().get(&request.scope())?);
            reply.code = Some("NATIVE_RECEIPT_UNKNOWN".into());
            reply.message =
                "原生提交尚未确认，原请求已保存；请查询原请求，修复依赖后只能显式重试同一编号"
                    .into();
            reply.warning = match (result, receipt) {
                (Err(error), _) | (_, Err(error)) => Some(error),
                _ => Some("未找到有效原生回执，不能按未执行或完成处理".into()),
            };
            Ok(reply)
        }
    }
}

#[tauri::command]
pub async fn assistant_native_resolve(
    scope: String,
    request: Option<NativeMessage>,
) -> Result<DesktopReply, String> {
    if !scope.starts_with("native:") || DesktopStore::validate_scope(&scope).is_err() {
        return Ok(DesktopReply::failure(
            "SCOPE_INVALID",
            "请指定有效的原生输入命名空间".into(),
        ));
    }
    let entry = match store().get(&scope) {
        Ok(entry) => entry,
        Err(message) => return Ok(DesktopReply::failure("DRAFT_READ_FAILED", message)),
    };
    let Some(request) = request.or_else(|| entry.native_pending.clone()) else {
        return Ok(DesktopReply::entry("loaded", entry));
    };
    if request.scope() != scope || request.validate().is_err() {
        return Ok(DesktopReply::failure(
            "SCOPE_INVALID",
            "原生请求与输入归属不一致，未查询或清理".into(),
        ));
    }
    match super::conversations::send_receipt(&request).await {
        Ok(Some(result)) => Ok(native_reply(&request, result).await),
        Ok(None) => {
            let mut reply = DesktopReply::entry("not_found", entry);
            reply.message =
                "尚未查到原生请求，不代表取消或未执行；原编号保留。核对依赖后可显式重试原请求"
                    .into();
            Ok(reply)
        }
        Err(message) => Ok(DesktopReply::failure("NATIVE_RECEIPT_FAILED", message)),
    }
}
