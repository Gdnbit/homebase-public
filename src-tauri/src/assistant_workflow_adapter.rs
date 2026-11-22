//! Shared task operations. No external effects, credentials or authority expansion.
use super::*;
pub(super) struct CompletionGate {
    pub feedback: Option<String>,
    pub stop: Option<workflow::StopInfo>,
}
impl Engine {
    pub(super) async fn task_tool(&self,tid:&str,args:&Value)->Result<Value,String>{
        let action=args["action"].as_str().ok_or("缺少计划动作")?;
        if action!="get" {
            let revision=args["revision"].as_u64().ok_or("缺少计划版本，请先get")?;
            self.change_turn(tid,|s,t|{
                if s.turns[t].workflow.version==0 {return Err("旧轮次保留原工具集；请在新对话建立任务计划".into());}
                if action=="set" {
                    let goal=args["goal"].as_str().ok_or("缺少任务目标")?.to_string();
                    let items=serde_json::from_value(args["items"].clone()).map_err(|_|"阶段格式无效；需要id/title/done_when/kind")?;
                    s.turns[t].workflow.set_plan(revision,goal,items)?;
                }else if action=="update" {
                    let evidence=serde_json::from_value(args.get("evidence").cloned().unwrap_or(json!([]))).map_err(|_|"证据须为本轮step编号列表")?;
                    let steps=s.turns[t].steps.clone();
                    s.turns[t].workflow.update(revision,arg_string(args,"item_id")?,arg_string(args,"state")?,args["note"].as_str().unwrap_or("").into(),evidence,&steps,&s.jobs)?;
                }else{return Err("计划动作不受支持".into());}
                Ok(())
            }).await?;
        }
        let state=self.state.lock().await;let(s,t)=locate(&state,tid)?;let turn=&state.sessions[s].turns[t];
        let projection=turn.workflow.view(turn,&state.sessions[s].jobs,chrono::Utc::now());
        let plan=projection.as_ref().and_then(|view|view.plan.as_ref());
        let recent=turn.steps.iter().filter(|s|s.name!="homebase_task").rev().take(16).map(|s|json!({"step_id":s.id,"tool":s.name,"action":s.arguments["request"]["action"],"state":s.state,"job_id":s.job_id})).collect::<Vec<_>>();
        Ok(json!({"status":"succeeded","turn_id":turn.id,"session_id":state.sessions[s].id,"revision":plan.map(|p|p.revision).unwrap_or(0),"plan":plan,"limits":projection.as_ref().map(|v|&v.limits),"recent_steps":recent,"notice":"set/update必填当前revision；尚无计划时revision为0。evidence须原样复制本工具或实际执行结果的step_id字符串，不能填说明文字、文件名或sha256。计划不是授权；写入、执行、附件登记和交付不能用普通读取代替。"}))
    }
    pub(super) async fn task_error(&self,tid:&str,args:&Value,message:String)->Result<Value,String>{
        let mut result=self.task_tool(tid,&json!({"action":"get"})).await?;
        result["status"]=json!("failed");result["code"]=json!("PLAN_INVALID");result["message"]=json!(message);
        result["next_action"]=json!("使用此回复中的revision修正原计划调用；evidence从recent_steps中选对应操作且state为completed的step_id并原样复制。不要为修正计划重做已经成功的写入或计算。write用write/edit等步骤，execute用terminal，artifact用export，deliver用deliver。");
        let state=self.state.lock().await;let(s,t)=locate(&state,tid)?;let turn=&state.sessions[s].turns[t];
        if let Some(item_id)=args["item_id"].as_str(){
            result["valid_evidence_step_ids"]=json!(turn.workflow.candidates(item_id,&turn.steps,&state.sessions[s].jobs));
            if let Some(item)=turn.workflow.plan.as_ref().and_then(|p|p.items.iter().find(|i|i.input.id==item_id)){
                result["required_kind"]=json!(item.input.kind);
                result["required_evidence"]=json!(match item.input.kind {
                    workflow::EvidenceKind::Execute=>"execute专指homebase_terminal真实执行且exit_code=0；仅read/write不能证明执行了命令。已有输出不得覆盖，可用只读终端核对输入、计算与已有结果。",
                    workflow::EvidenceKind::Write=>"需要homebase_files的write/edit/mkdir/copy/move/trash成功步骤。候选编号已有时直接引用，不重复写文件。",
                    workflow::EvidenceKind::Artifact=>"需要成功export或其他已登记产物的步骤，只有文件路径不够。",
                    workflow::EvidenceKind::Deliver=>"需要homebase_deliver对本会话成功产物的真实步骤，不能用普通读取或生成代替。",
                    workflow::EvidenceKind::Inspect=>"需要本轮对应的真实成功查询或读取步骤，说明文字不是step_id。",
                });
            }
        }
        Ok(result)
    }
    pub(super) async fn completion_gate(&self,tid:&str,has_calls:bool,reply:&str)->Result<CompletionGate,String>{
        let state=self.state.lock().await;let(s,t)=locate(&state,tid)?;let turn=&state.sessions[s].turns[t];let w=&turn.workflow;
        if has_calls{return Ok(CompletionGate{feedback:None,stop:None});}
        let request=turn.protocol.first().and_then(|m|m["content"].as_str()).unwrap_or("");
        let requested=["帮我记住","请记住","请纠正","请修改","帮我保存","帮我安排","请安排","请在","新增","记一下"].iter().any(|s|request.contains(s));
        let asks=["请确认","需要你确认","等你确认","确认后","确认是否","同意吗"].iter().any(|s|reply.contains(s));
        let draft=["草案","准备好","备好","准备新增","准备保存","准备修改"].iter().any(|s|reply.contains(s));
        let phantom=requested&&asks&&(draft||reply.contains("请确认是否保存")||reply.contains("请确认后再执行"))&&turn.pending.is_none();
        if phantom {
            let room=turn.rounds+1<w.max_rounds(turn.settings.max_rounds,turn.settings.free_only);
            if w.completion_deferrals<2&&room {return Ok(CompletionGate{feedback:Some("宿主核对：当前没有可确认的操作草案，不要只生成‘请确认’文字并结束。若用户已明确对象、内容和时间，请先查询必要版本，再调用相应写工具，宿主会暂停并出示一次真实确认卡；若必要条件缺失，只询问缺少的条件。工具结果含homebase_confirmation.approved=true时，本人已确认该次调用，成功后直接说明实际结果，不重复索取同一确认；新方案仍须新的确认。此反馈不授予操作权限。".into()),stop:None});}
            return Ok(CompletionGate{feedback:None,stop:Some(workflow::StopInfo{kind:if room{"needs_input"}else{"paused_budget"}.into(),message:"这次还没有准备出可确认的实际操作，尚未执行。我保留了原要求，避免把文字草案当成已经受理。".into(),next_action:"可以继续原任务并明确所需对象、内容或时间，原操作没有重复执行。".into(),retry_safe:true})});
        }
        if w.plan.is_none() || turn_has_pending_jobs(&state.sessions[s], turn) {
            return Ok(CompletionGate{feedback:None,stop:None});
        }
        if let Some(item) = w.unverified_item(&turn.steps, &state.sessions[s].jobs) {
            return Ok(CompletionGate { feedback: None, stop: Some(item.review_stop()) });
        }
        if !w.unfinished(){return Ok(CompletionGate{feedback:None,stop:None});}
        let room=turn.rounds+1<w.max_rounds(turn.settings.max_rounds,turn.settings.free_only);
        if !w.blocked()&&w.completion_deferrals<2&&room {
            return Ok(CompletionGate{feedback:Some("任务计划仍有未完成阶段。请继续执行并验证、使用已有step证据更新计划；只有前置条件确实缺失时才标记blocked并说明。不要声称全部完成，不要重做已完成操作，计划不扩大权限。".into()),stop:None});
        }
        Ok(CompletionGate{feedback:None,stop:Some(workflow::StopInfo{kind:if room{"needs_input"}else{"paused_budget"}.into(),message:w.incomplete_message(),next_action:if w.blocked(){"请处理上述缺失条件后再继续"}else{"可以明确要求继续原任务；已完成操作不会重新提交"}.into(),retry_safe:!state.sessions[s].jobs.iter().filter(|j|turn_has_job(turn,j)).any(|j|matches!(j.state.as_str(),"unknown"|"interrupted_unknown"))})})
    }
    pub(super) async fn workflow_failure(&self,tid:&str,error:&str)->Result<(),String>{
        let diagnostic=crate::model_capability_io::reliability::Failure::decode(error);
        let public_error=diagnostic.as_ref().map(|f|f.message());
        let error=public_error.as_deref().unwrap_or(error);
        let resumable = self.inspect_turn(tid, |session, turn| progress::resume_problem(session, turn).is_none()).await?;
        let budget=error.starts_with("HB_BUDGET:");
        let ledger_full=error.starts_with("HB_STALLED_LEDGER:");
        let stalled=error.starts_with("HB_STALLED");
        let state=if budget{"paused_budget"}else if stalled{"needs_input"}else{"failed"};
        let message=if error.starts_with("HB_STALLED_LEDGER:") {"这项任务积累的失败记录已到保护上限，我先停在这里，保留原结果和失败依据。后续工作需要拆分处理，不能清空记录后原样重做。".to_string()}else if error.starts_with("HB_STALLED_EFFECT:") {"这步操作没有成功，也可能留下了部分结果。我先停下来，不会原样再做；需要核对原结果或换个办法，已完成的工作会保留。".to_string()}else if stalled {"这一步用相同参数尝试了三次，都没有成功。我先停下来，保留已经完成的结果；需要核对缺少的条件或换个办法，再接着处理。".to_string()}else if budget {"这次执行已经到预算上限，我先停在这里。已完成的步骤和结果会保留；明确说“继续”后再接着处理。".to_string()}else if resumable {format!("这一步遇到了问题，已完成的结果会保留。{}",error.chars().take(500).collect::<String>())}else{format!("这一步暂时没能确认结果，我先停下了，不会重复执行。{}",error.chars().take(500).collect::<String>())};
        self.change_turn(tid,|s,t|{if s.turns[t].workflow.version>0{s.turns[t].workflow.stop=Some(workflow::StopInfo{kind:state.into(),message:message.clone(),next_action:if ledger_full{"保留原任务和结果，拆分未完成的后续工作；不要清空失败依据或原样重做"}else if budget||resumable{"可继续原任务；不会重复已完成步骤"}else{"先查询原任务，必要时补充信息；未知外部结果不会自动重试"}.into(),retry_safe:!s.turns[t].runtime_uncertain_model && s.turns[t].control_intents.is_empty()});}Ok(())}).await?;
        self.finish(tid,state,&message).await
    }
}
