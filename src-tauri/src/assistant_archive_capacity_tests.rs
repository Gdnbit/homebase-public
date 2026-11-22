use super::*;
#[path = "assistant_source_controls_tests.rs"]
mod source_controls_tests;
#[path = "assistant_control_receipt_residency_tests.rs"]
mod control_receipt_residency_tests;
#[path = "assistant_live_messages_tests.rs"]
mod live_message_tests;

#[tokio::test]
async fn receipt_queries_and_duplicate_sends_do_not_restore_writable_bodies() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store, dir.path().join("workspace"), calls.clone()).await;
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let receipt = e
        .desktop_receipt(&request(None, "仅合成的闲聊", "cold-original-0"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.turn_id, original.turns[0].id);
    assert!(
        e.state.lock().await.sessions[0].cold.is_some(),
        "receipt lookup must not retain the whole original body"
    );
    assert_eq!(
        e.send(request(None, "仅合成的闲聊", "cold-original-0"))
            .await
            .unwrap()
            .turn_id,
        original.turns[0].id
    );
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

async fn capacity_store(dir: &std::path::Path, mut state: State) -> (Arc<Mutex<Store>>, State) {
    let mut writer = Store::new(dir.join("history-v1"), dir.join("state.dpapi")).unwrap();
    writer.enable_capacity_segments();
    assert!(writer.load_index().unwrap().is_none());
    let store = Arc::new(Mutex::new(writer));
    enable_capacity(&mut state);
    let state = commit(store.clone(), state, Vec::new()).await.unwrap();
    (store, state)
}

async fn capacity_engine(
    store: Arc<Mutex<Store>>,
    state: State,
    workspace: PathBuf,
) -> Arc<Engine> {
    let save: Commit = Arc::new(move |state, protected| {
        let store = store.clone();
        Box::pin(async move { commit(store, state, protected).await })
    });
    let documents = Arc::new(crate::assistant_documents::DocumentStore::new(
        workspace.join(".documents"),
    ));
    Engine::new_with_commit(
        state,
        save,
        Arc::new(|_| Box::pin(async { panic!("capacity maintenance must not invoke a model") })),
        Arc::new(|_| Box::pin(async { panic!("capacity maintenance must not execute business") })),
        workspace,
        documents,
    )
    .await
    .unwrap()
}

fn populate_running_job_results(session: &mut Session, count: usize) {
    let turn = &mut session.turns[0];
    turn.state = "running".into();
    turn.history_committed = false;
    turn.finished_at = None;
    for position in 0..count {
        let job_id = format!("synthetic-running-job-{position}");
        let step_id = format!("synthetic-running-step-{position}");
        let mut job = session.jobs[0].clone();
        job.id = job_id.clone();
        job.artifacts.clear();
        job.result = json!({"status":"succeeded","ordinal":position,"text":"合成🙂".repeat(76_000)});
        session.jobs.push(job);
        turn.steps.push(Step {
            id: step_id.clone(),
            name: "homebase_call".into(),
            state: "completed".into(),
            arguments: json!({"command":"system_info","args":{}}),
            result: json!({"state":"succeeded","job_id":job_id,"step_id":step_id}),
            job_id: Some(job_id),
        });
    }
}

fn populate_completed_graph_replay(session: &mut Session, count: usize) -> Turn {
    let mut original = session.turns[0].clone();
    original.runtime_operations = vec![json!({"checkpoint": "x".repeat(700_000)})].into();
    original.runtime_models.insert("original-model".into(), json!("y".repeat(600_000)));
    original.runtime_results.insert("original-tool".into(), json!("z".repeat(600_000)));
    assert!(closed_task(&original));
    session.turns[0] = original.clone();
    for position in 1..count {
        let mut turn = original.clone();
        turn.id = format!("synthetic-replay-turn-{position}");
        turn.request_id = format!("synthetic-replay-request-{position}");
        session.turns.push(turn);
    }
    original
}

#[tokio::test]
async fn completed_graph_replay_fits_in_active_session_and_survives_restart() {
    let (dir, mut state, _, _) = completed_task(false, false).await;
    let session = &mut state.sessions[0];
    populate_completed_graph_replay(session, 15);
    let mut unknown = session.turns[0].clone();
    // A pair of individually valid records can exceed the store's range budget.
    for position in 0..2 {
        session.turns[position]
            .runtime_models
            .insert("large-model-reply".into(), json!("m".repeat(16 * 1024 * 1024)));
    }
    let original = session.turns[0].clone();
    unknown.id = "synthetic-unknown-turn".into();
    unknown.request_id = "synthetic-unknown-request".into();
    unknown.state = "interrupted_unknown".into();
    unknown.runtime_uncertain_model = true;
    session.turns.push(unknown.clone());
    let sid = session.id.clone();
    let original_value = serde_json::to_value(&original).unwrap();
    let unknown_value = serde_json::to_value(&unknown).unwrap();
    enable_capacity(&mut state);
    let logical = capacity::encoded_bytes(&state).unwrap();
    assert!(logical > capacity::RESIDENT_BYTES);

    let (store, mut committed) = capacity_store(dir.path(), state).await;
    assert!(capacity::encoded_bytes(&committed).unwrap() < 3 * 1024 * 1024);
    assert_eq!(logical_bytes(&committed).unwrap(), logical);
    assert_eq!(serde_json::to_value(&committed.sessions[0].turns[15]).unwrap(), unknown_value);
    assert_eq!(committed.sessions[0].turns[0].steps[0].result, original.steps[0].result);

    // A metadata-only save can reuse the original segment. An edit to a turn
    // must merge the original replay fields with the current visible fields.
    let segment_files = || {
        std::fs::read_dir(dir.path().join("history-v1/segments"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>()
    };
    let before = segment_files();
    committed.sessions[0].title = "合成标题".into();
    committed = commit(store.clone(), committed, Vec::new()).await.unwrap();
    assert_eq!(before, segment_files());
    committed.sessions[0].turns[0].text = "保留合成回执的编辑".into();
    committed = commit(store.clone(), committed, Vec::new()).await.unwrap();
    let after = committed.archive_base.clone().unwrap();
    let mut edited = original_value;
    edited["text"] = json!("保留合成回执的编辑");
    assert_eq!(
        store.lock().await.read_record(&after, &sid, Kind::Turns, &original.id).unwrap(),
        edited
    );
    assert_eq!(store.lock().await.read_record(&after, &sid, Kind::Turns, &unknown.id).unwrap(), unknown_value);

    let reopened = Arc::new(Mutex::new(Store::new(dir.path().join("history-v1"), dir.path().join("state.dpapi")).unwrap()));
    let snapshot = reopened.lock().await.load_index().unwrap().unwrap();
    let restarted = load(reopened, snapshot).await.unwrap();
    assert!(capacity::encoded_bytes(&restarted).unwrap() < 3 * 1024 * 1024);
    assert_eq!(logical_bytes(&restarted).unwrap(), logical_bytes(&committed).unwrap());
    assert_eq!(serde_json::to_value(&restarted.sessions[0].turns[15]).unwrap(), unknown_value);
    assert_eq!(restarted.sessions[0].turns[0].text, "保留合成回执的编辑");
}

#[tokio::test]
async fn cold_replay_reopens_and_restores_a_newly_protected_turn_atomically() {
    let (dir, mut state, _, _) = completed_task(false, false).await;
    let original = populate_completed_graph_replay(&mut state.sessions[0], 12);
    let sid = state.sessions[0].id.clone();
    let (store, state) = capacity_store(dir.path(), state).await;
    assert!(state.sessions[0].cold.is_some());
    let engine = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    let visible = engine.session(&sid).await.unwrap();
    assert_eq!(visible.turns.len(), 12);
    assert!(engine.state.lock().await.sessions[0].cold.is_some());
    let _lease = engine.ensure_archive_session(&sid).await.unwrap().unwrap();
    let opened = engine.state.lock().await.clone();
    assert!(capacity::encoded_bytes(&opened).unwrap() < 1024 * 1024);
    assert!(opened.sessions[0].turns[0].archived_replay_bytes.is_some());

    let mut touched = opened.clone();
    touched.sessions[0].turns[0].state = "interrupted_unknown".into();
    touched.sessions[0].turns[0].runtime_uncertain_model = true;
    let head = dir.path().join("history-v1/head.dpapi");
    let before = std::fs::read(&head).unwrap();
    store.lock().await.set_fail_before_head(true);
    let error = commit(store.clone(), touched.clone(), vec![sid.clone()]).await.err().unwrap();
    assert!(error.starts_with("ARCHIVE_INJECTED:"));
    assert_eq!(std::fs::read(&head).unwrap(), before);
    assert!(opened.sessions[0].turns[0].archived_replay_bytes.is_some());
    store.lock().await.set_fail_before_head(false);
    let accepted = commit(store.clone(), touched, vec![sid]).await.unwrap();
    let turn = &accepted.sessions[0].turns[0];
    assert_eq!(turn.state, "interrupted_unknown");
    assert!(turn.archived_replay_bytes.is_none());
    assert!(turn.runtime_operations.iter().eq(original.runtime_operations.iter()));
    assert_eq!(turn.runtime_models, original.runtime_models);
    assert_eq!(turn.runtime_results, original.runtime_results);
}

#[tokio::test]
#[ignore = "explicit synthetic replay profiles in a fresh C lab only"]
async fn seed_native_completed_graph_replay() {
    let parent = PathBuf::from(std::env::var_os("HOMEBASE_TURN_REPLAY_SEED_ROOT").unwrap());
    let canonical = parent.canonicalize().unwrap();
    assert!(canonical.to_string_lossy().replace('\\', "/").starts_with("//?/C:/HomeBase-Labs/turn-replay-"));
    assert_eq!(parent.file_name().and_then(|name| name.to_str()), Some("native-fixtures"));
    for (profile, count) in [("large", 15), ("compatible", 3)] {
        let home = parent.join(profile);
        std::fs::create_dir(&home).unwrap();
        let root = home.join(".homebase/assistant");
        std::fs::create_dir_all(&root).unwrap();
        let (_, mut state, _, _) = completed_task(false, false).await;
        enable_capacity(&mut state);
        state.settings.enabled = false;
        let session = &mut state.sessions[0];
        session.queue_paused = true;
        let original = populate_completed_graph_replay(session, count);
        let expected = json!({
            "synthetic_only": true, "session_id": session.id, "turn_id": original.id,
            "turn_count": count, "text": original.text
        });
        let mut store = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
        store.enable_capacity_segments();
        store.load_index().unwrap();
        store.save_partition(serde_json::to_value(state).unwrap(), &[]).await.unwrap();
        std::fs::write(home.join("task-expected.json"), serde_json::to_vec(&expected).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn single_running_turn_keeps_verified_original_results_past_resident_limit() {
    let (dir, mut state, _, _) = completed_task(false, false).await;
    let session = &mut state.sessions[0];
    populate_running_job_results(session, 36);
    let mut ambiguous = session.turns[0].steps.last().unwrap().clone();
    ambiguous.id = "duplicate-synthetic-step".into();
    ambiguous.result["step_id"] = json!(ambiguous.id);
    session.turns[0].steps.push(ambiguous);
    session.turns[0].control_intents.insert("synthetic-running-step-34".into(), "pending-synthetic-receipt".into());
    session.jobs[34].deliver = true;
    let sid = session.id.clone();
    let expected = [1, 31].map(|position| session.jobs[position].result.clone());
    enable_capacity(&mut state);
    assert!(capacity::encoded_bytes(&state).unwrap() > capacity::RESIDENT_BYTES);
    let original_total = capacity::encoded_bytes(&state).unwrap();
    let (store, committed) = capacity_store(dir.path(), state).await;
    assert!(capacity::encoded_bytes(&committed).unwrap() < 3 * 1024 * 1024);
    assert_eq!(logical_bytes(&committed).unwrap(), original_total);
    let active = &committed.sessions[0];
    assert!(active.cold.is_none());
    assert_eq!(active.turns[0].state, "running");
    for position in [34, 35, 36] {
        assert!(active.jobs[position].archived_result_bytes.is_none());
        assert!(active.jobs[position].result["text"].is_string());
    }
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let restarted = load(store, snapshot).await.unwrap();
    let session = &restarted.sessions[0];
    assert_eq!(session.id, sid);
    assert_eq!(session.turns[0].state, "running");
    assert_eq!(session.jobs.len(), 37);
    assert_eq!(logical_bytes(&restarted).unwrap(), original_total);
    for (position, original) in [1, 31].into_iter().zip(expected) {
        assert!(session.jobs[position].archived_result_bytes.is_some());
        let job = live_jobs::restored(&session.jobs[position], session.job_result_source.as_deref()).unwrap();
        assert_eq!(job.result, original);
    }
}

#[tokio::test]
async fn progress_reads_keep_approval_and_omit_large_original_bodies() {
    let (dir, mut state, _, _) = completed_task(false, false).await;
    let session = &mut state.sessions[0];
    populate_running_job_results(session, 3);
    let session_id = session.id.clone();
    let turn_id = session.turns[0].id.clone();
    let step_id = session.turns[0].steps[0].id.clone();
    let marker = "large-original-step";
    session.turns[0].steps[0].result = json!({ "body": marker.repeat(60_000) });
    let (store, committed) = capacity_store(dir.path(), state).await;
    let engine = capacity_engine(store, committed, dir.path().join("workspace")).await;
    {
        let mut state = engine.state.lock().await;
        let (session, turn) = locate(&state, &turn_id).unwrap();
        let turn = &mut state.sessions[session].turns[turn];
        turn.state = "awaiting_confirmation".into();
        turn.pending = Some(Pending {
            id: "original-approval".into(),
            tool_id: "original-tool".into(),
            description: "核对合成参数".into(),
            arguments: json!({"item": "synthetic"}),
            approved: false,
        });
    }
    let progress = serde_json::to_value(engine.observe_turn(&turn_id).await.unwrap()).unwrap();
    let full = serde_json::to_value(engine.turn_with_progress(&turn_id).await.unwrap()).unwrap();
    let inspection = serde_json::to_value(engine.inspect_turn_steps(&session_id, &turn_id).await.unwrap()).unwrap();
    let step = serde_json::to_value(engine.read_turn_step(&session_id, &turn_id, &step_id).await.unwrap()).unwrap();
    assert_eq!(progress["details_omitted"], true);
    assert_eq!(progress["step_count"], full["steps"].as_array().unwrap().len());
    assert_eq!(progress["steps"], json!([]));
    assert!(full.get("details_omitted").is_none());
    assert!(full["steps"][0]["result"]["body"].as_str().unwrap().starts_with(marker));
    for field in ["id", "session_id", "state", "pending", "artifacts", "progress"] {
        assert_eq!(progress[field], full[field], "{field}");
    }
    assert_eq!(progress["pending"]["id"], "original-approval");
    assert_eq!(progress["pending"]["arguments"], json!({"item": "synthetic"}));
    assert!(progress["jobs"].as_array().unwrap().iter().all(|job| job["result"].is_null()));
    assert!(serde_json::to_vec(&progress).unwrap().len() < 8 * 1024);
    assert!(serde_json::to_vec(&full).unwrap().len() > 2 * 1024 * 1024);
    assert!(progress.get("step_summaries").is_none());
    assert!(full.get("step_summaries").is_none());
    assert_eq!(inspection["steps"], json!([]));
    assert_eq!(inspection["step_count"], progress["step_count"]);
    assert_eq!(inspection["step_summaries"][0], json!({
        "id": step_id, "name": full["steps"][0]["name"], "state": full["steps"][0]["state"],
    }));
    for field in ["pending", "artifacts", "progress"] {
        assert_eq!(inspection[field], full[field], "{field}");
    }
    assert_eq!(step["session_id"], session_id);
    assert_eq!(step["turn_id"], turn_id);
    assert_eq!(step["step"], full["steps"][0]);
    assert!(serde_json::to_vec(&inspection).unwrap().len() < 8 * 1024);
    assert!(engine.inspect_turn_steps("foreign-session", &turn_id).await.err().unwrap()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert!(engine.read_turn_step("foreign-session", &turn_id, &step_id).await.err().unwrap()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert!(engine.read_turn_step(&session_id, &turn_id, "unknown-step").await.err().unwrap()
        .starts_with("HISTORY_STEP_NOT_FOUND:"));
}

#[tokio::test]
#[ignore = "explicit synthetic active workflow fixtures in F lab"]
async fn seed_native_running_job_results() {
    let parent = PathBuf::from(std::env::var_os("HOMEBASE_RUNNING_JOB_SEED_ROOT").unwrap());
    assert!(parent.to_string_lossy().replace('\\', "/").starts_with("F:/HomeBase-Labs/"));
    assert!(parent.is_dir());
    for (name, count) in [
        ("running-result-large-profile", 36),
        ("running-result-compat-profile", 2),
        ("running-result-memory-profile", 20),
    ] {
        let home = parent.join(name);
        std::fs::create_dir(&home).unwrap();
        let root = home.join(".homebase/assistant");
        std::fs::create_dir_all(&root).unwrap();
        let (_, mut state, _, _) = completed_task(false, false).await;
        enable_capacity(&mut state);
        let session = &mut state.sessions[0];
        session.jobs[0].artifacts.clear();
        populate_running_job_results(session, count);
        if name == "running-result-memory-profile" {
            session.turns[0].state = "interrupted_unknown".into();
        }
        let expected = json!({"synthetic_only":true,"session_id":session.id,"turn_id":session.turns[0].id,
            "jobs": session.jobs.len(), "most_recent_job": session.jobs.last().unwrap().id});
        let mut store = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
        store.enable_capacity_segments();
        store.load_index().unwrap();
        store.save_partition(serde_json::to_value(state).unwrap(), &[]).await.unwrap();
        std::fs::write(home.join("task-expected.json"), serde_json::to_vec(&expected).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn live_session_offloads_old_job_results_at_commit_and_restores_original_bodies() {
    use crate::assistant_history::{HistoryKind, HistoryQuery, HistoryRead};

    let (dir, mut original, executions, models) = completed_task(true, false).await;
    let sid = original.sessions[0].id.clone();
    let tid = original.sessions[0].turns[0].id.clone();
    let session = &mut original.sessions[0];
    session.jobs[0].result = json!({"status":"succeeded","text":"原始结果🙂".repeat(4096)});
    let job_id = session.jobs[0].id.clone();
    let expected = serde_json::to_value(view(session, &session.turns[0])).unwrap();
    for position in 0..36 {
        let mut older = session.turns[0].clone();
        older.id = format!("fixture-completed-{position}");
        older.request_id = format!("fixture-request-{position}");
        older.steps.clear();
        let mut job = session.jobs[0].clone();
        job.id = format!("fixture-job-{position}");
        job.turn_id = older.id.clone();
        job.artifacts.clear();
        job.result = json!({"status":"succeeded","text":"合成🙂".repeat(76_000),"position":position});
        session.jobs.push(job);
        session.turns.push(older);
    }
    let mut protected_job = session.jobs[0].clone();
    protected_job.id = "fixture-pending-delivery".into();
    protected_job.deliver = true;
    protected_job.artifacts.clear();
    protected_job.turn_id = "fixture-completed-35".into();
    session.jobs.push(protected_job);
    let mut unknown = session.turns[0].clone();
    unknown.id = "fixture-unknown-turn".into();
    unknown.request_id = "fixture-unknown-request".into();
    unknown.state = "interrupted_unknown".into();
    unknown.steps.clear();
    let mut unknown_job = session.jobs[0].clone();
    unknown_job.id = "fixture-unknown-job".into();
    unknown_job.turn_id = unknown.id.clone();
    unknown_job.artifacts.clear();
    unknown_job.state = "unknown".into();
    unknown_job.result["status"] = json!("unknown");
    session.turns.push(unknown);
    session.jobs.push(unknown_job);
    let mut nested_unknown = session.jobs[0].clone();
    nested_unknown.id = "fixture-original-nested-unknown".into();
    nested_unknown.artifacts.clear();
    nested_unknown.turn_id = "fixture-completed-35".into();
    nested_unknown.result["result"] = json!({"status":"unknown"});
    session.jobs.push(nested_unknown);

    let original_job = session.jobs[0].result.clone();
    enable_capacity(&mut original);
    assert!(capacity::encoded_bytes(&original).unwrap() > capacity::RESIDENT_BYTES);
    let (store, committed) = capacity_store(dir.path(), original).await;
    let archive = dir.path().join("history-v1/head.dpapi");
    let before = std::fs::read(&archive).unwrap();
    let active = &committed.sessions[0];
    assert!(active.cold.is_none());
    assert!(capacity::encoded_bytes(&committed).unwrap() < 2 * 1024 * 1024);
    assert!(live_jobs::archived_bytes(active) > 24 * 1024 * 1024);
    assert!(active.jobs[0].result.get("text").is_none());
    assert!(active.jobs[37].deliver);
    assert!(active.jobs[37].archived_result_bytes.is_none());
    assert_eq!(active.jobs[38].result["status"], "unknown");
    assert!(active.jobs[38].archived_result_bytes.is_none());
    assert!(active.jobs[39].archived_result_bytes.is_none());
    assert!(logical_bytes(&committed).unwrap() > capacity::RESIDENT_BYTES);

    let engine = capacity_engine(store.clone(), committed.clone(), dir.path().join("workspace")).await;
    assert_eq!(serde_json::to_value(engine.turn(&tid).await.unwrap()).unwrap(), expected);
    let channel = engine.channel(json!({"operation":"get","source_key":"synthetic-task-owner","turn_id":tid,"include_files":true})).await.unwrap();
    assert_eq!(channel["status"], "succeeded");
    assert_eq!(channel["result"]["artifacts"][0]["sha256"], expected["artifacts"][0]["sha256"]);
    assert!(engine.session(&sid).await.err().unwrap().starts_with("ARCHIVE_LIMIT"));
    let request = HistoryQuery { session_id: sid.clone(), kind: HistoryKind::Turns,
        query: String::new(), snapshot_id: None, through_turn_id: Some(tid.clone()), offset: 0, limit: 8 };
    let page = engine.history_query(request).await.unwrap();
    assert_eq!(page.items[0].id, tid);
    let mut history_text = String::new();
    let mut offset = 0;
    loop {
        let chunk = engine.history_read(HistoryRead { session_id: sid.clone(), snapshot_id: page.snapshot_id.clone(),
            item_id: tid.clone(), offset, limit: 4096 }).await.unwrap();
        history_text.push_str(&chunk.content);
        let Some(next) = chunk.next_offset else { break };
        offset = next;
    }
    let history: Value = serde_json::from_str(&history_text).unwrap();
    assert_eq!(history["jobs"][0]["result"], original_job);
    assert_eq!(std::fs::read(&archive).unwrap(), before);

    let mut deliver = committed.clone();
    deliver.sessions[0].jobs[0].deliver = true;
    store.lock().await.set_fail_before_head(true);
    assert!(commit(store.clone(), deliver.clone(), vec![sid.clone()]).await.err().unwrap().starts_with("ARCHIVE_INJECTED"));
    assert_eq!(std::fs::read(&archive).unwrap(), before);
    assert!(!committed.sessions[0].jobs[0].deliver);
    store.lock().await.set_fail_before_head(false);
    let updated = commit(store.clone(), deliver, vec![sid.clone()]).await.unwrap();
    let head = store.lock().await.load_index().unwrap().unwrap();
    let raw = store.lock().await.read_record(&head, &sid, Kind::Jobs, &job_id).unwrap();
    assert_eq!(raw["result"], original_job);
    assert_eq!(raw["deliver"], true);
    assert!(raw.get("archived_result_bytes").is_none());
    assert!(updated.sessions[0].jobs[0].archived_result_bytes.is_some());
    let restarted = load(store.clone(), head).await.unwrap();
    assert!(restarted.sessions[0].jobs[0].archived_result_bytes.is_none());
    assert!(live_jobs::archived_bytes(&restarted.sessions[0]) > 24 * 1024 * 1024);
    let restarted = capacity_engine(store, restarted, dir.path().join("workspace")).await;
    let result = restarted.turn(&tid).await.unwrap();
    assert!(result.jobs[0].deliver);
    assert_eq!(result.jobs[0].result, original_job);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
#[ignore = "explicit synthetic native result-residency fixture in F lab"]
async fn seed_native_live_job_results() {
    let home = PathBuf::from(std::env::var_os("HOMEBASE_LIVE_JOB_SEED_HOME").unwrap());
    assert!(home.to_string_lossy().replace('\\', "/").starts_with("F:/HomeBase-Labs/"));
    assert_eq!(home.file_name().and_then(|name| name.to_str()), Some("live-job-profile"));
    std::fs::create_dir(&home).unwrap();
    let root = home.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let (fixture, mut state, _, _) = completed_task(false, false).await;
    enable_capacity(&mut state);
    let session = &mut state.sessions[0];
    let artifact = home.join("task-result.md");
    std::fs::copy(fixture.path().join("workspace/task-result.md"), &artifact).unwrap();
    session.jobs[0].artifacts[0].path = artifact.to_string_lossy().into_owned();
    session.jobs[0].result = json!({"status":"succeeded","text":"原始结果🙂".repeat(4096)});
    let mut unknown = session.turns[0].clone();
    unknown.id = "fixture-native-unknown-turn".into();
    unknown.request_id = "fixture-native-unknown-request".into();
    unknown.state = "interrupted_unknown".into();
    unknown.steps.clear();
    session.turns.push(unknown);
    let expected = json!({"synthetic_only":true,"turn_id":session.turns[0].id,
        "session_id":session.id,"detail":view(session,&session.turns[0]),
        "unknown_id":session.turns[1].id});
    let mut store = Store::new(root.join("history-v1"), root.join("state.dpapi")).unwrap();
    store.enable_capacity_segments();
    store.load_index().unwrap();
    store.save_partition(serde_json::to_value(state).unwrap(), &[]).await.unwrap();
    std::fs::write(home.join("task-expected.json"), serde_json::to_vec(&expected).unwrap()).unwrap();
}

#[tokio::test]
async fn capacity_job_checkpoints_reuse_original_slots_and_publish_only_after_success() {
    let dir = tempfile::tempdir().unwrap();
    let mut state = State::default();
    enable_capacity(&mut state);
    let id = "synthetic-job-checkpoint".to_owned();
    let mut session = empty_session("desktop", "desktop", &id);
    for index in 0..3 {
        session.jobs.push(Job {
            archived_result_bytes: None,
            id: format!("checkpoint-job-{index}"),
            command: "system_info".into(),
            turn_id: String::new(),
            state: "succeeded".into(),
            result: json!({"value":index}),
            owned: true,
            deliver: false,
            artifacts: vec![],
            remote: None,
        });
    }
    state.sessions.push(session);
    let mut writer = Store::new(dir.path().join("history-v1"), dir.path().join("state.dpapi"))
        .unwrap();
    writer.enable_capacity_segments();
    assert!(writer.load_index().unwrap().is_none());
    let store = Arc::new(Mutex::new(writer));
    let state = commit(store.clone(), state, vec![id.clone()]).await.unwrap();
    let mut changed = state.clone();
    changed.sessions[0].jobs[1].result = json!({"value":99});
    let base = state.archive_base.as_ref().unwrap();
    let overlay = job_overlay(&changed.sessions[0], base).unwrap().unwrap();
    assert!(matches!(overlay.records[0], RecordSlot::Retained { index: 0, .. }));
    assert!(matches!(&overlay.records[1], RecordSlot::Owned(job) if job["result"]["value"] == 99));
    assert!(matches!(overlay.records[2], RecordSlot::Retained { index: 2, .. }));
    let head = dir.path().join("history-v1/head.dpapi");
    let old_head = std::fs::read(&head).unwrap();
    store.lock().await.set_fail_before_head(true);
    let failure = commit(store.clone(), changed.clone(), vec![id.clone()])
        .await
        .err()
        .unwrap();
    assert!(failure.starts_with("ARCHIVE_INJECTED:"));
    assert_eq!(std::fs::read(&head).unwrap(), old_head);
    assert_eq!(state.sessions[0].jobs[1].result["value"], 1);
    store.lock().await.set_fail_before_head(false);
    let changed = commit(store.clone(), changed, vec![id.clone()]).await.unwrap();
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    assert_eq!(
        snapshot.record_ids(&id, Kind::Jobs).unwrap(),
        base.record_ids(&id, Kind::Jobs).unwrap()
    );
    let loaded = load(store, snapshot).await.unwrap();
    let restored = read_session(loaded.sessions[0].clone()).await.unwrap();
    assert_eq!(serde_json::to_value(&restored.jobs).unwrap(), serde_json::to_value(&changed.sessions[0].jobs).unwrap());
    assert_eq!(restored.jobs[1].result["value"], 99);
}

#[tokio::test]
#[ignore = "explicit synthetic active-job budget fixture in a fresh F lab only"]
async fn seed_native_active_job_budget() {
    let home = PathBuf::from(
        std::env::var_os("HOMEBASE_JOB_BUDGET_SEED_HOME").expect("new synthetic home"),
    );
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home.to_string_lossy().replace('\\', "/").starts_with("F:/HomeBase-Labs/"));
    assert_eq!(home.file_name().and_then(|name| name.to_str()), Some("active-job-budget-profile"));
    let (_temp, mut original, _, _) = completed_task(false, false).await;
    let session = &mut original.sessions[0];
    session.turns[0].state = "interrupted_unknown".into();
    session.turns[0].runtime_uncertain_model = true;
    let mut row = session.jobs[0].clone();
    row.artifacts.clear();
    row.result = json!({"status":"succeeded","text":"中文🙂\n\"".repeat(4096)});
    session.jobs[0] = row.clone();
    for index in 1..8 {
        let mut next = row.clone();
        next.id = format!("synthetic-budget-job-{index}");
        session.jobs.push(next);
    }
    let expected = json!({"synthetic_only":true,"turn_id":session.turns[0].id,
        "jobs":session.jobs,"state":session.turns[0].state,"settings":original.settings});
    let root = home.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let (store, state) = capacity_store(&root, original).await;
    assert!(state.sessions[0].cold.is_none());
    assert!(store.lock().await.load_index().unwrap().unwrap().capacity_segmented());
    crate::control_protocol::publish_private(
        &home.join("task-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}

fn large_history(mut state: State) -> State {
    let template = state.sessions[0].clone();
    state.sessions.clear();
    state.settings.enabled = false;
    for n in 0..128 {
        let mut session = template.clone();
        session.id = format!("capacity-session-{n}");
        let mut messages = Vec::new();
        let mut turns = Vec::new();
        session.history.clear();
        for i in 0..8 {
            let tid = format!("capacity-turn-{n}-{i}");
            let mut user = template.messages[0].clone();
            user.id = format!("capacity-user-{n}-{i}");
            user.turn_id = Some(tid.clone());
            user.content = format!("原始合成问题🙂{n}-{i}:{}", "u".repeat(2048));
            let mut reply = template.messages[1].clone();
            reply.id = format!("capacity-reply-{n}-{i}");
            reply.turn_id = Some(tid.clone());
            reply.content = format!("原始合成答复🙂{n}-{i}:{}", "a".repeat(8192));
            let mut turn = template.turns[0].clone();
            turn.id = tid;
            turn.request_id = format!("capacity-request-{n}-{i}");
            turn.text = reply.content.clone();
            let original_request = SendRequest {
                image: None,
                session_id: (i > 0).then(|| session.id.clone()),
                text: user.content.clone(),
                request_id: turn.request_id.clone(),
                mode: Some("chat".into()),
                route_id: None,
                attachments: Vec::new(),
            };
            turn.fingerprint = request_fingerprint(&original_request, &session.source_key).unwrap();
            turn.protocol = vec![json!({"role":"user","content":user.content})];
            session.history.push(vec![
                json!({"role":"user","content":user.content}),
                json!({"role":"assistant","content":reply.content}),
            ]);
            messages.extend([user, reply]);
            turns.push(turn);
        }
        session.messages = messages.into();
        session.turns = turns.into();
        state.sessions.push(session);
    }
    state
}

#[tokio::test]
async fn capacity_retains_more_than_100_sessions_and_24_mib_without_resident_growth_or_legacy_overwrite(
) {
    let (dir, full, calls) = filled().await;
    let state = large_history(full);
    let original_first = state.sessions[0].clone();
    let original_last = state.sessions[127].clone();
    assert!(capacity::encoded_bytes(&state).unwrap() > 24 * 1024 * 1024);
    assert!(capacity::encoded_bytes(&state).unwrap() < 60 * 1024 * 1024);
    let legacy_root = dir.path().join("legacy");
    let mut legacy_store = Store::new(
        legacy_root.join("history-v1"),
        legacy_root.join("state.dpapi"),
    )
    .unwrap();
    legacy_store.load_index().unwrap();
    assert!(persist(&mut legacy_store, state.clone())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_LIMIT:"));
    assert!(!legacy_root.join("history-v1/head.dpapi").exists());
    assert!(session_admission(&state).is_err());
    let (store, state) = capacity_store(dir.path(), state).await;
    assert!(logical_bytes(&state).unwrap() > 24 * 1024 * 1024);
    assert!(capacity::encoded_bytes(&state).unwrap() < 24 * 1024 * 1024);
    assert_eq!(
        state.sessions.iter().filter(|s| s.cold.is_some()).count(),
        128
    );
    let e = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    let added = e.create_session().await.unwrap();
    assert!(!added.id.is_empty());
    assert_eq!(e.sessions().await.len(), 129);
    for original in [&original_first, &original_last] {
        assert_eq!(
            serde_json::to_value(e.session(&original.id).await.unwrap().messages).unwrap(),
            serde_json::to_value(&original.messages).unwrap()
        );
        assert_eq!(
            e.turn(&original.turns[0].id).await.unwrap().state,
            "completed"
        );
        let replay = request(
            None,
            &original.messages[0].content,
            &original.turns[0].request_id,
        );
        assert_eq!(
            e.desktop_receipt(&replay).await.unwrap().unwrap().turn_id,
            original.turns[0].id
        );
        assert_eq!(e.send(replay).await.unwrap().turn_id, original.turns[0].id);
    }
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        128
    );
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    assert!(snapshot.capacity_segmented());
    let export_root = dir.path().join("legacy-export");
    let mut export = Store::new(
        export_root.join("history-v1"),
        export_root.join("state.dpapi"),
    )
    .unwrap();
    assert!(store
        .lock()
        .await
        .export_v1(&snapshot, &mut export)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_LIMIT:"));
    assert!(!export_root.join("history-v1/head.dpapi").exists());
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    drop(e);
    let state = load(store.clone(), snapshot).await.unwrap();
    assert_eq!(state.sessions.len(), 129);
    let e = capacity_engine(store, state, dir.path().join("workspace")).await;
    assert_eq!(
        e.session(&original_last.id).await.unwrap().messages.len(),
        16
    );
    assert_eq!(
        e.state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.cold.is_some())
            .count(),
        128
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn capacity_commit_keeps_leased_bodies_and_adopts_cold_projection_only_after_success() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let (store, state) = capacity_store(dir.path(), full).await;
    let e = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    let lease = e
        .ensure_archive_turn(&original.turns[0].id, None)
        .await
        .unwrap();
    let mut settings = e.settings().await;
    settings.max_tokens = 3201;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_none());
    drop(lease);
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let head = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    store.lock().await.set_fail_before_head(true);
    let mut settings = e.settings().await;
    settings.max_tokens = 3202;
    assert!(e
        .save_settings(settings)
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_INJECTED:"));
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        head
    );
    store.lock().await.set_fail_before_head(false);
    let mut settings = e.settings().await;
    settings.max_tokens = 3202;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(
        serde_json::to_value(e.session(&original.id).await.unwrap().messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn capacity_closed_task_results_artifacts_and_owner_survive_projection_and_restart() {
    let (dir, full, effects, models) = completed_task(true, false).await;
    let original = full.sessions[0].clone();
    let artifact = original.jobs[0].artifacts[0].clone();
    let bytes = std::fs::read(&artifact.path).unwrap();
    let (store, state) = capacity_store(dir.path(), full).await;
    let e = capacity_engine(store.clone(), state, dir.path().join("workspace")).await;
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert!(e.channel(json!({"operation":"get","source_key":"foreign","turn_id":original.turns[0].id,"include_files":true})).await.is_err());
    let response = e.channel(json!({"operation":"get","source_key":"synthetic-task-owner","turn_id":original.turns[0].id,"include_files":true})).await.unwrap();
    assert_eq!(response["status"], "succeeded");
    assert_eq!(
        response["result"]["artifacts"][0]["sha256"],
        artifact.sha256
    );
    assert_eq!(
        e.artifact_content(&artifact.id, false).await.unwrap().1,
        bytes
    );
    let mut settings = e.settings().await;
    settings.max_tokens = 3401;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    drop(e);
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let state = load(store.clone(), snapshot).await.unwrap();
    let e = capacity_engine(store, state, dir.path().join("workspace")).await;
    assert_eq!(
        e.turn(&original.turns[0].id).await.unwrap().jobs[0].id,
        original.jobs[0].id
    );
    assert_eq!(
        e.artifact_content(&artifact.id, false).await.unwrap().1,
        bytes
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(models.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn capacity_commit_keeps_completed_workers_until_cleanup_without_fake_task_execution() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let (store, state) = capacity_store(dir.path(), full).await;
    let e = capacity_engine(store, state, dir.path().join("workspace")).await;
    drop(
        e.ensure_archive_turn(&original.turns[0].id, None)
            .await
            .unwrap(),
    );
    let (cancel, _) = watch::channel(false);
    e.running.lock().await.insert(
        original.turns[0].id.clone(),
        Running {
            cancel,
            approval: Arc::new(Notify::new()),
        },
    );
    let mut settings = e.settings().await;
    settings.max_tokens = 3203;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_none());
    e.running.lock().await.remove(&original.turns[0].id);
    let mut settings = e.settings().await;
    settings.max_tokens = 3204;
    e.save_settings(settings).await.unwrap();
    assert!(e.state.lock().await.sessions[0].cold.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn capacity_live_simulated_wechat_and_desktop_share_real_commit_and_reuse_original_receipts()
{
    let dir = tempfile::tempdir().unwrap();
    let mut initial = State::default();
    initial.settings.enabled = true;
    initial.settings.route_id = "synthetic-only".into();
    let (store, state) = capacity_store(dir.path(), initial).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let model: Model = Arc::new({
        let calls = calls.clone();
        move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                wire::parse(
                    "chat",
                    &json!({"choices":[{"message":{"content":"合成微信自然聊天🙂"}}],"usage":{"prompt_tokens":7,"completion_tokens":3,"cost":0.0}}),
                )
            })
        }
    });
    let save: Commit = Arc::new({
        let store = store.clone();
        move |state, protected| {
            let store = store.clone();
            Box::pin(async move { commit(store, state, protected).await })
        }
    });
    let workspace = dir.path().join("workspace");
    let documents = Arc::new(crate::assistant_documents::DocumentStore::new(
        workspace.join(".documents"),
    ));
    let e = Engine::new_with_commit(
        state,
        save,
        model,
        Arc::new(|_| Box::pin(async { panic!("chat must not execute tools") })),
        workspace,
        documents,
    )
    .await
    .unwrap();
    let mut ids = Vec::new();
    for i in 0..2 {
        let ack = e.channel(json!({"operation":"submit","source_key":"capacity-owner","text":"只聊合成测试","request_id":format!("capacity-wx-{i}")})).await.unwrap();
        let tid = ack["turn_id"].as_str().unwrap().to_string();
        complete(&e, &tid).await;
        for _ in 0..500 {
            if !e.running.lock().await.contains_key(&tid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!e.running.lock().await.contains_key(&tid));
        ids.push(tid);
        let mut settings = e.settings().await;
        settings.max_tokens = 3300 + i;
        e.save_settings(settings).await.unwrap();
        assert!(e
            .state
            .lock()
            .await
            .sessions
            .iter()
            .all(|s| s.cold.is_some()));
    }
    let before = serde_json::to_value(&*e.state.lock().await).unwrap();
    let sid = e.sessions().await[0].id.clone();
    assert_eq!(e.session(&sid).await.unwrap().messages.len(), 4);
    let context = e.context(Some(&sid), "聊天").await.unwrap();
    assert!(serde_json::to_vec(&context).unwrap().len() <= 9000);
    assert_eq!(
        serde_json::to_value(&*e.state.lock().await).unwrap(),
        before
    );
    let result = e
        .channel(json!({"operation":"get","source_key":"capacity-owner","turn_id":ids[1]}))
        .await
        .unwrap();
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["result"]["turn_id"], ids[1]);
    let repeated = e.channel(json!({"operation":"submit","source_key":"capacity-owner","text":"只聊合成测试","request_id":"capacity-wx-1"})).await.unwrap();
    assert_eq!(repeated["turn_id"], ids[1]);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    drop(e);
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let state = load(store.clone(), snapshot).await.unwrap();
    let e = capacity_engine(store, state, dir.path().join("workspace")).await;
    assert_eq!(
        e.channel(json!({"operation":"get","source_key":"capacity-owner","turn_id":ids[0]}))
            .await
            .unwrap()["status"],
        "succeeded"
    );
    assert_eq!(e.session(&sid).await.unwrap().messages.len(), 4);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn capacity_unknown_bodies_still_obey_resident_budget_before_any_head_publication() {
    let (dir, full, calls) = filled().await;
    let mut state = large_history(full);
    for session in &mut state.sessions {
        session.turns[0].state = "interrupted_unknown".into();
    }
    enable_capacity(&mut state);
    let mut writer = Store::new(
        dir.path().join("history-v1"),
        dir.path().join("state.dpapi"),
    )
    .unwrap();
    writer.enable_capacity_segments();
    writer.load_index().unwrap();
    let store = Arc::new(Mutex::new(writer));
    assert!(commit(store, state, Vec::new())
        .await
        .err()
        .unwrap()
        .starts_with("ARCHIVE_RESIDENT_LIMIT:"));
    assert!(!dir.path().join("history-v1/head.dpapi").exists());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn capacity_format_marker_mismatch_is_explicit_and_does_not_fall_back_or_rewrite() {
    let (dir, full, calls) = filled().await;
    let (_store, _) = capacity_store(dir.path(), full).await;
    let file = dir.path().join("history-v1/head.dpapi");
    let cipher = std::fs::read(&file).unwrap();
    let plain = crate::wechat_crypto::crypt(&cipher, false).unwrap();
    let mut index: Value = serde_json::from_slice(&plain).unwrap();
    index["metadata"]["archive_capacity_version"] = json!(0);
    let malformed =
        crate::wechat_crypto::crypt(&serde_json::to_vec(&index).unwrap(), true).unwrap();
    std::fs::write(&file, &malformed).unwrap();
    let mut reader = Store::new(
        dir.path().join("history-v1"),
        dir.path().join("state.dpapi"),
    )
    .unwrap();
    assert!(reader
        .load_index()
        .err()
        .unwrap()
        .starts_with("ARCHIVE_MODE:"));
    assert_eq!(
        std::fs::read(&file).unwrap().as_slice(),
        malformed.as_slice()
    );
    std::fs::write(&file, &cipher).unwrap();
    assert!(reader.load_index().unwrap().unwrap().capacity_segmented());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
#[ignore = "explicit synthetic capacity profile in a fresh F lab only"]
async fn seed_isolated_native_capacity_profile() {
    let home =
        PathBuf::from(std::env::var_os("HOMEBASE_CAPACITY_SEED_HOME").expect("new synthetic home"));
    assert!(home.is_absolute() && !home.exists() && home.parent().unwrap().is_dir());
    assert!(home
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("capacity-native-"));
    let (_temp, mut original, calls) = filled().await;
    original.settings.enabled = false;
    let mut state = large_history(original.clone());
    state.sessions.extend(original.sessions.clone());
    let mut critical = original.sessions[0].clone();
    critical.id = "capacity-critical-session".into();
    critical.turns[0].id = "capacity-critical-turn".into();
    critical.turns[0].request_id = "capacity-critical-request".into();
    critical.turns[0].state = "interrupted_unknown".into();
    critical.turns[0].runtime_uncertain_model = true;
    critical.turns[0].interaction = "task".into();
    for message in critical.messages.iter_mut() {
        message.turn_id = Some("capacity-critical-turn".into());
    }
    state.sessions.push(critical);
    let samples: Vec<_> = [0, 127, 128].into_iter().map(|i| {
        let s = &state.sessions[i];
        json!({"id":s.id,"messages":s.messages,"turns":s.turns.iter().map(|t|json!({"id":t.id,"state":t.state})).collect::<Vec<_>>(),
            "request":{"session_id":null,"text":s.messages[0].content,"request_id":s.turns[0].request_id,"mode":"chat","attachments":[]}})
    }).collect();
    let original_bytes = capacity::encoded_bytes(&state).unwrap();
    assert!(original_bytes > 24 * 1024 * 1024 && original_bytes < 60 * 1024 * 1024);
    let root = home.join(".homebase/assistant");
    std::fs::create_dir_all(&root).unwrap();
    let legacy =
        crate::wechat_crypto::crypt(&serde_json::to_vec(&original).unwrap(), true).unwrap();
    crate::control_protocol::publish_private(&root.join("state.dpapi"), &legacy)
        .await
        .unwrap();
    let (store, state) = capacity_store(&root, state).await;
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    assert!(snapshot.capacity_segmented());
    let expected = json!({"synthetic_only":true,"real_model_calls":0,"synthetic_model_calls_for_seed":calls.load(Ordering::SeqCst),
        "original_bytes":original_bytes,"retained_bytes":logical_bytes(&state).unwrap(),"resident_state_bytes":capacity::encoded_bytes(&state).unwrap(),
        "sessions":state.sessions.iter().map(|s|s.id.clone()).collect::<Vec<_>>(),"cold_sessions":state.sessions.iter().filter(|s|s.cold.is_some()).count(),
        "critical_turn":"capacity-critical-turn","critical_state":"interrupted_unknown","samples":samples});
    crate::control_protocol::publish_private(
        &home.join("capacity-expected.json"),
        &serde_json::to_vec(&expected).unwrap(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn restored_body_uses_keep_scoped_protection_for_nested_and_batch_operations() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[0].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls).await;
    let first = e
        .ensure_archive_turn(&original.turns[0].id, Some("desktop"))
        .await
        .unwrap();
    assert_eq!(
        e.archive_leases.protected().unwrap(),
        vec![original.id.clone()]
    );
    let second = e
        .ensure_archive_request(&original.turns[0].request_id, "desktop")
        .await
        .unwrap();
    drop(first);
    assert_eq!(
        e.archive_leases.protected().unwrap(),
        vec![original.id.clone()]
    );
    let batch = e.ensure_archive_source("desktop").await.unwrap();
    assert_eq!(e.archive_leases.protected().unwrap().len(), 2);
    drop(second);
    assert_eq!(e.archive_leases.protected().unwrap().len(), 2);
    drop(batch);
    assert!(e.archive_leases.protected().unwrap().is_empty());
    assert_eq!(
        serde_json::to_value(&e.state.lock().await.sessions[0].messages).unwrap(),
        serde_json::to_value(&original.messages).unwrap()
    );
}

#[tokio::test]
async fn denied_or_cancelled_restore_never_leaks_body_protection_or_adopts_partial_state() {
    let (dir, full, calls) = filled().await;
    let original = full.sessions[2].clone();
    let store = stored(dir.path(), full).await;
    let e = engine(store.clone(), dir.path().join("workspace"), calls).await;
    assert!(e
        .ensure_archive_turn(&original.turns[0].id, Some("foreign"))
        .await
        .is_err());
    assert!(e.archive_leases.protected().unwrap().is_empty());
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
    let reader = store.lock().await;
    let mut restore = Box::pin(e.ensure_archive_session(&original.id));
    tokio::select! {
        result = &mut restore => panic!("synthetic locked reader must wait: {}", result.is_ok()),
        _ = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    assert_eq!(
        e.archive_leases.protected().unwrap(),
        vec![original.id.clone()]
    );
    drop(restore);
    assert!(e.archive_leases.protected().unwrap().is_empty());
    drop(reader);
    assert!(e.state.lock().await.sessions[2].cold.is_some());
    assert_eq!(store.lock().await.metrics().segment_reads, 0);
}

#[tokio::test]
async fn damaged_batch_restore_releases_prior_protections_and_keeps_disk_unchanged() {
    let (dir, full, calls) = filled().await;
    let last = full.sessions[1].clone();
    let store = stored(dir.path(), full).await;
    let snapshot = store.lock().await.load_index().unwrap().unwrap();
    let e = engine(store.clone(), dir.path().join("workspace"), calls).await;
    let before = std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap();
    // Resolve this synthetic session's message segment through a separate read,
    // without warming the Engine Store cache before the damage is introduced.
    let head: Value = snapshot.metadata();
    assert_eq!(head["sessions"][1]["id"], last.id);
    for entry in std::fs::read_dir(dir.path().join("history-v1/segments")).unwrap() {
        let file = entry.unwrap().path();
        let cipher = std::fs::read(&file).unwrap();
        let plain = crate::wechat_crypto::crypt(&cipher, false).unwrap();
        let segment: Value = serde_json::from_slice(&plain).unwrap();
        if segment["session_id"] == last.id {
            std::fs::write(file, b"synthetic damaged segment").unwrap();
        }
    }
    assert!(e.ensure_archive_source("desktop").await.is_err());
    assert!(e.archive_leases.protected().unwrap().is_empty());
    assert!(e.state.lock().await.sessions[1].cold.is_some());
    assert_eq!(
        std::fs::read(dir.path().join("history-v1/head.dpapi")).unwrap(),
        before
    );
}
