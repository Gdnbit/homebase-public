use super::*;

#[tokio::test]
async fn wechat_quoted_attachment_failure_resends_only_the_original_version_after_restart() {
    const NOTICE: &str = "9007199254742203";
    const EARLIER: &str = "original-picture-version";
    const LATEST: &str = "new-picture-version";
    let root = tempfile::tempdir().unwrap();
    let art = |folder: &str, bytes: &[u8]| {
        let dir = root.path().join(folder);
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("result.png");
        std::fs::write(&path, bytes).unwrap();
        json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes))})
    };
    let original_bytes = b"original synthetic attachment";
    let original = art("version-one", original_bytes);
    let latest = art("version-two", b"unrelated new synthetic version");
    let (base, seen, server) = fake_api(200, json!({"__media":true,"__drop_first_file_ack":true,
        "__send_message_ids":{"original-picture-client-file-0-error":NOTICE}
    })).await;
    let mut stored = account(&base);
    for (id, client, turn, artifact) in [
        ("version-one-record", "original-picture-client", EARLIER, &original),
        ("version-two-record", "new-picture-client", LATEST, &latest),
    ] {
        stored.records.push(Record {
            id:id.into(), command:"assistant".into(), job_id:Some(turn.into()),
            status:"succeeded".into(), delivery:"pending".into(), client_id:client.into(),
            context:"synthetic".into(), result:Some(json!({"turn_id":turn,"status":"succeeded",
                "text":"已准备好这一版。","artifacts":[artifact]})), ..Default::default()
        });
    }
    let outcome = json!({"status":"succeeded","result":stored.records[0].result.clone().unwrap()});
    let count = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), count.clone()));
    let open = |stored, agent| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), root.path().into(), agent)
    };
    let bridge = open(stored, None);
    bridge.reply_assistant_for("version-one-record", &outcome, true, None, true).await.unwrap();
    let snapshot = saved.lock().await.clone();
    let key = identity::source_key(&snapshot.logical_key(), NOTICE);
    let earlier = &snapshot.records[0];
    let files = outcome["result"]["artifacts"].as_array().unwrap();
    assert_eq!(earlier.file_outcome(&original, files), Some(file_delivery::Outcome::Unknown));
    assert!(earlier.file_quotes.iter().any(|quote| quote.key == key), "lost-ack notice should identify the original attachment");
    let plain = serde_json::to_vec(&snapshot).unwrap();
    #[cfg(windows)]
    let plain = crate::wechat_crypto::crypt(&crate::wechat_crypto::crypt(&plain, true).unwrap(), false).unwrap();
    let agent: Agent = Arc::new(move |request| {
        let outcome = outcome.clone();
        Box::pin(async move {
            assert_eq!(request["operation"], "get");
            assert_eq!(request["turn_id"], EARLIER);
            Ok(outcome)
        })
    });
    let resumed = open(serde_json::from_slice(&plain).unwrap(), Some(agent));
    let mut retry = msg("try-original-notice", "这张没收到，重发一下。");
    retry["item_list"][0]["ref_msg"] = json!({"svr_id":NOTICE,"message_item":{"type":1}});
    resumed.ingest(&retry).await.unwrap();
    resumed.ingest(&retry).await.unwrap();
    let after = saved.lock().await;
    assert_eq!(after.records.last().unwrap().query(), Some((QueryTarget::Turn(EARLIER), true)));
    assert_eq!(after.records[0].file_delivery_view().0, "accepted");
    assert_eq!(after.records[1].delivery, "pending");
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let traffic = seen.lock().await;
    let request = |endpoint| traffic.iter().filter(move |row| {
        row["head"].as_str().is_some_and(|headers| headers.contains(endpoint))
    }).collect::<Vec<_>>();
    let params = request("/getuploadurl ");
    let uploads = request("/c2c/upload?");
    assert_eq!((params.len(), uploads.len()), (2, 2));
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
    for (param, upload) in params.into_iter().zip(uploads) {
        let key = param["body"]["aeskey"].as_str().unwrap();
        let key: Vec<_> = (0..32).step_by(2).map(|i| u8::from_str_radix(&key[i..i+2], 16).unwrap()).collect();
        let bytes = STANDARD.decode(upload["raw_base64"].as_str().unwrap()).unwrap();
        assert_eq!(ecb::Decryptor::<aes::Aes128>::new_from_slice(&key).unwrap()
            .decrypt_padded_vec_mut::<Pkcs7>(&bytes).unwrap(), original_bytes);
    }
    server.abort();
}

