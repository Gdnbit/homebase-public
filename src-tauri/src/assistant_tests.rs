use super::*;
#[path = "assistant_workflow_tests.rs"]
mod workflow_tests;
#[path = "assistant_control_tests.rs"]
mod control_tests;
#[path = "assistant_continuity_tests.rs"]
mod continuity_tests;
#[path = "assistant_dialogue_tests.rs"]
mod dialogue_tests;
#[path = "assistant_adaptation_tests.rs"]
mod adaptation_tests;
#[path = "assistant_reliability_tests.rs"]
mod reliability_tests;
#[path = "assistant_document_tests.rs"]
mod document_tests;
#[path = "assistant_runtime_compatibility_tests.rs"]
mod runtime_compatibility_tests;
#[path = "assistant_context_tests.rs"]
mod context_tests;
#[path = "assistant_capability_discovery_tests.rs"]
mod capability_discovery_tests;
#[path = "assistant_task_discovery_tests.rs"]
mod task_discovery_tests;
#[path = "assistant_records_tests.rs"]
mod records_tests;
#[path = "assistant_history_tests.rs"]
mod history_tests;
#[path = "assistant_job_access_tests.rs"]
mod job_access_tests;
#[path = "assistant_window_tests.rs"]
mod window_tests;
use crate::control_state::{Controller, Runner};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicUsize, Ordering},
};

fn reply(calls: Vec<(&str, Value)>) -> ModelReply {
    let tools:Vec<_>=calls.into_iter().enumerate().map(|(i,(name,args))|json!({"id":format!("call-{i}"),"function":{"name":name,"arguments":args.to_string()}})).collect();
    wire::parse("chat",&json!({"choices":[{"message":if tools.is_empty(){json!({"content":"已根据真实结果完成查询"})}else{json!({"tool_calls":tools})}}]})).unwrap()
}

#[tokio::test]
async fn repository_install_prepares_concrete_human_confirmation_and_host_binds_request_identity(){
    let(e,_,executions,_,_root)=fixture(true,vec![reply(vec![("homebase_call",json!({"command":"repo_project_install","args":{"request":{"project_id":"fixture-project","revision":2,"request_id":"model-chosen"}}}))]),reply(vec![])]).await;
    let ack=e.send(request("安装隔离项目","repo-install-consent")).await.unwrap();let pending=wait(&e,&ack.turn_id,"awaiting_confirmation").await.pending.unwrap();
    assert_eq!(executions.load(Ordering::SeqCst),0);assert_eq!(pending.arguments["request"]["project_id"],"fixture-project");assert_ne!(pending.arguments["request"]["request_id"],"model-chosen");
    e.confirm(&ack.turn_id,&pending.id,true).await.unwrap();wait(&e,&ack.turn_id,"completed").await;assert_eq!(executions.load(Ordering::SeqCst),1);
}

#[tokio::test]
async fn repository_results_cannot_be_guessed_across_sessions(){
    let(e,_,executions,_,_root)=fixture(false,vec![]).await;let ack=e.send(request("隔离归属测试","repo-result-owner")).await.unwrap();
    let call=ToolCall{id:"repo-other-run".into(),name:"homebase_call".into(),arguments:json!({"command":"repo_capability_result","args":{"runId":"unowned-run"}})};
    assert!(e.execute(&ack.turn_id,&call).await.unwrap_err().contains("本会话"));assert_eq!(executions.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn local_controller_failure_preserves_explicit_reason_instead_of_null_result() {
    let(e,_,_,saved,_root)=fixture(false,vec![]).await;
    let first=e.send(request("合成失败说明","local-error-reason")).await.unwrap();
    let control:Control=Arc::new(|_|Box::pin(async{Ok(json!({"id":"failed-local","command":"assistant_local_execute","status":"failed","result":null,"error":{"code":"LOCAL_CONFIGURATION","message":"合成凭据引用不可用，未回退其他来源"}}))}));
    let new=Engine::new(saved.lock().await.clone(),e.save.clone(),e.model.clone(),control,e.workspace.clone()).await.unwrap();
    new.change_turn(&first.turn_id,|session,_|{session.jobs.push(Job{id:"failed-local".into(),command:"assistant_local_execute".into(),turn_id:first.turn_id.clone(),state:"running".into(),result:Value::Null,owned:true,deliver:false,artifacts:vec![],remote:None,archived_result_bytes:None});Ok(())}).await.unwrap();
    new.refresh_job(&first.session_id,"failed-local").await.unwrap();
    let view=new.turn(&first.turn_id).await.unwrap();assert_eq!(view.jobs[0].state,"failed");assert_eq!(view.jobs[0].result["code"],"LOCAL_CONFIGURATION");assert!(view.jobs[0].result["message"].as_str().unwrap().contains("未回退"));
}

#[tokio::test]
async fn local_actions_use_owned_jobs_and_concrete_approval_without_changing_model_settings() {
    let(e,_,executions,saved,_root)=fixture(true,vec![reply(vec![("homebase_files",json!({"action":"write","path":"proof.txt","content":"synthetic"}))]),reply(vec![]),reply(vec![("homebase_files",json!({"action":"edit","path":"proof.txt","old_text":"synthetic","new_text":"updated","expected_sha256":"a".repeat(64)}))]),reply(vec![])]).await;
    let settings=serde_json::to_value(e.settings().await).unwrap();
    let first=e.send(request("创建隔离文件","local-write")).await.unwrap();
    let complete=wait(&e,&first.turn_id,"completed").await;
    assert_eq!(executions.load(Ordering::SeqCst),1);
    assert_eq!(complete.jobs[0].command,"assistant_local_execute");
    assert_eq!(complete.steps[0].arguments["policyRevision"],0);
    let job_id=complete.jobs[0].id.clone();
    e.change_turn(&first.turn_id,|s,t|{s.turns[t].rounds+=1;Ok(())}).await.unwrap();
    let query=e.execute(&first.turn_id,&ToolCall{id:"query-local-owned".into(),name:"homebase_job".into(),arguments:json!({"job_id":job_id})}).await.unwrap().unwrap();
    assert_eq!(query["state"],"succeeded");assert_eq!(executions.load(Ordering::SeqCst),1);
    let mut request=request("编辑隔离文件","local-edit");request.session_id=Some(first.session_id);
    let second=e.send(request).await.unwrap();let pending=wait(&e,&second.turn_id,"awaiting_confirmation").await;
    assert_eq!(pending.pending.as_ref().unwrap().arguments["request"]["path"],"proof.txt");
    assert_eq!(executions.load(Ordering::SeqCst),1);
    e.confirm(&second.turn_id,&pending.pending.unwrap().id,true).await.unwrap();wait(&e,&second.turn_id,"completed").await;
    assert_eq!(executions.load(Ordering::SeqCst),2);
    let mut config=e.local_config().await;config.enabled=false;e.save_local_config(config).await.unwrap();
    assert_eq!(saved.lock().await.local.revision,1);assert_eq!(serde_json::to_value(e.settings().await).unwrap(),settings);
}

#[tokio::test]
async fn stopping_local_waiting_job_cancels_even_when_submission_step_has_completed() {
    let(e,_,executions,_saved,_root)=fixture(true,vec![reply(vec![("homebase_terminal",json!({"command":"synthetic-only"}))]),reply(vec![])]).await;
    let first=e.send(request("合成终端等待","local-cancel-waiting")).await.unwrap();
    wait(&e,&first.turn_id,"completed").await;
    e.change_turn(&first.turn_id,|session,index|{
        session.turns[index].state="waiting_jobs".into();
        session.turns[index].history_committed=false;
        session.turns[index].steps[0].state="completed".into();
        session.jobs[0].state="running".into();Ok(())
    }).await.unwrap();
    e.cancel(&first.turn_id).await.unwrap();
    assert_eq!(e.turn(&first.turn_id).await.unwrap().state,"canceled");
    for _ in 0..30 {if executions.load(Ordering::SeqCst)==2 {return;}tokio::time::sleep(Duration::from_millis(20)).await;}
    panic!("local cancel command was not submitted");
}

#[tokio::test]
async fn skill_result_workspace_is_readable_only_in_its_own_session() {
    let(e,_,executions,_saved,_root)=fixture(true,vec![reply(vec![]),reply(vec![])]).await;
    let first=e.send(request("合成技能产物归属","skill-owner")).await.unwrap();wait(&e,&first.turn_id,"completed").await;
    let workspace="skill-12345678-1234-4234-8234-123456789abc";
    e.change_turn(&first.turn_id,|session,_|{
        session.jobs.push(Job{id:"owned-skill".into(),command:"assistant_local_execute".into(),turn_id:first.turn_id.clone(),state:"succeeded".into(),result:json!({"status":"succeeded","skill_id":"installed:fixture:example","workspace_id":workspace}),owned:true,deliver:false,artifacts:vec![],remote:None,archived_result_bytes:None});Ok(())
    }).await.unwrap();
    let call=ToolCall{id:"read-owned-skill".into(),name:"homebase_files".into(),arguments:json!({"action":"read","workspace_id":workspace,"path":"result.txt"})};
    assert!(e.execute(&first.turn_id,&call).await.unwrap().is_some());
    let other=e.send(request("合成另一会话","skill-other-session")).await.unwrap();wait(&e,&other.turn_id,"completed").await;
    assert!(e.execute(&other.turn_id,&call).await.unwrap_err().contains("不属于当前会话"));
    assert_eq!(executions.load(Ordering::SeqCst),1);
}

#[tokio::test]
async fn conversation_followups_are_durable_fifo_and_duplicate_input_is_not_queued_twice(){
    let(e,m,_,saved,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"合成队列","content":"测试"}))]),reply(vec![]),reply(vec![]),reply(vec![])]).await;
    let first=e.send(request("first","queue-first")).await.unwrap();
    let pending=wait(&e,&first.turn_id,"awaiting_confirmation").await;
    let mut second=request("second","queue-second");second.session_id=Some(first.session_id.clone());
    let a=e.send(second.clone()).await.unwrap();let duplicate=e.send(second).await.unwrap();assert_eq!(a.turn_id,duplicate.turn_id);assert_eq!(a.state,"waiting_input_queue");
    let mut third=request("third","queue-third");third.session_id=Some(first.session_id.clone());let b=e.send(third).await.unwrap();
    assert_eq!(m.load(Ordering::SeqCst),1);
    e.confirm(&first.turn_id,&pending.pending.unwrap().id,true).await.unwrap();
    wait(&e,&a.turn_id,"completed").await;wait(&e,&b.turn_id,"completed").await;
    assert_eq!(m.load(Ordering::SeqCst),4);
    let state=saved.lock().await;let session=state.sessions.iter().find(|s|s.id==first.session_id).unwrap();
    assert_eq!(session.history.iter().map(|h|h[0]["content"].as_str().unwrap()).collect::<Vec<_>>(),vec!["first","second","third"]);
}

