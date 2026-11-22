//! Bounded, read-only observation of existing streaming tasks. Never changes business state.
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Serialize)]
pub struct TaskEventRow {
    pub sequence: u64,
    pub payload: Value,
    pub redacted: bool,
}
#[derive(Serialize)]
pub struct TaskEventPage {
    pub topic: String,
    pub task_id: String,
    pub status: String,
    pub settled: bool,
    pub next_seq: u64,
    pub has_more: bool,
    pub history_lost: bool,
    pub truncated: bool,
    pub events: Vec<TaskEventRow>,
    pub message: String,
}
struct Record {
    order: u64,
    next: u64,
    streams: u8,
    status: String,
    terminal: bool,
    truncated: bool,
    bytes: usize,
    rows: VecDeque<StoredRow>,
}
struct StoredRow {
    event: TaskEventRow,
    bytes: usize,
}
#[derive(Default)]
pub struct Journal {
    records: BTreeMap<(String, String), Record>,
    order: u64,
    bytes: usize,
}
fn sanitize(value: &mut Value, key: &str, redacted: &mut bool, truncated: &mut bool, depth: usize) {
    let sensitive = [
        "api_key",
        "apikey",
        "token",
        "authorization",
        "password",
        "private_key",
        "secret_value",
    ];
    let key_lower = key.to_ascii_lowercase();
    if sensitive.iter().any(|part| key_lower.contains(part)) {
        *value = json!("[REDACTED]");
        *redacted = true;
        return;
    }
    if depth > 12 {
        *value = json!("[TRUNCATED]");
        *truncated = true;
        return;
    }
    match value {
        Value::String(text) => {
            if key == "line" {
                if let Ok(mut parsed) = serde_json::from_str::<Value>(text) {
                    sanitize(&mut parsed, "", redacted, truncated, depth + 1);
                    *text = parsed.to_string();
                }
            }
            let lower = text.to_ascii_lowercase();
            if ["bearer ", "api_key=", "api-key:", "authorization:"]
                .iter()
                .any(|pattern| lower.contains(pattern))
                || lower
                    .split_whitespace()
                    .any(|word| word.starts_with("sk-") && word.len() > 18)
            {
                *text = "[REDACTED_DIAGNOSTIC]".into();
                *redacted = true;
            }
            if text.len() > 8192 {
                let mut end = 8192;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                *truncated = true;
            }
        }
        Value::Object(map) => {
            for (name, item) in map {
                sanitize(item, name, redacted, truncated, depth + 1);
            }
        }
        Value::Array(items) => {
            if items.len() > 64 {
                items.truncate(64);
                *truncated = true;
            }
            for item in items {
                sanitize(item, "", redacted, truncated, depth + 1);
            }
        }
        _ => {}
    }
}
impl Journal {
    pub fn record(&mut self, topic: &str, mut payload: Value) {
        if !["studio:event", "wiki-agent:event", "gdnbit:event"].contains(&topic) {
            return;
        }
        let Some(id) = payload["task_id"].as_str().map(str::to_owned) else {
            return;
        };
        if id.len() > 160 {
            return;
        }
        let key = (topic.to_owned(), id);
        self.order += 1;
        let row = self.records.entry(key.clone()).or_insert_with(|| Record {
            order: self.order,
            next: 1,
            streams: 2,
            status: "running".into(),
            terminal: false,
            truncated: false,
            bytes: 0,
            rows: VecDeque::new(),
        });
        let kind = payload["kind"].as_str().unwrap_or("");
        if !row.terminal {
            match kind {
                "completed" => {
                    row.status = if payload["exit_code"].as_i64() == Some(0) {
                        "succeeded"
                    } else {
                        "failed"
                    }
                    .into();
                    row.terminal = true;
                }
                "failed" => {
                    row.status = "failed".into();
                    row.terminal = true;
                }
                "cancelled" => {
                    row.status = "cancelled".into();
                    row.terminal = true;
                }
                _ => {}
            }
        }
        let mut redacted = false;
        sanitize(&mut payload, "", &mut redacted, &mut row.truncated, 0);
        let mut bytes = payload.to_string().len();
        if bytes > 32768 {
            payload = json!({"kind":"truncated","task_id":key.1});
            bytes = payload.to_string().len();
            row.truncated = true;
        }
        let previous_bytes = row.bytes;
        row.bytes += bytes;
        row.rows.push_back(StoredRow {
            event: TaskEventRow { sequence: row.next, payload, redacted },
            bytes,
        });
        row.next += 1;
        while row.rows.len() > 512 || row.bytes > 512 * 1024 {
            if let Some(old) = row.rows.pop_front() {
                row.bytes -= old.bytes;
            }
        }
        self.bytes = self.bytes - previous_bytes + row.bytes;
        while self.records.len() > 64 || self.bytes > 16 * 1024 * 1024 {
            let Some(oldest) = self
                .records
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, r)| r.order)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(removed) = self.records.remove(&oldest) {
                self.bytes -= removed.bytes;
            }
        }
    }
    pub fn stream_closed(&mut self, topic: &str, id: &str) {
        if let Some(record) = self.records.get_mut(&(topic.into(), id.into())) {
            record.streams = record.streams.saturating_sub(1);
        }
    }
    pub fn stream_error(&mut self, topic: &str, id: &str) {
        if let Some(record) = self.records.get_mut(&(topic.into(), id.into())) {
            record.truncated = true;
        }
    }
    pub fn page(&self, topic: &str, id: &str, after: u64, limit: usize) -> TaskEventPage {
        let mut page = TaskEventPage {
            topic: topic.into(),
            task_id: id.into(),
            status: "unknown".into(),
            settled: false,
            next_seq: after,
            has_more: false,
            history_lost: false,
            truncated: false,
            events: vec![],
            message: "任务未由本实例捕获、已被淘汰或实例已重启；不能判断业务终态，不自动重跑。"
                .into(),
        };
        let Some(record) = self.records.get(&(topic.into(), id.into())) else {
            return page;
        };
        page.events = record
            .rows
            .iter()
            .map(|row| &row.event)
            .filter(|r| r.sequence > after)
            .take(limit.clamp(1, 100))
            .cloned()
            .collect();
        page.next_seq = page.events.last().map(|r| r.sequence).unwrap_or(after);
        page.has_more = record.rows.back().is_some_and(|row| row.event.sequence > page.next_seq);
        page.history_lost = record
            .rows
            .front()
            .is_some_and(|row| after.saturating_add(1) < row.event.sequence)
            || after >= record.next;
        page.truncated = record.truncated;
        page.status = record.status.clone();
        page.settled = record.terminal && record.streams == 0;
        page.message = if page.settled {
            "业务已报告终态，stdout/stderr 已读完；仍需按游标取完事件并核对业务输出。"
        } else {
            "仍在运行或终态后的输出管道尚未读完。"
        }
        .into();
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_streams_never_reset_terminal_and_settle_only_after_both_pipes() {
        let mut j = Journal::default();
        j.record("studio:event", json!({"kind":"started","task_id":"a"}));
        j.record(
            "studio:event",
            json!({"kind":"completed","task_id":"a","exit_code":0}),
        );
        assert!(!j.page("studio:event", "a", 0, 100).settled);
        j.record(
            "studio:event",
            json!({"kind":"stream","task_id":"a","line":"synthetic final"}),
        );
        j.stream_closed("studio:event", "a");
        j.stream_closed("studio:event", "a");
        let page = j.page("studio:event", "a", 0, 100);
        assert!(page.settled);
        assert_eq!(page.status, "succeeded");
        assert_eq!(page.events.len(), 3);
    }
    #[test]
    fn pagination_loss_missing_and_topics_are_explicit() {
        let mut j = Journal::default();
        for i in 0..520 {
            j.record(
                "gdnbit:event",
                json!({"kind":"stream","task_id":"a","line":i.to_string()}),
            );
        }
        let page = j.page("gdnbit:event", "a", 0, 2);
        assert!(page.history_lost && page.has_more);
        assert_eq!(page.events.len(), 2);
        assert_eq!(j.page("wiki-agent:event", "a", 0, 2).status, "unknown");
        assert!(!j.page("gdnbit:event", "a", page.next_seq, 100).history_lost);
    }
    #[test]
    fn private_fields_and_nested_json_lines_are_redacted_and_reads_are_passive() {
        let mut j = Journal::default();
        j.record("studio:event", json!({"kind":"stream","task_id":"a","line":"{\"api_key\":\"synthetic-secret\",\"text\":\"public synthetic text\"}"}));
        let a = serde_json::to_string(&j.page("studio:event", "a", 0, 10)).unwrap();
        assert!(!a.contains("synthetic-secret"));
        assert!(a.contains("public synthetic text"));
        assert_eq!(
            a,
            serde_json::to_string(&j.page("studio:event", "a", 0, 10)).unwrap()
        );
    }
    #[test]
    fn bounded_store_reports_eviction_and_long_lines() {
        let mut j = Journal::default();
        for i in 0..65 {
            j.record(
                "studio:event",
                json!({"kind":"started","task_id":i.to_string()}),
            );
        }
        assert_eq!(j.page("studio:event", "0", 0, 10).status, "unknown");
        j.record(
            "studio:event",
            json!({"kind":"stream","task_id":"64","line":"中".repeat(10000)}),
        );
        assert!(j.page("studio:event", "64", 0, 10).truncated);
    }
    #[test]
    fn payload_size_accounting_survives_record_limits_and_global_eviction() {
        let mut journal = Journal::default();
        let line = "\\\"中".repeat(1100);
        for task in 0..36 {
            for _ in 0..80 {
                journal.record("studio:event", json!({"kind":"stream","task_id":task.to_string(),"line":&line}));
            }
        }
        // Exercise the single-event replacement as well as both retention budgets.
        journal.record("studio:event", json!({"kind":"stream","task_id":"35","nested":vec![line; 7]}));
        let total = journal.records.values().map(|record| {
            let actual = record.rows.iter().map(|row| row.event.payload.to_string().len()).sum::<usize>();
            assert_eq!(record.bytes, actual);
            assert!(actual <= 512 * 1024);
            actual
        }).sum::<usize>();
        assert_eq!(journal.bytes, total);
        assert!(total <= 16 * 1024 * 1024);
        assert_eq!(journal.page("studio:event", "0", 0, 10).status, "unknown");
        let latest = journal.page("studio:event", "35", 80, 1);
        assert!(latest.truncated);
        assert_eq!(latest.events[0].payload["kind"], "truncated");
    }
    #[test]
    fn failed_cancelled_and_stream_errors_remain_explicit() {
        let mut j = Journal::default();
        for kind in ["failed", "cancelled"] {
            j.record("studio:event", json!({"kind":kind,"task_id":kind}));
            j.record("studio:event", json!({"kind":"completed","task_id":kind,"exit_code":0}));
            j.stream_error("studio:event",kind);
            j.stream_closed("studio:event",kind);j.stream_closed("studio:event",kind);
            let page=j.page("studio:event",kind,0,100);
            assert_eq!(page.status,kind);assert!(page.settled && page.truncated);
        }
    }
}
