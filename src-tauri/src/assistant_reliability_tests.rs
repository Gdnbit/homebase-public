use super::*;
use crate::model_capability_io::reliability::{Failure, Kind};
use workflow_tests::{task_wait, tool_reply};

#[path = "assistant_recovery_experience_tests.rs"]
mod recovery_experience_tests;

#[path = "assistant_resume_history_tests.rs"]
mod resume_history_tests;

#[path = "assistant_queue_resume_tests.rs"]
mod queue_resume_tests;

pub(super) async fn setup(
    model: Model,
) -> (
    Arc<Engine>,
    Arc<AtomicUsize>,
    Arc<Mutex<State>>,
    tempfile::TempDir,
) {
    let (base, _, effects, saved, root) = fixture(false, vec![]).await;
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "openrouter::fixture:free".into();
    let e = Engine::new(
        state,
        base.save.clone(),
        model,
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    (e, effects, saved, root)
}
#[tokio::test]
async fn rejected_model_after_successful_action_resumes_same_checkpoint_after_restart_without_reexecution(
) {
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let model: Model = Arc::new(move |_| {
        let n = n.clone();
        Box::pin(async move {
            match n.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(tool_reply(
                    1,
                    "homebase_files",
                    json!({"action":"write","path":"isolated-proof.txt","content":"synthetic"}),
                )),
                1 => {
                    let mut f = Failure::http(503);
                    f.attempts = 3;
                    Err(f.encode())
                }
                2 => Ok(reply(vec![])),
                _ => panic!("unexpected model replay"),
            }
        })
    });
    let (e, effects, saved, _root) = setup(model).await;
    let ack = e
        .send(request(
            "创建一份合成文件，不重复执行",
            "model-failure-resume",
        ))
        .await
        .unwrap();
    let failed = task_wait(&e, &ack.turn_id, "failed").await;
    assert!(failed.resumable);
    assert_eq!(failed.model_failure.as_ref().unwrap()["http_status"], 503);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let original_job = failed.jobs[0].id.clone();
    let original_step = failed.steps[0].id.clone();
    let restored = Engine::new(
        saved.lock().await.clone(),
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "restart must not retry by itself"
    );
    let resumed = restored.resume(&ack.turn_id).await.unwrap();
    assert_eq!(resumed.turn_id, ack.turn_id);
    let done = task_wait(&restored, &ack.turn_id, "completed").await;
    assert_eq!(done.jobs.len(), 1);
    assert_eq!(done.jobs[0].id, original_job);
    assert_eq!(done.steps[0].id, original_step);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert!(done.model_failure.is_none());
}
#[tokio::test]
async fn unknown_model_timeout_after_action_blocks_resume_across_restart_without_reexecution() {
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let model: Model = Arc::new(move |_| {
        let n = n.clone();
        Box::pin(async move {
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(tool_reply(
                    1,
                    "homebase_files",
                    json!({"action":"write","path":"isolated-unknown.txt","content":"synthetic"}),
                ))
            } else {
                Err(Failure::unknown(Kind::Timeout).encode())
            }
        })
    });
    let (e, effects, saved, _root) = setup(model).await;
    let ack = e
        .send(request("创建合成文件，仅执行一次", "model-unknown-stop"))
        .await
        .unwrap();
    let stopped = task_wait(&e, &ack.turn_id, "failed").await;
    assert!(!stopped.resumable);
    assert_eq!(stopped.model_failure.unwrap()["acceptance"], "unknown");
    assert!(e.resume(&ack.turn_id).await.is_err());
    let restored = Engine::new(
        saved.lock().await.clone(),
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(restored.resume(&ack.turn_id).await.is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn model_error_diagnostic_survives_controller_restart_without_private_payload_or_retry() {
    let root = tempfile::tempdir().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let runner: Runner = Arc::new(move |_, _| {
        let n = n.clone();
        Box::pin(async move {
            n.fetch_add(1, Ordering::SeqCst);
            Err(Failure::http(429).control_error())
        })
    });
    let ctl = Controller::new(catalog().clone(), root.path(), runner.clone()).unwrap();
    let call:Request=serde_json::from_value(json!({"protocol":1,"id":"request","token":"","method":"call","catalog_hash":catalog()["catalog_hash"],"command":"model_capability_call","args":{"request":{"route_id":"fixture","input":{"synthetic_private":"DO_NOT_PERSIST_BODY"}}},"grants":["private","write","external"],"request_id":"model-rejection-original"})).unwrap();
    let ack = ctl.clone().handle(call).await.unwrap();
    let id = ack["job"]["id"].as_str().unwrap();
    for _ in 0..100 {
        let jobs=ctl.clone().handle(serde_json::from_value(json!({"protocol":1,"id":"query","token":"","method":"jobs.get","job_id":id,"grants":["private","write","external"]})).unwrap()).await.unwrap();
        if jobs["status"] == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(ctl);
    let restored = Controller::new(catalog().clone(), root.path(), runner).unwrap();
    let job=restored.handle(serde_json::from_value(json!({"protocol":1,"id":"query","token":"","method":"jobs.get","job_id":id,"grants":["private","write","external"]})).unwrap()).await.unwrap();
    assert_eq!(job["error"]["code"], "MODEL_RATE_LIMITED");
    assert_eq!(job["error"]["details"]["http_status"], 429);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let ledger = std::fs::read_to_string(root.path().join("operations.jsonl")).unwrap();
    assert!(!ledger.contains("DO_NOT_PERSIST_BODY"));
}
