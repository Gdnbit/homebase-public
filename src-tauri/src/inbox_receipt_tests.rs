use super::*;

#[cfg(windows)]
#[tokio::test]
async fn equivalent_windows_root_separators_preserve_the_original_receipt_and_result() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("private");
    let vault = root.path().join("Wiki");
    let store = ReceiptStore::new(directory.clone(), vault.clone());
    let id = "87562a38-8fb4-4276-bdda-bd0d85bef544";
    let payload = InboxSubmitPayload {
        text: "SYNTHETIC ORIGINAL PATH🙂".into(),
        submission_id: Some(id.into()),
        ..Default::default()
    };
    let completed = store.submit(payload.clone()).await.unwrap();
    let cipher = fs::read(store.path(id).unwrap()).unwrap();
    let alias = PathBuf::from(vault.to_string_lossy().replace('\\', "/"));
    assert_ne!(alias.to_string_lossy(), vault.to_string_lossy());
    let restored = ReceiptStore::new(directory, alias);
    assert_eq!(restored.get(id).unwrap(), completed);
    assert_eq!(restored.submit(payload).await.unwrap(), completed);
    assert_eq!(fs::read(restored.path(id).unwrap()).unwrap(), cipher);
}
use std::sync::{atomic::Ordering, Arc};

const ID: &str = "7b7b0b2d-2232-4d2d-9ffd-97ca04ed9a90";
fn payload() -> InboxSubmitPayload {
    InboxSubmitPayload {
        text: "PRIVATE SYNTHETIC SOURCE🙂".into(),
        submission_id: Some(ID.into()),
        ..Default::default()
    }
}
fn store(root: &Path) -> ReceiptStore {
    ReceiptStore::new(root.join("private/receipts"), root.join("Wiki"))
}
fn files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for e in entries {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                found.extend(files(&e.path()));
            } else if e.path().extension().is_some_and(|v| v == "md") {
                found.push(e.path());
            }
        }
    }
    found
}

#[tokio::test]
async fn identical_intent_reuses_one_note_after_store_restart_and_never_stores_plaintext_receipt() {
    let root = tempfile::tempdir().unwrap();
    let first = store(root.path()).submit(payload()).await.unwrap();
    assert_eq!(first.state, InboxSubmissionState::Completed);
    assert_eq!(
        confirmed_result(first.clone()).unwrap(),
        first.result.clone().unwrap()
    );
    assert_eq!(first.file_state, Some(InboxSubmissionFileState::Present));
    let restarted = store(root.path());
    let second = restarted.submit(payload()).await.unwrap();
    assert_eq!(second.result, first.result);
    assert_eq!(files(&root.path().join("Wiki")).len(), 1);
    let cipher = fs::read(restarted.path(ID).unwrap()).unwrap();
    assert!(!cipher
        .windows(b"PRIVATE SYNTHETIC SOURCE".len())
        .any(|v| v == b"PRIVATE SYNTHETIC SOURCE"));
    let bytes = fs::read(first.result.unwrap().path).unwrap();
    assert!(String::from_utf8(bytes)
        .unwrap()
        .contains("PRIVATE SYNTHETIC SOURCE🙂"));
}

#[tokio::test]
async fn acknowledgment_lost_after_actual_publication_resolves_without_publishing_again() {
    for fault in [4, 5] {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        s.fault.store(fault, Ordering::SeqCst);
        assert!(s.submit(payload()).await.is_err());
        let original = files(&root.path().join("Wiki"));
        assert_eq!(original.len(), 1);
        let before = fs::read(&original[0]).unwrap();
        let receipt_before = fs::read(s.path(ID).unwrap()).unwrap();
        let restarted = store(root.path());
        let observed = restarted.get(ID).unwrap();
        assert_eq!(observed.state, InboxSubmissionState::Completed);
        assert_eq!(
            fs::read(s.path(ID).unwrap()).unwrap(),
            receipt_before,
            "observation must not mutate receipts"
        );
        restarted.submit(payload()).await.unwrap();
        assert_eq!(files(&root.path().join("Wiki")), original);
        assert_eq!(fs::read(&original[0]).unwrap(), before);
    }
}

#[tokio::test]
async fn unknown_after_boundary_with_missing_target_cannot_be_retried_or_canceled() {
    let root = tempfile::tempdir().unwrap();
    let s = store(root.path());
    s.fault.store(3, Ordering::SeqCst);
    assert!(s.submit(payload()).await.is_err());
    assert!(files(&root.path().join("Wiki")).is_empty());
    let original = fs::read(s.path(ID).unwrap()).unwrap();
    let restarted = store(root.path());
    assert_eq!(
        restarted.get(ID).unwrap().state,
        InboxSubmissionState::Unknown
    );
    assert_eq!(
        restarted.submit(payload()).await.unwrap().state,
        InboxSubmissionState::Unknown
    );
    assert!(restarted
        .cancel(ID)
        .await
        .unwrap_err()
        .contains("CANCEL_BLOCKED"));
    assert!(files(&root.path().join("Wiki")).is_empty());
    assert_eq!(fs::read(s.path(ID).unwrap()).unwrap(), original);
}

