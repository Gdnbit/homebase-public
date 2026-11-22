use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "assistant_archive_window_tests.rs"]
mod window_tests;

#[path = "assistant_archive_history_tests.rs"]
mod history_tests;

#[path = "assistant_archive_read_tests.rs"]
mod read_tests;

#[path = "assistant_archive_capacity_tests.rs"]
mod capacity_tests;

#[path = "assistant_route_replay_tests.rs"]
mod route_replay_tests;
#[path = "assistant_archive_catalog_tests.rs"]
mod catalog_tests;

fn request(sid: Option<String>, text: &str, rid: &str) -> SendRequest {
    SendRequest {
        image: None,
        session_id: sid,
        text: text.into(),
        request_id: rid.into(),
        route_id: None,
        attachments: vec![],
        mode: Some("chat".into()),
    }
}
async fn complete(engine: &Engine, tid: &str) {
    for _ in 0..1500 {
        let t = engine.turn(tid).await.unwrap();
        if t.state == "completed" {
            return;
        }
        assert!(active(&t.state), "{}", t.text);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("synthetic chat timed out");
}
pub(super) async fn filled() -> (tempfile::TempDir, State, Arc<AtomicUsize>) {
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let model: Model = Arc::new({
        let calls = calls.clone();
        move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                wire::parse(
                    "chat",
                    &json!({"choices":[{"message":{"content":"仅合成的聊天回复🙂"}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"cost":0.0}}),
                )
            })
        }
    });
    let save: Save = Arc::new(|_| Box::pin(async { Ok(()) }));
    let control: Control = Arc::new(|_| Box::pin(async { panic!("chat must not call tools") }));
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.route_id = "synthetic-only".into();
    let engine = Engine::new(state, save, model, control, dir.path().join("workspace"))
        .await
        .unwrap();
    for n in 0..2 {
        let ack = engine
            .send(request(None, "仅合成的闲聊", &format!("cold-original-{n}")))
            .await
            .unwrap();
        complete(&engine, &ack.turn_id).await;
    }
    let ack = engine.channel(json!({"operation":"submit","source_key":"synthetic-wechat-owner","text":"聊聊合成测试","request_id":"cold-wechat-original"})).await.unwrap();
    complete(&engine, ack["turn_id"].as_str().unwrap()).await;
    let state = engine.state.lock().await.clone();
    drop(engine);
    (dir, state, calls)
}
async fn stored(dir: &std::path::Path, state: State) -> Arc<Mutex<Store>> {
    let mut store = Store::new(dir.join("history-v1"), dir.join("state.dpapi")).unwrap();
    assert!(store.load_index().unwrap().is_none());
    persist(&mut store, state).await.unwrap();
    Arc::new(Mutex::new(store))
}
async fn engine(
    store: Arc<Mutex<Store>>,
    workspace: PathBuf,
    calls: Arc<AtomicUsize>,
) -> Arc<Engine> {
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let state = load(store.clone(), snapshot).await.unwrap();
    let save: Save = Arc::new(move |state| {
        let store = store.clone();
        Box::pin(async move { persist(&mut *store.lock().await, state).await })
    });
    let model: Model = Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err("unexpected synthetic model invocation".into()) })
    });
    let control: Control =
        Arc::new(|_| Box::pin(async { panic!("cold reads must not execute tools") }));
    Engine::new(state, save, model, control, workspace)
        .await
        .unwrap()
}

#[tokio::test]
async fn usage_groups_reconcile_hot_and_cold_records_without_loading_bodies() {
    let (dir, mut full, calls) = filled().await;
    full.sessions[0].turns[0].usage = (0..14).map(|index| wire::ModelUsage {
        model: Some(format!("synthetic/model-{index:02}")),
        cost: Some(0.125),
        prompt_tokens: Some(2),
        completion_tokens: Some(1),
    }).collect();
    full.sessions[1].turns[0].usage.clear();
    full.sessions[2].queue_paused = true;
    full.sessions[2].turns[0].usage = vec![
        wire::ModelUsage { model: Some("synthetic/model-13".into()), cost: None, prompt_tokens: Some(7), completion_tokens: None },
        wire::ModelUsage { model: None, cost: Some(0.0), prompt_tokens: None, completion_tokens: Some(2) },
        wire::ModelUsage { model: Some(" ".into()), cost: None, prompt_tokens: Some(3), completion_tokens: None },
    ];
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    assert_eq!(e.state.lock().await.sessions.iter().filter(|session| session.cold.is_some()).count(), 2);
    let before = store.lock().await.metrics().segment_reads;
    let result = serde_json::to_value(e.usage().await).unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, before);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(result["turns"], 3);
    assert_eq!(result["unreported_turns"], 1);
    assert_eq!(result["samples"], 17);
    assert_eq!(result["reported_cost_usd"], 1.75);
    assert_eq!(result["prompt_tokens"], 38);
    assert_eq!(result["completion_tokens"], 16);
    assert_eq!(result["complete"], false);
    let groups = result["groups"].as_array().unwrap();
    assert_eq!(groups.len(), 14);
    assert_eq!(groups[0]["model"], "synthetic/model-13");
    assert_eq!(groups[0]["totals"]["cost_unknown_samples"], 1);
    assert_eq!(groups[0]["totals"]["prompt_tokens"], 9);
    assert_eq!(groups[1]["model"], "synthetic/model-00");
    assert_eq!(groups[12]["kind"], "unidentified");
    assert!(groups[12]["model"].is_null());
    assert_eq!(groups[12]["totals"]["reported_cost_usd"], 0.0);
    assert_eq!(groups[12]["totals"]["cost_unknown_samples"], 1);
    assert_eq!(groups[13]["kind"], "other");
    assert_eq!(groups[13]["model_count"], 2);
    for field in ["samples", "cost_unknown_samples", "prompt_tokens", "prompt_unknown_samples", "completion_tokens", "completion_unknown_samples"] {
        assert_eq!(groups.iter().filter_map(|group| group["totals"][field].as_u64()).sum::<u64>(), result[field].as_u64().unwrap(), "{field}");
    }
    assert_eq!(groups.iter().filter_map(|group| group["totals"]["reported_cost_usd"].as_f64()).sum::<f64>(), 1.75);
}

