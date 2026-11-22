// Real capability CLI and graph; only synthetic planner and loopback WeChat.
use super::*;

#[test]
fn direct_repository_analysis_accepts_one_public_home_and_preserves_the_request() {
    let expected = "https://github.com/markedjs/marked";
    for request in [
        "分析仓库：https://github.com/markedjs/marked.git/",
        "/分析仓库 https://gitingest.com/markedjs/marked",
        "帮我分析仓库 https://gitmcp.io/markedjs/marked",
        "把 https://deepwiki.com/markedjs/marked 加入能力库。",
    ] {
        assert!(matches!(video::analyze_repository_link(request, "original"),
            Some(Ok(Action::Call(command, args))) if command == "repo_project_analyze"
                && args["request"]["repository_url"] == expected && args["request"]["request_id"] == "original"),
            "did not process {request}");
    }
    for address in [
        "", "http://github.com/markedjs/marked", "https://github.com/markedjs/marked?token=synthetic",
        "https://synthetic@github.com/markedjs/marked", "https://github.com:443/markedjs/marked",
        "https://github.com/markedjs/marked/tree/main", "https://github.com/markedjs/marked#section",
        "https://github.com/markedjs/marked https://github.com/other/project", "https://github.com.invalid/owner/repo",
        "https://deepwiki.com/markedjs/%2fmarked", "https://gitmcp.io/markedjs/../marked",
    ] {
        assert!(matches!(video::analyze_repository_link(&format!("分析仓库 {address}"), "original"), Some(Err(_))));
    }
    for request in ["https://github.com/markedjs/marked", "分析第 1 个项目", "把这个文档加入能力库"] {
        assert!(video::analyze_repository_link(request, "original").is_none());
    }
}

