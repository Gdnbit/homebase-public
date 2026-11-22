//! Read-only history; the model's session and cutoff come from its real turn.
use super::*;
use crate::assistant_history::{
    DirectTurnContext, HistoryChunk, HistoryKind, HistoryPage, HistoryQuery, HistoryRead, HistorySource,
};
use std::{collections::BTreeSet, time::Instant};

pub(super) fn history_source(
    session: &Session,
    request: &HistoryQuery,
) -> Result<HistorySource, String> {
    let ids: Vec<_> = session.turns.iter().map(|t| t.id.as_str()).collect();
    history_source_ordered(session, request, &ids)
}
pub(super) fn history_turn_count(ids: &[&str], through: Option<&str>) -> Result<usize, String> {
    let all: BTreeSet<_> = ids.iter().copied().collect();
    if all.len() != ids.len() {
        return Err("HISTORY_INVALID_DATA: 原轮次编号重复，未猜测历史边界".into());
    }
    match through {
        Some(id) => ids
            .iter()
            .position(|t| *t == id)
            .map(|n| n + 1)
            .ok_or_else(|| "HISTORY_TURN_NOT_FOUND: 截止轮次不属于本会话".into()),
        None => Ok(ids.len()),
    }
}
pub(super) fn history_source_ordered(
    session: &Session,
    request: &HistoryQuery,
    turn_ids: &[&str],
) -> Result<HistorySource, String> {
    let mut frozen = empty_session(&session.source, &session.source_key, &session.id);
    frozen.title = session.title.clone();
    frozen.updated_at = session.updated_at.clone();
    frozen.queue_paused = session.queue_paused;
    frozen.queue_resume_generation = session.queue_resume_generation;
    let count = history_turn_count(turn_ids, request.through_turn_id.as_deref())?;
    let all: BTreeSet<_> = turn_ids.iter().copied().collect();
    let allowed: BTreeSet<_> = turn_ids.iter().copied().take(count).collect();
    let mut omitted = 0;
    let needle = request.normalized_query();
    let matches = |text: &str| needle.is_empty() || text.to_lowercase().contains(&needle);
    match request.kind {
        HistoryKind::Messages => {
            if request.through_turn_id.is_some() {
                omitted = session
                    .messages
                    .iter()
                    .filter(|m| {
                        m.turn_id.as_deref().is_none_or(|id| !all.contains(id))
                            && matches(&m.content)
                    })
                    .count();
                frozen.messages = session.messages.select(|m| {
                    m.turn_id.as_deref().is_some_and(|id| allowed.contains(id))
                        && matches(&m.content)
                });
            } else {
                frozen.messages = if needle.is_empty() {
                    session.messages.clone()
                } else {
                    session.messages.select(|m| matches(&m.content))
                };
            }
        }
        HistoryKind::Turns => {
            let inputs: BTreeSet<_> = session
                .messages
                .iter()
                .filter(|m| matches(&m.content))
                .filter_map(|m| m.turn_id.as_deref())
                .filter(|id| allowed.contains(id))
                .collect();
            frozen.turns = session.turns.select(|t| {
                allowed.contains(t.id.as_str())
                    && (matches(&t.text) || inputs.contains(t.id.as_str()))
            });
            // TurnView derives an unplanned goal from its original user input.
            // Keep one actual input per selected turn, never a later match.
            let selected: BTreeSet<_> = frozen.turns.iter().map(|t| t.id.as_str()).collect();
            let mut seen = BTreeSet::new();
            frozen.messages = session.messages.select(|m| {
                m.role == "user"
                    && m.turn_id
                        .as_deref()
                        .is_some_and(|id| selected.contains(id) && seen.insert(id.to_string()))
            });
            let delivery: BTreeSet<_> = frozen
                .turns
                .iter()
                .flat_map(|t| t.delivery_jobs.iter().map(String::as_str))
                .collect();
            frozen.jobs = session.jobs.select(|j| {
                allowed.contains(j.turn_id.as_str()) || delivery.contains(j.id.as_str())
            });
            frozen.job_result_source = session.job_result_source.clone();
        }
    }
    Ok(HistorySource {
        session: frozen,
        source_key: session.source_key.clone(),
        kind: request.kind,
        query: needle,
        through_turn_id: request.through_turn_id.clone(),
        omitted_unassigned_messages: omitted,
        captured_at: chrono::Utc::now(),
        records: None,
        direct_turn: None,
    })
}
fn history_session<'a>(state: &'a State, id: &str) -> Result<&'a Session, String> {
    let mut matches = state.sessions.iter().filter(|s| s.id == id);
    let session = matches
        .next()
        .ok_or("HISTORY_SESSION_NOT_FOUND: 原会话不存在")?;
    if matches.next().is_some() {
        return Err("HISTORY_INVALID_DATA: 原会话编号重复，未猜测来源或历史归属".into());
    }
    Ok(session)
}
impl Engine {
    pub async fn history_turn_snapshot(&self, session_id: &str, turn_id: &str) -> Result<HistoryPage, String> {
        let mut query = HistoryQuery {
            session_id: session_id.into(),
            kind: HistoryKind::Turns,
            query: String::new(),
            snapshot_id: None,
            through_turn_id: None,
            offset: 0,
            limit: 1,
        };
        query.validate()?;
        if turn_id.is_empty() || turn_id.len() > 256 || turn_id.chars().any(char::is_control) {
            return Err("HISTORY_BAD_ARGUMENT: 原轮次编号无效".into());
        }
        let cold = self.read_cold_turn(turn_id, None, Some(session_id)).await?;
        let source = {
            let state = self.state.lock().await;
            let current = history_session(&state, session_id)?;
            let session = cold.as_ref().unwrap_or(current);
            if session.source != current.source || session.source_key != current.source_key {
                return Err("HISTORY_SCOPE_MISMATCH: 原会话来源已变化".into());
            }
            let mut turns = session.turns.iter().filter(|turn| turn.id == turn_id);
            let turn = turns.next().ok_or("HISTORY_TURN_NOT_FOUND: 原轮次无法读取")?;
            if turns.next().is_some() {
                return Err("HISTORY_INVALID_DATA: 原轮次编号重复".into());
            }
            let mut frozen = empty_session(&session.source, &session.source_key, &session.id);
            if let Some(input) = session.messages.iter().find(|message|
                message.role == "user" && message.turn_id.as_deref() == Some(turn_id)
            ) {
                frozen.messages = vec![input.clone()].into();
            }
            frozen.turns = session.turns.select(|candidate| candidate.id == turn_id);
            frozen.jobs = session.jobs.select(|job| turn_has_job(turn, job));
            frozen.job_result_source = session.job_result_source.clone();
            frozen.queue_paused = session.queue_paused;
            HistorySource {
                session: frozen,
                source_key: session.source_key.clone(),
                kind: HistoryKind::Turns,
                query: String::new(),
                through_turn_id: None,
                omitted_unassigned_messages: 0,
                captured_at: chrono::Utc::now(),
                records: None,
                direct_turn: Some(DirectTurnContext {
                    progress: progress::snapshot(session, turn),
                    resumable: progress::resume_problem(session, turn).is_none(),
                }),
            }
        };
        let now = Instant::now();
        let key = source.source_key.clone();
        let mut store = self.history.lock().await;
        query.snapshot_id = Some(store.insert(source, now)?);
        store.query(&query, &key, None, now)
    }

