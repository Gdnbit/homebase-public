use super::*;

async fn many() -> (tempfile::TempDir, State, Arc<AtomicUsize>) {
    let (dir, mut state, calls) = filled().await;
    let session = &mut state.sessions[0];
    session.messages = (0..180)
        .map(|n| Message {
            id: format!("lazy-message-{n}"),
            role: if n % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!(
                "合成正文 {n} 🙂\n{}{}",
                "正文".repeat(300),
                if [0, 79, 150].contains(&n) {
                    " EXACT_MATCH_中文🙂"
                } else {
                    ""
                }
            ),
            created_at: now(),
            turn_id: Some(session.turns[0].id.clone()),
            attachments: vec![],
        })
        .collect();
    (dir, state, calls)
}

#[tokio::test]
async fn lazy_first_page_reads_only_requested_segment_and_shared_pages_do_not_rescan() {
    let (dir, state, calls) = many().await;
    let sid = state.sessions[0].id.clone();
    let original = state.sessions[0].clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let mut request = query(&sid, HistoryKind::Messages);
    request.limit = 8;
    let first = e.history_query(request.clone()).await.unwrap();
    assert_eq!(first.total, 180);
    assert_eq!(first.items.len(), 8);
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    assert_eq!(
        body(&e, &sid, &first.snapshot_id, &first.items[0].id).await,
        serde_json::to_value(&original.messages[0]).unwrap()
    );
    let same = e.history_query(request.clone()).await.unwrap();
    assert_ne!(same.snapshot_id, first.snapshot_id);
    assert_eq!(same.items[0].json_sha256, first.items[0].json_sha256);
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    request.snapshot_id = Some(first.snapshot_id);
    request.offset = 32;
    let next = e.history_query(request).await.unwrap();
    assert_eq!(next.items[0].id, "lazy-message-32");
    assert_eq!(store.lock().await.metrics().segment_reads, 2);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn lazy_search_freezes_exact_matches_and_next_pages_reuse_the_original_scan() {
    let (dir, state, calls) = many().await;
    let sid = state.sessions[0].id.clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let mut request = query(&sid, HistoryKind::Messages);
    request.query = "exact_match_中文🙂".into();
    let first = e.history_query(request.clone()).await.unwrap();
    assert_eq!(first.total, 3);
    assert_eq!(first.items[0].id, "lazy-message-0");
    let reads = store.lock().await.metrics().segment_reads;
    assert_eq!(reads, 6);
    request.snapshot_id = Some(first.snapshot_id);
    request.offset = 1;
    let next = e.history_query(request.clone()).await.unwrap();
    assert_eq!(next.items[0].id, "lazy-message-79");
    request.offset = 2;
    let last = e.history_query(request).await.unwrap();
    assert!(last.complete);
    assert_eq!(last.items[0].id, "lazy-message-150");
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn lazy_unrelated_damage_does_not_block_first_page_but_requested_page_and_search_fail_explicitly(
) {
    let (dir, state, calls) = many().await;
    let sid = state.sessions[0].id.clone();
    let store = stored_context(dir.path(), state).await;
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let index: Value =
        serde_json::from_slice(&crate::wechat_crypto::crypt(&head, false).unwrap()).unwrap();
    let tail = index["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == sid && s["kind"] == "messages")
        .unwrap()["segments"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["file"]
        .as_str()
        .unwrap();
    let file = dir
        .path()
        .join("history-v1/segments")
        .join(format!("{tail}.dpapi"));
    let good = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"synthetic damaged tail").unwrap();
    let e = engine(store.clone(), dir.path().join("workspace"), calls).await;
    let mut request = query(&sid, HistoryKind::Messages);
    let first = e.history_query(request.clone()).await.unwrap();
    assert_eq!(first.items[0].id, "lazy-message-0");
    request.snapshot_id = Some(first.snapshot_id.clone());
    request.offset = 160;
    assert!(e
        .history_query(request)
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CHANGED:"));
    let mut search = query(&sid, HistoryKind::Messages);
    search.query = "definitely-no-match".into();
    assert!(e
        .history_query(search)
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CHANGED:"));
    assert_eq!(
        body(&e, &sid, &first.snapshot_id, "lazy-message-0").await["id"],
        "lazy-message-0"
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    std::fs::write(file, good).unwrap();
}

#[tokio::test]
async fn lazy_turn_view_keeps_first_real_input_and_exact_canonical_json() {
    let (dir, state, calls) = many().await;
    let original = state.sessions[0].clone();
    let sid = original.id.clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let mut request = query(&sid, HistoryKind::Turns);
    request.query = "EXACT_MATCH_中文🙂".into();
    let page = e.history_query(request).await.unwrap();
    assert_eq!(page.total, 1);
    let actual = body(&e, &sid, &page.snapshot_id, &page.items[0].id).await;
    let expected = serde_json::to_value(view_at(
        &original,
        &original.turns[0],
        page.captured_at.parse().unwrap(),
    ))
    .unwrap();
    assert_eq!(actual, expected);
    assert_eq!(store.lock().await.metrics().segment_reads, 7);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn lazy_frozen_original_survives_new_commit_and_cache_eviction_without_mixing_replacement() {
    let (dir, state, calls) = many().await;
    let original = state.sessions[0].clone();
    let sid = original.id.clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let first = e
        .history_query(query(&sid, HistoryKind::Messages))
        .await
        .unwrap();
    let lease = e.ensure_archive_session(&sid).await.unwrap();
    e.mutate(
        Change {
            session_id: Some(sid.clone()),
            ..Default::default()
        },
        |state| {
            state
                .sessions
                .iter_mut()
                .find(|s| s.id == sid)
                .unwrap()
                .messages[0]
                .content = "新提交的合成正文".into();
            Ok(())
        },
    )
    .await
    .unwrap();
    e.history.lock().await.clear_serialized_cache();
    assert_eq!(
        body(&e, &sid, &first.snapshot_id, &first.items[0].id).await,
        serde_json::to_value(&original.messages[0]).unwrap()
    );
    let fresh = e
        .history_query(query(&sid, HistoryKind::Messages))
        .await
        .unwrap();
    assert_ne!(fresh.items[0].json_sha256, first.items[0].json_sha256);
    assert_eq!(
        body(&e, &sid, &fresh.snapshot_id, &fresh.items[0].id).await["content"],
        "新提交的合成正文"
    );
    drop(lease);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn lazy_waiting_for_archive_does_not_hold_state_and_source_change_during_read_is_rejected() {
    let (dir, state, calls) = many().await;
    let sid = state.sessions[0].id.clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let held = store.lock().await;
    let pending = tokio::spawn({
        let e = e.clone();
        let sid = sid.clone();
        async move { e.history_query(query(&sid, HistoryKind::Messages)).await }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(250), e.sessions())
            .await
            .unwrap()
            .len(),
        3
    );
    e.state.lock().await.sessions[0].source_key = "changed-during-synthetic-read".into();
    drop(held);
    assert!(pending
        .await
        .unwrap()
        .unwrap_err()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert!(e
        .history_query(query(&sid, HistoryKind::Messages))
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_INVALID:"));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
#[ignore = "explicit fresh synthetic history profile in F lab only"]
async fn seed_native_lazy_history_profile() {
    let home = PathBuf::from(
        std::env::var_os("HOMEBASE_HISTORY_SEED_HOME").expect("fresh synthetic home"),
    );
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("lazy-native-"));
    crate::assistant_state_store::safe(&home).unwrap();
    let (_dir, mut state, calls) = many().await;
    state.settings.enabled = false;
    let template = state.sessions[1].clone();
    for n in 0..125 {
        let mut session = template.clone();
        session.id = format!("lazy-native-session-{n}");
        session.title = format!("隔离会话索引与分页保全 {n}");
        session.source = "wechat".into();
        session.source_key = format!("lazy-native-owner-{n}");
        session.turns[0].id = format!("lazy-native-turn-{n}");
        session.turns[0].request_id = format!("lazy-native-request-{n}");
        for (i, m) in session.messages.iter_mut().enumerate() {
            m.id = format!("lazy-native-message-{n}-{i}");
            m.turn_id = Some(format!("lazy-native-turn-{n}"));
        }
        state.sessions.push(session);
    }
    let mut critical = template;
    critical.id = "lazy-native-critical-session".into();
    critical.source_key = "lazy-native-critical-owner".into();
    critical.turns[0].id = "lazy-native-critical-turn".into();
    critical.turns[0].request_id = "lazy-native-critical-request".into();
    critical.turns[0].state = "interrupted_unknown".into();
    critical.turns[0].runtime_uncertain_model = true;
    critical.turns[0].interaction = "task".into();
    for (i, m) in critical.messages.iter_mut().enumerate() {
        m.id = format!("lazy-native-critical-message-{i}");
        m.turn_id = Some("lazy-native-critical-turn".into());
    }
    state.sessions.push(critical);
    let samples = vec![
        json!({"id":state.sessions[0].id,"messages":state.sessions[0].messages,"turns":state.sessions[0].turns.iter().map(|t|json!({"id":t.id,"state":t.state})).collect::<Vec<_>>()}),
    ];
    let root = home.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let legacy = crate::wechat_crypto::crypt(&serde_json::to_vec(&state).unwrap(), true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &legacy)
        .await
        .unwrap();
    enable_capacity(&mut state);
    let mut writer = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
    writer.enable_capacity_segments();
    writer.load_index().unwrap();
    let store = Arc::new(Mutex::new(writer));
    let projected = commit(store, state, vec![]).await.unwrap();
    assert_eq!(projected.sessions.len(), 129);
    assert!(projected.sessions[0].cold.is_some());
    let expected = json!({"synthetic_only":true,"real_model_calls":0,"samples":samples,"critical_turn":"lazy-native-critical-turn","critical_state":"interrupted_unknown","synthetic_model_calls_for_seed":calls.load(Ordering::SeqCst),"history_count":180});
    crate::control_protocol::publish_private(
        &home.join("route-receipt-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}
