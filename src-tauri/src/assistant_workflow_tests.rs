//! Real graph and synthetic planner: no provider requests and no user files.
use super::*;
pub(super) async fn task_wait(engine:&Engine,tid:&str,wanted:&str)->TurnView{
    let deadline=tokio::time::Instant::now()+Duration::from_secs(90);
    loop{
        let turn=engine.turn(tid).await.unwrap();
        if turn.state==wanted{return turn;}
        assert!(!terminal(&turn.state),"unexpected task state {}: {}",turn.state,turn.text);
        assert!(tokio::time::Instant::now()<deadline,"task stuck in {} with {} steps",turn.state,turn.steps.len());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
pub(super) fn tool_reply(n:usize,name:&str,args:Value)->ModelReply{
    wire::parse("chat",&json!({"choices":[{"message":{"tool_calls":[{"id":format!("task-test-{n}"),"function":{"name":name,"arguments":args.to_string()}}]}}]})).unwrap()
}
fn plan(kind:&str)->Value{json!({"action":"set","revision":0,"goal":"按顺序检查合成对象","items":[{"id":"inspect","title":"核对合成输入","done_when":"实际读取并检查所有指定对象","kind":kind}]})}
fn last_step(request:&ModelRequest)->String{
    let value=request.messages.iter().rev().find(|m|m["role"]=="tool").unwrap();
    serde_json::from_str::<Value>(value["content"].as_str().unwrap()).unwrap()["step_id"].as_str().unwrap().into()
}
async fn custom(free:bool,model:Model)->(Arc<Engine>,Arc<AtomicUsize>,tempfile::TempDir){
    let(e,_,effects,_,root)=fixture(false,vec![]).await;
    let mut state=State::default();state.settings.enabled=true;state.settings.free_only=free;state.settings.runtime="pi".into();state.settings.route_id="synthetic-only".into();
    let engine=Engine::new(state,e.save.clone(),model,e.control.clone(),e.workspace.clone()).await.unwrap();(engine,effects,root)
}
#[tokio::test]
async fn planned_free_task_finishes_after_original_eight_rounds_without_changing_settings(){
    let calls=Arc::new(AtomicUsize::new(0));let count=calls.clone();
    let model:Model=Arc::new(move|request|{let count=count.clone();Box::pin(async move{
        let n=count.fetch_add(1,Ordering::SeqCst);
        Ok(match n {
            0=>tool_reply(n,"homebase_task",plan("inspect")),
            1..=10=>tool_reply(n,"homebase_files",json!({"action":"read","path":format!("synthetic-{n}.txt")})),
            11=>tool_reply(n,"homebase_task",json!({"action":"update","revision":1,"item_id":"inspect","state":"completed","evidence":[last_step(&request)]})),
            12=>reply(vec![]),
            _=>panic!("unexpected model retry"),
        })
    })});
    let(e,effects,_root)=custom(true,model).await;let before=serde_json::to_value(e.settings().await).unwrap();
    let ack=e.send(request("执行合成多阶段任务","task-beyond-eight")).await.unwrap();
    let t=task_wait(&e,&ack.turn_id,"completed").await;
    assert_eq!(calls.load(Ordering::SeqCst),13);assert_eq!(effects.load(Ordering::SeqCst),10);
    assert_eq!(t.task.as_ref().unwrap().limits.max_model_calls,24);assert_eq!(t.task.unwrap().plan.unwrap().items[0].state,"completed");
    assert_eq!(serde_json::to_value(e.settings().await).unwrap(),before);
}
#[tokio::test]
async fn incomplete_model_final_gets_bounded_feedback_and_cannot_claim_false_delivery(){
    let calls=Arc::new(AtomicUsize::new(0));let count=calls.clone();
    let model:Model=Arc::new(move|request|{let count=count.clone();Box::pin(async move{
        let n=count.fetch_add(1,Ordering::SeqCst);
        if n==0{return Ok(tool_reply(n,"homebase_task",plan("deliver")));}
        if n>1{assert!(request.messages.iter().any(|m|m["role"]=="system"&&m["content"].as_str().is_some_and(|s|s.contains("计划仍有未完成"))));}
        Ok(wire::parse("chat",&json!({"choices":[{"message":{"content":"所有事情都已完成，文件已送达手机"}}]})).unwrap())
    })});
    let(e,effects,_root)=custom(true,model).await;let ack=e.send(request("合成交付检查","task-no-false-delivery")).await.unwrap();
    let t=task_wait(&e,&ack.turn_id,"needs_input").await;
    assert_eq!(calls.load(Ordering::SeqCst),4);assert_eq!(effects.load(Ordering::SeqCst),0);
    assert!(t.text.contains("还没全部完成"));assert!(!t.text.contains("已送达手机"));assert!(t.artifacts.is_empty());
}
#[tokio::test]
async fn paid_plan_keeps_original_round_limit_and_pauses_with_checkpoint(){
    let calls=Arc::new(AtomicUsize::new(0));let count=calls.clone();
    let model:Model=Arc::new(move|_|{let count=count.clone();Box::pin(async move{
        let n=count.fetch_add(1,Ordering::SeqCst);Ok(if n==0{tool_reply(n,"homebase_task",plan("inspect"))}else{tool_reply(n,"homebase_files",json!({"action":"read","path":format!("synthetic-{n}.txt")}))})
    })});
    let(e,effects,_root)=custom(false,model).await;let ack=e.send(request("合成预算检查","task-paid-budget")).await.unwrap();
    let t=task_wait(&e,&ack.turn_id,"paused_budget").await;
    assert_eq!(calls.load(Ordering::SeqCst),8);assert_eq!(effects.load(Ordering::SeqCst),7);
    assert_eq!(t.task.unwrap().limits.max_model_calls,8);
}

#[tokio::test]
async fn delivery_precondition_is_recoverable_and_points_to_original_file_without_rewriting_it(){
    let(e,models,effects,_,_root)=fixture(false,vec![]).await;let a=e.send(request("合成交付前置条件","delivery-actionable")).await.unwrap();
    e.change_turn(&a.turn_id, |s,t| { s.turns[t].state = "running".into(); Ok(()) }).await.unwrap();
    let write=e.execute(&a.turn_id,&ToolCall{id:"write-original".into(),name:"homebase_files".into(),arguments:json!({"action":"write","path":"synthetic.txt","content":"synthetic"})}).await.unwrap().unwrap();
    e.change_turn(&a.turn_id,|s,t|{s.turns[t].rounds+=1;Ok(())}).await.unwrap();
    let result=e.execute(&a.turn_id,&ToolCall{id:"bad-delivery".into(),name:"homebase_deliver".into(),arguments:json!({"job_id":write["job_id"]})}).await.unwrap().unwrap();
    assert_eq!(result["status"],"failed");assert_eq!(result["code"],"ARTIFACT_REQUIRED");assert_eq!(result["next_tool"],"homebase_files");assert_eq!(result["next_request"]["action"],"export");assert_eq!(result["next_request"]["path"],"synthetic.txt");assert_eq!(effects.load(Ordering::SeqCst),1);assert_eq!(models.load(Ordering::SeqCst),0);assert!(e.turn(&a.turn_id).await.unwrap().artifacts.is_empty());
}
#[tokio::test]
async fn malformed_plan_call_returns_current_revision_and_usable_write_proof_without_reexecution(){
    let(e,models,effects,_,_root)=fixture(false,vec![]).await;
    let a=e.send(request("合成纠错","plan-actionable-error")).await.unwrap();
    e.change_turn(&a.turn_id, |s,t| { s.turns[t].state = "running".into(); Ok(()) }).await.unwrap();
    let execute=|id:&str,args:Value|ToolCall{id:id.into(),name:"homebase_task".into(),arguments:args};
    let missing=e.execute(&a.turn_id,&execute("missing",json!({"action":"set"}))).await.unwrap().unwrap();assert_eq!(missing["revision"],0);
    e.change_turn(&a.turn_id,|s,t|{s.turns[t].rounds+=1;Ok(())}).await.unwrap();
    e.execute(&a.turn_id,&execute("set",plan("write"))).await.unwrap();
    e.change_turn(&a.turn_id,|s,t|{s.turns[t].rounds+=1;Ok(())}).await.unwrap();
    let written=e.execute(&a.turn_id,&ToolCall{id:"write".into(),name:"homebase_files".into(),arguments:json!({"action":"write","path":"synthetic.txt","content":"synthetic"})}).await.unwrap().unwrap();
    e.change_turn(&a.turn_id,|s,t|{s.turns[t].rounds+=1;Ok(())}).await.unwrap();
    let invalid=e.execute(&a.turn_id,&execute("invalid",json!({"action":"update","revision":1,"item_id":"inspect","state":"completed","evidence":["I wrote the file"]}))).await.unwrap().unwrap();
    assert_eq!(invalid["code"],"PLAN_INVALID");assert_eq!(invalid["revision"],1);assert_eq!(invalid["recent_steps"][0]["step_id"],written["step_id"]);
    assert_eq!(invalid["required_kind"],"write");assert_eq!(invalid["valid_evidence_step_ids"][0],written["step_id"]);
    e.change_turn(&a.turn_id,|s,t|{s.turns[t].rounds+=1;Ok(())}).await.unwrap();
    let valid=e.execute(&a.turn_id,&execute("corrected",json!({"action":"update","revision":invalid["revision"],"item_id":"inspect","state":"completed","evidence":[invalid["recent_steps"][0]["step_id"]]}))).await.unwrap().unwrap();
    assert_eq!(valid["plan"]["items"][0]["state"],"completed");assert_eq!(effects.load(Ordering::SeqCst),1);assert_eq!(models.load(Ordering::SeqCst),0);
}
#[tokio::test]
async fn explicit_continue_adds_one_input_to_same_checkpoint_and_finishes_remaining_plan(){
    let calls=Arc::new(AtomicUsize::new(0));let count=calls.clone();
    let model:Model=Arc::new(move|r|{let count=count.clone();Box::pin(async move{
        let n=count.fetch_add(1,Ordering::SeqCst);
        Ok(match n {
            0=>tool_reply(n,"homebase_task",plan("inspect")),
            1..=3=>reply(vec![]),
            4=>tool_reply(n,"homebase_files",json!({"action":"read","path":"synthetic.txt"})),
            5=>tool_reply(n,"homebase_task",json!({"action":"update","revision":1,"item_id":"inspect","state":"completed","evidence":[last_step(&r)]})),
            6=>reply(vec![]),
            _=>panic!("continuation duplicated model execution"),
        })
    })});
    let(e,effects,_root)=custom(true,model).await;let a=e.send(request("合成续接","continue-same-checkpoint")).await.unwrap();task_wait(&e,&a.turn_id,"needs_input").await;
    e.resume(&a.turn_id).await.unwrap();let result=task_wait(&e,&a.turn_id,"completed").await;
    assert_eq!(calls.load(Ordering::SeqCst),7);assert_eq!(effects.load(Ordering::SeqCst),1);assert_eq!(result.task.unwrap().plan.unwrap().items[0].state,"completed");
    let state=e.state.lock().await;let(s,t)=locate(&state,&a.turn_id).unwrap();assert!(!state.sessions[s].turns[t].workflow.continuation_requested);
}
#[tokio::test]
async fn safe_planned_final_checkpoint_recovers_automatically_but_uncertain_model_does_not(){
    let(e,models,effects,saved,_root)=fixture(true,vec![reply(vec![("homebase_call",json!({"command":"system_gpu","args":{}}))]),reply(vec![])]).await;
    let mut settings=e.settings().await;settings.runtime="pi".into();e.save_settings(settings).await.unwrap();
    let ack=e.send(request("合成恢复","planned-safe-recover")).await.unwrap();task_wait(&e,&ack.turn_id,"completed").await;
    let mut state=saved.lock().await.clone();let(s,t)=locate(&state,&ack.turn_id).unwrap();
    let session=&mut state.sessions[s];
    let turn=&mut session.turns[t];turn.workflow.set_plan(0,"合成检查".into(),vec![workflow::ItemInput{id:"check".into(),title:"核对结果".into(),done_when:"已经核对".into(),kind:workflow::EvidenceKind::Inspect}]).unwrap();
    let original_step = turn.steps.iter().find(|step| step.name == "homebase_call").unwrap().id.clone();
    turn.workflow.update(1, "check", "completed", String::new(), vec![original_step], &turn.steps, &session.jobs).unwrap();
    turn.state="running".into();turn.history_committed=false;turn.finished_at=None;state.sessions[s].history.clear();
    let mut unknown=state.clone();unknown.sessions[s].turns[t].runtime_uncertain_model=true;
    let mut missing=state.clone();missing.sessions[s].turns[t].workflow.plan.as_mut().unwrap().items[0].evidence.clear();
    let restored=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    task_wait(&restored,&ack.turn_id,"completed").await;
    assert_eq!(models.load(Ordering::SeqCst),2);assert_eq!(effects.load(Ordering::SeqCst),1);
    let blocked=Engine::new(unknown,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    assert_eq!(blocked.turn(&ack.turn_id).await.unwrap().state,"interrupted_unknown");assert!(!blocked.turn(&ack.turn_id).await.unwrap().resumable);
    let legacy=Engine::new(missing,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    let review=task_wait(&legacy,&ack.turn_id,"needs_input").await;
    assert!(review.text.contains("完成结果待核对"));
    assert!(!review.resumable);
    assert_eq!(models.load(Ordering::SeqCst),2);assert_eq!(effects.load(Ordering::SeqCst),1);
}

#[tokio::test]
#[ignore="explicit 95-second isolated long-job regression; no provider or files"]
async fn accepted_long_tool_waits_beyond_ninety_seconds_and_submits_once(){
    let(e,_,effects,_,_root)=fixture(false,vec![]).await;
    let original=e.control.clone();let began=tokio::time::Instant::now();
    let control:Control=Arc::new(move|r|{let original=original.clone();Box::pin(async move{
        if r.method=="jobs.get"&&began.elapsed()<Duration::from_secs(95){return Ok(json!({"status":"running","result":null}));}
        original(r).await
    })});
    let engine=Engine::new(State::default(),e.save.clone(),e.model.clone(),control,e.workspace.clone()).await.unwrap();
    let ack=engine.send(request("合成长任务","slow-accepted-once")).await.unwrap();
    let output=engine.execute(&ack.turn_id,&ToolCall{id:"slow-tool".into(),name:"homebase_terminal".into(),arguments:json!({"command":"synthetic only"})}).await.unwrap().unwrap();
    assert!(began.elapsed()>=Duration::from_secs(95));assert_eq!(output["state"],"succeeded");assert_eq!(effects.load(Ordering::SeqCst),1);
}
