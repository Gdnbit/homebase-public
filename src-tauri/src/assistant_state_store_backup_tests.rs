use super::*;
use serde_json::json;

fn fresh() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = backup_store(directory.path()).unwrap();
    (directory, store)
}

fn session(index: usize, body: String) -> Value {
    json!({
        "id":format!("session-{index}"), "source":"wechat", "source_key":"synthetic-owner",
        "messages":[{"id":"original-message","content":body}],
        "turns":[{"id":"original-turn","state":"interrupted_unknown","request_id":"original-request"}],
        "jobs":[{"id":"original-job","state":"unknown","result":{"code":"RESULT_UNAVAILABLE"}}],
        "history":[[{"role":"user","content":"原上下文🙂"}]]
    })
}

#[tokio::test]
async fn backup_pins_a_complete_over_64_mib_generation_while_source_advances() {
    let (_directory, mut source) = fresh();
    let anchor = encode(&json!({"fixture":"synthetic legacy snapshot"}), 4096).unwrap();
    fs::write(&source.legacy, &anchor).unwrap();
    source.enable_capacity_segments();
    assert!(source.load_index().unwrap().is_none());
    let payload = "x".repeat(17 * 1024 * 1024);
    for index in 0..4 {
        let current = source.load_index().unwrap();
        let mut metadata = current
            .as_ref()
            .map(Snapshot::metadata)
            .unwrap_or_else(|| json!({"archive_capacity_version":1,"revision":1,"sessions":[]}));
        let sessions = metadata["sessions"].as_array_mut().unwrap();
        let count = sessions.len();
        let preserved = current
            .iter()
            .flat_map(|prior| (0..count).map(move |n| (format!("session-{n}"), prior.clone())))
            .collect::<Vec<_>>();
        sessions.push(session(index, format!("第{index}条🙂{payload}")));
        source.save_partition(metadata, &preserved).await.unwrap();
    }
    let original = source.load_index().unwrap().unwrap();
    let retained_head = fs::read(source.head()).unwrap();
    assert!(source
        .materialize(&original)
        .unwrap_err()
        .starts_with("ARCHIVE_LIMIT:"));
    let pinned = PinnedArchive::read(&source).unwrap();
    let mut newer = original.metadata();
    newer["revision"] = json!(2);
    let preserved = (0..4)
        .map(|n| (format!("session-{n}"), original.clone()))
        .collect::<Vec<_>>();
    let latest = source.save_partition(newer, &preserved).await.unwrap();
    assert_ne!(latest.generation, original.generation());
    let live_head = fs::read(source.head()).unwrap();

    let backup_id = uuid::Uuid::new_v4().to_string();
    let destination = source.backup_directory(&backup_id).unwrap();
    crate::control_protocol::private_directory(&destination)
        .await
        .unwrap();
    let report = pinned
        .publish(
            &source,
            &destination.join("workspace"),
            &destination,
            backup_id.clone(),
        )
        .unwrap();
    assert_eq!(report.state, ArchiveBackupState::Published);
    assert!(report.encrypted_bytes.unwrap() > 64 * 1024 * 1024);
    assert_eq!(report.generation.as_deref(), Some(original.generation()));
    let mut restored = backup_store(&destination).unwrap();
    assert_eq!(fs::read(restored.head()).unwrap(), retained_head);
    assert_eq!(fs::read(&restored.legacy).unwrap(), anchor.as_slice());
    let restored_head = restored.load_index().unwrap().unwrap();
    assert!(restored_head.index == original.index);
    assert_eq!(restored_head.metadata()["revision"], 1);
    for index in 0..4 {
        let id = format!("session-{index}");
        assert!(restored_head.owner_matches(&id, "wechat", "synthetic-owner"));
        let msg = restored
            .read_record(&restored_head, &id, Kind::Messages, "original-message")
            .unwrap();
        assert_eq!(
            msg["content"].as_str().unwrap(),
            format!("第{index}条🙂{payload}")
        );
        let turn = restored
            .read_record(&restored_head, &id, Kind::Turns, "original-turn")
            .unwrap();
        assert_eq!(turn["state"], "interrupted_unknown");
        let job = restored
            .read_record(&restored_head, &id, Kind::Jobs, "original-job")
            .unwrap();
        assert_eq!(job["result"]["code"], "RESULT_UNAVAILABLE");
        let context = restored
            .read_range(&restored_head, &id, Kind::Context, 0, 1)
            .unwrap();
        assert_eq!(context[0][0]["content"], "原上下文🙂");
    }
    let status = source.backup_status(backup_id).unwrap();
    assert_eq!(status.state, ArchiveBackupState::Published);
    assert_eq!(status.generation, report.generation);
    assert_eq!(fs::read(source.head()).unwrap(), live_head);
}

