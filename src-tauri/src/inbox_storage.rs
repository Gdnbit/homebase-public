//! Inbox file creation boundaries. No profile discovery or credential access.
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[path = "inbox_references.rs"]
pub mod references;

// Only fixed, public guidance may cross the CLI/channel error boundary. Never
// return an arbitrary filesystem message, filename, or user-supplied value.
pub fn public_error(error: &str) -> Option<(&'static str, &'static str)> {
    if let Some(error) = references::public_error(error) {
        return Some(error);
    }
    match error {
        "附件日期必须为有效的 YYYY-MM-DD，不接受路径。" => Some((
            "INBOX_ATTACHMENT_DAY_INVALID",
            "附件日期必须为有效的 YYYY-MM-DD，不接受路径；未写入。",
        )),
        "附件文件名无效，未写入。" => {
            Some(("INBOX_ATTACHMENT_NAME_INVALID", "附件文件名无效，未写入。"))
        }
        "空文件" => Some(("INBOX_ATTACHMENT_EMPTY", "附件为空，未写入。")),
        "同名附件数量已达上限，未覆盖已有文件。" => Some((
            "INBOX_ATTACHMENT_LIMIT",
            "同名附件数量已达上限，未覆盖已有文件。",
        )),
        _ if error.starts_with("创建附件目录失败（") => Some((
            "INBOX_ATTACHMENT_DIRECTORY_FAILED",
            "附件目录不可创建，请检查目录和权限；未覆盖已有文件。",
        )),
        _ if error.starts_with("附件写入未完成（") => Some((
            "INBOX_ATTACHMENT_WRITE_UNCONFIRMED",
            "附件写入未完成，可能已保留部分文件；请核对原任务和附件目录，不要自动换请求重复写入。",
        )),
        _ => None,
    }
}

pub fn validate_day(day: &str) -> Result<(), String> {
    let b = day.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
        || day.starts_with("0000-")
        || chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_err()
    {
        return Err("附件日期必须为有效的 YYYY-MM-DD，不接受路径。".into());
    }
    Ok(())
}

// create_new is one atomic decision; checking exists before fs::write is not.
pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)
}

pub struct AttachmentWrite {
    pub path: PathBuf,
    pub filename: String,
}
pub fn write_attachment(
    root: &Path,
    day: &str,
    safe_name: &str,
    bytes: &[u8],
) -> Result<AttachmentWrite, String> {
    validate_day(day)?;
    if !references::valid_filename(safe_name) {
        return Err("附件文件名无效，未写入。".into());
    }
    if bytes.is_empty() {
        return Err("空文件".into());
    }
    let reference = references::attachment_reference(day, safe_name)?;
    references::check_path(root, &reference, true)?;
    let dir = root
        .join("raw")
        .join("inbox")
        .join(day)
        .join("_attachments");
    fs::create_dir_all(&dir)
        .map_err(|e| format!("创建附件目录失败（{:?}）；未覆盖已有文件。", e.kind()))?;
    references::check_path(root, &reference, true)?;
    let p = Path::new(safe_name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    for i in 0..10_000 {
        let filename = if i == 0 {
            safe_name.to_string()
        } else if ext.is_empty() {
            format!("{stem}-{i:03}")
        } else {
            format!("{stem}-{i:03}.{ext}")
        };
        let path = dir.join(&filename);
        match write_new(&path, bytes) {
            Ok(()) => return Ok(AttachmentWrite { path, filename }),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("附件写入未完成（{:?}）：{day}/_attachments/{filename}；请核对原文件，不自动重复写入。",e.kind())),
        }
    }
    Err("同名附件数量已达上限，未覆盖已有文件。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_errors_explain_recovery_without_exposing_private_values() {
        let invalid = validate_day("../../../private-marker").unwrap_err();
        let (code, message) = public_error(&invalid).unwrap();
        assert_eq!(code, "INBOX_ATTACHMENT_DAY_INVALID");
        assert!(message.contains("YYYY-MM-DD") && message.contains("未写入"));
        for (error, expected) in [
            ("附件文件名无效，未写入。", "INBOX_ATTACHMENT_NAME_INVALID"),
            ("空文件", "INBOX_ATTACHMENT_EMPTY"),
            (
                "同名附件数量已达上限，未覆盖已有文件。",
                "INBOX_ATTACHMENT_LIMIT",
            ),
            (
                "创建附件目录失败（PermissionDenied）：PRIVATE_MARKER",
                "INBOX_ATTACHMENT_DIRECTORY_FAILED",
            ),
            (
                "附件写入未完成（Other）：PRIVATE_MARKER",
                "INBOX_ATTACHMENT_WRITE_UNCONFIRMED",
            ),
        ] {
            let (code, message) = public_error(error).unwrap();
            assert_eq!(code, expected);
            assert!(!message.contains("PRIVATE_MARKER"));
        }
        assert!(public_error("untrusted PRIVATE_MARKER").is_none());
        assert!(public_error("空文件 PRIVATE_MARKER").is_none());
        let message = public_error("附件写入未完成（Other）：PRIVATE_MARKER")
            .unwrap()
            .1;
        assert!(message.contains("可能") && message.contains("不要自动"));
    }
    #[test]
    fn directory_write_failure_is_explicit_and_preserves_the_existing_source() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("Wiki");
        fs::write(&blocked, b"PRESERVE").unwrap();
        let error = write_attachment(&blocked, "2026-11-17", "fixture.txt", b"synthetic")
            .err()
            .unwrap();
        assert_eq!(error, "INBOX_REFERENCE_UNREADABLE");
        assert!(public_error(&error).unwrap().1.contains("未上传或收录"));
        assert_eq!(fs::read(&blocked).unwrap(), b"PRESERVE");
    }
    #[test]
    fn only_exact_valid_calendar_days_are_accepted() {
        for day in ["2026-11-17", "2024-02-29", "0001-01-01"] {
            validate_day(day).unwrap();
        }
        for day in [
            "../../../escape",
            "C:\\outside",
            "2026-2-03",
            "2026-02-29",
            "2026-02-30",
            "0000-01-01",
            "2026-11-17/../escape",
            "2026-11-17 ",
            "２０２６-11-17",
        ] {
            assert!(validate_day(day).is_err(), "{day}");
        }
    }
    #[test]
    fn invalid_day_or_name_cannot_create_any_output_directory() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Wiki");
        for day in ["../../../outside", "../invalid", "2026-02-30"] {
            assert!(write_attachment(&root, day, "fixture.txt", b"synthetic").is_err());
        }
        assert!(write_attachment(&root, "2026-11-17", "../escape.txt", b"synthetic").is_err());
        assert!(write_attachment(&root, "2026-11-17", "fixture.txt", b"").is_err());
        assert!(!root.exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
    #[test]
    fn creating_a_note_never_truncates_an_existing_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("original.md");
        write_new(&path, b"PRESERVE").unwrap();
        assert_eq!(
            write_new(&path, b"REPLACE").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(path).unwrap(), b"PRESERVE");
    }
    #[test]
    fn concurrent_same_named_attachments_keep_every_original_payload() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Wiki");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let handles = (0u8..16)
            .map(|i| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let bytes = vec![i; 256];
                    let written =
                        write_attachment(&root, "2026-11-17", "合成资料.txt", &bytes).unwrap();
                    (written.path, bytes)
                })
            })
            .collect::<Vec<_>>();
        let files = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>();
        let unique = files
            .iter()
            .map(|(p, _)| p)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), 16);
        for (path, bytes) in files {
            assert!(path.starts_with(&root));
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
}
