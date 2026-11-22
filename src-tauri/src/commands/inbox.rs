//! Inbox 投料口命令（Phase 14.5c — 用户需求：微博式低门槛输入）
//!
//! 用户场景：
//! - 用户习惯手机备忘录 / 微博随手写，不愿专门打开 Obsidian
//! - 想要把文字 / 文件一股脑扔进来，AI 负责后续归类 / 批注 / 沉淀
//!
//! 本模块只做 **I. 输入层**：
//! - 落盘到 `C:\HomeBase\Wiki\raw\inbox\YYYY-MM-DD\HHmmss-<shortid>.md`
//! - 带 frontmatter（created_at / source / tags / 可选 title）
//! - 快捷打开 Obsidian 应用（用 `obsidian://` URI scheme）
//!
//! **II. 消化层**（raw → wiki）不在这里，由 Obsidian Wiki Agent 异步做。

use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;
pub use crate::inbox_document::{InboxSubmitPayload, InboxSubmitResult};
use crate::inbox_document::render_inbox_md;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

// =====================================================================
// 类型
// =====================================================================

#[derive(Debug, Serialize)]
pub struct InboxOpenObsidianResult {
    pub opened_with: String,
    pub vault: Option<String>,
}

// =====================================================================
// 命令 1：提交投料 → 落盘到 raw/inbox/
// =====================================================================

#[tauri::command]
pub async fn inbox_submit(payload: InboxSubmitPayload) -> Result<InboxSubmitResult, String> {
    tracing::debug!("inbox_submit 调用（{} 字符）", payload.text.chars().count());
    if payload.submission_id.is_some() {
        return crate::inbox_receipts::confirmed_result(submission_store().submit(payload).await?);
    }

    // 1. 计算投递目录：C:\HomeBase\Wiki\raw\inbox\YYYY-MM-DD\
    //    （如果用户之后换 vault 路径，这里要改成读 config）
    let vault_root = wiki_vault_root();
    crate::inbox_storage::references::validate_references(&vault_root, &payload.attachments)
        .map_err(crate::inbox_storage::references::display_error)?;
    let now = Local::now();
    let day = now.format("%Y-%m-%d").to_string();
    let day_dir = vault_root
        .join("raw")
        .join("inbox")
        .join(&day);

    fs::create_dir_all(&day_dir)
        .map_err(|e| format!("创建目录 {} 失败：{e}", day_dir.display()))?;

    // 2. 文件名：HHmmss-<8位uuid>.md
    let hhmmss = now.format("%H%M%S").to_string();
    let short_id = Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>();
    let filename = format!("{hhmmss}-{short_id}.md");
    let file_path = day_dir.join(&filename);

    // 3. 生成 frontmatter + body
    let content = render_inbox_md(&payload, &now);
    let char_count = payload.text.chars().count();

    crate::inbox_storage::write_new(&file_path, content.as_bytes())
        .map_err(|e| format!("写入 {} 失败：{e}", file_path.display()))?;

    tracing::info!(
        "inbox_submit 完成：{} ({} chars)",
        file_path.display(),
        char_count
    );

    Ok(InboxSubmitResult {
        path: file_path.to_string_lossy().into_owned(),
        filename,
        day,
        char_count,
    })
}

pub(crate) fn submission_store() -> &'static crate::inbox_receipts::ReceiptStore {
    static STORE: std::sync::OnceLock<crate::inbox_receipts::ReceiptStore> = std::sync::OnceLock::new();
    STORE.get_or_init(|| crate::inbox_receipts::ReceiptStore::new(
        crate::adapters::homebase_home_dir().join(".homebase/inbox/receipts"),
        wiki_vault_root(),
    ))
}

/// Query one original durable Inbox intent. Never executes or recreates a note.
#[tauri::command]
pub async fn inbox_submission_get(submission_id: String) -> Result<crate::inbox_receipts::InboxSubmissionView, String> {
    submission_store().get(&submission_id)
}