#[tokio::test]
async fn backup_publishes_once_for_a_shared_id_and_rejects_noncanonical_ids() {
    let (directory, mut writer) = fresh();
    let workspace = || directory.path().join("workspace");
    let source = || backup_store(directory.path()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        source().backup_status(id.clone()).unwrap().state,
        ArchiveBackupState::NotFound
    );
    assert!(source()
        .backup("../outside".into(), workspace())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_IDENTITY:"));
    assert!(source()
        .backup(id.clone(), workspace())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_BACKUP_NO_INDEX:"));
    assert!(!directory.path().join("backups").exists());
    writer.load_index().unwrap();
    let saved = writer
        .save(json!({"sessions":[session(0,"合成内容".into())]}))
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        source().backup(id.clone(), workspace()),
        source().backup(id.clone(), workspace())
    );
    let (accepted, rejected) = if first.is_ok() {
        (first, second)
    } else {
        (second, first)
    };
    let accepted = accepted.unwrap();
    assert_eq!(accepted.state, ArchiveBackupState::Published);
    assert!(rejected
        .err()
        .unwrap()
        .starts_with("ARCHIVE_BACKUP_EXISTS:"));
    let status = source().backup_status(id).unwrap();
    assert_eq!(
        status.generation.as_deref(),
        Some(saved.generation.as_str())
    );
    assert_eq!(status.encrypted_bytes, accepted.encrypted_bytes);
}

#[tokio::test]
async fn backup_rechecks_cached_cipher_and_leaves_failed_attempt_explicitly_incomplete() {
    let (directory, mut writer) = fresh();
    let workspace = || directory.path().join("workspace");
    let source = || backup_store(directory.path()).unwrap();
    writer.load_index().unwrap();
    let saved = writer
        .save(json!({"sessions":[session(0,"合成内容".into())]}))
        .await
        .unwrap();
    let original_head = fs::read(writer.head()).unwrap();
    let body = saved
        .snapshot
        .index
        .series
        .iter()
        .find(|series| series.kind == Kind::Messages)
        .unwrap();
    let file = writer.segment(&body.segments[0].file).unwrap();
    let original_cipher = fs::read(&file).unwrap();
    writer
        .read_record(
            &saved.snapshot,
            "session-0",
            Kind::Messages,
            "original-message",
        )
        .unwrap();
    let mut corrupt = original_cipher.clone();
    corrupt[0] ^= 1;
    fs::write(&file, corrupt).unwrap();
    let corrupt_id = uuid::Uuid::new_v4().to_string();
    assert!(writer
        .backup(corrupt_id.clone(), workspace())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_CHANGED:"));
    assert_eq!(
        source().backup_status(corrupt_id.clone()).unwrap().state,
        ArchiveBackupState::Incomplete
    );
    assert!(!source()
        .backup_directory(&corrupt_id)
        .unwrap()
        .join("history-v1/head.dpapi")
        .exists());
    assert!(source()
        .backup(corrupt_id, workspace())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_BACKUP_EXISTS:"));

    fs::rename(&file, file.with_extension("damaged")).unwrap();
    let missing_id = uuid::Uuid::new_v4().to_string();
    assert!(source()
        .backup(missing_id.clone(), workspace())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_MISSING:"));
    assert_eq!(
        source().backup_status(missing_id).unwrap().state,
        ArchiveBackupState::Incomplete
    );
    fs::write(&file, original_cipher).unwrap();
    assert_eq!(fs::read(source().head()).unwrap(), original_head);
    assert_eq!(
        source()
            .backup(uuid::Uuid::new_v4().to_string(), workspace())
            .await
            .unwrap()
            .state,
        ArchiveBackupState::Published
    );
}
