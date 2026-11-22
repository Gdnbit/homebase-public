// Actual SDK + loopback WeChat + isolated artifact. Never contacts real services.
use super::*;
use crate::assistant::{
    Engine as AssistantEngine, FollowupInput, MatterInput, State as AssistantState,
};
use crate::assistant_protocol::{self as wire, ModelReply, ModelRequest};
fn response(n: usize, name: &str, args: Value) -> ModelReply {
    wire::parse("chat", &json!({"choices":[{"message":{"tool_calls":[{"id":format!("dialogue-test-{n}"),"function":{"name":name,"arguments":args.to_string()}}]}}]})).unwrap()
}
async fn until_pending(e: &AssistantEngine) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        for s in e.sessions().await {
            let v = e.session(&s.id).await.unwrap();
            if let Some(t) = v.turns.iter().find(|t| t.state == "awaiting_confirmation") {
                return t.id.clone();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no pending operation"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
async fn until_delivered(b: &Bridge, id: &str) {
    let id = format!(
        "wx-{:x}",
        Sha256::digest(format!("synthetic-bot:synthetic-owner:{id}").as_bytes())
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        if b.status().await.tasks.iter().any(|t| {
            t.id == id
                && matches!(t.status.as_str(), "succeeded" | "completed")
                && t.delivery == "accepted"
        }) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "message not delivered: {}",
            serde_json::to_string(&b.status().await).unwrap()
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
}

#[tokio::test]
async fn pi_wechat_greeting_and_model_questions_send_one_direct_reply_each() {
    let lab = tempfile::tempdir().unwrap();
    let (base, sent, server) = fake_api(200, json!({})).await;
    let stored = account(&base);
    let effects = Arc::new(AtomicUsize::new(0));
    let (template, _) = test_bridge(stored.clone(), lab.path(), controller(lab.path(), effects.clone()));
    let calls = Arc::new(AtomicUsize::new(0));
    let model: crate::assistant::Model = Arc::new({
        let calls = calls.clone();
        move |request| {
            assert_eq!(calls.fetch_add(1, Ordering::SeqCst), 0, "model identity must be answered by the host");
            assert_eq!(request.definitions.as_deref(), Some([].as_slice()));
            assert_eq!(request.messages.last().unwrap()["content"], "nihaoya");
            Box::pin(async { wire::parse("chat", &json!({"choices":[{"message":{"content":"你好呀！"}}]})) })
        }
    });
    let mut state = AssistantState::default();
    state.settings.enabled = true;
    state.settings.wechat_enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "openrouter::synthetic/model:free".into();
    let engine = AssistantEngine::new(state, Arc::new(|_| Box::pin(async { Ok(()) })), model,
        template.control.clone(), lab.path().into()).await.unwrap();
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let agent: Agent = Arc::new(move |request| {
        let engine = engine.clone(); Box::pin(async move { engine.channel(request).await })
    });
    let bridge = Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(),
        lab.path().into(), Some(agent));
    bridge.start().await.unwrap();
    for (id, text) in [("greeting", "nihaoya"), ("model-one", "你是什么模型"), ("model-two", "你是什么模型？")] {
        let inbound = msg(id, text);
        bridge.ingest(&inbound).await.unwrap();
        bridge.ingest(&inbound).await.unwrap();
        until_delivered(&bridge, id).await;
    }
    bridge.stop().await.unwrap();
    let rows = sent.lock().await;
    let replies: Vec<_> = rows.iter().filter_map(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
    assert_eq!(replies, ["你好呀！", "我现在用的是 synthetic/model:free，通过 openrouter 调用。",
        "我现在用的是 synthetic/model:free，通过 openrouter 调用。"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn pi_wechat_preserves_natural_conversation_context_and_deduplicates_inbound_messages() {
    let lab = tempfile::tempdir().unwrap();
    let (base, sent, server) = fake_api(200, json!({})).await;
    let stored = account(&base);
    let effects = Arc::new(AtomicUsize::new(0));
    let (template, wx_saved) = test_bridge(stored.clone(), lab.path(), controller(lab.path(), effects.clone()));
    let model_calls = Arc::new(AtomicUsize::new(0));
    let model: crate::assistant::Model = Arc::new({
        let model_calls = model_calls.clone();
        move |request| {
            let call = model_calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                assert!(!request.messages.iter().any(|m| m["content"].as_str()
                    .is_some_and(|text| text.contains("旧会话隔离标识"))));
                let content = match call {
                    0 => "好，这段对话里我们叫它银杏。",
                    1 => {
                        assert!(request.messages.iter().any(|message| message["role"]=="user"
                            && message["content"].as_str().is_some_and(|message| message.contains("叫银杏"))));
                        assert!(request.messages.iter().any(|message| message["role"]=="assistant"
                            && message["content"]=="好，这段对话里我们叫它银杏。"));
                        "我们刚才叫它银杏。"
                    }
                    2 => {
                        assert!(request.messages.iter().any(|message| message["role"]=="assistant"
                            && message["content"]=="我们刚才叫它银杏。"));
                        "重启后仍记得，项目代号是银杏。"
                    }
                    _ => panic!("Repeated model request for a duplicate inbound message"),
                };
                wire::parse("chat", &json!({"choices":[{"message":{"content":content}}]}))
            })
        }
    });
    let mut state = AssistantState::default();
    state.settings.runtime = "deepagents".into();
    let saved = Arc::new(Mutex::new(state.clone()));
    let save: crate::assistant::Save = Arc::new({
        let saved = saved.clone();
        move |state| { let saved = saved.clone(); Box::pin(async move { *saved.lock().await = state; Ok(()) }) }
    });
    let old = AssistantEngine::new(state, save.clone(), model.clone(), template.control.clone(), lab.path().into()).await.unwrap();
    let source = stored.logical_key();
    let history = old.channel(json!({"operation":"submit","source_key":source,"request_id":"archived-chat",
        "text":"旧会话隔离标识"})).await.unwrap();
    let old_id = history["turn_id"].as_str().unwrap();
    let archived = old.channel(json!({"operation":"get","source_key":source,"turn_id":old_id})).await.unwrap();
    drop(old);
    let mut state = saved.lock().await.clone();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let engine = AssistantEngine::new(state, save.clone(), model.clone(),
        template.control.clone(), lab.path().into()).await.unwrap();
    let connect = |stored, agent: Arc<AssistantEngine>| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), lab.path().into(),
            Some(Arc::new(move |request| { let agent = agent.clone(); Box::pin(async move { agent.channel(request).await }) })))
    };
    let bridge = connect(stored, engine.clone());
    bridge.start().await.unwrap();
    let first = msg("pi-chat-first", "这个虚构的项目在这段对话里叫银杏。");
    bridge.ingest(&first).await.unwrap();
    bridge.ingest(&first).await.unwrap();
    until_delivered(&bridge, "pi-chat-first").await;
    let second = msg("pi-chat-second", "我们刚才给它起的名字是什么？");
    bridge.ingest(&second).await.unwrap();
    bridge.ingest(&second).await.unwrap();
    until_delivered(&bridge, "pi-chat-second").await;
    let sessions = engine.sessions().await;
    assert_eq!(sessions.len(), 2);
    let active = sessions.iter().find(|session| session.id != history["session_id"]).unwrap();
    let session = engine.session(&active.id).await.unwrap();
    assert_eq!(session.turns.len(), 2);
    assert!(session.turns.iter().all(|turn|turn.runtime == "pi" && turn.state == "completed"));
    assert_eq!(model_calls.load(Ordering::SeqCst), 2);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(sent.lock().await.iter().any(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"]
        .as_str().is_some_and(|text|text.contains("我们刚才叫它银杏。"))));
    assert_eq!(engine.channel(json!({"operation":"get","source_key":source,"turn_id":old_id})).await.unwrap(), archived);
    let stored = wx_saved.lock().await.clone();
    bridge.stop().await.unwrap();
    drop(bridge);
    drop(engine);
    let restored = AssistantEngine::new(saved.lock().await.clone(), save, model,
        template.control.clone(), lab.path().into()).await.unwrap();
    let bridge = connect(stored, restored.clone());
    bridge.start().await.unwrap();
    bridge.ingest(&first).await.unwrap();
    let third = msg("pi-chat-third", "重启以后还记得吗？");
    bridge.ingest(&third).await.unwrap();
    until_delivered(&bridge, "pi-chat-third").await;
    assert_eq!(restored.sessions().await.len(), 2);
    assert_eq!(restored.session(&active.id).await.unwrap().turns.len(), 3);
    assert_eq!(model_calls.load(Ordering::SeqCst), 3);
    bridge.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn pi_wechat_compacts_early_context_and_continues_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let (base, outgoing, server) = fake_api(200, json!({})).await;
    let stored = account(&base);
    let effects = Arc::new(AtomicUsize::new(0));
    let (template, wx_saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), effects.clone()));
    let replies = Arc::new(AtomicUsize::new(0));
    let summaries = Arc::new(AtomicUsize::new(0));
    let model: crate::assistant::Model = Arc::new({
        let (replies, summaries) = (replies.clone(), summaries.clone());
        move |request| {
            let (replies, summaries) = (replies.clone(), summaries.clone());
            Box::pin(async move {
                let summary_request = request.messages[0]["content"].as_str().unwrap().starts_with("HomeBase Pi 自动摘要");
                let answer = if summary_request {
                    assert_eq!(summaries.fetch_add(1, Ordering::SeqCst), 0);
                    assert_eq!(request.definitions.as_ref().unwrap().len(), 0);
                    assert!(request.messages[1]["content"].as_str().unwrap().contains("初始代号琥珀松"));
                    "这个会话里的原始代号是琥珀松。"
                } else {
                    let round = replies.fetch_add(1, Ordering::SeqCst);
                    if round >= 8 {
                        assert!(!request.messages.iter().any(|m| m["role"] != "system" && m["content"]
                            .as_str().is_some_and(|content| content.contains("初始代号琥珀松"))));
                        assert!(request.messages.iter().any(|m| m["role"] == "system" && m["content"]
                            .as_str().is_some_and(|content| content.contains("conversation_summary") && content.contains("琥珀松"))));
                        assert!(request.messages.iter().any(|m| m["role"] == "system" && m["content"]
                            .as_str().is_some_and(|content| content.contains("字幕采用简短中文"))));
                        "代号依然是琥珀松，字幕用简短中文。"
                    } else {
                        "收到，这一轮的聊天已经完成。"
                    }
                };
                wire::parse("chat", &json!({"choices":[{"message":{"content":answer}}]}))
            })
        }
    });
    let mut state = AssistantState::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "openrouter::synthetic:free".into();
    let shared = Arc::new(Mutex::new(state.clone()));
    let save: crate::assistant::Save = Arc::new({
        let shared = shared.clone();
        move |state| { let shared = shared.clone(); Box::pin(async move { *shared.lock().await = state; Ok(()) }) }
    });
    let engine = AssistantEngine::new(state, save.clone(), model.clone(), template.control.clone(), root.path().into()).await.unwrap();
    engine.memory_save(crate::assistant_personal::MemoryInput {
        revision:0, id:None, title:"字幕偏好".into(), content:"字幕采用简短中文".into(), expires_at:None,
    }).await.unwrap();
    let bridge_for = |account, engine: Arc<AssistantEngine>| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let agent: Agent = Arc::new(move |r| { let engine = engine.clone(); Box::pin(async move { engine.channel(r).await }) });
        Bridge::new_with_agent(account, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent))
    };
    let bridge = bridge_for(stored, engine.clone());
    bridge.start().await.unwrap();
    for i in 0..9 {
        let id = format!("history-pi-{i}");
        let content = match i {
            0 => "初始代号琥珀松".to_string(),
            8 => "最早的代号和我的字幕偏好是什么？".to_string(),
            _ => format!("继续聊这个合成项目，第 {i} 轮。"),
        };
        bridge.ingest(&msg(&id, &content)).await.unwrap();
        until_delivered(&bridge, &id).await;
    }
    assert_eq!(summaries.load(Ordering::SeqCst), 1);
    let original_session = engine.sessions().await[0].id.clone();
    assert!(engine.session(&original_session).await.unwrap().messages.iter().any(|m| m.content == "初始代号琥珀松"));
    let wx = wx_saved.lock().await.clone();
    bridge.stop().await.unwrap();
    drop(bridge);
    drop(engine);
    let restored = AssistantEngine::new(shared.lock().await.clone(), save, model,
        template.control.clone(), root.path().into()).await.unwrap();
    let bridge = bridge_for(wx, restored.clone());
    bridge.start().await.unwrap();
    let next = msg("history-pi-restarted", "重启以后，再告诉我代号和字幕偏好。");
    bridge.ingest(&next).await.unwrap();
    bridge.ingest(&next).await.unwrap();
    until_delivered(&bridge, "history-pi-restarted").await;
    assert_eq!(replies.load(Ordering::SeqCst), 10);
    assert_eq!(summaries.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(restored.session(&original_session).await.unwrap().turns.len(), 10);
    assert_eq!(restored.personal().await["memories"].as_array().unwrap().len(), 1);
    assert!(outgoing.lock().await.iter().any(|row| row["body"]["msg"].to_string().contains("代号依然是琥珀松")));
    bridge.stop().await.unwrap();
    server.abort();
}

