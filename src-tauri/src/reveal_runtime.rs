//! Shared lookup for the bundled Windows secret helper.
use std::io;
use std::path::{Path, PathBuf};

const NAME: &str = "homebase-reveal.exe";

pub(crate) fn current() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "主程序目录无法定位"))?;
    let configured = std::env::var_os("HOMEBASE_REVEAL_EXE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    Ok(resolve(directory, configured.as_deref()))
}

fn resolve(directory: &Path, configured: Option<&Path>) -> PathBuf {
    let bundled = directory.join("distribution").join("bin").join(NAME);
    let sibling = directory.join(NAME);
    let selected = [configured, Some(bundled.as_path()), Some(sibling.as_path())]
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
        .or(configured)
        .unwrap_or(&bundled);
    selected.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn stale_override_falls_back_to_the_package_then_the_development_sibling() {
        let root = tempfile::tempdir().unwrap();
        let bundled = root.path().join("distribution").join("bin").join(NAME);
        let stale = root.path().join("retired-build").join(NAME);
        let sibling = root.path().join(NAME);
        fs::write(&sibling, []).unwrap();
        assert_eq!(resolve(root.path(), Some(&stale)), sibling);
        fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        fs::write(&bundled, []).unwrap();
        assert_eq!(resolve(root.path(), Some(&stale)), bundled);
        assert_eq!(resolve(root.path(), None), bundled);
    }

    #[test]
    fn valid_explicit_helper_wins_and_directories_do_not_pass() {
        let root = tempfile::tempdir().unwrap();
        let bundled = root.path().join("distribution").join("bin").join(NAME);
        let custom = root.path().join("custom-helper.exe");
        fs::create_dir_all(&bundled).unwrap();
        assert!(!resolve(root.path(), None).is_file());
        let missing = root.path().join("missing-helper.exe");
        assert_eq!(resolve(root.path(), Some(&missing)), missing);
        fs::write(&custom, []).unwrap();
        assert_eq!(resolve(root.path(), Some(&custom)), custom);
    }
}
