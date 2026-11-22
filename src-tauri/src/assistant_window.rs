//! Actual bounded timeline windows, retaining original IDs and full message text.
use super::*;
use std::collections::BTreeSet;

const WINDOW_BYTES: usize = 256 * 1024;
pub fn public_error(error: &str) -> Option<(&'static str, &'static str)> {
    [
        (
            "WINDOW_BAD_ARGUMENT",
            "会话窗口参数无效，请核对原会话和读取范围；未执行或修改任务",
        ),
        (
            "WINDOW_SESSION_NOT_FOUND",
            "原会话不存在或当前不可读取；未回退其他会话",
        ),
        (
            "WINDOW_IDENTITY_CONFLICT",
            "会话记录编号无效或重复，未猜测来源或任务归属",
        ),
        (
            "WINDOW_CURSOR_NOT_FOUND",
            "原消息游标不可读取，请刷新原会话；未跳过或清空记录",
        ),
        ("WINDOW_INVALID_DATA", "会话窗口数据无法编码，原记录未修改"),
        (
            "WINDOW_ENTRY_TOO_LARGE",
            "单条消息或任务概况超过窗口预算，请使用原历史分块接口；未截断正文",
        ),
    ]
    .into_iter()
    .find(|(code, _)| error.starts_with(&format!("{code}:")))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineRequest {
    pub session_id: String,
    pub before_message_id: Option<String>,
    #[serde(default = "window_limit")]
    pub limit: usize,
}
fn window_limit() -> usize {
    40
}
#[derive(Serialize)]
pub struct WindowTurn {
    pub turn: TurnView,
    pub details_omitted: bool,
    pub step_count: usize,
    pub job_count: usize,
    pub artifact_count: usize,
}
#[derive(Serialize)]
pub struct WindowPage {
    pub version: String,
    pub captured_revision: u64,
    pub total_messages: usize,
    pub start_index: usize,
    pub end_index: usize,
    pub returned: usize,
    pub requested_limit: usize,
    pub before_message_id: Option<String>,
    pub has_more: bool,
}
#[derive(Serialize)]
pub struct SessionWindow {
    pub id: String,
    pub title: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_resume_generation: Option<u64>,
    pub messages: Vec<Message>,
    pub turns: Vec<WindowTurn>,
    pub page: WindowPage,
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}
fn unique<'a>(ids: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !valid_id(id) || !seen.insert(id) {
            return Err(
                "WINDOW_IDENTITY_CONFLICT: 会话记录编号无效或重复，未猜测历史或任务归属".into(),
            );
        }
    }
    Ok(())
}
impl Engine {
    pub async fn session_window(&self, request: TimelineRequest) -> Result<SessionWindow, String> {
        if !valid_id(&request.session_id)
            || !(1..=40).contains(&request.limit)
            || request
                .before_message_id
                .as_deref()
                .is_some_and(|id| !valid_id(id))
        {
            return Err("WINDOW_BAD_ARGUMENT: 会话窗口参数无效，未切换其他会话".into());
        }
        let (session, revision) = {
            let state = self.state.lock().await;
            let mut matches = state.sessions.iter().filter(|s| s.id == request.session_id);
            let session = matches
                .next()
                .ok_or("WINDOW_SESSION_NOT_FOUND: 原会话不存在，未回退其他来源")?;
            if matches.next().is_some() {
                return Err("WINDOW_IDENTITY_CONFLICT: 会话编号重复，未猜测来源".into());
            }
            if session.cold.is_none() && session.message_prefix.is_none() {
                return full_snapshot(session, &request, state.revision);
            }
            (session.clone(), state.revision)
        };
        if let Some(result) = archive::session_window(&session, &request, revision).await? {
            return Ok(result);
        }
        full_snapshot(&session, &request, revision)
    }
}

pub(super) fn full_snapshot(
    session: &Session,
    request: &TimelineRequest,
    revision: u64,
) -> Result<SessionWindow, String> {
    unique(session.messages.iter().map(|m| m.id.as_str()))?;
    unique(session.turns.iter().map(|t| t.id.as_str()))?;
    unique(session.jobs.iter().map(|j| j.id.as_str()))?;
    let end = if let Some(id) = request.before_message_id.as_deref() {
        session.messages.iter().position(|m| m.id == id).ok_or(
            "WINDOW_CURSOR_NOT_FOUND: 原消息游标已不可读取，请刷新原会话；未跳过或清空历史",
        )?
    } else {
        session.messages.len()
    };
    project(session, request, revision, session.messages.len(), end, 0, None)
}

/// The available rows can be a bounded cold range, but all cursors remain in
/// the original session's global message order. Never persist this projection.
pub(super) fn project(
    session: &Session,
    request: &TimelineRequest,
    revision: u64,
    total: usize,
    end: usize,
    base: usize,
    goals: Option<&std::collections::BTreeMap<String, String>>,
) -> Result<SessionWindow, String> {
    let mut count = (end - base).min(request.limit);
    let now = chrono::Utc::now();
    loop {
        let start = end - count;
        let messages: Vec<_> = session
            .messages
            .iter()
            .skip(start - base)
            .take(count)
            .cloned()
            .collect();
        let mut ids: BTreeSet<&str> = messages
            .iter()
            .filter_map(|m| m.turn_id.as_deref())
            .collect();
        // Keep the same active/FIFO selection discoverable outside this page.
        if let Some(t) = session
            .turns
            .iter()
            .find(|t| active(&t.state) && t.state != "waiting_input_queue")
        {
            ids.insert(&t.id);
        }
        if let Some(t) = session
            .turns
            .iter()
            .find(|t| t.state == "waiting_input_queue")
        {
            ids.insert(&t.id);
        }
        let turns = session
            .turns
            .iter()
            .filter(|t| ids.contains(t.id.as_str()))
            .map(|t| WindowTurn {
                turn: {
                    let mut view = view_summary_at(session, t, now);
                    if let Some(goal) = goals.and_then(|values| values.get(&t.id)) {
                        view.progress.goal = goal.clone();
                    }
                    view
                },
                details_omitted: true,
                step_count: t.steps.len(),
                job_count: session.jobs.iter().filter(|j| turn_has_job(t, j)).count(),
                artifact_count: session
                    .jobs
                    .iter()
                    .filter(|j| j.owned && turn_has_job(t, j))
                    .map(|j| j.artifacts.len())
                    .sum(),
            })
            .collect();
        let result = SessionWindow {
            id: session.id.clone(),
            title: session.title.clone(),
            source: session.source.clone(),
            queue_resume_generation: session.paused_queue_generation(),
            messages,
            turns,
            page: WindowPage {
                version: "homebase-session-window-v1".into(),
                captured_revision: revision,
                total_messages: total,
                start_index: start,
                end_index: end,
                returned: count,
                requested_limit: request.limit,
                before_message_id: if start > 0 {
                    Some(session.messages[start - base].id.clone())
                } else {
                    None
                },
                has_more: start > 0,
            },
        };
        if serde_json::to_vec(&result)
            .map_err(|_| "WINDOW_INVALID_DATA: 原会话窗口无法编码，未修改记录")?
            .len()
            <= WINDOW_BYTES
        {
            return Ok(result);
        }
        if count <= 1 {
            return Err("WINDOW_ENTRY_TOO_LARGE: 单条消息或任务概况超过窗口预算，请通过历史分块接口读取原正文；未返回截断成功".into());
        }
        count = (count / 2).max(1);
    }
}