#[tokio::test]
async fn conversation_restart_preserves_approval_and_pauses_unstarted_followup_until_explicit_resume(){
    let(e,m,_,saved,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"合成恢复","content":"测试"}))]),reply(vec![]),reply(vec![])]).await;
    let first=e.send(request("first","restart-first")).await.unwrap();let pending=wait(&e,&first.turn_id,"awaiting_confirmation").await;
    let mut second=request("second","restart-second");second.session_id=Some(first.session_id.clone());let next=e.send(second).await.unwrap();
    let state=saved.lock().await.clone();let restored=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    assert!(restored.turn(&next.turn_id).await.unwrap().queue_paused);
    assert_eq!(m.load(Ordering::SeqCst),1);
    restored.confirm(&first.turn_id,&pending.pending.unwrap().id,true).await.unwrap();wait(&restored,&first.turn_id,"completed").await;
    assert_eq!(m.load(Ordering::SeqCst),2);assert_eq!(restored.turn(&next.turn_id).await.unwrap().state,"waiting_input_queue");
    restored.resume_queue(&first.session_id).await.unwrap();wait(&restored,&next.turn_id,"completed").await;
    assert_eq!(m.load(Ordering::SeqCst),3);assert_eq!(restored.personal().await["memories"].as_array().unwrap().len(),1);
}

#[tokio::test]
async fn conversation_queue_has_a_limit_and_rejection_does_not_start_followups(){
    let(e,m,_,_,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"未确认","content":"测试"}))])]).await;
    let first=e.send(request("first","limit-first")).await.unwrap();let pending=wait(&e,&first.turn_id,"awaiting_confirmation").await;
    for n in 0..10{let mut next=request("queued",&format!("limit-{n}"));next.session_id=Some(first.session_id.clone());assert_eq!(e.send(next).await.unwrap().state,"waiting_input_queue");}
    let mut overflow=request("overflow","limit-overflow");overflow.session_id=Some(first.session_id.clone());assert!(e.send(overflow).await.err().unwrap().contains("队列已满"));
    e.confirm(&first.turn_id,&pending.pending.unwrap().id,false).await.unwrap();
    assert_eq!(m.load(Ordering::SeqCst),1);let meta=e.sessions().await.into_iter().find(|s|s.id==first.session_id).unwrap();assert_eq!(meta.queue_depth,10);assert!(meta.queue_paused);
}

#[tokio::test]
async fn conversation_short_controls_bind_to_owner_and_new_chat_keeps_memory(){
    let(e,m,c,_,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"合成共享","content":"蓝色圆点"}))]),reply(vec![])]).await;
    let first=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"short-first","text":"请记住合成共享偏好"})).await.unwrap();let tid=first["turn_id"].as_str().unwrap();wait(&e,tid,"awaiting_confirmation").await;
    assert!(e.channel(json!({"operation":"submit","source_key":"owner-b","request_id":"wrong-owner","text":"确认"})).await.is_err());
    e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"short-confirm","text":"确认"})).await.unwrap();wait(&e,tid,"completed").await;
    let status=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"short-status","text":"进度"})).await.unwrap();assert_eq!(status["view"]["status"],"succeeded");
    let fresh=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"short-new","text":"新对话"})).await.unwrap();assert_ne!(fresh["session_id"],first["session_id"]);
    assert_eq!(e.personal().await["memories"][0]["content"],"蓝色圆点");
    assert_eq!(m.load(Ordering::SeqCst),2);assert_eq!(c.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn conversation_new_context_cannot_approve_old_context_with_bare_confirmation(){
    let(e,m,_,_,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"旧对话","content":"未经确认"}))])]).await;
    let value=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"context-first","text":"请记住旧对话测试"})).await.unwrap();let tid=value["turn_id"].as_str().unwrap();let pending=wait(&e,tid,"awaiting_confirmation").await;
    e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"context-new","text":"新对话"})).await.unwrap();
    assert!(e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"context-confirm","text":"确认"})).await.is_err());
    assert!(e.personal().await["memories"].as_array().unwrap().is_empty());
    e.confirm(tid,&pending.pending.unwrap().id,false).await.unwrap();assert_eq!(m.load(Ordering::SeqCst),1);
}