    pub async fn history_query(&self, request: HistoryQuery) -> Result<HistoryPage, String> {
        self.history_query_bound(request, None).await
    }
    async fn history_query_bound(
        &self,
        mut request: HistoryQuery,
        bound: Option<&str>,
    ) -> Result<HistoryPage, String> {
        request.validate()?;
        let (key, source) = {
            let state = self.state.lock().await;
            let session = history_session(&state, &request.session_id)?;
            (
                session.source_key.clone(),
                if request.snapshot_id.is_none() {
                    Some(session.clone())
                } else {
                    None
                },
            )
        };
        let source = if let Some(session) = source {
            Some(match archive::history_source(&session, &request).await? {
                Some(source) => source,
                None => history_source(&session, &request)?,
            })
        } else { None };
        {
            let state = self.state.lock().await;
            if history_session(&state,&request.session_id)?.source_key != key {
                return Err("HISTORY_SCOPE_MISMATCH: 来源在读取期间变化，未建立其他来源快照".into());
            }
        }
        let mut store = self.history.lock().await;
        let now = Instant::now();
        if let Some(source) = source {
            request.snapshot_id = Some(store.insert(source, now)?);
        }
        store.query(&request, &key, bound, now)
    }
    pub async fn history_read(&self, request: HistoryRead) -> Result<HistoryChunk, String> {
        self.history_read_bound(request, None).await
    }
    async fn history_read_bound(
        &self,
        request: HistoryRead,
        bound: Option<&str>,
    ) -> Result<HistoryChunk, String> {
        request.validate()?;
        let key = {
            let state = self.state.lock().await;
            history_session(&state, &request.session_id)?
                .source_key
                .clone()
        };
        self.history
            .lock()
            .await
            .read(&request, &key, bound, Instant::now())
    }
    pub(super) async fn history_tool(&self, tid: &str, args: &Value) -> Result<Value, String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Args {
            action: String,
            kind: Option<HistoryKind>,
            query: Option<String>,
            snapshot_id: Option<String>,
            item_id: Option<String>,
            offset: Option<usize>,
            limit: Option<usize>,
        }
        let args: Args = serde_json::from_value(args.clone())
            .map_err(|_| "HISTORY_BAD_ARGUMENT: 历史工具不接受会话、来源、路径或额外参数")?;
        let session = {
            let state = self.state.lock().await;
            let (s, t) = locate(&state, tid)?;
            if !wire::supports_history(&state.sessions[s].turns[t].runtime_version) {
                return Err(
                    "HISTORY_BAD_ARGUMENT: 原任务运行时未挂载历史工具，未迁移检查点".into(),
                );
            }
            state.sessions[s].id.clone()
        };
        match args.action.as_str() {
            "query" => {
                if args.item_id.is_some() {
                    return Err("HISTORY_BAD_ARGUMENT: query不接受正文编号".into());
                }
                serde_json::to_value(
                    self.history_query_bound(
                        HistoryQuery {
                            session_id: session,
                            kind: args.kind.unwrap_or_default(),
                            query: args.query.unwrap_or_default(),
                            snapshot_id: args.snapshot_id,
                            through_turn_id: Some(tid.into()),
                            offset: args.offset.unwrap_or(0),
                            limit: args.limit.unwrap_or(8),
                        },
                        Some(tid),
                    )
                    .await?,
                )
                .map_err(|_| "HISTORY_INVALID_DATA: 历史页无法编码".into())
            }
            "read" => {
                if args.query.is_some() {
                    return Err(
                        "HISTORY_BAD_ARGUMENT: read不接受关键词；使用已发现的记录编号".into(),
                    );
                }
                let result = self
                    .history_read_bound(
                        HistoryRead {
                            session_id: session,
                            snapshot_id: args
                                .snapshot_id
                                .ok_or("HISTORY_BAD_ARGUMENT: read必须指定原快照")?,
                            item_id: args
                                .item_id
                                .ok_or("HISTORY_BAD_ARGUMENT: read必须指定已发现的记录")?,
                            offset: args.offset.unwrap_or(0),
                            limit: args.limit.unwrap_or(4096),
                        },
                        Some(tid),
                    )
                    .await?;
                if args.kind.is_some_and(|kind| kind != result.kind) {
                    return Err("HISTORY_SCOPE_MISMATCH: 正文种类与原快照不一致".into());
                }
                serde_json::to_value(result)
                    .map_err(|_| "HISTORY_INVALID_DATA: 历史正文无法编码".into())
            }
            _ => Err("HISTORY_BAD_ARGUMENT: 历史操作必须为query或read".into()),
        }
    }
}
