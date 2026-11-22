// Explicit opt-in: real enabled free model, synthetic WeChat and isolated business state.
use super::*;
use crate::assistant::{Engine as Core, Settings, State as CoreState, TurnView};
use crate::assistant_protocol::{self as wire, ModelRequest};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

async fn live_send(bridge: &Bridge, id: &str, text: &str) {
    bridge.ingest(&msg(id, text)).await.unwrap();
}
fn live_id(id: &str) -> String {
    format!(
        "wx-{:x}",
        Sha256::digest(format!("synthetic-bot:synthetic-owner:{id}").as_bytes())
    )
}
async fn live_turn(bridge: &Bridge, core: &Core, id: &str) -> TurnView {
    let until = tokio::time::Instant::now() + Duration::from_secs(240);
    loop {
        if let Some(tid) = bridge
            .status()
            .await
            .tasks
            .iter()
            .find(|t| t.id == live_id(id))
            .and_then(|t| t.job_id.as_ref())
        {
            return core.turn(tid).await.unwrap();
        }
        assert!(tokio::time::Instant::now() < until, "message not submitted");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn live_wait(core: &Core, tid: &str) -> TurnView {
    let until = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        let t = core.turn(tid).await.unwrap();
        if !matches!(
            t.state.as_str(),
            "queued" | "running" | "waiting_jobs" | "waiting_input_queue"
        ) {
            return t;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "live task timeout: {}",
            t.state
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
async fn live_card(seen: &Arc<Mutex<Vec<Value>>>, needle: &str) {
    let until = tokio::time::Instant::now() + Duration::from_secs(40);
    loop {
        if seen
            .lock()
            .await
            .iter()
            .any(|r| r["body"]["msg"]["item_list"].to_string().contains(needle))
        {
            return;
        }
        assert!(tokio::time::Instant::now() < until, "confirmation not sent");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
#[tokio::test]
#[ignore = "explicit live free-model authorization, owned backend and isolated lab required"]
async fn live_free_model_personal_agent_roundtrip() {
    let lab =
        PathBuf::from(std::env::var_os("HOMEBASE_PERSONAL_LIVE_LAB").expect("explicit live lab"));
    assert!(lab.is_absolute());
    let case = std::env::var("HOMEBASE_PERSONAL_LIVE_CASE").unwrap_or("memory".into());
    let root = lab.join(format!("run-{}-{}", case, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("scratch")).unwrap();
    std::fs::write(
        root.join("scratch/check.json"),
        "{\"status\":\"ready\",\"version\":1}",
    )
    .unwrap();
    let sentinel = root.join("untouched.txt");
    std::fs::write(&sentinel, "KEEP ISOLATED ORIGINAL").unwrap();
    let settings: Settings =
        serde_json::from_slice(&std::fs::read(lab.join("settings.json")).unwrap()).unwrap();
    assert!(settings.enabled && settings.free_only && settings.route_id.ends_with(":free"));
    let trace = Arc::new(Mutex::new(Vec::<Value>::new()));
    let output = trace.clone();
    let model_lab = lab.clone();
    let model_root = root.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = calls.clone();
    let fault_case = case.clone();
    let injected = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let injected_model = injected.clone();
    let model: crate::assistant::Model = Arc::new(move |r: ModelRequest| {
        let lab = model_lab.clone();
        let output = output.clone();
        let root = model_root.clone();
        let calls = calls.clone();
        let fault_case = fault_case.clone();
        let injected = injected_model.clone();
        Box::pin(async move {
            if matches!(fault_case.as_str(), "recovery" | "unknown")
                && root.join("scratch/recovery.txt").is_file()
                && !injected.swap(true, Ordering::SeqCst)
            {
                let failure = if fault_case == "recovery" {
                    let mut f = crate::model_capability_io::reliability::Failure::http(503);
                    f.attempts = 3;
                    f
                } else {
                    crate::model_capability_io::reliability::Failure::unknown(
                        crate::model_capability_io::reliability::Kind::Timeout,
                    )
                };
                std::fs::write(root.join("injected-failure.json"),serde_json::to_vec_pretty(&json!({"simulated_fault":true,"after_real_local_write":true,"failure":failure})).unwrap()).unwrap();
                return Err(failure.encode());
            }
            let index = calls.fetch_add(1, Ordering::SeqCst) + 1;
            eprintln!("live model request {index}");
            let input = wire::body("chat", "openrouter", &r)?;
            let payload=serde_json::to_vec(&json!({"route_id":r.route_id,"input":input,"credential_source":r.credential_source})).map_err(|_|"model payload encoding failed")?;
            let mut cmd = tokio::process::Command::new("node");
            cmd.arg(".tmp/live-personal-model.mjs")
                .arg("model")
                .current_dir("C:/HomeBase/source")
                .env("HOMEBASE_PERSONAL_LIVE_LAB", lab)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x08000000);
            let mut child = cmd.spawn().map_err(|_| "live proxy launch failed")?;
            child
                .stdin
                .take()
                .ok_or("proxy stdin missing")?
                .write_all(&payload)
                .await
                .map_err(|_| "proxy input failed")?;
            let result = tokio::time::timeout(Duration::from_secs(220), child.wait_with_output())
                .await
                .map_err(|_| "live provider result unknown; do not retry")?
                .map_err(|_| "live proxy disconnected")?;
            let value: Value = serde_json::from_slice(&result.stdout)
                .map_err(|_| "live provider response invalid")?;
            if !result.status.success() {
                if let Some(details) = value
                    .get("model_failure")
                    .and_then(|v| {
                        serde_json::from_value::<crate::model_capability_io::reliability::Failure>(
                            v.clone(),
                        )
                        .ok()
                    })
                    .filter(|f| {
                        crate::model_capability_io::reliability::Failure::decode(&f.encode())
                            .is_some()
                    })
                {
                    return Err(details.encode());
                }
                return Err(format!(
                    "real model call failed: {}",
                    value["live_proxy_error"].as_str().unwrap_or("unknown")
                ));
            }
            let reply = wire::parse("chat", &value)?;
            if matches!(fault_case.as_str(), "recovery" | "unknown") && reply.diagnostics.is_none()
            {
                return Err("Updated backend transport diagnostic missing".into());
            }
            let record = json!({"index":index,"text":reply.text,"calls":reply.calls,"usage":reply.usage,"transport":reply.diagnostics});
            eprintln!(
                "live model {index}: tools={} reply={}",
                reply
                    .calls
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
                reply.text.chars().take(120).collect::<String>()
            );
            let mut trace = output.lock().await;
            trace.push(record);
            std::fs::write(
                root.join("model-trace.json"),
                serde_json::to_vec_pretty(&*trace).unwrap(),
            )
            .map_err(|_| "trace save failed")?;
            Ok(reply)
        })
    });
    let reads = root.join("scratch/check.json");
    let read_file = reads.clone();
    let effect_file = root.join("scratch/recovery.txt");
    let write_file = effect_file.clone();
    let effects = Arc::new(AtomicUsize::new(0));
    let writes = effects.clone();
    let allow_write = matches!(case.as_str(), "recovery" | "unknown");
    let runner: Runner = Arc::new(move |command, args| {
        let file = write_file.clone();
        let writes = writes.clone();
        Box::pin(async move {
            if allow_write
                && command == "assistant_local_execute"
                && args["operation"] == "files"
                && args["request"]["action"] == "write"
                && args["request"]["path"] == "recovery.txt"
            {
                use std::io::Write;
                let content = args["request"]["content"]
                    .as_str()
                    .filter(|s| s.len() <= 1000)
                    .ok_or_else(|| {
                        crate::control_protocol::ControlError::new(
                            "LIVE_LAB_DENIED",
                            "only short synthetic content",
                        )
                    })?;
                let mut output = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&file)
                    .map_err(|_| {
                        crate::control_protocol::ControlError::new(
                            "LIVE_LAB_EXISTS",
                            "test file already exists; never overwrite",
                        )
                    })?;
                output.write_all(content.as_bytes()).unwrap();
                writes.fetch_add(1, Ordering::SeqCst);
                return Ok(
                    json!({"status":"succeeded","workspace_id":"scratch","path":"recovery.txt","bytes":content.len(),"sha256":format!("{:x}",Sha256::digest(content.as_bytes())),"created":true}),
                );
            }
            Err(crate::control_protocol::ControlError::new(
                "LIVE_LAB_DENIED",
                "测试禁止调用真实业务或任意文件/终端",
            ))
        })
    });
    std::fs::create_dir_all(root.join("control")).unwrap();
    let ctl = Controller::new(
        serde_json::from_str(include_str!("../../control/commands.json")).unwrap(),
        &root.join("control"),
        runner,
    )
    .unwrap();
    let controller = ctl.clone();
    let control: crate::assistant::Control = Arc::new(move |r| {
        let ctl = ctl.clone();
        let file = read_file.clone();
        Box::pin(async move {
            if r.method == "assistant.local_read" {
                let req = &r.args["request"];
                if r.args["operation"] != "files" {
                    return Err(crate::control_protocol::ControlError::new(
                        "LIVE_LAB_DENIED",
                        "仅测试文件读取",
                    ));
                }
                match req["action"].as_str() {
                    Some("workspaces") => {
                        return Ok(
                            json!({"status":"succeeded","workspaces":[{"id":"scratch","name":"隔离测试工作区","read":true,"write":allow_write}]}),
                        )
                    }
                    Some("list")
                        if req["path"].is_null()
                            || matches!(req["path"].as_str(), Some("" | "." | "/")) =>
                    {
                        return Ok(
                            json!({"status":"succeeded","workspace_id":"scratch","entries":[{"name":"check.json","is_dir":false,"bytes":std::fs::metadata(&file).unwrap().len()}],"next_offset":null}),
                        )
                    }
                    Some("stat") if req["path"] == "check.json" => {
                        return Ok(
                            json!({"status":"succeeded","path":"check.json","is_dir":false,"bytes":std::fs::metadata(&file).unwrap().len()}),
                        )
                    }
                    Some("read") if req["path"] == "check.json" => {
                        let bytes = std::fs::read(&file).unwrap();
                        return Ok(
                            json!({"status":"succeeded","workspace_id":"scratch","path":"check.json","content":String::from_utf8(bytes.clone()).unwrap(),"sha256":format!("{:x}",Sha256::digest(&bytes)),"bytes":bytes.len(),"next_char_offset":null}),
                        );
                    }
                    _ => {
                        return Err(crate::control_protocol::ControlError::new(
                            "LIVE_LAB_DENIED",
                            "只允许读取隔离check.json",
                        ))
                    }
                }
            }
            ctl.handle(r).await
        })
    });
    let mut initial = CoreState::default();
    initial.settings = settings;
    let saved = Arc::new(Mutex::new(initial.clone()));
    let persisted = saved.clone();
    let state_file = root.join("state.dpapi");
    let save: crate::assistant::Save = Arc::new(move |s| {
        let saved = persisted.clone();
        let file = state_file.clone();
        Box::pin(async move {
            let plain = serde_json::to_vec(&s).map_err(|_| "isolated state encoding")?;
            let cipher = crate::wechat_crypto::crypt(&plain, true)?;
            crate::control_protocol::publish_private(&file, &cipher)
                .await
                .map_err(|_| "isolated save failed")?;
            *saved.lock().await = s;
            Ok(())
        })
    });
    let core = Core::new(
        initial,
        save.clone(),
        model.clone(),
        control.clone(),
        root.join("workspace"),
    )
    .await
    .unwrap();
    let title = "HB-LIVE 测试动画偏好";
    let seed = core
        .memory_save(crate::assistant_personal::MemoryInput {
            revision: 0,
            id: None,
            title: title.into(),
            content: "测试动画使用红色圆点，字幕使用详细英文".into(),
            expires_at: Some((chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339()),
        })
        .await
        .unwrap();
    let (base, seen, server) = fake_api(200, json!({})).await;
    let stored = account(&base);
    let (template, _) = test_bridge(stored.clone(), &root, controller);
    let e = core.clone();
    let agent: Agent = Arc::new(move |r| {
        let e = e.clone();
        Box::pin(async move { e.channel(r).await })
    });
    let mut api = Api::new().unwrap();
    api.test_origin = Some(base.clone());
    let mut bridge = Bridge::new_with_agent(
        stored,
        api,
        template.save.clone(),
        control.clone(),
        root.clone(),
        Some(agent),
    );
    bridge.start().await.unwrap();
    let outcome=async {
        if matches!(case.as_str(),"recovery"|"unknown") {
            live_send(&bridge,"live-write-recover","请在已授权的 scratch 工作区新建 recovery.txt，内容为‘HB-LIVE 隔离恢复验证，只创建一次’。只创建这一个测试文件，不改已有文件或记忆。创建成功后，简短告诉我文件名和结果，不用创建计划或其他任务。").await;
            let t=live_turn(&bridge,&core,"live-write-recover").await;let stopped=live_wait(&core,&t.id).await;
            assert_eq!(stopped.state,"failed","{}",stopped.text);assert!(injected.load(Ordering::SeqCst));assert_eq!(effects.load(Ordering::SeqCst),1);assert_eq!(stopped.jobs.len(),1);
            let job_id=stopped.jobs[0].id.clone();let hash=format!("{:x}",Sha256::digest(std::fs::read(&effect_file).unwrap()));
            bridge.stop().await.unwrap();
            let restore_state=saved.lock().await.clone();
            let restored=Core::new(restore_state,save.clone(),model.clone(),control.clone(),root.join("workspace")).await.unwrap();
            let stored=bridge.state.lock().await.stored.clone();let e=restored.clone();
            let agent:Agent=Arc::new(move |r|{let e=e.clone();Box::pin(async move{e.channel(r).await})});
            let mut api=Api::new().unwrap();api.test_origin=Some(base.clone());
            bridge=Bridge::new_with_agent(stored,api,template.save.clone(),control.clone(),root.clone(),Some(agent));bridge.start().await.unwrap();
            tokio::time::sleep(Duration::from_millis(250)).await;
            assert_eq!(effects.load(Ordering::SeqCst),1,"restart replayed file action");
            if case=="recovery" {
                assert!(stopped.resumable);assert!(restored.turn(&t.id).await.unwrap().resumable);
                live_send(&bridge,"live-resume-original",&format!("继续 {}",t.id)).await;
                let done=live_wait(&restored,&t.id).await;assert_eq!(done.state,"completed","{}",done.text);assert_eq!(done.jobs.len(),1);assert_eq!(done.jobs[0].id,job_id);
                let until=tokio::time::Instant::now()+Duration::from_secs(40);
                loop {
                    let channel=bridge.status().await;
                    let delivered=channel.tasks.iter().any(|r|r.id==live_id("live-resume-original")&&r.status=="succeeded"&&r.delivery=="accepted");
                    let actual_reply=seen.lock().await.iter().any(|r|r["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().is_some_and(|s|s.contains(&done.text)));
                    if delivered&&actual_reply{std::fs::write(root.join("final-delivery.json"),serde_json::to_vec_pretty(&json!({"channel_record_accepted":true,"actual_final_text_sent":true,"tasks":channel.tasks})).unwrap()).unwrap();break;}
                    if tokio::time::Instant::now()>=until{std::fs::write(root.join("delivery-diagnostic.json"),serde_json::to_vec_pretty(&json!({"tasks":channel.tasks,"expected_text":done.text,"received":*seen.lock().await})).unwrap()).unwrap();panic!("final result not delivered to simulated WeChat");}
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                let before=requests.load(Ordering::SeqCst);live_send(&bridge,"live-resume-original",&format!("继续 {}",t.id)).await;tokio::time::sleep(Duration::from_millis(300)).await;assert_eq!(requests.load(Ordering::SeqCst),before,"duplicate WeChat message inferred again");
                assert_eq!(effects.load(Ordering::SeqCst),1);assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(&effect_file).unwrap())),hash);
                Ok(json!({"known_rejection_resumed":true,"fault_injected_not_provider_failure":true,"original_turn_id":t.id,"final_turn_id":done.id,"original_job_id":job_id,"write_count":1,"file_hash_preserved":true,"wechat_duplicate_message_dedup":true,"final_reply_delivered_to_fake_wechat":true,"final_reply":done.text,"transport":done.model_diagnostics}))
            }else{
                assert!(!stopped.resumable);assert!(restored.resume(&t.id).await.is_err());let before=requests.load(Ordering::SeqCst);
                live_send(&bridge,"live-unknown-continue",&format!("继续 {}",t.id)).await;tokio::time::sleep(Duration::from_millis(300)).await;
                live_card(&seen,"不能重复推理").await;
                assert_eq!(requests.load(Ordering::SeqCst),before);assert_eq!(effects.load(Ordering::SeqCst),1);assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(&effect_file).unwrap())),hash);
                Ok(json!({"unknown_timeout_blocked":true,"fault_injected_not_provider_failure":true,"original_turn_id":t.id,"original_job_id":job_id,"write_count":1,"file_hash_preserved":true,"no_model_retry_after_restart_or_wechat_continue":true,"message":stopped.text}))
            }
        }else if case=="memory" {
            live_send(&bridge,"live-correct","之前那条 HB-LIVE 测试动画偏好记错了：应该用蓝色圆点，字幕用简短中文。请纠正原记录，原有效期保持不变，只改这条测试记录。").await;
            let t=live_turn(&bridge,&core,"live-correct").await;let pending=live_wait(&core,&t.id).await;
            std::fs::write(root.join("pending.json"),serde_json::to_vec_pretty(&pending).unwrap()).unwrap();
            assert_eq!(pending.state,"awaiting_confirmation","{}",pending.text);
            let p=pending.pending.as_ref().unwrap();assert_eq!(p.arguments["action"],"correct","real model did not select correction");assert_eq!(p.arguments["id"],seed["id"]);
            live_card(&seen,"纠正原来的长期记忆").await;
            live_send(&bridge,"live-correct-confirm",&format!("确认 {}",p.id)).await;
            let done=live_wait(&core,&t.id).await;assert_eq!(done.state,"completed","{}",done.text);
            let personal=core.personal().await;assert_eq!(personal["memories"].as_array().unwrap().len(),1);assert_eq!(personal["memories"][0]["id"],seed["id"]);assert_eq!(personal["memories"][0]["expires_at"],seed["expires_at"]);assert!(personal["memories"][0]["content"].as_str().unwrap().contains("蓝色"));assert!(personal["memories"][0]["content"].as_str().unwrap().contains("中文"));assert_eq!(personal["memories"][0]["corrections"].as_array().unwrap().len(),1);
            Ok::<Value,String>(json!({"memory_correction":true,"turn_id":t.id,"final_reply":done.text,"memory":personal["memories"][0]}))
        }else if case=="steering" {
            live_send(&bridge,"live-old-plan","帮我记住一条临时测试事项，标题为 HB-LIVE 待撤销方案，内容是‘测试动画先用橙色方块’，有效期一天。只新增这条测试记录，等我确认。").await;
            let t=live_turn(&bridge,&core,"live-old-plan").await;let old=live_wait(&core,&t.id).await;assert_eq!(old.state,"awaiting_confirmation","{}",old.text);
            let nonce=old.pending.as_ref().unwrap().id.clone();live_card(&seen,"HB-LIVE 待撤销方案").await;
            live_send(&bridge,"live-change-plan","先别保存刚才那条了。我改主意：这次只给我两句简短中文的动画说明，画面改成绿色三角形，不要更改任何记忆或创建其他任务，继续在原来这件事里处理。").await;
            let chat=live_turn(&bridge,&core,"live-change-plan").await;let receipt=live_wait(&core,&chat.id).await;assert_eq!(receipt.state,"completed","{}",receipt.text);assert_eq!(receipt.steered_turn_id.as_deref(),Some(t.id.as_str()),"{}",receipt.text);
            assert!(core.confirm(&t.id,&nonce,true).await.is_err());
            let done=live_wait(&core,&t.id).await;assert_eq!(done.state,"completed","{}",done.text);assert_eq!(done.requirements_revision,1);assert!(done.text.contains("绿色")&&done.text.contains("三角"),"{}",done.text);
            let personal=core.personal().await;assert_eq!(personal["memories"].as_array().unwrap().len(),1);assert_eq!(personal["memories"][0]["content"],seed["content"]);
            Ok(json!({"in_flight_change":true,"original_turn_id":t.id,"final_turn_id":done.id,"requirements_revision":done.requirements_revision,"old_confirmation_rejected":true,"no_memory_written":true,"receipt":receipt.text,"final_reply":done.text}))
        }else if case=="events" {
            live_send(&bridge,"live-event-plan","请在这条任务完成后，单独跟进一次：实际读取测试工作区 scratch 的 check.json，告诉我里面的状态和版本号。最多等待半小时，只安排这一次后续，不改文件、记忆或其他计划，需要确认就问我。").await;
            let t=live_turn(&bridge,&core,"live-event-plan").await;let pending=live_wait(&core,&t.id).await;
            std::fs::write(root.join("event-proposal.json"),serde_json::to_vec_pretty(&pending).unwrap()).unwrap();
            assert_eq!(pending.state,"awaiting_confirmation","{}",pending.text);let p=pending.pending.as_ref().unwrap();assert_eq!(p.description,"homebase_followup create");assert_eq!(p.arguments["trigger"]["turn_id"],t.id);assert_eq!(p.arguments["trigger"]["on"],"completed");
            live_card(&seen,"触发条件").await;live_send(&bridge,"live-event-confirm",&format!("确认 {}",p.id)).await;
            let done=live_wait(&core,&t.id).await;assert_eq!(done.state,"completed","{}",done.text);assert!(!["请确认","需要你确认","等待你确认"].iter().any(|phrase|done.text.contains(phrase)),"duplicate confirmation after accepted action: {}",done.text);
            let until=tokio::time::Instant::now()+Duration::from_secs(600);
            let follow=loop {let state=core.followups().await;if let Some(id)=state["entries"][0]["runs"][0]["turn_id"].as_str(){break id.to_string();}assert!(tokio::time::Instant::now()<until,"event did not trigger");bridge.deliver_reminders().await;tokio::time::sleep(Duration::from_millis(200)).await;};
            let result=live_wait(&core,&follow).await;assert_eq!(result.state,"completed","{}",result.text);assert!(result.steps.iter().any(|s|s.name=="homebase_files"&&s.result["content"].as_str().is_some_and(|v|v.contains("ready"))),"followup did not read actual fixture");
            live_card(&seen,"ready").await;
            assert_eq!(core.followups().await["entries"][0]["runs"].as_array().unwrap().len(),1);
            Ok(json!({"event_followup":true,"original_turn_id":t.id,"followup_turn_id":result.id,"separate_followup":result.id!=t.id,"final_reply":result.text,"exactly_one_run":true}))
        }else if case=="notifications" {
            live_send(&bridge,"live-watch-file","帮我安排一个测试巡检：一分钟后开始，每隔五分钟实际读取一次测试工作区 scratch 的 check.json，共检查三次。每次都重新读取，不凭历史猜测；告诉我里面的状态和版本号，首次和有变化时通知，没变化就安静保留结果。只新增这个巡检，不修改文件、记忆或其他计划。").await;
            let t=live_turn(&bridge,&core,"live-watch-file").await;let pending=live_wait(&core,&t.id).await;
            assert_eq!(pending.state,"awaiting_confirmation","{}",pending.text);let p=pending.pending.as_ref().unwrap();assert_eq!(p.description,"homebase_followup create");assert_eq!(p.arguments["max_runs"],3);assert_eq!(p.arguments["interval_minutes"],5);assert_ne!(p.arguments["notification_policy"],"all");
            live_card(&seen,"最多执行次数").await;live_send(&bridge,"live-watch-confirm",&format!("确认 {}",p.id)).await;
            let done=live_wait(&core,&t.id).await;assert_eq!(done.state,"completed","{}",done.text);
            let fid=core.followups().await["entries"][0]["id"].as_str().unwrap().to_string();let mut runs=vec![];
            for ordinal in 1..=3 {
                if ordinal==3 {std::fs::write(&reads,"{\"status\":\"ready\",\"version\":2}").unwrap();}
                core.advance_isolated_live_followup(&fid).await.unwrap();
                let until=tokio::time::Instant::now()+Duration::from_secs(600);
                let id=loop {let state=core.followups().await;if let Some(id)=state["entries"][0]["runs"][(ordinal-1)as usize]["turn_id"].as_str(){break id.to_string();}assert!(tokio::time::Instant::now()<until);bridge.deliver_reminders().await;tokio::time::sleep(Duration::from_millis(200)).await;};
                let result=live_wait(&core,&id).await;assert_eq!(result.state,"completed","{}",result.text);assert!(result.steps.iter().any(|s|s.name=="homebase_files"&&s.result["content"].as_str().is_some_and(|c|c.contains("ready"))),"model did not reread actual data");
                let delivery=loop {bridge.deliver_reminders().await;if let Some(record)=bridge.status().await.tasks.iter().find(|r|r.id==format!("hb-proactive-{id}")&&matches!(r.delivery.as_str(),"accepted"|"suppressed")){break record.delivery.clone();}assert!(tokio::time::Instant::now()<until,"proactive result not settled");tokio::time::sleep(Duration::from_millis(200)).await;};
                runs.push(json!({"ordinal":ordinal,"turn_id":id,"delivery":delivery,"reply":result.text,"steps":result.steps}));
                std::fs::write(root.join("notification-runs.json"),serde_json::to_vec_pretty(&runs).unwrap()).unwrap();
                assert_eq!(delivery,if ordinal==2{"suppressed"}else{"accepted"},"unchanged evidence was not quiet");
                if ordinal<3 {loop {if core.followups().await["entries"][0]["state"]=="scheduled"{break;}assert!(tokio::time::Instant::now()<until);tokio::time::sleep(Duration::from_millis(100)).await;}}
            }
            Ok(json!({"notification_filter":true,"real_model_each_run":true,"isolated_schedule_clock_advanced":true,"runs":runs}))
        }else {Err("Unknown live case".into())}
    }.await;
    bridge.stop().await.unwrap();
    server.abort();
    assert_eq!(
        std::fs::read_to_string(sentinel).unwrap(),
        "KEEP ISOLATED ORIGINAL"
    );
    let result = outcome.unwrap();
    let report = json!({"case":case,"passed":true,"root":root,"route_id":core.settings().await.route_id,"real_model_requests":requests.load(Ordering::SeqCst),"fake_wechat_only":true,"result":result});
    std::fs::write(
        root.join("validation.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("LIVE_VALIDATION {}", root.display());
}
