//! Exact vault-relative attachment identities. No directory search or content reads.
use std::{fs, io, path::Path};

const INVALID: &str = "INBOX_REFERENCE_INVALID";
const LEGACY: &str = "INBOX_REFERENCE_LEGACY";
const MISSING: &str = "INBOX_REFERENCE_MISSING";
const UNREADABLE: &str = "INBOX_REFERENCE_UNREADABLE";
const LINKED: &str = "INBOX_REFERENCE_LINKED";
const EMPTY: &str = "INBOX_REFERENCE_EMPTY";

pub fn public_error(error: &str) -> Option<(&'static str, &'static str)> {
    let (code, provided) = error.split_once(": ").unwrap_or((error, ""));
    let result = match code {
        INVALID => (INVALID, "附件引用格式无效；请原样使用上传返回的完整库内路径，未收录。"),
        LEGACY => (LEGACY, "旧附件引用缺少原日期，不能确定文件；请核对原上传结果或原日期后使用完整库内路径，未猜测或收录。"),
        MISSING => (MISSING, "指定附件不存在，未收录；请核对原引用，不会寻找同名替代文件。"),
        UNREADABLE => (UNREADABLE, "附件路径属性不可核对或不是普通文件，未上传或收录；请检查原目录和权限。"),
        LINKED => (LINKED, "附件路径含链接或重解析点，未跟随、上传或收录。"),
        EMPTY => (EMPTY, "指定附件为空，未收录；请核对原文件。"),
        _ => return None,
    };
    if provided.is_empty() || provided == result.1 {
        Some(result)
    } else {
        None
    }
}

pub fn display_error(error: String) -> String {
    public_error(&error)
        .map(|(code, message)| format!("{code}: {message}"))
        .unwrap_or(error)
}

pub(super) fn valid_filename(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.trim() == name
        && !name.ends_with('.')
        && !name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*#^[]%".contains(c))
}

pub fn sanitize_filename(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*#^[]%".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        return "attachment".into();
    }
    if trimmed.chars().count() <= 100 {
        return trimmed.into();
    }
    let p = Path::new(trimmed);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment");
    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
    let stem: String = stem.chars().take(80).collect();
    if ext.is_empty() {
        stem
    } else {
        format!("{stem}.{ext}")
    }
}

pub fn attachment_reference(day: &str, filename: &str) -> Result<String, String> {
    super::validate_day(day)?;
    if !valid_filename(filename) {
        return Err("附件文件名无效，未写入。".into());
    }
    Ok(format!("raw/inbox/{day}/_attachments/{filename}"))
}

fn parse(reference: &str) -> Result<(), String> {
    if reference.starts_with("_attachments/") {
        return Err(LEGACY.into());
    }
    let parts: Vec<_> = reference.split('/').collect();
    if parts.len() != 5
        || parts[0] != "raw"
        || parts[1] != "inbox"
        || parts[3] != "_attachments"
        || super::validate_day(parts[2]).is_err()
        || !valid_filename(parts[4])
    {
        return Err(INVALID.into());
    }
    Ok(())
}

fn check_metadata(path: &Path, allow_missing: bool, directory: bool) -> Result<bool, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return if allow_missing {
                Ok(false)
            } else {
                Err(MISSING.into())
            }
        }
        Err(_) => return Err(UNREADABLE.into()),
    };
    let mut linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        linked |= metadata.file_attributes() & 0x400 != 0;
    }
    if linked {
        return Err(LINKED.into());
    }
    if directory && !metadata.is_dir() {
        return Err(UNREADABLE.into());
    }
    Ok(true)
}

// Check existing components before traversing them. This is a path boundary,
// not an OS sandbox against concurrent external renames.
pub(super) fn check_path(root: &Path, reference: &str, allow_missing: bool) -> Result<(), String> {
    parse(reference)?;
    if !root.is_absolute() {
        return Err(INVALID.into());
    }
    for ancestor in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if !check_metadata(ancestor, allow_missing, true)? {
            return Ok(());
        }
    }
    let mut path = root.to_path_buf();
    for (index, component) in reference.split('/').enumerate() {
        path.push(component);
        if !check_metadata(&path, allow_missing, index < 4)? {
            return Ok(());
        }
    }
    Ok(())
}