#[tokio::test]
async fn failure_before_boundary_can_only_continue_the_same_prepared_intent() {
    let root = tempfile::tempdir().unwrap();
    let s = store(root.path());
    s.fault.store(2, Ordering::SeqCst);
    assert!(s.submit(payload()).await.is_err());
    assert_eq!(s.get(ID).unwrap().state, InboxSubmissionState::Prepared);
    assert!(files(&root.path().join("Wiki")).is_empty());
    let restarted = store(root.path());
    let completed = restarted.commit(ID).await.unwrap();
    assert_eq!(completed.state, InboxSubmissionState::Completed);
    assert_eq!(files(&root.path().join("Wiki")).len(), 1);
}

#[tokio::test]
async fn prepared_intent_cancels_without_deleting_or_publishing_any_note() {
    let root = tempfile::tempdir().unwrap();
    let s = store(root.path());
    s.prepare(ID, payload()).await.unwrap();
    let canceled = s.cancel(ID).await.unwrap();
    assert_eq!(canceled.state, InboxSubmissionState::Canceled);
    assert_eq!(
        store(root.path()).submit(payload()).await.unwrap().state,
        InboxSubmissionState::Canceled
    );
    assert!(files(&root.path().join("Wiki")).is_empty());
}

#[tokio::test]
async fn same_id_different_content_and_changed_root_are_conflicts_before_any_write() {
    let root = tempfile::tempdir().unwrap();
    let s = store(root.path());
    s.prepare(ID, payload()).await.unwrap();
    let before = fs::read(s.path(ID).unwrap()).unwrap();
    let mut changed = payload();
    changed.text.push_str(" CHANGED");
    assert!(s.submit(changed).await.unwrap_err().contains("CONFLICT"));
    assert_eq!(fs::read(s.path(ID).unwrap()).unwrap(), before);
    let other = ReceiptStore::new(
        root.path().join("private/receipts"),
        root.path().join("OTHER"),
    );
    assert!(other.get(ID).unwrap_err().contains("SOURCE_CHANGED"));
    assert!(!root.path().join("OTHER").exists());
}

#[tokio::test]
async fn completed_files_deleted_or_modified_are_never_recreated_or_overwritten() {
    for remove in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let s = store(root.path());
        let first = s.submit(payload()).await.unwrap();
        let note = first.result.unwrap().path;
        if remove {
            fs::remove_file(&note).unwrap();
        } else {
            fs::write(&note, b"NEW USER EDIT").unwrap();
        }
        let before = fs::read(s.path(ID).unwrap()).unwrap();
        let restarted = store(root.path());
        let view = restarted.submit(payload()).await.unwrap();
        assert_eq!(view.state, InboxSubmissionState::Completed);
        let code = confirmed_result(view.clone()).unwrap_err();
        assert!(code.contains(if remove {
            "FILE_MISSING"
        } else {
            "FILE_CHANGED"
        }));
        assert_eq!(
            view.file_state,
            Some(if remove {
                InboxSubmissionFileState::Missing
            } else {
                InboxSubmissionFileState::Modified
            })
        );
        if remove {
            assert!(!Path::new(&note).exists());
        } else {
            assert_eq!(fs::read(&note).unwrap(), b"NEW USER EDIT");
        }
        assert_eq!(fs::read(s.path(ID).unwrap()).unwrap(), before);
    }
}

#[tokio::test]
async fn broken_cipher_is_not_empty_state_and_cannot_start_another_publication() {
    let root = tempfile::tempdir().unwrap();
    let s = store(root.path());
    s.prepare(ID, payload()).await.unwrap();
    fs::write(s.path(ID).unwrap(), b"BROKEN SYNTHETIC CIPHER").unwrap();
    assert!(s.get(ID).unwrap_err().contains("SUBMISSION_READ"));
    assert!(s
        .submit(payload())
        .await
        .unwrap_err()
        .contains("SUBMISSION_READ"));
    assert_eq!(
        fs::read(s.path(ID).unwrap()).unwrap(),
        b"BROKEN SYNTHETIC CIPHER"
    );
    assert!(files(&root.path().join("Wiki")).is_empty());
}

#[tokio::test]
async fn failed_preparation_starts_no_business_write_and_keeps_existing_data() {
    let root = tempfile::tempdir().unwrap();
    let s = store(root.path());
    s.fault.store(1, Ordering::SeqCst);
    assert!(s.submit(payload()).await.is_err());
    assert_eq!(s.get(ID).unwrap().state, InboxSubmissionState::NotFound);
    assert!(files(&root.path().join("Wiki")).is_empty());
}

