//! A validated final control response can replay without a writable body.
//! This never evicts a receipt or proves its underlying business task succeeded.
use super::ControlReceipt;
use serde_json::Value;

fn identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn fields(value: &Value, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == keys.len() && keys.iter().all(|key| object.contains_key(*key))
    })
}
fn known_state(value: &Value) -> bool {
    matches!(
        value.as_str(),
        Some(
            "queued"
                | "running"
                | "waiting_jobs"
                | "awaiting_confirmation"
                | "waiting_input_queue"
                | "completed"
                | "failed"
                | "canceled"
                | "rejected"
                | "disabled"
                | "interrupted_unknown"
                | "unknown"
                | "paused_budget"
                | "needs_input"
        )
    )
}
fn message(value: &Value, keys: &[&str]) -> bool {
    fields(value, keys)
        && value["action"] == "message"
        && value["text"]
            .as_str()
            .is_some_and(|text| !text.trim().is_empty() && text.len() <= 16 * 1024)
}
fn final_response(receipt: &ControlReceipt) -> bool {
    if !digest(&receipt.key) || !digest(&receipt.fingerprint) || !identity(&receipt.session_id) {
        return false;
    }
    let Some(response) = &receipt.response else {
        return false;
    };
    match receipt.action.as_str() {
        "新对话" | "开启新对话" | "/new" => {
            receipt.turn_id.is_empty()
                && receipt.confirmation.is_empty()
                && receipt.generation == 0
                && message(response, &["action", "text", "session_id"])
                && response["session_id"].as_str() == Some(&receipt.session_id)
        }
        "继续排队" => {
            receipt.turn_id.is_empty()
                && receipt.confirmation.is_empty()
                && message(response, &["action", "text"])
        }
        "停止" | "停止助理" | "停止当前任务" | "/stop" => {
            identity(&receipt.turn_id)
                && receipt.confirmation.is_empty()
                && message(response, &["action", "text", "turn_id"])
                && response["turn_id"].as_str() == Some(&receipt.turn_id)
        }
        "确认" | "拒绝" | "继续" => {
            identity(&receipt.turn_id)
                && (if receipt.action == "继续" {
                    receipt.confirmation.is_empty()
                } else {
                    uuid::Uuid::parse_str(&receipt.confirmation).is_ok()
                })
                && fields(response, &["session_id", "turn_id", "state"])
                && response["session_id"].as_str() == Some(&receipt.session_id)
                && response["turn_id"].as_str() == Some(&receipt.turn_id)
                && known_state(&response["state"])
        }
        "进度" | "结果" => {
            identity(&receipt.turn_id)
                && receipt.confirmation.is_empty()
                && fields(response, &["action", "turn_id", "view", "files"])
                && response["action"] == "view"
                && response["turn_id"].as_str() == Some(&receipt.turn_id)
                && response["files"].as_bool() == Some(receipt.action == "结果")
                && fields(&response["view"], &["status", "result", "error"])
                && response["view"]["error"].is_null()
                && (response["view"]["status"] == "succeeded"
                    || known_state(&response["view"]["status"]))
                && response["view"]["result"]["status"] == response["view"]["status"]
                && response["view"]["result"]["turn_id"].as_str() == Some(&receipt.turn_id)
        }
        _ => false,
    }
}

pub(super) fn protects(receipt: &ControlReceipt, session: &str, turns: &[&str]) -> bool {
    if receipt.session_id != session && !turns.contains(&receipt.turn_id.as_str()) {
        return false;
    }
    // A conflicting session/turn association stays protected, even with an ACK.
    receipt.session_id != session
        || !receipt.turn_id.is_empty() && !turns.contains(&receipt.turn_id.as_str())
        || !final_response(receipt)
}
