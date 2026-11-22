//! Isolated conversation, progress and ongoing-responsibility regression.
use super::*;
use workflow_tests::{task_wait, tool_reply};
fn matter_input(revision: u64) -> MatterInput {
    MatterInput {
        revision,
        id: None,
        title: "发布动画".into(),
        goal: "持续跟进动画与审核".into(),
        summary: "已核对素材".into(),
        next_action: "等审核后制作".into(),
        state: "waiting".into(),
        review_at: Some((chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339()),
    }
}
async fn msg(e: &Arc<Engine>, rid: &str, text: &str) -> Value {
    e.channel(
        json!({"operation":"submit","source_key":"synthetic-owner","request_id":rid,"text":text}),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn resolve_current_wechat_query_is_read_only_and_source_bound() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    engine.mutate(Change::default(), |state| {
        state.settings.wechat_enabled = true;
        Ok(())
    }).await.unwrap();
    let original = msg(&engine, "current-query-fixture", "隔离任务").await;
    let before = {
        let state = engine.state.lock().await;
        (state.sessions.len(), state.channel_controls.len())
    };
    for (owner, expected) in [
        ("synthetic-owner", original["turn_id"].clone()),
        ("other-owner", Value::Null),
    ] {
        let response = engine.channel(json!({"operation":"resolve_current_query","source_key":owner})).await.unwrap();
        assert_eq!(response["target"], expected);
    }
    let after = engine.state.lock().await;
    assert_eq!((after.sessions.len(), after.channel_controls.len()), before);
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn quoted_completed_work_binds_the_selected_file_and_cannot_steer_another_task() {
    let (engine, models, effects, _, root) = fixture(false, vec![]).await;
    let a = msg(&engine, "finished-quote", "为产品卡生成两份素材").await;
    let b = msg(&engine, "unrelated-current", "正在准备不相关的文字稿").await;
    let a_id = a["turn_id"].as_str().unwrap();
    let b_id = b["turn_id"].as_str().unwrap();
    let path = root.path().join("selected.png").to_string_lossy().into_owned();
    let sha256 = format!("{:x}", Sha256::digest(b"same bytes in two fixtures"));
    engine.mutate(Change::default(), |state| {
        let (s, a) = locate(state, a_id)?;
        let (_, b) = locate(state, b_id)?;
        let session = &mut state.sessions[s];
        session.turns[a].state = "completed".into();
        session.turns[b].state = "awaiting_confirmation".into();
        let files = ["other.png", "selected.png"].into_iter().map(|name| Artifact {
            id:name.into(), job_id:"quote-job".into(), name:name.into(),
            path:root.path().join(name).to_string_lossy().into_owned(), sha256:sha256.clone(),
            bytes:25, mime_type:"image/png".into(),
        }).collect();
        session.jobs.push(Job { id:"quote-job".into(), command:"model_capability_call".into(),
            turn_id:a_id.into(), state:"succeeded".into(), result:Value::Null, owned:true, deliver:false,
            artifacts:files, remote:None, archived_result_bytes:None });
        Ok(())
    }).await.unwrap();
    let file = json!({"digest":sha256,"path_digest":format!("{:x}", Sha256::digest(path.as_bytes()))});
    let request = json!({"operation":"submit_quoted_task","source_key":"synthetic-owner","request_id":"quote-new-version",
        "turn_id":a_id,"text":"用这份图片的风格做个新版本","file":file});
    let ack = engine.channel(request.clone()).await.unwrap();
    let tid = ack["turn_id"].as_str().unwrap();
    assert_eq!(engine.channel(request.clone()).await.unwrap()["turn_id"], tid);
    engine.inspect_turn(tid, |_, turn| {
        assert_eq!(turn.parent_turn_id.as_deref(), Some(a_id));
        assert_eq!(turn.interaction, "task");
        assert!(!turn.route_work);
        assert!(!turn.runtime_prompt.contains(&path));
        let marked = turn.runtime_prompt.lines().last().unwrap();
        let context: Value = serde_json::from_str(marked).unwrap();
        assert_eq!(context["id"], a_id);
        assert_eq!(context["selected_artifact"]["id"], "selected.png");
        assert_eq!(context["selected_artifact"]["job_id"], "quote-job");
    }).await.unwrap();
    let mut wrong_file = request.clone();
    wrong_file["request_id"] = json!("unverified-file");
    wrong_file["file"]["path_digest"] = json!("a".repeat(64));
    assert!(engine.channel(wrong_file).await.unwrap_err().contains("原附件"));
    let mut wrong_owner = request;
    wrong_owner["source_key"] = json!("other-owner");
    assert!(engine.channel(wrong_owner).await.is_err());
    assert_eq!(engine.turn(b_id).await.unwrap().requirements_revision, 0);
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[test]
fn discussion_detection_never_treats_actions_or_questions_as_consent() {
    assert!(dialogue::is_chat("为什么这样安排？", None, true).unwrap());
    assert!(dialogue::is_chat("聊聊：另一件事", None, false).unwrap());
    assert!(dialogue::is_chat("删除全部文件", Some("chat"), false).unwrap());
    assert!(!dialogue::is_chat("请修改字幕", None, true).unwrap());
    assert!(!dialogue::is_chat("为什么失败后请重做", Some("task"), true).unwrap());
    for text in ["先讨论方案", "先聊聊剪辑方案", "请先讨论一下", "帮我只聊聊", "确认？", "同意执行?"] {
        assert!(dialogue::is_chat(text, None, false).unwrap(), "{text}");
        assert!(dialogue::is_chat(text, None, true).unwrap(), "{text}");
    }
    assert!(!dialogue::is_chat("先讨论方案", Some("task"), false).unwrap());
    assert!(!dialogue::is_chat("确认执行", None, true).unwrap());
    assert!(dialogue::is_chat("hello", Some("invalid"), true).is_err());
}

#[tokio::test]
async fn desktop_dialogue_stream_previews_through_loopback_and_preserves_original_receipt() {
    check_desktop_dialogue_stream(crate::model_capability_io::dialogue_stream::Protocol::Chat).await;
}

#[tokio::test]
async fn desktop_responses_stream_previews_through_loopback_and_preserves_original_receipt() {
    check_desktop_dialogue_stream(crate::model_capability_io::dialogue_stream::Protocol::Responses).await;
}

async fn check_desktop_dialogue_stream(protocol: crate::model_capability_io::dialogue_stream::Protocol) {
    use crate::model_capability_io::dialogue_stream::{test_frames, Protocol};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    for outcome in ["completed", "failed", "canceled"] {
        let (base, _, _, saved, _root) = fixture(false, vec![]).await;
        let mut state = saved.lock().await.clone();
        state.settings.enabled = true;
        let model: Model = Arc::new(|_| Box::pin(async { panic!("a started stream must not fall back") }));
        let engine = Engine::new(
            state, base.save.clone(), model, base.control.clone(), base.workspace.clone(),
        ).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (path, provider, prefix, suffix): (_, _, String, String) = match protocol {
            Protocol::Chat => ("/chat/completions", "openrouter",
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"合成前缀\",\"reasoning_content\":\"private-reasoning\"}}]}\n\n".into(),
                concat!("data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"与结尾\"},\"finish_reason\":\"stop\"}]}\n\n", "data: [DONE]\n\n").into(),
            ),
            Protocol::Responses => ("/v1/responses", "openai",
                test_frames::prefix().iter().map(test_frames::frame).collect(),
                [test_frames::delta(7, "与结尾"), test_frames::completed(8, "合成前缀与结尾")]
                    .iter().map(test_frames::frame).collect(),
            ),
            Protocol::ChatTools | Protocol::ResponsesTools => unreachable!("tool rounds are tested separately"),
        };
        let url = format!("http://{}{path}", listener.local_addr().unwrap());
        let release = Arc::new(Notify::new());
        let resume = release.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
                assert!(request.len() <= 4096);
            }
            assert!(request.starts_with(format!("POST {path} ").as_bytes()));
            let headers = std::str::from_utf8(&request).unwrap();
            let length = headers.lines().find_map(|line| {
                line.to_ascii_lowercase().strip_prefix("content-length: ")?.parse::<usize>().ok()
            }).unwrap();
            assert!(length <= 32_768);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["stream"], true);
            assert!(body.get("tools").is_none());
            if matches!(protocol, Protocol::Responses) {
                assert_eq!(body["store"], false);
                assert!(body["input"].is_array());
                assert!(body.get("include").is_none());
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
            socket.write_all(prefix.as_bytes()).await.unwrap();
            resume.notified().await;
            if outcome == "completed" {
                socket.write_all(suffix.as_bytes()).await.unwrap();
            }
            socket.shutdown().await.ok();
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let notices = Arc::new(Notify::new());
        let announcement = notices.clone();
        engine.configure_dialogue_stream(
            Arc::new(move |mut request, publish| {
                assert!(request.definitions.as_ref().unwrap().is_empty());
                observed.fetch_add(1, Ordering::SeqCst);
                let url = url.clone();
                Box::pin(async move {
                    let protocol_name = match protocol { Protocol::Chat | Protocol::ChatTools => "chat", Protocol::Responses | Protocol::ResponsesTools => "responses" };
                    // Only synthetic requests are sent to the local fixture. The independent route
                    // test checks that the production API guard refuses OpenAI in free-only mode.
                    request.free_only = false;
                    let mut body = crate::assistant_protocol::body(protocol_name, provider, &request)?;
                    protocol.enable(provider, &mut body);
                    let response = crate::model_capability_io::client()?.post(url).json(&body).send().await.unwrap();
                    protocol.read(response, |text| publish(text)).await.map(Some)
                })
            }),
            Arc::new(move |_, _| announcement.notify_one()),
        ).unwrap();
        let mut input = request("讨论合成问题", "streamed-original-request");
        input.mode = Some("chat".into());
        let ack = engine.send(input.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), notices.notified()).await.unwrap();
        let preview = engine.turn_with_progress(&ack.turn_id).await.unwrap();
        assert_eq!(preview.state, "running");
        assert_eq!(preview.live_progress.unwrap()["output"], "合成前缀");
        let progress = engine.observe_turn(&ack.turn_id).await.unwrap();
        assert_eq!(progress.details_omitted, Some(true));
        assert_eq!(progress.live_progress.unwrap()["output"], "合成前缀");
        assert!(!serde_json::to_string(&*saved.lock().await).unwrap().contains("合成前缀"));
        if outcome == "failed" {
            let in_flight = saved.lock().await.clone();
            let restarted = Engine::new(
                in_flight,
                Arc::new(|_| Box::pin(async { Ok(()) })),
                Arc::new(|_| Box::pin(async { panic!("restart must not replay an in-flight chat") })),
                base.control.clone(),
                base.workspace.join("restarted-dialogue"),
            ).await.unwrap();
            let recovered = restarted.turn_with_progress(&ack.turn_id).await.unwrap();
            assert_eq!(recovered.state, "interrupted_unknown");
            assert!(recovered.live_progress.is_none());
            assert_eq!(restarted.send(input.clone()).await.unwrap().turn_id, ack.turn_id);
        }
        if outcome == "canceled" {
            engine.cancel(&ack.turn_id).await.unwrap();
        }
        release.notify_one();
        let final_view = wait(&engine, &ack.turn_id, outcome).await;
        assert!(engine.turn_with_progress(&ack.turn_id).await.unwrap().live_progress.is_none());
        if outcome == "completed" {
            assert_eq!(final_view.text, "合成前缀与结尾");
            assert!(!serde_json::to_string(&*saved.lock().await).unwrap().contains("private-reasoning"));
        } else {
            assert!(!final_view.text.contains("合成前缀"));
        }
        assert_eq!(engine.send(input).await.unwrap().turn_id, ack.turn_id);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn desktop_deepseek_task_stream_preserves_tool_rounds_and_never_replays_a_truncated_call() {
    check_desktop_task_stream(false).await;
}

#[tokio::test]
async fn desktop_openai_task_stream_preserves_tool_rounds_and_never_replays_a_truncated_call() {
    check_desktop_task_stream(true).await;
}

async fn check_desktop_task_stream(responses: bool) {
    use crate::model_capability_io::dialogue_stream::{test_frames, Protocol};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let (provider, wire, path, route_id) = if responses {
        ("openai", "responses", "/v1/responses", "openai::gpt-5-mini")
    } else {
        ("deepseek", "chat", "/chat/completions", "deepseek::deepseek-flash")
    };
    let mut routes = crate::model_capabilities::seed_models();
    routes.extend(crate::model_capability_io::discover(
        "openai", &json!({"data":[{"id":"gpt-5-mini"}]}), "https://api.openai.com/v1/models",
    ).unwrap());
    let route = routes.into_iter().find(|route| route.route_id == route_id).unwrap();
    for complete in [true, false] {
        let (base, _, effects, saved, _root) = fixture(false, vec![]).await;
        let mut state = saved.lock().await.clone();
        state.settings.enabled = true;
        state.settings.free_only = false;
        state.settings.runtime = "pi".into();
        state.settings.route_id = route_id.into();
        let model: Model = Arc::new(|_| Box::pin(async { panic!("a started task stream must not fall back") }));
        let engine = Engine::new(state, base.save.clone(), model, base.control.clone(), base.workspace.clone()).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{path}", listener.local_addr().unwrap());
        let release = Arc::new(Notify::new());
        let resume = release.clone();
        let server = tokio::spawn(async move {
            for round in 0..(if complete { 2 } else { 1 }) {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    head.push(socket.read_u8().await.unwrap());
                    assert!(head.len() <= 4096);
                }
                assert!(head.starts_with(format!("POST {path} ").as_bytes()));
                let length = std::str::from_utf8(&head).unwrap().lines().find_map(|line| {
                    line.to_ascii_lowercase().strip_prefix("content-length: ")?.parse::<usize>().ok()
                }).unwrap();
                assert!(length <= 128 * 1024);
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["stream"], true);
                assert!(!body["tools"].as_array().unwrap().is_empty());
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await.unwrap();
                if responses {
                    assert_eq!(body["store"], false);
                    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
                    if round == 0 {
                        let events = test_frames::tool_round();
                        for event in &events[..8] {
                            socket.write_all(test_frames::frame(event).as_bytes()).await.unwrap();
                        }
                        resume.notified().await;
                        let end = events.len() - usize::from(!complete);
                        for event in &events[8..end] {
                            socket.write_all(test_frames::frame(event).as_bytes()).await.unwrap();
                        }
                    } else {
                        let items = body["input"].as_array().unwrap();
                        assert!(items.iter().any(|item| item["type"] == "reasoning"
                            && item["encrypted_content"] == "synthetic-encrypted-context"));
                        for kind in ["function_call", "function_call_output"] {
                            assert!(items.iter().any(|item| item["type"] == kind
                                && item["call_id"] == "original-tool-call"));
                        }
                        let result = "已保存合成偏好。";
                        let mut events = test_frames::prefix();
                        events[5]["delta"] = json!(result);
                        for event in events.into_iter().chain([test_frames::completed(7, result)]) {
                            socket.write_all(test_frames::frame(&event).as_bytes()).await.unwrap();
                        }
                    }
                } else if round == 0 {
                    let prefix = json!({"id":"synthetic-round-1","choices":[{"index":0,"delta":{
                        "content":"正在核对合成偏好。","reasoning_content":"synthetic-provider-metadata",
                        "tool_calls":[{"index":0,"id":"original-tool-call","type":"function","function":{
                            "name":"homebase_memory","arguments":"{\"action\":\"remember\",\"revision\":0,\"title\":\"合成偏好\",\"content\":\""}}]
                    }}]});
                    socket.write_all(format!("data: {prefix}\n\n").as_bytes()).await.unwrap();
                    resume.notified().await;
                    let suffix = json!({"id":"synthetic-round-1","choices":[{"index":0,"delta":{
                        "tool_calls":[{"index":0,"function":{"arguments":"蓝点\"}"}}]},"finish_reason":"tool_calls"}]});
                    socket.write_all(format!("data: {suffix}\n\n").as_bytes()).await.unwrap();
                    if complete { socket.write_all(b"data: [DONE]\n\n").await.unwrap(); }
                } else {
                    let messages = body["messages"].as_array().unwrap();
                    let assistant = messages.iter().find(|m| m["tool_calls"][0]["id"] == "original-tool-call").unwrap();
                    assert_eq!(assistant["reasoning_content"], "synthetic-provider-metadata");
                    assert!(messages.iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "original-tool-call"));
                    let result = json!({"id":"synthetic-round-2","choices":[{"index":0,"delta":{"content":"已保存合成偏好。"},"finish_reason":"stop"}]});
                    socket.write_all(format!("data: {result}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
                }
                socket.shutdown().await.unwrap();
            }
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let update = Arc::new(Notify::new());
        let announced = update.clone();
        let route = route.clone();
        engine.configure_dialogue_stream(
            Arc::new(move |request, publish| {
                let url = url.clone();
                let route = route.clone();
                observed.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    assert_eq!(route.route_id, request.route_id);
                    let protocol = Protocol::for_route(&route, &request).unwrap();
                    assert_eq!(matches!(protocol, Protocol::ResponsesTools), responses);
                    let mut body = crate::assistant_protocol::body(wire, provider, &request)?;
                    protocol.enable(provider, &mut body);
                    let response = crate::model_capability_io::client()?.post(url).json(&body).send().await.unwrap();
                    protocol.read(response, |text| publish(text)).await.map(Some)
                })
            }),
            Arc::new(move |_, _| announced.notify_one()),
        ).unwrap();

        let input = request("保存合成偏好", "task-stream-original");
        let ack = engine.send(input.clone()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(10), update.notified()).await.unwrap();
        let view = engine.observe_turn(&ack.turn_id).await.unwrap();
        assert_eq!(view.live_progress.unwrap(),json!({"state":"task_stream","output":"正在核对合成偏好。"}));
        assert!(view.pending.is_none());
        assert!(engine.personal().await["memories"].as_array().unwrap().is_empty());
        assert!(!serde_json::to_string(&*saved.lock().await).unwrap().contains("正在核对合成偏好。"));
        if !complete {
            let in_flight = saved.lock().await.clone();
            let recovered = Engine::new(
                in_flight,
                Arc::new(|_| Box::pin(async { Ok(()) })),
                Arc::new(|_| Box::pin(async { panic!("restart must retain the unknown model request") })),
                base.control.clone(),
                base.workspace.join("task-stream-recovery"),
            ).await.unwrap();
            let original = recovered.observe_turn(&ack.turn_id).await.unwrap();
            assert_eq!(original.state, "interrupted_unknown");
            assert!(original.live_progress.is_none());
            assert_eq!(recovered.send(input.clone()).await.unwrap().turn_id, ack.turn_id);
        }
        release.notify_one();
        if complete {
            let pending = wait(&engine,&ack.turn_id,"awaiting_confirmation").await.pending.unwrap();
            assert_eq!(pending.arguments["content"],"蓝点");
            assert!(engine.observe_turn(&ack.turn_id).await.unwrap().live_progress.is_none());
            engine.confirm(&ack.turn_id,&pending.id,true).await.unwrap();
            let finished = wait(&engine,&ack.turn_id,"completed").await;
            assert_eq!(finished.text,"已保存合成偏好。");
            assert_eq!(engine.personal().await["memories"].as_array().unwrap().len(),1);
        } else {
            let failed = wait(&engine,&ack.turn_id,"failed").await;
            assert!(failed.model_outcome_unknown);
            assert!(failed.pending.is_none() && failed.steps.is_empty());
            assert!(engine.personal().await["memories"].as_array().unwrap().is_empty());
        }
        assert!(engine.observe_turn(&ack.turn_id).await.unwrap().live_progress.is_none());
        assert_eq!(engine.send(input).await.unwrap().turn_id,ack.turn_id);
        assert_eq!(effects.load(Ordering::SeqCst),0);
        assert_eq!(calls.load(Ordering::SeqCst),if complete {2} else {1});
        tokio::time::timeout(Duration::from_secs(5),server).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn discussion_completes_while_original_waits_and_never_steals_confirmation_or_status() {
    let (e, _, effects, _, _root) = fixture(
        true,
        vec![
            reply(vec![(
                "homebase_memory",
                json!({"action":"remember","revision":0,"title":"临时合成","content":"隔离偏好"}),
            )]),
            reply(vec![]),
        ],
    )
    .await;
    let a = msg(&e, "main", "保存合成偏好").await;
    let tid = a["turn_id"].as_str().unwrap();
    let before = wait(&e, tid, "awaiting_confirmation").await;
    let b = msg(&e, "chat", "为什么要确认？").await;
    let chat = b["turn_id"].as_str().unwrap();
    let result = wait(&e, chat, "completed").await;
    assert_eq!(result.interaction, "chat");
    assert_eq!(result.parent_turn_id.as_deref(), Some(tid));
    assert!(result.jobs.is_empty());
    assert!(result.steps.is_empty());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let unchanged = e.turn(tid).await.unwrap();
    assert_eq!(unchanged.state, "awaiting_confirmation");
    assert_eq!(unchanged.pending.unwrap().id, before.pending.unwrap().id);
    let status = msg(&e, "status", "进度").await;
    assert_eq!(status["turn_id"], tid);
    assert_eq!(msg(&e, "chat", "为什么要确认？").await["turn_id"], chat);
    assert_eq!(e.personal().await["memories"].as_array().unwrap().len(), 0);
    assert_eq!(e.sessions().await[0].active_turn_id.as_deref(), Some(tid));
}
#[tokio::test]
async fn discussion_has_no_tools_even_when_model_attempts_execution_and_mode_replay_is_bound() {
    let (e, _, effects, saved, _root) = fixture(false, vec![]).await;
    let mut state = saved.lock().await.clone();
    state.settings.enabled = true;
    let model: Model = Arc::new(|r| {
        Box::pin(async move {
            assert!(r.definitions.unwrap().is_empty());
            assert!(r
                .messages
                .iter()
                .any(|m| m.to_string().contains("只读讨论")));
            Ok(reply(vec![(
                "homebase_video",
                json!({"title":"must not render"}),
            )]))
        })
    });
    let e = Engine::new(
        state,
        e.save.clone(),
        model,
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    let mut input = request("执行一个视频", "chat-safety");
    input.mode = Some("chat".into());
    let a = e.send(input.clone()).await.unwrap();
    let t = wait(&e, &a.turn_id, "completed").await;
    assert!(t.text.contains("没有执行任何操作"));
    assert!(t.jobs.is_empty());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(e.send(input.clone()).await.unwrap().turn_id, a.turn_id);
    input.mode = Some("task".into());
    assert!(e.send(input).await.is_err());
}
#[tokio::test]
async fn auto_conversation_answers_freeform_questions_and_only_queues_original_action_once() {
    let (base, _, effects, saved, _root) = fixture(
        true,
        vec![reply(vec![(
            "homebase_memory",
            json!({"action":"remember","revision":0,"title":"合成偏好","content":"不自动批准"}),
        )])],
    )
    .await;
    let a = msg(&base, "router-main", "保存合成偏好").await;
    let original = a["turn_id"].as_str().unwrap();
    wait(&base, original, "awaiting_confirmation").await;
    let state = saved.lock().await.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let n = calls.clone();
    let model: Model = Arc::new(move |r| {
        let n = n.clone();
        Box::pin(async move {
            n.fetch_add(1, Ordering::SeqCst);
            let text = r.messages.last().unwrap()["content"].as_str().unwrap();
            let defs = r.definitions.unwrap();
            assert_eq!(defs.len(), 2);
            assert_eq!(defs[0]["function"]["name"], "homebase_queue_message");
            assert_eq!(defs[1]["function"]["name"], "homebase_steer_message");
            assert!(r.messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("下一步需要你确认"));
            Ok(if text.contains("字幕") {
                tool_reply(0, "homebase_queue_message", json!({}))
            } else {
                wire::parse("chat",&json!({"choices":[{"message":{"content":"现在还在等你确认保存偏好。这样可以避免记错；当前工作没有变化。"}}]})).unwrap()
            })
        })
    });
    let e = Engine::new(
        state,
        base.save.clone(),
        model,
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let b = msg(&e, "freeform-question", "能说说目前安排的理由吗？").await;
    let t = wait(&e, b["turn_id"].as_str().unwrap(), "completed").await;
    assert_eq!(t.interaction, "chat");
    assert!(t.text.contains("等你确认"));
    let c = msg(&e, "freeform-action", "把字幕改成英文").await;
    let queued = c["turn_id"].as_str().unwrap();
    wait(&e, queued, "waiting_input_queue").await;
    assert_eq!(e.turn(queued).await.unwrap().interaction, "task");
    assert!(e.turn(queued).await.unwrap().steps.is_empty());
    assert_eq!(
        e.turn(original).await.unwrap().state,
        "awaiting_confirmation"
    );
    assert_eq!(
        msg(&e, "freeform-action", "把字幕改成英文").await["turn_id"],
        queued
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn queued_discussion_runs_after_previous_chat_without_waiting_for_the_main_task() {
    let (base, _, _, saved, _root) = fixture(
        true,
        vec![reply(vec![(
            "homebase_memory",
            json!({"action":"remember","revision":0,"title":"合成等待","content":"等待确认"}),
        )])],
    )
    .await;
    let a = msg(&base, "concurrent-main", "保存合成偏好").await;
    let original = a["turn_id"].as_str().unwrap();
    wait(&base, original, "awaiting_confirmation").await;
    let state = saved.lock().await.clone();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let gate = release.clone();
    let start = entered.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let n = calls.clone();
    let model: Model = Arc::new(move |r| {
        let gate = gate.clone();
        let start = start.clone();
        let n = n.clone();
        Box::pin(async move {
            assert!(r.definitions.unwrap().is_empty());
            if n.fetch_add(1, Ordering::SeqCst) == 0 {
                start.notify_one();
                gate.notified().await;
            }
            Ok(reply(vec![]))
        })
    });
    let e = Engine::new(
        state,
        base.save.clone(),
        model,
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let one = msg(&e, "chat-one", "聊聊制作安排").await;
    entered.notified().await;
    let two = msg(&e, "chat-two", "聊聊配色").await;
    assert_eq!(two["state"], "waiting_input_queue");
    release.notify_one();
    wait(&e, one["turn_id"].as_str().unwrap(), "completed").await;
    wait(&e, two["turn_id"].as_str().unwrap(), "completed").await;
    assert_eq!(
        e.turn(original).await.unwrap().state,
        "awaiting_confirmation"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn discussion_context_is_source_bound_and_model_unknown_is_not_replayed_on_restart() {
    let (e, _, effects, saved, _root) = fixture(false, vec![]).await;
    let mut state = saved.lock().await.clone();
    state.settings.enabled = true;
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let model: Model = Arc::new(move |_| {
        n.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err("synthetic unknown".into()) })
    });
    let e = Engine::new(
        state,
        e.save.clone(),
        model,
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    let a = msg(&e, "unknown", "聊聊今天的工作").await;
    let tid = a["turn_id"].as_str().unwrap();
    wait(&e, tid, "failed").await;
    assert!(e
        .channel(json!({"operation":"get","source_key":"other-owner","turn_id":tid}))
        .await
        .is_err());
    let state = saved.lock().await.clone();
    let e = Engine::new(
        state,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(msg(&e, "unknown", "聊聊今天的工作").await["turn_id"], tid);
    assert!(e.resume(tid).await.is_err());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn evidence_progress_names_finished_current_blocked_and_next_without_new_execution() {
    let (e, _, effects, _, _root) = fixture(false, vec![]).await;
    let a = e
        .send(request("制作发布动画", "progress-proof"))
        .await
        .unwrap();
    e.change_turn(&a.turn_id, |s, t| {
        let turn = &mut s.turns[t];
        turn.state = "running".into();
        turn.workflow.set_plan(
            0,
            "制作发布动画".into(),
            serde_json::from_value(json!([
                {"id":"a","title":"核对素材","done_when":"读到真实资料","kind":"inspect"},
                {"id":"b","title":"渲染动画","done_when":"产物解码成功","kind":"artifact"},
                {"id":"c","title":"返回文件","done_when":"安排附件交付","kind":"deliver"}
            ]))
            .unwrap(),
        )?;
        turn.steps.push(Step {
            id: "fixture-read-input".into(),
            name: "homebase_files".into(),
            state: "completed".into(),
            arguments: json!({"request":{"action":"read","path":"fixture-input.txt"}}),
            result: json!({"status":"succeeded","content":"合成资料"}),
            job_id: None,
        });
        turn.workflow.update(1, "a", "completed", String::new(), vec!["fixture-read-input".into()], &turn.steps, &s.jobs)?;
        turn.workflow.update(2, "b", "in_progress", String::new(), vec![], &turn.steps, &s.jobs)?;
        Ok(())
    })
    .await
    .unwrap();
    let v = e.channel_view("desktop", &a.turn_id, false).await.unwrap();
    let text = v["result"]["text"].as_str().unwrap();
    assert!(text.contains("核对素材"));
    assert!(text.contains("正在做：渲染动画"));
    assert!(text.contains("下一步：返回文件"));
    assert!(text.contains("1/3"));
    assert!(!text.contains('%'));
    e.change_turn(&a.turn_id, |s, t| {
        let turn = &mut s.turns[t];
        turn.workflow.update(3, "b", "blocked", "缺少审核后的文案".into(), vec![], &turn.steps, &s.jobs)
    })
    .await
    .unwrap();
    let v = e.channel_view("desktop", &a.turn_id, false).await.unwrap();
    assert!(v["result"]["text"]
        .as_str()
        .unwrap()
        .contains("缺少审核后的文案"));
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn matters_preserve_history_conflicts_scope_review_claim_and_restart_without_execution() {
    let (e, _, effects, saved, _root) = fixture(false, vec![]).await;
    let first = e
        .matter_save_from(matter_input(0), "wechat", "synthetic-owner")
        .await
        .unwrap();
    let id = first["id"].as_str().unwrap();
    assert!(e
        .matter_save_from(matter_input(1), "wechat", "synthetic-owner")
        .await
        .is_err());
    let mut update = matter_input(1);
    update.id = Some(id.into());
    update.summary = "文案已审核".into();
    update.next_action = "生成动画".into();
    e.matter_save_from(update, "wechat", "synthetic-owner")
        .await
        .unwrap();
    assert_eq!(
        e.matters().await["entries"][0]["previous"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    {
        let mut s = e.state.lock().await;
        s.matters.entries[0].review_at =
            Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339());
    }
    let wrong = e
        .channel(json!({"operation":"notifications","source_key":"other-owner"}))
        .await
        .unwrap();
    assert_eq!(wrong, json!([]));
    let notice = e
        .channel(json!({"operation":"notifications","source_key":"synthetic-owner"}))
        .await
        .unwrap();
    assert_eq!(notice.as_array().unwrap().len(), 1);
    assert_eq!(notice[0]["kind"], "matter_notice");
    assert!(notice[0]["text"].as_str().unwrap().contains("没有重新执行"));
    assert_eq!(
        e.channel(json!({"operation":"notifications","source_key":"synthetic-owner"}))
            .await
            .unwrap(),
        json!([])
    );
    assert!(e.channel(json!({"operation":"notification_ack","source_key":"other-owner","kind":"matter_notice","reminder_id":id,"delivery":"accepted"})).await.is_err());
    let release = |client: Value| json!({"operation":"notification_ack","source_key":"synthetic-owner",
        "kind":"matter_notice","reminder_id":id,"client_id":client,"delivery":"not_sent"});
    assert!(e.channel(release(json!("older-review"))).await.is_err());
    e.channel(release(notice[0]["client_id"].clone())).await.unwrap();
    assert_eq!(e.channel(json!({"operation":"notifications","source_key":"synthetic-owner"})).await.unwrap(), notice);
    let state = saved.lock().await.clone();
    let e = Engine::new(
        state,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(e.matters().await["entries"][0]["delivery"], "unknown");
    assert_eq!(
        e.channel(json!({"operation":"notifications","source_key":"synthetic-owner"}))
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn matter_proposal_requires_confirmation_and_cross_chat_recall_uses_original_record() {
    let(e,_,_,saved,_root)=fixture(true,vec![reply(vec![("homebase_matter",json!({"action":"save","revision":0,"title":"发布动画","goal":"持续跟进动画与审核","summary":"已核对素材","next_action":"等审核后制作","state":"waiting","review_at":null}))]),reply(vec![]),reply(vec![])]).await;
    let a = msg(&e, "create-matter", "请持续跟进发布动画").await;
    let tid = a["turn_id"].as_str().unwrap();
    wait(&e, tid, "awaiting_confirmation").await;
    assert_eq!(e.matters().await["entries"], json!([]));
    msg(&e, "approve-matter", "确认").await;
    wait(&e, tid, "completed").await;
    assert_eq!(e.matters().await["entries"].as_array().unwrap().len(), 1);
    msg(&e, "new-matter-chat", "新对话").await;
    let b = msg(&e, "recall-matter", "发布动画上次进展怎样？").await;
    wait(&e, b["turn_id"].as_str().unwrap(), "completed").await;
    let state = saved.lock().await;
    let (si, ti) = locate(&state, b["turn_id"].as_str().unwrap()).unwrap();
    assert!(state.sessions[si].turns[ti]
        .runtime_prompt
        .contains("等审核后制作"));
    let other = Engine::matter_list(&state, "other-owner", "");
    assert_eq!(other["entries"], json!([]));
}
#[tokio::test]
async fn actual_graph_uses_new_matter_tool_and_restores_older_tool_contract() {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let mut state = saved.lock().await.clone();
    state.settings.runtime = "pi".into();
    state.settings.enabled = true;
    let count = Arc::new(AtomicUsize::new(0));
    let n = count.clone();
    let model: Model = Arc::new(move |r| {
        let n = n.clone();
        Box::pin(async move {
            assert!(r
                .definitions
                .unwrap()
                .iter()
                .any(|d| d["function"]["name"] == "homebase_matter"));
            Ok(if n.fetch_add(1, Ordering::SeqCst) == 0 {
                tool_reply(0, "homebase_matter", json!({"action":"list"}))
            } else {
                reply(vec![])
            })
        })
    });
    let e = Engine::new(
        state,
        e.save.clone(),
        model,
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    let a = e
        .send(request("检查持续事项", "new-tool-contract"))
        .await
        .unwrap();
    task_wait(&e, &a.turn_id, "completed").await;
    let old = wire::tools_for_runtime("homebase-da-20261113.4");
    assert!(!old
        .iter()
        .any(|d| d["function"]["name"] == "homebase_matter"));
    assert_eq!(
        old.iter()
            .find(|d| d["function"]["name"] == "homebase_memory")
            .unwrap()["function"]["parameters"]["properties"]["expires_at"]["type"],
        json!(["string", "null"])
    );
}

#[tokio::test]
async fn named_matter_continue_keeps_original_checkpoint_and_replay_target_after_new_link() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let arrived = reached.clone();
    let released = release.clone();
    let model: Model = Arc::new(move |_| {
        let (observed, arrived, released) = (observed.clone(), arrived.clone(), released.clone());
        Box::pin(async move {
            if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(crate::model_capability_io::reliability::Failure::http(503).encode());
            }
            arrived.notify_one();
            released.notified().await;
            Ok(reply(vec![]))
        })
    });
    let (e, effects, _, _root) = reliability_tests::setup(model).await;
    let a = msg(&e, "named-main", "发布动画的原任务").await;
    let tid = a["turn_id"].as_str().unwrap().to_string();
    wait(&e, &tid, "failed").await;
    for _ in 0..100 {
        if !e.running.lock().await.contains_key(&tid) { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!e.running.lock().await.contains_key(&tid));
    let first = e
        .matter_save_from(matter_input(0), "wechat", "synthetic-owner")
        .await
        .unwrap();
    {
        let mut s = e.state.lock().await;
        s.matters.entries[0].turns.push(tid.clone());
    }
    let before = msg(&e, "named-status", "进度发布动画").await;
    assert_eq!(before["turn_id"], tid);
    let resumed = msg(&e, "named-continue", "继续发布动画").await;
    assert_eq!(resumed["turn_id"], tid);
    assert_eq!(resumed["state"], "running");
    tokio::time::timeout(Duration::from_secs(30), reached.notified()).await.unwrap();
    // Replay must return the saved receipt before resolving a later changed link.
    {
        let mut s = e.state.lock().await;
        s.matters.entries[0]
            .turns
            .push("later-target-not-present-in-this-fixture".into());
    }
    assert_eq!(msg(&e, "named-continue", "继续发布动画").await, resumed);
    assert!(e.channel(json!({"operation":"submit","source_key":"other-owner","request_id":"other-named","text":format!("继续 {tid}")})).await.is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(first["state"], "waiting");
    release.notify_one();
    wait(&e, &tid, "completed").await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn linked_schedule_reserves_slots_checks_original_scope_and_pauses_changed_goal() {
    let (e, models, effects, _, _root) = fixture(false, vec![]).await;
    e.mutate(Change::default(), |s| {
        s.settings.enabled = true;
        s.settings.free_only = true;
        s.settings.runtime = "pi".into();
        s.settings.route_id = "openrouter::synthetic:free".into();
        Ok(())
    })
    .await
    .unwrap();
    let matter = e.matter_save(matter_input(0)).await.unwrap();
    let mid = matter["id"].as_str().unwrap().to_string();
    let make = |revision| FollowupInput {
        trigger:None,notification_policy:None,
        revision,
        title: "复查原事项".into(),
        instruction: "只核对状态".into(),
        due_at: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
        interval_minutes: Some(5),
        max_runs: 30,
        matter_id: Some(mid.clone()),
    };
    assert!(e
        .followup_save(make(0), "wechat", "foreign-source")
        .await
        .is_err());
    e.followup_save(make(0), "desktop", "desktop")
        .await
        .unwrap();
    e.followup_save(make(1), "desktop", "desktop")
        .await
        .unwrap();
    assert!(
        e.followup_save(make(2), "desktop", "desktop")
            .await
            .is_err(),
        "reserved 60 of 64 slots must reject another 30"
    );
    e.mutate(Change::default(), |s| {
        let entry = &mut s.followups.entries[0];
        entry.runs.push(followup::FollowupRun {
            ordinal: 1,
            scheduled_at: entry.due_at.clone(),
            request_id: "hb-followup-test-original".into(),
            turn_id: None,
            state: "prepared".into(),
        });
        Ok(())
    })
    .await
    .unwrap();
    {
        let s = e.state.lock().await;
        assert!(Engine::scheduled_submission_allowed(
            &s,
            "hb-followup-test-original",
            "desktop",
            "desktop",
            &s.settings
        ));
    }
    let mut edit = matter_input(1);
    edit.id = Some(mid.clone());
    edit.summary = "只更新已确认笔记".into();
    e.matter_save(edit.clone()).await.unwrap();
    {
        let s = e.state.lock().await;
        assert!(Engine::followup_matter_ready(&s, &s.followups.entries[0]));
    }
    edit.revision = 2;
    edit.next_action = "新的后续目标".into();
    e.matter_save(edit).await.unwrap();
    e.mutate(Change::default(), |s| {
        s.followups.entries[0].due_at =
            (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        assert!(!Engine::scheduled_submission_allowed(
            s,
            "hb-followup-test-original",
            "desktop",
            "desktop",
            &s.settings
        ));
        Ok(())
    })
    .await
    .unwrap();
    e.followup_tick().await.unwrap();
    assert_eq!(e.followups().await["entries"][0]["state"], "paused");
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn linked_task_progress_invalidates_personal_only_when_evidence_changes() {
    let (e, _, _, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("原事项任务", "linked-cache-original"))
        .await
        .unwrap();
    e.matter_save(matter_input(0)).await.unwrap();
    e.mutate(Change::default(), |s| {
        s.matters.entries[0].turns.push(ack.turn_id.clone());
        Ok(())
    })
    .await
    .unwrap();
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].state = "needs_input".into();
        s.turns[t].text = "等待审核".into();
        Ok(())
    })
    .await
    .unwrap();
    assert!(e.state.lock().await.changed.personal);
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].rounds += 1;
        Ok(())
    })
    .await
    .unwrap();
    assert!(
        !e.state.lock().await.changed.personal,
        "checkpoint counters must not reload personal panel"
    );
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].state = "completed".into();
        Ok(())
    })
    .await
    .unwrap();
    assert!(e.state.lock().await.changed.personal);
}

#[tokio::test]
async fn completed_action_is_never_presented_as_current_work_between_model_rounds() {
    let (e, _, effects, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("核对文件后整理说明", "completed-progress-label"))
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, t| {
        let turn = &mut s.turns[t];
        turn.state = "running".into();
        turn.steps.push(Step {
            id: "finished-read".into(),
            name: "homebase_files".into(),
            state: "completed".into(),
            arguments: json!({"action":"read","path":"already-read.txt"}),
            result: json!({"status":"succeeded"}),
            job_id: None,
        });
        Ok(())
    })
    .await
    .unwrap();
    let p = e.turn(&ack.turn_id).await.unwrap().progress;
    assert_eq!(p.current.as_deref(), Some("核对已有结果并准备下一步"));
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].state = "waiting_jobs".into();
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        e.turn(&ack.turn_id)
            .await
            .unwrap()
            .progress
            .current
            .as_deref(),
        Some("等待已受理任务返回结果")
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
