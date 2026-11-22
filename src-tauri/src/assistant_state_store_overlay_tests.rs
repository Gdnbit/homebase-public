use super::*;

async fn seed(mut value: Value) -> (tempfile::TempDir, Store, Snapshot, Value) {
    let (dir, mut store) = fixture();
    store.enable_capacity_segments();
    store.load_index().unwrap();
    value["archive_capacity_version"] = json!(1);
    let snapshot = store.save(value.clone()).await.unwrap().snapshot;
    (dir, store, snapshot, value)
}

fn retained(snapshot: &Snapshot, session: &str, kind: Kind) -> SeriesOverlay {
    SeriesOverlay {
        session_id: session.into(),
        kind,
        snapshot: snapshot.clone(),
        records: snapshot
            .record_ids(session, kind)
            .unwrap()
            .into_iter()
            .enumerate()
            .map(|(index, id)| RecordSlot::Retained { index, id })
            .collect(),
    }
}

fn projection(value: &Value, overlays: &[SeriesOverlay]) -> Value {
    let mut value = value.clone();
    for overlay in overlays {
        let session = value["sessions"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|s| s["id"].as_str() == Some(overlay.session_id.as_str()))
            .unwrap();
        session[overlay.kind.key()] = json!([]);
    }
    value
}

fn fresh(store: &Store) -> Store {
    let mut result = Store::new(store.root.clone(), store.legacy.clone()).unwrap();
    result.load_index().unwrap();
    result
}

#[tokio::test]
async fn sparse_all_kinds_preserve_original_payloads_and_ids_and_read_only_touched_segments() {
    let mut original = state(325);
    original["sessions"][0]["turns"] = json!((0..130)
        .map(|n| json!({
            "id":format!("turn-{n}"),"request_id":format!("request-{n}"),
            "checkpoint":{"opaque":[true,null,"原始🙂"]},"state":"completed"
        }))
        .collect::<Vec<_>>());
    original["sessions"][0]["jobs"] = json!((0..165)
        .map(|n| json!({
            "id":format!("job-{n}"),"state":"unknown","owned":true,
            "step_id":format!("step-{n}"),"result":{"path":"合成产物","extra":[1,null]}
        }))
        .collect::<Vec<_>>());
    let (_dir, store, snapshot, mut expected) = seed(original).await;
    let mut reader = fresh(&store);
    let before: BTreeMap<_, _> = snapshot
        .index
        .series
        .iter()
        .flat_map(|s| &s.segments)
        .map(|r| {
            (
                r.file.clone(),
                fs::read(store.segment(&r.file).unwrap()).unwrap(),
            )
        })
        .collect();
    let mut overlays = vec![];
    for kind in KINDS {
        let mut overlay = retained(&snapshot, "session-A", kind);
        let index = overlay.records.len() - 1;
        expected["sessions"][0][kind.key()][index]["overlay_note"] = json!("新的合成状态🙂");
        overlay.records[index] =
            RecordSlot::Owned(expected["sessions"][0][kind.key()][index].clone());
        let append =
            json!({"id":format!("{}-append", kind.key()),"extra":{"raw":true,"text":"追加"}});
        expected["sessions"][0][kind.key()]
            .as_array_mut()
            .unwrap()
            .push(append.clone());
        overlay.records.push(RecordSlot::Owned(append));
        overlays.push(overlay);
    }
    let input = projection(&expected, &overlays);
    let saved = reader
        .save_record_partition(input, &snapshot, &[], &overlays)
        .await
        .unwrap();
    assert_eq!(reader.metrics().segment_reads, 3);
    assert_eq!(saved.segments_written, 3);
    assert!(saved.segments_reused >= 19);
    for (id, bytes) in before {
        assert_eq!(fs::read(store.segment(&id).unwrap()).unwrap(), bytes);
    }
    assert_eq!(reader.materialize(&saved.snapshot).unwrap(), expected);
    let mut restarted = fresh(&store);
    let restored = restarted.load_index().unwrap().unwrap();
    assert_eq!(restarted.materialize(&restored).unwrap(), expected);
    assert_eq!(restored.count("session-A", Kind::Messages).unwrap(), 326);
    assert_eq!(restored.count("session-A", Kind::Jobs).unwrap(), 166);
}

