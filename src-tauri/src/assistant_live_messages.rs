//! Keep old finalized chat messages in the canonical archive while their session stays active.
use super::*;
use crate::assistant_history::{HistoryKind, HistoryQuery, HistorySource};
use crate::assistant_state_store::{RecordSlot, SeriesOverlay};

const RECENT_MESSAGES: usize = 48;

pub(in crate::assistant) struct Prefix {
    snapshot: Snapshot,
    store: Arc<Mutex<Store>>,
    trim: Arc<Trim>,
}

#[derive(Clone, Default)]
pub(super) struct Trim {
    ids: Vec<String>,
    goals: BTreeMap<String, String>,
    bytes: usize,
}

impl Prefix {
    pub(super) fn bytes(&self) -> usize {
        self.trim.bytes
    }
}

pub(in crate::assistant) fn count(session: &Session) -> usize {
    session.messages.len()
        + session
            .message_prefix
            .as_ref()
            .map_or(0, |p| p.trim.ids.len())
}

pub(in crate::assistant) fn goal<'a>(session: &'a Session, turn: &str) -> Option<&'a str> {
    session
        .message_prefix
        .as_ref()?
        .trim
        .goals
        .get(turn)
        .map(String::as_str)
}

pub(super) fn hint_bytes(session: &Session) -> Result<usize, String> {
    session
        .message_prefix
        .as_ref()
        .map_or(Ok(0), |prefix| prefix.trim.hint_bytes())
}

fn eligible(turn: &Turn) -> bool {
    turn.state == "completed"
        && turn.interaction == "chat"
        && turn.history_committed
        && turn.finished_at.is_some()
        && turn.steps.is_empty()
        && turn.pending.is_none()
        && turn.queued.is_empty()
        && turn.delivery_jobs.is_empty()
        && turn.parent_turn_id.is_none()
        && turn.steered_turn_id.is_none()
        && !turn.runtime_uncertain_model
        && turn.requirements.is_empty()
        && turn.workflow.plan.is_none()
        && turn.workflow.stop.is_none()
}

fn sealed(session: &Session) -> BTreeSet<&str> {
    let mut eligible_ids = BTreeSet::new();
    let mut duplicate = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for turn in &session.turns {
        if !seen.insert(turn.id.as_str()) {
            duplicate.insert(turn.id.as_str());
        }
        if eligible(turn) {
            eligible_ids.insert(turn.id.as_str());
        }
    }
    for id in duplicate {
        eligible_ids.remove(id);
    }
    for job in &session.jobs {
        eligible_ids.remove(job.turn_id.as_str());
    }
    eligible_ids
}

fn can_move(message: &Message, sealed: &BTreeSet<&str>) -> bool {
    message.attachments.is_empty()
        && matches!(message.role.as_str(), "user" | "assistant")
        && message
            .turn_id
            .as_deref()
            .is_some_and(|id| sealed.contains(id))
}

impl Trim {
    pub(super) fn hint_bytes(&self) -> Result<usize, String> {
        capacity::encoded_bytes(&(&self.ids, &self.goals))
    }
    fn push(&mut self, message: &Message) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_add(capacity::encoded_bytes(message)? + 1)
            .ok_or_else(invalid)?;
        self.ids.push(message.id.clone());
        if message.role == "user" {
            if let Some(id) = &message.turn_id {
                self.goals.entry(id.clone()).or_insert_with(|| {
                    message
                        .content
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(100)
                        .collect()
                });
            }
        }
        Ok(())
    }
}

pub(super) fn retain_restored_prefix(shell: &Session, loaded: &mut Session) -> Result<(), String> {
    let cold = shell.cold.as_ref().ok_or_else(invalid)?;
    if let Some(trim) = trim(loaded)? {
        loaded.message_prefix = Some(Arc::new(Prefix {
            snapshot: cold.snapshot.clone(),
            store: cold.store.clone(),
            trim: Arc::new(trim),
        }));
    }
    Ok(())
}

