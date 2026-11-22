use super::*;
async fn message(e:&Arc<Engine>,rid:&str,text:&str)->Result<Value,String>{e.channel(json!({"operation":"submit","source_key":"synthetic-owner","request_id":rid,"text":text})).await}

#[tokio::test]
async fn confirmation_control_race_reconciles_the_original_desktop_decision() {
    use std::{future::Future, task::Poll};

    for approve_on_desktop in [true, false] {
        let (engine, _, _, saved, _root) = fixture(true, vec![
            reply(vec![("homebase_memory", json!({
                "action":"remember", "revision":0, "title":"synthetic race", "content":"synthetic"
            }))]),
            reply(vec![]),
        ]).await;
        let original = message(&engine, "original", "保存合成记忆").await.unwrap();
        let turn = original["turn_id"].as_str().unwrap();
        let pending = wait(&engine, turn, "awaiting_confirmation").await.pending.unwrap();
        let text = format!("确认 {}", pending.id);

        let gate = engine.send_gate.lock().await;
        let mut desktop = std::pin::pin!(engine.confirm(turn, &pending.id, approve_on_desktop));
        assert!(std::future::poll_fn(|cx| Poll::Ready(desktop.as_mut().poll(cx))).await.is_pending());
        let mut wechat = std::pin::pin!(message(&engine, "racing-approval", &text));
        assert!(std::future::poll_fn(|cx| Poll::Ready(wechat.as_mut().poll(cx))).await.is_pending());
        {
            let state = saved.lock().await;
            assert_eq!(state.channel_controls.len(), 1);
            assert!(state.channel_controls[0].response.is_none());
        }
        drop(gate);

        desktop.await.unwrap();
        let response = wechat.await.unwrap();
        assert_eq!(response["action"], "confirmation");
        assert_eq!(response["turn_id"], turn);
        assert_eq!(response["session_id"], original["session_id"]);
        if approve_on_desktop {
            assert!(matches!(response["state"].as_str(), Some("running" | "completed")));
            wait(&engine, turn, "completed").await;
        } else {
            assert_eq!(response["state"], "rejected");
            wait(&engine, turn, "rejected").await;
        }
        assert_eq!(message(&engine, "racing-approval", &text).await.unwrap(), response);
        // Keep the durable receipt compatible with versions that predate this response classification.
        assert!(saved.lock().await.channel_controls[0].response.as_ref().unwrap()["action"].is_null());
        assert_eq!(engine.personal().await["memories"].as_array().unwrap().len(), usize::from(approve_on_desktop));
    }
}

