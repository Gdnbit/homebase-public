use super::*;
use std::path::Path;
use std::sync::atomic::Ordering;

const ID: &str = "87562a38-8fb4-4276-bdda-bd0d85bef544";
fn draft(text: &str) -> InboxWorkspaceDraft {
    InboxWorkspaceDraft {
        text: text.into(),
        title: "原题".into(),
        tags_input: "synthetic".into(),
        ..Default::default()
    }
}
fn setup(root: &Path) -> (WorkspaceStore, ReceiptStore) {
    (
        WorkspaceStore::new(root.join("private")),
        ReceiptStore::new(root.join("private/receipts"), root.join("Wiki")),
    )
}
fn notes(root: &Path) -> Vec<PathBuf> {
    let mut all = vec![];
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries {
            let e = entry.unwrap();
            if e.file_type().unwrap().is_dir() {
                all.extend(notes(&e.path()));
            } else if e.path().extension().is_some_and(|v| v == "md") {
                all.push(e.path());
            }
        }
    }
    all
}

#[tokio::test]
async fn reading_or_resolving_prepared_source_does_not_execute_or_change_cipher() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    assert_eq!(ws.get(&receipts).unwrap().entry.revision, 0);
    assert!(!root.path().join("private").exists());
    let d = draft("合成原文🙂");
    let saved = ws.save(0, d.clone()).await.unwrap();
    let ready = ws
        .prepare(&receipts, saved.entry.revision, ID, d)
        .await
        .unwrap();
    let before = fs::read(ws.path()).unwrap();
    let receipt_before = fs::read(
        root.path()
            .join(format!("private/receipts/{ID}.receipt.dpapi")),
    )
    .unwrap();
    let restored = setup(root.path());
    let observed = restored.0.get(&restored.1).unwrap();
    assert_eq!(
        observed.submission.unwrap().state,
        InboxSubmissionState::Prepared
    );
    assert_eq!(
        restored.0.resolve(&restored.1, ID).await.unwrap().entry,
        ready.entry
    );
    assert!(notes(&root.path().join("Wiki")).is_empty());
    assert_eq!(fs::read(ws.path()).unwrap(), before);
    assert_eq!(
        fs::read(
            root.path()
                .join(format!("private/receipts/{ID}.receipt.dpapi"))
        )
        .unwrap(),
        receipt_before
    );
}

#[tokio::test]
async fn source_compare_and_swap_and_snapshot_identity_precede_business_preparation() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let d = draft("FIRST");
    let saved = ws.save(0, d.clone()).await.unwrap();
    let before = fs::read(ws.path()).unwrap();
    assert!(ws
        .save(0, draft("STALE"))
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    assert!(ws
        .prepare(&receipts, 0, ID, draft("CHANGED"))
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    assert_eq!(
        receipts.get(ID).unwrap().state,
        InboxSubmissionState::NotFound
    );
    assert_eq!(fs::read(ws.path()).unwrap(), before);
    let clicked = draft("CHANGED AT CLICK");
    let ready = ws
        .prepare(&receipts, saved.entry.revision, ID, clicked.clone())
        .await
        .unwrap();
    assert_eq!(ready.entry.draft, clicked);
    assert!(ws
        .prepare(&receipts, ready.entry.revision, ID, draft("WRONG SNAPSHOT"))
        .await
        .unwrap_err()
        .contains("WORKSPACE_ID"));
}

#[tokio::test]
async fn later_saved_fields_survive_the_original_snapshot_settlement() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("SUBMITTED ORIGINAL");
    let s = ws.save(0, a.clone()).await.unwrap();
    let p = ws
        .prepare(&receipts, s.entry.revision, ID, a)
        .await
        .unwrap();
    let mut b = draft("NEXT UNSENT INPUT");
    b.title = "新题".into();
    b.tags_input = "next".into();
    ws.save(p.entry.revision, b.clone()).await.unwrap();
    let done = ws.submit(&receipts, ID).await.unwrap();
    assert_eq!(done.entry.draft, b);
    assert!(done.entry.pending.is_none());
    let path = done.submission.unwrap().result.unwrap().path;
    let text = fs::read_to_string(path).unwrap();
    assert!(text.contains("SUBMITTED ORIGINAL"));
    assert!(!text.contains("NEXT UNSENT INPUT"));
    assert_eq!(notes(&root.path().join("Wiki")).len(), 1);
}

