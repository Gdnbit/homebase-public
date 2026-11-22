//! All metadata/model/control traffic here is synthetic and locally owned.
use super::*;
use crate::capability_discovery::{self as discovery, DiscoveryRequest, DiscoveryStore};

#[test]
fn every_model_describe_is_self_contained_and_fits_without_preview_truncation() {
    fn check(value: &Value, defs: &Value) {
        match value {
            Value::Object(map) => {
                if let Some(r) = map.get("$ref").and_then(Value::as_str) {
                    assert!(
                        defs.get(r.strip_prefix("#/$defs/").unwrap()).is_some(),
                        "unresolved {r}"
                    );
                }
                for child in map.values() {
                    check(child, defs);
                }
            }
            Value::Array(a) => {
                for v in a {
                    check(v, defs);
                }
            }
            _ => (),
        }
    }
    for c in catalog()["commands"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        if !allowed(name) {
            continue;
        }
        let d = describe_contract(name).unwrap_or_else(|e|panic!("{name}: {e}"));
        assert!(d.to_string().len() <= 9000, "{name}");
        for key in ["input_schema", "output_schema"] {
            check(&d[key], &d[key]["$defs"]);
        }
    }
    assert!(describe_contract("assistant_config_get").is_err());
    let query = describe_contract("capability_index_query").unwrap();
    let reference=query["input_schema"]["properties"]["request"]["$ref"].as_str().unwrap().strip_prefix("#/$defs/").unwrap();
    assert_eq!(query["input_schema"]["$defs"][reference]["properties"]["limit"]["maximum"],40);
}

