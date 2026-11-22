use super::*;

#[tokio::test]
async fn wechat_browses_unified_catalog_without_models_and_recovers_the_original_result() {
    use crate::capability_discovery::{normalize, DiscoveryRequest, DiscoveryStore};

    let root = tempfile::tempdir().unwrap();
    let control_root = root.path().join("control");
    std::fs::create_dir(&control_root).unwrap();
    let (base, sent, server) = fake_api(200, json!({"ret": 0})).await;
    let reads = Arc::new(Mutex::new(Vec::new()));
    let requests = reads.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let requests = requests.clone();
        Box::pin(async move {
            assert_eq!(command, capability_browse::COMMAND);
            let mut request: DiscoveryRequest =
                serde_json::from_value(args["request"].clone()).unwrap();
            requests.lock().await.push(args["request"].clone());
            let now = std::time::Instant::now();
            let mut store = DiscoveryStore::new();
            let index = normalize(
                vec![
                    json!({"id":"mcp:fixture","function":"tools","kind":"mcp","name":"合成页面工具","state":"authorization_required","provider":"MCP","cost":"unknown","discover":"homebase_mcp","evidence":"合成目录","target":"C:/synthetic-private/mcp.exe","credential_binding":"sops:synthetic-private"}),
                    json!({"id":"mcp:missing","function":"tools","kind":"mcp","name":"合成调色工具","state":"not_configured","provider":"MCP","cost":"unknown","discover":"homebase_mcp","evidence":"合成目录","target":"synthetic-mcp-missing"}),
                    json!({"id":"skill:fixture","function":"workflow","kind":"skill","name":"合成录音转写","state":"instructions_available","provider":"Skill","cost":"depends_on_tools","discover":"homebase_installed_skill","evidence":"合成目录","target":"C:/synthetic-private/SKILL.md"}),
                    json!({"id":"service:fixture","function":"text","kind":"service","name":"合成 API","state":"configured","provider":"sops:synthetic-private","cost":"unknown","discover":"model_capability_resolve","evidence":"合成目录","target":"https://synthetic-private.invalid/v1"}),
                ],
                vec![json!({"source":"configured","message":"synthetic-private diagnostics"})],
            );
            request.catalog_id = Some(store.insert(index, now).unwrap());
            Ok(json!(store.page(&request, now).unwrap()))
        })
    });
    let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(catalog, &control_root, runner.clone()).unwrap();
    let (bridge, saved) = test_bridge(account(&base), root.path(), controller);
    let mut visitor = msg("visitor-browse", "能力 MCP");
    visitor["from_user_id"] = json!("synthetic-visitor");
    bridge.ingest(&visitor).await.unwrap();
    for (id, text) in [
        ("catalog-overview", "能力"),
        ("catalog-mcp", "能力 MCP"),
        ("catalog-skill", "找能力 转写"),
        ("catalog-service", "/能力 API"),
        ("catalog-unmatched", "找能力 从未登记"),
    ] {
        let message = msg(id, text);
        bridge.ingest(&message).await.unwrap();
        bridge.ingest(&message).await.unwrap();
    }
    let monitor = tokio::spawn(bridge.clone().monitor(0));
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if saved
                .lock()
                .await
                .records
                .iter()
                .filter(|record| record.status == "succeeded" && record.delivery == "accepted")
                .count()
                == 5
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let snapshot = saved.lock().await.clone();
    let original = snapshot
        .records
        .iter()
        .find(|r| r.args["request"]["kind"] == "mcp")
        .unwrap();
    let id = original.id.clone();
    bridge.stop().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), monitor)
        .await
        .unwrap()
        .unwrap();
    drop(bridge);
    let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(catalog, &control_root, runner).unwrap();
    let (bridge, _) = test_bridge(snapshot, root.path(), controller);
    bridge
        .ingest(&msg("catalog-mcp", "能力 MCP"))
        .await
        .unwrap();
    bridge
        .ingest(&msg("catalog-original-status", &format!("/状态 {id}")))
        .await
        .unwrap();
    bridge
        .ingest(&msg("catalog-original-result", &format!("/结果 {id}")))
        .await
        .unwrap();
    bridge
        .ingest(&msg(
            "catalog-no-unscoped-api",
            "/调用 capability_index_query {\"request\":{}}",
        ))
        .await
        .unwrap();
    let requests = reads.lock().await;
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().any(|request| request["kind"] == "service"));
    assert!(requests.iter().any(|request| request["query"] == "转写"));
    assert!(requests.iter().all(|request| request["limit"] == 6));
    let traffic = sent.lock().await;
    let replies: Vec<_> = traffic
        .iter()
        .filter_map(|r| r["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str())
        .collect();
    assert!(replies.iter().any(|text| text.contains("功能有")));
    assert!(replies.iter().any(|text| text.contains("合成调色工具（MCP，配置待补齐）")));
    assert!(replies
        .iter()
        .any(|text| text.contains("合成录音转写（Skill，可读取工作流）")));
    assert!(replies
        .iter()
        .any(|text| text.contains("合成 API（API 服务，已配置，接口待核对）")));
    assert!(replies
        .iter()
        .any(|text| text.contains("没有匹配项") && text.contains("部分来源暂未读全")));
    assert!(replies
        .iter()
        .any(|text| text.contains(&id) && text.contains("已处理完毕") && text.contains("/结果")));
    assert_eq!(
        replies
            .iter()
            .filter(|text| text.contains(&id) && text.contains("合成页面工具（MCP，需要授权）"))
            .count(),
        2
    );
    assert!(replies
        .iter()
        .all(|text| !text.contains("synthetic-private")));
    assert!(replies
        .iter()
        .any(|text| text.contains("该接口未开放给微信")));
    server.abort();
}

#[test]
fn capability_browse_validates_input_before_dispatch() {
    assert!(capability_browse::parse("能力库怎么用").is_none());
    for text in [
        "找能力",
        "能力 一行\n两行",
        &format!("能力 {}", "字".repeat(81)),
    ] {
        assert!(capability_browse::parse(text).unwrap().is_err());
    }
    assert_eq!(
        capability_browse::parse("能力： Skill"),
        Some(Ok(Action::Call(
            capability_browse::COMMAND.into(),
            json!({"request":{"kind":"skill","query":"","limit":6}}),
        )))
    );
    assert_eq!(
        parse("模型目录", Path::new(".")),
        Ok(Action::Call(
            "model_capabilities_catalog".into(),
            json!({"offset":0,"limit":12}),
        ))
    );
}