pub fn validate_references(root: &Path, references: &[String]) -> Result<(), String> {
    // Reject malformed input before looking up even an earlier valid reference.
    for reference in references {
        parse(reference)?;
    }
    for reference in references {
        check_path(root, reference, false)?;
        let metadata = fs::symlink_metadata(root.join(reference)).map_err(|_| UNREADABLE)?;
        if !metadata.file_type().is_file() {
            return Err(UNREADABLE.into());
        }
        if metadata.len() == 0 {
            return Err(EMPTY.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seed(root: &Path, day: &str, name: &str, bytes: &[u8]) -> String {
        let reference = attachment_reference(day, name).unwrap();
        let path = root.join(&reference);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
        reference
    }
    #[test]
    fn dates_are_part_of_identity_and_validate_without_reading_contents() {
        let temp = tempfile::tempdir().unwrap();
        let one = seed(temp.path(), "2026-11-15", "same.txt", b"FIRST");
        let two = seed(temp.path(), "2026-11-16", "same.txt", b"SECOND");
        assert_ne!(one, two);
        validate_references(temp.path(), &[one.clone(), two.clone()]).unwrap();
        assert_eq!(fs::read(temp.path().join(one)).unwrap(), b"FIRST");
        assert_eq!(fs::read(temp.path().join(two)).unwrap(), b"SECOND");
    }
    #[test]
    fn malformed_or_undated_references_never_search_or_create_directories() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("missing-Wiki");
        for reference in [
            "../outside",
            "/raw/inbox/2026-11-15/_attachments/a",
            "C:/private/a",
            "raw/inbox/2026-02-30/_attachments/a",
            "raw/inbox/2026-11-15/_attachments/../a",
            "raw/inbox/2026-11-15/_attachments/a#heading",
            "raw/inbox/2026-11-15/_attachments/a\nstatus:raw",
            "raw/inbox/2026-11-15/_attachments/a:stream",
            "raw/inbox/2026-11-15/_attachments/a.",
        ] {
            assert_eq!(
                validate_references(&root, &[reference.into()]),
                Err(INVALID.into())
            );
        }
        assert_eq!(
            validate_references(&root, &["_attachments/a".into()]),
            Err(LEGACY.into())
        );
        assert!(!root.exists());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        validate_references(&root, &[]).unwrap();
    }
    #[test]
    fn missing_empty_and_directory_are_distinct_and_do_not_fall_back_by_name() {
        let temp = tempfile::tempdir().unwrap();
        seed(temp.path(), "2026-11-15", "same.txt", b"KEEP");
        let missing = attachment_reference("2026-11-16", "same.txt").unwrap();
        assert_eq!(
            validate_references(temp.path(), &[missing]),
            Err(MISSING.into())
        );
        let empty = seed(temp.path(), "2026-11-15", "empty.txt", b"");
        assert_eq!(
            validate_references(temp.path(), &[empty]),
            Err(EMPTY.into())
        );
        let dir = attachment_reference("2026-11-15", "dir").unwrap();
        fs::create_dir(temp.path().join(&dir)).unwrap();
        assert_eq!(
            validate_references(temp.path(), &[dir]),
            Err(UNREADABLE.into())
        );
        assert_eq!(
            fs::read(
                temp.path()
                    .join("raw/inbox/2026-11-15/_attachments/same.txt")
            )
            .unwrap(),
            b"KEEP"
        );
    }
    #[test]
    fn new_names_cannot_change_wikilink_or_yaml_meaning() {
        for raw in [
            "report#[x]%%.png",
            "pic^block|alias.png",
            "name\nstatus:published.md",
            ".. ",
            "截图🙂.png",
        ] {
            let name = sanitize_filename(raw);
            assert!(valid_filename(&name));
            let reference = attachment_reference("2026-11-15", &name).unwrap();
            parse(&reference).unwrap();
        }
        assert_eq!(sanitize_filename("截图🙂.png"), "截图🙂.png");
        assert_eq!(sanitize_filename("report#[x]%%.png"), "report__x___.png");
    }
    #[test]
    fn public_errors_are_fixed_and_never_echo_arbitrary_values() {
        for code in [INVALID, LEGACY, MISSING, UNREADABLE, LINKED, EMPTY] {
            let (returned, message) = public_error(code).unwrap();
            assert_eq!(returned, code);
            assert!(!message.is_empty());
            assert!(public_error(&format!("{code}:PRIVATE_MARKER")).is_none());
            assert!(public_error(&format!("{code}: PRIVATE_MARKER")).is_none());
            let displayed = display_error(code.into());
            assert!(displayed.contains(message));
            assert_eq!(public_error(&displayed), Some((returned, message)));
        }
    }
    #[test]
    #[ignore = "explicit isolated Node junction probe required on Windows"]
    fn linked_directories_are_rejected_before_upload_or_reference_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("Wiki");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("same.txt"), b"PRESERVE").unwrap();
        let linked = root.join("raw");
        fs::create_dir_all(&root).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let node = std::env::var_os("HOMEBASE_ISOLATION_NODE").expect("explicit isolated Node");
            assert!(std::process::Command::new(node)
                .args([
                    "-e",
                    "require('fs').symlinkSync(process.argv[1],process.argv[2],'junction')"
                ])
                .arg(&outside)
                .arg(&linked)
                .creation_flags(0x08000000)
                .status()
                .unwrap()
                .success());
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &linked).unwrap();
        let reference = attachment_reference("2026-11-15", "same.txt").unwrap();
        assert_eq!(check_path(&root, &reference, true), Err(LINKED.into()));
        assert_eq!(validate_references(&root, &[reference]), Err(LINKED.into()));
        assert!(super::super::write_attachment(&root, "2026-11-15", "new.txt", b"NO").is_err());
        assert_eq!(fs::read(outside.join("same.txt")).unwrap(), b"PRESERVE");
        assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
    }
}
