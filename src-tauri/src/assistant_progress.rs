//! Evidence-based presentation shared by chat, desktop and WeChat. Never executes work.
use super::*;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Progress {
    pub goal: String,
    pub completed: Vec<String>,
    pub current: Option<String>,
    pub blocked: Option<String>,
    pub next: Option<String>,
    pub completed_stages: usize,
    pub total_stages: usize,
}
pub(super) const CANCELED_MODEL_UNKNOWN: &str = "后续步骤已停下，模型请求的结果仍未确认。";
pub(super) const REVIEW_CANCELED_MODEL: &str = "可以核对原任务和已有结果。";

pub(super) fn model_outcome_unknown(turn: &Turn) -> bool {
    terminal(&turn.state) && turn.runtime_uncertain_model
}

pub(super) fn job_outcome_unknown(job: &Job) -> bool {
    workflow::unknown_result_state(&job.state) || workflow::result_outcome_unknown(&job.result)
}

fn short(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}
fn label(step: &Step) -> String {
    let request = step.arguments.get("request").unwrap_or(&step.arguments);
    if step.name == "homebase_files" {
        let path = request["path"].as_str().unwrap_or("").replace('\\', "/");
        let file = short(path.rsplit('/').next().unwrap_or(""), 80);
        if !file.is_empty() {
            return format!(
                "{} {file}",
                match request["action"].as_str() {
                    Some("read") => "读取",
                    Some("write") => "保存",
                    Some("edit") => "修改",
                    Some("export") => "整理附件",
                    Some("search") => "查找",
                    Some("copy") => "复制",
                    Some("move") => "移动",
                    Some("trash") => "回收",
                    _ => "核对",
                }
            );
        }
    }
    if step.name == "homebase_video" {
        if let Some(title) = step.arguments["title"]
            .as_str()
            .or(step.arguments["input"]["title"].as_str())
        {
            return format!("制作“{}”的视频", short(title, 80));
        }
    }
    match step.name.as_str() {
        "homebase_video" => "制作视频",
        "homebase_files" => "处理工作区文件",
        "homebase_terminal" => "执行隔离命令",
        "homebase_deliver" => "安排文件返回",
        "homebase_memory" => "核对偏好",
        "homebase_history" => "查阅之前的对话",
        "homebase_matter" => "整理持续事项",
        "homebase_mcp" => "调用连接的能力",
        "homebase_installed_skill" => "使用已授权技能",
        "homebase_discover" | "homebase_describe" | "homebase_skill" => "核对可用功能与条件",
        _ => "核对处理结果",
    }
    .into()
}
// Presentation and explicit continuation share this structural gate. Current
// model enablement is still checked by the execution entry, never by a snapshot.
pub(super) fn resume_problem(session: &Session, turn: &Turn) -> Option<&'static str> {
    if !matches!(
        turn.state.as_str(),
        "interrupted_unknown" | "failed" | "paused_budget" | "needs_input"
    ) {
        return Some("仅能继续已中断或失败的轮次");
    }
    if !turn.control_intents.is_empty() {
        return Some("原步骤的受理或结果尚待核对，请先查询原任务，保留原请求");
    }
    if turn.settings.runtime != "pi" || turn.runtime_operations.is_empty() {
        return Some("此轮次没有可恢复的助理检查点");
    }
    if turn.runtime_uncertain_model {
        return Some("模型请求结果未知，不能重复推理；请查询原业务任务");
    }
    if turn.workflow.unverified_item(&turn.steps, &session.jobs).is_some() {
        return Some("原阶段的完成结果待核对，请查看原步骤和原任务");
    }
    if turn.steps.len() >= workflow::MAX_TASK_STEPS
        || matches!(turn.state.as_str(), "paused_budget" | "needs_input")
            && turn
                .workflow
                .max_rounds(turn.settings.max_rounds, turn.settings.free_only)
                >= workflow::MAX_TASK_ROUNDS
    {
        return Some("这项任务已到单次累计上限；请新建对话并沿用已生成结果，未重跑原操作");
    }
    if !compatible_runtime(&turn.runtime_version) {
        return Some("运行时版本不兼容，不能直接继续");
    }
    let Some(position) = session.turns.iter().position(|item| item.id == turn.id) else {
        return Some("原任务不属于当前会话，不能直接继续");
    };
    if session.turns.iter().enumerate().any(|(index, item)| {
        index != position && active(&item.state) && item.state != "waiting_input_queue"
    }) {
        return Some("请先处理当前会话中的其他任务");
    }
    resume_history::resume_index(session, turn).err()
}
fn recovery_hint(session: &Session, turn: &Turn) -> Option<(String, String)> {
    if turn.state == "canceled" && model_outcome_unknown(turn) {
        return Some((CANCELED_MODEL_UNKNOWN.into(), REVIEW_CANCELED_MODEL.into()));
    }
    if !matches!(
        turn.state.as_str(),
        "interrupted_unknown" | "failed" | "paused_budget" | "needs_input" | "unknown"
    ) {
        return None;
    }
    let problem = resume_problem(session, turn);
    let next = if turn.runtime_uncertain_model {
        "先核对这件事已受理的任务和已有结果；模型是否受理尚不明确，保留原请求。"
    } else if problem == Some("请先处理当前会话中的其他任务") {
        "先核对这个会话中的其他任务和原结果，再决定怎样继续这件事。"
    } else if problem == Some(resume_history::UNCERTAIN) {
        "原任务的上下文历史归属还不能唯一确认；保留全部对话，请先核对原结果，未续接或重做。"
    } else if problem.is_none() {
        "可以明确继续这件事，沿用原任务、原模型设置和已保存结果。"
    } else {
        "先核对原任务和已有结果；这次无法直接续接，不要把未知操作重新提交。"
    };
    let reason = if let Some(failure) = &turn.model_failure {
        use crate::model_capability_io::reliability::Kind;
        match failure.kind {
            Kind::Connect | Kind::ConnectTimeout => {
                "连接模型服务没有成功，这次推理尚未提交；原进度已保留。"
            }
            Kind::Build => "这次模型请求的格式没有通过检查，尚未发送；原进度已保留。",
            Kind::Http | Kind::Provider => match failure.provider_status.or(failure.http_status) {
                Some(429) => "模型服务暂时限流，明确拒绝了这次请求；已完成的步骤保留。",
                Some(401 | 403) => "模型服务拒绝了这次身份验证；已完成的步骤保留。",
                Some(402) => "模型服务因账户或额度限制拒绝了这次请求；原进度已保留。",
                _ => "模型服务明确返回失败，这件事暂时停下了；原进度已保留。",
            },
            _ => "这次模型响应没有完整确认，是否已被受理仍不明确；原任务和已有结果保留。",
        }
    } else if turn.runtime_uncertain_model {
        "模型请求的结果还没有确认，原任务和已有结果保留。"
    } else if turn.state == "unknown" {
        "这件事的执行结果还没有确认，不能当作已经完成。"
    } else if turn.state == "interrupted_unknown" {
        "这件事在中途停下了，原任务和已保存结果都保留着。"
    } else if turn.state == "paused_budget" {
        "这次处理已到预算，原任务和已完成的步骤保留。"
    } else if turn.state == "needs_input" {
        "这件事还需要补充信息或明确继续，已有结果保留。"
    } else {
        "这件事暂时没有完成，原记录和已完成的步骤保留。"
    };
    Some((reason.into(), next.into()))
}
pub fn snapshot(session: &Session, turn: &Turn) -> Progress {
    let plan = turn.workflow.plan.as_ref();
    let mut stage_to_review = None;
    let completed: Vec<_> = if let Some(plan) = plan {
        plan.items
            .iter()
            .filter(|i| i.state == "completed")
            .filter_map(|i| {
                if i.completion_unverified(&turn.steps, &session.jobs) {
                    stage_to_review.get_or_insert(i);
                    None
                } else {
                    Some(short(&i.input.title, 100))
                }
            })
            .collect()
    } else {
        let mut labels = Vec::new();
        for step in turn
            .steps
            .iter()
            .filter(|s| workflow::successful(s, &session.jobs))
        {
            let name = label(step).to_string();
            if !labels.contains(&name) {
                labels.push(name);
            }
        }
        labels.into_iter().rev().take(3).collect()
    };
    let unknown = session.jobs.iter().any(|job| turn_has_job(turn, job) && job_outcome_unknown(job));
    let blocked_item = plan.and_then(|p| p.items.iter().find(|i| i.state == "blocked"));
    let recovery = recovery_hint(session, turn);
    let blocked = if unknown {
        Some("有一步还没确认执行结果，我先停在这里，避免重复操作。".into())
    } else if turn.pending.as_ref().is_some_and(|p| !p.approved) {
        Some("下一步需要你确认；确认前不会执行。".into())
    } else if turn.model_failure.is_some() || turn.runtime_uncertain_model {
        recovery.as_ref().map(|(reason, _)| reason.clone())
    } else if let Some(item) = stage_to_review {
        Some(format!("「{}」的完成结果待核对。", short(&item.input.title, 100)))
    } else if let Some(i) = blocked_item {
        Some(format!(
            "{}：{}",
            short(&i.input.title, 100),
            short(&i.note, 240)
        ))
    } else {
        turn.workflow
            .stop
            .as_ref()
            .map(|s| short(&s.message, 240))
            .or_else(|| recovery.as_ref().map(|(reason, _)| reason.clone()))
    };
    let working = active(&turn.state)
        && !matches!(turn.state.as_str(), "queued" | "waiting_input_queue")
        && blocked.is_none();
    let current = if working {
        plan.and_then(|p| p.items.iter().find(|i| i.state == "in_progress"))
            .map(|i| short(&i.input.title, 100))
            .or_else(|| {
                turn.steps
                    .iter()
                    .rev()
                    .find(|s| s.state == "running")
                    .map(label)
            })
            .or_else(|| {
                if turn.interaction == "chat" {
                    Some("回应你的问题".into())
                } else if turn.state == "waiting_jobs" {
                    Some("等待已受理任务返回结果".into())
                } else if !turn.steps.is_empty() {
                    Some("核对已有结果并准备下一步".into())
                } else {
                    None
                }
            })
    } else {
        None
    };
    let next = if unknown || stage_to_review.is_some() {
        Some(workflow::REVIEW_NEXT.into())
    } else if recovery.is_some()
        && (turn.model_failure.is_some()
            || turn.runtime_uncertain_model
            || resume_problem(session, turn).is_some())
    {
        recovery.as_ref().map(|(_, next)| next.clone())
    } else if blocked.is_some() {
        turn.workflow
            .stop
            .as_ref()
            .map(|s| short(&s.next_action, 240))
            .or_else(|| recovery.as_ref().map(|(_, next)| next.clone()))
    } else if turn.state != "canceled" && turn.state != "rejected" {
        plan.and_then(|p| p.items.iter().find(|i| i.state == "pending"))
            .map(|i| short(&i.input.title, 100))
    } else {
        None
    };
    Progress {
        goal: plan.map(|p| short(&p.goal, 180)).unwrap_or_else(|| {
            if let Some(goal) = archive::live_messages::goal(session, &turn.id) {
                return goal.to_owned();
            }
            session
                .messages
                .iter()
                .find(|m| m.turn_id.as_deref() == Some(&turn.id) && m.role == "user")
                .map(|m| short(&m.content, 100))
                .unwrap_or_default()
        }),
        completed_stages: plan.map(|_| completed.len()).unwrap_or(0),
        total_stages: plan.map(|p| p.items.len()).unwrap_or(0),
        completed,
        current,
        blocked,
        next,
    }
}
impl Progress {
    pub(super) fn completed_text(&self) -> Option<String> {
        (!self.completed.is_empty()).then(|| {
            format!(
                "已完成：{}。",
                self.completed
                    .iter()
                    .rev()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("、")
            )
        })
    }

    pub fn text(&self) -> String {
        let mut lines = Vec::new();
        if self.total_stages > 0 {
            lines.push(format!(
                "这件事已完成 {}/{} 个阶段。",
                self.completed_stages, self.total_stages
            ));
        }
        if let Some(completed) = self.completed_text() {
            lines.push(completed);
        }
        if let Some(value) = &self.current {
            lines.push(format!("正在做：{}。", value.trim_end_matches('。')));
        }
        if let Some(value) = &self.blocked {
            lines.push(format!("目前卡在：{value}"));
        }
        if let Some(value) = &self.next {
            lines.push(format!("下一步：{}。", value.trim_end_matches('。')));
        }
        if lines.is_empty() {
            "这件事还没有可核验的执行进展。".into()
        } else {
            lines.join("\n")
        }
    }
}