#[tokio::test]
async fn concurrent_stores_share_an_exclusive_boundary_for_one_intent() {
    let root = tempfile::tempdir().unwrap();
    let a = Arc::new(store(root.path()));
    let b = Arc::new(store(root.path()));
    a.prepare(ID, payload()).await.unwrap();
    let (a_result, b_result) = tokio::join!(a.commit(ID), b.commit(ID));
    assert!(
        a_result
            .as_ref()
            .is_ok_and(|v| v.state == InboxSubmissionState::Completed)
            || b_result
                .as_ref()
                .is_ok_and(|v| v.state == InboxSubmissionState::Completed)
    );
    assert_eq!(
        store(root.path()).submit(payload()).await.unwrap().state,
        InboxSubmissionState::Completed
    );
    assert_eq!(files(&root.path().join("Wiki")).len(), 1);
}

#[test]
fn public_error_messages_never_echo_untrusted_private_values() {
    for code in [
        "INBOX_SUBMISSION_UNCONFIRMED",
        "INBOX_SUBMISSION_READ",
        "INBOX_SUBMISSION_CONFLICT",
    ] {
        assert!(public_error(code).is_some());
        assert!(public_error(&format!("{code}: PRIVATE_MARKER")).is_none());
        assert!(public_error(&error(code)).is_some());
    }
}

#[tokio::test]
async fn controller_binds_an_omitted_business_id_to_the_durable_original_job_and_keeps_explicit_ids(
) {
    use crate::control_protocol::{ControlError, Request};
    use crate::control_state::{Controller, Runner};
    use serde_json::{json, Value};
    use std::sync::atomic::AtomicUsize;
    let root = tempfile::tempdir().unwrap();
    let receipts = Arc::new(store(root.path()));
    let count = Arc::new(AtomicUsize::new(0));
    let runner: Runner = Arc::new({
        let receipts = receipts.clone();
        let count = count.clone();
        move |name, args| {
            let receipts = receipts.clone();
            let count = count.clone();
            Box::pin(async move {
                assert_eq!(name, "inbox_submit");
                count.fetch_add(1, Ordering::SeqCst);
                let p: InboxSubmitPayload =
                    serde_json::from_value(args["payload"].clone()).unwrap();
                let view = receipts
                    .submit(p)
                    .await
                    .map_err(|_| ControlError::new("SYNTHETIC_FAILURE", "receipt failed"))?;
                Ok(serde_json::to_value(confirmed_result(view).unwrap()).unwrap())
            })
        }
    });
    let catalog = json!({"catalog_hash":"inbox-bridge","commands":[{"name":"inbox_submit","status":"available","requires":["write"],"reason":null,"parameters":[{"name":"payload","rust_name":"payload","optional":false}]}]});
    let control_dir = root.path().join("control");
    fs::create_dir(&control_dir).unwrap();
    let c = Controller::new(catalog.clone(), &control_dir, runner.clone()).unwrap();
    let req = |method: &str, patch: Value| -> Request {
        let mut v = json!({"protocol":1,"id":"synthetic","token":"unused","method":method,"catalog_hash":"inbox-bridge","request_id":"original-inbox-intent","grants":["write"],"args":{}});
        v.as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    };
    let args = json!({"payload":{"text":"CONTROL INTENT WITHOUT CLIENT BUSINESS ID"}});
    let accepted = c
        .clone()
        .handle(req("call", json!({"command":"inbox_submit","args":args})))
        .await
        .unwrap();
    let id = accepted["job"]["id"].as_str().unwrap();
    for _ in 0..1000 {
        let j = c
            .clone()
            .handle(req("jobs.get", json!({"job_id":id})))
            .await
            .unwrap();
        if j["status"] != "running" {
            assert_eq!(j["status"], "succeeded");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        receipts.get(id).unwrap().state,
        InboxSubmissionState::Completed
    );
    let restored = Controller::new(catalog, &control_dir, runner).unwrap();
    let reused = restored
        .clone()
        .handle(req("call", json!({"command":"inbox_submit","args":args})))
        .await
        .unwrap();
    assert_eq!(reused["job"]["id"], id);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(files(&root.path().join("Wiki")).len(), 1);
    let accepted=restored.clone().handle(req("call",json!({"command":"inbox_submit","request_id":"different-explicit-intent","args":{"payload":{"text":"EXPLICIT BUSINESS ID", "submission_id":ID}}}))).await.unwrap();
    let explicit_job = accepted["job"]["id"].as_str().unwrap();
    for _ in 0..1000 {
        let j = restored
            .clone()
            .handle(req("jobs.get", json!({"job_id":explicit_job})))
            .await
            .unwrap();
        if j["status"] != "running" {
            assert_eq!(j["status"], "succeeded");
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_ne!(explicit_job, ID);
    assert_eq!(
        receipts.get(ID).unwrap().state,
        InboxSubmissionState::Completed
    );
    assert_eq!(
        receipts.get(explicit_job).unwrap().state,
        InboxSubmissionState::NotFound
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(files(&root.path().join("Wiki")).len(), 2);
}
