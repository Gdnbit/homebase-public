use homebase_control_tests::{
    assistant_documents::{DocumentStore, Phase},
    document_service::DocumentService,
};
use std::{path::PathBuf, sync::Arc};
fn context() -> (PathBuf, PathBuf) {
    let lab = PathBuf::from(
        std::env::var_os("HOMEBASE_DOCUMENT_NATIVE_LAB")
            .expect("Explicit isolated document lab required"),
    );
    let runtime = PathBuf::from(
        std::env::var_os("HOMEBASE_DOCUMENT_RUNTIME")
            .expect("Explicit fixed document runtime required"),
    );
    assert!(lab.is_absolute() && runtime.is_absolute());
    let root = lab.join(format!("document-service-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    (root, runtime)
}
#[tokio::test]
#[ignore = "explicit isolated lab, fixed package and synthetic PDF required"]
async fn document_service_real_parser_caches_restarts_and_reports_missing_runtime_without_launching(
) {
    let (root, runtime) = context();
    let bytes =
        std::fs::read(std::env::var_os("HOMEBASE_DOCUMENT_FIXTURE").expect("Synthetic PDF only"))
            .unwrap();
    let store = Arc::new(DocumentStore::new(root.join("d")));
    let id = uuid::Uuid::new_v4().to_string();
    store
        .stage(&id, "new", "synthetic.pdf", &bytes)
        .await
        .unwrap();
    let access = store.access(&id, "new", false, false).unwrap();
    let missing = Arc::new(DocumentService::new(store.clone(), root.join("missing")));
    assert_eq!(
        missing
            .extract(access.clone(), None, false)
            .await
            .unwrap()
            .status,
        "runtime_unavailable"
    );
    assert!(store.extraction_index(&access).unwrap().is_empty());
    let service = Arc::new(DocumentService::new(store.clone(), runtime));
    let started = service.extract(access.clone(), None, false).await.unwrap();
    assert_eq!(started.status, "started");
    let key = started.extraction.unwrap().id;
    let duplicate = service.extract(access.clone(), None, false).await.unwrap();
    assert_eq!(duplicate.status, "cached");
    assert_eq!(duplicate.extraction.unwrap().id, key);
    let result = service.wait(&access, &key).await.unwrap();
    assert_eq!(result.phase, Phase::Ready);
    let attempt = result.attempt;
    let restarted = Arc::new(DocumentService::new(
        store.clone(),
        root.join("still-missing"),
    ));
    let cached = restarted.extract(access.clone(), None, true).await.unwrap();
    assert_eq!(cached.status, "cached");
    assert_eq!(cached.extraction.unwrap().attempt, attempt);
    assert_eq!(store.extraction_index(&access).unwrap().len(), 1);
}
#[tokio::test]
#[ignore = "explicit isolated lab and fixed Node package required"]
async fn document_service_capacity_cancel_and_other_process_ownership_are_explicit() {
    let (root, runtime) = context();
    let fake = root.join("r");
    std::fs::create_dir_all(&fake).unwrap();
    std::fs::copy(runtime.join("node.exe"), fake.join("node.exe")).unwrap();
    std::fs::write(
        fake.join("worker.mjs"),
        "for await (const _ of process.stdin) {} setInterval(()=>{},1000);",
    )
    .unwrap();
    let store = Arc::new(DocumentStore::new(root.join("d")));
    let service = Arc::new(DocumentService::new(store.clone(), fake.clone()));
    let other = Arc::new(DocumentService::new(store.clone(), fake));
    let mut runs = vec![];
    for _ in 0..2 {
        let id = uuid::Uuid::new_v4().to_string();
        store
            .stage(&id, "new", "synthetic.pdf", b"synthetic bytes")
            .await
            .unwrap();
        let access = store.access(&id, "new", false, false).unwrap();
        let start = service.extract(access.clone(), None, false).await.unwrap();
        assert_eq!(start.status, "started");
        runs.push((access, start.extraction.unwrap().id));
    }
    let (access, key) = &runs[0];
    assert_eq!(
        other.cancel(access, key).await.unwrap().status,
        "not_running_here"
    );
    assert_eq!(
        service
            .extract(access.clone(), None, false)
            .await
            .unwrap()
            .status,
        "cached"
    );
    let id = uuid::Uuid::new_v4().to_string();
    store
        .stage(&id, "new", "third.pdf", b"third source")
        .await
        .unwrap();
    let third = store.access(&id, "new", false, false).unwrap();
    assert_eq!(
        service
            .extract(third.clone(), None, false)
            .await
            .unwrap()
            .status,
        "capacity"
    );
    assert!(store.extraction_index(&third).unwrap().is_empty());
    for (access, key) in runs {
        let cancel = service.cancel(&access, &key).await.unwrap();
        assert_eq!(cancel.status, "cancel_requested");
        let done = service.wait(&access, &key).await.unwrap();
        assert_eq!(done.phase, Phase::Cancelled);
        let duplicate = service.extract(access, None, false).await.unwrap();
        assert_eq!(duplicate.status, "cached");
    }
}