#[tokio::test]
async fn all_retained_is_zero_read_noop_and_owned_equal_records_reuse_original_cipher() {
    let (_dir, mut store, snapshot, original) = seed(state(96)).await;
    let mut overlays = KINDS.map(|kind| retained(&snapshot, "session-A", kind));
    let head = fs::read(store.head()).unwrap();
    let first = store
        .save_record_partition(projection(&original, &overlays), &snapshot, &[], &overlays)
        .await
        .unwrap();
    assert!(!first.index_written);
    assert_eq!(first.segments_written, 0);
    assert_eq!(store.metrics().segment_reads, 0);
    overlays[0].records[42] = RecordSlot::Owned(original["sessions"][0]["messages"][42].clone());
    let second = store
        .save_record_partition(projection(&original, &overlays), &snapshot, &[], &overlays)
        .await
        .unwrap();
    assert!(!second.index_written);
    assert_eq!(second.segments_written, 0);
    assert_eq!(store.metrics().segment_reads, 1);
    assert_eq!(fs::read(store.head()).unwrap(), head);
    assert_eq!(second.generation, snapshot.generation());
}

#[tokio::test]
async fn appends_keep_current_segments_dense_and_full_boundary_does_not_read_old_body() {
    let (_dir, mut store, mut snapshot, mut original) = seed(state(64)).await;
    for n in 64..132 {
        let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
        let append = json!({"id":format!("message-{n}"),"content":"隔离追加🙂"});
        original["sessions"][0]["messages"]
            .as_array_mut()
            .unwrap()
            .push(append.clone());
        overlay.records.push(RecordSlot::Owned(append));
        let reads = store.metrics().segment_reads;
        let saved = store
            .save_record_partition(
                projection(&original, &[overlay.clone()]),
                &snapshot,
                &[],
                &[overlay],
            )
            .await
            .unwrap();
        assert_eq!(saved.segments_written, 1);
        if n % RECORDS == 0 {
            assert_eq!(store.metrics().segment_reads, reads);
        } else {
            assert!(store.metrics().segment_reads <= reads + 1);
        }
        let series = saved
            .snapshot
            .index
            .series
            .iter()
            .find(|s| s.kind == Kind::Messages)
            .unwrap();
        assert_eq!(series.segments.len(), (n + 1).div_ceil(RECORDS));
        assert_eq!(
            series.segments.iter().map(|s| s.count).sum::<usize>(),
            n + 1
        );
        snapshot = saved.snapshot;
    }
    assert_eq!(store.materialize(&snapshot).unwrap(), original);
}

#[tokio::test]
async fn untouched_damage_is_not_read_or_hidden_and_touched_damage_cannot_publish() {
    let (_dir, mut store, snapshot, original) = seed(state(70)).await;
    let first = snapshot
        .index
        .series
        .iter()
        .find(|s| s.kind == Kind::Messages)
        .unwrap()
        .segments[0]
        .clone();
    let tail = snapshot
        .index
        .series
        .iter()
        .find(|s| s.kind == Kind::Messages)
        .unwrap()
        .segments[2]
        .clone();
    fs::write(
        store.segment(&first.file).unwrap(),
        b"synthetic corrupt segment",
    )
    .unwrap();
    let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
    overlay
        .records
        .push(RecordSlot::Owned(json!({"id":"append","content":"合成"})));
    let saved = store
        .save_record_partition(
            projection(&original, &[overlay.clone()]),
            &snapshot,
            &[],
            &[overlay],
        )
        .await
        .unwrap();
    assert_eq!(store.metrics().segment_reads, 1);
    assert!(store
        .read_record(&saved.snapshot, "session-A", Kind::Messages, "message-0")
        .unwrap_err()
        .starts_with("ARCHIVE_CHANGED:"));
    assert_eq!(
        store
            .read_record(&saved.snapshot, "session-A", Kind::Messages, "message-69")
            .unwrap(),
        original["sessions"][0]["messages"][69]
    );
    let head = fs::read(store.head()).unwrap();
    let mut reader = fresh(&store);
    let mut overlay = retained(&saved.snapshot, "session-A", Kind::Messages);
    overlay.records[1] = RecordSlot::Owned(json!({"id":"message-1","content":"合成更新"}));
    let result = reader
        .save_record_partition(
            projection(&original, &[overlay.clone()]),
            &saved.snapshot,
            &[],
            &[overlay],
        )
        .await;
    assert!(result.unwrap_err().starts_with("ARCHIVE_CHANGED:"));
    assert_eq!(fs::read(store.head()).unwrap(), head);
    assert!(store.segment(&tail.file).unwrap().exists());
}

