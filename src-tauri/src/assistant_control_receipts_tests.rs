use super::*;
use crate::control_state::{Controller, Runner};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Clone, Copy)]
enum Fault {
    Response,
    Save,
    Missing,
}

struct Lab {
    root: tempfile::TempDir,
    executions: Arc<AtomicUsize>,
    submissions: Arc<AtomicUsize>,
    lookups: Arc<AtomicUsize>,
    fail_save: Arc<AtomicBool>,
    state: Arc<Mutex<State>>,
    result_text: Option<Arc<str>>,
}

impl Lab {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            executions: Arc::default(),
            submissions: Arc::default(),
            lookups: Arc::default(),
            fail_save: Arc::default(),
            state: Arc::default(),
            result_text: None,
        }
    }

    fn controller(&self) -> Arc<Controller> {
        let count = self.executions.clone();
        let result_text = self.result_text.clone();
        let runner: Runner = Arc::new(move |command, _| {
            count.fetch_add(1, Ordering::SeqCst);
            let result_text = result_text.clone();
            Box::pin(async move {
                let mut result = json!({"status":"succeeded","command":command,"synthetic_value":47});
                if let Some(text) = result_text {
                    result["text"] = json!(text.as_ref());
                }
                Ok(result)
            })
        });
        Controller::new(catalog().clone(), self.root.path(), runner).unwrap()
    }

    fn control(&self, controller: Arc<Controller>, fault: Option<Fault>) -> Control {
        let count = self.lookups.clone();
        let submissions = self.submissions.clone();
        let fail = self.fail_save.clone();
        Arc::new(move |request| {
            let controller = controller.clone();
            let count = count.clone();
            let submissions = submissions.clone();
            let fail = fail.clone();
            Box::pin(async move {
                if request.method == "assistant.control_receipt" {
                    count.fetch_add(1, Ordering::SeqCst);
                    return controller.assistant_receipt(&request).await;
                }
                if request.method == "call" {
                    submissions.fetch_add(1, Ordering::SeqCst);
                    if matches!(fault, Some(Fault::Missing)) {
                        return Err(ControlError::new("NOT_READY", "synthetic unavailable"));
                    }
                    let response = controller.handle(request).await?;
                    match fault {
                        Some(Fault::Response) => {
                            return Err(ControlError::new("LOST_RESPONSE", "synthetic lost ACK"))
                        }
                        Some(Fault::Save) => fail.store(true, Ordering::SeqCst),
                        _ => (),
                    }
                    return Ok(response);
                }
                controller.handle(request).await
            })
        })
    }

    async fn engine(&self, controller: Arc<Controller>, fault: Option<Fault>) -> Arc<Engine> {
        let control = self.control(controller, fault);
        let state = self.state.clone();
        let fail = self.fail_save.clone();
        let file = self.root.path().join("state.dpapi");
        let save_file = file.clone();
        let save: Save = Arc::new(move |new| {
            let state = state.clone();
            let fail = fail.clone();
            let file = save_file.clone();
            Box::pin(async move {
                if fail.swap(false, Ordering::SeqCst) {
                    return Err("synthetic failed save after acceptance".into());
                }
                let json = zeroize::Zeroizing::new(serde_json::to_vec(&new).unwrap());
                let cipher = crate::wechat_crypto::crypt(&json, true).unwrap();
                crate::control_protocol::publish_private(&file, &cipher)
                    .await
                    .unwrap();
                *state.lock().await = new;
                Ok(())
            })
        });
        let initial = crate::assistant_state_store::read_legacy(&file)
            .unwrap()
            .map(|plain| serde_json::from_slice(&plain).unwrap())
            .unwrap_or_default();
        Engine::new(
            initial,
            save,
            Arc::new(|_| Box::pin(async { panic!("no model calls") })),
            control,
            self.root.path().join("workspace"),
        )
        .await
        .unwrap()
    }

    async fn finished_job(&self, controller: &Arc<Controller>, request_id: &str) -> Value {
        let request = controller_request("assistant.control_receipt", request_id, json!({}));
        for _ in 0..40 {
            let response = controller.assistant_receipt(&request).await.unwrap();
            if response["job"]["status"] == "succeeded" {
                return response["job"].clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("synthetic controller job timed out");
    }
}

async fn capacity_engine(lab: &Lab, fault: Option<Fault>) -> Arc<Engine> {
    use crate::assistant_state_store::Store;
    let mut writer = Store::new(
        lab.root.path().join("history-v1"),
        lab.root.path().join("state.dpapi"),
    )
    .unwrap();
    writer.enable_capacity_segments();
    let snapshot = writer.load_index().unwrap();
    let store = Arc::new(Mutex::new(writer));
    let state = match snapshot {
        Some(snapshot) => archive::load(store.clone(), snapshot).await.unwrap(),
        None => {
            let mut fresh = State::default();
            archive::enable_capacity(&mut fresh);
            archive::commit(store.clone(), fresh, vec![]).await.unwrap()
        }
    };
    let save: Commit = Arc::new(move |state, protected| {
        let store = store.clone();
        Box::pin(async move { archive::commit(store, state, protected).await })
    });
    let workspace = lab.root.path().join("workspace");
    let documents = Arc::new(crate::assistant_documents::DocumentStore::new(
        workspace.join(".documents"),
    ));
    Engine::new_with_commit(
        state,
        save,
        Arc::new(|_| Box::pin(async { panic!("no model calls") })),
        lab.control(lab.controller(), fault),
        workspace,
        documents,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn one_workflow_settles_105_real_control_jobs_and_preserves_ends_across_restart() {
    let lab = Lab::new();
    verify_long_workflow(&lab).await;
}

#[tokio::test]
async fn verified_result_is_retrievable_while_same_workflow_keeps_running() {
    let mut lab = Lab::new();
    let text = "合成结果🙂".repeat(220);
    lab.result_text = Some(Arc::from(text.as_str()));
    let engine = capacity_engine(&lab, None).await;
    let ack = start(&engine, "wechat", "synthetic-owner", "live-result-workflow").await;
    let first = engine.execute(&ack.turn_id, &tool()).await.unwrap().unwrap();
    assert_eq!(first["result"]["text"], text);
    assert_eq!(engine.turn_state(&ack.turn_id).await.unwrap(), "running");
    {
        let state = engine.state.lock().await;
        let job = &state.sessions[0].jobs[0];
        assert!(job.archived_result_bytes.is_some());
        assert!(job.result.get("text").is_none());
    }
    let replay = engine.execute(&ack.turn_id, &tool()).await.unwrap().unwrap();
    assert_eq!(replay, first);
    engine.change_turn(&ack.turn_id, |session, index| {
        session.turns[index].next_call += 1;
        Ok(())
    }).await.unwrap();
    let second = engine.execute(&ack.turn_id, &tool()).await.unwrap().unwrap();
    assert_ne!(first["job_id"], second["job_id"]);
    assert_eq!(second["result"]["text"], text);
    assert_eq!(lab.submissions.load(Ordering::SeqCst), 2);
    assert_eq!(lab.executions.load(Ordering::SeqCst), 2);
    let detail = engine.turn(&ack.turn_id).await.unwrap();
    assert_eq!(detail.jobs[0].result, first["result"]);
    assert_eq!(detail.jobs[1].result, second["result"]);
    drop(engine);

    let restarted = capacity_engine(&lab, None).await;
    let detail = restarted.turn(&ack.turn_id).await.unwrap();
    assert_eq!(detail.jobs[0].result, first["result"]);
    assert_eq!(detail.jobs[1].result, second["result"]);
    assert_eq!(detail.jobs[0].id, first["job_id"]);
    assert_eq!(detail.jobs[1].id, second["job_id"]);
    assert_eq!(lab.submissions.load(Ordering::SeqCst), 2);
    assert_eq!(lab.executions.load(Ordering::SeqCst), 2);
}

async fn verify_long_workflow(lab: &Lab) -> (Ack, Vec<String>) {
    let e = capacity_engine(lab, None).await;
    let ack = start(&e, "wechat", "synthetic-owner", "long-control-workflow").await;
    let mut ids = Vec::new();
    for ordinal in 0..105 {
        e.change_turn(&ack.turn_id, |session, index| {
            session.turns[index].rounds = ordinal / 4;
            session.turns[index].next_call = (ordinal % 4) as usize;
            Ok(())
        })
        .await
        .unwrap();
        let value = e.execute(&ack.turn_id, &tool()).await.unwrap().unwrap();
        assert_eq!(value["state"], "succeeded");
        assert_eq!(value["result"]["synthetic_value"], 47);
        ids.push(value["job_id"].as_str().unwrap().to_owned());
    }
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 105);
    let replay = e.execute(&ack.turn_id, &tool()).await.unwrap().unwrap();
    assert_eq!(replay["job_id"], ids[104]);
    assert_eq!(lab.submissions.load(Ordering::SeqCst), 105);
    assert_eq!(lab.executions.load(Ordering::SeqCst), 105);
    assert_eq!(
        occupied_job_slots(&e.state.lock().await.sessions[0], true),
        0
    );

    e.change_turn(&ack.turn_id, |session, index| {
        let turn = &mut session.turns[index];
        turn.state = "completed".into();
        turn.finished_at = Some(now());
        turn.history_committed = true;
        Ok(())
    })
    .await
    .unwrap();
    let ledger = std::fs::read(lab.root.path().join("operations.jsonl")).unwrap();
    drop(e);
    let resumed = capacity_engine(lab, None).await;
    let view = resumed.turn(&ack.turn_id).await.unwrap();
    assert_eq!(view.state, "completed");
    assert_eq!(view.jobs.len(), 105);
    assert_eq!(view.steps.len(), 105);
    for (index, (job, step)) in view.jobs.iter().zip(view.steps.iter()).enumerate() {
        assert_eq!(job.id, ids[index]);
        assert_eq!(step.job_id.as_ref(), Some(&job.id));
        assert_eq!(job.result["synthetic_value"], 47);
    }
    let request = json!({
        "operation":"get","source_key":"synthetic-owner","turn_id":ack.turn_id,"include_files":true
    });
    let result = resumed.channel(request.clone()).await.unwrap();
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["result"]["jobs"][0]["id"], ids[0]);
    assert_eq!(result["result"]["jobs"][104]["id"], ids[104]);
    let mut other = request;
    other["source_key"] = json!("another-owner");
    assert!(resumed.channel(other).await.is_err());
    assert_eq!(lab.executions.load(Ordering::SeqCst), 105);
    assert_eq!(
        std::fs::read(lab.root.path().join("operations.jsonl")).unwrap(),
        ledger
    );
    (ack, ids)
}

#[tokio::test]
#[ignore = "explicit synthetic native long-job archive fixture"]
async fn seed_native_long_control_workflow() {
    let lab = Lab::new();
    assert!(lab
        .root
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .starts_with("F:/HomeBase-Labs/"));
    let (ack, ids) = verify_long_workflow(&lab).await;
    let retained = lab.root.keep();
    println!(
        "LONG_JOB_FIXTURE {}",
        json!({"root":retained,"session_id":ack.session_id,"turn_id":ack.turn_id,"job_ids":ids})
    );
}

#[test]
fn only_verified_settled_jobs_release_capacity_slots() {
    let mut session = empty_session("desktop", "desktop", "capacity-slot-fixture");
    let mut turn: Turn = serde_json::from_value(json!({
        "id":"owning-turn","request_id":"synthetic","state":"running","text":"",
        "route_id":"","created_at":"","finished_at":null,"steps":[],"pending":null,
        "fingerprint":"","settings":Settings::default(),"protocol":[],"queued":[],
        "next_call":0,"rounds":0,"history_committed":false
    }))
    .unwrap();
    let mut job = Job {
        archived_result_bytes: None,
        id: "original-job".into(),
        command: "system_info".into(),
        turn_id: turn.id.clone(),
        state: "succeeded".into(),
        result: json!({"status":"succeeded","value":47}),
        owned: true,
        deliver: false,
        artifacts: vec![],
        remote: None,
    };
    turn.steps.push(Step {
        id: "owning-step".into(),
        name: "homebase_call".into(),
        state: "completed".into(),
        arguments: json!({}),
        result: json!({"state":"succeeded","step_id":"owning-step","job_id":job.id}),
        job_id: Some(job.id.clone()),
    });
    session.turns.push(turn);
    session.jobs.push(job.clone());
    assert_eq!(occupied_job_slots(&session, true), 0);
    assert_eq!(occupied_job_slots(&session, false), 1);
    let mut query = session.turns[0].steps[0].clone();
    query.name = "homebase_job".into();
    session.turns[0].steps.push(query);
    assert_eq!(occupied_job_slots(&session, true), 0);
    for result in [
        Value::Null,
        json!({"code":"RESULT_UNAVAILABLE"}),
        json!({"status":"partial"}),
        json!({"complete":false}),
        json!({"truncated":true,"preview":"only a preview"}),
        json!({"message":"进程重启后控制结果不可恢复，禁止盲目重跑"}),
    ] {
        session.jobs[0].result = result;
        assert_eq!(occupied_job_slots(&session, true), 1);
    }
    job.deliver = true;
    session.jobs[0] = job.clone();
    assert_eq!(occupied_job_slots(&session, true), 1);
    job.deliver = false;
    job.remote = Some(Remote {
        route_id: "synthetic".into(),
        task_id: "remote".into(),
        query_job: Some("outstanding".into()),
        polls: 1,
        next_poll: 0,
        finished: true,
        credential_source: Default::default(),
    });
    session.jobs[0] = job;
    assert_eq!(occupied_job_slots(&session, true), 1);
    session.jobs[0].remote.as_mut().unwrap().query_job = None;
    assert_eq!(occupied_job_slots(&session, true), 0);
    session.turns[0].steps[0].result["step_id"] = json!("wrong-step");
    assert_eq!(occupied_job_slots(&session, true), 1);
}

fn controller_request(method: &str, id: &str, args: Value) -> Request {
    Request {
        protocol: 1,
        id: "fixture".into(),
        token: String::new(),
        method: method.into(),
        catalog_hash: catalog()["catalog_hash"].as_str().map(str::to_owned),
        command: Some("system_info".into()),
        args,
        grants: vec!["private".into(), "write".into(), "external".into()],
        request_id: Some(id.into()),
        job_id: None,
    }
}

fn tool() -> ToolCall {
    ToolCall {
        id: "fixture-call".into(),
        name: "homebase_call".into(),
        arguments: json!({"command":"system_info","args":{}}),
    }
}

async fn start(e: &Arc<Engine>, source: &str, key: &str, rid: &str) -> Ack {
    let ack = e
        .send_from(
            SendRequest {
                image: None,
                session_id: None,
                text: "检查合成本机信息".into(),
                request_id: rid.into(),
                route_id: None,
                attachments: vec![],
                mode: Some("task".into()),
            },
            source,
            key,
        )
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].state = "running".into();
        Ok(())
    })
    .await
    .unwrap();
    ack
}

