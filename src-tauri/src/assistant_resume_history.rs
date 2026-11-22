//! Reconcile the existing bounded context without changing its stored format.
use super::*;

pub(super) const UNCERTAIN: &str = "原任务的上下文历史归属无法唯一确认，未移除任何历史或继续执行";

fn committed_group(turn: &Turn) -> Option<Vec<Value>> {
    if turn.state == "completed" {
        return Some(turn.protocol.clone());
    }
    Some(vec![
        turn.protocol.first()?.clone(),
        json!({"role":"assistant","content":turn.text}),
    ])
}

/// A matching value alone is insufficient when another committed turn has the
/// same group. Missing/evicted or duplicate legacy context stays untouched.
pub(super) fn resume_index(session: &Session, turn: &Turn) -> Result<Option<usize>, &'static str> {
    if !turn.history_committed {
        return Ok(None);
    }
    let group = committed_group(turn).ok_or(UNCERTAIN)?;
    let mut matches = session
        .history
        .iter()
        .enumerate()
        .filter(|(_, old)| **old == group);
    let (index, _) = matches.next().ok_or(UNCERTAIN)?;
    if matches.next().is_some()
        || session.turns.iter().any(|other| {
            other.id != turn.id
                && other.history_committed
                && committed_group(other).is_some_and(|candidate| candidate == group)
        })
    {
        return Err(UNCERTAIN);
    }
    Ok(Some(index))
}
