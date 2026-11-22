//! Synthetic control replies only; original jobs must retain their identity.
#[test]
fn development_result_requires_original_owned_development_job() {
    let mut job = sample_job("dev-job", "development_run", "turn");
    job.result = serde_json::json!({"task_id":"dev-synthetic"});
    assert!(job_access::owns_result(&[job.clone()], job_access::ResultRef::Development("dev-synthetic")).unwrap());
    assert!(!job_access::owns_result(&[job.clone()], job_access::ResultRef::Repository("dev-synthetic")).unwrap());
    assert!(!job_access::owns_result(&[job.clone()], job_access::ResultRef::Development("another-task")).unwrap());
    job.owned = false;
    assert!(!job_access::owns_result(&[job], job_access::ResultRef::Development("dev-synthetic")).unwrap());
}
use super::*;

#[tokio::test]
async fn job_boundary_late_refresh_cannot_overwrite_new_delivery_and_retry_only_reads_original() {
    let (base, _, effects, saved, _root) = fixture(false, vec![]).await;
    let ack = base
        .send(request("合成迟到查询", "job-boundary-stale"))
        .await
        .unwrap();
    base.change_turn(&ack.turn_id, |s, _| {
        s.jobs
            .push(sample_job("stale-job", "system_info", &ack.turn_id));
        Ok(())
    })
    .await
    .unwrap();
    let waiting = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let queries = Arc::new(AtomicUsize::new(0));
    let control: Control = Arc::new({
        let waiting = waiting.clone();
        let release = release.clone();
        let queries = queries.clone();
        move |request| {
            let waiting = waiting.clone();
            let release = release.clone();
            let queries = queries.clone();
            Box::pin(async move {
                assert_eq!(request.method, "jobs.get");
                assert_eq!(request.job_id.as_deref(), Some("stale-job"));
                if queries.fetch_add(1, Ordering::SeqCst) == 0 {
                    waiting.notify_one();
                    release.notified().await;
                }
                Ok(
                    json!({"id":"stale-job","command":"system_info","status":"succeeded","result":{"status":"succeeded"}}),
                )
            })
        }
    });
    let e = Engine::new(
        saved.lock().await.clone(),
        base.save.clone(),
        base.model.clone(),
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let refresh = tokio::spawn({
        let e = e.clone();
        let sid = ack.session_id.clone();
        async move { e.refresh_job_once(&sid, "stale-job").await }
    });
    tokio::time::timeout(Duration::from_secs(3), waiting.notified())
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, _| {
        s.jobs[0].deliver = true;
        Ok(())
    })
    .await
    .unwrap();
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(3), refresh)
        .await
        .unwrap()
        .unwrap();
    assert!(
        result
            .err()
            .as_deref()
            .is_some_and(|e| e.starts_with("JOB_READ_STALE:")),
        "a late whole-job reply must not replace newer delivery state"
    );
    assert!(e.state.lock().await.sessions[0].jobs[0].deliver);
    assert_eq!(e.state.lock().await.sessions[0].jobs[0].state, "running");
    e.refresh_job(&ack.session_id, "stale-job").await.unwrap();
    assert!(e.state.lock().await.sessions[0].jobs[0].deliver);
    assert_eq!(e.state.lock().await.sessions[0].jobs[0].state, "succeeded");
    assert_eq!(queries.load(Ordering::SeqCst), 2);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

fn sample_job(id: &str, command: &str, turn: &str) -> Job {
    Job {
        archived_result_bytes: None,
        id: id.into(),
        command: command.into(),
        turn_id: turn.into(),
        state: "running".into(),
        result: Value::Null,
        owned: true,
        deliver: false,
        artifacts: vec![],
        remote: None,
    }
}