/// Cancel only a prepared intent before its publication boundary.
#[tauri::command]
pub async fn inbox_submission_cancel(submission_id: String) -> Result<crate::inbox_receipts::InboxSubmissionView, String> {
    submission_store().cancel(&submission_id).await
}

// =====================================================================
// 命令 1.5：上传附件 → 写到 raw/inbox/<day>/_attachments/
// =====================================================================

#[derive(Debug, Deserialize)]
pub struct InboxAttachPayload {
    /// 文件名（来自 File.name 或 paste 自动生成 `screenshot-001.png`）
    pub filename: String,
    /// 文件字节（前端 `Array.from(new Uint8Array(buffer))` 传过来）
    pub bytes: Vec<u8>,
    /// 可选：有效YYYY-MM-DD日期；不填用今天，不接受路径
    #[serde(default)]
    pub day: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InboxAttachResult {
    /// 写入的绝对路径
    pub absolute_path: String,
    /// 完整库内路径 raw/inbox/YYYY-MM-DD/_attachments/文件名，不是操作系统路径
    pub relative_path: String,
    /// 安全后的实际文件名（清洗特殊字符 + 去重序号）
    pub final_filename: String,
    /// 字节大小
    pub size: usize,
}

#[tauri::command]
pub async fn inbox_attach(payload: InboxAttachPayload) -> Result<InboxAttachResult, String> {
    tracing::debug!(
        "inbox_attach 调用：{} ({} bytes)",
        payload.filename,
        payload.bytes.len()
    );

    if payload.bytes.is_empty() {
        return Err("空文件".into());
    }

    // 1. 决定 day 目录
    let day = payload
        .day
        .clone()
        .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string());

    let vault_root = wiki_vault_root();
    // 2. Validate calendar day before any directory write; reserve names atomically.
    let safe = sanitize_attachment_filename(&payload.filename);
    let written = crate::inbox_storage::write_attachment(&vault_root,&day,&safe,&payload.bytes)
        .map_err(crate::inbox_storage::references::display_error)?;
    let final_name = written.filename;
    let abs_path = written.path;

    let rel_path = crate::inbox_storage::references::attachment_reference(&day, &final_name)?;
    let size = payload.bytes.len();

    tracing::info!(
        "inbox_attach 完成：{} ({} bytes)",
        abs_path.display(),
        size
    );

    Ok(InboxAttachResult {
        absolute_path: abs_path.to_string_lossy().into_owned(),
        relative_path: rel_path,
        final_filename: final_name,
        size,
    })
}

/// 清洗文件名 — 替换 Windows 不允许字符 + 限长
fn sanitize_attachment_filename(raw: &str) -> String {
    crate::inbox_storage::references::sanitize_filename(raw)
}

// =====================================================================
// 命令 2：打开 Obsidian 应用
// =====================================================================

#[tauri::command]
pub async fn inbox_open_obsidian(
    vault: Option<String>,
) -> Result<InboxOpenObsidianResult, String> {
    tracing::debug!("inbox_open_obsidian 调用（vault: {:?}）", vault);

    let vault_name = vault.as_deref().unwrap_or("Wiki");
    let uri = format!("obsidian://open?vault={vault_name}");

    // Windows: `cmd /c start <uri>` 让 OS 去找 obsidian:// 协议关联的应用
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = Command::new("cmd")
            .args(["/c", "start", "", &uri])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map_err(|e| format!("启动 Obsidian 失败（cmd start）：{e}"))?;

        if !status.success() {
            return Err(format!("Obsidian 启动返回非 0：{status}"));
        }

        return Ok(InboxOpenObsidianResult {
            opened_with: "cmd_start_uri".into(),
            vault: Some(vault_name.to_string()),
        });
    }

    // macOS: `open obsidian://...`
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg(&uri)
            .status()
            .map_err(|e| format!("启动 Obsidian 失败（open）：{e}"))?;
        return Ok(InboxOpenObsidianResult {
            opened_with: "open_uri".into(),
            vault: Some(vault_name.to_string()),
        });
    }

    // Linux: `xdg-open obsidian://...`
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open")
            .arg(&uri)
            .status()
            .map_err(|e| format!("启动 Obsidian 失败（xdg-open）：{e}"))?;
        Ok(InboxOpenObsidianResult {
            opened_with: "xdg_open_uri".into(),
            vault: Some(vault_name.to_string()),
        })
    }
}