#[cfg(windows)]
#[tokio::test]
async fn video_name_search_keeps_the_selected_public_candidate_after_restart() {
    let root = tempfile::tempdir().unwrap();
    let reply_key = "9007199254740714";
    let video_key = "9007199254740715";
    let (base, traffic, server) = fake_api(200, json!({"ret":0, "message_id":reply_key})).await;
    let mut stored = account(&base);
    let source = stored.logical_key();
    let receipt = identity::legacy_receipt_id(&stored, video_key);
    let search_id = identity::legacy_receipt_id(&stored, "repository-query");
    stored.records.push(Record {
        id: receipt.clone(), command: "wechat_video_transcript".into(),
        status: "completed".into(),
        args: json!({"source":source,"quote_source":identity::source_key(&source, video_key)}),
        result: Some(json!({
            "text":"这是合成视频的本机文字稿 SYNTHETIC_PRIVATE_CANARY，项目名叫 Marked。",
            "project_names":["Other","Marked"]
        })),
        ..Default::default()
    });
    let foreign = format!("wx-{}", "e".repeat(64));
    stored.records.push(Record {
        id: foreign.clone(), command: "wechat_video_transcript".into(), status: "completed".into(),
        args: json!({"source":"another-account"}), result: Some(json!({"text":"另一来源"})),
        ..Default::default()
    });
    let searches = Arc::new(AtomicUsize::new(0));
    let analyses = Arc::new(AtomicUsize::new(0));
    let selected = "https://github.com/second/Marked";
    let runner: Runner = Arc::new({
        let (searches, analyses) = (searches.clone(), analyses.clone());
        move |command, args| {
            let (searches, analyses) = (searches.clone(), analyses.clone());
            Box::pin(async move {
                match command.as_str() {
                    repository_search::COMMAND => {
                        searches.fetch_add(1, Ordering::SeqCst);
                        assert_eq!(args.len(), 1, "only send explicitly selected query");
                        if args["query"] == "limited" {
                            return Ok(json!({"status":"failed","error":{
                                "code":"SOURCE_RATE_LIMITED","message":"SYNTHETIC_RAW_ERROR_CANARY"
                            }}));
                        }
                        assert_eq!(args["query"], "Marked");
                        Ok(json!({"status":"succeeded","partial":true,"candidates":[
                            {"repository_url":"https://github.com/first/marked","stars":23,"archived":false},
                            {"repository_url":selected,"stars":12,"archived":false}
                        ]}))
                    }
                    "repo_project_analyze" => {
                        assert_eq!(args["request"]["repository_url"], selected);
                        analyses.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({"status":"succeeded","registered":false,"project":{
                            "id":"project_synthetic-choice","repository":selected,"phase":"needs_adapter","revision":2
                        }}))
                    }
                    _ => panic!("unexpected command {command}"),
                }
            })
        }
    });
    let catalog: Value = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(catalog.clone(), root.path(), runner.clone()).unwrap();
    let (bridge, memory) = test_bridge(stored, root.path(), controller.clone());
    bridge.ingest(&msg("foreign-video", &format!("/找项目 {foreign} Marked"))).await.unwrap();
    bridge.ingest(&msg("qualifier", &format!("/找项目 {receipt} Marked user:someone"))).await.unwrap();
    let mut query = msg("repository-query","找第二个项目。");
    query["item_list"][0]["ref_msg"] = json!({"svr_id":video_key});
    bridge.ingest(&query).await.unwrap();
    bridge.ingest(&query).await.unwrap();
    bridge.ingest(&msg("repository-rate-limit",&format!("/找项目 {receipt} limited"))).await.unwrap();
    let tasks = bridge.status().await.tasks;
    let search = tasks.iter().find(|r| r.command == repository_search::COMMAND && r.id == search_id).unwrap();
    let limited = tasks.iter().find(|r| r.command == repository_search::COMMAND && r.id != search.id).unwrap();
    for task in [search, limited] {
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let job = bridge.request("jobs.get",None,json!({}),"test-read",task.job_id.as_deref()).await.unwrap();
                if job["status"] == "succeeded" { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
    }
    assert_eq!(searches.load(Ordering::SeqCst),2);
    assert_eq!(analyses.load(Ordering::SeqCst),0);
    let mut saved = memory.lock().await.clone();
    for record in &mut saved.records { record.outbound_quote_keys.clear(); }
    drop(bridge);
    drop(controller);
    let restarted = Controller::new(catalog, root.path(), runner).unwrap();
    let (bridge, _) = test_bridge(saved, root.path(), restarted);
    bridge.ingest(&query).await.unwrap();
    bridge.ingest(&msg("repository-result",&format!("/结果 {}", search.id))).await.unwrap();
    let protected = bridge.request("jobs.get",None,json!({}),"test-read-restart",search.job_id.as_deref()).await.unwrap();
    assert_eq!(protected["result_storage"],"protected");
    let mut choice = msg("repository-choice","分析第 2 个项目");
    choice["item_list"][0]["ref_msg"] = json!({"svr_id":reply_key});
    bridge.ingest(&choice).await.unwrap();
    bridge.ingest(&choice).await.unwrap();
    bridge.ingest(&msg("repository-result-limited",&format!("/结果 {}", limited.id))).await.unwrap();
    let new_task = bridge.status().await.tasks.into_iter()
        .find(|r| r.command == "repo_project_analyze").expect("selected candidate should be analyzed");
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let job = bridge.request("jobs.get",None,json!({}),"test-selection",new_task.job_id.as_deref()).await.unwrap();
            if job["status"] == "succeeded" { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(analyses.load(Ordering::SeqCst),1);
    assert_eq!(searches.load(Ordering::SeqCst),2);
    let replies = traffic.lock().await;
    let replies: Vec<_> = replies.iter().filter_map(|item| item["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
    assert!(replies.iter().any(|text| text.contains("https://github.com/first/marked") && text.contains(selected)));
    assert!(replies.iter().any(|text| text.contains("GitHub 暂时限制了公开搜索")));
    assert!(replies.iter().all(|text| !text.contains("SYNTHETIC_PRIVATE_CANARY") && !text.contains("SYNTHETIC_RAW_ERROR_CANARY")));
    let ledger = std::fs::read_to_string(root.path().join("operations.jsonl")).unwrap();
    assert!(!ledger.contains(selected) && !ledger.contains("SYNTHETIC_RAW_ERROR_CANARY"));
    server.abort();
}

#[cfg(windows)]
#[tokio::test]
async fn quoted_repository_install_preserves_confirmation_and_pinned_revision() {
    let root = tempfile::tempdir().unwrap();
    let analysis_key = "9007199254740712";
    let (base, traffic, server) = fake_api(200, json!({"ret":0})).await;
    let mut stored = account(&base);
    let source = stored.logical_key();
    let repository = "https://github.com/synthetic/fixture";
    let video_key = "9007199254740713";
    let video = identity::legacy_receipt_id(&stored, video_key);
    stored.records.push(Record {
        id: video.clone(), command: "wechat_video_transcript".into(), status: "completed".into(),
        args: json!({"source":source, "quote_source":identity::source_key(&source, video_key)}),
        result: Some(json!({"text":"合成文字稿","repository_refs":[repository]})),
        ..Record::default()
    });
    let analyses = Arc::new(AtomicUsize::new(0));
    let installs = Arc::new(AtomicUsize::new(0));
    let stale = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hold_install = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let operation = Arc::new(tokio::sync::OnceCell::<String>::new());
    let cancellations = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(tokio::sync::Notify::new());
    let runner: Runner = Arc::new({
        let (analyses, installs, stale) = (analyses.clone(), installs.clone(), stale.clone());
        let (hold_install, operation, cancellations, stop) =
            (hold_install.clone(), operation.clone(), cancellations.clone(), stop.clone());
        move |command, args| {
            let (analyses, installs, stale) = (analyses.clone(), installs.clone(), stale.clone());
            let (hold_install, operation, cancellations, stop) =
                (hold_install.clone(), operation.clone(), cancellations.clone(), stop.clone());
            Box::pin(async move {
                let input = &args["request"];
                let (phase, revision, registered) = match command.as_str() {
                    "repo_project_analyze" => {
                        assert_eq!(input["repository_url"], repository);
                        analyses.fetch_add(1, Ordering::SeqCst);
                        ("analyzed", 2, false)
                    }
                    "repo_project_install" => {
                        assert_eq!(input["project_id"], "project_synthetic");
                        assert_eq!(input["revision"], 2);
                        assert!(input["request_id"].as_str().unwrap().starts_with("assistant-"));
                        installs.fetch_add(1, Ordering::SeqCst);
                        if hold_install.load(Ordering::SeqCst) {
                            operation.set(input["request_id"].as_str().unwrap().into()).unwrap();
                            stop.notified().await;
                            return Ok(json!({"status":"canceled","project":{
                                "id":"project_synthetic","phase":"analyzed","revision":2
                            }}));
                        }
                        if stale.load(Ordering::SeqCst) {
                            assert!(input.get("examples").is_none());
                            return Ok(json!({"status":"failed","error":{"code":"PROJECT_STALE","message":"合成版本冲突"}}));
                        }
                        assert_eq!(input["examples"], json!([{"input":{"stdin":"# 合成"},"contains":["<h1>合成</h1>"]}]));
                        ("registered", 7, true)
                    }
                    "repo_operation_cancel" => {
                        assert_eq!(input["project_id"], "project_synthetic");
                        assert_eq!(input["operation_id"].as_str(), operation.get().map(String::as_str));
                        cancellations.fetch_add(1, Ordering::SeqCst);
                        return Ok(json!({"status":"succeeded","state":"cancel_requested"}));
                    }
                    _ => panic!("unexpected command {command}"),
                };
                Ok(json!({"status":"succeeded","registered":registered,"project":{
                    "repository":repository,"id":"project_synthetic","phase":phase,"revision":revision,
                    "source":{"readme_preview":"SYNTHETIC-NO-MODEL-CANARY".repeat(850)}
                }}))
            })
        }
    });
    let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(catalog, root.path(), runner.clone()).unwrap();
    let (analysis_bridge, wechat_state) = test_bridge(stored, root.path(), controller.clone());
    let invalid = msg("invalid-direct-repository", "分析仓库 https://github.com/synthetic/fixture?token=SYNTHETIC_QUERY_CANARY");
    analysis_bridge.ingest(&invalid).await.unwrap();
    assert_eq!(analyses.load(Ordering::SeqCst), 0);
    let direct = msg(analysis_key, "把 https://deepwiki.com/synthetic/fixture 加入能力库。");
    analysis_bridge.ingest(&direct).await.unwrap();
    analysis_bridge.ingest(&direct).await.unwrap();
    let analysis = analysis_bridge.status().await.tasks.into_iter()
        .find(|task| task.command == "repo_project_analyze").unwrap();
    let job = analysis.job_id.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = analysis_bridge.request("jobs.get", None, json!({}), "fixture", Some(&job)).await.unwrap();
            if value["status"] == "succeeded" { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let reopened_wechat = wechat_state.lock().await.clone();
    let saved = serde_json::to_string(&reopened_wechat).unwrap();
    assert!(!saved.contains("SYNTHETIC_QUERY_CANARY") && !saved.contains("deepwiki.com/"));
    drop(analysis_bridge);
    drop(controller);
    // Load the compact protected analysis answer rather than its in-memory README.
    let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(catalog, root.path(), runner).unwrap();
    let control: Control = Arc::new(move |request| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(request).await })
    });
    let mut state = crate::assistant::State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    let last_state = Arc::new(Mutex::new(state.clone()));
    let save: crate::assistant::Save = Arc::new({
        let last_state = last_state.clone();
        move |state| {
            let last_state = last_state.clone();
            Box::pin(async move { *last_state.lock().await = state; Ok(()) })
        }
    });
    let inference = Arc::new(AtomicUsize::new(0));
    let model: crate::assistant::Model = Arc::new({
        let inference = inference.clone();
        move |_| {
            inference.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err("仓库确认不需要模型".into()) })
        }
    });
    let core = crate::assistant::Engine::new(state, save.clone(), model.clone(), control.clone(), root.path().into()).await.unwrap();
    let make_bridge = |account: Stored, engine: Arc<crate::assistant::Engine>| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let agent: Agent = Arc::new(move |request| {
            let engine = engine.clone();
            Box::pin(async move { engine.channel(request).await })
        });
        Bridge::new_with_agent(account, api, Arc::new(|_| Box::pin(async { Ok(()) })), control.clone(), root.path().into(), Some(agent))
    };
    let bridge = make_bridge(reopened_wechat, core.clone());
    bridge.ingest(&msg("fixture-analysis-result", &format!("/结果 {}", analysis.id))).await.unwrap();
    assert!(bridge.state.lock().await.stored.records.iter().all(|record| record.outbound_quote_keys.is_empty()));
    for (key, command) in [
        ("unresolved-analysis", format!("/安装项目 {}", video)),
        ("malformed-examples", format!("/安装项目 {} {{}}", analysis.id)),
        ("unrecognized-example-fields", format!("/安装项目 {} [{{\"input\":{{\"stdin\":\"样例\",\"extra\":true}},\"contains\":[\"预期\"]}}]", analysis.id)),
        ("without-reference", "安装这个项目".into()),
    ] {
        bridge.ingest(&msg(key, &command)).await.unwrap();
    }
    assert!(!bridge.status().await.tasks.iter().any(|task| task.command == "assistant"));
    let examples = json!([{"input":{"stdin":"# 合成"},"contains":["<h1>合成</h1>"]}]);
    let quoted = msg("fixture-install", &format!("/安装项目 {} {examples}", analysis.id));
    let mut foreign = quoted.clone();
    foreign["from_user_id"] = json!("another-person");
    bridge.ingest(&foreign).await.unwrap();
    bridge.ingest(&quoted).await.unwrap();
    bridge.ingest(&quoted).await.unwrap();
    let tid = bridge.status().await.tasks.into_iter().find(|task| task.command == "assistant")
        .expect("the numbered analysis should bind to an installation turn").job_id.unwrap();
    let pending = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turn = core.turn(&tid).await.unwrap();
            if turn.state == "awaiting_confirmation" { break turn.pending.unwrap(); }
            assert_ne!(turn.state, "failed", "{}", turn.text);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert!(pending.description.contains(repository));
    assert_eq!(pending.arguments["request"]["revision"], 2);
    assert_eq!(pending.arguments["request"]["examples"], examples);
    assert_eq!(installs.load(Ordering::SeqCst), 0);
    // The same stored channel request and confirmation survive an Engine restart.
    let account = bridge.state.lock().await.stored.clone();
    drop(bridge);
    let reopened = serde_json::from_value(serde_json::to_value(last_state.lock().await.clone()).unwrap()).unwrap();
    let recovered_core = crate::assistant::Engine::new(reopened, save.clone(), model.clone(), control.clone(), root.path().into()).await.unwrap();
    let bridge = make_bridge(account, recovered_core.clone());
    let monitor = tokio::spawn(bridge.clone().monitor(0));
    bridge.ingest(&quoted).await.unwrap();
    let resumed = recovered_core.turn(&tid).await.unwrap().pending.unwrap();
    assert_eq!(resumed.id, pending.id);
    assert_eq!(resumed.arguments, pending.arguments);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if traffic.lock().await.iter().any(|item| {
                item["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()
                    .is_some_and(|text| text.contains("我准备安装并验收") && text.contains(&pending.id))
            }) { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    bridge.ingest(&msg("fixture-install-status", &format!("/状态 {tid}"))).await.unwrap();
    let confirmation = msg("fixture-confirm", "确认");
    bridge.ingest(&confirmation).await.unwrap();
    bridge.ingest(&confirmation).await.unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turn = recovered_core.turn(&tid).await.unwrap();
            if matches!(turn.state.as_str(), "completed" | "failed") { break turn; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(finished.state, "completed", "{}", finished.text);
    assert!(finished.text.contains("已通过验收"));
    assert_eq!(finished.jobs.len(), 1);
    assert_eq!(installs.load(Ordering::SeqCst), 1);
    bridge.ingest(&msg("fixture-install-result", &format!("/结果 {tid}"))).await.unwrap();
    let mut wrong = msg("fixture-wrong-quote", "安装这个项目");
    wrong["item_list"][0]["ref_msg"] = json!({"svr_id":video_key});
    bridge.ingest(&wrong).await.unwrap();
    stale.store(true, Ordering::SeqCst);
    let mut retry = msg("fixture-stale-quote", "安装这个仓库");
    retry["item_list"][0]["ref_msg"] = json!({"message_item":{"msg_id":analysis_key}});
    bridge.ingest(&retry).await.unwrap();
    let stale_tid = bridge.status().await.tasks.into_iter().find_map(|task| {
        task.job_id.filter(|job| task.command == "assistant" && job != &tid)
    }).unwrap();
    let stale_pending = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turn = recovered_core.turn(&stale_tid).await.unwrap();
            if let Some(pending) = turn.pending { break pending; }
            assert_ne!(turn.state, "failed", "{}", turn.text);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert_eq!(installs.load(Ordering::SeqCst), 1);
    bridge.ingest(&msg("fixture-stale-confirm", &format!("确认 {}", stale_pending.id))).await.unwrap();
    let stale_outcome = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turn = recovered_core.turn(&stale_tid).await.unwrap();
            if turn.state == "failed" { break turn; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert!(stale_outcome.text.contains("项目版本已更新"), "{}", stale_outcome.text);
    assert_eq!(stale_outcome.jobs.len(), 1);
    assert_eq!(installs.load(Ordering::SeqCst), 2);

    let prepare = |key: &'static str| {
        let bridge = bridge.clone();
        let core = recovered_core.clone();
        let request = format!("/安装项目 {}", analysis.id);
        async move {
            bridge.ingest(&msg(key, &request)).await.unwrap();
            let original = identity::legacy_receipt_id(&bridge.state.lock().await.stored, key);
            let turn = bridge.status().await.tasks.into_iter()
                .find(|task| task.id == original).and_then(|task| task.job_id).unwrap();
            let pending = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(pending) = core.turn(&turn).await.unwrap().pending { break pending; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            (turn, pending.id)
        }
    };
    let (declined, _) = prepare("9007199254740714").await;
    bridge.ingest(&msg("decline-install-natural", "先别装了。")).await.unwrap();
    assert_eq!(recovered_core.turn(&declined).await.unwrap().state, "rejected");
    assert_eq!(installs.load(Ordering::SeqCst), 2);
    assert_eq!(cancellations.load(Ordering::SeqCst), 0);

    hold_install.store(true, Ordering::SeqCst);
    let held_key = "9007199254740715";
    let (held, confirmation) = prepare(held_key).await;
    bridge.ingest(&msg("hold-install-confirm", &format!("确认 {confirmation}"))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let turn = recovered_core.turn(&held).await.unwrap();
            assert_eq!(turn.state, "running", "{}", turn.text);
            if operation.get().is_some() && turn.jobs.len() == 1 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    bridge.ingest(&msg("another-conversation", "新对话")).await.unwrap();
    bridge.ingest(&msg("unscoped-cancel", "先别装了")).await.unwrap();
    assert_eq!(cancellations.load(Ordering::SeqCst), 0);
    assert!(recovered_core.channel(json!({"operation":"submit_quoted_task","source_key":"different-binding",
        "request_id":"foreign-cancel","turn_id":held,"text":"先别装了","file":null})).await.is_err());
    let mut cancel = msg("hold-install-cancel", "先别装了！");
    cancel["item_list"][0]["ref_msg"] = json!({"message_item":{"msg_id":held_key}});
    bridge.ingest(&cancel).await.unwrap();
    bridge.ingest(&cancel).await.unwrap();
    assert_eq!(cancellations.load(Ordering::SeqCst), 1);
    assert_eq!(recovered_core.turn(&held).await.unwrap().state, "running");
    assert!(bridge.status().await.tasks.iter().any(|task|
        task.job_id.as_deref() == Some(held.as_str()) && task.status == "running"));
    stop.notify_one();
    let final_state = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let turn = recovered_core.turn(&held).await.unwrap();
            if turn.state == "canceled" { break turn; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    assert_eq!(final_state.jobs.len(), 1);
    assert!(final_state.text.contains("安装已取消"));
    assert_eq!(installs.load(Ordering::SeqCst), 3);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if traffic.lock().await.iter().any(|item| {
                item["body"]["msg"]["item_list"][0]["text_item"]["text"]
                    .as_str().is_some_and(|text| text.contains("仓库安装已取消。"))
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    bridge.ingest(&msg("hold-install-result", &format!("/结果 {held}"))).await.unwrap();

    let account = bridge.state.lock().await.stored.clone();
    let request_id = identity::legacy_receipt_id(&account, "hold-install-cancel");
    monitor.abort();
    drop(bridge);
    drop(recovered_core);
    let reopened = serde_json::from_value(serde_json::to_value(last_state.lock().await.clone()).unwrap()).unwrap();
    let recovered = crate::assistant::Engine::new(reopened, save, model, control.clone(), root.path().into()).await.unwrap();
    let replay = recovered.channel(json!({"operation":"submit_quoted_task","source_key":source,
        "request_id":request_id,"turn_id":held,"text":"先别装了！","file":null})).await.unwrap();
    assert!(replay["text"].as_str().unwrap().contains("我已请求取消这次安装"));
    let bridge = make_bridge(account, recovered);
    bridge.ingest(&cancel).await.unwrap();
    assert_eq!(cancellations.load(Ordering::SeqCst), 1);
    let replies = traffic.lock().await;
    let texts: Vec<_> = replies.iter().filter_map(|item| item["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
    assert!(texts.iter().any(|text| text.contains("我准备安装并验收") && text.contains(repository)));
    assert!(texts.iter().any(|text| text.contains("你提供的 1 条验收示例")));
    let expected = format!("/安装项目 {}", analysis.id);
    assert!(texts.iter().any(|text| text.lines().any(|line| line == expected)));
    assert!(texts.iter().any(|text| text.contains("已通过验收并加入能力库")));
    assert!(texts.iter().any(|text| text.contains("请引用仓库分析的回复")));
    assert!(texts.iter().any(|text| text.contains("这次安装已取消，还没有开始下载")));
    assert!(texts.iter().any(|text| text.contains("当前对话没有正在处理的安装")));
    assert!(texts.iter().any(|text| text.contains("我已请求取消这次安装")));
    assert!(texts.iter().any(|text| text.contains("仓库安装已取消")));
    assert!(texts.iter().all(|text| !text.contains("SYNTHETIC-NO-MODEL-CANARY")));
    assert_eq!(inference.load(Ordering::SeqCst), 0);
    assert_eq!(analyses.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn wechat_video_repository_analysis_keeps_its_original_project_after_restart() {
    for quoted in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (base, traffic, server) = fake_api(200, json!({"ret":0})).await;
        let mut account = account(&base);
        let source = account.logical_key();
        let message_key = "9007199254741013";
        let original = identity::legacy_receipt_id(&account, message_key);
        let selected = "https://github.com/example-fixture/second";
        account.records.push(Record {
            id: original.clone(), command: "wechat_video_transcript".into(), status: "completed".into(),
            args: json!({"source":source,"quote_source":identity::source_key(&source, message_key)}),
            result: Some(json!({"text":"合成转写","repository_refs":["https://github.com/example-fixture/first",selected]})),
            ..Record::default()
        });
        let foreign = format!("wx-{}", "a".repeat(64));
        account.records.push(Record {
            id: foreign.clone(), command: "wechat_video_transcript".into(), status: "completed".into(),
            args: json!({"source":"foreign-source"}),
            result: account.records[0].result.clone(),
            ..Record::default()
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let runner: Runner = Arc::new(move |command, args| {
            let count = count.clone();
            Box::pin(async move {
                assert_eq!(command, "repo_project_analyze");
                assert_eq!(args["request"]["repository_url"], selected);
                assert!(args["request"]["request_id"].as_str().is_some_and(|id| id.starts_with("wx-")));
                count.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"status":"succeeded","registered":false,"project":{
                    "id":"project_synthetic-second","repository":selected,"name":"example-fixture/second",
                    "phase":"needs_adapter","revision":2,
                    "source":{"readme_preview":"SYNTHETIC_README_CANARY","supported":false}
                }}))
            })
        });
        let catalog: Value = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
        let ctl = Controller::new(catalog.clone(), root.path(), runner.clone()).unwrap();
        let (bridge, memory) = test_bridge(account, root.path(), ctl.clone());
        bridge.ingest(&msg("wrong-source", &format!("/分析项目 {foreign} 2"))).await.unwrap();
        bridge.ingest(&msg("wrong-index", &format!("/分析项目 {original} 6"))).await.unwrap();
        let mut request = if quoted { msg("analysis-request", "分析第 2 个项目") }
            else { msg("analysis-request", &format!("/分析项目 {original} 2")) };
        if quoted { request["item_list"][0]["ref_msg"] = json!({"svr_id":message_key}); }
        bridge.ingest(&request).await.unwrap();
        bridge.ingest(&request).await.unwrap();
        let task = bridge.status().await.tasks.into_iter().find(|task| task.command == "repo_project_analyze")
            .expect("selected original repository should start one analysis");
        let job = task.job_id.unwrap();
        let before = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let info = bridge.request("jobs.get", None, json!({}), "fixture", Some(&job)).await.unwrap();
                if info["status"] != "running" && info["status"] != "queued" { break info; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(before["status"], "succeeded");
        assert_eq!(before["result"]["project"]["source"]["readme_preview"], "SYNTHETIC_README_CANARY");
        let snapshot = memory.lock().await.clone();
        assert!(snapshot.records.iter().find(|r| r.id == task.id).unwrap().result.is_none());
        drop(bridge);
        drop(ctl);
        let restarted = Controller::new(catalog, root.path(), runner).unwrap();
        let (bridge, _) = test_bridge(snapshot, root.path(), restarted);
        bridge.ingest(&request).await.unwrap();
        bridge.ingest(&msg("analysis-result", &format!("/结果 {}", task.id))).await.unwrap();
        bridge.ingest(&msg("unavailable-install", &format!("/安装项目 {}", task.id))).await.unwrap();
        let after = bridge.request("jobs.get", None, json!({}), "fixture-restart", Some(&job)).await.unwrap();
        assert_eq!(after["result_storage"], "protected");
        assert_eq!(after["result"]["project"]["id"], "project_synthetic-second");
        assert!(after["result"]["project"].get("source").is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let replies = traffic.lock().await;
        let texts: Vec<_> = replies.iter().filter_map(|item| item["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
        assert!(texts.iter().any(|text| text.contains("唯一 npm 命令行入口") && text.contains("project_synthetic-second")));
        assert!(texts.iter().any(|text| text.contains("需要先适配") && !text.contains("项目编号")));
        assert!(texts.iter().all(|text| !text.contains("/安装项目 wx-")));
        assert!(texts.iter().all(|text| !text.contains("SYNTHETIC_README_CANARY")));
        assert!(!std::fs::read_to_string(root.path().join("operations.jsonl")).unwrap().contains("SYNTHETIC_README_CANARY"));
        server.abort();
    }
}

#[tokio::test]
#[ignore = "requires an explicit keyless capability lab receipt"]
async fn repository_capability_graph_wechat_and_desktop_share_real_results() {
    let receipt=PathBuf::from(std::env::var_os("HOMEBASE_REPO_RECEIPT").expect("explicit lab receipt"));
    assert!(receipt.to_string_lossy().replace('\\',"/").starts_with("F:/HomeBase-Labs/github-capabilities-"));
    let proof:Value=serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();assert_eq!(proof["passed"],true);
    let root=receipt.parent().unwrap().join(format!("wechat-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&root).unwrap();
    let context=proof["context"].clone();let workspace=PathBuf::from(context["output_root"].as_str().unwrap());
    let script=PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/repo-capabilities.mjs");assert!(script.is_file());
    let executions=Arc::new(AtomicUsize::new(0));let effects=executions.clone();
    let runner:Runner=Arc::new(move|command,args|{let context=context.clone();let script=script.clone();let effects=effects.clone();Box::pin(async move{
        let (action,input)=match command.as_str(){
            "repo_capabilities_list"=>("capabilities",json!({})),
            "repo_capability_describe"=>("describe",json!({"capability_id":args["capabilityId"]})),
            "repo_capability_run"=>{effects.fetch_add(1,Ordering::SeqCst);("run",args["request"].clone())},
            "repo_capability_result"=>("result",json!({"run_id":args["runId"]})),
            _=>return Err(ControlError::new("FIXTURE_BLOCKED","Only isolated repository capability commands")),
        };
        let mut command=tokio::process::Command::new("node");command.arg(script).arg(action).env_clear().stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        for key in ["SystemRoot","WINDIR","PATH","PATHEXT"]{if let Some(v)=std::env::var_os(key){command.env(key,v);}}
        #[cfg(windows)] command.creation_flags(0x08000000);
        let mut child=command.spawn().map_err(|_|ControlError::new("FIXTURE_SPAWN","Node unavailable"))?;let mut stdin=child.stdin.take().unwrap();
        stdin.write_all(format!("{}\n",json!({"context":context,"request":input})).as_bytes()).await.unwrap();
        let output=tokio::time::timeout(Duration::from_secs(60),child.wait_with_output()).await.unwrap().unwrap();drop(stdin);
        serde_json::from_slice(&output.stdout).map_err(|_|ControlError::new("FIXTURE_JSON","Invalid isolated response"))
    })});
    std::fs::create_dir_all(root.join("control")).unwrap();
    let controller=Controller::new(serde_json::from_str(include_str!("../../control/commands.json")).unwrap(),&root.join("control"),runner).unwrap();
    let control:Control=Arc::new(move|r|{let c=controller.clone();Box::pin(async move{c.handle(r).await})});
    let model_rounds=Arc::new(AtomicUsize::new(0));let rounds=model_rounds.clone();let capability=proof["capability_id"].clone();
    let original=Arc::new(Mutex::new(String::new()));let original_job=original.clone();
    let model:crate::assistant::Model=Arc::new(move|request|{let n=rounds.fetch_add(1,Ordering::SeqCst);let capability=capability.clone();let job=original_job.clone();Box::pin(async move{
        let (name,args)=match n{
            0=>("homebase_discover",json!({"query":"repo_capabilities"})),
            1=>("homebase_call",json!({"command":"repo_capabilities_list","args":{}})),
            2=>("homebase_call",json!({"command":"repo_capability_describe","args":{"capabilityId":capability}})),
            3=>("homebase_call",json!({"command":"repo_capability_run","args":{"request":{"capability_id":capability,"input":{"stdin":"# WeChat shared capability\n\n**Keyless**"},"request_id":"ignored-model-id"}}})),
            4=>{let last=request.messages.iter().rev().find(|m|m["role"]=="tool").unwrap();let v:Value=serde_json::from_str(last["content"].as_str().unwrap()).unwrap();let id=v["job_id"].as_str().unwrap().to_string();*job.lock().await=id.clone();("homebase_job",json!({"job_id":id}))},
            5|7=>("homebase_deliver",json!({"job_id":job.lock().await.clone()})),
            6|8=>return crate::assistant_protocol::parse("chat",&json!({"choices":[{"message":{"content":"已把 Markdown 转好了，结果文件发给你。"}}]})),
            _=>return Err("Unexpected extra synthetic model call".into()),
        };
        crate::assistant_protocol::parse("chat",&json!({"choices":[{"message":{"tool_calls":[{"id":format!("repo-fixture-{n}"),"function":{"name":name,"arguments":args.to_string()}}]}}]}))
    })});
    let mut state=crate::assistant::State::default();state.settings.enabled=true;state.settings.runtime="pi".into();state.settings.route_id="synthetic-no-key".into();
    let saved=Arc::new(Mutex::new(state.clone()));let memory=saved.clone();
    let save:crate::assistant::Save=Arc::new(move|s|{let dest=memory.clone();Box::pin(async move{*dest.lock().await=s;Ok(())})});
    let core=crate::assistant::Engine::new(state,save.clone(),model.clone(),control.clone(),workspace.clone()).await.unwrap();let engine=core.clone();
    let(base,seen,server)=fake_api(200,json!({"__media":true})).await;let mut api=Api::new().unwrap();api.test_origin=Some(base.clone());
    let bridge=Bridge::new_with_agent(account(&base),api,Arc::new(|_|Box::pin(async{Ok(())})),control.clone(),workspace.clone(),Some(Arc::new(move|r|{let e=engine.clone();Box::pin(async move{e.channel(r).await})})));
    bridge.start().await.unwrap();let message=msg("repo-natural","把这段Markdown转成HTML文本，并把结果文件发给我");bridge.ingest(&message).await.unwrap();bridge.ingest(&message).await.unwrap();
    let tid=bridge.status().await.tasks[0].job_id.clone().unwrap();
    let turn=tokio::time::timeout(Duration::from_secs(90),async{loop{let t=core.turn(&tid).await.unwrap();if !["queued","running","waiting_jobs"].contains(&t.state.as_str()){break t;}tokio::time::sleep(Duration::from_millis(100)).await;}}).await.unwrap();
    assert_eq!(turn.state,"completed","{}",turn.text);assert_eq!(turn.artifacts.len(),1);assert_eq!(executions.load(Ordering::SeqCst),1);
    let output=std::fs::read_to_string(&turn.artifacts[0].path).unwrap();assert!(output.contains("<h1>WeChat shared capability</h1>"));assert!(output.contains("<strong>Keyless</strong>"));
    bridge.ingest(&msg("repo-status",&format!("/状态 {tid}"))).await.unwrap();bridge.ingest(&msg("repo-result",&format!("/结果 {tid}"))).await.unwrap();
    assert!(seen.lock().await.iter().any(|v|v["body"]["msg"]["item_list"][0]["type"]==4));assert_eq!(executions.load(Ordering::SeqCst),1);
    // Desktop reads the same session; a restart can return the same owned artifact without execution.
    let desktop=core.turn(&tid).await.unwrap();assert_eq!(desktop.artifacts[0].sha256,turn.artifacts[0].sha256);
    let restored=crate::assistant::Engine::new(saved.lock().await.clone(),save,model,control,workspace).await.unwrap();
    assert_eq!(restored.turn(&tid).await.unwrap().artifacts[0].sha256,turn.artifacts[0].sha256);
    bridge.stop().await.unwrap();server.abort();
    std::fs::write(root.join("validation.json"),serde_json::to_vec_pretty(&json!({"passed":true,"actual_cli_executions":1,"real_model_calls":0,"real_wechat_messages":0,"graph_rounds":model_rounds.load(Ordering::SeqCst),"desktop_same_artifact":true,"restart_preserved":true,"encrypted_loopback_attachment":true,"turn":turn})).unwrap()).unwrap();
    println!("Repository WeChat receipt: {}",root.join("validation.json").display());
}
