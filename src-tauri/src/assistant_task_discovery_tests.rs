use super::*;

async fn catalog_fixture() -> (Arc<Engine>, Arc<Mutex<State>>, tempfile::TempDir) {
    let (engine, _, executions, saved, root) = fixture(true, vec![reply(vec![])]).await;
    let ack = engine
        .send(request("隔离任务发现", "metadata-session"))
        .await
        .unwrap();
    wait(&engine, &ack.turn_id, "completed").await;
    for n in 0..12 {
        let accepted = engine
            .control(
                "call",
                Some("system_info"),
                json!({}),
                &format!("external-{n}"),
                None,
            )
            .await
            .unwrap();
        let id = accepted["job"]["id"].as_str().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let job = engine
                    .control("jobs.get", None, json!({}), "metadata-poll", Some(id))
                    .await
                    .unwrap();
                if job["status"] == "succeeded" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    assert_eq!(executions.load(Ordering::SeqCst), 12);
    (engine, saved, root)
}

#[tokio::test]
async fn task_discovery_is_one_evidence_save_without_registering_foreign_jobs() {
    let (engine, saved, _root) = catalog_fixture().await;
    let tid = saved.lock().await.sessions[0].turns[0].id.clone();
    let before = saved.lock().await.clone();
    let result = engine
        .execute(
            &tid,
            &ToolCall {
                id: "metadata-list".into(),
                name: "homebase_jobs".into(),
                arguments: json!({}),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["jobs"].as_array().unwrap().len(), 12);
    let after = saved.lock().await.clone();
    assert!(
        after.sessions[0].jobs.is_empty(),
        "browsing metadata must not claim foreign jobs or occupy execution slots"
    );
    assert_eq!(
        after.revision,
        before.revision + 1,
        "one tool evidence save, not one encrypted full-state write per row"
    );
    assert_eq!(
        after.sessions[0].turns[0].steps.len(),
        before.sessions[0].turns[0].steps.len() + 1
    );
}

#[tokio::test]
async fn task_discovery_still_works_when_the_owned_job_index_is_full() {
    let (engine, saved, _root) = catalog_fixture().await;
    let tid = saved.lock().await.sessions[0].turns[0].id.clone();
    engine
        .change_turn(&tid, |s, _| {
            s.jobs = (0..100)
                .map(|n| Job {
                    id: format!("owned-{n}"),
                    archived_result_bytes: None,
                    command: "system_info".into(),
                    turn_id: tid.clone(),
                    state: "succeeded".into(),
                    result: json!({"status":"succeeded","marker":n}),
                    owned: true,
                    deliver: false,
                    artifacts: vec![],
                    remote: None,
                })
                .collect();
            Ok(())
        })
        .await
        .unwrap();
    let before = serde_json::to_value(&saved.lock().await.sessions[0].jobs).unwrap();
    let result = engine
        .execute(
            &tid,
            &ToolCall {
                id: "full-index-list".into(),
                name: "homebase_jobs".into(),
                arguments: json!({}),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["jobs"].as_array().unwrap().len(), 12);
    assert_eq!(
        serde_json::to_value(&saved.lock().await.sessions[0].jobs).unwrap(),
        before
    );
}

#[tokio::test]
async fn task_discovery_legacy_observations_do_not_consume_owned_slots_or_become_owned() {
    let (engine, saved, _root) = catalog_fixture().await;
    let tid = saved.lock().await.sessions[0].turns[0].id.clone();
    engine
        .change_turn(&tid, |s, t| {
            s.turns[t].state = "running".into();
            s.jobs = (0..100)
                .map(|n| Job {
                    id: format!("legacy-observed-{n}"),
                    archived_result_bytes: None,
                    command: "system_info".into(),
                    turn_id: String::new(),
                    state: "succeeded".into(),
                    result: Value::Null,
                    owned: false,
                    deliver: false,
                    artifacts: vec![],
                    remote: None,
                })
                .collect();
            Ok(())
        })
        .await
        .unwrap();
    let result = engine
        .execute(
            &tid,
            &ToolCall {
                id: "actual-owned-query".into(),
                name: "homebase_call".into(),
                arguments: json!({"command":"system_info","args":{}}),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["state"], "succeeded");
    let state = saved.lock().await;
    assert_eq!(state.sessions[0].jobs.iter().filter(|j| j.owned).count(), 1);
    assert_eq!(
        state.sessions[0].jobs.iter().filter(|j| !j.owned).count(),
        100
    );
}

#[tokio::test]
async fn task_discovery_private_result_uses_saved_evidence_confirmation_and_restart_without_execution(
) {
    let (base, saved, _root) = catalog_fixture().await;
    let private = base
        .control(
            "call",
            Some("system_processes"),
            json!({}),
            "private-observed-job",
            None,
        )
        .await
        .unwrap();
    let id = private["job"]["id"].as_str().unwrap().to_string();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let value = base
                .control("jobs.get", None, json!({}), "private-ready", Some(&id))
                .await
                .unwrap();
            if value["status"] == "succeeded" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let models = Arc::new(AtomicUsize::new(0));
    let model: Model = Arc::new({
        let models = models.clone();
        move |request| {
            let n = models.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(match n {
                    0 => workflow_tests::tool_reply(n, "homebase_jobs", json!({})),
                    1 => {
                        let result: Value = serde_json::from_str(
                            request
                                .messages
                                .iter()
                                .rev()
                                .find(|m| m["role"] == "tool")
                                .unwrap()["content"]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap();
                        let row = result["jobs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|row| row["command"] == "system_processes")
                            .unwrap();
                        assert!(
                            row.get("result").is_none(),
                            "metadata must not expose private output before confirmation"
                        );
                        workflow_tests::tool_reply(
                            n,
                            "homebase_job",
                            json!({"job_id":row["job_id"]}),
                        )
                    }
                    2 => {
                        let result: Value = serde_json::from_str(
                            request
                                .messages
                                .iter()
                                .rev()
                                .find(|m| m["role"] == "tool")
                                .unwrap()["content"]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(result["query_only"], true);
                        assert_eq!(result["owned"], false);
                        assert_eq!(result["state"], "succeeded");
                        reply(vec![])
                    }
                    _ => panic!("duplicate model inference after original checkpoint recovery"),
                })
            })
        }
    });
    let queries = Arc::new(AtomicUsize::new(0));
    let submissions = Arc::new(AtomicUsize::new(0));
    let control: Control = Arc::new({
        let original = base.control.clone();
        let queries = queries.clone();
        let submissions = submissions.clone();
        move |request| {
            let original = original.clone();
            let queries = queries.clone();
            let submissions = submissions.clone();
            Box::pin(async move {
                if request.method == "jobs.get" {
                    queries.fetch_add(1, Ordering::SeqCst);
                }
                if request.method == "call" {
                    submissions.fetch_add(1, Ordering::SeqCst);
                }
                original(request).await
            })
        }
    });
    let mut state = saved.lock().await.clone();
    state.settings.runtime = "pi".into();
    let engine = Engine::new(
        state,
        base.save.clone(),
        model.clone(),
        control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let incoming = json!({"operation":"submit","source_key":"observing-wechat","request_id":"observed-original-message","text":"查看最近任务，查询进程任务的私人结果。只查询原编号，不重新执行。"});
    let ack = engine.channel(incoming.clone()).await.unwrap();
    let tid = ack["turn_id"].as_str().unwrap();
    let pending = workflow_tests::task_wait(&engine, tid, "awaiting_confirmation").await;
    assert_eq!(queries.load(Ordering::SeqCst), 0);
    assert_eq!(models.load(Ordering::SeqCst), 2);
    let confirmation = pending.pending.unwrap().id;
    let state = saved.lock().await.clone();
    assert!(state
        .sessions
        .iter()
        .find(|s| s.id == ack["session_id"])
        .unwrap()
        .jobs
        .is_empty());
    let restored = Engine::new(
        state,
        base.save.clone(),
        model,
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(restored.channel(json!({"operation":"submit","source_key":"other-source","request_id":"foreign-confirmation","text":format!("确认 {confirmation}")})).await.is_err());
    assert_eq!(queries.load(Ordering::SeqCst), 0);
    restored.channel(json!({"operation":"submit","source_key":"observing-wechat","request_id":"correct-confirmation","text":format!("确认 {confirmation}")})).await.unwrap();
    let done = workflow_tests::task_wait(&restored, tid, "completed").await;
    assert_eq!(queries.load(Ordering::SeqCst), 1);
    assert_eq!(submissions.load(Ordering::SeqCst), 0);
    assert_eq!(models.load(Ordering::SeqCst), 3);
    assert!(done.jobs.is_empty());
    assert!(done.artifacts.is_empty());
    let from_channel = restored
        .channel(json!({"operation":"get","source_key":"observing-wechat","turn_id":tid}))
        .await
        .unwrap();
    assert_eq!(from_channel["result"]["turn_id"], done.id);
    assert_eq!(from_channel["result"]["status"], "succeeded");
    assert!(from_channel["result"]["jobs"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(restored.channel(incoming).await.unwrap()["turn_id"], tid);
    assert_eq!(queries.load(Ordering::SeqCst), 1);
    assert!(restored
        .execute(
            tid,
            &ToolCall {
                id: "guess-other-job".into(),
                name: "homebase_job".into(),
                arguments: json!({"job_id":"not-discovered"})
            }
        )
        .await
        .unwrap_err()
        .contains("未发现"));
    let delivery = restored
        .execute(
            tid,
            &ToolCall {
                id: "deliver-observed".into(),
                name: "homebase_deliver".into(),
                arguments: json!({"job_id":id}),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery["code"], "JOB_NOT_OWNED");
    assert_eq!(queries.load(Ordering::SeqCst), 1);
    assert_eq!(submissions.load(Ordering::SeqCst), 0);
}
