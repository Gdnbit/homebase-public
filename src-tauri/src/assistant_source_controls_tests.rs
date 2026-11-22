use super::*;

async fn source_fixture() -> (tempfile::TempDir, Arc<Mutex<Store>>, Arc<Engine>) {
    let (dir, full, _) = filled().await;
    let mut state = large_history(full);
    state.settings.enabled = false;
    state.settings.wechat_enabled = true;
    for session in &mut state.sessions {
        session.source = "wechat".into();
        session.source_key = "large-source".into();
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
            session.turns[i].fingerprint = request_fingerprint(&input, "large-source").unwrap();
        }
    }
    let (store, state) = capacity_store(dir.path(), state).await;
    let engine = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    (dir, store, engine)
}

#[tokio::test]
async fn new_conversation_does_not_restore_large_source_and_replay_keeps_original_identity() {
    let (_dir, store, e) = source_fixture().await;
    let request = json!({"operation":"submit","source_key":"large-source","request_id":"bounded-new","text":"新对话"});
    let first = e.channel(request.clone()).await.unwrap();
    assert!(!first["session_id"].as_str().unwrap().is_empty());
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        128
    );
    let other = e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"another-new","text":"新对话"})).await.unwrap();
    assert_ne!(first["session_id"], other["session_id"]);
    assert_eq!(e.channel(request).await.unwrap(), first);
    assert_eq!(e.sessions().await.len(), 130);
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
}

#[tokio::test]
async fn bare_status_reads_bounded_turn_candidates_without_adopting_all_source_bodies() {
    let (dir, store, e) = source_fixture().await;
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let response = e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"bounded-progress","text":"进度"})).await.unwrap();
    assert_eq!(response["action"], "view");
    assert_eq!(response["turn_id"], "capacity-turn-127-7");
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    let reads = store.lock().await.metrics().segment_reads;
    assert!(
        reads <= 132,
        "selection must not read every message/context: {reads}"
    );
    let repeated = e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"bounded-result","text":"结果"})).await.unwrap();
    assert_eq!(repeated["turn_id"], response["turn_id"]);
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        128
    );
}

#[tokio::test]
async fn source_target_matches_existing_hot_policy_for_order_ties_empty_fences_and_ambiguity() {
    let (dir, full, _) = filled().await;
    let e = Engine::new(
        State::default(),
        Arc::new(|_| Box::pin(async { Ok(()) })),
        Arc::new(|_| Box::pin(async { panic!("selection must not call a model") })),
        Arc::new(|_| Box::pin(async { panic!("selection must not execute") })),
        dir.path().join("policy"),
    )
    .await
    .unwrap();
    for variant in 0..96 {
        let mut state = State::default();
        state.settings.enabled = false;
        for index in 0..6 {
            let mut session = full.sessions[0].clone();
            session.id = format!("policy-{index}");
            session.source_key = if index % 3 == 0 { "foreign" } else { "owner" }.into();
            session.updated_at = format!("2026-11-18T00:00:0{}Z", (index + variant) % 4);
            for (turn_index, turn) in session.turns.iter_mut().enumerate() {
                turn.id = format!("policy-turn-{index}-{turn_index}");
                turn.created_at = format!("2026-11-18T00:00:0{}Z", (index + variant) % 3);
                turn.interaction = if (variant >> index) % 2 == 0 {
                    "chat"
                } else {
                    "task"
                }
                .into();
                turn.state = ["completed", "running", "waiting_input_queue", "failed"]
                    [(variant + index) % 4]
                    .into();
            }
            if variant % 5 == 0 && index == 5 {
                session.turns = Records::default();
            }
            state.sessions.push(session);
        }
        let expected = Engine::channel_target(&state, "owner")
            .map(|value| value.map(|(s, t)| (s.id.clone(), t.id.clone())));
        *e.state.lock().await = state;
        let actual = e
            .source_target("owner")
            .await
            .map(|value| value.map(|choice| (choice.session_id, choice.turn_id)));
        assert_eq!(actual, expected, "policy variant {variant}");
    }
}

async fn small_source() -> (tempfile::TempDir, Arc<Mutex<Store>>, Arc<Engine>, String) {
    let (dir, mut state, _) = filled().await;
    state.settings.enabled = false;
    state.settings.wechat_enabled = true;
    for session in &mut state.sessions {
        session.source = "wechat".into();
        session.source_key = "small-source".into();
    }
    state.sessions[0].turns[0].interaction = "task".into();
    state.sessions[0].turns[0].created_at = "2026-11-18T00:00:00Z".into();
    state.sessions[1].turns[0].created_at = "2026-11-18T01:00:00Z".into();
    state.sessions[2].turns[0].created_at = "2026-11-18T02:00:00Z".into();
    let task = state.sessions[0].turns[0].id.clone();
    let (store, state) = capacity_store(dir.path(), state).await;
    let e = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    (dir, store, e, task)
}

