//! Test-only executable, requires an explicitly named fresh crash lab.
use homebase_control_tests::{
    assistant_documents::{Begin, DocumentStore},
    document_runtime,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
const ID: &str = "75c8874c-109f-4c59-a445-ad1c7077eb0b";
#[tokio::main]
async fn main() {
    let home = PathBuf::from(
        std::env::var_os("HOMEBASE_DOCUMENT_CRASH_HOME").expect("Explicit crash home required"),
    );
    assert!(
        home.is_absolute()
            && home
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("document-crash-")
    );
    let store = DocumentStore::new(home.join("private-documents"));
    if std::env::args().any(|a| a == "--inspect") {
        let entry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(home.join("started.json")).unwrap()).unwrap();
        let access = store.access(ID, "new", false, false).unwrap();
        let summary = store
            .summary(&access, entry["extraction_id"].as_str().unwrap())
            .unwrap();
        println!("{}", serde_json::to_string(&summary).unwrap());
        return;
    }
    let runtime = home.join("runtime");
    assert!(runtime.join("worker.mjs").is_file());
    let node =
        PathBuf::from(std::env::var_os("HOMEBASE_DOCUMENT_NODE").expect("Explicit Node required"));
    assert!(node.is_absolute());
    store
        .stage(
            ID,
            "new",
            "synthetic.pdf",
            b"%PDF-1.7\nsynthetic-crash-source",
        )
        .await
        .unwrap();
    let access = store.access(ID, "new", false, false).unwrap();
    let Begin::Run(lease) = store.begin(&access, None, false).await.unwrap() else {
        panic!("Fresh lease required")
    };
    std::fs::write(
        home.join("started.json"),
        serde_json::to_vec(
            &serde_json::json!({"document_id":ID,"extraction_id":lease.receipt().id}),
        )
        .unwrap(),
    )
    .unwrap();
    let result = document_runtime::run(
        &runtime,
        &node,
        &home,
        &serde_json::json!({}),
        Arc::default(),
        Duration::from_secs(60),
    )
    .await;
    drop(lease);
    println!("{}", result.is_ok());
}
