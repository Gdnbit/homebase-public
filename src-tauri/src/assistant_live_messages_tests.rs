use super::*;
use crate::assistant_history::{HistoryKind, HistoryQuery};

fn long_session(template: &Session, n: usize) -> Session {
    let mut session = template.clone();
    session.messages = Records::default();
    session.turns = Records::default();
    for index in 0..n {
        let mut turn = template.turns[0].clone();
        turn.id = format!("{}-old-turn-{index}", session.id);
        turn.request_id = format!("old-request-{index}");
        let mut user = template.messages[0].clone();
        user.id = format!("{}-old-user-{index}", session.id);
        user.content = format!("聊聊合成原文 {index} 🙂");
        user.turn_id = Some(turn.id.clone());
        turn.fingerprint = request_fingerprint(
            &request(Some(session.id.clone()), &user.content, &turn.request_id),
            &session.source_key,
        )
        .unwrap();
        turn.protocol[0]["content"] = json!(user.content);
        let mut answer = template.messages[1].clone();
        answer.id = format!("{}-old-answer-{index}", session.id);
        answer.turn_id = Some(turn.id.clone());
        session.messages.push(user);
        session.messages.push(answer);
        session.turns.push(turn);
    }
    session
}

async fn runner(
    store: Arc<Mutex<Store>>,
    state: State,
    workspace: PathBuf,
    calls: Arc<AtomicUsize>,
) -> Arc<Engine> {
    let save: Commit = Arc::new(move |state, protected| {
        let store = store.clone();
        Box::pin(async move { commit(store, state, protected).await })
    });
    let documents = Arc::new(crate::assistant_documents::DocumentStore::new(
        workspace.join(".documents"),
    ));
    let model: Model = Arc::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            wire::parse(
                "chat",
                &json!({"choices":[{"message":{"content":"新的合成答复🙂"}}]}),
            )
        })
    });
    let control: Control =
        Arc::new(|_| Box::pin(async { panic!("synthetic chat must not execute tools") }));
    Engine::new_with_commit(state, save, model, control, workspace, documents)
        .await
        .unwrap()
}

fn search(sid: &str) -> HistoryQuery {
    HistoryQuery {
        session_id: sid.into(),
        kind: HistoryKind::Messages,
        query: String::new(),
        snapshot_id: None,
        through_turn_id: None,
        offset: 0,
        limit: 8,
    }
}

