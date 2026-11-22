//! Provider-owned native history with a bounded desktop projection and encrypted receipts.
use super::conversations::{NativeAction,NativeRequest};
use serde::{Serialize,Deserialize};use serde_json::{json,Value};use sha2::{Digest,Sha256};
use std::{collections::BTreeMap,path::PathBuf,sync::Arc,process::Stdio};
use tokio::{io::{AsyncBufReadExt,AsyncWriteExt,BufReader},sync::{Mutex,OnceCell,watch},process::Command};

#[derive(Default,Serialize,Deserialize)]struct Store { records:BTreeMap<String,Record>,receipts:BTreeMap<String,Value> }
#[derive(Serialize,Deserialize)]struct Record { id:String,provider:String,native_id:Option<String>,cwd:PathBuf,linked:String,credential_ref:Option<String>,#[serde(default)]updated_at:String,turns:Vec<Value> }
struct Service { state:Mutex<Store>,active:Mutex<BTreeMap<String,(String,watch::Sender<bool>)>> }
static SERVICE:OnceCell<Arc<Service>>=OnceCell::const_new();
fn root()->PathBuf{crate::adapters::homebase_home_dir().join(".homebase/conversations")}
fn key(provider:&str,id:&str)->String{format!("{provider}:{id}")}
fn file()->PathBuf{root().join("providers.dpapi")}
/// Loading this state initializes no SDK process and writes no files.
pub(crate) async fn send_receipt(request:&NativeRequest,provider:&str)->Result<Option<Value>,String>{
    if !["claude-code","gemini-cli"].contains(&provider){return Err("原生Agent未登记".into());}
    let s=service().await?;
    let state=s.state.lock().await;
    let id=request.request_id.as_deref().ok_or("缺少原请求编号")?;
    let Some(previous)=state.receipts.get(&key(provider,id))else{return Ok(None)};
    let fingerprint=format!("{:x}",Sha256::digest(serde_json::to_vec(request).map_err(|_|"原请求编码失败")?));
    super::conversations::checked_receipt(previous,&fingerprint,request.thread_id.as_deref().ok_or("缺少原会话编号")?)
}
async fn service()->Result<&'static Arc<Service>,String>{SERVICE.get_or_try_init(||async{
    let mut store:Store=match std::fs::read(file()){
        Ok(v)=>{if v.len()>32*1024*1024{return Err("原生会话投影达到维护上限；原数据未清空".into());}let plain=crate::wechat_crypto::crypt(&v,false)?;serde_json::from_slice(&plain).map_err(|_|"原生会话状态不可解析，原数据未覆盖")?},
        Err(e)if e.kind()==std::io::ErrorKind::NotFound=>Store::default(),Err(_)=>return Err("原生会话状态不可读".into())
    };
    // Observation after restart, never resume/rewrite existing tasks on a query.
    for record in store.records.values_mut(){for turn in &mut record.turns{if turn["status"]=="inProgress"{turn["status"]=json!("unknown");turn["error"]=json!({"message":"原生运行已断开，结果未知；未自动恢复或重发"});}}}
    Ok(Arc::new(Service{state:Mutex::new(store),active:Mutex::new(BTreeMap::new())}))
}).await}
async fn save(store:&Store)->Result<(),String>{
    std::fs::create_dir_all(root()).map_err(|_|"原生目录不可创建")?;
    let plain=zeroize::Zeroizing::new(serde_json::to_vec(store).map_err(|_|"原生状态编码失败")?);
    if plain.len()>32*1024*1024{return Err("原生会话投影达到维护上限，未删除旧数据".into());}
    let cipher=crate::wechat_crypto::crypt(&plain,true)?;crate::control_protocol::publish_private(&file(),&cipher).await.map_err(|_|"原生状态保存失败，未继续执行".into())
}
pub fn runtime_root()->Option<PathBuf>{
    std::env::current_exe().ok().and_then(|p|p.parent().map(|p|p.join("distribution/native-agent-runtime"))).filter(|p|p.join("worker.mjs").is_file()).or_else(||{
        let p=PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent()?.join("native-agent-runtime");p.join("worker.mjs").is_file().then_some(p)
    })
}
pub fn claude_binary()->Option<PathBuf>{
    let home=crate::adapters::homebase_home_dir();
    // Test profiles still use the real executable, but never its real profile/auth.
    let real=dirs::home_dir().map(|p|p.join(".local/bin/claude.exe"));
    for p in std::iter::once(home.join(".local/bin/claude.exe")).chain(real){if p.is_file(){return Some(p);}}
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {let p=dir.join(if cfg!(windows){"claude.exe"}else{"claude"});if p.is_file(){return Some(p);}}
    None
}
pub fn available(provider:&str)->bool{runtime_root().is_some_and(|p|match provider{"claude-code"=>claude_binary().is_some(),"gemini-cli"=>p.join("node_modules/@google/gemini-cli/bundle/gemini.js").is_file(),_=>false})}
pub fn shutdown(){if let Some(s)=SERVICE.get(){if let Ok(active)=s.active.try_lock(){for (_,signal) in active.values(){let _=signal.send(true);}}}}

