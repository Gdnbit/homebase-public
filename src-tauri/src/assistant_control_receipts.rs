//! Recover a controller acceptance from the original durable assistant step.
use super::*;
use std::collections::BTreeSet;

#[cfg(test)]
#[path = "assistant_control_receipts_tests.rs"]
mod tests;

const OWNED_JOB_LIMIT: usize = 100;
const UNVERIFIED: &str = "JOB_RECEIPT_UNVERIFIED: 原步骤的受理回执尚待核对，请查询原任务";
const CONFLICT: &str = "JOB_IDENTITY_CONFLICT: 原步骤、任务编号或命令不一致";

#[derive(Clone)]
struct Intent {
    session_id: String,
    source_key: String,
    turn_id: String,
    step_id: String,
    name: String,
    command: String,
    args: Value,
}

enum Prepared {
    Submit(Intent),
    Lookup(Intent),
    Linked(String),
}

fn step_index(turn: &Turn, id: &str) -> Result<Option<usize>, String> {
    let mut rows = turn
        .steps
        .iter()
        .enumerate()
        .filter(|(_, step)| step.id == id);
    let position = rows.next().map(|(index, _)| index);
    if rows.next().is_some() {
        return Err(CONFLICT.into());
    }
    Ok(position)
}

fn verify_job(session: &Session, id: &str, turn: &str, command: &str) -> Result<(), String> {
    match job_access::owned(&session.jobs, id)? {
        Some(job) if job.turn_id == turn && job.command == command => Ok(()),
        _ => Err(CONFLICT.into()),
    }
}

pub(super) fn complete_job_result(result: &Value) -> bool {
    !result.is_null()
        && result["code"] != "RESULT_UNAVAILABLE"
        && result["code"] != "JOB_RESULT_UNAVAILABLE"
        && result["message"] != "进程重启后控制结果不可恢复，禁止盲目重跑"
        && result["ok"] != false
        && result["success"] != false
        && result["complete"] != false
        && result["partial"] != true
        && result["truncated"] != true
        && [&result["status"], &result["state"]].iter().all(|status| {
            !matches!(
                status.as_str(),
                Some(
                    "failed"
                        | "error"
                        | "unknown"
                        | "uncertain"
                        | "interrupted_unknown"
                        | "running"
                        | "queued"
                        | "pending"
                        | "submitted"
                        | "processing"
                        | "canceled"
                        | "timed_out"
                        | "blocked"
                        | "partial"
                        | "superseded"
                )
            )
        })
}

pub(super) fn verified_job_steps(session: &Session) -> BTreeMap<(&str, &str), bool> {
    let mut completed = BTreeMap::new();
    for turn in &session.turns {
        for step in &turn.steps {
            if step.name == "homebase_job" {
                continue;
            }
            let Some(id) = step.job_id.as_deref() else {
                continue;
            };
            let verified = step.state == "completed"
                && step.result["state"] == "succeeded"
                && step.result["job_id"] == id
                && step.result["step_id"] == step.id
                && !turn.pending.as_ref().is_some_and(|pending| !pending.approved && pending.id == step.id)
                && !turn.control_intents.contains_key(&step.id);
            completed
                .entry((turn.id.as_str(), id))
                .and_modify(|value| *value = false)
                .or_insert(verified);
        }
    }
    completed
}

fn occupied_job_slots(session: &Session, capacity_mode: bool) -> usize {
    if !capacity_mode {
        return session.jobs.iter().filter(|job| job.owned).count();
    }
    let completed = verified_job_steps(session);
    session
        .jobs
        .iter()
        .filter(|job| {
            job.owned
                && !(job.state == "succeeded"
                    && !job.deliver
                    && job
                        .remote
                        .as_ref()
                        .is_none_or(|remote| remote.finished && remote.query_job.is_none())
                    && completed.get(&(job.turn_id.as_str(), job.id.as_str())) == Some(&true)
                    && complete_job_result(&job.result))
        })
        .count()
}

