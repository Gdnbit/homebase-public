//! Lazy, immutable history rows. Task execution and task job ownership remain outside this reader.
use super::*;
use crate::assistant_history::{HistoryKind, HistoryQuery, HistoryRecords, HistorySource};
use crate::assistant_state_store::RecordView;

struct ArchivedRows {
    view: Arc<RecordView>,
    shell: Session,
    kind: HistoryKind,
    ids: Vec<String>,
    inputs: BTreeMap<String, String>,
    turns: BTreeMap<String, (String, Vec<wire::ModelUsage>)>,
    allow_attachments: bool,
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|_| invalid())
}
fn message(value: Value) -> Result<Message, String> {
    let row: Message = decode(value)?;
    if !row.attachments.is_empty() {
        return Err(invalid());
    }
    Ok(row)
}
fn chat(turn: &Turn, hint: &(String, Vec<wire::ModelUsage>)) -> Result<(), String> {
    let mut probe = empty_session("history-data", "", "history-data");
    probe.turns = vec![turn.clone()].into();
    if turn.request_id != hint.0
        || body_summary(&probe).is_none()
        || serde_json::to_value(&turn.usage).map_err(|_| invalid())?
            != serde_json::to_value(&hint.1).map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    Ok(())
}
/// Filtering needs an exact total, so scan only the relevant immutable series in bounded batches.
/// Byte-limited batches shrink rather than treating a failed read as an empty match set.
pub(super) fn visit(
    view: &RecordView,
    kind: Kind,
    mut each: impl FnMut(Value) -> Result<(), String>,
) -> Result<(), String> {
    let total = view.count(kind)?;
    let mut offset = 0;
    while offset < total {
        let mut count = (total - offset).min(32);
        let rows = loop {
            match view.range(kind, offset, count) {
                Ok(rows) => break rows,
                Err(e) if count > 1 && e.starts_with("ARCHIVE_LIMIT:") => {
                    count = count.div_ceil(2);
                }
                Err(e) => return Err(e),
            }
        };
        if rows.len() != count {
            return Err(invalid());
        }
        for row in rows {
            each(row)?;
        }
        offset += count;
    }
    Ok(())
}
pub(super) fn live_message_source(
    session: &Session,
    view: Arc<RecordView>,
    request: &HistoryQuery,
) -> Result<HistorySource, String> {
    let turn_ids: Vec<_> = session.turns.iter().map(|turn| turn.id.as_str()).collect();
    message_source(session, view, request, &turn_ids, true)
}

