//! Conservative cold conversation adapter. Cold data never grants task authority.
use super::*;
use crate::assistant_state_store::{Kind, RecordSlot, SeriesOverlay, Snapshot, Store, ARCHIVE_JOB_TURNS_KEY};
use std::collections::BTreeSet;
#[path = "assistant_archive_capacity.rs"]
mod capacity;
#[path = "assistant_archive_history.rs"]
mod history_reader;
#[path = "assistant_live_jobs.rs"]
pub(super) mod live_jobs;
#[path = "assistant_live_messages.rs"]
pub(super) mod live_messages;
#[path = "assistant_live_turn_replay.rs"]
pub(super) mod live_turn_replay;

pub(crate) fn enable_capacity(state: &mut State) { capacity::enable(state); }
pub(super) fn check_budget(state: &mut State) -> Result<(), String> { capacity::check_retained(state) }
pub(super) fn session_admission(state: &State) -> Result<(), String> { capacity::session_admission(state) }

pub(super) fn contains_registered_job(session: &Session, id: &str) -> bool {
    session.jobs.iter().any(|job| job.id == id)
        || session.cold.as_ref().is_some_and(|cold| cold.summary.job_ids.iter().any(|job_id| job_id == id))
}

pub(super) async fn cold_last_turn_uses_pi(session: &Session) -> Result<bool, String> {
    let cold = session.cold.as_ref().ok_or_else(invalid)?;
    let (turn_id, _) = cold.summary.receipts.last().ok_or_else(invalid)?;
    let mut reader = cold.store.lock().await;
    reader.load_index()?;
    Ok(archived_turn(&mut reader, session, cold, turn_id)?.settings.runtime == "pi")
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Summary {
    version: u32,
    pub(super) messages: usize,
    receipts: Vec<(String, String)>,
    pub(super) usage: Vec<Vec<wire::ModelUsage>>,
    extra_bytes: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    job_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    artifact_ids: Vec<String>,
}
#[derive(Clone)]
pub(super) struct Cold {
    snapshot: Snapshot,
    store: Arc<Mutex<Store>>,
    pub(super) summary: Summary,
    selection: tokio::sync::OnceCell<source_selection::ClosedHints>,
}
fn invalid() -> String {
    "ARCHIVE_INVALID: 冷会话索引与原记录不一致，未回退为空或重新执行任务".into()
}
pub(super) fn scoped_turn_session<'a>(
    state: &'a State,
    tid: &str,
    owner: Option<&str>,
    session_id: Option<&str>,
) -> Result<&'a Session, String> {
    let mut sessions = state.sessions.iter().filter(|session| {
        session.cold.as_ref().is_some_and(|cold| {
            cold.summary.receipts.iter().any(|(id, _)| id == tid)
        })
            || session.turns.iter().any(|turn| turn.id == tid)
    });
    let session = sessions.next().ok_or_else(|| {
        if session_id.is_some() {
            "HISTORY_TURN_NOT_FOUND: 原轮次不存在"
        } else {
            "未找到助理轮次"
        }
    })?;
    if sessions.next().is_some()
        || session.turns.iter().filter(|turn| turn.id == tid).nth(1).is_some()
    {
        return Err(invalid());
    }
    if owner.is_some_and(|key| key != session.source_key) {
        return Err("不能查询其他来源的助理轮次".into());
    }
    if session_id.is_some_and(|id| id != session.id) {
        return Err("HISTORY_SCOPE_MISMATCH: 原轮次不属于指定会话".into());
    }
    Ok(session)
}

pub(super) fn task_disabled(value: &u32) -> bool {
    *value == 0
}
pub(crate) fn enable_tasks(state: &mut State) {
    state.archive_task_version = 1;
}
fn hint_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}
fn ids(session: &Session) -> Vec<&str> {
    if let Some(cold) = &session.cold {
        cold.summary
            .receipts
            .iter()
            .map(|(id, _)| id.as_str())
            .collect()
    } else {
        session.turns.iter().map(|t| t.id.as_str()).collect()
    }
}
fn referenced(state: &State, session: &Session) -> bool {
    let ids = ids(session);
    // Pending or unverifiable controls retain the original target. Valid final
    // replies stay in the durable ledger and replay without a writable body.
    channel_control::references(&state.channel_controls, &session.id, &ids)
        || state
            .matters
            .entries
            .iter()
            .any(|e| e.turns.iter().any(|t| ids.contains(&t.as_str())))
        || state.followups.entries.iter().any(|e| {
            e.event_session_id.as_deref() == Some(session.id.as_str())
                || e.trigger
                    .as_ref()
                    .is_some_and(|t| ids.contains(&t.turn_id.as_str()))
                || e.runs
                    .iter()
                    .filter_map(|r| r.turn_id.as_deref())
                    .any(|t| ids.contains(&t))
        })
        || state.sessions.iter().flat_map(|s| s.turns.iter()).any(|t| {
            t.parent_turn_id
                .as_deref()
                .is_some_and(|id| ids.contains(&id))
                || t.steered_turn_id
                    .as_deref()
                    .is_some_and(|id| ids.contains(&id))
        })
}
fn summary(state: &State, s: &Session) -> Option<Summary> {
    if let Some(cold) = &s.cold {
        return Some(cold.summary.clone());
    }
    if referenced(state, s) {
        return None;
    }
    body_summary(s)
}
fn with_context(mut summary: Summary, s: &Session, context: bool) -> Option<Summary> {
    if context {
        summary.version += 1;
        summary.extra_bytes = summary
            .extra_bytes
            .checked_add(serde_json::to_vec(&s.history).ok()?.len().checked_sub(2)?)?;
    }
    Some(summary)
}
fn summary_for(state: &State, s: &Session, context: bool) -> Option<Summary> {
    if s.message_prefix.is_some() { return None; }
    if let Some(cold) = &s.cold {
        return (matches!(cold.summary.version, 1..=4)
            && (cold.summary.version % 2 == 0) == context
            && (cold.summary.version <= 2 || state.archive_task_version == 1))
            .then(|| cold.summary.clone());
    }
    let value = summary(state, s).or_else(|| {
        (state.archive_task_version == 1 && !referenced(state, s))
            .then(|| task_summary(s))
            .flatten()
    })?;
    with_context(value, s, context).filter(|summary| summary.extra_bytes <= capacity::RESIDENT_BYTES)
}
fn body_summary(s: &Session) -> Option<Summary> {
    if s.turns.is_empty()
        || s.queue_paused
        || !s.jobs.is_empty()
        || s.messages.iter().any(|m| !m.attachments.is_empty())
        || s.turns.iter().any(|turn| !closed_chat(turn))
    {
        return None;
    }
    Some(Summary {
        version: 1,
        messages: s.messages.len(),
        receipts: s
            .turns
            .iter()
            .map(|t| (t.id.clone(), t.request_id.clone()))
            .collect(),
        usage: s.turns.iter().map(|t| t.usage.clone()).collect(),
        extra_bytes: s.messages.encoded_extra_bytes().ok()?
            .checked_add(s.turns.encoded_extra_bytes().ok()?)?
            .checked_add(live_turn_replay::archived_bytes(s))?,
        job_ids: vec![],
        artifact_ids: vec![],
    })
}
fn settled_job(job: &Job) -> bool {
    settled_job_with_result(job, &job.result)
}

