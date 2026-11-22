//! Explicit test profiles take precedence over legacy machine-specific defaults.
//! This is a test boundary, not an operating-system sandbox or production policy.
use std::path::{Component, Path, PathBuf};

pub(crate) fn test_path(relative: &str) -> Option<PathBuf> {
    std::env::var_os("HOMEBASE_TEST_HOME").map(|home| PathBuf::from(home).join(relative))
}

pub(crate) fn wiki_root() -> PathBuf {
    test_path("Wiki").unwrap_or_else(|| PathBuf::from(r"C:\HomeBase\Wiki"))
}

pub(crate) fn manual_index() -> PathBuf {
    test_path("Desktop/本机操作指南/INDEX.md")
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\homebase\INDEX.md"))
}

pub(crate) fn studio_root() -> PathBuf {
    test_path("Wiki").unwrap_or_else(|| {
        std::env::var("HB_VAULT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(r"C:\HomeBase\Wiki"))
    })
}

fn ordinary_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| match component {
            Component::ParentDir => false,
            Component::Normal(name) => name
                .to_str()
                .is_some_and(|name| !name.ends_with(['.', ' ']) && !name.contains(':')),
            _ => true,
        })
}

fn check_under(home: &Path, path: &Path) -> Result<(), String> {
    if !ordinary_absolute(home) || !ordinary_absolute(path) || !path.starts_with(home) {
        return Err(
            "ISOLATION_SCOPE: 测试路径必须位于显式绝对测试根内，未回退或读取真实资料".into(),
        );
    }
    let mut current = home.to_path_buf();
    let mut paths = vec![current.clone()];
    for component in path
        .strip_prefix(home)
        .map_err(|_| "ISOLATION_SCOPE")?
        .components()
    {
        current.push(component);
        paths.push(current.clone());
    }
    for current in paths {
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                let mut linked = metadata.file_type().is_symlink();
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    linked |= metadata.file_attributes() & 0x400 != 0;
                }
                if linked {
                    return Err("ISOLATION_SCOPE: 测试资料路径包含链接或重解析点，未跟随".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return Err("QUERY_FAILED: 测试资料路径不可核对，未读取或回退真实资料".into()),
        }
    }
    Ok(())
}

pub(crate) fn ensure_test_path(path: &Path) -> Result<(), String> {
    if let Some(home) = std::env::var_os("HOMEBASE_TEST_HOME") {
        check_under(&PathBuf::from(home), path)?;
    }
    Ok(())
}

pub(crate) fn ensure_test_url(value: &str) -> Result<(), String> {
    if std::env::var_os("HOMEBASE_TEST_HOME").is_none() {
        return Ok(());
    }
    let url = reqwest::Url::parse(value).map_err(|_| "ISOLATION_SCOPE: 测试API地址无效")?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
    {
        return Err("ISOLATION_SCOPE: 测试API只允许回环地址，未访问真实服务".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_isolation_scope_rejects_escape_alias_and_invalid_roots_without_reading_outside() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("profile");
        std::fs::create_dir(&home).unwrap();
        let marker = root.path().join("outside.md");
        std::fs::write(&marker, "EXTERNAL-SYNTHETIC-SENTINEL").unwrap();
        assert!(check_under(&home, &home.join("Wiki/中文.md")).is_ok());
        for path in [
            marker.clone(),
            home.join("../outside.md"),
            home.join("Wiki/.. /outside.md"),
            home.join("Wiki/item.md:stream"),
        ] {
            assert!(check_under(&home, &path).is_err());
        }
        assert!(check_under(Path::new("relative-profile"), &home).is_err());
        assert_eq!(
            std::fs::read_to_string(marker).unwrap(),
            "EXTERNAL-SYNTHETIC-SENTINEL"
        );
    }
}
