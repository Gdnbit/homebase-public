use super::*;
use serde_json::json;
#[path = "assistant_state_store_overlay_tests.rs"]
mod record_overlay_tests;
#[path = "assistant_state_store_view_tests.rs"]
mod read_view_tests;
#[test]
fn new_storage_initialization_requires_explicit_isolated_mode_and_never_accepts_unknown_flags() {
    use std::ffi::OsStr;
    assert!(!initialization_allowed(None, None).unwrap());
    assert!(!initialization_allowed(Some(OsStr::new("0")), None).unwrap());
    assert!(initialization_allowed(Some(OsStr::new("1")), None).is_err());
    assert!(initialization_allowed(Some(OsStr::new("1")), Some(OsStr::new("relative"))).is_err());
    assert!(initialization_allowed(
        Some(OsStr::new("1")),
        Some(std::env::temp_dir().as_os_str())
    )
    .unwrap());
    assert!(initialization_allowed(
        Some(OsStr::new("unknown")),
        Some(std::env::temp_dir().as_os_str())
    )
    .is_err());
}
fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::Builder::new()
        .prefix("state-store-")
        .tempdir()
        .unwrap();
    let store = Store::new(
        temp.path().join("history-v1"),
        temp.path().join("state.dpapi"),
    )
    .unwrap();
    (temp, store)
}
fn state(n: usize) -> Value {
    json!({"settings":{"fixture":true},"revision":1,"personal":{"memory":"私人的合成偏好"},"sessions":[{"id":"session-A","source":"wechat","source_key":"synthetic-private-owner","title":"合成对话🙂","messages":(0..n).map(|i|json!({"id":format!("message-{i}"),"content":format!("合成中文🙂内容{i}"),"extra":{"preserved":true}})).collect::<Vec<_>>(),"turns":[{"id":"turn-A","state":"completed","request_id":"original-request","checkpoint":{"opaque":"unchanged"}}],"jobs":[{"id":"job-A","owned":false,"state":"unknown","result":{"private":"合成结果"}}],"history":[[{"role":"user","content":"原模型投影"}]]}]})
}
fn read_index(store: &Store) -> Index {
    decode(&fs::read(store.head()).unwrap()).unwrap()
}
async fn replace_index(store: &Store, index: &Index) {
    crate::control_protocol::publish_private(&store.head(), &encode(index, CIPHER_LIMIT).unwrap())
        .await
        .unwrap();
}