#[tokio::test]
async fn startup_metadata_avoids_body_reads_and_original_queries_remain_cold() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert_eq!(e.sessions().await.len(), 3);
    assert!(e
        .sessions()
        .await
        .iter()
        .all(|s| s.message_count == 2 && s.active_turn_id.is_none()));
    let usage = e.usage().await;
    assert_eq!(usage.prompt_tokens, Some(21));
    assert_eq!(usage.turns, 3);
    assert!(usage.complete);
    assert!(e.settings().await.enabled);
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
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
    let view = e.session(&original.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(view.messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
    assert_eq!(store.lock().await.metrics().segment_reads, 2);
    e.turn(&original.turns[0].id).await.unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, 2);
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
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn metadata_save_preserves_cold_bodies_and_logical_budget_and_restart() {
    let (dir, full, calls) = filled().await;
    let expected: Vec<_> = full
        .sessions
        .iter()
        .map(|s| serde_json::to_value(&s.messages).unwrap())
        .collect();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let logical = logical_bytes(&*e.state.lock().await).unwrap();
    let raw = store.lock().await.materialize(&snapshot).unwrap();
    assert_eq!(logical, serde_json::to_vec(&raw).unwrap().len());
    let before = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let mut settings = e.settings().await;
    settings.max_tokens = 3000;
    e.save_settings(settings).await.unwrap();
    assert_ne!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        before
    );
    let updated = store.lock().await.load_index().unwrap().unwrap();
    for s in &e.state.lock().await.sessions {
        for kind in [Kind::Messages, Kind::Turns, Kind::Jobs] {
            assert_eq!(
                snapshot.record_ids(&s.id, kind).unwrap(),
                updated.record_ids(&s.id, kind).unwrap()
            );
        }
    }
    drop(e);
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    assert_eq!(e.settings().await.max_tokens, 3000);
    let sessions = e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .map(|s| s.id.clone())
        .collect::<Vec<_>>();
    for (id, expected) in sessions.iter().zip(expected) {
        assert_eq!(
            serde_json::to_value(e.session(id).await.unwrap().messages).unwrap(),
            expected
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn desktop_and_wechat_original_receipts_are_reused_and_foreign_owner_is_rejected_before_body_read(
) {
    let (dir, full, calls) = filled().await;
    let desktop = full.sessions[0].clone();
    let wechat = full.sessions[2].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    assert!(e
        .channel(
            json!({"operation":"get","source_key":"foreign-owner","turn_id":wechat.turns[0].id})
        )
        .await
        .is_err());
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    let original = request(None, "仅合成的闲聊", "cold-original-0");
    assert_eq!(
        e.desktop_receipt(&original).await.unwrap().unwrap().turn_id,
        desktop.turns[0].id
    );
    assert_eq!(e.send(original).await.unwrap().turn_id, desktop.turns[0].id);
    let replay = e.channel(json!({"operation":"submit","source_key":"synthetic-wechat-owner","text":"聊聊合成测试","request_id":"cold-wechat-original"})).await.unwrap();
    assert_eq!(replay["turn_id"], wechat.turns[0].id);
    assert_eq!(e.channel(json!({"operation":"get","source_key":"synthetic-wechat-owner","turn_id":wechat.turns[0].id})).await.unwrap()["status"], "succeeded");
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
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn body_corruption_cannot_replace_cold_state_or_be_misreported_as_empty() {
    let (dir, full, calls) = filled().await;
    let sid = full.sessions[0].id.clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store, dir.path().join("workspace"), calls).await;
    let files = std::fs::read_dir(dir.path().join("history-v1/segments")).unwrap();
    for entry in files {
        std::fs::write(entry.unwrap().path(), b"synthetic-corruption").unwrap();
    }
    assert!(e
        .session(&sid)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_CHANGED"));
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(e.sessions().await[0].message_count, 2);
}
#[tokio::test]
async fn tasks_pending_recovery_links_and_control_receipts_are_never_projected_as_cold_chat() {
    let (_dir, full, _) = filled().await;
    assert!(summary(&full, &full.sessions[0]).is_some());
    for status in [
        "queued",
        "waiting_jobs",
        "awaiting_confirmation",
        "interrupted_unknown",
        "failed",
        "disabled",
    ] {
        let mut changed = full.clone();
        changed.sessions[0].turns[0].state = status.into();
        assert!(summary(&changed, &changed.sessions[0]).is_none());
    }
    let mut changed = full.clone();
    let owned_turn = changed.sessions[0].turns[0].id.clone();
    changed.sessions[0].jobs.push(Job {
        archived_result_bytes: None,
        id: "synthetic-owned".into(),
        command: "system_info".into(),
        turn_id: owned_turn,
        state: "succeeded".into(),
        result: json!({}),
        owned: true,
        deliver: false,
        artifacts: vec![],
        remote: None,
    });
    assert!(summary(&changed, &changed.sessions[0]).is_none());
    let mut changed = full.clone();
    changed.sessions[1].turns[0].parent_turn_id = Some(changed.sessions[0].turns[0].id.clone());
    assert!(summary(&changed, &changed.sessions[0]).is_none());
    assert!(summary(&changed, &changed.sessions[1]).is_none());
    let mut changed = full.clone();
    changed.matters.entries.push(serde_json::from_value(json!({"id":"synthetic-matter","title":"test","goal":"test","summary":"","next_action":"","state":"waiting","review_at":null,"delivery":"idle","updated_at":now(),"source":"desktop","source_key":"desktop","turns":[changed.sessions[0].turns[0].id],"previous":[]})).unwrap());
    assert!(summary(&changed, &changed.sessions[0]).is_none());
    assert!(summary(&changed, &changed.sessions[1]).is_some());
    let mut changed = full;
    changed.channel_controls.push(serde_json::from_value(json!({"key":"synthetic","fingerprint":"synthetic","action":"新对话","session_id":changed.sessions[0].id,"turn_id":"","confirmation":"","generation":0,"response":null})).unwrap());
    assert!(summary(&changed, &changed.sessions[0]).is_none());
    assert!(summary(&changed, &changed.sessions[1]).is_some());
}

#[tokio::test]
async fn cold_save_failure_keeps_settings_and_cipher_and_cannot_bypass_legacy_or_capacity_guards() {
    let (dir, full, calls) = filled().await;
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls).await;
    assert!(ensure_legacy(&*e.state.lock().await).is_err());
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let before = e.settings().await;
    std::fs::write(dir.path().join("state.dpapi"), b"synthetic-legacy-writer").unwrap();
    let mut next = before.clone();
    next.max_tokens += 1;
    assert!(e
        .save_settings(next)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_LEGACY_CONFLICT"));
    assert_eq!(e.settings().await.max_tokens, before.max_tokens);
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    let mut oversized = e.state.lock().await.clone();
    Arc::make_mut(oversized.sessions[0].cold.as_mut().unwrap())
        .summary
        .extra_bytes = 24 * 1024 * 1024;
    assert!(logical_bytes(&oversized).unwrap() > 24 * 1024 * 1024);
    assert!(persist(&mut *store.lock().await, oversized)
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_LIMIT"));
}

#[tokio::test]
#[ignore = "explicit synthetic native fixture output only"]
async fn seed_isolated_native_cold_conversations() {
    let target =
        std::env::var_os("HOMEBASE_COLD_SEED_HOME").expect("explicit fixture home required");
    let target = PathBuf::from(target);
    assert!(target.is_absolute() && !target.exists());
    assert!(target.parent().unwrap().is_dir());
    let (_dir, mut state, calls) = filled().await;
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    state.settings.enabled = false;
    if std::env::var("HOMEBASE_COLD_SEED_CONTEXT_TEXT").as_deref() == Ok("1") {
        for (i, session) in state.sessions.iter_mut().enumerate() {
            session.history = vec![vec![
                json!({"role":"user","content":format!("CTX_NATIVE_ONLY_蓝色圆点🙂_{i}")}),
                json!({"role":"assistant","content":"仅合成的上下文，保持原值"}),
            ]];
        }
    }
    let root = target.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let plain = serde_json::to_vec(&state).unwrap();
    let cipher = crate::wechat_crypto::crypt(&plain, true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &cipher)
        .await
        .unwrap();
    let mut store = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
    store.load_index().unwrap();
    persist(&mut store, state.clone()).await.unwrap();
    let expected = json!({"synthetic_only":true,"real_model_calls":0,"sessions":state.sessions.iter().map(|s|json!({"id":s.id,"source":s.source,"history":s.history,"messages":s.messages,"turns":s.turns.iter().map(|t|json!({"id":t.id,"state":t.state,"request_id":t.request_id})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
    crate::control_protocol::publish_private(
        &target.join("cold-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn context_migration_hydrates_old_once_then_restarts_without_context_or_message_body_reads() {
    let (dir, mut full, calls) = filled().await;
    for session in &mut full.sessions {
        session.history = vec![vec![json!({"role":"user","content":"CTX_LAZY_ONLY_🙂"})]];
    }
    let original = full.sessions[0].clone();
    let old = stored(dir.path(), full).await;
    let snapshot = old.lock().await.load_index().unwrap().unwrap();
    old.lock().await.enable_context_segments();
    let migration = load(old.clone(), snapshot).await.unwrap();
    assert!(migration.sessions.iter().all(|s| s.cold.is_none()));
    assert_eq!(migration.sessions[0].history, original.history);
    persist(&mut *old.lock().await, migration).await.unwrap();
    drop(old);
    let store = Arc::new(Mutex::new(
        Store::new(
            dir.path().join("history-v1"),
            dir.path().join("state.dpapi"),
        )
        .unwrap(),
    ));
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.cold.is_some() && s.history.is_empty()));
    assert_eq!(e.usage().await.prompt_tokens, Some(21));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    assert!(!serde_json::to_string(&snapshot.metadata())
        .unwrap()
        .contains("CTX_LAZY_ONLY"));
    let logical = logical_bytes(&*e.state.lock().await).unwrap();
    // Use a separate reader so the Engine's measured cache remains cold.
    let mut check = Store::new(
        dir.path().join("history-v1"),
        dir.path().join("state.dpapi"),
    )
    .unwrap();
    let check_snapshot = check.load_index().unwrap().unwrap();
    assert_eq!(
        logical,
        serde_json::to_vec(&check.materialize(&check_snapshot).unwrap())
            .unwrap()
            .len()
    );
    e.session(&original.id).await.unwrap();
    assert_eq!(e.read_archive_session(&original.id).await.unwrap().history, original.history);
    assert!(e.state.lock().await.sessions[0].history.is_empty());
    assert_eq!(store.lock().await.metrics().segment_reads, 3);
    let mut settings = e.settings().await;
    settings.max_tokens = 3000;
    e.save_settings(settings).await.unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, 3);
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
    let repeated = e
        .send(request(None, "仅合成的闲聊", "cold-original-0"))
        .await
        .unwrap();
    assert_eq!(repeated.turn_id, original.turns[0].id);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let wechat_session = e.state.lock().await.sessions[2].id.clone();
    let wechat_turn = snapshot.record_ids(&wechat_session, Kind::Turns).unwrap()[0].clone();
    assert!(e
        .channel(json!({"operation":"get","source_key":"foreign-owner","turn_id":wechat_turn}))
        .await
        .is_err());
    assert_eq!(store.lock().await.metrics().segment_reads, 3);
    assert_eq!(
        e.channel(
            json!({"operation":"get","source_key":"synthetic-wechat-owner","turn_id":wechat_turn})
        )
        .await
        .unwrap()["status"],
        "succeeded"
    );
    assert_eq!(store.lock().await.metrics().segment_reads, 5);
    assert_eq!(e.read_archive_session(&wechat_session).await.unwrap().history, original.history);
    assert_eq!(store.lock().await.metrics().segment_reads, 6);
    assert!(e.state.lock().await.sessions[2].history.is_empty());
}

#[tokio::test]
#[ignore = "explicit synthetic snapshot export/verification in fresh lab only"]
async fn export_or_verify_native_context_snapshot() {
    let home = PathBuf::from(
        std::env::var_os("HOMEBASE_CONTEXT_SOURCE_HOME").expect("explicit source home"),
    );
    assert!(
        home.is_absolute()
            && home
                .to_string_lossy()
                .replace('\\', "/")
                .starts_with("F:/HomeBase-Labs/")
    );
    let expected: Value =
        serde_json::from_slice(&std::fs::read(home.join("cold-expected.json")).unwrap()).unwrap();
    assert_eq!(expected["synthetic_only"], true);
    let root = home.join(".homebase/assistant");
    let mut source = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
    let snapshot = source.load_index().unwrap().unwrap();
    if let Ok(format) = std::env::var("HOMEBASE_CONTEXT_FIXTURE_FORMAT") {
        assert!(matches!(format.as_str(), "1" | "2"));
        assert_eq!(snapshot.context_segmented(), format == "2");
    }
    let value = source.materialize(&snapshot).unwrap();
    let state: State = serde_json::from_value(value.clone()).unwrap();
    for row in expected["sessions"].as_array().unwrap() {
        let session = state
            .sessions
            .iter()
            .find(|s| Some(s.id.as_str()) == row["id"].as_str())
            .unwrap();
        assert_eq!(
            serde_json::to_value(&session.messages).unwrap(),
            row["messages"]
        );
        assert_eq!(
            serde_json::to_value(&session.history).unwrap(),
            row["history"]
        );
        assert_eq!(session.turns.len(), row["turns"].as_array().unwrap().len());
        for (turn, original) in session.turns.iter().zip(row["turns"].as_array().unwrap()) {
            assert_eq!(turn.id, original["id"]);
            assert_eq!(turn.request_id, original["request_id"]);
            assert_eq!(turn.state, original["state"]);
        }
        assert!(session.jobs.is_empty());
    }
    let mode = std::env::var("HOMEBASE_CONTEXT_FIXTURE_MODE").unwrap();
    if mode == "export" {
        assert!(snapshot.context_segmented());
        let target = PathBuf::from(
            std::env::var_os("HOMEBASE_CONTEXT_EXPORT_HOME").expect("explicit new target"),
        );
        assert!(target.is_absolute() && !target.exists() && target.parent().unwrap().is_dir());
        assert!(target
            .to_string_lossy()
            .replace('\\', "/")
            .starts_with("F:/HomeBase-Labs/"));
        let root = target.join(".homebase/assistant");
        std::fs::create_dir_all(&root).unwrap();
        let mut output = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
        source.export_v1(&snapshot, &mut output).await.unwrap();
        let target_snapshot = output.load_index().unwrap().unwrap();
        assert!(!target_snapshot.context_segmented());
        let mut complete = value;
        complete.as_object_mut().unwrap().remove("archive_index");
        assert_eq!(output.materialize(&target_snapshot).unwrap(), complete);
        crate::control_protocol::publish_private(
            &target.join("cold-expected.json"),
            &serde_json::to_vec(&expected).unwrap(),
        )
        .await
        .unwrap();
    } else {
        assert_eq!(mode, "verify");
    }
}
async fn completed_task(
    wechat: bool,
    deep: bool,
) -> (tempfile::TempDir, State, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    use crate::control_state::{Controller, Runner};
    let dir = tempfile::tempdir().unwrap();
    let executions = Arc::new(AtomicUsize::new(0));
    let models = Arc::new(AtomicUsize::new(0));
    let runner: Runner = Arc::new({
        let count = executions.clone();
        move |command, _| {
            let count = count.clone();
            Box::pin(async move {
                assert_eq!(command, "system_info");
                count.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"status":"succeeded","synthetic":true,"gpu_status":"not_detected"}))
            })
        }
    });
    let control_root = dir.path().join("control");
    std::fs::create_dir(&control_root).unwrap();
    let controller = Controller::new(catalog().clone(), &control_root, runner).unwrap();
    let control: Control = Arc::new(move |request| {
        let c = controller.clone();
        Box::pin(async move { c.handle(request).await })
    });
    let replies = Arc::new(Mutex::new(std::collections::VecDeque::from([
        json!({"choices":[{"message":{"tool_calls":[{"id":"cold-task-tool","function":{"name":"homebase_call","arguments":json!({"command":"system_info","args":{}}).to_string()}}]}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"cost":0.0}}),
        json!({"choices":[{"message":{"content":"合成任务已完成，原结果保留。"}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"cost":0.0}}),
    ])));
    let model: Model = Arc::new({
        let models = models.clone();
        move |_| {
            let models = models.clone();
            let replies = replies.clone();
            Box::pin(async move {
                models.fetch_add(1, Ordering::SeqCst);
                wire::parse(
                    "chat",
                    &replies
                        .lock()
                        .await
                        .pop_front()
                        .ok_or("synthetic planner exhausted")?,
                )
            })
        }
    });
    let save: Save = Arc::new(|_| Box::pin(async { Ok(()) }));
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.route_id = "synthetic-only".into();
    if deep {
        state.settings.runtime = "pi".into();
    }
    let e = Engine::new(state, save, model, control, dir.path().join("workspace"))
        .await
        .unwrap();
    let tid = if wechat {
        e.channel(json!({"operation":"submit","source_key":"synthetic-task-owner","text":"请查询合成系统信息","request_id":"cold-task-original"})).await.unwrap()["turn_id"].as_str().unwrap().to_string()
    } else {
        let mut r = request(None, "查询合成系统信息", "cold-task-original");
        r.mode = Some("task".into());
        e.send(r).await.unwrap().turn_id
    };
    complete(&e, &tid).await;
    let mut state = e.state.lock().await.clone();
    drop(e);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let bytes = "仅合成的原任务产物🙂".as_bytes();
    let file = workspace.join("task-result.md");
    std::fs::write(&file, bytes).unwrap();
    let job = state.sessions[0].jobs.get_mut(0).unwrap();
    job.artifacts.push(Artifact {
        id: "synthetic-task-artifact".into(),
        job_id: job.id.clone(),
        name: "task-result.md".into(),
        path: file.to_string_lossy().into_owned(),
        bytes: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
        mime_type: "text/markdown".into(),
    });
    enable_tasks(&mut state);
    state.settings.enabled = false;
    (dir, state, executions, models)
}
async fn stored_task(dir: &std::path::Path, state: State) -> Arc<Mutex<Store>> {
    let mut store = Store::new(dir.join("history-v1"), dir.join("state.dpapi")).unwrap();
    store.enable_context_segments();
    store.load_index().unwrap();
    persist(&mut store, state).await.unwrap();
    Arc::new(Mutex::new(store))
}

#[tokio::test]
async fn completed_owned_task_cold_restart_keeps_original_results_context_and_artifact_without_execution(
) {
    let (dir, full, executions, models) = completed_task(false, false).await;
    let original = full.sessions[0].clone();
    let summary = task_summary(&original).unwrap();
    let job = &original.jobs[0];
    for (key, value) in [
        ("turn_id", json!("foreign-turn")),
        ("result", json!({"partial": true})),
    ] {
        let mut malformed = serde_json::to_value(job).unwrap();
        malformed[key] = value;
        assert!(archived_job(malformed, &job.id, &summary)
            .err()
            .unwrap()
            .starts_with("ARCHIVE_INVALID"));
    }
    let store = stored_task(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert_eq!(e.sessions().await[0].message_count, 2);
    assert_eq!(e.usage().await.prompt_tokens, Some(14));
    let artifact = e.artifact("synthetic-task-artifact").await.unwrap();
    assert_eq!(artifact.sha256, original.jobs[0].artifacts[0].sha256);
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    e.artifact("synthetic-task-artifact").await.unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    let _archive_lease = e.ensure_archive_session(&original.id).await.unwrap();
    let restored = e.state.lock().await.sessions[0].clone();
    assert_eq!(
        serde_json::to_value(&restored.jobs).unwrap(),
        serde_json::to_value(&original.jobs).unwrap()
    );
    assert_eq!(restored.history, original.history);
    assert_eq!(
        serde_json::to_value(&restored.turns).unwrap(),
        serde_json::to_value(&original.turns).unwrap()
    );
    let mut repeated = request(None, "查询合成系统信息", "cold-task-original");
    repeated.mode = Some("task".into());
    assert_eq!(
        e.send(repeated).await.unwrap().turn_id,
        original.turns[0].id
    );
    assert!(e.resume(&original.turns[0].id).await.is_err());
    e.cancel(&original.turns[0].id).await.unwrap();
    assert_eq!(
        e.turn(&original.turns[0].id).await.unwrap().state,
        "completed"
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn completed_pi_wechat_task_cold_lookup_is_source_bound_and_reuses_original_receipt()
{
    let (dir, full, executions, models) = completed_task(true, true).await;
    let original = full.sessions[0].clone();
    assert!(task_summary(&original).is_some());
    let store = stored_task(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert!(e
        .channel(json!({"operation":"get","source_key":"foreign","turn_id":original.turns[0].id}))
        .await
        .is_err());
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert_eq!(e.channel(json!({"operation":"get","source_key":"synthetic-task-owner","turn_id":original.turns[0].id})).await.unwrap()["status"],"succeeded");
    assert_eq!(e.channel(json!({"operation":"submit","source_key":"synthetic-task-owner","text":"请查询合成系统信息","request_id":"cold-task-original"})).await.unwrap()["turn_id"],original.turns[0].id);
    assert_eq!(
        e.turn(&original.turns[0].id).await.unwrap().jobs[0].id,
        original.jobs[0].id
    );
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn task_protection_retains_incomplete_unknown_pending_delivery_and_remote_records_hot() {
    let (_dir, full, _, _) = completed_task(false, false).await;
    let original = &full.sessions[0];
    assert!(task_summary(original).is_some());
    for state in [
        "failed",
        "queued",
        "waiting_jobs",
        "interrupted_unknown",
        "awaiting_confirmation",
    ] {
        let mut s = original.clone();
        s.turns[0].state = state.into();
        assert!(task_summary(&s).is_none());
    }
    for state in [
        "running",
        "queued",
        "unknown",
        "failed",
        "interrupted_unknown",
    ] {
        let mut s = original.clone();
        s.jobs[0].state = state.into();
        assert!(task_summary(&s).is_none());
    }
    let mut s = original.clone();
    s.jobs[0].deliver = true;
    assert!(task_summary(&s).is_none());
    let mut s = original.clone();
    s.turns[0].delivery_jobs.push(s.jobs[0].id.clone());
    assert!(task_summary(&s).is_none());
    let mut s = original.clone();
    s.turns[0].steps[0].state = "unknown".into();
    assert!(task_summary(&s).is_none());
    let mut s = original.clone();
    s.jobs[0].result = Value::Null;
    assert!(task_summary(&s).is_none());
    let mut s = original.clone();
    s.jobs[0].remote = Some(Remote {
        route_id: "synthetic".into(),
        task_id: "original".into(),
        query_job: None,
        polls: 0,
        next_poll: 0,
        finished: false,
        credential_source: Default::default(),
    });
    assert!(task_summary(&s).is_none());
}
fn task_variant(original: &Session, suffix: &str) -> Session {
    let mut s = original.clone();
    assert_eq!(s.turns.len(), 1);
    assert_eq!(s.jobs.len(), 1);
    s.id = format!("task-session-{suffix}");
    s.turns[0].id = format!("task-turn-{suffix}");
    s.turns[0].request_id = format!("task-request-{suffix}");
    let tid = s.turns[0].id.clone();
    let job = format!("task-job-{suffix}");
    for message in s.messages.iter_mut() {
        message.id = format!("{}-{suffix}", message.id);
        message.turn_id = Some(tid.clone());
    }
    s.jobs[0].id = job.clone();
    s.jobs[0].turn_id = tid;
    for item in &mut s.jobs[0].artifacts {
        item.job_id = job.clone();
    }
    for step in &mut s.turns[0].steps {
        if step.job_id.is_some() {
            step.job_id = Some(job.clone());
        }
        step.id = format!("{}-{suffix}", step.id);
    }
    s
}

#[tokio::test]
async fn cold_job_hints_and_observed_owned_flags_never_grant_files_or_cross_result_namespaces() {
    let (dir, mut full, _, models) = completed_task(false, false).await;
    let original = full.sessions[0].jobs[0].clone();
    full.sessions[0].jobs[0].owned = false;
    full.sessions[0].turns[0].steps[0].name = "homebase_jobs".into();
    full.sessions[0].turns[0].steps[0].result =
        json!({"jobs":[{"job_id":original.id,"command":"repo_capability_run","owned":true}]});
    for (suffix, command, result) in [
        ("repo", "repo_capability_run", json!({"run_id":"repo-run"})),
        (
            "skill",
            "assistant_local_execute",
            json!({"workspace_id":"skill-workspace","skill_id":"installed:fixture"}),
        ),
        ("video", "capability_render", json!({"run_id":"video-run"})),
    ] {
        let mut job = original.clone();
        job.id = format!("namespace-{suffix}");
        job.command = command.into();
        job.result = result;
        job.artifacts.clear();
        full.sessions[0].jobs.push(job);
    }
    let before = serde_json::to_value(&full.sessions[0].jobs).unwrap();
    let tid = full.sessions[0].turns[0].id.clone();
    let store = stored_task(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    assert!(e
        .artifact("synthetic-task-artifact")
        .await
        .err()
        .unwrap()
        .starts_with("ARTIFACT_NOT_FOUND"));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    e.turn(&tid).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    // The authority checks below exercise explicit writable restoration too.
    let _archive_lease = e.ensure_archive_turn(&tid, None).await.unwrap();
    let locked = e.state.lock().await;
    let jobs = &locked.sessions[0].jobs;
    assert_eq!(serde_json::to_value(jobs).unwrap(), before);
    assert!(job_access::owned(jobs, &original.id).unwrap().is_none());
    let mut metadata_only = locked.sessions[0].clone();
    metadata_only.jobs = metadata_only.jobs.select(|job| job.id != original.id);
    assert!(
        !task_discovery::known(&metadata_only, &original.id)
            .unwrap()
            .unwrap()
            .owned
    );
    assert!(job_access::owns_result(jobs, job_access::ResultRef::Repository("repo-run")).unwrap());
    assert!(!job_access::owns_result(jobs, job_access::ResultRef::Video("repo-run")).unwrap());
    assert!(job_access::owns_result(
        jobs,
        job_access::ResultRef::SkillWorkspace("skill-workspace")
    )
    .unwrap());
    assert!(
        !job_access::owns_result(jobs, job_access::ResultRef::Repository("skill-workspace"))
            .unwrap()
    );
    assert!(job_access::owns_result(jobs, job_access::ResultRef::Video("video-run")).unwrap());
    assert!(
        !job_access::owns_result(jobs, job_access::ResultRef::Repository("video-run")).unwrap()
    );
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn duplicate_artifact_and_parent_job_collisions_across_cold_sessions_are_rejected_before_open(
) {
    for (parent_collision, owned) in [(false, true), (true, true), (true, false)] {
        let (dir, mut full, _, models) = completed_task(false, false).await;
        let mut second = task_variant(&full.sessions[0], "collision");
        if parent_collision {
            let id = full.sessions[0].jobs[0].id.clone();
            second.jobs[0].id = id.clone();
            second.jobs[0].owned = owned;
            for step in &mut second.turns[0].steps {
                step.job_id = Some(id.clone());
            }
            second.jobs[0].artifacts.clear();
        }
        full.sessions.push(second);
        let store = stored_task(dir.path(), full).await;
        let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
        let e = engine(store.clone(), dir.path().join("workspace"), models).await;
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
        let result = e.artifact("synthetic-task-artifact").await;
        if owned {
            assert!(result.err().unwrap().starts_with("ARTIFACT_IDENTITY_CONFLICT"));
        } else {
            assert_eq!(result.unwrap().id, "synthetic-task-artifact");
        }
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
        assert_eq!(store.lock().await.metrics().segment_reads, 2);
        assert_eq!(
            std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
            head
        );
    }
}

#[tokio::test]
async fn malformed_parent_and_changed_file_fail_in_hot_or_cold_paths_without_poisoning_unrelated_files(
) {
    let (dir, full, _, models) = completed_task(false, false).await;
    let mut bad = full.sessions[0].jobs[0].clone();
    bad.artifacts[0].job_id = "foreign-parent".into();
    assert!(job_access::artifact([&bad], "synthetic-task-artifact")
        .err()
        .unwrap()
        .starts_with("ARTIFACT_SCOPE_MISMATCH"));
    let good = &full.sessions[0].jobs[0];
    let mut unrelated = good.clone();
    unrelated.id = "unrelated-duplicate".into();
    unrelated.artifacts.clear();
    assert!(
        job_access::artifact([good, &unrelated, &unrelated], "synthetic-task-artifact")
            .unwrap()
            .is_some()
    );
    let mut unowned = good.clone();
    unowned.owned = false;
    assert!(
        job_access::artifact([good, &unowned], "synthetic-task-artifact")
            .unwrap()
            .is_some()
    );
    let file = good.artifacts[0].path.clone();
    let store = stored_task(dir.path(), full).await;
    let e = engine(store, dir.path().join("workspace"), models).await;
    e.artifact("synthetic-task-artifact").await.unwrap();
    std::fs::write(file, b"changed synthetic artifact").unwrap();
    assert!(e
        .artifact("synthetic-task-artifact")
        .await
        .err()
        .unwrap()
        .contains("已变化"));
    for code in [
        "ARTIFACT_NOT_FOUND",
        "ARTIFACT_IDENTITY_CONFLICT",
        "ARTIFACT_SCOPE_MISMATCH",
    ] {
        let (actual, message) =
            artifact_error(&format!("{code}: private-path-must-not-echo")).unwrap();
        assert_eq!(actual, code);
        assert!(!message.contains("private-path"));
    }
    assert!(artifact_error("prefix ARTIFACT_NOT_FOUND: private").is_none());
}
#[tokio::test]
#[ignore = "explicit synthetic task seed in fresh lab only"]
async fn seed_isolated_native_closed_task() {
    let home =
        PathBuf::from(std::env::var_os("HOMEBASE_TASK_SEED_HOME").expect("new synthetic home"));
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    let (_dir, mut state, effects, models) = completed_task(false, false).await;
    let original = state.sessions[0].clone();
    let mut parent = task_variant(&original, "bad-parent");
    parent.jobs[0].artifacts[0].id = "synthetic-bad-parent".into();
    parent.jobs[0].artifacts[0].job_id = "foreign-parent".into();
    let mut a = task_variant(&original, "duplicate-a");
    a.jobs[0].artifacts[0].id = "synthetic-duplicate".into();
    let mut b = task_variant(&original, "duplicate-b");
    b.jobs[0].artifacts[0].id = "synthetic-duplicate".into();
    let mut unowned = task_variant(&original, "unowned");
    unowned.jobs[0].owned = false;
    unowned.jobs[0].artifacts[0].id = "synthetic-unowned".into();
    state.sessions.extend([parent, a, b, unowned]);
    if std::env::var("HOMEBASE_TASK_SEED_ARCHIVED").as_deref() != Ok("1") {
        state.archive_task_version = 0;
    }
    let root = home.join(".homebase/assistant");
    let workspace = home.join(".homebase/wechat/workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let file = workspace.join("task-result.md");
    let bytes = "仅合成的原任务产物🙂".as_bytes();
    crate::control_protocol::publish_private(&file, bytes)
        .await
        .unwrap();
    for session in &mut state.sessions {
        for job in session.jobs.iter_mut() {
            for artifact in &mut job.artifacts {
                artifact.path = file.to_string_lossy().into_owned();
            }
        }
    }
    let cipher = crate::wechat_crypto::crypt(&serde_json::to_vec(&state).unwrap(), true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &cipher)
        .await
        .unwrap();
    let mut store = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
    store.enable_context_segments();
    store.load_index().unwrap();
    persist(&mut store, state.clone()).await.unwrap();
    let expected = json!({"synthetic_only":true,"genuine_effects":effects.load(Ordering::SeqCst),"synthetic_model_calls":models.load(Ordering::SeqCst),"adversarial_record_copies":4,"request":{"session_id":null,"text":"查询合成系统信息","request_id":"cold-task-original","mode":"task","attachments":[]},"content":String::from_utf8(bytes.to_vec()).unwrap(),"sessions":state.sessions.iter().map(|s|json!({"id":s.id,"messages":s.messages,"turns":s.turns,"jobs":s.jobs,"history":s.history})).collect::<Vec<_>>()});
    crate::control_protocol::publish_private(
        &home.join("task-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "explicit synthetic task verification/export only"]
async fn verify_or_export_native_closed_task() {
    let home = PathBuf::from(
        std::env::var_os("HOMEBASE_TASK_SOURCE_HOME").expect("synthetic source home"),
    );
    assert!(
        home.is_absolute()
            && home
                .to_string_lossy()
                .replace('\\', "/")
                .starts_with("F:/HomeBase-Labs/")
    );
    let expected: Value =
        serde_json::from_slice(&std::fs::read(home.join("task-expected.json")).unwrap()).unwrap();
    assert_eq!(expected["synthetic_only"], true);
    assert_eq!(expected["genuine_effects"], 1);
    let root = home.join(".homebase/assistant");
    let mut store = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
    let snapshot = store.load_index().unwrap().unwrap();
    let value = store.materialize(&snapshot).unwrap();
    let state: State = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(state.sessions.len(), 5);
    for row in expected["sessions"].as_array().unwrap() {
        let s = state
            .sessions
            .iter()
            .find(|s| Some(s.id.as_str()) == row["id"].as_str())
            .unwrap();
        assert_eq!(serde_json::to_value(&s.messages).unwrap(), row["messages"]);
        assert_eq!(serde_json::to_value(&s.turns).unwrap(), row["turns"]);
        assert_eq!(serde_json::to_value(&s.jobs).unwrap(), row["jobs"]);
        assert_eq!(serde_json::to_value(&s.history).unwrap(), row["history"]);
    }
    if std::env::var("HOMEBASE_TASK_FIXTURE_MODE").as_deref() == Ok("export") {
        let home =
            PathBuf::from(std::env::var_os("HOMEBASE_TASK_EXPORT_HOME").expect("new target home"));
        assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
        assert!(home
            .to_string_lossy()
            .replace('\\', "/")
            .starts_with("F:/HomeBase-Labs/"));
        let root = home.join(".homebase/assistant");
        std::fs::create_dir_all(&root).unwrap();
        let mut target = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
        store.export_v1(&snapshot, &mut target).await.unwrap();
        let exported = target.load_index().unwrap().unwrap();
        let mut complete = value;
        complete.as_object_mut().unwrap().remove("archive_index");
        assert_eq!(target.materialize(&exported).unwrap(), complete);
        crate::control_protocol::publish_private(
            &home.join("task-expected.json"),
            &serde_json::to_vec(&expected).unwrap(),
        )
        .await
        .unwrap();
    } else {
        assert_eq!(
            std::env::var("HOMEBASE_TASK_FIXTURE_MODE").unwrap(),
            "verify"
        );
    }
}
#[tokio::test]
async fn artifact_preview_reuses_verified_bytes_and_rejects_format_size_and_growth_before_full_read(
) {
    let (dir, full, _, models) = completed_task(false, false).await;
    let file = full.sessions[0].jobs[0].artifacts[0].path.clone();
    let original = std::fs::read(&file).unwrap();
    let store = stored_task(dir.path(), full).await;
    let e = engine(store, dir.path().join("workspace"), models).await;
    let (artifact, bytes) = e
        .artifact_content("synthetic-task-artifact", true)
        .await
        .unwrap();
    assert_eq!(bytes, original);
    std::fs::write(&file, b"changed after the validated read").unwrap();
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), artifact.sha256);
    assert!(e
        .artifact_content("synthetic-task-artifact", true)
        .await
        .err()
        .unwrap()
        .starts_with("ARTIFACT_CHANGED"));
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    let sid = e.state.lock().await.sessions[0].id.clone();
    let _archive_lease = e.ensure_archive_session(&sid).await.unwrap();
    {
        let mut state = e.state.lock().await;
        let item = &mut state.sessions[0].jobs[0].artifacts[0];
        item.name = "unsupported.mp4".into();
        item.path = dir
            .path()
            .join("missing.txt")
            .to_string_lossy()
            .into_owned();
    }
    assert!(e
        .artifact_content("synthetic-task-artifact", true)
        .await
        .err()
        .unwrap()
        .starts_with("ARTIFACT_PREVIEW_FORMAT"));
    {
        let mut state = e.state.lock().await;
        let item = &mut state.sessions[0].jobs[0].artifacts[0];
        item.name = "result.md".into();
        item.bytes = 256 * 1024 + 1;
    }
    assert!(e
        .artifact_content("synthetic-task-artifact", true)
        .await
        .err()
        .unwrap()
        .starts_with("ARTIFACT_PREVIEW_LIMIT"));
    {
        let mut state = e.state.lock().await;
        let item = &mut state.sessions[0].jobs[0].artifacts[0];
        item.bytes = 1;
        item.path = file.clone();
    }
    let large = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
    large.set_len(1024 * 1024).unwrap();
    drop(large);
    assert!(e
        .artifact_content("synthetic-task-artifact", true)
        .await
        .err()
        .unwrap()
        .starts_with("ARTIFACT_UNAVAILABLE"));
}

#[tokio::test]
async fn artifact_image_preview_requires_registered_bytes_and_matching_format() {
    let (dir, mut full, _, models) = completed_task(false, false).await;
    let workspace = dir.path().join("workspace");
    let image_path = workspace.join("result.PNG");
    let original = b"\x89PNG\r\n\x1a\nsynthetic-image-header";
    std::fs::write(&image_path, original).unwrap();
    let image = &mut full.sessions[0].jobs[0].artifacts[0];
    image.name = "result.PNG".into();
    image.path = image_path.to_string_lossy().into_owned();
    image.bytes = original.len() as u64;
    image.sha256 = format!("{:x}", Sha256::digest(original));
    let store = stored_task(dir.path(), full).await;
    let e = engine(store, workspace, models).await;
    let bytes = e.artifact_image_content("synthetic-task-artifact").await.unwrap();
    assert_eq!(bytes, original);
    assert!(e
        .artifact_content("synthetic-task-artifact", true)
        .await
        .err()
        .unwrap()
        .starts_with("ARTIFACT_PREVIEW_FORMAT"));
    let mut changed = original.to_vec();
    changed[0] = 0;
    std::fs::write(&image_path, &changed).unwrap();
    assert!(e
        .artifact_image_content("synthetic-task-artifact")
        .await
        .unwrap_err()
        .starts_with("ARTIFACT_CHANGED"));
    let sid = e.state.lock().await.sessions[0].id.clone();
    let _archive_lease = e.ensure_archive_session(&sid).await.unwrap();
    e.state.lock().await.sessions[0].jobs[0].artifacts[0].sha256 =
        format!("{:x}", Sha256::digest(&changed));
    assert!(e
        .artifact_image_content("synthetic-task-artifact")
        .await
        .unwrap_err()
        .starts_with("ARTIFACT_IMAGE_FORMAT"));
}
