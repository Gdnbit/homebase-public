use homebase_control_tests::{
    control_protocol::{Endpoint, Handler, Request, Server, MAX_REQUEST},
    control_state::{Controller, Runner},
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};

fn catalog() -> Value {
    json!({"catalog_hash":"fixture-hash","commands":[
        {"name":"read","status":"available","requires":[],"reason":null,"parameters":[]},
        {"name":"change","status":"available","requires":["write"],"reason":null,"parameters":[{"name":"testValue","rust_name":"test_value","optional":false}]},
        {"name":"private","status":"available","requires":["private"],"reason":null,"parameters":[]},
        {"name":"task_cancel","status":"available","requires":["write"],"reason":null,"parameters":[]},
        {"name":"external","status":"available","requires":["external"],"reason":null,"parameters":[]},
        {"name":"secrets_save","status":"available","requires":["write"],"reason":null,"parameters":[{"name":"request","rust_name":"request","optional":false}]},
        {"name":"blocked","status":"blocked","requires":[],"reason":"manual only","parameters":[]},
        {"name":"stub","status":"unavailable","requires":[],"reason":"not implemented","parameters":[]}
    ]})
}
fn request(method: &str, patch: Value) -> Request {
    let mut base = json!({"protocol":1,"id":"fixture","token":"unused","method":method,"catalog_hash":"fixture-hash","request_id":"request-1","args":{}});
    base.as_object_mut()
        .unwrap()
        .extend(patch.as_object().unwrap().clone());
    serde_json::from_value(base).unwrap()
}
fn fixture() -> (tempfile::TempDir, Arc<Controller>, Arc<AtomicUsize>) {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let runner: Runner = Arc::new(move |name, args| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(
                json!({"name":name,"args":args,"apiKey":"synthetic-secret","env":{"TOKEN":"synthetic-secret"}}),
            )
        })
    });
    let controller = Controller::new(catalog(), directory.path(), runner).unwrap();
    (directory, controller, calls)
}
async fn finished(controller: &Arc<Controller>, id: &str, grants: Value) -> Value {
    for _ in 0..100 {
        let result = controller
            .clone()
            .handle(request("jobs.get", json!({"job_id":id,"grants":grants})))
            .await
            .unwrap();
        if result["status"] != "running" {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("synthetic job failed to finish");
}

#[tokio::test]
async fn rollout_status_counts_the_full_control_ledger_without_dispatch_or_audit() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = dir.path().join("operations.jsonl");
    let mut before = String::new();
    for number in 0..105 {
        let record = json!({
            "id":format!("job-{number}"), "request_id":format!("request-{number}"),
            "command":"read", "status":if number == 0 { "running" } else { "succeeded" },
            "created_at":if number == 0 { "2026-11-19T00:00:00Z" } else { "2026-11-20T00:00:00Z" },
            "finished_at":null, "fingerprint":"synthetic-canary", "requires":[]
        });
        before.push_str(&format!("{record}\n"));
    }
    std::fs::write(&ledger, &before).unwrap();
    let runner: Runner = Arc::new(|_, _| Box::pin(async { panic!("the probe must not dispatch tools") }));
    let controller = Controller::new(catalog(), dir.path(), runner).unwrap();
    let absent_grant = request("rollout.status", json!({"request_id":null}));
    assert_eq!(controller.clone().handle(absent_grant).await.unwrap_err().code, "SCOPE_REQUIRED");
    let payload = request("rollout.status", json!({"grants":["private"],"command":"read","request_id":null}));
    assert_eq!(controller.clone().handle(payload).await.unwrap_err().code, "BAD_ARGUMENT");
    let result = controller.clone().handle(request("rollout.status", json!({"grants":["private"],"request_id":null}))).await.unwrap();
    assert_eq!(result["control"], json!({"healthy":true,"jobs":105,"active":0,"to_review":1}));
    assert_eq!(result["wechat"], Value::Null);
    assert_eq!(result["safe_to_update"], false);
    assert!(!result.to_string().contains("synthetic-canary"));
    assert_eq!(std::fs::read_to_string(ledger).unwrap(), before);
}

#[tokio::test]
async fn ledger_near_limit_rejects_before_execution_when_terminal_cannot_fit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.jsonl");
    let record = json!({
        "id":"old-job", "request_id":"old-request", "command":"read", "status":"succeeded",
        "created_at":"2026-11-19T00:00:00Z", "finished_at":"2026-11-19T00:00:01Z",
        "fingerprint":"old-fingerprint", "requires":[]
    });
    let line = format!("{record}\n");
    let mut bytes = vec![b' '; 32 * 1024 * 1024 - 550 - line.len()];
    bytes.extend_from_slice(line.as_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let runner: Runner = Arc::new(move |_, _| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"changed":true}))
        })
    });
    let controller = Controller::new(catalog(), directory.path(), runner).unwrap();
    let response = controller.clone().handle(request("call", json!({
        "command":"change", "args":{"testValue":"fixture"}, "grants":["write"]
    }))).await;
    if let Ok(ref accepted) = response {
        let id = accepted["job"]["id"].as_str().unwrap();
        let status = finished(&controller, id, json!(["write"])).await;
        panic!("task executed without terminal space: {}", status["status"]);
    }
    assert_eq!(response.unwrap_err().code, "STATE_LIMIT");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(std::fs::read(path).unwrap() == bytes);
}

