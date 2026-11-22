use super::*;
use crate::assistant::session_catalog::SessionQuery;
fn query() -> SessionQuery {
    SessionQuery {
        snapshot_id: None,
        query: String::new(),
        source: None,
        offset: 0,
        limit: 40,
    }
}

#[tokio::test]
async fn catalog_discovery_remains_cold_preserves_disk_and_never_reads_damaged_bodies() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    for file in std::fs::read_dir(dir.path().join("history-v1/segments")).unwrap() {
        std::fs::write(file.unwrap().path(), b"synthetic damage").unwrap();
    }
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let page = e.sessions_query(query()).await.unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.items.len(), 3);
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    assert!(e
        .session(&original.id)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_CHANGED:"));
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
async fn catalog_snapshot_is_fixed_across_configuration_save_and_has_no_body_or_authority_fields() {
    let (dir, full, calls) = filled().await;
    let store = stored(dir.path(), full).await;
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    let first = e.sessions_query(query()).await.unwrap();
    assert_eq!(
        e.sessions_query(query()).await.unwrap().snapshot_id,
        first.snapshot_id
    );
    let mut settings = e.settings().await;
    settings.max_tokens = 3218;
    e.save_settings(settings).await.unwrap();
    let refreshed = e.sessions_query(query()).await.unwrap();
    assert_ne!(first.snapshot_id, refreshed.snapshot_id);
    let old = e
        .sessions_query(SessionQuery {
            snapshot_id: Some(first.snapshot_id.clone()),
            ..query()
        })
        .await
        .unwrap();
    assert_eq!(old.captured_revision, first.captured_revision);
    for item in serde_json::to_value(old).unwrap()["items"]
        .as_array()
        .unwrap()
    {
        for forbidden in [
            "messages",
            "turns",
            "source_key",
            "jobs",
            "fingerprint",
            "pending",
            "settings",
        ] {
            assert!(item.get(forbidden).is_none(), "{forbidden}");
        }
    }
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.cold.is_some()));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