#[tokio::test]
async fn lost_response_links_original_receipt_once_and_rejects_other_wechat_owner() {
    let lab = Lab::new();
    let controller = lab.controller();
    let e = lab.engine(controller.clone(), Some(Fault::Response)).await;
    let ack = start(&e, "wechat", "synthetic-owner", "response-loss").await;
    assert!(e
        .execute(&ack.turn_id, &tool())
        .await
        .err()
        .unwrap()
        .contains("LOST_RESPONSE"));
    let step = format!("assistant-{}-0", ack.turn_id);
    let expected = lab.finished_job(&controller, &step).await;
    assert_eq!(
        lab.state.lock().await.sessions[0].turns[0].control_intents[&step],
        "system_info"
    );
    assert!(lab.state.lock().await.sessions[0].jobs.is_empty());
    let original_ledger = std::fs::read(lab.root.path().join("operations.jsonl")).unwrap();
    drop(e);
    let resumed = lab.engine(controller.clone(), None).await;
    assert!(resumed
        .channel_view("other-owner", &ack.turn_id, true)
        .await
        .is_err());
    assert_eq!(lab.lookups.load(Ordering::SeqCst), 0);
    let (result, other) = tokio::join!(
        resumed.channel_view("synthetic-owner", &ack.turn_id, true),
        resumed.turn_with_progress(&ack.turn_id)
    );
    let result = result.unwrap();
    let other = other.unwrap();
    assert_eq!(result["result"]["jobs"][0]["id"], expected["id"]);
    assert_eq!(other.jobs[0].id, expected["id"]);
    let view = resumed.turn_with_progress(&ack.turn_id).await.unwrap();
    assert_eq!(view.state, "interrupted_unknown");
    assert_eq!(view.steps[0].state, "completed");
    assert_eq!(view.jobs[0].result["synthetic_value"], 47);
    let revision = lab.state.lock().await.revision;
    resumed
        .channel_view("synthetic-owner", &ack.turn_id, true)
        .await
        .unwrap();
    assert_eq!(lab.state.lock().await.revision, revision);
    assert_eq!(lab.executions.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(lab.root.path().join("operations.jsonl")).unwrap(),
        original_ledger
    );
    let forged = controller_request("assistant.control_receipt", &step, json!({"forged":true}));
    assert_eq!(
        controller
            .assistant_receipt(&forged)
            .await
            .unwrap_err()
            .code,
        "REQUEST_CONFLICT"
    );
    assert_eq!(
        controller.handle(forged).await.unwrap_err().code,
        "UNKNOWN_METHOD"
    );
}