#[tokio::test]
async fn smaller_hot_updates_keep_internal_byte_limited_segment_boundaries() {
    let mut original = state(35);
    for item in original["sessions"][0]["messages"].as_array_mut().unwrap() {
        item["content"] = json!("x".repeat(800 * 1024));
    }
    let (_dir, mut store, snapshot, mut original) = seed(original).await;
    let old = snapshot
        .index
        .series
        .iter()
        .find(|s| s.kind == Kind::Messages)
        .unwrap();
    assert_eq!(
        old.segments.iter().map(|s| s.count).collect::<Vec<_>>(),
        [16, 19]
    );
    let untouched = old.segments[1].clone();
    let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
    for n in 0..2 {
        original["sessions"][0]["messages"][n]["content"] = json!("新的小值");
        overlay.records[n] = RecordSlot::Owned(original["sessions"][0]["messages"][n].clone());
    }
    let saved = store
        .save_record_partition(
            projection(&original, &[overlay.clone()]),
            &snapshot,
            &[],
            &[overlay],
        )
        .await
        .unwrap();
    assert_eq!(store.metrics().segment_reads, 1);
    assert_eq!(saved.segments_written, 1);
    let current = saved
        .snapshot
        .index
        .series
        .iter()
        .find(|s| s.kind == Kind::Messages)
        .unwrap();
    assert_eq!(current.segments[1], untouched);
    assert_eq!(store.materialize(&saved.snapshot).unwrap(), original);
}

#[tokio::test]
async fn invalid_record_identity_order_and_dual_authority_never_change_head() {
    let (_dir, mut store, snapshot, original) = seed(state(3)).await;
    let valid = retained(&snapshot, "session-A", Kind::Messages);
    let input = projection(&original, &[valid.clone()]);
    let head = fs::read(store.head()).unwrap();
    let mut cases = vec![];
    let mut short = valid.clone();
    short.records.pop();
    cases.push(short);
    let mut swapped = valid.clone();
    swapped.records.swap(0, 1);
    cases.push(swapped);
    let mut wrong_offset = valid.clone();
    wrong_offset.records[0] = RecordSlot::Retained {
        index: 1,
        id: "message-0".into(),
    };
    cases.push(wrong_offset);
    let mut wrong_old_id = valid.clone();
    wrong_old_id.records[0] = RecordSlot::Owned(json!({"id":"changed-id"}));
    cases.push(wrong_old_id);
    let mut duplicate = valid.clone();
    duplicate
        .records
        .push(RecordSlot::Owned(json!({"id":"message-0"})));
    cases.push(duplicate);
    let mut missing = valid.clone();
    missing
        .records
        .push(RecordSlot::Owned(json!({"content":"missing-id"})));
    cases.push(missing);
    let mut invalid = valid.clone();
    invalid
        .records
        .push(RecordSlot::Owned(json!({"id":"bad\nID"})));
    cases.push(invalid);
    let mut foreign_retained = valid.clone();
    foreign_retained.records.push(RecordSlot::Retained {
        index: 3,
        id: "not-in-original".into(),
    });
    cases.push(foreign_retained);
    let mut unknown = valid.clone();
    unknown.session_id = "foreign-session".into();
    cases.push(unknown);
    let mut context = valid.clone();
    context.kind = Kind::Context;
    cases.push(context);
    let (_foreign_dir, _foreign, foreign_snapshot, _) = seed(state(3)).await;
    let mut wrong_store = valid.clone();
    wrong_store.snapshot = foreign_snapshot;
    cases.push(wrong_store);
    for invalid in cases {
        let error = store
            .save_record_partition(input.clone(), &snapshot, &[], &[invalid])
            .await
            .unwrap_err();
        assert!(
            error.starts_with("ARCHIVE_SCOPE:") || error.starts_with("ARCHIVE_IDENTITY:"),
            "{error}"
        );
        assert_eq!(fs::read(store.head()).unwrap(), head);
    }
    for (value, cold, overlays) in [
        (original.clone(), vec![], vec![valid.clone()]),
        (input.clone(), vec![], vec![valid.clone(), valid.clone()]),
        (
            snapshot.metadata(),
            vec![("session-A".into(), snapshot.clone())],
            vec![valid],
        ),
    ] {
        assert!(store
            .save_record_partition(value, &snapshot, &cold, &overlays)
            .await
            .unwrap_err()
            .starts_with("ARCHIVE_SCOPE:"));
        assert_eq!(fs::read(store.head()).unwrap(), head);
    }
}

