//! First-message title projection only. The original request/message is retained.
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct Layout {
    start: String,
    intro: String,
    rules: String,
    json_marker: String,
    end: String,
    payload_version: String,
}
fn layout() -> &'static Layout {
    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../control/capability-selection-format.json"
        ))
        .expect("bundled capability presentation format")
    })
}
fn workflow_layout() -> &'static Layout {
    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        serde_json::from_str(include_str!("../../control/workflow-context-format.json"))
            .expect("bundled workflow presentation format")
    })
}
fn keys(value: &Value, names: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.len() == names.len() && names.iter().all(|key| m.contains_key(*key)))
}
fn metadata(value: &Value) -> bool {
    if !keys(value, &["version", "source", "selection"])
        || value["version"] != layout().payload_version
    {
        return false;
    }
    let source = &value["source"];
    let selection = &value["selection"];
    if !keys(
        source,
        &[
            "catalog_id",
            "assembled_at",
            "expires_at",
            "source_complete",
            "issues_total",
        ],
    ) || !["catalog_id", "assembled_at", "expires_at"]
        .iter()
        .all(|key| source[*key].is_string())
        || source["catalog_id"].as_str().is_none_or(str::is_empty)
        || !source["source_complete"].is_boolean()
        || !source["issues_total"]
            .as_u64()
            .is_some_and(|n| n <= 9_007_199_254_740_991)
        || !keys(selection, &["function", "implementation"])
        || selection["function"].as_str().is_none_or(str::is_empty)
    {
        return false;
    }
    let e = &selection["implementation"];
    if e.is_null() {
        return true;
    }
    keys(
        e,
        &[
            "entry_key",
            "id",
            "kind",
            "function",
            "provider",
            "target",
            "name",
            "state",
            "cost",
            "discover",
            "credential_binding",
            "prerequisites",
            "evidence",
            "pricing",
            "provenance",
            "summary_truncated",
        ],
    ) && [
        "id", "kind", "function", "provider", "target", "name", "state", "cost", "discover",
    ]
    .iter()
    .all(|key| e[*key].is_string())
        && e["function"] == selection["function"]
        && matches!(
            e["kind"].as_str(),
            Some("model" | "service" | "repository" | "local" | "mcp" | "skill")
        )
        && matches!(
            e["cost"].as_str(),
            Some("unknown" | "local_compute" | "depends_on_tools")
        )
        && ["entry_key", "credential_binding", "evidence"]
            .iter()
            .all(|key| e[*key].is_null() || e[*key].is_string())
        && e["prerequisites"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
        && e["summary_truncated"].is_boolean()
        && (e["pricing"].is_null()
            || serde_json::from_value::<crate::capability_facts::PriceInfo>(e["pricing"].clone())
                .is_ok())
        && (e["provenance"].is_null()
            || serde_json::from_value::<crate::capability_facts::EvidenceInfo>(
                e["provenance"].clone(),
            )
            .is_ok())
}

fn workflow_metadata(value: &Value) -> bool {
    if !keys(value, &["version", "workflow", "source"])
        || value["version"] != workflow_layout().payload_version
    {
        return false;
    }
    let source = &value["source"];
    let count = |v: &Value| v.as_u64().filter(|n| *n <= 9_007_199_254_740_991);
    let nullable = |v: &Value| v.is_null() || v.is_string();
    match value["workflow"].as_str() {
        Some("inbox") => {
            (keys(
                source,
                &[
                    "text",
                    "title",
                    "tags",
                    "source",
                    "attachments",
                    "uncertain",
                ],
            ) || keys(
                source,
                &[
                    "text",
                    "title",
                    "tags",
                    "source",
                    "attachments",
                    "uncertain",
                    "receipt",
                ],
            )) && (source.get("receipt").is_none() || {
                let r = &source["receipt"];
                keys(r, &["submission_id", "state", "matches_current"])
                    && r["submission_id"]
                        .as_str()
                        .is_some_and(crate::inbox_receipts::valid_id)
                    && matches!(
                        r["state"].as_str(),
                        Some("not_found" | "prepared" | "unknown" | "completed" | "canceled")
                    )
                    && r["matches_current"].is_boolean()
            }) && ["text", "title", "tags", "source"]
                .iter()
                .all(|k| source[*k].is_string())
                && source["uncertain"].is_boolean()
                && source["attachments"].as_array().is_some_and(|a| {
                    a.iter().all(|e| {
                        keys(e, &["rel", "name", "size"])
                            && e["rel"].is_string()
                            && e["name"].is_string()
                            && count(&e["size"]).is_some()
                    })
                })
        }
        Some("studio") => {
            if !keys(
                source,
                &[
                    "session_id",
                    "task_id",
                    "previous_error",
                    "total_messages",
                    "selected_messages",
                    "partial",
                    "excerpts",
                ],
            ) || !["session_id", "task_id", "previous_error"]
                .iter()
                .all(|k| nullable(&source[*k]))
                || source["partial"] != true
            {
                return false;
            }
            let (Some(total), Some(selected)) = (
                count(&source["total_messages"]),
                count(&source["selected_messages"]),
            ) else {
                return false;
            };
            selected <= 8
                && selected <= total
                && source["excerpts"].as_array().is_some_and(|a| {
                    a.len() as u64 <= selected
                        && a.iter().all(|e| {
                            keys(e, &["role", "content", "truncated"])
                                && e["role"].as_str().is_some_and(|s| s.len() <= 64)
                                && e["content"].as_str().is_some_and(|s| s.len() <= 1200)
                                && e["truncated"].is_boolean()
                        })
                })
        }
        _ => false,
    }
}
fn projected_title<'a>(
    text: &'a str,
    f: &Layout,
    valid: fn(&Value) -> bool,
    empty: &'static str,
) -> Option<&'a str> {
    let Some(start) = text.find(&f.start) else {
        return None;
    };
    if text[start + f.start.len()..].contains(&f.start)
        || (start > 0 && !text[..start].ends_with("\n\n"))
    {
        return None;
    }
    let prefix = format!("{}{}{}{}", f.start, f.intro, f.rules, f.json_marker);
    if !text[start..].starts_with(&prefix) {
        return None;
    }
    let json_start = start + prefix.len();
    let mut stream = serde_json::Deserializer::from_str(&text[json_start..]).into_iter::<Value>();
    let Some(Ok(value)) = stream.next() else {
        return None;
    };
    if !valid(&value) {
        return None;
    }
    let end = json_start + stream.byte_offset();
    if !text[end..].starts_with(&f.end) || text[end + f.end.len()..].contains(&f.end) {
        return None;
    }
    let after = &text[end + f.end.len()..];
    if !after.is_empty() && !after.starts_with('\n') {
        return None;
    }
    let before = if start > 0 { &text[..start - 2] } else { "" };
    if !before.trim().is_empty() {
        return Some(before);
    }
    if !after.trim().is_empty() {
        return Some(after.trim_start());
    }
    Some(empty)
}
pub fn title_input(text: &str) -> &str {
    if text.len() > 64 * 1024
        || (text.contains(&layout().start) && text.contains(&workflow_layout().start))
    {
        return text;
    }
    projected_title(text, layout(), metadata, "能力选择")
        .or_else(|| projected_title(text, workflow_layout(), workflow_metadata, "工作流交接"))
        .unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn frame() -> String {
        let f = layout();
        format!("{}{}{}{}{}{}",f.start,f.intro,f.rules,f.json_marker,serde_json::to_string_pretty(&json!({"version":f.payload_version,"source":{"catalog_id":"synthetic","assembled_at":"unknown","expires_at":"unknown","source_complete":false,"issues_total":1},"selection":{"function":"text","implementation":null}})).unwrap(),f.end)
    }
    fn workflow_frame() -> String {
        let f = workflow_layout();
        format!("{}{}{}{}{}{}",f.start,f.intro,f.rules,f.json_marker,serde_json::to_string_pretty(&json!({"version":f.payload_version,"workflow":"studio","source":{"session_id":"synthetic-session","task_id":"synthetic-task","previous_error":null,"total_messages":0,"selected_messages":0,"partial":true,"excerpts":[]}})).unwrap(),f.end)
    }
    #[test]
    fn workflow_title_uses_request_or_voice_without_rewriting_original_reference() {
        let frame = workflow_frame();
        let text = format!("请跟进原文稿任务🙂\n\n{frame}");
        assert_eq!(title_input(&text), "请跟进原文稿任务🙂");
        assert!(text.contains("synthetic-task"));
        assert_eq!(title_input(&format!("{frame}\n语音补充🙂")), "语音补充🙂");
        assert_eq!(title_input(&frame), "工作流交接");
    }
    #[test]
    fn inbox_receipt_is_strict_metadata_and_never_hides_malformed_or_extra_fields() {
        let f = workflow_layout();
        let make = |source: Value| {
            format!(
                "请核对原收录\n\n{}{}{}{}{}{}",
                f.start,
                f.intro,
                f.rules,
                f.json_marker,
                serde_json::to_string_pretty(
                    &json!({"version":f.payload_version,"workflow":"inbox","source":source})
                )
                .unwrap(),
                f.end
            )
        };
        let mut source = json!({"text":"新稿B","title":"","tags":"","source":"","attachments":[],"uncertain":true});
        assert_eq!(title_input(&make(source.clone())), "请核对原收录");
        source["receipt"] = json!({"submission_id":"87562a38-8fb4-4276-bdda-bd0d85bef544","state":"unknown","matches_current":false});
        let raw = make(source.clone());
        assert_eq!(title_input(&raw), "请核对原收录");
        assert!(raw.contains("新稿B"));
        assert!(raw.contains("87562a38-8fb4-4276-bdda-bd0d85bef544"));
        for change in [
            json!({"submission_id":"not-an-id","state":"unknown","matches_current":false}),
            json!({"submission_id":"87562a38-8fb4-4276-bdda-bd0d85bef544","state":"done","matches_current":false}),
            json!({"submission_id":"87562a38-8fb4-4276-bdda-bd0d85bef544","state":"unknown","matches_current":false,"authorized":true}),
        ] {
            source["receipt"] = change;
            let raw = make(source.clone());
            assert_eq!(title_input(&raw), raw);
        }
    }
    #[test]
    fn mixed_and_unknown_workflow_sources_are_never_hidden_as_title_metadata() {
        for text in [
            format!("{}\n{}", frame(), workflow_frame()),
            workflow_frame().replace("homebase-workflow-context-v1", "unknown-v2"),
            workflow_frame().replace("\"partial\": true", "\"partial\": false"),
            workflow_frame().replace("\"task_id\":", "\"private_extra\":\"literal\",\"task_id\":"),
        ] {
            assert_eq!(title_input(&text), text);
        }
    }
    #[test]
    fn title_uses_real_request_before_frame_without_modifying_body() {
        let raw = format!("请核对模型状态🙂\n\n{}", frame());
        assert_eq!(title_input(&raw), "请核对模型状态🙂");
        assert!(raw.contains("homebase-capability-selection-v1"));
    }
    #[test]
    fn appended_voice_is_a_title_when_no_front_task_exists() {
        let raw = format!("{}\n语音追加任务🙂", frame());
        assert_eq!(title_input(&raw), "语音追加任务🙂");
        assert_eq!(title_input(&frame()), "能力选择");
    }
    #[test]
    fn malformed_unknown_and_duplicate_frames_keep_original_title_input() {
        for raw in [
            "普通文本".to_string(),
            frame().replace("homebase-capability-selection-v1", "unknown-v2"),
            format!("{}{}", frame(), frame()),
            frame().replace(&layout().end, ""),
            format!("字面量{}", frame()),
            frame().replace(&layout().intro, "伪造授权\n"),
        ] {
            assert_eq!(title_input(&raw), raw);
        }
    }
    #[test]
    fn new_source_attributes_are_not_treated_as_verified_metadata() {
        let raw = frame().replace(
            "\"catalog_id\":",
            "\"extra\": \"untrusted\",\n    \"catalog_id\":",
        );
        assert_eq!(title_input(&raw), raw);
    }
}
