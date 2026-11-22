// Loopback-only WeChat + actual agent graph + real artifact validation.
use super::*;
use crate::assistant::{Engine as AssistantEngine, State as AssistantState};
use crate::assistant_protocol::{self as wire, ModelReply, ModelRequest};

fn response(n: usize, name: &str, args: Value) -> ModelReply {
    wire::parse("chat",&json!({"choices":[{"message":{"tool_calls":[{"id":format!("proactive-test-{n}"),"function":{"name":name,"arguments":args.to_string()}}]}}]})).unwrap()
}
#[tokio::test]
async fn pi_wechat_timed_work_recalls_delivers_and_recovers_without_reexecution() {
    scheduled_agent("pi").await;
}

async fn scheduled_agent(runtime: &str) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let artifact = workspace.join("isolated-result.txt");
    let data = "HB-PERSONAL-FOLLOWUP：总和18；字幕用简短中文。";
    std::fs::write(&artifact, data).unwrap();
    std::fs::write(
        root.path().join("outside-sentinel.txt"),
        "UNCHANGED SYNTHETIC DATA",
    )
    .unwrap();
    let hash = format!("{:x}", Sha256::digest(data.as_bytes()));
    let count = Arc::new(AtomicUsize::new(0));
    let exports = count.clone();
    let file = artifact.clone();
    let sha = hash.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let file = file.clone();
        let sha = sha.clone();
        let count = exports.clone();
        Box::pin(async move {
            assert_eq!(command, "assistant_local_execute");
            assert_eq!(args["operation"], "files");
            assert_eq!(args["request"]["action"], "export");
            count.fetch_add(1, Ordering::SeqCst);
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
    let models = Arc::new(AtomicUsize::new(0));
    let calls = models.clone();
    let model: crate::assistant::Model = Arc::new(move |request: ModelRequest| {
        let calls = calls.clone();
        Box::pin(async move {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            Ok(match n {
            0=>response(n,"homebase_followup",json!({"action":"create","revision":0,"title":"隔离结果检查","instruction":"按已保存字幕偏好检查准备好的隔离结果，并返回原文件","due_at":(chrono::Utc::now()+chrono::Duration::seconds(10)).to_rfc3339(),"max_runs":1})),
            1=>wire::parse("chat",&json!({"choices":[{"message":{"content":"安排好了，到时检查结果并发给你。"}}]})).unwrap(),
            2=>{assert!(request.messages.iter().any(|m|m.to_string().contains("简短中文")));response(n,"homebase_memory",json!({"action":"search","query":"字幕 偏好"}))},
            3=>response(n,"homebase_files",json!({"action":"export","path":"isolated-result.txt"})),
            4=>{
                let last=request.messages.iter().rev().find(|m|m["role"]=="tool").unwrap();let result:Value=serde_json::from_str(last["content"].as_str().unwrap()).unwrap();
                response(n,"homebase_deliver",json!({"job_id":result["job_id"]}))
            },
            5=>wire::parse("chat",&json!({"choices":[{"message":{"content":"检查好了，结果是18，字幕沿用简短中文。原文件一起给你。"}}]})).unwrap(),
            _=>panic!("unexpected duplicate model request"),
        })
        })
    });
    let mut state = AssistantState::default();
    state.settings.enabled = true;
    state.settings.wechat_enabled = true;
    state.settings.free_only = true;
    state.settings.runtime = runtime.into();
    state.settings.route_id = "openrouter::synthetic:free".into();
    let saved = Arc::new(Mutex::new(state.clone()));
    let output = saved.clone();
    let save: crate::assistant::Save = Arc::new(move |s| {
        let out = output.clone();
        Box::pin(async move {
            *out.lock().await = s;
            Ok(())
        })
    });
    let engine = AssistantEngine::new(state, save.clone(), model.clone(), control.clone(), workspace.clone())
        .await
        .unwrap();
    engine
        .memory_save(crate::assistant_personal::MemoryInput {
            revision: 0,
            id: None,
            title: "字幕偏好".into(),
            content: "动画字幕使用简短中文".into(),
            expires_at: None,
        })
        .await
        .unwrap();
    let baseline_memory = engine.personal().await["memories"].clone();
    let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
    let stored = account(&base);
    let (template, wx_saved) = test_bridge(stored.clone(), &workspace, controller);
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base.clone());
    let core = engine.clone();
    let agent: Agent = Arc::new(move |r| {
        let e = core.clone();
        Box::pin(async move { e.channel(r).await })
    });
    let bridge = Bridge::new_with_agent(
        stored,
        api,
        template.save.clone(),
        control.clone(),
        workspace.clone(),
        Some(agent),
    );
    bridge.start().await.unwrap();
    bridge
        .ingest(&msg(
            "followup-create",
            "请稍后检查准备好的隔离结果，按字幕偏好把原文件发给我，只做一次",
        ))
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if engine
            .sessions()
            .await
            .iter()
            .any(|s| s.active_turn_id.is_some())
        {
            let rows = engine.sessions().await;
            let session = engine.session(&rows[0].id).await.unwrap();
            if session
                .turns
                .iter()
                .any(|t| t.state == "awaiting_confirmation")
            {
                break;
            }
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert_eq!(
        engine.followups().await["entries"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let consent = msg("followup-confirm", "确认");
    bridge.ingest(&consent).await.unwrap();
    bridge.ingest(&consent).await.unwrap();
    bridge.deliver_reminders().await;
    loop {
        bridge.deliver_reminders().await;
        let status = bridge.status().await;
        if let Some(failed) = status
            .tasks
            .iter()
            .find(|t| t.id.starts_with("hb-proactive-") && t.status == "failed")
        {
            panic!(
                "scheduled turn failed: {}",
                serde_json::to_string(
                    &engine
                        .turn(failed.job_id.as_deref().unwrap())
                        .await
                        .unwrap()
                )
                .unwrap()
            );
        }
        if status.tasks.iter().any(|t| {
            t.id.starts_with("hb-proactive-") && t.status == "succeeded" && t.delivery == "accepted"
        }) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "proactive task not delivered: {}",
            serde_json::to_string(&status).unwrap()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(models.load(Ordering::SeqCst), 6);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(engine.personal().await["memories"], baseline_memory);
    assert_eq!(
        std::fs::read_to_string(root.path().join("outside-sentinel.txt")).unwrap(),
        "UNCHANGED SYNTHETIC DATA"
    );
    let rows = seen.lock().await.clone();
    assert!(rows
        .iter()
        .any(|r| r["body"]["msg"]["item_list"][0]["type"] == 4));
    assert!(rows
        .iter()
        .any(|r| r["body"]["msg"].to_string().contains("结果是18")));
    let uploaded = rows
        .iter()
        .find(|r| r["head"].as_str().unwrap_or("").contains("/c2c/upload?"))
        .unwrap();
    assert_ne!(
        STANDARD
            .decode(uploaded["raw_base64"].as_str().unwrap())
            .unwrap(),
        data.as_bytes()
    );
    let original_turn = engine.followups().await["entries"][0]["runs"][0]["turn_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        engine.turn(&original_turn).await.unwrap().artifacts[0].sha256,
        hash
    );
    assert_eq!(engine.turn(&original_turn).await.unwrap().runtime, runtime);
    assert!(engine
        .channel(json!({"operation":"get","source_key":"other-owner","turn_id":original_turn}))
        .await
        .is_err());
    bridge.stop().await.unwrap();
    let old = wx_saved.lock().await.clone();
    let restored = AssistantEngine::new(saved.lock().await.clone(), save, model, control, workspace.clone())
        .await.unwrap();
    let shared = restored.clone();
    let agent: Agent = Arc::new(move |request| {
        let core = shared.clone();
        Box::pin(async move { core.channel(request).await })
    });
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let new = Bridge::new_with_agent(
        old,
        api,
        bridge.save.clone(),
        bridge.control.clone(),
        workspace,
        Some(agent),
    );
    let before = seen.lock().await.len();
    new.deliver_reminders().await;
    new.deliver_reminders().await;
    assert_eq!(
        new.status()
            .await
            .tasks
            .iter()
            .filter(|t| t.id.starts_with("hb-proactive-"))
            .count(),
        1
    );
    assert_eq!(seen.lock().await.len(), before);
    assert_eq!(models.load(Ordering::SeqCst), 6);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(restored.turn(&original_turn).await.unwrap().artifacts[0].sha256, hash);
    server.abort();
}
