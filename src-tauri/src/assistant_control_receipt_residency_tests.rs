use super::*;

fn closed_control(session: &Session, action: &str, response: Value) -> Value {
    json!({"key":"a".repeat(64),"fingerprint":"b".repeat(64),"action":action,
        "session_id":session.id,"turn_id":session.turns[0].id,
        "confirmation":"","generation":0,"response":response})
}

fn keep_body(value: Value, session: &Session) -> bool {
    let receipt = serde_json::from_value(value).unwrap();
    channel_control::references(
        &[receipt],
        &session.id,
        &session
            .turns
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
    )
}

async fn receipt_history() -> (tempfile::TempDir, State, Arc<AtomicUsize>) {
    let (dir, full, calls) = filled().await;
    let mut state = large_history(full);
    state.settings.wechat_enabled = true;
    for (n, session) in state.sessions.iter_mut().enumerate() {
        session.source = "wechat".into();
        session.source_key = format!("receipt-owner-{n}");
        for i in 0..session.turns.len() {
            let input = SendRequest {
                image: None,
                session_id: (i > 0).then(|| session.id.clone()),
                text: session.messages[i * 2].content.clone(),
                request_id: session.turns[i].request_id.clone(),
                mode: Some("chat".into()),
                route_id: None,
                attachments: vec![],
            };
            session.turns[i].fingerprint =
                request_fingerprint(&input, &session.source_key).unwrap();
        }
    }
    (dir, state, calls)
}

