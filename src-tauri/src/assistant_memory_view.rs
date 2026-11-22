//! Current-version search pages sized for the host tool-result envelope.
use super::*;
use serde_json::{json, Value};

impl Personal {
    pub fn search_page(
        &self,
        query: &str,
        offset: usize,
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        if query.len() > 1000 || offset > 12 {
            return Err("查询最多1000字节，offset须为0至12".into());
        }
        let matches = self.search_refs(query, now);
        let mut records = vec![];
        let mut bytes = 2;
        let mut next = offset.min(matches.len());
        for memory in matches.iter().skip(offset) {
            let mut record = json!({"id":memory.id,"title":memory.title,"content":memory.content,
                "expires_at":memory.expires_at,"updated_at":memory.updated_at,"origin":memory.origin,
                "expected_sha256":memory_fingerprint(memory),"correction_count":memory.corrections.len()});
            let mut size = serde_json::to_vec(&record)
                .map_err(|_| "记忆检索编码失败")?
                .len()
                + 1;
            if bytes + size > 7200 && !records.is_empty() {
                break;
            }
            if size > 7200 {
                record.as_object_mut().unwrap().remove("content");
                record["content_preview"] =
                    json!(memory.content.chars().take(300).collect::<String>());
                record["content_truncated"] = json!(true);
                record["notice"]=json!("该记录超出单次工具结果预算，仅展示预览；完整历史与正文见assistant_personal_get，不把预览当完整原文或据此自动纠错。");
                size = serde_json::to_vec(&record)
                    .map_err(|_| "记忆检索编码失败")?
                    .len()
                    + 1;
            }
            bytes += size;
            records.push(record);
            next += 1;
        }
        Ok(
            json!({"revision":self.revision,"memories":records,"offset":offset,
            "next_offset":if next<matches.len(){Some(next)}else{None},"has_more":next<matches.len(),"ranked_match_limit":12,
            "retrieval":"offline_ranked_lexical","notice":"只检索有效当前内容，不读取更新历史；按中英文关键词排序，非向量检索。has_more时沿用query与next_offset继续；版本变化重新检索。提供correct时使用原id、最新revision/expected_sha256纠错并确认；旧工具用原id更新，临时要求不写长期记忆。"}),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_current_records_page_without_losing_fingerprints_or_loading_history() {
        let mut p = Personal::default();
        let at = timestamp("2026-11-13T10:00:00+08:00").unwrap();
        for i in 0..4 {
            p.save_memory(
                MemoryInput {
                    revision: p.revision,
                    id: None,
                    title: format!("合成偏好{i}"),
                    content: "蓝".repeat(1900),
                    expires_at: None,
                },
                at,
            )
            .unwrap();
        }
        let mut offset = 0;
        let mut ids = std::collections::BTreeSet::new();
        loop {
            let page = p.search_page("合成偏好", offset, at).unwrap();
            assert!(serde_json::to_vec(&page).unwrap().len() < 9500);
            for m in page["memories"].as_array().unwrap() {
                assert!(m.get("previous").is_none());
                assert!(m.get("corrections").is_none());
                assert_eq!(m["content"].as_str().unwrap().chars().count(), 1900);
                assert_eq!(m["expected_sha256"].as_str().unwrap().len(), 64);
                assert!(ids.insert(m["id"].as_str().unwrap().to_string()));
            }
            if !page["has_more"].as_bool().unwrap() {
                break;
            }
            offset = page["next_offset"].as_u64().unwrap() as usize;
        }
        assert_eq!(ids.len(), 4);
    }
    #[test]
    fn exceptional_escaped_record_is_an_explicit_preview_instead_of_silent_truncation() {
        let mut p = Personal::default();
        let at = timestamp("2026-11-13T10:00:00+08:00").unwrap();
        p.save_memory(
            MemoryInput {
                revision: 0,
                id: None,
                title: "合成特殊正文".into(),
                content: format!("x{}x", "\0".repeat(1998)),
                expires_at: None,
            },
            at,
        )
        .unwrap();
        let page = p.search_page("特殊正文", 0, at).unwrap();
        assert_eq!(page["memories"][0]["content_truncated"], true);
        assert!(page["memories"][0].get("content").is_none());
        assert!(!page["memories"][0]["notice"].as_str().unwrap().is_empty());
        assert!(serde_json::to_vec(&page).unwrap().len() < 9500);
    }
}