#[tokio::test]
async fn omitted_payloads_sessions_and_changed_source_are_rejected_but_explicit_cold_is_preserved()
{
    let mut original = state(3);
    let mut other = original["sessions"][0].clone();
    other["id"] = json!("session-B");
    original["sessions"].as_array_mut().unwrap().push(other);
    let (_dir, mut store, snapshot, original) = seed(original).await;
    let overlay = retained(&snapshot, "session-A", Kind::Messages);
    let input = projection(&original, &[overlay.clone()]);
    let head = fs::read(store.head()).unwrap();
    let mut cases = vec![];
    let mut missing_kind = input.clone();
    missing_kind["sessions"][0]["jobs"] = json!([]);
    cases.push(missing_kind);
    let mut missing_other = input.clone();
    missing_other["sessions"][1]["messages"] = json!([]);
    cases.push(missing_other);
    let mut missing_session = input.clone();
    missing_session["sessions"].as_array_mut().unwrap().pop();
    cases.push(missing_session);
    let mut reordered = input.clone();
    reordered["sessions"].as_array_mut().unwrap().swap(0, 1);
    cases.push(reordered);
    let mut source = input.clone();
    source["sessions"][0]["source_key"] = json!("other-owner");
    cases.push(source);
    let mut reorder_kind = input.clone();
    reorder_kind["sessions"][1]["messages"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    cases.push(reorder_kind);
    for invalid in cases {
        assert!(store
            .save_record_partition(invalid, &snapshot, &[], &[overlay.clone()])
            .await
            .unwrap_err()
            .starts_with("ARCHIVE_SCOPE:"));
        assert_eq!(fs::read(store.head()).unwrap(), head);
    }
    let mut cold = input;
    for kind in CONTEXT_KINDS {
        cold["sessions"][1][kind.key()] = json!([]);
    }
    cold["revision"] = json!(2);
    let saved = store
        .save_record_partition(
            cold,
            &snapshot,
            &[("session-B".into(), snapshot.clone())],
            &[overlay],
        )
        .await
        .unwrap();
    assert_eq!(saved.segments_written, 0);
    assert_eq!(store.metrics().segment_reads, 0);
    let mut expected = original;
    expected["revision"] = json!(2);
    assert_eq!(store.materialize(&saved.snapshot).unwrap(), expected);
}

#[tokio::test]
async fn metadata_only_new_generation_requires_fresh_global_base_but_unchanged_series_may_retain_old_snapshot(
) {
    let (_dir, mut store, old, original) = seed(state(3)).await;
    let overlay = retained(&old, "session-A", Kind::Messages);
    let mut changed = original.clone();
    changed["settings"]["new"] = json!("合成新设置");
    let saved = store.save(changed.clone()).await.unwrap();
    let head = fs::read(store.head()).unwrap();
    assert!(store
        .save_record_partition(
            projection(&original, &[overlay.clone()]),
            &old,
            &[],
            &[overlay.clone()]
        )
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT:"));
    assert_eq!(fs::read(store.head()).unwrap(), head);
    // Old per-series references are safe only with an independently fresh whole-state base.
    let noop = store
        .save_record_partition(
            projection(&changed, &[overlay.clone()]),
            &saved.snapshot,
            &[],
            &[overlay.clone()],
        )
        .await
        .unwrap();
    assert!(!noop.index_written);
    let mut same_generation_wrong_metadata = saved.snapshot.clone();
    Arc::make_mut(&mut same_generation_wrong_metadata.index).metadata["revision"] = json!(999);
    assert!(store
        .save_record_partition(
            projection(&changed, &[overlay.clone()]),
            &same_generation_wrong_metadata,
            &[],
            &[overlay.clone()]
        )
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT:"));
    let mut external = fresh(&store);
    changed["revision"] = json!(2);
    let latest = external.save(changed).await.unwrap();
    assert!(store
        .save_record_partition(
            projection(&original, &[overlay.clone()]),
            &saved.snapshot,
            &[],
            &[overlay]
        )
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT:"));
    assert_eq!(
        fresh(&store).load_index().unwrap().unwrap().generation(),
        latest.generation
    );
}

#[tokio::test]
async fn injected_publication_failure_preserves_original_head_and_restart_then_same_append_commits_once(
) {
    let (_dir, mut store, snapshot, mut original) = seed(state(33)).await;
    let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
    let append =
        json!({"id":"same-original-append","request_id":"original-request","content":"合成🙂"});
    overlay.records.push(RecordSlot::Owned(append.clone()));
    let input = projection(&original, &[overlay.clone()]);
    let head = fs::read(store.head()).unwrap();
    store.set_fail_before_head(true);
    assert!(store
        .save_record_partition(input.clone(), &snapshot, &[], &[overlay.clone()])
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_INJECTED:"));
    assert_eq!(fs::read(store.head()).unwrap(), head);
    assert_eq!(
        store.current.as_ref().unwrap().generation,
        snapshot.generation()
    );
    let mut restarted = fresh(&store);
    let base = restarted.load_index().unwrap().unwrap();
    assert_eq!(restarted.materialize(&base).unwrap(), original);
    original["sessions"][0]["messages"]
        .as_array_mut()
        .unwrap()
        .push(append);
    let saved = restarted
        .save_record_partition(input, &base, &[], &[overlay.clone()])
        .await
        .unwrap();
    assert_eq!(restarted.materialize(&saved.snapshot).unwrap(), original);
    assert!(restarted
        .save_record_partition(
            projection(&original, &[overlay.clone()]),
            &saved.snapshot,
            &[],
            &[overlay]
        )
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT:"));
    assert_eq!(
        fresh(&restarted)
            .load_index()
            .unwrap()
            .unwrap()
            .count("session-A", Kind::Messages)
            .unwrap(),
        34
    );
}

#[tokio::test]
async fn hot_input_and_pending_cipher_budgets_fail_before_head_publication() {
    let (_dir, mut store, snapshot, original) = seed(state(1)).await;
    let head = fs::read(store.head()).unwrap();
    let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
    overlay.records.push(RecordSlot::Owned(
        json!({"id":"over-budget","content":"x".repeat(MATERIALIZE_LIMIT)}),
    ));
    assert!(store
        .save_record_partition(
            projection(&original, &[overlay.clone()]),
            &snapshot,
            &[],
            &[overlay]
        )
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_LIMIT:"));
    assert_eq!(fs::read(store.head()).unwrap(), head);
    let mut pending = vec![];
    let mut bytes = MATERIALIZE_LIMIT - 1;
    overlay::push_pending(
        &mut pending,
        &mut bytes,
        true,
        "one".into(),
        Zeroizing::new(vec![1]),
    )
    .unwrap();
    assert_eq!(bytes, MATERIALIZE_LIMIT);
    assert!(overlay::push_pending(
        &mut pending,
        &mut bytes,
        true,
        "two".into(),
        Zeroizing::new(vec![2])
    )
    .unwrap_err()
    .starts_with("ARCHIVE_LIMIT:"));
    assert_eq!(pending.len(), 1);
    assert_eq!(bytes, MATERIALIZE_LIMIT);
}

#[tokio::test]
async fn rewritten_retained_payload_is_budgeted_even_when_hot_overlay_is_small() {
    let (_dir, mut store, mut snapshot, mut original) = seed(state(0)).await;
    // More than 64MiB of retained text is admitted in separate bounded transactions.
    // Updating four tiny fields must not queue all rewritten cold ciphertext without a cap.
    for batch in 0..4 {
        let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
        for offset in 0..32 {
            overlay.records.push(RecordSlot::Owned(json!({
                "id":format!("large-{}",batch * 32 + offset),"content":"x".repeat(600 * 1024)
            })));
        }
        snapshot = store
            .save_record_partition(
                projection(&original, &[overlay.clone()]),
                &snapshot,
                &[],
                &[overlay],
            )
            .await
            .unwrap()
            .snapshot;
    }
    let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
    for n in [0, 32, 64, 96] {
        overlay.records[n] =
            RecordSlot::Owned(json!({"id":format!("large-{n}"),"content":"新的小值"}));
    }
    let head = fs::read(store.head()).unwrap();
    original["revision"] = json!(2);
    let input = projection(&original, &[overlay.clone()]);
    assert!(serde_json::to_vec(&(&input, &overlay)).unwrap().len() < 32 * 1024);
    assert!(store
        .save_record_partition(input, &snapshot, &[], &[overlay])
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_LIMIT:"));
    assert_eq!(fs::read(store.head()).unwrap(), head);
    assert_eq!(
        store.current.as_ref().unwrap().generation,
        snapshot.generation()
    );
    let mut restarted = fresh(&store);
    let base = restarted.load_index().unwrap().unwrap();
    assert_eq!(base.count("session-A", Kind::Messages).unwrap(), 128);
    assert_eq!(
        restarted
            .read_record(&base, "session-A", Kind::Messages, "large-96")
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .len(),
        600 * 1024
    );
}

#[tokio::test]
async fn legacy_mode_is_not_silently_upgraded_by_record_overlay() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let snapshot = store.save(state(3)).await.unwrap().snapshot;
    let overlay = retained(&snapshot, "session-A", Kind::Messages);
    let head = fs::read(store.head()).unwrap();
    assert!(store
        .save_record_partition(
            projection(&state(3), &[overlay.clone()]),
            &snapshot,
            &[],
            &[overlay]
        )
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE:"));
    assert_eq!(fs::read(store.head()).unwrap(), head);
}

#[tokio::test]
async fn growing_hot_record_splits_its_old_segment_without_repacking_following_history() {
    let mut original = state(35);
    for item in original["sessions"][0]["messages"].as_array_mut().unwrap() {
        item["content"] = json!("x".repeat(800 * 1024));
    }
    let (_dir, mut store, snapshot, mut expected) = seed(original).await;
    let old = snapshot
        .index
        .series
        .iter()
        .find(|s| s.kind == Kind::Messages)
        .unwrap();
    let untouched = old.segments[1].clone();
    let mut overlay = retained(&snapshot, "session-A", Kind::Messages);
    expected["sessions"][0]["messages"][0]["content"] = json!("y".repeat(16 * 1024 * 1024));
    overlay.records[0] = RecordSlot::Owned(expected["sessions"][0]["messages"][0].clone());
    let saved = store
        .save_record_partition(
            projection(&expected, &[overlay.clone()]),
            &snapshot,
            &[],
            &[overlay],
        )
        .await
        .unwrap();
    assert_eq!(store.metrics().segment_reads, 1);
    assert_eq!(saved.segments_written, 2);
    let current = saved
        .snapshot
        .index
        .series
        .iter()
        .find(|s| s.kind == Kind::Messages)
        .unwrap();
    assert_eq!(
        current.segments.iter().map(|s| s.count).collect::<Vec<_>>(),
        [8, 8, 19]
    );
    assert_eq!(current.segments[2], untouched);
    let mut actual = vec![];
    for offset in (0..35).step_by(8) {
        actual.extend(
            store
                .read_range(&saved.snapshot, "session-A", Kind::Messages, offset, 8)
                .unwrap(),
        );
    }
    assert_eq!(
        actual,
        *expected["sessions"][0]["messages"].as_array().unwrap()
    );
    let mut restarted = fresh(&store);
    let base = restarted.load_index().unwrap().unwrap();
    assert_eq!(
        restarted
            .read_record(&base, "session-A", Kind::Messages, "message-34")
            .unwrap(),
        expected["sessions"][0]["messages"][34]
    );
}