// =====================================================================
// 命令 3：列最近 N 条投料（方便用户确认写进去了）
// =====================================================================

#[derive(Debug, Serialize)]
pub struct InboxRecentEntry {
    pub path: String,
    pub filename: String,
    pub day: String,
    /// 首 60 字符预览
    pub preview: String,
    /// 文件大小（字节）
    pub size: u64,
    /// ISO 8601 修改时间
    pub modified_at: String,
}

#[tauri::command]
pub async fn inbox_list_recent(limit: Option<usize>) -> Result<Vec<InboxRecentEntry>, String> {
    let limit = limit.unwrap_or(10).min(50);
    let vault_root = wiki_vault_root();
    let inbox_root = vault_root.join("raw").join("inbox");

    if !inbox_root.exists() {
        return Ok(Vec::new());
    }

    // 扫 inbox_root/*/*.md 收集所有文件
    let mut entries: Vec<(PathBuf, std::fs::Metadata)> = Vec::new();
    for day_entry in fs::read_dir(&inbox_root).map_err(|e| format!("读 inbox 目录失败：{e}"))? {
        let day_entry = day_entry.map_err(|_| "QUERY_FAILED: Inbox 日期目录条目不可读取")?;
        if !day_entry.file_type().map_err(|_| "QUERY_FAILED: Inbox 条目类型不可读取")?.is_dir() {
            continue;
        }
        for file_entry in crate::query::entries(&day_entry.path(), false)? {
            let path = file_entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let meta = file_entry.metadata().map_err(|_| "QUERY_FAILED: Inbox 文件元数据不可读取")?;
            entries.push((path, meta));
        }
    }

    // 按修改时间倒序
    entries.sort_by(|a, b| {
        b.1.modified()
            .unwrap_or(std::time::UNIX_EPOCH)
            .cmp(&a.1.modified().unwrap_or(std::time::UNIX_EPOCH))
    });
    entries.truncate(limit);

    let result = entries
        .into_iter()
        .map(|(path, meta)| read_inbox_entry(&path, &meta))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(result)
}

fn read_inbox_entry(path: &Path, meta: &std::fs::Metadata) -> Result<InboxRecentEntry, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;

    // 剥 frontmatter 取预览
    let body = if raw.starts_with("---") {
        if let Some(end) = raw[3..].find("---") {
            raw[3 + end + 3..].trim_start()
        } else {
            &raw
        }
    } else {
        &raw
    };
    let preview: String = body.chars().take(60).collect();

    let day = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();

    let modified = meta.modified().map_err(|_| "QUERY_FAILED: Inbox 修改时间不可读取")?;
    let modified_at = chrono::DateTime::<Local>::from(modified).format("%Y-%m-%d %H:%M:%S").to_string();

    Ok(InboxRecentEntry {
        path: path.to_string_lossy().into_owned(),
        filename,
        day,
        preview,
        size: meta.len(),
        modified_at,
    })
}

// =====================================================================
// 内部工具
// =====================================================================