pub(super) fn trim(session: &mut Session) -> Result<Option<Trim>, String> {
    let mut limit = session.messages.len().saturating_sub(RECENT_MESSAGES);
    // Foreground discussion selects the last six non-empty previous messages.
    limit = limit.min(
        session
            .messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, message)| !message.content.is_empty())
            .nth(7)
            .map_or(0, |(index, _)| index),
    );
    if limit == 0 {
        return Ok(None);
    }
    let sealed = sealed(session);
    let removed = session
        .messages
        .iter()
        .take(limit)
        .take_while(|m| can_move(m, &sealed))
        .count();
    if removed == 0 {
        return Ok(None);
    }
    let mut projection = session
        .message_prefix
        .as_ref()
        .map_or_else(Trim::default, |p| (*p.trim).clone());
    for message in session.messages.iter().take(removed) {
        projection.push(message)?;
    }
    session.messages.drop_prefix(removed);
    Ok(Some(projection))
}

pub(super) fn overlay(
    session: &Session,
    value: &mut Value,
) -> Result<Option<SeriesOverlay>, String> {
    let Some(prefix) = &session.message_prefix else {
        return Ok(None);
    };
    let messages = std::mem::take(value["messages"].as_array_mut().ok_or_else(invalid)?);
    let records = prefix
        .trim
        .ids
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, id)| RecordSlot::Retained { index, id })
        .chain(messages.into_iter().map(RecordSlot::Owned))
        .collect();
    Ok(Some(SeriesOverlay {
        session_id: session.id.clone(),
        kind: Kind::Messages,
        snapshot: prefix.snapshot.clone(),
        records,
    }))
}

pub(super) fn publish(
    state: &mut State,
    trims: Vec<(usize, Trim)>,
    saved: &Snapshot,
    store: &Arc<Mutex<Store>>,
) {
    let mut changed: BTreeMap<_, _> = trims.into_iter().collect();
    for (position, session) in state.sessions.iter_mut().enumerate() {
        let trim = changed.remove(&position).map(Arc::new).or_else(|| {
            session
                .message_prefix
                .as_ref()
                .map(|prefix| prefix.trim.clone())
        });
        if let Some(trim) = trim {
            session.message_prefix = Some(Arc::new(Prefix {
                snapshot: saved.clone(),
                store: store.clone(),
                trim,
            }));
        }
    }
}

