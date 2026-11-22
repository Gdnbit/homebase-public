use super::*;
use crate::assistant::window::TimelineRequest;

async fn timeline(large: bool) -> (tempfile::TempDir, State, Arc<AtomicUsize>) {
    let (dir, mut state, calls) = filled().await;
    let s = &mut state.sessions[0];
    s.messages = (0..180)
        .map(|n| Message {
            id: format!("cold-window-{n}"),
            role: if n % 2 == 0 { "user" } else { "assistant" }.into(),
            content: format!("{n} 中文🙂引号\"\\\n").repeat(if large { 1000 } else { 2 }),
            created_at: now(),
            turn_id: Some(s.turns[0].id.clone()),
            attachments: vec![],
        })
        .collect();
    s.history = vec![vec![
        json!({"role":"user","content":"UNREQUESTED_CONTEXT_🙂".repeat(30000)}),
    ]];
    (dir, state, calls)
}
async fn archive(dir: &std::path::Path, state: State, context: bool) -> Arc<Mutex<Store>> {
    let mut store = Store::new(dir.join("history-v1"), dir.join("state.dpapi")).unwrap();
    store.load_index().unwrap();
    if context {
        store.enable_context_segments();
    }
    persist(&mut store, state).await.unwrap();
    Arc::new(Mutex::new(
        Store::new(dir.join("history-v1"), dir.join("state.dpapi")).unwrap(),
    ))
}
fn query(id: &str, before: Option<String>) -> TimelineRequest {
    TimelineRequest {
        session_id: id.into(),
        before_message_id: before,
        limit: 40,
    }
}
fn head(dir: &std::path::Path) -> Vec<u8> {
    std::fs::read(dir.join("history-v1/head.dpapi")).unwrap()
}
fn segment(dir: &std::path::Path, sid: &str, kind: &str, last: bool) -> PathBuf {
    let value: Value =
        serde_json::from_slice(&crate::wechat_crypto::crypt(&head(dir), false).unwrap()).unwrap();
    let list = value["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == sid && s["kind"] == kind)
        .unwrap()["segments"]
        .as_array()
        .unwrap();
    let row = if last { list.last().unwrap() } else { &list[0] };
    dir.join("history-v1/segments")
        .join(format!("{}.dpapi", row["file"].as_str().unwrap()))
}

#[tokio::test]
async fn cold_window_reads_only_overlapping_segments_stays_cold_and_matches_every_hot_page() {
    let (dir, state, calls) = timeline(false).await;
    let original = state.sessions[0].clone();
    let revision = state.revision;
    let store = archive(dir.path(), state, true).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let before = head(dir.path());
    let first = e.session_window(query(&original.id, None)).await.unwrap();
    assert_eq!(first.page.start_index, 140);
    assert_eq!(first.page.total_messages, 180);
    // Two message segments, one turn and the earliest original input's segment.
    // The remaining messages and model context are not materialized.
    assert_eq!(store.lock().await.metrics().segment_reads, 4);
    e.session_window(query(&original.id, None)).await.unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, 4);
    let mut cursor = None;
    let mut pages = Vec::new();
    loop {
        let request = query(&original.id, cursor.clone());
        let end = cursor.as_ref().map_or(180, |id| {
            original.messages.iter().position(|m| &m.id == id).unwrap()
        });
        let expected = window::project(&original, &request, revision, 180, end, 0, None).unwrap();
        let actual = e.session_window(request).await.unwrap();
        assert_eq!(
            serde_json::to_value(&actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        cursor = actual.page.before_message_id;
        pages.push(actual.messages);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(store.lock().await.metrics().segment_reads, 7);
    assert_eq!(
        pages
            .into_iter()
            .rev()
            .flatten()
            .map(|m| m.id)
            .collect::<Vec<_>>(),
        original
            .messages
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>()
    );
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.cold.is_some() && s.messages.is_empty() && s.turns.is_empty()));
    assert_eq!(head(dir.path()), before);
    let mut settings = e.settings().await;
    settings.max_tokens = 3000;
    e.save_settings(settings).await.unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, 7);
    drop(e);
    let restarted = engine(store, dir.path().join("workspace"), calls.clone()).await;
    assert_eq!(
        serde_json::to_value(restarted.session(&original.id).await.unwrap().messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
    assert_eq!(
        restarted.read_archive_session(&original.id).await.unwrap().history,
        original.history
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cold_window_budget_empty_boundary_and_legacy_context_match_hot_projection() {
    let (dir, state, calls) = timeline(true).await;
    let original = state.sessions[0].clone();
    let revision = state.revision;
    let store = archive(dir.path(), state, false).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls).await;
    let request = query(&original.id, None);
    let actual = e.session_window(request).await.unwrap();
    assert!(actual.page.returned < 40 && actual.page.returned > 0);
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(
            window::project(
                &original,
                &query(&original.id, None),
                revision,
                180,
                180,
                0,
                None
            )
            .unwrap()
        )
        .unwrap()
    );
    assert!(serde_json::to_vec(&actual).unwrap().len() <= 256 * 1024);
    let reads = store.lock().await.metrics().segment_reads;
    let empty = e
        .session_window(query(&original.id, Some(original.messages[0].id.clone())))
        .await
        .unwrap();
    assert_eq!(empty.page.end_index, 0);
    assert_eq!(empty.page.returned, 0);
    assert!(!empty.page.has_more);
    assert!(empty.messages.is_empty() && empty.turns.is_empty());
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
}

#[tokio::test]
async fn cold_window_unknown_cursor_and_unrequested_damage_do_not_load_unrelated_bodies() {
    let (dir, state, calls) = timeline(false).await;
    let sid = state.sessions[0].id.clone();
    let store = archive(dir.path(), state, true).await;
    let context = segment(dir.path(), &sid, "context", false);
    std::fs::rename(&context, context.with_extension("retained-fixture")).unwrap();
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let before = head(dir.path());
    assert!(e
        .session_window(query(&sid, Some("missing-original-cursor".into())))
        .await
        .err()
        .unwrap()
        .starts_with("WINDOW_CURSOR_NOT_FOUND:"));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert_eq!(
        e.session_window(query(&sid, None))
            .await
            .unwrap()
            .messages
            .len(),
        40
    );
    assert!(e
        .session(&sid)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_MISSING:"));
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(head(dir.path()), before);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cold_window_checks_selected_receipt_projection_and_does_not_replace_state_on_error() {
    let (dir, state, calls) = timeline(false).await;
    let sid = state.sessions[0].id.clone();
    let store = archive(dir.path(), state, true).await;
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    let before = head(dir.path());
    Arc::make_mut(e.state.lock().await.sessions[0].cold.as_mut().unwrap())
        .summary
        .receipts[0]
        .1 = "conflicting-receipt".into();
    assert!(e
        .session_window(query(&sid, None))
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INVALID:"));
    let state = e.state.lock().await;
    assert!(state.sessions[0].cold.is_some() && state.sessions[0].messages.is_empty());
    assert_eq!(head(dir.path()), before);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
#[ignore = "explicit synthetic native cold-window corpus only"]
async fn seed_native_cold_window() {
    let home =
        PathBuf::from(std::env::var_os("HOMEBASE_WINDOW_SEED_HOME").expect("synthetic home"));
    assert!(
        home.is_absolute()
            && home
                .to_string_lossy()
                .replace('\\', "/")
                .starts_with("F:/HomeBase-Labs/")
    );
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("cold-window-native-seed-"));
    let root = home.join(".homebase/assistant");
    assert!(!root.exists(), "Never overwrite an existing corpus");
    let (_dir, mut state, calls) = timeline(false).await;
    state.settings.enabled = false;
    state.settings.wechat_enabled = false;
    let original = state.sessions[0].clone();
    std::fs::create_dir_all(&root).unwrap();
    let cipher = crate::wechat_crypto::crypt(&serde_json::to_vec(&state).unwrap(), true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &cipher)
        .await
        .unwrap();
    archive(&root, state, true).await;
    let context = segment(&root, &original.id, "context", false);
    let expected = json!({"synthetic_only":true,"model_disabled":true,"synthetic_model_calls":calls.load(Ordering::SeqCst),"session_id":original.id,"messages":original.messages,"goal":progress::snapshot(&original,&original.turns[0]).goal,"context_file":context,"request":{"session_id":null,"text":"仅合成的闲聊","request_id":"cold-original-0","mode":"chat","attachments":[]},"turn_id":original.turns[0].id});
    crate::control_protocol::publish_private(
        &home.join("window-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn cold_window_damaged_target_is_explicit_and_restoring_fixture_does_not_reexecute() {
    let (dir, state, calls) = timeline(false).await;
    let sid = state.sessions[0].id.clone();
    let store = archive(dir.path(), state, true).await;
    let file = segment(dir.path(), &sid, "messages", true);
    let original = std::fs::read(&file).unwrap();
    let mut damaged = original.clone();
    damaged[5] ^= 1;
    std::fs::write(&file, damaged).unwrap();
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    let before = head(dir.path());
    assert!(e
        .session_window(query(&sid, None))
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_CHANGED:"));
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(head(dir.path()), before);
    std::fs::write(file, original).unwrap();
    assert_eq!(
        e.session_window(query(&sid, None))
            .await
            .unwrap()
            .messages
            .len(),
        40
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn window_duplicate_session_retains_original_error_without_reading_cold_bodies() {
    let (dir, state, calls) = timeline(false).await;
    let store = archive(dir.path(), state, true).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let sid = {
        let mut state = e.state.lock().await;
        let duplicate = state.sessions[0].clone();
        let sid = duplicate.id.clone();
        state.sessions.push(duplicate);
        sid
    };
    assert!(e
        .session_window(query(&sid, None))
        .await
        .err()
        .unwrap()
        .starts_with("WINDOW_IDENTITY_CONFLICT:"));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cold_task_window_keeps_full_original_job_path_and_shared_wechat_receipt() {
    let (dir, state, effects, models) = completed_task(true, false).await;
    let original = state.sessions[0].clone();
    let revision = state.revision;
    let store = stored_task(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    let actual = e.session_window(query(&original.id, None)).await.unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(
            window::project(
                &original,
                &query(&original.id, None),
                revision,
                original.messages.len(),
                original.messages.len(),
                0,
                None
            )
            .unwrap()
        )
        .unwrap()
    );
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(store.lock().await.metrics().segment_reads, 4);
    let response=e.channel(json!({"operation":"get","source_key":"synthetic-task-owner","turn_id":original.turns[0].id})).await.unwrap();
    assert_eq!(response["status"], "succeeded");
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}
