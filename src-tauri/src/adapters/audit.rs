use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::VecDeque, fs::File, io::BufReader, path::PathBuf};
use uuid::Uuid;
use crate::query::{visit_json_lines, JsonLinesError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub ts: String,
    pub event: String,
    pub actor: String,
    pub source: String,
    pub target: String,
    pub result: String,
    pub risk: String,
    pub correlation_id: String,
    pub details: Value,
}

impl AuditEvent {
    pub fn new(event: &str, target: impl Into<String>) -> Self {
        Self {
            ts: Local::now().to_rfc3339(),
            event: event.into(),
            actor: "homebase".into(),
            source: "tauri-backend".into(),
            target: target.into(),
            result: "success".into(),
            risk: "medium".into(),
            correlation_id: Uuid::new_v4().to_string(),
            details: json!({}),
        }
    }

    pub fn details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
}

pub fn audit_dir() -> PathBuf {
    super::homebase_home_dir().join(".homebase").join("audit")
}

pub fn events_path() -> PathBuf {
    audit_dir().join("events.jsonl")
}

pub fn append(event: AuditEvent) -> Result<(), String> {
    let path = events_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir {parent:?}: {e}"))?;
    }
    let line = serde_json::to_string(&event).map_err(|e| format!("serialize audit event: {e}"))?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("open audit log {path:?}: {e}"))?;
    writeln!(file, "{line}").map_err(|e| format!("write audit log {path:?}: {e}"))?;
    Ok(())
}

pub fn record(event: &str, target: impl Into<String>, details: Value) {
    let audit = AuditEvent::new(event, target).details(details);
    if let Err(e) = append(audit) {
        tracing::warn!("audit append failed: {}", e);
    }
}

pub fn list_recent(limit: usize) -> Result<Vec<AuditEvent>, String> {
    let path = events_path();
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("read audit log {path:?}: {e}")),
    };
    let keep = limit.max(1);
    let mut recent = VecDeque::new();
    visit_json_lines(BufReader::new(file), |event: AuditEvent| {
        if recent.len() == keep {
            recent.pop_front();
        }
        recent.push_back(event);
    }).map_err(|error| match error {
        JsonLinesError::Read(e) => format!("read audit log {path:?}: {e}"),
        JsonLinesError::Invalid => "QUERY_INVALID: 审计记录格式无效或尚未写完，查询结果不完整".into(),
    })?;
    Ok(recent.into_iter().rev().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::shared_context::tests::TEST_ENV_LOCK;

    #[test]
    fn append_creates_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());

        append(AuditEvent::new("test.event", "target").details(json!({"ok": true}))).unwrap();
        let s = std::fs::read_to_string(events_path()).unwrap();
        assert!(s.contains("\"event\":\"test.event\""));

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }
}