pub(super) fn load(
    reader: &mut Store,
    snapshot: &Snapshot,
    shell: &Session,
    store: Arc<Mutex<Store>>,
) -> Result<Session, String> {
    // Load one record group at a time. Only the protected suffix survives startup.
    let mut session = shell.clone();
    live_turn_replay::load(&mut session, reader, snapshot, live_turn_replay::LoadMode::Startup)?;
    live_jobs::load(&mut session, reader, snapshot)?;
    session.history = body::<Vec<Value>>(reader, snapshot, &shell.id, Kind::Context)?.to_vec();
    let total = snapshot.count(&shell.id, Kind::Messages)?;
    if total <= RECENT_MESSAGES {
        session.messages = body(reader, snapshot, &shell.id, Kind::Messages)?;
        return Ok(session);
    }
    let view = reader.record_view(snapshot, &shell.id)?;
    let mut prefix = Trim::default();
    let mut messages = Vec::new();
    let mut suffix_bytes = 0usize;
    let mut moving = true;
    {
        let closed = sealed(&session);
        // Locate enough non-empty recent messages even if there is a sparse legacy tail.
        let mut recent = total;
        let mut nonempty = 0;
        while recent > 0 && nonempty < 8 {
            let start = recent.saturating_sub(32);
            let rows = view.range(Kind::Messages, start, recent - start)?;
            for value in rows.into_iter().rev() {
                recent -= 1;
                if value["content"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
                {
                    nonempty += 1;
                }
                if nonempty == 8 {
                    break;
                }
            }
        }
        let limit = recent.min(total.saturating_sub(RECENT_MESSAGES));
        let mut index = 0;
        history_reader::visit(&view, Kind::Messages, |value| {
            let message: Message = serde_json::from_value(value).map_err(|_| invalid())?;
            moving &= index < limit && can_move(&message, &closed);
            if moving {
                prefix.push(&message)?;
            } else {
                suffix_bytes = suffix_bytes
                    .checked_add(capacity::encoded_bytes(&message)?)
                    .ok_or_else(invalid)?;
                if suffix_bytes > capacity::RESIDENT_BYTES {
                    return Err(invalid());
                }
                messages.push(message);
            }
            index += 1;
            Ok(())
        })?;
    }
    session.messages = messages.into();
    if !prefix.ids.is_empty() {
        session.message_prefix = Some(Arc::new(Prefix {
            snapshot: snapshot.clone(),
            store,
            trim: Arc::new(prefix),
        }));
    }
    Ok(session)
}

async fn view(session: &Session) -> Result<Arc<crate::assistant_state_store::RecordView>, String> {
    let prefix = session.message_prefix.as_ref().ok_or_else(invalid)?;
    if !prefix
        .snapshot
        .owner_matches(&session.id, &session.source, &session.source_key)
    {
        return Err(invalid());
    }
    let mut reader = prefix.store.lock().await;
    reader.load_index()?;
    reader.record_view(&prefix.snapshot, &session.id)
}

pub(super) async fn full(mut session: Session) -> Result<Session, String> {
    let prefix = session.message_prefix.clone().ok_or_else(invalid)?;
    let mut reader = prefix.store.lock().await;
    reader.load_index()?;
    if !prefix
        .snapshot
        .owner_matches(&session.id, &session.source, &session.source_key)
    {
        return Err(invalid());
    }
    session.messages = body(&mut reader, &prefix.snapshot, &session.id, Kind::Messages)?;
    session.message_prefix = None;
    Ok(session)
}

pub(super) async fn history(
    session: &Session,
    request: &HistoryQuery,
) -> Result<HistorySource, String> {
    if request.kind == HistoryKind::Turns {
        return history_ops::history_source(&full(session.clone()).await?, request);
    }
    history_reader::live_message_source(session, view(session).await?, request)
}

pub(super) async fn window(
    session: &Session,
    request: &window::TimelineRequest,
    revision: u64,
) -> Result<window::SessionWindow, String> {
    let view = view(session).await?;
    let ids = view.ids(Kind::Messages)?;
    if ids.len() != count(session) {
        return Err(invalid());
    }
    let end = match request.before_message_id.as_deref() {
        Some(id) => ids
            .iter()
            .position(|original| original == id)
            .ok_or("WINDOW_CURSOR_NOT_FOUND: 原消息游标已不可读取，请刷新原会话")?,
        None => ids.len(),
    };
    let start = end.saturating_sub(request.limit);
    let mut row = session.clone();
    row.messages = if start == end {
        Records::default()
    } else {
        view.range(Kind::Messages, start, end - start)?
            .into_iter()
            .map(|v| serde_json::from_value(v).map_err(|_| invalid()))
            .collect::<Result<Vec<Message>, _>>()?
            .into()
    };
    // The first real input can be outside the page but still in the protected suffix.
    let mut goals = BTreeMap::new();
    for turn in &session.turns {
        if turn.workflow.plan.is_none() {
            if let Some(goal) = goal(session, &turn.id) {
                goals.insert(turn.id.clone(), goal.to_owned());
            } else if let Some(message) = session
                .messages
                .iter()
                .find(|m| m.role == "user" && m.turn_id.as_deref() == Some(&turn.id))
            {
                goals.insert(
                    turn.id.clone(),
                    message
                        .content
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(100)
                        .collect(),
                );
            }
        }
    }
    window::project(&row, request, revision, ids.len(), end, start, Some(&goals))
}
