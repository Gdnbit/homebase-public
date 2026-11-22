use super::*;
use crate::assistant_documents::{Begin, Outcome, PageRange};

#[cfg(windows)]
#[tokio::test]
#[ignore = "explicit installed PDF parser and synthetic fixture paths required"]
async fn wechat_pdf_real_parser_reopens_original_without_model_or_foreign_access() {
    let runtime = PathBuf::from(std::env::var_os("HOMEBASE_DOCUMENT_RUNTIME").expect("fixed local runtime"));
    let fixture_path = PathBuf::from(std::env::var_os("HOMEBASE_DOCUMENT_FIXTURE").expect("synthetic PDF"));
    assert!(runtime.is_absolute() && fixture_path.is_absolute());
    let bytes = std::fs::read(fixture_path).unwrap();
    let (engine, models, controls, saved, _root) = fixture(false, vec![]).await;
    engine.configure_document_runtime(runtime.clone()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let request = |source: &str, bytes: Option<Vec<u8>>, read_only| crate::wechat::PdfRequest {
        source: source.into(), id: id.clone(), name: "隔离.pdf".into(),
        bytes: bytes.map(zeroize::Zeroizing::new), read_only,
    };
    assert!(engine.channel_pdf(request("synthetic-owner", None, true)).await.unwrap().is_none());
    assert!(saved.lock().await.sessions.is_empty());
    let result = engine.channel_pdf(request("synthetic-owner", Some(bytes), false)).await.unwrap().unwrap();
    assert!(result.contains("中文🙂"));
    let state = saved.lock().await.clone();
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(engine.documents().metadata(&id).unwrap().origin_scope, state.sessions[0].id);
    assert!(state.sessions[0].turns.is_empty());
    let reopened = Engine::new_with_documents(state, engine.save.clone(), engine.model.clone(), engine.control.clone(), engine.workspace.clone(), engine.documents().clone()).await.unwrap();
    reopened.configure_document_runtime(runtime).unwrap();
    assert_eq!(reopened.channel_pdf(request("synthetic-owner", None, true)).await.unwrap(), Some(result));
    assert!(reopened.channel_pdf(request("synthetic-owner", Some(b"%PDF-different".to_vec()), false)).await.is_err());
    assert!(reopened.channel_pdf(request("foreign-owner", None, true)).await.is_err());
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(controls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn document_tool_paginates_serialized_json_without_losing_escaped_text_or_references() {
    let(e,_,_,_,_root)=fixture(false,vec![]).await;
    let text="\"\\\n中文🙂".repeat(600);
    let file=attachment(&e,"new",&text).await;let reference=file.document.as_ref().unwrap().clone();
    let mut req=request("分页读取合成资料","doc-wire-pagination");req.attachments=vec![file];let ack=e.send(req).await.unwrap();
    let mut joined=String::new();let mut offset=0;
    loop {
        let value=e.document_tool(&ack.turn_id,&json!({"action":"read","document_id":reference.document_id,"offset":offset,"limit":8000})).await.unwrap();
        assert!(value.to_string().len()<=9000);let wire=wire::bounded(value);assert!(wire["text"].is_string());assert!(wire["truncated"].is_null());
        joined.push_str(wire["text"].as_str().unwrap());
        if let Some(next)=wire["next_offset"].as_u64(){assert!(next>offset as u64);offset=next as usize;}else{break;}
    }
    assert_eq!(joined,format!("【第1页】\n{text}"));
    e.change_turn(&ack.turn_id,|s,t|{
        let template=s.messages.iter().find(|m|m.role=="user").unwrap().clone();
        for _ in 0..30 {
            let mut message=template.clone();message.id=uuid::Uuid::new_v4().to_string();message.turn_id=Some(s.turns[t].id.clone());
            let info=&mut message.attachments[0];let doc=info.document.as_mut().unwrap();doc.document_id=uuid::Uuid::new_v4().to_string();doc.source_name=format!("{}.pdf","\"".repeat(176));
            s.messages.push(message);
        }Ok(())
    }).await.unwrap();
    let mut offset=0;let mut ids=std::collections::BTreeSet::new();
    loop {
        let value=e.document_tool(&ack.turn_id,&json!({"action":"list","offset":offset,"limit":20})).await.unwrap();
        assert_eq!(value["total"],31);assert!(value.to_string().len()<=9000);let wire=wire::bounded(value);assert!(wire["documents"].is_array());
        for doc in wire["documents"].as_array().unwrap(){assert!(ids.insert(doc["document_id"].as_str().unwrap().to_string()));}
        if let Some(next)=wire["next_offset"].as_u64(){assert!(next>offset as u64);offset=next as usize;}else{break;}
    }
    assert_eq!(ids.len(),31);
}

#[tokio::test]
async fn document_default_read_uses_the_latest_committed_selection_of_the_same_file() {
    let(e,_,_,_,_root)=fixture(false,vec![]).await;
    let file=attachment(&e,"new","前面🙂后面🙂").await;let reference=file.document.as_ref().unwrap().clone();
    let mut req=request("先看整页","doc-selection-old");req.attachments=vec![file];let first=e.send(req).await.unwrap();
    let access=e.document_access(&reference.document_id,&first.session_id).await.unwrap();let from="【第1页】\n前面🙂".len();
    let selected=e.documents().excerpt(&access,&reference.extraction_id,PageRange{from:1,to:1},Some((from,from+10))).unwrap();
    let mut req=request("这次只看后半段","doc-selection-latest");req.session_id=Some(first.session_id.clone());req.attachments=vec![TextAttachment{name:selected.name,content:selected.content,document:Some(selected.document)}];
    let next=e.send(req).await.unwrap();
    assert_eq!(e.document_tool(&next.turn_id,&json!({"action":"read","document_id":reference.document_id})).await.unwrap()["text"],"后面🙂");
    assert_eq!(e.document_tool(&first.turn_id,&json!({"action":"read","document_id":reference.document_id})).await.unwrap()["text"],"【第1页】\n前面🙂后面🙂");
}

#[tokio::test]
async fn document_model_round_trip_receives_real_verified_tool_text_without_reading_sources() {
    let(e,_,executions,saved,_root)=fixture(false,vec![]).await;
    let file=attachment(&e,"new","合成模型往返：蓝色圆点🙂").await;
    let id=file.document.as_ref().unwrap().document_id.clone();
    let count=Arc::new(AtomicUsize::new(0));let captured=Arc::new(Mutex::new(vec![]));
    let calls=count.clone();let requests=captured.clone();
    let model:Model=Arc::new(move|request|{
        let calls=calls.clone();let requests=requests.clone();let id=id.clone();
        Box::pin(async move{
            requests.lock().await.push(request);
            Ok(if calls.fetch_add(1,Ordering::SeqCst)==0{reply(vec![("homebase_document",json!({"action":"read","document_id":id}))])}else{reply(vec![])})
        })
    });
    let mut state=saved.lock().await.clone();state.settings.enabled=true;
    let engine=Engine::new_with_documents(state,e.save.clone(),model,e.control.clone(),e.workspace.clone(),e.documents().clone()).await.unwrap();
    let mut req=request("总结已发送PDF","doc-model-round-trip");req.attachments=vec![file];
    let ack=engine.send(req).await.unwrap();let done=wait(&engine,&ack.turn_id,"completed").await;
    assert_eq!(count.load(Ordering::SeqCst),2);assert_eq!(executions.load(Ordering::SeqCst),0);
    assert_eq!(done.steps[0].name,"homebase_document");assert_eq!(done.steps[0].result["status"],"ready");
    let protocol=serde_json::to_string(&captured.lock().await[1].messages).unwrap();
    assert!(protocol.contains("合成模型往返：蓝色圆点🙂"));assert!(protocol.contains("pdf_text_only"));
    assert!(engine.document_access(done.steps[0].result["document_id"].as_str().unwrap(),"new").await.is_err());
}

#[tokio::test]
async fn document_model_tool_hides_staged_foreign_and_future_queued_inputs() {
    let(e,models,executions,_,_root)=fixture(false,vec![]).await;
    let first=attachment(&e,"new","第一份已发送资料🙂").await;
    let first_id=first.document.as_ref().unwrap().document_id.clone();
    let mut req=request("只看第一份","doc-tool-visible");req.attachments=vec![first];
    let accepted=e.send(req).await.unwrap();
    let staged=attachment(&e,&accepted.session_id,"尚未发送的草稿").await;
    let staged_id=staged.document.as_ref().unwrap().document_id.clone();
    let future=attachment(&e,&accepted.session_id,"后续排队消息的资料").await;
    let future_id=future.document.as_ref().unwrap().document_id.clone();
    let mut req=request("之后处理第二份","doc-tool-future");req.session_id=Some(accepted.session_id.clone());req.attachments=vec![future];
    let queued=e.send(req).await.unwrap();
    let foreign=attachment(&e,"new","其他会话资料").await;
    let foreign_id=foreign.document.as_ref().unwrap().document_id.clone();
    let mut req=request("另一会话","doc-tool-foreign");req.attachments=vec![foreign];e.send(req).await.unwrap();
    let visible=e.document_tool(&accepted.turn_id,&json!({"action":"list"})).await.unwrap();
    assert_eq!(visible["documents"].as_array().unwrap().len(),1);
    assert_eq!(visible["documents"][0]["document_id"],first_id);
    for id in [staged_id,future_id.clone(),foreign_id] {
        assert!(e.document_tool(&accepted.turn_id,&json!({"action":"read","document_id":id})).await.is_err());
    }
    assert!(e.document_tool(&queued.turn_id,&json!({"action":"read","document_id":future_id})).await.is_ok());
    assert!(e.document_tool(&accepted.turn_id,&json!({"action":"list","scope":"new"})).await.is_err());
    assert!(e.document_tool(&accepted.turn_id,&json!({"action":"read","document_id":first_id,"path":"C:/private.pdf"})).await.is_err());
    assert_eq!(models.load(Ordering::SeqCst),0);assert_eq!(executions.load(Ordering::SeqCst),0);
}

#[tokio::test]
async fn document_model_reads_exact_committed_selection_and_pages_without_breaking_utf8() {
    let(e,_,_,_,_root)=fixture(false,vec![]).await;
    let file=attachment(&e,"new",&"中文🙂".repeat(80)).await;
    let reference=file.document.unwrap();
    let access=e.document_access(&reference.document_id,"new").await.unwrap();
    let from="【第1页】\n".len();
    let selected=e.documents().excerpt(&access,&reference.extraction_id,PageRange{from:1,to:1},Some((from,from+10))).unwrap();
    let mut req=request("处理选中文字","doc-tool-selection");
    req.attachments=vec![TextAttachment{name:selected.name,content:selected.content,document:Some(selected.document)}];
    let ack=e.send(req).await.unwrap();
    let exact=e.document_tool(&ack.turn_id,&json!({"action":"read","document_id":reference.document_id})).await.unwrap();
    assert_eq!(exact["text"],"中文🙂");
    assert_eq!(exact["partial"],true);
    assert!(exact["next_offset"].is_null());
    assert!(e.document_tool(&ack.turn_id,&json!({"action":"read","document_id":reference.document_id,"offset":from+1})).await.is_err());
    let mut offset=0;let mut joined=String::new();
    loop {
        let page=e.document_tool(&ack.turn_id,&json!({"action":"read","document_id":reference.document_id,"pages":{"from":1,"to":1},"offset":offset,"limit":27})).await.unwrap();
        joined.push_str(page["text"].as_str().unwrap());
        if let Some(next)=page["next_offset"].as_u64(){assert!(next>offset as u64);offset=next as usize;}else{break;}
    }
    assert_eq!(joined,format!("【第1页】\n{}","中文🙂".repeat(80)));
    assert!(!joined.contains('\u{fffd}'));
    assert!(e.document_tool(&ack.turn_id,&json!({"action":"extract","document_id":reference.document_id})).await.is_err());
    let call=ToolCall{id:"doc-current-runtime".into(),name:"homebase_document".into(),arguments:json!({"action":"list"})};
    e.change_turn(&ack.turn_id,|s,t|{s.turns[t].state="running".into();Ok(())}).await.unwrap();
    assert_eq!(e.execute(&ack.turn_id,&call).await.unwrap().unwrap()["documents"].as_array().unwrap().len(),1);
    e.change_turn(&ack.turn_id,|s,t|{s.turns[t].runtime_version="homebase-da-20261113.6".into();Ok(())}).await.unwrap();
    assert!(e.execute(&ack.turn_id,&call).await.is_err());
    assert!(!wire::tools_for_runtime("homebase-da-20261113.6").iter().any(|v|v["function"]["name"]=="homebase_document"));
    assert!(wire::tools_for_runtime(RUNTIME_VERSION).iter().any(|v|v["function"]["name"]=="homebase_document"));
}
async fn attachment(engine: &Arc<Engine>, scope: &str, text: &str) -> TextAttachment {
    let id = uuid::Uuid::new_v4().to_string();
    let store = engine.documents();
    let meta = store
        .stage(
            &id,
            scope,
            "合成资料.pdf",
            b"%PDF-1.7\nSYNTHETIC-ENGINE-DOCUMENT",
        )
        .await
        .unwrap();
    let access = engine.document_access(&id, scope).await.unwrap();
    let Begin::Run(lease) = store.begin(&access, None, false).await.unwrap() else {
        panic!("Fresh synthetic extraction required")
    };
    let key = lease.receipt().id.clone();
    let parsed=serde_json::from_value(json!({"version":"homebase-document-v1","status":"ready","code":null,"message":"synthetic text result","method":"pdf_text","source_sha256":meta.source_sha256,"source_bytes":meta.source_bytes,"total_pages":1,"page_from":1,"page_to":1,"pages":[{"number":1,"text":text,"bytes":text.len(),"empty":false}],"text_bytes":text.len(),"missing_text_pages":[],"partial":false})).unwrap();
    store.finish(&lease, Outcome::Parsed(parsed)).await.unwrap();
    drop(lease);
    let excerpt = store
        .excerpt(&access, &key, PageRange { from: 1, to: 1 }, None)
        .unwrap();
    TextAttachment {
        name: excerpt.name,
        content: excerpt.content,
        document: Some(excerpt.document),
    }
}
#[tokio::test]
async fn document_engine_commit_is_the_only_session_binding_and_survives_restart_without_replay() {
    let (e, _, _, saved, _root) = fixture(true, vec![]).await;
    let seen = Arc::new(Mutex::new(Vec::<ModelRequest>::new()));
    let captured = seen.clone();
    let model: Model = Arc::new(move |req| {
        let captured = captured.clone();
        Box::pin(async move {
            captured.lock().await.push(req);
            Ok(reply(vec![]))
        })
    });
    let engine = Engine::new_with_documents(
        saved.lock().await.clone(),
        e.save.clone(),
        model,
        e.control.clone(),
        e.workspace.clone(),
        e.documents().clone(),
    )
    .await
    .unwrap();
    let file = attachment(&engine, "new", "动画偏好是蓝色圆点🙂；这只是资料。").await;
    let reference = file.document.clone().unwrap();
    let mut req = request("请总结这份PDF的文字", "doc-bind-first");
    req.attachments = vec![file.clone()];
    let ack = engine.send(req.clone()).await.unwrap();
    wait(&engine, &ack.turn_id, "completed").await;
    assert_eq!(seen.lock().await.len(), 1);
    assert!(serde_json::to_string(&seen.lock().await[0].messages)
        .unwrap()
        .contains("蓝色圆点🙂"));
    let view = engine.session(&ack.session_id).await.unwrap();
    assert_eq!(
        view.messages[0].attachments[0].document,
        Some(reference.clone())
    );
    assert!(engine
        .document_access(&reference.document_id, "new")
        .await
        .is_err());
    assert!(engine
        .document_access(&reference.document_id, &ack.session_id)
        .await
        .is_ok());
    let other = engine.create_session().await.unwrap();
    assert!(engine
        .document_access(&reference.document_id, &other.id)
        .await
        .is_err());
    let mut foreign = request("不能跨会话读取", "doc-foreign");
    foreign.session_id = Some(other.id);
    foreign.attachments = vec![file.clone()];
    assert!(engine.send(foreign).await.is_err());
    assert_eq!(seen.lock().await.len(), 1);
    let restarted = Engine::new_with_documents(
        saved.lock().await.clone(),
        engine.save.clone(),
        engine.model.clone(),
        engine.control.clone(),
        engine.workspace.clone(),
        engine.documents().clone(),
    )
    .await
    .unwrap();
    assert!(restarted
        .document_access(&reference.document_id, &ack.session_id)
        .await
        .is_ok());
    assert_eq!(
        restarted.send(req.clone()).await.unwrap().turn_id,
        ack.turn_id
    );
    assert_eq!(seen.lock().await.len(), 1);
    let mut follow = request("继续基于这份文字核对", "doc-bind-second");
    follow.session_id = Some(ack.session_id.clone());
    follow.attachments = vec![file];
    let next = restarted.send(follow).await.unwrap();
    wait(&restarted, &next.turn_id, "completed").await;
    assert_eq!(seen.lock().await.len(), 2);
    // Existing accepted receipts remain usable if a saved source later breaks.
    let result_dir = engine
        .workspace
        .join(".documents")
        .join(&reference.document_id);
    let source = result_dir.join("source.dpapi");
    std::fs::write(&source, b"synthetic broken source").unwrap();
    assert_eq!(restarted.send(req).await.unwrap().turn_id, ack.turn_id);
    assert_eq!(seen.lock().await.len(), 2);
}
#[tokio::test]
async fn document_tampering_and_wrong_origin_are_rejected_before_session_creation_or_models() {
    let (e, models, controller, _, _root) = fixture(false, vec![]).await;
    let file = attachment(&e, "new", "原始摘录🙂").await;
    let mut req = request("核对文档", "doc-tamper");
    req.attachments = vec![file.clone()];
    req.attachments[0].content.push_str("伪造文字");
    assert!(e.send(req.clone()).await.is_err());
    assert!(e.sessions().await.is_empty());
    req.attachments[0] = file.clone();
    req.attachments[0].document.as_mut().unwrap().source_name = "另一来源.pdf".into();
    assert!(e.send(req.clone()).await.is_err());
    assert!(e.sessions().await.is_empty());
    req.attachments[0] = file;
    req.attachments[0].document.as_mut().unwrap().text_from += 1;
    assert!(e.send(req).await.is_err());
    assert!(e.sessions().await.is_empty());
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(controller.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn document_reference_changes_identity_and_late_ack_does_not_clear_a_new_reference() {
    let (e, _, _, _, root) = fixture(false, vec![]).await;
    let file = attachment(&e, "new", "同一文字🙂").await;
    let mut req = request("原稿", "doc-durable-ack");
    req.attachments = vec![file];
    let store = crate::assistant_desktop::DesktopStore::new(root.path().join("desktop-doc"));
    let original = crate::assistant_desktop::DesktopBody {
        image: None,
        text: req.text.clone(),
        route: String::new(),
        mode: req.mode.clone().unwrap_or_else(|| "auto".into()),
        files: req.attachments.clone(),
    };
    let saved = store.save("new", 0, original.clone()).await.unwrap();
    let pending = store
        .prepare("new", saved.revision, req.clone())
        .await
        .unwrap();
    let ack = e.send(req.clone()).await.unwrap();
    let mut changed = original;
    changed.files[0].document.as_mut().unwrap().extraction_id = "f".repeat(64);
    let newer = store
        .save("new", pending.revision, changed.clone())
        .await
        .unwrap();
    assert!(e
        .send(SendRequest {
            image: None,
            attachments: changed.files.clone(),
            ..req.clone()
        })
        .await
        .is_err());
    let accepted = store.accept("new", &req, ack.clone()).await;
    assert_eq!(accepted.ack.unwrap().turn_id, ack.turn_id);
    assert!(accepted.entry.unwrap().body.files == changed.files);
    assert!(store.get("new").unwrap().revision > newer.revision);
}
#[tokio::test]
async fn failed_engine_publish_does_not_archive_document_and_original_retry_uses_one_accepted_message(
) {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = fail.clone();
    let capture = saved.clone();
    let save: Save = Arc::new(move |state| {
        let flag = flag.clone();
        let capture = capture.clone();
        Box::pin(async move {
            if flag.load(Ordering::SeqCst)
                && state.sessions.iter().any(|s| {
                    s.messages
                        .iter()
                        .any(|m| m.attachments.iter().any(|a| a.document.is_some()))
                })
            {
                return Err("synthetic message publish failure".into());
            }
            *capture.lock().await = state;
            Ok(())
        })
    });
    let engine = Engine::new_with_documents(
        saved.lock().await.clone(),
        save,
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
        e.documents().clone(),
    )
    .await
    .unwrap();
    let file = attachment(&engine, "new", "失败后保留的资料").await;
    let id = file.document.as_ref().unwrap().document_id.clone();
    let mut req = request("保存这次资料", "doc-save-fault");
    req.attachments = vec![file];
    assert!(engine.send(req.clone()).await.is_err());
    assert!(
        engine.sessions().await.is_empty(),
        "A failed first message must not leave an empty conversation"
    );
    assert!(engine.desktop_receipt(&req).await.unwrap().is_none());
    assert!(engine.document_access(&id, "new").await.is_ok());
    fail.store(false, Ordering::SeqCst);
    let ack = engine.send(req.clone()).await.unwrap();
    assert_eq!(engine.sessions().await.len(), 1);
    assert!(engine.document_access(&id, "new").await.is_err());
    assert!(engine.document_access(&id, &ack.session_id).await.is_ok());
    assert_eq!(engine.send(req).await.unwrap().turn_id, ack.turn_id);
    assert_eq!(
        saved
            .lock()
            .await
            .sessions
            .iter()
            .flat_map(|s| &s.messages)
            .filter(|m| m.role == "user" && m.attachments.iter().any(|a| a.document.is_some()))
            .count(),
        1
    );
}
#[test]
fn legacy_text_attachment_json_and_request_fingerprint_are_byte_compatible() {
    let mut req = request("旧文本消息", "legacy-doc-compat");
    req.mode = None;
    req.attachments = vec![TextAttachment {
        name: "旧文本.md".into(),
        content: "原字节🙂".into(),
        document: None,
    }];
    assert_eq!(
        serde_json::to_value(&req.attachments).unwrap(),
        json!([{"name":"旧文本.md","content":"原字节🙂"}])
    );
    let original = format!("{:x}", Sha256::digest(format!("desktop::{}", req.text)));
    let expected=format!("{:x}",Sha256::digest(serde_json::to_vec(&json!({"legacy":original,"route":null,"attachments":[{"name":"旧文本.md","content":"原字节🙂"}]})).unwrap()));
    assert_eq!(request_fingerprint(&req, "desktop").unwrap(), expected);
    let (_, info) = input::prepare(&req.text, &req.attachments).unwrap();
    assert!(serde_json::to_value(&info[0])
        .unwrap()
        .get("document")
        .is_none());
}

#[tokio::test]
async fn first_message_publish_is_atomic_for_wechat_and_does_not_leave_empty_conversations() {
    let (e, models, controller, saved, _root) = fixture(false, vec![]).await;
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = fail.clone();
    let capture = saved.clone();
    let save: Save = Arc::new(move |state| {
        let flag = flag.clone();
        let capture = capture.clone();
        Box::pin(async move {
            if flag.load(Ordering::SeqCst) && state.sessions.iter().any(|s| !s.messages.is_empty())
            {
                return Err("synthetic first message failure".into());
            }
            *capture.lock().await = state;
            Ok(())
        })
    });
    let engine = Engine::new_with_documents(
        saved.lock().await.clone(),
        save,
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
        e.documents().clone(),
    )
    .await
    .unwrap();
    let message = json!({"operation":"submit","source_key":"atomic-wechat-fixture","text":"合成首次微信消息","request_id":"stable-first-wechat"});
    assert!(engine.channel(message.clone()).await.is_err());
    assert!(engine.sessions().await.is_empty());
    assert!(saved.lock().await.sessions.is_empty());
    fail.store(false, Ordering::SeqCst);
    let ack = engine.channel(message.clone()).await.unwrap();
    let duplicate = engine.channel(message).await.unwrap();
    assert_eq!(ack["turn_id"], duplicate["turn_id"]);
    assert_eq!(engine.sessions().await.len(), 1);
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(controller.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_new_draft_messages_cannot_claim_the_same_source_in_two_sessions() {
    let (e, models, _, _, _root) = fixture(false, vec![]).await;
    let file = attachment(&e, "new", "并发测试资料").await;
    let mut a = request("第一条", "doc-race-a");
    a.attachments = vec![file];
    let mut b = a.clone();
    b.request_id = "doc-race-b".into();
    let (first, second) = tokio::join!(e.send(a), e.send(b));
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(e.sessions().await.len(), 1);
    assert_eq!(models.load(Ordering::SeqCst), 0);
}
