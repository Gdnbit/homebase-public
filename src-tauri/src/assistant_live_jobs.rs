//! Read settled results from their original encrypted record instead of keeping a second body.
use super::*;

const LOAD_BATCH: usize = 8;

pub(in crate::assistant) struct JobSource {
    snapshot: Snapshot,
    reader: std::sync::Mutex<Store>,
    session_id: String,
}

impl JobSource {
    fn new(session: &Session, snapshot: &Snapshot, store: &Store) -> Arc<Self> {
        Arc::new(Self {
            snapshot: snapshot.clone(),
            reader: std::sync::Mutex::new(store.pinned_reader()),
            session_id: session.id.clone(),
        })
    }

    fn restore(&self, job: &mut Job) -> Result<(), String> {
        let Some(saved_bytes) = job.archived_result_bytes else {
            return Ok(());
        };
        let raw = self.reader.lock().map_err(|_| invalid())?.read_record(
            &self.snapshot,
            &self.session_id,
            Kind::Jobs,
            &job.id,
        )?;
        let original: Job = serde_json::from_value(raw).map_err(|_| invalid())?;
        if original.id != job.id
            || original.turn_id != job.turn_id
            || original.command != job.command
            || original.state != job.state
            || original.owned != job.owned
            || result_projection(&original.result) != job.result
            || capacity::encoded_bytes(&original.result)?
                .checked_sub(capacity::encoded_bytes(&job.result)?)
                != Some(saved_bytes)
        {
            return Err(invalid());
        }
        job.result = original.result;
        job.archived_result_bytes = None;
        Ok(())
    }
}

fn result_projection(result: &Value) -> Value {
    // These fields feed progress, result identity and settled-job accounting.
    // Public results are always restored from the pinned record before encoding.
    let mut data = serde_json::Map::new();
    for name in [
        "status",
        "state",
        "code",
        "ok",
        "success",
        "complete",
        "partial",
        "truncated",
        "run_id",
        "task_id",
        "workspace_id",
        "skill_id",
    ] {
        if let Some(value) = result.get(name) {
            data.insert(name.into(), value.clone());
        }
    }
    if let Some(status) = result["result"].get("status") {
        data.insert("result".into(), json!({"status": status}));
    }
    Value::Object(data)
}

pub(in crate::assistant) fn restored(job: &Job, source: Option<&JobSource>) -> Result<Job, String> {
    let mut result = job.clone();
    restore_one(&mut result, source)?;
    Ok(result)
}

pub(in crate::assistant) fn restore_one(
    job: &mut Job,
    source: Option<&JobSource>,
) -> Result<(), String> {
    if job.archived_result_bytes.is_some() {
        source.ok_or_else(invalid)?.restore(job)?;
    }
    Ok(())
}

pub(in crate::assistant) fn restore(
    jobs: &mut Records<Job>,
    source: Option<&JobSource>,
) -> Result<(), String> {
    for job in jobs.matching_mut(|job| job.archived_result_bytes.is_some()) {
        restore_one(job, source)?;
    }
    Ok(())
}

pub(super) fn archived_bytes(session: &Session) -> usize {
    session
        .jobs
        .iter()
        .filter_map(|job| job.archived_result_bytes)
        .sum()
}

pub(super) struct Projection {
    result: Value,
    released: usize,
}

fn project(job: &Job, eligible: &EligibleJobs<'_>) -> Result<Option<Projection>, String> {
    if job.archived_result_bytes.is_some()
        || !eligible.can_archive(job)
        || !settled_job(job)
        || !control_receipts::complete_job_result(&job.result)
        || job.result.get("result").is_some_and(|result| {
            result.is_object() && !control_receipts::complete_job_result(result)
        })
    {
        return Ok(None);
    }
    let result = result_projection(&job.result);
    let released =
        capacity::encoded_bytes(&job.result)?.saturating_sub(capacity::encoded_bytes(&result)?);
    Ok((released >= 1024).then_some(Projection { result, released }))
}

pub(super) struct Plan {
    changes: Vec<(usize, Projection)>,
    pub(super) released: usize,
}

struct EligibleJobs<'a> {
    closed: BTreeSet<&'a str>,
    verified: BTreeMap<(&'a str, &'a str), bool>,
}

impl<'a> EligibleJobs<'a> {
    fn new(session: &'a Session) -> Self {
        Self {
            closed: session
                .turns
                .iter()
                .filter(|turn| closed_task(turn))
                .map(|turn| turn.id.as_str())
                .collect(),
            verified: control_receipts::verified_job_steps(session),
        }
    }

    fn can_archive(&self, job: &Job) -> bool {
        self.closed.contains(job.turn_id.as_str())
            || job.owned
                && self.verified.get(&(job.turn_id.as_str(), job.id.as_str())) == Some(&true)
    }
}

pub(super) fn plan(session: &Session) -> Result<Plan, String> {
    let eligible = EligibleJobs::new(session);
    let mut changes = Vec::new();
    let mut released = 0usize;
    for (position, job) in session.jobs.iter().enumerate() {
        if let Some(projection) = project(job, &eligible)? {
            released = released
                .checked_add(projection.released)
                .ok_or_else(invalid)?;
            changes.push((position, projection));
        }
    }
    Ok(Plan { changes, released })
}

fn apply(job: &mut Job, projection: Projection) {
    job.result = projection.result;
    job.archived_result_bytes = Some(projection.released);
}

pub(super) fn publish(session: &mut Session, plan: Plan, snapshot: &Snapshot, store: &Store) {
    for (position, projection) in plan.changes {
        apply(&mut session.jobs[position], projection);
    }
    session.job_result_source =
        (archived_bytes(session) != 0).then(|| JobSource::new(session, snapshot, store));
}

pub(super) fn load(
    session: &mut Session,
    store: &mut Store,
    snapshot: &Snapshot,
) -> Result<(), String> {
    let eligible = EligibleJobs::new(session);
    let mut jobs = Vec::new();
    let mut resident = 0usize;
    for offset in (0..snapshot.count(&session.id, Kind::Jobs)?).step_by(LOAD_BATCH) {
        for raw in store.read_range(snapshot, &session.id, Kind::Jobs, offset, LOAD_BATCH)? {
            let mut job: Job = serde_json::from_value(raw).map_err(|_| invalid())?;
            if let Some(projection) = project(&job, &eligible)? {
                apply(&mut job, projection);
            }
            resident = resident
                .checked_add(capacity::encoded_bytes(&job)?)
                .ok_or_else(invalid)?;
            if resident > capacity::RESIDENT_BYTES {
                return Err("ARCHIVE_RESIDENT_LIMIT: 当前任务记录超过驻留预算，原档案保留".into());
            }
            jobs.push(job);
        }
    }
    session.jobs = jobs.into();
    session.job_result_source =
        (archived_bytes(session) != 0).then(|| JobSource::new(session, snapshot, store));
    Ok(())
}