#[tokio::test]
async fn older_cold_task_beats_newer_chat_and_stop_replay_keeps_original_after_new_chat_and_restart(
) {
    let (dir, store, e, task) = small_source().await;
    let progress=e.channel(json!({"operation":"submit","source_key":"small-source","request_id":"small-progress","text":"进度"})).await.unwrap();
    assert_eq!(progress["turn_id"], task);
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        3
    );
    let stop = json!({"operation":"submit","source_key":"small-source","request_id":"small-stop","text":"停止"});
    let first = e.channel(stop.clone()).await.unwrap();
    assert_eq!(first["turn_id"], task);
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        2
    );
    e.channel(json!({"operation":"submit","source_key":"small-source","request_id":"small-new","text":"新对话"})).await.unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    assert_eq!(e.channel(stop.clone()).await.unwrap(), first);
    drop(e);
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let state = load(store.clone(), snapshot).await.unwrap();
    let e = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    let reads = store.lock().await.metrics().segment_reads;
    assert_eq!(e.channel(stop).await.unwrap(), first);
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    assert!(e.archive_leases.protected().unwrap().is_empty());
}

#[tokio::test]
async fn fresh_empty_fence_needs_no_old_body_and_unique_hot_task_still_has_priority() {
    let (_dir, store, e) = source_fixture().await;
    e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"fence-new","text":"新对话"})).await.unwrap();
    let response=e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"fence-progress","text":"进度"})).await.unwrap();
    assert_eq!(response["action"], "message");
    assert!(response["text"]
        .as_str()
        .unwrap()
        .contains("当前新对话还没有任务"));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    let mut active = e.read_archive_session("capacity-session-0").await.unwrap();
    active.turns[0].interaction = "task".into();
    active.turns[0].state = "running".into();
    e.state.lock().await.sessions[0] = active;
    let reads = store.lock().await.metrics().segment_reads;
    let response=e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"hot-progress","text":"进度"})).await.unwrap();
    assert_eq!(response["turn_id"], "capacity-turn-0-0");
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    let mut second = e.read_archive_session("capacity-session-1").await.unwrap();
    second.turns[0].interaction = "task".into();
    second.turns[0].state = "running".into();
    e.state.lock().await.sessions[1] = second;
    let reads = store.lock().await.metrics().segment_reads;
    assert!(e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"ambiguous-stop","text":"停止"})).await.err().unwrap().contains("有几件事"));
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(e.turn("capacity-turn-0-0").await.unwrap().state, "running");
}

#[tokio::test]
async fn corrupt_candidate_is_explicit_error_and_never_an_empty_source_or_writable_restore() {
    let (dir, _store, e, _) = small_source().await;
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    for entry in std::fs::read_dir(dir.path().join("history-v1/segments")).unwrap() {
        std::fs::write(entry.unwrap().path(), b"synthetic damaged source").unwrap();
    }
    let error=e.channel(json!({"operation":"submit","source_key":"small-source","request_id":"damaged-progress","text":"进度"})).await.err().unwrap();
    assert!(error.starts_with("ARCHIVE_CHANGED:"));
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
}

