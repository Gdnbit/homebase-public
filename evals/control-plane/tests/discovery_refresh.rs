//! Fresh directory queries must refresh; repeated writes still execute once.
use homebase_control_tests::{
    assistant::{Control, Model, SendRequest, State},
    assistant_protocol,
    capability_discovery::{self, DiscoveryRequest, DiscoveryStore},
    control_state::{Controller, Runner},
    test_support::{legacy_engine, Save},
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::sync::Mutex;

#[tokio::test]
async fn explicit_fresh_discovery_is_not_reused_as_a_mutating_effect_but_duplicate_write_is() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let writes = Arc::new(AtomicUsize::new(0));
    let snapshots = Arc::new(Mutex::new(DiscoveryStore::new()));
    let marker = workspace.join("only-once.txt");
    let runner: Runner = Arc::new({
        let reads = reads.clone();
        let writes = writes.clone();
        move |command, args| {
            let reads = reads.clone();
            let writes = writes.clone();
            let snapshots = snapshots.clone();
            let marker = marker.clone();
            Box::pin(async move {
                match command.as_str() {
                    "capability_index_query" => {
                        let n = reads.fetch_add(1, Ordering::SeqCst) + 1;
                        let mut request: DiscoveryRequest =
                            serde_json::from_value(args["request"].clone()).unwrap();
                        assert!(request.catalog_id.is_none());
                        let index = capability_discovery::normalize(
                            vec![
                                json!({"id":"fixture","function":"text","kind":"model","name":format!("第{n}次目录读取"),"provider":"fixture","state":"not_verified","cost":"unknown","discover":"model_capability_resolve","target":"fixture"}),
                            ],
                            vec![],
                        );
                        let mut snapshots = snapshots.lock().await;
                        request.catalog_id = Some(snapshots.insert(index, Instant::now()).unwrap());
                        Ok(
                            serde_json::to_value(snapshots.page(&request, Instant::now()).unwrap())
                                .unwrap(),
                        )
                    }
                    "assistant_local_execute" => {
                        assert_eq!(args["operation"], "files");
                        assert_eq!(args["request"]["action"], "write");
                        writes.fetch_add(1, Ordering::SeqCst);
                        std::fs::write(marker, b"synthetic-only").unwrap();
                        Ok(json!({"status":"succeeded"}))
                    }
                    _ => panic!("unexpected isolated command {command}"),
                }
            })
        }
    });
    let catalog: Value =
        serde_json::from_str(include_str!("../../../control/commands.json")).unwrap();
    let controller = Controller::new(catalog, root.path(), runner).unwrap();
    let control: Control = Arc::new(move |r| {
        let c = controller.clone();
        Box::pin(async move { c.handle(r).await })
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let model: Model = Arc::new(move |_| {
        let n = calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let call = match n {
                0 | 1 => Some((
                    "homebase_call",
                    json!({"command":"capability_index_query","args":{"request":{"offset":0,"limit":1}}}),
                )),
                2 | 3 => Some((
                    "homebase_files",
                    json!({"action":"write","path":"only-once.txt","content":"synthetic-only"}),
                )),
                4 => None,
                _ => panic!("unexpected repeated model turn"),
            };
            let message = if let Some((name, args)) = call {
                json!({"tool_calls":[{"id":format!("fixture-{n}"),"function":{"name":name,"arguments":args.to_string()}}]})
            } else {
                json!({"content":"已核对两次最新目录，测试文件只保存一次。"})
            };
            Ok(
                assistant_protocol::parse("chat", &json!({"choices":[{"message":message}]}))
                    .unwrap(),
            )
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.route_id = "synthetic-only".into();
    let saved = Arc::new(Mutex::new(state.clone()));
    let save: Save = Arc::new(move |s| {
        let saved = saved.clone();
        Box::pin(async move {
            *saved.lock().await = s;
            Ok(())
        })
    });
    let e = legacy_engine(state, save, model, control, workspace)
        .await
        .unwrap();
    let request:SendRequest=serde_json::from_value(json!({"text":"显式查询两次最新目录，然后保存一次合成文件。","request_id":"fresh-discovery-idempotence"})).unwrap();
    let ack = e.send(request).await.unwrap();
    let done = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let t = e.turn_with_progress(&ack.turn_id).await.unwrap();
            if ["completed", "failed", "awaiting_confirmation"].contains(&t.state.as_str()) {
                break t;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(done.state, "completed", "{}", done.text);
    assert_eq!(
        reads.load(Ordering::SeqCst),
        2,
        "an explicit fresh query must not replay its stale prior result"
    );
    assert_eq!(
        writes.load(Ordering::SeqCst),
        1,
        "duplicate mutating effects must still reuse their first result"
    );
    assert_ne!(
        done.steps[0].result["result"]["catalog_id"],
        done.steps[1].result["result"]["catalog_id"]
    );
}
