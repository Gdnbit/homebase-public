// Official graph + real local file host; synthetic planner and encrypted loopback WeChat only.
use super::*;
#[tokio::test]
#[ignore = "explicit isolated runtime lab required"]
async fn writing_workflow_skill_files_delivery_restart_and_duplicate_message() {
    const REQUEST: &str = "帮我按照之前的方式完成一下例会纪要。";
    const PREFERENCE: &str = "先写结论，再写待办，保存为 Markdown。";
    const UNRELATED: &str = "fixture-private-billing";
    const MINUTES: &str = "# 例会纪要\n\n## 结论\n使用隔离夹具核对流程。\n\n## 待办\n- 完成模拟验收。";
    let parent=PathBuf::from(std::env::var_os("HOMEBASE_WORKFLOW_LAB").expect("isolated lab"));
    assert!(parent.to_string_lossy().replace('\\',"/").starts_with("F:/HomeBase-Labs/"));
    let root=parent.join(format!("wechat-writing-{}",uuid::Uuid::new_v4()));let workspace=root.join("workspace");let scratch=workspace.join("agent");
    std::fs::create_dir_all(&scratch).unwrap();std::fs::write(root.join("outside.txt"),"PRESERVE_SENTINEL").unwrap();
    let runtime=PathBuf::from(std::env::var_os("HOMEBASE_LOCAL_RUNTIME").expect("runtime"));
    let host=crate::assistant_local_host::LocalHost::start(&runtime,&root.join("profile")).await.unwrap();
    let context=json!({"workspaces":[{"id":"scratch","path":scratch,"access":"write"}],"default_workspace":"scratch","output_root":workspace.join("local-results")});
    let effects=Arc::new(AtomicUsize::new(0));let count=effects.clone();let runner_host=host.clone();
    let runner:Runner=Arc::new(move|command,args|{let host=runner_host.clone();let context=context.clone();let count=count.clone();Box::pin(async move{
        if command!="assistant_local_execute"{return Err(ControlError::new("FIXTURE_BLOCKED","Only isolated file operations"));}
        assert_eq!(args["operation"],"files");count.fetch_add(1,Ordering::SeqCst);
        host.execute(args["operationId"].as_str().unwrap(),"files",context,args["request"].clone()).await.map_err(|e|ControlError::new("LOCAL_FIXTURE",&e))
    })});
    std::fs::create_dir_all(root.join("control")).unwrap();let controller=Controller::new(serde_json::from_str(include_str!("../../control/commands.json")).unwrap(),&root.join("control"),runner).unwrap();
    let control:Control=Arc::new(move|r|{let c=controller.clone();Box::pin(async move{c.handle(r).await})});
    let rounds=Arc::new(AtomicUsize::new(0));let n=rounds.clone();
    let model:crate::assistant::Model=Arc::new(move|request|{let n=n.fetch_add(1,Ordering::SeqCst);Box::pin(async move{
        let (name,args)=match n{
            0=>{
                let prompt = serde_json::to_string(&request.messages).unwrap();
                assert!(prompt.contains(REQUEST));
                assert!(prompt.contains(PREFERENCE));
                assert!(!prompt.contains(UNRELATED));
                ("homebase_skill",json!({"action":"read","id":"personal-workflows"}))
            },
            1=>{assert!(serde_json::to_string(&request.messages).unwrap().contains("inbox_submit"));("homebase_files",json!({"action":"write","path":"minutes.md","content":MINUTES}))},
            2=>("homebase_files",json!({"action":"export","path":"minutes.md"})),
            3=>{let data:Value=serde_json::from_str(request.messages.iter().rev().find(|m|m["role"]=="tool").unwrap()["content"].as_str().unwrap()).unwrap();("homebase_deliver",json!({"job_id":data["job_id"]}))},
            4=>return crate::assistant_protocol::parse("chat",&json!({"choices":[{"message":{"content":"草稿写好了，文件已安排返回；可以在桌面预览。"}}]})),
            _=>return Err("Unexpected repeated planner call".into())
        };
        crate::assistant_protocol::parse("chat",&json!({"choices":[{"message":{"tool_calls":[{"id":format!("writing-{n}"),"function":{"name":name,"arguments":args.to_string()}}]}}]}))
    })});
    let mut state=crate::assistant::State::default();state.settings.enabled=true;state.settings.runtime="pi".into();state.settings.route_id="synthetic-only".into();
    for (revision, (title, content)) in [("例会纪要", PREFERENCE), ("账单归档", "fixture-private-billing 按照之前的方式完成一下")].into_iter().enumerate() {
        state.personal.save_memory(crate::assistant_personal::MemoryInput {
            revision: revision as u64,
            id: None,
            title: title.into(),
            content: content.into(),
            expires_at: None,
        }, chrono::Utc::now()).unwrap();
    }
    let saved=Arc::new(Mutex::new(state.clone()));let memory=saved.clone();let save:crate::assistant::Save=Arc::new(move|s|{let memory=memory.clone();Box::pin(async move{*memory.lock().await=s;Ok(())})});
    let core=crate::assistant::Engine::new(state,save.clone(),model.clone(),control.clone(),workspace.clone()).await.unwrap();let engine=core.clone();
    let(base,seen,server)=fake_api(200,json!({"__media":true})).await;let mut api=Api::new().unwrap();api.test_origin=Some(base.clone());
    let bridge=Bridge::new_with_agent(account(&base),api,Arc::new(|_|Box::pin(async{Ok(())})),control.clone(),workspace.clone(),Some(Arc::new(move|r|{let e=engine.clone();Box::pin(async move{e.channel(r).await})})));
    bridge.start().await.unwrap();let input=msg("writing-natural",REQUEST);bridge.ingest(&input).await.unwrap();bridge.ingest(&input).await.unwrap();
    let tid=bridge.status().await.tasks[0].job_id.clone().unwrap();
    let turn=tokio::time::timeout(Duration::from_secs(90),async{loop{let t=core.turn(&tid).await.unwrap();if !["queued","running","waiting_jobs"].contains(&t.state.as_str()){break t;}tokio::time::sleep(Duration::from_millis(100)).await;}}).await.unwrap();
    assert_eq!(turn.state,"completed","{}",turn.text);assert_eq!(turn.artifacts.len(),1);assert_eq!(effects.load(Ordering::SeqCst),2);
    let a=&turn.artifacts[0];assert_eq!(std::fs::read_to_string(&a.path).unwrap(),MINUTES);assert_eq!(core.artifact(&a.id).await.unwrap().sha256,a.sha256);
    bridge.ingest(&msg("writing-status", "刚才那件事做到哪了？")).await.unwrap();
    bridge.ingest(&msg("writing-result", "帮我把刚才的结果发过来。")).await.unwrap();
    {
        let state = bridge.state.lock().await;
        let records = &state.stored.records;
        assert_eq!(records[records.len() - 2].query(), Some((QueryTarget::Turn(&tid), false)));
        assert_eq!(records[records.len() - 1].query(), Some((QueryTarget::Turn(&tid), true)));
    }
    assert!(seen.lock().await.iter().any(|v|v["body"]["msg"]["item_list"][0]["type"]==4));
    let restored=crate::assistant::Engine::new(saved.lock().await.clone(),save,model,control,workspace).await.unwrap();assert_eq!(restored.turn(&tid).await.unwrap().artifacts[0].sha256,a.sha256);
    assert_eq!(std::fs::read_to_string(root.join("outside.txt")).unwrap(),"PRESERVE_SENTINEL");assert_eq!(rounds.load(Ordering::SeqCst),5);assert_eq!(effects.load(Ordering::SeqCst),2);
    // Tampering must be explicit failure, never a stale preview or regenerated result.
    std::fs::write(&a.path,"changed fixture").unwrap();assert!(core.artifact(&a.id).await.is_err());
    bridge.stop().await.unwrap();server.abort();
    std::fs::write(root.join("validation.json"),serde_json::to_vec_pretty(&json!({"passed":true,"real_model_calls":0,"real_wechat_messages":0,"actual_file_operations":2,"planner_rounds":5,"desktop_same_artifact":true,"restart_preserved":true,"duplicate_message_deduplicated":true,"tampered_result_rejected":true})).unwrap()).unwrap();
    println!("Writing workflow receipt: {}",root.join("validation.json").display());
}
