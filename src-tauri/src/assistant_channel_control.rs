//! Write-ahead, source-bound conversational controls. A replay keeps its original target.
use super::*;
#[path = "assistant_control_receipt_retention.rs"]
mod retention;
#[derive(Clone, Serialize, Deserialize)]
pub struct ControlReceipt {
    key: String,
    fingerprint: String,
    action: String,
    session_id: String,
    turn_id: String,
    confirmation: String,
    generation: u64,
    pub(super) response: Option<Value>,
}
impl ControlReceipt {
    fn present_response(&self, mut response: Value) -> Value {
        // Also classify responses stored by older versions, without rewriting receipts.
        if matches!(self.action.as_str(), "确认" | "拒绝") {
            response["action"] = json!("confirmation");
        }
        response
    }
    fn cached_response(&self) -> Option<Value> {
        self.response.clone().map(|response| self.present_response(response))
    }
}
pub(super) fn references(receipts: &[ControlReceipt], session: &str, turns: &[&str]) -> bool {
    receipts.iter().any(|r| retention::protects(r, session, turns))
}
// A generation mismatch is verification of an old control, not another release.
// Its queued count describes current waiting input and must not imply execution.
pub(super) fn queue_response(outcome: &Value) -> Value {
    let text = if outcome["resumed"] == true {
        format!("后续 {} 条要求已接上，我会按顺序处理。你可以问我进度；原来结果还不确定的事没有重新做。", outcome["queued"])
    } else {
        "这条继续请求已核对，没有再次启动后续任务。你可以问我进度，查看原任务的结果。".into()
    };
    json!({"action":"message","text":text})
}
// None means the current session has only an explicitly paused input queue.
// Include unsafe/unknown stopped tasks so a failed resume never falls through
// to releasing unrelated work. Stored receipts bypass this new selection.
pub(super) fn continuation_turn(session: &Session) -> Result<Option<&Turn>, String> {
    if session.turns.iter().any(|turn| active(&turn.state) && turn.state != "waiting_input_queue") {
        return Err("当前对话还有工作正在处理或等待确认；可先问进度，需要放开后续消息请明确说“继续排队”".into());
    }
    let mut choices = session.turns.iter().filter(|turn| turn.interaction != "chat"
        && matches!(turn.state.as_str(), "failed" | "interrupted_unknown" | "paused_budget" | "needs_input" | "unknown"));
    let selected = choices.next();
    if choices.next().is_some() {
        return Err("当前对话有多件暂停的事，请说“继续 原任务编号”；后续队列仍保持暂停".into());
    }
    if let Some(turn) = selected { return Ok(Some(turn)); }
    if session.queue_paused && session.turns.iter().any(|turn| turn.state == "waiting_input_queue") {
        return Ok(None);
    }
    Err("当前对话没有可明确继续的暂停任务；旧任务请使用原编号，需要处理后续消息请说“继续排队”".into())
}
pub(super) const CANCEL_INSTALL: &str = "取消安装";

pub(super) fn repository_cancel_intent(text: &str) -> bool {
    matches!(
        text.trim().trim_end_matches(['。', '！', '!']).trim_end(),
        "取消安装" | "取消这次安装" | "停止安装" | "先别装了" | "别装了"
    )
}