#[tokio::test]
async fn successful_note_with_failed_source_ack_resolves_original_and_preserves_next_input() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("SYNTHETIC ORIGINAL");
    let s = ws.save(0, a.clone()).await.unwrap();
    ws.prepare(&receipts, s.entry.revision, ID, a)
        .await
        .unwrap();
    ws.fail_publish.store(true, Ordering::SeqCst);
    assert!(ws
        .submit(&receipts, ID)
        .await
        .unwrap_err()
        .contains("WORKSPACE_SAVE"));
    assert_eq!(notes(&root.path().join("Wiki")).len(), 1);
    let (ws2, r2) = setup(root.path());
    let loaded = ws2.get(&r2).unwrap();
    assert!(loaded.entry.pending.is_some());
    assert_eq!(
        loaded.submission.unwrap().state,
        InboxSubmissionState::Completed
    );
    let mut next = draft("NEXT PRIVATE DRAFT🙂");
    next.title = "NEXT TITLE".into();
    next.tags_input = "next-tag".into();
    ws2.save(loaded.entry.revision, next.clone()).await.unwrap();
    let settled = ws2.resolve(&r2, ID).await.unwrap();
    assert_eq!(settled.entry.draft, next);
    assert!(settled.entry.pending.is_none());
    assert_eq!(notes(&root.path().join("Wiki")).len(), 1);
    assert_eq!(ws2.resolve(&r2, ID).await.unwrap().entry, settled.entry);
}

#[tokio::test]
async fn failed_source_pointer_never_authorizes_commit_and_can_recover_the_same_prepared_id() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("PREPARE ONLY");
    let s = ws.save(0, a.clone()).await.unwrap();
    ws.fail_ready_publish.store(true, Ordering::SeqCst);
    assert!(ws
        .prepare(&receipts, s.entry.revision, ID, a.clone())
        .await
        .is_err());
    assert_eq!(
        receipts.get(ID).unwrap().state,
        InboxSubmissionState::Prepared
    );
    assert_eq!(
        ws.get(&receipts).unwrap().entry.pending.unwrap().stage,
        InboxWorkspaceStage::Reserved
    );
    assert!(ws
        .submit(&receipts, ID)
        .await
        .unwrap_err()
        .contains("NOT_READY"));
    assert!(notes(&root.path().join("Wiki")).is_empty());
    let (ws2, r2) = setup(root.path());
    ws2.prepare(&r2, s.entry.revision, ID, a).await.unwrap();
    ws2.submit(&r2, ID).await.unwrap();
    assert_eq!(notes(&root.path().join("Wiki")).len(), 1);
}

#[tokio::test]
async fn cancellation_preserves_source_and_corrupt_source_never_becomes_empty() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("PRESERVE SOURCE");
    let s = ws.save(0, a.clone()).await.unwrap();
    ws.prepare(&receipts, s.entry.revision, ID, a.clone())
        .await
        .unwrap();
    let resolved = ws.cancel(&receipts, ID).await.unwrap();
    assert_eq!(resolved.entry.draft, a);
    assert!(resolved.entry.pending.is_none());
    fs::write(ws.path(), b"BROKEN SOURCE CIPHER").unwrap();
    assert!(ws.get(&receipts).unwrap_err().contains("WORKSPACE_READ"));
    assert!(ws.save(0, Default::default()).await.is_err());
    assert_eq!(fs::read(ws.path()).unwrap(), b"BROKEN SOURCE CIPHER");
    assert!(notes(&root.path().join("Wiki")).is_empty());
}

#[tokio::test]
async fn failed_initial_reservation_keeps_original_source_and_creates_no_receipt() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("ORIGINAL SOURCE");
    let s = ws.save(0, a.clone()).await.unwrap();
    ws.fail_publish.store(true, Ordering::SeqCst);
    assert!(ws
        .prepare(&receipts, s.entry.revision, ID, draft("CLICK SNAPSHOT"))
        .await
        .is_err());
    assert_eq!(ws.get(&receipts).unwrap().entry.draft, a);
    assert!(ws.get(&receipts).unwrap().entry.pending.is_none());
    assert_eq!(
        receipts.get(ID).unwrap().state,
        InboxSubmissionState::NotFound
    );
}

