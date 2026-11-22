use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub snapshot_id: String,
    pub created_at: String,
    pub reason: String,
    pub actor: String,
    pub files: Vec<SnapshotFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotFile {
    pub source: String,
    pub exists: bool,
    pub kind: String,
    pub stored_as: Option<String>,
    pub size_bytes: Option<u64>,
}

pub fn snapshots_root() -> PathBuf {
    super::homebase_home_dir()
        .join(".homebase")
        .join("snapshots")
}

pub fn create_snapshot(reason: &str, paths: &[PathBuf]) -> Result<SnapshotManifest, String> {
    let snapshot_id = format!(
        "{}-{}",
        Local::now().format("%Y%m%d-%H%M%S"),
        Uuid::new_v4().simple()
    );
    let root = snapshots_root().join(&snapshot_id);
    let files_root = root.join("files");
    std::fs::create_dir_all(&files_root).map_err(|e| format!("create_dir {files_root:?}: {e}"))?;

    let mut manifest = SnapshotManifest {
        snapshot_id,
        created_at: Local::now().to_rfc3339(),
        reason: reason.into(),
        actor: "homebase".into(),
        files: Vec::new(),
    };

    for (idx, path) in paths.iter().enumerate() {
        let source = path.to_string_lossy().into_owned();
        if !path.exists() {
            manifest.files.push(SnapshotFile {
                source,
                exists: false,
                kind: "missing".into(),
                stored_as: None,
                size_bytes: None,
            });
            continue;
        }

        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("path");
        let dest_name = format!("{idx:02}-{file_name}");
        let dest = files_root.join(&dest_name);
        let metadata = std::fs::metadata(path).map_err(|e| format!("metadata {path:?}: {e}"))?;
        if metadata.is_dir() {
            copy_dir_recursive(path, &dest)?;
            manifest.files.push(SnapshotFile {
                source,
                exists: true,
                kind: "directory".into(),
                stored_as: Some(format!("files/{dest_name}")),
                size_bytes: None,
            });
        } else {
            std::fs::copy(path, &dest).map_err(|e| format!("copy {path:?} -> {dest:?}: {e}"))?;
            manifest.files.push(SnapshotFile {
                source,
                exists: true,
                kind: "file".into(),
                stored_as: Some(format!("files/{dest_name}")),
                size_bytes: Some(metadata.len()),
            });
        }
    }

    let manifest_path = root.join("manifest.json");
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("serialize snapshot manifest: {e}"))?;
    std::fs::write(&manifest_path, json)
        .map_err(|e| format!("write snapshot manifest {manifest_path:?}: {e}"))?;

    Ok(manifest)
}

pub fn list_snapshots(limit: usize) -> Result<Vec<SnapshotManifest>, String> {
    let root = snapshots_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).map_err(|e| format!("read_dir {root:?}: {e}"))? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => return Err("QUERY_FAILED: 快照目录条目不可读取".into()),
        };
        let path = entry.path().join("manifest.json");
        if !path.exists() {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return Err("QUERY_FAILED: 快照清单不可读取".into()),
        };
        match serde_json::from_str::<SnapshotManifest>(&text) {
            Ok(m) => out.push(m),
            Err(_) => return Err("QUERY_INVALID: 快照清单格式无效".into()),
        }
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out.truncate(limit.max(1));
    Ok(out)
}

pub fn rollback_snapshot(snapshot_id: &str) -> Result<SnapshotManifest, String> {
    if snapshot_id.contains("..") || snapshot_id.contains('\\') || snapshot_id.contains('/') {
        return Err("invalid snapshot_id".into());
    }
    let root = snapshots_root().join(snapshot_id);
    let manifest_path = root.join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read snapshot manifest {manifest_path:?}: {e}"))?;
    let manifest: SnapshotManifest =
        serde_json::from_str(&text).map_err(|e| format!("parse snapshot manifest: {e}"))?;

    let existing_targets: Vec<PathBuf> = manifest
        .files
        .iter()
        .map(|file| PathBuf::from(&file.source))
        .filter(|path| path.exists())
        .collect();
    if !existing_targets.is_empty() {
        let _ = create_snapshot(&format!("before rollback {snapshot_id}"), &existing_targets)?;
    }

    for file in &manifest.files {
        let Some(stored_as) = &file.stored_as else {
            continue;
        };
        if stored_as.contains("..") {
            return Err(format!("invalid stored path in snapshot: {stored_as}"));
        }
        let stored = root.join(stored_as);
        let target = PathBuf::from(&file.source);
        match file.kind.as_str() {
            "file" => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("create_dir {parent:?}: {e}"))?;
                }
                std::fs::copy(&stored, &target)
                    .map_err(|e| format!("restore file {stored:?} -> {target:?}: {e}"))?;
            }
            "directory" => {
                if target.exists() {
                    std::fs::remove_dir_all(&target)
                        .map_err(|e| format!("remove_dir_all {target:?}: {e}"))?;
                }
                copy_dir_recursive(&stored, &target)?;
            }
            _ => {}
        }
    }

    Ok(manifest)
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("create_dir {dest:?}: {e}"))?;
    for entry in walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let p = entry.path();
        let rel = p
            .strip_prefix(src)
            .map_err(|e| format!("strip_prefix {src:?}: {e}"))?;
        let target = dest.join(rel);
        if p.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| format!("create_dir {target:?}: {e}"))?;
        } else if p.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("create_dir {parent:?}: {e}"))?;
            }
            std::fs::copy(p, &target)
                .map_err(|e| format!("copy {} -> {}: {e}", p.display(), target.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::shared_context::tests::TEST_ENV_LOCK;

    #[test]
    fn snapshot_copies_existing_file_and_records_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());

        let src = tmp.path().join("source.txt");
        std::fs::write(&src, "hello").unwrap();
        let missing = tmp.path().join("missing.txt");
        let manifest = create_snapshot("test", &[src.clone(), missing]).unwrap();
        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.files[0].exists);
        assert!(!manifest.files[1].exists);
        let stored = manifest.files[0].stored_as.as_ref().unwrap();
        assert!(snapshots_root()
            .join(&manifest.snapshot_id)
            .join(stored)
            .exists());

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }

    #[test]
    fn rollback_restores_file() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = TEST_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("HOMEBASE_TEST_HOME").ok();
        std::env::set_var("HOMEBASE_TEST_HOME", tmp.path());

        let src = tmp.path().join("source.txt");
        std::fs::write(&src, "before").unwrap();
        let manifest = create_snapshot("test", &[src.clone()]).unwrap();
        std::fs::write(&src, "after").unwrap();
        rollback_snapshot(&manifest.snapshot_id).unwrap();
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "before");

        match prev {
            Some(v) => std::env::set_var("HOMEBASE_TEST_HOME", v),
            None => std::env::remove_var("HOMEBASE_TEST_HOME"),
        }
    }
}
