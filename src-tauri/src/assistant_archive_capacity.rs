//! Separate retained history from the serialized resident State budget.
use super::*;

pub(super) const RESIDENT_BYTES: usize = 24 * 1024 * 1024;
pub(super) const RETAINED_BYTES: usize = 256 * 1024 * 1024;
pub(super) const RETAINED_SESSIONS: usize = 10_000;

pub(super) fn encoded_bytes(value: &impl Serialize) -> Result<usize, String> {
    records::encoded_bytes(value).map_err(|_| invalid())
}

fn state_bytes(state: &mut State) -> Result<usize, String> {
    if state.archive_capacity_version != 1 {
        return encoded_bytes(state);
    }
    let record_bytes = state.sessions.iter().try_fold(0usize, |total, session| {
        [
            session.messages.encoded_extra_bytes(),
            session.turns.encoded_extra_bytes(),
            session.jobs.encoded_extra_bytes(),
        ]
        .into_iter()
        .try_fold(total, |total, bytes| {
            total
                .checked_add(bytes.map_err(|_| invalid())?)
                .ok_or_else(invalid)
        })
    })?;
    if record_bytes == 0 {
        return encoded_bytes(state);
    }
    // Only a private snapshot or exclusively locked State reaches this path.
    // Restore the original arrays even if encoding the shell fails.
    let arrays: Vec<_> = state
        .sessions
        .iter_mut()
        .map(|session| {
            (
                std::mem::take(&mut session.messages),
                std::mem::take(&mut session.turns),
                std::mem::take(&mut session.jobs),
            )
        })
        .collect();
    let encoded = encoded_bytes(state);
    for (session, arrays) in state.sessions.iter_mut().zip(arrays) {
        (session.messages, session.turns, session.jobs) = arrays;
    }
    encoded?.checked_add(record_bytes).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capacity_reuses_unchanged_message_and_turn_lengths_after_a_step_edit() {
        let (_workspace, mut state, _) = super::super::tests::filled().await;
        enable(&mut state);
        let session = &mut state.sessions[0];
        session.messages = session.messages.to_vec().into();
        session.turns = session.turns.to_vec().into();
        session.messages[0].content = "合成消息🙂\n".repeat(32 * 1024);
        session.turns[0].steps.push(Step {
            id: "synthetic-large-step".into(),
            name: "homebase_call".into(),
            state: "completed".into(),
            arguments: json!({"command":"system_info","args":{}}),
            result: json!({"text":"合成步骤🙂".repeat(128 * 1024)}),
            job_id: None,
        });
        let before = serde_json::to_vec(&state).unwrap();
        assert_eq!(state_bytes(&mut state).unwrap(), before.len());
        let original = &state.sessions[0];
        assert_eq!(
            original.messages.cached_byte_lengths(),
            original.messages.len()
        );
        assert_eq!(original.turns.cached_byte_lengths(), original.turns.len());

        let mut next = state.clone();
        next.sessions[0].turns[0].steps.last_mut().unwrap().result = json!({"text":"缩短为一行"});
        assert_eq!(
            next.sessions[0].turns.cached_byte_lengths() + 1,
            original.turns.len()
        );
        assert_eq!(
            next.sessions[0].messages.cached_byte_lengths(),
            original.messages.len()
        );
        let changed = serde_json::to_vec(&next).unwrap();
        assert_eq!(state_bytes(&mut next).unwrap(), changed.len());
        assert_eq!(
            next.sessions[0].turns.cached_byte_lengths(),
            original.turns.len()
        );
        assert_eq!(serde_json::to_vec(&next).unwrap(), changed);
        assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    }

    #[test]
    fn capacity_state_budget_matches_full_json_after_job_updates() {
        let mut state = State::default();
        enable(&mut state);
        state
            .sessions
            .push(empty_session("desktop", "desktop", "empty"));
        for sid in ["synthetic-a", "synthetic-b"] {
            let mut session = empty_session("desktop", "desktop", sid);
            for index in 0..3 {
                session.jobs.push(Job {
                    archived_result_bytes: None,
                    id: format!("{sid}-{index}"),
                    command: "system_info".into(),
                    turn_id: String::new(),
                    state: "succeeded".into(),
                    result: json!({"text": "中文🙂\n\"", "number": index}),
                    owned: true,
                    deliver: false,
                    artifacts: Vec::new(),
                    remote: None,
                });
            }
            state.sessions.push(session);
        }
        let before = serde_json::to_vec(&state).unwrap();
        assert_eq!(state_bytes(&mut state).unwrap(), before.len());
        assert_eq!(serde_json::to_vec(&state).unwrap(), before);
        let mut next = state.clone();
        next.sessions[1].jobs[1].result = json!({"value": "合成🙂".repeat(32 * 1024)});
        let changed = serde_json::to_vec(&next).unwrap();
        assert_eq!(state_bytes(&mut next).unwrap(), changed.len());
        assert_eq!(serde_json::to_vec(&next).unwrap(), changed);
        assert_eq!(state_bytes(&mut state).unwrap(), before.len());
        assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    }
}
pub(crate) fn enable(state: &mut State) {
    state.archive_capacity_version = 1;
    enable_tasks(state);
}
pub(super) fn check_retained(state: &mut State) -> Result<(), String> {
    if state.archive_capacity_version > 1 {
        return Err(invalid());
    }
    let limit = if state.archive_capacity_version == 1 {
        RETAINED_BYTES
    } else {
        RESIDENT_BYTES
    };
    let bytes = state_bytes(state)?;
    if logical_bytes_for(state, bytes)? > limit {
        return Err(if state.archive_capacity_version == 1 {
            "ARCHIVE_RETAINED_LIMIT: 加密留存历史达到256MiB边界，原记录保留；请先导出或拆分工作区"
        } else {
            "助理历史达到容量上限，请新建独立工作区；未覆盖旧历史"
        }
        .into());
    }
    Ok(())
}
pub(super) fn candidates(
    state: &mut State,
    protected: &[String],
    added_hints: usize,
    job_plans: &[live_jobs::Plan],
    replay_plans: &[live_turn_replay::Plan],
) -> Result<Vec<(usize, Summary)>, String> {
    if state.archive_capacity_version == 0 {
        return Ok(Vec::new());
    }
    let mut resident = resident_bytes(state)?
        .checked_add(added_hints)
        .ok_or_else(invalid)?;
    let mut candidates = Vec::new();
    let protected: BTreeSet<_> = protected.iter().map(String::as_str).collect();
    for (position, ((session, job_plan), replay_plan)) in
        state.sessions.iter().zip(job_plans).zip(replay_plans).enumerate()
    {
        if session.cold.is_none() && !protected.contains(session.id.as_str()) {
            if let Some(summary) = state.archive_index.get(&session.id) {
                resident = resident
                    .checked_sub(summary.extra_bytes
                        .checked_sub(live_jobs::archived_bytes(session))
                        .and_then(|bytes| bytes.checked_sub(live_turn_replay::archived_bytes(session)))
                        .ok_or_else(invalid)?)
                    .ok_or_else(invalid)?;
                candidates.push((position, summary.clone()));
                continue;
            }
        }
        resident = resident
            .checked_sub(job_plan.released)
            .and_then(|bytes| bytes.checked_sub(replay_plan.released))
            .ok_or_else(invalid)?;
    }
    if resident > RESIDENT_BYTES {
        return Err(resident_error());
    }
    Ok(candidates)
}
fn resident_error() -> String {
    "ARCHIVE_RESIDENT_LIMIT: 正在使用或受保护的助理状态达到24MiB边界，原任务和历史保留；请先完成在途工作".into()
}
pub(super) fn check_loaded(state: &mut State) -> Result<(), String> {
    if state.archive_capacity_version == 1 {
        check_retained(state)?;
        if resident_bytes(state)? > RESIDENT_BYTES {
            return Err(resident_error());
        }
    }
    Ok(())
}
pub(super) fn check_restore(
    state: &mut State,
    position: usize,
    loaded: &Session,
) -> Result<(), String> {
    if state.archive_capacity_version == 1 {
        let resident = resident_bytes(state)?
            .checked_sub(session_bytes(&state.sessions[position])?)
            .ok_or_else(invalid)?
            .checked_add(session_bytes(loaded)?)
            .ok_or_else(invalid)?;
        if resident > RESIDENT_BYTES {
            return Err(resident_error());
        }
    }
    Ok(())
}
fn session_bytes(session: &Session) -> Result<usize, String> {
    encoded_bytes(session)?
        .checked_add(live_messages::hint_bytes(session)?)
        .ok_or_else(invalid)
}
fn resident_bytes(state: &mut State) -> Result<usize, String> {
    let bytes = state_bytes(state)?;
    state.sessions.iter().try_fold(bytes, |bytes, session| {
        bytes
            .checked_add(live_messages::hint_bytes(session)?)
            .ok_or_else(invalid)
    })
}
pub(super) fn session_admission(state: &State) -> Result<(), String> {
    if state.archive_capacity_version == 1 && state.sessions.len() >= RETAINED_SESSIONS {
        return Err(
            "ARCHIVE_SESSION_LIMIT: 已留存10000个原会话，未删除历史；请先完整导出或拆分工作区"
                .into(),
        );
    }
    let count = state
        .sessions
        .iter()
        .filter(|s| state.archive_capacity_version == 0 || s.cold.is_none())
        .count();
    if count >= 100 {
        return Err("会话数量达到上限，旧会话不会自动删除".into());
    }
    Ok(())
}
