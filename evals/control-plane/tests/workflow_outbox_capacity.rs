#![cfg(windows)]
use homebase_control_tests::assistant_desktop::{DesktopBody, DesktopStore};
use sha2::{Digest, Sha256};

fn body(text: &str) -> DesktopBody {
    DesktopBody {
        image: None,
        text: text.into(),
        ..DesktopBody::default()
    }
}

#[tokio::test]
async fn completed_handoff_copies_do_not_exhaust_active_draft_capacity() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("desktop");
    let store = DesktopStore::new(directory.clone());
    let original = store
        .save("new", 0, body("原有待发送正文保持"))
        .await
        .unwrap();
    let mut completed = Vec::new();
    for _ in 0..205 {
        let scope = format!("handoff:{}", uuid::Uuid::new_v4());
        let draft = store
            .save(&scope, 0, body("仅合成已完成交接"))
            .await
            .unwrap();
        let cleared = store
            .save(&scope, draft.revision, DesktopBody::default())
            .await
            .unwrap();
        completed.push((scope, cleared.revision));
    }
    let recovery = format!("handoff:{}", uuid::Uuid::new_v4());
    store
        .save(&recovery, 0, body("尚未转入，必须保留可恢复"))
        .await
        .unwrap();
    let restarted = DesktopStore::new(directory);
    assert_eq!(restarted.get("new").unwrap().body.text, original.body.text);
    assert_eq!(restarted.get("new").unwrap().revision, original.revision);
    assert_eq!(
        restarted.get(&recovery).unwrap().body.text,
        "尚未转入，必须保留可恢复"
    );
    let index = restarted.index().unwrap();
    assert!(index.complete);
    assert!(index
        .entries
        .iter()
        .any(|entry| entry.scope == recovery && entry.has_draft));
    for (scope, revision) in completed {
        let entry = restarted.get(&scope).unwrap();
        assert_eq!(entry.revision, revision);
        assert!(entry.body.text.is_empty() && entry.pending.is_none());
    }
}

#[tokio::test]
async fn completed_handoff_reactivation_never_reuses_a_stale_revision() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("desktop");
    let scope = format!("handoff:{}", uuid::Uuid::new_v4());
    let store = DesktopStore::new(directory.clone());
    let original = store.save(&scope, 0, body("第一份合成输入")).await.unwrap();
    let cleared = store
        .save(&scope, original.revision, DesktopBody::default())
        .await
        .unwrap();
    let restarted = DesktopStore::new(directory);
    assert_eq!(restarted.get(&scope).unwrap().revision, cleared.revision);
    let current = restarted
        .save(&scope, cleared.revision, body("新的合成输入"))
        .await
        .unwrap();
    assert!(current.revision > cleared.revision);
    assert!(restarted
        .save(&scope, original.revision, body("过时覆盖"))
        .await
        .is_err());
    assert!(restarted
        .save(&scope, cleared.revision, DesktopBody::default())
        .await
        .is_err());
    assert_eq!(restarted.get(&scope).unwrap().body.text, "新的合成输入");
}

#[tokio::test]
async fn unfinished_drafts_still_have_the_original_capacity_guard() {
    let root = tempfile::tempdir().unwrap();
    let store = DesktopStore::new(root.path().join("desktop"));
    for n in 0..200 {
        store
            .save(
                &format!("native:codex:active_{n}"),
                0,
                body("未完成输入，不可移走"),
            )
            .await
            .unwrap();
    }
    let scope = format!("handoff:{}", uuid::Uuid::new_v4());
    assert!(store
        .save(&scope, 0, body("超过活跃保护容量"))
        .await
        .is_err());
    assert_eq!(
        store.get("native:codex:active_0").unwrap().body.text,
        "未完成输入，不可移走"
    );
    let index = store.index().unwrap();
    assert!(index.complete);
    assert_eq!(index.entries.len(), 200);
}

#[tokio::test]
async fn corrupt_completed_or_active_cipher_never_resets_or_falls_back() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("desktop");
    let store = DesktopStore::new(directory.clone());
    let scope = format!("handoff:{}", uuid::Uuid::new_v4());
    let first = store.save(&scope, 0, body("合成原输入")).await.unwrap();
    let cleared = store
        .save(&scope, first.revision, DesktopBody::default())
        .await
        .unwrap();
    let filename = format!("{:x}.draft.dpapi", Sha256::digest(scope.as_bytes()));
    let completed = directory.join("completed-handoffs").join(&filename);
    let original_completed = std::fs::read(&completed).unwrap();
    std::fs::write(&completed, b"SYNTHETIC-CORRUPT-COMPLETED").unwrap();
    assert!(store.get(&scope).is_err());
    assert!(store.save(&scope, 0, body("不能回退版本0")).await.is_err());
    assert_eq!(
        std::fs::read(&completed).unwrap(),
        b"SYNTHETIC-CORRUPT-COMPLETED"
    );
    std::fs::write(&completed, &original_completed).unwrap();
    let active = store
        .save(&scope, cleared.revision, body("当前合成输入"))
        .await
        .unwrap();
    let active_path = directory.join(filename);
    std::fs::write(&active_path, b"SYNTHETIC-CORRUPT-ACTIVE").unwrap();
    assert!(store.get(&scope).is_err());
    assert!(store
        .save(&scope, active.revision, body("不能改读旧完成记录"))
        .await
        .is_err());
    assert_eq!(
        std::fs::read(active_path).unwrap(),
        b"SYNTHETIC-CORRUPT-ACTIVE"
    );
    assert_eq!(std::fs::read(completed).unwrap(), original_completed);
}

