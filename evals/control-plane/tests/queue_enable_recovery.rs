//! Public API lifecycle with real graph and synthetic planner/controller only.
use homebase_control_tests::{
    assistant::{Control, Model, SendRequest, State},
    assistant_protocol,
    control_state::{Controller, Runner},
    test_support::{legacy_engine, Save},
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::Mutex;

#[tokio::test]
async fn unpaused_queue_is_paused_by_restart_and_enable_never_reauthorizes_it() {
    let root = tempfile::tempdir().unwrap();
    let versions = Arc::new(Mutex::new(Vec::<State>::new()));
    let saved = versions.clone();
    let save: Save = Arc::new(move |state| {
        let saved = saved.clone();
        Box::pin(async move {
            saved.lock().await.push(state);
            Ok(())
        })
    });
    let effects = Arc::new(AtomicUsize::new(0));
    let count = effects.clone();
    let runner: Runner = Arc::new(move |command, _| {
        let count = count.clone();
        Box::pin(async move {
            assert_eq!(command, "system_info");
            count.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"status":"succeeded","description":"synthetic only"}))
        })
    });
    let catalog: Value =
        serde_json::from_str(include_str!("../../../control/commands.json")).unwrap();
    let control_root = root.path().join("control");
    std::fs::create_dir(&control_root).unwrap();
    let controller = Controller::new(catalog, &control_root, runner).unwrap();
    let control: Control = Arc::new(move |request| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(request).await })
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let model_calls = calls.clone();
    let model: Model = Arc::new(move |_| {
        let model_calls = model_calls.clone();
        Box::pin(async move {
            let (name, args) = match model_calls.fetch_add(1, Ordering::SeqCst) {
                0 => (
                    "homebase_memory",
                    json!({"action":"remember","revision":0,"title":"合成确认","content":"不保存"}),
                ),
                1 => ("homebase_call", json!({"command":"system_info","args":{}})),
                2 => {
                    return assistant_protocol::parse(
                        "chat",
                        &json!({"choices":[{"message":{"content":"已查到合成结果"}}]}),
                    )
                }
                _ => panic!("Restart/enable repeated original work"),
            };
            assistant_protocol::parse(
                "chat",
                &json!({"choices":[{"message":{"tool_calls":[{
                    "id":"synthetic-call","function":{"name":name,"arguments":args.to_string()}
                }]}}]}),
            )
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = "deepagents".into();
    state.settings.route_id = "synthetic-only".into();
    let engine = legacy_engine(
        state,
        save.clone(),
        model.clone(),
        control.clone(),
        root.path().join("workspace"),
    )
    .await
    .unwrap();
    let request = |text: &str, rid: &str, sid: Option<String>| SendRequest {
        image: None,
        session_id: sid,
        text: text.into(),
        request_id: rid.into(),
        mode: Some("task".into()),
        route_id: None,
        attachments: vec![],
    };
    let first = engine
        .send(request("准备合成确认", "public-first", None))
        .await
        .unwrap();
    let pending = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let turn = engine.turn_with_progress(&first.turn_id).await.unwrap();
            if turn.state == "awaiting_confirmation" {
                break turn.pending.unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let queued = engine
        .send(request(
            "之后查询合成信息",
            "public-queued",
            Some(first.session_id.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(queued.state, "waiting_input_queue");
    assert!(
        !engine
            .sessions()
            .await
            .iter()
            .find(|s| s.id == first.session_id)
            .unwrap()
            .queue_paused
    );
    let mut settings = engine.settings().await;
    settings.enabled = false;
    engine.save_settings(settings).await.unwrap();
    // Capture the actual committed settings transaction before cancellation
    // cleanup. No manually fabricated queue or receipt enters the restart.
    let snapshot = versions
        .lock()
        .await
        .iter()
        .find(|state| {
            !state.settings.enabled
                && state.sessions.iter().any(|s| {
                    serde_json::to_value(s).unwrap()["queue_paused"] == false
                        && s.turns
                            .iter()
                            .any(|t| t.id == queued.turn_id && t.state == "waiting_input_queue")
                })
        })
        .unwrap()
        .clone();
    let restored = legacy_engine(
        snapshot,
        save,
        model,
        control,
        root.path().join("restart-workspace"),
    )
    .await
    .unwrap();
    assert!(
        restored
            .sessions()
            .await
            .iter()
            .find(|s| s.id == first.session_id)
            .unwrap()
            .queue_paused
    );
    let mut settings = restored.settings().await;
    settings.enabled = true;
    restored.save_settings(settings).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert!(
        restored
            .sessions()
            .await
            .iter()
            .find(|s| s.id == first.session_id)
            .unwrap()
            .queue_paused
    );
    assert_eq!(
        restored.turn_with_progress(&queued.turn_id).await.unwrap().state,
        "waiting_input_queue"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    restored
        .confirm(&first.turn_id, &pending.id, false)
        .await
        .unwrap();
    restored.resume_queue(&first.session_id).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let turn = restored.turn_with_progress(&queued.turn_id).await.unwrap();
            if turn.state == "completed" {
                break;
            }
            assert!(!["failed", "unknown"].contains(&turn.state.as_str()));
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let ack = restored
        .send(request(
            "之后查询合成信息",
            "public-queued",
            Some(first.session_id),
        ))
        .await
        .unwrap();
    assert_eq!(ack.turn_id, queued.turn_id);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}