impl Engine {
    pub(super) async fn submit_control_job(
        &self,
        sid: &str,
        tid: &str,
        call: &ToolCall,
        step_id: &str,
        command: &str,
        args: &Value,
    ) -> Result<String, String> {
        let prepared = self
            .mutate_when_changed(
                Change {
                    session_id: Some(sid.into()),
                    turn_id: Some(tid.into()),
                    ..Change::default()
                },
                |state| {
                    let capacity_mode = state.archive_capacity_version == 1;
                    let (s, t) = locate(state, tid)?;
                    let session = &mut state.sessions[s];
                    let turn = &session.turns[t];
                    if session.id != sid || terminal(&turn.state) {
                        return Err(CONFLICT.into());
                    }
                    let intent = Intent {
                        session_id: sid.into(),
                        source_key: session.source_key.clone(),
                        turn_id: tid.into(),
                        step_id: step_id.into(),
                        name: call.name.clone(),
                        command: command.into(),
                        args: args.clone(),
                    };
                    if let Some(index) = step_index(turn, step_id)? {
                        let step = &turn.steps[index];
                        if step.name != call.name || &step.arguments != args {
                            return Err(CONFLICT.into());
                        }
                        if let Some(job) = &step.job_id {
                            verify_job(session, job, tid, command)?;
                            return Ok((Prepared::Linked(job.clone()), false));
                        }
                        if turn.control_intents.get(step_id).map(String::as_str) != Some(command) {
                            return Err(UNVERIFIED.into());
                        }
                        return Ok((Prepared::Lookup(intent), false));
                    }
                    if !turn.control_intents.is_empty() {
                        return Err(UNVERIFIED.into());
                    }
                    let owned = occupied_job_slots(session, capacity_mode);
                    let pending = session
                        .turns
                        .iter()
                        .flat_map(|turn| {
                            turn.steps.iter().filter(|step| {
                                step.job_id.is_none() && turn.control_intents.contains_key(&step.id)
                            })
                        })
                        .count();
                    if owned.saturating_add(pending) >= OWNED_JOB_LIMIT {
                        return Err("会话任务及待核对回执已达上限，本次操作尚未提交".into());
                    }
                    let turn = &mut session.turns[t];
                    turn.steps.push(Step {
                        id: step_id.into(),
                        name: call.name.clone(),
                        state: "running".into(),
                        arguments: args.clone(),
                        result: Value::Null,
                        job_id: None,
                    });
                    turn.control_intents.insert(step_id.into(), command.into());
                    Ok((Prepared::Submit(intent), true))
                },
            )
            .await?;
        let (intent, job) = match prepared {
            Prepared::Linked(job) => return Ok(job),
            Prepared::Lookup(intent) => {
                let job = self
                    .lookup_control_receipt(&intent)
                    .await?
                    .ok_or(UNVERIFIED)?;
                (intent, job)
            }
            Prepared::Submit(intent) => {
                let response = self
                    .control("call", Some(command), args.clone(), step_id, None)
                    .await?;
                let job = accepted_id(&intent, &response["job"], cfg!(not(test)))?;
                (intent, job)
            }
        };
        self.link_control_receipt(&intent, &job, false).await?;
        Ok(job)
    }

    async fn lookup_control_receipt(&self, intent: &Intent) -> Result<Option<String>, String> {
        let response = self
            .control(
                "assistant.control_receipt",
                Some(&intent.command),
                intent.args.clone(),
                &intent.step_id,
                None,
            )
            .await?;
        match response["state"].as_str() {
            Some("not_found") => Ok(None),
            Some("found") => accepted_id(intent, &response["job"], true).map(Some),
            _ => Err(UNVERIFIED.into()),
        }
    }

    async fn link_control_receipt(
        &self,
        intent: &Intent,
        job_id: &str,
        recovered: bool,
    ) -> Result<(), String> {
        self.mutate_when_changed(
            Change {
                session_id: Some(intent.session_id.clone()),
                turn_id: Some(intent.turn_id.clone()),
                ..Change::default()
            },
            |state| {
                let (s, t) = locate(state, &intent.turn_id)?;
                if state.sessions.iter().enumerate().any(|(index, session)| {
                    index != s && archive::contains_registered_job(session, job_id)
                }) {
                    return Err(CONFLICT.into());
                }
                let session = &mut state.sessions[s];
                if session.id != intent.session_id || session.source_key != intent.source_key {
                    return Err(CONFLICT.into());
                }
                let turn = &session.turns[t];
                let index = step_index(turn, &intent.step_id)?.ok_or(CONFLICT)?;
                let step = &turn.steps[index];
                if step.name != intent.name || step.arguments != intent.args {
                    return Err(CONFLICT.into());
                }
                if let Some(linked) = &step.job_id {
                    if linked != job_id {
                        return Err(CONFLICT.into());
                    }
                    verify_job(session, job_id, &intent.turn_id, &intent.command)?;
                    return Ok(((), false));
                }
                if turn.control_intents.get(&intent.step_id) != Some(&intent.command) {
                    return Err(CONFLICT.into());
                }
                if job_access::record(&session.jobs, job_id)?.is_some() {
                    return Err(CONFLICT.into());
                }
                session.jobs.push(Job {
                    archived_result_bytes: None,
                    id: job_id.into(),
                    command: intent.command.clone(),
                    turn_id: intent.turn_id.clone(),
                    state: "running".into(),
                    result: Value::Null,
                    owned: true,
                    deliver: false,
                    artifacts: vec![],
                    remote: None,
                });
                let turn = &mut session.turns[t];
                turn.steps[index].job_id = Some(job_id.into());
                if !recovered {
                    turn.control_intents.remove(&intent.step_id);
                }
                Ok(((), true))
            },
        )
        .await
    }

