//! Explicitly opted-in local process tests; never use a real profile or model.
use homebase_control_tests::document_runtime::{self, Cancellation, RuntimeError};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn context() -> (PathBuf, PathBuf, PathBuf) {
    let root = PathBuf::from(
        std::env::var_os("HOMEBASE_DOCUMENT_NATIVE_LAB")
            .expect("Explicit isolated host lab required"),
    );
    let runtime = PathBuf::from(
        std::env::var_os("HOMEBASE_DOCUMENT_RUNTIME")
            .expect("Explicit fixed document runtime required"),
    );
    let node = PathBuf::from(
        std::env::var_os("HOMEBASE_DOCUMENT_NODE").expect("Explicit Node executable required"),
    );
    assert!(root.is_absolute() && runtime.is_absolute() && node.is_absolute());
    let home = root.join(format!("document-host-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    (home, runtime, node)
}
fn stub(home: &std::path::Path, text: &str) -> PathBuf {
    let dir = home.join("fixture-runtime");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("worker.mjs"), text).unwrap();
    dir
}

#[tokio::test]
#[ignore = "explicit document runtime, Node and isolated lab required"]
async fn document_host_real_parser_and_environment_are_bounded() {
    let (home, runtime, node) = context();
    let bytes = std::fs::read(
        std::env::var_os("HOMEBASE_DOCUMENT_FIXTURE").expect("Synthetic PDF fixture required"),
    )
    .unwrap();
    use base64::{engine::general_purpose::STANDARD, Engine};
    let result = document_runtime::run(
        &runtime,
        &node,
        &home,
        &json!({"version":1,"mime_type":"application/pdf","bytes_base64":STANDARD.encode(bytes)}),
        Arc::default(),
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert_eq!(result.status, "ready");
    assert_eq!(result.pages.unwrap()[0].text, "中文🙂");
    let runtime=stub(&home,"console.log(JSON.stringify({version:'homebase-document-v1',status:'failed',code:'TEST_ENV',message:process.env.HOMEBASE_SYNTHETIC_DOCUMENT_SECRET?'inherited':'clean'}));");
    std::env::set_var(
        "HOMEBASE_SYNTHETIC_DOCUMENT_SECRET",
        "synthetic-not-a-real-key",
    );
    let result = document_runtime::run(
        &runtime,
        &node,
        &home,
        &json!({}),
        Arc::default(),
        Duration::from_secs(5),
    )
    .await;
    std::env::remove_var("HOMEBASE_SYNTHETIC_DOCUMENT_SECRET");
    assert_eq!(result.unwrap().message, "clean");
}
#[tokio::test]
#[ignore = "explicit document runtime, Node and isolated lab required"]
async fn document_host_timeout_and_cancel_stop_the_owned_process_during_blocked_io() {
    let (home, _, node) = context();
    let runtime = stub(&home, "while(true) {};");
    let large = json!({"bytes":"x".repeat(2*1024*1024)});
    let started = Instant::now();
    let result = document_runtime::run(
        &runtime,
        &node,
        &home,
        &large,
        Arc::default(),
        Duration::from_millis(350),
    )
    .await;
    assert_eq!(result.unwrap_err(), RuntimeError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(5));
    let cancel = Arc::new(Cancellation::default());
    let signal = cancel.clone();
    let notify = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        signal.cancel();
        signal.cancel();
    });
    let started = Instant::now();
    let result = document_runtime::run(
        &runtime,
        &node,
        &home,
        &large,
        cancel,
        Duration::from_secs(20),
    )
    .await;
    notify.await.unwrap();
    assert_eq!(result.unwrap_err(), RuntimeError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    let cancelled = Arc::new(Cancellation::default());
    cancelled.cancel();
    assert_eq!(
        document_runtime::run(
            &home.join("missing"),
            &node,
            &home,
            &json!({}),
            cancelled,
            Duration::from_secs(5)
        )
        .await
        .unwrap_err(),
        RuntimeError::Cancelled
    );
}
#[tokio::test]
#[ignore = "explicit document runtime, Node and isolated lab required"]
async fn document_host_rejects_oversized_invalid_and_conflicting_output_without_waiting_for_a_hung_exit(
) {
    let (home, _, node) = context();
    let huge = json!({"bytes":"x".repeat(6*1024*1024)});
    assert_eq!(
        document_runtime::run(
            &home.join("missing"),
            &node,
            &home,
            &huge,
            Arc::default(),
            Duration::from_secs(5)
        )
        .await
        .unwrap_err(),
        RuntimeError::InputLimit
    );
    let runtime = stub(&home, "console.log('{}');");
    assert_eq!(
        document_runtime::run(
            &runtime,
            &node,
            &home,
            &json!({}),
            Arc::default(),
            Duration::from_secs(5)
        )
        .await
        .unwrap_err(),
        RuntimeError::InvalidOutput
    );
    std::fs::write(
        runtime.join("worker.mjs"),
        "process.stdout.write('x'.repeat(2*1024*1024+1));setInterval(()=>{},10000);",
    )
    .unwrap();
    let started = Instant::now();
    assert_eq!(
        document_runtime::run(
            &runtime,
            &node,
            &home,
            &json!({}),
            Arc::default(),
            Duration::from_secs(20)
        )
        .await
        .unwrap_err(),
        RuntimeError::OutputLimit
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    std::fs::write(runtime.join("worker.mjs"),"console.log(JSON.stringify({version:'homebase-document-v1',status:'ready',message:'fake successful document'}));").unwrap();
    assert_eq!(
        document_runtime::run(
            &runtime,
            &node,
            &home,
            &json!({}),
            Arc::default(),
            Duration::from_secs(5)
        )
        .await
        .unwrap_err(),
        RuntimeError::InvalidOutput
    );
    std::fs::write(runtime.join("worker.mjs"), "process.exit(9);").unwrap();
    assert_eq!(
        document_runtime::run(
            &runtime,
            &node,
            &home,
            &json!({}),
            Arc::default(),
            Duration::from_secs(5)
        )
        .await
        .unwrap_err(),
        RuntimeError::ProcessFailed
    );
    let inconsistent = json!({"version":"homebase-document-v1","status":"ready","code":null,"message":"synthetic","method":"pdf_text","source_sha256":"ab".repeat(32),"source_bytes":1200,"total_pages":1,"page_from":1,"page_to":1,"pages":[{"number":1,"text":"中文","bytes":2,"empty":false}],"text_bytes":2,"missing_text_pages":[],"partial":false});
    std::fs::write(
        runtime.join("worker.mjs"),
        format!(
            "console.log({});",
            serde_json::to_string(&inconsistent.to_string()).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(
        document_runtime::run(
            &runtime,
            &node,
            &home,
            &json!({}),
            Arc::default(),
            Duration::from_secs(5)
        )
        .await
        .unwrap_err(),
        RuntimeError::InvalidOutput
    );
}
