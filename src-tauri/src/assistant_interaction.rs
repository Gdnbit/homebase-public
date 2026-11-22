//! Channel-neutral conversation controls and presentation. No model calls or credential access.
use super::*;

fn approval(p: &Pending, personal: &crate::assistant_personal::Personal, matters: &matters::Matters) -> String {
    if let Some(text) = repository_install::approval(p) {
        return text;
    }
    if p.description=="执行 assistant_local_execute" {
        let request=&p.arguments["request"];
        let operation=p.arguments["operation"].as_str().unwrap_or("本机操作");
        let action=request["action"].as_str().unwrap_or("执行命令");
        let workspace=request["workspace_id"].as_str().unwrap_or("scratch");
        let description=match operation {
            "files"=>format!("在工作区 {workspace} {}文件",match action {"edit"=>"编辑","move"=>"移动","trash"=>"回收","copy"=>"复制","write"=>"新建",_=>"处理"}),
            "terminal"=>format!("在工作区 {workspace} 的隔离终端执行命令"),
            "mcp"=>format!("调用已信任的 {} 连接：{}",request["server_id"].as_str().unwrap_or("MCP"),request["tool"].as_str().unwrap_or(action)),
            "skill"=>"在新的隔离目录执行这项技能脚本".to_string(),
            _=>"执行这项本机操作".to_string(),
        };
        let details=match operation {
            "files"=>{
                let mut details=format!("文件：{}",request["path"].as_str().unwrap_or("未指定"));
                if let Some(destination)=request["destination"].as_str(){details.push_str(&format!("\n目标：{destination}"));}
                if action=="edit" {details.push_str(&format!("\n把这段：\n{}\n改为：\n{}\n文件版本：{}",request["old_text"].as_str().unwrap_or(""),request["new_text"].as_str().unwrap_or(""),request["expected_sha256"].as_str().unwrap_or("未提供")));}
                if let Some(content)=request["content"].as_str(){details.push_str(&format!("\n内容：\n{content}"));}
                if action=="trash" {details.push_str("\n移到此工作区的回收目录，可按返回位置恢复。");}
                details
            },
            "terminal"=>format!("目录：{}\n命令：\n{}\n这条命令可修改所选工作区内的文件，不访问其他磁盘或网络。",request["cwd"].as_str().unwrap_or("工作区根目录"),request["command"].as_str().unwrap_or("")),
            "skill"=>format!("技能：{}\n脚本：{}\n参数：{}\n仅在新隔离目录运行，不修改原技能。",request["id"].as_str().unwrap_or(""),request["script"].as_str().unwrap_or(""),request.get("args").cloned().unwrap_or(json!([]))),
            _=>format!("参数：{}",request.get("arguments").unwrap_or(request)),
        };
        return format!("我准备{description}：\n{details}\n\n这样可以吗？回复“确认”就好，也可以回复“拒绝”。");
    }
    let action = match p.description.as_str() {
        "homebase_memory remember" => "保存这条记忆",
        "homebase_memory correct" => "纠正原来的长期记忆（保留原有效期与更正记录）",
        "homebase_memory forget" => "删除这条长期记忆（既有聊天仍保留）",
        "homebase_reminder create" => "创建这条提醒",
        "homebase_reminder cancel" => "取消这条提醒",
        "homebase_followup create" => "安排到时实际处理这件事（额外权限仍会单独询问）",
        "homebase_followup cancel" => "取消这项任务的后续自动执行",
        "homebase_matter save" => "保存这件持续跟进的事（记录与复查提醒不会自动执行业务）",
        "homebase_matter attach" => "把当前任务关联到这件事，后续直接查看原进展",
        _ => p.description.as_str(),
    };
    let mut details = Vec::new();
    let matter=if p.description=="homebase_matter attach"{matters.entries.iter().find(|m|Some(m.id.as_str())==p.arguments["id"].as_str())}else{None};
    if let Some(m)=matter {details.push(format!("事项：{}\n目标：{}",m.title,m.goal));}
    if p.description=="homebase_followup create" {
        if let Some(event)=p.arguments["trigger"].as_object() {
            details.push(format!("触发条件：原任务{}时，仅处理一次后续事项；等待截止时间见下方。",if event.get("on").and_then(Value::as_str)==Some("completed"){"完成"}else{"需要帮助"}));
        }
        details.push(format!("通知：{}",if p.arguments["notification_policy"]=="all"{"每次都汇报"}else{"有实际变化时汇报；重复结果安静保留，确认、异常和附件仍提示"}));
        if let Some(m)=matters.entries.iter().find(|m|Some(m.id.as_str())==p.arguments["matter_id"].as_str()) {
            details.push(format!("关联事项：{}\n原目标：{}\n每次执行会关联原事项；目标或下一步改变时暂停旧安排。",m.title,m.goal));
        }
    }
    if p.description=="homebase_matter save" && p.arguments["review_at"].is_null() {details.push("复查时间：不安排（若原来有复查时间，将清除）".into());}
    if p.description=="homebase_memory remember" && p.arguments.get("expires_at").is_some_and(Value::is_null) {
        details.push("到期时间：不设到期（如果原来有日期，将清除原日期）".into());
    }
    if p.description == "homebase_memory forget" {
        if let Some(memory) = personal
            .memories
            .iter()
            .find(|m| Some(m.id.as_str()) == p.arguments["id"].as_str())
        {
            details.push(format!(
                "目标记忆：{}\n内容：{}",
                memory.title, memory.content
            ));
        }
    }
    if p.description=="homebase_memory correct" {
        if let Some(memory)=personal.memories.iter().find(|m|Some(m.id.as_str())==p.arguments["id"].as_str()) {
            details.push(format!("原记忆：{}\n原内容：{}\n改为：{}\n原因：{}\n有效期：{}",memory.title,memory.content,p.arguments["content"].as_str().unwrap_or("未提供"),p.arguments["reason"].as_str().unwrap_or("未提供"),memory.expires_at.as_deref().unwrap_or("不设到期（沿用原设置）")));
        }
    }
    if p.description == "homebase_reminder cancel" {
        if let Some(reminder) = personal
            .reminders
            .iter()
            .find(|r| Some(r.id.as_str()) == p.arguments["id"].as_str())
        {
            details.push(format!(
                "目标提醒：{}\n提醒时间：{}",
                reminder.text, reminder.due_at
            ));
        }
    }
    for (key, label) in [
        ("title", "标题"),
        ("content", "内容"),
        ("text", "提醒"),
        ("instruction", "具体事项"),
        ("goal", "目标"),
        ("summary", "已确认进展"),
        ("next_action", "下一步"),
        ("state", "事项状态"),
        ("review_at", "复查时间"),
        ("max_runs", "最多执行次数"),
        ("due_at", "提醒时间"),
        ("expires_at", "到期时间"),
        ("interval_minutes", "重复间隔（分钟）"),
        ("id", "对象编号"),
    ] {
        if key=="id" && (matter.is_some() || p.description.starts_with("homebase_memory ")){continue;}
        if key=="content" && p.description=="homebase_memory correct"{continue;}
        if let Some(value) = p.arguments.get(key).filter(|v| !v.is_null()) {
            details.push(format!(
                "{}：{}",if key=="due_at" && p.arguments["trigger"].is_object(){"等待截止"}else{label},
                value.as_str().map(|text| {
                    if matches!(key, "due_at" | "expires_at") {
                        chrono::DateTime::parse_from_rfc3339(text)
                            .map(|d| d.with_timezone(&chrono::Local).format("%Y年%m月%d日 %H:%M（本机时间）").to_string())
                            .unwrap_or_else(|_| text.to_string())
                    } else { text.to_string() }
                }).unwrap_or_else(|| value.to_string())
            ));
        }
    }
    if !p.description.starts_with("homebase_memory ")
        && !p.description.starts_with("homebase_reminder ")
        && !p.description.starts_with("homebase_followup ")
        && !p.description.starts_with("homebase_matter ")
    {
        details.clear();
    }
    if details.is_empty() {
        details.push(format!("参数：{}", p.arguments));
    }
    format!("我准备{action}：\n{}\n\n这样可以吗？回复“确认”就好，也可以回复“拒绝”。",details.join("\n"))
}
impl Engine {
    pub(super) async fn claim_queued_turn(&self, tid: &str, running: &[String]) -> Result<bool, String> {
        let _archive_lease = self.ensure_archive_turn(tid, None).await?;
        let sid = {
            let state = self.state.lock().await;
            let (s, _) = locate(&state, tid)?;
            state.sessions[s].id.clone()
        };
        self.mutate_when_changed(Change {
            session_id: Some(sid), turn_id: Some(tid.into()), ..Default::default()
        }, |state| {
            if !state.settings.enabled || running.len() >= 4 { return Ok((false, false)); }
            let (s, t) = locate(state, tid)?;
            let session = &mut state.sessions[s];
            if session.turns[t].state != "waiting_input_queue" { return Ok((false, false)); }
            let discussion = session.turns[t].interaction == "chat";
            let busy = session.turns.iter().any(|turn|
                (!discussion || turn.interaction == "chat") &&
                (running.contains(&turn.id) || active(&turn.state) && turn.state != "waiting_input_queue"));
            if busy || !discussion && session.queue_paused { return Ok((false, false)); }
            let turn = &mut session.turns[t];
            turn.state = "queued".into();
            turn.text.clear();
            let received_at = turn.created_at.clone();
            turn.runtime_prompt.push_str(&format!(
                "\n消息收到时间：{}；实际开始处理时间：{}。本条相对日期以消息收到时间为基准；若目标时间已过，先向用户核实，不能擅自延后。",
                received_at, chrono::Local::now().to_rfc3339()
            ));
            Ok((true, true))
        }).await
    }
    pub async fn resume_queue(&self, sid: &str) -> Result<Value, String> {
        self.resume_queue_once(sid,None).await
    }
    pub(crate) async fn resume_queue_once(&self,sid:&str,expected:Option<u64>)->Result<Value,String>{
        let _archive_lease = self.ensure_archive_session(sid).await?;
        let result = self
            .mutate_when_changed(
                Change {
                    session_id: Some(sid.into()),
                    ..Default::default()
                },
                |state| {
                    let enabled = state.settings.enabled;
                    let session = state
                        .sessions
                        .iter_mut()
                        .find(|s| s.id == sid)
                        .ok_or("会话不存在")?;
                    let count = session
                        .turns
                        .iter()
                        .filter(|t| t.state == "waiting_input_queue")
                        .count();
                    // Reconcile an already committed release even if the model was
                    // disabled later. This path neither saves nor wakes the queue.
                    if expected.is_some_and(|n| n != session.queue_resume_generation || !session.queue_paused || count == 0) {
                        return Ok((json!({"queued":count,"resumed":false}),false));
                    }
                    if !enabled {
                        return Err("模型已停用，队列未继续".into());
                    }
                    if count == 0 {
                        return Err("没有待继续的排队消息".into());
                    }
                    session.queue_paused = false;
                    session.queue_resume_generation+=1;
                    Ok((json!({"queued":count,"resumed":true}),true))
                },
            )
            .await?;
        if result["resumed"] == true { self.queue_wake.notify_one(); }
        Ok(result)
    }
    pub(super) async fn channel_view(
        &self,
        source: &str,
        tid: &str,
        include_files: bool,
    ) -> Result<Value, String> {
        self.reconcile_control_receipts(tid, Some(source), None, false).await?;
        let unverified = self.refresh_accepted_turn_jobs(tid, Some(source), None).await?;
        let mut snapshot = self.read_cold_turn_with_jobs(tid, Some(source), None, false).await?;
        let mut state = self.state.lock().await;
        // A follow-up may be linked while the read is in flight. Its notification
        // fingerprint depends on original results, including nested evidence.
        if snapshot.is_some() && notifications::evidence_followup(&state, source, tid).is_some() {
            drop(state);
            snapshot = self.read_cold_turn(tid, Some(source), None).await?;
            state = self.state.lock().await;
        }
        let session = if let Some(session) = &snapshot { session } else { &state.sessions[locate(&state, tid)?.0] };
        let turn = session.turns.iter().find(|t| t.id == tid).ok_or("未找到助理轮次")?;
        if session.source_key != source {
            return Err("不能查询其他来源的助理轮次".into());
        }
        let status = match turn.state.as_str() {
            "queued" | "running" | "waiting_jobs" => "running",
            "completed" => "succeeded",
            other => other,
        };
        let details = progress::snapshot(session, turn);
        let model_outcome_unknown = progress::model_outcome_unknown(turn);
        let has_unknown_job = session.jobs.iter().any(|job| turn_has_job(turn, job) && progress::job_outcome_unknown(job));
        let pending_accepted_jobs = session.jobs.iter().filter(|job| accepted_job_pending(turn, job)).count();
        let status_unverified = |job: &Job| accepted_job_pending(turn, job) && unverified.contains(&job.id);
        let unverified_accepted_jobs = session.jobs.iter().filter(|job| status_unverified(job)).count();
        let progress = if turn.state == "waiting_input_queue" {
            if session.queue_paused {
                "后续消息队列已暂停。说“继续排队”可执行后续消息，原结果未知的任务不会重做。".to_string()
            } else {
                "这条要求记下了，会在手头工作结束后处理；还没有修改原任务。".into()
            }
        } else {
            details.text()
        };
        let mut text = if let Some(p) = turn.pending.as_ref().filter(|p| !p.approved) {
            approval(p, &state.personal, &state.matters)
        } else if turn.text.is_empty() || active(&turn.state)
            || terminal(&turn.state) && (has_unknown_job || details.completed_stages < details.total_stages)
            || matches!(turn.state.as_str(), "failed" | "interrupted_unknown" | "paused_budget" | "needs_input" | "unknown")
                && (turn.model_failure.is_some() || turn.runtime_uncertain_model)
        {
            progress.to_string()
        } else {
            turn.text.clone()
        };
        if matches!(turn.state.as_str(), "failed" | "interrupted_unknown" | "paused_budget" | "needs_input" | "unknown") {
            if let Some(next) = details.next.as_ref().filter(|next| !text.contains(*next)) {
                text.push_str(&format!("\n\n下一步：{next}"));
            }
        }
        if turn.state == "canceled" {
            text = if model_outcome_unknown {
                format!("{}{}", progress::CANCELED_MODEL_UNKNOWN, progress::REVIEW_CANCELED_MODEL)
            } else if turn.has_repository_install() && session.jobs.iter().any(|job| {
                job.owned && job.turn_id == turn.id && job.command == "repo_project_install"
                    && job.state == "canceled"
            }) {
                "仓库安装已取消。原项目状态可以在能力库查看。".into()
            } else {
                "这件事的后续步骤已经停下，已有结果会保留。".into()
            };
            if model_outcome_unknown {
                if let Some(completed) = details.completed_text() {
                    text.push_str("\n\n");
                    text.push_str(&completed);
                }
            }
            let still_pending = pending_accepted_jobs - unverified_accepted_jobs;
            if still_pending > 0 {
                text.push_str(&format!("还有 {still_pending} 项原任务尚未结束。你可以查看原任务的状态和已有结果。"));
            }
        }
        if unverified_accepted_jobs > 0 {
            text.push_str(&format!("\n\n有 {unverified_accepted_jobs} 项原任务暂未查到最新状态，可以稍后再核对。已确认的结果仍可查看。"));
        }
        if terminal(&turn.state) && (turn.state != "completed" || has_unknown_job) {
            let related = session.jobs.iter().filter(|job| turn_has_job(turn, job));
            let jobs: Vec<_> = related.clone()
                .filter(|job| accepted_job_pending(turn, job))
                .chain(related.clone().filter(|job| job.owned && !accepted_job_pending(turn, job) && progress::job_outcome_unknown(job)))
                .chain(related.clone().filter(|job| job.owned && !accepted_job_pending(turn, job) && !progress::job_outcome_unknown(job)))
                .chain(related.filter(|job| !job.owned))
                .take(6)
                .map(|job| {
                    let label = match job.command.as_str() {
                        "assistant_local_execute" => "本机操作",
                        "capability_render" => "视频制作",
                        "model_capability_call" => "模型处理",
                        "repo_project_install" => "仓库安装",
                        _ => "处理步骤",
                    };
                    let status = if status_unverified(job) {
                        "暂未查到最新状态"
                    } else if progress::job_outcome_unknown(job) {
                        "结果还未确认"
                    } else {
                        match job.state.as_str() {
                            "succeeded" if workflow::job_has_success_evidence(job) => "已完成",
                            "succeeded" => "尚无可确认的完成结果",
                            "running" => "仍在处理",
                            "queued" => "排队中",
                            "failed" => "未完成",
                            "canceled" => "已停止",
                            "timed_out" => "已超时",
                            _ => "结果还未确认",
                        }
                    };
                    format!("{label}：{status}")
                })
                .collect();
            if !jobs.is_empty() {
                text.push_str(&format!(
                    "\n\n原任务的最新情况（没有重新执行）：\n{}",
                    jobs.join("\n")
                ));
            }
        }
        let signature = format!("{:x}",Sha256::digest(format!("{}:{}:{}",turn.state,progress,turn.pending.as_ref().map(|p|p.id.as_str()).unwrap_or(""))));
        let artifacts: Vec<_> = session
            .jobs
            .iter()
            .filter(|j| j.owned && (j.deliver || include_files) && turn_has_job(turn, j))
            .flat_map(|j| j.artifacts.iter().map(|a| json!(a)))
            .take(3)
            .collect();
        let notice=super::notifications::metadata(&state,session,turn,!artifacts.is_empty());
        let mut result=json!({"status":status,"result":{"status":status,"turn_id":tid,"text":text,"progress":progress,"progress_details":details,"model_outcome_unknown":model_outcome_unknown,"pending_accepted_jobs":pending_accepted_jobs,"unverified_accepted_jobs":unverified_accepted_jobs,"interaction":turn.interaction,"parent_turn_id":turn.parent_turn_id,"progress_signature":signature,"queue_paused":session.queue_paused,"queue_depth":session.turns.iter().filter(|t|t.state=="waiting_input_queue").count(),"artifacts":artifacts,"jobs":session.jobs.iter().filter(|j|turn_has_job(turn,j)).map(|j|json!({"id":j.id,"state":j.state,"command":j.command})).collect::<Vec<_>>()},"error":null});
        result["result"]["progress_has_execution_evidence"] = json!(turn.interaction != "chat"
            && session.jobs.iter().any(|job| job.owned && turn_has_job(turn, job)
                && !matches!(job.command.as_str(), "capability_index_query" | "repo_capability_describe")));
        if !notice.is_null() {result["result"]["notification"]=notice;}
        if status == "awaiting_confirmation" {
            if let Some(pending) = turn.pending.as_ref().filter(|p| !p.approved) {
                result["result"]["confirmation_id"] = json!(pending.id);
            }
        }
        Ok(result)
    }
    pub(super) async fn named_file(&self, source: &str, request: &Value) -> Result<Value, String> {
        use crate::wechat::file_delivery::{valid_attachment_name, FileSelection};
        let name = arg_string(request, "name")?;
        if !valid_attachment_name(name) {
            return Err("请提供原附件名称。".into());
        }
        let file: Option<FileSelection> = serde_json::from_value(request["file"].clone())
            .map_err(|_| "附件引用无法核对。")?;
        if !request["turn_id"].is_null() {
            let turn_id = arg_string(request, "turn_id")?;
            if uuid::Uuid::parse_str(turn_id).is_err() {
                return Err("原任务引用无法核对。".into());
            }
            return self.inspect_scoped_turn(turn_id, None, TurnProjection::Inspection, |session, turn| {
                if session.source != "wechat" || session.source_key != source {
                    return Err("无法核对这笔任务的微信归属。".into());
                }
                let artifacts = session.jobs.iter().filter(|job| job.owned && turn_has_job(turn, job))
                    .flat_map(|job| &job.artifacts).take(3).map(|artifact| (turn_id, artifact));
                Ok(select_named_file(artifacts, name, file.as_ref()))
            }).await?;
        }
        if file.is_some() {
            return Err("附件引用缺少原任务。".into());
        }
        let Some(scope) = self.current_wechat_scope(source).await else {
            return Ok(json!({"state":"missing"}));
        };
        let window = self.session_window(window::TimelineRequest {
            session_id: scope.clone(), before_message_id: None, limit: 40,
        }).await?;
        if window.source != "wechat" || self.current_wechat_scope(source).await.as_deref() != Some(scope.as_str()) {
            return Err("当前对话刚有变化，请按原消息重试。".into());
        }
        let artifacts = window.turns.iter().flat_map(|row| {
            row.turn.artifacts.iter().take(3).map(|artifact| (row.turn.id.as_str(), artifact))
        });
        Ok(select_named_file(artifacts, name, None))
    }

