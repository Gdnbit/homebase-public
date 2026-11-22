//! Real graph/host/WSL/control path, synthetic planner only. Never a provider request.
use super::*;
use crate::{
    assistant_local_host::LocalHost,
    control_state::{Controller, Runner},
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
#[ignore = "explicit isolated local runtime and WSL lab required"]
async fn local_agent_graph_file_terminal_delivery_and_followup_use_no_keys() {
    let parent = PathBuf::from(std::env::var_os("HOMEBASE_LOCAL_TEST_LAB").expect("lab"));
    assert!(parent
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    let root = parent.join(format!("graph-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    let scratch = workspace.join("agent");
    tokio::fs::create_dir_all(&scratch).await.unwrap();
    tokio::fs::write(scratch.join("seed.txt"),"SYNTHETIC PLAN INPUT").await.unwrap();
    tokio::fs::write(root.join("outside.txt"), "SYNTHETIC OUTSIDE SENTINEL")
        .await
        .unwrap();
    let runtime = PathBuf::from(std::env::var_os("HOMEBASE_LOCAL_RUNTIME").expect("runtime"));
    let host = LocalHost::start(&runtime, &root.join("profile"))
        .await
        .unwrap();
    let context = json!({"workspaces":[{"id":"scratch","path":scratch,"access":"write"}],"default_workspace":"scratch","output_root":workspace.join("local-results"),"sandbox":{"kind":"wsl-bubblewrap","distro":"Ubuntu","executable":std::env::var("HOMEBASE_BWRAP_EXE").unwrap(),"sha256":std::env::var("HOMEBASE_BWRAP_SHA256").unwrap()}});
    let effects = Arc::new(AtomicUsize::new(0));
    let count = effects.clone();
    let runner_host = host.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let host = runner_host.clone();
        let context = context.clone();
        let count = count.clone();
        Box::pin(async move {
            if command != "assistant_local_execute" {
                return Err(ControlError::new(
                    "FIXTURE_COMMAND",
                    "Only synthetic local operations",
                ));
            }
            count.fetch_add(1, Ordering::SeqCst);
            host.execute(
                args["operationId"].as_str().unwrap(),
                args["operation"].as_str().unwrap(),
                context,
                args["request"].clone(),
            )
            .await
            .map_err(|e| ControlError::new("LOCAL_FIXTURE", &e))
        })
    });
    tokio::fs::create_dir_all(root.join("control")).await.unwrap();
    let read_runner=runner.clone();
    let controller = Controller::new(catalog().clone(), &root.join("control"), runner).unwrap();
    let progress_host = host.clone();
    let control: Control = Arc::new(move |request| {
        let controller = controller.clone();
        let host = progress_host.clone();
        let read_runner=read_runner.clone();
        Box::pin(async move {
            if request.method=="assistant.local_read" {
                assert!(crate::assistant_local::readonly_operation(request.args["operation"].as_str().unwrap(),&request.args["request"]));
                return read_runner("assistant_local_execute".into(),request.args.as_object().unwrap().clone()).await;
            }
            if request.method == "assistant.local_progress" {
                return Ok(host
                    .progress(request.args["operationId"].as_str().unwrap())
                    .await);
            }
            controller.handle(request).await
        })
    });
    let models = Arc::new(AtomicUsize::new(0));
    let calls = models.clone();
    let original_job = Arc::new(Mutex::new(String::new()));
    let job = original_job.clone();
    let evidence=Arc::new(Mutex::new(BTreeMap::<String,String>::new()));
    let model: Model = Arc::new(move |request| {
        let calls = calls.clone();
        let job = job.clone();
        let evidence=evidence.clone();
        Box::pin(async move {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            let definitions = request.definitions.as_ref().unwrap();
            assert!(definitions
                .iter()
                .any(|d| d["function"]["name"] == "homebase_terminal"));
            if (11..=14).contains(&n){
                let data:Value=serde_json::from_str(request.messages.iter().rev().find(|m|m["role"]=="tool").unwrap()["content"].as_str().unwrap()).unwrap();
                evidence.lock().await.insert(["write","execute","artifact","deliver"][n-11].into(),data["step_id"].as_str().unwrap().into());
                if n==13 {*job.lock().await=data["job_id"].as_str().unwrap().into();}
            }
            let (name, args) = match n {
                0=>("homebase_task",json!({"action":"set","revision":0,"goal":"隔离统计与交付","items":[
                    {"id":"write","title":"写入合成输入","done_when":"写入指定数据","kind":"write"},
                    {"id":"execute","title":"执行统计","done_when":"实际命令退出0","kind":"execute"},
                    {"id":"artifact","title":"登记统计文件","done_when":"生成可校验附件","kind":"artifact"},
                    {"id":"deliver","title":"准备返回本人","done_when":"使用原job安排交付","kind":"deliver"}]})),
                1..=9=>("homebase_files",json!({"action":"read","path":"seed.txt"})),
                10 => (
                    "homebase_files",
                    json!({"action":"write","path":"numbers.json","content":"[2,3,5]"}),
                ),
                11 => (
                    "homebase_terminal",
                    json!({"command":"python3 -c 'import json; from pathlib import Path; print(\"SYNTHETIC_GRAPH_READY\",flush=True); total=sum(json.loads(Path(\"numbers.json\").read_text())); Path(\"result.txt\").write_text(str(total))'","timeout_ms":10000}),
                ),
                12 => (
                    "homebase_files",
                    json!({"action":"export","path":"result.txt"}),
                ),
                13 => {
                    let last = request
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m["role"] == "tool")
                        .unwrap();
                    let data: Value =
                        serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
                    let id = data["job_id"].as_str().unwrap().to_string();
                    *job.lock().await = id.clone();
                    ("homebase_deliver", json!({"job_id":id}))
                }
                14..=17=>{
                    let stage=["write","execute","artifact","deliver"][n-14];
                    ("homebase_task",json!({"action":"update","revision":n-13,"item_id":stage,"state":"completed","evidence":[evidence.lock().await[stage]]}))
                },
                19 => (
                    "homebase_deliver",
                    json!({"job_id":job.lock().await.clone()}),
                ),
                18 | 20 => {
                    return wire::parse(
                        "chat",
                        &json!({"choices":[{"message":{"content":"统计文件已生成，结果是10；已准备好返回这份文件。"}}]}),
                    )
                }
                _ => return Err("Synthetic planner received an unexpected extra request".into()),
            };
            wire::parse(
                "chat",
                &json!({"choices":[{"message":{"tool_calls":[{"id":format!("local-fixture-{n}"),"function":{"name":name,"arguments":args.to_string()}}]}}]}),
            )
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.route_id = "synthetic-only".into();
    state.settings.runtime = "pi".into();
    state.settings.free_only=true;
    let snapshot = Arc::new(Mutex::new(state.clone()));
    let saved = snapshot.clone();
    let save: Save = Arc::new(move |state| {
        let saved = saved.clone();
        Box::pin(async move {
            *saved.lock().await = state;
            Ok(())
        })
    });
    let engine = Engine::new(state, save, model, control, workspace.clone())
        .await
        .unwrap();
    let first=engine.channel(json!({"operation":"submit","source_key":"synthetic-owner","text":"HB-SELFTEST-GRAPH 请统计合成数据并返回文件","request_id":"graph-first"})).await.unwrap();
    let tid = first["turn_id"].as_str().unwrap();
    let wait = |id: String| {
        let engine = engine.clone();
        async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
            loop {
                let turn = engine.turn(&id).await.unwrap();
                if terminal(&turn.state) {
                    assert_eq!(turn.state, "completed", "{}", turn.text);
                    return turn;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "Graph fixture timed out"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };
    let completed = wait(tid.into()).await;
    assert!(completed.task.as_ref().unwrap().plan.as_ref().unwrap().items.iter().all(|i|i.state=="completed"));
    assert_eq!(completed.jobs.len(), 3);
    assert_eq!(completed.artifacts.len(), 1);
    assert_eq!(
        tokio::fs::read_to_string(&completed.artifacts[0].path)
            .await
            .unwrap(),
        "10"
    );
    let result=engine.channel(json!({"operation":"get","source_key":"synthetic-owner","turn_id":tid,"include_files":true})).await.unwrap();
    assert_eq!(result["status"], "succeeded");
    let second=engine.channel(json!({"operation":"submit","source_key":"synthetic-owner","text":"请再发刚才的统计文件，不要重新执行","request_id":"graph-followup"})).await.unwrap();
    let followup = wait(second["turn_id"].as_str().unwrap().into()).await;
    assert_eq!(followup.artifacts[0].sha256, completed.artifacts[0].sha256);
    assert_eq!(effects.load(Ordering::SeqCst), 12);
    assert_eq!(models.load(Ordering::SeqCst), 21);
    assert_eq!(
        tokio::fs::read_to_string(root.join("outside.txt"))
            .await
            .unwrap(),
        "SYNTHETIC OUTSIDE SENTINEL"
    );
    let state = snapshot.lock().await.clone();
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.sessions[0].source, "wechat");
    tokio::fs::write(root.join("validation.json"),serde_json::to_vec_pretty(&json!({"lab":root,"real_provider_calls":0,"real_keys":0,"graph_worker":true,"native_host":true,"wsl":true,"wechat_adapter":true,"desktop_shared_state":true,"owned_delivery":true,"followup_reuses_original":true,"effects":effects.load(Ordering::SeqCst)})).unwrap()).await.unwrap();
}
