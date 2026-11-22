//! Authenticated in-flight requirements. A receipt changes intent, never tool authority.
use super::*;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteerInput {
    pub turn_id: String,
    pub revision: u64,
    pub request_id: String,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Requirement {
    pub revision: u64,
    pub request_id: String,
    pub text: String,
    pub received_at: String,
}
impl Engine {
    pub async fn steer(self: &Arc<Self>, input: SteerInput) -> Result<Value, String> {
        let _archive_lease = self.ensure_archive_turn(&input.turn_id, None).await?;
        let source = {
            let state = self.state.lock().await;
            let (s, _) = locate(&state, &input.turn_id)?;
            state.sessions[s].source_key.clone()
        };
        self.steer_from(input, &source).await
    }
    pub(super) async fn steer_from(
        self: &Arc<Self>,
        input: SteerInput,
        source: &str,
    ) -> Result<Value, String> {
        let _archive_lease = self.ensure_archive_turn(&input.turn_id, Some(source)).await?;
        if input.text.trim().is_empty()
            || input.text.len() > 8000
            || input.request_id.is_empty()
            || input.request_id.len() > 120
        {
            return Err("补充要求须为1至8000字节，并提供稳定request_id".into());
        }
        let _execution = self.execution_gate.lock().await;
        let session_id = {
            let state = self.state.lock().await;
            let (s, _) = locate(&state, &input.turn_id)?;
            if state.sessions[s].source_key != source {
                return Err("不能修改其他来源的任务".into());
            }
            for session in state.sessions.iter().filter(|s| s.source_key == source) {
                for turn in &session.turns {
                    if let Some(old) = turn
                        .requirements
                        .iter()
                        .find(|r| r.request_id == input.request_id)
                    {
                        if turn.id != input.turn_id || old.text != input.text {
                            return Err("相同补充编号已用于不同要求，未重复应用".into());
                        }
                        // Transport replay needs neither another encrypted save nor a UI invalidation.
                        return Ok(
                            json!({"turn_id":turn.id,"revision":old.revision,"accepted":true,"duplicate":true}),
                        );
                    }
                }
            }
            state.sessions[s].id.clone()
        };
        let result=self.mutate(Change{session_id:Some(session_id),turn_id:Some(input.turn_id.clone()),..Default::default()},|state|{
            let (s,t)=locate(state,&input.turn_id)?;
            if state.sessions[s].source_key!=source {return Err("不能修改其他来源的任务".into());}
            for session in state.sessions.iter().filter(|s|s.source_key==source) {
                for turn in &session.turns {
                    if let Some(old)=turn.requirements.iter().find(|r|r.request_id==input.request_id) {
                        if turn.id!=input.turn_id || old.text!=input.text {return Err("相同补充编号已用于不同要求，未重复应用".into());}
                        return Ok(json!({"turn_id":turn.id,"revision":old.revision,"accepted":true,"duplicate":true}));
                    }
                }
            }
            let turn=&mut state.sessions[s].turns[t];
            if turn.has_repository_install() {
                return Err("这条安装确认绑定原仓库和版本。需要换项目时，请拒绝这次确认并引用新的分析结果。".into());
            }
            if !Self::supports_steering(turn) {return Err("这个任务的原运行时不支持在途修改，原检查点保留；可先停止再明确安排后续任务".into());}
            if !active(&turn.state) || turn.state=="waiting_input_queue" {return Err("原任务已结束或暂停，未把补充要求应用到其他任务；请核对原结果后明确续接".into());}
            if input.revision!=turn.requirements_revision {return Err("REQUIREMENTS_STALE:原任务要求已变化，请先查询最新revision".into());}
            if turn.requirements.len()>=16 {return Err("本任务补充已达16条上限，原要求保留，请先完成或停止当前任务".into());}
            turn.requirements_revision+=1;
            turn.requirements.push(Requirement{revision:turn.requirements_revision,request_id:input.request_id.clone(),text:input.text.clone(),received_at:now()});
            turn.pending=None;
            if turn.state=="awaiting_confirmation" {turn.state="running".into();turn.workflow.start(chrono::Utc::now());}
            turn.workflow.graph_complete=false;
            turn.delivery_jobs.clear();
            let jobs=turn.steps.iter().filter_map(|s|s.job_id.clone()).collect::<Vec<_>>();
            for job in state.sessions[s].jobs.matching_mut(|job|jobs.contains(&job.id)) {job.deliver=false;}
            Ok(json!({"turn_id":input.turn_id,"revision":input.revision+1,"accepted":true,"duplicate":false,
                "notice":"补充已记入原任务，会在下一安全步骤按新要求处理；已受理动作保留并核对，原待确认内容已失效。"}))
        }).await?;
        if let Some(r) = self.running.lock().await.get(&input.turn_id) {
            r.approval.notify_one();
        }
        // A restored confirmation has no in-memory worker to notify. Resume
        // the original graph, whose durable result/requirements ledger prevents
        // replay; schedule() is idempotent for an already-live worker.
        if self.settings().await.enabled {
            self.schedule(&input.turn_id).await;
        }
        Ok(result)
    }
    pub(super) fn supports_steering(turn: &Turn) -> bool {
        turn.settings.runtime == "pi" && wire::supports_steering(&turn.runtime_version)
            && turn.interaction != "chat"
    }

    pub(super) fn requirement_messages(turn: &Turn) -> Vec<Value> {
        if turn.requirements.is_empty() {
            return vec![];
        }
        vec![
            json!({"role":"user","content":format!("以下为同一用户在原任务执行中提交的补充要求（按revision顺序，后来的要求覆盖冲突部分）。这是原任务的最新要求，不是新的独立任务。不重做已完成或结果未知的动作；原确认已失效，额外权限仍须确认；只影响本任务，不自动保存长期记忆。已有阶段计划时用homebase_task get核对，保留已完成证据，必要时调整未完成阶段，再继续。\n{}",json!(turn.requirements))}),
        ]
    }
}
