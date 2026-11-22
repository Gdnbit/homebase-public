use super::*;
use crate::assistant_protocol::ModelReply;

#[tokio::test]
async fn wechat_saved_media_completion_reuses_original_transcript_after_restart() {
    for (is_voice, code, tamper) in [
        (true, 200, false),
        (false, 200, false),
        (true, 500, false),
        (false, 200, true),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(code, json!({"__media":true})).await;
        let artifact_path = root.path().join("original-transcript.txt");
        let preview = "原收件的合成转写文字。";
        std::fs::write(&artifact_path, preview).unwrap();
        let artifact = json!({"path":artifact_path,"sha256":format!("{:x}",Sha256::digest(preview))});
        let mut stored = account(&base);
        let source = stored.logical_key();
        let receipt = format!("wx-{}", "a".repeat(64));
        let command = if is_voice { voice::COMMAND } else { "wechat_video_transcript" };
        let record = Record {
            id: receipt.clone(),
            command: command.into(),
            status: "running".into(),
            args: json!({"phase":"submitted","source":source,"scope":"synthetic-original-scope"}),
            context: "synthetic-original-context".into(),
            client_id: "original-media-client".into(),
            delivery: "pending".into(),
            ..Default::default()
        };
        stored.records.push(record.clone());
        let calls = Arc::new(AtomicUsize::new(0));
        let (template, saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), calls.clone()));
        let block_claim = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let save: Save = {
            let (block_claim, persist) = (block_claim.clone(), template.save.clone());
            Arc::new(move |value| {
                let (block_claim, persist) = (block_claim.clone(), persist.clone());
                Box::pin(async move {
                    let original = &value.records[0];
                    if value.enabled && original.status == "completed" && original.delivery == "unknown"
                        && block_claim.load(Ordering::SeqCst)
                    {
                        return Err("synthetic failure saving the first send attempt".into());
                    }
                    persist(value).await
                })
            })
        };
        let reads = Arc::new(AtomicUsize::new(0));
        let inbox: video::VideoInbox = {
            let (reads, receipt, artifact) = (reads.clone(), receipt.clone(), artifact.clone());
            Arc::new(move |request| {
                let (reads, receipt, artifact) = (reads.clone(), receipt.clone(), artifact.clone());
                Box::pin(async move {
                    match request {
                        video::VideoRequest::ObserveVoice { id } if is_voice => assert_eq!(id, receipt),
                        video::VideoRequest::Observe { id } if !is_voice => assert_eq!(id, receipt),
                        _ => panic!("only the original local receipt may be queried"),
                    }
                    assert_eq!(reads.fetch_add(1, Ordering::SeqCst), 0);
                    video::VideoResult::Ready { preview: preview.into(), artifact, repositories: Default::default() }
                })
            })
        };
        let make_bridge = |stored| {
            let mut api = Api::new().unwrap();
            api.test_origin = Some(base.clone());
            let bridge = Bridge::new(stored, api, save.clone(), template.control.clone(), root.path().into());
            bridge.configure_video(inbox.clone());
            bridge
        };
        let original = make_bridge(stored);
        assert!(original.update_media(&record, 0).await.is_err());
        original.stop().await.unwrap();
        assert!(seen.lock().await.is_empty());
        let snapshot = serde_json::to_vec(&*saved.lock().await).unwrap();
        let ciphertext = crate::wechat_crypto::crypt(&snapshot, true).unwrap();
        let decrypted = crate::wechat_crypto::crypt(&ciphertext, false).unwrap();
        let mut stored: Stored = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(stored.records[0].status, "completed");
        assert_eq!(stored.records[0].delivery, "pending");
        assert_eq!(stored.records[0].result.as_ref().unwrap()["artifact"], artifact);
        if tamper {
            std::fs::write(&artifact_path, "changed isolated transcript").unwrap();
        }
        stored.enabled = true;
        block_claim.store(false, Ordering::SeqCst);
        let restarted = make_bridge(stored);
        let monitor = tokio::spawn(restarted.clone().monitor(0));
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
        restarted.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        let mut stored = saved.lock().await.clone();
        let delivery = if tamper { "attachment_failed" } else if code == 200 { "accepted" } else { "unknown" };
        assert_eq!(stored.records[0].delivery, delivery);
        assert_eq!(stored.records[0].client_id, "original-media-client");
        assert_eq!(stored.records[0].result.as_ref().unwrap()["artifact"], artifact);
        let mut legacy = stored.records[0].clone();
        legacy.id = format!("wx-{}", "b".repeat(64));
        legacy.client_id = "legacy-media-client".into();
        legacy.delivery = "pending".into();
        legacy.args = record.args.clone();
        stored.records.push(legacy);
        stored.enabled = true;
        let again = make_bridge(stored);
        let monitor = tokio::spawn(again.clone().monitor(0));
        tokio::time::sleep(Duration::from_millis(100)).await;
        {
            let traffic = seen.lock().await;
            let count = if tamper { 2 } else if code == 200 { 4 } else { 1 };
            assert_eq!(traffic.len(), count);
            let text = &traffic[0]["body"]["msg"];
            assert_eq!(text["client_id"], "original-media-client");
            let body = text["item_list"][0]["text_item"]["text"].as_str().unwrap();
            assert!(body.contains(preview) && body.contains(&receipt));
            if tamper {
                assert!(traffic[1]["body"]["msg"]["item_list"][0]["text_item"]["text"]
                    .as_str().unwrap().contains("哈希"));
            } else if code == 200 {
                let file = &traffic[3]["body"]["msg"];
                assert_eq!(file["client_id"], "original-media-client-file-0");
                assert_eq!(file["item_list"][0]["file_item"]["file_name"], "original-transcript.txt");
            }
        }
        if code == 200 && !tamper {
            again.ingest(&msg("original-media-explicit", &format!("/结果 {receipt}"))).await.unwrap();
            let traffic = seen.lock().await;
            assert_eq!(traffic.len(), 8);
            assert_eq!(traffic[7]["body"]["msg"]["item_list"][0]["file_item"]["file_name"], "original-transcript.txt");
        }
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        again.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        server.abort();
    }
}