#[tokio::test]
async fn job_boundary_mismatched_owned_reply_cannot_replace_original_job_or_save() {
    let (base, _, effects, saved, _root) = fixture(false, vec![]).await;
    let ack = base
        .send(request("合成原任务", "job-boundary-original"))
        .await
        .unwrap();
    base.change_turn(&ack.turn_id, |s, _| {
        s.jobs
            .push(sample_job("original-job", "system_info", &ack.turn_id));
        Ok(())
    })
    .await
    .unwrap();
    let state = saved.lock().await.clone();
    let control: Control = Arc::new(|_| {
        Box::pin(async {
            Ok(
                json!({"id":"another-job","command":"system_info","status":"succeeded","result":{"marker":"foreign-result"}}),
            )
        })
    });
    let e = Engine::new(
        state,
        base.save.clone(),
        base.model.clone(),
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let before = serde_json::to_value(saved.lock().await.clone()).unwrap();
    let error = e
        .refresh_job_once(&ack.session_id, "original-job")
        .await
        .err();
    assert!(
        error
            .as_deref()
            .is_some_and(|message| message.starts_with("JOB_IDENTITY_MISMATCH:")),
        "mismatched control identity must fail before publication"
    );
    assert_eq!(
        serde_json::to_value(saved.lock().await.clone()).unwrap(),
        before
    );
    assert_eq!(e.state.lock().await.sessions[0].jobs[0].state, "running");
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn job_boundary_repository_run_cannot_authorize_video_result_with_same_run_id() {
    let (e, _, effects, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("合成结果归属", "job-boundary-run"))
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, _| {
        let mut job = sample_job("repository-job", "repo_capability_run", &ack.turn_id);
        job.state = "succeeded".into();
        job.result = json!({"run_id":"same-synthetic-run"});
        s.jobs.push(job);
        Ok(())
    })
    .await
    .unwrap();
    let result = e.execute(&ack.turn_id, &ToolCall {
        id:"wrong-result-kind".into(), name:"homebase_call".into(),
        arguments:json!({"command":"capability_result","args":{"runId":"same-synthetic-run"}}),
    }).await;
    assert!(
        result.is_err(),
        "a repository run is not video ownership evidence"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[test]
fn job_boundary_same_lookup_supports_borrowed_partitions_and_rejects_conflicting_evidence() {
    struct Partitioned {
        hot: Vec<Job>,
        retained: Vec<Job>,
    }
    impl workflow::JobSource for Partitioned {
        fn evidence_jobs(&self) -> impl Iterator<Item = &Job> {
            self.hot.iter().chain(&self.retained)
        }
    }
    let mut source = Partitioned {
        hot: vec![sample_job("hot", "system_info", "t")],
        retained: vec![sample_job("retained", "system_info", "t")],
    };
    assert_eq!(
        job_access::owned(&source, "retained").unwrap().unwrap().id,
        "retained"
    );
    assert!(job_access::record(&source, "missing").unwrap().is_none());
    source.retained[0].state = "succeeded".into();
    source.retained[0].result = json!({"status":"succeeded"});
    let step = Step {
        id: "evidence".into(),
        name: "homebase_call".into(),
        state: "completed".into(),
        arguments: json!({}),
        result: json!({"status":"succeeded"}),
        job_id: Some("retained".into()),
    };
    assert!(workflow::successful(&step, &source));
    source
        .hot
        .push(sample_job("retained", "different-command", "t"));
    assert!(job_access::record(&source, "retained")
        .err()
        .unwrap()
        .starts_with("JOB_IDENTITY_CONFLICT:"));
    assert!(
        !workflow::successful(&step, &source),
        "duplicate IDs are not completion evidence"
    );
    assert!(job_access::owns_result(&source, job_access::ResultRef::Video("run")).is_err());
}

#[test]
fn job_boundary_result_bindings_keep_command_and_ownership_domains_separate() {
    let mut repo = sample_job("repo", "repo_capability_run", "t");
    repo.result = json!({"run_id":"shared"});
    let mut video = sample_job("video", "capability_render", "t");
    video.result = json!({"run_id":"shared"});
    assert!(
        job_access::owns_result(&[repo.clone()], job_access::ResultRef::Repository("shared"))
            .unwrap()
    );
    assert!(!job_access::owns_result(&[repo], job_access::ResultRef::Video("shared")).unwrap());
    assert!(
        job_access::owns_result(&[video.clone()], job_access::ResultRef::Video("shared")).unwrap()
    );
    video.owned = false;
    assert!(!job_access::owns_result(&[video], job_access::ResultRef::Video("shared")).unwrap());
    let mut skill = sample_job("skill", "assistant_local_execute", "t");
    skill.result = json!({"workspace_id":"skill-synthetic","skill_id":"installed:fixture"});
    assert!(job_access::owns_result(
        &[skill.clone()],
        job_access::ResultRef::SkillWorkspace("skill-synthetic")
    )
    .unwrap());
    skill.command = "system_info".into();
    assert!(!job_access::owns_result(
        &[skill],
        job_access::ResultRef::SkillWorkspace("skill-synthetic")
    )
    .unwrap());
    for value in [
        json!({}),
        json!({"id":"wanted","command":"different"}),
        json!({"id":"other","command":"system_info"}),
    ] {
        let message = job_access::control_identity("wanted", "system_info", &value).unwrap_err();
        assert!(message.starts_with("JOB_IDENTITY_MISMATCH:"));
        assert!(!message.contains("different"));
    }
    job_access::control_identity(
        "wanted",
        "system_info",
        &json!({"id":"wanted","command":"system_info"}),
    )
    .unwrap();
}

#[tokio::test]
async fn job_boundary_remote_query_identity_mismatch_never_publishes_or_submits() {
    let (base, _, _, saved, _root) = fixture(false, vec![]).await;
    let ack = base
        .send(request("合成远端任务", "job-boundary-remote"))
        .await
        .unwrap();
    base.change_turn(&ack.turn_id, |s, _| {
        let mut job = sample_job("original-model-job", "model_capability_call", &ack.turn_id);
        job.result = json!({"status":"running","task_id":"original-provider-task"});
        job.remote = Some(Remote {
            route_id: "synthetic-only".into(),
            task_id: "original-provider-task".into(),
            query_job: Some("original-query".into()),
            polls: 1,
            next_poll: 0,
            finished: false,
            credential_source: Default::default(),
        });
        s.jobs.push(job);
        Ok(())
    })
    .await
    .unwrap();
    let queries = Arc::new(AtomicUsize::new(0));
    let control: Control = Arc::new({
        let queries = queries.clone();
        move |request| {
            let queries = queries.clone();
            Box::pin(async move {
                assert_eq!(request.method, "jobs.get", "only read the original IDs");
                queries.fetch_add(1, Ordering::SeqCst);
                Ok(if request.job_id.as_deref() == Some("original-model-job") {
                    json!({"id":"original-model-job","command":"model_capability_call","status":"succeeded","result":{"status":"running","task_id":"original-provider-task"}})
                } else {
                    assert_eq!(request.job_id.as_deref(), Some("original-query"));
                    json!({"id":"wrong-query","command":"model_capability_task","status":"succeeded","result":{"status":"succeeded","marker":"do-not-publish"}})
                })
            })
        }
    });
    let e = Engine::new(
        saved.lock().await.clone(),
        base.save.clone(),
        base.model.clone(),
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let before = serde_json::to_value(e.state.lock().await.clone()).unwrap();
    assert!(e
        .refresh_job_once(&ack.session_id, "original-model-job")
        .await
        .unwrap_err()
        .starts_with("JOB_IDENTITY_MISMATCH:"));
    assert_eq!(queries.load(Ordering::SeqCst), 2);
    assert_eq!(
        serde_json::to_value(e.state.lock().await.clone()).unwrap(),
        before
    );
    assert_eq!(
        serde_json::to_value(saved.lock().await.clone()).unwrap(),
        before
    );
}

#[tokio::test]
async fn job_boundary_duplicate_job_cannot_be_delivered_or_queried() {
    let (e, _, effects, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("合成重复编号", "job-boundary-duplicate"))
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, _| {
        s.jobs
            .push(sample_job("duplicate", "system_info", &ack.turn_id));
        s.jobs
            .push(sample_job("duplicate", "system_info", &ack.turn_id));
        Ok(())
    })
    .await
    .unwrap();
    for name in ["homebase_job", "homebase_deliver"] {
        let error = e
            .execute(
                &ack.turn_id,
                &ToolCall {
                    id: format!("duplicate-{name}"),
                    name: name.into(),
                    arguments: json!({"job_id":"duplicate"}),
                },
            )
            .await
            .err()
            .unwrap();
        assert!(error.starts_with("JOB_IDENTITY_CONFLICT:"));
    }
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