    pub(super) async fn reconcile_control_receipts(
        &self,
        tid: &str,
        owner: Option<&str>,
        session_id: Option<&str>,
        refresh_linked: bool,
    ) -> Result<(), String> {
        let intents = {
            let state = self.state.lock().await;
            // The normal read path validates identity. Check it here before
            // making any controller lookup.
            let Ok((s, t)) = locate(&state, tid) else {
                return Ok(());
            };
            let turn = &state.sessions[s].turns[t];
            if !terminal(&turn.state) || turn.control_intents.is_empty() {
                return Ok(());
            }
            let session = archive::scoped_turn_session(&state, tid, owner, session_id)?;
            turn.control_intents
                .iter()
                .map(|(id, command)| {
                    let step = &turn.steps[step_index(turn, id)?.ok_or(CONFLICT)?];
                    let prior = if let Some(job_id) = &step.job_id {
                        verify_job(session, job_id, tid, command)?;
                        job_access::owned(&session.jobs, job_id)?
                            .map(|job| (job.id.clone(), job.state.clone()))
                    } else {
                        None
                    };
                    Ok((
                        Intent {
                            session_id: session.id.clone(),
                            source_key: session.source_key.clone(),
                            turn_id: tid.into(),
                            step_id: id.clone(),
                            name: step.name.clone(),
                            command: command.clone(),
                            args: step.arguments.clone(),
                        },
                        prior,
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?
        };
        for (intent, prior) in intents {
            if let Some((id, state)) = prior {
                if refresh_linked && matches!(state.as_str(), "running" | "queued") {
                    let _ = self.refresh_job(&intent.session_id, &id).await;
                }
                continue;
            }
            // A temporarily unavailable or mismatched receipt leaves the original step pending.
            if let Ok(Some(job)) = self.lookup_control_receipt(&intent).await {
                self.link_control_receipt(&intent, &job, true).await?;
                if refresh_linked {
                    let _ = self.refresh_job(&intent.session_id, &job).await;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn refresh_accepted_turn_jobs(
        &self,
        tid: &str,
        owner: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<BTreeSet<String>, String> {
        let (sid, jobs) = {
            let state = self.state.lock().await;
            // Archived turns have no in-flight jobs. Terminal hot turns may still
            // have a previously accepted business job that needs observation.
            let Ok((s, t)) = locate(&state, tid) else {
                return Ok(BTreeSet::new());
            };
            let session = &state.sessions[s];
            let turn = &session.turns[t];
            if !terminal(&turn.state) {
                return Ok(BTreeSet::new());
            }
            let jobs = session.jobs.iter().filter(|job| accepted_job_pending(turn, job))
                .map(|job| job.id.clone()).collect::<Vec<_>>();
            if jobs.is_empty() {
                return Ok(BTreeSet::new());
            }
            archive::scoped_turn_session(&state, tid, owner, session_id)?;
            (session.id.clone(), jobs)
        };
        let mut unverified = BTreeSet::new();
        for job in jobs {
            if self.refresh_job(&sid, &job).await.is_err() {
                unverified.insert(job);
            }
        }
        Ok(unverified)
    }
}

pub(super) fn update_recovered_step(turn: &mut Turn, job: &Job) {
    if matches!(job.state.as_str(), "running" | "queued") {
        return;
    }
    for step in &mut turn.steps {
        if step.job_id.as_deref() != Some(&job.id)
            || turn.control_intents.get(&step.id) != Some(&job.command)
        {
            continue;
        }
        step.result = wire::bounded(
            json!({"job_id":job.id,"state":job.state,"result":job.result,"step_id":step.id}),
        );
        step.state = match job.state.as_str() {
            "succeeded" => "completed",
            "failed" | "canceled" | "timed_out" => "failed",
            _ => "unknown",
        }
        .into();
        if step.state != "unknown" {
            turn.control_intents.remove(&step.id);
        }
    }
}

fn accepted_id(intent: &Intent, job: &Value, strict: bool) -> Result<String, String> {
    let id = arg_string(job, "id")?;
    for (field, expected) in [
        ("request_id", &intent.step_id),
        ("command", &intent.command),
    ] {
        if (strict || job.get(field).is_some()) && job[field].as_str() != Some(expected.as_str()) {
            return Err(CONFLICT.into());
        }
    }
    Ok(id.into())
}