#[tokio::test]
async fn completed_area_failure_keeps_files_and_can_finish_after_explicit_repair() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("desktop");
    let store = DesktopStore::new(directory.clone());
    store.save("new", 0, body("原有普通草稿")).await.unwrap();
    let scope = format!("handoff:{}", uuid::Uuid::new_v4());
    let first = store
        .save(&scope, 0, body("目标已另行保存的合成副本"))
        .await
        .unwrap();
    let obstruction = directory.join("completed-handoffs");
    std::fs::write(&obstruction, b"SYNTHETIC-OBSTRUCTION").unwrap();
    assert!(store
        .save(&scope, first.revision, DesktopBody::default())
        .await
        .is_err());
    assert!(store.get(&scope).is_err());
    assert_eq!(
        std::fs::read(&obstruction).unwrap(),
        b"SYNTHETIC-OBSTRUCTION"
    );
    assert_eq!(store.get("new").unwrap().body.text, "原有普通草稿");
    std::fs::rename(&obstruction, directory.join("retained-obstruction")).unwrap();
    let preserved = store.get(&scope).unwrap();
    assert_eq!(preserved.body.text, first.body.text);
    assert_eq!(preserved.revision, first.revision);
    let retry = store
        .save(&scope, preserved.revision, DesktopBody::default())
        .await
        .unwrap();
    assert!(retry.revision > preserved.revision);
    assert_eq!(
        DesktopStore::new(directory).get(&scope).unwrap().revision,
        retry.revision
    );
}

#[tokio::test]
async fn restored_stale_active_cipher_cannot_override_a_later_completed_revision() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("desktop");
    let store = DesktopStore::new(directory.clone());
    let scope = format!("handoff:{}", uuid::Uuid::new_v4());
    let first = store.save(&scope, 0, body("旧备份合成正文")).await.unwrap();
    let filename = format!("{:x}.draft.dpapi", Sha256::digest(scope.as_bytes()));
    let active_path = directory.join(&filename);
    let old_cipher = std::fs::read(&active_path).unwrap();
    store
        .save(&scope, first.revision, DesktopBody::default())
        .await
        .unwrap();
    let completed_path = directory.join("completed-handoffs").join(filename);
    let completed_cipher = std::fs::read(&completed_path).unwrap();
    std::fs::write(&active_path, &old_cipher).unwrap();
    assert!(store.get(&scope).is_err());
    assert!(store
        .save(&scope, first.revision, body("过时客户端新写入"))
        .await
        .is_err());
    assert_eq!(std::fs::read(active_path).unwrap(), old_cipher);
    assert_eq!(std::fs::read(completed_path).unwrap(), completed_cipher);
}

#[tokio::test]
async fn a_locked_completed_destination_keeps_the_committed_empty_active_copy() {
    use std::os::windows::fs::OpenOptionsExt;
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("desktop");
    let store = DesktopStore::new(directory.clone());
    let scope = format!("handoff:{}", uuid::Uuid::new_v4());
    let first = store.save(&scope, 0, body("合成初稿")).await.unwrap();
    let completed = store
        .save(&scope, first.revision, DesktopBody::default())
        .await
        .unwrap();
    let current = store
        .save(&scope, completed.revision, body("合成新稿"))
        .await
        .unwrap();
    let filename = format!("{:x}.draft.dpapi", Sha256::digest(scope.as_bytes()));
    let completed_path = directory.join("completed-handoffs").join(filename);
    let previous_cipher = std::fs::read(&completed_path).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&completed_path)
        .unwrap();
    assert!(store
        .save(&scope, current.revision, DesktopBody::default())
        .await
        .is_err());
    let cleared = store.get(&scope).unwrap();
    assert!(cleared.body.text.is_empty());
    assert!(cleared.revision > current.revision);
    assert_eq!(std::fs::read(&completed_path).unwrap(), previous_cipher);
    drop(lock);
    let finished = store
        .save(&scope, cleared.revision, DesktopBody::default())
        .await
        .unwrap();
    assert_eq!(finished.revision, cleared.revision);
    assert_eq!(
        DesktopStore::new(directory).get(&scope).unwrap().revision,
        cleared.revision
    );
}

#[tokio::test]
async fn concurrent_reactivation_has_one_winner_without_version_reuse() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("desktop");
    let one = DesktopStore::new(directory.clone());
    let two = DesktopStore::new(directory);
    let scope = format!("handoff:{}", uuid::Uuid::new_v4());
    let first = one.save(&scope, 0, body("合成起点")).await.unwrap();
    let cleared = one
        .save(&scope, first.revision, DesktopBody::default())
        .await
        .unwrap();
    let (left, right) = tokio::join!(
        one.save(&scope, cleared.revision, body("左侧新输入")),
        two.save(&scope, cleared.revision, body("右侧新输入")),
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let saved = left.or(right).unwrap();
    assert!(saved.revision > cleared.revision);
    assert_eq!(one.get(&scope).unwrap().body.text, saved.body.text);
}