#[tokio::test]
async fn natural_control_prefixes_are_conversation_and_uuid_controls_remain_owner_scoped(){
    let(e,m,c,_,_root)=fixture(true,vec![reply(vec![]),reply(vec![]),reply(vec![])]).await;
    for (n,text) in ["继续 HB-SELFTEST：请核对原结果，不重新生成","确认 一下刚才是否完成","拒绝 不必要的重复计算，查询原任务就好"].iter().enumerate(){
        let ack=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":format!("natural-prefix-{n}"),"text":text})).await.unwrap();
        wait(&e,ack["turn_id"].as_str().unwrap(),"completed").await;
    }
    let tid=e.sessions().await.into_iter().find(|s|s.source=="wechat").unwrap().active_turn_id;
    assert!(tid.is_none());
    assert_eq!(m.load(Ordering::SeqCst),3);assert_eq!(c.load(Ordering::SeqCst),0);
    let state=e.state.lock().await;let original=state.sessions.last().unwrap().turns[0].id.clone();drop(state);
    assert!(e.channel(json!({"operation":"submit","source_key":"owner-b","request_id":"foreign-valid-resume","text":format!("继续 {original}")})).await.err().unwrap().contains("其他来源"));
}

#[tokio::test]
async fn local_effects_reuse_results_across_model_call_ids_but_file_reads_stay_fresh(){
    for (name,args,expected) in [
        ("homebase_terminal",json!({"command":"echo SYNTHETIC-ONLY"}),1),
        ("homebase_files",json!({"action":"read","path":"synthetic.txt"}),2),
    ]{
        let first=reply(vec![(name,args.clone())]);
        let mut second=reply(vec![(name,args)]);second.calls[0].id="different-model-call-id".into();
        let(e,models,executions,_,_root)=fixture(true,vec![first,second,reply(vec![])]).await;
        let ack=e.send(request("只使用合成执行器验证重复动作","local-effect-cache")).await.unwrap();
        wait(&e,&ack.turn_id,"completed").await;
        assert_eq!(executions.load(Ordering::SeqCst),expected,"{name}");assert_eq!(models.load(Ordering::SeqCst),3);
        assert_eq!(e.turn(&ack.turn_id).await.unwrap().steps.len(),2);
    }
}

#[tokio::test]
async fn conversation_global_slots_are_bounded_and_waiting_requests_start_after_a_slot_releases(){
    let replies=(0..5).map(|_|reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"并发隔离","content":"未确认"}))])).collect();
    let(e,m,_,_,_root)=fixture(true,replies).await;
    let mut started=Vec::new();
    for n in 0..4{let turn=e.send(request("first",&format!("slot-{n}"))).await.unwrap();wait(&e,&turn.turn_id,"awaiting_confirmation").await;started.push(turn);}
    let queued=e.send(request("fifth","slot-fifth")).await.unwrap();assert_eq!(queued.state,"waiting_input_queue");assert_eq!(m.load(Ordering::SeqCst),4);
    e.cancel(&started[0].turn_id).await.unwrap();wait(&e,&queued.turn_id,"awaiting_confirmation").await;assert_eq!(m.load(Ordering::SeqCst),5);
    for turn in started.iter().skip(1){e.cancel(&turn.turn_id).await.unwrap();}e.cancel(&queued.turn_id).await.unwrap();
}

#[tokio::test]
async fn conversation_empty_paused_queue_does_not_trap_a_new_independent_request(){
    let(e,_,_,saved,_root)=fixture(true,vec![reply(vec![])]).await;let session=e.create_session().await.unwrap();
    let mut state=saved.lock().await.clone();state.sessions[0].queue_paused=true;
    let restored=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    let mut input=request("fresh","fresh-after-cleared-queue");input.session_id=Some(session.id.clone());
    let ack=restored.send(input).await.unwrap();wait(&restored,&ack.turn_id,"completed").await;
    assert!(!restored.sessions().await[0].queue_paused);
}

#[tokio::test]
async fn personal_optional_input_errors_never_silently_remove_expiry_or_repeat_settings(){
    for (name,args) in [
        ("homebase_memory",json!({"action":"remember","revision":0,"title":"合成参数","content":"测试","expires_at":123})),
        ("homebase_reminder",json!({"action":"create","revision":0,"text":"合成参数","due_at":"2026-12-12T10:00:00+08:00","interval_minutes":-1}))
    ] {
        let(e,m,c,_,_root)=fixture(true,vec![reply(vec![(name,args)])]).await;let ack=e.send(request("invalid optional input",&format!("invalid-{name}"))).await.unwrap();
        let pending=wait(&e,&ack.turn_id,"awaiting_confirmation").await;
        e.confirm(&ack.turn_id,&pending.pending.unwrap().id,true).await.unwrap();let failed=wait(&e,&ack.turn_id,"failed").await;assert!(failed.text.contains("必须"));
        let personal=e.personal().await;assert!(personal["memories"].as_array().unwrap().is_empty());assert!(personal["reminders"].as_array().unwrap().is_empty());
        assert_eq!(m.load(Ordering::SeqCst),1);assert_eq!(c.load(Ordering::SeqCst),0);
    }
}

