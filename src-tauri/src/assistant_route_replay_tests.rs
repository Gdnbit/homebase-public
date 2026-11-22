use super::*;

#[tokio::test]
async fn removed_route_receipt_recovers_cold_completed_and_hot_unknown_without_validation() {
    let (dir, mut state, calls) = filled().await;
    let mut inputs = Vec::new();
    for i in 0..2 {
        let mut input = request(None, "仅合成的闲聊", &format!("cold-original-{i}"));
        input.route_id = Some("openrouter::homebase-fixture/retired:free".into());
        state.sessions[i].turns[0].fingerprint = request_fingerprint(&input, "desktop").unwrap();
        if i == 1 {
            state.sessions[i].turns[0].state = "interrupted_unknown".into();
            state.sessions[i].turns[0].runtime_uncertain_model = true;
        }
        inputs.push(input);
    }
    let turns: Vec<_> = state
        .sessions
        .iter()
        .map(|s| s.turns[0].id.clone())
        .collect();
    let store = stored(dir.path(), state).await;
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    for (i, input) in inputs.into_iter().enumerate() {
        let view = serde_json::to_value(e.receipt_lookup(&input).await.unwrap()).unwrap();
        assert_eq!(view["lookup_state"], "found");
        assert_eq!(view["receipt"]["turn_id"], turns[i]);
        assert_eq!(
            view["receipt"]["state"],
            if i == 0 {
                "completed"
            } else {
                "interrupted_unknown"
            }
        );
        let ack = e
            .send_checked(input, |_| {
                panic!("old receipt must not validate or select a replacement route")
            })
            .await
            .unwrap();
        assert_eq!(ack.turn_id, turns[i]);
    }
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn receipt_absence_is_explicit_and_new_submission_still_validates_route() {
    let (dir, state, calls) = filled().await;
    let store = stored(dir.path(), state).await;
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    let input = request(None, "新合成请求", "not-accepted");
    let view = serde_json::to_value(e.receipt_lookup(&input).await.unwrap()).unwrap();
    assert_eq!(view["lookup_state"], "not_found");
    assert!(view["receipt"].is_null());
    assert!(view["message"].as_str().unwrap().contains("不证明从未执行"));
    assert_eq!(
        e.send_checked(input, |_| Err("synthetic route absent".into()))
            .await
            .err()
            .unwrap(),
        "synthetic route absent"
    );
    assert_eq!(e.sessions().await.len(), 3);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn receipt_fingerprint_conflicts_never_fall_through_to_submission() {
    let (dir, state, calls) = filled().await;
    let store = stored(dir.path(), state).await;
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    let original = request(None, "仅合成的闲聊", "cold-original-0");
    let mut changed = Vec::new();
    let mut input = original.clone();
    input.attachments.push(TextAttachment {
        name: "fixture.txt".into(),
        content: "合成附件".into(),
        document: None,
    });
    changed.push(input);
    let mut input = original.clone();
    input.text.push('变');
    changed.push(input);
    let mut input = original.clone();
    input.mode = Some("task".into());
    changed.push(input);
    let mut input = original.clone();
    input.route_id = Some("other:free".into());
    changed.push(input);
    let mut input = original.clone();
    input.session_id = Some("foreign".into());
    changed.push(input);
    for input in changed {
        assert!(e.receipt_lookup(&input).await.is_err());
        assert!(e
            .send_checked(input, |_| panic!("conflict is not a new submission"))
            .await
            .is_err());
    }
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn damaged_original_receipt_is_error_not_absence_and_never_reexecutes() {
    let (dir, state, calls) = filled().await;
    let store = stored(dir.path(), state).await;
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    for file in std::fs::read_dir(dir.path().join("history-v1/segments")).unwrap() {
        std::fs::write(file.unwrap().path(), b"synthetic receipt corruption").unwrap();
    }
    let input = request(None, "仅合成的闲聊", "cold-original-0");
    assert!(e
        .receipt_lookup(&input)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_CHANGED"));
    assert!(e
        .send_checked(input, |_| panic!(
            "unreadable original must not submit again"
        ))
        .await
        .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
}

#[tokio::test]
#[ignore = "explicit removed-route synthetic fixture in a new F home only"]
async fn seed_isolated_removed_route_receipts() {
    let home = PathBuf::from(
        std::env::var_os("HOMEBASE_ROUTE_REPLAY_SEED_HOME").expect("new synthetic home"),
    );
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    let (_dir, mut state, calls) = filled().await;
    state.settings.enabled = false;
    let route = "openrouter::homebase-fixture/retired:free";
    let mut requests = Vec::new();
    for i in 0..2 {
        let mut input = request(None, "仅合成的闲聊", &format!("cold-original-{i}"));
        input.route_id = Some(route.into());
        state.sessions[i].turns[0].fingerprint = request_fingerprint(&input, "desktop").unwrap();
        state.sessions[i].turns[0].settings.route_id = route.into();
        if i == 1 {
            state.sessions[i].turns[0].state = "interrupted_unknown".into();
            state.sessions[i].turns[0].runtime_uncertain_model = true;
        }
        requests.push(input);
    }
    let expected = json!({"synthetic_only":true,"real_model_calls":0,"synthetic_model_calls_for_seed":calls.load(Ordering::SeqCst),
        "completed_request":requests[0],"unknown_request":requests[1],
        "completed_turn":state.sessions[0].turns[0].id,"completed_session":state.sessions[0].id,
        "unknown_turn":state.sessions[1].turns[0].id,"unknown_session":state.sessions[1].id,
        "sessions":state.sessions.iter().map(|s|s.id.clone()).collect::<Vec<_>>()});
    let root = home.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let legacy = crate::wechat_crypto::crypt(&serde_json::to_vec(&state).unwrap(), true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &legacy)
        .await
        .unwrap();
    let mut writer = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
    writer.enable_capacity_segments();
    writer.load_index().unwrap();
    enable_capacity(&mut state);
    let state = commit(Arc::new(Mutex::new(writer)), state, Vec::new())
        .await
        .unwrap();
    assert!(state.sessions[0].cold.is_some());
    assert!(state.sessions[1].cold.is_none());
    crate::control_protocol::publish_private(
        &home.join("route-receipt-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
