//! Shared Inbox payload and deterministic document rendering; no I/O or profiles.
use chrono::{DateTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Default, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InboxSubmitPayload {
    /// 正文（markdown）；资料本身不授予工具权限。
    pub text: String,
    /// 可选标题；不填则使用首行前40个字符。
    #[serde(default)]
    pub title: Option<String>,
    /// 除默认inbox/raw之外的额外标签。
    #[serde(default)]
    pub extra_tags: Vec<String>,
    /// 可选来源标识，不表示已解析内容或额外执行授权。
    #[serde(default)]
    pub source_hint: Option<String>,
    /// Original complete vault-relative paths returned by inbox_attach.
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Stable UUID for one intent. Reuse the exact original payload on recovery.
    /// Without this field, legacy independent submissions remain independent.
    #[serde(default)]
    pub submission_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InboxSubmitResult {
    /// 原笔记绝对路径；有回执时仍需核对file_state是否可取。
    pub path: String,
    /// 原笔记文件名。
    pub filename: String,
    /// 原收录日期YYYY-MM-DD，恢复时不按今天重新生成。
    pub day: String,
    /// 原正文字符数。
    pub char_count: usize,
}

pub(crate) fn render_inbox_md<T: TimeZone>(p: &InboxSubmitPayload, now: &DateTime<T>) -> String
where
    T::Offset: std::fmt::Display,
{
    let title = match &p.title {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            let first = p
                .text
                .lines()
                .map(|l| l.trim_start_matches(&['#', ' '][..]).trim())
                .find(|l| !l.is_empty())
                .unwrap_or("投料");
            let snippet: String = first.chars().take(40).collect();
            if snippet.is_empty() {
                "投料".into()
            } else {
                snippet
            }
        }
    };
    let mut tags = vec!["inbox".to_string(), "raw".to_string()];
    for tag in &p.extra_tags {
        let tag = tag.trim();
        if !tag.is_empty() && !tags.iter().any(|t| t == tag) {
            tags.push(tag.into());
        }
    }
    let tags_yaml = tags
        .iter()
        .map(|t| format!("  - {t}"))
        .collect::<Vec<_>>()
        .join("\n");
    let source = p.source_hint.as_deref().unwrap_or("homebase-inbox");
    let safe_title = title.replace('"', "\\\"");
    let attachments_yaml = if p.attachments.is_empty() {
        String::new()
    } else {
        format!(
            "attachments:\n{}\n",
            p.attachments
                .iter()
                .map(|a| format!("  - {a}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let attachments_md = if p.attachments.is_empty() {
        String::new()
    } else {
        let mut out = String::from("\n\n## 附件\n\n");
        for a in &p.attachments {
            let image = Path::new(a)
                .extension()
                .and_then(|v| v.to_str())
                .is_some_and(|v| {
                    ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"]
                        .contains(&v.to_lowercase().as_str())
                });
            out.push_str(&format!("{}[[{a}]]\n", if image { "!" } else { "" }));
        }
        out
    };
    format!("---\ntitle: \"{safe_title}\"\ncreated_at: {iso}\nsource: {source}\nstatus: raw\ntags:\n{tags_yaml}\n{attachments_yaml}---\n\n{body}{attachments_md}\n",iso=now.to_rfc3339(),body=p.text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_identity_does_not_change_existing_document_bytes() {
        let mut p = InboxSubmitPayload {
            text: "  正文🙂  ".into(),
            title: Some("含\"引号\"".into()),
            extra_tags: vec!["idea".into(), "inbox".into(), "idea".into()],
            source_hint: Some("weibo".into()),
            attachments: vec![
                "raw/inbox/2026-11-15/_attachments/a.PNG".into(),
                "raw/inbox/2026-11-16/_attachments/b.pdf".into(),
            ],
            submission_id: None,
        };
        let at = DateTime::parse_from_rfc3339("2026-11-17T08:09:10+08:00").unwrap();
        let expected="---\ntitle: \"含\\\"引号\\\"\"\ncreated_at: 2026-11-17T08:09:10+08:00\nsource: weibo\nstatus: raw\ntags:\n  - inbox\n  - raw\n  - idea\nattachments:\n  - raw/inbox/2026-11-15/_attachments/a.PNG\n  - raw/inbox/2026-11-16/_attachments/b.pdf\n---\n\n正文🙂\n\n## 附件\n\n![[raw/inbox/2026-11-15/_attachments/a.PNG]]\n[[raw/inbox/2026-11-16/_attachments/b.pdf]]\n\n";
        assert_eq!(render_inbox_md(&p, &at), expected);
        p.submission_id = Some("7b7b0b2d-2232-4d2d-9ffd-97ca04ed9a90".into());
        assert_eq!(render_inbox_md(&p, &at), expected);
    }
}