#[tokio::test]
async fn complete_roundtrip_is_private_and_metadata_and_range_reads_are_lazy_and_cached() {
    let (_dir, mut store) = fixture();
    assert!(store.load_index().unwrap().is_none());
    let value = state(70);
    let saved = store.save(value.clone()).await.unwrap();
    assert_eq!(saved.segments_written, 5);
    let mut reader = Store::new(store.root.clone(), store.legacy.clone()).unwrap();
    let snap = reader.load_index().unwrap().unwrap();
    assert_eq!(reader.metrics().segment_reads, 0);
    assert_eq!(snap.count("session-A", Kind::Messages).unwrap(), 70);
    let first = reader
        .read_range(&snap, "session-A", Kind::Messages, 0, 10)
        .unwrap();
    assert_eq!(
        first,
        value["sessions"][0]["messages"].as_array().unwrap()[..10]
    );
    assert_eq!(reader.metrics().segment_reads, 1);
    reader
        .read_range(&snap, "session-A", Kind::Messages, 10, 10)
        .unwrap();
    assert_eq!(reader.metrics().segment_reads, 1);
    assert_eq!(reader.materialize(&snap).unwrap(), value);
    for file in fs::read_dir(store.root.join("segments"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .chain(std::iter::once(store.head()))
    {
        let bytes = fs::read(file).unwrap();
        for phrase in ["synthetic-private-owner", "合成中文", "original-request"] {
            assert!(!bytes.windows(phrase.len()).any(|w| w == phrase.as_bytes()));
        }
    }
}

#[cfg(windows)]
#[tokio::test]
async fn locked_head_preserves_original_generation_and_reports_the_failed_publication_stage() {
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (dir, mut store) = fixture();
    store.load_index().unwrap();
    let original = state(1);
    let prior = store.save(original.clone()).await.unwrap();
    let old_head = fs::read(store.head()).unwrap();
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(store.head())
        .unwrap();
    let mut changed = original.clone();
    changed["sessions"][0]["title"] = json!("合成标题修改");

    let failure = store.save(changed.clone()).await.err().unwrap();
    assert!(failure.starts_with("ARCHIVE_IO:"));
    assert!(failure.contains("提交历史索引 · 原子替换"));
    assert!(failure.contains("系统代码"));
    assert!(!failure.contains(dir.path().to_string_lossy().as_ref()));
    assert_eq!(public_error(&failure).unwrap().0, "ARCHIVE_IO");
    eprintln!("synthetic archive failure: {failure}");
    assert_eq!(fs::read(store.head()).unwrap(), old_head);
    let previous = store.load_index().unwrap().unwrap();
    assert_eq!(previous.index.generation, prior.generation);
    assert_eq!(store.materialize(&previous).unwrap(), original);

    let retried = Arc::new(AtomicBool::new(false));
    let observed_retry = Arc::clone(&retried);
    store.before_index_retry = Some(Box::new(move || {
        drop(held);
        observed_retry.store(true, Ordering::SeqCst);
    }));
    let stored = store.save(changed.clone()).await.unwrap();
    assert!(retried.load(Ordering::SeqCst));
    assert_ne!(stored.generation, prior.generation);
    assert_eq!(store.materialize(&stored.snapshot).unwrap(), changed);
}

#[cfg(windows)]
#[tokio::test]
async fn index_retry_rejects_a_changed_generation_without_overwriting_it() {
    use std::os::windows::fs::OpenOptionsExt;

    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let original = state(1);
    let prior = store.save(original.clone()).await.unwrap();
    let mut foreign = read_index(&store);
    foreign.generation = uuid::Uuid::new_v4().to_string();
    foreign.metadata["revision"] = json!(9);
    let foreign_bytes = encode(&foreign, CIPHER_LIMIT).unwrap();
    let replacement = foreign_bytes.clone();
    let head = store.head();
    let target = head.clone();
    let held = fs::OpenOptions::new().read(true).share_mode(1).open(&head).unwrap();
    store.before_index_retry = Some(Box::new(move || {
        drop(held);
        fs::write(target, replacement.as_slice()).unwrap();
    }));
    let mut changed = original;
    changed["sessions"][0]["title"] = json!("不能覆盖更新者");
    let failure = store.save(changed).await.err().unwrap();
    assert!(failure.starts_with("ARCHIVE_CONFLICT:"));
    assert_eq!(fs::read(head).unwrap().as_slice(), foreign_bytes.as_slice());
    assert_eq!(store.current.as_ref().unwrap().generation, prior.generation);
}

#[tokio::test]
async fn original_record_lookup_decrypts_only_its_segment_and_checks_payload_identity() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let value = state(90);
    store.save(value.clone()).await.unwrap();
    let mut reader = Store::new(store.root.clone(), store.legacy.clone()).unwrap();
    let snap = reader.load_index().unwrap().unwrap();
    reader.load_index().unwrap();
    assert_eq!(reader.metrics().index_decryptions, 1);
    assert_eq!(
        reader
            .read_record(&snap, "session-A", Kind::Messages, "message-82")
            .unwrap(),
        value["sessions"][0]["messages"][82]
    );
    assert_eq!(reader.metrics().segment_reads, 1);
    assert!(reader
        .read_record(&snap, "session-A", Kind::Jobs, "message-82")
        .unwrap_err()
        .starts_with("ARCHIVE_RECORD_MISSING"));
    let mut wrong = read_index(&store);
    wrong.series[0].segments[0].item_ids[0] = "foreign-id".into();
    replace_index(&store, &wrong).await;
    let snap = reader.load_index().unwrap().unwrap();
    assert!(reader
        .read_record(&snap, "session-A", Kind::Messages, "foreign-id")
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE"));
}

#[tokio::test]
async fn malformed_root_and_noncanonical_filenames_are_rejected_without_panics_or_path_reflection()
{
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    for value in [json!(null), json!([]), json!({}), json!({"sessions":"bad"})] {
        assert!(store
            .save(value)
            .await
            .unwrap_err()
            .starts_with("ARCHIVE_INVALID"));
    }
    assert!(!store.head().exists());
    let id = uuid::Uuid::new_v4().to_string();
    for value in [
        format!("urn:uuid:{id}"),
        id.replace('-', ""),
        format!("../{id}"),
        "synthetic-private-owner".into(),
    ] {
        let e = store.segment(&value).unwrap_err();
        assert!(e.starts_with("ARCHIVE_PATH"));
        assert!(!e.contains("synthetic-private-owner"));
    }
}
#[tokio::test]
async fn unchanged_segments_are_reused_and_only_a_changed_record_group_is_published() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let value = state(70);
    store.save(value.clone()).await.unwrap();
    let before = read_index(&store);
    let head = fs::read(store.head()).unwrap();
    let noop = store.save(value.clone()).await.unwrap();
    assert!(!noop.index_written);
    assert_eq!(noop.segments_written, 0);
    assert_eq!(noop.segments_reused, 5);
    assert_eq!(fs::read(store.head()).unwrap(), head);
    let mut changed = value;
    changed["sessions"][0]["messages"][35]["content"] = json!("修正后的合成内容");
    changed["revision"] = json!(2);
    let result = store.save(changed.clone()).await.unwrap();
    assert_eq!(result.segments_written, 1);
    assert_eq!(result.segments_reused, 4);
    let after = read_index(&store);
    assert_eq!(before.series[0].segments[0], after.series[0].segments[0]);
    assert_ne!(before.series[0].segments[1], after.series[0].segments[1]);
    let snap = store.load_index().unwrap().unwrap();
    assert_eq!(store.materialize(&snap).unwrap(), changed);
}
#[tokio::test]
async fn frozen_snapshot_reads_original_records_after_new_commit_without_mixing_pages() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let value = state(70);
    store.save(value.clone()).await.unwrap();
    let old = store.load_index().unwrap().unwrap();
    let mut next = value.clone();
    next["sessions"][0]["messages"][0]["content"] = json!("新版本");
    store.save(next).await.unwrap();
    assert_eq!(store.materialize(&old).unwrap(), value);
    let current = store.load_index().unwrap().unwrap();
    assert_ne!(old.generation(), current.generation());
    assert!(store
        .read_range(&old, "other", Kind::Messages, 0, 1)
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE"));
    assert!(store
        .read_range(&old, "session-A", Kind::Messages, 71, 1)
        .unwrap_err()
        .starts_with("ARCHIVE_RANGE"));
    let (_other, mut foreign) = fixture();
    assert!(foreign
        .read_range(&old, "session-A", Kind::Messages, 0, 1)
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE"));
}
#[tokio::test]
async fn stale_writers_and_changed_legacy_source_are_rejected_without_overwrite() {
    let (_dir, mut first) = fixture();
    let mut second = Store::new(first.root.clone(), first.legacy.clone()).unwrap();
    first.load_index().unwrap();
    second.load_index().unwrap();
    first.save(state(1)).await.unwrap();
    let original = fs::read(first.head()).unwrap();
    assert!(second
        .save(state(2))
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT"));
    assert_eq!(fs::read(first.head()).unwrap(), original);
    fs::write(&first.legacy, b"synthetic-old-writer").unwrap();
    assert!(first
        .load_index()
        .unwrap_err()
        .starts_with("ARCHIVE_LEGACY_CONFLICT"));
    assert!(first
        .save(state(3))
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_LEGACY_CONFLICT"));
    assert_eq!(fs::read(first.head()).unwrap(), original);
}
#[tokio::test]
async fn failed_publication_preserves_old_head_and_does_not_adopt_pending_data() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let old = state(1);
    store.save(old.clone()).await.unwrap();
    let before = fs::read(store.head()).unwrap();
    store.fail_before_head = true;
    assert!(store
        .save(state(50))
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_INJECTED"));
    assert_eq!(fs::read(store.head()).unwrap(), before);
    let mut restarted = Store::new(store.root.clone(), store.legacy.clone()).unwrap();
    let snapshot = restarted.load_index().unwrap().unwrap();
    assert_eq!(restarted.materialize(&snapshot).unwrap(), old);
    let (_new, mut new) = fixture();
    new.load_index().unwrap();
    new.fail_before_head = true;
    new.save(state(1)).await.unwrap_err();
    let mut restart = Store::new(new.root.clone(), new.legacy.clone()).unwrap();
    assert!(restart
        .load_index()
        .unwrap_err()
        .starts_with("ARCHIVE_INCOMPLETE"));
}
#[tokio::test]
async fn invalid_record_and_session_identity_is_rejected_before_publication() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    for mut value in [state(2), state(2)] {
        value["sessions"][0]["messages"][1]["id"] =
            value["sessions"][0]["messages"][0]["id"].clone();
        assert!(store
            .save(value)
            .await
            .unwrap_err()
            .starts_with("ARCHIVE_IDENTITY"));
    }
    let mut duplicate = state(1);
    let session = duplicate["sessions"][0].clone();
    duplicate["sessions"].as_array_mut().unwrap().push(session);
    assert!(store
        .save(duplicate)
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_IDENTITY"));
    assert!(!store.head().exists());
}
#[tokio::test]
async fn corrupt_missing_and_foreign_segment_records_are_explicit_not_empty() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    store.save(state(3)).await.unwrap();
    let snap = store.load_index().unwrap().unwrap();
    let reference = &snap.index.series[0].segments[0];
    let file = store.segment(&reference.file).unwrap();
    let original = fs::read(&file).unwrap();
    fs::write(&file, b"changed").unwrap();
    assert!(store
        .read_range(&snap, "session-A", Kind::Messages, 0, 1)
        .unwrap_err()
        .starts_with("ARCHIVE_CHANGED"));
    fs::rename(&file, file.with_extension("saved")).unwrap();
    assert!(store
        .read_range(&snap, "session-A", Kind::Messages, 0, 1)
        .unwrap_err()
        .starts_with("ARCHIVE_MISSING"));
    fs::write(&file, original).unwrap();
    assert_eq!(
        store
            .read_range(&snap, "session-A", Kind::Messages, 0, 1)
            .unwrap()
            .len(),
        1
    );
    let mut bad = read_index(&store);
    bad.series[0].source_digest = "0".repeat(64);
    replace_index(&store, &bad).await;
    assert!(store.load_index().unwrap_err().starts_with("ARCHIVE_SCOPE"));
}
#[tokio::test]
async fn invalid_index_ranges_filenames_and_versions_fail_closed() {
    for change in 0..4 {
        let (_dir, mut store) = fixture();
        store.load_index().unwrap();
        store.save(state(2)).await.unwrap();
        let mut index = read_index(&store);
        match change {
            0 => index.format = 3,
            1 => index.series[0].segments[0].file = "../foreign".into(),
            2 => index.series[0].segments[0].start = 1,
            _ => index.series[0].segments[0].count = 33,
        };
        replace_index(&store, &index).await;
        assert!(store.load_index().is_err());
    }
}
#[test]
fn reads_are_bounded_before_body_allocation_and_do_not_follow_links() {
    let (_dir, store) = fixture();
    let file = fs::File::create(&store.legacy).unwrap();
    file.set_len((CIPHER_LIMIT + 1) as u64).unwrap();
    drop(file);
    assert!(read_legacy(&store.legacy)
        .unwrap_err()
        .starts_with("ARCHIVE_LIMIT"));
    let linked = store.legacy.with_extension("hardlink");
    fs::hard_link(&store.legacy, &linked).unwrap();
    assert!(read_bounded(&linked, CIPHER_LIMIT + 2)
        .unwrap_err()
        .starts_with("ARCHIVE_PATH"));
}
#[tokio::test]
async fn legacy_ciphertext_is_read_without_rewrite_and_lock_contention_is_explicit() {
    let (_dir, mut store) = fixture();
    let value = state(3);
    let cipher = encode(&value, CIPHER_LIMIT).unwrap();
    fs::write(&store.legacy, &cipher).unwrap();
    let before = fs::read(&store.legacy).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&read_legacy(&store.legacy).unwrap().unwrap()).unwrap(),
        value
    );
    store.load_index().unwrap();
    store.save(value).await.unwrap();
    assert_eq!(fs::read(&store.legacy).unwrap(), before);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(store.root.join("writer.lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    assert!(store
        .save(state(4))
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_BUSY"));
}
#[test]
fn public_errors_are_specific_and_never_echo_private_details() {
    for code in [
        "ARCHIVE_CHANGED",
        "ARCHIVE_LEGACY_CONFLICT",
        "ARCHIVE_CONFLICT",
        "ARCHIVE_BUSY",
        "ARCHIVE_INCOMPLETE",
        "ARCHIVE_MISSING",
        "ARCHIVE_RECORD_MISSING",
        "ARCHIVE_PATH",
        "ARCHIVE_LIMIT",
        "ARCHIVE_CRYPTO",
        "ARCHIVE_INVALID",
        "ARCHIVE_IDENTITY",
        "ARCHIVE_SCOPE",
        "ARCHIVE_RANGE",
        "ARCHIVE_NOT_LOADED",
        "ARCHIVE_IO",
    ] {
        let (actual, text) =
            public_error(&format!("{code}: synthetic-private-path-and-content")).unwrap();
        assert_eq!(actual, code);
        assert!(!text.contains("synthetic-private"));
        assert!(!text.is_empty());
    }
    for private in [
        "unrecognized-private-error",
        "prefix ARCHIVE_IO: private",
        "ARCHIVE_IO_EXTRA: private",
        "ARCHIVE_IO",
        "ARCHIVE_INJECTED: private",
    ] {
        assert!(public_error(private).is_none());
    }
}
#[tokio::test]
async fn partial_saves_preserve_frozen_bodies_and_reject_missing_foreign_or_changed_bindings() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let original = state(70);
    store.save(original.clone()).await.unwrap();
    let frozen = store.load_index().unwrap().unwrap();
    let sid = original["sessions"][0]["id"].as_str().unwrap().to_string();
    let mut metadata = frozen.metadata();
    metadata["revision"] = serde_json::json!(42);
    let saved = store
        .save_partition(metadata.clone(), &[(sid.clone(), frozen.clone())])
        .await
        .unwrap();
    assert_eq!(saved.segments_written, 0);
    assert!(saved.segments_reused > 0);
    let latest = store.load_index().unwrap().unwrap();
    let mut expected = original;
    expected["revision"] = serde_json::json!(42);
    assert_eq!(store.materialize(&latest).unwrap(), expected);
    let committed = fs::read(store.head()).unwrap();
    let mut nonempty = metadata.clone();
    nonempty["sessions"][0]["messages"] = serde_json::json!([{"id":"smuggled-body"}]);
    assert!(store
        .save_partition(nonempty, &[(sid.clone(), frozen.clone())])
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE"));
    let mut missing = metadata.clone();
    missing["sessions"] = serde_json::json!([]);
    assert!(store
        .save_partition(missing, &[(sid.clone(), frozen.clone())])
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE"));
    let (_foreign_dir, mut foreign) = fixture();
    foreign.load_index().unwrap();
    foreign.save(state(70)).await.unwrap();
    let foreign_snapshot = foreign.load_index().unwrap().unwrap();
    assert!(store
        .save_partition(metadata.clone(), &[(sid.clone(), foreign_snapshot)])
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE"));
    assert_eq!(fs::read(store.head()).unwrap(), committed);
    store.save(state(71)).await.unwrap();
    assert!(store
        .save_partition(metadata, &[(sid, frozen)])
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT"));
}
#[tokio::test]
async fn context_segments_preserve_original_arrays_without_loading_text_in_metadata() {
    let (_dir, mut store) = fixture();
    store.enable_context_segments();
    store.load_index().unwrap();
    let mut original = state(2);
    let history = (0..65).map(|i|json!([{"role":"user","content":format!("CTX_ONLY_蓝色圆点🙂_{i}")},{"role":"assistant","content":"合成回复"}])).collect::<Vec<_>>();
    original["sessions"][0]["history"] = json!(history);
    store.save(original.clone()).await.unwrap();
    let mut reader = Store::new(store.root.clone(), store.legacy.clone()).unwrap();
    let snapshot = reader.load_index().unwrap().unwrap();
    assert!(snapshot.context_segmented());
    assert!(reader.context_segments());
    assert!(!serde_json::to_string(&snapshot.metadata())
        .unwrap()
        .contains("CTX_ONLY"));
    assert_eq!(snapshot.count("session-A", Kind::Context).unwrap(), 65);
    assert_eq!(reader.metrics().segment_reads, 0);
    assert_eq!(
        reader
            .read_range(&snapshot, "session-A", Kind::Context, 31, 2)
            .unwrap(),
        history[31..33]
    );
    assert_eq!(reader.metrics().segment_reads, 2);
    reader
        .read_record(&snapshot, "session-A", Kind::Context, "context-32")
        .unwrap();
    assert_eq!(reader.metrics().segment_reads, 2);
    assert_eq!(reader.materialize(&snapshot).unwrap(), original);
    let mut metadata = snapshot.metadata();
    metadata["revision"] = json!(42);
    let stats = reader
        .save_partition(metadata, &[("session-A".into(), snapshot.clone())])
        .await
        .unwrap();
    assert_eq!(stats.segments_written, 0);
    let newest = reader.load_index().unwrap().unwrap();
    original["revision"] = json!(42);
    assert_eq!(reader.materialize(&newest).unwrap(), original);
}

#[tokio::test]
async fn context_upgrade_and_complete_v1_export_preserve_all_original_fields_and_source_cipher() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let mut value = state(3);
    value["sessions"][0]["history"] = json!([[{"role":"user","content":"export-context🙂"}]]);
    value["extra_metadata"] = json!({"owned":false,"unknown":"must be preserved"});
    value["archive_index"] = json!({"derived":"must be rebuilt"});
    store.save(value.clone()).await.unwrap();
    let old = store.load_index().unwrap().unwrap();
    assert!(!old.context_segmented());
    store.enable_context_segments();
    assert!(store
        .save_partition(old.metadata(), &[("session-A".into(), old.clone())])
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE"));
    store.save(value.clone()).await.unwrap();
    let current = store.load_index().unwrap().unwrap();
    assert!(current.context_segmented());
    assert_eq!(store.materialize(&current).unwrap(), value);
    let source_cipher = fs::read(store.head()).unwrap();
    let (_export, mut target) = fixture();
    store.export_v1(&current, &mut target).await.unwrap();
    let exported = target.load_index().unwrap().unwrap();
    assert!(!exported.context_segmented());
    value.as_object_mut().unwrap().remove("archive_index");
    assert_eq!(target.materialize(&exported).unwrap(), value);
    assert_eq!(fs::read(store.head()).unwrap(), source_cipher);
    assert!(store
        .export_v1(&old, &mut target)
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT"));
    assert!(store
        .export_v1(&current, &mut target)
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_CONFLICT"));
}

#[tokio::test]
async fn context_corruption_missing_and_invalid_wrapper_are_explicit_not_empty() {
    let (_dir, mut store) = fixture();
    store.enable_context_segments();
    store.load_index().unwrap();
    let mut value = state(1);
    value["sessions"][0]["history"] = json!([[{"role":"user","content":"ctx-synthetic"}]]);
    store.save(value.clone()).await.unwrap();
    let snapshot = store.load_index().unwrap().unwrap();
    let reference = snapshot
        .index
        .series
        .iter()
        .find(|s| s.kind == Kind::Context)
        .unwrap()
        .segments[0]
        .clone();
    let file = store.segment(&reference.file).unwrap();
    let bytes = fs::read(&file).unwrap();
    fs::write(&file, b"synthetic corrupt context").unwrap();
    assert!(store
        .read_range(&snapshot, "session-A", Kind::Context, 0, 1)
        .unwrap_err()
        .starts_with("ARCHIVE_CHANGED"));
    fs::write(&file, &bytes).unwrap();
    let missing = file.with_extension("missing");
    fs::rename(&file, &missing).unwrap();
    assert!(store
        .read_range(&snapshot, "session-A", Kind::Context, 0, 1)
        .unwrap_err()
        .starts_with("ARCHIVE_MISSING"));
    fs::rename(&missing, &file).unwrap();
    let head = fs::read(store.head()).unwrap();
    value["sessions"][0]["history"] = json!([{"invalid":"not an array"}]);
    assert!(store
        .save(value)
        .await
        .unwrap_err()
        .starts_with("ARCHIVE_INVALID"));
    assert_eq!(fs::read(store.head()).unwrap(), head);
    let mut bad = (*snapshot.index).clone();
    bad.series
        .iter_mut()
        .find(|s| s.kind == Kind::Context)
        .unwrap()
        .segments[0]
        .item_ids[0] = "wrong-position".into();
    assert!(validate(&bad).is_err());
}
#[tokio::test]
async fn empty_existing_index_still_commits_the_explicit_context_format_upgrade() {
    let (_dir, mut store) = fixture();
    store.load_index().unwrap();
    let value = json!({"sessions":[],"revision":0});
    store.save(value.clone()).await.unwrap();
    let first = store.load_index().unwrap().unwrap();
    assert!(!first.context_segmented());
    store.enable_context_segments();
    assert!(store.save(value).await.unwrap().index_written);
    let second = store.load_index().unwrap().unwrap();
    assert!(second.context_segmented());
    assert_ne!(first.generation(), second.generation());
}