fn quote_confirmation(id: &str, text: &str, quoted: &str) -> Value {
    let mut message = msg(id, text);
    message["item_list"][0]["ref_msg"] = json!({"svr_id":quoted});
    message
}

async fn until_confirmation_receipt(bridge: &Bridge, id: &str) {
    let key = identity::source_key(&bridge.source_key().await, id);
    tokio::time::timeout(Duration::from_secs(15), async {
        while !bridge.state.lock().await.stored.records.iter().any(|row| row.outbound_quote_keys.contains(&key)) {
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    }).await.expect("confirmation receipt did not persist");
}

#[tokio::test]
async fn delayed_wechat_classification_does_not_supersede_a_newer_desktop_change() {
    use crate::assistant::SteerInput;

    const WECHAT_CHANGE: &str = "这次改用绿色方案";
    const DESKTOP_CHANGE: &str = "改用蓝色方案，只核对，不保存旧偏好";
    let lab = tempfile::tempdir().unwrap();
    let (base, traffic, server) = fake_api(200, json!({})).await;
    let stored = account(&base);
    let effects = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(stored.clone(), lab.path(), controller(lab.path(), effects.clone()));
    let classifier_started = Arc::new(Notify::new());
    let release_classifier = Arc::new(Notify::new());
    let release_task = Arc::new(Notify::new());
    let classifier_calls = Arc::new(AtomicUsize::new(0));
    let model: crate::assistant::Model = Arc::new({
        let (started, classifier, task, count) = (classifier_started.clone(), release_classifier.clone(),
            release_task.clone(), classifier_calls.clone());
        move |r| {
            let (started, classifier, task, count) = (started.clone(), classifier.clone(), task.clone(), count.clone());
            Box::pin(async move {
                if r.definitions.as_ref().is_some_and(|tools| tools.iter().any(|tool| tool["function"]["name"] == "homebase_steer_message")) {
                    assert_eq!(r.messages.last().unwrap()["content"], WECHAT_CHANGE);
                    count.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    classifier.notified().await;
                    return Ok(response(1, "homebase_steer_message", json!({})));
                }
                if r.messages.iter().any(|m| m["content"].as_str().is_some_and(|text| text.contains(DESKTOP_CHANGE))) {
                    task.notified().await;
                    return wire::parse("chat", &json!({"choices":[{"message":{"content":"已经按蓝色方案核对。"}}]}));
                }
                Ok(response(0, "homebase_memory", json!({"action":"remember","revision":0,"title":"合成旧偏好","content":"红色"})))
            })
        }
    });
    let mut state = AssistantState::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let engine = AssistantEngine::new(state, Arc::new(|_| Box::pin(async { Ok(()) })), model,
        template.control.clone(), lab.path().into()).await.unwrap();
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let agent = engine.clone();
    let bridge = Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), lab.path().into(),
        Some(Arc::new(move |request| { let agent = agent.clone(); Box::pin(async move { agent.channel(request).await }) })));
    bridge.start().await.unwrap();
    bridge.ingest(&msg("route-existing-job", "保存这条合成旧偏好")).await.unwrap();
    let original = until_pending(&engine).await;
    let old_confirmation = engine.turn(&original).await.unwrap().pending.unwrap().id;
    let reply = msg("route-old-edit", WECHAT_CHANGE);
    bridge.ingest(&reply).await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), classifier_started.notified()).await.unwrap();

    let accepted = engine.steer(SteerInput { turn_id: original.clone(), revision: 0,
        request_id: "route-new-desktop-edit".into(), text: DESKTOP_CHANGE.into() }).await.unwrap();
    assert_eq!(accepted["revision"], 1);
    assert!(engine.confirm(&original, &old_confirmation, true).await.is_err());
    release_classifier.notify_one();
    until_delivered(&bridge, "route-old-edit").await;
    bridge.ingest(&reply).await.unwrap();
    let original_after_routing = engine.turn(&original).await.unwrap();
    assert_eq!(original_after_routing.requirements_revision, 1, "late routing must not override the newer requirement");
    assert_eq!(original_after_routing.requirements[0].text, DESKTOP_CHANGE);
    let routed = saved.lock().await.records.last().unwrap().job_id.clone().unwrap();
    assert!(engine.turn(&routed).await.unwrap().steered_turn_id.is_none());
    assert!(traffic.lock().await.iter().any(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"]
        .as_str().is_some_and(|text| text.contains("原任务已有更新") && text.contains("未应用"))));

    release_task.notify_one();
    let completed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let turn = engine.turn(&original).await.unwrap();
            if turn.state == "completed" { break turn; }
            assert_ne!(turn.state, "failed", "{}", turn.text);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    assert_eq!(completed.id, original);
    assert!(completed.text.contains("蓝色"));
    assert_eq!(classifier_calls.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(engine.personal().await["memories"].as_array().unwrap().is_empty());
    bridge.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn wechat_quoted_reuse_passes_the_original_workflow_preference_to_the_shared_model() {
    const QUOTE: &str = "9007199254741996";
    const REUSE: &str = "再照上次的做法做一版。";
    let root = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({"__send_message_ids":{"workflow-reuse-origin":QUOTE}})).await;
    let mut stored = account(&base);
    let source = stored.logical_key();
    let effects = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), effects.clone()));
    let reused = Arc::new(AtomicUsize::new(0));
    let model: crate::assistant::Model = Arc::new({
        let reused = reused.clone();
        move |request| {
            let reused = reused.clone();
            Box::pin(async move {
                let text = if request.messages.last().is_some_and(|message| message["content"] == REUSE) {
                    let prompt = request.messages[0]["content"].as_str().unwrap();
                    assert!(prompt.contains("fixture-cover-square"));
                    assert!(!prompt.contains("fixture-clip-fast"));
                    assert!(prompt.contains("微信引用已核对"));
                    reused.fetch_add(1, Ordering::SeqCst);
                    "这版沿用之前的方形封面做法。"
                } else {
                    "上一版封面已整理。"
                };
                wire::parse("chat", &json!({"choices":[{"message":{"content":text}}]}))
            })
        }
    });
    let mut settings = AssistantState::default();
    settings.settings.enabled = true;
    settings.settings.runtime = "pi".into();
    settings.settings.route_id = "synthetic-only".into();
    let save: crate::assistant::Save = Arc::new(|_| Box::pin(async { Ok(()) }));
    let engine = AssistantEngine::new(settings, save, model, template.control.clone(), root.path().into()).await.unwrap();
    for (revision, title, content) in [(0, "默认出图", "fixture-cover-square"), (1, "常用剪片", "fixture-clip-fast")] {
        engine.memory_save(crate::assistant_personal::MemoryInput {
            revision, id:None, title:title.into(), content:content.into(), expires_at:None,
        }).await.unwrap();
    }
    let original = engine.channel(json!({"operation":"submit","source_key":source,
        "request_id":"workflow-reuse-seed","text":"帮我生成一张图片做蓝色封面。"})).await.unwrap();
    let turn = original["turn_id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            let view = engine.turn(turn).await.unwrap();
            if view.state == "completed" { break; }
            assert_ne!(view.state, "failed", "{}", view.text);
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }).await.unwrap();
    stored.records.push(Record {
        id:"workflow-reuse-origin".into(), command:"assistant".into(), status:"succeeded".into(),
        client_id:"workflow-reuse-origin".into(), context:"synthetic-context".into(),
        job_id:Some(turn.into()), ..Default::default()
    });
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let agent = Arc::new({
        let engine = engine.clone();
        move |request| -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>> {
            let engine = engine.clone();
            Box::pin(async move { engine.channel(request).await })
        }
    });
    let bridge = Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent));
    bridge.reply("workflow-reuse-origin", "上一版封面已整理。", None).await.unwrap();
    bridge.start().await.unwrap();
    let message = quote_confirmation("workflow-reuse-quote", REUSE, QUOTE);
    bridge.ingest(&message).await.unwrap();
    bridge.ingest(&message).await.unwrap();
    until_delivered(&bridge, "workflow-reuse-quote").await;
    let latest = saved.lock().await.records.last().unwrap().job_id.clone().unwrap();
    assert_ne!(latest, turn);
    assert_eq!(engine.turn(&latest).await.unwrap().parent_turn_id.as_deref(), Some(turn));
    assert_eq!(reused.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(seen.lock().await.iter().any(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"]
        .as_str().is_some_and(|text| text.contains("这版沿用之前的方形封面做法。"))));
    bridge.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn quoted_followup_restarts_and_changes_only_the_original_inflight_task() {
    let root = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({"__send_message_ids":{
        "older-quote-fixture":"9007199254741991", "newer-quote-fixture":"9007199254741992"
    }})).await;
    let stored = account(&base);
    let source = stored.logical_key();
    let effects = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), effects.clone()));
    let steers = Arc::new(AtomicUsize::new(0));
    let count = steers.clone();
    let model: crate::assistant::Model = Arc::new(move |r| {
        let count = count.clone();
        Box::pin(async move {
            const CHANGE: &str = "把这张图的字幕改成绿色";
            if r.definitions.as_ref().is_some_and(|tools| tools.iter().any(|tool| tool["function"]["name"] == "homebase_steer_message")) {
                assert_eq!(r.messages.last().unwrap()["content"], CHANGE);
                let prompt = r.messages[0]["content"].as_str().unwrap();
                assert!(prompt.contains("微信引用已核对") && prompt.contains("旧合成封面"));
                assert!(!prompt.contains("新合成事项"));
                count.fetch_add(1, Ordering::SeqCst);
                return Ok(response(90, "homebase_steer_message", json!({})));
            }
            if r.messages.iter().any(|msg| msg["content"].as_str().is_some_and(|text| text.contains(CHANGE))) {
                return wire::parse("chat", &json!({"choices":[{"message":{"content":"我已核对绿色字幕的要求。"}}]}));
            }
            let original = r.messages.iter().any(|msg| msg["content"] == "为旧合成封面准备字幕");
            let title = if original {"旧合成封面"} else {"新合成事项"};
            Ok(response(1, "homebase_memory", json!({"action":"remember","revision":0,"title":title,"content":"仅供隔离测试"})))
        })
    });
    let mut settings = AssistantState::default();
    settings.settings.enabled = true;
    settings.settings.runtime = "pi".into();
    settings.settings.route_id = "synthetic-only".into();
    let save: crate::assistant::Save = Arc::new(|_| Box::pin(async { Ok(()) }));
    let engine = AssistantEngine::new(settings, save, model, template.control.clone(), root.path().into()).await.unwrap();
    let submit = |id, text| engine.channel(json!({"operation":"submit","source_key":source,"request_id":id,"text":text}));
    let old = submit("older-work", "为旧合成封面准备字幕").await.unwrap()["turn_id"].as_str().unwrap().to_owned();
    assert_eq!(until_pending(&engine).await, old);
    submit("new-chat", "新对话").await.unwrap();
    let new = submit("newer-work", "准备新合成事项").await.unwrap()["turn_id"].as_str().unwrap().to_owned();
    let newer_confirmation = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(pending) = engine.turn(&new).await.unwrap().pending { break pending.id; }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();

    let mut stored = stored;
    for (id, turn) in [("older-quote-fixture", &old), ("newer-quote-fixture", &new)] {
        stored.records.push(Record { id:id.into(), command:"assistant".into(), status:"awaiting_confirmation".into(),
            client_id:id.into(), context:"synthetic-context".into(), job_id:Some(turn.clone()), ..Default::default() });
    }
    let entered = Arc::new(Notify::new());
    let hold: Agent = Arc::new({
        let entered = entered.clone();
        move |request| {
            let entered = entered.clone();
            Box::pin(async move {
                assert_eq!(request["operation"], "submit_quoted_task");
                entered.notify_one();
                std::future::pending().await
            })
        }
    });
    let bridge = |stored, agent| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent))
    };
    let first = bridge(stored, hold);
    first.reply("older-quote-fixture", "旧合成封面正在等确认", None).await.unwrap();
    first.reply("newer-quote-fixture", "新合成事项正在等确认", None).await.unwrap();
    let incoming = quote_confirmation("quote-edit-old", "把这张图的字幕改成绿色", "9007199254741991");
    let ingress = tokio::spawn({ let first = first.clone(); let incoming = incoming.clone(); async move { first.ingest(&incoming).await } });
    tokio::time::timeout(Duration::from_secs(3), entered.notified()).await.unwrap();
    ingress.abort();
    assert!(ingress.await.unwrap_err().is_cancelled());
    let durable = saved.lock().await.clone();
    let pending = durable.records.last().unwrap();
    assert_eq!(pending.command, "message");
    assert!(pending.pending_query().is_none());
    assert_eq!(pending.pending_assistant_followup().unwrap()["turn"], old);
    let receipt = pending.id.clone();
    let reopened = bridge(serde_json::from_slice(&serde_json::to_vec(&durable).unwrap()).unwrap(), Arc::new({
        let engine = engine.clone();
        move |request| { let engine = engine.clone(); Box::pin(async move { engine.channel(request).await }) }
    }));
    reopened.submit(&receipt).await.unwrap();
    reopened.ingest(&incoming).await.unwrap();
    let followup = saved.lock().await.records.last().unwrap().job_id.clone().unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let turn = engine.turn(&followup).await.unwrap();
            if turn.state == "completed" { break turn; }
            assert_ne!(turn.state, "failed", "{}", turn.text);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    assert_eq!(completed.steered_turn_id.as_deref(), Some(old.as_str()));
    assert_eq!(engine.turn(&old).await.unwrap().requirements_revision, 1);
    let unaffected = engine.turn(&new).await.unwrap();
    assert_eq!(unaffected.requirements_revision, 0);
    assert_eq!(unaffected.pending.unwrap().id, newer_confirmation);
    assert!(engine.channel(json!({"operation":"submit_quoted_task","source_key":source,
        "request_id":receipt,"text":"把这张图的字幕改成绿色","turn_id":new})).await.is_err());
    let mut ambiguous = quote_confirmation("quote-edit-conflict", "把这张图的字幕改成绿色", "9007199254741991");
    ambiguous["item_list"][0]["ref_msg"]["message_item"] = json!({"msg_id":"9007199254741992"});
    reopened.ingest(&ambiguous).await.unwrap();
    assert_eq!(steers.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(seen.lock().await.iter().any(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"]
        .as_str().is_some_and(|text| text.contains("我没能核对这条引用"))));
    engine.cancel(&new).await.unwrap();
    server.abort();
}

