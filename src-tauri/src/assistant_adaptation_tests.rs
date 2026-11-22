//! Real graph, synthetic models/controllers and isolated persistence; no credentials.
use super::*;
use workflow_tests::{task_wait, tool_reply};
// Available only in the opt-in test binary, never in the desktop/CLI interface.
impl Engine {
    pub(crate) async fn advance_isolated_live_followup(&self,fid:&str)->Result<(),String>{
        let lab=PathBuf::from(std::env::var_os("HOMEBASE_PERSONAL_LIVE_LAB").ok_or("explicit live lab required")?);
        if !lab.is_absolute()||!self.workspace.starts_with(&lab){return Err("not an isolated live workspace".into());}
        let _gate=self.followup_gate.lock().await;
        self.mutate(Change{personal:true,..Default::default()},|s|{
            let entry=s.followups.entries.iter_mut().find(|e|e.id==fid).ok_or("isolated plan missing")?;
            if !entry.enabled||entry.state!="scheduled" {return Err("only the next idle test schedule may be advanced".into());}
            entry.due_at=(chrono::Utc::now()+chrono::Duration::milliseconds(100)).to_rfc3339();s.followups.revision+=1;Ok(())
        }).await?;self.personal_wake.notify_one();Ok(())
    }
}
fn answer(text: &str) -> ModelReply {
    wire::parse("chat", &json!({"choices":[{"message":{"content":text}}]})).unwrap()
}
#[tokio::test]
async fn textual_consent_draft_is_reviewed_into_real_approval_and_never_claimed_prepared(){
    let calls=Arc::new(AtomicUsize::new(0));let n=calls.clone();
    let model:Model=Arc::new(move|_|{let n=n.clone();Box::pin(async move{Ok(match n.fetch_add(1,Ordering::SeqCst){
        0=>answer("已备好草案，请确认是否保存。"),
        1=>tool_reply(1,"homebase_memory",json!({"action":"remember","revision":0,"title":"合成确认","content":"合成内容"})),
        2=>answer("已保存这条测试记忆。"),_=>panic!("unexpected replay")
    })})});
    let(e,_,_,_root)=setup(model,false).await;let ack=e.send(request("帮我记住合成内容","consent-draft-real")).await.unwrap();let pending=task_wait(&e,&ack.turn_id,"awaiting_confirmation").await;
    assert!(e.personal().await["memories"].as_array().unwrap().is_empty());e.confirm(&ack.turn_id,&pending.pending.unwrap().id,true).await.unwrap();
    let done=task_wait(&e,&ack.turn_id,"completed").await;assert_eq!(done.steps[0].result["homebase_confirmation"]["approved"],true);assert_eq!(calls.load(Ordering::SeqCst),3);
}
#[tokio::test]
async fn repeated_textual_consent_is_a_bounded_stop_without_fake_completion(){
    let model:Model=Arc::new(|_|Box::pin(async{Ok(answer("已备好草案，请确认是否保存。"))}));let(e,effects,_,_root)=setup(model,false).await;
    let ack=e.send(request("帮我记住合成内容","consent-draft-stop")).await.unwrap();let stopped=task_wait(&e,&ack.turn_id,"needs_input").await;
    assert!(stopped.pending.is_none());assert!(stopped.text.contains("没有准备出可确认"));assert_eq!(effects.load(Ordering::SeqCst),0);assert!(e.personal().await["memories"].as_array().unwrap().is_empty());
}
async fn setup(
    model: Model,
    free: bool,
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
    state.settings.free_only = free;
    state.settings.route_id = "openrouter::synthetic:free".into();
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
async fn in_flight_change_invalidates_old_approval_and_tool_keeps_original_task_and_restarts_without_replay(
) {
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let model: Model = Arc::new(move |r| {
        let n = n.clone();
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            Ok(match i {
                0 => tool_reply(
                    i,
                    "homebase_memory",
                    json!({"action":"remember","revision":0,"title":"合成旧方案","content":"红点"}),
                ),
                1 => {
                    let input = serde_json::to_string(&r.messages).unwrap();
                    assert!(input.contains("蓝色圆点"));
                    assert!(input.contains("superseded"));
                    tool_reply(
                        i,
                        "homebase_files",
                        json!({"action":"read","path":"synthetic.txt"}),
                    )
                }
                2 => answer("已按补充核对蓝色圆点方案，没有保存旧偏好。"),
                _ => panic!("unexpected replay"),
            })
        })
    });
    let (e, effects, saved, _root) = setup(model, false).await;
    let ack = e
        .send(request("先保存合成旧偏好", "adapt-steer-first"))
        .await
        .unwrap();
    let old = task_wait(&e, &ack.turn_id, "awaiting_confirmation").await;
    assert!(e
        .steer_from(
            SteerInput {
                turn_id: ack.turn_id.clone(),
                revision: 0,
                request_id: "foreign".into(),
                text: "改蓝".into()
            },
            "foreign-owner"
        )
        .await
        .is_err());
    let input = || SteerInput {
        turn_id: ack.turn_id.clone(),
        revision: 0,
        request_id: "adapt-steer-change".into(),
        text: "这次改成蓝色圆点，只核对合成文件，不保存偏好".into(),
    };
    assert_eq!(e.steer(input()).await.unwrap()["revision"], 1);
    assert!(e
        .confirm(&ack.turn_id, &old.pending.unwrap().id, true)
        .await
        .is_err());
    assert_eq!(e.steer(input()).await.unwrap()["duplicate"], true);
    let done = task_wait(&e, &ack.turn_id, "completed").await;
    assert_eq!(done.id, ack.turn_id);
    assert_eq!(done.requirements_revision, 1);
    assert_eq!(done.requirements.len(), 1);
    assert!(done.text.contains("蓝色圆点"));
    assert_eq!(e.personal().await["memories"].as_array().unwrap().len(), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    let restored = Engine::new(
        saved.lock().await.clone(),
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        restored
            .turn(&ack.turn_id)
            .await
            .unwrap()
            .requirements_revision,
        1
    );
    let before_replay = restored.state.lock().await.revision;
    assert_eq!(restored.steer(input()).await.unwrap()["duplicate"], true);
    assert_eq!(restored.state.lock().await.revision, before_replay);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(count.load(Ordering::SeqCst), 3);
}
#[tokio::test]
async fn change_during_model_request_cannot_commit_old_final_answer() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let signal = entered.clone();
    let gate = release.clone();
    let model: Model = Arc::new(move |r| {
        let n = n.clone();
        let signal = signal.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i == 0 {
                signal.notify_one();
                gate.notified().await;
                return Ok(answer("旧方案已完成"));
            }
            assert_eq!(i, 1);
            assert!(serde_json::to_string(&r.messages)
                .unwrap()
                .contains("只要两句中文"));
            Ok(answer("已经改成两句中文。原任务没有重新提交。"))
        })
    });
    let (e, effects, _, _root) = setup(model, false).await;
    let ack = e
        .send(request("处理合成内容", "adapt-mid-model"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    e.steer(SteerInput {
        turn_id: ack.turn_id.clone(),
        revision: 0,
        request_id: "mid-model-change".into(),
        text: "改一下，只要两句中文".into(),
    })
    .await
    .unwrap();
    release.notify_one();
    let done = task_wait(&e, &ack.turn_id, "completed").await;
    assert!(!done.text.contains("旧方案"));
    assert!(done.text.contains("两句中文"));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn event_followup_wakes_on_original_completion_once_and_survives_restart() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let signal = entered.clone();
    let gate = release.clone();
    let model: Model = Arc::new(move |r| {
        let n = n.clone();
        let signal = signal.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i == 0 {
                signal.notify_one();
                gate.notified().await;
                Ok(answer("合成原任务已核对"))
            } else {
                assert_eq!(i, 1);
                assert!(serde_json::to_string(&r.messages)
                    .unwrap()
                    .contains("原任务不重做"));
                Ok(answer("原任务完成后已核对后续事项，没有重新执行原任务。"))
            }
        })
    });
    let (e, effects, saved, _root) = setup(model, true).await;
    let ack = e
        .send(request("处理合成原任务", "adapt-event-original"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    let make = |revision| FollowupInput {
        revision,
        title: "完成后核对".into(),
        instruction: "只核对原结果并简短汇报，不重复执行".into(),
        due_at: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        interval_minutes: None,
        max_runs: 1,
        matter_id: None,
        trigger: Some(followup::FollowupTrigger {
            turn_id: ack.turn_id.clone(),
            on: "completed".into(),
        }),
        notification_policy: None,
    };
    assert!(e.followup_save(make(0), "wechat", "foreign").await.is_err());
    let follow = e
        .followup_save(make(0), "desktop", "desktop")
        .await
        .unwrap();
    assert_eq!(follow["state"], "waiting_event");
    e.followup_tick().await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    release.notify_one();
    task_wait(&e, &ack.turn_id, "completed").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    let follow_turn = loop {
        e.followup_tick().await.unwrap();
        let f = e.followups().await;
        if let Some(tid) = f["entries"][0]["runs"][0]["turn_id"].as_str() {
            break tid.to_string();
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    let result = task_wait(&e, &follow_turn, "completed").await;
    assert_eq!(result.session_id, ack.session_id);
    e.followup_tick().await.unwrap();
    let persisted = saved.lock().await.clone();
    let restored = Engine::new(
        persisted,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    restored.followup_tick().await.unwrap();
    assert_eq!(
        restored.followups().await["entries"][0]["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn expired_event_never_runs_and_already_happened_attention_is_not_replayed() {
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let model: Model = Arc::new(move |_| {
        let n = n.clone();
        Box::pin(async move {
            n.fetch_add(1, Ordering::SeqCst);
            Ok(tool_reply(
                0,
                "homebase_memory",
                json!({"action":"remember","revision":0,"title":"合成等待","content":"测试"}),
            ))
        })
    });
    let (e, effects, _, _root) = setup(model, true).await;
    let ack = e
        .send(request("等待确认", "adapt-event-expiry"))
        .await
        .unwrap();
    task_wait(&e, &ack.turn_id, "awaiting_confirmation").await;
    let make = |on: &str| FollowupInput {
        revision: 0,
        title: "等待事件".into(),
        instruction: "仅在约定事件之后核对".into(),
        due_at: (chrono::Utc::now() + chrono::Duration::milliseconds(150)).to_rfc3339(),
        interval_minutes: None,
        max_runs: 1,
        matter_id: None,
        trigger: Some(followup::FollowupTrigger {
            turn_id: ack.turn_id.clone(),
            on: on.into(),
        }),
        notification_policy: None,
    };
    assert!(e
        .followup_save(make("needs_attention"), "desktop", "desktop")
        .await
        .is_err());
    e.followup_save(make("completed"), "desktop", "desktop")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(220)).await;
    e.followup_tick().await.unwrap();
    let state = e.followups().await;
    assert_eq!(state["entries"][0]["state"], "missed");
    assert!(state["entries"][0]["runs"].as_array().unwrap().is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    e.cancel(&ack.turn_id).await.unwrap();
}
#[tokio::test]
async fn attention_event_uses_separate_context_without_approving_or_waiting_behind_original_task() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let n = Arc::new(AtomicUsize::new(0));
    let count = n.clone();
    let signal = entered.clone();
    let gate = release.clone();
    let model: Model = Arc::new(move |r| {
        let n = n.clone();
        let signal = signal.clone();
        let gate = gate.clone();
        Box::pin(async move {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i == 0 {
                signal.notify_one();
                gate.notified().await;
                Ok(tool_reply(
                    0,
                    "homebase_memory",
                    json!({"action":"remember","revision":0,"title":"合成待确认","content":"测试"}),
                ))
            } else {
                assert_eq!(i, 1);
                assert!(serde_json::to_string(&r.messages)
                    .unwrap()
                    .contains("awaiting_confirmation"));
                Ok(answer("原任务正在等你确认，没有替你批准。"))
            }
        })
    });
    let (e, _, _, _root) = setup(model, true).await;
    let ack = e
        .send(request("处理合成原任务", "adapt-attention-original"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), entered.notified())
        .await
        .unwrap();
    e.followup_save(
        FollowupInput {
            revision: 0,
            title: "需要帮助时跟进".into(),
            instruction: "仅核对阻塞原因，告诉我下一步，不替我批准".into(),
            due_at: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            interval_minutes: None,
            max_runs: 1,
            matter_id: None,
            trigger: Some(followup::FollowupTrigger {
                turn_id: ack.turn_id.clone(),
                on: "needs_attention".into(),
            }),
            notification_policy: None,
        },
        "desktop",
        "desktop",
    )
    .await
    .unwrap();
    release.notify_one();
    task_wait(&e, &ack.turn_id, "awaiting_confirmation").await;
    let end = tokio::time::Instant::now() + Duration::from_secs(40);
    let tid = loop {
        e.followup_tick().await.unwrap();
        if let Some(id) = e.followups().await["entries"][0]["runs"][0]["turn_id"].as_str() {
            break id.to_string();
        };
        assert!(tokio::time::Instant::now() < end);
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    let done = task_wait(&e, &tid, "completed").await;
    assert_ne!(done.session_id, ack.session_id);
    assert_eq!(
        e.turn(&ack.turn_id).await.unwrap().state,
        "awaiting_confirmation"
    );
    assert!(e.personal().await["memories"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    e.cancel(&ack.turn_id).await.unwrap();
}
#[tokio::test]
async fn notification_fingerprints_use_actual_proof_and_never_suppress_ambiguous_or_mutating_results(
) {
    let model: Model = Arc::new(|_| Box::pin(async { Ok(answer("合成检查完成")) }));
    let (e, _, saved, _root) = setup(model, true).await;
    let ack = e
        .send(request("合成检查", "adapt-notice-proof"))
        .await
        .unwrap();
    task_wait(&e, &ack.turn_id, "completed").await;
    e.followup_save(
        FollowupInput {
            revision: 0,
            title: "定时合成检查".into(),
            instruction: "只读取合成文件".into(),
            due_at: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            interval_minutes: Some(5),
            max_runs: 2,
            matter_id: None,
            trigger: None,
            notification_policy: None,
        },
        "desktop",
        "desktop",
    )
    .await
    .unwrap();
    let mut state = saved.lock().await.clone();
    state.followups.entries[0].runs.push(followup::FollowupRun {
        ordinal: 1,
        scheduled_at: now(),
        request_id: "synthetic-run".into(),
        turn_id: Some(ack.turn_id.clone()),
        state: "completed".into(),
    });
    let (s, t) = locate(&state, &ack.turn_id).unwrap();
    let notice = |state: &State, artifacts| {
        notifications::metadata(
            state,
            &state.sessions[s],
            &state.sessions[s].turns[t],
            artifacts,
        )
    };
    assert!(notice(&state, false).is_null());
    state.sessions[s].turns[t].steps.push(Step{id:"proof".into(),name:"homebase_files".into(),state:"completed".into(),arguments:json!({"request":{"action":"read","path":"synthetic.txt"}}),result:json!({"status":"succeeded","content":"blue","step_id":"first","checked_at":"before"}),job_id:None});
    let first = notice(&state, false);
    assert!(!first.is_null());
    state.sessions[s].turns[t].steps.push(Step{id:"setup".into(),name:"homebase_files".into(),state:"completed".into(),arguments:json!({"request":{"action":"workspaces"}}),result:json!({"status":"succeeded","workspaces":[{"id":"scratch"}]}),job_id:None});
    state.sessions[s].turns[t].steps.push(Step{id:"setup-plan".into(),name:"homebase_followup".into(),state:"completed".into(),arguments:json!({"action":"list"}),result:json!({"revision":900,"entries":[{"state":"running","run_id":"self-new-run"}]}),job_id:None});
    assert_eq!(notice(&state,false),first);state.sessions[s].turns[t].steps.truncate(1);
    state.sessions[s].turns[t].text = "另一种措辞，实际证据相同".into();
    state.sessions[s].turns[t].steps[0].result["step_id"] = json!("second");
    state.sessions[s].turns[t].steps[0].result["checked_at"] = json!("after");
    assert_eq!(notice(&state, false), first);
    state.sessions[s].turns[t].steps[0].result["content"] = json!("red");
    assert_ne!(notice(&state, false), first);
    assert!(notice(&state, true).is_null());
    state.sessions[s].turns[t].steps[0].result["status"] = json!("unknown");
    assert!(notice(&state, false).is_null());
    state.sessions[s].turns[t].steps[0].result["status"] = json!("succeeded");
    state.sessions[s].turns[t].steps[0].arguments["request"]["action"] = json!("write");
    assert!(notice(&state, false).is_null());
}