#[tokio::test]
async fn later_requests_cannot_use_space_reserved_by_in_flight_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.jsonl");
    let record = json!({
        "id":"last-unique-job", "request_id":"last-unique-request", "command":"read", "status":"succeeded",
        "created_at":"2026-11-19T00:00:00Z", "finished_at":"2026-11-19T00:00:01Z",
        "fingerprint":"original-fingerprint", "requires":[]
    });
    let line = format!("{record}\n");
    let mut bytes = vec![b' '; 32 * 1024 * 1024 - 3100 - line.len()];
    bytes.extend_from_slice(line.as_bytes());
    std::fs::write(&path, bytes).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let runner: Runner = {
        let release = release.clone();
        let calls = calls.clone();
        Arc::new(move |_, _| {
            let release = release.clone();
            let calls = calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
                Ok(json!({"changed":true}))
            })
        })
    };
    let controller = Controller::new(catalog(), directory.path(), runner).unwrap();
    let response = controller.clone().handle(request("call", json!({"command":"read", "request_id":"first-pending"}))).await.unwrap();
    let id = response["job"]["id"].as_str().unwrap();
    let later = controller.clone().handle(request("call", json!({"command":"read", "request_id":"second-pending"}))).await.unwrap_err();
    assert_eq!(later.code, "STATE_LIMIT");
    release.notify_one();
    assert_eq!(finished(&controller, id, json!([])).await["status"], "succeeded");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(std::fs::metadata(path).unwrap().len() <= 32 * 1024 * 1024);
}