fn settled_job_with_result(job: &Job, result: &Value) -> bool {
    job.state == "succeeded"
        && !result.is_null()
        && !job.deliver
        && job.remote.as_ref().is_none_or(|r| r.finished && r.query_job.is_none())
        && result["partial"] != true
        && result["complete"] != false
        && result["status"]
            .as_str()
            .is_none_or(|s| matches!(s, "succeeded" | "completed" | "ready" | "success"))
}

fn closed_chat(turn: &Turn) -> bool {
    turn.state == "completed"
        && turn.interaction == "chat"
        && turn.steps.is_empty()
        && turn.pending.is_none()
        && turn.queued.is_empty()
        && turn.delivery_jobs.is_empty()
        && turn.control_intents.is_empty()
        && !turn.runtime_uncertain_model
        && turn.parent_turn_id.is_none()
        && turn.steered_turn_id.is_none()
        && turn.requirements.is_empty()
        && turn.workflow.plan.is_none()
}

fn closed_task(turn: &Turn) -> bool {
    turn.state == "completed"
        && matches!(turn.interaction.as_str(), "chat" | "task")
        && turn.history_committed
        && turn.finished_at.is_some()
        && turn.pending.is_none()
        && turn.queued.is_empty()
        && turn.delivery_jobs.is_empty()
        && turn.control_intents.is_empty()
        && !turn.runtime_uncertain_model
        && turn.parent_turn_id.is_none()
        && turn.steered_turn_id.is_none()
        && turn.requirements.is_empty()
        && !turn.workflow.unfinished()
        && turn.workflow.stop.is_none()
        && turn.requirements_revision <= turn.model_requirements_revision
        && turn.steps.iter().all(|step| step.state == "completed")
}

fn task_summary(s: &Session) -> Option<Summary> {
    if s.turns.is_empty()
        || s.queue_paused
        || s.messages.iter().any(|m| !m.attachments.is_empty())
        || s.turns.iter().any(|turn| !closed_task(turn))
        || s.jobs.iter().any(|j| !settled_job(j) || !s.turns.iter().any(|t| t.id == j.turn_id))
    {
        return None;
    }
    let mut jobs = BTreeSet::new();
    let mut artifacts = BTreeSet::new();
    for job in &s.jobs {
        if !hint_id(&job.id) || !jobs.insert(job.id.as_str()) {
            return None;
        }
        for item in &job.artifacts {
            if item.job_id != job.id
                || !hint_id(&item.id)
                || job.owned && !artifacts.insert(item.id.as_str())
            {
                return None;
            }
        }
    }
    if artifacts.len() > 4096
        || s.turns
            .iter()
            .flat_map(|t| &t.steps)
            .filter_map(|step| step.job_id.as_deref())
            .any(|id| !jobs.contains(id))
    {
        return None;
    }
    Some(Summary {
        version: 3,
        messages: s.messages.len(),
        receipts: s
            .turns
            .iter()
            .map(|t| (t.id.clone(), t.request_id.clone()))
            .collect(),
        usage: s.turns.iter().map(|t| t.usage.clone()).collect(),
        extra_bytes: s.messages.encoded_extra_bytes().ok()?
            .checked_add(s.turns.encoded_extra_bytes().ok()?)?
            .checked_add(live_turn_replay::archived_bytes(s))?
            .checked_add(s.jobs.encoded_extra_bytes().ok()?)?
            .checked_add(live_jobs::archived_bytes(s))?,
        job_ids: s.jobs.iter().map(|j| j.id.clone()).collect(),
        artifact_ids: artifacts.into_iter().map(str::to_string).collect(),
    })
}

fn indexed_job_turns<'a>(
    snapshot: &'a Snapshot,
    session: &str,
    summary: &Summary,
) -> Result<Option<&'a [Value]>, String> {
    let Some(index) = snapshot.archive_job_turns() else {
        return Ok(None);
    };
    let index = index.as_object().ok_or_else(invalid)?;
    let Some(positions) = index.get(session) else {
        return Ok(None);
    };
    let positions = positions.as_array().ok_or_else(invalid)?;
    if summary.version <= 2
        || positions.len() != summary.job_ids.len()
        || positions.iter().any(|offset| {
            offset
                .as_u64()
                .is_none_or(|offset| offset >= summary.receipts.len() as u64)
        })
    {
        return Err(invalid());
    }
    Ok(Some(positions))
}