#[tokio::test]
async fn candidate_cache_survives_unrelated_save_but_still_checks_external_legacy_anchor() {
    let (dir, store, e, task) = small_source().await;
    assert_eq!(
        e.source_target("small-source")
            .await
            .unwrap()
            .unwrap()
            .turn_id,
        task
    );
    let reads = store.lock().await.metrics().segment_reads;
    let mut settings = e.settings().await;
    settings.max_tokens = 3223;
    e.save_settings(settings).await.unwrap();
    assert_eq!(
        e.source_target("small-source")
            .await
            .unwrap()
            .unwrap()
            .turn_id,
        task
    );
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    std::fs::write(
        dir.path().join("state.dpapi"),
        b"synthetic external legacy change",
    )
    .unwrap();
    assert!(e
        .source_target("small-source")
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_LEGACY_CONFLICT:"));
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
}

#[tokio::test]
async fn rejecting_one_current_confirmation_never_loads_large_closed_history_or_approves_foreign_work(
) {
    let (_dir, store, e) = source_fixture().await;
    let (_tmp, full, _) = filled().await;
    let mut pending = full.sessions[0].clone();
    pending.source = "wechat".into();
    pending.source_key = "large-source".into();
    pending.id = uuid::Uuid::new_v4().to_string();
    let tid = pending.turns[0].id.clone();
    pending.turns[0].state = "awaiting_confirmation".into();
    pending.turns[0].interaction = "task".into();
    pending.turns[0].history_committed = false;
    pending.turns[0].finished_at = None;
    pending.turns[0].pending = Some(Pending {
        id: uuid::Uuid::new_v4().to_string(),
        tool_id: "fixture".into(),
        description: "homebase_memory save".into(),
        arguments: json!({"title":"must not be saved"}),
        approved: false,
    });
    let mut foreign = pending.clone();
    foreign.id = uuid::Uuid::new_v4().to_string();
    foreign.source_key = "foreign-source".into();
    foreign.turns[0].id = uuid::Uuid::new_v4().to_string();
    let foreign_id = foreign.turns[0].id.clone();
    e.mutate(Change::default(), |state| {
        state.sessions.push(pending);
        state.sessions.push(foreign);
        Ok(())
    })
    .await
    .unwrap();
    let reads = store.lock().await.metrics().segment_reads;
    let reply=e.channel(json!({"operation":"submit","source_key":"large-source","request_id":"reject-only-own","text":"拒绝"})).await.unwrap();
    assert_eq!(reply["turn_id"], tid);
    assert_ne!(e.turn(&tid).await.unwrap().state, "awaiting_confirmation");
    assert_eq!(
        e.turn(&foreign_id).await.unwrap().state,
        "awaiting_confirmation"
    );
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        128
    );
}

#[tokio::test]
async fn concurrent_own_source_change_is_rejected_but_foreign_source_does_not_retarget() {
    for own_source in [true, false] {
        let (_dir, store, e, task) = small_source().await;
        let cold = e.state.lock().await.sessions[0].cold.clone().unwrap();
        let baseline = Arc::strong_count(&cold);
        let reader = store.lock().await;
        let worker = {
            let e = e.clone();
            tokio::spawn(async move { e.source_target("small-source").await })
        };
        tokio::time::timeout(Duration::from_secs(3), async {
            while Arc::strong_count(&cold) <= baseline {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("selection must capture immutable probes before testing the race");
        {
            let mut state = e.state.lock().await;
            if own_source {
                state.sessions[0].updated_at = "2026-11-18T18:01:00Z".into();
            } else {
                state
                    .sessions
                    .push(empty_session("wechat", "another-source", &id()));
            }
        }
        drop(reader);
        let result = worker.await.unwrap();
        if own_source {
            assert!(result
                .err()
                .unwrap()
                .starts_with("SOURCE_SELECTION_CHANGED:"));
        } else {
            assert_eq!(result.unwrap().unwrap().turn_id, task);
        }
        assert!(e.archive_leases.protected().unwrap().is_empty());
        assert_eq!(
            e.state
                .lock()
                .await
                .sessions
                .iter()
                .filter(|s| s.cold.is_some())
                .count(),
            3
        );
    }
}

#[tokio::test]
async fn failed_control_receipt_save_leaves_no_action_and_retry_creates_once() {
    let (dir, store, e, _) = small_source().await;
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let request = json!({"operation":"submit","source_key":"small-source","request_id":"failed-source-new","text":"新对话"});
    store.lock().await.set_fail_before_head(true);
    assert!(e
        .channel(request.clone())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INJECTED:"));
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    store.lock().await.set_fail_before_head(false);
    let response = e.channel(request.clone()).await.unwrap();
    assert_eq!(e.channel(request).await.unwrap(), response);
    assert_eq!(e.sessions().await.len(), 4);
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert!(e.archive_leases.protected().unwrap().is_empty());
}

#[tokio::test]
async fn bare_continuation_restores_only_last_inserted_session_and_does_not_resume_older_task() {
    let (dir, store, e, task) = small_source().await;
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let error = e.channel(json!({"operation":"submit","source_key":"small-source","request_id":"continue-current-only","text":"继续"})).await.err().unwrap();
    assert!(error.contains("没有可明确继续"));
    let state = e.state.lock().await;
    assert!(state.sessions[0].cold.is_some());
    assert!(state.sessions[1].cold.is_some());
    assert!(state.sessions[2].cold.is_none());
    assert!(state.channel_controls.is_empty());
    drop(state);
    assert_eq!(e.turn(&task).await.unwrap().state, "completed");
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    assert!(store.lock().await.metrics().segment_reads <= 6);
    assert!(e.archive_leases.protected().unwrap().is_empty());
}
