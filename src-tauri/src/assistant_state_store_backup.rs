//! A local DPAPI backup of one published archive generation.
use super::*;
use std::io::{ErrorKind, Write};
#[path = "assistant_state_store_backup_files.rs"]
mod files;
pub(crate) use files::{ArchiveDraftView, ArchiveFilePage, ArchiveRecoveredFile};
use files::{ArchiveFileCounts, FileCollector, FileManifest};

#[derive(Serialize)]
pub struct ArchiveBackup {
    pub backup_id: String,
    pub state: ArchiveBackupState,
    pub directory: Option<String>,
    pub generation: Option<String>,
    pub format: Option<u32>,
    pub segments: Option<usize>,
    pub encrypted_bytes: Option<u64>,
    pub files: Option<ArchiveFileCounts>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveBackupState {
    NotFound,
    Incomplete,
    Published,
}

struct PinnedArchive {
    index: Index,
    head: Vec<u8>,
    anchor: Option<Vec<u8>>,
}

impl PinnedArchive {
    fn read(store: &Store) -> Result<Self, String> {
        let head = read_bounded(&store.head(), CIPHER_LIMIT)?
            .ok_or_else(|| error("ARCHIVE_BACKUP_NO_INDEX"))?;
        let index: Index = decode(&head)?;
        validate(&index)?;
        let anchor = read_bounded(&store.legacy, CIPHER_LIMIT)?;
        if index.anchor_sha256 != anchor.as_deref().map(digest) {
            return Err(error("ARCHIVE_LEGACY_CONFLICT"));
        }
        Ok(Self {
            index,
            head,
            anchor,
        })
    }

    fn receipt(&self, backup_id: String, directory: &Path) -> Result<ArchiveBackup, String> {
        let references = self.index.series.iter().flat_map(|series| &series.segments);
        let encrypted_bytes = references
            .clone()
            .try_fold(
                (self.head.len() + self.anchor.as_ref().map_or(0, Vec::len)) as u64,
                |bytes, reference| bytes.checked_add(reference.cipher_bytes as u64),
            )
            .ok_or_else(|| error("ARCHIVE_LIMIT"))?;
        Ok(ArchiveBackup {
            backup_id,
            state: ArchiveBackupState::Published,
            directory: Some(directory.to_string_lossy().into_owned()),
            generation: Some(self.index.generation.clone()),
            format: Some(self.index.format),
            segments: Some(references.count()),
            encrypted_bytes: Some(encrypted_bytes),
            files: None,
        })
    }

    fn publish(
        mut self,
        source: &Store,
        workspace: &Path,
        directory: &Path,
        backup_id: String,
    ) -> Result<ArchiveBackup, String> {
        let mut receipt = self.receipt(backup_id, directory)?;
        let target = backup_store(directory)?;
        safe(&target.root)?;
        fs::create_dir(&target.root).map_err(|_| error("ARCHIVE_IO"))?;
        let segments = target.root.join("segments");
        fs::create_dir(&segments).map_err(|_| error("ARCHIVE_IO"))?;
        if let Some(anchor) = self.anchor.take() {
            write_new(&target.legacy, &anchor)?;
        }
        let source_directory = source.root.parent().ok_or_else(|| error("ARCHIVE_PATH"))?;
        let mut files =
            FileCollector::new(directory, workspace, source_directory.join("documents"));
        let desktop_sessions: BTreeSet<_> = self.index.metadata["sessions"]
            .as_array()
            .ok_or_else(|| error("ARCHIVE_INVALID"))?
            .iter()
            .filter(|session| session["source"] == "desktop" && session["source_key"] == "desktop")
            .filter_map(|session| session["id"].as_str())
            .collect();
        for series in &self.index.series {
            for reference in &series.segments {
                // Verify the file being copied even when an active reader has
                // already cached this segment. No plaintext is published.
                let bytes = segment_cipher(&source.segment(&reference.file)?, reference)?;
                let segment = decode_segment(&bytes, reference)?;
                validate_segment(&segment, series, reference)?;
                files.collect(
                    &segment,
                    desktop_sessions.contains(series.session_id.as_str()),
                )?;
                write_new(&target.segment(&reference.file)?, &bytes)?;
            }
        }
        files.collect_desktop(source_directory)?;
        receipt.files = Some(files.finish(&self.index.generation)?);
        #[cfg(unix)]
        for path in [
            segments,
            target.root.clone(),
            directory.join("files"),
            directory.to_path_buf(),
        ] {
            if !path.exists() {
                continue;
            }
            fs::File::open(path)
                .and_then(|file| file.sync_all())
                .map_err(|_| error("ARCHIVE_IO"))?;
        }
        // The final head is the publication boundary. A killed or failed copy
        // leaves a directory whose status is explicitly incomplete.
        if !publish_new(&target.head(), &self.head)? {
            return Err(error("ARCHIVE_BACKUP_EXISTS"));
        }
        Ok(receipt)
    }
}

fn publish_new(path: &Path, bytes: &[u8]) -> Result<bool, String> {
    safe(path)?;
    let parent = path.parent().ok_or_else(|| error("ARCHIVE_PATH"))?;
    let name = path.file_name().ok_or_else(|| error("ARCHIVE_PATH"))?;
    let dir = fs::canonicalize(parent).map_err(|_| error("ARCHIVE_IO"))?;
    let mut staged = tempfile::NamedTempFile::new_in(&dir).map_err(|_| error("ARCHIVE_IO"))?;
    staged
        .write_all(bytes)
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|_| error("ARCHIVE_IO"))?;
    match staged.persist_noclobber(dir.join(name)) {
        Ok(_) => Ok(true),
        Err(failure) if failure.error.kind() == ErrorKind::AlreadyExists => Ok(false),
        Err(_) => Err(error("ARCHIVE_IO")),
    }
}

