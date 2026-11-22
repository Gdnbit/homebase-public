// Loopback-only delivery receipts. No real account, token, model or file is used.
use super::*;

#[tokio::test]
async fn wechat_confirmation_delivery_follows_the_original_state_without_a_false_start() {
    let root = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let mut stored = account(&base);
    let cases = [
        ("rejected", "rejected", "已拒绝这项操作。"),
        ("completed", "succeeded", "这件事已经完成。"),
        ("awaiting_confirmation", "awaiting_confirmation", "还有一项不同的操作需要确认。"),
        ("running", "succeeded", "原任务已经完成。"),
    ];
    for (ack, _, _) in cases {
        stored.records.push(Record {
            id: format!("previous-{ack}"), command: "assistant".into(),
            status: "awaiting_confirmation".into(), job_id: Some(format!("turn-{ack}")),
            context: "old-context".into(), client_id: format!("old-client-{ack}"),
            ..Default::default()
        });
        stored.records.push(Record {
            id: format!("incoming-{ack}"), command: "assistant".into(), status: "received".into(),
            context: format!("latest-context-{ack}"), client_id: format!("latest-client-{ack}"),
            delivery: "pending".into(), args: json!({"text":"确认执行"}), ..Default::default()
        });
    }

    let runs = Arc::new(AtomicUsize::new(0));
    let observed = Arc::new(Notify::new());
    let agent: Agent = Arc::new({
        let (runs, observed) = (runs.clone(), observed.clone());
        move |request| {
            let (runs, observed) = (runs.clone(), observed.clone());
            Box::pin(async move {
                match request["operation"].as_str() {
                    Some("notifications") => Ok(json!([])),
                    Some("submit") => {
                        runs.fetch_add(1, Ordering::SeqCst);
                        let ack = request["request_id"].as_str().unwrap().strip_prefix("incoming-").unwrap();
                        Ok(json!({"action":"confirmation", "turn_id":format!("turn-{ack}"), "state":ack}))
                    }
                    Some("get") => {
                        let ack = request["turn_id"].as_str().unwrap().strip_prefix("turn-").unwrap();
                        let (_, status, text) = cases.iter().find(|(state, _, _)| *state == ack).unwrap();
                        if ack == "awaiting_confirmation" { observed.notify_one(); }
                        Ok(json!({"status":status, "result":{"turn_id":request["turn_id"], "text":text, "progress_signature":"next-confirmation"}}))
                    }
                    _ => panic!("unexpected synthetic assistant operation"),
                }
            })
        }
    });
    let (template, saved) = test_bridge(stored.clone(), root.path(), bridge_controller(root.path()));
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base.clone());
    let bridge = Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent.clone()));
    for (ack, _, _) in cases {
        bridge.submit(&format!("incoming-{ack}")).await.unwrap();
    }
    let text = |row: &Value| row["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap().to_string();
    {
        let traffic = seen.lock().await;
        let messages = traffic.iter().map(text).collect::<Vec<_>>();
        assert_eq!(messages, ["收到，我会继续跟进这件事。"]);
        assert_eq!(traffic[0]["body"]["msg"]["context_token"], "latest-context-running");
    }

    let monitor = tokio::spawn(bridge.clone().monitor(0));
    for (_, _, expected) in cases { sent_text(&seen, expected).await; }
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if saved.lock().await.records.iter().filter(|r| r.id.starts_with("incoming-") && r.delivery == "accepted").count() == 4 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    monitor.abort();
    assert!(monitor.await.unwrap_err().is_cancelled());
    observed.notified().await;

    let persisted = saved.lock().await.clone();
    for (ack, status, expected) in cases {
        let incoming = persisted.records.iter().find(|record| record.id == format!("incoming-{ack}")).unwrap();
        let previous = persisted.records.iter().find(|record| record.id == format!("previous-{ack}")).unwrap();
        assert_eq!(incoming.job_id.as_deref(), Some(format!("turn-{ack}").as_str()));
        assert_eq!(&incoming.status, status);
        assert_eq!(previous.status, "continued");
        let traffic = seen.lock().await;
        let reply = traffic.iter().find(|row| text(row) == expected).unwrap();
        assert_eq!(reply["body"]["msg"]["context_token"], format!("latest-context-{ack}"));
    }

    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let restored = Bridge::new_with_agent(persisted, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent));
    for (ack, _, _) in cases { restored.submit(&format!("incoming-{ack}")).await.unwrap(); }
    let monitor = tokio::spawn(restored.clone().monitor(0));
    tokio::time::timeout(Duration::from_secs(3), observed.notified()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(runs.load(Ordering::SeqCst), 4);
    assert_eq!(seen.lock().await.len(), 5);
    monitor.abort();
    server.abort();
}

