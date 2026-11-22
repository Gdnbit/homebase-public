use super::*;

fn original(base: &str) -> Stored {
    let mut stored = account(base);
    stored.enabled = false;
    stored.cursor = "old-cursor".into();
    stored.records.push(Record {
        id: format!(
            "wx-{:x}",
            Sha256::digest("synthetic-bot:synthetic-owner:old-message")
        ),
        command: "assistant".into(),
        status: "failed".into(),
        job_id: Some("original-turn".into()),
        delivery: "unknown".into(),
        context: "old-private-context".into(),
        args: json!({"text":"original-private-message"}),
        result: Some(json!({"status":"failed"})),
        ..Default::default()
    });
    stored.notices.push(NoticeReceipt {
        owner: stored.owner.clone(),
        group: "same-topic".into(),
        fingerprint: "already-notified".into(),
    });
    stored
}
async fn wait_for(bridge: &Bridge, predicate: impl Fn(&Status, &PairingView) -> bool) {
    let end = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        if predicate(&bridge.status().await, &bridge.pairing().await) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < end,
            "synthetic protocol deadline"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
fn pending(generation: u64) -> Pairing {
    Pairing {
        generation,
        code: "synthetic-qr".into(),
        base: ORIGIN.into(),
        pin: None,
        view: PairingView {
            state: "need_pin".into(),
            message: "synthetic".into(),
            qr_data_url: None,
        },
    }
}

#[tokio::test]
async fn wechat_inbound_identity_preserves_one_receipt_across_shapes_restart_and_pairing() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let count = Arc::new(AtomicUsize::new(0));
    let control = controller(tmp.path(), count.clone());
    let (bridge, memory) = test_bridge(account(&base), tmp.path(), control.clone());
    let mut fallback = msg("unused", "帮助");
    fallback.as_object_mut().unwrap().remove("message_id");
    bridge.ingest(&fallback).await.unwrap();
    assert!(memory.lock().await.records.is_empty());
    fallback["item_list"][0]["msg_id"] = json!(9007199254740993u64);
    let mut foreign = fallback.clone();
    foreign["from_user_id"] = json!("unbound-sender");
    bridge.ingest(&foreign).await.unwrap();
    assert!(memory.lock().await.records.is_empty());

    bridge.ingest(&fallback).await.unwrap();
    let receipt = {
        let ledger = memory.lock().await;
        assert_eq!(ledger.records.len(), 1, "the item ID is sufficient for a receipt");
        ledger.records[0].id.clone()
    };
    let mut both = fallback.clone();
    both["message_id"] = json!(9007199254740995u64);
    both["item_list"][0]["msg_id"] = json!("9007199254740993");
    bridge.ingest(&both).await.unwrap();
    let ledger = memory.lock().await.clone();
    assert_eq!(ledger.records.len(), 1);
    assert_eq!(ledger.records[0].id, receipt);
    let cipher = crate::wechat_crypto::crypt(&serde_json::to_vec(&ledger).unwrap(), true).unwrap();
    let plain = crate::wechat_crypto::crypt(&cipher, false).unwrap();
    let (restarted, memory) = test_bridge(serde_json::from_slice(&plain).unwrap(), tmp.path(), control.clone());
    let top_only = msg("9007199254740995", "帮助");
    restarted.ingest(&top_only).await.unwrap();
    assert_eq!(memory.lock().await.records.len(), 1);
    restarted.state.lock().await.pairing = Some(pending(0));
    restarted.commit_pairing(0, base.clone(), "new-test-token", "new-test-bot", "synthetic-owner").await.unwrap();
    let mut paired_ledger = memory.lock().await.clone();
    paired_ledger.enabled = true;
    let (rebound, memory) = test_bridge(paired_ledger, tmp.path(), control);
    let mut redelivered = top_only;
    redelivered["to_user_id"] = json!("new-test-bot");
    redelivered["context_token"] = json!("new-reply-context");
    rebound.ingest(&redelivered).await.unwrap();
    {
        let ledger = memory.lock().await;
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(ledger.records[0].id, receipt);
        assert_eq!(ledger.records[0].context, "synthetic-context");
        assert!(ledger.reply_context_pending);
    }
    redelivered.as_object_mut().unwrap().remove("message_id");
    redelivered["item_list"][0]["msg_id"] = json!("9007199254740996");
    rebound.ingest(&redelivered).await.unwrap();
    let before_conflict = memory.lock().await.clone();
    assert_eq!(before_conflict.records.len(), 2);
    assert!(!before_conflict.reply_context_pending);
    redelivered["message_id"] = json!("9007199254740995");
    rebound.ingest(&redelivered).await.unwrap();
    assert_eq!(
        serde_json::to_value(&*memory.lock().await).unwrap(),
        serde_json::to_value(before_conflict).unwrap(),
        "IDs already tied to different receipts cannot merge them or cause a new action"
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(seen.lock().await.len(), 2, "each distinct incoming message gets one reply");
    server.abort();
}

#[tokio::test]
async fn reauthorization_keeps_original_task_but_retires_unsent_old_context_query() {
    let tmp = tempfile::tempdir().unwrap();
    let mut stored = original("http://127.0.0.1:1");
    let task = serde_json::to_value(&stored.records[0]).unwrap();
    stored.records.push(Record {
        id: "pending-query".into(), command: "message".into(),
        status: "received".into(), delivery: "pending".into(),
        args: json!({"numbered_query":{"target":"original-turn","files":true}}),
        ..Default::default()
    });
    stored.records.push(Record {
        id: "pending-video-question".into(), command: "assistant".into(),
        status: "received".into(), delivery: "pending".into(),
        args: json!({"video_receipt":"original-video","text":"请总结视频"}),
        ..Default::default()
    });
    stored.records.push(Record {
        id: "pending-voice-question".into(), command: "assistant".into(),
        status: "received".into(), delivery: "pending".into(),
        args: json!({"voice_receipt":"original-voice","text":"请总结语音"}),
        ..Default::default()
    });
    stored.records.push(Record {
        id: "original-voice".into(), command: voice::COMMAND.into(),
        status: "running".into(), delivery: "pending".into(),
        args: json!({"phase":"submitted"}),
        ..Default::default()
    });
    let completion = json!({"text":"synthetic finished result","artifacts":[]});
    stored.records.push(Record {
        id: "pending-completion".into(), command: "assistant".into(),
        status: "succeeded".into(), delivery: "pending".into(),
        job_id: Some("finished-turn".into()), result: Some(completion.clone()),
        context: "old-private-context".into(), client_id: "original-update".into(),
        ..Default::default()
    });
    stored.records.push(Record {
        id: "pending-control-completion".into(), command: "capability_render".into(),
        status: "succeeded".into(), delivery: "pending".into(),
        job_id: Some("finished-control-job".into()), result: Some(completion.clone()),
        args: json!({(COMPLETION_REPLY):"原控制结果通知","input":{"title":"fixture"}}),
        context: "old-private-context".into(), client_id: "original-control-update".into(),
        ..Default::default()
    });
    let source = stored.logical_key();
    let media_result = json!({"text":"合成的原文字稿","artifact":null});
    stored.records.push(Record {
        id: "pending-media-completion".into(), command: voice::COMMAND.into(),
        status: "completed".into(), delivery: "pending".into(),
        args: json!({(COMPLETION_REPLY):true,"phase":"submitted","source":source}),
        result: Some(media_result.clone()),
        context: "old-private-context".into(), client_id: "original-media-update".into(),
        ..Default::default()
    });
    let pdf_result = json!({"text":"合成的原 PDF 预览"});
    stored.records.push(Record {
        id: "pending-pdf-completion".into(), command: "wechat_pdf".into(),
        status: "completed".into(), delivery: "pending".into(),
        args: json!({(COMPLETION_REPLY):true,"source":source,"name":"合成.pdf"}),
        result: Some(pdf_result.clone()),
        context: "old-private-context".into(), client_id: "original-pdf-update".into(),
        ..Default::default()
    });
    let (bridge, saved) = test_bridge(stored, tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
    bridge.state.lock().await.pairing = Some(pending(0));
    bridge.commit_pairing(0, ORIGIN.into(), "replacement-test-token", "new-test-bot", "synthetic-owner").await.unwrap();
    let snapshot = saved.lock().await;
    assert_eq!(serde_json::to_value(&snapshot.records[0]).unwrap(), task);
    assert_eq!(snapshot.records[1].status, "canceled");
    assert_eq!(snapshot.records[1].delivery, "not_sent");
    assert!(snapshot.records[1].pending_query().is_none());
    assert_eq!(snapshot.records[1].numbered_query(), Some(("original-turn",true)));
    assert_eq!(snapshot.records[2].status, "canceled");
    assert_eq!(snapshot.records[2].delivery, "not_sent");
    assert_eq!(snapshot.records[2].args["video_receipt"], "original-video");
    assert_eq!(snapshot.records[3].status, "canceled");
    assert_eq!(snapshot.records[3].delivery, "not_sent");
    assert_eq!(snapshot.records[3].args["voice_receipt"], "original-voice");
    assert_eq!(snapshot.records[4].status, "running");
    assert_eq!(snapshot.records[4].args["phase"], "submitted");
    assert_eq!(snapshot.records[4].args["delivery_allowed"], false);
    assert_eq!(snapshot.records[5].status, "succeeded");
    assert_eq!(snapshot.records[5].delivery, "not_sent");
    assert_eq!(snapshot.records[5].result.as_ref(), Some(&completion));
    assert_eq!(snapshot.records[5].job_id.as_deref(), Some("finished-turn"));
    assert_eq!(snapshot.records[5].client_id, "original-update");
    assert!(!snapshot.records[5].pending_assistant_completion());
    assert_eq!(snapshot.records[6].status, "succeeded");
    assert_eq!(snapshot.records[6].delivery, "not_sent");
    assert_eq!(snapshot.records[6].result.as_ref(), Some(&completion));
    assert_eq!(snapshot.records[6].job_id.as_deref(), Some("finished-control-job"));
    assert_eq!(snapshot.records[6].client_id, "original-control-update");
    assert!(snapshot.records[6].args.get(COMPLETION_REPLY).is_none());
    assert_eq!(snapshot.records[6].args["input"]["title"], "fixture");
    assert!(snapshot.records[6].pending_control_completion().is_none());
    assert_eq!(snapshot.records[7].status, "completed");
    assert_eq!(snapshot.records[7].delivery, "not_sent");
    assert_eq!(snapshot.records[7].args["source"], source);
    assert_eq!(snapshot.records[7].result.as_ref(), Some(&media_result));
    assert_eq!(snapshot.records[7].client_id, "original-media-update");
    assert!(snapshot.records[7].args.get(COMPLETION_REPLY).is_none());
    assert!(!snapshot.records[7].pending_media_completion());
    assert_eq!(snapshot.records[8].status, "completed");
    assert_eq!(snapshot.records[8].delivery, "not_sent");
    assert_eq!(snapshot.records[8].args["source"], source);
    assert_eq!(snapshot.records[8].result.as_ref(), Some(&pdf_result));
    assert_eq!(snapshot.records[8].client_id, "original-pdf-update");
    assert!(!snapshot.records[8].pending_pdf_completion());
}
#[tokio::test]
async fn stale_http_token_preserves_originals_and_survives_encrypted_restart_without_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({"__updates_response":{"errcode":-14}})).await;
    let mut stored = original(&base);
    stored.enabled = true;
    let history = serde_json::to_value(&stored.records).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let ctl = controller(tmp.path(), count.clone());
    let (bridge, memory) = test_bridge(stored, tmp.path(), ctl.clone());
    bridge.start().await.unwrap();
    wait_for(&bridge, |s, _| s.state == "expired").await;
    assert!(!bridge.status().await.enabled);
    assert!(bridge.status().await.bound);
    let saved = memory.lock().await.clone();
    assert!(saved.reauth_required);
    assert_eq!(serde_json::to_value(&saved.records).unwrap(), history);
    assert_eq!(saved.cursor, "old-cursor");
    let cipher = crate::wechat_crypto::crypt(&serde_json::to_vec(&saved).unwrap(), true).unwrap();
    let plain = crate::wechat_crypto::crypt(&cipher, false).unwrap();
    let roundtrip = serde_json::from_slice(&plain).unwrap();
    let (restarted, _) = test_bridge(roundtrip, tmp.path(), ctl);
    assert_eq!(restarted.status().await.state, "expired");
    assert!(restarted.start().await.is_err());
    assert_eq!(seen.lock().await.len(), 1);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    let public = serde_json::to_string(&restarted.status().await).unwrap();
    for marker in [
        "synthetic-token",
        "synthetic-owner",
        "old-private-context",
        "original-private-message",
    ] {
        assert!(!public.contains(marker));
    }
    server.abort();
}
#[tokio::test]
async fn explicit_pairing_cancel_and_duplicate_clicks_preserve_existing_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({"__pairing":true})).await;
    let stored = original(&base);
    let before = serde_json::to_value(&stored).unwrap();
    let (bridge, memory) = test_bridge(
        stored,
        tmp.path(),
        controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
    );
    let (a, b) = tokio::join!(bridge.begin(), bridge.begin());
    a.unwrap();
    b.unwrap();
    wait_for(&bridge, |_, p| p.state == "need_pin").await;
    assert!(bridge.start().await.is_err());
    bridge.cancel_pairing().await.unwrap();
    assert_eq!(bridge.pairing().await.state, "none");
    assert_eq!(serde_json::to_value(&*memory.lock().await).unwrap(), before);
    let calls = seen.lock().await;
    assert_eq!(
        calls
            .iter()
            .filter(|r| r["head"].as_str().unwrap().contains("/get_bot_qrcode?"))
            .count(),
        1
    );
    let qr = calls
        .iter()
        .find(|r| r["head"].as_str().unwrap().contains("/get_bot_qrcode?"))
        .unwrap();
    assert_eq!(qr["body"]["local_token_list"], json!(["synthetic-token"]));
    server.abort();
}
#[tokio::test]
async fn same_owner_reauthorization_preserves_history_dedup_and_logical_channel() {
    for bot in ["synthetic-bot", "rotated-bot"] {
        let tmp = tempfile::tempdir().unwrap();
        let(base,_,server)=fake_api(200,json!({"__pairing":true,"__pairing_confirmation":{"status":"confirmed","bot_token":"replacement-token","ilink_bot_id":bot,"ilink_user_id":"synthetic-owner","baseurl":ORIGIN}})).await;
        let mut stored = original(&base);
        stored.reauth_required = true;
        let key = stored.logical_key();
        let old_records = serde_json::to_value(&stored.records).unwrap();
        let notices = serde_json::to_value(&stored.notices).unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let (bridge, memory) =
            test_bridge(stored, tmp.path(), controller(tmp.path(), count.clone()));
        bridge.begin().await.unwrap();
        wait_for(&bridge, |_, p| p.state == "need_pin").await;
        bridge.pin("123456".into()).await.unwrap();
        wait_for(&bridge, |s, _| s.state == "connected").await;
        {
            let saved = memory.lock().await;
            assert_eq!(saved.token, "replacement-token");
            assert_eq!(saved.bot, bot);
            assert!(!saved.reauth_required);
            assert!(saved.reply_context_pending);
            assert_eq!(serde_json::to_value(&saved.records).unwrap(), old_records);
            assert_eq!(serde_json::to_value(&saved.notices).unwrap(), notices);
        }
        assert_eq!(bridge.source_key().await, key);
        assert!(bridge
            .self_test_message_inner("进度", "hb-selftest-reauth-context")
            .await
            .is_err());
        if bot == "synthetic-bot" {
            bridge.ingest(&msg("old-message", "系统")).await.unwrap();
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(memory.lock().await.reply_context_pending);
        }
        let mut fresh = msg("new-context", "帮助");
        fresh["to_user_id"] = json!(bot);
        fresh["context_token"] = json!("new-context");
        bridge.ingest(&fresh).await.unwrap();
        assert!(!memory.lock().await.reply_context_pending);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert_eq!(memory.lock().await.records.len(), 2);
        assert_eq!(bridge.source_key().await, key);
        bridge.stop().await.unwrap();
        server.abort();
    }
}
#[tokio::test]
async fn other_owner_is_rejected_without_mutating_original_credentials_or_ledger() {
    let tmp = tempfile::tempdir().unwrap();
    let(base,_,server)=fake_api(200,json!({"__pairing":true,"__pairing_confirmation":{"status":"confirmed","bot_token":"outsider-token","ilink_bot_id":"other-bot","ilink_user_id":"other-owner","baseurl":ORIGIN}})).await;
    let stored = original(&base);
    let before = serde_json::to_value(&stored).unwrap();
    let (bridge, memory) = test_bridge(
        stored,
        tmp.path(),
        controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
    );
    bridge.begin().await.unwrap();
    wait_for(&bridge, |_, p| p.state == "need_pin").await;
    bridge.pin("123456".into()).await.unwrap();
    wait_for(&bridge, |_, p| p.state == "error").await;
    assert!(bridge.pairing().await.message.contains("本人不一致"));
    assert_eq!(serde_json::to_value(&*memory.lock().await).unwrap(), before);
    assert!(!bridge.status().await.enabled);
    server.abort();
}
#[tokio::test]
async fn stale_and_authorization_save_failures_leave_original_ledger_intact() {
    for operation in ["expire", "authorize"] {
        let tmp = tempfile::tempdir().unwrap();
        let stored = original("http://127.0.0.1:1");
        let before = serde_json::to_value(&stored).unwrap();
        let (template, _) = test_bridge(
            stored.clone(),
            tmp.path(),
            controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
        );
        let bridge = Bridge::new(
            stored,
            Api::new().unwrap(),
            Arc::new(|_| Box::pin(async { Err("private-write-error".into()) })),
            template.control.clone(),
            tmp.path().into(),
        );
        if operation == "expire" {
            bridge.mark_stale(0).await;
            assert_eq!(bridge.status().await.state, "error");
            assert!(!bridge
                .status()
                .await
                .message
                .contains("private-write-error"));
        } else {
            bridge.state.lock().await.pairing = Some(pending(0));
            assert!(bridge
                .commit_pairing(
                    0,
                    ORIGIN.into(),
                    "new-token",
                    "synthetic-bot",
                    "synthetic-owner"
                )
                .await
                .is_err());
        }
        assert_eq!(
            serde_json::to_value(&bridge.state.lock().await.stored).unwrap(),
            before
        );
    }
}
#[tokio::test]
async fn cancelled_authorization_cannot_commit_start_or_change_replacement_dialog() {
    let tmp = tempfile::tempdir().unwrap();
    let stored = original("http://127.0.0.1:1");
    let before = serde_json::to_value(&stored).unwrap();
    let (bridge, memory) = test_bridge(
        stored,
        tmp.path(),
        controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
    );
    bridge.state.lock().await.pairing = Some(pending(0));
    let gate = bridge.submit_lock.lock().await;
    let b = bridge.clone();
    let late = tokio::spawn(async move {
        b.commit_pairing(
            0,
            ORIGIN.into(),
            "late-token",
            "synthetic-bot",
            "synthetic-owner",
        )
        .await
    });
    bridge.cancel_pairing().await.unwrap();
    drop(gate);
    assert!(late.await.unwrap().is_err());
    bridge.state.lock().await.pairing = Some(pending(1));
    bridge.pair_error(0, "error", "late private error").await;
    assert_eq!(bridge.pairing().await.state, "need_pin");
    assert!(bridge.start_checked(Some(0)).await.is_err());
    assert_eq!(serde_json::to_value(&*memory.lock().await).unwrap(), before);
}
#[tokio::test]
async fn wechat_expired_authorization_recovers_with_original_assistant_watch_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({
        "__pairing":true,
        "__pairing_confirmation":{
            "status":"confirmed","bot_token":"replacement-token",
            "ilink_bot_id":"rotated-bot","ilink_user_id":"synthetic-owner","baseurl":ORIGIN
        },
        "__media":true
    })).await;
    let mut stored = original(&base);
    stored.reauth_required = true;
    let source = stored.logical_key();
    let watch = &mut stored.records[0];
    watch.status = "running".into();
    watch.delivery = "accepted".into();
    watch.client_id = "old-original-client".into();
    watch.created_at = (chrono::Utc::now() - chrono::Duration::seconds(30)).to_rfc3339();
    watch.result = None;
    let receipt = watch.id.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(
        stored.clone(), root.path(), controller(root.path(), calls.clone()),
    );
    let path = root.path().join("finished.txt");
    let bytes = b"original output after expired authorization";
    std::fs::write(&path, bytes).unwrap();
    let artifact = json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes))});
    let phase = Arc::new(AtomicUsize::new(0));
    let watch_reads = Arc::new(AtomicUsize::new(0));
    let explicit_reads = Arc::new(AtomicUsize::new(0));
    let agent: Agent = {
        let (phase, watch_reads, explicit_reads, source, artifact) = (
            phase.clone(), watch_reads.clone(), explicit_reads.clone(), source.clone(), artifact.clone(),
        );
        Arc::new(move |request| {
            let (phase, watch_reads, explicit_reads, source, artifact) = (
                phase.clone(), watch_reads.clone(), explicit_reads.clone(), source.clone(), artifact.clone(),
            );
            Box::pin(async move {
                assert_eq!(request["source_key"], source);
                if request["operation"] == "notifications" {
                    return Ok(json!([]));
                }
                assert_eq!(request["operation"], "get");
                assert_eq!(request["turn_id"], "original-turn");
                if request["include_files"] == true {
                    explicit_reads.fetch_add(1, Ordering::SeqCst);
                } else {
                    watch_reads.fetch_add(1, Ordering::SeqCst);
                }
                Ok(match phase.load(Ordering::SeqCst) {
                    0 => json!({"status":"running","result":{"turn_id":"original-turn","progress_signature":"step-a","progress":"旧任务的进度"}}),
                    1 => json!({"status":"awaiting_confirmation","result":{"turn_id":"original-turn","text":"旧任务需要确认","progress_signature":"step-b"}}),
                    _ => json!({"status":"succeeded","result":{"turn_id":"original-turn","text":"旧任务的原结果","artifacts":[artifact]}}),
                })
            })
        })
    };
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let bridge = Bridge::new_with_agent(
        stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent),
    );
    assert!(bridge.start().await.is_err());
    bridge.begin().await.unwrap();
    wait_for(&bridge, |_, pairing| pairing.state == "need_pin").await;
    bridge.pin("123456".into()).await.unwrap();
    wait_for(&bridge, |status, _| status.state == "connected").await;
    {
        let state = saved.lock().await;
        assert!(state.reply_context_pending);
        assert_eq!(state.logical_key(), source);
        assert_eq!(state.records[0].status, "running");
        assert_eq!(state.records[0].delivery, "accepted");
    }
    let mut fresh = msg("new-message", "帮助");
    fresh["to_user_id"] = json!("rotated-bot");
    fresh["context_token"] = json!("current-context");
    bridge.ingest(&fresh).await.unwrap();
    wait_for(&bridge, |_, _| watch_reads.load(Ordering::SeqCst) >= 2).await;
    phase.store(1, Ordering::SeqCst);
    wait_for(&bridge, |status, _| status.tasks.iter().any(|task| {
        task.id == receipt && task.status == "awaiting_confirmation"
    })).await;
    phase.store(2, Ordering::SeqCst);
    wait_for(&bridge, |status, _| status.tasks.iter().any(|task| {
        task.id == receipt && task.status == "succeeded" && task.delivery == "not_sent"
    })).await;
    let mut lookup = msg("new-result-lookup", "/结果 original-turn");
    lookup["to_user_id"] = json!("rotated-bot");
    lookup["context_token"] = json!("lookup-context");
    bridge.ingest(&lookup).await.unwrap();
    bridge.stop().await.unwrap();
    {
        let state = saved.lock().await;
        assert_eq!(state.records.len(), 3);
        assert_eq!(state.records[0].job_id.as_deref(), Some("original-turn"));
        assert_eq!(state.records[0].result.as_ref().unwrap()["artifacts"][0], artifact);
        assert_eq!(state.records[0].progress_count, 0);
        assert_eq!(state.records[2].delivery, "accepted", "{:?}", state.records[2].delivery_error);
    }
    let traffic = seen.lock().await;
    let sends: Vec<_> = traffic.iter().filter(|call| {
        call["head"].as_str().unwrap().contains("/sendmessage ")
    }).collect();
    assert_eq!(sends.len(), 3);
    assert_eq!(sends[0]["body"]["msg"]["context_token"], "current-context");
    assert_eq!(sends[1]["body"]["msg"]["context_token"], "lookup-context");
    assert_eq!(sends[1]["body"]["msg"]["item_list"][0]["text_item"]["text"], "旧任务的原结果");
    assert_eq!(sends[2]["body"]["msg"]["context_token"], "lookup-context");
    assert_eq!(sends[2]["body"]["msg"]["item_list"][0]["type"], 4);
    assert_eq!(explicit_reads.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.abort();
}
#[tokio::test]
async fn wechat_expired_authorization_restores_submitted_control_and_media() {
    let root = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({
        "__pairing":true,"__media":true,
        "__pairing_confirmation":{
            "status":"confirmed","bot_token":"replacement-token",
            "ilink_bot_id":"rotated-bot","ilink_user_id":"synthetic-owner","baseurl":ORIGIN
        }
    })).await;
    let mut stored = account(&base);
    let source = stored.logical_key();
    let media_id = format!("wx-{}", "d".repeat(64));
    stored.records.push(Record {
        id: media_id.clone(),
        command: "wechat_video_transcript".into(),
        status: "running".into(),
        args: json!({"phase":"submitted","source":source}),
        context: "old-media-context".into(),
        client_id: "old-media-client".into(),
        delivery: "pending".into(),
        ..Default::default()
    });
    let artifact = |name, bytes: &[u8]| {
        let path = root.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes))})
    };
    let output = artifact("control.mp4", b"synthetic original video");
    let transcript = artifact("transcript.txt", b"synthetic original transcript");
    let calls = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let runner: Runner = {
        let (release, calls, output) = (release.clone(), calls.clone(), output.clone());
        Arc::new(move |name, _| {
            let (release, calls, output) = (release.clone(), calls.clone(), output.clone());
            Box::pin(async move {
                assert_eq!(name, "capability_render");
                calls.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
                Ok(json!({"status":"succeeded","artifact":output}))
            })
        })
    };
    let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let ctl = Controller::new(catalog, root.path(), runner).unwrap();
    let (bridge, saved) = test_bridge(stored, root.path(), ctl);
    let media_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observations = Arc::new(AtomicUsize::new(0));
    let inbox: video::VideoInbox = {
        let (media_ready, observations, media_id, transcript) =
            (media_ready.clone(), observations.clone(), media_id.clone(), transcript.clone());
        Arc::new(move |request| {
            let (media_ready, observations, media_id, transcript) =
                (media_ready.clone(), observations.clone(), media_id.clone(), transcript.clone());
            Box::pin(async move {
                assert!(matches!(request, video::VideoRequest::Observe { id } if id == media_id));
                observations.fetch_add(1, Ordering::SeqCst);
                if media_ready.load(Ordering::SeqCst) {
                    video::VideoResult::Ready { preview: "原转写文本".into(), artifact: transcript, repositories: Default::default() }
                } else {
                    video::VideoResult::Running
                }
            })
        })
    };
    bridge.configure_video(inbox);
    let mut initial = msg("initial-render", "视频 合成动画");
    initial["context_token"] = json!("old-control-context");
    bridge.ingest(&initial).await.unwrap();
    let (control_id, job) = {
        let state = saved.lock().await;
        let task = &state.records[1];
        assert_eq!(task.status, "running");
        (task.id.clone(), task.job_id.clone().unwrap())
    };
    bridge.mark_stale(0).await;
    assert_eq!(bridge.status().await.state, "expired");
    bridge.begin().await.unwrap();
    wait_for(&bridge, |_, pairing| pairing.state == "need_pin").await;
    bridge.pin("123456".into()).await.unwrap();
    wait_for(&bridge, |status, _| status.state == "connected").await;
    let mut fresh = msg("fresh-context", "帮助");
    fresh["to_user_id"] = json!("rotated-bot");
    fresh["context_token"] = json!("fresh-context");
    bridge.ingest(&fresh).await.unwrap();
    media_ready.store(true, Ordering::SeqCst);
    release.notify_one();
    wait_for(&bridge, |status, _| {
        status.tasks.iter().any(|r| r.id == media_id && r.status == "completed" && r.delivery == "not_sent")
            && status.tasks.iter().any(|r| r.id == control_id && r.status == "succeeded" && r.delivery == "not_sent")
    }).await;
    for (message, target, context) in [
        ("control-result", &control_id, "current-control-query"),
        ("media-result", &media_id, "current-media-query"),
    ] {
        let mut request = msg(message, &format!("/结果 {target}"));
        request["to_user_id"] = json!("rotated-bot");
        request["context_token"] = json!(context);
        bridge.ingest(&request).await.unwrap();
    }
    bridge.stop().await.unwrap();
    let state = saved.lock().await;
    assert_eq!(state.logical_key(), source);
    assert_eq!(state.records.len(), 5);
    assert_eq!(state.records[0].result.as_ref().unwrap()["artifact"], transcript);
    assert_eq!(state.records[1].job_id.as_deref(), Some(job.as_str()));
    assert_eq!(state.records[1].result.as_ref().unwrap()["artifact"], output);
    assert_eq!(state.records[3].delivery, "accepted", "{:?}", state.records[3].delivery_error);
    assert_eq!(state.records[4].delivery, "accepted", "{:?}", state.records[4].delivery_error);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(observations.load(Ordering::SeqCst) >= 1);
    let traffic = seen.lock().await;
    let sent: Vec<_> = traffic.iter().filter(|call| {
        call["head"].as_str().unwrap().contains("/sendmessage ")
    }).map(|call| &call["body"]["msg"]).collect();
    assert_eq!(sent.len(), 6);
    assert_eq!(sent[0]["context_token"], "old-control-context");
    assert_eq!(sent[1]["context_token"], "fresh-context");
    assert_eq!(sent[3]["context_token"], "current-control-query");
    assert_eq!(sent[3]["item_list"][0]["file_item"]["file_name"], "control.mp4");
    assert_eq!(sent[5]["context_token"], "current-media-query");
    assert_eq!(sent[5]["item_list"][0]["file_item"]["file_name"], "transcript.txt");
    server.abort();
}
#[tokio::test]
async fn wechat_pause_detaches_control_and_media_observations_without_writing_results() {
    for media in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        let receipt = format!("wx-{}", "c".repeat(64));
        stored.records.push(Record {
            id: receipt.clone(),
            command: if media { "wechat_video_transcript" } else { "capability_render" }.into(),
            status: "running".into(),
            job_id: (!media).then(|| "original-control-job".into()),
            args: if media {
                json!({"phase":"submitted","source":stored.logical_key()})
            } else {
                json!({})
            },
            context: "old-context".into(),
            client_id: "original-client".into(),
            delivery: "pending".into(),
            ..Default::default()
        });
        let before = serde_json::to_value(&stored.records).unwrap();
        let (template, saved) = test_bridge(
            stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let control: Control = {
            let (entered, release) = (entered.clone(), release.clone());
            Arc::new(move |request| {
                let (entered, release) = (entered.clone(), release.clone());
                Box::pin(async move {
                    assert!(!media);
                    assert_eq!(request.method, "jobs.get");
                    assert_eq!(request.job_id.as_deref(), Some("original-control-job"));
                    entered.notify_one();
                    release.notified().await;
                    Ok(json!({"status":"succeeded","result":{"text":"迟到的控制结果"}}))
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new(stored, api, template.save.clone(), control, root.path().into());
        if media {
            let (entered, release) = (entered.clone(), release.clone());
            bridge.configure_video(Arc::new(move |request| {
                let (entered, release, receipt) = (entered.clone(), release.clone(), receipt.clone());
                Box::pin(async move {
                    assert!(matches!(request, video::VideoRequest::Observe { id } if id == receipt));
                    entered.notify_one();
                    release.notified().await;
                    video::VideoResult::Failed("迟到的本机结果".into())
                })
            }));
        }
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), entered.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await
            .unwrap_or_else(|_| panic!("paused observer did not exit: media={media}"))
            .unwrap();
        release.notify_one();
        assert_eq!(serde_json::to_value(&saved.lock().await.records).unwrap(), before);
        assert!(seen.lock().await.is_empty());
        server.abort();
    }
}
#[tokio::test]
async fn wechat_control_watch_retries_original_lookup_after_a_temporary_failure() {
    let root = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let mut stored = account(&base);
    stored.records.push(Record {
        id: "retry-control-receipt".into(),
        command: "capability_render".into(),
        status: "running".into(),
        job_id: Some("original-control-job".into()),
        context: "synthetic-context".into(),
        client_id: "original-control-client".into(),
        delivery: "accepted".into(),
        ..Default::default()
    });
    let (template, saved) = test_bridge(
        stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let control: Control = {
        let reads = reads.clone();
        Arc::new(move |request| {
            let reads = reads.clone();
            Box::pin(async move {
                assert_eq!(request.method, "jobs.get");
                assert_eq!(request.job_id.as_deref(), Some("original-control-job"));
                match reads.fetch_add(1, Ordering::SeqCst) {
                    0 => Err(ControlError::new("TEMPORARY", "synthetic controller unavailable")),
                    1 => Ok(json!({"status":"running"})),
                    2 => Ok(json!({"status":"succeeded","result":{"status":"succeeded","text":"原任务已完成"}})),
                    _ => panic!("completed task must not be polled again"),
                }
            })
        })
    };
    let make_bridge = |stored| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new(stored, api, template.save.clone(), control.clone(), root.path().into())
    };
    let bridge = make_bridge(stored);
    let monitor = tokio::spawn(bridge.clone().monitor(0));
    let snapshot = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let state = saved.lock().await.clone();
            if state.records[0].query_failures != 0 || state.records[0].status != "running" {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }).await.unwrap();
    assert_eq!(snapshot.records[0].status, "running");
    assert_eq!(snapshot.records[0].query_failures, 1);
    assert!(snapshot.records[0].query_retry_at > chrono::Utc::now().timestamp());
    assert!(seen.lock().await.is_empty());
    bridge.stop().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();

    let snapshot = serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let restored = make_bridge(snapshot);
    let monitor = tokio::spawn(restored.clone().monitor(0));
    tokio::time::sleep(Duration::from_millis(125)).await;
    assert_eq!(reads.load(Ordering::SeqCst), 1, "reopen must respect the saved delay");
    restored.change("retry-control-receipt", |record| record.query_retry_at = 0).await.unwrap();
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let state = saved.lock().await;
            if state.records[0].status == "succeeded" && state.records[0].delivery == "accepted" {
                assert_eq!(state.records[0].query_failures, 0);
                assert_eq!(state.records[0].query_retry_at, 0);
                assert_eq!(state.records[0].result.as_ref().unwrap()["text"], "原任务已完成");
                break;
            }
            drop(state);
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }).await.unwrap();
    restored.stop().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    assert_eq!(seen.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn wechat_control_watch_exhaustion_can_be_reconciled_by_explicit_lookup() {
    let root = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let mut stored = account(&base);
    stored.records.push(Record {
        id: "exhausted-control-receipt".into(),
        command: "capability_render".into(),
        status: "running".into(),
        job_id: Some("original-control-job".into()),
        context: "synthetic-context".into(),
        client_id: "original-control-client".into(),
        delivery: "accepted".into(),
        query_failures: 3,
        ..Default::default()
    });
    let (template, saved) = test_bridge(
        stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let control: Control = {
        let reads = reads.clone();
        Arc::new(move |request| {
            let reads = reads.clone();
            Box::pin(async move {
                assert_eq!(request.method, "jobs.get");
                assert_eq!(request.job_id.as_deref(), Some("original-control-job"));
                match reads.fetch_add(1, Ordering::SeqCst) {
                    0 => Ok(json!({"status":"synthetic-invalid"})),
                    1 => Ok(json!({"status":"succeeded","result":{"status":"succeeded","text":"显式核对取回原结果"}})),
                    _ => panic!("verified result should be reused from the same receipt"),
                }
            })
        })
    };
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let bridge = Bridge::new(stored, api, template.save.clone(), control, root.path().into());
    let monitor = tokio::spawn(bridge.clone().monitor(0));
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let state = saved.lock().await;
            if state.records[0].status != "running" && state.records[0].delivery == "accepted" {
                assert_eq!(state.records[0].status, "unknown");
                assert_eq!(state.records[0].query_failures, 4);
                assert!(state.records[0].result.is_none());
                break;
            }
            drop(state);
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }).await.unwrap();
    assert_eq!(seen.lock().await.len(), 1);
    assert!(seen.lock().await[0]["body"]["msg"]["item_list"][0]["text_item"]["text"]
        .as_str().unwrap().contains("结果待核对"));
    bridge.ingest(&msg("verify-original-after-exhaustion", "/结果 exhausted-control-receipt")).await.unwrap();
    bridge.ingest(&msg("reuse-verified-original", "/状态 exhausted-control-receipt")).await.unwrap();
    bridge.stop().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
    let state = saved.lock().await;
    assert_eq!(state.records[0].job_id.as_deref(), Some("original-control-job"));
    assert_eq!(state.records[0].status, "succeeded");
    assert_eq!(state.records[0].query_failures, 0);
    assert_eq!(state.records[0].result.as_ref().unwrap()["text"], "显式核对取回原结果");
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(seen.lock().await.len(), 3);
    server.abort();
}

#[tokio::test]
async fn wechat_saved_control_completion_resumes_and_explicitly_returns_original_artifact() {
    for code in [200, 500] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original-control-output.txt");
        let bytes = b"synthetic original control output";
        std::fs::write(&path, bytes).unwrap();
        let artifact = json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes))});
        let payload = json!({"status":"succeeded","text":"原控制任务已完成","artifact":artifact});
        let (base, seen, server) = fake_api(code, json!({"__media":true})).await;
        let mut stored = account(&base);
        stored.records.push(Record {
            id: "original-control-receipt".into(),
            command: "capability_render".into(),
            status: "running".into(),
            job_id: Some("original-control-job".into()),
            context: "original-control-context".into(),
            client_id: "original-control-client".into(),
            delivery: "accepted".into(),
            args: json!({"input":{"title":"synthetic original"}}),
            ..Default::default()
        });
        let (template, saved) = test_bridge(
            stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
        );
        let forbid_send = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let stored_result = Arc::new(tokio::sync::Notify::new());
        let claimed = Arc::new(tokio::sync::Notify::new());
        let save: Save = {
            let (forbid_send, stored_result, claimed, persist) =
                (forbid_send.clone(), stored_result.clone(), claimed.clone(), template.save.clone());
            Arc::new(move |value| {
                let (forbid_send, stored_result, claimed, persist) =
                    (forbid_send.clone(), stored_result.clone(), claimed.clone(), persist.clone());
                Box::pin(async move {
                    let settling = value.enabled && value.records[0].status == "succeeded";
                    if settling && value.records[0].delivery == "unknown" && forbid_send.load(Ordering::SeqCst) {
                        claimed.notify_one();
                        return Err("synthetic persistence failure before network".into());
                    }
                    persist(value).await?;
                    if settling {stored_result.notify_one();}
                    Ok(())
                })
            })
        };
        let lookup_count = Arc::new(AtomicUsize::new(0));
        let control: Control = {
            let (lookup_count, payload) = (lookup_count.clone(), payload.clone());
            Arc::new(move |request| {
                let (lookup_count, payload) = (lookup_count.clone(), payload.clone());
                Box::pin(async move {
                    assert_eq!(request.method, "jobs.get");
                    assert_eq!(request.job_id.as_deref(), Some("original-control-job"));
                    assert_eq!(lookup_count.fetch_add(1, Ordering::SeqCst), 0);
                    Ok(json!({"status":"succeeded","result":payload}))
                })
            })
        };
        let make_bridge = |stored| {
            let mut api = Api::new().unwrap();
            api.test_origin = Some(base.clone());
            Bridge::new(stored, api, save.clone(), control.clone(), root.path().into())
        };
        let bridge = make_bridge(stored);
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), stored_result.notified()).await.unwrap();
        {
            let state = saved.lock().await;
            let record = &state.records[0];
            assert_eq!(record.status, "succeeded");
            assert_eq!(record.delivery, "pending");
            assert_eq!(record.result.as_ref(), Some(&payload));
        }
        tokio::time::timeout(Duration::from_secs(5), claimed.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        assert!(seen.lock().await.is_empty());
        let plain = serde_json::to_vec(&*saved.lock().await).unwrap();
        let sealed = crate::wechat_crypto::crypt(&plain, true).unwrap();
        let reopened = crate::wechat_crypto::crypt(&sealed, false).unwrap();
        let mut reloaded: Stored = serde_json::from_slice(&reopened).unwrap();
        assert_eq!(reloaded.records[0].client_id, "original-control-client-done");
        reloaded.enabled = true;
        forbid_send.store(false, Ordering::SeqCst);
        let resumed = make_bridge(reloaded);
        let monitor = tokio::spawn(resumed.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let settled = {
                    let state = saved.lock().await;
                    state.records[0].delivery == "accepted" || state.records[0].delivery_error.is_some()
                };
                if settled {break;}
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        resumed.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        let mut final_state = saved.lock().await.clone();
        assert_eq!(final_state.records[0].delivery, if code == 200 { "accepted" } else { "unknown" });
        assert_eq!(final_state.records[0].job_id.as_deref(), Some("original-control-job"));
        assert_eq!(final_state.records[0].result.as_ref(), Some(&payload));
        final_state.records.push(Record {
            id: "legacy-saved-result".into(),
            command: "capability_render".into(),
            status: "succeeded".into(),
            job_id: Some("legacy-control-job".into()),
            client_id: "legacy-client-done".into(),
            delivery: "pending".into(),
            result: Some(payload.clone()),
            ..Default::default()
        });
        final_state.enabled = true;
        let again = make_bridge(final_state);
        let monitor = tokio::spawn(again.clone().monitor(0));
        tokio::time::sleep(Duration::from_millis(100)).await;
        {
            let traffic = seen.lock().await;
            assert_eq!(traffic.len(), 1, "completion must send text only, once");
            assert_eq!(traffic[0]["body"]["msg"]["client_id"], "original-control-client-done");
            assert!(traffic[0]["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap().contains("原控制任务已完成"));
        }
        if code == 200 {
            again.ingest(&msg("explicit-original", "/结果 original-control-receipt")).await.unwrap();
            let traffic = seen.lock().await;
            assert_eq!(traffic.len(), 5);
            assert_eq!(
                traffic[4]["body"]["msg"]["item_list"][0]["file_item"]["file_name"],
                "original-control-output.txt"
            );
        }
        assert_eq!(lookup_count.load(Ordering::SeqCst), 1);
        again.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        server.abort();
    }
}
#[tokio::test]
async fn active_and_unreviewed_work_blocks_reauthorization_before_requesting_qr() {
    for active in [
        "enabled",
        "received",
        "running",
        "awaiting_confirmation",
        "waiting_input_queue",
        "expired_received",
        "expired_unidentified",
        "expired_unsubmitted_media",
        "expired_foreign_media",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({"__pairing":true})).await;
        let mut stored = original(&base);
        if active == "enabled" {
            stored.enabled = true;
        } else if active == "expired_received" {
            stored.reauth_required = true;
            stored.records[0].status = "received".into();
        } else if active == "expired_unidentified" {
            stored.reauth_required = true;
            stored.records[0].status = "running".into();
            stored.records[0].job_id = None;
        } else if matches!(active, "expired_unsubmitted_media" | "expired_foreign_media") {
            let source = stored.logical_key();
            stored.reauth_required = true;
            stored.records[0].status = "running".into();
            stored.records[0].command = "wechat_video_transcript".into();
            stored.records[0].args = if active == "expired_unsubmitted_media" {
                json!({"phase":"claimed","source":source})
            } else {
                json!({"phase":"submitted","source":"different-source"})
            };
        } else {
            stored.records[0].status = active.into();
        }
        let before = serde_json::to_value(&stored).unwrap();
        let (bridge, memory) = test_bridge(
            stored,
            tmp.path(),
            controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
        );
        assert!(bridge.begin().await.is_err());
        assert!(seen.lock().await.is_empty());
        assert_eq!(serde_json::to_value(&*memory.lock().await).unwrap(), before);
        server.abort();
    }
}
#[test]
fn legacy_missing_reauthorization_fields_keep_original_semantics() {
    let stored = original(ORIGIN);
    let mut value = serde_json::to_value(&stored).unwrap();
    let obj = value.as_object_mut().unwrap();
    obj.remove("reauth_required");
    obj.remove("channel_key");
    obj.remove("reply_context_pending");
    let legacy: Stored = serde_json::from_value(value).unwrap();
    assert!(!legacy.reauth_required);
    assert!(!legacy.reply_context_pending);
    assert_eq!(legacy.logical_key(), stored.logical_key());
}

#[tokio::test]
async fn proactive_notifications_wait_for_new_owner_context_after_reauthorization() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, _, server) = fake_api(200, json!({})).await;
    let mut stored = original(&base);
    stored.enabled = true;
    stored.reply_context_pending = true;
    let key = stored.logical_key();
    let (template, memory) = test_bridge(
        stored.clone(),
        tmp.path(),
        controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let agent: Agent = Arc::new(move |r| {
        let count = count.clone();
        let key = key.clone();
        Box::pin(async move {
            assert_eq!(r["operation"], "notifications");
            assert_eq!(r["source_key"], key);
            count.fetch_add(1, Ordering::SeqCst);
            Ok(json!([]))
        })
    });
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let bridge = Bridge::new_with_agent(
        stored,
        api,
        template.save.clone(),
        template.control.clone(),
        tmp.path().into(),
        Some(agent),
    );
    bridge.deliver_reminders().await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    bridge.ingest(&msg("fresh-context", "帮助")).await.unwrap();
    assert!(!memory.lock().await.reply_context_pending);
    bridge.deliver_reminders().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn proactive_notification_pause_releases_only_not_yet_posted_claims() {
    for pause_before_first in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = original(&base);
        stored.enabled = true;
        let origin = stored.logical_key();
        let (template, _) = test_bridge(
            stored.clone(),
            tmp.path(),
            controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
        );
        let entered = Arc::new(Notify::new());
        let proceed = Arc::new(Notify::new());
        let settled = Arc::new(Mutex::new(Vec::<Value>::new()));
        let agent: Agent = {
            let entered = entered.clone();
            let proceed = proceed.clone();
            let settled = settled.clone();
            let origin = origin.clone();
            Arc::new(move |request| {
                let entered = entered.clone();
                let proceed = proceed.clone();
                let settled = settled.clone();
                let origin = origin.clone();
                Box::pin(async move {
                    assert_eq!(request["source_key"], origin);
                    match request["operation"].as_str().unwrap() {
                        "notifications" if request["connected"] == true => {
                            if pause_before_first {
                                entered.notify_one();
                                proceed.notified().await;
                            }
                            Ok(json!([
                                {"id":"first","client_id":"first-occurrence","text":"隔离提醒"},
                                {"kind":"matter_notice","id":"next","client_id":"next-occurrence","text":"隔离复查"},
                                {"kind":"agent_task","turn_id":"deferred-turn"}
                            ]))
                        }
                        "notifications" => Ok(json!([])),
                        "notification_ack" => {
                            if !pause_before_first && request["reminder_id"] == "first" {
                                entered.notify_one();
                                proceed.notified().await;
                            }
                            settled.lock().await.push(request);
                            Ok(json!({"saved":true}))
                        }
                        _ => panic!("unexpected isolated request"),
                    }
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new_with_agent(
            stored,
            api,
            template.save.clone(),
            template.control.clone(),
            tmp.path().into(),
            Some(agent),
        );
        bridge.state.lock().await.state = "connected".into();
        let run = {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.deliver_reminders().await })
        };
        tokio::time::timeout(Duration::from_secs(5), entered.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        proceed.notify_one();
        tokio::time::timeout(Duration::from_secs(5), run).await.unwrap().unwrap();
        assert_eq!(seen.lock().await.len(), usize::from(!pause_before_first));
        let acknowledgements = settled.lock().await;
        assert_eq!(acknowledgements.len(), 2);
        assert_eq!(acknowledgements[0]["client_id"], "first-occurrence");
        assert_eq!(acknowledgements[0]["delivery"], if pause_before_first { "not_sent" } else { "accepted" });
        assert_eq!(acknowledgements[1]["client_id"], "next-occurrence");
        assert_eq!(acknowledgements[1]["delivery"], "not_sent");
        assert_eq!(bridge.status().await.tasks.len(), 1);
        server.abort();
    }
}

#[tokio::test]
async fn cancel_during_durable_authorization_reports_committed_instead_of_claiming_rollback() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, _, server) = fake_api(200, json!({"__pairing":true,"__pairing_confirmation":{"status":"confirmed","bot_token":"replacement-token","ilink_bot_id":"synthetic-bot","ilink_user_id":"synthetic-owner","baseurl":ORIGIN}})).await;
    let stored = original(&base);
    let history = serde_json::to_value(&stored.records).unwrap();
    let (template, _) = test_bridge(
        stored.clone(),
        tmp.path(),
        controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
    );
    let memory = Arc::new(Mutex::new(stored.clone()));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let saves = Arc::new(AtomicUsize::new(0));
    let (m, e, r, n) = (
        memory.clone(),
        entered.clone(),
        release.clone(),
        saves.clone(),
    );
    let save: Save = Arc::new(move |value| {
        let (m, e, r, n) = (m.clone(), e.clone(), r.clone(), n.clone());
        Box::pin(async move {
            if value.token == "replacement-token" && n.fetch_add(1, Ordering::SeqCst) == 0 {
                e.notify_one();
                r.notified().await;
            }
            *m.lock().await = value;
            Ok(())
        })
    });
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let bridge = Bridge::new(
        stored,
        api,
        save,
        template.control.clone(),
        tmp.path().into(),
    );
    bridge.begin().await.unwrap();
    wait_for(&bridge, |_, p| p.state == "need_pin").await;
    bridge.pin("123456".into()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(8), entered.notified())
        .await
        .unwrap();
    let b = bridge.clone();
    let cancel = tokio::spawn(async move { b.cancel_pairing().await });
    tokio::task::yield_now().await;
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(8), cancel)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap_err().contains("已经保存"));
    wait_for(&bridge, |s, _| s.state == "connected").await;
    let saved = memory.lock().await.clone();
    assert_eq!(saved.token, "replacement-token");
    assert_eq!(serde_json::to_value(saved.records).unwrap(), history);
    bridge.stop().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn cancellation_without_a_qr_never_stops_an_existing_receiver() {
    let tmp = tempfile::tempdir().unwrap();
    let (base, _, server) = fake_api(200, json!({})).await;
    let (bridge, _) = test_bridge(
        account(&base),
        tmp.path(),
        controller(tmp.path(), Arc::new(AtomicUsize::new(0))),
    );
    bridge.start().await.unwrap();
    wait_for(&bridge, |s, _| s.state == "connected").await;
    let generation = bridge.generation.load(Ordering::SeqCst);
    bridge.cancel_pairing().await.unwrap();
    assert_eq!(bridge.generation.load(Ordering::SeqCst), generation);
    assert_eq!(bridge.status().await.state, "connected");
    assert!(bridge.status().await.enabled);
    bridge.stop().await.unwrap();
    server.abort();
}
