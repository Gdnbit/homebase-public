// Generated frontend workflow text, actual shared graph and loopback channel.
use super::*;

#[tokio::test]
#[ignore = "explicit generated workflow fixture and isolated graph runtime required"]
async fn generated_workflow_context_queries_original_records_and_restores_without_resubmission() {
    let file = std::env::var("HOMEBASE_WORKFLOW_CONTEXT_FIXTURE").unwrap();
    assert!(file.replace('\\', "/").starts_with("F:/HomeBase-Labs/"));
    let fixture: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(fixture["synthetic_only"], true);
    let studio = fixture["scenario"] == "studio";
    let receipt = fixture["scenario"] == "inbox_receipt";
    let text = fixture["text"].as_str().unwrap().to_string();
    let marker = fixture["marker"].as_str().unwrap().to_string();
    let expected = fixture["payload"].clone();
    let command = if studio {
        "task_events_get"
    } else if receipt {
        "inbox_submission_get"
    } else {
        "inbox_list_recent"
    };
    let root = tempfile::tempdir().unwrap();
    assert!(root
        .path()
        .to_string_lossy()
        .replace('\\', "/")
        .starts_with("F:/HomeBase-Labs/"));
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let sentinel = root.path().join("outside.txt");
    std::fs::write(&sentinel, b"PRESERVE").unwrap();
    let prior = workspace.join("synthetic-prior.md");
    std::fs::write(&prior, "合成原始条目🙂").unwrap();
    let receipts = Arc::new(crate::inbox_receipts::ReceiptStore::new(
        root.path().join("inbox/receipts"),
        workspace.join("Wiki"),
    ));
    let receipt_id = expected["source"]["receipt"]["submission_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let receipt_before = if receipt {
        let payload = crate::inbox_document::InboxSubmitPayload {
            text: "合成原始收录A🙂".into(),
            submission_id: Some(receipt_id.clone()),
            ..Default::default()
        };
        assert_eq!(
            receipts.submit(payload).await.unwrap().state,
            crate::inbox_receipts::InboxSubmissionState::Completed
        );
        Some(
            std::fs::read(
                root.path()
                    .join(format!("inbox/receipts/{receipt_id}.receipt.dpapi")),
            )
            .unwrap(),
        )
    } else {
        None
    };
    let mut journal = crate::task_event_store::Journal::default();
    journal.record("studio:event",json!({"kind":"assistant_text","task_id":"synthetic-original-task","text":"合成原文稿结果🙂"}));
    journal.record("studio:event",json!({"kind":"completed","task_id":"synthetic-original-task","session_id":"synthetic-studio","exit_code":0}));
    journal.stream_closed("studio:event", "synthetic-original-task");
    journal.stream_closed("studio:event", "synthetic-original-task");
    let before =
        serde_json::to_string(&journal.page("studio:event", "synthetic-original-task", 0, 100))
            .unwrap();
    let journal = Arc::new(Mutex::new(journal));
    let queries = Arc::new(AtomicUsize::new(0));
    let runner: Runner = Arc::new({
        let journal = journal.clone();
        let queries = queries.clone();
        let prior = prior.clone();
        let receipts = receipts.clone();
        let receipt_id = receipt_id.clone();
        move |name, args| {
            let journal = journal.clone();
            let queries = queries.clone();
            let prior = prior.clone();
            let receipts = receipts.clone();
            let receipt_id = receipt_id.clone();
            Box::pin(async move {
                assert_eq!(name,command,"Workflow handoff must only query existing records, never submit/cancel/publish");
                queries.fetch_add(1, Ordering::SeqCst);
                if studio {
                    assert_eq!(args["topic"], "studio:event");
                    assert_eq!(args["taskId"], "synthetic-original-task");
                    Ok(serde_json::to_value(journal.lock().await.page(
                        "studio:event",
                        "synthetic-original-task",
                        0,
                        100,
                    ))
                    .unwrap())
                } else if receipt {
                    assert_eq!(args["submissionId"], receipt_id);
                    Ok(serde_json::to_value(receipts.get(&receipt_id).unwrap()).unwrap())
                } else {
                    assert_eq!(args["limit"], 1);
                    Ok(
                        json!([{"path":prior,"filename":"synthetic-prior.md","day":"2026-11-17","preview":"合成原始条目🙂","size":std::fs::metadata(&prior).unwrap().len(),"modified_at":"2026-11-17T00:00:00Z"}]),
                    )
                }
            })
        }
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
    let model: crate::assistant::Model = Arc::new({
        let rounds = rounds.clone();
        let input = text.clone();
        move |r: crate::assistant_protocol::ModelRequest| {
            let rounds = rounds.clone();
            let input = input.clone();
            let marker = marker.clone();
            let expected = expected.clone();
            Box::pin(async move {
                let message = match rounds.fetch_add(1, Ordering::SeqCst) {
                    0 => {
                        let actual = r
                            .messages
                            .iter()
                            .filter_map(|m| m["content"].as_str())
                            .find(|s| s.contains(&input))
                            .expect(
                                "Full prepared source and task missing from shared model request",
                            );
                        let raw = actual.split_once(&marker).unwrap().1;
                        let data = serde_json::Deserializer::from_str(raw)
                            .into_iter::<Value>()
                            .next()
                            .unwrap()
                            .unwrap();
                        assert_eq!(data, expected);
                        if studio {
                            assert_eq!(data["source"]["task_id"], "synthetic-original-task");
                        } else if receipt {
                            assert_eq!(data["source"]["receipt"]["matches_current"], false);
                            assert_eq!(data["source"]["receipt"]["state"], "unknown");
                            assert!(data["source"]["text"].as_str().unwrap().contains("新稿B"));
                        } else {
                            assert_eq!(data["source"]["uncertain"], true);
                            assert!(matches!(
                                data["source"]["attachments"][0]["rel"].as_str(),
                                Some("_attachments/synthetic.txt" | "raw/inbox/2026-11-15/_attachments/synthetic.txt")
                            ), "Preserve the exact old or date-qualified reference, never infer a new date");
                        }
                        json!({"tool_calls":[{"id":"workflow-describe","function":{"name":"homebase_describe","arguments":json!({"command":command}).to_string()}}]})
                    }
                    1 => {
                        let contract = r
                            .messages
                            .iter()
                            .filter(|m| m["role"] == "tool")
                            .map(Value::to_string)
                            .collect::<String>();
                        assert!(contract.contains("input_schema"));
                        if studio {
                            assert!(contract.contains("studio:event"));
                        }
                        let args = if studio {
                            json!({"topic":"studio:event","taskId":"synthetic-original-task","afterSeq":0,"limit":100})
                        } else if receipt {
                            json!({"submissionId":expected["source"]["receipt"]["submission_id"]})
                        } else {
                            json!({"limit":1})
                        };
                        json!({"tool_calls":[{"id":"workflow-query-original","function":{"name":"homebase_call","arguments":json!({"command":command,"args":args}).to_string()}}]})
                    }
                    2 => {
                        let result = r
                            .messages
                            .iter()
                            .filter(|m| m["role"] == "tool")
                            .map(Value::to_string)
                            .collect::<String>();
                        let reply = if studio {
                            assert!(
                                result.contains("succeeded") && result.contains("合成原文稿结果")
                            );
                            "原文稿任务已经完成，结果是“合成原文稿结果🙂”。这次只查询原编号，没有重新生成。"
                        } else if receipt {
                            assert!(result.contains("completed") && result.contains("present"));
                            "原收录已经核对，新稿B仍保留待整理。这次只查询原编号，没有重复收录。"
                        } else {
                            assert!(result.contains("synthetic-prior.md"));
                            "查到了原条目“合成原始条目🙂”。这次只查询已有记录，没有重复收录。"
                        };
                        json!({"content":reply})
                    }
                    _ => panic!("duplicate message or observation repeated inference"),
                };
                crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":message}]}))
            })
        }
    });
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
            std::fs::write(output, &*cipher).map_err(|_| "Synthetic save failed".to_string())
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
    let message = msg("synthetic-workflow-context", &text);
    bridge.ingest(&message).await.unwrap();
    bridge.ingest(&message).await.unwrap();
    let turn_id = bridge.status().await.tasks[0].job_id.clone().unwrap();
    let mut confirmed = false;
    let turn = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let t = core.turn(&turn_id).await.unwrap();
            if t.state == "awaiting_confirmation" {
                assert!(!confirmed, "Only the original read may require consent");
                assert_eq!(
                    queries.load(Ordering::SeqCst),
                    0,
                    "Unapproved source text must not authorize access"
                );
                let pending = t.pending.as_ref().unwrap();
                assert!(pending.description.contains(command));
                let confirmation = msg(
                    "synthetic-workflow-confirmation",
                    &format!("确认 {}", pending.id),
                );
                bridge.ingest(&confirmation).await.unwrap();
                bridge.ingest(&confirmation).await.unwrap();
                confirmed = true;
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            if !matches!(t.state.as_str(), "running" | "queued" | "waiting_jobs") {
                break t;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(turn.state, "completed", "{}", turn.text);
    assert!(turn.text.contains("没有重新") || turn.text.contains("没有重复"));
    let session = core.session(&turn.session_id).await.unwrap();
    assert_eq!(session.title, fixture["expected_title"].as_str().unwrap());
    assert!(session
        .messages
        .iter()
        .any(|m| m.role == "user" && m.content == text));
    bridge
        .ingest(&msg(
            "synthetic-workflow-status",
            &format!("/状态 {turn_id}"),
        ))
        .await
        .unwrap();
    bridge
        .ingest(&msg(
            "synthetic-workflow-result",
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
            .is_some_and(|s| s.contains("这次只查询"))));
    bridge.stop().await.unwrap();
    server.abort();
    let encrypted = std::fs::read(&state_file).unwrap();
    let hash = Sha256::digest(&encrypted);
    let plain = crate::wechat_crypto::crypt(&encrypted, false).unwrap();
    let restored = crate::assistant::Engine::new(
        serde_json::from_slice(&plain).unwrap(),
        save,
        model,
        control,
        workspace,
    )
    .await
    .unwrap();
    assert_eq!(restored.turn(&turn_id).await.unwrap().text, turn.text);
    assert_eq!(
        restored.session(&turn.session_id).await.unwrap().title,
        session.title
    );
    assert_eq!(Sha256::digest(std::fs::read(&state_file).unwrap()), hash);
    assert_eq!(rounds.load(Ordering::SeqCst), 3);
    assert_eq!(queries.load(Ordering::SeqCst), 1);
    assert_eq!(
        serde_json::to_string(&journal.lock().await.page(
            "studio:event",
            "synthetic-original-task",
            0,
            100
        ))
        .unwrap(),
        before
    );
    assert_eq!(std::fs::read_to_string(&prior).unwrap(), "合成原始条目🙂");
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"PRESERVE");
    if let Some(bytes) = receipt_before {
        assert_eq!(
            std::fs::read(
                root.path()
                    .join(format!("inbox/receipts/{receipt_id}.receipt.dpapi"))
            )
            .unwrap(),
            bytes
        );
        let restored = crate::inbox_receipts::ReceiptStore::new(
            root.path().join("inbox/receipts"),
            root.path().join("workspace/Wiki"),
        );
        let view = restored.get(&receipt_id).unwrap();
        assert_eq!(
            view.state,
            crate::inbox_receipts::InboxSubmissionState::Completed
        );
        let result = view.result.unwrap();
        assert_eq!(
            std::fs::read_dir(
                root.path()
                    .join("workspace/Wiki/raw/inbox")
                    .join(result.day)
            )
            .unwrap()
            .count(),
            1
        );
        assert!(std::fs::read_to_string(result.path)
            .unwrap()
            .contains("合成原始收录A🙂"));
    }
}