fn tool_result(messages: &[Value]) -> Value {
    let text = messages.iter().rev().find(|m| m["role"] == "tool").unwrap()["content"]
        .as_str()
        .unwrap();
    serde_json::from_str(text).unwrap()
}
#[tokio::test]
async fn simulated_wechat_discovers_from_large_catalog_calls_once_and_restores_original_result() {
    let (base, _, _, saved, _root) = fixture(false, vec![]).await;
    let mut raw=(0..1200).map(|i|json!({"id":format!("fixture-{i}"),"function":"text","kind":"model","name":format!("测试模型{i}"),"provider":"fixture","state":"not_verified","cost":"unknown","discover":"model_capability_resolve","target":format!("fixture-{i}")})).collect::<Vec<_>>();
    raw.push(json!({"id":"fixture-marked","function":"document","kind":"repository","name":"合成Marked","provider":"public-fixture","state":"verified","cost":"local_compute","discover":"repo_capability_describe","target":"fixture-marked"}));
    let mut store = DiscoveryStore::new();
    let catalog_id = store
        .insert(discovery::normalize(raw, vec![]), std::time::Instant::now())
        .unwrap();
    let store = Arc::new(Mutex::new(store));
    let runs = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(Value::Null));
    let output = base.workspace.join("capability-fixture.txt");
    std::fs::create_dir_all(&base.workspace).unwrap();
    let runner: Runner = Arc::new({
        let store = store.clone();
        let runs = runs.clone();
        let result = result.clone();
        let output = output.clone();
        let catalog_id = catalog_id.clone();
        move |command, args| {
            let store = store.clone();
            let runs = runs.clone();
            let result = result.clone();
            let output = output.clone();
            let catalog_id = catalog_id.clone();
            Box::pin(async move {
                match command.as_str() {
                    "capability_index_query" => {
                        let mut request: DiscoveryRequest =
                            serde_json::from_value(args["request"].clone()).unwrap();
                        assert!(request.snapshot_id.is_none());
                        if request.entry_keys.is_some() {
                            assert_eq!(request.catalog_id.as_deref(), Some(catalog_id.as_str()));
                        } else {
                            assert!(request.catalog_id.is_none(), "initial discovery must not receive a hidden catalog ID");
                        }
                        request.catalog_id = Some(catalog_id);
                        Ok(serde_json::to_value(
                            store
                                .lock()
                                .await
                                .page(&request, std::time::Instant::now())
                                .unwrap(),
                        )
                        .unwrap())
                    }
                    "repo_capability_describe" => {
                        assert_eq!(args["capabilityId"], "fixture-marked");
                        Ok(
                            json!({"status":"succeeded","capability":{"id":"fixture-marked","input_schema":{"type":"object","properties":{"markdown":{"type":"string"}},"required":["markdown"],"additionalProperties":false},"examples":[{"markdown":"仅合成内容"}],"limits":{"network":false},"verification":{"passed":true}}}),
                        )
                    }
                    "repo_capability_run" => {
                        assert_eq!(args["request"]["capability_id"], "fixture-marked");
                        assert!(args["request"]["request_id"]
                            .as_str()
                            .unwrap()
                            .starts_with("assistant-"));
                        assert_eq!(
                            runs.fetch_add(1, Ordering::SeqCst),
                            0,
                            "a repeated run must reuse its original result"
                        );
                        let bytes = "这是隔离能力产物🙂".as_bytes();
                        std::fs::write(&output, bytes).unwrap();
                        let value = json!({"status":"succeeded","run_id":"fixture-run-original","stdout":"已生成隔离产物","artifact":{"path":output.to_string_lossy(),"bytes":bytes.len(),"sha256":format!("{:x}",Sha256::digest(bytes)),"mime_type":"text/plain"}});
                        *result.lock().await = value.clone();
                        Ok(value)
                    }
                    "repo_capability_result" => {
                        assert_eq!(args["runId"], "fixture-run-original");
                        Ok(result.lock().await.clone())
                    }
                    _ => panic!("unexpected discovery fixture command {command}"),
                }
            })
        }
    });
    std::fs::create_dir_all(_root.path().join("discovery-control")).unwrap();
    let controller = Controller::new(
        catalog().clone(),
        &_root.path().join("discovery-control"),
        runner,
    )
    .unwrap();
    let control: Control = Arc::new(move |r| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(r).await })
    });
    let models = Arc::new(AtomicUsize::new(0));
    let run_job = Arc::new(Mutex::new(String::new()));
    let selected_target = Arc::new(Mutex::new(String::new()));
    let original_run = Arc::new(Mutex::new(String::new()));
    let model: Model = Arc::new({
        let models = models.clone();
        move |r| {
            let models = models.clone();
            let run_job = run_job.clone();
            let selected_target = selected_target.clone();
            let original_run = original_run.clone();
            Box::pin(async move {
                let n = models.fetch_add(1, Ordering::SeqCst);
                let (name, args) = match n {
                    0 => {
                        let prompt = r.messages.iter().filter_map(|m| m["content"].as_str()).collect::<Vec<_>>().join("\n");
                        assert!(prompt.contains("用合成Marked作为默认实现"));
                        assert!(prompt.contains("background_not_authority"));
                        ("homebase_discover", json!({"module":"capability_index"}))
                    }
                    1 => (
                        "homebase_describe",
                        json!({"command":"capability_index_query"}),
                    ),
                    2 => {
                        let contract = tool_result(&r.messages);
                        assert!(contract["input_schema"]["$defs"].is_object());
                        assert!(contract["output_schema"]["$defs"].is_object());
                        (
                            "homebase_call",
                            json!({"command":"capability_index_query","args":{"request":{"query":"Marked","function":"document","offset":0,"limit":1}}}),
                        )
                    }
                    3 => {
                        let page = tool_result(&r.messages);
                        assert_eq!(page["result"]["total"], 1);
                        assert!(page["result"]["snapshot_id"].is_string());
                        assert_eq!(page["result"]["entries"][0]["target"], "fixture-marked");
                        (
                            "homebase_call",
                            json!({"command":"capability_index_query","args":{"request":{
                                "catalog_id":page["result"]["catalog_id"], "function":"document",
                                "entry_keys":[page["result"]["entries"][0]["entry_key"], "0".repeat(64)]
                            }}}),
                        )
                    }
                    4 => {
                        let exact = tool_result(&r.messages);
                        assert_eq!(exact["result"]["total"], 1);
                        assert_eq!(exact["result"]["unmatched_entry_keys"], json!(["0".repeat(64)]));
                        *selected_target.lock().await = exact["result"]["entries"][0]["target"]
                            .as_str()
                            .unwrap()
                            .into();
                        assert!(exact["result"].get("truncated").is_none());
                        (
                            "homebase_describe",
                            json!({"command":"repo_capability_describe"}),
                        )
                    }
                    5 => (
                        "homebase_call",
                        json!({"command":"repo_capability_describe","args":{"capabilityId":selected_target.lock().await.clone()}}),
                    ),
                    6 => {
                        let description = tool_result(&r.messages);
                        assert_eq!(
                            description["result"]["capability"]["input_schema"]["properties"]
                                ["markdown"]["type"],
                            "string"
                        );
                        (
                            "homebase_describe",
                            json!({"command":"repo_capability_run"}),
                        )
                    }
                    7 => (
                        "homebase_call",
                        json!({"command":"repo_capability_run","args":{"request":{"capability_id":selected_target.lock().await.clone(),"input":{"markdown":"仅合成内容"}}}}),
                    ),
                    8 => {
                        let result = tool_result(&r.messages);
                        let job = result["job_id"].as_str().unwrap().to_owned();
                        *run_job.lock().await = job.clone();
                        assert_eq!(result["result"]["status"], "succeeded");
                        *original_run.lock().await =
                            result["result"]["run_id"].as_str().unwrap().into();
                        ("homebase_job", json!({"job_id":job}))
                    }
                    9 => (
                        "homebase_call",
                        json!({"command":"repo_capability_result","args":{"runId":original_run.lock().await.clone()}}),
                    ),
                    10 => (
                        "homebase_deliver",
                        json!({"job_id":run_job.lock().await.clone()}),
                    ),
                    11 => return Ok(reply(vec![])),
                    _ => panic!("duplicate model execution after recovery"),
                };
                Ok(workflow_tests::tool_reply(n, name, args))
            })
        }
    });
    let mut state = saved.lock().await.clone();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.max_rounds = 16;
    state.personal.save_memory(crate::assistant_personal::MemoryInput {
        revision: state.personal.revision, id: None, title: "Markdown 转换偏好".into(),
        content: "用合成Marked作为默认实现。".into(), expires_at: None,
    }, chrono::Utc::now()).unwrap();
    let e = Engine::new(
        state,
        base.save.clone(),
        model,
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let message = json!({"operation":"submit","source_key":"discovery-wechat-fixture","request_id":"discovery-original-message","text":"帮我按以前的方式转换这段 Markdown：仅合成内容。把结果发给我。"});
    let ack = e.channel(message.clone()).await.unwrap();
    let tid = ack["turn_id"].as_str().unwrap();
    let done = workflow_tests::task_wait(&e, tid, "completed").await;
    let desktop = e.turn(tid).await.unwrap();
    assert_eq!(desktop.artifacts.len(), 1);
    assert_eq!(done.artifacts[0].sha256, desktop.artifacts[0].sha256);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    let restored = Engine::new(
        saved.lock().await.clone(),
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(restored.channel(message).await.unwrap()["turn_id"], tid);
    let channel = restored
        .channel(json!({"operation":"get","source_key":"discovery-wechat-fixture","turn_id":tid}))
        .await
        .unwrap();
    assert_eq!(channel["result"]["status"], "succeeded");
    assert_eq!(
        channel["result"]["artifacts"][0]["sha256"],
        desktop.artifacts[0].sha256
    );
    assert!(restored
        .channel(json!({"operation":"get","source_key":"other-wechat-owner","turn_id":tid}))
        .await
        .is_err());
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 12);
}