fn backup_store(directory: &Path) -> Result<Store, String> {
    Store::new(directory.join("history-v1"), directory.join("state.dpapi"))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    safe(path)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| error("ARCHIVE_IO"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| error("ARCHIVE_IO"))
}

impl Store {
    fn backup_directory(&self, backup_id: &str) -> Result<PathBuf, String> {
        if !uuid(backup_id) {
            return Err(error("ARCHIVE_IDENTITY"));
        }
        let directory = self
            .root
            .parent()
            .ok_or_else(|| error("ARCHIVE_PATH"))?
            .join("backups")
            .join(backup_id);
        safe(&directory)?;
        Ok(directory)
    }

    pub(crate) async fn backup(
        self,
        backup_id: String,
        workspace: PathBuf,
    ) -> Result<ArchiveBackup, String> {
        let directory = self.backup_directory(&backup_id)?;
        let (source, pinned, directory) = tokio::task::spawn_blocking(move || {
            if directory.try_exists().map_err(|_| error("ARCHIVE_IO"))? {
                return Err(error("ARCHIVE_BACKUP_EXISTS"));
            }
            let pinned = PinnedArchive::read(&self)?;
            fs::create_dir_all(directory.parent().ok_or_else(|| error("ARCHIVE_PATH"))?)
                .map_err(|_| error("ARCHIVE_IO"))?;
            safe(&directory)?;
            fs::create_dir(&directory).map_err(|failure| {
                error(if failure.kind() == ErrorKind::AlreadyExists {
                    "ARCHIVE_BACKUP_EXISTS"
                } else {
                    "ARCHIVE_IO"
                })
            })?;
            Ok::<_, String>((self, pinned, directory))
        })
        .await
        .map_err(|_| error("ARCHIVE_IO"))??;
        crate::control_protocol::private_directory(&directory)
            .await
            .map_err(|_| error("ARCHIVE_IO"))?;
        tokio::task::spawn_blocking(move || {
            pinned.publish(&source, &workspace, &directory, backup_id)
        })
        .await
        .map_err(|_| error("ARCHIVE_IO"))?
    }

    pub(crate) fn backup_status(&self, backup_id: String) -> Result<ArchiveBackup, String> {
        let directory = self.backup_directory(&backup_id)?;
        let exists = directory.try_exists().map_err(|_| error("ARCHIVE_IO"))?;
        if exists && !directory.is_dir() {
            return Err(error("ARCHIVE_PATH"));
        }
        let target = backup_store(&directory)?;
        match PinnedArchive::read(&target) {
            Ok(published) => {
                let mut receipt = published.receipt(backup_id, &directory)?;
                receipt.files = FileManifest::read(&directory, &published.index.generation)?
                    .map(|manifest| manifest.counts());
                Ok(receipt)
            }
            Err(failure) if failure.starts_with("ARCHIVE_BACKUP_NO_INDEX:") => Ok(ArchiveBackup {
                backup_id,
                state: if exists {
                    ArchiveBackupState::Incomplete
                } else {
                    ArchiveBackupState::NotFound
                },
                directory: exists.then(|| directory.to_string_lossy().into_owned()),
                generation: None,
                format: None,
                segments: None,
                encrypted_bytes: None,
                files: None,
            }),
            Err(failure) => Err(failure),
        }
    }
}

#[cfg(test)]
#[path = "assistant_state_store_backup_tests.rs"]
mod tests;