#[tokio::test]
async fn mismatched_receipt_never_clears_source_or_authorizes_publish_or_cancels_other_intent() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("SOURCE A");
    ws.prepare(&receipts, 0, ID, a.clone()).await.unwrap();
    // Simulate an identity collision after a reservation: only private test data.
    let mut entry = ws.load().unwrap();
    entry.pending.as_mut().unwrap().stage = InboxWorkspaceStage::Reserved;
    ws.publish(&entry).await.unwrap();
    fs::remove_file(
        root.path()
            .join(format!("private/receipts/{ID}.receipt.dpapi")),
    )
    .unwrap();
    let b = draft("OTHER INTENT B");
    receipts.prepare(ID, b.payload(ID)).await.unwrap();
    let observed = ws.get(&receipts).unwrap();
    assert_eq!(observed.observation, InboxWorkspaceObservation::Failed);
    assert!(observed.observation_error.unwrap().contains("CONFLICT"));
    assert!(ws
        .resolve(&receipts, ID)
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    assert!(receipts
        .commit_for(ID, &a.payload(ID))
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    assert!(receipts
        .cancel_for(ID, &a.payload(ID))
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    assert!(receipts
        .resolve_for(ID, &a.payload(ID))
        .await
        .unwrap_err()
        .contains("CONFLICT"));
    let canceled = ws.cancel(&receipts, ID).await.unwrap();
    assert_eq!(canceled.entry.draft, a);
    assert!(canceled.entry.pending.is_none());
    assert_eq!(
        receipts.get(ID).unwrap().state,
        InboxSubmissionState::Prepared
    );
    assert!(notes(&root.path().join("Wiki")).is_empty());
}

#[tokio::test]
async fn ready_missing_receipt_never_recreates_it_and_reserved_retry_preserves_later_edits() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("A");
    ws.fail_ready_publish.store(true, Ordering::SeqCst);
    assert!(ws.prepare(&receipts, 0, ID, a.clone()).await.is_err());
    let (ws, r) = setup(root.path());
    let saved = ws.get(&r).unwrap();
    let b = draft("B");
    ws.save(saved.entry.revision, b.clone()).await.unwrap();
    let ready = ws.prepare(&r, 0, ID, a.clone()).await.unwrap();
    assert_eq!(ready.entry.draft, b);
    fs::remove_file(
        root.path()
            .join(format!("private/receipts/{ID}.receipt.dpapi")),
    )
    .unwrap();
    assert_eq!(
        ws.prepare(&r, ready.entry.revision, ID, a)
            .await
            .unwrap()
            .submission
            .unwrap()
            .state,
        InboxSubmissionState::NotFound
    );
    assert_eq!(ws.resolve(&r, ID).await.unwrap().entry.draft, b);
    assert!(ws.submit(&r, ID).await.unwrap_err().contains("NOT_FOUND"));
    assert!(ws.cancel(&r, ID).await.is_err());
    assert!(notes(&root.path().join("Wiki")).is_empty());
}

#[tokio::test]
async fn saving_new_input_does_not_query_broken_historical_receipt_and_get_reports_the_failure() {
    let root = tempfile::tempdir().unwrap();
    let (ws, receipts) = setup(root.path());
    let a = draft("OLD");
    let s = ws.save(0, a.clone()).await.unwrap();
    ws.prepare(&receipts, s.entry.revision, ID, a)
        .await
        .unwrap();
    let done = ws.submit(&receipts, ID).await.unwrap();
    fs::write(
        root.path()
            .join(format!("private/receipts/{ID}.receipt.dpapi")),
        b"BROKEN HISTORY CIPHER",
    )
    .unwrap();
    let b = draft("NEW INPUT MUST SAVE");
    let saved = ws.save(done.entry.revision, b.clone()).await.unwrap();
    assert_eq!(saved.observation, InboxWorkspaceObservation::NotChecked);
    assert!(saved.submission.is_none());
    let get = ws.get(&receipts).unwrap();
    assert_eq!(get.entry.draft, b);
    assert_eq!(get.observation, InboxWorkspaceObservation::Failed);
    assert!(get.observation_error.unwrap().contains("SUBMISSION_READ"));
    assert_eq!(get.entry.last_submission_id.as_deref(), Some(ID));
}