#[tokio::test]
async fn wechat_status_counts_original_local_transcriptions_outside_recent_tasks() {
    let root = tempfile::tempdir().unwrap();
    let mut stored = Stored::default();
    for (id, command, status, args) in [
        (
            "original-voice",
            "wechat_voice_transcript",
            "running",
            json!({"phase":"submitted"}),
        ),
        (
            "unverified-voice",
            "wechat_voice_transcript",
            "unknown",
            json!({"phase":"receiving"}),
        ),
        (
            "original-video",
            "wechat_video_transcript",
            "received",
            json!({"phase":"receiving"}),
        ),
        (
            "unverified-video",
            "wechat_video_transcript",
            "unknown",
            json!({"phase":"submitted"}),
        ),
        (
            "completed-voice",
            "wechat_voice_transcript",
            "completed",
            json!({"phase":"submitted"}),
        ),
        (
            "attached-transcript",
            "wechat_voice_transcript",
            "unknown",
            json!({}),
        ),
        ("other-task", "assistant", "running", json!({})),
    ] {
        stored.records.push(Record {
            id: id.into(),
            command: command.into(),
            status: status.into(),
            args,
            ..Record::default()
        });
    }
    for index in 0..51 {
        stored.records.push(Record {
            id: format!("recent-{index}"),
            command: "message".into(),
            status: "completed".into(),
            ..Record::default()
        });
    }
    let count = Arc::new(AtomicUsize::new(0));
    let (bridge, saved) = test_bridge(stored, root.path(), controller(root.path(), count.clone()));
    let snapshot = serde_json::to_value(&*saved.lock().await).unwrap();
    let status = bridge.status().await;
    assert_eq!(status.tasks.len(), 50);
    assert_eq!(status.tasks.last().unwrap().id, "recent-1");
    assert_eq!(status.local_transcriptions.voice.processing, 1);
    assert_eq!(status.local_transcriptions.voice.unverified, 1);
    assert_eq!(status.local_transcriptions.video.processing, 1);
    assert_eq!(status.local_transcriptions.video.unverified, 1);
    assert_eq!(
        serde_json::to_value(&*saved.lock().await).unwrap(),
        snapshot
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

fn voice_message(id: &str, text: Option<&str>) -> Value {
    let mut value = msg(id, "");
    value["item_list"] = json!([{"type":3,"msg_id":id,"voice_item":{
        "text":text,"media":{"encrypt_query_param":"never-fetch-this-fixture"}
    }}]);
    value
}

#[tokio::test]
async fn wechat_voice_raw_cdn_download_and_original_asr_are_claimed_once() {
    use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};

    let temp = tempfile::tempdir().unwrap();
    let content = "原语音的合成离线结果。";
    let file = temp.path().join("transcript.txt");
    std::fs::write(&file, content).unwrap();
    let artifact = json!({"path": file.to_string_lossy(),
        "sha256":format!("{:x}", Sha256::digest(content.as_bytes()))});
    let key = [79; 16];
    let audio = b"\x02#!SILK_V3synthetic-transport-only";
    let cipher =
        ecb::Encryptor::<aes::Aes128>::new((&key).into()).encrypt_padded_vec_mut::<Pkcs7>(audio);
    let (base, seen, server) = fake_api(
        200,
        json!({"__pdf_cipher":STANDARD.encode(cipher),"__media":true}),
    )
    .await;
    let starts = Arc::new(AtomicUsize::new(0));
    let started = starts.clone();
    let ready = Arc::new(AtomicUsize::new(0));
    let state = ready.clone();
    let original = Arc::new(Mutex::new(None::<String>));
    let identity = original.clone();
    let inbox: video::VideoInbox = Arc::new(move |request| {
        let started = started.clone();
        let state = state.clone();
        let identity = identity.clone();
        let artifact = artifact.clone();
        Box::pin(async move {
            match request {
                video::VideoRequest::StartVoice { id, bytes } => {
                    assert_eq!(&**bytes, audio);
                    assert!(video::receipt_id(&id));
                    assert!(identity.lock().await.replace(id).is_none());
                    started.fetch_add(1, Ordering::SeqCst);
                    video::VideoResult::Running
                }
                video::VideoRequest::ObserveVoice { id } => {
                    assert_eq!(identity.lock().await.as_deref(), Some(id.as_str()));
                    if state.load(Ordering::SeqCst) == 0 {
                        video::VideoResult::Running
                    } else {
                        video::VideoResult::Ready {
                            preview: content.into(),
                            artifact,
                            repositories: Default::default(),
                        }
                    }
                }
                _ => panic!("voice test received a video request"),
            }
        })
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let control = controller(temp.path(), calls.clone());
    let (bridge, ledger) = test_bridge(account(&base), temp.path(), control.clone());
    bridge.configure_video(inbox.clone());
    let mut entry = voice_message("9007199254741077", None);
    entry["item_list"][0]["voice_item"]["media"]["aes_key"] = json!(STANDARD.encode(key));
    entry["item_list"][0]["voice_item"]["playtime"] = json!(7020);
    let mut bad = entry.clone();
    bad["message_id"] = json!("bad-origin");
    bad["item_list"][0]["msg_id"] = json!("bad-item");
    bad["item_list"][0]["voice_item"]["media"]["full_url"] =
        json!("https://example.invalid/c2c/download?do-not-connect");
    bridge.ingest(&bad).await.unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    bridge.ingest(&entry).await.unwrap();
    let receipt = original.lock().await.clone().unwrap();
    bridge.ingest(&entry).await.unwrap();
    bridge
        .ingest(&msg("raw-voice-status", &format!("/状态 {receipt}")))
        .await
        .unwrap();
    assert!(seen
        .lock()
        .await
        .last()
        .unwrap()
        .to_string()
        .contains("正在处理原语音"));
    assert_eq!(starts.load(Ordering::SeqCst), 1);

    let stored = ledger.lock().await.clone();
    let record = stored
        .records
        .iter()
        .find(|record| record.id == receipt)
        .unwrap()
        .clone();
    assert_eq!(record.command, voice::COMMAND);
    assert_eq!(record.args["phase"], "submitted");
    assert!(!serde_json::to_string(&stored)
        .unwrap()
        .contains("never-fetch-this-fixture"));
    let (restored, ledger) = test_bridge(stored, temp.path(), control);
    restored.configure_video(inbox);
    ready.store(1, Ordering::SeqCst);
    restored.update_media(&record, 0).await.unwrap();
    restored.update_media(&record, 0).await.unwrap();
    restored.ingest(&entry).await.unwrap();
    restored
        .ingest(&msg("raw-voice-result", &format!("/结果 {receipt}")))
        .await
        .unwrap();

    let restored = ledger.lock().await;
    let record = restored
        .records
        .iter()
        .find(|record| record.id == receipt)
        .unwrap();
    assert_eq!(record.status, "completed");
    assert_eq!(record.delivery, "accepted");
    assert_eq!(record.result.as_ref().unwrap()["transcript"], content);
    assert_eq!(starts.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let traffic = seen.lock().await;
    let downloads = traffic
        .iter()
        .filter(|row| row["head"].as_str().unwrap().contains("GET /c2c/download?"))
        .collect::<Vec<_>>();
    assert_eq!(downloads.len(), 1);
    assert!(!downloads[0]["head"]
        .as_str()
        .unwrap()
        .to_ascii_lowercase()
        .contains("authorization"));
    assert_eq!(
        traffic
            .iter()
            .filter(|row| row["head"].as_str().unwrap().contains("/getuploadurl "))
            .count(),
        2
    );
    server.abort();
}

#[tokio::test]
async fn wechat_explicit_query_records_verified_original_transcription_without_auto_reply() {
    let temp = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let mut stored = account(&base);
    let voice_id = format!("wx-{}", "a".repeat(64));
    let video_id = format!("wx-{}", "b".repeat(64));
    let source = stored.logical_key();
    for (id, command, status, delivery) in [
        (&voice_id, voice::COMMAND, "unknown", "accepted"),
        (&video_id, "wechat_video_transcript", "running", "pending"),
    ] {
        stored.records.push(Record {
            id: id.clone(),
            command: command.into(),
            status: status.into(),
            delivery: delivery.into(),
            args: json!({"phase":"submitted","source":source,"scope":"original-scope"}),
            ..Record::default()
        });
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let controller = controller(temp.path(), calls.clone());
    let (bridge, ledger) = test_bridge(stored, temp.path(), controller.clone());
    let observations = Arc::new(AtomicUsize::new(0));
    let observed = observations.clone();
    let original_voice = voice_id.clone();
    let original_video = video_id.clone();
    let artifact_path = temp.path().join("original-synthetic.txt");
    let content = "晚到的合成语音文字。";
    std::fs::write(&artifact_path, content).unwrap();
    let artifact =
        json!({"path":artifact_path,"sha256":format!("{:x}",Sha256::digest(content.as_bytes()))});
    let inbox: video::VideoInbox = Arc::new(move |request| {
        let observed = observed.clone();
        let voice_id = original_voice.clone();
        let video_id = original_video.clone();
        let artifact = artifact.clone();
        Box::pin(async move {
            observed.fetch_add(1, Ordering::SeqCst);
            match request {
                video::VideoRequest::ObserveVoice { id } if id == voice_id => {
                    video::VideoResult::Ready {
                        preview: content.into(),
                        artifact,
                        repositories: Default::default(),
                    }
                }
                video::VideoRequest::Observe { id } if id == video_id => {
                    video::VideoResult::Failed("原视频未识别到语音。".into())
                }
                _ => panic!("expected observation of the original local receipt"),
            }
        })
    });
    bridge.configure_video(inbox.clone());
    bridge
        .ingest(&msg("recover-voice", &format!("/状态 {voice_id}")))
        .await
        .unwrap();
    bridge
        .ingest(&msg("recover-video", &format!("/状态 {video_id}")))
        .await
        .unwrap();
    let saved = ledger.lock().await.clone();
    assert_eq!(saved.records[0].status, "completed");
    assert_eq!(saved.records[0].delivery, "accepted");
    assert_eq!(
        saved.records[0].result.as_ref().unwrap()["transcript"],
        content
    );
    assert_eq!(saved.records[1].status, "failed");
    assert_eq!(saved.records[1].delivery, "not_sent");
    assert_eq!(seen.lock().await.len(), 2);

    let (reopened, _) = test_bridge(saved, temp.path(), controller);
    reopened.configure_video(inbox);
    reopened
        .ingest(&msg("query-again", &format!("/状态 {voice_id}")))
        .await
        .unwrap();
    let status = reopened.status().await;
    assert_eq!(status.local_transcriptions.voice.unverified, 0);
    assert_eq!(status.local_transcriptions.video.processing, 0);
    assert_eq!(observations.load(Ordering::SeqCst), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(seen.lock().await.len(), 3);
    server.abort();
}

#[tokio::test]
async fn wechat_paused_query_leaves_original_transcription_for_the_next_context() {
    let temp = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let mut stored = account(&base);
    let source = stored.logical_key();
    let id = format!("wx-{}", "c".repeat(64));
    stored.records.push(Record {
        id: id.clone(),
        command: voice::COMMAND.into(),
        status: "unknown".into(),
        delivery: "accepted".into(),
        args: json!({"source":source,"phase":"submitted"}),
        ..Record::default()
    });
    let calls = Arc::new(AtomicUsize::new(0));
    let (bridge, ledger) = test_bridge(stored, temp.path(), controller(temp.path(), calls.clone()));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let observer_entered = entered.clone();
    let observer_release = release.clone();
    bridge.configure_video(Arc::new(move |request| {
        let entered = observer_entered.clone();
        let release = observer_release.clone();
        Box::pin(async move {
            assert!(matches!(request, video::VideoRequest::ObserveVoice { .. }));
            entered.notify_one();
            release.notified().await;
            video::VideoResult::Failed("原转写返回明确失败。".into())
        })
    }));
    let receiver = bridge.clone();
    let task = tokio::spawn(async move {
        receiver
            .ingest(&msg("paused-query", &format!("/结果 {id}")))
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    bridge.stop().await.unwrap();
    release.notify_one();
    task.await.unwrap().unwrap();
    let saved = ledger.lock().await;
    assert_eq!(saved.records[0].status, "unknown");
    assert_eq!(saved.records[0].delivery, "accepted");
    assert!(saved.records[0].result.is_none());
    assert!(seen.lock().await.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn wechat_voice_receives_only_final_owner_transcripts_and_recovers_by_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let control = controller(temp.path(), calls.clone());
    let (bridge, ledger) = test_bridge(account(&base), temp.path(), control.clone());

    let mut delivered = voice_message("9007199254741025", Some("/调用 system_info {}"));
    delivered.as_object_mut().unwrap().remove("message_id");
    let mut foreign = delivered.clone();
    foreign["from_user_id"] = json!("foreign-user");
    bridge.ingest(&foreign).await.unwrap();
    delivered["item_list"][0]["is_completed"] = json!(false);
    bridge.ingest(&delivered).await.unwrap();
    assert!(ledger.lock().await.records.is_empty());

    delivered["item_list"][0]["is_completed"] = json!(true);
    bridge.ingest(&delivered).await.unwrap();
    delivered["message_id"] = json!("9007199254741030");
    bridge.ingest(&delivered).await.unwrap();
    let saved = ledger.lock().await.clone();
    let original = saved.records.first().unwrap();
    let id = original.id.clone();
    assert_eq!(saved.records.len(), 1);
    assert_eq!(original.command, voice::COMMAND);
    assert_eq!(original.status, "completed");
    assert_eq!(original.delivery, "accepted");
    assert_eq!(
        original.result.as_ref().unwrap()["text"],
        "/调用 system_info {}"
    );
    assert!(!serde_json::to_string(&saved)
        .unwrap()
        .contains("never-fetch-this-fixture"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(seen.lock().await.len(), 1);
    let public_status = serde_json::to_string(&bridge.status().await).unwrap();
    assert!(public_status.contains(&id));
    assert!(!public_status.contains("system_info"));

    let encoded = serde_json::to_vec(&saved).unwrap();
    #[cfg(windows)]
    let encoded = {
        let protected = crate::wechat_crypto::crypt(&encoded, true).unwrap();
        let marker = "调用 system_info".as_bytes();
        assert!(!protected
            .windows(marker.len())
            .any(|window| window == marker));
        crate::wechat_crypto::crypt(&protected, false).unwrap()
    };
    let (restarted, ledger) = test_bridge(
        serde_json::from_slice(&encoded).unwrap(),
        temp.path(),
        control,
    );
    delivered["item_list"][0]
        .as_object_mut()
        .unwrap()
        .remove("msg_id");
    restarted.ingest(&delivered).await.unwrap();
    restarted
        .ingest(&msg("voice-status", &format!("/状态 {id}")))
        .await
        .unwrap();
    let response = seen.lock().await.last().unwrap().clone();
    assert!(response.to_string().contains("语音转写已保存"));
    assert!(!response.to_string().contains("system_info"));
    restarted
        .ingest(&msg("voice-result", &format!("/结果 {id}")))
        .await
        .unwrap();
    assert!(seen
        .lock()
        .await
        .last()
        .unwrap()
        .to_string()
        .contains("system_info"));
    assert_eq!(
        ledger
            .lock()
            .await
            .records
            .iter()
            .filter(|record| record.command == voice::COMMAND)
            .count(),
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn wechat_voice_reports_empty_malformed_and_mixed_inputs_without_model() {
    let temp = tempfile::tempdir().unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let (bridge, ledger) = test_bridge(
        account(&base),
        temp.path(),
        controller(temp.path(), calls.clone()),
    );
    let mut empty = voice_message("1", None);
    empty["item_list"][0]["voice_item"]
        .as_object_mut()
        .unwrap()
        .remove("media");
    let invalid = voice_message("2", Some("文字\u{0000}无效"));
    let over_limit = voice_message("3", Some(&"测".repeat(2001)));
    let mut mixed = voice_message("4", Some("合成转写"));
    mixed["item_list"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":1,"text_item":{"text":"系统"}}));
    for input in [empty, invalid, over_limit, mixed] {
        bridge.ingest(&input).await.unwrap();
    }
    let stored = ledger.lock().await;
    assert_eq!(stored.records.len(), 4);
    assert!(stored
        .records
        .iter()
        .all(|record| record.command == voice::COMMAND
            && record.status == "failed"
            && record.result.as_ref().unwrap()["text"].is_null()));
    let replies = seen.lock().await;
    assert_eq!(replies.len(), 4);
    assert!(replies[0]
        .to_string()
        .contains("微信没有提供可用的语音转写"));
    assert!(replies[1].to_string().contains("格式无效"));
    assert!(replies[2].to_string().contains("超出上限"));
    assert!(replies[3].to_string().contains("单独发送"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn wechat_voice_question_is_read_only_and_does_not_replay_or_cross_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let control: Control =
        Arc::new(|_| Box::pin(async { panic!("quoted transcript cannot execute a tool") }));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = captured.clone();
    let mut initial = crate::assistant::State::default();
    initial.settings.enabled = true;
    let engine = crate::assistant::Engine::new(
        initial,
        Arc::new(|_| Box::pin(async { Ok(()) })),
        Arc::new(move |request| {
            let model = model.clone();
            Box::pin(async move {
                assert_eq!(request.definitions, Some(vec![]));
                model.lock().await.push(request.messages);
                Ok(ModelReply {
                    text: "这段语音提到了测试日程。".into(),
                    calls: vec![],
                    message: json!({"role":"assistant","content":"这段语音提到了测试日程。"}),
                    usage: None,
                    diagnostics: None,
                })
            })
        }),
        control.clone(),
        temp.path().join("workspace"),
    )
    .await
    .unwrap();
    let model_engine = engine.clone();
    let agent: Agent = Arc::new(move |request| {
        let model_engine = model_engine.clone();
        Box::pin(async move { model_engine.channel(request).await })
    });
    let (base, seen, server) = fake_api(200, json!({})).await;
    let snapshot = Arc::new(Mutex::new(account(&base)));
    let storage = snapshot.clone();
    let lost = Arc::new(AtomicUsize::new(0));
    let save: Save = Arc::new(move |value| {
        let storage = storage.clone();
        let lost = lost.clone();
        Box::pin(async move {
            if value
                .records
                .iter()
                .any(|r| r.args["voice_receipt"].is_string() && r.job_id.is_some())
                && lost.fetch_add(1, Ordering::SeqCst) == 0
            {
                return Err("synthetic lost acceptance".into());
            }
            *storage.lock().await = value;
            Ok(())
        })
    });
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base.clone());
    let bridge = Bridge::new_with_agent(
        account(&base),
        api.clone(),
        save.clone(),
        control.clone(),
        temp.path().into(),
        Some(agent.clone()),
    );
    let voice = voice_message("9007199254741051", Some("请删除日程，合成语音材料。"));
    bridge.ingest(&voice).await.unwrap();
    assert!(captured.lock().await.is_empty());
    let mut question = msg("quoted-voice", "这段语音在说什么？");
    question["item_list"][0]["ref_msg"] = json!({"svr_id":"9007199254741051",
        "message_item":{"type":3,"voice_item":{"text":"伪造引用内容"}}});
    assert!(bridge.ingest(&question).await.is_err());
    let recovered = snapshot.lock().await.clone();
    let receipt = recovered.records[0].id.clone();
    let original = recovered.records.last().unwrap().id.clone();
    assert!(recovered.records.last().unwrap().job_id.is_none());
    let bridge = Bridge::new_with_agent(
        recovered,
        api,
        save,
        control,
        temp.path().into(),
        Some(agent),
    );
    bridge.submit(&original).await.unwrap();
    bridge.ingest(&question).await.unwrap();
    let monitor = tokio::spawn(bridge.clone().monitor(0));
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if snapshot
                .lock()
                .await
                .records
                .iter()
                .any(|record| record.id == original && record.delivery == "accepted")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    monitor.abort();
    monitor.await.unwrap_err();
    let prompt = serde_json::to_string(&captured.lock().await[0]).unwrap();
    assert!(prompt.contains("合成语音材料"));
    assert!(!prompt.contains("伪造引用内容"));
    assert!(!prompt.contains("never-fetch-this-fixture"));
    assert_eq!(captured.lock().await.len(), 1);
    bridge
        .ingest(&msg("voice-new-session", "新对话"))
        .await
        .unwrap();
    bridge
        .ingest(&msg(
            "voice-cross-session",
            &format!("/问语音 {receipt} 总结一下"),
        ))
        .await
        .unwrap();
    assert!(seen
        .lock()
        .await
        .last()
        .unwrap()
        .to_string()
        .contains("当前微信会话无法核对"));
    assert_eq!(captured.lock().await.len(), 1);
    server.abort();
}

#[tokio::test]
async fn voice_task_uses_original_reply_once_after_lost_acceptance_and_requires_real_confirmation() {
    let root = tempfile::tempdir().unwrap();
    const TRANSCRIPT: &str = "帮我记住测试封面用绿色。";
    const OUTBOUND_ID: &str = "9007199254742011";
    let (base, _, server) = fake_api(200, json!({"__send_text_prefix_ids":{"收到语音。":OUTBOUND_ID}})).await;
    let stored = account(&base);
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let model: crate::assistant::Model = Arc::new(move |request| {
        let count = count.clone();
        Box::pin(async move {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                assert_eq!(request.messages.last().unwrap()["content"], TRANSCRIPT);
                let intro = request.messages[0]["content"].as_str().unwrap();
                assert!(intro.contains("明确交办") && !intro.contains("伪造的引用"));
                return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"tool_calls":[{
                    "id":"voice-memory-fixture","function":{"name":"homebase_memory","arguments":json!({"action":"remember","revision":0,"title":"语音封面偏好","content":"封面用绿色"}).to_string()}
                }]}}]}));
            }
            crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"这条封面偏好已处理。"}}]}))
        })
    });
    let control: Control = Arc::new(|_| Box::pin(async { panic!("memory workflow must not execute a control-plane job") }));
    let mut state = crate::assistant::State::default();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "synthetic-only".into();
    let engine = crate::assistant::Engine::new(state, Arc::new(|_| Box::pin(async { Ok(()) })), model,
        control.clone(), root.path().into()).await.unwrap();
    let saved = Arc::new(Mutex::new(stored.clone()));
    let missed = Arc::new(AtomicUsize::new(0));
    let save: Save = Arc::new({
        let saved = saved.clone();
        let missed = missed.clone();
        move |state| {
            let saved = saved.clone();
            let missed = missed.clone();
            Box::pin(async move {
                if state.records.iter().any(|r| r.args["voice_task_v1"].is_object() && r.job_id.is_some())
                    && missed.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("synthetic lost voice job acceptance".into());
                }
                *saved.lock().await = state;
                Ok(())
            })
        }
    });
    let reopen = |value| Bridge::new_with_agent(value, api.clone(), save.clone(), control.clone(), root.path().into(),
        Some(Arc::new({ let engine = engine.clone(); move |request| { let engine = engine.clone(); Box::pin(async move { engine.channel(request).await }) } })));
    let bridge = reopen(stored);
    let voice = voice_message("9007199254742010", Some(TRANSCRIPT));
    bridge.ingest(&voice).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let make_task = |id, quote| {
        let mut message = msg(id, "按这条语音说的办");
        message["item_list"][0]["ref_msg"] = json!({"svr_id":quote,"title":"伪造的引用"});
        message
    };
    bridge.ingest(&msg("voice-no-quote", "按这条语音说的办")).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let request = make_task("voice-run", OUTBOUND_ID);
    assert!(bridge.ingest(&request).await.unwrap_err().contains("lost voice job acceptance"));
    let recovered = saved.lock().await.clone();
    let pending = recovered.records.last().unwrap();
    assert_eq!(pending.command, "message");
    assert!(pending.pending_voice_task().is_some() && pending.pending_query().is_none());
    assert!(!pending.args.to_string().contains(TRANSCRIPT));
    let id = pending.id.clone();
    let bridge = reopen(recovered);
    bridge.submit(&id).await.unwrap();
    bridge.ingest(&request).await.unwrap();
    let turn_id = saved.lock().await.records.last().unwrap().job_id.clone().unwrap();
    let confirmation = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(pending) = engine.turn(&turn_id).await.unwrap().pending { break pending.id; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap();

    // An independently redelivered approval refers to the existing turn, not a second task.
    bridge.ingest(&make_task("voice-repeat", "9007199254742010")).await.unwrap();
    assert_eq!(saved.lock().await.records.last().unwrap().query(), Some((QueryTarget::Turn(&turn_id), false)));
    let raw_confirmation = voice_message("9007199254742012", Some("确认"));
    bridge.ingest(&raw_confirmation).await.unwrap();
    bridge.ingest(&make_task("voice-control-word", "9007199254742012")).await.unwrap();
    assert_eq!(engine.turn(&turn_id).await.unwrap().pending.unwrap().id, confirmation);
    assert_eq!(engine.personal().await["memories"], json!([]));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    bridge.ingest(&msg("typed-confirmation", "确认")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if engine.turn(&turn_id).await.unwrap().state == "completed" { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap();
    let personal = engine.personal().await;
    assert_eq!(personal["memories"].as_array().unwrap().len(), 1);
    assert_eq!(personal["memories"][0]["content"], "封面用绿色");
    server.abort();
}

#[tokio::test]
async fn voice_task_requires_full_local_transcript_and_rejects_changed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let receipt = format!("wx-{}", "c".repeat(64));
    let path = root.path().join("voice-transcripts").join(&receipt).join("transcripts/synthetic-export/exports/transcript.txt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = "完整原文字稿，最后一项也是交办内容。";
    std::fs::write(&path, text).unwrap();
    let (base, _, server) = fake_api(200, json!({})).await;
    let mut stored = account(&base);
    let source = stored.logical_key();
    let alias = identity::source_key(&source, "9007199254742021");
    let mut record = Record { id:receipt.clone(), command:voice::COMMAND.into(), status:"completed".into(), args:json!({"source":source,"scope":"synthetic-scope","phase":"submitted"}),
        result:Some(json!({"transcript":"预览只有前半句", "artifact":{"path":path,"sha256":format!("{:x}", Sha256::digest(text.as_bytes()))}})), ..Default::default() };
    record.remember_outbound_keys([alias]);
    record.outbound_quote_keys.clear(); // Restore metadata kept when an older build writes the ledger.
    stored.records.push(record);
    let (bridge, _) = test_bridge(stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))));
    let mut message = msg("local-voice-job", "按这条语音说的办");
    message["item_list"][0]["ref_msg"] = json!({"svr_id":"9007199254742021"});
    let Some(Action::VoiceTask(intent)) = bridge.voice_task(&stored, &message, "按这条语音说的办").await else { panic!("durable voice quote should still resolve") };
    let intent = json!(intent);
    let voice::VoiceTaskAdmission::Submit(input) = bridge.prepare_voice_task(&intent, false).await.unwrap() else { panic!() };
    assert_eq!(input["voice"]["transcript"], text);
    assert_eq!(input["voice"]["local"], true);
    std::fs::write(&path, "改动后的文字稿").unwrap();
    let failure = bridge.prepare_voice_task(&intent, false).await.err().unwrap();
    assert!(failure.contains("摘要不一致"));
    server.abort();
}

#[cfg(windows)]
#[tokio::test]
async fn voice_task_reattaches_after_both_services_restart_without_original_file() {
    use crate::assistant::{Engine, State};
    use crate::control_protocol::publish_private;

    async fn save_encrypted(path: &Path, data: &impl Serialize) -> Result<(), String> {
        let plain = zeroize::Zeroizing::new(serde_json::to_vec(data).map_err(|e| e.to_string())?);
        let sealed = crate::wechat_crypto::crypt(&plain, true)?;
        publish_private(path, &sealed).await.map_err(|error| error.message)
    }
    fn load_encrypted<T: serde::de::DeserializeOwned>(path: &Path) -> T {
        let plain = crate::assistant_state_store::read_legacy(path).unwrap().unwrap();
        serde_json::from_slice(&plain).unwrap()
    }

    const TEXT: &str = "请记住隔离封面使用绿色字幕。";
    const QUOTE_ID: &str = "9007199254742151";
    const UNACCEPTED_QUOTE_ID: &str = "9007199254742152";
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let assistant_path = root.path().join("assistant.dpapi");
    let wechat_path = root.path().join("channel.dpapi");
    let missing_receipt = format!("wx-{}", "a".repeat(64));
    let voice_path = workspace.join("voice-transcripts").join(&missing_receipt)
        .join("transcripts/synthetic-export/exports/transcript.txt");
    std::fs::create_dir_all(voice_path.parent().unwrap()).unwrap();
    std::fs::write(&voice_path, TEXT).unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let account = account(&base);
    let source = account.logical_key();
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base);
    let calls = Arc::new(AtomicUsize::new(0));
    let model: crate::assistant::Model = Arc::new({
        let calls = calls.clone();
        move |request| {
            let calls = calls.clone();
            Box::pin(async move {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    assert_eq!(request.messages.last().unwrap()["content"], TEXT);
                    return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"tool_calls":[{
                        "id":"voice-restart-confirmation","function":{"name":"homebase_memory","arguments":json!({"action":"remember","revision":0,"title":"隔离字幕偏好","content":"绿色字幕"}).to_string()}
                    }]}}]}));
                }
                crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"已保存隔离偏好。"}}]}))
            })
        }
    });
    let control: Control = Arc::new(|_| Box::pin(async { panic!("voice test must not execute an external job") }));
    let assistant_save: crate::assistant::Save = Arc::new({ let path = assistant_path.clone(); move |state| {
        let path = path.clone(); Box::pin(async move { save_encrypted(&path, &state).await })
    }});
    let wechat_save = |fail_acceptance| -> Save {
        let path = wechat_path.clone();
        Arc::new(move |state| { let path = path.clone(); Box::pin(async move {
            if fail_acceptance && state.records.iter().any(|r| r.args["voice_task_v1"].is_object() && r.job_id.is_some()) {
                return Err("synthetic acceptance not written".into());
            }
            save_encrypted(&path, &state).await
        }) })
    };
    let quote = |id, target| {
        let mut message = msg(id, "按这条语音说的办");
        message["item_list"][0]["ref_msg"] = json!({"svr_id":target});
        message
    };
    let original_message = quote("voice-before-both-restart", QUOTE_ID);
    let make_agent = |engine: Arc<Engine>| -> Agent {
        Arc::new(move |request| { let engine = engine.clone(); Box::pin(async move { engine.channel(request).await }) })
    };
    let (accepted, confirmation, old_scope) = tokio::task::spawn_blocking({
        let (workspace, api, control, model, assistant_save, voice_path, source, missing_receipt, message) =
            (workspace.clone(), api.clone(), control.clone(), model.clone(), assistant_save.clone(), voice_path.clone(),
                source.clone(), missing_receipt.clone(), original_message.clone());
        let save = wechat_save(true);
        move || {
            let first_runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            let result = first_runtime.block_on(async move {
                let mut initial = State::default();
                initial.settings.enabled = true;
                initial.settings.runtime = "pi".into();
                initial.settings.route_id = "synthetic-only".into();
                let engine = Engine::new(initial, assistant_save, model, control.clone(), workspace.clone()).await.unwrap();
                let intake = engine.channel(json!({"operation":"intake_scope","source_key":source})).await.unwrap();
                let scope = intake["session_id"].as_str().unwrap().to_owned();
                let mut stored = account;
                for (id, alias, args, result) in [
                    (missing_receipt, QUOTE_ID, json!({"source":source,"scope":scope,"phase":"submitted"}),
                        json!({"artifact":{"path":voice_path,"sha256":format!("{:x}", Sha256::digest(TEXT.as_bytes()))}})),
                    (format!("wx-{}", "b".repeat(64)), UNACCEPTED_QUOTE_ID, json!({"source":source,"scope":scope}), json!({"text":"未交办的旧语音。"})),
                ] {
                    let mut receipt = Record { id, args, result:Some(result), command:voice::COMMAND.into(), status:"completed".into(), ..Default::default() };
                    receipt.remember_outbound_keys([identity::source_key(&source, alias)]);
                    stored.records.push(receipt);
                }
                let bridge = Bridge::new_with_agent(stored, api, save, control, workspace, Some(make_agent(engine.clone())));
                assert!(bridge.ingest(&message).await.unwrap_err().contains("acceptance not written"));
                let (tid, confirmation) = tokio::time::timeout(Duration::from_secs(25), async {
                    loop {
                        let session = engine.session(&scope).await.unwrap();
                        if let Some(turn) = session.turns.first() {
                            if let Some(pending) = &turn.pending { break (turn.id.clone(), pending.id.clone()); }
                        }
                        tokio::time::sleep(Duration::from_millis(30)).await;
                    }
                }).await.unwrap();
                (tid, confirmation, scope)
            });
            first_runtime.shutdown_timeout(Duration::from_secs(3));
            result
        }
    }).await.unwrap();
    std::fs::rename(&voice_path, root.path().join("removed-synthetic-transcript.txt")).unwrap();
    let recorded: Stored = load_encrypted(&wechat_path);
    let pending = recorded.records.last().unwrap();
    assert!(pending.pending_voice_task().is_some() && pending.job_id.is_none());
    let pending_id = pending.id.clone();
    let engine = Engine::new(load_encrypted(&assistant_path), assistant_save, model, control.clone(), workspace.clone()).await.unwrap();
    let new_chat = engine.channel(json!({"operation":"submit","source_key":source,"request_id":"new-chat-after-restart","text":"新对话"})).await.unwrap();
    assert_ne!(new_chat["session_id"], old_scope);
    let lookup = |owner: &str, scope: &str| json!({"operation":"lookup_voice_task","source_key":owner,"receipt_id":missing_receipt,"scope":scope});
    assert_eq!(engine.channel(lookup("different-synthetic-owner", &old_scope)).await.unwrap()["receipt"], Value::Null);
    assert!(engine.channel(lookup(&source, new_chat["session_id"].as_str().unwrap())).await.is_err());
    let bridge = Bridge::new_with_agent(recorded, api, wechat_save(false), control, workspace, Some(make_agent(engine.clone())));
    bridge.submit(&pending_id).await.unwrap();
    bridge.ingest(&original_message).await.unwrap();
    let recovered: Stored = load_encrypted(&wechat_path);
    assert_eq!(recovered.records.last().unwrap().job_id.as_deref(), Some(accepted.as_str()));
    bridge.ingest(&quote("unaccepted-from-old-chat", UNACCEPTED_QUOTE_ID)).await.unwrap();
    assert!(seen.lock().await.last().unwrap().to_string().contains("当前微信会话无法核对这条语音"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(engine.turn(&accepted).await.unwrap().pending.unwrap().id, confirmation);
    bridge.ingest(&msg("user-confirm-after-both-restart", &format!("确认 {confirmation}"))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(25), async {
        while engine.turn(&accepted).await.unwrap().state != "completed" {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    assert_eq!(engine.personal().await["memories"].as_array().unwrap().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
}