#[tokio::test]
async fn control_replay_after_lost_ack_never_approves_next_operation_or_opens_second_chat(){
    let(e,_,_,saved,_root)=fixture(true,vec![reply(vec![("homebase_memory",json!({"action":"remember","revision":0,"title":"first","content":"synthetic"}))]),reply(vec![]),reply(vec![("homebase_memory",json!({"action":"remember","revision":1,"title":"second","content":"synthetic"}))])]).await;
    let a=message(&e,"one","save synthetic memory").await.unwrap();let first=a["turn_id"].as_str().unwrap();wait(&e,first,"awaiting_confirmation").await;
    message(&e,"approve-one","确认执行").await.unwrap();wait(&e,first,"completed").await;
    let b=message(&e,"two","save another synthetic memory").await.unwrap();let second=b["turn_id"].as_str().unwrap();let pending=wait(&e,second,"awaiting_confirmation").await.pending.unwrap().id;
    let mut state=saved.lock().await.clone();
    // Simulate a crash after approval was applied, before its channel ACK persisted.
    for receipt in &mut state.channel_controls {receipt.response=None;}
    let restored=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    let replay=message(&restored,"approve-one","确认执行").await.unwrap();assert_eq!(replay["turn_id"],first);
    assert_eq!(restored.turn(second).await.unwrap().pending.unwrap().id,pending);
    assert_eq!(restored.personal().await["memories"].as_array().unwrap().len(),1);
    let new=message(&restored,"new-one","新对话").await.unwrap();
    let mut state=restored.state.lock().await.clone();for receipt in &mut state.channel_controls {receipt.response=None;}
    let again=Engine::new(state,e.save.clone(),e.model.clone(),e.control.clone(),e.workspace.clone()).await.unwrap();
    assert_eq!(message(&again,"new-one","新对话").await.unwrap(),new);assert_eq!(again.sessions().await.len(),2);
    assert!(message(&again,"new-one","停止").await.is_err());
    assert_eq!(again.turn(second).await.unwrap().state,"awaiting_confirmation");
}
#[test]
fn short_aliases_never_turn_questions_or_long_instructions_into_consent(){
    assert_eq!(channel_control::normalized("继续吧"),"继续");assert_eq!(channel_control::normalized("确认？"),"确认？");
    assert_eq!(channel_control::normalized("确认一下是否完成"),"确认一下是否完成");
}
#[tokio::test]
async fn checkpoint_resume_clears_old_failure_presentation_before_execution(){
    let(base,_,effects,_,_root)=fixture(false,vec![]).await;
    let calls=Arc::new(AtomicUsize::new(0));let observed=calls.clone();
    let reached=Arc::new(Notify::new());let release=Arc::new(Notify::new());
    let arrived=reached.clone();let released=release.clone();
    let model:Model=Arc::new(move |_|{let(observed,arrived,released)=(observed.clone(),arrived.clone(),released.clone());Box::pin(async move {
        if observed.fetch_add(1,Ordering::SeqCst)==0 {return Err(crate::model_capability_io::reliability::Failure::http(503).encode());}
        arrived.notify_one();released.notified().await;Ok(reply(vec![]))
    })});
    let mut state=State::default();state.settings.enabled=true;state.settings.runtime="pi".into();state.settings.route_id="openrouter::fixture:free".into();
    let engine=Engine::new(state,base.save.clone(),model,base.control.clone(),base.workspace.clone()).await.unwrap();
    let original=engine.send(request("synthetic","clear-resume-error")).await.unwrap();
    wait(&engine,&original.turn_id,"failed").await;
    for _ in 0..100 {if !engine.running.lock().await.contains_key(&original.turn_id){break;}tokio::time::sleep(Duration::from_millis(10)).await;}
    assert!(!engine.running.lock().await.contains_key(&original.turn_id));
    engine.resume(&original.turn_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(30),reached.notified()).await.unwrap();
    let result=engine.turn(&original.turn_id).await.unwrap();assert_eq!(result.state,"running");assert!(result.text.is_empty());assert!(result.task.unwrap().stop.is_none());
    let session=engine.session(&original.session_id).await.unwrap();assert!(session.messages.iter().filter(|m|m.role=="assistant").all(|m|m.content.is_empty()));
    release.notify_one();wait(&engine,&original.turn_id,"completed").await;
    assert_eq!(calls.load(Ordering::SeqCst),2);assert_eq!(effects.load(Ordering::SeqCst),0);
}
#[tokio::test]
async fn stopped_channel_status_is_natural_while_machine_job_fields_stay_queryable(){
    let(e,_,_,_,_root)=fixture(false,vec![]).await;let a=e.send(request("synthetic","natural-stop-status")).await.unwrap();
    e.change_turn(&a.turn_id,|s,t|{s.turns[t].state="canceled".into();s.turns[t].text="助理轮次已停止".into();s.jobs.push(Job{id:"owned".into(),turn_id:a.turn_id.clone(),command:"assistant_local_execute".into(),state:"succeeded".into(),result:json!({"status":"succeeded"}),owned:true,deliver:false,artifacts:vec![],remote:None,archived_result_bytes:None});Ok(())}).await.unwrap();
    let result=e.channel_view("desktop",&a.turn_id,false).await.unwrap();let text=result["result"]["text"].as_str().unwrap();
    assert!(text.contains("本机操作：已完成"));assert!(!text.contains("assistant_local_execute"));assert!(!text.contains("轮次"));assert_eq!(result["result"]["jobs"][0]["command"],"assistant_local_execute");
}
#[tokio::test]
async fn resume_generation_prevents_a_lost_control_ack_granting_budget_twice(){
    let(e,_,_,_,_root)=fixture(false,vec![]).await;
    let a=message(&e,"task","synthetic").await.unwrap();let tid=a["turn_id"].as_str().unwrap();
    e.change_turn(tid, |s, t| {
        let turn = &mut s.turns[t];
        turn.workflow.resume_generation = 1;
        turn.state = "paused_budget".into();
        turn.control_intents.insert("receipt-still-being-resolved".into(), "capability_render".into());
        Ok(())
    }).await.unwrap();
    let generation = e.inspect_turn(tid, |s, t| view_summary_at(s, t, chrono::Utc::now()).resume_generation).await.unwrap();
    assert_eq!(generation, 1);
    let ack=e.resume_once(tid,Some(0)).await.unwrap();assert_eq!(ack.state,"paused_budget");
    let state=e.state.lock().await;let(s,t)=locate(&state,tid).unwrap();
    let turn = &state.sessions[s].turns[t];
    assert_eq!(turn.workflow.extra_rounds, 0);
    assert_eq!(turn.workflow.resume_generation, generation);
    assert!(turn.control_intents.contains_key("receipt-still-being-resolved"));
}
