use super::*;

fn add_job_turn_fixture(session: &mut Session) {
    session.turns[0].steps[0].result["body"] = json!("合成步骤".repeat(200_000));
    let mut other = session.turns[0].clone();
    other.id = "synthetic-other-turn".into();
    other.request_id = "synthetic-other-request".into();
    other.steps.clear();
    for index in 1..=96 {
        session.jobs.push(Job {
            archived_result_bytes: None,
            id: format!("synthetic-other-job-{index}"),
            command: "system_info".into(),
            turn_id: other.id.clone(),
            state: "succeeded".into(),
            result: json!({"status":"succeeded","text":"合成🙂".repeat(1024),"index":index}),
            owned: true,
            deliver: false,
            artifacts: vec![],
            remote: None,
        });
    }
    let cross_turn_job = &session.jobs[32];
    session.turns[0].steps.push(Step {
        id: "synthetic-cross-turn-step".into(),
        name: "homebase_call".into(),
        state: "completed".into(),
        arguments: json!({"command":"system_info"}),
        result: json!({"job_id":cross_turn_job.id,"state":"succeeded"}),
        job_id: Some(cross_turn_job.id.clone()),
    });
    session.turns.push(other);
}

#[tokio::test]
#[ignore = "explicit synthetic archived job index fixture in F lab"]
async fn seed_native_indexed_job_turn() {
    let home = PathBuf::from(std::env::var_os("HOMEBASE_JOB_LINKS_SEED_HOME").unwrap());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    assert_eq!(
        home.file_name().and_then(|name| name.to_str()),
        Some("indexed-job-profile")
    );
    std::fs::create_dir(&home).unwrap();
    let state_dir = home.join(".homebase/assistant");
    std::fs::create_dir_all(&state_dir).unwrap();
    let (fixture, mut state, executions, models) = completed_task(false, false).await;
    let session = &mut state.sessions[0];
    let artifact = &mut session.jobs[0].artifacts[0];
    let copy = home.join("task-result.md");
    std::fs::copy(fixture.path().join("workspace/task-result.md"), &copy).unwrap();
    artifact.path = copy.to_string_lossy().into_owned();
    add_job_turn_fixture(session);
    let expected = json!({"synthetic_only":true,"turn_id":session.turns[0].id,
        "session_id":session.id,"jobs_total":session.jobs.len(),
        "detail":view(session,&session.turns[0])});
    let _ = stored_task(&state_dir, state).await;
    std::fs::write(
        home.join("task-expected.json"),
        serde_json::to_vec(&expected).unwrap(),
    )
    .unwrap();
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn readonly_turn_skips_unrelated_job_segments_and_keeps_cross_turn_steps() {
    let (dir, mut full, executions, models) = completed_task(false, false).await;
    add_job_turn_fixture(&mut full.sessions[0]);
    full.sessions[0].jobs[0].result["result"] =
        json!({"status":"unknown","text":"合成正文".repeat(100_000)});
    let turn = &mut full.sessions[0].turns[0];
    turn.workflow.plan = None;
    turn.protocol.push(json!({"role":"assistant","content":"合成重放上下文".repeat(18_000)}));
    for (id, name, arguments, result) in [
        (
            "synthetic-read-label",
            "homebase_files",
            json!({"request":{"action":"read","path":"test/原始资料.txt"}}),
            json!({"ok":true}),
        ),
        (
            "synthetic-video-excluded",
            "homebase_video",
            json!({"input":{"title":"没有完成的动画"}}),
            json!({"result":{"state":"failed"}}),
        ),
    ] {
        turn.steps.push(Step {
            id: id.into(),
            name: name.into(),
            state: "completed".into(),
            arguments,
            result,
            job_id: None,
        });
    }
    let original = full.sessions[0].clone();
    let store = stored_task(dir.path(), full).await;
    let workspace = dir.path().join("workspace");
    let reader = engine(store.clone(), workspace.clone(), models.clone()).await;
    let tid = &original.turns[0].id;
    let before = store.lock().await.metrics();
    let foreign = reader
        .read_turn_step("foreign-session", tid, "synthetic-cross-turn-step")
        .await;
    assert!(foreign.err().unwrap().starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert_eq!(store.lock().await.metrics().segment_reads, before.segment_reads);
    let step = reader.read_turn_step(&original.id, tid, "synthetic-cross-turn-step").await.unwrap();
    let original_step = original.turns[0].steps.iter()
        .find(|step| step.id == "synthetic-cross-turn-step").unwrap();
    assert_eq!(serde_json::to_value(step.step).unwrap(),
        serde_json::to_value(original_step).unwrap());
    let after = store.lock().await.metrics();
    assert_eq!(after.segment_reads - before.segment_reads, 1);
    assert_eq!(
        after.turn_range_bytes - before.turn_range_bytes,
        0,
        "single-step read serialized the entire archived turn"
    );
    assert!(reader.state.lock().await.sessions[0].cold.is_some());
    let before_range = store.lock().await.metrics();
    let inspection = reader.inspect_turn_steps(&original.id, tid).await.unwrap();
    let progress = reader.observe_turn(tid).await.unwrap();
    let expected = view(&original, &original.turns[0]);
    assert!(
        inspection.progress == expected.progress,
        "inspection lost progress evidence"
    );
    assert!(inspection.progress.blocked.is_some());
    assert!(inspection.progress.completed.iter().any(|label| label == "读取 原始资料.txt"));
    assert!(inspection.progress.completed.iter().all(|label| !label.contains("没有完成的动画")));
    assert!(progress.progress == expected.progress, "polling lost progress evidence");
    assert_eq!(inspection.text, expected.text);
    assert_eq!(progress.text, expected.text);
    assert_eq!(
        serde_json::to_value(&inspection.artifacts).unwrap(),
        serde_json::to_value(&expected.artifacts).unwrap()
    );
    assert!(inspection.jobs.iter().all(|job| job.result.is_null()));
    let after_range = store.lock().await.metrics();
    assert_eq!(
        after_range.job_range_bytes - before_range.job_range_bytes,
        0,
        "light projections serialized complete cold job results"
    );
    assert_eq!(
        after_range.turn_range_bytes - before_range.turn_range_bytes,
        0,
        "light projections serialized the full archived turn"
    );
    let partial = reader.read_cold_turn(tid, None, None).await.unwrap().unwrap();
    assert_eq!(partial.jobs.len(), 2);
    assert_eq!(partial.jobs[0].id, original.jobs[0].id);
    assert_eq!(partial.jobs[1].id, original.jobs[32].id);
    assert_eq!(
        serde_json::to_value(reader.turn(tid).await.unwrap()).unwrap(),
        serde_json::to_value(view(&original, &original.turns[0])).unwrap()
    );
    assert_eq!(store.lock().await.metrics().segment_reads, 4);
    assert!(reader.state.lock().await.sessions[0].cold.is_some());

    let archive_root = dir.path().join("history-v1");
    let unrelated = std::fs::read_dir(archive_root.join("segments"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let cipher = std::fs::read(&path).unwrap();
            let plain = crate::wechat_crypto::crypt(&cipher, false).unwrap();
            let value: Value = serde_json::from_slice(&plain).unwrap();
            (path, cipher, value)
        })
        .find(|(_, _, data)| data["kind"] == "jobs" && data["start"] == 64)
        .expect("unrelated synthetic job segment");
    let (path, cipher, _) = unrelated;
    std::fs::write(&path, b"damaged synthetic segment").unwrap();
    assert_eq!(reader.turn(tid).await.unwrap().jobs.len(), 1);
    assert!(reader.session(&original.id).await.is_err());
    std::fs::write(path, cipher).unwrap();
    drop(reader);

    let current = store.lock().await.load_index().unwrap().unwrap();
    let mut wrong = current.metadata();
    assert_eq!(wrong["archive_job_turns"][&original.id][32], 1);
    wrong["archive_job_turns"][&original.id][32] = json!(0);
    store
        .lock()
        .await
        .save_partition(wrong, &[(original.id.clone(), current.clone())])
        .await
        .unwrap();
    let wrong_owner = engine(store.clone(), workspace.clone(), models.clone()).await;
    assert!(wrong_owner
        .turn(tid)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INVALID"));
    assert!(wrong_owner
        .inspect_turn_steps(&original.id, tid)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INVALID"));
    assert!(wrong_owner
        .session(&original.id)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INVALID"));
    drop(wrong_owner);

    let mut legacy = current.metadata();
    legacy.as_object_mut().unwrap().remove("archive_job_turns");
    let latest = store.lock().await.load_index().unwrap().unwrap();
    store
        .lock()
        .await
        .save_partition(legacy, &[(original.id.clone(), latest)])
        .await
        .unwrap();
    let old_format_store = Arc::new(Mutex::new(
        Store::new(archive_root, dir.path().join("state.dpapi")).unwrap(),
    ));
    let old_format = engine(old_format_store.clone(), workspace, models.clone()).await;
    assert_eq!(
        serde_json::to_value(old_format.turn(tid).await.unwrap()).unwrap(),
        serde_json::to_value(view(&original, &original.turns[0])).unwrap()
    );
    assert_eq!(old_format_store.lock().await.metrics().segment_reads, 6);
    let before = old_format_store.lock().await.metrics().job_range_bytes;
    let legacy_inspection = old_format
        .inspect_turn_steps(&original.id, tid)
        .await
        .unwrap();
    assert!(legacy_inspection.progress == expected.progress);
    assert_eq!(old_format_store.lock().await.metrics().job_range_bytes, before);
    assert!(old_format.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cold_step_selection_rejects_duplicates_and_preserves_exact_payload() {
    let (dir, mut full, executions, models) = completed_task(false, false).await;
    let session = &mut full.sessions[0];
    let original = session.turns[0].steps[0].clone();
    let mut duplicate = original.clone();
    duplicate.id = "duplicate-cold-step".into();
    session.turns[0].steps.push(duplicate.clone());
    session.turns[0].steps.push(duplicate);
    let (sid, tid) = (session.id.clone(), session.turns[0].id.clone());
    let store = stored_task(dir.path(), full).await;
    let reader = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    assert!(reader.read_turn_step(&sid, &tid, "absent").await.err().unwrap()
        .starts_with("HISTORY_STEP_NOT_FOUND:"));
    assert!(reader.read_turn_step(&sid, &tid, "duplicate-cold-step").await.err().unwrap()
        .starts_with("HISTORY_INVALID_DATA:"));
    let step = reader.read_turn_step(&sid, &tid, &original.id).await.unwrap();
    assert_eq!(serde_json::to_value(step.step).unwrap(), serde_json::to_value(&original).unwrap());
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    assert_eq!(store.lock().await.metrics().turn_range_bytes, 0);
    assert!(reader.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn step_projection_omits_other_bodies_but_checks_original_shapes() {
    let (_, state, _, _) = completed_task(false, false).await;
    let mut row = serde_json::to_value(&state.sessions[0].turns[0]).unwrap();
    row["protocol"] = json!([{"content":"original context"}]);
    row["runtime_results"] = json!({"result":{"text":"original replay"}});
    let project = |row: &Value| project_archived_turn(row, row["steps"].as_array().unwrap(), false);
    let view = project(&row).unwrap();
    assert!(closed_task(&view));
    assert!(view.protocol.is_empty() && view.runtime_results.is_empty() && view.text.is_empty());
    assert!(view.steps.iter().all(|step| step.arguments.is_null() && step.result.is_null()));
    assert_eq!(view.steps[0].job_id, state.sessions[0].turns[0].steps[0].job_id);
    let progress = project_archived_turn(&row, row["steps"].as_array().unwrap(), true).unwrap();
    assert_eq!(progress.text, state.sessions[0].turns[0].text);
    assert!(progress.protocol.is_empty() && progress.runtime_results.is_empty());
    assert_eq!(progress.steps[0].arguments, state.sessions[0].turns[0].steps[0].arguments);
    let mut positional = row.clone();
    let step = &row["steps"][0];
    positional["steps"][0] = json!([step["id"], step["name"], step["state"], step["arguments"], step["result"], step["job_id"]]);
    assert!(serde_json::from_value::<Turn>(positional.clone()).is_ok());
    let header = project(&positional).unwrap().steps.remove(0);
    assert_eq!(header.id, view.steps[0].id);
    assert!(header.arguments.is_null() && header.result.is_null());
    positional["steps"][0][4] = json!({"result":{"status":"partial","payload":"原正文"}});
    let legacy_progress =
        project_archived_turn(&positional, positional["steps"].as_array().unwrap(), true).unwrap();
    assert!(workflow::result_blocks_step(&legacy_progress.steps[0].result));
    assert_eq!(legacy_progress.steps[0].result, json!({"ok":false}));
    assert_eq!(legacy_progress.steps[0].arguments, step["arguments"]);
    for (field, value) in [("protocol", json!("not-an-array")), ("runtime_results", json!([])), ("text", json!(null))] {
        let mut damaged = row.clone();
        damaged[field] = value;
        assert!(serde_json::from_value::<Turn>(damaged.clone()).is_err());
        assert!(project(&damaged).is_err());
    }
    row["steps"][0]["state"] = json!(false);
    assert!(project(&row).is_err());
    row["steps"][0]["state"] = json!("completed");
    row["steps"][0].as_object_mut().unwrap().remove("arguments");
    assert!(serde_json::from_value::<Turn>(row.clone()).is_err());
    assert!(project(&row).is_err());
}

#[tokio::test]
async fn projected_archive_job_checks_original_evidence_and_ownership() {
    let (_, state, _, _) = completed_task(true, false).await;
    let session = &state.sessions[0];
    let job = &session.jobs[0];
    let summary = task_summary(session).unwrap();
    let row = serde_json::to_value(job).unwrap();
    let project = |row: &Value| project_archived_job(row, &job.id, &summary);
    assert!(project(&row).is_ok());
    for (field, value) in [
        ("result", Value::Null),
        ("result", json!({"partial":true})),
        ("result", json!({"complete":false})),
        ("result", json!({"status":"failed"})),
        ("turn_id", json!("foreign-turn")),
        ("command", json!(42)),
    ] {
        let mut changed = row.clone();
        changed[field] = value;
        assert!(project(&changed).err().unwrap().starts_with("ARCHIVE_INVALID"));
    }
    let mut changed = row;
    changed["artifacts"][0]["job_id"] = json!("foreign-job");
    assert!(project(&changed).err().unwrap().starts_with("ARCHIVE_INVALID"));
}

#[tokio::test]
async fn readonly_full_session_and_turn_do_not_adopt_cold_bodies() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let message_view = e.session(&original.id).await.unwrap();
    assert_eq!(
        serde_json::to_value(message_view.messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
    assert!(
        e.state.lock().await.sessions[0].cold.is_some(),
        "read-only session query must not adopt the whole body"
    );
    let reads = store.lock().await.metrics().segment_reads;
    for _ in 0..4 {
        let turn = e.turn(&original.turns[0].id).await.unwrap();
        assert_eq!(turn.id, original.turns[0].id);
        assert_eq!(turn.state, "completed");
        assert_eq!(
            e.session(&original.id).await.unwrap().messages.len(),
            original.messages.len()
        );
    }
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn readonly_wechat_closed_task_keeps_original_result_without_adopting_body() {
    let (dir, mut full, executions, models) = completed_task(true, false).await;
    full.sessions[0].turns[0].steps[0].result["body"] = json!("合成步骤".repeat(120_000));
    full.sessions[0].jobs[0].result["result"] =
        json!({"state":"unknown","body":"合成结果".repeat(80_000)});
    let turn = &mut full.sessions[0].turns[0];
    let unfinished = turn.steps[0].id.clone();
    turn.steps.push(Step {
        id: "synthetic-verified-execute".into(), name: "homebase_terminal".into(), state: "completed".into(),
        arguments: json!({}), result: json!({"result":{"exit_code":0,"stdout":"合成输出".repeat(20_000)}}),
        job_id: None,
    });
    turn.steps.push(Step {
        id: "synthetic-verified-output".into(), name: "homebase_files".into(), state: "completed".into(),
        arguments: json!({"request":{"action":"export"}}), result: json!({"files":[{"name":"task-result.md"}]}),
        job_id: None,
    });
    turn.workflow.plan = Some(workflow::Plan {
        revision: 4, goal: "完成合成多阶段任务".into(),
        items: [
            ("inspect", "核对系统信息", workflow::EvidenceKind::Inspect, unfinished.as_str()),
            ("execute", "处理合成内容", workflow::EvidenceKind::Execute, "synthetic-verified-execute"),
            ("artifact", "登记合成结果", workflow::EvidenceKind::Artifact, "synthetic-verified-output"),
        ].into_iter().map(|(id, title, kind, step)| workflow::Item {
            input: workflow::ItemInput {id:id.into(), title:title.into(), kind, done_when:"核对记录".into()},
            state:"completed".into(), note:String::new(), evidence:vec![step.into()],
        }).collect(),
    });
    let original = full.sessions[0].clone();
    let store = stored_task(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    assert!(e.channel(json!({"operation":"get","source_key":"foreign","turn_id":original.turns[0].id,"include_files":true})).await.is_err());
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    let receipt = e.read_archive_receipt(&original.turns[0].request_id, "wechat", "synthetic-task-owner")
        .await.unwrap().unwrap();
    assert_eq!(receipt.0.turn_id, original.turns[0].id);
    assert_eq!(receipt.1, original.turns[0].fingerprint);
    assert_eq!(store.lock().await.metrics().segment_reads, 1);
    let projected = e.turn(&original.turns[0].id).await.unwrap();
    assert_eq!(projected.progress.completed_stages, 2);
    assert_eq!(projected.progress.total_stages, 3);
    let plan = projected.task.as_ref().unwrap().plan.as_ref().unwrap();
    assert_eq!(plan.revision, 4);
    assert_eq!(plan.items[0].state, "blocked");
    assert_eq!(plan.items[0].evidence, [unfinished]);
    assert_eq!(serde_json::to_value(projected).unwrap(), serde_json::to_value(view(&original, &original.turns[0])).unwrap());
    assert_eq!(store.lock().await.metrics().segment_reads, 2, "the plan already supplies the goal");
    for include_files in [false, true, true] {
        let before = store.lock().await.metrics();
        let response = e.channel(json!({"operation":"get","source_key":"synthetic-task-owner","turn_id":original.turns[0].id,"include_files":include_files})).await.unwrap();
        assert_eq!(response["status"], "succeeded");
        assert_eq!(response["result"]["turn_id"], original.turns[0].id);
        let text = response["result"]["text"].as_str().unwrap();
        assert!(text.contains("结果还未确认"), "{text}");
        assert!(text.contains("已完成 2/3"), "{text}");
        assert!(!text.contains("视频制作：已完成"), "{text}");
        assert_eq!(response["result"]["jobs"][0]["id"], original.jobs[0].id);
        assert_eq!(
            response["result"]["progress_details"],
            serde_json::to_value(progress::snapshot(&original, &original.turns[0])).unwrap(),
        );
        assert!(response["result"]["progress_details"]["blocked"].is_string());
        if include_files {
            assert_eq!(
                response["result"]["artifacts"][0]["sha256"],
                original.jobs[0].artifacts[0].sha256
            );
        } else {
            assert_eq!(response["result"]["artifacts"], json!([]));
        }
        let after = store.lock().await.metrics();
        assert_eq!(after.turn_range_bytes - before.turn_range_bytes, 0, "WeChat re-encoded the archived turn");
        assert_eq!(after.job_range_bytes - before.job_range_bytes, 0, "WeChat re-encoded the archived job");
        assert!(
            e.state.lock().await.sessions[0].cold.is_some(),
            "WeChat result query must not adopt a closed task body"
        );
        let page = e
            .session_window(window::TimelineRequest {
                session_id: original.id.clone(),
                before_message_id: None,
                limit: 40,
            })
            .await
            .unwrap();
        assert_eq!(page.messages.len(), original.messages.len());
        assert!(
            e.state.lock().await.sessions[0].cold.is_some(),
            "desktop task window must also remain read-only"
        );
    }
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert!(store.lock().await.metrics().segment_reads <= 4);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn readonly_wechat_cold_view_reads_original_evidence_for_a_late_followup() {
    let (dir, mut full, executions, models) = completed_task(true, false).await;
    let source = "synthetic-task-owner";
    full.sessions[0].turns[0].steps = vec![Step {
        id: "synthetic-inspection".into(),
        name: "homebase_files".into(),
        state: "completed".into(),
        arguments: json!({"request":{"action":"read","path":"synthetic.txt"}}),
        result: json!({"status":"succeeded","body":"原始证据".repeat(70_000),"checked_at":"fixture"}),
        job_id: None,
    }];
    let original = full.sessions[0].clone();
    let turn = &original.turns[0];
    let store = stored_task(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), models.clone()).await;
    let request = json!({"operation":"get","source_key":source,"turn_id":turn.id,"include_files":false});
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let reader = store.lock().await;
    let before = reader.metrics();
    let mut pending = Box::pin(e.channel(request.clone()));
    tokio::select! {
        biased;
        result = &mut pending => panic!("read completed before the fixture released its archive: {result:?}"),
        _ = tokio::task::yield_now() => {}
    }
    let expected = {
        let mut state = tokio::time::timeout(Duration::from_secs(5), e.state.lock())
            .await.unwrap();
        assert!(state.sessions[0].cold.is_some());
        // Populate a completed synthetic follow-up without scheduling workers
        // while the test holds the archive lock.
        state.followups.entries.push(serde_json::from_value(json!({
            "id":"synthetic-followup","title":"合成只读复查","instruction":"检查示例资料",
            "due_at":now(),"interval_minutes":null,"max_runs":1,"enabled":false,
            "state":"completed","notice":"","source":"wechat","source_key":source,
            "settings_revision":0,"local_revision":0,"notification_policy":"meaningful",
            "runs":[{
                "ordinal":1,"scheduled_at":now(),"request_id":"synthetic-followup-run",
                "turn_id":turn.id,"state":"completed"
            }]
        })).unwrap());
        notifications::metadata(&state, &original, turn, false)
    };
    assert_eq!(expected["policy"], "meaningful");
    drop(reader);

    let response = tokio::time::timeout(Duration::from_secs(10), pending)
        .await.unwrap().unwrap();
    assert_eq!(response["result"]["notification"], expected);
    assert_eq!(e.channel(request).await.unwrap(), response);
    assert!(store.lock().await.metrics().turn_range_bytes > before.turn_range_bytes);
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(), head);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn readonly_queries_concurrent_with_save_preserve_the_new_revision_and_restart() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let revision = e.state.lock().await.revision;
    let mut settings = e.settings().await;
    settings.max_tokens = 3011;
    let (reads, saved) = tokio::join!(
        async {
            for _ in 0..12 {
                assert_eq!(
                    e.session(&original.id).await.unwrap().messages.len(),
                    original.messages.len()
                );
                assert_eq!(
                    e.turn(&original.turns[0].id).await.unwrap().state,
                    "completed"
                );
            }
        },
        e.save_settings(settings)
    );
    let _ = reads;
    saved.unwrap();
    assert_eq!(e.state.lock().await.revision, revision + 1);
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.cold.is_some()));
    drop(e);
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    assert_eq!(e.settings().await.max_tokens, 3011);
    assert_eq!(
        serde_json::to_value(e.session(&original.id).await.unwrap().messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.cold.is_some()));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn readonly_queries_after_failed_save_keep_original_state_and_cipher() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let state = load(store.clone(), snapshot).await.unwrap();
    let e = Engine::new(
        state,
        Arc::new(|_| Box::pin(async { Err("synthetic disk full".into()) })),
        Arc::new(|_| Box::pin(async { panic!("read-only history must not call a model") })),
        Arc::new(|_| Box::pin(async { panic!("read-only history must not execute") })),
        dir.path().join("workspace"),
    )
    .await
    .unwrap();
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let mut settings = e.settings().await;
    settings.max_tokens = 3012;
    assert_eq!(
        e.save_settings(settings).await.err().unwrap(),
        "synthetic disk full"
    );
    assert_eq!(
        e.session(&original.id).await.unwrap().messages.len(),
        original.messages.len()
    );
    assert_eq!(
        e.turn(&original.turns[0].id).await.unwrap().state,
        "completed"
    );
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.cold.is_some()));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn readonly_duplicate_ids_fail_before_body_reads_and_cached_reads_check_legacy_anchor() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    {
        let mut state = e.state.lock().await;
        let mut duplicate = state.sessions[0].clone();
        duplicate.id = "duplicate-turn-in-other-session".into();
        state.sessions.push(duplicate);
    }
    assert!(e
        .turn(&original.turns[0].id)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INVALID:"));
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    e.state.lock().await.sessions.pop();
    e.session(&original.id).await.unwrap();
    let reads = store.lock().await.metrics().segment_reads;
    std::fs::write(
        dir.path().join("state.dpapi"),
        b"synthetic external legacy change",
    )
    .unwrap();
    assert!(e
        .turn(&original.turns[0].id)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_LEGACY_CONFLICT:"));
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.cold.is_some()));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn browsing_many_original_sessions_does_not_accumulate_body_bytes_in_state() {
    let (dir, mut full, calls) = filled().await;
    let template = full.sessions[0].clone();
    full.sessions.clear();
    for n in 0..36 {
        let mut session = template.clone();
        session.id = format!("residency-session-{n}");
        let tid = format!("residency-turn-{n}");
        session.turns[0].id = tid.clone();
        session.turns[0].request_id = format!("residency-request-{n}");
        for (i, message) in session.messages.iter_mut().enumerate() {
            message.id = format!("residency-message-{n}-{i}");
            message.turn_id = Some(tid.clone());
            message.content = format!("原始合成正文🙂{n}-{i}{}", "x".repeat(48 * 1024));
        }
        session.turns[0].text = session.messages[1].content.clone();
        full.sessions.push(session);
    }
    let full_bytes = serde_json::to_vec(&full).unwrap().len();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls.clone()).await;
    let before = serde_json::to_vec(&*e.state.lock().await).unwrap();
    assert!(full_bytes > 4 * 1024 * 1024);
    assert!(before.len() * 10 < full_bytes);
    for n in 0..36 {
        let session = e.session(&format!("residency-session-{n}")).await.unwrap();
        assert_eq!(session.messages.len(), 2);
        assert!(session.messages[0]
            .content
            .starts_with(&format!("原始合成正文🙂{n}-0")));
        assert!(session.messages[0]
            .content
            .ends_with(&"x".repeat(48 * 1024)));
    }
    assert_eq!(serde_json::to_vec(&*e.state.lock().await).unwrap(), before);
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        36
    );
    let reads = store.lock().await.metrics().segment_reads;
    e.session("residency-session-35").await.unwrap();
    assert_eq!(store.lock().await.metrics().segment_reads, reads);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