fn verify_job_turns(snapshot: &Snapshot, session: &Session, summary: &Summary) -> Result<(), String> {
    if let Some(positions) = indexed_job_turns(snapshot, &session.id, summary)? {
        if session.jobs.len() != positions.len()
            || session.jobs.iter().zip(positions).any(|(job, position)| {
                position
                    .as_u64()
                    .and_then(|position| summary.receipts.get(position as usize))
                    .is_none_or(|(turn, _)| &job.turn_id != turn)
            })
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn validate(snapshot: &Snapshot, s: &Session, summary: &Summary) -> Result<(), String> {
    if !snapshot.owner_matches(&s.id, &s.source, &s.source_key)
        || !matches!(summary.version, 1..=4)
        || (summary.version % 2 == 0) != snapshot.context_segmented()
        || snapshot.context_segmented() && !s.history.is_empty()
        || summary.messages != snapshot.count(&s.id, Kind::Messages)?
        || summary.extra_bytes > 24 * 1024 * 1024
        || summary.receipts.len() != summary.usage.len()
        || summary.receipts.is_empty()
        || s.queue_paused
        || summary.job_ids != snapshot.record_ids(&s.id, Kind::Jobs)?
        || summary.version <= 2 && (!summary.job_ids.is_empty() || !summary.artifact_ids.is_empty())
        || summary.artifact_ids.len() > 4096
        || summary.artifact_ids.iter().any(|id| !hint_id(id))
        || summary
            .receipts
            .iter()
            .any(|(id, request)| id.is_empty() || request.is_empty() || request.len() > 120)
        || snapshot.record_ids(&s.id, Kind::Turns)?
            != summary
                .receipts
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
    {
        return Err(invalid());
    }
    indexed_job_turns(snapshot, &s.id, summary)?;
    Ok(())
}
fn body<T: serde::de::DeserializeOwned>(
    store: &mut Store,
    snapshot: &Snapshot,
    session: &str,
    kind: Kind,
) -> Result<Records<T>, String> {
    let mut rows = Vec::new();
    let mut bytes = 0usize;
    for offset in (0..snapshot.count(session, kind)?).step_by(64) {
        for value in store.read_range(snapshot, session, kind, offset, 64)? {
            bytes = bytes
                .checked_add(serde_json::to_vec(&value).map_err(|_| invalid())?.len())
                .ok_or_else(invalid)?;
            if bytes > 24 * 1024 * 1024 {
                return Err("ARCHIVE_LIMIT: 会话装载超过安全边界，未替换原记录".into());
            }
            rows.push(serde_json::from_value(value).map_err(|_| invalid())?);
        }
    }
    Ok(rows.into())
}
fn hydrate(store: &mut Store, snapshot: &Snapshot, shell: &Session) -> Result<Session, String> {
    let mut loaded = shell.clone();
    loaded.messages = body(store, snapshot, &shell.id, Kind::Messages)?;
    if snapshot.capacity_segmented() {
        live_turn_replay::load(&mut loaded, store, snapshot, live_turn_replay::LoadMode::Cached)?;
    } else {
        loaded.turns = body(store, snapshot, &shell.id, Kind::Turns)?;
    }
    loaded.jobs = body(store, snapshot, &shell.id, Kind::Jobs)?;
    loaded.job_result_source = None;
    if snapshot.context_segmented() {
        loaded.history = body::<Vec<Value>>(store, snapshot, &shell.id, Kind::Context)?.to_vec();
    }
    loaded.cold = None;
    Ok(loaded)
}
/// No body is read for eligible sessions. Existing indexes without projections
/// retain the previous full load path; no undocumented migration is inferred.
pub(crate) async fn load(store: Arc<Mutex<Store>>, snapshot: Snapshot) -> Result<State, String> {
    let mut state: State = serde_json::from_value(snapshot.metadata()).map_err(|_| invalid())?;
    state.archive_base = Some(snapshot.clone());
    if state.archive_task_version > 1
        || state.archive_task_version == 0 && state.archive_index.values().any(|s| s.version >= 3)
        || state.archive_capacity_version > 1
        || (state.archive_capacity_version == 1) != snapshot.capacity_segmented()
    {
        return Err(invalid());
    }
    if state
        .archive_index
        .keys()
        .any(|id| !state.sessions.iter().any(|s| s.id == *id))
    {
        return Err(invalid());
    }
    let mut reader = store.lock().await;
    // Enabling the new codec on an old index first hydrates all original bodies.
    // A later successful save can upgrade them; loading itself never commits.
    if reader.context_segments() && !snapshot.context_segmented() {
        return serde_json::from_value(reader.materialize(&snapshot)?).map_err(|_| invalid());
    }
    for i in 0..state.sessions.len() {
        let shell = &state.sessions[i];
        if !shell.messages.is_empty() || !shell.turns.is_empty() || !shell.jobs.is_empty() {
            return Err(invalid());
        }
        let cold = if let Some(summary) = state.archive_index.get(&shell.id) {
            validate(&snapshot, shell, summary)?;
            Some(Arc::new(Cold {
                snapshot: snapshot.clone(),
                store: store.clone(),
                summary: summary.clone(),
                selection: tokio::sync::OnceCell::new(),
            }))
        } else {
            None
        };
        if let Some(cold) = cold {
            state.sessions[i].cold = Some(cold);
            if referenced(&state, &state.sessions[i]) {
                let restored = hydrate(&mut reader, &snapshot, &state.sessions[i])?;
                verify_job_turns(&snapshot, &restored, &state.archive_index[&restored.id])?;
                state.sessions[i] = restored;
            }
        } else {
            state.sessions[i] = if snapshot.capacity_segmented() {
                live_messages::load(&mut reader, &snapshot, &state.sessions[i], store.clone())?
            } else { hydrate(&mut reader, &snapshot, &state.sessions[i])? };
        }
    }
    // A later hot session can contain a reference to an earlier cold session.
    for i in 0..state.sessions.len() {
        if state.sessions[i].cold.is_some() && referenced(&state, &state.sessions[i]) {
            let restored = hydrate(&mut reader, &snapshot, &state.sessions[i])?;
            verify_job_turns(&snapshot, &restored, &state.archive_index[&restored.id])?;
            state.sessions[i] = restored;
        }
    }
    if state.archive_capacity_version == 1 {
        for session in &mut state.sessions {
            if session.cold.is_none() && session.job_result_source.is_none() {
                let plan = live_jobs::plan(session)?;
                live_jobs::publish(session, plan, &snapshot, &reader);
            }
            checkpoint_records(session);
        }
    }
    capacity::check_loaded(&mut state)?;
    Ok(state)
}

fn checkpoint_records(session: &mut Session) {
    session.saved_jobs = (!session.jobs.is_empty()).then(|| Arc::new(session.jobs.clone()));
    session.saved_turns = (live_turn_replay::archived_bytes(session) != 0)
        .then(|| Arc::new(session.turns.clone()));
}

fn job_overlay(session: &Session, base: &Snapshot) -> Result<Option<SeriesOverlay>, String> {
    let Some(saved) = &session.saved_jobs else {
        return Ok(None);
    };
    let records = session
        .jobs
        .iter()
        .enumerate()
        .map(|(index, job)| {
            if session.jobs.shares_record(saved, index) {
                Ok(RecordSlot::Retained {
                    index,
                    id: job.id.clone(),
                })
            } else {
                let value = if job.archived_result_bytes.is_some() {
                    serde_json::to_value(live_jobs::restored(job, session.job_result_source.as_deref())?)
                } else {
                    serde_json::to_value(job)
                };
                value.map(RecordSlot::Owned).map_err(|_| invalid())
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Some(SeriesOverlay {
        session_id: session.id.clone(),
        kind: Kind::Jobs,
        snapshot: base.clone(),
        records,
    }))
}

fn job_turn_metadata(state: &State) -> Result<serde_json::Map<String, Value>, String> {
    // This is an optional lookup aid in the encrypted head. Keep its total size
    // bounded independently of the canonical job and turn records.
    const MAX_BYTES: usize = 256 * 1024;
    let mut bytes = 2;
    let mut index = serde_json::Map::new();
    for session in &state.sessions {
        let Some(summary) = state.archive_index.get(&session.id) else {
            continue;
        };
        if summary.job_ids.is_empty() {
            continue;
        }
        let positions = if let Some(cold) = &session.cold {
            let Some(positions) = indexed_job_turns(&cold.snapshot, &session.id, summary)? else {
                continue;
            };
            Value::Array(positions.to_vec())
        } else {
            let turns: BTreeMap<_, _> = summary
                .receipts
                .iter()
                .enumerate()
                .map(|(position, (id, _))| (id.as_str(), position))
                .collect();
            let positions = session
                .jobs
                .iter()
                .map(|job| turns.get(job.turn_id.as_str()).copied().ok_or_else(invalid))
                .collect::<Result<Vec<_>, _>>()?;
            serde_json::to_value(positions).map_err(|_| invalid())?
        };
        let charge = capacity::encoded_bytes(&session.id)? + capacity::encoded_bytes(&positions)? + 2;
        if bytes + charge <= MAX_BYTES {
            index.insert(session.id.clone(), positions);
            bytes += charge;
        }
    }
    Ok(index)
}

struct Publication {
    saved: crate::assistant_state_store::Saved,
    candidates: Vec<(usize, Summary)>,
    trims: Vec<(usize, live_messages::Trim)>,
    job_plans: Vec<live_jobs::Plan>,
    replay_plans: Vec<live_turn_replay::Plan>,
}

async fn persist_inner(
    store: &mut Store,
    state: &mut State,
    protected: &[String],
) -> Result<Publication, String> {
    if store.capacity_segments() != (state.archive_capacity_version == 1) {
        return Err(invalid());
    }
    for session in &mut state.sessions {
        if session.turn_replay_source.is_some() {
            live_turn_replay::restore_unsettled(session)?;
        }
    }
    state.archive_index = state
        .sessions
        .iter()
        .filter_map(|s| summary_for(&state, s, store.context_segments()).map(|v| (s.id.clone(), v)))
        .collect();
    if state.archive_capacity_version == 0 && logical_bytes(state)? > 24 * 1024 * 1024 {
        return Err("ARCHIVE_LIMIT: 完整热冷历史超过原24MiB边界，未保存或删除旧记录".into());
    }
    capacity::check_retained(state)?;
    let cold: Vec<_> = state
        .sessions
        .iter()
        .filter_map(|s| s.cold.as_ref().map(|c| (s.id.clone(), c.snapshot.clone())))
        .collect();
    let mut overlays = Vec::new();
    let mut job_positions = Vec::new();
    let mut turn_positions = Vec::new();
    if let Some(base) = state.archive_base.as_ref().filter(|_| state.archive_capacity_version == 1) {
        for (index, session) in state.sessions.iter().enumerate() {
            if let Some(overlay) = job_overlay(session, base)? {
                overlays.push(overlay);
                job_positions.push(index);
            }
            if live_turn_replay::archived_bytes(session) != 0 {
                overlays.push(live_turn_replay::overlay(session, base)?);
                turn_positions.push(index);
            }
        }
    }
    if state.sessions.iter().enumerate().any(|(index, session)| {
        live_jobs::archived_bytes(session) != 0 && job_positions.binary_search(&index).is_err()
            || live_turn_replay::archived_bytes(session) != 0 && turn_positions.binary_search(&index).is_err()
    }) {
        return Err(invalid());
    }
    // The store resolves retained slots from the pinned base. Keep the full
    // records in State for authorized readers and restore them before any error.
    let parked_jobs: Vec<_> = job_positions
        .into_iter()
        .map(|index| (index, std::mem::take(&mut state.sessions[index].jobs)))
        .collect();
    let parked_turns: Vec<_> = turn_positions
        .into_iter()
        .map(|index| (index, std::mem::take(&mut state.sessions[index].turns)))
        .collect();
    let encoded = serde_json::to_value(&state);
    for (index, jobs) in parked_jobs {
        state.sessions[index].jobs = jobs;
    }
    for (index, turns) in parked_turns {
        state.sessions[index].turns = turns;
    }
    let mut value = encoded.map_err(|_| invalid())?;
    let mut trims = Vec::new();
    if state.archive_capacity_version == 1 {
        let protected: BTreeSet<_> = protected.iter().map(String::as_str).collect();
        for (index, session) in state.sessions.iter_mut().enumerate() {
            if session.cold.is_some()
                || (state.archive_index.contains_key(&session.id)
                    && !protected.contains(session.id.as_str()))
            {
                continue;
            }
            if let Some(overlay) = live_messages::overlay(session, &mut value["sessions"][index])? {
                overlays.push(overlay);
            }
            if let Some(trim) = live_messages::trim(session)? {
                trims.push((index, trim));
                state.archive_index.remove(&session.id);
            }
        }
        if state.archive_index.is_empty() {
            value.as_object_mut().ok_or_else(invalid)?.remove("archive_index");
        } else {
            value["archive_index"] =
                serde_json::to_value(&state.archive_index).map_err(|_| invalid())?;
        }
    }
    let job_turns = job_turn_metadata(state)?;
    if !job_turns.is_empty() {
        value[ARCHIVE_JOB_TURNS_KEY] = Value::Object(job_turns);
    }
    let mut added_hints = 0usize;
    for (index, trim) in &trims {
        let previous = live_messages::hint_bytes(&state.sessions[*index])?;
        added_hints = added_hints
            .checked_add(trim.hint_bytes()?.saturating_sub(previous))
            .ok_or_else(invalid)?;
    }
    let job_plans = if state.archive_capacity_version == 1 {
        state.sessions.iter().map(live_jobs::plan).collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let replay_plans = if state.archive_capacity_version == 1 {
        state.sessions.iter().map(live_turn_replay::plan).collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };
    let candidates = capacity::candidates(state, protected, added_hints, &job_plans, &replay_plans)?;
    let saved = if overlays.is_empty() {
        store.save_partition(value, &cold).await?
    } else {
        let base = state.archive_base.as_ref().ok_or_else(invalid)?;
        store.save_record_partition(value, base, &cold, &overlays).await?
    };
    Ok(Publication { saved, candidates, trims, job_plans, replay_plans })
}
#[cfg(test)]
pub(crate) async fn persist(store: &mut Store, mut state: State) -> Result<(), String> {
    if state.archive_capacity_version != 0 { return Err("ARCHIVE_MODE: 容量格式需要提交投影，未写入旧保存器".into()); }
    persist_inner(store, &mut state, &[]).await?;
    Ok(())
}
pub(crate) async fn commit(
    store: Arc<Mutex<Store>>,
    mut state: State,
    protected: Vec<String>,
) -> Result<State, String> {
    let mut writer = store.lock().await;
    let Publication { saved, candidates, trims, job_plans, replay_plans } =
        persist_inner(&mut writer, &mut state, &protected).await?;
    state.archive_base = Some(saved.snapshot.clone());
    live_messages::publish(&mut state, trims, &saved.snapshot, &store);
    // The saved snapshot is returned by the successful head publication itself;
    // no fallible second disk read can turn this commit into an ambiguous error.
    for (position, summary) in candidates {
        {
            let session = &mut state.sessions[position];
            session.messages = Records::default();
            session.turns = Records::default();
            session.jobs = Records::default();
            session.history.clear();
            session.cold = Some(Arc::new(Cold { snapshot: saved.snapshot.clone(), store: store.clone(), summary, selection:tokio::sync::OnceCell::new() }));
        }
    }
    for ((session, job_plan), replay_plan) in state.sessions.iter_mut().zip(job_plans).zip(replay_plans) {
        if session.cold.is_none() {
            live_jobs::publish(session, job_plan, &saved.snapshot, &writer);
            live_turn_replay::publish(session, replay_plan, &saved.snapshot, &writer);
        } else {
            session.job_result_source = None;
            session.turn_replay_source = None;
        }
        checkpoint_records(session);
    }
    Ok(state)
}
pub(super) fn logical_bytes(state: &State) -> Result<usize, String> {
    logical_bytes_for(state, capacity::encoded_bytes(state)?)
}

fn logical_bytes_for(state: &State, mut bytes: usize) -> Result<usize, String> {
    for cold in state.sessions.iter().filter_map(|s| s.cold.as_ref()) {
        bytes = bytes
            .checked_add(cold.summary.extra_bytes)
            .ok_or_else(invalid)?;
    }
    for prefix in state.sessions.iter().filter_map(|s| s.message_prefix.as_ref()) {
        bytes = bytes.checked_add(prefix.bytes()).ok_or_else(invalid)?;
    }
    for session in &state.sessions {
        bytes = bytes
            .checked_add(live_jobs::archived_bytes(session))
            .and_then(|bytes| bytes.checked_add(live_turn_replay::archived_bytes(session)))
            .ok_or_else(invalid)?;
    }
    Ok(bytes)
}
pub(crate) fn ensure_legacy(state: &State) -> Result<(), String> {
    if state.archive_capacity_version != 0 || state.sessions.iter().any(|s| s.cold.is_some()) {
        Err("ARCHIVE_INVALID: 冷会话不能写入旧格式，原记录未覆盖".into())
    } else {
        Ok(())
    }
}

/// Freeze only the records required by the history contract. The resulting
/// source is read-only data; neither it nor its hints grant execution authority.
pub(super) async fn history_source(
    shell: &Session,
    request: &crate::assistant_history::HistoryQuery,
) -> Result<Option<crate::assistant_history::HistorySource>, String> {
    use crate::assistant_history::HistoryKind;
    if shell.message_prefix.is_some() {
        return live_messages::history(shell, request).await.map(Some);
    }
    let Some(cold) = &shell.cold else {
        return Ok(None);
    };
    validate(&cold.snapshot, shell, &cold.summary)?;
    if request.kind == HistoryKind::Messages || cold.summary.version <= 2 {
        return history_reader::source(shell, cold, request).await.map(Some);
    }
    let ids: Vec<_> = cold
        .summary
        .receipts
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    history_ops::history_turn_count(&ids, request.through_turn_id.as_deref())?;
    let mut reader = cold.store.lock().await;
    reader.load_index()?;
    let mut partial = empty_session(&shell.source, &shell.source_key, &shell.id);
    partial.title = shell.title.clone();
    partial.updated_at = shell.updated_at.clone();
    partial.messages = body(&mut reader, &cold.snapshot, &shell.id, Kind::Messages)?;
    if partial.messages.iter().any(|m| !m.attachments.is_empty()) {
        return Err(invalid());
    }
    if request.kind == HistoryKind::Turns {
        partial.turns = body(&mut reader, &cold.snapshot, &shell.id, Kind::Turns)?;
        partial.jobs = body(&mut reader, &cold.snapshot, &shell.id, Kind::Jobs)?;
        verify_job_turns(&cold.snapshot, &partial, &cold.summary)?;
        let mut actual = (if cold.summary.version >= 3 {
            task_summary(&partial)
        } else {
            body_summary(&partial)
        })
        .and_then(|s| with_context(s, &partial, cold.snapshot.context_segmented()))
        .ok_or_else(invalid)?;
        // Context is deliberately not read for history. Validate the complete
        // requested message/turn/job projection, without claiming context bytes
        // were checked. Full execution/recovery retains full hydration checks.
        if cold.snapshot.context_segmented() {
            if actual.extra_bytes > cold.summary.extra_bytes {
                return Err(invalid());
            }
            actual.extra_bytes = cold.summary.extra_bytes;
        }
        if serde_json::to_value(actual).map_err(|_| invalid())?
            != serde_json::to_value(&cold.summary).map_err(|_| invalid())?
        {
            return Err(invalid());
        }
    }
    history_ops::history_source_ordered(&partial, request, &ids).map(Some)
}

/// Pure chats use bounded ranges. Closed tasks validate a complete temporary
/// body so job ownership is never inferred from hints. Neither enters State.
pub(super) async fn session_window(
    shell: &Session,
    request: &window::TimelineRequest,
    revision: u64,
) -> Result<Option<window::SessionWindow>, String> {
    if shell.message_prefix.is_some() {
        return live_messages::window(shell, request, revision).await.map(Some);
    }
    let Some(cold) = &shell.cold else {
        return Ok(None);
    };
    if cold.summary.version >= 3 {
        let snapshot = read_session(shell.clone()).await?;
        return window::full_snapshot(&snapshot, request, revision).map(Some);
    }
    validate(&cold.snapshot, shell, &cold.summary)?;
    let message_ids = cold.snapshot.record_ids(&shell.id, Kind::Messages)?;
    let end = match request.before_message_id.as_deref() {
        Some(id) => message_ids.iter().position(|m| m == id).ok_or(
            "WINDOW_CURSOR_NOT_FOUND: 原消息游标已不可读取，请刷新原会话；未跳过或清空历史",
        )?,
        None => message_ids.len(),
    };
    let start = end.saturating_sub(request.limit);
    let mut reader = cold.store.lock().await;
    reader.load_index()?;
    let mut partial = empty_session(&shell.source, &shell.source_key, &shell.id);
    partial.title = shell.title.clone();
    partial.updated_at = shell.updated_at.clone();
    if start < end {
        partial.messages = reader
            .read_range(
                &cold.snapshot,
                &shell.id,
                Kind::Messages,
                start,
                end - start,
            )?
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(|_| invalid()))
            .collect::<Result<Vec<Message>, _>>()?
            .into();
    }
    let wanted: BTreeSet<_> = partial
        .messages
        .iter()
        .filter_map(|m| m.turn_id.as_deref())
        .collect();
    let mut turns = Vec::new();
    for (offset, (id, receipt)) in cold.summary.receipts.iter().enumerate() {
        if !wanted.contains(id.as_str()) {
            continue;
        }
        let row = reader.read_range(&cold.snapshot, &shell.id, Kind::Turns, offset, 1)?;
        let turn: Turn = serde_json::from_value(row.into_iter().next().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        if turn.id != *id
            || turn.request_id != *receipt
            || serde_json::to_value(&turn.usage).map_err(|_| invalid())?
                != serde_json::to_value(cold.summary.usage.get(offset).ok_or_else(invalid)?)
                    .map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        turns.push(turn);
    }
    partial.turns = turns.into();
    if partial.messages.iter().any(|m| !m.attachments.is_empty())
        || !partial.turns.is_empty() && body_summary(&partial).is_none()
    {
        return Err(invalid());
    }
    // Progress uses the earliest user input of each visible turn, which can be
    // outside this window. Resolve it in bounded batches without adopting a
    // partial Session or replacing the goal with a later message's text.
    let mut pending: BTreeSet<_> = partial.turns.iter().map(|t| t.id.clone()).collect();
    let mut goals: BTreeMap<String, String> = pending
        .iter()
        .map(|id| (id.clone(), String::new()))
        .collect();
    for offset in (0..message_ids.len()).step_by(32) {
        if pending.is_empty() {
            break;
        }
        for value in reader.read_range(&cold.snapshot, &shell.id, Kind::Messages, offset, 32)? {
            let message: Message = serde_json::from_value(value).map_err(|_| invalid())?;
            if message.role == "user" {
                if let Some(id) = message.turn_id.filter(|id| pending.remove(id)) {
                    goals.insert(
                        id,
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
    }
    window::project(
        &partial,
        request,
        revision,
        message_ids.len(),
        end,
        start,
        Some(&goals),
    )
    .map(Some)
}

impl Engine {
    pub(super) async fn read_archive_receipt(
        &self,
        request: &str,
        source: &str,
        key: &str,
    ) -> Result<Option<(Ack, String)>, String> {
        let shell = {
            let state = self.state.lock().await;
            let mut found = None;
            for session in state.sessions.iter().filter(|s| s.source == source && s.source_key == key) {
                let count = if let Some(cold) = &session.cold {
                    cold.summary.receipts.iter().filter(|(_, rid)| rid == request).count()
                } else { session.turns.iter().filter(|t| t.request_id == request).count() };
                if count > 1 || count == 1 && found.is_some() { return Err(invalid()); }
                if count == 1 { found = Some(session); }
            }
            let Some(shell) = found else { return Ok(None); };
            if shell.cold.is_none() { return receipt(shell, request).map(Some); }
            shell.clone()
        };
        let cold = shell.cold.as_ref().ok_or_else(invalid)?;
        let (id, _) = cold.summary.receipts.iter()
            .find(|(_, rid)| rid == request)
            .ok_or_else(invalid)?;
        let mut reader = cold.store.lock().await;
        reader.load_index()?;
        let turn = archived_turn(&mut reader, &shell, cold, id)?;
        Ok(Some((Ack {
            session_id: shell.id.clone(),
            turn_id: turn.id,
            state: turn.state,
        }, turn.fingerprint)))
    }
    /// Read an immutable query snapshot. Never adopt it as writable State: a
    /// concurrent mutation may have committed after the shell was copied.
    pub(super) async fn read_archive_session(&self, id: &str) -> Result<Session, String> {
        let shell = {
            let state = self.state.lock().await;
            let mut matches = state.sessions.iter().filter(|s| s.id == id);
            let shell = matches.next().ok_or("未找到会话")?;
            if matches.next().is_some() { return Err(invalid()); }
            shell.clone()
        };
        read_session(shell).await
    }
    pub(super) async fn read_cold_session(&self, id: &str) -> Result<Option<Session>, String> {
        let shell = {
            let state = self.state.lock().await;
            let mut matches = state.sessions.iter().filter(|s| s.id == id);
            let Some(shell) = matches.next() else { return Ok(None); };
            if matches.next().is_some() { return Err(invalid()); }
            if shell.cold.is_none() { return Ok(None); }
            shell.clone()
        };
        read_session(shell).await.map(Some)
    }
    async fn cold_turn_shell(
        &self,
        tid: &str,
        owner: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Option<Session>, String> {
        let state = self.state.lock().await;
        let shell = scoped_turn_session(&state, tid, owner, session_id)?;
        // Hot callers borrow current state without cloning the model history.
        Ok(shell.cold.is_some().then(|| shell.clone()))
    }

    pub(super) async fn read_cold_step(
        &self,
        session_id: &str,
        tid: &str,
        step_id: &str,
    ) -> Result<Option<Step>, String> {
        let Some(shell) = self.cold_turn_shell(tid, None, Some(session_id)).await? else {
            return Ok(None);
        };
        let cold = shell.cold.as_ref().ok_or_else(invalid)?;
        let mut reader = cold.store.lock().await;
        reader.load_index()?;
        let offset = archived_turn_offset(&shell, cold, tid)?;
        reader.inspect_record_at(&cold.snapshot, &shell.id, Kind::Turns, offset, |row| {
            let steps = row["steps"].as_array().ok_or_else(invalid)?;
            let view = project_archived_turn(row, steps, false)?;
            validate_archived_turn(&view, cold, offset)?;
            let position = step_position(&view.steps, step_id)?;
            Step::deserialize(&steps[position])
                .map(Some)
                .map_err(|_| invalid())
        })
    }

    pub(super) async fn read_cold_turn(
        &self,
        tid: &str,
        owner: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Option<Session>, String> {
        self.read_cold_turn_with_jobs(tid, owner, session_id, true)
            .await
    }

    /// Partial turn and job results are only used for a read-only presentation.
    /// A full result read or activation still uses the archived originals.
    pub(super) async fn read_cold_turn_with_jobs(
        &self,
        tid: &str,
        owner: Option<&str>,
        session_id: Option<&str>,
        include_results: bool,
    ) -> Result<Option<Session>, String> {
        let Some(shell) = self.cold_turn_shell(tid, owner, session_id).await? else {
            return Ok(None);
        };
        let cold = shell.cold.as_ref().ok_or_else(invalid)?;
        let mut reader = cold.store.lock().await;
        reader.load_index()?;
        let turn = if include_results {
            archived_turn(&mut reader, &shell, cold, tid)?
        } else {
            let offset = archived_turn_offset(&shell, cold, tid)?;
            reader.inspect_record_at(&cold.snapshot, &shell.id, Kind::Turns, offset, |row| {
                let turn = if row.is_array() {
                    Turn::deserialize(row).map_err(|_| invalid())?
                } else {
                    let steps = row["steps"].as_array().ok_or_else(invalid)?;
                    project_archived_turn(row, steps, true)?
                };
                validate_archived_turn(&turn, cold, offset)?;
                Ok(turn)
            })?
        };
        let step_jobs: BTreeSet<_> = turn.steps.iter()
            .filter_map(|step| step.job_id.as_deref())
            .chain(turn.steps.iter().filter(|step| step.name == "homebase_deliver")
                .filter_map(|step| step.result["job_id"].as_str()))
            .collect();
        let positions = indexed_job_turns(&cold.snapshot, &shell.id, &cold.summary)?;
        let mut jobs = Vec::new();
        for (offset, expected_id) in cold.summary.job_ids.iter().enumerate() {
            let indexed_owner = positions
                .and_then(|positions| positions[offset].as_u64())
                .map(|position| cold.summary.receipts[position as usize].0.as_str());
            if indexed_owner.is_some_and(|owner| owner != tid)
                && !step_jobs.contains(expected_id.as_str())
            {
                continue;
            }
            let job = if include_results {
                let row = archived_row(&mut reader, &cold.snapshot, &shell.id, Kind::Jobs, offset)?;
                archived_job(row, expected_id, &cold.summary)?
            } else {
                reader.inspect_record_at(&cold.snapshot, &shell.id, Kind::Jobs, offset, |row| {
                    project_archived_job(row, expected_id, &cold.summary)
                })?
            };
            if indexed_owner.is_some_and(|owner| owner != job.turn_id) {
                return Err(invalid());
            }
            if turn_has_job(&turn, &job) || step_jobs.contains(job.id.as_str()) {
                jobs.push(job);
            }
        }
        let mut partial = empty_session(&shell.source, &shell.source_key, &shell.id);
        if turn.workflow.plan.is_none() {
            for offset in 0..cold.summary.messages {
                let row = archived_row(&mut reader, &cold.snapshot, &shell.id, Kind::Messages, offset)?;
                let message: Message = serde_json::from_value(row).map_err(|_| invalid())?;
                if !message.attachments.is_empty() {
                    return Err(invalid());
                }
                if message.turn_id.as_deref() == Some(tid) && message.role == "user" {
                    partial.messages = vec![message].into();
                    break;
                }
            }
        }
        partial.title = shell.title.clone();
        partial.updated_at = shell.updated_at.clone();
        partial.queue_paused = shell.queue_paused;
        partial.queue_resume_generation = shell.queue_resume_generation;
        partial.jobs = jobs.into();
        partial.turns = vec![turn].into();
        Ok(Some(partial))
    }
    pub(super) async fn ensure_archive_session(&self, id: &str) -> Result<Option<archive_leases::Lease>, String> {
        let mut state = self.state.lock().await;
        let Some(i) = state.sessions.iter().position(|s| s.id == id) else {
            return Ok(None);
        };
        let lease = self.archive_leases.pin(id)?;
        if state.sessions[i].cold.is_some() {
            let mut loaded = read_session(state.sessions[i].clone()).await?;
            if state.archive_capacity_version == 1 {
                live_messages::retain_restored_prefix(&state.sessions[i], &mut loaded)?;
                let cold = state.sessions[i].cold.as_ref().ok_or_else(invalid)?;
                let plan = live_jobs::plan(&loaded)?;
                let reader = cold.store.lock().await;
                live_jobs::publish(&mut loaded, plan, &cold.snapshot, &reader);
                checkpoint_records(&mut loaded);
            }
            capacity::check_restore(&mut state, i, &loaded)?;
            state.sessions[i] = loaded;
        }
        Ok(Some(lease))
    }
    pub(super) async fn ensure_archive_turn(
        &self,
        tid: &str,
        owner: Option<&str>,
    ) -> Result<Option<archive_leases::Lease>, String> {
        let sid = {
            let state = self.state.lock().await;
            let matches: Vec<_> = state
                .sessions
                .iter()
                .filter(|s| {
                    s.cold
                        .as_ref()
                        .is_some_and(|c| c.summary.receipts.iter().any(|(id, _)| id == tid))
                        || s.turns.iter().any(|t| t.id == tid)
                })
                .collect();
            if matches.len() > 1 {
                return Err(invalid());
            }
            match matches.first() {
                Some(s) => {
                    if owner.is_some_and(|key| key != s.source_key) {
                        return Err("不能查询其他来源的助理轮次".into());
                    }
                    Some(s.id.clone())
                }
                None => None,
            }
        };
        if let Some(sid) = sid {
            return self.ensure_archive_session(&sid).await;
        }
        Ok(None)
    }
    #[cfg(test)]
    pub(super) async fn ensure_archive_request(
        &self,
        request: &str,
        key: &str,
    ) -> Result<Vec<archive_leases::Lease>, String> {
        let sessions: Vec<_> = self
            .state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| {
                s.source_key == key
                    && (s.cold
                        .as_ref()
                        .is_some_and(|c| c.summary.receipts.iter().any(|(_, rid)| rid == request))
                        || s.turns.iter().any(|t| t.request_id == request))
            })
            .map(|s| s.id.clone())
            .collect();
        let mut leases = Vec::new();
        for id in sessions {
            leases.extend(self.ensure_archive_session(&id).await?);
        }
        Ok(leases)
    }
    #[cfg(test)]
    pub(super) async fn ensure_archive_source(&self, key: &str) -> Result<Vec<archive_leases::Lease>, String> {
        let sessions: Vec<_> = self
            .state
            .lock()
            .await
            .sessions
            .iter()
            .filter(|s| s.source_key == key)
            .map(|s| s.id.clone())
            .collect();
        let mut leases = Vec::new();
        for id in sessions {
            leases.extend(self.ensure_archive_session(&id).await?);
        }
        Ok(leases)
    }
}

/// The archive summaries select where to look. Only authenticated job bodies
/// can establish ownership of the artifact or a collision with its parent.
pub(super) async fn registered_artifact(
    state: &State,
    id: &str,
) -> Result<Option<Artifact>, String> {
    let cold = state
        .sessions
        .iter()
        .filter_map(|session| session.cold.as_ref().map(|cold| (session, cold)));
    let mut archived = None;
    let mut checked = BTreeSet::new();
    for (session, cold) in cold.clone().filter(|(_, cold)| {
        cold.summary.artifact_ids.iter().any(|artifact| artifact == id)
    }) {
        validate(&cold.snapshot, session, &cold.summary)?;
        let mut reader = cold.store.lock().await;
        if checked.insert(Arc::as_ptr(&cold.store) as usize) {
            reader.load_index()?;
        }
        let mut matched = false;
        for (offset, expected_id) in cold.summary.job_ids.iter().enumerate() {
            let value = archived_row(&mut reader, &cold.snapshot, &session.id, Kind::Jobs, offset)?;
            let job = archived_job(value, expected_id, &cold.summary)?;
            if job_access::artifact([&job], id)?.is_some() {
                if archived.is_some() {
                    return Err("ARTIFACT_IDENTITY_CONFLICT: 产物编号重复，未选择任一文件".into());
                }
                archived = Some((&session.id, job));
                matched = true;
            }
        }
        if !matched {
            return Err(invalid());
        }
    }
    let hot = state.sessions.iter().flat_map(|session| session.jobs.iter());
    let jobs = hot.chain(archived.as_ref().map(|(_, job)| job));
    let Some(artifact) = job_access::artifact(jobs, id)? else {
        return Ok(None);
    };

    for (session, cold) in cold.filter(|(session, cold)| {
        archived.as_ref().is_none_or(|(owner, _)| *owner != &session.id)
            && cold.summary.job_ids.contains(&artifact.job_id)
    }) {
        validate(&cold.snapshot, session, &cold.summary)?;
        let mut reader = cold.store.lock().await;
        if checked.insert(Arc::as_ptr(&cold.store) as usize) {
            reader.load_index()?;
        }
        let value = reader.read_record(&cold.snapshot, &session.id, Kind::Jobs, &artifact.job_id)?;
        let job = archived_job(value, &artifact.job_id, &cold.summary)?;
        if job.owned {
            return Err("ARTIFACT_IDENTITY_CONFLICT: 产物的父任务编号重复，未猜测归属".into());
        }
    }
    Ok(Some(artifact.clone()))
}

fn archived_job(value: Value, id: &str, summary: &Summary) -> Result<Job, String> {
    let job: Job = serde_json::from_value(value).map_err(|_| invalid())?;
    validate_archived_job(&job, &job.result, id, summary)?;
    Ok(job)
}

fn project_archived_job(row: &Value, id: &str, summary: &Summary) -> Result<Job, String> {
    let fields = row.as_object().ok_or_else(invalid)?;
    let result = &row["result"];
    // Keep only the evidence needed by progress. Validate the original below.
    let evidence = if workflow::result_outcome_unknown(result) {
        json!({"status":"unknown"})
    } else if workflow::result_blocks_step(result) {
        json!({"ok":false})
    } else {
        Value::Bool(true)
    };
    let fields = fields.iter().map(|(key, value)| {
        (key.as_str(), if key == "result" { &evidence } else { value })
    });
    let job = Job::deserialize(serde::de::value::MapDeserializer::new(fields))
        .map_err(|_| invalid())?;
    validate_archived_job(&job, result, id, summary)?;
    Ok(job)
}

fn validate_archived_job(
    job: &Job,
    result: &Value,
    id: &str,
    summary: &Summary,
) -> Result<(), String> {
    if job.id != id
        || !settled_job_with_result(job, result)
        || !summary.receipts.iter().any(|(turn, _)| turn == &job.turn_id)
        || job.artifacts.iter().any(|artifact| {
            artifact.job_id != job.id
                || !hint_id(&artifact.id)
                || job.owned && !summary.artifact_ids.contains(&artifact.id)
        })
    {
        return Err(invalid());
    }
    Ok(())
}

fn archived_row(
    reader: &mut Store,
    snapshot: &Snapshot,
    session: &str,
    kind: Kind,
    offset: usize,
) -> Result<Value, String> {
    reader.read_range(snapshot, session, kind, offset, 1)?.pop().ok_or_else(invalid)
}

fn archived_turn(
    reader: &mut Store,
    shell: &Session,
    cold: &Cold,
    id: &str,
) -> Result<Turn, String> {
    let offset = archived_turn_offset(shell, cold, id)?;
    let row = archived_row(reader, &cold.snapshot, &shell.id, Kind::Turns, offset)?;
    let turn: Turn = serde_json::from_value(row).map_err(|_| invalid())?;
    validate_archived_turn(&turn, cold, offset)?;
    Ok(turn)
}

fn archived_turn_offset(shell: &Session, cold: &Cold, id: &str) -> Result<usize, String> {
    validate(&cold.snapshot, shell, &cold.summary)?;
    cold.summary
        .receipts
        .iter()
        .position(|(turn_id, _)| turn_id == id)
        .ok_or_else(invalid)
}

fn validate_archived_turn(turn: &Turn, cold: &Cold, offset: usize) -> Result<(), String> {
    let (id, request) = &cold.summary.receipts[offset];
    let closed = if cold.summary.version <= 2 {
        closed_chat(turn)
    } else {
        closed_task(turn)
    };
    if !closed
        || &turn.id != id
        || &turn.request_id != request
        || serde_json::to_value(&turn.usage).map_err(|_| invalid())?
            != serde_json::to_value(&cold.summary.usage[offset]).map_err(|_| invalid())?
        || turn.steps.iter().filter_map(|step| step.job_id.as_ref())
            .any(|job| !cold.summary.job_ids.contains(job))
    {
        return Err(invalid());
    }
    Ok(())
}

fn project_step_result(name: Option<&str>, result: &Value, include_progress: bool) -> Value {
    if !include_progress || result.is_null() {
        return Value::Null;
    }
    if workflow::result_blocks_step(result) {
        return json!({"ok":false});
    }
    // Keep the small facts used to revalidate completed phases. The complete
    // result remains in its original encrypted step record.
    let mut evidence = json!({});
    if name == Some("homebase_terminal") && result["result"]["exit_code"] == 0 {
        evidence["result"]["exit_code"] = json!(0);
    }
    if workflow::artifact_evidence(result) {
        evidence["result"]["artifact"] = json!({});
    }
    if name == Some("homebase_deliver") {
        if let Some(job) = result["job_id"].as_str() {
            evidence["job_id"] = json!(job);
        }
    }
    evidence
}

fn project_archived_turn(row: &Value, steps: &[Value], include_progress: bool) -> Result<Turn, String> {
    let mut headers = Vec::with_capacity(steps.len());
    for step in steps {
        let header = if let Some(fields) = step.as_object() {
            let proof = project_step_result(step["name"].as_str(), &step["result"], include_progress);
            let fields = fields.iter().map(|(key, value)| {
                let value = match key.as_str() {
                    "arguments" if !include_progress => &Value::Null,
                    "result" => &proof,
                    _ => value,
                };
                (key.as_str(), value)
            });
            Step::deserialize(serde::de::value::MapDeserializer::new(fields))
                .map_err(|_| invalid())?
        } else {
            let mut legacy = Step::deserialize(step).map_err(|_| invalid())?;
            if !include_progress {
                legacy.arguments = Value::Null;
            }
            legacy.result = project_step_result(Some(&legacy.name), &legacy.result, include_progress);
            legacy
        };
        headers.push(header);
    }
    let empty_array = json!([]);
    let empty_object = json!({});
    let empty_text = json!("");
    let fields = row.as_object().ok_or_else(invalid)?;
    let fields = fields.iter().map(|(key, value)| {
        // The other fields still pass through the original Turn deserializer.
        // Keep invalid shapes so the same deserializer rejects damaged rows.
        let value = match key.as_str() {
            "steps" => &empty_array,
            "protocol" | "runtime_operations" | "model_diagnostics" if value.is_array() => {
                &empty_array
            }
            "runtime_results" | "runtime_models" if value.is_object() => &empty_object,
            "runtime_prompt" if value.is_string() => &empty_text,
            "text" if !include_progress && value.is_string() => &empty_text,
            _ => value,
        };
        (key.as_str(), value)
    });
    let mut turn = Turn::deserialize(serde::de::value::MapDeserializer::new(fields))
        .map_err(|_| invalid())?;
    turn.steps = headers;
    Ok(turn)
}

async fn read_session(shell: Session) -> Result<Session, String> {
    let mut shell = if shell.message_prefix.is_some() { live_messages::full(shell).await? } else { shell };
    let Some(cold) = &shell.cold else {
        if shell.job_result_source.is_some()
            && shell.jobs.encoded_extra_bytes().map_err(|_| invalid())?
                .checked_add(live_jobs::archived_bytes(&shell)).ok_or_else(invalid)? > capacity::RESIDENT_BYTES
        {
            return Err("ARCHIVE_LIMIT: 完整任务正文超过单次读取上限，请按轮次查询；原档案保留".into());
        }
        live_jobs::restore(&mut shell.jobs, shell.job_result_source.as_deref())?;
        shell.job_result_source = None;
        return Ok(shell);
    };
    validate(&cold.snapshot, &shell, &cold.summary)?;
    let mut reader = cold.store.lock().await;
    reader.load_index()?; // Detect concurrent legacy writes, including cached bodies.
    let loaded = hydrate(&mut reader, &cold.snapshot, &shell)?;
    verify_job_turns(&cold.snapshot, &loaded, &cold.summary)?;
    let actual = (if cold.summary.version >= 3 {
        task_summary(&loaded)
    } else {
        body_summary(&loaded)
    })
    .and_then(|v| with_context(v, &loaded, cold.snapshot.context_segmented()))
    .ok_or_else(invalid)?;
    if serde_json::to_value(actual).map_err(|_| invalid())?
        != serde_json::to_value(&cold.summary).map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    Ok(loaded)
}

fn receipt(session: &Session, request: &str) -> Result<(Ack, String), String> {
    let mut matches = session.turns.iter().filter(|t| t.request_id == request);
    let turn = matches.next().ok_or_else(invalid)?;
    if matches.next().is_some() { return Err(invalid()); }
    Ok((Ack { session_id: session.id.clone(), turn_id: turn.id.clone(), state: turn.state.clone() }, turn.fingerprint.clone()))
}

pub(super) fn selection_receipts(session:&Session)->Vec<(&str,&str)>{
    if let Some(cold)=&session.cold {cold.summary.receipts.iter().map(|(id,rid)|(id.as_str(),rid.as_str())).collect()}
    else {session.turns.iter().map(|t|(t.id.as_str(),t.request_id.as_str())).collect()}
}
pub(super) fn selection_probe(session:&Session)->Option<Session>{
    let cold=session.cold.as_ref()?;
    let mut probe=empty_session(&session.source,&session.source_key,&session.id);
    probe.updated_at=session.updated_at.clone();
    probe.queue_paused=session.queue_paused;
    probe.queue_resume_generation=session.queue_resume_generation;
    probe.cold=Some(cold.clone());
    Some(probe)
}
pub(super) async fn selection_hints(shell:&Session,checked:&mut BTreeSet<usize>)->Result<source_selection::ClosedHints,String>{
    let cold=shell.cold.as_ref().ok_or_else(invalid)?;
    validate(&cold.snapshot,shell,&cold.summary)?;
    let mut reader=cold.store.lock().await;
    if checked.insert(Arc::as_ptr(&cold.store) as usize){reader.load_index()?;}
    cold.selection.get_or_try_init(||async{
        let mut hints=source_selection::ClosedHints::default();
        let mut seen=0;
        for offset in (0..cold.summary.receipts.len()).step_by(32){
            for (index,value) in reader.read_range(&cold.snapshot,&shell.id,Kind::Turns,offset,32)?.into_iter().enumerate(){
                let turn:Turn=serde_json::from_value(value).map_err(|_|invalid())?;
                let index=offset+index;
                let (id,rid)=cold.summary.receipts.get(index).ok_or_else(invalid)?;
                if turn.id!=*id || turn.request_id!=*rid || turn.state!="completed"
                    || !matches!(turn.interaction.as_str(),"chat"|"task") || cold.summary.version<=2&&turn.interaction!="chat"
                    || !turn.history_committed || turn.finished_at.is_none() || turn.pending.is_some()
                    || !turn.queued.is_empty() || !turn.delivery_jobs.is_empty() || turn.runtime_uncertain_model
                    || turn.parent_turn_id.is_some() || turn.steered_turn_id.is_some() || !turn.requirements.is_empty()
                    || turn.workflow.unfinished() || turn.workflow.stop.is_some() || turn.requirements_revision>turn.model_requirements_revision
                    || turn.steps.iter().any(|step|step.state!="completed")
                    || serde_json::to_value(&turn.usage).map_err(|_|invalid())?!=serde_json::to_value(cold.summary.usage.get(index).ok_or_else(invalid)?).map_err(|_|invalid())? {
                    return Err(invalid());
                }
                hints.push(&turn,index)?;
                seen+=1;
            }
        }
        if seen!=cold.summary.receipts.len(){return Err(invalid());}
        Ok(hints)
    }).await.cloned()
}

#[cfg(test)]
#[path = "assistant_archive_tests.rs"]
mod tests;
