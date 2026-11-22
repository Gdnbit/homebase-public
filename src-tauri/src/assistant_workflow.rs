//! Durable task metadata, evidence and bounded execution. A plan never grants permission.
use super::{Job, Records, Step, Turn};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FREE_TASK_ROUNDS: u32 = 24;
pub const MAX_TASK_ROUNDS: u32 = 48;
pub const MAX_TASK_STEPS: usize = 192;
pub const ACTIVE_SECONDS: u64 = 1800;

// Evidence reads need borrowed jobs, independent of snapshot representation.
pub trait JobSource {
    fn evidence_jobs(&self) -> impl Iterator<Item=&Job>;
}
impl JobSource for Records<Job> {
    fn evidence_jobs(&self) -> impl Iterator<Item=&Job> {self.iter()}
}
impl JobSource for Vec<Job> {
    fn evidence_jobs(&self) -> impl Iterator<Item=&Job> {self.iter()}
}
impl JobSource for [Job] {
    fn evidence_jobs(&self) -> impl Iterator<Item=&Job> {self.iter()}
}
impl<const N:usize> JobSource for [Job;N] {
    fn evidence_jobs(&self) -> impl Iterator<Item=&Job> {self.iter()}
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workflow {
    pub version: u8,
    pub plan: Option<Plan>,
    pub stop: Option<StopInfo>,
    pub completion_deferrals: u8,
    pub extra_rounds: u32,
    pub active_seconds: u64,
    pub active_since: Option<String>,
    pub graph_complete: bool,
    pub continuation_requested: bool,
    pub recoveries: u8,
    pub resume_generation: u64,
    pub tool_failures: std::collections::BTreeMap<String, u8>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Plan {
    pub revision: u64,
    pub goal: String,
    pub items: Vec<Item>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ItemInput {
    pub id: String,
    pub title: String,
    pub done_when: String,
    pub kind: EvidenceKind,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Inspect,
    Write,
    Execute,
    Artifact,
    Deliver,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    #[serde(flatten)]
    pub input: ItemInput,
    pub state: String,
    pub note: String,
    pub evidence: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct StopInfo {
    pub kind: String,
    pub message: String,
    pub next_action: String,
    pub retry_safe: bool,
}
#[derive(Clone, Serialize)]
pub struct Limits {
    pub model_calls_used: u32,
    pub max_model_calls: u32,
    pub steps_used: usize,
    pub max_steps: usize,
    pub active_seconds: u64,
    pub max_active_seconds: u64,
}
#[derive(Clone, Serialize)]
pub struct TaskWorkflowView {
    pub plan: Option<TaskPlanView>,
    pub stop: Option<StopInfo>,
    pub limits: Limits,
}
#[derive(Clone, Serialize)]
pub struct TaskPlanView {
    pub revision: u64,
    pub goal: String,
    pub items: Vec<TaskPlanItemView>,
}
#[derive(Clone, Serialize)]
pub struct TaskPlanItemView {
    pub id: String,
    pub title: String,
    pub done_when: String,
    pub kind: EvidenceKind,
    pub state: String,
    pub note: String,
    pub evidence: Vec<String>,
}

fn short(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= max && !text.contains('\0')
}
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}
fn reported_result_states(result: &Value) -> impl Iterator<Item = &str> {
    [result, &result["result"]].into_iter().flat_map(|value| {
        [value["status"].as_str(), value["state"].as_str()]
            .into_iter()
            .flatten()
    })
}
pub(super) fn unknown_result_state(state: &str) -> bool {
    matches!(state, "unknown" | "uncertain" | "interrupted_unknown")
}
pub(super) fn result_outcome_unknown(result: &Value) -> bool {
    reported_result_states(result).any(unknown_result_state)
        || [result, &result["result"]].into_iter().any(|value| {
            matches!(value["code"].as_str(), Some("RESULT_UNAVAILABLE" | "JOB_RESULT_UNAVAILABLE"))
        })
}
pub(super) fn result_blocks_step(result: &Value) -> bool {
    result_outcome_unknown(result) || [result, &result["result"]].into_iter().any(|value| {
        value["ok"] == false || value["success"] == false || value["partial"] == true || value["complete"] == false
    }) || reported_result_states(result).any(|state| {
        matches!(state,
            "failed" | "error" | "running" | "queued" | "pending" | "submitted" | "processing"
                | "canceled" | "timed_out" | "blocked" | "partial" | "superseded")
    })
}
pub(super) fn job_has_success_evidence(job: &Job) -> bool {
    job.state == "succeeded" && !job.result.is_null() && !result_blocks_step(&job.result)
}
pub(super) fn successful(step: &Step, jobs: &(impl JobSource+?Sized)) -> bool {
    if step.state != "completed" || step.result.is_null() || result_blocks_step(&step.result) { return false; }
    step.job_id.as_ref().is_none_or(|id|super::job_access::owned(jobs,id).ok().flatten().is_some_and(job_has_success_evidence))
}
fn matches_kind(kind: &EvidenceKind, step: &Step, jobs: &(impl JobSource+?Sized)) -> bool {
    let req=&step.arguments["request"];
    match kind {
        EvidenceKind::Deliver=>step.name=="homebase_deliver" && step.result["job_id"].as_str().is_some_and(|id|super::job_access::owned(jobs,id).ok().flatten().is_some_and(|j|job_has_success_evidence(j)&&!j.artifacts.is_empty())),
        EvidenceKind::Artifact=>artifact_evidence(&step.result),
        EvidenceKind::Execute=>step.name=="homebase_terminal" && step.result["result"]["exit_code"]==0,
        EvidenceKind::Write=>step.name=="homebase_files" && matches!(req["action"].as_str(),Some("write"|"edit"|"mkdir"|"copy"|"move"|"trash")),
        EvidenceKind::Inspect=>!matches!(step.name.as_str(),"homebase_task"|"homebase_deliver"),
    }
}
pub(super) fn artifact_evidence(result: &Value) -> bool {
    result["files"].as_array().is_some_and(|files| !files.is_empty())
        || result["result"]["artifact"].is_object()
}

fn verify_evidence(
    kind: &EvidenceKind,
    evidence: &[String],
    steps: &[Step],
    jobs: &(impl JobSource + ?Sized),
) -> Result<(), &'static str> {
    let mut matches_stage = false;
    for id in evidence {
        let position = super::step_position(steps, id)
            .map_err(|_| "证据不存在、不属于本轮或无法唯一核对")?;
        let step = &steps[position];
        if !successful(step, jobs) {
            return Err("证据对应的执行尚未成功或结果待核对");
        }
        matches_stage |= matches_kind(kind, step, jobs);
    }
    if !matches_stage {
        return Err("缺少此阶段对应的真实成功证据，请先核对原步骤结果");
    }
    Ok(())
}

pub(super) const REVIEW_STAGE: &str = "原完成凭据还未核对，请先查看原步骤和原任务结果。";
pub(super) const REVIEW_NEXT: &str = "先核对原任务的状态和已有结果，再决定后续步骤。";

impl Item {
    pub(super) fn completion_unverified(&self, steps: &[Step], jobs: &(impl JobSource + ?Sized)) -> bool {
        self.state == "completed"
            && verify_evidence(&self.input.kind, &self.evidence, steps, jobs).is_err()
    }

    pub(super) fn review_stop(&self) -> StopInfo {
        StopInfo {
            kind: "needs_input".into(),
            message: format!("「{}」的完成结果待核对。已确认的结果会保留。", self.input.title),
            next_action: REVIEW_NEXT.into(),
            retry_safe: false,
        }
    }
}
impl Workflow {
    pub fn enabled() -> Self {Self{version:1,..Self::default()}}
    pub fn start(&mut self, now: DateTime<Utc>) {
        if self.version>0 && self.active_since.is_none(){self.active_since=Some(now.to_rfc3339());}
    }
    pub fn elapsed(&self, now: DateTime<Utc>) -> u64 {
        self.active_seconds.saturating_add(self.active_since.as_ref().and_then(|s|DateTime::parse_from_rfc3339(s).ok()).map(|at|(now-at.with_timezone(&Utc)).num_seconds().max(0)as u64).unwrap_or(0))
    }
    pub fn pause(&mut self, now: DateTime<Utc>) {self.active_seconds=self.elapsed(now);self.active_since=None;}
    pub fn checkpoint(&mut self,now:DateTime<Utc>){if self.active_since.is_some(){self.active_seconds=self.elapsed(now);self.active_since=Some(now.to_rfc3339());}}
    pub fn recovered_pause(&mut self){self.active_since=None;}
    pub fn max_rounds(&self, normal: u32, free_only: bool) -> u32 {
        let base=if self.plan.is_some()&&free_only {normal.max(FREE_TASK_ROUNDS)}else{normal};
        base.saturating_add(self.extra_rounds).min(MAX_TASK_ROUNDS)
    }
    pub fn check(&self, normal:u32,free_only:bool,rounds:u32,steps:usize,now:DateTime<Utc>) -> Result<(),String> {
        if self.version==0 {return Ok(());}
        if rounds>=self.max_rounds(normal,free_only) {return Err("HB_BUDGET:本次推理次数已到上限".into());}
        if steps>=MAX_TASK_STEPS {return Err("HB_BUDGET:本次执行步骤已到上限".into());}
        if self.elapsed(now)>=ACTIVE_SECONDS {return Err("HB_BUDGET:本次连续执行时间已到上限".into());}
        Ok(())
    }
    pub fn check_step(&self,steps:usize,now:DateTime<Utc>)->Result<(),String>{
        if self.version==0{return Ok(());}
        if steps>=MAX_TASK_STEPS{return Err("HB_BUDGET:本次执行步骤已到上限".into());}
        if self.elapsed(now)>=ACTIVE_SECONDS{return Err("HB_BUDGET:本次连续执行时间已到上限".into());}
        Ok(())
    }
    pub fn view(&self, turn: &Turn, jobs: &(impl JobSource + ?Sized), now: DateTime<Utc>) -> Option<TaskWorkflowView> {
        if self.version == 0 {
            return None;
        }
        let plan = self.plan.as_ref().map(|plan| TaskPlanView {
            revision: plan.revision,
            goal: plan.goal.clone(),
            items: plan.items.iter().map(|item| {
                let unverified = item.completion_unverified(&turn.steps, jobs);
                TaskPlanItemView {
                    id: item.input.id.clone(),
                    title: item.input.title.clone(),
                    done_when: item.input.done_when.clone(),
                    kind: item.input.kind.clone(),
                    state: if unverified { "blocked".into() } else { item.state.clone() },
                    note: if unverified { REVIEW_STAGE.into() } else { item.note.clone() },
                    evidence: item.evidence.clone(),
                }
            }).collect(),
        });
        Some(TaskWorkflowView {
            plan,
            stop: self.stop.clone(),
            limits: Limits {
                model_calls_used: turn.rounds,
                max_model_calls: self.max_rounds(turn.settings.max_rounds, turn.settings.free_only),
                steps_used: turn.steps.len(),
                max_steps: MAX_TASK_STEPS,
                active_seconds: self.elapsed(now),
                max_active_seconds: ACTIVE_SECONDS,
            },
        })
    }
    pub(super) fn unverified_item(&self, steps: &[Step], jobs: &(impl JobSource + ?Sized)) -> Option<&Item> {
        self.plan.as_ref()?.items.iter().find(|item| item.completion_unverified(steps, jobs))
    }
    pub fn set_plan(&mut self, revision:u64,goal:String,items:Vec<ItemInput>)->Result<(),String>{
        let previous=self.plan.as_ref();
        if revision!=previous.map(|p|p.revision).unwrap_or(0){return Err("计划已变化，请先get再更新".into());}
        if !short(&goal,240)||items.is_empty()||items.len()>12{return Err("计划须有明确目标及1至12个阶段".into());}
        let mut ids=std::collections::BTreeSet::new();
        for item in &items {
            if !safe_id(&item.id)||!ids.insert(item.id.clone())||!short(&item.title,120)||!short(&item.done_when,240){return Err("阶段编号须唯一，标题和验收条件须简短明确".into());}
        }
        if previous.is_some_and(|p|p.items.iter().filter(|i|i.state=="completed").any(|old|!items.contains(&old.input))){return Err("不能删除或改写已完成阶段的验收定义；发现问题可先重新打开该阶段".into());}
        let items=items.into_iter().map(|input|previous.and_then(|p|p.items.iter().find(|i|i.input==input)).cloned().unwrap_or(Item{input,state:"pending".into(),note:String::new(),evidence:vec![]})).collect();
        self.plan=Some(Plan{revision:revision+1,goal,items});self.stop=None;Ok(())
    }
    pub fn update(&mut self,revision:u64,id:&str,state:&str,note:String,evidence:Vec<String>,steps:&[Step],jobs:&(impl JobSource+?Sized))->Result<(),String>{
        let plan=self.plan.as_mut().ok_or("请先建立计划")?;
        if plan.revision!=revision {return Err("计划已变化，请先get再更新".into());}
        if !matches!(state,"pending"|"in_progress"|"completed"|"blocked")||note.chars().count()>400||evidence.len()>4{return Err("阶段状态、说明或证据超出范围".into());}
        let index=plan.items.iter().position(|i|i.input.id==id).ok_or("计划阶段不存在")?;
        if state=="in_progress"&&plan.items.iter().enumerate().any(|(n,i)|n!=index&&i.state=="in_progress"){return Err("请先结束或暂停当前阶段，再开始下一阶段".into());}
        let item=&mut plan.items[index];
        if state=="blocked"&&!short(&note,400){return Err("阻塞阶段须说明缺少什么及下一步".into());}
        if item.state=="completed"&&state!="completed"&&!short(&note,400){return Err("重新打开已完成阶段须说明新发现的问题".into());}
        if state=="completed" {
            verify_evidence(&item.input.kind, &evidence, steps, jobs)?;
        }
        item.state=state.into();item.note=note;item.evidence=if state=="completed"{evidence}else{vec![]};plan.revision+=1;self.stop=None;Ok(())
    }
    pub fn unfinished(&self)->bool{self.plan.as_ref().is_some_and(|p|p.items.iter().any(|i|i.state!="completed"))}
    pub fn candidates(&self,item_id:&str,steps:&[Step],jobs:&(impl JobSource+?Sized))->Vec<String>{
        let Some(item)=self.plan.as_ref().and_then(|p|p.items.iter().find(|i|i.input.id==item_id))else{return vec![]};
        steps.iter().rev().filter(|s|successful(s,jobs)&&matches_kind(&item.input.kind,s,jobs)).take(8).map(|s|s.id.clone()).collect()
    }
    pub fn blocked(&self)->bool{self.plan.as_ref().is_some_and(|p|p.items.iter().any(|i|i.state=="blocked"))}
    pub fn incomplete_message(&self)->String{
        let rest=self.plan.as_ref().map(|p|p.items.iter().filter(|i|i.state!="completed").map(|i|if i.note.is_empty(){i.input.title.clone()}else{format!("{}：{}",i.input.title,i.note)}).take(3).collect::<Vec<_>>().join("；")).unwrap_or_default();
        format!("这件事还没全部完成。剩余：{rest}。已完成的结果会保留，后续会沿用原任务。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(kind:EvidenceKind)->ItemInput{ItemInput{id:"one".into(),title:"核对合成文件".into(),done_when:"实际读取并验证内容".into(),kind}}
    fn step()->Step{Step{id:"proof".into(),name:"homebase_files".into(),state:"completed".into(),arguments:serde_json::json!({"request":{"action":"read"}}),result:serde_json::json!({"status":"succeeded","content":"synthetic"}),job_id:None}}
    #[test]
    fn completion_requires_correct_owned_success_and_definition_cannot_be_rewritten(){
        let mut w=Workflow::enabled();w.set_plan(0,"合成任务".into(),vec![item(EvidenceKind::Inspect)]).unwrap();
        assert!(w.update(1,"one","completed",String::new(),vec!["invented".into()],&[],&[]).is_err());
        let mut s=step();s.result=serde_json::json!({"result":{"status":"failed"}});
        assert!(w.update(1,"one","completed",String::new(),vec!["proof".into()],&[s],&[]).is_err());
        for result in [Value::Null, serde_json::json!({"result":{"status":"processing"}}),
            serde_json::json!({"status":"succeeded","complete":false})] {
            let mut s = step();
            s.result = result;
            assert!(w.update(1,"one","completed",String::new(),vec!["proof".into()],&[s],&[]).is_err());
        }
        let duplicate = [step(), step()];
        assert!(w.update(1,"one","completed",String::new(),vec!["proof".into()],&duplicate,&[]).is_err());
        w.update(1,"one","completed",String::new(),vec!["proof".into()],&[step()],&[]).unwrap();
        assert!(w.set_plan(2,"换目标".into(),vec![item(EvidenceKind::Deliver)]).is_err());
        assert!(w.update(1,"one","pending","新问题".into(),vec![],&[],&[]).is_err());
    }
    #[test]
    fn reading_is_not_delivery_and_no_success_is_inferred_from_running_job(){
        let mut w=Workflow::enabled();w.set_plan(0,"发送合成结果".into(),vec![item(EvidenceKind::Deliver)]).unwrap();
        assert!(w.update(1,"one","completed",String::new(),vec!["proof".into()],&[step()],&[]).is_err());
        let mut s=step();s.job_id=Some("foreign".into());assert!(!successful(&s,&[]));
    }
    #[test]
    fn task_extension_is_bounded_free_only_and_human_wait_is_not_active_time(){
        let at=DateTime::parse_from_rfc3339("2026-11-13T00:00:00Z").unwrap().with_timezone(&Utc);
        let mut w=Workflow::enabled();assert_eq!(w.max_rounds(8,true),8);w.set_plan(0,"任务".into(),vec![item(EvidenceKind::Inspect)]).unwrap();
        assert_eq!(w.max_rounds(8,true),24);assert_eq!(w.max_rounds(8,false),8);
        w.start(at);w.pause(at+chrono::Duration::seconds(20));w.start(at+chrono::Duration::hours(6));
        assert_eq!(w.elapsed(at+chrono::Duration::hours(6)+chrono::Duration::seconds(10)),30);
        assert!(w.check(8,true,24,1,at).is_err());assert!(w.check(8,true,1,MAX_TASK_STEPS,at).is_err());
    }
}
