use super::*;
use crate::assistant_history::{HistoryKind, HistoryQuery, HistoryRead, HistoryStore};
use std::time::Instant;
#[path = "assistant_archive_history_lazy_tests.rs"]
mod lazy_tests;

fn query(sid: &str, kind: HistoryKind) -> HistoryQuery {
    HistoryQuery {
        session_id: sid.into(),
        kind,
        query: String::new(),
        snapshot_id: None,
        through_turn_id: None,
        offset: 0,
        limit: 1,
    }
}
async fn stored_context(dir: &std::path::Path, state: State) -> Arc<Mutex<Store>> {
    let mut writer = Store::new(dir.join("history-v1"), dir.join("state.dpapi")).unwrap();
    writer.enable_context_segments();
    writer.load_index().unwrap();
    persist(&mut writer, state).await.unwrap();
    Arc::new(Mutex::new(
        Store::new(dir.join("history-v1"), dir.join("state.dpapi")).unwrap(),
    ))
}
fn segment(dir: &std::path::Path, sid: &str, kind: &str) -> PathBuf {
    let head = std::fs::read(dir.join("history-v1/head.dpapi")).unwrap();
    let value: Value =
        serde_json::from_slice(&crate::wechat_crypto::crypt(&head, false).unwrap()).unwrap();
    let series = value["series"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == sid && s["kind"] == kind)
        .unwrap();
    dir.join("history-v1/segments").join(format!(
        "{}.dpapi",
        series["segments"][0]["file"].as_str().unwrap()
    ))
}
async fn body(e: &Engine, sid: &str, snap: &str, item: &str) -> Value {
    let mut text = String::new();
    let mut offset = 0;
    loop {
        let chunk = e
            .history_read(HistoryRead {
                session_id: sid.into(),
                snapshot_id: snap.into(),
                item_id: item.into(),
                offset,
                limit: 8192,
            })
            .await
            .unwrap();
        text.push_str(&chunk.content);
        if let Some(next) = chunk.next_offset {
            offset = next;
        } else {
            assert!(chunk.complete);
            assert_eq!(text.len(), chunk.total_bytes);
            assert_eq!(
                format!("{:x}", Sha256::digest(text.as_bytes())),
                chunk.json_sha256
            );
            break;
        }
    }
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn cold_history_message_snapshot_does_not_require_turn_or_model_context_body() {
    let (dir, mut state, calls) = filled().await;
    let original = state.sessions[0].clone();
    state.sessions[0].history = vec![vec![
        json!({"role":"user","content":"UNREQUESTED_MODEL_CONTEXT"}),
    ]];
    let store = stored_context(dir.path(), state).await;
    for kind in ["context", "turns"] {
        let file = segment(dir.path(), &original.id, kind);
        std::fs::rename(&file, file.with_extension("retained-fixture")).unwrap();
    }
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let page = e
        .history_query(query(&original.id, HistoryKind::Messages))
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(
        body(&e, &original.id, &page.snapshot_id, &page.items[0].id).await,
        serde_json::to_value(&original.messages[0]).unwrap()
    );
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn history_turn_snapshot_preserves_original_goal_instead_of_empty_string() {
    let (_dir, state, _calls) = filled().await;
    let original = &state.sessions[0];
    let mut request = query(&original.id, HistoryKind::Turns);
    let now = Instant::now();
    let mut store = HistoryStore::default();
    request.snapshot_id = Some(
        store
            .insert(
                history_ops::history_source(original, &request).unwrap(),
                now,
            )
            .unwrap(),
    );
    let page = store
        .query(&request, &original.source_key, None, now)
        .unwrap();
    let chunk = store
        .read(
            &HistoryRead {
                session_id: original.id.clone(),
                snapshot_id: page.snapshot_id,
                item_id: page.items[0].id.clone(),
                offset: 0,
                limit: 8192,
            },
            &original.source_key,
            None,
            now,
        )
        .unwrap();
    assert!(chunk.complete);
    let value: Value = serde_json::from_str(&chunk.content).unwrap();
    assert_eq!(
        value["progress"]["goal"],
        progress::snapshot(original, &original.turns[0]).goal
    );
}

#[tokio::test]
async fn cold_history_existing_snapshot_survives_unavailable_source_and_restart_is_explicit() {
    let (dir, state, calls) = filled().await;
    let sid = state.sessions[0].id.clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let mut request = query(&sid, HistoryKind::Messages);
    let first = e.history_query(request.clone()).await.unwrap();
    let reads = store.lock().await.metrics();
    let head = dir.path().join("history-v1/head.dpapi");
    let held = head.with_extension("retained-fixture");
    std::fs::rename(&head, &held).unwrap();
    request.snapshot_id = Some(first.snapshot_id.clone());
    request.offset = 1;
    let second = e.history_query(request.clone()).await.unwrap();
    assert!(second.complete);
    assert_eq!(
        body(&e, &sid, &first.snapshot_id, &first.items[0].id).await["id"],
        first.items[0].id
    );
    let after = store.lock().await.metrics();
    assert_eq!(after.segment_reads, reads.segment_reads);
    assert_eq!(after.index_reads, reads.index_reads);
    assert!(e
        .history_query(query(&sid, HistoryKind::Messages))
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INCOMPLETE:"));
    let mut changed = request.clone();
    changed.query = "different".into();
    assert!(e
        .history_query(changed)
        .await
        .err()
        .unwrap()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    std::fs::rename(held, &head).unwrap();
    drop(e);
    let fresh = Arc::new(Mutex::new(
        Store::new(
            dir.path().join("history-v1"),
            dir.path().join("state.dpapi"),
        )
        .unwrap(),
    ));
    let e = engine(fresh.clone(), dir.path().join("workspace"), calls.clone()).await;
    assert!(e
        .history_query(request)
        .await
        .err()
        .unwrap()
        .starts_with("HISTORY_SNAPSHOT_EXPIRED:"));
    assert_eq!(fresh.lock().await.metrics().segment_reads, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cold_history_cutoff_and_unassigned_records_use_original_order_without_turn_body_reads() {
    let (dir, mut state, calls) = filled().await;
    let later = state.sessions.remove(1);
    let first = state.sessions[0].turns[0].id.clone();
    for turn in later.turns.iter() {
        state.sessions[0].turns.push(turn.clone());
    }
    for message in later.messages.iter() {
        state.sessions[0].messages.push(message.clone());
    }
    let mut orphan = state.sessions[0].messages[0].clone();
    orphan.id = "cold-history-unassigned".into();
    orphan.turn_id = None;
    state.sessions[0].messages.push(orphan);
    let sid = state.sessions[0].id.clone();
    let original = state.sessions[0].messages.clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let mut request = query(&sid, HistoryKind::Messages);
    request.through_turn_id = Some("foreign-cutoff".into());
    assert!(e
        .history_query(request.clone())
        .await
        .err()
        .unwrap()
        .starts_with("HISTORY_TURN_NOT_FOUND:"));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    request.through_turn_id = Some(first);
    let page = e.history_query(request).await.unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.omitted_unassigned_messages, 1);
    assert_eq!(
        body(&e, &sid, &page.snapshot_id, &page.items[0].id).await,
        serde_json::to_value(&original[0]).unwrap()
    );
    let mut request = query(&sid, HistoryKind::Messages);
    request.query = "not-present-in-any-synthetic-message".into();
    let page = e.history_query(request).await.unwrap();
    assert_eq!(page.total, 0);
    assert!(page.complete && page.items.is_empty());
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn cold_history_duplicate_identity_and_changed_source_reject_before_loading_body() {
    let (dir, state, calls) = filled().await;
    let sid = state.sessions[0].id.clone();
    let store = stored_context(dir.path(), state).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls).await;
    let page = e
        .history_query(query(&sid, HistoryKind::Messages))
        .await
        .unwrap();
    let index_reads = store.lock().await.metrics().index_reads;
    e.state.lock().await.sessions[0].source_key = "changed-synthetic-owner".into();
    let mut request = query(&sid, HistoryKind::Messages);
    request.snapshot_id = Some(page.snapshot_id.clone());
    assert!(e
        .history_query(request.clone())
        .await
        .err()
        .unwrap()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert!(e
        .history_read(HistoryRead {
            session_id: sid.clone(),
            snapshot_id: page.snapshot_id.clone(),
            item_id: page.items[0].id.clone(),
            offset: 0,
            limit: 8192
        })
        .await
        .err()
        .unwrap()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    {
        let mut state = e.state.lock().await;
        let duplicate = state.sessions[0].clone();
        state.sessions.push(duplicate);
    }
    assert!(e
        .history_query(request)
        .await
        .err()
        .unwrap()
        .starts_with("HISTORY_INVALID_DATA:"));
    assert!(e
        .history_query(query(&sid, HistoryKind::Messages))
        .await
        .err()
        .unwrap()
        .starts_with("HISTORY_INVALID_DATA:"));
    assert_eq!(store.lock().await.metrics().index_reads, index_reads);
}

#[tokio::test]
async fn cold_history_task_body_keeps_jobs_and_goal_without_context_and_wechat_does_not_replay() {
    let (dir, state, effects, models) = completed_task(true, true).await;
    let original = state.sessions[0].clone();
    let sid = original.id.clone();
    let store = stored_context(dir.path(), state).await;
    let context = segment(dir.path(), &sid, "context");
    let held = context.with_extension("retained-fixture");
    std::fs::rename(&context, &held).unwrap();
    let e = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    let page = e
        .history_query(query(&sid, HistoryKind::Turns))
        .await
        .unwrap();
    let view = body(&e, &sid, &page.snapshot_id, &page.items[0].id).await;
    assert_eq!(
        view["progress"]["goal"],
        progress::snapshot(&original, &original.turns[0]).goal
    );
    assert_eq!(view["jobs"][0]["id"], original.jobs[0].id);
    assert_eq!(view["jobs"][0]["owned"], true);
    assert_eq!(view["jobs"][0]["result"], original.jobs[0].result);
    assert_eq!(store.lock().await.metrics().segment_reads, 3);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    let before = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let mut settings = e.settings().await;
    settings.max_tokens = 3000;
    e.save_settings(settings).await.unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, 3);
    assert_ne!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        before
    );
    std::fs::rename(held, context).unwrap();
    let response=e.channel(json!({"operation":"get","source_key":"synthetic-task-owner","turn_id":original.turns[0].id})).await.unwrap();
    assert_eq!(response["status"], "succeeded");
    let repeated=e.channel(json!({"operation":"submit","source_key":"synthetic-task-owner","text":"请查询合成系统信息","request_id":"cold-task-original"})).await.unwrap();
    assert_eq!(repeated["turn_id"], original.turns[0].id);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cold_history_damaged_requested_segment_is_error_and_keeps_original_state() {
    let (dir, state, calls) = filled().await;
    let sid = state.sessions[0].id.clone();
    let store = stored_context(dir.path(), state).await;
    let file = segment(dir.path(), &sid, "messages");
    let original = std::fs::read(&file).unwrap();
    let mut changed = original.clone();
    changed[5] ^= 1;
    std::fs::write(&file, changed).unwrap();
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    assert!(e
        .history_query(query(&sid, HistoryKind::Messages))
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_CHANGED:"));
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    std::fs::write(file, original).unwrap();
    assert_eq!(
        e.history_query(query(&sid, HistoryKind::Messages))
            .await
            .unwrap()
            .total,
        2
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