#[tokio::test]
async fn accepted_job_save_failure_recovers_id_after_control_restart_and_preserves_unknown_result()
{
    let lab = Lab::new();
    let controller = lab.controller();
    let e = lab.engine(controller.clone(), Some(Fault::Save)).await;
    let ack = start(&e, "desktop", "desktop", "failed-job-save").await;
    assert!(e
        .execute(&ack.turn_id, &tool())
        .await
        .err()
        .unwrap()
        .contains("failed save"));
    let step = format!("assistant-{}-0", ack.turn_id);
    let original = lab.finished_job(&controller, &step).await;
    assert!(lab.state.lock().await.sessions[0].jobs.is_empty());
    drop(e);
    drop(controller);
    let resumed = lab.engine(lab.controller(), None).await;
    let view = resumed.turn_with_progress(&ack.turn_id).await.unwrap();
    assert_eq!(view.jobs[0].id, original["id"]);
    assert_eq!(view.jobs[0].state, "unknown");
    assert_eq!(view.steps[0].state, "unknown");
    assert!(!view.resumable);
    // A concurrent reader can have selected the same running ID just before
    // the previous query published the now-unknown outcome.
    resumed
        .refresh_job(&ack.session_id, &view.jobs[0].id)
        .await
        .unwrap();
    let refreshed = resumed.turn(&ack.turn_id).await.unwrap();
    assert_eq!(refreshed.jobs[0].state, "unknown");
    assert_eq!(refreshed.steps[0].state, "unknown");
    let revision = lab.state.lock().await.revision;
    let lookups = lab.lookups.load(Ordering::SeqCst);
    resumed.turn_with_progress(&ack.turn_id).await.unwrap();
    assert_eq!(lab.state.lock().await.revision, revision);
    assert_eq!(lab.lookups.load(Ordering::SeqCst), lookups);
    assert_eq!(lab.executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unconfirmed_acceptance_keeps_last_slot_and_never_reissues_the_mutation() {
    let lab = Lab::new();
    let controller = lab.controller();
    let e = lab.engine(controller, Some(Fault::Missing)).await;
    let first = start(&e, "desktop", "desktop", "last-slot").await;
    e.change_turn(&first.turn_id, |s, _| {
        for i in 0..99 {
            s.jobs.push(Job {
                archived_result_bytes: None,
                id: format!("fixture-job-{i}"),
                command: "system_info".into(),
                turn_id: first.turn_id.clone(),
                state: "succeeded".into(),
                result: json!({"synthetic":i}),
                owned: true,
                deliver: false,
                artifacts: vec![],
                remote: None,
            });
        }
        Ok(())
    })
    .await
    .unwrap();
    assert!(e
        .execute(&first.turn_id, &tool())
        .await
        .err()
        .unwrap()
        .contains("NOT_READY"));
    assert!(e
        .execute(&first.turn_id, &tool())
        .await
        .err()
        .unwrap()
        .starts_with("JOB_RECEIPT_UNVERIFIED:"));
    e.change_turn(&first.turn_id, |s, t| {
        s.turns[t].state = "failed".into();
        Ok(())
    })
    .await
    .unwrap();
    let second = e
        .send_from(
            SendRequest {
                image: None,
                session_id: Some(first.session_id),
                text: "新的合成要求".into(),
                request_id: "no-slot".into(),
                route_id: None,
                attachments: vec![],
                mode: Some("task".into()),
            },
            "desktop",
            "desktop",
        )
        .await
        .unwrap();
    e.change_turn(&second.turn_id, |s, t| {
        s.turns[t].state = "running".into();
        Ok(())
    })
    .await
    .unwrap();
    assert!(e
        .execute(&second.turn_id, &tool())
        .await
        .err()
        .unwrap()
        .contains("上限"));
    assert_eq!(lab.executions.load(Ordering::SeqCst), 0);
    assert_eq!(lab.submissions.load(Ordering::SeqCst), 1);
    assert_eq!(lab.lookups.load(Ordering::SeqCst), 1);
    assert_eq!(lab.state.lock().await.sessions[0].jobs.len(), 99);
}

#[tokio::test]
#[ignore = "explicit synthetic native receipt fixture in F lab only"]
async fn seed_native_unlinked_control_receipt() {
    let home = PathBuf::from(
        std::env::var_os("HOMEBASE_JOB_RECEIPT_SEED_HOME").expect("fresh synthetic home"),
    );
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("receipt-native-"));
    crate::assistant_state_store::safe(&home).unwrap();
    let lab = Lab::new();
    let controller = lab.controller();
    let engine = lab.engine(controller.clone(), Some(Fault::Response)).await;
    let ack = start(&engine, "desktop", "desktop", "native-unlinked-acceptance").await;
    assert!(engine
        .execute(&ack.turn_id, &tool())
        .await
        .err()
        .unwrap()
        .contains("LOST_RESPONSE"));
    let step = format!("assistant-{}-0", ack.turn_id);
    let job = lab.finished_job(&controller, &step).await;
    assert_eq!(lab.executions.load(Ordering::SeqCst), 1);
    assert!(lab.state.lock().await.sessions[0].jobs.is_empty());
    for (file, folder) in [
        ("state.dpapi", "assistant"),
        ("operations.jsonl", "control"),
    ] {
        let directory = home.join(".homebase").join(folder);
        std::fs::create_dir_all(&directory).unwrap();
        let bytes = std::fs::read(lab.root.path().join(file)).unwrap();
        crate::control_protocol::publish_private(&directory.join(file), &bytes)
            .await
            .unwrap();
    }
    let expected = json!({"synthetic_only":true,"real_model_calls":0,"id":ack.session_id,"turn_id":ack.turn_id,"job_id":job["id"],"step_id":step});
    crate::control_protocol::publish_private(
        &home.join("control-receipt-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}