pub fn normalized(text:&str)->&str {
    if repository_cancel_intent(text) {
        return CANCEL_INSTALL;
    }
    match text.trim(){
        "取消"|"停一下"|"先停一下"|"先不要做了"=>"停止",
        "继续吧"|"接着做"=>"继续",
        "确认执行"|"同意执行"=>"确认",
        "进度呢"|"现在到哪了"|"做到哪一步了"|"做到哪一步了？"|"进展怎么样"|"进展怎么样？"|"现在在做什么"=>"进度",
        "把结果发我"=>"结果",
        text=>text,
    }
}
impl Engine {
    pub(super) async fn channel_control(self:&Arc<Self>,source:&str,rid:&str,text:&str)->Result<Option<Value>,String>{
        let text=normalized(text);
        let key=format!("{:x}",Sha256::digest(serde_json::to_vec(&(source,rid)).unwrap()));
        let fingerprint=format!("{:x}",Sha256::digest(text.as_bytes()));
        if let Some(receipt)=self.state.lock().await.channel_controls.iter().find(|r|r.key==key){if receipt.fingerprint!=fingerprint{return Err("同一消息编号对应了不同控制内容，未执行".into());}if let Some(response)=receipt.cached_response(){return Ok(Some(response));}}
        let named={let s=self.state.lock().await;Self::named_matter_target(&s,source,text)?};
        let explicit=text.split_once(' ').filter(|(_,id)|uuid::Uuid::parse_str(id.trim()).is_ok()).or_else(||named.as_ref().map(|(a,t)|(a.as_str(),t.as_str())));
        let action=explicit.map(|(verb,_)|verb).unwrap_or(text);
        if !["确认","拒绝","继续","继续排队","停止","停止助理","停止当前任务","/stop","新对话","开启新对话","/new",CANCEL_INSTALL].contains(&action) && !(named.is_some()&&["进度","结果"].contains(&action)){return Ok(None);}
        if rid.is_empty()||rid.len()>256{return Err("控制消息编号为空或过长".into());}
        let _gate=self.control_gate.lock().await;
        let existing=self.state.lock().await.channel_controls.iter().find(|r|r.key==key).cloned();
        let mut _archive_leases=Vec::new();
        let mut selected_turn=None;
        let mut selected_session=None;
        if let Some(receipt)=&existing {
            if receipt.fingerprint!=fingerprint{return Err("同一消息编号对应了不同控制内容，未执行".into());}
            if let Some(response)=receipt.cached_response(){return Ok(Some(response));}
            if !receipt.turn_id.is_empty(){_archive_leases.extend(self.ensure_archive_turn(&receipt.turn_id,Some(source)).await?);}
            else if receipt.action=="继续排队" {
                let state=self.state.lock().await;
                if !state.sessions.iter().any(|s|s.id==receipt.session_id&&s.source_key==source){return Err("没有找到属于当前来源的原队列会话".into());}
                drop(state);
                _archive_leases.extend(self.ensure_archive_session(&receipt.session_id).await?);
            }
        }else if let Some((_,target))=explicit {
            _archive_leases.extend(self.ensure_archive_turn(target.trim(),Some(source)).await?);
        }else if ["继续","继续排队",CANCEL_INSTALL].contains(&action){
            let sid=self.source_current_session(source).await?;
            _archive_leases.extend(self.ensure_archive_session(&sid).await?);
            selected_session=Some(sid);
        }else if !["新对话","开启新对话","/new","确认","拒绝"].contains(&action){
            if let Some(choice)=self.source_target(source).await? {
                _archive_leases.extend(self.ensure_archive_turn(&choice.turn_id,Some(source)).await?);
                selected_turn=Some(choice);
            }
        }
        let receipt=if let Some(receipt)=existing {
            if receipt.fingerprint!=fingerprint{return Err("同一消息编号对应了不同控制内容，未执行".into());}
            if let Some(response)=receipt.cached_response(){return Ok(Some(response));}
            receipt
        }else{
            let state=self.state.lock().await;
            let new=["新对话","开启新对话","/new"].contains(&action);
            let session=if let Some(id)=&selected_session{state.sessions.iter().find(|s|s.id==*id&&s.source_key==source)}else{state.sessions.iter().rev().find(|s|s.source_key==source)};
            let mut receipt=ControlReceipt{key:key.clone(),fingerprint,action:action.into(),session_id:session.map(|s|s.id.clone()).unwrap_or_default(),turn_id:String::new(),confirmation:String::new(),generation:0,response:None};
            if new {receipt.session_id=id();}
            else {
                let session=session.ok_or(if explicit.is_some(){"不能操作其他来源的任务"}else{"当前还没有助理会话，请先描述任务"})?;
                if action=="继续排队" {
                    let queue_session = if let Some((_, target)) = explicit {
                        state.sessions.iter().find(|s| s.source_key == source && s.turns.iter().any(|t| t.id == target.trim()))
                            .ok_or("没有找到属于当前来源的队列会话")?
                    } else { session };
                    receipt.action="继续排队".into();
                    receipt.session_id=queue_session.id.clone();
                    receipt.generation=queue_session.queue_resume_generation;
                } else if action=="继续" && explicit.is_none() {
                    if let Some(turn) = continuation_turn(session)? {
                        receipt.turn_id=turn.id.clone();
                        receipt.generation=turn.workflow.resume_generation;
                    } else {
                        receipt.action="继续排队".into();
                        receipt.generation=session.queue_resume_generation;
                    }
                }
                else {
                    let turn=if let Some((verb,target))=explicit {
                        state.sessions.iter().filter(|s|s.source_key==source).flat_map(|s|&s.turns).find(|t| {
                            if matches!(verb, "确认" | "拒绝") {
                                t.state == "awaiting_confirmation"
                                    && t.pending.as_ref().is_some_and(|p| !p.approved && p.id == target.trim())
                            } else {
                                t.id == target.trim()
                            }
                        })
                    }else if ["确认","拒绝"].contains(&action){
                        let choices=Self::confirmation_targets(&state,source);
                        if choices.len()!=1{return Err("目前没有唯一待确认操作；未执行任何确认，请核对原操作".into());}choices.first().copied()
                    }else if action == CANCEL_INSTALL {
                        let mut installs = session.turns.iter().filter(|turn| {
                            turn.has_repository_install() && active(&turn.state)
                                && turn.state != "waiting_input_queue"
                        });
                        let install = installs.next();
                        if installs.next().is_some() {
                            return Err("这里有多笔安装，请引用要停的那一笔。".into());
                        }
                        if install.is_none() {
                            return Err("当前对话没有正在处理的安装。要停较早的安装，请引用那一笔的回复。".into());
                        }
                        install
                    }else{
                        selected_turn.as_ref().and_then(|choice|state.sessions.iter().find(|s|s.source_key==source&&s.id==choice.session_id).and_then(|s|s.turns.iter().find(|t|t.id==choice.turn_id)))
                    }.ok_or(if explicit.is_some() && matches!(action,"确认"|"拒绝") {
                        "这份确认已失效、已处理或不属于当前会话。请查看最新的确认提示。"
                    } else { "没有找到属于当前来源的目标任务" })?;
                    receipt.turn_id=turn.id.clone();receipt.generation=turn.workflow.resume_generation;
                    if ["确认","拒绝"].contains(&action){receipt.confirmation=turn.pending.as_ref().ok_or("没有待确认内容")?.id.clone();}
                    receipt.session_id=state.sessions.iter().find(|s|s.turns.iter().any(|t|t.id==turn.id)).unwrap().id.clone();
                }
            }
            drop(state);
            self.mutate(Change::default(),|s|{
                // Never evict a receipt and silently re-authorize a stale message.
                if s.channel_controls.len()>=4096{return Err("控制消息记录已达安全上限，请从桌面操作；未重复执行".into());}
                s.channel_controls.push(receipt.clone());Ok(())
            }).await?;receipt
        };
        let response=match receipt.action.as_str(){
            "新对话"|"开启新对话"|"/new"=>{
                let _submission = self.send_gate.lock().await;
                let session=self.make_session_fixed("wechat",source,&receipt.session_id).await?;
                json!({"action":"message","text":"已开启新对话。长期记忆与配置仍然共用，原任务不会被取消。","session_id":session.id})
            },
            "确认"|"拒绝"=>{
                json!(self.confirm_once(&receipt.turn_id, &receipt.confirmation,
                    receipt.action == "确认", Some(&receipt.session_id)).await?)
            },
            "继续"=>json!(self.resume_once(&receipt.turn_id,Some(receipt.generation)).await?),
            CANCEL_INSTALL => {
                let text = self.cancel_repository_install(
                    source, &receipt.session_id, &receipt.turn_id, &receipt.key,
                ).await?;
                // Keep the watcher for the original installation and its actual outcome.
                json!({"action":"message","text":text})
            },
            "进度"|"结果"=>{let files=receipt.action=="结果";let view=self.channel_view(source,&receipt.turn_id,files).await?;json!({"action":"view","turn_id":receipt.turn_id,"view":view,"files":files})},
            "继续排队"=>{
                let count=self.resume_queue_once(&receipt.session_id,Some(receipt.generation)).await?;
                queue_response(&count)
            },
            _=>{self.cancel(&receipt.turn_id).await?;json!({"action":"message","text":"这件事我先停下了，已经产生的结果会保留。稍后问我进度，就能核对还在处理的工作。","turn_id":receipt.turn_id})},
        };
        self.mutate(Change::default(),|s|{s.channel_controls.iter_mut().find(|r|r.key==key).ok_or("控制记录不存在")?.response=Some(response.clone());Ok(())}).await?;
        Ok(Some(receipt.present_response(response)))
    }
}