#[tokio::test]
async fn live_prefix_desktop_and_simulated_wechat_continue_same_session_past_200_with_original_replays(
) {
    let (dir, mut full, _) = filled().await;
    full.sessions.remove(1);
    full.sessions = full.sessions.iter().map(|s| long_session(s, 100)).collect();
    let desktop = full.sessions[0].clone();
    let wechat = full.sessions[1].clone();
    let (store, state) = capacity_store(dir.path(), full).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let e = runner(
        store.clone(),
        state,
        dir.path().join("workspace"),
        calls.clone(),
    )
    .await;
    let submitted = request(
        Some(desktop.id.clone()),
        "再聊一条合成消息🙂",
        "new-long-desktop",
    );
    let ack = e.send(submitted.clone()).await.unwrap();
    assert_eq!(ack.session_id, desktop.id);
    complete(&e, &ack.turn_id).await;
    let inbound = json!({"operation":"submit","source_key":wechat.source_key,"request_id":"new-long-wechat","text":"聊聊合成微信续接🙂"});
    let accepted = e.channel(inbound.clone()).await.unwrap();
    assert_eq!(accepted["session_id"], wechat.id);
    let wx_turn = accepted["turn_id"].as_str().unwrap().to_string();
    complete(&e, &wx_turn).await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    for original in [&desktop, &wechat] {
        let state = e.state.lock().await;
        let projected = state.sessions.iter().find(|s| s.id == original.id).unwrap();
        assert!(projected.message_prefix.is_some());
        assert!(projected.messages.len() <= 48);
        assert_eq!(projected.turns.len(), 101);
        assert_eq!(meta(projected).message_count, 202);
        drop(state);
        let old = e.history_query(search(&original.id)).await.unwrap();
        assert_eq!(old.total, 202);
        assert_eq!(old.items[0].id, original.messages[0].id);
        let mut window = e
            .session_window(crate::assistant::window::TimelineRequest {
                session_id: original.id.clone(),
                before_message_id: Some(original.messages[8].id.clone()),
                limit: 8,
            })
            .await
            .unwrap();
        assert_eq!(window.page.total_messages, 202);
        assert_eq!(window.page.start_index, 0);
        assert_eq!(window.messages.remove(0).id, original.messages[0].id);
        assert_eq!(
            e.turn(&original.turns[0].id).await.unwrap().progress.goal,
            original.messages[0].content
        );
        let flat = e.session(&original.id).await.unwrap();
        assert_eq!(flat.messages.len(), 202);
        assert_eq!(
            serde_json::to_value(&flat.messages[..200]).unwrap(),
            serde_json::to_value(&original.messages).unwrap()
        );
    }
    let old_request = request(
        Some(desktop.id.clone()),
        &desktop.messages[0].content,
        &desktop.turns[0].request_id,
    );
    assert_eq!(
        e.send(old_request.clone()).await.unwrap().turn_id,
        desktop.turns[0].id
    );
    assert_eq!(
        e.send(submitted.clone()).await.unwrap().turn_id,
        ack.turn_id
    );
    assert_eq!(
        e.channel(inbound.clone()).await.unwrap()["turn_id"],
        wx_turn
    );
    drop(e);
    let new_store = Arc::new(Mutex::new(
        Store::new(
            dir.path().join("history-v1"),
            dir.path().join("state.dpapi"),
        )
        .unwrap(),
    ));
    let snap = new_store.lock().await.load_index().unwrap().unwrap();
    let state = load(new_store.clone(), snap).await.unwrap();
    assert!(state
        .sessions
        .iter()
        .all(|s| s.message_prefix.is_some() && s.messages.len() <= 48));
    let e = runner(
        new_store.clone(),
        state,
        dir.path().join("workspace"),
        calls.clone(),
    )
    .await;
    assert_eq!(
        e.send(old_request).await.unwrap().turn_id,
        desktop.turns[0].id
    );
    assert_eq!(e.channel(inbound).await.unwrap()["turn_id"], wx_turn);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let next = e
        .send(request(
            Some(desktop.id.clone()),
            "重启后继续聊🙂",
            "long-after-restart",
        ))
        .await
        .unwrap();
    complete(&e, &next.turn_id).await;
    assert_eq!(
        e.history_query(search(&desktop.id)).await.unwrap().total,
        204
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn live_prefix_failed_send_keeps_previous_projection_and_complete_disk_history() {
    let (dir, mut full, _) = filled().await;
    full.sessions.truncate(1);
    full.sessions[0] = long_session(&full.sessions[0], 32);
    let sid = full.sessions[0].id.clone();
    let (store, state) = capacity_store(dir.path(), full).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let e = runner(
        store.clone(),
        state,
        dir.path().join("workspace"),
        calls.clone(),
    )
    .await;
    let initial = e
        .send(request(Some(sid.clone()), "合成预热就绪", "initial-trim"))
        .await
        .unwrap();
    complete(&e, &initial.turn_id).await;
    let before = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let original = e.history_query(search(&sid)).await.unwrap();
    store.lock().await.set_fail_before_head(true);
    let req = request(
        Some(sid.clone()),
        "同编号提交失败后再试🙂",
        "original-failed-store",
    );
    assert!(e
        .send(req.clone())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INJECTED:"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        before
    );
    assert_eq!(e.history_query(search(&sid)).await.unwrap().total, 66);
    store.lock().await.set_fail_before_head(false);
    let ack = e.send(req.clone()).await.unwrap();
    complete(&e, &ack.turn_id).await;
    assert_eq!(e.send(req).await.unwrap().turn_id, ack.turn_id);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let mut frozen = search(&sid);
    frozen.snapshot_id = Some(original.snapshot_id);
    assert_eq!(e.history_query(frozen).await.unwrap().total, 66);
    assert_eq!(e.history_query(search(&sid)).await.unwrap().total, 68);
}

#[tokio::test]
async fn live_prefix_preserves_attachments_and_unknown_turns_across_later_saves_and_restart() {
    let (dir, mut full, _) = filled().await;
    full.sessions.truncate(1);
    let mut original = long_session(&full.sessions[0], 40);
    let sid = original.id.clone();
    original.messages[10].attachments = input::prepare(
        "合成附件",
        &[TextAttachment {
            name: "synthetic.txt".into(),
            content: "只读的合成文本".into(),
            document: None,
        }],
    )
    .unwrap()
    .1;
    original.turns[8].state = "interrupted_unknown".into();
    original.turns[8].runtime_uncertain_model = true;
    let unknown = original.turns[8].id.clone();
    let attachment_id = original.messages[10].id.clone();
    full.sessions[0] = original.clone();
    let (store, state) = capacity_store(dir.path(), full).await;
    let live = &state.sessions[0];
    assert!(live.message_prefix.is_some());
    assert_eq!(live.messages[0].id, attachment_id);
    assert_eq!(live.messages.len(), 70);
    let calls = Arc::new(AtomicUsize::new(0));
    let e = runner(
        store.clone(),
        state,
        dir.path().join("workspace"),
        calls.clone(),
    )
    .await;
    let mut settings = e.settings().await;
    settings.max_tokens += 1;
    e.save_settings(settings).await.unwrap();
    let head = store.lock().await.load_index().unwrap().unwrap();
    let restored = load(store.clone(), head).await.unwrap();
    let live = &restored.sessions[0];
    assert_eq!(live.messages[0].id, attachment_id);
    assert_eq!(live.messages.len(), 70);
    assert_eq!(live.turns[8].state, "interrupted_unknown");
    assert!(live.turns[8].runtime_uncertain_model);
    let restored_engine =
        runner(store, restored, dir.path().join("workspace"), calls.clone()).await;
    let whole = restored_engine.session(&sid).await.unwrap();
    assert_eq!(
        serde_json::to_value(whole.messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
    assert_eq!(
        restored_engine.turn(&unknown).await.unwrap().state,
        "interrupted_unknown"
    );
    let mut query = search(&sid);
    query.query = original.messages[10].content.clone();
    let page = restored_engine.history_query(query).await.unwrap();
    let row = page
        .items
        .iter()
        .find(|row| row.id == attachment_id)
        .unwrap();
    let result = restored_engine
        .history_read(crate::assistant_history::HistoryRead {
            session_id: sid,
            snapshot_id: page.snapshot_id,
            item_id: row.id.clone(),
            offset: 0,
            limit: 8192,
        })
        .await
        .unwrap();
    assert!(result.complete);
    assert_eq!(
        serde_json::from_str::<Value>(&result.content).unwrap(),
        serde_json::to_value(&original.messages[10]).unwrap()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "explicit fresh synthetic CLI profile in F lab only"]
async fn seed_native_live_prefix_profile() {
    let home =
        PathBuf::from(std::env::var_os("HOMEBASE_PREFIX_SEED_HOME").expect("fresh synthetic home"));
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("prefix-native-"));
    crate::assistant_state_store::safe(&home).unwrap();
    let (_dir, mut state, _) = filled().await;
    state.sessions.truncate(1);
    state.settings.enabled = false;
    let original = long_session(&state.sessions[0], 100);
    state.sessions[0] = original.clone();
    let root = home.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let legacy = crate::wechat_crypto::crypt(&serde_json::to_vec(&state).unwrap(), true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &legacy)
        .await
        .unwrap();
    let (_, projected) = capacity_store(&root, state).await;
    assert!(projected.sessions[0].cold.is_some());
    let expected = json!({
        "synthetic_only": true, "real_model_calls": 0,
        "id": original.id, "messages": original.messages,
        "first_turn": original.turns[0].id, "first_request": original.turns[0].request_id,
    });
    crate::control_protocol::publish_private(
        &home.join("prefix-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}
