//! Source discovery holds bounded metadata, never writable history or authority.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(super) struct TurnHint {
    pub id: String,
    pub created_at: String,
    pub order: usize,
}
impl TurnHint {
    pub(super) fn from_turn(turn: &Turn, order: usize) -> Result<Self, String> {
        if turn.id.is_empty()
            || turn.id.len() > 256
            || turn.id.chars().any(char::is_control)
            || turn.created_at.len() > 128
            || turn.created_at.chars().any(char::is_control)
        {
            return Err("SOURCE_SELECTION_INVALID: 原任务候选无法核对，未猜测目标".into());
        }
        Ok(Self {
            id: turn.id.clone(),
            created_at: turn.created_at.clone(),
            order,
        })
    }
}
#[derive(Clone, Default)]
pub(super) struct ClosedHints {
    pub task: Option<TurnHint>,
    pub chat: Option<TurnHint>,
}
impl ClosedHints {
    pub(super) fn push(&mut self, turn: &Turn, order: usize) -> Result<(), String> {
        let hint = TurnHint::from_turn(turn, order)?;
        let slot = if turn.interaction == "chat" {
            &mut self.chat
        } else {
            &mut self.task
        };
        if slot
            .as_ref()
            .is_none_or(|old| (&old.created_at, old.order) <= (&hint.created_at, hint.order))
        {
            *slot = Some(hint);
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(super) struct Choice {
    pub session_id: String,
    pub turn_id: String,
}
struct Ranked {
    choice: Choice,
    time: String,
    session_order: usize,
    turn_order: usize,
}
#[derive(Default)]
struct Targets {
    active_tasks: Vec<Choice>,
    active_chat: Option<Choice>,
    task: Option<Ranked>,
    chat: Option<Ranked>,
}
impl Targets {
    fn closed(&mut self, sid: &str, order: usize, hint: &TurnHint, chat: bool) {
        let slot = if chat { &mut self.chat } else { &mut self.task };
        if slot.as_ref().is_none_or(|old| {
            (&old.time, old.session_order, old.turn_order) <= (&hint.created_at, order, hint.order)
        }) {
            *slot = Some(Ranked {
                choice: Choice {
                    session_id: sid.into(),
                    turn_id: hint.id.clone(),
                },
                time: hint.created_at.clone(),
                session_order: order,
                turn_order: hint.order,
            });
        }
    }
    fn hot(&mut self, session: &Session, order: usize) -> Result<(), String> {
        for (index, turn) in session.turns.iter().enumerate() {
            let hint = TurnHint::from_turn(turn, index)?;
            let chat = turn.interaction == "chat";
            if active(&turn.state) && turn.state != "waiting_input_queue" {
                let choice = Choice {
                    session_id: session.id.clone(),
                    turn_id: turn.id.clone(),
                };
                if chat {
                    if self.active_chat.is_none() {
                        self.active_chat = Some(choice);
                    }
                } else {
                    self.active_tasks.push(choice);
                    if self.active_tasks.len() > 1 {
                        return Err("有几件事同时在处理，请使用对应任务的完整编号查询或操作，避免选错。桌面助理也能查看它们。".into());
                    }
                }
            }
            self.closed(&session.id, order, &hint, chat);
        }
        Ok(())
    }
    fn finish(self) -> Option<Choice> {
        self.active_tasks
            .into_iter()
            .next()
            .or(self.active_chat)
            .or(self.task.map(|r| r.choice))
            .or(self.chat.map(|r| r.choice))
    }
}
fn check_ids(state: &State) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    if state.sessions.iter().any(|s| !ids.insert(s.id.as_str())) {
        return Err("SOURCE_SELECTION_INVALID: 会话编号重复，未猜测来源或目标".into());
    }
    Ok(())
}
// Cold adoption and derived-cache fills do not change this fingerprint. Mutable
// source facts do; another source's progress does not force a retry.
fn stamp(state: &State, source: &str) -> Result<Vec<u8>, String> {
    let mut digest = Sha256::new();
    for session in state.sessions.iter().filter(|s| s.source_key == source) {
        let receipts = archive::selection_receipts(session);
        let live: Vec<_> = session
            .turns
            .iter()
            .filter(|t| t.state != "completed" || t.pending.is_some())
            .map(|t| {
                (
                    &t.id,
                    &t.state,
                    &t.interaction,
                    &t.created_at,
                    t.workflow.resume_generation,
                    t.pending.as_ref().map(|p| (&p.id, p.approved)),
                )
            })
            .collect();
        digest.update(
            serde_json::to_vec(&(
                &session.id,
                &session.source,
                &session.updated_at,
                session.queue_paused,
                session.queue_resume_generation,
                receipts,
                live,
            ))
            .map_err(|_| "SOURCE_SELECTION_INVALID: 来源候选无法编码")?,
        );
    }
    Ok(digest.finalize().to_vec())
}
impl Engine {
    pub(super) async fn source_current_session(&self, source: &str) -> Result<String, String> {
        let state = self.state.lock().await;
        check_ids(&state)?;
        state
            .sessions
            .iter()
            .rev()
            .find(|s| s.source_key == source)
            .map(|s| s.id.clone())
            .ok_or_else(|| "当前还没有助理会话，请先描述任务".into())
    }
    pub(super) async fn source_target(&self, source: &str) -> Result<Option<Choice>, String> {
        let (mut targets, probes, captured) = {
            let state = self.state.lock().await;
            check_ids(&state)?;
            let mut targets = Targets::default();
            let mut latest = None;
            for (order, session) in state
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| s.source_key == source)
            {
                if latest
                    .as_ref()
                    .is_none_or(|(time, index, _)| (time, *index) <= (&session.updated_at, order))
                {
                    latest = Some((
                        session.updated_at.clone(),
                        order,
                        session.cold.is_none() && session.turns.is_empty(),
                    ));
                }
                if session.cold.is_none() {
                    targets.hot(session, order)?;
                }
            }
            if !targets.active_tasks.is_empty() {
                return Ok(targets.active_tasks.into_iter().next());
            }
            if latest.is_some_and(|(_, _, empty)| empty) {
                return Ok(None);
            }
            if targets.active_chat.is_some() {
                return Ok(targets.active_chat);
            }
            let probes = state
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| s.source_key == source)
                .filter_map(|(order, s)| archive::selection_probe(s).map(|probe| (order, probe)))
                .collect::<Vec<_>>();
            (targets, probes, stamp(&state, source)?)
        };
        let mut checked = BTreeSet::new();
        for (order, probe) in probes {
            let hints = archive::selection_hints(&probe, &mut checked).await?;
            if let Some(hint) = hints.task {
                targets.closed(&probe.id, order, &hint, false);
            }
            if let Some(hint) = hints.chat {
                targets.closed(&probe.id, order, &hint, true);
            }
        }
        if stamp(&*self.state.lock().await, source)? != captured {
            return Err(
                "SOURCE_SELECTION_CHANGED: 当前对话刚有变化，请按原消息重试；未执行或改选其他任务"
                    .into(),
            );
        }
        Ok(targets.finish())
    }
}
