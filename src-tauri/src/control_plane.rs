//! Local CLI adapter over the same business commands used by the desktop UI.
use crate::control_protocol::{ControlError, Handler, Server};
use crate::control_state::{Controller, Runner};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex, Weak};

#[derive(Clone, Serialize)]
pub struct ServiceStatus {
    pub state: String,
    pub message: String,
}
static STATUS: Mutex<Option<ServiceStatus>> = Mutex::new(None);
static CONTROLLER: Mutex<Option<Weak<Controller>>> = Mutex::new(None);
fn set_status(state: &str, message: &str) {
    if let Ok(mut status) = STATUS.lock() {
        *status = Some(ServiceStatus {
            state: state.into(),
            message: message.into(),
        });
    }
}
pub fn disabled() {
    set_status("disabled", "控制服务已通过环境设置关闭");
}
pub fn service_status() -> ServiceStatus {
    let status = STATUS
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .unwrap_or(ServiceStatus {
            state: "starting".into(),
            message: "正在启动本机控制服务".into(),
        });
    if status.state == "ready" {
        let controller = CONTROLLER
            .lock()
            .ok()
            .and_then(|value| value.as_ref().and_then(Weak::upgrade));
        match controller {
            Some(controller) if controller.healthy() => return status,
            Some(_) => {
                return ServiceStatus {
                    state: "error".into(),
                    message: "控制服务账本异常，已停止接受新操作".into(),
                }
            }
            None => {
                return ServiceStatus {
                    state: "error".into(),
                    message: "控制服务已停止，请重新启动应用".into(),
                }
            }
        }
    }
    status
}

fn decode_arg<T: DeserializeOwned>(
    args: &Map<String, Value>,
    name: &str,
    rust_name: &str,
    optional: bool,
) -> Result<T, ControlError> {
    let value = args.get(name).or_else(|| args.get(rust_name));
    if value.is_none() && !optional {
        return Err(ControlError::new(
            "BAD_ARGUMENT",
            &format!("缺少参数 {name}"),
        ));
    }
    serde_json::from_value(value.cloned().unwrap_or(Value::Null)).map_err(|_| {
        ControlError::new(
            "BAD_ARGUMENT",
            &format!("参数 {name} 的类型或字段无效，请用 hb describe 查看接口 Schema"),
        )
    })
}
fn encode_result<T: Serialize>(value: T) -> Result<Value, ControlError> {
    serde_json::to_value(value)
        .map_err(|_| ControlError::new("ENCODE_FAILED", "操作结果无法转换为 JSON"))
}
fn command_error(error: String) -> ControlError {
    if let Some((code,message))=crate::development_workspace::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::inbox_workspace::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::inbox_receipts::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::inbox_storage::public_error(&error){return ControlError::new(code,message);}
    if let Some(failure)=crate::model_capability_io::reliability::Failure::decode(&error){return failure.control_error();}
    if let Some((code,message))=crate::capability_discovery::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::model_capabilities::vision::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::assistant_history::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::assistant::session_catalog::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::assistant::window::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::assistant_state_store::public_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::assistant::artifact_error(&error){return ControlError::new(code,message);}
    if let Some((code,message))=crate::assistant::session_create::public_error(&error){return ControlError::new(code,message);}
    if let Some((code, _)) = error.split_once(':') {
        if let Some(message) = crate::wechat::clip_video::trim_failure_message(code)
            .or_else(|| crate::wechat::video_cover::failure_message(code))
        {
            return ControlError::new(code, message);
        }
    }
    if error.starts_with("QUERY_FAILED:") {
        ControlError::new("QUERY_FAILED", "查询未能完整读取数据，请检查对应目录、权限或系统依赖；不能判定为空或正常")
    } else if error.starts_with("QUERY_INVALID:") {
        ControlError::new("QUERY_INVALID", "查询结果或配置格式无效，请核对数据；未静默忽略或回退默认值")
    } else {
        ControlError::new("COMMAND_FAILED", "命令执行失败，请检查参数、依赖及应用日志")
    }
}
include!("control_dispatch.generated.rs");

// Internal channel adapter: reuse the exact controller ledger, policy and jobs.
pub async fn channel_request(request: crate::control_protocol::Request) -> Result<Value, ControlError> {
    let controller = CONTROLLER.lock().ok()
        .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
        .ok_or_else(|| ControlError::new("NOT_READY", "本机控制服务尚未就绪"))?;
    controller.handle(request).await
}

pub(crate) async fn assistant_job_receipt(request: &crate::control_protocol::Request) -> Result<Value, ControlError> {
    let controller = CONTROLLER.lock().ok()
        .and_then(|slot| slot.as_ref().and_then(Weak::upgrade))
        .ok_or_else(|| ControlError::new("NOT_READY", "本机控制账本尚未就绪"))?;
    controller.assistant_receipt(request).await
}

pub async fn run(app: tauri::AppHandle) -> Result<(), ControlError> {
    set_status("starting", "正在启动本机控制服务");
    let result = run_inner(app).await;
    if let Err(error) = &result {
        set_status("error", &error.message);
    } else {
        set_status("stopped", "控制服务已停止");
    }
    result
}
async fn run_inner(app: tauri::AppHandle) -> Result<(), ControlError> {
    let directory = crate::adapters::homebase_home_dir()
        .join(".homebase")
        .join("control");
    let catalog: Value = serde_json::from_str(CATALOG)
        .map_err(|_| ControlError::new("CATALOG_INVALID", "控制命令目录损坏"))?;
    let server = Server::bind(&directory, catalog["catalog_hash"].as_str().unwrap_or("")).await?;
    crate::commands::wechat_video::configure(app.clone());
    let runner: Runner = Arc::new(move |name, args| {
        let app = app.clone();
        Box::pin(async move { dispatch(&app, &name, &args).await })
    });
    let controller = Controller::new(catalog, &directory, runner)?;
    if let Ok(mut slot) = CONTROLLER.lock() {
        *slot = Some(Arc::downgrade(&controller));
    }
    let handler: Handler = Arc::new(move |request| {
        let controller = controller.clone();
        Box::pin(async move {
            let rollout = request.method == "rollout.status";
            let mut reply = controller.handle(request).await?;
            if rollout {
                reply["wechat"] = json!(crate::commands::wechat::rollout_activity());
            }
            Ok(reply)
        })
    });
    tracing::info!("HomeBase 本机控制服务已启动（认证信息仅写入受保护的本机端点文件）");
    set_status("ready", "本机控制服务已就绪");
    tauri::async_runtime::spawn(crate::commands::wechat::resume());
    server.serve(handler).await
}