#[test]
fn wechat_named_attachment_intent_requires_a_single_filename() {
    for (text, name, resend) in [
        ("请把 result.png 发给我。", "result.png", false),
        ("把「蓝色 封面.PNG」发过来", "蓝色 封面.PNG", false),
        ("只发 footage.mp4", "footage.mp4", false),
        ("重发 result.png", "result.png", true),
        ("麻烦把「蓝色 封面.png」再发给我。", "蓝色 封面.png", true),
    ] {
        assert_eq!(file_delivery::named_intent(text), Some((name, resend)), "{text}");
    }
    for text in [
        "把 result.png 改成 blue.png 发给我",
        "转发链接 https://example.invalid/result.png",
        r"把 C:\fixture\result.png 发给我",
        "把../result.png发给我",
        "重发 result.png？",
        "把结果发给我",
        "发给我 result.png，再剪一下",
        "把 result.png 和 blue.png 发给我",
    ] {
        assert_eq!(file_delivery::named_intent(text), None, "{text}");
    }
}

#[tokio::test]
async fn wechat_numbered_attachment_keeps_the_displayed_file_and_distinguishes_equal_bytes() {
    wechat_single_attachment_keeps_selection_after_restart(false).await;
}

#[tokio::test]
async fn wechat_named_attachment_keeps_selection_after_restart() {
    wechat_single_attachment_keeps_selection_after_restart(true).await;
}