#[tokio::test]
async fn assistant_reminder_claims_are_channel_scoped_and_unknown_is_not_resent() {
    let (e, models, executions, saved, _root) = fixture(false, vec![]).await;
    let request = crate::assistant_personal::ReminderInput {
        revision: 0, id: None, text: "隔离到期提醒".into(),
        due_at: "2026-01-01T00:00:00Z".into(), interval_minutes: None, enabled: true,
    };
    let reminder = e.reminder_save(request, "wechat", "owner-a").await.unwrap();
    let read = |source: &str| json!({"source_key":source,"operation":"notifications"});
    assert_eq!(e.channel(read("owner-b")).await.unwrap(), json!([]));
    let first = e.channel(read("owner-a")).await.unwrap();
    assert_eq!(first.as_array().unwrap().len(), 1);
    assert_eq!(e.channel(read("owner-a")).await.unwrap(), json!([]));
    assert!(e.channel(json!({"source_key":"owner-b","operation":"notification_ack","reminder_id":reminder["id"],"delivery":"accepted"})).await.is_err());
    let release = |source: &str, client: Value| json!({
        "source_key":source,"operation":"notification_ack","reminder_id":reminder["id"],
        "client_id":client,"delivery":"not_sent"
    });
    assert!(e.channel(release("owner-b", first[0]["client_id"].clone())).await.is_err());
    assert!(e.channel(release("owner-a", Value::Null)).await.is_err());
    assert!(e.channel(release("owner-a", json!("previous-occurrence"))).await.is_err());
    e.channel(release("owner-a", first[0]["client_id"].clone())).await.unwrap();
    let persisted = saved.lock().await.clone();
    let e = Engine::new(persisted, e.save.clone(), e.model.clone(), e.control.clone(), e.workspace.clone()).await.unwrap();
    assert_eq!(e.channel(read("owner-a")).await.unwrap(), first);
    e.channel(json!({"source_key":"owner-a","operation":"notification_ack","reminder_id":reminder["id"],"delivery":"unknown"})).await.unwrap();
    assert!(e.channel(release("owner-a", first[0]["client_id"].clone())).await.is_err());
    assert_eq!(e.channel(read("owner-a")).await.unwrap(), json!([]));
    assert_eq!(e.personal().await["reminders"][0]["delivery"], "unknown");
    assert!(e.personal().await["reminders"][0].get("source_key").is_none());
    assert_eq!(models.load(Ordering::SeqCst),0);
    assert_eq!(executions.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn assistant_reminder_storage_failure_is_visible_and_does_not_claim_delivery() {
    let (base, models, executions, _, _root)=fixture(false,vec![]).await;
    let save:Save=Arc::new(|state|Box::pin(async move {
        if state.personal.reminders.iter().any(|r|r.delivery=="desktop_due") {
            Err("合成存储失败".into())
        } else {Ok(())}
    }));
    let engine=Engine::new(State::default(),save,base.model.clone(),base.control.clone(),base.workspace.clone()).await.unwrap();
    engine.reminder_save(crate::assistant_personal::ReminderInput{revision:0,id:None,text:"隔离故障提醒".into(),due_at:"2026-01-01T00:00:00Z".into(),interval_minutes:None,enabled:true},"desktop","desktop").await.unwrap();
    tokio::time::timeout(Duration::from_secs(3),async{loop{
        let state=engine.personal().await;
        if state["warning"].as_str().is_some_and(|s|s.contains("合成存储失败")) {
            assert_eq!(state["reminders"][0]["delivery"],"scheduled");break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }}).await.unwrap();
    assert_eq!(models.load(Ordering::SeqCst),0);assert_eq!(executions.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn assistant_resume_refuses_uncertain_model_or_incompatible_runtime() {
    let (e, models, _, saved, _root) = fixture(true, vec![]).await;
    let mut state = saved.lock().await.clone();
    state.settings.enabled = false;
    let isolated = Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    let ack=isolated.send(request("合成恢复边界","resume-boundaries")).await.unwrap();
    let mut state = saved.lock().await.clone();
    state.settings.enabled=true;
    let(s,t)=locate(&state,&ack.turn_id).unwrap();
    let turn=&mut state.sessions[s].turns[t];
    turn.state="interrupted_unknown".into();turn.settings.runtime="pi".into();
    turn.runtime_operations.push(json!({"synthetic":true}));turn.runtime_uncertain_model=true;
    let uncertain=Engine::new(state.clone(),e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    assert!(uncertain.resume(&ack.turn_id).await.err().unwrap().contains("未知"));
    state.sessions[s].turns[t].runtime_uncertain_model=false;
    state.sessions[s].turns[t].runtime_version="incompatible-fixture".into();
    let incompatible=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    assert!(incompatible.resume(&ack.turn_id).await.err().unwrap().contains("不兼容"));
    assert_eq!(models.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn assistant_pi_real_worker_uses_existing_controller_and_saves_checkpoints() {
    let(e,models,executions,saved,_root)=fixture(true,vec![
        reply(vec![("homebase_call",json!({"command":"system_gpu","args":{}}))]),reply(vec![])
    ]).await;
    let mut settings=e.settings().await;
    settings.runtime="pi".into();
    e.save_settings(settings).await.unwrap();
    let ack=e.send(request("隔离查询 GPU","deep-worker-fixture")).await.unwrap();
    let turn=tokio::time::timeout(Duration::from_secs(40),async {
        loop { let turn=e.turn(&ack.turn_id).await.unwrap();if terminal(&turn.state){break turn;}tokio::time::sleep(Duration::from_millis(30)).await; }
    }).await.unwrap();
    assert_eq!(turn.state,"completed","{}",turn.text);
    assert_eq!(models.load(Ordering::SeqCst),2);
    assert_eq!(executions.load(Ordering::SeqCst),1);
    assert_eq!(turn.jobs.len(),1);
    assert_eq!(turn.jobs[0].result["gpu_status"],"not_detected");
    let state=saved.lock().await;
    let(s,t)=locate(&state,&ack.turn_id).unwrap();
    assert!(!state.sessions[s].turns[t].runtime_operations.is_empty());
    assert_eq!(state.sessions[s].turns[t].runtime_results.len(),1);
    assert!(!state.sessions[s].turns[t].runtime_uncertain_model);
}

#[tokio::test]
async fn assistant_pi_memory_write_waits_for_human_and_persists_only_after_approval(){
    let(e,models,executions,_,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"风格","content":"合成偏好"}))]),reply(vec![])]).await;
    let mut settings=e.settings().await;settings.runtime="pi".into();e.save_settings(settings).await.unwrap();
    let ack=e.send(request("记住合成偏好","deep-memory-confirm")).await.unwrap();
    let pending=tokio::time::timeout(Duration::from_secs(30),async{loop{let t=e.turn(&ack.turn_id).await.unwrap();if t.state=="awaiting_confirmation"{break t;}assert!(!terminal(&t.state),"{}",t.text);tokio::time::sleep(Duration::from_millis(30)).await;}}).await.unwrap();
    assert_eq!(e.personal().await["memories"].as_array().unwrap().len(),0);
    e.confirm(&ack.turn_id,&pending.pending.unwrap().id,true).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30),async{loop{let t=e.turn(&ack.turn_id).await.unwrap();if terminal(&t.state){assert_eq!(t.state,"completed","{}",t.text);break;}tokio::time::sleep(Duration::from_millis(30)).await;}}).await.unwrap();
    assert_eq!(e.personal().await["memories"][0]["content"],"合成偏好");
    assert_eq!(models.load(Ordering::SeqCst),2);assert_eq!(executions.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn assistant_pi_marks_active_worker_running_before_model_response(){
    let(e,_,executions,_,_root)=fixture(true,vec![reply(vec![])]).await;
    let mut settings=e.settings().await;settings.runtime="pi".into();e.save_settings(settings).await.unwrap();
    let ack=e.send(request("合成活动状态检查","deep-running-state")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10),async{loop{
        let t=e.turn(&ack.turn_id).await.unwrap();
        if t.state=="running" {break;}
        assert!(!terminal(&t.state),"worker never exposed a running state");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.unwrap();
    e.cancel(&ack.turn_id).await.unwrap();
    wait(&e,&ack.turn_id,"canceled").await;
    assert_eq!(executions.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn assistant_pi_disable_during_approval_finishes_without_saving_memory(){
    let(e,models,executions,_,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"未批准","content":"不能写入"}))])]).await;
    let mut settings=e.settings().await;settings.runtime="pi".into();e.save_settings(settings).await.unwrap();
    let ack=e.send(request("隔离停用验证","deep-disable-confirm")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30),async{loop{let t=e.turn(&ack.turn_id).await.unwrap();if t.state=="awaiting_confirmation"{break;}assert!(!terminal(&t.state),"{}",t.text);tokio::time::sleep(Duration::from_millis(30)).await;}}).await.unwrap();
    let mut settings=e.settings().await;settings.enabled=false;e.save_settings(settings).await.unwrap();
    wait(&e,&ack.turn_id,"canceled").await;
    assert!(e.personal().await["memories"].as_array().unwrap().is_empty());
    assert_eq!(models.load(Ordering::SeqCst),1);assert_eq!(executions.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn assistant_pi_recovery_after_final_checkpoint_never_repeats_model_or_job(){
    let(e,models,executions,saved,_root)=fixture(true,vec![reply(vec![("homebase_call",json!({"command":"system_gpu","args":{}}))]),reply(vec![])]).await;
    let mut settings=e.settings().await;settings.runtime="pi".into();e.save_settings(settings).await.unwrap();
    let ack=e.send(request("隔离恢复验证","deep-recovery")).await.unwrap();wait(&e,&ack.turn_id,"completed").await;
    let mut state=saved.lock().await.clone();
    let(s,t)=locate(&state,&ack.turn_id).unwrap();
    state.sessions[s].turns[t].state="running".into();
    state.sessions[s].turns[t].history_committed=false;
    state.sessions[s].turns[t].finished_at=None;
    state.sessions[s].history.clear();
    let restored=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    wait(&restored,&ack.turn_id,"completed").await;
    assert_eq!(models.load(Ordering::SeqCst),2);assert_eq!(executions.load(Ordering::SeqCst),1);
    assert_eq!(restored.turn(&ack.turn_id).await.unwrap().jobs.len(),1);
}

#[tokio::test]
async fn assistant_pi_reused_tool_id_with_different_action_fails_before_second_write(){
    let(e,_,executions,_,_root)=fixture(true,vec![reply(vec![("homebase_call",json!({"command":"system_gpu","args":{}}))]),reply(vec![("homebase_call",json!({"command":"system_info","args":{}}))])]).await;
    let mut settings=e.settings().await;settings.runtime="pi".into();e.save_settings(settings).await.unwrap();
    let ack=e.send(request("合成编号碰撞验证","deep-collision")).await.unwrap();
    wait(&e,&ack.turn_id,"failed").await;
    assert!(e.turn(&ack.turn_id).await.unwrap().text.contains("编号"));assert_eq!(executions.load(Ordering::SeqCst),1);
}

#[tokio::test]
async fn retired_assistant_session_keeps_results_and_refuses_new_execution() {
    let (base, models, executions, saved, _root) = fixture(false, vec![]).await;
    let original = base.send(request("合成旧会话", "archive-original")).await.unwrap();
    let mut state = saved.lock().await.clone();
    state.settings.runtime = "pi".into();
    let (s, t) = locate(&state, &original.turn_id).unwrap();
    let old = &mut state.sessions[s].turns[t];
    old.settings.runtime = "deepagents".into();
    old.state = "failed".into();
    old.text = "合成原结果".into();
    old.runtime_operations.push(json!({"kind":"put","opaque":"archived"}));
    let app = Engine::new(state, base.save.clone(), base.model.clone(), base.control.clone(), base.workspace.clone()).await.unwrap();
    let old = app.turn(&original.turn_id).await.unwrap();
    assert_eq!(old.text, "合成原结果");
    assert_eq!(old.runtime, "deepagents");
    assert!(!old.resumable);
    assert!(app.resume(&original.turn_id).await.is_err());
    let mut next = request("合成续聊", "archive-next");
    next.session_id = Some(original.session_id);
    assert!(app.send(next).await.err().unwrap().contains("已留档"));
    let fresh = app.send(request("合成新会话", "archive-fresh")).await.unwrap();
    assert_eq!(app.turn(&fresh.turn_id).await.unwrap().runtime, "pi");
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
async fn fixture(
    enabled: bool,
    replies: Vec<ModelReply>,
) -> (
    Arc<Engine>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<Mutex<State>>,
    tempfile::TempDir,
) {
    let root = tempfile::tempdir().unwrap();
    let models = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(AtomicUsize::new(0));
    let modelcount = models.clone();
    let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
    let model: Model = Arc::new(move |_| {
        let queue = queue.clone();
        let count = modelcount.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            queue
                .lock()
                .await
                .pop_front()
                .ok_or("fixture exhausted".into())
        })
    });
    let count = executions.clone();
    let runner: Runner = Arc::new(move |command, _| {
        let count = count.clone();
        Box::pin(async move {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(
                json!({"status":"succeeded","command":command,"gpu_status":"not_detected","gpu_message":"未检测到 GPU"}),
            )
        })
    });
    let read_runner=runner.clone();
    let controller = Controller::new(catalog().clone(), root.path(), runner).unwrap();
    let control: Control = Arc::new(move |r| {
        let c = controller.clone();
        let read_runner=read_runner.clone();
        Box::pin(async move {if r.method=="assistant.local_read" {
            assert!(crate::assistant_local::readonly_operation(r.args["operation"].as_str().unwrap(),&r.args["request"]));
            return read_runner("assistant_local_execute".into(),r.args.as_object().unwrap().clone()).await;
        }c.handle(r).await })
    });
    let mut state = State::default();
    state.settings.enabled = enabled;
    state.settings.route_id = "synthetic-only".into();
    let memory = Arc::new(Mutex::new(state.clone()));
    let saved = memory.clone();
    let save: Save = Arc::new(move |s| {
        let saved = saved.clone();
        Box::pin(async move {
            *saved.lock().await = s;
            Ok(())
        })
    });
    let engine = Engine::new(state, save, model, control, root.path().join("workspace"))
        .await
        .unwrap();
    (engine, models, executions, memory, root)
}

