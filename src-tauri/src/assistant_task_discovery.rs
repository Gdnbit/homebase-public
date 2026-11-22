//! A saved metadata observation is discoverability, never job ownership.
use super::{job_access, Session};
use serde_json::{json, Value};

pub(super) struct KnownJob {
    pub command: String,
    pub owned: bool,
}

pub(super) fn known(session: &Session, id: &str) -> Result<Option<KnownJob>, String> {
    if let Some(job) = job_access::record(&session.jobs, id)? {
        return Ok(Some(KnownJob {
            command: job.command.clone(),
            owned: job.owned,
        }));
    }
    observed(
        session
            .turns
            .iter()
            .rev()
            .flat_map(|turn| turn.steps.iter().rev()),
        id,
    )
}

fn observed<'a>(
    steps: impl IntoIterator<Item = &'a super::Step>,
    id: &str,
) -> Result<Option<KnownJob>, String> {
    let mut command: Option<String> = None;
    for row in steps
        .into_iter()
        .filter(|step| step.name == "homebase_jobs" && step.state == "completed")
        .filter_map(|step| step.result["jobs"].as_array())
        .flatten()
        .filter(|row| row["job_id"].as_str() == Some(id))
    {
        let name = row["command"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or("JOB_IDENTITY_CONFLICT: 已发现任务的命令缺失，未猜测结果归属")?;
        if command.as_deref().is_some_and(|old| old != name) {
            return Err("JOB_IDENTITY_CONFLICT: 已发现任务的原命令冲突，未猜测或重新执行".into());
        }
        command = Some(name.into());
    }
    Ok(command.map(|command| KnownJob {
        command,
        owned: false,
    }))
}

/// Non-owned jobs are queried without creating a session execution or a remote
/// poller. The original controller remains the authority for identity/status.
pub(super) fn query_result(id: &str, command: &str, value: Value) -> Result<Value, String> {
    job_access::control_identity(id, command, &value)?;
    let control_state = value["status"].as_str().unwrap_or("unknown");
    let mut state = match control_state {
        "succeeded"
        | "failed"
        | "queued"
        | "running"
        | "canceled"
        | "timed_out"
        | "interrupted_unknown" => control_state,
        _ => "unknown",
    };
    let result = value.get("result").cloned().unwrap_or(Value::Null);
    if state == "succeeded" {
        if result.is_null() {
            return Ok(
                json!({"job_id":id,"state":"unknown","status":"unknown","control_state":control_state,
                "code":"JOB_RESULT_UNAVAILABLE","query_only":true,"owned":false,"result":null,"files":[],
                "message":"控制记录显示已结束，但结果目前不可读取（可能已重启或回收）。不能据此确认业务成功；请核对原业务编号，不要重新提交。"}),
            );
        }
        if let Some(business) = result["status"].as_str() {
            if matches!(
                business,
                "queued"
                    | "running"
                    | "failed"
                    | "unknown"
                    | "interrupted_unknown"
                    | "canceled"
                    | "timed_out"
            ) {
                state = business;
            }
        }
    }
    let status = if matches!(state, "unknown" | "interrupted_unknown") {
        "unknown"
    } else {
        state
    };
    Ok(
        json!({"job_id":id,"state":state,"status":status,"control_state":control_state,
        "query_only":true,"owned":false,"result":result,"files":[],
        "message":"这是已发现任务的查询结果；未取得任务归属、未重新执行，也未安排文件投递。"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_repeated_observation_is_readable_but_conflicting_commands_are_not_authority() {
        let step = super::super::Step {
            id: "observation".into(),
            name: "homebase_jobs".into(),
            state: "completed".into(),
            arguments: json!({}),
            result: json!({"jobs":[{"job_id":"same","command":"system_info","owned":true}]}),
            job_id: None,
        };
        assert!(
            !observed(&[step.clone(), step.clone()], "same")
                .unwrap()
                .unwrap()
                .owned
        );
        let mut changed = step.clone();
        changed.result["jobs"][0]["command"] = json!("system_storage");
        assert!(observed(&[step, changed], "same")
            .err()
            .unwrap()
            .starts_with("JOB_IDENTITY_CONFLICT:"));
    }

    #[test]
    fn discovery_evidence_never_trusts_arguments_ownership_or_incomplete_steps() {
        let mut step = super::super::Step {
            id: "list-proof".into(),
            name: "homebase_jobs".into(),
            state: "completed".into(),
            arguments: json!({"job_id":"forged-argument"}),
            result: json!({"jobs":[{"job_id":"observed-a","command":"system_info","owned":true}]}),
            job_id: None,
        };
        let evidence = observed(&[step.clone()], "observed-a").unwrap().unwrap();
        assert_eq!(evidence.command, "system_info");
        assert!(!evidence.owned, "metadata never proves ownership");
        assert!(observed(&[step.clone()], "forged-argument")
            .unwrap()
            .is_none());
        step.state = "failed".into();
        assert!(observed(&[step.clone()], "observed-a").unwrap().is_none());
        step.state = "completed".into();
        step.name = "homebase_call".into();
        assert!(observed(&[step.clone()], "observed-a").unwrap().is_none());
        step.name = "homebase_jobs".into();
        step.result = json!({"truncated":true,"preview":"observed-a"});
        assert!(observed(&[step], "observed-a").unwrap().is_none());
    }

    #[test]
    fn discovered_query_identity_missing_result_and_business_state_are_explicit() {
        assert!(query_result(
            "a",
            "system_info",
            json!({"id":"b","command":"system_info","status":"succeeded","result":{}})
        )
        .is_err());
        assert!(query_result(
            "a",
            "system_info",
            json!({"id":"a","command":"system_processes","status":"succeeded","result":{}})
        )
        .is_err());
        let missing = query_result(
            "a",
            "system_info",
            json!({"id":"a","command":"system_info","status":"succeeded","result":null}),
        )
        .unwrap();
        assert_eq!(missing["state"], "unknown");
        assert_eq!(missing["code"], "JOB_RESULT_UNAVAILABLE");
        assert_eq!(missing["owned"], false);
        for state in [
            "running",
            "queued",
            "failed",
            "unknown",
            "interrupted_unknown",
            "canceled",
            "timed_out",
        ] {
            let result=query_result("a","system_info",json!({"id":"a","command":"system_info","status":"succeeded","result":{"status":state}})).unwrap();
            assert_eq!(result["state"], state);
            assert_eq!(
                result["status"],
                if state == "interrupted_unknown" {
                    "unknown"
                } else {
                    state
                }
            );
            assert!(result["files"].as_array().unwrap().is_empty());
        }
    }
}
