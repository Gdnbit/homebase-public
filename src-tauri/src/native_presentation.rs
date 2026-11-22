//! Read-only presentation helpers. Raw SDK records remain authoritative and unchanged.
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Re-associate only byte-identical text with the original SDK log blocks.
/// App-server may flatten them. Its raw content is never replaced or removed.
pub fn annotate_blocks(thread: &mut Value, raw: &str) -> bool {
    let Some(id) = thread["id"].as_str() else {
        return false;
    };
    let records: Vec<Value> = match raw
        .lines()
        .filter(|s| !s.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
    {
        Ok(records) => records,
        Err(_) => return false,
    };
    if !records
        .iter()
        .any(|r| r["type"] == "session_meta" && r["payload"]["id"] == id)
    {
        return false;
    }
    let mut candidates = std::collections::BTreeMap::<[u8; 32], Option<Vec<String>>>::new();
    for r in &records {
        let p = &r["payload"];
        if r["type"] != "response_item" || p["type"] != "message" || p["role"] != "user" {
            continue;
        }
        let Some(blocks) = p["content"].as_array() else {
            continue;
        };
        // Mixed input cannot be reconstructed as a text-only representation.
        if blocks
            .iter()
            .any(|b| b["type"] != "input_text" || !b["text"].is_string())
        {
            continue;
        }
        let texts: Vec<String> = blocks
            .iter()
            .map(|b| b["text"].as_str().unwrap().to_owned())
            .collect();
        let key = digest(&texts.concat());
        if let Some(existing) = candidates.get_mut(&key) {
            if existing.as_ref() != Some(&texts) {
                *existing = None;
            }
        } else if candidates.len() < 256 {
            candidates.insert(key, Some(texts));
        } else {
            return false;
        }
    }
    let mut annotated = false;
    if let Some(turns) = thread["turns"].as_array_mut() {
        for turn in turns {
            let Some(items) = turn["items"].as_array_mut() else {
                continue;
            };
            for item in items {
                if item["type"] != "userMessage" {
                    continue;
                }
                let Some(blocks) = item["content"].as_array() else {
                    continue;
                };
                if blocks
                    .iter()
                    .any(|b| b["type"] != "text" || !b["text"].is_string())
                {
                    continue;
                }
                let joined: String = blocks.iter().map(|b| b["text"].as_str().unwrap()).collect();
                if let Some(Some(texts)) = candidates.get(&digest(&joined)) {
                    if texts.len() > 1 {
                        item["homebase_input_blocks"] = serde_json::json!(texts);
                        annotated = true;
                    }
                }
            }
        }
    }
    annotated
}

#[derive(Default)]
pub struct CodexPreview {
    first_event: Option<[u8; 32]>,
    verified: Option<String>,
    legacy: Option<String>,
    candidates: Vec<([u8; 32], [u8; 32], String)>,
}
fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).into()
}
fn title(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(120).collect())
}
fn legacy_prelude(text: &str) -> bool {
    let t = text.trim_start();
    [
        "# AGENTS.md instructions",
        "<INSTRUCTIONS>",
        "<PROJECT_INSTRUCTIONS>",
        "<user_instructions>",
        "<environment_context>",
    ]
    .iter()
    .any(|prefix| t.starts_with(prefix))
}
impl CodexPreview {
    pub fn observe(&mut self, record: &Value) {
        if self.verified.is_some() {
            return;
        }
        let p = &record["payload"];
        if record["type"] == "event_msg"
            && p["type"] == "user_message"
            && self.first_event.is_none()
        {
            if let Some(text) = p["message"].as_str() {
                let hash = digest(text);
                self.first_event = Some(hash);
                self.verified = self
                    .candidates
                    .iter()
                    .find(|(a, b, _)| *a == hash || *b == hash)
                    .map(|(_, _, title)| title.clone());
            }
            return;
        }
        if record["type"] != "response_item" || p["type"] != "message" || p["role"] != "user" {
            return;
        }
        let Some(blocks) = p["content"].as_array() else {
            return;
        };
        let texts: Vec<&str> = blocks
            .iter()
            .filter(|b| b["type"] == "input_text")
            .filter_map(|b| b["text"].as_str())
            .collect();
        let Some(first) = texts.first().and_then(|s| title(s)) else {
            return;
        };
        if self.legacy.is_none() && !legacy_prelude(texts[0]) {
            self.legacy = Some(first.clone());
        }
        let concat = digest(&texts.concat());
        let joined = digest(&texts.join("\n"));
        if self
            .first_event
            .is_some_and(|hash| hash == concat || hash == joined)
        {
            self.verified = Some(first);
        } else if self.candidates.len() < 128 {
            self.candidates.push((concat, joined, first));
        }
    }
    pub fn finish(self) -> Option<String> {
        // An event without a matching response is incomplete, not proof the
        // first environment-looking record was the human's request.
        self.verified.or_else(|| {
            if self.first_event.is_none() {
                self.legacy
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn response(texts: &[&str]) -> Value {
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":texts.iter().map(|text|json!({"type":"input_text","text":text})).collect::<Vec<_>>()}})
    }
    fn event(text: &str) -> Value {
        json!({"type":"event_msg","payload":{"type":"user_message","message":text}})
    }
    fn log(blocks: &[&str]) -> String {
        [
            json!({"type":"session_meta","payload":{"id":"owned"}}),
            response(blocks),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
    }
    fn flattened(text: &str) -> Value {
        json!({"id":"owned","turns":[{"items":[{"id":"u","type":"userMessage","content":[{"type":"text","text":text}]}]}]})
    }
    #[test]
    fn flattened_sdk_input_reuses_exact_original_blocks_without_changing_raw_content() {
        let mut thread = flattened("用户问题附带资料");
        let raw_content = thread["turns"][0]["items"][0]["content"].clone();
        assert!(annotate_blocks(
            &mut thread,
            &log(&["用户问题", "附带资料"])
        ));
        assert_eq!(
            thread["turns"][0]["items"][0]["homebase_input_blocks"],
            json!(["用户问题", "附带资料"])
        );
        assert_eq!(thread["turns"][0]["items"][0]["content"], raw_content);
    }
    #[test]
    fn block_mapping_rejects_wrong_session_mismatched_text_and_ambiguous_splits() {
        for raw in [
            log(&["用户", "附带"]).replace("owned", "another"),
            log(&["不匹配", "的文字"]),
            format!(
                "{}\n{}",
                log(&["用户", "附带"]),
                response(&["用户附", "带"])
            ),
        ] {
            let mut thread = flattened("用户附带");
            let original = thread.clone();
            assert!(!annotate_blocks(&mut thread, &raw));
            assert_eq!(thread, original);
        }
    }
    #[test]
    fn literal_background_prefix_in_single_original_block_is_never_split() {
        let text = "我的原话HomeBase 自动背景快照并不是自动资料";
        let mut thread = flattened(text);
        let original = thread.clone();
        assert!(!annotate_blocks(&mut thread, &log(&[text])));
        assert_eq!(thread, original);
    }
    #[test]
    fn preview_uses_matching_human_event_in_either_order_and_only_first_input_block() {
        for first in [true, false] {
            let mut view = CodexPreview::default();
            view.observe(&response(&[
                "<environment_context>\n<cwd>synthetic</cwd>\n</environment_context>",
            ]));
            let texts = [
                "请写一个蓝色圆点动画",
                "HomeBase 自动背景快照\n{\"test\":true}",
            ];
            if first {
                view.observe(&event(&texts.concat()));
            }
            view.observe(&response(&texts));
            if !first {
                view.observe(&event(&texts.concat()));
            }
            assert_eq!(view.finish().as_deref(), Some(texts[0]));
        }
    }
    #[test]
    fn event_verified_literal_instructions_or_environment_are_real_user_text() {
        for text in [
            "# AGENTS.md instructions\n这是用户要讨论的原文",
            "<environment_context>这是用户写的文字",
        ] {
            let mut view = CodexPreview::default();
            view.observe(&event(text));
            view.observe(&response(&[text]));
            assert_eq!(view.finish().as_deref(), title(text).as_deref());
        }
    }
    #[test]
    fn unmatched_event_stays_unnamed_and_legacy_never_changes_raw_records() {
        let original = response(&["<environment_context>\n自动资料"]);
        let copy = original.clone();
        let mut view = CodexPreview::default();
        view.observe(&original);
        view.observe(&event("还未落入response的新消息"));
        assert!(view.finish().is_none());
        assert_eq!(original, copy);
        let mut legacy = CodexPreview::default();
        legacy.observe(&original);
        legacy.observe(&response(&["旧客户端真实输入\n第二行"]));
        assert_eq!(legacy.finish().as_deref(), Some("旧客户端真实输入"));
    }
    #[test]
    fn preview_is_bounded_and_preserves_unicode_code_points() {
        let text = "🙂蓝".repeat(150);
        let mut view = CodexPreview::default();
        view.observe(&event(&text));
        view.observe(&response(&[&text]));
        let preview = view.finish().unwrap();
        assert_eq!(preview.chars().count(), 120);
        assert!(!preview.contains('\u{fffd}'));
    }
}
