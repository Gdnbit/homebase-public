//! Shared desktop/WeChat core with real DPAPI persistence; all planner/tool data synthetic.
use homebase_control_tests::{
    assistant::{Control, Model, State},
    assistant_protocol,
    assistant_state_store::Store,
    control_state::{Controller, Runner},
    test_support::{legacy_engine, Save},
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;

#[tokio::test]
async fn segmented_wechat_task_desktop_read_restart_and_original_receipt_execute_once() {
    let root = tempfile::Builder::new()
        .prefix("segmented-engine-")
        .tempdir()
        .unwrap();
    let mut storage = Store::new(
        root.path().join("history-v1"),
        root.path().join("state.dpapi"),
    )
    .unwrap();
    assert!(storage.load_index().unwrap().is_none());
    let store = Arc::new(Mutex::new(storage));
    let save: Save = Arc::new({
        let store = store.clone();
        move |state| {
            let store = store.clone();
            Box::pin(async move {
                store
                    .lock()
                    .await
                    .save(serde_json::to_value(&state).unwrap())
                    .await?;
                Ok(())
            })
        }
    });
    let executions = Arc::new(AtomicUsize::new(0));
    let calls = executions.clone();
    let runner: Runner = Arc::new(move |command, _| {
        let calls = calls.clone();
        Box::pin(async move {
            assert_eq!(command, "system_info");
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(
                json!({"status":"succeeded","description":"synthetic system only","gpu_status":"not_detected","gpu_message":"未检测到GPU"}),
            )
        })
    });
    let catalog: Value =
        serde_json::from_str(include_str!("../../../control/commands.json")).unwrap();
    let control_root = root.path().join("control");
    std::fs::create_dir(&control_root).unwrap();
    let controller = Controller::new(catalog.clone(), &control_root, runner.clone()).unwrap();
    let control: Control = Arc::new(move |request| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(request).await })
    });
    let replies = VecDeque::from([
        json!({"choices":[{"message":{"tool_calls":[{"id":"synthetic-info","function":{"name":"homebase_call","arguments":json!({"command":"system_info","args":{}}).to_string()}}]}}]}),
        json!({"choices":[{"message":{"content":"合成系统信息已查到，没有调用外部模型。"}}]}),
    ]);
    let queue = Arc::new(Mutex::new(replies));
    let model_calls = Arc::new(AtomicUsize::new(0));
    let model: Model = Arc::new({
        let model_calls = model_calls.clone();
        move |_| {
            let queue = queue.clone();
            let model_calls = model_calls.clone();
            Box::pin(async move {
                model_calls.fetch_add(1, Ordering::SeqCst);
                assistant_protocol::parse(
                    "chat",
                    &queue
                        .lock()
                        .await
                        .pop_front()
                        .ok_or("synthetic planner exhausted")?,
                )
            })
        }
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.route_id = "synthetic-only".into();
    let engine = legacy_engine(
        state,
        save,
        model.clone(),
        control.clone(),
        root.path().join("workspace"),
    )
    .await
    .unwrap();
    let message = json!({"operation":"submit","source_key":"synthetic-wechat-owner","text":"请查询合成系统信息","request_id":"segmented-original-request"});
    let ack = engine.channel(message.clone()).await.unwrap();
    let tid = ack["turn_id"].as_str().unwrap();
    let mut finished = None;
    for _ in 0..200 {
        let turn = engine.turn_with_progress(tid).await.unwrap();
        if turn.state == "completed" {
            finished = Some(turn);
            break;
        }
        assert!(
            !["failed", "interrupted", "awaiting_confirmation"].contains(&turn.state.as_str()),
            "{}",
            serde_json::to_string(&turn).unwrap()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let finished = finished.expect("owned synthetic task must complete");
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(model_calls.load(Ordering::SeqCst), 2);
    assert_eq!(finished.jobs.len(), 1);
    let sid = engine.sessions().await[0].id.clone();
    let desktop = serde_json::to_value(engine.session(&sid).await.unwrap()).unwrap();
    let mut restarted_storage = Store::new(
        root.path().join("history-v1"),
        root.path().join("state.dpapi"),
    )
    .unwrap();
    let snapshot = restarted_storage.load_index().unwrap().unwrap();
    let restored: State =
        serde_json::from_value(restarted_storage.materialize(&snapshot).unwrap()).unwrap();
    drop(engine);
    drop(control);
    let controller = Controller::new(catalog, &control_root, runner).unwrap();
    let control: Control = Arc::new(move |request| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(request).await })
    });
    let restarted_store = Arc::new(Mutex::new(restarted_storage));
    let save: Save = Arc::new({
        let store = restarted_store.clone();
        move |state| {
            let store = store.clone();
            Box::pin(async move {
                store
                    .lock()
                    .await
                    .save(serde_json::to_value(&state).unwrap())
                    .await?;
                Ok(())
            })
        }
    });
    let restarted = legacy_engine(
        restored,
        save,
        model,
        control,
        root.path().join("workspace"),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(restarted.session(&sid).await.unwrap()).unwrap(),
        desktop
    );
    let repeated = restarted.channel(message).await.unwrap();
    assert_eq!(repeated["turn_id"], tid);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(model_calls.load(Ordering::SeqCst), 2);
    let result = restarted
        .channel(json!({"operation":"get","source_key":"synthetic-wechat-owner","turn_id":tid}))
        .await
        .unwrap();
    assert_eq!(result["status"], "succeeded");
    assert!(restarted
        .channel(json!({"operation":"get","source_key":"synthetic-foreign-owner","turn_id":tid}))
        .await
        .is_err());
    std::fs::write(
        root.path().join("state.dpapi"),
        b"synthetic concurrent legacy writer",
    )
    .unwrap();
    let blocked=restarted.channel(json!({"operation":"submit","source_key":"synthetic-wechat-owner","text":"不应执行此合成请求","request_id":"segmented-blocked-new-request"})).await.unwrap_err();
    assert!(blocked.contains("ARCHIVE_LEGACY_CONFLICT"));
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(model_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        serde_json::to_value(restarted.session(&sid).await.unwrap()).unwrap(),
        desktop
    );
}
