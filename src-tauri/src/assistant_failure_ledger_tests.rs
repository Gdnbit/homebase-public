//! Synthetic model/control boundaries; the real graph runs without keys or messages.
use super::*;

#[tokio::test]
async fn failure_ledger_one_feedback_per_continuation_prevents_model_spinning() {
    let (e, models, controls, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("合成恢复反馈预算", "ledger-feedback"))
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].workflow.resume_generation = 1;
        s.turns[t]
            .workflow
            .tool_failures
            .insert("original-effect".into(), 1);
        Ok(())
    })
    .await
    .unwrap();
    let call = ToolCall {
        id: "first-blocked".into(),
        name: "homebase_terminal".into(),
        arguments: json!({"command":"synthetic-only"}),
    };
    let blocked = e
        .execute_guarded(&ack.turn_id, "original-effect", &call)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(blocked["status"], "failed");
    assert_eq!(blocked["executed"], false);
    let changed_id = ToolCall {
        id: "new-id-same-effect".into(),
        ..call.clone()
    };
    assert!(e
        .execute_guarded(&ack.turn_id, "original-effect", &changed_id)
        .await
        .unwrap_err()
        .starts_with("HB_STALLED_EFFECT:"));
    let state = e.state.lock().await;
    let (s, t) = locate(&state, &ack.turn_id).unwrap();
    assert_eq!(state.sessions[s].turns[t].steps.len(), 1);
    assert!(!workflow::successful(
        &state.sessions[s].turns[t].steps[0],
        &state.sessions[s].jobs
    ));
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(controls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failure_ledger_wechat_restart_queries_original_failed_write_without_another_effect() {
    let (base, _, _, _, _root) = fixture(false, vec![]).await;
    let executions = Arc::new(AtomicUsize::new(0));
    let effects = executions.clone();
    let runner: Runner = Arc::new(move |command, _| {
        let count = effects.clone();
        Box::pin(async move {
            assert_eq!(command, "assistant_local_execute");
            count.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"status":"failed","message":"合成写入失败，可能留下部分结果"}))
        })
    });
    let controller = Controller::new(catalog().clone(), _root.path(), runner).unwrap();
    let control: Control = Arc::new(move |r| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(r).await })
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let model: Model = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            let n = count.fetch_add(1, Ordering::SeqCst);
            Ok(match n {
                0..=1 => workflow_tests::tool_reply(n, "homebase_files", json!({"action":"write","path":"synthetic-only.txt","content":"synthetic"})),
                2 => {
                    let job = r.messages.iter().filter(|m| m["role"] == "tool")
                        .filter_map(|m| serde_json::from_str::<Value>(m["content"].as_str()?).ok())
                        .find_map(|v| v["job_id"].as_str().map(str::to_string)).unwrap();
                    workflow_tests::tool_reply(n, "homebase_job", json!({"job_id":job}))
                }
                3 => wire::parse("chat", &json!({"choices":[{"message":{"content":"原写入没有成功，可能留下的部分结果已保留。我核对了原任务，没有重新写入。"}}]})).unwrap(),
                _ => panic!("unexpected model retry"),
            })
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let e = Engine::new(
        state,
        base.save.clone(),
        model,
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let original = json!({"operation":"submit","source_key":"write-ledger-owner","request_id":"write-ledger-original","text":"写入合成文件并核对实际结果"});
    let first = e.channel(original.clone()).await.unwrap();
    let tid = first["turn_id"].as_str().unwrap();
    let stopped = workflow_tests::task_wait(&e, tid, "needs_input").await;
    assert_eq!(stopped.jobs.len(), 1);
    let original_job = stopped.jobs[0].id.clone();
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    for _ in 0..100 {
        if !e.running.lock().await.contains_key(tid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!e.running.lock().await.contains_key(tid));
    let checkpoint = e.state.lock().await.clone();
    // Exercise the real OS-protected format, not just an in-memory clone.
    let history = _root.path().join("history-v1");
    let legacy = _root.path().join("unused-ledger-legacy.dpapi");
    let mut protected =
        crate::assistant_state_store::Store::new(history.clone(), legacy.clone()).unwrap();
    assert!(protected.load_index().unwrap().is_none());
    protected
        .save(serde_json::to_value(checkpoint).unwrap())
        .await
        .unwrap();
    drop(protected);
    let mut reopened = crate::assistant_state_store::Store::new(history, legacy).unwrap();
    let snapshot = reopened.load_index().unwrap().unwrap();
    let checkpoint: State =
        serde_json::from_value(reopened.materialize(&snapshot).unwrap()).unwrap();
    let restored = Engine::new(
        checkpoint,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    let continuation = json!({"operation":"submit","source_key":"write-ledger-owner","request_id":"write-ledger-continue","text":format!("继续 {tid}")});
    restored.channel(continuation.clone()).await.unwrap();
    let done = workflow_tests::task_wait(&restored, tid, "completed").await;
    restored.channel(continuation).await.unwrap();
    assert_eq!(restored.channel(original).await.unwrap()["turn_id"], tid);
    let result = restored
        .channel(json!({"operation":"get","source_key":"write-ledger-owner","turn_id":tid}))
        .await
        .unwrap();
    assert!(result["result"]["text"]
        .as_str()
        .unwrap()
        .contains("没有重新写入"));
    assert_eq!(done.jobs.len(), 1);
    assert_eq!(done.jobs[0].id, original_job);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert!(done
        .steps
        .iter()
        .any(|s| s.result["code"] == "TOOL_RETRY_BLOCKED" && s.result["executed"] == false));
}

#[tokio::test]
async fn failure_ledger_retains_original_effect_after_more_than_32_distinct_failures() {
    let (e, _, _, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("合成失败台账容量", "ledger-capacity"))
        .await
        .unwrap();
    let call = ToolCall {
        id: "effect".into(),
        name: "homebase_terminal".into(),
        arguments: json!({"command":"synthetic-only"}),
    };
    for n in 0..35 {
        e.record_tool_outcome(
            &ack.turn_id,
            &format!("failure-{n}"),
            &call,
            &mut json!({"status":"failed"}),
        )
        .await
        .unwrap();
    }
    assert!(e
        .check_repeated_failure(&ack.turn_id, "failure-0", &call)
        .await
        .unwrap_err()
        .starts_with("HB_STALLED_EFFECT:"));
    let state = e.state.lock().await;
    let (s, t) = locate(&state, &ack.turn_id).unwrap();
    assert_eq!(state.sessions[s].turns[t].workflow.tool_failures.len(), 35);
}

#[tokio::test]
async fn failure_ledger_explicit_continue_feeds_blocked_call_back_without_reexecuting_it() {
    let (base, _, _, _, _root) = fixture(false, vec![]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let reads = Arc::new(AtomicUsize::new(0));
    let read_count = reads.clone();
    let blocked = Arc::new(AtomicUsize::new(0));
    let saw_blocked = blocked.clone();
    let model: Model = Arc::new(move |r| {
        let count = count.clone();
        let saw_blocked = saw_blocked.clone();
        Box::pin(async move {
            let n = count.fetch_add(1, Ordering::SeqCst);
            Ok(match n {
                0..=3 => workflow_tests::tool_reply(
                    n,
                    "homebase_files",
                    json!({"action":"read","path":"missing-fixture.txt"}),
                ),
                4 => {
                    if serde_json::to_string(&r.messages)
                        .unwrap()
                        .contains("TOOL_RETRY_BLOCKED")
                    {
                        saw_blocked.fetch_add(1, Ordering::SeqCst);
                    }
                    workflow_tests::tool_reply(
                        n,
                        "homebase_files",
                        json!({"action":"read","path":"corrected-fixture.txt"}),
                    )
                }
                5 => reply(vec![]),
                _ => panic!("duplicate model execution"),
            })
        })
    });
    let control: Control = Arc::new(move |r| {
        let count = read_count.clone();
        Box::pin(async move {
            assert_eq!(r.method, "assistant.local_read");
            count.fetch_add(1, Ordering::SeqCst);
            Ok(if r.args["request"]["path"] == "missing-fixture.txt" {
                json!({"status":"failed","message":"合成文件尚不存在"})
            } else {
                json!({"status":"succeeded","content":"合成修正结果"})
            })
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let e = Engine::new(
        state,
        base.save.clone(),
        model,
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let ack=e.channel(json!({"operation":"submit","source_key":"ledger-owner","request_id":"ledger-original","text":"读取合成文件并核对结果"})).await.unwrap();
    let tid = ack["turn_id"].as_str().unwrap();
    workflow_tests::task_wait(&e, tid, "needs_input").await;
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    for _ in 0..100 {
        if !e.running.lock().await.contains_key(tid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!e.running.lock().await.contains_key(tid));
    let checkpoint = e.state.lock().await.clone();
    let restored = Engine::new(
        checkpoint,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    let continuation = json!({"operation":"submit","source_key":"ledger-owner","request_id":"ledger-continue","text":format!("继续 {tid}")});
    restored.channel(continuation.clone()).await.unwrap();
    workflow_tests::task_wait(&restored, tid, "completed").await;
    let model_count = calls.load(Ordering::SeqCst);
    restored.channel(continuation).await.unwrap();
    let result = restored
        .channel(json!({"operation":"get","source_key":"ledger-owner","turn_id":tid}))
        .await
        .unwrap();
    assert_eq!(result["result"]["status"], "succeeded");
    assert!(result["result"]["text"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty()));
    assert!(restored
        .channel(json!({"operation":"get","source_key":"other-owner","turn_id":tid}))
        .await
        .is_err());
    assert_eq!(restored.turn(tid).await.unwrap().id, tid);
    assert_eq!(calls.load(Ordering::SeqCst), model_count);
    assert_eq!(
        blocked.load(Ordering::SeqCst),
        1,
        "continued graph must receive a nonexecuted failure, not retry the old read"
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        4,
        "three original failures plus one corrected read; no fourth old read"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn failure_ledger_full_rejects_new_calls_and_preserves_every_original_counter() {
    let (e, models, controls, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("合成失败保护满额", "ledger-full"))
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].workflow.tool_failures = (0..workflow::MAX_TASK_STEPS)
            .map(|i| (format!("failed-{i}"), 1))
            .collect();
        Ok(())
    })
    .await
    .unwrap();
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let call = ToolCall {
        id: "must-not-execute".into(),
        name: "homebase_terminal".into(),
        arguments: json!({"command":"synthetic"}),
    };
    assert!(e
        .execute_guarded(&ack.turn_id, "new-failure", &call)
        .await
        .unwrap_err()
        .starts_with("HB_STALLED_LEDGER:"));
    assert!(e
        .record_tool_outcome(
            &ack.turn_id,
            "new-failure",
            &call,
            &mut json!({"status":"failed"})
        )
        .await
        .unwrap_err()
        .starts_with("HB_STALLED_LEDGER:"));
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(controls.load(Ordering::SeqCst), 0);
}
