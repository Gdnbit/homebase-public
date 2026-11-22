//! Upgrade fixtures retain a saved old turn; they never use a provider or key.
use super::*;
use workflow_tests::{task_wait, tool_reply};

#[tokio::test]
async fn simulated_wechat_modification_wakes_restored_old_task_and_queries_original_result() {
    let (base, _, effects, _, _root) = fixture(false, vec![]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let model: Model = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            Ok(match count.fetch_add(1,Ordering::SeqCst) {
            0=>tool_reply(0,"homebase_memory",json!({"action":"remember","revision":0,"title":"微信合成旧方案","content":"红色"})),
            1=>{assert!(r.definitions.unwrap().iter().any(|d|d["function"]["name"]=="homebase_steer_message"));tool_reply(1,"homebase_steer_message",json!({}))},
            2=>{assert!(serde_json::to_string(&r.messages).unwrap().contains("蓝色圆点"));tool_reply(2,"homebase_files",json!({"action":"read","path":"wechat-fixture.txt"}))},
            3=>wire::parse("chat",&json!({"choices":[{"message":{"content":"已经按蓝色圆点要求核对了测试文件，没有保存旧偏好。"}}]})).unwrap(),
            _=>panic!("simulated WeChat caused duplicate model execution"),
        })
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let original = Engine::new(
        state,
        base.save.clone(),
        model.clone(),
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let message = json!({"operation":"submit","source_key":"compat-wechat-owner","request_id":"wechat-original","text":"保存这条测试偏好"});
    let first = original.channel(message).await.unwrap();
    let tid = first["turn_id"].as_str().unwrap().to_string();
    task_wait(&original, &tid, "awaiting_confirmation").await;
    let mut checkpoint = original.state.lock().await.clone();
    let (s, t) = locate(&checkpoint, &tid).unwrap();
    checkpoint.sessions[s].turns[t].runtime_version = "homebase-da-20261113.6".into();
    original.cancel(&tid).await.unwrap();
    for _ in 0..100 {
        if !original.running.lock().await.contains_key(&tid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!original.running.lock().await.contains_key(&tid));
    let restored = Engine::new(
        checkpoint,
        base.save.clone(),
        model,
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let change = json!({"operation":"submit","source_key":"compat-wechat-owner","request_id":"wechat-modification","text":"这次改成蓝色圆点，只核对测试文件，不保存偏好"});
    let routed = restored.channel(change.clone()).await.unwrap();
    let chat = routed["turn_id"].as_str().unwrap();
    assert_ne!(chat, tid);
    let discussion = task_wait(&restored, chat, "completed").await;
    assert_eq!(discussion.steered_turn_id.as_deref(), Some(tid.as_str()));
    assert!(discussion.text.starts_with("好，"));
    assert!(!discussion.text.contains("系统"));
    let done = task_wait(&restored, &tid, "completed").await;
    assert_eq!(done.requirements_revision, 1);
    assert!(done.text.contains("蓝色圆点"));
    let result = restored
        .channel(json!({"operation":"get","source_key":"compat-wechat-owner","turn_id":tid}))
        .await
        .unwrap();
    assert_eq!(result["result"]["turn_id"], tid);
    assert_eq!(result["result"]["status"], "succeeded");
    assert!(result["result"]["text"]
        .as_str()
        .unwrap()
        .contains("没有保存旧偏好"));
    assert!(restored
        .channel(json!({"operation":"get","source_key":"other-wechat-owner","turn_id":tid}))
        .await
        .is_err());
    assert_eq!(restored.channel(change).await.unwrap()["turn_id"], chat);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert!(restored.personal().await["memories"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn restored_confirmation_steering_wakes_the_real_original_graph_without_replaying_old_write()
{
    let (base, _, effects, _, _root) = fixture(false, vec![]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let model: Model = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            Ok(match count.fetch_add(1, Ordering::SeqCst) {
                0 => tool_reply(
                    0,
                    "homebase_memory",
                    json!({"action":"remember","revision":0,"title":"合成旧方案","content":"红点"}),
                ),
                1 => {
                    assert!(serde_json::to_string(&r.messages)
                        .unwrap()
                        .contains("蓝色圆点"));
                    tool_reply(
                        1,
                        "homebase_files",
                        json!({"action":"read","path":"synthetic.txt"}),
                    )
                }
                2 => reply(vec![]),
                _ => panic!("original graph replayed model calls"),
            })
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let original = Engine::new(
        state,
        base.save.clone(),
        model.clone(),
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let ack = original
        .send(request("保存旧测试偏好", "compat-real-graph"))
        .await
        .unwrap();
    task_wait(&original, &ack.turn_id, "awaiting_confirmation").await;
    let mut checkpoint = original.state.lock().await.clone();
    let (s, t) = locate(&checkpoint, &ack.turn_id).unwrap();
    checkpoint.sessions[s].turns[t].runtime_version = "homebase-da-20261113.6".into();
    let operations = checkpoint.sessions[s].turns[t].runtime_operations.clone();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0]["kind"], "pi_turn");
    assert_eq!(operations[0]["complete"], false);
    original.cancel(&ack.turn_id).await.unwrap();
    for _ in 0..100 {
        if !original.running.lock().await.contains_key(&ack.turn_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!original.running.lock().await.contains_key(&ack.turn_id));
    let restored = Engine::new(
        checkpoint,
        base.save.clone(),
        model,
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(!restored.running.lock().await.contains_key(&ack.turn_id));
    let input = SteerInput {
        turn_id: ack.turn_id.clone(),
        revision: 0,
        request_id: "compat-real-change".into(),
        text: "改为蓝色圆点，只读测试文件，不保存旧偏好".into(),
    };
    assert_eq!(
        restored.steer(input.clone()).await.unwrap()["accepted"],
        true
    );
    let finished = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let view = restored.turn(&ack.turn_id).await.unwrap();
            if terminal(&view.state) {
                return view;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("accepted steering left the restored original graph without a worker");
    assert_eq!(finished.state, "completed", "{}", finished.text);
    assert_eq!(finished.id, ack.turn_id);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert!(restored.personal().await["memories"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(restored.steer(input).await.unwrap()["duplicate"], true);
    let state = restored.state.lock().await;
    let (s, t) = locate(&state, &ack.turn_id).unwrap();
    assert_eq!(
        state.sessions[s].turns[t].runtime_version,
        "homebase-da-20261113.6"
    );
    let saved = &state.sessions[s].turns[t].runtime_operations;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0]["kind"], "pi_turn");
    assert_eq!(saved[0]["complete"], true);
    assert!(saved[0]["messages"].as_array().unwrap().starts_with(operations[0]["messages"].as_array().unwrap()));
}

#[tokio::test]
async fn old_runtime_steering_preserves_saved_identity_checkpoint_and_idempotence() {
    for (version, supported) in [
        ("homebase-da-20261113.6", true),
        (RUNTIME_VERSION, true),
        ("homebase-da-20261113.5", false),
    ] {
        let (base, models, effects, _, _root) = fixture(false, vec![]).await;
        let ack = base
            .send(request("合成旧任务", "compat-old-main"))
            .await
            .unwrap();
        base.change_turn(&ack.turn_id, |s, t| {
            let turn = &mut s.turns[t];
            turn.runtime_version = version.into();
            turn.state = "awaiting_confirmation".into();
            turn.settings.runtime = "pi".into();
            turn.runtime_operations = vec![json!({"fixture":"saved-checkpoint-sentinel"})].into();
            Ok(())
        })
        .await
        .unwrap();
        let before = base.state.lock().await.clone();
        let restored = Engine::new(
            before.clone(),
            base.save.clone(),
            base.model.clone(),
            base.control.clone(),
            base.workspace.clone(),
        )
        .await
        .unwrap();
        let input = SteerInput {
            turn_id: ack.turn_id.clone(),
            revision: 0,
            request_id: "compat-change-original".into(),
            text: "这次改为蓝色圆点，不改长期偏好".into(),
        };
        let result = restored.steer(input.clone()).await;
        assert_eq!(
            result.is_ok(),
            supported,
            "old runtime feature lost: {version}"
        );
        let current = restored.state.lock().await.clone();
        let (s, t) = locate(&current, &ack.turn_id).unwrap();
        let (bs, bt) = locate(&before, &ack.turn_id).unwrap();
        assert_eq!(current.sessions[s].turns[t].runtime_version, version);
        assert_eq!(
            current.sessions[s].turns[t].fingerprint,
            before.sessions[bs].turns[bt].fingerprint
        );
        assert!(current.sessions[s].turns[t].runtime_operations.iter()
            .eq(before.sessions[bs].turns[bt].runtime_operations.iter()));
        if supported {
            assert_eq!(current.sessions[s].turns[t].requirements_revision, 1);
            assert_eq!(restored.steer(input).await.unwrap()["duplicate"], true);
            assert_eq!(restored.state.lock().await.revision, current.revision);
        } else {
            assert_eq!(
                serde_json::to_value(current).unwrap(),
                serde_json::to_value(before).unwrap()
            );
        }
        assert_eq!(models.load(Ordering::SeqCst), 0);
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn old_dialogue_exposes_only_its_original_routing_features_and_applies_to_original_task() {
    for (version, supported) in [
        ("homebase-da-20261113.6", true),
        (RUNTIME_VERSION, true),
        ("homebase-da-20261113.5", false),
    ] {
        let (base, _, effects, _, _root) = fixture(false, vec![]).await;
        let main = base
            .send(request("处理旧任务", "compat-dialogue-main"))
            .await
            .unwrap();
        let mut input = request("改成蓝色圆点", "compat-dialogue-change");
        input.session_id = Some(main.session_id.clone());
        input.mode = Some("chat".into());
        let chat = base.send(input).await.unwrap();
        let mut state = base.state.lock().await.clone();
        state.settings.enabled = true;
        let (s, t) = locate(&state, &main.turn_id).unwrap();
        let parent = &mut state.sessions[s].turns[t];
        parent.runtime_version = version.into();
        parent.settings.runtime = "pi".into();
        parent.state = "awaiting_confirmation".into();
        let (s, t) = locate(&state, &chat.turn_id).unwrap();
        let discussion = &mut state.sessions[s].turns[t];
        discussion.runtime_version = version.into();
        discussion.settings.enabled = true;
        discussion.state = "awaiting_confirmation".into();
        discussion.route_work = true;
        discussion.parent_turn_id = Some(main.turn_id.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let model: Model = Arc::new(move |r| {
            let count = count.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                let tools = r.definitions.unwrap();
                assert_eq!(
                    tools
                        .iter()
                        .any(|d| d["function"]["name"] == "homebase_steer_message"),
                    supported,
                    "dialogue lost old steering schema: {version}"
                );
                Ok(reply(vec![("homebase_steer_message", json!({}))]))
            })
        });
        let restored = Engine::new(
            state,
            base.save.clone(),
            model,
            base.control.clone(),
            base.workspace.clone(),
        )
        .await
        .unwrap();
        // This fixture exercises a live foreground discussion; the real
        // restored-worker path is covered above with an actual graph process.
        let (cancel, _) = watch::channel(false);
        restored.running.lock().await.insert(
            main.turn_id.clone(),
            Running {
                cancel,
                approval: Arc::new(Notify::new()),
            },
        );
        restored.run_dialogue(&chat.turn_id).await.unwrap();
        let parent = restored.turn(&main.turn_id).await.unwrap();
        let answer = restored.turn(&chat.turn_id).await.unwrap();
        assert_eq!(parent.requirements_revision, if supported { 1 } else { 0 });
        assert_eq!(answer.state, "completed");
        if supported {
            assert_eq!(
                answer.steered_turn_id.as_deref(),
                Some(main.turn_id.as_str())
            );
            assert!(answer.text.contains("已记到"));
        } else {
            assert!(answer.steered_turn_id.is_none());
            assert!(answer.text.contains("没有执行"));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        restored.running.lock().await.remove(&main.turn_id);
    }
}
