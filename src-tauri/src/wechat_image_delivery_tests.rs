use super::*;

#[tokio::test]
async fn natural_image_task_uses_saved_route_and_returns_verified_png_after_restart() {
    natural_image_task(true).await;
}

#[tokio::test]
async fn natural_image_task_without_preference_checks_candidates_and_delivers() {
    natural_image_task(false).await;
}

async fn natural_image_task(has_preference: bool) {
    use crate::assistant::{Engine as Core, Model, Save as AgentSave, State};
    use crate::capability_discovery::{self as discovery, DiscoveryRequest, DiscoveryStore};
    use crate::model_capability_io::{self as io, ResponseMode};

    const SAVED_IMAGE_REQUEST: &str = "帮我生成一张图。";
    const DESCRIBED_IMAGE_REQUEST: &str = "帮我画一张蓝色封面";
    const ANOTHER_IMAGE: &str = "麻烦你就照咱们上回那套再来一版，这次把封面配色换成绿色。";
    const THIRD_IMAGE: &str = "再来一版。";
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let control_root = root.path().join("control");
    std::fs::create_dir(&control_root).unwrap();
    let png_response = |rgb: &[u8]| {
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, 2, 1);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgb).unwrap();
        drop(writer);
        let body = json!({"output_format":"png","data":[{"b64_json":STANDARD.encode(png),"url":"https://synthetic-private.invalid/raw"}],"usage":{"output_tokens":9}});
        io::fixture_response("200 OK", "application/json", body.to_string().as_bytes())
    };
    let (url, provider) = png_response(&[0, 30, 255, 0, 30, 255]);
    let second_provider = has_preference.then(|| png_response(&[0, 190, 0, 0, 190, 0]));
    let second_url = second_provider.as_ref().map(|(url, _)| url.clone());
    let routes = io::discover(
        "openai",
        &json!({"data":[{"id":"gpt-image-1.5"}]}),
        "https://api.openai.com/v1/models",
    )
    .unwrap();
    let route = routes.iter().find(|route| route.model_id == "gpt-image-1.5").unwrap().clone();
    let unavailable = crate::model_capabilities::seed_models()
        .into_iter()
        .find(|route| route.provider_id == "minimax" && route.model_id == "image-01")
        .unwrap();
    let unavailable_id = unavailable.route_id.clone();
    let catalog = discovery::normalize(routes.iter().chain(std::iter::once(&unavailable)).map(|route| {
        crate::capability_catalog_projection::model_entry(route, "image", &[], &[])
    }).collect(), vec![]);
    let route_id = route.route_id.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let preflights = Arc::new(Mutex::new(Vec::new()));
    let checked_routes = preflights.clone();
    let control_workspace = workspace.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let catalog = catalog.clone();
        let unavailable = unavailable.clone();
        let checked_routes = checked_routes.clone();
        let second_url = second_url.clone();
        let (route, url, count, workspace) = (
            route.clone(),
            url.clone(),
            count.clone(),
            control_workspace.clone(),
        );
        Box::pin(async move {
            if command == "capability_index_query" {
                let mut request: DiscoveryRequest = serde_json::from_value(args["request"].clone()).unwrap();
                assert_eq!(request.function.as_deref(), Some("image"));
                let clock = std::time::Instant::now();
                let mut store = DiscoveryStore::new();
                request.catalog_id = Some(store.insert(catalog, clock).unwrap());
                return Ok(json!(store.page(&request, clock).unwrap()));
            }
            if command == "model_capability_resolve" {
                let request = &args["request"];
                let candidate = match request["route_id"].as_str().unwrap() {
                    id if id == route.route_id => &route,
                    id if id == unavailable.route_id => &unavailable,
                    _ => panic!("only discovered routes may be checked"),
                };
                let (_, body) = crate::model_capabilities::prepare(candidate, &request["input"]).unwrap();
                assert_eq!(body["prompt"], "隔离蓝色封面");
                assert_eq!(count.load(Ordering::SeqCst), 0);
                checked_routes.lock().await.push(candidate.route_id.clone());
                return Ok(json!({"route":candidate,"callable":candidate.route_id == route.route_id}));
            }
            assert_eq!(command, "model_capability_call");
            let url = match count.fetch_add(1, Ordering::SeqCst) {
                0 => url,
                1 => second_url.expect("fixture must explicitly provide a second output"),
                _ => panic!("unconfirmed or duplicate generation"),
            };
            let request = &args["request"];
            assert_eq!(request["route_id"], route.route_id);
            let output = request["output_root"].as_str().unwrap();
            assert!(Path::new(output).starts_with(workspace.join("models")));
            let (endpoint, body) =
                crate::model_capabilities::prepare(&route, &request["input"]).unwrap();
            assert_eq!(endpoint, "https://api.openai.com/v1/images/generations");
            let mode = ResponseMode::for_submission(&route, &body, Some(output));
            let response = io::authorized(
                io::client().unwrap().post(url),
                "openai",
                "fixture-only-key",
            )
            .json(&body)
            .send()
            .await
            .unwrap();
            Ok(json!(io::response_result(
                &route,
                "sops:synthetic-private".into(),
                response,
                Some(output),
                mode
            )
            .await
            .unwrap()))
        })
    });
    let contract = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(contract, &control_root, runner.clone()).unwrap();
    let counts = Arc::new(AtomicUsize::new(0));
    let rounds = counts.clone();
    let model: Model = Arc::new(move |request| {
        let count = rounds.fetch_add(1, Ordering::SeqCst);
        let route_id = route_id.clone();
        let unavailable_id = unavailable_id.clone();
        Box::pin(async move {
            let last_result = || -> Value {
                let envelope = request.messages.iter().rev().find(|m| m["role"] == "tool").unwrap();
                serde_json::from_str(envelope["content"].as_str().unwrap()).unwrap()
            };
            let arguments = |route_id: &str| {
                let input = if route_id == unavailable_id {
                    json!({"prompt":"隔离蓝色封面"})
                } else {
                    json!({"prompt":"隔离蓝色封面","n":1,"output_format":"png"})
                };
                json!({"request":{"route_id":route_id,"input":input}})
            };
            let call_round = if has_preference { 2 } else { 3 };
            let tool = match count {
                0 => {
                    let context = request
                        .messages
                        .iter()
                        .filter_map(|v| v["content"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    assert_eq!(context.contains("默认主题为蓝色封面"), has_preference);
                    assert_eq!(context.contains(&route_id), has_preference);
                    assert!(context.contains("由助理做选择") && context.contains("排除 callable=false"));
                    assert_eq!(
                        request.messages.last().unwrap()["content"],
                        if has_preference { SAVED_IMAGE_REQUEST } else { DESCRIBED_IMAGE_REQUEST }
                    );
                    (
                        "homebase_call",
                        json!({"command":"capability_index_query","args":{"request":{"function":"image","kind":"model"}}}),
                    )
                }
                1 => {
                    let page = last_result();
                    let entries = page["result"]["entries"].as_array().unwrap();
                    assert_eq!(entries.len(), 2);
                    let first = if has_preference { &route_id } else { &unavailable_id };
                    let selected = entries.iter().find(|entry| entry["target"] == *first).unwrap();
                    (
                        "homebase_call",
                        json!({"command":"model_capability_resolve","args":arguments(selected["target"].as_str().unwrap())}),
                    )
                }
                2 if !has_preference => {
                    assert_eq!(last_result()["result"]["callable"], false);
                    (
                        "homebase_call",
                        json!({"command":"model_capability_resolve","args":arguments(&route_id)}),
                    )
                }
                round if round == call_round => {
                    let plan = last_result();
                    assert_eq!(plan["result"]["callable"], true);
                    assert_eq!(plan["result"]["route"]["route_id"], route_id);
                    ("homebase_call", json!({"command":"model_capability_call","args":arguments(&route_id)}))
                }
                round if round == call_round + 1 => {
                    let result = last_result();
                    assert_eq!(result["result"]["artifact"]["mime_type"], "image/png", "{result}");
                    assert!(!result.to_string().contains("b64_json"));
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                round if round == call_round + 2 => {
                    return crate::assistant_protocol::parse(
                        "chat",
                        &json!({"choices":[{"message":{"content":"蓝色封面已生成，正在返回图片。"}}]}),
                    )
                }
                round if has_preference && round == call_round + 3 => {
                    assert_eq!(request.messages.last().unwrap()["content"], ANOTHER_IMAGE);
                    let prompt = request.messages[0]["content"].as_str().unwrap();
                    assert!(prompt.contains("同一会话刚完成的交办"));
                    assert!(prompt.contains("默认主题为蓝色封面"));
                    assert!(prompt.contains("本轮明确说出的改动优先"));
                    let mut args = arguments(&route_id);
                    args["request"]["input"]["prompt"] = json!("隔离绿色封面");
                    ("homebase_call", json!({"command":"model_capability_call","args":args}))
                }
                round if has_preference && round == call_round + 4 => {
                    let result = last_result();
                    assert_eq!(result["result"]["artifact"]["mime_type"], "image/png");
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                round if has_preference && round == call_round + 5 => {
                    return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{
                        "content":"第二版绿色封面已生成。"
                    }}]}));
                }
                round if has_preference && round == call_round + 6 => {
                    assert_eq!(request.messages.last().unwrap()["content"], THIRD_IMAGE);
                    let prompt = request.messages[0]["content"].as_str().unwrap();
                    assert!(prompt.contains("同一会话刚完成的交办"));
                    assert!(prompt.contains("original_workflow_request"));
                    assert!(prompt.contains("默认主题为蓝色封面"));
                    assert!(prompt.contains(&format!("\"workflow_revision_requests\":{}", json!([
                        {"request":ANOTHER_IMAGE,"request_truncated":false}
                    ]))));
                    let mut args = arguments(&route_id);
                    args["request"]["input"]["prompt"] = json!("隔离绿色封面的另一版");
                    ("homebase_call", json!({"command":"model_capability_call","args":args}))
                }
                _ => panic!("original image inference must not run again"),
            };
            crate::assistant_protocol::parse(
                "chat",
                &json!({"choices":[{"message":{"tool_calls":[{"id":format!("fixture-image-{count}"),"function":{"name":tool.0,"arguments":tool.1.to_string()}}]}}]}),
            )
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "isolated-conversation-model".into();
    let core_save = Arc::new(Mutex::new(state.clone()));
    let output = core_save.clone();
    let save: AgentSave = Arc::new(move |state| {
        let output = output.clone();
        Box::pin(async move {
            *output.lock().await = state;
            Ok(())
        })
    });
    let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
    let (template, bridge_save) = test_bridge(account(&base), &workspace, controller);
    let core = Core::new(
        state,
        save.clone(),
        model.clone(),
        template.control.clone(),
        workspace.clone(),
    )
    .await
    .unwrap();
    if has_preference {
        core.memory_save(crate::assistant_personal::MemoryInput {
            revision: 0,
            id: None,
            title: "头图默认设置".into(),
            content: "使用 openai::gpt-image-1.5，格式为 PNG。默认主题为蓝色封面。".into(),
            expires_at: None,
        })
        .await
        .unwrap();
    }
    let connect = |stored: Stored, core: Arc<Core>, control: Control| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(
            stored,
            api,
            template.save.clone(),
            control,
            workspace.clone(),
            Some(Arc::new(move |request| {
                let core = core.clone();
                Box::pin(async move { core.channel(request).await })
            })),
        )
    };
    let bridge = connect(account(&base), core.clone(), template.control.clone());
    bridge.start().await.unwrap();
    let incoming = msg("natural-image", if has_preference { SAVED_IMAGE_REQUEST } else { DESCRIBED_IMAGE_REQUEST });
    bridge.ingest(&incoming).await.unwrap();
    bridge.ingest(&incoming).await.unwrap();
    let id = bridge.status().await.tasks[0].job_id.clone().unwrap();
    let wait = |state: &'static str| {
        let core = core.clone();
        let id = id.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(25), async move {
                loop {
                    let turn = core.turn(&id).await.unwrap();
                    assert_ne!(turn.state, "failed", "{}", turn.text);
                    if turn.state == state {
                        break turn;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap()
        }
    };
    let confirmation = wait("awaiting_confirmation").await;
    assert!(confirmation.text.contains("把本次请求交给 openai::gpt-image-1.5"));
    assert!(confirmation.text.contains("费用以该服务商账户的实际计费为准"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    bridge
        .ingest(&msg("natural-image-confirm", "确认"))
        .await
        .unwrap();
    let done = wait("completed").await;
    let [artifact] = done.artifacts.as_slice() else {
        panic!("one PNG should be returned")
    };
    let bytes = std::fs::read(&artifact.path).unwrap();
    assert_eq!(artifact.sha256, format!("{:x}", Sha256::digest(&bytes)));
    let mut reader = png::Decoder::new(bytes.as_slice()).read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((frame.width, frame.height), (2, 1));
    assert_eq!(&pixels[..frame.buffer_size()], &[0, 30, 255, 0, 30, 255]);
    assert!(provider.join().unwrap().contains("隔离蓝色封面"));
    // A request arriving at the loopback server does not prove that its
    // acknowledgement has been committed. Restart after the durable receipt.
    let snapshot = tokio::time::timeout(Duration::from_secs(10), async {
        let selected = json!(artifact);
        loop {
            {
                let saved = bridge_save.lock().await;
                if saved.records.iter().any(|record| {
                    record.command == "assistant"
                        && record.job_id.as_deref() == Some(id.as_str())
                        && record.file_outcome(&selected, std::slice::from_ref(&selected))
                            == Some(file_delivery::Outcome::Accepted)
                }) {
                    break saved.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    bridge.stop().await.unwrap();
    let contract = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(contract, &control_root, runner).unwrap();
    let control: Control = Arc::new(move |request| {
        let controller = controller.clone();
        Box::pin(async move { controller.handle(request).await })
    });
    let before_restart = core_save.lock().await.clone();
    let restored = Core::new(
        before_restart,
        save,
        model,
        control.clone(),
        workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        restored.turn(&id).await.unwrap().artifacts[0].sha256,
        artifact.sha256
    );
    let bridge = connect(snapshot, restored.clone(), control);
    bridge.ingest(&incoming).await.unwrap();
    let progress = msg("natural-image-status", "刚才那件事做到哪了？");
    bridge.ingest(&progress).await.unwrap();
    bridge.ingest(&progress).await.unwrap();
    let result = msg("natural-image-result", if has_preference {
        "请把 result.png 发给我。"
    } else {
        "帮我把刚才的结果发过来。"
    });
    bridge.ingest(&result).await.unwrap();
    bridge.ingest(&result).await.unwrap();
    let ledger = bridge_save.lock().await;
    let last = &ledger.records[ledger.records.len() - 2..];
    assert_eq!(last[0].query(), Some((QueryTarget::Turn(&id), false)));
    assert_eq!(last[1].query(), Some((QueryTarget::Turn(&id), true)));
    if has_preference {
        assert_eq!(last[1].args["file_request_v1"]["resend"], false);
        assert!(last[1].args.get("current_query").is_none());
    }
    drop(ledger);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(counts.load(Ordering::SeqCst), if has_preference { 5 } else { 6 });
    let checked = preflights.lock().await;
    let working = "openai::gpt-image-1.5";
    let expected = if has_preference {
        vec![working]
    } else {
        vec!["minimax::image-01", working]
    };
    assert_eq!(checked.as_slice(), expected);
    let traffic = seen.lock().await;
    let file_messages = traffic
        .iter()
        .filter_map(|r| r["body"]["msg"]["item_list"][0]["file_item"].as_object())
        .collect::<Vec<_>>();
    assert_eq!(file_messages.len(), 2);
    assert!(file_messages
        .iter()
        .all(|file| file["file_name"] == "result.png"));
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
    let requests_to = |endpoint| {
        traffic.iter().filter(move |request| {
            request["head"].as_str().unwrap_or_default().contains(endpoint)
        })
    };
    let params = requests_to("/getuploadurl ").collect::<Vec<_>>();
    let uploads = requests_to("/c2c/upload?").collect::<Vec<_>>();
    assert_eq!((params.len(), uploads.len()), (2, 2));
    for (param, upload) in params.into_iter().zip(uploads) {
        let key = param["body"]["aeskey"].as_str().unwrap();
        let key = (0..32)
            .step_by(2)
            .map(|i| u8::from_str_radix(&key[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let ciphertext = STANDARD
            .decode(upload["raw_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            ecb::Decryptor::<aes::Aes128>::new_from_slice(&key)
                .unwrap()
                .decrypt_padded_vec_mut::<Pkcs7>(&ciphertext)
                .unwrap(),
            bytes
        );
    }
    assert!(!traffic
        .iter()
        .any(|row| row["body"]["msg"].to_string().contains("synthetic-private")));
    drop(traffic);
    drop(checked);
    if has_preference {
        let memory_before = restored.memory_search("默认出图", 0).await.unwrap();
        let wait_for_version = |turn_id: String, state: &'static str| {
            let restored = restored.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(20), async {
                    loop {
                        let turn = restored.turn(&turn_id).await.unwrap();
                        assert_ne!(turn.state, "failed", "{}", turn.text);
                        if turn.state == state { break turn; }
                        tokio::time::sleep(Duration::from_millis(30)).await;
                    }
                }).await.unwrap()
            }
        };
        bridge.start().await.unwrap();
        let repeat = msg("natural-image-another-version", ANOTHER_IMAGE);
        bridge.ingest(&repeat).await.unwrap();
        bridge.ingest(&repeat).await.unwrap();
        let followup_id = bridge_save.lock().await.records.last().unwrap().job_id.clone().unwrap();
        assert_ne!(followup_id, id);
        let proposed = wait_for_version(followup_id.clone(), "awaiting_confirmation").await;
        assert_eq!(proposed.parent_turn_id.as_deref(), Some(id.as_str()));
        assert!(proposed.text.contains("把本次请求交给 openai::gpt-image-1.5"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let confirm_second = msg("natural-image-confirm-second", "确认");
        bridge.ingest(&confirm_second).await.unwrap();
        bridge.ingest(&confirm_second).await.unwrap();
        let second = wait_for_version(followup_id.clone(), "completed").await;
        let [second] = second.artifacts.as_slice() else { panic!("second PNG missing"); };
        let second_bytes = std::fs::read(&second.path).unwrap();
        assert_ne!(second.sha256, artifact.sha256);
        assert_eq!(second.sha256, format!("{:x}", Sha256::digest(&second_bytes)));
        assert_eq!(std::fs::read(&artifact.path).unwrap(), bytes);
        let mut reader = png::Decoder::new(second_bytes.as_slice()).read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let frame = reader.next_frame(&mut pixels).unwrap();
        assert_eq!(&pixels[..frame.buffer_size()], &[0, 190, 0, 0, 190, 0]);
        assert!(second_provider.unwrap().1.join().unwrap().contains("隔离绿色封面"));
        assert_eq!(restored.memory_search("默认出图", 0).await.unwrap(), memory_before);
        tokio::time::timeout(Duration::from_secs(20), async {
            let file = json!(second);
            loop {
                let delivered = bridge_save.lock().await.records.iter().any(|record| {
                    record.job_id.as_deref() == Some(followup_id.as_str())
                        && record.file_outcome(&file, std::slice::from_ref(&file))
                            == Some(file_delivery::Outcome::Accepted)
                });
                if delivered {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.unwrap();
        let third = msg("natural-image-third-version", THIRD_IMAGE);
        bridge.ingest(&third).await.unwrap();
        bridge.ingest(&third).await.unwrap();
        let third_id = bridge_save.lock().await.records.last().unwrap().job_id.clone().unwrap();
        assert_ne!(third_id, followup_id);
        let third = wait_for_version(third_id, "awaiting_confirmation").await;
        assert_eq!(third.parent_turn_id.as_deref(), Some(followup_id.as_str()));
        let pending = third.pending.unwrap();
        assert_ne!(pending.id, proposed.pending.unwrap().id);
        assert_eq!(pending.arguments["request"]["input"]["prompt"], "隔离绿色封面的另一版");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(counts.load(Ordering::SeqCst), 9);
        bridge.stop().await.unwrap();
    }
    server.abort();
}
