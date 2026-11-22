//! Synthetic business runner; real production control transport and job engine.
use homebase_control_tests::{
    control_protocol::{ControlError, Handler, Server},
    control_state::{Controller, Runner},
};
use serde_json::{json, Value};
use homebase_control_tests::task_event_store::Journal;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[tokio::main]
async fn main() {
    let directory = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .expect("explicit synthetic state directory required"),
    );
    assert!(directory.is_absolute() && directory.is_dir());
    let catalog: Value =
        serde_json::from_str(include_str!("../../../../control/commands.json")).unwrap();
    let server = Server::bind(&directory, catalog["catalog_hash"].as_str().unwrap())
        .await
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let journal = Arc::new(Mutex::new(Journal::default()));
    let runner: Runner = Arc::new(move |name, args| {
        let calls = calls.clone();
        let journal = journal.clone();
        Box::pin(async move {
            let number = calls.fetch_add(1, Ordering::SeqCst) + 1;
            match name.as_str() {
                "model_capability_resolve" | "model_capability_call" => {
                    use homebase_control_tests::model_capabilities::*;
                    let request:ModelCapabilityRequest=serde_json::from_value(args["request"].clone()).map_err(|_|ControlError::new("BAD_ARGUMENT","合成请求格式无效"))?;
                    let route=model("openrouter","openai/fixture","Fixture","openai",&["text"],"chat","https://example.com/docs","fixture");
                    if request.route_id!=route.route_id{return Err(ControlError::new("BAD_ARGUMENT","合成路由不存在"));}
                    let (endpoint,body)=prepare(&route,&request.input).map_err(|_|ControlError::new("BAD_ARGUMENT","模型或调用参数不匹配，未执行"))?;
                    let credential=CapabilityCredential{provider_id:"openrouter".into(),provider_name:"OpenRouter fixture".into(),credential_ref:"sops:fixture_openrouter".into(),source:"fixture".into(),state:"configured".into(),message:"synthetic only".into(),remote_state:"not_checked".into(),expires_at:None,expiry_warning:false};
                    if name=="model_capability_resolve"{return Ok(serde_json::to_value(ModelCapabilityPlan{route,credential,method:"POST".into(),endpoint,request:body,callable:true,message:"synthetic only".into()}).unwrap());}
                    let reply=if request.input["forceFailure"]==true{json!({"error":"synthetic failure"})}else{json!({"choices":[{"message":{"content":"synthetic model result"}}]})};
                    let(status,task_id)=outcome(&route,&reply,true);
                    Ok(serde_json::to_value(ModelCapabilityResult{provider_id:route.provider_id,model_id:route.model_id,credential_ref:credential.credential_ref,status,task_id,output:reply,artifact:None,message:"synthetic only; no provider request".into()}).unwrap())
                }
                "system_info" => Ok(json!({"os":"synthetic-fixture","calls":number})),
                "service_credentials_status" => Ok(json!([
                    {"service_id":"fal","state":"linked","remote_state":"not_checked","credential_ref":"api-key-manager:fal"},
                    {"service_id":"elevenlabs","state":"not_configured","remote_state":"not_checked","credential_ref":"api-key-manager:elevenlabs"}
                ])),
                "system_disks" => Err(ControlError::new("QUERY_INVALID", "合成查询失败；错误结果可用，不能当作无效任务")),
                "agents_status" => Ok(json!({"id":args.get("id"),"state":"synthetic"})),
                "agents_set_model" => {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    Ok(json!({"calls":number,"model":args.get("model")}))
                }
                "shared_mcp_get" => Ok(
                    json!({"api_key":"synthetic-hidden","token":"synthetic-hidden","env":{"KEY":"synthetic-hidden","REFERENCE":"${secrets.sample}"}}),
                ),
                "openrouter_models_catalog" => {
                    Ok(json!({"models":[],"source":"synthetic-no-network"}))
                }
                "studio_chat_send" => {
                    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    let id = uuid::Uuid::new_v4().to_string();
                    journal.lock().unwrap().record("studio:event",json!({"kind":"started","task_id":id,"session_id":"synthetic-session","cmd_preview":"synthetic-only"}));
                    let task_id = id.clone(); let store = journal.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        let mut store=store.lock().unwrap();
                        store.record("studio:event",json!({"kind":"assistant_text","task_id":task_id,"text":"Synthetic documentation-only result"}));
                        store.record("studio:event",json!({"kind":"completed","task_id":task_id,"session_id":"synthetic-session","exit_code":0}));
                        store.stream_closed("studio:event",&task_id);store.stream_closed("studio:event",&task_id);
                    });
                    Ok(json!({"session_id":"synthetic-session","task_id":id,"cmd_preview":"synthetic-only"}))
                }
                "task_events_get" => {
                    let page=journal.lock().unwrap().page(args["topic"].as_str().unwrap_or(""),args["taskId"].as_str().unwrap_or(""),args["afterSeq"].as_u64().unwrap_or(0),args["limit"].as_u64().unwrap_or(100) as usize);
                    Ok(serde_json::to_value(page).unwrap())
                }
                _ => Err(ControlError::new(
                    "FIXTURE_ONLY",
                    "夹具未实现此合成业务，未连接任何真实工作区",
                )),
            }
        })
    });
    let controller = Controller::new(catalog, &directory, runner).unwrap();
    let handler: Handler = Arc::new(move |request| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(request).await })
    });
    println!("READY");
    server.serve(handler).await.unwrap();
}