async fn wechat_single_attachment_keeps_selection_after_restart(use_name: bool) {
    const SUMMARY: &str = "9007199254742190";
    const SECOND: &str = "9007199254742192";
    const TURN: &str = "numbered-attachment-original";
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.mp4");
    let second = root.path().join("second.mp4");
    let bytes = b"same bytes in two named attachments";
    std::fs::write(&first, bytes).unwrap();
    std::fs::write(&second, bytes).unwrap();
    let digest = format!("{:x}", Sha256::digest(bytes));
    let result = json!({"turn_id":TURN,"status":"succeeded","text":"已准备两份文件。","artifacts":[
        {"path":first,"sha256":digest},{"path":second,"sha256":digest}
    ]});
    let value = json!({"status":"succeeded","result":result});
    let (base, seen, server) = fake_api(200, json!({"__media":true,"__drop_first_file_ack":true,"__send_message_ids":{
        "numbered-origin-client":SUMMARY,"numbered-origin-client-file-1":SECOND
    }})).await;
    let mut stored = account(&base);
    stored.records.push(Record {
        id:"numbered-origin".into(), command:"assistant".into(), job_id:Some(TURN.into()),
        status:"succeeded".into(), delivery:"pending".into(), result:Some(result),
        client_id:"numbered-origin-client".into(), context:"synthetic".into(), ..Default::default()
    });
    let count = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), count.clone()));
    let open = |stored, agent| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent))
    };
    let entered = Arc::new(Notify::new());
    let selected = file_delivery::FileSelection::from_path(second.to_str().unwrap(), &digest).unwrap();
    let stalled: Agent = Arc::new({ let entered = entered.clone(); let selected = selected.clone(); move |request| {
        let entered = entered.clone();
        let selected = selected.clone();
        Box::pin(async move {
            if use_name && request["operation"] == "resolve_named_file" {
                assert_eq!(request["turn_id"], TURN);
                assert_eq!(request["name"], "second.mp4");
                return Ok(json!({"state":"found","target":TURN,"file":selected}));
            }
            assert_eq!(request["operation"], "get");
            assert_eq!(request["turn_id"], TURN);
            entered.notify_one();
            std::future::pending().await
        })
    }});
    let bridge = open(stored, stalled);
    bridge.reply_assistant_for("numbered-origin", &value, true, None, true).await.unwrap();
    let files = value["result"]["artifacts"].as_array().unwrap();
    let original = saved.lock().await.records[0].clone();
    assert_eq!(original.file_outcome(&files[0], files), Some(file_delivery::Outcome::Unknown));
    assert_eq!(original.file_outcome(&files[1], files), Some(file_delivery::Outcome::Accepted));
    assert_eq!(original.file_delivery_view().0, "attachment_unknown");
    let quote = |id, text, target| {
        let mut message = msg(id, text);
        message["item_list"][0]["ref_msg"] = json!({"svr_id":target});
        message
    };
    let phrase = if use_name { "请把 second.mp4 发给我" } else { "只发第二份" };
    let query = quote("fetch-second", phrase, SUMMARY);
    let request = query.clone();
    let worker = tokio::spawn(async move { bridge.ingest(&request).await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await.unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    let snapshot = saved.lock().await.clone();
    let pending = snapshot.records.last().unwrap();
    assert_eq!(pending.pending_query(), Some((QueryTarget::Turn(TURN), true)));
    assert!(pending.args.get("file_query").is_none() && pending.args.get("current_query").is_none());
    assert_eq!(pending.args["file_request_v1"]["resend"], false);
    let pending_id = pending.id.clone();
    let mut legacy = serde_json::to_value(&snapshot).unwrap();
    legacy["records"][0].as_object_mut().unwrap().remove("attachment_receipts");
    let plain = serde_json::to_vec(&legacy).unwrap();
    #[cfg(windows)]
    let plain = crate::wechat_crypto::crypt(&crate::wechat_crypto::crypt(&plain, true).unwrap(), false).unwrap();
    let restored = serde_json::from_slice(&plain).unwrap();
    let mut updated = value.clone();
    updated["result"]["artifacts"].as_array_mut().unwrap().reverse();
    let resend_selection = file_delivery::FileSelection::from_path(first.to_str().unwrap(), &digest).unwrap();
    let agent: Agent = Arc::new(move |request| {
        let updated = updated.clone();
        let resend_selection = resend_selection.clone();
        Box::pin(async move {
            if request["operation"] == "resolve_current_query" { return Ok(json!({"target":TURN})); }
            if use_name && request["operation"] == "resolve_named_file" {
                assert_eq!(request["turn_id"], TURN);
                assert_eq!(request["name"], "first.mp4");
                return Ok(json!({"state":"found","target":TURN,"file":resend_selection}));
            }
            assert_eq!(request["operation"], "get");
            assert_eq!(request["turn_id"], TURN);
            Ok(updated)
        })
    });
    let bridge = open(restored, agent);
    bridge.resume_saved_query(&pending_id, 0).await.unwrap();
    bridge.ingest(&query).await.unwrap();
    bridge.ingest(&quote("fetch-numbered-first-unknown", "只发第一份", SUMMARY)).await.unwrap();
    bridge.ingest(&quote("fetch-numbered-invalid", "只发第三份", SUMMARY)).await.unwrap();
    bridge.ingest(&quote("choose-number-with-file-reference", "只发第二份", SECOND)).await.unwrap();
    bridge.ingest(&quote("fetch-just-one", "发给我刚才那份", SECOND)).await.unwrap();
    bridge.ingest(&msg("fetch-current-second", "只发第2份")).await.unwrap();
    let resend_request = if use_name { "重发 first.mp4" } else { "重发第一份" };
    bridge.ingest(&quote("resend-first", resend_request, SUMMARY)).await.unwrap();

    let seen = seen.lock().await;
    let file_names = seen.iter().filter_map(|r| r["body"]["msg"]["item_list"][0]["file_item"]["file_name"].as_str()).collect::<Vec<_>>();
    assert_eq!(file_names, ["first.mp4", "second.mp4", "second.mp4", "second.mp4", "second.mp4", "first.mp4"]);
    let texts = seen.iter().filter_map(|r| r["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect::<Vec<_>>();
    assert!(texts.iter().any(|t| t.contains("1. first.mp4\n2. second.mp4")));
    assert!(texts.iter().any(|t| t.contains("这份附件的发送回执待核对")));
    assert!(texts.iter().any(|t| t.contains("没有这一项")));
    assert_eq!(saved.lock().await.records[0].file_delivery_view().0, "accepted");
    assert_eq!(count.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn wechat_resends_only_the_quoted_file_after_reopening_the_encrypted_query() {
    const FIRST: &str = "9007199254741060";
    const SECOND: &str = "9007199254741061";
    const LEGACY: &str = "9007199254741062";
    const TURN: &str = "original-file-turn";
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.mp4");
    let second = root.path().join("second.mp4");
    let bytes = b"identical synthetic bytes under distinct output names";
    std::fs::write(&first, bytes).unwrap();
    std::fs::write(&second, bytes).unwrap();
    let digest = format!("{:x}", Sha256::digest(bytes));
    let result = json!({"turn_id":TURN,"status":"succeeded","text":"已准备两份文件。","artifacts":[
        {"path":first,"sha256":digest},{"path":second,"sha256":digest}
    ]});
    let value = json!({"status":"succeeded","result":result});
    let (base, seen, server) = fake_api(200, json!({"__media":true,"__send_message_ids":{
        "origin-client-file-0":FIRST,"origin-client-file-1":SECOND
    }})).await;
    let mut stored = account(&base);
    stored.records.push(Record {
        id: "origin-file".into(), command: "assistant".into(), job_id: Some(TURN.into()),
        status: "succeeded".into(), delivery: "pending".into(), result: Some(result),
        client_id: "origin-client".into(), context: "synthetic".into(), ..Default::default()
    });
    stored.records.push(Record {
        id: "unrelated-legacy".into(), command: "assistant".into(), job_id: Some("another-turn".into()),
        status: "succeeded".into(), delivery: "accepted".into(),
        outbound_quote_keys: vec![identity::source_key(&stored.logical_key(), LEGACY)],
        ..Default::default()
    });
    let count = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), count.clone()));
    let open = |stored, agent| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent))
    };

    let read_started = Arc::new(Notify::new());
    let started = read_started.clone();
    let stalled: Agent = Arc::new(move |request| {
        let started = started.clone();
        Box::pin(async move {
            assert_eq!(request["operation"], "get");
            assert_eq!(request["turn_id"], TURN);
            assert_eq!(request["include_files"], true);
            started.notify_one();
            std::future::pending().await
        })
    });
    let bridge = open(stored, stalled);
    bridge.reply_assistant_for("origin-file", &value, true, None, true).await.unwrap();
    let quote = |id, phrase, message_id| {
        let mut message = msg(id, phrase);
        message["item_list"][0]["ref_msg"] = json!({"svr_id":message_id,"message_item":{
            "type":4,"file_item":{"file_name":"first.mp4"}
        }});
        message
    };
    let incoming = quote("file-resend", "重发这份！", SECOND);
    let request = incoming.clone();
    let running = tokio::spawn(async move { bridge.ingest(&request).await });
    tokio::time::timeout(Duration::from_secs(4), read_started.notified()).await.unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    let snapshot = saved.lock().await.clone();
    let pending = snapshot.records.last().unwrap();
    assert_eq!(pending.pending_query(), Some((QueryTarget::Turn(TURN), true)));
    assert!(pending.args["file_query"]["selection"].is_object());
    assert!(pending.args.get("current_query").is_none());
    assert!(pending.args.get("numbered_query").is_none());
    assert!(pending.args.get("resend_attachment").is_none());
    let query_id = pending.id.clone();
    let mut legacy = serde_json::to_value(&snapshot).unwrap();
    for record in legacy["records"].as_array_mut().unwrap() {
        let record = record.as_object_mut().unwrap();
        for key in ["outbound_quote_keys", "file_quotes", "attachment_receipts"] {
            record.remove(key);
        }
    }
    let plain = serde_json::to_vec(&legacy).unwrap();
    #[cfg(windows)]
    let plain = {
        let sealed = crate::wechat_crypto::crypt(&plain, true).unwrap();
        crate::wechat_crypto::crypt(&sealed, false).unwrap()
    };
    let snapshot = serde_json::from_slice(&plain).unwrap();

    let payload = Arc::new(Mutex::new(value.clone()));
    let immutable_read = payload.clone();
    let good: Agent = Arc::new(move |request| {
        let payload = immutable_read.clone();
        Box::pin(async move {
            assert_eq!(request["operation"], "get", "resume must never select latest or execute again");
            assert_eq!(request["turn_id"], TURN);
            Ok(payload.lock().await.clone())
        })
    });
    let restarted = open(snapshot, good);
    restarted.resume_saved_query(&query_id, restarted.generation.load(Ordering::SeqCst)).await.unwrap();
    restarted.ingest(&incoming).await.unwrap();
    for (id, conflicting) in [("same-task-conflict", FIRST), ("other-task-conflict", LEGACY)] {
        let mut message = quote(id, "重发这份", SECOND);
        message["item_list"][0]["ref_msg"]["message_item"]["msg_id"] = json!(conflicting);
        restarted.ingest(&message).await.unwrap();
    }
    restarted.ingest(&quote("old-file", "重新发送附件", LEGACY)).await.unwrap();
    restarted.ingest(&msg("missing-reference", "重发这份")).await.unwrap();
    payload.lock().await["result"]["artifacts"][1]["sha256"] = json!("f".repeat(64));
    restarted.ingest(&quote("changed-result", "重发这份", SECOND)).await.unwrap();
    *payload.lock().await = value;
    std::fs::write(&second, b"changed on disk").unwrap();
    restarted.ingest(&quote("changed-on-disk", "重发这份", SECOND)).await.unwrap();

    let traffic = seen.lock().await;
    let names = traffic.iter().filter_map(|request| request["body"]["msg"]["item_list"][0]["file_item"]["file_name"].as_str()).collect::<Vec<_>>();
    assert_eq!(names, ["first.mp4", "second.mp4", "second.mp4"]);
    let text = traffic.iter().filter_map(|request| request["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect::<Vec<_>>();
    assert!(text.iter().any(|text| text.contains(file_delivery::FILE_UNAVAILABLE)));
    assert!(text.iter().any(|text| text.contains("附件哈希与任务结果不一致")));
    assert!(text.iter().any(|text| text.contains("请引用你想重发的那份文件")));
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(first).unwrap().as_slice(), bytes);
    server.abort();
}