#[tokio::test]
async fn simulated_wechat_chats_during_work_tracks_original_matter_and_reviews_once_after_restart()
{
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let artifact = workspace.join("dialogue-result.txt");
    let data = "HB-DIALOGUE-ISOLATED：动画素材已核对，等待最终审核。";
    std::fs::write(&artifact, data).unwrap();
    let sentinel = root.path().join("outside-sentinel.txt");
    std::fs::write(&sentinel, "KEEP ORIGINAL ISOLATED DATA").unwrap();
    let hash = format!("{:x}", Sha256::digest(data.as_bytes()));
    let effects = Arc::new(AtomicUsize::new(0));
    let executions = effects.clone();
    let sha = hash.clone();
    let file = artifact.clone();
    let entered = Arc::new(tokio::sync::Notify::new());
    let started = entered.clone();
    let release = Arc::new(tokio::sync::Notify::new());
    let gate = release.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let executions = executions.clone();
        let sha = sha.clone();
        let file = file.clone();
        let started = started.clone();
        let gate = gate.clone();
        Box::pin(async move {
            assert_eq!(command, "assistant_local_execute");
            assert_eq!(args["request"]["action"], "export");
            executions.fetch_add(1, Ordering::SeqCst);
            started.notify_one();
            gate.notified().await;
            Ok(
                json!({"status":"succeeded","artifact":{"path":file,"sha256":sha,"mime_type":"text/plain"}}),
            )
        })
    });
    let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    std::fs::create_dir_all(root.path().join("control")).unwrap();
    let controller = Controller::new(catalog, &root.path().join("control"), runner).unwrap();
    let ctl = controller.clone();
    let control: Control = Arc::new(move |r| {
        let c = ctl.clone();
        Box::pin(async move { c.handle(r).await })
    });
    let rounds = Arc::new(AtomicUsize::new(0));
    let calls = rounds.clone();
    let chats = Arc::new(AtomicUsize::new(0));
    let foreground = chats.clone();
    let discussions = Arc::new(AtomicUsize::new(0));
    let read_only = discussions.clone();
    let model: crate::assistant::Model = Arc::new(move |r: ModelRequest| {
        let calls = calls.clone();
        let chats = foreground.clone();
        let read_only = read_only.clone();
        Box::pin(async move {
            let latest = r.messages.last().unwrap()["content"].as_str().unwrap();
            if matches!(latest, "请先讨论这段视频截取的范围" | "先聊聊核对素材的方案" | "确认？") {
                assert!(r.definitions.as_ref().is_some_and(Vec::is_empty), "discussion receives no tool or route definitions");
                assert!(r.messages[0]["content"].as_str().unwrap().contains("只读讨论"));
                read_only.fetch_add(1, Ordering::SeqCst);
                if latest == "确认？" {
                    return Ok(response(97, "homebase_queue_message", json!({})));
                }
                return Ok(wire::parse("chat", &json!({"choices":[{"message":{"content":"我们先核对时间范围，再决定怎么剪。"}}]})).unwrap());
            }
            if r.definitions.as_ref().is_some_and(|defs| {
                defs.iter().any(|d|d["function"]["name"]=="homebase_queue_message")
            }) {
                chats.fetch_add(1, Ordering::SeqCst);
                assert!(
                    r.messages[0].to_string().contains("原任务")
                        || r.messages[0].to_string().contains("progress")
                );
                if r.messages.last().unwrap()["content"]
                    .as_str()
                    .unwrap()
                    .contains("再处理")
                {
                    return Ok(response(99, "homebase_queue_message", json!({})));
                }
                return Ok(wire::parse("chat",&json!({"choices":[{"message":{"content":"我在整理这次结果文件，随后会发给你。持续事项会保留，复查只提醒，不会重做。"}}]})).unwrap());
            }
            let n = calls.fetch_add(1, Ordering::SeqCst);
            let last = || {
                let m = r
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m["role"] == "tool")
                    .unwrap();
                serde_json::from_str::<Value>(m["content"].as_str().unwrap()).unwrap()
            };
            Ok(match n {
            0=>response(n,"homebase_matter",json!({"action":"save","revision":0,"title":"发布动画","goal":"持续跟进动画发布和审核","summary":"已核对素材","next_action":"等待审核后发布","state":"waiting","review_at":null})),
            1=>response(n,"homebase_matter",json!({"action":"attach","revision":1,"id":last()["id"]})),
            2=>response(n,"homebase_files",json!({"action":"export","path":"dialogue-result.txt"})),
            3=>response(n,"homebase_deliver",json!({"job_id":last()["job_id"]})),
            4=>wire::parse("chat",&json!({"choices":[{"message":{"content":"素材已经核对，结果文件已准备好，接着发给你。发布事项仍在等待审核，没有提前标成完成。"}}]})).unwrap(),
            5=>response(n,"homebase_matter",json!({"action":"list","query":"发布动画"})),
            6=>{assert_eq!(last()["entries"][0]["state"],"waiting");wire::parse("chat",&json!({"choices":[{"message":{"content":"我按约定复查了发布动画，事项仍在等审核，原文件已保留，没有重新制作。"}}]})).unwrap()},
            _=>panic!("unexpected model request"),
        })
        })
    });
    let mut initial = AssistantState::default();
    initial.settings.enabled = true;
    initial.settings.wechat_enabled = true;
    initial.settings.runtime = "pi".into();
    initial.settings.free_only = true;
    initial.settings.route_id = "openrouter::synthetic:free".into();
    let saved = Arc::new(Mutex::new(initial.clone()));
    let output = saved.clone();
    let save: crate::assistant::Save = Arc::new(move |s| {
        let o = output.clone();
        Box::pin(async move {
            *o.lock().await = s;
            Ok(())
        })
    });
    let saved_state = save.clone();
    let saved_model = model.clone();
    let saved_control = control.clone();
    let engine = AssistantEngine::new(initial, save, model, control.clone(), workspace.clone())
        .await
        .unwrap();
    const FIRST_CONFIRMATION: &str = "9007199254741030";
    const SECOND_CONFIRMATION: &str = "9007199254741031";
    let (base, seen, server) = fake_api(200, json!({"__media":true,"__send_text_prefix_ids":{
        "我准备保存这件持续跟进的事": FIRST_CONFIRMATION,
        "我准备把当前任务关联到这件事": SECOND_CONFIRMATION
    }})).await;
    let stored = account(&base);
    let (template, wx_saved) = test_bridge(stored.clone(), &workspace, controller);
    let reopen = |stored, core: Arc<AssistantEngine>| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let agent: Agent = Arc::new(move |r| {
            let e = core.clone();
            Box::pin(async move { e.channel(r).await })
        });
        Bridge::new_with_agent(stored, api, template.save.clone(), control.clone(), workspace.clone(), Some(agent))
    };
    let bridge = reopen(stored, engine.clone());
    bridge.start().await.unwrap();
    let idle_discussion = msg("dialogue-idle-plan", "请先讨论这段视频截取的范围");
    bridge.ingest(&idle_discussion).await.unwrap();
    until_delivered(&bridge, "dialogue-idle-plan").await;
    assert_eq!(discussions.load(Ordering::SeqCst), 1);
    assert_eq!(rounds.load(Ordering::SeqCst), 0);
    assert!(engine.sessions().await[0].active_turn_id.is_none());
    bridge
        .ingest(&msg(
            "dialogue-create",
            "请持续跟进发布动画，核对并返回准备好的文件",
        ))
        .await
        .unwrap();
    let original = until_pending(&engine).await;
    let before_discussion = engine.turn(&original).await.unwrap();
    let planning = msg("dialogue-pending-plan", "先聊聊核对素材的方案");
    let confirmation_question = msg("dialogue-consent-question", "确认？");
    bridge.ingest(&planning).await.unwrap();
    until_delivered(&bridge, "dialogue-pending-plan").await;
    bridge.ingest(&confirmation_question).await.unwrap();
    until_delivered(&bridge, "dialogue-consent-question").await;
    let after_discussion = engine.turn(&original).await.unwrap();
    assert_eq!(after_discussion.state, "awaiting_confirmation");
    assert_eq!(after_discussion.pending.as_ref().unwrap().id, before_discussion.pending.as_ref().unwrap().id);
    assert_eq!(after_discussion.requirements_revision, before_discussion.requirements_revision);
    assert_eq!(discussions.load(Ordering::SeqCst), 3);
    assert_eq!(rounds.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(engine.matters().await["entries"], json!([]));
    until_confirmation_receipt(&bridge, FIRST_CONFIRMATION).await;
    bridge.ingest(&quote_confirmation("dialogue-consent-1", "就这么办。", FIRST_CONFIRMATION)).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        let t = engine.turn(&original).await.unwrap();
        if t.state == "awaiting_confirmation"
            && t.pending
                .as_ref()
                .is_some_and(|p| p.description == "homebase_matter attach")
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "attachment consent missing"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    until_confirmation_receipt(&bridge, SECOND_CONFIRMATION).await;
    let second = engine.turn(&original).await.unwrap().pending.unwrap().id;
    assert_ne!(second, before_discussion.pending.unwrap().id);
    let mut stale = quote_confirmation("dialogue-stale-approval", "确认", FIRST_CONFIRMATION);
    stale["item_list"][0]["ref_msg"]["title"] = json!(format!("确认 {second}"));
    bridge.ingest(&stale).await.unwrap();
    assert!(seen.lock().await.iter().any(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().is_some_and(|text| text.contains("这份确认已失效"))));
    let mut conflict = quote_confirmation("dialogue-conflicting-approval", "就这么办", SECOND_CONFIRMATION);
    conflict["item_list"][0]["ref_msg"]["message_item"] = json!({"type":1,"msg_id":FIRST_CONFIRMATION});
    bridge.ingest(&conflict).await.unwrap();
    let unmapped = quote_confirmation("dialogue-unverified-approval", "按这个来", "9007199254741032");
    bridge.ingest(&unmapped).await.unwrap();
    let refreshed = engine.turn(&original).await.unwrap();
    assert_eq!(refreshed.state, "awaiting_confirmation");
    assert_eq!(refreshed.pending.unwrap().id, second);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    bridge.stop().await.unwrap();
    let mut legacy = serde_json::to_value(&*wx_saved.lock().await).unwrap();
    for record in legacy["records"].as_array_mut().unwrap() {
        let record = record.as_object_mut().unwrap();
        record.remove("outbound_quote_keys");
        record.remove("confirmation_quotes");
    }
    let mut snapshot = serde_json::to_vec(&legacy).unwrap();
    #[cfg(windows)] {
        let sealed = crate::wechat_crypto::crypt(&snapshot, true).unwrap();
        snapshot = crate::wechat_crypto::crypt(&sealed, false).unwrap().to_vec();
    }
    let mut restored_store: Stored = serde_json::from_slice(&snapshot).unwrap();
    restored_store.enabled = true;
    let bridge = reopen(restored_store, engine.clone());
    bridge.start().await.unwrap();
    let confirm = quote_confirmation("dialogue-consent-2", "按这个来！", SECOND_CONFIRMATION);
    bridge.ingest(&confirm).await.unwrap();
    bridge.ingest(&confirm).await.unwrap();
    tokio::time::timeout(Duration::from_secs(40), entered.notified())
        .await
        .unwrap();
    let already_handled = quote_confirmation("dialogue-handled-approval", "就这么办", SECOND_CONFIRMATION);
    bridge.ingest(&already_handled).await.unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    bridge
        .ingest(&msg("dialogue-chat", "这一步大概在处理什么呢？"))
        .await
        .unwrap();
    until_delivered(&bridge, "dialogue-chat").await;
    assert_ne!(engine.turn(&original).await.unwrap().state, "completed");
    bridge
        .ingest(&msg("dialogue-progress", "进展怎么样？"))
        .await
        .unwrap();
    until_delivered(&bridge, "dialogue-progress").await;
    bridge
        .ingest(&msg("dialogue-queued-action", "再处理一份字幕，按原队列来"))
        .await
        .unwrap();
    let queued_id = format!(
        "wx-{:x}",
        Sha256::digest(b"synthetic-bot:synthetic-owner:dialogue-queued-action")
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let queued_turn = loop {
        if let Some(r) = bridge.status().await.tasks.iter().find(|r| {
            r.id == queued_id && r.status == "waiting_input_queue" && r.delivery == "accepted"
        }) {
            break r.job_id.clone().unwrap();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no concrete acknowledgement after auto routing to the original queue"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    engine.cancel(&queued_turn).await.unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    release.notify_one();
    until_delivered(&bridge, "dialogue-consent-2").await;
    bridge.ingest(&msg("dialogue-new", "新对话")).await.unwrap();
    bridge
        .ingest(&msg("dialogue-named-status", "进度发布动画"))
        .await
        .unwrap();
    until_delivered(&bridge, "dialogue-named-status").await;
    bridge
        .ingest(&msg("dialogue-named-result", "结果发布动画"))
        .await
        .unwrap();
    until_delivered(&bridge, "dialogue-named-result").await;
    assert_eq!(engine.matters().await["entries"][0]["state"], "waiting");
    assert_eq!(
        engine.matters().await["entries"][0]["latest_task"]["turn_id"],
        original
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(rounds.load(Ordering::SeqCst), 5);
    assert_eq!(chats.load(Ordering::SeqCst), 2);
    let matters = engine.matters().await;
    let e = &matters["entries"][0];
    engine
        .matter_save(MatterInput {
            revision: matters["revision"].as_u64().unwrap(),
            id: Some(e["id"].as_str().unwrap().into()),
            title: "发布动画".into(),
            goal: "持续跟进动画发布和审核".into(),
            summary: "已核对素材".into(),
            next_action: "等待审核后发布".into(),
            state: "waiting".into(),
            review_at: Some(
                (chrono::Utc::now() + chrono::Duration::milliseconds(150)).to_rfc3339(),
            ),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(220)).await;
    bridge.deliver_reminders().await;
    bridge.deliver_reminders().await;
    let matters = engine.matters().await;
    engine
        .followup_save(
            FollowupInput {
                trigger:None,notification_policy:None,
                revision: engine.followups().await["revision"].as_u64().unwrap(),
                title: "发布动画后续复查".into(),
                instruction: "只读取发布动画事项并报告进展，不重做原文件，不修改进展笔记".into(),
                due_at: (chrono::Utc::now() + chrono::Duration::milliseconds(250)).to_rfc3339(),
                interval_minutes: None,
                max_runs: 1,
                matter_id: Some(matters["entries"][0]["id"].as_str().unwrap().into()),
            },
            "wechat",
            &bridge.source_key().await,
        )
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        bridge.deliver_reminders().await;
        if bridge.status().await.tasks.iter().any(|r| {
            r.id.starts_with("hb-proactive-") && r.status == "succeeded" && r.delivery == "accepted"
        }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "linked scheduled check not delivered"
        );
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert_eq!(
        engine.matters().await["entries"][0]["turn_ids"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    bridge
        .ingest(&msg("dialogue-result-after-review", "结果发布动画"))
        .await
        .unwrap();
    until_delivered(&bridge, "dialogue-result-after-review").await;
    assert_eq!(
        effects.load(Ordering::SeqCst),
        1,
        "reading old result must not repeat export"
    );
    assert_eq!(rounds.load(Ordering::SeqCst), 7);
    let rows = seen.lock().await.clone();
    let chat_id = format!(
        "wx-{:x}",
        Sha256::digest(b"synthetic-bot:synthetic-owner:dialogue-chat")
    );
    let client = {
        let state = wx_saved.lock().await;
        state
            .records
            .iter()
            .find(|r| r.id == chat_id)
            .unwrap()
            .client_id
            .clone()
    };
    let client = client.strip_suffix("-update").unwrap_or(&client);
    let chat_replies: Vec<_> = rows
        .iter()
        .filter(|r| {
            r["body"]["msg"]["client_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(client))
        })
        .collect();
    assert_eq!(
        chat_replies.len(),
        1,
        "short discussion should send its answer without a second generic receipt"
    );
    assert!(chat_replies[0]["body"]["msg"]
        .to_string()
        .contains("整理这次结果文件"));
    let review_count = rows
        .iter()
        .filter(|r| r["body"]["msg"].to_string().contains("该回顾一下"))
        .count();
    assert_eq!(review_count, 1);
    assert!(rows
        .iter()
        .any(|r| r["body"]["msg"]["item_list"][0]["type"] == 4));
    let state = saved.lock().await.clone();
    let restored = AssistantEngine::new(
        state,
        saved_state,
        saved_model,
        saved_control,
        workspace.clone(),
    )
    .await
    .unwrap();
    bridge.stop().await.unwrap();
    let mut stored = wx_saved.lock().await.clone();
    stored.enabled = true;
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let core = restored.clone();
    let agent: Agent = Arc::new(move |r| {
        let e = core.clone();
        Box::pin(async move { e.channel(r).await })
    });
    let restarted = Bridge::new_with_agent(
        stored,
        api,
        bridge.save.clone(),
        bridge.control.clone(),
        workspace,
        Some(agent),
    );
    restarted.start().await.unwrap();
    assert!(restarted.status().await.enabled);
    restarted.ingest(&idle_discussion).await.unwrap();
    restarted.ingest(&planning).await.unwrap();
    restarted.ingest(&confirmation_question).await.unwrap();
    assert_eq!(discussions.load(Ordering::SeqCst), 3);
    restarted.deliver_reminders().await;
    restarted.deliver_reminders().await;
    assert_eq!(
        seen.lock()
            .await
            .iter()
            .filter(|r| r["body"]["msg"].to_string().contains("该回顾一下"))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_to_string(sentinel).unwrap(),
        "KEEP ORIGINAL ISOLATED DATA"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(
        rounds.load(Ordering::SeqCst),
        7,
        "restart must not repeat scheduled check"
    );
    assert_eq!(
        restored.matters().await["entries"][0]["delivery"],
        "accepted"
    );
    restarted.stop().await.unwrap();
    server.abort();
}