#[tokio::test]
async fn assistant_desktop_image_reopens_original_request_without_resending_pixels() {
    use base64::Engine as _;
    use crate::assistant_desktop::{DesktopBody, DesktopStore};

    let (base, _, executions, saved, root) = fixture(true, vec![]).await;
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.add_text_chunk("Comment".into(), "isolated-original-metadata".into()).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[17, 42, 93]).unwrap();
        writer.finish().unwrap();
    }
    let base64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let reference = base.stage_image("new".into(), "fixture.png".into(), &base64).await.unwrap();
    let again = base.stage_image("new".into(), "fixture.png".into(), &base64).await.unwrap();
    assert!(again == reference);
    assert!(base.stage_image("new".into(), "../fixture.png".into(), &base64).await.is_err());
    let scope = uuid::Uuid::new_v4().to_string();
    assert!(base.stage_image(scope.clone(), "fixture.png".into(), &base64).await.is_err());
    let files = std::fs::read_dir(base.documents.image_directory()).unwrap()
        .map(|entry| entry.unwrap().path()).filter(|path| path.extension().is_some_and(|ext| ext == "dpapi"))
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    let cipher = std::fs::read(&files[0]).unwrap();
    assert!(!cipher.starts_with(&png[..8]));

    let directory = root.path().join("drafts");
    let store = DesktopStore::new(directory.clone());
    let mut original = request("这张图是什么颜色？", "synthetic-desktop-png");
    original.route_id = Some("openrouter::fixture-vision:free".into());
    original.mode = Some("chat".into());
    original.image = Some(reference.clone());
    let draft = store.save("new", 0, DesktopBody {
        text: original.text.clone(), route: original.route_id.clone().unwrap(),
        mode: "chat".into(), files: vec![], image: original.image.clone(),
    }).await.unwrap();
    assert!(store.index().unwrap().entries.iter().any(|entry| entry.scope == "new" && entry.has_draft));
    assert!(store.save(&scope, 0, draft.body.clone()).await.is_err());
    let prepared = store.prepare("new", draft.revision, original.clone()).await.unwrap();
    drop(store);

    let observed = Arc::new(Mutex::new(Vec::<ModelRequest>::new()));
    let observed_by_model = observed.clone();
    let model: Model = Arc::new(move |input| {
        let observed = observed_by_model.clone();
        Box::pin(async move {
            observed.lock().await.push(input);
            Ok(reply(vec![]))
        })
    });
    let snapshot = saved.lock().await.clone();
    let engine = Engine::new(snapshot, base.save.clone(), model.clone(), base.control.clone(), base.workspace.clone()).await.unwrap();
    let store = DesktopStore::new(directory.clone());
    let unresolved = store.recover(&engine, "new").await;
    assert_eq!(unresolved.status, "not_found");
    assert!(unresolved.entry.unwrap().body.image == original.image);
    assert!(observed.lock().await.is_empty());
    let mut changed = original.clone();
    changed.image.as_mut().unwrap().sha256 = "a".repeat(64);
    assert_eq!(store.transmit(&engine, prepared.revision, changed).await.status, "failed");
    let accepted = store.transmit(&engine, prepared.revision, original.clone()).await;
    let ack = accepted.ack.expect("original image request must be accepted");
    wait(&engine, &ack.turn_id, "completed").await;
    assert!(accepted.entry.unwrap().body.image.is_none());

    let snapshot = saved.lock().await.clone();
    let restored = Engine::new(snapshot, base.save.clone(), model, base.control.clone(), base.workspace.clone()).await.unwrap();
    let store = DesktopStore::new(directory);
    let same = store.transmit(&restored, prepared.revision, original).await.ack.unwrap();
    assert_eq!(same.turn_id, ack.turn_id);
    assert_eq!(observed.lock().await.len(), 1);
    let mut followup = request("普通文字讨论", "synthetic-text-followup");
    followup.session_id = Some(ack.session_id);
    followup.mode = Some("chat".into());
    let next = restored.send(followup).await.unwrap();
    wait(&restored, &next.turn_id, "completed").await;
    let observed = observed.lock().await;
    assert_eq!(observed.len(), 2);
    let first = observed[0].vision.as_ref().unwrap();
    assert_eq!(first.prompt, "这张图是什么颜色？");
    assert_eq!(observed[0].route_id, "openrouter::fixture-vision:free");
    let clean = base64::engine::general_purpose::STANDARD.decode(&first.png_base64).unwrap();
    assert_ne!(clean, png);
    let mut reader = png::Decoder::new(std::io::Cursor::new(clean)).read_info().unwrap();
    let mut pixels = [0; 3];
    reader.next_frame(&mut pixels).unwrap();
    assert_eq!(pixels, [17, 42, 93]);
    assert!(observed[1].vision.is_none());
    assert!(!serde_json::to_string(&observed[1].messages).unwrap().contains(&first.png_base64));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn assistant_desktop_image_restart_distinguishes_ready_from_attempted_checkpoint() {
    let (seed, _, _, initial, _root) = fixture(true, vec![]).await;
    let versions = Arc::new(Mutex::new(Vec::new()));
    let history = versions.clone();
    let save: Save = Arc::new(move |state| {
        let history = history.clone();
        Box::pin(async move {
            history.lock().await.push(state);
            Ok(())
        })
    });
    let observed = Arc::new(AtomicUsize::new(0));
    let capture = observed.clone();
    let model: Model = Arc::new(move |request| {
        assert!(request.vision.is_some(), "a visual turn must never degrade to text");
        capture.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(reply(vec![])) })
    });
    let init = initial.lock().await.clone();
    let engine = Engine::new(init, save.clone(), model.clone(), seed.control.clone(), seed.workspace.clone()).await.unwrap();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGMQVDIGAACuAGcVHqFfAAAAAElFTkSuQmCC";
    let image = engine.stage_image("new".into(), "restart.png".into(), png).await.unwrap();
    let mut original = request("复核合成图片", "visual-checkpoint-case");
    original.mode = Some("chat".into());
    original.route_id = Some("openrouter::fixture-vision:free".into());
    original.image = Some(image.clone());
    let mut reserved = original.clone();
    reserved.request_id = "hb-followup-only-for-scheduler".into();
    assert!(engine.send(reserved).await.is_err());
    let ack = engine.send(original.clone()).await.unwrap();
    wait(&engine, &ack.turn_id, "completed").await;
    assert_eq!(observed.load(Ordering::SeqCst), 1);

    let states = versions.lock().await.clone();
    let snapshot = |phase: &str, attempted: bool| {
        states.iter().find(|state| {
            let Ok((s,t)) = locate(state, &ack.turn_id) else { return false };
            let turn = &state.sessions[s].turns[t];
            turn.state == phase && turn.vision_checkpoint().unwrap().unwrap().attempted == attempted
        }).unwrap().clone()
    };
    let ready = snapshot("queued", false);
    let in_flight = snapshot("running", true);
    for value in [&ready, &in_flight] {
        let (s,t) = locate(value, &ack.turn_id).unwrap();
        let turn = &value.sessions[s].turns[t];
        assert!(turn.runtime_uncertain_model);
        let raw = serde_json::to_value(turn).unwrap();
        assert!(raw.get("image").is_none());
        assert!(archived_image(&raw).unwrap().unwrap() == image);
    }
    let restored = Engine::new(ready, save.clone(), model.clone(), seed.control.clone(), seed.workspace.clone()).await.unwrap();
    wait(&restored, &ack.turn_id, "completed").await;
    assert_eq!(observed.load(Ordering::SeqCst), 2);
    assert_eq!(restored.desktop_receipt(&original).await.unwrap().unwrap().turn_id, ack.turn_id);

    let stopped = Engine::new(in_flight, save, model, seed.control.clone(), seed.workspace.clone()).await.unwrap();
    let previous = stopped.desktop_receipt(&original).await.unwrap().unwrap();
    assert_eq!(previous.turn_id, ack.turn_id);
    assert_eq!(previous.state, "interrupted_unknown");
    assert_eq!(stopped.send(original).await.unwrap().turn_id, ack.turn_id);
    assert_eq!(observed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn followup_delivery_reuses_owned_original_job_across_restart_and_stays_in_its_session() {
    let(e,_,executions,saved,_root)=fixture(true,vec![reply(vec![]),reply(vec![("homebase_deliver",json!({"job_id":"original-video"}))]),reply(vec![]),reply(vec![("homebase_deliver",json!({"job_id":"original-video"}))])]).await;
    let original=e.channel(json!({"operation":"submit","source_key":"owner-a","text":"合成原视频","request_id":"delivery-original"})).await.unwrap();
    let tid=original["turn_id"].as_str().unwrap();wait(&e,tid,"completed").await;
    e.change_turn(tid,|session,_|{
        session.jobs.push(Job{id:"original-video".into(),command:"capability_render".into(),turn_id:tid.into(),state:"succeeded".into(),result:json!({"status":"succeeded"}),owned:true,deliver:false,remote:None,archived_result_bytes:None,
          artifacts:vec![Artifact{id:"fixture-file".into(),job_id:"original-video".into(),name:"fixture.mp4".into(),path:"X:/isolated/fixture.mp4".into(),bytes:10,sha256:"a".repeat(64),mime_type:"video/mp4".into()}]});Ok(())
    }).await.unwrap();
    let followup=e.channel(json!({"operation":"submit","source_key":"owner-a","text":"再发刚才的视频，不要重新生成","request_id":"delivery-followup"})).await.unwrap();
    let next=followup["turn_id"].as_str().unwrap();let result=wait(&e,next,"completed").await;
    assert_eq!(result.jobs.len(),1);assert_eq!(result.jobs[0].id,"original-video");assert_eq!(result.artifacts.len(),1);
    let state=saved.lock().await.clone();let restored=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    let delivered=restored.channel(json!({"operation":"get","source_key":"owner-a","turn_id":next})).await.unwrap();
    assert_eq!(delivered["result"]["artifacts"][0]["sha256"],"a".repeat(64));
    assert!(restored.channel(json!({"operation":"get","source_key":"owner-b","turn_id":next})).await.is_err());
    let foreign=restored.channel(json!({"operation":"submit","source_key":"owner-b","text":"合成越界获取","request_id":"delivery-foreign"})).await.unwrap();
    assert!(wait(&restored,foreign["turn_id"].as_str().unwrap(),"failed").await.artifacts.is_empty());
    assert_eq!(executions.load(Ordering::SeqCst),0);
}
async fn wait(engine: &Engine, tid: &str, wanted: &str) -> TurnView {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let t = engine.turn(tid).await.unwrap();
            if t.state == wanted {
                return t;
            }
            if terminal(&t.state) {
                panic!("unexpected state {}: {}", t.state, t.text);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
fn request(text: &str, id: &str) -> SendRequest {
    SendRequest {
        image: None,
        route_id: None,
        attachments: vec![],
        // These fixtures exercise execution/FIFO; auto discussion has separate regressions.
        mode: Some("task".into()),
        session_id: None,
        text: text.into(),
        request_id: id.into(),
    }
}

#[tokio::test]
async fn desktop_receipt_lookup_never_submits_or_resumes_and_rejects_changed_input() {
    let (engine, models, controller, _, _root) = fixture(false, vec![]).await;
    let req = request("只查询原请求", "desktop-receipt-proof");
    assert!(engine.desktop_receipt(&req).await.unwrap().is_none());
    assert!(engine.sessions().await.is_empty());
    let original = engine.send(req.clone()).await.unwrap();
    let recovered = engine.desktop_receipt(&req).await.unwrap().unwrap();
    assert_eq!(recovered.turn_id, original.turn_id);
    assert_eq!(recovered.session_id, original.session_id);
    let mut changed = req; changed.route_id = Some("different-route".into());
    assert!(engine.desktop_receipt(&changed).await.is_err());
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(controller.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn desktop_durable_send_guards_writes_and_recovers_lost_ack_without_replay() {
    let (engine, models, controller, _, _root) = fixture(false, vec![]).await;
    let root = tempfile::tempdir().unwrap();
    let store = crate::assistant_desktop::DesktopStore::new(root.path().into());
    let req = request("受保护的隔离发送", "desktop-durable-original");
    store.fail_writes(true);
    let blocked = store.transmit(&engine, 0, req.clone()).await;
    assert_eq!(blocked.code.as_deref(), Some("DRAFT_PREPARE_FAILED"));
    assert!(engine.sessions().await.is_empty());
    store.fail_writes(false);
    let prepared = store.prepare("new", 0, req.clone()).await.unwrap();
    let missing = store.recover(&engine, "new").await;
    assert_eq!(missing.status, "not_found");
    assert_eq!(store.get("new").unwrap().revision, prepared.revision);
    assert!(engine.sessions().await.is_empty());
    let ack = engine.send(req.clone()).await.unwrap();
    store.fail_writes(true);
    let local_failure = store.recover(&engine, "new").await;
    assert_eq!(local_failure.status, "partial");
    assert_eq!(local_failure.ack.unwrap().turn_id, ack.turn_id);
    assert!(store.get("new").unwrap().pending.is_some());
    store.fail_writes(false);
    let recovered = store.recover(&engine, "new").await;
    assert_eq!(recovered.ack.unwrap().turn_id, ack.turn_id);
    assert!(store.get("new").unwrap().pending.is_none());
    let repeated = store.transmit(&engine, 0, req).await;
    assert_eq!(repeated.ack.unwrap().turn_id, ack.turn_id);
    assert_eq!(engine.sessions().await.len(), 1);
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(controller.load(Ordering::SeqCst), 0);
    assert!(!allowed("assistant_desktop_send"));
}

#[tokio::test]
async fn chat_model_override_and_text_attachments_are_scoped_and_idempotent() {
    let (e, _, _, saved, _root) = fixture(true, vec![]).await;
    let mut state = saved.lock().await.clone();
    state.settings.route_id = "openrouter::fixture-default:free".into();
    state.settings.free_only = true;
    let defaults = serde_json::to_value(&state.settings).unwrap();
    let seen = Arc::new(Mutex::new(Vec::<ModelRequest>::new()));
    let captured = seen.clone();
    let model: Model = Arc::new(move |req| {
        let captured = captured.clone();
        Box::pin(async move { captured.lock().await.push(req); Ok(reply(vec![])) })
    });
    let engine = Engine::new(state, e.save.clone(), model, e.control.clone(), e.workspace.clone()).await.unwrap();
    let mut req = request("请总结附件", "chat-files-model-1");
    req.route_id = Some("openrouter::fixture-other:free".into());
    req.attachments = vec![TextAttachment { name: "隔离.md".into(), content: "只在隔离数据内验证。".into(), document:None }];
    let ack = engine.send(req.clone()).await.unwrap();
    wait(&engine, &ack.turn_id, "completed").await;
    assert_eq!(engine.send(req.clone()).await.unwrap().turn_id, ack.turn_id);
    assert_eq!(seen.lock().await.len(), 1);
    let requests = seen.lock().await;
    assert_eq!(requests[0].route_id, "openrouter::fixture-other:free");
    assert!(requests[0].free_only);
    assert!(serde_json::to_string(&requests[0].messages).unwrap().contains("只在隔离数据内验证"));
    drop(requests);
    let view = engine.session(&ack.session_id).await.unwrap();
    assert_eq!(view.messages[0].content, "请总结附件");
    assert_eq!(view.messages[0].attachments[0].name, "隔离.md");
    assert_eq!(serde_json::to_value(engine.settings().await).unwrap(), defaults);
    req.attachments[0].content.push_str("changed");
    assert!(engine.send(req.clone()).await.is_err());
    req.attachments[0].content = "只在隔离数据内验证。".into();
    req.route_id = Some("openrouter::different:free".into());
    assert!(engine.send(req).await.is_err());
    let mut paid = request("不能产生费用", "reject-paid-override");
    paid.route_id = Some("openai::paid-model".into());
    assert!(engine.send(paid).await.is_err());
    assert_eq!(seen.lock().await.len(), 1);
    let wx = engine.channel(json!({"operation":"submit","source_key":"isolated-chat-ui","text":"模拟微信默认模型查询","request_id":"wx-chat-ui-default"})).await.unwrap();
    wait(&engine, wx["turn_id"].as_str().unwrap(), "completed").await;
    assert_eq!(seen.lock().await.last().unwrap().route_id, "openrouter::fixture-default:free");
    assert_eq!(serde_json::to_value(engine.settings().await).unwrap(), defaults);
}

#[tokio::test]
async fn assistant_preserves_source_and_free_policy_in_each_model_request(){
    let(e,_,_,saved,_root)=fixture(true,vec![]).await;let mut state=saved.lock().await.clone();state.settings.credential_source=crate::model_capabilities::CredentialSource::ApiKeyManager;state.settings.free_only=true;
    let model:Model=Arc::new(|r|Box::pin(async move{assert_eq!(r.credential_source,crate::model_capabilities::CredentialSource::ApiKeyManager);assert!(r.free_only);Ok(reply(vec![]))}));
    let new=Engine::new(state,e.save.clone(),model,e.control.clone(),e.workspace.clone()).await.unwrap();let ack=new.send(request("synthetic","policy-snapshot")).await.unwrap();wait(&new,&ack.turn_id,"completed").await;
    let legacy:Settings=serde_json::from_value(json!({"revision":0,"enabled":false,"route_id":"","wechat_enabled":true,"max_rounds":8,"max_tokens":2048})).unwrap();assert_eq!(legacy.credential_source,Default::default());assert!(!legacy.free_only);
}

#[tokio::test]
async fn assistant_disabled_never_contacts_model_or_controller() {
    let (e, m, c, _, _root) = fixture(false, vec![]).await;
    let ack = e.send(request("查询系统", "disabled-1")).await.unwrap();
    let t = e.turn(&ack.turn_id).await.unwrap();
    assert_eq!(t.state, "disabled");
    assert!(t.text.contains("尚未启用"));
    assert_eq!(m.load(Ordering::SeqCst), 0);
    assert_eq!(c.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn assistant_real_controller_tool_loop_and_idempotent_submission() {
    let (e, m, c, _, _root) = fixture(
        true,
        vec![
            reply(vec![("homebase_discover", json!({"module":"system"}))]),
            reply(vec![(
                "homebase_describe",
                json!({"command":"system_info"}),
            )]),
            reply(vec![(
                "homebase_call",
                json!({"command":"system_info","args":{}}),
            )]),
            reply(vec![]),
        ],
    )
    .await;
    let (a, b) = tokio::join!(
        e.send(request("查询系统", "same-1")),
        e.send(request("查询系统", "same-1"))
    );
    let a = a.unwrap();
    assert_eq!(a.turn_id, b.unwrap().turn_id);
    let t = wait(&e, &a.turn_id, "completed").await;
    assert_eq!(t.steps.len(), 3);
    assert_eq!(t.jobs.len(), 1);
    assert_eq!(t.jobs[0].state, "succeeded");
    assert_eq!(c.load(Ordering::SeqCst), 1);
    assert_eq!(m.load(Ordering::SeqCst), 4);
    assert!(e.send(request("different", "same-1")).await.is_err());
}
#[tokio::test]
async fn assistant_confirmation_is_human_scoped_and_exactly_once() {
    let(e,_,c,_,_root)=fixture(true,vec![reply(vec![("homebase_call",json!({"command":"agents_set_model","args":{"id":"synthetic-agent","model":"fixture"}}))]),reply(vec![])]).await;
    let ack = e.send(request("改变模型", "approve-1")).await.unwrap();
    let t = wait(&e, &ack.turn_id, "awaiting_confirmation").await;
    assert_eq!(c.load(Ordering::SeqCst), 0);
    assert!(e.confirm(&t.id, "forged", true).await.is_err());
    let token = t.pending.unwrap().id;
    let (a, b) = tokio::join!(
        e.confirm(&t.id, &token, true),
        e.confirm(&t.id, &token, true)
    );
    assert!(a.is_ok() ^ b.is_ok());
    wait(&e, &t.id, "completed").await;
    assert_eq!(c.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn assistant_rejection_cancel_and_restart_do_not_replay() {
    let (e, m, c, saved, _root) = fixture(
        true,
        vec![reply(vec![(
            "homebase_call",
            json!({"command":"agents_set_model","args":{"id":"fixture","model":"fixture"}}),
        )])],
    )
    .await;
    let a = e.send(request("更改设置", "reject-1")).await.unwrap();
    let t = wait(&e, &a.turn_id, "awaiting_confirmation").await;
    let snapshot = saved.lock().await.clone();
    e.confirm(&t.id, &t.pending.unwrap().id, false)
        .await
        .unwrap();
    assert_eq!(e.turn(&t.id).await.unwrap().state, "rejected");
    assert_eq!(c.load(Ordering::SeqCst), 0);
    let save: Save = Arc::new(|_| Box::pin(async { Ok(()) }));
    let model = e.model.clone();
    let control = e.control.clone();
    let restored = Engine::new(snapshot, save, model, control, e.workspace.clone())
        .await
        .unwrap();
    assert_eq!(
        restored.turn(&t.id).await.unwrap().state,
        "awaiting_confirmation"
    );
    restored.cancel(&t.id).await.unwrap();
    assert_eq!(m.load(Ordering::SeqCst), 1);
    assert_eq!(c.load(Ordering::SeqCst), 0);
    let mut state = saved.lock().await.clone();
    state.sessions[0].turns[0].state = "running".into();
    let restored = Engine::new(
        state,
        Arc::new(|_| Box::pin(async { Ok(()) })),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        restored.turn(&t.id).await.unwrap().state,
        "interrupted_unknown"
    );
    assert_eq!(c.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn assistant_model_cannot_call_authorization_or_credentials() {
    for command in [
        "conversation_native",
        "assistant_confirm",
        "secrets_reveal",
        "shared_secrets_consent_resolve",
        "wechat_forget",
    ] {
        let (e, _, c, _, _root) = fixture(
            true,
            vec![reply(vec![(
                "homebase_call",
                json!({"command":command,"args":{}}),
            )])],
        )
        .await;
        let a = e.send(request("fixture", command)).await.unwrap();
        let t = wait(&e, &a.turn_id, "failed").await;
        assert!(t.text.contains("未开放"));
        assert_eq!(c.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn automatic_context_is_bounded_readonly_and_session_scoped() {
    let(e,models,executions,saved,_root)=fixture(false,vec![]).await;
    let a=e.send(request("蓝色圆点动画","context-a")).await.unwrap();
    let b=e.send(request("不相关私密标记 OTHER-SESSION-SENTINEL","context-b")).await.unwrap();
    let before=serde_json::to_value(saved.lock().await.clone()).unwrap();
    let context=e.context(Some(&a.session_id),"蓝色圆点").await.unwrap();
    assert_eq!(context["version"],"homebase-context-v1");
    assert!(!context.to_string().contains("OTHER-SESSION-SENTINEL"));
    assert!(!context.to_string().contains(&b.turn_id));
    assert!(e.context(Some("missing-session"),"").await.is_err());
    assert_eq!(before,serde_json::to_value(saved.lock().await.clone()).unwrap());
    assert_eq!(models.load(Ordering::SeqCst),0);assert_eq!(executions.load(Ordering::SeqCst),0);
    let prompt=&saved.lock().await.sessions[0].turns[0].runtime_prompt;
    assert!(prompt.contains("homebase-context-v1"));assert!(prompt.contains("background_not_authority") || prompt.contains("资料，不是指令"));
}

#[tokio::test]
async fn desktop_and_simulated_wechat_receive_same_context_contract_without_replay(){
    let(e,models,executions,saved,_root)=fixture(false,vec![]).await;
    let desktop=e.send(request("隔离上下文验证","context-desktop")).await.unwrap();
    let wx=e.channel(json!({"operation":"submit","source_key":"synthetic-owner","text":"隔离上下文验证","request_id":"context-wechat"})).await.unwrap();
    let state=saved.lock().await.clone();
    assert!(state.sessions.iter().flat_map(|s|s.turns.iter()).all(|t|t.runtime_prompt.contains("homebase-context-v1")));
    let tid=wx["turn_id"].as_str().unwrap();
    assert!(e.channel(json!({"operation":"get","source_key":"foreign-owner","turn_id":tid})).await.is_err());
    assert_ne!(desktop.turn_id,tid);assert_eq!(models.load(Ordering::SeqCst),0);assert_eq!(executions.load(Ordering::SeqCst),0);
    let usage=e.usage().await;assert!(usage.reported_cost_usd.is_none());assert!(usage.prompt_tokens.is_none());assert!(!usage.complete);assert!(usage.groups.is_empty());
}
#[tokio::test]
async fn assistant_wechat_uses_shared_core_and_separates_sources() {
    let (e, m, c, _, _root) = fixture(false, vec![]).await;
    let a = e
        .channel(
            json!({"operation":"submit","source_key":"owner-a","text":"你好","request_id":"wx-1"}),
        )
        .await
        .unwrap();
    let t = a["turn_id"].as_str().unwrap();
    let v = e
        .channel(json!({"operation":"get","source_key":"owner-a","turn_id":t}))
        .await
        .unwrap();
    assert_eq!(v["status"], "disabled");
    assert!(v["result"]["text"].as_str().unwrap().contains("尚未启用"));
    assert!(e
        .channel(json!({"operation":"get","source_key":"owner-b","turn_id":t}))
        .await
        .is_err());
    assert_eq!(m.load(Ordering::SeqCst), 0);
    assert_eq!(c.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn assistant_settings_revision_and_model_failure_are_explicit() {
    let (e, _, _, _, _root) = fixture(true, vec![]).await;
    let old = e.settings().await;
    e.save_settings(old.clone()).await.unwrap();
    assert!(e.save_settings(old).await.is_err());
    let a = e.send(request("fixture", "empty-model")).await.unwrap();
    assert!(wait(&e, &a.turn_id, "failed")
        .await
        .text
        .contains("exhausted"));
}

#[tokio::test]
async fn assistant_cancel_after_acceptance_preserves_controller_job() {
    let (base, models, executions, saved, _root) = fixture(
        true,
        vec![
            reply(vec![(
                "homebase_call",
                json!({"command":"system_info","args":{}}),
            )]),
            reply(vec![]),
        ],
    )
    .await;
    let observed = Arc::new(Notify::new());
    let signal = observed.clone();
    let controller = base.control.clone();
    let control: Control = Arc::new(move |request| {
        let controller = controller.clone();
        let signal = signal.clone();
        Box::pin(async move {
            if request.method == "jobs.get" {
                signal.notify_one();
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            (controller)(request).await
        })
    });
    let state = saved.lock().await.clone();
    let e = Engine::new(
        state,
        base.save.clone(),
        base.model.clone(),
        control,
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let ack = e
        .send(request("查询系统后取消等待", "cancel-accepted"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), observed.notified())
        .await
        .unwrap();
    let turn = e.turn(&ack.turn_id).await.unwrap();
    assert_eq!(turn.jobs.len(), 1);
    let job = turn.jobs[0].id.clone();
    e.cancel(&ack.turn_id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(e.turn(&ack.turn_id).await.unwrap().state, "canceled");
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let actual = e
        .control("jobs.get", None, json!({}), "verify-original", Some(&job))
        .await
        .unwrap();
    assert_eq!(actual["status"], "succeeded");
    let view=e.channel(json!({"operation":"get","source_key":"desktop","turn_id":ack.turn_id,"include_files":true})).await.unwrap();
    assert_eq!(view["result"]["jobs"][0]["state"],"succeeded");
    assert_eq!(executions.load(Ordering::SeqCst),1);
    let restored = Engine::new(
        saved.lock().await.clone(),
        base.save.clone(),
        base.model.clone(),
        e.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(restored.turn(&ack.turn_id).await.unwrap().state, "canceled");
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}