fn snapshot(record:&Record)->Value{
    let turn=record.turns.last();let state=turn.map(|t|t["status"].clone()).unwrap_or(json!("idle"));
    let mut items=serde_json::Map::new();let mut order=vec![];
    for t in record.turns.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev(){for item in t["items"].as_array().into_iter().flatten(){if let Some(id)=item["id"].as_str(){order.push(json!(id));items.insert(id.into(),item.clone());}}}
    json!({"id":record.id,"provider":record.provider,"native_id":record.native_id,"state":state,"item_order":order,"items":items,"active_turn_id":turn.filter(|t|t["status"]=="inProgress").map(|t|t["id"].clone()),"error":turn.map(|t|t["error"].clone())})
}

pub async fn handle(request:NativeRequest,provider:&str)->Result<Value,String>{
    if !["claude-code","gemini-cli"].contains(&provider){return Err("原生 Agent 未登记".into());}
    let s=service().await?;let tid=request.thread_id.as_deref();
    if tid.is_some_and(|id|id.is_empty()||id.len()>120||!id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b))){return Err("原生会话编号无效".into());}
    let mut state=s.state.lock().await;
    if matches!(request.action,NativeAction::List){return Ok(json!({"data":state.records.values().filter(|r|r.provider==provider).map(|r|json!({"id":r.id,"provider":r.provider,"native_id":r.native_id,"cwd":r.cwd,"updatedAt":r.updated_at,"preview":r.turns.first().and_then(|t|t["prompt"].as_str()).unwrap_or("新会话"),"assistant_session_id":r.linked})).collect::<Vec<_>>(),"nextCursor":null}));}
    if matches!(request.action,NativeAction::Status){return Ok(state.records.get(&key(provider,tid.ok_or("缺少会话编号")?)).map(snapshot).unwrap_or(json!({"id":tid,"state":"not_loaded","message":"尚未由 HomeBase 接续此原生会话；未自动执行"})));}
    if matches!(request.action,NativeAction::Read){
        let id=tid.ok_or("缺少会话编号")?;
        if let Some(r)=state.records.get(&key(provider,id)){
            let end=request.cursor.as_deref().unwrap_or("").parse::<usize>().unwrap_or(r.turns.len());if end>r.turns.len(){return Err("会话分页已失效".into());}let start=end.saturating_sub(20);
            return Ok(json!({"thread":{"id":r.id,"provider":r.provider,"native_id":r.native_id,"cwd":r.cwd,"turns":r.turns[start..end]},"assistant_session_id":r.linked,"next_cursor":(start>0).then(||start.to_string())}));
        }
        drop(state);let history=super::agents_history::agents_history_read(provider.into(),id.into()).await?;
        return Ok(json!({"thread":{"id":id,"provider":provider,"cwd":history.cwd,"turns":[{"id":"historical","status":"completed","items":history.messages.iter().map(|m|json!({"id":m.uuid,"type":if m.role=="user"{"userMessage"}else{"agentMessage"},"text":m.content})).collect::<Vec<_>>() }]},"next_cursor":null}));
    }
    let request_id=request.request_id.as_deref().filter(|id|!id.is_empty()&&id.len()<=120).ok_or("执行需要稳定 request_id")?;
    let receipt=key(provider,request_id);let fingerprint=format!("{:x}",Sha256::digest(serde_json::to_vec(&request).map_err(|_|"请求编码失败")?));
    if let Some(old)=state.receipts.get(&receipt){if old["fingerprint"]!=fingerprint{return Err("请求编号已用于其他内容，未执行".into());}return Ok(old["result"].clone());}
    if state.receipts.len()>=2000{return Err("原生回执达到维护上限，未删除或重跑".into());}
    if matches!(request.action,NativeAction::Create){
        if !available(provider){return Err("原生运行包或程序尚未安装完整".into());}
        if request.credential_ref.as_deref().is_some_and(|r|!crate::assistant_local::credential_reference(r)){return Err("只能保存已有凭据的引用，不能填写密钥正文".into());}
        state.receipts.insert(receipt.clone(),json!({"fingerprint":fingerprint,"result":{"state":"unknown","message":"新建受理结果未知，未自动重建"}}));save(&state).await?;
        let id=uuid::Uuid::new_v4().to_string();let cwd=root().join("workspaces").join(&id);std::fs::create_dir_all(&cwd).map_err(|_|"工作区不可创建")?;
        let session=super::assistant::service().await?.create_session().await?;
        super::conversations::bind(provider,&id,&session.id).await?;
        let record=Record{id:id.clone(),provider:provider.into(),native_id:None,cwd:cwd.clone(),linked:session.id.clone(),credential_ref:request.credential_ref.clone(),updated_at:chrono::Utc::now().to_rfc3339(),turns:vec![]};
        state.records.insert(key(provider,&id),record);
        let result=json!({"state":"prepared","thread":{"id":id,"provider":provider,"native_id":null,"cwd":cwd,"turns":[]},"assistant_session_id":session.id});
        state.receipts.insert(receipt,json!({"fingerprint":fingerprint,"result":result}));save(&state).await?;return Ok(result);
    }
    let id=tid.ok_or("缺少会话编号")?.to_string();let record_key=key(provider,&id);
    if matches!(request.action,NativeAction::Interrupt){
        let active=s.active.lock().await;
        let (_,signal)=active.get(&record_key).ok_or("没有已知的在途原生任务，未猜测其他任务")?;let _=signal.send(true);
        let result=json!({"state":"interrupt_requested","thread_id":id});state.receipts.insert(receipt,json!({"fingerprint":fingerprint,"result":result}));save(&state).await?;return Ok(result);
    }
    if !matches!(request.action,NativeAction::Send){return Err("此原生动作不支持".into());}
    let text=request.text.as_deref().filter(|t|!t.trim().is_empty()&&t.len()<=8000).ok_or("消息须为1–8000字节")?.to_string();
    if !state.records.contains_key(&record_key){
        // Adopt only a known native history with its real working directory. Never guess cwd/latest.
        let history=super::agents_history::agents_history_read(provider.into(),id.clone()).await?;
        let cwd=history.cwd.map(PathBuf::from).filter(|p|p.is_absolute()&&p.is_dir()).ok_or("旧记录没有可恢复的原生会话工作目录；请新建并显式交接，未把日志摘录当续接")?;
        if provider=="gemini-cli" {return Err("全局旧版 Gemini 日志不具备当前原生检查点；请新建独立原生会话并交接，原日志保留".into());}
        let session=super::assistant::service().await?.create_session().await?;super::conversations::bind(provider,&id,&session.id).await?;
        state.records.insert(record_key.clone(),Record{id:id.clone(),provider:provider.into(),native_id:Some(id.clone()),cwd,linked:session.id,credential_ref:None,updated_at:chrono::Utc::now().to_rfc3339(),turns:vec![]});
    }
    let record=state.records.get(&record_key).unwrap();
    if record.turns.last().is_some_and(|t|t["status"]=="inProgress"){return Err("此原生会话正在处理；未重复提交".into());}
    if s.active.lock().await.len()>=4{return Err("原生并发达到上限，未受理本条".into());}
    let linked=record.linked.clone();let cwd=record.cwd.clone();let native_id=record.native_id.clone();let credential_ref=record.credential_ref.clone();
    let turn_id=uuid::Uuid::new_v4().to_string();let run_root=root().join("runs").join(&turn_id);
    let context=super::assistant::service().await?.context(Some(&linked),&text).await?;
    let runtime=runtime_root().ok_or("原生运行包不可用")?;
    let mut command=Command::new(node_binary());command.arg(runtime.join("worker.mjs")).current_dir(&cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)] command.creation_flags(0x08000000);
    let test=std::env::var_os("HOMEBASE_TEST_HOME").is_some();
    if test {
        command.env_clear();for name in ["SystemRoot","WINDIR","COMSPEC","PATH","PATHEXT"]{if let Some(v)=std::env::var_os(name){command.env(name,v);}}
        let home=crate::adapters::homebase_home_dir();command.env("HOME",&home).env("USERPROFILE",&home).env("APPDATA",home.join("AppData/Roaming")).env("LOCALAPPDATA",home.join("AppData/Local")).env("TEMP",&home).env("TMP",&home).env("CLAUDE_CONFIG_DIR",home.join(".claude"));
        // Only explicit synthetic loopback fixtures may supply model credentials in a test profile.
        for name in ["ANTHROPIC_API_KEY","ANTHROPIC_BASE_URL","ANTHROPIC_MODEL","GEMINI_API_KEY","GOOGLE_GEMINI_BASE_URL"]{if let Some(v)=std::env::var_os(name){
            if (name.ends_with("URL")&&!v.to_string_lossy().starts_with("http://127.0.0.1:"))||(!name.ends_with("URL")&&!v.to_string_lossy().starts_with("homebase-fixture-")){return Err("测试原生客户端拒绝非合成模型环境".into());}command.env(name,v);
        }}
        if credential_ref.is_some(){return Err("隔离测试不解析真实凭据引用".into());}
    }else if let Some(reference)=credential_ref {
        let secret=if let Some(name)=reference.strip_prefix("sops:"){crate::adapters::sops::SopsAdapter::for_vault(&crate::secret_policy::vault_path()).decrypt_key(&crate::secret_policy::vault_path(),name).await.map_err(|_|"所选凭据引用不可用或已到期，未启动模型")?}
        else if let Some(name)=reference.strip_prefix("api_key_manager:"){crate::adapters::service_credentials::resolve_selected_api_key(name,true).await?}else{return Err("凭据引用无效".into());};
        command.env(if provider=="gemini-cli"{"GEMINI_API_KEY"}else{"ANTHROPIC_API_KEY"},secret.as_str());
    }
    // New Gemini owns its profile; it never migrates/writes the user's global 0.1.7 settings/auth.
    command.env("GEMINI_CLI_HOME",root().join("gemini-profile").join(&id));
    let start=json!({"type":"start","protocol":1,"provider":provider,"session_id":id,"native_id":if provider=="claude-code"{Some(native_id.clone().unwrap_or(id.clone()))}else{native_id.clone()},"resume":native_id.is_some(),"cwd":cwd,"run_root":run_root,"turn_id":turn_id,"prompt":text,"context":context,"test":test,"claude_binary":claude_binary()});
    state.receipts.insert(receipt.clone(),json!({"fingerprint":fingerprint,"result":{"state":"unknown","thread_id":id,"message":"原生受理结果未知，查询原会话，不重发"}}));
    let record=state.records.get_mut(&record_key).unwrap();record.updated_at=chrono::Utc::now().to_rfc3339();
    record.turns.push(json!({"id":turn_id,"status":"inProgress","prompt":text,"items":[],"error":null}));
    if save(&state).await.is_err(){
        let turn=state.records.get_mut(&record_key).unwrap().turns.last_mut().unwrap();turn["status"]=json!("failed");turn["error"]=json!({"message":"受理回执未能保存，未启动原生进程"});
        return Err("原生回执保存失败，未启动；查询原会话，不自动重发".into());
    }
    let mut child=match command.spawn(){Ok(child)=>child,Err(_)=>{
        let turn=state.records.get_mut(&record_key).unwrap().turns.last_mut().unwrap();
        turn["status"]=json!("failed");turn["error"]=json!({"message":"原生传输未能启动；未发起模型调用"});save(&state).await?;
        return Err("原生传输未能启动，原会话可查；未自动重发".into());
    }};
    let mut stdin=child.stdin.take().ok_or("原生输入管道缺失")?;let stdout=child.stdout.take().ok_or("原生输出管道缺失")?;
    if stdin.write_all(format!("{start}\n").as_bytes()).await.is_err(){
        let _=child.kill().await;let turn=state.records.get_mut(&record_key).unwrap().turns.last_mut().unwrap();
        turn["status"]=json!("unknown");turn["error"]=json!({"message":"原生输入中断，受理结果未知；未重发"});save(&state).await?;
        return Err("原生输入失败，结果未知；查询原会话".into());
    }
    let result=json!({"state":"accepted","thread_id":id,"turn":{"id":turn_id,"status":"inProgress"},"assistant_session_id":linked});
    state.receipts.insert(receipt,json!({"fingerprint":fingerprint,"result":result}));
    if save(&state).await.is_err(){
        let turn=state.records.get_mut(&record_key).unwrap().turns.last_mut().unwrap();turn["status"]=json!("unknown");turn["error"]=json!({"message":"原生输入已提交，但受理回执保存失败；未重发"});
        let _=stdin.write_all(b"{\"type\":\"cancel\"}\n").await;if tokio::time::timeout(std::time::Duration::from_secs(3),child.wait()).await.is_err(){let _=child.kill().await;}
        return Err("原生受理结果未能保存；请查询原会话，不重新提交".into());
    }
    let(tx,mut cancel)=watch::channel(false);s.active.lock().await.insert(record_key.clone(),(turn_id.clone(),tx));drop(state);
    let service=s.clone();let provider=provider.to_string();
    tokio::spawn(async move{
        let mut lines=BufReader::new(stdout).lines();let mut terminal=false;
        loop {tokio::select!{
            _=cancel.changed()=>{let _=stdin.write_all(b"{\"type\":\"cancel\"}\n").await;if tokio::time::timeout(std::time::Duration::from_secs(3),child.wait()).await.is_err(){let _=child.kill().await;}break;},
            line=lines.next_line()=>{
                let Ok(Some(line))=line else{break};if line.len()>8*1024*1024{break;}let Ok(event)=serde_json::from_str::<Value>(&line)else{continue};
                if event["event"]=="tool_request"{
                    let response=super::conversations::gateway(json!({"provider":provider,"threadId":id,"turnId":turn_id,"tool":"homebase","arguments":event["arguments"]})).await;
                    let reply=match response{Ok(v)=>json!({"type":"tool_result","id":event["id"],"result":v}),Err(_)=>json!({"type":"tool_result","id":event["id"],"error":"HomeBase拒绝了此操作；请检查当前会话、授权及参数，未绕过权限"})};
                    if stdin.write_all(format!("{reply}\n").as_bytes()).await.is_err(){break;}continue;
                }
                let mut store=service.state.lock().await;let Some(record)=store.records.get_mut(&record_key)else{break};
                if event["event"]=="native_session" {
                    if let Some(id)=event["id"].as_str(){record.native_id=Some(id.into());}
                    if save(&store).await.is_err(){
                        if let Some(turn)=store.records.get_mut(&record_key).and_then(|r|r.turns.last_mut()).filter(|t|t["id"]==turn_id){turn["status"]=json!("unknown");turn["error"]=json!({"message":"原生会话编号未能保存，已停止等待；请核对原会话，未自动重发"});}
                        let _=stdin.write_all(b"{\"type\":\"cancel\"}\n").await;terminal=true;break;
                    }
                    continue;
                }
                let Some(turn)=record.turns.last_mut().filter(|t|t["id"]==turn_id)else{break};
                match event["event"].as_str().unwrap_or(""){
                    "item"=>{let items=turn["items"].as_array_mut().unwrap();if let Some(old)=items.iter_mut().find(|i|i["id"]==event["item"]["id"]){*old=event["item"].clone();}else if items.len()<128{items.push(event["item"].clone());}},
                    "delta"=>{let items=turn["items"].as_array_mut().unwrap();if let Some(old)=items.iter_mut().find(|i|i["id"]==event["id"]){let text=format!("{}{}",old["text"].as_str().unwrap_or(""),event["text"].as_str().unwrap_or(""));old["text"]=json!(text);}else if items.len()<128{items.push(json!({"id":event["id"],"type":"agentMessage","text":event["text"]}));}},
                    "terminal"=>{
                        turn["status"]=event["state"].clone();turn["error"]=event["error"].clone();
                        if save(&store).await.is_err(){
                            let turn=store.records.get_mut(&record_key).unwrap().turns.last_mut().unwrap();
                            turn["status"]=json!("unknown");turn["error"]=json!({"message":"已收到原生结果，但回执保存失败；不能确认可恢复状态，请核对原会话，未重做"});
                        }
                        terminal=true;break;
                    },_=>{}
                }
            }
        }}
        if !terminal || tokio::time::timeout(std::time::Duration::from_secs(5),child.wait()).await.is_err(){let _=child.kill().await;}
        if !terminal {let mut store=service.state.lock().await;if let Some(record)=store.records.get_mut(&record_key){if let Some(turn)=record.turns.last_mut().filter(|t|t["id"]==turn_id){turn["status"]=json!(if *cancel.borrow(){"interrupted"}else{"unknown"});turn["error"]=json!({"message":"原生连接已结束；已受理的共享任务仍需按原编号查询，不会自动重做"});let _=save(&store).await;}}}
        // A completed SDK may still be closing when the next turn starts. Its cleanup
        // must never drop the next turn's cancellation sender or concurrency slot.
        let mut active=service.active.lock().await;
        if active.get(&record_key).is_some_and(|(generation,_)|generation==&turn_id){active.remove(&record_key);}
    });
    Ok(result)
}
fn node_binary()->PathBuf{std::env::current_exe().ok().and_then(|p|p.parent().map(|p|p.join("distribution/agent-runtime/node.exe"))).filter(|p|p.is_file()).unwrap_or_else(||PathBuf::from("node"))}