#[tokio::test]
async fn proactive_unchanged_results_are_quiet_but_explicit_queries_changes_and_failures_reply() {
    let root=tempfile::tempdir().unwrap();let(base,seen,server)=fake_api(200,json!({})).await;
    let mut stored=account(&base);
    for id in ["hb-proactive-first","hb-proactive-repeat","hb-proactive-changed","hb-proactive-failure","explicit-query"] {
        stored.records.push(Record{id:id.into(),command:"assistant".into(),status:"succeeded".into(),context:"synthetic-context".into(),client_id:id.into(),..Default::default()});
    }
    let(bridge,saved)=test_bridge(stored,root.path(),controller(root.path(),Arc::new(AtomicUsize::new(0))));
    let result=|fingerprint:&str|json!({"status":"succeeded","result":{"text":"已核对合成事项，内容没有变化。","artifacts":[],"notification":{"group":"synthetic-plan","fingerprint":fingerprint,"policy":"meaningful"}}});
    bridge.reply_assistant("hb-proactive-first",&result("one"),true).await.unwrap();
    bridge.reply_assistant("hb-proactive-first",&result("one"),true).await.unwrap();assert_eq!(seen.lock().await.len(),1);
    bridge.reply_assistant("hb-proactive-repeat",&result("one"),true).await.unwrap();
    assert_eq!(bridge.status().await.tasks.iter().find(|t|t.id=="hb-proactive-repeat").unwrap().delivery,"suppressed");assert_eq!(seen.lock().await.len(),1);
    // The receipt survives a new bridge instance and reconnect, not just a RAM cache.
    let restored=saved.lock().await.clone();let(next,_)=test_bridge(restored,root.path(),bridge_controller(root.path()));
    next.reply_assistant("hb-proactive-repeat",&result("one"),true).await.unwrap();assert_eq!(seen.lock().await.len(),1);
    next.reply_assistant("hb-proactive-changed",&result("two"),true).await.unwrap();assert_eq!(seen.lock().await.len(),2);
    next.reply_assistant("explicit-query",&result("two"),true).await.unwrap();assert_eq!(seen.lock().await.len(),3);
    next.reply_assistant("hb-proactive-failure",&json!({"status":"failed","result":{"text":"检查失败，原结果还不能确认。","artifacts":[]}}),true).await.unwrap();assert_eq!(seen.lock().await.len(),4);
    server.abort();
}
fn bridge_controller(root:&Path)->Arc<Controller> {controller(root,Arc::new(AtomicUsize::new(0)))}
fn model_text(text:&str)->crate::assistant_protocol::ModelReply {
    crate::assistant_protocol::parse("chat",&json!({"choices":[{"message":{"content":text}}]})).unwrap()
}
fn model_tool(index:usize,name:&str,args:Value)->crate::assistant_protocol::ModelReply {
    crate::assistant_protocol::parse("chat",&json!({"choices":[{"message":{"tool_calls":[{"id":format!("adapt-wx-{index}"),"function":{"name":name,"arguments":args.to_string()}}]}}]})).unwrap()
}
async fn pending_action(engine:&crate::assistant::Engine,action:&str)->crate::assistant::TurnView {
    let end=tokio::time::Instant::now()+Duration::from_secs(40);
    loop {
        for session in engine.sessions().await {
            if let Some(turn)=engine.session(&session.id).await.unwrap().turns.into_iter().find(|t|t.state=="awaiting_confirmation"&&t.pending.as_ref().is_some_and(|p|p.arguments["action"]==action)){return turn;}
        }
        assert!(tokio::time::Instant::now()<end,"missing {action} confirmation");tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
async fn sent_text(seen:&Arc<Mutex<Vec<Value>>>,needle:&str) {
    let end=tokio::time::Instant::now()+Duration::from_secs(40);
    loop {
        if seen.lock().await.iter().any(|r|r["body"]["msg"]["item_list"].to_string().contains(needle)){return;}
        assert!(tokio::time::Instant::now()<end,"confirmation not actually sent: {needle}");tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
#[tokio::test]
async fn pi_wechat_steers_original_task_and_delivers_event_followup_once_after_restart() {
    adaptation_and_restart("pi").await;
}

async fn adaptation_and_restart(runtime: &str) {
    use crate::assistant::{Engine as Core,State as CoreState,FollowupInput};
    let root=tempfile::tempdir().unwrap();let sentinel=root.path().join("untouched.txt");std::fs::write(&sentinel,"KEEP SYNTHETIC ORIGINAL").unwrap();
    let ctl=bridge_controller(root.path());let controller=ctl.clone();
    let control:crate::assistant::Control=Arc::new(move|r|{let ctl=ctl.clone();Box::pin(async move{ctl.handle(r).await})});
    let rounds=Arc::new(AtomicUsize::new(0));let calls=rounds.clone();let routing=Arc::new(AtomicUsize::new(0));let routers=routing.clone();
    let model:crate::assistant::Model=Arc::new(move|r|{let calls=calls.clone();let routers=routers.clone();Box::pin(async move{
        if r.definitions.as_ref().is_some_and(|defs|defs.iter().any(|d|d["function"]["name"]=="homebase_steer_message")) {
            routers.fetch_add(1,Ordering::SeqCst);assert!(r.messages.last().unwrap()["content"].as_str().unwrap().contains("别保存旧方案"));
            return Ok(model_tool(99,"homebase_steer_message",json!({})));
        }
        let n=calls.fetch_add(1,Ordering::SeqCst);
        let last=||{let m=r.messages.iter().rev().find(|m|m["role"]=="tool").unwrap();serde_json::from_str::<Value>(m["content"].as_str().unwrap()).unwrap()};
        Ok(match n {
            0=>model_tool(n,"homebase_memory",json!({"action":"remember","revision":1,"title":"合成旧动作","content":"红点"})),
            1|4=>model_tool(n,"homebase_memory",json!({"action":"search","query":"动画偏好"})),
            2=>{let old=last();assert_eq!(old["memories"][0]["content"],"红色圆点");model_tool(n,"homebase_memory",json!({"action":"correct","revision":old["revision"],"id":old["memories"][0]["id"],"expected_sha256":old["memories"][0]["expected_sha256"],"content":"蓝色圆点，简短中文字幕","reason":"用户明确纠正旧动画偏好"}))},
            3=>model_text("已经把原来的动画偏好改为蓝色圆点和简短中文字幕，旧方案没有保存。"),
            5=>{assert_eq!(last()["memories"][0]["content"],"蓝色圆点，简短中文字幕");model_text("我按约定跟进了：最新动画偏好是蓝色圆点，字幕用简短中文，没有重新执行旧任务。")},
            _=>panic!("unexpected model replay {n}"),
        })
    })});
    let mut state=CoreState::default();state.settings.enabled=true;state.settings.runtime=runtime.into();state.settings.free_only=true;state.settings.route_id="openrouter::synthetic:free".into();
    let saved=Arc::new(Mutex::new(state.clone()));let output=saved.clone();let save:crate::assistant::Save=Arc::new(move|s|{let o=output.clone();Box::pin(async move{*o.lock().await=s;Ok(())})});
    let engine=Core::new(state,save.clone(),model.clone(),control.clone(),root.path().join("workspace")).await.unwrap();
    let memory=engine.memory_save(crate::assistant_personal::MemoryInput{revision:0,id:None,title:"动画偏好".into(),content:"红色圆点".into(),expires_at:None}).await.unwrap();
    let(base,seen,server)=fake_api(200,json!({})).await;let stored=account(&base);let(template,wx_saved)=test_bridge(stored.clone(),root.path(),controller.clone());
    let core=engine.clone();let agent:Agent=Arc::new(move|r|{let core=core.clone();Box::pin(async move{core.channel(r).await})});
    let mut api=Api::new().unwrap();api.test_origin=Some(base.clone());let bridge=Bridge::new_with_agent(stored,api,template.save.clone(),template.control.clone(),root.path().into(),Some(agent));bridge.start().await.unwrap();
    bridge.ingest(&msg("adapt-original","先保存一个合成旧方案")).await.unwrap();
    let original=pending_action(&engine,"remember").await;let old_confirmation=original.pending.as_ref().unwrap().id.clone();
    assert_eq!(original.runtime, runtime);
    sent_text(&seen,"保存这条记忆").await;
    let mut input=json!({"revision":0,"title":"纠错完成后跟进","instruction":"完成后核对最新动画偏好，只读并简短汇报，不重新保存或执行旧任务","due_at":(chrono::Utc::now()+chrono::Duration::hours(1)).to_rfc3339(),"interval_minutes":null,"max_runs":1,"matter_id":null,"trigger":{"turn_id":original.id,"on":"completed"}});
    let watch:FollowupInput=serde_json::from_value(input.take()).unwrap();engine.followup_save(watch,"wechat",&bridge.source_key().await).await.unwrap();
    bridge.ingest(&msg("adapt-change","别保存旧方案；把原有动画偏好纠正为蓝色圆点，字幕用简短中文，继续原任务")).await.unwrap();
    let corrected=pending_action(&engine,"correct").await;assert_eq!(corrected.id,original.id);assert_eq!(corrected.requirements_revision,1);
    assert!(engine.confirm(&original.id,&old_confirmation,true).await.is_err());
    sent_text(&seen,"纠正原来的长期记忆").await;
    assert_eq!(seen.lock().await.iter().filter(|r|r["body"]["msg"]["item_list"].to_string().contains("纠正原来的长期记忆")).count(),1);
    bridge.ingest(&msg("adapt-confirm","确认")).await.unwrap();
    sent_text(&seen, "收到，我会继续跟进这件事。").await;
    let end=tokio::time::Instant::now()+Duration::from_secs(50);
    loop {
        bridge.deliver_reminders().await;
        if engine.followups().await["entries"][0]["state"]=="completed" && bridge.status().await.tasks.iter().any(|t|t.id.starts_with("hb-proactive-")&&t.delivery=="accepted") {break;}
        assert!(tokio::time::Instant::now()<end,"event followup not delivered");tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let desktop=engine.personal().await;assert_eq!(desktop["memories"].as_array().unwrap().len(),1);assert_eq!(desktop["memories"][0]["id"],memory["id"]);assert_eq!(desktop["memories"][0]["content"],"蓝色圆点，简短中文字幕");assert_eq!(desktop["memories"][0]["corrections"][0]["origin"]["source"],"wechat");
    assert_eq!(engine.session(&original.session_id).await.unwrap().turns.iter().filter(|t|!t.requirements.is_empty()).count(),1);
    assert_eq!(rounds.load(Ordering::SeqCst),6);assert_eq!(routing.load(Ordering::SeqCst),1);assert_eq!(std::fs::read_to_string(&sentinel).unwrap(),"KEEP SYNTHETIC ORIGINAL");
    let mut wx=wx_saved.lock().await.clone();bridge.stop().await.unwrap();wx.enabled=true;
    let restored=Core::new(saved.lock().await.clone(),save,model,control,root.path().join("workspace")).await.unwrap();
    let core=restored.clone();let agent:Agent=Arc::new(move|r|{let core=core.clone();Box::pin(async move{core.channel(r).await})});let mut api=Api::new().unwrap();api.test_origin=Some(base);
    let resumed=Bridge::new_with_agent(wx,api,template.save.clone(),template.control.clone(),root.path().into(),Some(agent));let before=seen.lock().await.iter().filter(|r|r["body"]["msg"].is_object()).count();resumed.start().await.unwrap();tokio::time::sleep(Duration::from_millis(300)).await;resumed.deliver_reminders().await;
    assert_eq!(seen.lock().await.iter().filter(|r|r["body"]["msg"].is_object()).count(),before);assert_eq!(rounds.load(Ordering::SeqCst),6);assert_eq!(restored.personal().await["memories"][0]["id"],memory["id"]);resumed.stop().await.unwrap();server.abort();
}
#[tokio::test]
async fn proactive_receipt_save_failure_cannot_send_or_claim_success() {
    let root=tempfile::tempdir().unwrap();let(base,seen,server)=fake_api(200,json!({})).await;
    let mut stored=account(&base);stored.records.push(Record{id:"hb-proactive-save-failure".into(),command:"assistant".into(),status:"succeeded".into(),context:"synthetic".into(),client_id:"synthetic".into(),..Default::default()});
    let(template,_)=test_bridge(stored.clone(),root.path(),bridge_controller(root.path()));
    let save:Save=Arc::new(|_|Box::pin(async {Err("synthetic disk failure".into())}));let mut api=Api::new().unwrap();api.test_origin=Some(base);
    let bridge=Bridge::new(stored,api,save,template.control.clone(),root.path().into());
    let value=json!({"status":"succeeded","result":{"text":"合成结果","artifacts":[],"notification":{"group":"synthetic-plan","fingerprint":"one"}}});
    assert!(bridge.reply_assistant("hb-proactive-save-failure",&value,true).await.is_err());
    assert!(seen.lock().await.is_empty());assert!(bridge.state.lock().await.stored.notices.is_empty());server.abort();
}