#[cfg(windows)]
#[tokio::test]
async fn ledger_compaction_keeps_protected_receipts_and_original_requests() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("operations.jsonl");
    let mut catalog = catalog();
    catalog["commands"].as_array_mut().unwrap().push(json!({
        "name":"assistant_send", "status":"available", "requires":["write"],
        "reason":null, "parameters":[]
    }));
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let runner: Runner = Arc::new(move |name, _| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(if name == "assistant_send" {
                json!({"session_id":"original-session","turn_id":"original-turn","state":"disabled"})
            } else {
                json!({"observed":true})
            })
        })
    });
    let original = json!({"command":"assistant_send", "request_id":"protected-first", "grants":["write"]});
    let next = json!({"command":"read", "request_id":"next-after-compaction"});
    let controller = Controller::new(catalog.clone(), directory.path(), runner.clone()).unwrap();
    let accepted = controller.clone().handle(request("call", original.clone())).await.unwrap();
    let first_id = accepted["job"]["id"].as_str().unwrap();
    let first = finished(&controller, first_id, json!(["write"])).await;
    assert_eq!(first["result_storage"], "protected");
    drop(controller);

    let originals = std::fs::read(&path).unwrap();
    let last = originals.split(|byte| *byte == b'\n').nth(1).unwrap();
    let mut fixture = vec![b' '; 32 * 1024 * 1024 - 550 - originals.len()];
    fixture.extend_from_slice(&originals);
    std::fs::write(&path, &fixture).unwrap();
    let controller = Controller::new(catalog.clone(), directory.path(), runner.clone()).unwrap();

    let locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&path)
        .unwrap();
    let denied = controller.clone().handle(request("call", next.clone())).await.unwrap_err();
    assert_eq!(denied.code, "STATE_IO");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(std::fs::read(&path).unwrap() == fixture);
    drop(locked);

    let accepted = controller.clone().handle(request("call", next.clone())).await.unwrap();
    let next_id = accepted["job"]["id"].as_str().unwrap();
    assert_eq!(finished(&controller, next_id, json!([])).await["status"], "succeeded");
    let compacted = std::fs::read(&path).unwrap();
    assert!(compacted.len() < 4096);
    assert_eq!(compacted.split(|byte| *byte == b'\n').next().unwrap(), last);
    drop(controller);

    let controller = Controller::new(catalog, directory.path(), runner).unwrap();
    assert_eq!(finished(&controller, first_id, json!(["write"])).await["result"], first["result"]);
    for (parameters, id) in [(original, first_id), (next, next_id)] {
        let replay = controller.clone().handle(request("call", parameters)).await.unwrap();
        assert_eq!(replay["reused"], true);
        assert_eq!(replay["job"]["id"], id);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[cfg(windows)]
#[tokio::test]
async fn protected_control_receipts_survive_restart_and_keep_original_identity() {
    let directory = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let original_path = directory.path().join("synthetic-private-inbox.md");
    let inbox = json!({"path":original_path,"filename":"synthetic-private-inbox.md","day":"2026-11-19","char_count":19});
    let assistant = json!({"session_id":"synthetic-session","turn_id":"synthetic-turn","state":"disabled"});
    let inbox_result = inbox.clone();
    let assistant_result = assistant.clone();
    let runner: Runner = Arc::new(move |name, _| {
        let counter = counter.clone();
        let answer = match name.as_str() {
            "assistant_send" => assistant_result.clone(),
            "inbox_submit" => inbox_result.clone(),
            _ => json!({"text":"transient-private-body"}),
        };
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(answer)
        })
    });
    let mut catalog = catalog();
    let commands = catalog["commands"].as_array_mut().unwrap();
    for (command, argument, scopes) in [
        ("assistant_send", "request", json!(["private","write","external"])),
        ("inbox_submit", "payload", json!(["write"])),
    ] {
        commands.push(json!({"name":command,"status":"available","requires":scopes,"parameters":[
            {"name":argument,"rust_name":argument,"optional":false}
        ]}));
    }
    let c = Controller::new(catalog.clone(), directory.path(), runner.clone()).unwrap();
    let assistant_request = json!({"command":"assistant_send","request_id":"assistant-original","grants":["private","write","external"],"args":{"request":{"text":"synthetic-original-draft-text"}}});
    let inbox_request = json!({"command":"inbox_submit","request_id":"inbox-original","grants":["write"],"args":{"payload":{"text":"synthetic-original-inbox-body"}}});
    let mut ids = Vec::new();
    for args in [assistant_request.clone(), inbox_request.clone(), json!({"command":"private","request_id":"private-only","grants":["private"]})] {
        let scopes = args["grants"].clone();
        let accepted = c.clone().handle(request("call", args)).await.unwrap();
        let id = accepted["job"]["id"].as_str().unwrap().to_owned();
        assert_eq!(finished(&c, &id, scopes).await["status"], "succeeded");
        ids.push(id);
    }
    let ledger = directory.path().join("operations.jsonl");
    let original = std::fs::read_to_string(&ledger).unwrap();
    for secret in [
        "synthetic-private-inbox.md", "synthetic-original-draft-text",
        "synthetic-original-inbox-body", "synthetic-turn", "transient-private-body",
    ] {
        assert!(!original.contains(secret));
    }
    assert_eq!(original.matches("\"protected_result\"").count(), 2);
    drop(c);
    let recovered = Controller::new(catalog.clone(), directory.path(), runner.clone()).unwrap();
    let hello = recovered.clone().handle(request("hello", json!({}))).await.unwrap();
    assert_eq!(hello["results_persistent"], false);
    assert_eq!(hello["protected_result_commands"], json!([
        "assistant_send", "inbox_submit", "repo_project_analyze", "repo_search_public",
        "capability_clip_video", "capability_assemble_video", "capability_trim_clip_artifact",
        "capability_video_cover",
        "capability_video_publish_package",
    ]));
    for (i, payload, result) in [(0, assistant_request.clone(), assistant), (1, inbox_request, inbox)] {
        let scopes = payload["grants"].clone();
        let denied = recovered.clone()
            .handle(request("jobs.get", json!({"job_id":ids[i],"grants":[]})))
            .await.unwrap_err();
        assert_eq!(denied.code, "SCOPE_REQUIRED");
        let recovered_result = finished(&recovered, &ids[i], scopes).await;
        assert_eq!(recovered_result["result"], result);
        assert_eq!(recovered_result["result_storage"], "protected");
        assert_eq!(recovered_result["result_available"], true);
        assert!(recovered_result.get("protected_result").is_none());
        let replay = recovered.clone().handle(request("call", payload)).await.unwrap();
        assert_eq!(replay["job"]["id"], ids[i]);
        assert_eq!(replay["reused"], true);
    }
    let unavailable = finished(&recovered, &ids[2], json!(["private"])).await;
    assert_eq!(unavailable["result_available"], false);
    let mut conflict = assistant_request;
    conflict["args"]["request"]["text"] = json!("different content");
    assert_eq!(
        recovered.clone().handle(request("call", conflict)).await.unwrap_err().code,
        "REQUEST_CONFLICT"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    drop(recovered);
    let mut rows: Vec<Value> = original.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let other = rows.iter().find(|row| row["id"] == ids[1] && row["status"] == "succeeded").unwrap()["protected_result"].clone();
    let first = rows.iter_mut().find(|row| row["id"] == ids[0] && row["status"] == "succeeded").unwrap();
    first["protected_result"] = other;
    std::fs::write(&ledger, rows.iter().map(|row| format!("{row}\n")).collect::<String>()).unwrap();
    let tampered = Controller::new(catalog, directory.path(), runner).unwrap();
    let result = finished(&tampered, &ids[0], json!(["private","write","external"])).await;
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["result_available"], false);
    assert_eq!(result["error"]["code"], "RESULT_UNAVAILABLE");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn permissions_unknown_methods_blocked_and_stubs_never_execute() {
    let (_dir, c, calls) = fixture();
    for (name, code) in [
        ("change", "SCOPE_REQUIRED"),
        ("private", "SCOPE_REQUIRED"),
        ("external", "SCOPE_REQUIRED"),
        ("blocked", "HUMAN_REQUIRED"),
        ("stub", "NOT_IMPLEMENTED"),
        ("missing", "UNKNOWN_COMMAND"),
    ] {
        assert_eq!(
            c.clone()
                .handle(request("call", json!({"command":name})))
                .await
                .unwrap_err()
                .code,
            code
        );
    }
    assert_eq!(
        c.clone()
            .handle(request("arbitrary-shell", json!({})))
            .await
            .unwrap_err()
            .code,
        "UNKNOWN_METHOD"
    );
    assert_eq!(
        c.clone()
            .handle(request("hello", json!({"grants":["admin"]})))
            .await
            .unwrap_err()
            .code,
        "BAD_GRANT"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn arguments_and_version_checked_before_execution() {
    let (_dir, c, calls) = fixture();
    for patch in [
        json!({"args":{}}),
        json!({"args":{"testValue":1,"unexpected":true}}),
        json!({"args":{"testValue":1,"test_value":1}}),
        json!({"args":[]}),
    ] {
        let mut fields = json!({"command":"change","grants":["write"]});
        fields
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert_eq!(
            c.clone()
                .handle(request("call", fields))
                .await
                .unwrap_err()
                .code,
            "BAD_ARGUMENT"
        );
    }
    assert_eq!(
        c.clone()
            .handle(request(
                "call",
                json!({"command":"read","catalog_hash":"stale"})
            ))
            .await
            .unwrap_err()
            .code,
        "CATALOG_MISMATCH"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn secrets_body_cannot_be_written_through_cli() {
    let (_dir, c, calls) = fixture();
    assert_eq!(c.clone().handle(request("call",json!({"command":"secrets_save","grants":["write"],"args":{"request":{"value":"synthetic-never-write"}}}))).await.unwrap_err().code,"HUMAN_REQUIRED");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn idempotent_replay_is_exactly_once_and_conflicts_are_rejected() {
    let (_dir, c, calls) = fixture();
    let fields =
        json!({"command":"change","grants":["write"],"args":{"testValue":"synthetic-value"}});
    let first = c
        .clone()
        .handle(request("call", fields.clone()))
        .await
        .unwrap();
    let second = c
        .clone()
        .handle(request("call", fields.clone()))
        .await
        .unwrap();
    assert_eq!(first["job"]["id"], second["job"]["id"]);
    assert_eq!(second["reused"], true);
    finished(&c, first["job"]["id"].as_str().unwrap(), json!(["write"])).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        c.clone()
            .handle(request(
                "call",
                json!({"command":"change","grants":["write"],"args":{"testValue":"different"}})
            ))
            .await
            .unwrap_err()
            .code,
        "REQUEST_CONFLICT"
    );
}
#[tokio::test]
async fn ledger_persists_only_metadata_and_restarts_do_not_repeat_operations() {
    let (dir, c, calls) = fixture();
    let fields = json!({"command":"change","grants":["write"],"args":{"test_value":"synthetic-private-body"}});
    let accepted = c
        .clone()
        .handle(request("call", fields.clone()))
        .await
        .unwrap();
    let id = accepted["job"]["id"].as_str().unwrap();
    let result = finished(&c, id, json!(["write"])).await;
    assert_eq!(result["result"]["apiKey"], "[redacted]");
    assert_eq!(result["result"]["env"]["TOKEN"], "[redacted]");
    let ledger = std::fs::read_to_string(dir.path().join("operations.jsonl")).unwrap();
    assert!(!ledger.contains("synthetic-private-body"));
    assert!(!ledger.contains("synthetic-secret"));
    let runner: Runner =
        Arc::new(|_, _| Box::pin(async { panic!("must never replay after restart") }));
    let restarted = Controller::new(catalog(), dir.path(), runner).unwrap();
    let replay = restarted
        .clone()
        .handle(request("call", fields))
        .await
        .unwrap();
    assert_eq!(replay["reused"], true);
    let old = finished(&restarted, id, json!(["write"])).await;
    assert_eq!(old["result_available"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn result_access_requires_original_scope() {
    let (_dir, c, _) = fixture();
    let accepted = c
        .clone()
        .handle(request(
            "call",
            json!({"command":"private","grants":["private"]}),
        ))
        .await
        .unwrap();
    assert_eq!(
        c.clone()
            .handle(request("jobs.get", json!({"job_id":accepted["job"]["id"]})))
            .await
            .unwrap_err()
            .code,
        "SCOPE_REQUIRED"
    );
}
#[test]
fn corrupt_journal_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("operations.jsonl"), b"{partial").unwrap();
    let runner: Runner = Arc::new(|_, _| Box::pin(async { Ok(json!(null)) }));
    assert!(
        matches!(Controller::new(catalog(),dir.path(),runner),Err(error) if error.code=="STATE_CORRUPT")
    );
}
#[tokio::test]
async fn failed_acceptance_never_invokes_business_code() {
    let (dir, c, calls) = fixture();
    std::fs::create_dir(dir.path().join("operations.jsonl")).unwrap();
    assert_eq!(
        c.clone()
            .handle(request("call", json!({"command":"read"})))
            .await
            .unwrap_err()
            .code,
        "STATE_IO"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn four_running_operations_limit_is_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let wait = release.clone();
    let runner: Runner = Arc::new(move |name, _| {
        let wait = wait.clone();
        Box::pin(async move {
            if name == "change" {
                wait.acquire().await.unwrap().forget();
            }
            Ok(json!(true))
        })
    });
    let c = Controller::new(catalog(), dir.path(), runner).unwrap();
    let mut ids = Vec::new();
    for index in 0..4 {
        let accepted = c.clone()
            .handle(request(
                "call",
                json!({"command":"change","grants":["write"],"args":{"testValue":1},"request_id":format!("change-{index}")}),
            ))
            .await
            .unwrap();
        ids.push(accepted["job"]["id"].as_str().unwrap().to_string());
    }
    assert_eq!(
        c.clone()
            .handle(request(
                "call",
                json!({"command":"change","grants":["write"],"args":{"testValue":1},"request_id":"fifth"})
            ))
            .await
            .unwrap_err()
            .code,
        "BUSY"
    );
    for (name, scopes) in [("read", json!([])), ("task_cancel", json!(["write"]))] {
        let accepted = c
            .clone()
            .handle(request(
                "call",
                json!({"command":name,"request_id":name,"grants":scopes}),
            ))
            .await
            .unwrap();
        assert_eq!(
            finished(&c, accepted["job"]["id"].as_str().unwrap(), scopes).await["status"],
            "succeeded"
        );
    }
    release.add_permits(4);
    for id in ids {
        finished(&c, &id, json!(["write"])).await;
    }
}
#[tokio::test]
async fn oversized_results_are_explicit_failures() {
    let dir = tempfile::tempdir().unwrap();
    let runner: Runner =
        Arc::new(|_, _| Box::pin(async { Ok(json!({"text":"x".repeat(2*1024*1024)})) }));
    let c = Controller::new(catalog(), dir.path(), runner).unwrap();
    let accepted = c
        .clone()
        .handle(request("call", json!({"command":"read"})))
        .await
        .unwrap();
    let done = finished(&c, accepted["job"]["id"].as_str().unwrap(), json!([])).await;
    assert_eq!(done["error"]["code"], "RESULT_LIMIT");
}

#[tokio::test]
async fn panic_is_tracked_and_never_silently_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let runner: Runner = Arc::new(|_, _| Box::pin(async { panic!("synthetic business failure") }));
    let c = Controller::new(catalog(), dir.path(), runner).unwrap();
    let accepted = c
        .clone()
        .handle(request("call", json!({"command":"read"})))
        .await
        .unwrap();
    let done = finished(&c, accepted["job"]["id"].as_str().unwrap(), json!([])).await;
    assert_eq!(done["status"], "failed");
    assert_eq!(done["error"]["code"], "COMMAND_PANIC");
    let replay = c
        .clone()
        .handle(request("call", json!({"command":"read"})))
        .await
        .unwrap();
    assert_eq!(replay["reused"], true);
}

#[test]
fn interrupted_runtime_preserves_unknown_outcome_and_deduplication() {
    let dir = tempfile::tempdir().unwrap();
    // 在后台操作尚未调度时停止，确定性模拟“接受后失去运行时”。
    // 多线程 shutdown 会竞争取消/完成处理，偶尔记录 failed 而不是未定结果。
    let first = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let runner: Runner = Arc::new(|_, _| Box::pin(std::future::pending()));
    let c = Controller::new(catalog(), dir.path(), runner).unwrap();
    let accepted = first
        .block_on(c.clone().handle(request("call", json!({"command":"read"}))))
        .unwrap();
    first.shutdown_timeout(Duration::from_millis(20));
    let next = tokio::runtime::Runtime::new().unwrap();
    let runner: Runner =
        Arc::new(|_, _| Box::pin(async { panic!("must never restart an accepted operation") }));
    let restored = Controller::new(catalog(), dir.path(), runner).unwrap();
    let replay = next
        .block_on(
            restored
                .clone()
                .handle(request("call", json!({"command":"read"}))),
        )
        .unwrap();
    assert_eq!(replay["job"]["id"], accepted["job"]["id"]);
    assert_eq!(replay["job"]["status"], "interrupted_unknown");
    assert_eq!(replay["reused"], true);
}

#[tokio::test]
async fn evicted_results_keep_idempotency_records() {
    let (_dir, c, calls) = fixture();
    let mut first = String::new();
    for index in 0..129 {
        let accepted = c
            .clone()
            .handle(request(
                "call",
                json!({"command":"read","request_id":format!("eviction-{index}")}),
            ))
            .await
            .unwrap();
        let id = accepted["job"]["id"].as_str().unwrap();
        if index == 0 {
            first = id.into();
        }
        finished(&c, id, json!([])).await;
    }
    let original = c
        .clone()
        .handle(request("jobs.get", json!({"job_id":first})))
        .await
        .unwrap();
    assert_eq!(original["result_available"], false);
    let replay = c
        .clone()
        .handle(request(
            "call",
            json!({"command":"read","request_id":"eviction-0"}),
        ))
        .await
        .unwrap();
    assert_eq!(replay["reused"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 129);
}

async fn wire(endpoint: &Endpoint, packet: Value) -> Value {
    let mut socket = TcpStream::connect(&endpoint.endpoint).await.unwrap();
    socket
        .write_all(format!("{packet}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}
#[tokio::test]
async fn real_transport_auth_protocol_limits_and_owner_endpoint_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::bind(dir.path(), "fixture-hash").await.unwrap();
    let endpoint: Endpoint =
        serde_json::from_slice(&std::fs::read(dir.path().join("endpoint.json")).unwrap()).unwrap();
    assert!(endpoint.endpoint.starts_with("127.0.0.1:"));
    assert!(
        matches!(Server::bind(dir.path(),"fixture-hash").await,Err(error) if error.code=="ALREADY_RUNNING")
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let handler: Handler = Arc::new(move |_| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"synthetic":true}))
        })
    });
    let task = tokio::spawn(server.serve(handler));
    let base = json!({"protocol":1,"id":"test","token":endpoint.token,"method":"hello"});
    let mut packet = base.clone();
    packet["token"] = json!("wrong");
    assert_eq!(
        wire(&endpoint, packet).await["error"]["code"],
        "UNAUTHORIZED"
    );
    let mut packet = base.clone();
    packet["protocol"] = json!(2);
    assert_eq!(
        wire(&endpoint, packet).await["error"]["code"],
        "PROTOCOL_MISMATCH"
    );
    let mut packet = base.clone();
    packet["unexpected"] = json!(true);
    assert_eq!(
        wire(&endpoint, packet).await["error"]["code"],
        "BAD_REQUEST"
    );
    assert_eq!(wire(&endpoint, base).await["result"]["synthetic"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let mut socket = TcpStream::connect(&endpoint.endpoint).await.unwrap();
    socket
        .write_all(&vec![b'x'; MAX_REQUEST + 1])
        .await
        .unwrap();
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line).await.unwrap();
    assert!(line.contains("REQUEST_LIMIT"));
    assert!(!line.contains(&endpoint.token));
    task.abort();
    let _ = task.await;
    assert!(!dir.path().join("endpoint.json").exists());
}
