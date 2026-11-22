//! Keep completed graph replay in its original encrypted turn record.
use super::*;

const MIN_RELEASE: usize = 1024;

pub(in crate::assistant) struct ReplaySource {
    snapshot: Snapshot,
    reader: std::sync::Mutex<Store>,
    session_id: String,
}

fn replay_bytes(turn: &Turn) -> Result<usize, String> {
    let operations = turn
        .runtime_operations
        .encoded_extra_bytes()
        .map_err(|_| invalid())?;
    let models = capacity::encoded_bytes(&turn.runtime_models)?
        .checked_sub(2)
        .ok_or_else(invalid)?;
    let results = capacity::encoded_bytes(&turn.runtime_results)?
        .checked_sub(2)
        .ok_or_else(invalid)?;
    operations
        .checked_add(models)
        .and_then(|value| value.checked_add(results))
        .ok_or_else(invalid)
}

fn charge(turn: &Turn) -> Result<Option<usize>, String> {
    if turn.archived_replay_bytes.is_some() || !closed_task(turn) {
        return Ok(None);
    }
    let released = replay_bytes(turn)?;
    Ok((released >= MIN_RELEASE).then_some(released))
}

fn apply(turn: &mut Turn, released: usize) {
    turn.runtime_operations = Records::default();
    turn.runtime_models = BTreeMap::new();
    turn.runtime_results = BTreeMap::new();
    turn.archived_replay_bytes = Some(released);
}

pub(super) fn archived_bytes(session: &Session) -> usize {
    session
        .turns
        .iter()
        .filter_map(|turn| turn.archived_replay_bytes)
        .sum()
}

pub(super) struct Plan {
    changes: Vec<(usize, usize)>,
    pub(super) released: usize,
}

pub(super) fn plan(session: &Session) -> Result<Plan, String> {
    let mut plan = Plan {
        changes: Vec::new(),
        released: 0,
    };
    for (index, turn) in session.turns.iter().enumerate() {
        if let Some(released) = charge(turn)? {
            plan.released = plan.released.checked_add(released).ok_or_else(invalid)?;
            plan.changes.push((index, released));
        }
    }
    Ok(plan)
}

fn pin(session: &mut Session, snapshot: &Snapshot, store: &Store) {
    session.turn_replay_source = (archived_bytes(session) != 0).then(|| {
        Arc::new(ReplaySource {
            snapshot: snapshot.clone(),
            reader: std::sync::Mutex::new(store.pinned_reader()),
            session_id: session.id.clone(),
        })
    });
}

pub(super) fn publish(session: &mut Session, plan: Plan, snapshot: &Snapshot, store: &Store) {
    for (index, released) in plan.changes {
        apply(&mut session.turns[index], released);
    }
    pin(session, snapshot, store);
}

fn restore(turn: &mut Turn, source: Option<&ReplaySource>) -> Result<(), String> {
    let Some(released) = turn.archived_replay_bytes else {
        return Ok(());
    };
    let source = source.ok_or_else(invalid)?;
    let raw = source.reader.lock().map_err(|_| invalid())?.read_record(
        &source.snapshot,
        &source.session_id,
        Kind::Turns,
        &turn.id,
    )?;
    let original: Turn = serde_json::from_value(raw).map_err(|_| invalid())?;
    if original.id != turn.id
        || original.request_id != turn.request_id
        || original.fingerprint != turn.fingerprint
        || original.runtime_version != turn.runtime_version
        || replay_bytes(&original)? != released
        || replay_bytes(turn)? != 0
    {
        return Err(invalid());
    }
    turn.runtime_operations = original.runtime_operations;
    turn.runtime_models = original.runtime_models;
    turn.runtime_results = original.runtime_results;
    turn.archived_replay_bytes = None;
    Ok(())
}

pub(super) fn restore_unsettled(session: &mut Session) -> Result<(), String> {
    let source = session.turn_replay_source.as_deref().ok_or_else(invalid)?;
    verify_source(session, source)?;
    for turn in session
        .turns
        .matching_mut(|turn| turn.archived_replay_bytes.is_some() && !closed_task(turn))
    {
        restore(turn, Some(source))?;
    }
    Ok(())
}

fn verify_source(session: &Session, source: &ReplaySource) -> Result<(), String> {
    if source.session_id != session.id
        || !source
            .snapshot
            .owner_matches(&session.id, &session.source, &session.source_key)
    {
        return Err(invalid());
    }
    Ok(())
}

pub(super) fn overlay(session: &Session, base: &Snapshot) -> Result<SeriesOverlay, String> {
    let saved = session.saved_turns.as_ref().ok_or_else(invalid)?;
    let source = session.turn_replay_source.as_deref().ok_or_else(invalid)?;
    verify_source(session, source)?;
    let records = session
        .turns
        .iter()
        .enumerate()
        .map(|(index, turn)| {
            if session.turns.shares_record(saved, index) {
                return Ok(RecordSlot::Retained {
                    index,
                    id: turn.id.clone(),
                });
            }
            let mut full = turn.clone();
            restore(&mut full, Some(source))?;
            serde_json::to_value(full)
                .map(RecordSlot::Owned)
                .map_err(|_| invalid())
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(SeriesOverlay {
        session_id: session.id.clone(),
        kind: Kind::Turns,
        snapshot: base.clone(),
        records,
    })
}

pub(super) enum LoadMode {
    Startup,
    Cached,
}

pub(super) fn load(
    session: &mut Session,
    store: &mut Store,
    snapshot: &Snapshot,
    mode: LoadMode,
) -> Result<(), String> {
    let mut turns = Vec::new();
    let mut resident = 0usize;
    let mut add_turn = |raw: Value| -> Result<(), String> {
        let mut turn: Turn = serde_json::from_value(raw).map_err(|_| invalid())?;
        if let Some(released) = charge(&turn)? {
            apply(&mut turn, released);
        }
        resident = resident
            .checked_add(capacity::encoded_bytes(&turn)?)
            .ok_or_else(invalid)?;
        if resident > capacity::RESIDENT_BYTES {
            return Err("ARCHIVE_RESIDENT_LIMIT: 当前轮次记录超过驻留预算，原档案保留".into());
        }
        turns.push(turn);
        Ok(())
    };
    match mode {
        LoadMode::Startup => store.visit_turns(snapshot, &session.id, add_turn)?,
        LoadMode::Cached => {
            // Explicit cold reads share the range cache. Fetch one turn at a
            // time because adjacent records may exceed the aggregate limit.
            for offset in 0..snapshot.count(&session.id, Kind::Turns)? {
                let raw = store
                    .read_range(snapshot, &session.id, Kind::Turns, offset, 1)?
                    .pop()
                    .ok_or_else(invalid)?;
                add_turn(raw)?;
            }
        }
    }
    session.turns = turns.into();
    pin(session, snapshot, store);
    Ok(())
}