/// 获取 Wiki vault 根目录
///
/// 正常模式沿用 `C:\HomeBase\Wiki`；显式 HOMEBASE_TEST_HOME 使用测试根/Wiki。
/// payload 不选择 vault，不在页面打开时迁移或改写用户配置。
fn wiki_vault_root() -> PathBuf {
    if std::env::var_os("HOMEBASE_TEST_HOME").is_some() {
        return crate::adapters::homebase_home_dir().join("Wiki");
    }
    PathBuf::from("C:\\HomeBase\\Wiki")
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_inbox_md_uses_explicit_title() {
        let p = InboxSubmitPayload {
            text: "hello world".into(),
            submission_id: None,
            title: Some("My Title".into()),
            extra_tags: vec![],
            source_hint: None,
            attachments: vec![],
        };
        let now = Local::now();
        let md = render_inbox_md(&p, &now);
        assert!(md.contains("title: \"My Title\""));
        assert!(md.contains("source: homebase-inbox"));
        assert!(md.contains("\n  - inbox\n"));
        assert!(md.contains("\n  - raw\n"));
        assert!(md.ends_with("hello world\n"));
    }

    #[test]
    fn render_inbox_md_falls_back_to_first_line() {
        let p = InboxSubmitPayload {
            text: "# 关于 context engineering\n\n这是第二行".into(),
            submission_id: None,
            title: None,
            extra_tags: vec![],
            source_hint: None,
            attachments: vec![],
        };
        let now = Local::now();
        let md = render_inbox_md(&p, &now);
        assert!(md.contains("title: \"关于 context engineering\""));
    }

    #[test]
    fn render_inbox_md_dedupes_and_merges_tags() {
        let p = InboxSubmitPayload {
            text: "note".into(),
            submission_id: None,
            title: None,
            extra_tags: vec!["idea".into(), "inbox".into(), "  ".into(), "tech".into()],
            source_hint: Some("weibo".into()),
            attachments: vec![],
        };
        let now = Local::now();
        let md = render_inbox_md(&p, &now);
        // 自动合并：inbox/raw + extra (idea, tech)，且 inbox 不重复
        assert!(md.matches("  - inbox\n").count() == 1);
        assert!(md.contains("  - idea\n"));
        assert!(md.contains("  - tech\n"));
        assert!(md.contains("source: weibo"));
    }

    #[test]
    fn render_inbox_md_escapes_title_quotes() {
        let p = InboxSubmitPayload {
            text: "body".into(),
            submission_id: None,
            title: Some("has \"quotes\"".into()),
            extra_tags: vec![],
            source_hint: None,
            attachments: vec![],
        };
        let now = Local::now();
        let md = render_inbox_md(&p, &now);
        assert!(md.contains("title: \"has \\\"quotes\\\"\""));
    }

    #[test]
    fn render_inbox_md_defaults_title_on_empty_text() {
        let p = InboxSubmitPayload {
            text: "".into(),
            submission_id: None,
            title: None,
            extra_tags: vec![],
            source_hint: None,
            attachments: vec![],
        };
        let now = Local::now();
        let md = render_inbox_md(&p, &now);
        assert!(md.contains("title: \"投料\""));
    }

    #[test]
    fn render_inbox_md_embeds_attachments_yaml_and_body() {
        let p = InboxSubmitPayload {
            text: "text body".into(),
            submission_id: None,
            title: Some("with attachments".into()),
            extra_tags: vec![],
            source_hint: None,
            attachments: vec![
                "_attachments/screenshot-001.png".into(),
                "_attachments/notes.pdf".into(),
            ],
        };
        let now = Local::now();
        let md = render_inbox_md(&p, &now);
        // YAML 列表
        assert!(md.contains("attachments:\n  - _attachments/screenshot-001.png\n  - _attachments/notes.pdf"));
        // 正文末尾 ## 附件 区块
        assert!(md.contains("## 附件"));
        // 图片用 ![[]]
        assert!(md.contains("![[_attachments/screenshot-001.png]]"));
        // 非图片用 [[]]
        assert!(md.contains("[[_attachments/notes.pdf]]"));
        assert!(!md.contains("![[_attachments/notes.pdf]]"));
    }

    #[test]
    fn sanitize_attachment_filename_replaces_bad_chars() {
        assert_eq!(sanitize_attachment_filename("a/b\\c?d.png"), "a_b_c_d.png");
        assert_eq!(sanitize_attachment_filename("  trim.txt  "), "trim.txt");
        assert_eq!(sanitize_attachment_filename(""), "attachment");
        assert_eq!(sanitize_attachment_filename("normal-file.jpg"), "normal-file.jpg");
    }
}
