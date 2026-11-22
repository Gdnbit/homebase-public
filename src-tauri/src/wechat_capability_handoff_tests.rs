// Actual shared graph, generated frontend context and loopback channel only.
use super::*;

#[tokio::test]
#[ignore = "explicit generated synthetic capability context and isolated graph runtime required"]
async fn generated_capability_selection_is_data_and_duplicate_wechat_never_executes_it() {
    let file = PathBuf::from(
        std::env::var_os("HOMEBASE_CAPABILITY_HANDOFF_FIXTURE")
            .expect("explicit synthetic fixture"),
    );
    assert!(
        file.is_absolute()
            && file
                .to_string_lossy()
                .replace('\\', "/")
                .starts_with("F:/HomeBase-Labs/")
    );
    let fixture: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(fixture["synthetic_only"], true);
    let text = fixture["text"].as_str().unwrap().to_string();
    let marker = fixture["marker"].as_str().unwrap().to_string();
    let expected = fixture["payload"].clone();
    let model_blocked = fixture["scenario"] == "model_blocked";
    let function = if model_blocked { "text" } else { "tools" };
    let kind = if model_blocked { "model" } else { "mcp" };
    let root = tempfile::tempdir().unwrap();
    let sentinel = root.path().join("outside.txt");
    std::fs::write(&sentinel, b"PRESERVE").unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let queries = reads.clone();
    let expected_query = expected.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let queries = queries.clone();
        let selected = expected_query.clone();
        Box::pin(async move {
            assert_eq!(
                command, "capability_index_query",
                "selection must not execute a capability"
            );
            assert!(
                args["request"].get("catalog_id").is_none(),
                "expired reference must be explicitly rediscovered"
            );
            assert_eq!(
                args["request"]["function"],
                selected["selection"]["function"]
            );
            queries.fetch_add(1, Ordering::SeqCst);
            Ok(
                json!({"version":"homebase-capability-discovery-v1","catalog_id":"33333333-3333-4333-8333-333333333333","snapshot_id":"22222222-2222-4222-8222-222222222222","assembled_at":chrono::Utc::now().to_rfc3339(),"expires_at":(chrono::Utc::now()+chrono::Duration::minutes(5)).to_rfc3339(),"offset":0,"total":1,"next_offset":null,"entries":[selected["selection"]["implementation"]],"groups":[{"function":function,"total":1,"kinds":[kind]}],"groups_has_more":false,"issues":[{"source":"synthetic","message":"未探测连接"}],"issues_total":1,"issues_has_more":false,"source_complete":false,"routing":"先核对，未授权不执行"}),
            )
        })
    });
    let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    std::fs::create_dir(root.path().join("control")).unwrap();
    let controller = Controller::new(catalog, &root.path().join("control"), runner).unwrap();
    let c = controller.clone();
    let control: Control = Arc::new(move |r| {
        let c = c.clone();
        Box::pin(async move { c.handle(r).await })
    });
    let rounds = Arc::new(AtomicUsize::new(0));
    let count = rounds.clone();
    let input = text.clone();
    let boundary = marker.clone();
    let model: crate::assistant::Model = Arc::new(
        move |r: crate::assistant_protocol::ModelRequest| {
            let count = count.clone();
            let input = input.clone();
            let boundary = boundary.clone();
            Box::pin(async move {
                match count.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        let message = r
                            .messages
                            .iter()
                            .filter_map(|m| m["content"].as_str())
                            .find(|s| s.contains(&input))
                            .expect("actual user context missing");
                        let raw = message.split_once(&boundary).unwrap().1;
                        let payload = serde_json::Deserializer::from_str(raw)
                            .into_iter::<Value>()
                            .next()
                            .unwrap()
                            .unwrap();
                        let implementation = &payload["selection"]["implementation"];
                        assert_eq!(implementation["kind"], kind);
                        assert_eq!(implementation["function"], function);
                        if model_blocked {
                            assert_eq!(implementation["state"], "not_verified");
                            assert_eq!(implementation["credential_binding"], "sops:synthetic_ref");
                            assert!(implementation["prerequisites"][1]
                                .as_str()
                                .unwrap()
                                .contains("配置：blocked"));
                            assert!(implementation["prerequisites"][2]
                                .as_str()
                                .unwrap()
                                .contains("来源状态：error"));
                        } else {
                            assert_eq!(implementation["state"], "authorization_required");
                        }
                        assert_eq!(payload["selection"]["implementation"]["cost"], "unknown");
                        assert!(
                            chrono::DateTime::parse_from_rfc3339(
                                payload["source"]["expires_at"].as_str().unwrap()
                            )
                            .unwrap()
                                < chrono::Utc::now()
                        );
                        let args = json!({"command":"capability_index_query","args":{"request":{"function":function,"kind":kind,"offset":0,"limit":1}}});
                        crate::assistant_protocol::parse(
                            "chat",
                            &json!({"choices":[{"message":{"tool_calls":[{"id":"synthetic-selection-query","function":{"name":"homebase_call","arguments":args.to_string()}}]}}]}),
                        )
                    }
                    1 => {
                        let observed = r
                            .messages
                            .iter()
                            .filter(|m| m["role"] == "tool")
                            .map(Value::to_string)
                            .collect::<String>();
                        assert!(observed.contains("unknown"));
                        let answer = if model_blocked {
                            assert!(
                                observed.contains("blocked") && observed.contains("not_verified")
                            );
                            assert!(observed.contains("来源状态：error"));
                            "默认凭据已阻断，目录同步也未成功；这不等于账号权限已经验证。你具体想做什么？我先不执行，也不更换凭据来源。"
                        } else {
                            assert!(observed.contains("authorization_required"));
                            "这个实现还需要授权，费用也未核对。你想用它具体做什么？我先不执行。"
                        };
                        crate::assistant_protocol::parse(
                            "chat",
                            &json!({"choices":[{"message":{"content":answer}}]}),
                        )
                    }
                    _ => panic!("duplicate message or status query repeated inference"),
                }
            })
        },
    );
    let mut state = crate::assistant::State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let state_file = root.path().join("state.dpapi");
    let output = state_file.clone();
    let save: crate::assistant::Save = Arc::new(move |state| {
        let output = output.clone();
        Box::pin(async move {
            let plain = zeroize::Zeroizing::new(serde_json::to_vec(&state).unwrap());
            let cipher = crate::wechat_crypto::crypt(&plain, true)?;
            std::fs::write(output, &*cipher).map_err(|_| "synthetic save failed".to_string())
        })
    });
    let core = crate::assistant::Engine::new(
        state,
        save.clone(),
        model.clone(),
        control.clone(),
        workspace.clone(),
    )
    .await
    .unwrap();
    let engine = core.clone();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base.clone());
    let bridge = Bridge::new_with_agent(
        account(&base),
        api,
        Arc::new(|_| Box::pin(async { Ok(()) })),
        control.clone(),
        workspace.clone(),
        Some(Arc::new(move |r| {
            let engine = engine.clone();
            Box::pin(async move { engine.channel(r).await })
        })),
    );
    bridge.start().await.unwrap();
    let message = msg("synthetic-capability-selection", &text);
    bridge.ingest(&message).await.unwrap();
    bridge.ingest(&message).await.unwrap();
    let turn_id = bridge.status().await.tasks[0].job_id.clone().unwrap();
    let turn = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let t = core.turn(&turn_id).await.unwrap();
            if !matches!(t.state.as_str(), "running" | "queued" | "waiting_jobs") {
                break t;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(turn.state, "completed", "{}", turn.text);
    assert!(turn.text.contains("先不执行"));
    let session = core.session(&turn.session_id).await.unwrap();
    assert!(session.messages.iter().any(|m| m.role == "user" && m.content == text));
    if let Some(expected_title) = fixture["expected_title"].as_str() {
        assert_eq!(session.title, expected_title);
    }
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    bridge
        .ingest(&msg(
            "synthetic-selection-status",
            &format!("/状态 {turn_id}"),
        ))
        .await
        .unwrap();
    bridge
        .ingest(&msg(
            "synthetic-selection-result",
            &format!("/结果 {turn_id}"),
        ))
        .await
        .unwrap();
    assert!(seen
        .lock()
        .await
        .iter()
        .any(|v| v["body"]["msg"]["item_list"][0]["text_item"]["text"]
            .as_str()
            .is_some_and(|s| s.contains("先不执行"))));
    bridge.stop().await.unwrap();
    server.abort();
    let encrypted = std::fs::read(&state_file).unwrap();
    let hash = Sha256::digest(&encrypted);
    let plain = crate::wechat_crypto::crypt(&encrypted, false).unwrap();
    let restored_state = serde_json::from_slice(&plain).unwrap();
    let restored = crate::assistant::Engine::new(restored_state, save, model, control, workspace)
        .await
        .unwrap();
    assert_eq!(restored.turn(&turn_id).await.unwrap().text, turn.text);
    assert_eq!(restored.session(&turn.session_id).await.unwrap().title, session.title);
    assert_eq!(Sha256::digest(std::fs::read(&state_file).unwrap()), hash);
    assert_eq!(rounds.load(Ordering::SeqCst), 2);
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"PRESERVE");
}
