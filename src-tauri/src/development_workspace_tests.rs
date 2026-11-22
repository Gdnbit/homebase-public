use super::*;
use std::sync::atomic::Ordering;

fn draft(text: &str) -> DevelopmentDraft {
    DevelopmentDraft {
        workspace: "F:/Synthetic/项目".into(),
        brief: text.into(),
        ..Default::default()
    }
}
#[tokio::test]
async fn empty_get_is_read_only_and_round_trip_is_encrypted() {
    let root = tempfile::tempdir().unwrap();
    let store = DevelopmentWorkspaceStore::new(root.path().join("private"));
    assert_eq!(store.get().unwrap().revision, 0);
    assert!(!root.path().join("private").exists());
    let mut d = draft("合成未提交需求🙂");
    d.checks_json = "[{unfinished".into();
    let e = store.save(0, d.clone()).await.unwrap();
    let cipher = fs::read(store.path()).unwrap();
    assert!(!cipher
        .windows(d.brief.len())
        .any(|w| w == d.brief.as_bytes()));
    let restarted = DevelopmentWorkspaceStore::new(root.path().join("private"));
    assert_eq!(restarted.get().unwrap(), e);
    assert_eq!(restarted.get().unwrap().draft.checks_json, "[{unfinished");
    assert_eq!(fs::read(store.path()).unwrap(), cipher);
}
#[tokio::test]
async fn cas_failure_and_corruption_preserve_original_cipher() {
    let root = tempfile::tempdir().unwrap();
    let store = DevelopmentWorkspaceStore::new(root.path().join("private"));
    let e = store.save(0, draft("ORIGINAL")).await.unwrap();
    let before = fs::read(store.path()).unwrap();
    assert!(store
        .save(0, draft("STALE"))
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    store.fail_publish.store(true, Ordering::SeqCst);
    assert!(store
        .save(e.revision, draft("FAILED SAVE"))
        .await
        .unwrap_err()
        .contains("SAVE"));
    assert_eq!(fs::read(store.path()).unwrap(), before);
    fs::write(store.path(), b"synthetic corrupt").unwrap();
    assert!(store.get().unwrap_err().contains("READ"));
    assert!(store
        .save(e.revision, draft("DO NOT REPLACE"))
        .await
        .is_err());
    assert_eq!(fs::read(store.path()).unwrap(), b"synthetic corrupt");
}
#[tokio::test]
async fn immutable_pending_and_archive_survive_restart_without_executor() {
    let root = tempfile::tempdir().unwrap();
    let store = DevelopmentWorkspaceStore::new(root.path().join("private"));
    let d = draft("ORIGINAL REQUEST");
    let request = d.snapshot("original-id".into()).unwrap();
    let e = store.prepare(0, d.clone(), request.clone()).await.unwrap();
    let before = fs::read(store.path()).unwrap();
    assert_eq!(
        store.prepare(0, d.clone(), request.clone()).await.unwrap(),
        e
    );
    assert_eq!(fs::read(store.path()).unwrap(), before);
    let mut wrong = request.clone();
    wrong.brief = "CHANGED".into();
    assert!(store
        .prepare(e.revision, d.clone(), wrong)
        .await
        .unwrap_err()
        .contains("ID"));
    assert!(store
        .prepare(
            e.revision,
            d.clone(),
            d.snapshot("other-id".into()).unwrap()
        )
        .await
        .unwrap_err()
        .contains("PENDING"));
    assert!(store
        .fresh(e.revision, Some("wrong-id".into()))
        .await
        .unwrap_err()
        .contains("ID"));
    let archived = store
        .fresh(e.revision, Some(request.request_id.clone()))
        .await
        .unwrap();
    assert_eq!(archived.previous, vec![request.clone()]);
    assert!(archived.pending.is_none());
    assert_eq!(archived.draft, d);
    let restarted = DevelopmentWorkspaceStore::new(root.path().join("private"));
    assert_eq!(restarted.get().unwrap(), archived);
    assert!(restarted
        .prepare(archived.revision, d, request)
        .await
        .unwrap_err()
        .contains("ID"));
}
#[tokio::test]
async fn saves_can_preserve_later_input_while_pending_snapshot_stays_original() {
    let root = tempfile::tempdir().unwrap();
    let store = DevelopmentWorkspaceStore::new(root.path().join("private"));
    let a = draft("SNAPSHOT A");
    let p = store
        .prepare(0, a.clone(), a.snapshot("id-a".into()).unwrap())
        .await
        .unwrap();
    let b = draft("LATER B");
    let saved = store.save(p.revision, b.clone()).await.unwrap();
    assert_eq!(saved.draft, b);
    assert_eq!(saved.pending.unwrap().brief, "SNAPSHOT A");
    let before = fs::read(store.path()).unwrap();
    assert!(store
        .fresh(p.revision, Some("id-a".into()))
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    assert_eq!(fs::read(store.path()).unwrap(), before);
}
#[tokio::test]
async fn oversized_draft_and_invalid_snapshot_are_not_truncated_or_published() {
    let root = tempfile::tempdir().unwrap();
    let store = DevelopmentWorkspaceStore::new(root.path().join("private"));
    let original = store.save(0, draft("KEEP")).await.unwrap();
    let before = fs::read(store.path()).unwrap();
    assert!(store
        .save(original.revision, draft(&"a".repeat(256 * 1024 + 1)))
        .await
        .unwrap_err()
        .contains("LIMIT"));
    let long = draft(&"字".repeat(6000));
    assert!(long.snapshot("id".into()).is_err());
    let mut invalid = draft("VALID");
    invalid.checks_json = "[[]]".into();
    assert!(invalid.snapshot("id".into()).is_err());
    assert_eq!(fs::read(store.path()).unwrap(), before);
}
#[test]
fn error_allowlist_never_exports_arbitrary_private_suffixes() {
    assert!(public_error("DEVELOPMENT_WORKSPACE_READ").is_some());
    assert!(public_error("DEVELOPMENT_WORKSPACE_READ: private body").is_none());
}

#[tokio::test]
async fn archive_capacity_is_explicit_and_never_discards_the_oldest_request() {
    let root = tempfile::tempdir().unwrap();
    let store = DevelopmentWorkspaceStore::new(root.path().join("private"));
    let d = draft("SYNTHETIC HISTORY");
    let e = DevelopmentWorkspaceEntry {
        previous: (0..MAX_REQUESTS)
            .map(|n| d.snapshot(format!("original-{n}")).unwrap())
            .collect(),
        ..Default::default()
    };
    let full = {
        let _private_lock = store.lock().await.unwrap();
        store.publish(e).await.unwrap()
    };
    let before = fs::read(store.path()).unwrap();
    assert_eq!(store.get().unwrap().previous.len(), MAX_REQUESTS);
    assert!(store
        .prepare(
            full.revision,
            d.clone(),
            d.snapshot("one-more".into()).unwrap()
        )
        .await
        .unwrap_err()
        .contains("LIMIT"));
    assert_eq!(fs::read(store.path()).unwrap(), before);
    assert_eq!(store.get().unwrap().previous[0].request_id, "original-0");
}

#[tokio::test]
async fn independent_store_handles_serialize_against_the_persisted_revision() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let a = DevelopmentWorkspaceStore::new(directory.clone());
    let b = DevelopmentWorkspaceStore::new(directory);
    let saved = a.save(0, draft("FIRST")).await.unwrap();
    assert!(b
        .prepare(
            0,
            draft("STALE"),
            draft("STALE").snapshot("stale-id".into()).unwrap()
        )
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    assert_eq!(b.get().unwrap(), saved);
    assert!(!root.path().join("capabilities/development").exists());
}

#[tokio::test]
async fn total_encoded_budget_preserves_cli_result_headroom_without_truncation() {
    let root = tempfile::tempdir().unwrap();
    let store = DevelopmentWorkspaceStore::new(root.path().join("private"));
    let saved = store.save(0, draft("KEEP")).await.unwrap();
    let before = fs::read(store.path()).unwrap();
    let mut large = draft(&"x".repeat(256 * 1024));
    large.workspace = "w".repeat(256 * 1024);
    large.checks_json = "c".repeat(256 * 1024);
    large.protected_files_input = "p".repeat(256 * 1024);
    assert!(store
        .save(saved.revision, large)
        .await
        .unwrap_err()
        .contains("LIMIT"));
    assert_eq!(fs::read(store.path()).unwrap(), before);
    assert_eq!(store.get().unwrap(), saved);
}