    pub(super) async fn quick_channel(
        self: &Arc<Self>,
        source: &str,
        text: &str,
    ) -> Result<Option<Value>, String> {
        if dialogue::model_identity_question(text) {
            let settings = self.settings().await;
            return Ok(Some(json!({"action":"message","text":dialogue::current_model_answer(&settings)})));
        }
        if ["持续事项","待跟进事项","还有哪些事"].contains(&text.trim()) {
            let s=self.state.lock().await;
            let entries=Self::matter_list(&s,source,"");
            let rows=entries["entries"].as_array().unwrap();
            let lines:Vec<_>=rows.iter().filter(|e|!matches!(e["state"].as_str(),Some("completed"|"canceled"))).take(8).map(|e|format!("• {}：{}；下一步{}",e["title"].as_str().unwrap_or("这件事"),e["summary"].as_str().filter(|s|!s.is_empty()).unwrap_or("尚无确认的新进展"),e["next_action"].as_str().filter(|s|!s.is_empty()).unwrap_or("待决定"))).collect();
            return Ok(Some(json!({"action":"message","text":if lines.is_empty(){"目前没有待跟进的持续事项。你可以告诉我想长期跟进哪件事。".to_string()}else{format!("还在跟进这些事：\n{}\n想继续哪件，告诉我它的名称就好。",lines.join("\n"))}})));
        }
        let text = match text.trim() {
            "取消" => "停止",
            value => value,
        };
        let query_files = current_query_files(text);
        if query_files.is_none() && ![
            "确认",
            "拒绝",
            "停止",
            "停止助理",
            "停止当前任务",
            "继续",
            "继续排队",
            "新对话",
            "开启新对话",
            "/new",
            "/stop",
        ]
        .contains(&text)
        {
            return Ok(None);
        }
        if ["新对话", "开启新对话", "/new"].contains(&text) {
            let session = self.make_session("wechat", source).await?;
            return Ok(Some(
                json!({"action":"message","text":"已开启新对话。长期记忆与配置仍然共用，原任务不会被取消。","session_id":session.id}),
            ));
        }
        if ["确认", "拒绝"].contains(&text) {
            let choices = {
                let state = self.state.lock().await;
                Self::confirmation_targets(&state,source).into_iter().filter_map(|t| {
                        t.pending
                            .as_ref()
                            .filter(|p| !p.approved)
                            .map(|p| (t.id.clone(), p.id.clone()))
                    })
                    .collect::<Vec<_>>()
            };
            if choices.len() != 1 {
                return Err(if choices.is_empty() {
                    "目前没有待确认操作；未执行任何确认"
                } else {
                    "有多个待确认操作，请使用消息中的完整确认编号，避免误批"
                }
                .into());
            }
            return Ok(Some(
                serde_json::to_value(
                    self.confirm(&choices[0].0, &choices[0].1, text == "确认")
                        .await?,
                )
                .map_err(|_| "确认结果无法编码")?,
            ));
        }
        if ["继续", "继续排队"].contains(&text) {
            let selected=self.source_current_session(source).await?;
            let _archive_lease=self.ensure_archive_session(&selected).await?;
            let (sid, turn_id) = {
                let state = self.state.lock().await;
                let session = state.sessions.iter().find(|s| s.id==selected&&s.source_key == source)
                    .ok_or("当前还没有助理会话，请先描述任务")?;
                (session.id.clone(), if text == "继续排队" { None }
                    else { channel_control::continuation_turn(session)?.map(|turn| turn.id.clone()) })
            };
            if let Some(turn_id) = turn_id {
                return Ok(Some(serde_json::to_value(self.resume(&turn_id).await?).map_err(|_| "继续结果无法编码")?));
            }
            let value = self.resume_queue(&sid).await?;
            return Ok(Some(channel_control::queue_response(&value)));
        }
        self.source_current_session(source).await?;
        let tid = self.source_target(source).await?.map(|choice|choice.turn_id);
        let Some(tid) = tid else {
            return Ok(Some(
                json!({"action":"message","text":"当前新对话还没有任务，请直接描述你想完成的事情。"}),
            ));
        };
        if ["停止", "停止助理", "停止当前任务", "/stop"].contains(&text) {
            self.cancel(&tid).await?;
            return Ok(Some(
                json!({"action":"message","text":"已停止当前助理轮次并暂停后续队列。已受理的业务任务可能继续运行，可说“进度”查询；需要执行后续消息时说“继续排队”。","turn_id":tid}),
            ));
        }
        let files = query_files.unwrap_or(false);
        let view = self.channel_view(source, &tid, files).await?;
        Ok(Some(
            json!({"action":"view","turn_id":tid,"view":view,"files":files}),
        ))
    }
}

