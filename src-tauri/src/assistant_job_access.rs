//! Read-only identity/ownership boundary. Callers supply this session's records;
//! controller observations never become records or execution authority here.
use super::{archive, workflow::JobSource, Artifact, Job, Session};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn artifact<'a>(
    jobs: impl IntoIterator<Item = &'a Job>,
    id: &str,
) -> Result<Option<&'a Artifact>, String> {
    let mut ids = BTreeMap::new();
    let mut found = None;
    for job in jobs {
        if !job.owned {
            continue;
        }
        *ids.entry(job.id.as_str()).or_insert(0usize) += 1;
        for item in job.artifacts.iter().filter(|item| item.id == id) {
            if item.job_id != job.id {
                return Err("ARTIFACT_SCOPE_MISMATCH: 产物父任务与原记录不符，未读取".into());
            }
            if found.is_some() {
                return Err("ARTIFACT_IDENTITY_CONFLICT: 产物编号重复，未选择任一文件".into());
            }
            found = Some((job.id.as_str(), item));
        }
    }
    if found
        .as_ref()
        .is_some_and(|(parent, _)| ids.get(parent) != Some(&1))
    {
        return Err("ARTIFACT_IDENTITY_CONFLICT: 产物的父任务编号重复，未猜测归属".into());
    }
    Ok(found.map(|(_, item)| item))
}

pub(super) fn record<'a>(
    source: &'a (impl JobSource + ?Sized),
    id: &str,
) -> Result<Option<&'a Job>, String> {
    let mut found = None;
    for job in source.evidence_jobs().filter(|job| job.id == id) {
        if found.is_some() {
            return Err("JOB_IDENTITY_CONFLICT: 原任务编号重复，未猜测归属或采用任意结果".into());
        }
        found = Some(job);
    }
    Ok(found)
}

pub(super) fn owned<'a>(
    source: &'a (impl JobSource + ?Sized),
    id: &str,
) -> Result<Option<&'a Job>, String> {
    Ok(record(source, id)?.filter(|job| job.owned))
}

pub(super) fn control_identity(id: &str, command: &str, value: &Value) -> Result<(), String> {
    if id.is_empty()
        || command.is_empty()
        || value["id"].as_str() != Some(id)
        || value["command"].as_str() != Some(command)
    {
        return Err(
            "JOB_IDENTITY_MISMATCH: 查询结果与原任务编号或命令不一致，未采纳、投递或重新执行"
                .into(),
        );
    }
    Ok(())
}

pub(super) fn ensure_snapshot(
    session: &Session,
    id: &str,
    expected: &Value,
) -> Result<(), String> {
    let current = record(&session.jobs, id)?
        .ok_or("JOB_READ_STALE: 原任务已变化，迟到查询未发布；只可重新查询原编号")?;
    let current = archive::live_jobs::restored(current, session.job_result_source.as_deref())?;
    let current = serde_json::to_value(current)
        .map_err(|_| "JOB_INVALID_DATA: 原任务记录不可编码，未修改或重新执行")?;
    if &current != expected {
        return Err(
            "JOB_READ_STALE: 查询期间原任务已更新，迟到结果未覆盖新状态；只可重新查询原编号".into(),
        );
    }
    Ok(())
}

pub(super) enum ResultRef<'a> {
    Video(&'a str),
    Repository(&'a str),
    Development(&'a str),
    SkillWorkspace(&'a str),
}

pub(super) fn owns_result(
    source: &(impl JobSource + ?Sized),
    reference: ResultRef<'_>,
) -> Result<bool, String> {
    let mut ids = BTreeSet::new();
    let mut found = false;
    for job in source.evidence_jobs() {
        if !ids.insert(job.id.as_str()) {
            return Err("JOB_IDENTITY_CONFLICT: 原任务编号重复，未猜测结果或目录归属".into());
        }
        if !job.owned {
            continue;
        }
        found |= match reference {
            ResultRef::Video(id) => {
                job.command == "capability_render" && job.result["run_id"].as_str() == Some(id)
            }
            ResultRef::Repository(id) => {
                job.command == "repo_capability_run" && job.result["run_id"].as_str() == Some(id)
            }
            ResultRef::Development(id) => {
                job.command == "development_run" && job.result["task_id"].as_str() == Some(id)
            }
            ResultRef::SkillWorkspace(id) => {
                job.command == "assistant_local_execute"
                    && job.result["workspace_id"].as_str() == Some(id)
                    && job.result["skill_id"]
                        .as_str()
                        .is_some_and(|skill| skill.starts_with("installed:"))
            }
        };
    }
    Ok(found)
}