#[tokio::test]
async fn many_completed_controls_keep_original_receipts_without_retaining_all_closed_bodies() {
    let (dir, state, calls) = receipt_history().await;
    assert!(capacity::encoded_bytes(&state).unwrap() > 24 * 1024 * 1024);
    let first = state.sessions[0].clone();
    let (store, state) = capacity_store(dir.path(), state).await;
    let e = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    let mut originals = vec![];
    for n in 0..96 {
        let request = json!({"operation":"submit","source_key":format!("receipt-owner-{n}"),"request_id":format!("original-stop-{n}"),"text":"停止"});
        let response = e
            .channel(request.clone())
            .await
            .unwrap_or_else(|error| panic!("closed control {n}: {error}"));
        assert_eq!(response["turn_id"], format!("capacity-turn-{n}-7"));
        originals.push((request, response));
    }
    let mut settings = e.settings().await;
    settings.max_tokens = 3511;
    e.save_settings(settings).await.unwrap();
    {
        let state = e.state.lock().await;
        assert_eq!(state.channel_controls.len(), 96);
        assert_eq!(
            state.sessions.iter().filter(|s| s.cold.is_some()).count(),
            128
        );
        assert!(capacity::encoded_bytes(&*state).unwrap() < 2 * 1024 * 1024);
    }
    let reads = store.lock().await.metrics().segment_reads;
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    for (request, response) in &originals {
        assert_eq!(e.channel(request.clone()).await.unwrap(), *response);
    }
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    e.channel(json!({"operation":"submit","source_key":"receipt-owner-0","request_id":"later-new","text":"新对话"})).await.unwrap();
    assert_eq!(
        e.channel(originals[0].0.clone()).await.unwrap(),
        originals[0].1
    );
    assert!(e.channel(json!({"operation":"submit","source_key":"receipt-owner-0","request_id":"original-stop-0","text":"确认"})).await.err().unwrap().contains("不同控制内容"));
    assert!(e.channel(json!({"operation":"submit","source_key":"foreign-source","request_id":"original-stop-0","text":"停止"})).await.is_err());
    drop(e);
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let state = load(store.clone(), snapshot).await.unwrap();
    let e = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    let reads = store.lock().await.metrics().segment_reads;
    for (request, response) in &originals {
        assert_eq!(e.channel(request.clone()).await.unwrap(), *response);
    }
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(
        serde_json::to_value(e.session(&first.id).await.unwrap().messages).unwrap(),
        serde_json::to_value(first.messages).unwrap()
    );
    assert!(e.archive_leases.protected().unwrap().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn final_response_shapes_release_only_their_original_session_and_turn() {
    let (_dir, state, _) = filled().await;
    let s = &state.sessions[0];
    let tid = &s.turns[0].id;
    for action in ["停止", "停止助理", "停止当前任务", "/stop"] {
        assert!(!keep_body(
            closed_control(
                s,
                action,
                json!({"action":"message","text":"原响应","turn_id":tid})
            ),
            s
        ));
    }
    for action in ["确认", "拒绝", "继续"] {
        let mut value = closed_control(
            s,
            action,
            json!({"session_id":s.id,"turn_id":tid,"state":"running"}),
        );
        if action != "继续" {
            value["confirmation"] = json!(id());
        }
        assert!(!keep_body(value, s));
    }
    for action in ["新对话", "开启新对话", "/new", "继续排队"] {
        let mut value = closed_control(s, action, json!({"action":"message","text":"原响应"}));
        value["turn_id"] = json!("");
        if action != "继续排队" {
            value["response"]["session_id"] = json!(s.id);
        }
        assert!(!keep_body(value, s));
    }
    for action in ["进度", "结果"] {
        let response = json!({"action":"view","turn_id":tid,"files":action=="结果","view":{"status":"succeeded","result":{"status":"succeeded","turn_id":tid},"error":null}});
        assert!(!keep_body(closed_control(s, action, response), s));
    }
}

#[tokio::test]
async fn missing_ambiguous_or_mismatched_responses_keep_original_body_protection() {
    let (_dir, state, _) = filled().await;
    let s = &state.sessions[0];
    let tid = &s.turns[0].id;
    let good = closed_control(
        s,
        "停止",
        json!({"action":"message","text":"原响应","turn_id":tid}),
    );
    for (field, value) in [
        ("response", Value::Null),
        ("response", json!({})),
        (
            "response",
            json!({"action":"message","text":"原响应","turn_id":"foreign"}),
        ),
        (
            "response",
            json!({"action":"message","text":"","turn_id":tid}),
        ),
        ("action", json!("unknown")),
        ("session_id", json!("foreign-session")),
        ("key", json!("invalid")),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        assert!(keep_body(bad, s), "{field}");
    }
    let mut bad = good.clone();
    bad["turn_id"] = json!("foreign-turn");
    assert!(keep_body(bad, s));
    let mut bad = good.clone();
    bad["response"]["error"] = json!("synthetic uncertainty");
    assert!(keep_body(bad, s));
    let mut bad = good.clone();
    bad["fingerprint"] = json!("invalid");
    assert!(keep_body(bad, s));
    let view = closed_control(
        s,
        "结果",
        json!({"action":"view","turn_id":tid,"files":true,"view":{"status":"succeeded","result":{"status":"succeeded","turn_id":tid},"error":null}}),
    );
    for (field, value) in [
        ("files", json!(false)),
        ("turn_id", json!("foreign")),
        (
            "view",
            json!({"status":"unknown","result":{"status":"succeeded","turn_id":tid},"error":null}),
        ),
    ] {
        let mut bad = view.clone();
        bad["response"][field] = value;
        assert!(keep_body(bad, s));
    }
    let ack = closed_control(
        s,
        "继续",
        json!({"session_id":s.id,"turn_id":tid,"state":"unrecognized"}),
    );
    assert!(keep_body(ack, s));
    assert!(keep_body(
        closed_control(
            s,
            "继续",
            json!({"session_id":s.id,"turn_id":tid,"state":"succeeded"})
        ),
        s
    ));
    let unrelated: channel_control::ControlReceipt = serde_json::from_value(good).unwrap();
    assert!(!channel_control::references(
        &[unrelated],
        "other-session",
        &["other-turn"]
    ));
}

#[tokio::test]
async fn explicit_null_in_memory_response_remains_protected() {
    let (_dir, state, _) = filled().await;
    let s = &state.sessions[0];
    let mut receipt: channel_control::ControlReceipt = serde_json::from_value(closed_control(
        s,
        "停止",
        json!({"action":"message","text":"原响应","turn_id":s.turns[0].id}),
    ))
    .unwrap();
    receipt.response = Some(Value::Null);
    assert!(channel_control::references(
        &[receipt],
        &s.id,
        &[&s.turns[0].id]
    ));
}

#[tokio::test]
async fn failed_final_response_save_keeps_original_body_until_replayed_and_persisted() {
    let (dir, mut state, _) = filled().await;
    state.settings.enabled = false;
    state.settings.wechat_enabled = true;
    for (n, s) in state.sessions.iter_mut().enumerate() {
        s.source = "wechat".into();
        s.source_key = format!("small-receipt-{n}");
    }
    let original = state.sessions[0].clone();
    let (store, state) = capacity_store(dir.path(), state).await;
    let fail = Arc::new(AtomicUsize::new(1));
    let save: Commit = Arc::new({
        let store = store.clone();
        move |state, protected| {
            let store = store.clone();
            let fail = fail.clone();
            Box::pin(async move {
                if state.channel_controls.iter().any(|r| r.response.is_some())
                    && fail.swap(0, Ordering::SeqCst) == 1
                {
                    return Err("synthetic-final-response-save-lost".into());
                }
                commit(store, state, protected).await
            })
        }
    });
    let workspace = dir.path().join("workspace");
    let e = Engine::new_with_commit(
        state,
        save,
        Arc::new(|_| Box::pin(async { panic!("no model") })),
        Arc::new(|_| Box::pin(async { panic!("no business") })),
        workspace.clone(),
        Arc::new(crate::assistant_documents::DocumentStore::new(
            workspace.join(".documents"),
        )),
    )
    .await
    .unwrap();
    let request = json!({"operation":"submit","source_key":"small-receipt-0","request_id":"lost-final","text":"停止"});
    assert_eq!(
        e.channel(request.clone()).await.err().unwrap(),
        "synthetic-final-response-save-lost"
    );
    assert!(e.state.lock().await.channel_controls[0].response.is_none());
    let mut settings = e.settings().await;
    settings.max_tokens = 3512;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_none());
    e.channel(json!({"operation":"submit","source_key":"small-receipt-0","request_id":"new-after-lost","text":"新对话"})).await.unwrap();
    drop(e);
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let state = load(store.clone(), snapshot).await.unwrap();
    let e = capacity_engine(store.clone(), state, workspace).await;
    let response = e.channel(request.clone()).await.unwrap();
    assert_eq!(response["turn_id"], original.turns[0].id);
    let mut settings = e.settings().await;
    settings.max_tokens = 3513;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    let reads = store.lock().await.metrics().segment_reads;
    assert_eq!(e.channel(request).await.unwrap(), response);
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(
        e.turn(&original.turns[0].id).await.unwrap().state,
        "completed"
    );
    assert!(e.archive_leases.protected().unwrap().is_empty());
}

#[tokio::test]
async fn final_ack_never_overrides_active_uncertain_queue_and_reference_guards() {
    let (_dir, state, _) = filled().await;
    for status in [
        "running",
        "awaiting_confirmation",
        "interrupted_unknown",
        "unknown",
        "failed",
        "waiting_input_queue",
    ] {
        let mut changed = state.clone();
        let s = &mut changed.sessions[0];
        s.turns[0].state = status.into();
        let receipt = closed_control(
            s,
            "停止",
            json!({"action":"message","text":"原响应","turn_id":s.turns[0].id}),
        );
        changed
            .channel_controls
            .push(serde_json::from_value(receipt).unwrap());
        assert!(summary(&changed, &changed.sessions[0]).is_none());
    }
    let mut changed = state.clone();
    changed.sessions[0].queue_paused = true;
    let s = &changed.sessions[0];
    let receipt = closed_control(
        s,
        "停止",
        json!({"action":"message","text":"原响应","turn_id":s.turns[0].id}),
    );
    changed
        .channel_controls
        .push(serde_json::from_value(receipt).unwrap());
    assert!(summary(&changed, &changed.sessions[0]).is_none());
    let mut changed = state;
    let s = &changed.sessions[0];
    let receipt = closed_control(
        s,
        "停止",
        json!({"action":"message","text":"原响应","turn_id":s.turns[0].id}),
    );
    changed
        .channel_controls
        .push(serde_json::from_value(receipt).unwrap());
    changed.sessions[1].turns[0].parent_turn_id = Some(changed.sessions[0].turns[0].id.clone());
    assert!(summary(&changed, &changed.sessions[0]).is_none());
}

#[tokio::test]
#[ignore = "explicit synthetic closed-receipt profile in fresh F lab only"]
async fn seed_native_closed_receipt_profile() {
    let home =
        PathBuf::from(std::env::var_os("HOMEBASE_RECEIPT_SEED_HOME").expect("new synthetic home"));
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("receipt-native-"));
    let (_dir, mut original, calls) = filled().await;
    original.settings.enabled = false;
    let mut state = large_history(original.clone());
    for (n, s) in state.sessions.iter_mut().enumerate() {
        s.source = "wechat".into();
        s.source_key = format!("native-receipt-{n}");
        for i in 0..s.turns.len() {
            let request = SendRequest {
                image: None,
                session_id: (i > 0).then(|| s.id.clone()),
                text: s.messages[i * 2].content.clone(),
                request_id: s.turns[i].request_id.clone(),
                mode: Some("chat".into()),
                route_id: None,
                attachments: vec![],
            };
            s.turns[i].fingerprint = request_fingerprint(&request, &s.source_key).unwrap();
        }
    }
    let mut critical = original.sessions[0].clone();
    critical.id = "capacity-critical-session".into();
    critical.turns[0].id = "capacity-critical-turn".into();
    critical.turns[0].request_id = "capacity-critical-request".into();
    critical.turns[0].state = "interrupted_unknown".into();
    critical.turns[0].runtime_uncertain_model = true;
    critical.turns[0].interaction = "task".into();
    for message in critical.messages.iter_mut() {
        message.turn_id = Some("capacity-critical-turn".into());
    }
    state.sessions.push(critical);
    let samples = vec![
        json!({"id":state.sessions[0].id,"messages":state.sessions[0].messages,"turns":state.sessions[0].turns.iter().map(|t|json!({"id":t.id,"state":t.state})).collect::<Vec<_>>()}),
    ];
    let root = home.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let legacy =
        crate::wechat_crypto::crypt(&serde_json::to_vec(&original).unwrap(), true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &legacy)
        .await
        .unwrap();
    let (store, state) = capacity_store(&root, state).await;
    let e = capacity_engine(store.clone(), state, root.join("workspace")).await;
    let request = json!({"operation":"submit","source_key":"native-receipt-0","request_id":"native-closed-stop","text":"停止"});
    let response = e.channel(request.clone()).await.unwrap();
    let mut settings = e.settings().await;
    settings.max_tokens = 3514;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(e.channel(request.clone()).await.unwrap(), response);
    let expected = json!({"synthetic_only":true,"real_model_calls":0,"samples":samples,"critical_turn":"capacity-critical-turn","critical_state":"interrupted_unknown","control_request":request,"control_response":response,"closed_control_cold":true,"synthetic_model_calls_for_seed":calls.load(Ordering::SeqCst)});
    crate::control_protocol::publish_private(
        &home.join("route-receipt-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}
