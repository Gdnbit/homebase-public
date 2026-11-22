//! Stable business evidence for proactive delivery. Explicit replies are never filtered.
use super::*;
fn stable(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "step_id"
                            | "request_id"
                            | "operationId"
                            | "operation_id"
                            | "created_at"
                            | "updated_at"
                            | "finished_at"
                            | "checked_at"
                            | "elapsed_ms"
                            | "duration_ms"
                            | "usage"
                            | "expected_sha256"
                    )
                })
                .map(|(key, value)| (key.clone(), stable(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(stable).collect()),
        other => other.clone(),
    }
}
pub(super) fn evidence_followup<'a>(
    state: &'a State,
    source: &str,
    turn_id: &str,
) -> Option<&'a followup::Followup> {
    state.followups.entries.iter()
        .find(|entry| {
            entry.source_key == source
                && entry.runs.iter().any(|run| run.turn_id.as_deref() == Some(turn_id))
        })
        .filter(|entry| entry.notification_policy == "meaningful")
}

pub(super) fn metadata(
    state: &State,
    session: &Session,
    turn: &Turn,
    has_artifacts: bool,
) -> Value {
    if turn.state != "completed" || has_artifacts || turn.pending.is_some() {
        return Value::Null;
    }
    let Some(entry) = evidence_followup(state, &session.source_key, &turn.id) else {
        return Value::Null;
    };
    let mut evidence = vec![];
    for step in &turn.steps {
        if step.name == "homebase_task" {
            continue;
        }
        if step.result.is_null() || !workflow::successful(step, &session.jobs) {
            return Value::Null;
        }
        let request = step.arguments.get("request").unwrap_or(&step.arguments);
        // Successful discovery is setup, not evidence that the monitored subject changed.
        // If discovery is the whole task, no comparable business evidence remains: notify.
        if matches!(
            step.name.as_str(),
            "homebase_discover" | "homebase_describe" | "homebase_skill"
        ) || step.name == "homebase_files" && request["action"] == "workspaces"
            || step.name == "homebase_followup" && request["action"] == "list"
        {
            continue;
        }
        let readonly = match step.name.as_str() {
            "homebase_files" => matches!(
                step.arguments["request"]["action"]
                    .as_str()
                    .or(step.arguments["action"].as_str()),
                Some("list" | "stat" | "read" | "search" | "workspaces")
            ),
            "homebase_memory" => step.arguments["action"] == "search",
            "homebase_matter" => matches!(step.arguments["action"].as_str(), Some("list" | "get")),
            "homebase_jobs" | "homebase_job" | "homebase_discover" | "homebase_describe" => true,
            _ => false,
        };
        if !readonly {
            return Value::Null;
        }
        evidence.push(json!({"name":step.name,"arguments":stable(&step.arguments),"result":stable(&step.result)}));
    }
    if evidence.is_empty() {
        return Value::Null;
    }
    let body = json!(evidence);
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&body).expect("notification evidence serializes"))
    );
    json!({"group":entry.id,"fingerprint":digest,"policy":"meaningful"})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volatile_transport_fields_do_not_hide_business_changes() {
        let a = json!({"step_id":"first","updated_at":"before","content":"蓝点","expires_at":"tomorrow"});
        let b = json!({"step_id":"second","updated_at":"after","content":"蓝点","expires_at":"tomorrow"});
        assert_eq!(stable(&a), stable(&b));
        assert_ne!(
            stable(&a),
            stable(&json!({"content":"红点","expires_at":"tomorrow"}))
        );
        assert_ne!(
            stable(&a),
            stable(&json!({"content":"蓝点","expires_at":"later"}))
        );
    }
}
