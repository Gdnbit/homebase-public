use super::*;

#[tokio::test]
async fn startup_turn_scan_uses_existing_cache_without_retaining_new_bodies() {
    let (_dir, mut writer) = fixture();
    writer.load_index().unwrap();
    let mut original = state(1);
    let turns: Vec<_> = (0..35)
        .map(|index| json!({"id": format!("turn-{index}"), "payload": "synthetic".repeat(12_000)}))
        .collect();
    original["sessions"][0]["turns"] = json!(turns);
    writer.save(original).await.unwrap();
    let head = fs::read(writer.head()).unwrap();
    let mut reader = Store::new(writer.root.clone(), writer.legacy.clone()).unwrap();
    let snapshot = reader.load_index().unwrap().unwrap();
    let expected_first = &turns[0];
    assert_eq!(
        &reader.read_record(&snapshot, "session-A", Kind::Turns, "turn-0").unwrap(),
        expected_first
    );
    assert_eq!(reader.metrics().segment_reads, 1);

    let mut index = 0;
    reader.visit_turns(&snapshot, "session-A", |row| {
        assert_eq!(row, turns[index]);
        index += 1;
        Ok(())
    }).unwrap();
    assert_eq!(index, turns.len());
    assert_eq!(reader.metrics().segment_reads, 2);
    assert_eq!(reader.cache.lock().unwrap().len(), 1);

    let expected_stop = "synthetic scan stopped";
    assert_eq!(
        reader.visit_turns(&snapshot, "session-A", |_| Err(expected_stop.into())).unwrap_err(),
        expected_stop
    );
    let view = reader.record_view(&snapshot, "session-A").unwrap();
    assert_eq!(view.record(Kind::Turns, "turn-34").unwrap(), turns[34]);
    assert_eq!(view.record(Kind::Turns, "turn-33").unwrap(), turns[33]);
    assert_eq!(reader.metrics().segment_reads, 3);
    assert_eq!(fs::read(writer.head()).unwrap(), head);
}

#[tokio::test]
async fn readonly_views_share_cache_only_while_live_and_keep_original_scope_after_new_head() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let original = state(70);
    let first = store.save(original.clone()).await.unwrap().snapshot;
    let view = store.record_view(&first, "session-A").unwrap();
    assert_eq!(view.count(Kind::Messages).unwrap(), 70);
    assert_eq!(view.segment_reads(), 0);
    assert!(view.charge_bytes() < 5 * 1024 * 1024);
    assert_eq!(
        view.record(Kind::Messages, "message-33").unwrap(),
        original["sessions"][0]["messages"][33]
    );
    let same = store.record_view(&first, "session-A").unwrap();
    assert!(Arc::ptr_eq(&view, &same));
    same.record(Kind::Messages, "message-34").unwrap();
    assert_eq!(store.metrics().segment_reads, 1);
    assert_eq!(same.segment_reads(), 1);
    assert!(view
        .record(Kind::Messages, "turn-A")
        .unwrap_err()
        .starts_with("ARCHIVE_RECORD_MISSING:"));
    assert!(store
        .record_view(&first, "not-this-session")
        .err()
        .unwrap()
        .starts_with("ARCHIVE_SCOPE:"));
    let mut changed = original.clone();
    changed["sessions"][0]["messages"][33]["content"] = json!("新合成正文");
    let next = store.save(changed).await.unwrap().snapshot;
    let latest = store.record_view(&next, "session-A").unwrap();
    assert!(!Arc::ptr_eq(&latest, &view));
    assert_eq!(
        view.record(Kind::Messages, "message-33").unwrap(),
        original["sessions"][0]["messages"][33]
    );
    let weak = Arc::downgrade(&view);
    drop(view);
    drop(same);
    assert!(weak.upgrade().is_none());
    assert_eq!(store.record_views.len(), 2);
    let again = store.record_view(&next, "session-A").unwrap();
    assert!(Arc::ptr_eq(&again, &latest));
    assert_eq!(store.record_views.len(), 1);
}

#[tokio::test]
async fn readonly_view_large_segments_share_bounded_cache_without_repeated_reads_or_publication() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let mut original = state(2);
    original["sessions"][0]["messages"][0]["content"] = json!("x".repeat(5 * 1024 * 1024));
    let snapshot = store.save(original.clone()).await.unwrap().snapshot;
    let head = fs::read(store.head()).unwrap();
    let view = store.record_view(&snapshot, "session-A").unwrap();
    assert_eq!(
        view.record(Kind::Messages, "message-0").unwrap(),
        original["sessions"][0]["messages"][0]
    );
    assert!(view.charge_bytes() < 5 * 1024 * 1024);
    view.record(Kind::Messages, "message-1").unwrap();
    assert_eq!(view.segment_reads(), 1); // A second row in the same verified segment must reuse its bytes.
    store
        .read_record(&snapshot, "session-A", Kind::Messages, "message-0")
        .unwrap();
    assert_eq!(store.metrics().segment_reads, 1);
    let cache = store.cache.lock().unwrap();
    assert!(cache.len() <= 8);
    assert!(cache.iter().map(|c| c.reference.plain_bytes).sum::<usize>() <= CIPHER_LIMIT);
    assert_eq!(fs::read(store.head()).unwrap(), head);
    assert_eq!(
        store.current.as_ref().unwrap().generation,
        snapshot.generation()
    );
}