fn message_source(
    session: &Session,
    view: Arc<RecordView>,
    request: &HistoryQuery,
    turn_ids: &[&str],
    allow_attachments: bool,
) -> Result<HistorySource, String> {
    let count = history_ops::history_turn_count(turn_ids, request.through_turn_id.as_deref())?;
    let needle = request.normalized_query();
    let mut omitted = 0;
    let ids = if needle.is_empty() && request.through_turn_id.is_none() {
        view.ids(Kind::Messages)?
    } else {
        let all: BTreeSet<_> = turn_ids.iter().copied().collect();
        let allowed: BTreeSet<_> = turn_ids.iter().copied().take(count).collect();
        let mut ids = Vec::new();
        visit(&view, Kind::Messages, |value| {
            let row: Message = if allow_attachments {
                decode(value)?
            } else {
                message(value)?
            };
            if needle.is_empty() || row.content.to_lowercase().contains(&needle) {
                if request.through_turn_id.is_none()
                    || row
                        .turn_id
                        .as_deref()
                        .is_some_and(|id| allowed.contains(id))
                {
                    ids.push(row.id);
                } else if row.turn_id.as_deref().is_none_or(|id| !all.contains(id)) {
                    omitted += 1;
                }
            }
            Ok(())
        })?;
        ids
    };
    let shell = empty_session(&session.source, &session.source_key, &session.id);
    let records = ArchivedRows {
        view,
        shell: shell.clone(),
        kind: HistoryKind::Messages,
        ids,
        inputs: BTreeMap::new(),
        turns: BTreeMap::new(),
        allow_attachments,
    };
    Ok(HistorySource {
        session: shell,
        source_key: session.source_key.clone(),
        kind: HistoryKind::Messages,
        query: needle,
        through_turn_id: request.through_turn_id.clone(),
        omitted_unassigned_messages: omitted,
        captured_at: chrono::Utc::now(),
        records: Some(Arc::new(records)),
        direct_turn: None,
    })
}
pub(super) async fn source(
    shell: &Session,
    cold: &Cold,
    request: &HistoryQuery,
) -> Result<HistorySource, String> {
    let turn_ids: Vec<_> = cold
        .summary
        .receipts
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    let count = history_ops::history_turn_count(&turn_ids, request.through_turn_id.as_deref())?;
    let allowed: BTreeSet<_> = turn_ids.iter().copied().take(count).collect();
    let view = {
        let mut reader = cold.store.lock().await;
        reader.load_index()?;
        reader.record_view(&cold.snapshot, &shell.id)?
    };
    if request.kind == HistoryKind::Messages {
        return message_source(shell, view, request, &turn_ids, false);
    }
    let mut frozen = empty_session(&shell.source, &shell.source_key, &shell.id);
    frozen.title = shell.title.clone();
    frozen.updated_at = shell.updated_at.clone();
    frozen.queue_paused = shell.queue_paused;
    frozen.queue_resume_generation = shell.queue_resume_generation;
    let needle = request.normalized_query();
    let matches = |text: &str| needle.is_empty() || text.to_lowercase().contains(&needle);
    let mut inputs = BTreeMap::new();
    // Task TurnViews retain the full ownership check.
    if cold.summary.version > 2 {
        return Err(invalid());
    }
    let mut input_matches = BTreeSet::new();
    visit(&view, Kind::Messages, |value| {
        let row = message(value)?;
        if let Some(id) = row.turn_id.as_deref().filter(|id| allowed.contains(id)) {
            if matches(&row.content) {
                input_matches.insert(id.to_string());
            }
            if row.role == "user" {
                inputs.entry(id.to_string()).or_insert(row.id);
            }
        }
        Ok(())
    })?;
    let hints: BTreeMap<_, _> = cold
        .summary
        .receipts
        .iter()
        .zip(&cold.summary.usage)
        .map(|((id, rid), usage)| (id.clone(), (rid.clone(), usage.clone())))
        .collect();
    let ids = if needle.is_empty() {
        turn_ids
            .iter()
            .take(count)
            .map(|id| id.to_string())
            .collect()
    } else {
        let mut ids = Vec::new();
        visit(&view, Kind::Turns, |value| {
            let turn: Turn = decode(value)?;
            chat(&turn, hints.get(&turn.id).ok_or_else(invalid)?)?;
            if allowed.contains(turn.id.as_str())
                && (matches(&turn.text) || input_matches.contains(&turn.id))
            {
                ids.push(turn.id);
            }
            Ok(())
        })?;
        ids
    };
    let records = ArchivedRows {
        view,
        shell: frozen.clone(),
        kind: request.kind,
        ids,
        inputs,
        turns: hints,
        allow_attachments: false,
    };
    Ok(HistorySource {
        session: frozen,
        source_key: shell.source_key.clone(),
        kind: request.kind,
        query: needle,
        through_turn_id: request.through_turn_id.clone(),
        omitted_unassigned_messages: 0,
        captured_at: chrono::Utc::now(),
        records: Some(Arc::new(records)),
        direct_turn: None,
    })
}
impl HistoryRecords for ArchivedRows {
    fn ids(&self) -> &[String] {
        &self.ids
    }
    fn charge_bytes(&self) -> Result<usize, String> {
        self.view
            .charge_bytes()
            .checked_add(capacity::encoded_bytes(&(
                &self.ids,
                &self.inputs,
                &self.turns,
            ))?)
            .ok_or_else(invalid)
    }
    fn row(&self, index: usize) -> Result<Session, String> {
        let id = self.ids.get(index).ok_or_else(invalid)?;
        let mut row = self.shell.clone();
        match self.kind {
            HistoryKind::Messages => {
                let value = self.view.record(Kind::Messages, id)?;
                row.messages = vec![if self.allow_attachments {
                    decode(value)?
                } else {
                    message(value)?
                }]
                .into();
            }
            HistoryKind::Turns => {
                let turn: Turn = decode(self.view.record(Kind::Turns, id)?)?;
                chat(&turn, self.turns.get(id).ok_or_else(invalid)?)?;
                if let Some(input) = self.inputs.get(id) {
                    let input = message(self.view.record(Kind::Messages, input)?)?;
                    if input.role != "user" || input.turn_id.as_deref() != Some(id.as_str()) {
                        return Err(invalid());
                    }
                    row.messages = vec![input].into();
                }
                row.turns = vec![turn].into();
            }
        }
        Ok(row)
    }
}