fn select_named_file<'a>(
    artifacts: impl Iterator<Item = (&'a str, &'a Artifact)>,
    name: &str,
    reference: Option<&crate::wechat::file_delivery::FileSelection>,
) -> Value {
    use crate::wechat::file_delivery::FileSelection;
    let mut matching = artifacts.filter(|(_, file)| {
        std::path::Path::new(&file.path).file_name().and_then(|name| name.to_str())
            .is_some_and(|file| file.eq_ignore_ascii_case(name))
            && reference.is_none_or(|reference| reference.matches(&file.path, &file.sha256))
    });
    let Some((turn_id, artifact)) = matching.next() else { return json!({"state":"missing"}); };
    if matching.next().is_some() {
        return json!({"state":"ambiguous"});
    }
    let Some(file) = FileSelection::from_path(&artifact.path, &artifact.sha256) else {
        return json!({"state":"unavailable"});
    };
    json!({"state":"found","target":turn_id,"file":file})
}

pub(crate) fn current_query_files(text: &str) -> Option<bool> {
    let phrase = text.trim().trim_end_matches(['?', '？', '。', '!', '！']).trim_end();
    match channel_control::normalized(phrase) {
        "进度" | "状态" | "当前任务" | "/status" | "状态呢" | "好了吗" | "完成了吗"
        | "结果出来了吗" | "查一下进度" | "帮我看看进度" | "做到哪了"
        | "刚才那件事做到哪了" | "刚才那件事好了吗" | "刚才那件事进展怎么样"
        | "帮我看看刚才那件事做到哪了" => Some(false),
        "结果" | "结果呢" | "查一下结果" | "发一下结果" | "把结果发给我"
        | "把结果发过来" | "把刚才的结果发给我" | "把刚才的结果发过来"
        | "帮我把刚才的结果发过来" => Some(true),
        _ if referred_progress_query(phrase) => Some(false),
        _ => None,
    }
}

