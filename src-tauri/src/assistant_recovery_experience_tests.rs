use super::*;
use std::sync::atomic::AtomicBool;

async fn interrupted(
    model_unknown: bool,
) -> (
    Arc<Engine>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Ack,
    tempfile::TempDir,
) {
    let (engine, models, effects, _, root) = fixture(false, vec![]).await;
    let ack = engine
        .send_from(
            request("继续整理合成资料", "recovery-experience"),
            "wechat",
            "synthetic-owner",
        )
        .await
        .unwrap();
    engine
        .change_turn(&ack.turn_id, |session, index| {
            let turn = &mut session.turns[index];
            turn.state = "interrupted_unknown".into();
            turn.text.clear();
            turn.settings.runtime = "pi".into();
            turn.runtime_version = RUNTIME_VERSION.into();
            turn.runtime_operations = vec![json!({"kind":"synthetic-checkpoint"})].into();
            turn.runtime_uncertain_model = model_unknown;
            if model_unknown {
                turn.model_failure = Some(Failure::unknown(Kind::Timeout));
            }
            Ok(())
        })
        .await
        .unwrap();
    (engine, models, effects, ack, root)
}

#[tokio::test]
async fn explicit_recovery_checks_the_already_accepted_job_across_desktop_and_wechat() {
    let (original, models, _, ack, _root) = interrupted(true).await;
    let mut saved = original.state.lock().await.clone();
    let (index, _) = locate(&saved, &ack.turn_id).unwrap();
    let accepted = Job {
        id: "synthetic-accepted-job".into(),
        command: "capability_render".into(),
        turn_id: ack.turn_id.clone(),
        state: "running".into(),
        result: Value::Null,
        owned: true,
        deliver: false,
        artifacts: vec![],
        remote: None,
        archived_result_bytes: None,
    };
    let mut foreign = accepted.clone();
    foreign.id = "synthetic-observed-job".into();
    foreign.owned = false;
    saved.sessions[index].jobs.push(accepted.clone());
    saved.sessions[index].jobs.push(foreign);
    std::fs::create_dir_all(&original.workspace).unwrap();
    let path = original.workspace.join("synthetic-result.txt");
    let bytes = b"original synthetic result";
    std::fs::write(&path, bytes).unwrap();
    let digest = format!("{:x}", Sha256::digest(bytes));
    let reads = Arc::new(AtomicUsize::new(0));
    let control: Control = Arc::new({
        let reads = reads.clone();
        let digest = digest.clone();
        move |request| {
            assert_eq!(request.method, "jobs.get", "the original job must not be resubmitted");
            assert_eq!(request.job_id.as_deref(), Some("synthetic-accepted-job"));
            reads.fetch_add(1, Ordering::SeqCst);
            let result = json!({"id":"synthetic-accepted-job","command":"capability_render","status":"succeeded",
                "result":{"status":"succeeded","artifact":{"path":path,"sha256":digest,"mime_type":"text/plain"}}});
            Box::pin(async move { Ok(result) })
        }
    });
    let query = json!({"operation":"get","source_key":"synthetic-owner","turn_id":ack.turn_id,"include_files":true});
    for wechat_first in [false, true] {
        let restored = Engine::new(saved.clone(), original.save.clone(), original.model.clone(), control.clone(), original.workspace.clone()).await.unwrap();
        let before = reads.load(Ordering::SeqCst);
        assert!(restored.inspect_turn_steps("wrong-session", &ack.turn_id).await.is_err());
        let mut wrong_owner = query.clone();
        wrong_owner["source_key"] = json!("wrong-owner");
        assert!(restored.channel(wrong_owner).await.is_err());
        let observation = restored.observe_turn(&ack.turn_id).await.unwrap();
        assert_eq!(observation.jobs[0].state, "running");
        assert_eq!(reads.load(Ordering::SeqCst), before);
        if wechat_first {
            let receipt = restored.channel(query.clone()).await.unwrap();
            assert_eq!(receipt["result"]["jobs"][0]["state"], "succeeded");
        }
        let inspection = restored.inspect_turn_steps(&ack.session_id, &ack.turn_id).await.unwrap();
        assert_eq!(inspection.state, "interrupted_unknown");
        assert!(inspection.model_outcome_unknown);
        assert!(!inspection.resumable);
        assert_eq!(inspection.jobs[0].id, accepted.id);
        assert_eq!(inspection.jobs[0].state, "succeeded");
        assert_eq!(inspection.jobs[1].state, "running");
        assert_eq!(inspection.artifacts[0].sha256, digest);
        let receipt = restored.channel(query.clone()).await.unwrap();
        assert_eq!(receipt["result"]["artifacts"][0]["sha256"], digest);
        assert_eq!(reads.load(Ordering::SeqCst), before + 1);
    }
    assert_eq!(models.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stopped_turn_reports_pending_accepted_jobs_past_the_summary_limit() {
    let (original, models, _, ack, _root) = interrupted(false).await;
    original.change_turn(&ack.turn_id, |session, index| {
        session.turns[index].state = "canceled".into();
        for number in 0..8 {
            session.jobs.push(Job {
                id: format!("synthetic-observation-{number}"),
                command: "system_info".into(),
                turn_id: ack.turn_id.clone(),
                state: "running".into(),
                result: Value::Null,
                owned: false,
                deliver: false,
                artifacts: vec![],
                remote: None,
                archived_result_bytes: None,
            });
        }
        session.jobs.push(Job {
            id: "synthetic-original-video".into(),
            command: "capability_render".into(),
            turn_id: ack.turn_id.clone(),
            state: "queued".into(),
            result: Value::Null,
            owned: true,
            deliver: false,
            artifacts: vec![],
            remote: None,
            archived_result_bytes: None,
        });
        Ok(())
    }).await.unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let control: Control = Arc::new({
        let reads = reads.clone();
        move |request| {
            assert_eq!(request.method, "jobs.get");
            assert_eq!(request.job_id.as_deref(), Some("synthetic-original-video"));
            let state = if reads.fetch_add(1, Ordering::SeqCst) == 0 { "queued" } else { "succeeded" };
            Box::pin(async move { Ok(json!({
                "id":"synthetic-original-video", "command":"capability_render", "status":state,
                "result":{"status":state}
            })) })
        }
    });
    let snapshot = original.state.lock().await.clone();
    let engine = Engine::new(snapshot, original.save.clone(), original.model.clone(), control, original.workspace.clone()).await.unwrap();
    let window = engine.observe_turn(&ack.turn_id).await.unwrap();
    assert_eq!(window.state, "canceled");
    assert!(!window.model_outcome_unknown);
    assert_eq!(window.jobs.len(), 8);
    assert!(window.jobs.iter().all(|job| !job.owned));
    assert_eq!(window.pending_accepted_jobs, 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);

    let query = json!({"operation":"get", "source_key":"synthetic-owner", "turn_id":ack.turn_id});
    let message = engine.channel(query.clone()).await.unwrap();
    assert_eq!(message["result"]["pending_accepted_jobs"], 1);
    let text = message["result"]["text"].as_str().unwrap();
    assert!(text.contains("还有 1 项原任务尚未结束"));
    assert!(text.contains("视频制作：排队中"));
    let inspection = engine.inspect_turn_steps(&ack.session_id, &ack.turn_id).await.unwrap();
    assert_eq!(inspection.pending_accepted_jobs, 0);
    assert_eq!(inspection.state, "canceled");
    let message = engine.channel(query).await.unwrap();
    assert_eq!(message["result"]["pending_accepted_jobs"], 0);
    assert!(!message["result"]["text"].as_str().unwrap().contains("还有 1 项原任务尚未结束"));
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(models.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn original_query_returns_confirmed_artifact_when_another_job_cannot_be_read() {
    let (original, models, _, ack, _root) = interrupted(false).await;
    original.change_turn(&ack.turn_id, |session, index| {
        session.turns[index].state = "canceled".into();
        for (id, command) in [("unavailable-original", "assistant_local_execute"), ("finished-original", "capability_render")] {
            session.jobs.push(Job {
                id: id.into(), command: command.into(), turn_id: ack.turn_id.clone(),
                state: "running".into(), result: Value::Null, owned: true, deliver: false,
                artifacts: vec![], remote: None, archived_result_bytes: None,
            });
        }
        Ok(())
    }).await.unwrap();
    std::fs::create_dir_all(&original.workspace).unwrap();
    let path = original.workspace.join("finished-original.txt");
    let data = b"already completed";
    std::fs::write(&path, data).unwrap();
    let digest = format!("{:x}", Sha256::digest(data));
    let recovered = Arc::new(AtomicBool::new(false));
    let unavailable_reads = Arc::new(AtomicUsize::new(0));
    let finished_reads = Arc::new(AtomicUsize::new(0));
    let control: Control = Arc::new({
        let (recovered, unavailable_reads, finished_reads, digest) = (recovered.clone(), unavailable_reads.clone(), finished_reads.clone(), digest.clone());
        move |request| {
            assert_eq!(request.method, "jobs.get");
            let reply = match request.job_id.as_deref() {
                Some("unavailable-original") => {
                    unavailable_reads.fetch_add(1, Ordering::SeqCst);
                    if recovered.load(Ordering::SeqCst) {
                        Ok(json!({"id":"unavailable-original","command":"assistant_local_execute","status":"succeeded","result":{"status":"succeeded"}}))
                    } else {
                        Err(crate::control_protocol::ControlError::new("SYNTHETIC_UNAVAILABLE", "synthetic transport detail"))
                    }
                }
                Some("finished-original") => {
                    finished_reads.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"id":"finished-original","command":"capability_render","status":"succeeded","result":{"status":"succeeded","artifact":{"path":path,"sha256":digest,"mime_type":"text/plain"}}}))
                }
                _ => panic!("must query only original accepted tasks"),
            };
            Box::pin(async move { reply })
        }
    });
    let saved = original.state.lock().await.clone();
    let query = json!({"operation":"get","source_key":"synthetic-owner","turn_id":ack.turn_id,"include_files":true});
    for desktop in [false, true] {
        let engine = Engine::new(saved.clone(), original.save.clone(), original.model.clone(), control.clone(), original.workspace.clone()).await.unwrap();
        if desktop {
            let view = engine.inspect_turn_steps(&ack.session_id, &ack.turn_id).await.unwrap();
            assert_eq!(view.state, "canceled");
            assert_eq!(serde_json::to_value(&view).unwrap()["unverified_accepted_jobs"], 1);
            assert_eq!(view.artifacts[0].sha256, digest);
        } else {
            let response = engine.channel(query.clone()).await.unwrap();
            assert_eq!(response["status"], "canceled");
            assert_eq!(response["result"]["unverified_accepted_jobs"], 1);
            assert_eq!(response["result"]["artifacts"][0]["sha256"], digest);
            let text = response["result"]["text"].as_str().unwrap();
            assert!(text.contains("本机操作：暂未查到最新状态"));
            assert!(text.contains("视频制作：已完成"));
            assert!(!text.contains("synthetic transport detail"));
        }
        let snapshot = engine.observe_turn(&ack.turn_id).await.unwrap();
        assert_eq!(snapshot.jobs[0].state, "running");
        assert_eq!(snapshot.jobs[1].state, "succeeded");
        if desktop {
            recovered.store(true, Ordering::SeqCst);
            let result = engine.channel(query.clone()).await.unwrap();
            assert_eq!(result["result"]["unverified_accepted_jobs"], 0);
            assert_eq!(result["result"]["pending_accepted_jobs"], 0);
            assert_eq!(result["result"]["artifacts"][0]["sha256"], digest);
        }
    }
    assert_eq!(unavailable_reads.load(Ordering::SeqCst), 7);
    assert_eq!(finished_reads.load(Ordering::SeqCst), 2);
    assert_eq!(models.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_experience_unknown_model_has_a_read_only_next_step_shared_with_wechat() {
    let (engine, models, effects, ack, _root) = interrupted(true).await;
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    let turn = engine.turn(&ack.turn_id).await.unwrap();
    assert!(!turn.resumable);
    let blocked = turn
        .progress
        .blocked
        .as_deref()
        .expect("Unknown model needs a concrete reason");
    assert!(blocked.contains("模型"));
    assert!(turn.progress.next.as_deref().unwrap().contains("核对"));
    for _ in 0..2 {
        let result = engine
            .channel(
                json!({"operation":"get","source_key":"synthetic-owner","turn_id":ack.turn_id}),
            )
            .await
            .unwrap();
        assert!(result["result"]["text"].as_str().unwrap().contains("模型"));
        assert_eq!(result["result"]["progress_details"], json!(turn.progress));
    }
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn canceled_wechat_turn_retains_unknown_model_outcome_without_resuming() {
    let (engine, models, effects, ack, _root) = interrupted(true).await;
    engine.change_turn(&ack.turn_id, |session, index| {
        let turn = &mut session.turns[index];
        turn.state = "canceled".into();
        turn.text = "合成原取消记录".into();
        turn.model_failure = None;
        for (id, name, result) in [
            ("retained-discovery", "homebase_discover", json!({"fixture":"synthetic-discovery-result"})),
            ("incomplete-skill", "homebase_installed_skill", json!({"result":{"status":"failed"}})),
        ] {
            turn.steps.push(Step {
                id: id.into(),
                name: name.into(),
                state: "completed".into(),
                arguments: json!({}),
                job_id: None,
                result,
            });
        }
        Ok(())
    }).await.unwrap();
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    let turn = engine.turn(&ack.turn_id).await.unwrap();
    assert_eq!(turn.state, "canceled");
    assert_eq!(turn.text, "合成原取消记录");
    assert!(turn.model_outcome_unknown);
    assert!(!turn.resumable);
    assert!(turn.progress.blocked.as_deref().unwrap().contains("仍未确认"));
    assert!(turn.progress.next.as_deref().unwrap().contains("核对原任务"));

    let query = json!({"operation":"get","source_key":"synthetic-owner","turn_id":ack.turn_id});
    let result = engine.channel(query.clone()).await.unwrap();
    assert_eq!(result["result"]["status"], "canceled");
    assert_eq!(result["result"]["model_outcome_unknown"], true);
    assert_eq!(result["result"]["progress_details"], json!(turn.progress));
    let message = result["result"]["text"].as_str().unwrap();
    assert!(message.contains("后续步骤已停下"));
    assert!(message.contains("模型请求的结果仍未确认"));
    assert!(message.contains("已完成：核对可用功能与条件"));
    assert!(!message.contains("使用已授权技能"));
    assert!(!message.contains("synthetic-discovery-result"));
    assert!(engine.resume(&ack.turn_id).await.is_err());
    assert_eq!(serde_json::to_value(&*engine.state.lock().await).unwrap(), before);

    engine.change_turn(&ack.turn_id, |session, index| {
        session.turns[index].runtime_uncertain_model = false;
        Ok(())
    }).await.unwrap();
    let ordinary = engine.channel(query).await.unwrap();
    assert_eq!(ordinary["result"]["model_outcome_unknown"], false);
    assert_eq!(ordinary["result"]["text"], "这件事的后续步骤已经停下，已有结果会保留。");
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_experience_resume_hint_matches_the_other_turn_guard_without_starting_work() {
    let (engine, models, effects, ack, _root) = interrupted(false).await;
    let mut later = request("同会话后来的合成消息", "recovery-later");
    later.session_id = Some(ack.session_id.clone());
    let later_ack = engine
        .send_from(later, "wechat", "synthetic-owner")
        .await
        .unwrap();
    engine
        .change_turn(&later_ack.turn_id, |session, index| {
            session.turns[index].state = "running".into();
            Ok(())
        })
        .await
        .unwrap();
    engine
        .mutate(Change::default(), |state| {
            state.settings.enabled = true;
            Ok(())
        })
        .await
        .unwrap();
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    let turn = engine.turn(&ack.turn_id).await.unwrap();
    assert!(
        !turn.resumable,
        "The UI must not offer a continuation that the engine rejects"
    );
    let error = match engine.resume(&ack.turn_id).await {
        Err(error) => error,
        Ok(_) => panic!("Other turn must block continuation"),
    };
    assert!(error.contains("其他任务"));
    assert!(turn.progress.next.as_deref().unwrap().contains("核对"));
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_experience_rejected_model_retains_cause_and_only_offers_the_original_checkpoint()
{
    let (engine, models, effects, ack, _root) = interrupted(false).await;
    engine
        .change_turn(&ack.turn_id, |session, index| {
            session.turns[index].state = "failed".into();
            session.turns[index].model_failure = Some(Failure::http(429));
            Ok(())
        })
        .await
        .unwrap();
    let view = engine.turn(&ack.turn_id).await.unwrap();
    assert!(view.resumable);
    assert!(view.progress.blocked.as_deref().unwrap().contains("限流"));
    assert!(view
        .progress
        .next
        .as_deref()
        .unwrap()
        .contains("原模型设置"));
    let result = engine
        .channel(json!({"operation":"get","source_key":"synthetic-owner","turn_id":ack.turn_id}))
        .await
        .unwrap();
    assert!(result["result"]["text"].as_str().unwrap().contains("限流"));
    engine
        .change_turn(&ack.turn_id, |session, index| {
            session.turns[index].runtime_operations = Records::default();
            Ok(())
        })
        .await
        .unwrap();
    let missing = engine.turn(&ack.turn_id).await.unwrap();
    assert!(!missing.resumable);
    assert!(missing
        .progress
        .next
        .as_deref()
        .unwrap()
        .contains("无法直接续接"));
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_experience_unknown_business_result_never_recommends_the_old_retry_instruction() {
    let (engine, models, effects, ack, _root) = interrupted(false).await;
    engine
        .change_turn(&ack.turn_id, |session, index| {
            session.turns[index].workflow.stop = Some(workflow::StopInfo {
                kind: "failed".into(),
                message: "原任务暂停".into(),
                next_action: "直接重做旧动作".into(),
                retry_safe: false,
            });
            session.jobs.push(Job {
                archived_result_bytes: None,
                id: "unknown-original-job".into(),
                command: "assistant_local_execute".into(),
                turn_id: ack.turn_id.clone(),
                state: "succeeded".into(),
                result: json!({"status":"succeeded","result":{"status":"unknown"}}),
                owned: true,
                deliver: false,
                artifacts: vec![],
                remote: None,
            });
            session.turns[index].steps.push(Step {
                id: "legacy-step".into(), name: "homebase_files".into(), state: "completed".into(),
                arguments: json!({"request":{"action":"write","path":"fixture.txt"}}),
                result: json!({"status":"succeeded"}), job_id: Some("unknown-original-job".into()),
            });
            let turn = &mut session.turns[index];
            turn.workflow.plan = Some(workflow::Plan {
                revision: 2,
                goal: "保存合成结果".into(),
                items: vec![workflow::Item {
                    input: workflow::ItemInput {
                        id: "write".into(), title: "保存合成文件".into(),
                        done_when: "原写入已经核对".into(), kind: workflow::EvidenceKind::Write,
                    },
                    state: "completed".into(), note: "旧计划声称成功".into(), evidence: vec!["legacy-step".into()],
                }],
            });
            Ok(())
        })
        .await
        .unwrap();
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    let view = engine.turn(&ack.turn_id).await.unwrap();
    assert_eq!(view.progress.completed_stages, 0);
    assert_eq!(view.progress.total_stages, 1);
    let item = &view.task.as_ref().unwrap().plan.as_ref().unwrap().items[0];
    assert_eq!(item.state, "blocked");
    assert_eq!(item.evidence, ["legacy-step"]);
    let task = engine.task_tool(&ack.turn_id, &json!({"action":"get"})).await.unwrap();
    assert_eq!(task["plan"]["items"][0]["state"], "blocked");
    assert_eq!(task["revision"], 2);
    let gate = engine.completion_gate(&ack.turn_id, false, "全部完成").await.unwrap();
    assert!(gate.feedback.is_none());
    assert!(!gate.stop.unwrap().retry_safe);
    {
        let state = engine.state.lock().await;
        let (s, t) = locate(&state, &ack.turn_id).unwrap();
        let mut completing = state.sessions[s].clone();
        completing.turns[t].history_committed = false;
        Engine::finish_turn(&mut completing, t, &ack.turn_id, "completed", "全部完成").unwrap();
        assert_eq!(completing.turns[t].state, "needs_input");
        assert!(completing.turns[t].text.contains("完成结果待核对"));
    }
    assert!(view
        .progress
        .blocked
        .as_deref()
        .unwrap()
        .contains("确认执行结果"));
    assert!(view
        .progress
        .next
        .as_deref()
        .unwrap()
        .contains("核对原任务"));
    let result = engine
        .channel(json!({"operation":"get","source_key":"synthetic-owner","turn_id":ack.turn_id}))
        .await
        .unwrap();
    let text = result["result"]["text"].as_str().unwrap();
    assert!(!text.contains("直接重做旧动作"));
    assert!(text.contains("本机操作：结果还未确认"), "{text}");
    assert!(!text.contains("已完成：保存 fixture.txt"), "{text}");
    assert!(!text.contains("已完成 1/1"), "{text}");
    assert!(engine
        .channel(json!({"operation":"get","source_key":"foreign-owner","turn_id":ack.turn_id}))
        .await
        .is_err());
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_experience_cumulative_cap_blocks_failed_continuation_before_any_model_or_state_change(
) {
    let (engine, models, effects, ack, _root) = interrupted(false).await;
    engine
        .change_turn(&ack.turn_id, |session, index| {
            session.turns[index].state = "failed".into();
            session.turns[index].steps = (0..workflow::MAX_TASK_STEPS)
                .map(|n| Step {
                    id: format!("saved-{n}"),
                    name: "synthetic-only".into(),
                    state: "completed".into(),
                    arguments: Value::Null,
                    result: json!({"status":"ready"}),
                    job_id: None,
                })
                .collect();
            Ok(())
        })
        .await
        .unwrap();
    engine
        .mutate(Change::default(), |state| {
            state.settings.enabled = true;
            Ok(())
        })
        .await
        .unwrap();
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    assert!(!engine.turn(&ack.turn_id).await.unwrap().resumable);
    let error = match engine.resume(&ack.turn_id).await {
        Err(error) => error,
        Ok(_) => panic!("Cumulative cap must block before scheduling"),
    };
    assert!(error.contains("累计上限"));
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
#[ignore = "Explicit new F-drive synthetic native-UI seed only"]
async fn recovery_experience_export_native_seed() {
    let destination = PathBuf::from(
        std::env::var_os("HOMEBASE_RECOVERY_SEED").expect("Explicit seed destination"),
    );
    assert!(destination.is_absolute());
    assert!(destination
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    assert!(
        !destination.exists(),
        "Never overwrite a profile or previous evidence"
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let model: Model = Arc::new(move |_| {
        let observed = observed.clone();
        Box::pin(async move {
            observed.fetch_add(1, Ordering::SeqCst);
            Err(Failure::unknown(Kind::Timeout).encode())
        })
    });
    let (engine, effects, saved, _temporary) = setup(model).await;
    let ack = engine
        .send(request(
            "HB-RECOVERY-NATIVE 合成中断展示，仅核对原任务",
            "synthetic-native-recovery",
        ))
        .await
        .unwrap();
    let failed = task_wait(&engine, &ack.turn_id, "failed").await;
    assert!(!failed.resumable);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let mut state = saved.lock().await.clone();
    assert!(
        !state.sessions[0].turns[0].runtime_operations.is_empty(),
        "Use an actual synthetic graph checkpoint, not fabricated operations"
    );
    state.settings.enabled = false;
    std::fs::create_dir(&destination).unwrap();
    let assistant = destination.join(".homebase/assistant");
    std::fs::create_dir_all(&assistant).unwrap();
    let plain = zeroize::Zeroizing::new(serde_json::to_vec(&state).unwrap());
    let cipher = crate::wechat_crypto::crypt(&plain, true).unwrap();
    crate::control_protocol::publish_private(&assistant.join("state.dpapi"), &cipher)
        .await
        .unwrap();
    let metadata = json!({"passed":true,"session_id":ack.session_id,"turn_id":ack.turn_id,
        "source":"desktop","settings_enabled":false,"actual_synthetic_graph_checkpoint":true,
        "synthetic_model_calls":1,"business_effects":0,"real_model_calls":0,"real_wechat_messages":0});
    use std::io::Write;
    let mut report = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination.join("synthetic-seed.json"))
        .unwrap();
    report
        .write_all(serde_json::to_string_pretty(&metadata).unwrap().as_bytes())
        .unwrap();
}