fn referred_progress_query(phrase: &str) -> bool {
    let question = ["帮我看看", "帮我查一下", "请问", "看看"]
        .into_iter()
        .find_map(|prefix| phrase.strip_prefix(prefix))
        .unwrap_or(phrase)
        .trim_start();
    let Some(reference) = ["刚才", "刚刚"].into_iter().find_map(|prefix| question.strip_prefix(prefix)) else {
        return false;
    };
    ["那个", "那件事", "那项任务", "那条任务"].into_iter()
        .filter_map(|reference_word| reference.trim_start().strip_prefix(reference_word))
        .any(|verb| matches!(verb.trim_start(),
            "做到哪了" | "做到哪一步了" | "做完了吗" | "好了吗" | "进展如何" | "进展怎么样" | "进度呢" | "进度怎么样"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_queries_match_entire_phrases_and_preserve_file_intent() {
        for text in ["进度呢？", " 做到哪一步了? ", "结果出来了吗？", "好了吗？",
            "刚才那件事做到哪了？", " 帮我看看刚才那件事做到哪了？ ", "帮我看看进度。",
            "刚才那个做完了吗？", "帮我看看刚才那个做到哪一步了？", "刚刚那件事进展如何？"] {
            assert_eq!(current_query_files(text), Some(false), "{text}");
        }
        for text in ["结果呢？", "把结果发我！", "把结果发给我。", "把刚才的结果发过来。",
            "帮我把刚才的结果发过来。"] {
            assert_eq!(current_query_files(text), Some(true), "{text}");
        }
        for text in ["解释一下结果呢？", "好了吗？改成蓝色", "停止？", "/结果 original-id",
            "刚才那件事做到哪了？再帮我加一段字幕", "把刚才的结果发过来之前先把颜色改成绿色",
            "帮我看看进度并把今天的工作总结一下", "把上周那次的结果发过来",
            "刚才那个做到哪一步了，再帮我剪一段", "刚才那个视频上传了吗"] {
            assert_eq!(current_query_files(text), None, "{text}");
        }
    }
    #[test]
    fn confirmation_shows_all_business_arguments_and_resolves_destructive_targets() {
        let mut personal = crate::assistant_personal::Personal::default();
        let pending = Pending {
            id: "synthetic-consent".into(),
            tool_id: "call".into(),
            description: "agents_set_model".into(),
            arguments: json!({"id":"fixture-agent","model":"fixture-model","endpoint":"https://example.invalid"}),
            approved: false,
        };
        let text = approval(&pending, &personal, &matters::Matters::default());
        assert!(text.contains("fixture-agent"));
        assert!(text.contains("fixture-model"));
        assert!(text.contains("https://example.invalid"));
        assert!(!text.contains("synthetic-consent"));
        assert!(text.contains("确认"));
        let memory = personal
            .save_memory(
                crate::assistant_personal::MemoryInput {
                    revision: 0,
                    id: None,
                    title: "待删除的合成记忆".into(),
                    content: "合成内容".into(),
                    expires_at: None,
                },
                chrono::Utc::now(),
            )
            .unwrap();
        let pending = Pending {
            description: "homebase_memory forget".into(),
            arguments: json!({"id":memory.id,"revision":1,"action":"forget"}),
            ..pending
        };
        assert!(approval(&pending, &personal, &matters::Matters::default()).contains("待删除的合成记忆"));
    }
}
