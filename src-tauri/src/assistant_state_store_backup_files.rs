//! Files referenced by an authenticated archive generation.
use super::*;
use crate::assistant::{Artifact, AttachmentInfo, ImageReference, ImageStore};
use crate::assistant_documents::{DocumentRef, DocumentStore};
#[path = "assistant_state_store_backup_drafts.rs"]
mod drafts;
pub(crate) use drafts::ArchiveDraftView;

const FILE_LIMIT: usize = 20 * 1024 * 1024;
const PAGE_SIZE: usize = 20;

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFileKind {
    Document,
    Artifact,
    Draft,
    Recording,
    Image,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FilePart {
    Session,
    Desktop,
    Image,
}

impl FilePart {
    fn of(kind: ArchiveFileKind) -> Self {
        match kind {
            ArchiveFileKind::Draft | ArchiveFileKind::Recording => Self::Desktop,
            ArchiveFileKind::Image => Self::Image,
            ArchiveFileKind::Document | ArchiveFileKind::Artifact => Self::Session,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveFileState {
    Saved,
    Unavailable,
    Changed,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveFile {
    pub id: String,
    pub kind: ArchiveFileKind,
    pub state: ArchiveFileState,
    pub original_id: String,
    pub session_id: String,
    pub name: String,
    pub sha256: String,
    pub bytes: Option<usize>,
}

#[derive(Serialize)]
pub struct ArchiveFileCounts {
    pub saved: usize,
    pub unavailable: usize,
}

#[derive(Serialize)]
pub struct ArchiveFilePage {
    pub backup_id: String,
    pub indexed: bool,
    pub total: usize,
    pub items: Vec<ArchiveFile>,
    pub next_offset: Option<usize>,
    pub unreadable_drafts: usize,
    pub drafts_indexed: bool,
    pub images_indexed: bool,
}

#[derive(Serialize)]
pub struct ArchiveRecoveredFile {
    pub backup_id: String,
    pub file: ArchiveFile,
    pub path: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileManifest {
    version: u32,
    generation: String,
    items: Vec<ArchiveFile>,
    #[serde(default, skip_serializing_if = "empty_count")]
    unreadable_drafts: usize,
    #[serde(skip)]
    drafts_indexed: bool,
    #[serde(skip)]
    images_indexed: bool,
}

fn empty_count(value: &usize) -> bool {
    *value == 0
}

fn name_is_safe(name: &str) -> bool {
    identifier(name, 1024, false) && !name.contains(['/', '\\', ':'])
}

fn valid_image_entry(file: &ArchiveFile) -> bool {
    let Some((scope, hash)) = file.original_id.split_once(':') else {
        return false;
    };
    (file.session_id == "new" || uuid(&file.session_id))
        && (scope == "new" || scope == file.session_id)
        && hash == file.sha256
        && ImageReference {
            scope: scope.into(),
            name: file.name.clone(),
            sha256: file.sha256.clone(),
            bytes: file.bytes.unwrap_or(1),
        }
        .validate(scope)
        .is_ok()
}

fn part_path(directory: &Path, id: &str) -> PathBuf {
    directory.join("files").join(format!("{id}.dpapi"))
}

impl FileManifest {
    pub(super) fn read(directory: &Path, generation: &str) -> Result<Option<Self>, String> {
        let Some(mut result) = Self::read_part(
            &directory.join("files.dpapi"),
            generation,
            FilePart::Session,
        )?
        else {
            for name in ["draft-files.dpapi", "image-files.dpapi"] {
                let part = directory.join(name);
                safe(&part)?;
                if part.try_exists().map_err(|_| error("ARCHIVE_IO"))? {
                    return Err(error("ARCHIVE_MISSING"));
                }
            }
            return Ok(None);
        };
        for (name, part) in [
            ("draft-files.dpapi", FilePart::Desktop),
            ("image-files.dpapi", FilePart::Image),
        ] {
            if let Some(extra) = Self::read_part(&directory.join(name), generation, part)? {
                let mut ids: BTreeSet<_> =
                    result.items.iter().map(|file| file.id.as_str()).collect();
                if extra.items.iter().any(|file| !ids.insert(&file.id)) {
                    return Err(error("ARCHIVE_IDENTITY"));
                }
                result.items.extend(extra.items);
                if part == FilePart::Desktop {
                    result.unreadable_drafts = extra.unreadable_drafts;
                    result.drafts_indexed = true;
                }
                result.images_indexed |= part == FilePart::Image;
            }
        }
        Ok(Some(result))
    }

    fn read_part(path: &Path, generation: &str, part: FilePart) -> Result<Option<Self>, String> {
        let Some(cipher) = read_bounded(path, CIPHER_LIMIT)? else {
            return Ok(None);
        };
        let result: Self = decode(&cipher)?;
        let mut ids = BTreeSet::new();
        if result.version != 1
            || result.generation != generation
            || result.unreadable_drafts > crate::assistant_desktop::MAX_SCOPES
            || part != FilePart::Desktop && result.unreadable_drafts != 0
            || result.items.iter().any(|file| {
                !uuid(&file.id)
                    || !ids.insert(&file.id)
                    || !identifier(&file.original_id, 256, false)
                    || !identifier(&file.session_id, 256, false)
                    || !name_is_safe(&file.name)
                    || !sha(&file.sha256)
                    || FilePart::of(file.kind) != part
                    || file.kind == ArchiveFileKind::Image && !valid_image_entry(file)
                    || match file.state {
                        ArchiveFileState::Saved => {
                            file.bytes.is_none_or(|size| size == 0 || size > FILE_LIMIT)
                                || file.kind == ArchiveFileKind::Draft
                                    && !crate::assistant_desktop::DesktopStore::archive_filename(
                                        &file.name,
                                    )
                        }
                        _ => file.bytes.is_some(),
                    }
            })
        {
            return Err(error("ARCHIVE_INVALID"));
        }
        Ok(Some(result))
    }

    pub(super) fn counts(&self) -> ArchiveFileCounts {
        let saved = self
            .items
            .iter()
            .filter(|file| file.state == ArchiveFileState::Saved)
            .count();
        ArchiveFileCounts {
            saved,
            unavailable: self.items.len() - saved + self.unreadable_drafts,
        }
    }
}

pub(super) struct FileCollector<'a> {
    directory: &'a Path,
    workspace: &'a Path,
    documents: DocumentStore,
    images: ImageStore,
    image_claims: BTreeSet<(String, String, String, usize)>,
    claims: BTreeMap<String, (String, String)>,
    artifact_ids: BTreeSet<String>,
    owned_jobs: BTreeSet<String>,
    items: Vec<ArchiveFile>,
    unreadable_drafts: usize,
}

impl<'a> FileCollector<'a> {
    pub(super) fn new(directory: &'a Path, workspace: &'a Path, documents: PathBuf) -> Self {
        let documents = DocumentStore::new(documents);
        Self {
            directory,
            workspace,
            images: ImageStore::new(documents.image_directory()),
            image_claims: BTreeSet::new(),
            documents,
            claims: BTreeMap::new(),
            artifact_ids: BTreeSet::new(),
            owned_jobs: BTreeSet::new(),
            items: Vec::new(),
            unreadable_drafts: 0,
        }
    }

    pub(super) fn collect(&mut self, segment: &Segment, desktop: bool) -> Result<(), String> {
        for record in &segment.items {
            match segment.kind {
                Kind::Messages if record["role"] == "user" => {
                    let Some(attachments) = record.get("attachments") else {
                        continue;
                    };
                    let attachments = attachments
                        .as_array()
                        .ok_or_else(|| error("ARCHIVE_INVALID"))?;
                    for raw in attachments.iter().filter(|raw| !raw["document"].is_null()) {
                        let attachment: AttachmentInfo = serde_json::from_value(raw.clone())
                            .map_err(|_| error("ARCHIVE_INVALID"))?;
                        let reference = attachment
                            .document
                            .ok_or_else(|| error("ARCHIVE_INVALID"))?;
                        reference.validate().map_err(|_| error("ARCHIVE_INVALID"))?;
                        if reference.content_sha256 != attachment.sha256
                            || attachment.bytes != reference.text_to - reference.text_from
                        {
                            return Err(error("ARCHIVE_SCOPE"));
                        }
                        self.document(&segment.session_id, reference, true)?;
                    }
                }
                Kind::Jobs if record["owned"] == true => {
                    let job = record["id"]
                        .as_str()
                        .ok_or_else(|| error("ARCHIVE_IDENTITY"))?;
                    if !self.owned_jobs.insert(job.to_owned()) {
                        return Err(error("ARCHIVE_IDENTITY"));
                    }
                    let Some(artifacts) = record.get("artifacts") else {
                        continue;
                    };
                    for raw in artifacts
                        .as_array()
                        .ok_or_else(|| error("ARCHIVE_INVALID"))?
                    {
                        let artifact: Artifact = serde_json::from_value(raw.clone())
                            .map_err(|_| error("ARCHIVE_INVALID"))?;
                        if artifact.job_id != job || !self.artifact_ids.insert(artifact.id.clone())
                        {
                            return Err(error("ARCHIVE_SCOPE"));
                        }
                        self.artifact(&segment.session_id, artifact)?;
                    }
                }
                Kind::Turns => {
                    if let Some(reference) = crate::assistant::archived_image(record)
                        .map_err(|_| error("ARCHIVE_INVALID"))?
                    {
                        if !desktop || !uuid(&segment.session_id) {
                            return Err(error("ARCHIVE_SCOPE"));
                        }
                        self.image(&segment.session_id, &reference, true)?;
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }

    fn image(
        &mut self,
        owner: &str,
        reference: &ImageReference,
        accepted: bool,
    ) -> Result<(), String> {
        let scope = if accepted && reference.scope == "new" {
            "new"
        } else {
            owner
        };
        reference
            .validate(scope)
            .map_err(|_| error("ARCHIVE_SCOPE"))?;
        if self.image_claims.insert((
            owner.into(),
            scope.into(),
            reference.sha256.clone(),
            reference.bytes,
        )) {
            self.save_file(
                owner,
                ArchiveFileKind::Image,
                format!("{scope}:{}", reference.sha256),
                reference.name.clone(),
                reference.sha256.clone(),
                self.images.archive_source(reference),
            )?;
        }
        Ok(())
    }

    fn document(
        &mut self,
        session: &str,
        reference: DocumentRef,
        linked_message: bool,
    ) -> Result<(), String> {
        let claim = (
            reference.source_sha256.clone(),
            reference.source_name.clone(),
        );
        if let Some(previous) = self.claims.get(&reference.document_id) {
            return if *previous == claim {
                Ok(())
            } else {
                Err(error("ARCHIVE_SCOPE"))
            };
        }
        let bytes = self
            .documents
            .archive_source(session, &reference, linked_message);
        if bytes.is_ok() || linked_message {
            self.claims.insert(reference.document_id.clone(), claim);
        }
        self.save_file(
            session,
            ArchiveFileKind::Document,
            reference.document_id,
            reference.source_name,
            reference.source_sha256,
            bytes,
        )
    }

    fn artifact(&mut self, session: &str, artifact: Artifact) -> Result<(), String> {
        let bytes = artifact_bytes(&artifact, self.workspace);
        self.save_file(
            session,
            ArchiveFileKind::Artifact,
            artifact.id,
            artifact.name,
            artifact.sha256,
            bytes,
        )
    }

    fn save_file(
        &mut self,
        session: &str,
        kind: ArchiveFileKind,
        original_id: String,
        name: String,
        sha256: String,
        bytes: Result<Zeroizing<Vec<u8>>, String>,
    ) -> Result<(), String> {
        if !identifier(&original_id, 256, false) || !name_is_safe(&name) || !sha(&sha256) {
            return Err(error("ARCHIVE_INVALID"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (state, size) = match bytes {
            Ok(bytes)
                if !bytes.is_empty() && bytes.len() <= FILE_LIMIT && digest(&bytes) == sha256 =>
            {
                let cipher = crate::wechat_crypto::crypt(&bytes, true)
                    .map_err(|_| error("ARCHIVE_CRYPTO"))?;
                let path = part_path(self.directory, &id);
                safe(&path)?;
                fs::create_dir_all(path.parent().ok_or_else(|| error("ARCHIVE_PATH"))?)
                    .map_err(|_| error("ARCHIVE_IO"))?;
                write_new(&path, &cipher)?;
                (ArchiveFileState::Saved, Some(bytes.len()))
            }
            Ok(_) => (ArchiveFileState::Changed, None),
            Err(failure) if failure.starts_with("ARCHIVE_CHANGED:") => {
                (ArchiveFileState::Changed, None)
            }
            Err(_) => (ArchiveFileState::Unavailable, None),
        };
        self.items.push(ArchiveFile {
            id,
            kind,
            state,
            session_id: session.into(),
            original_id,
            name,
            sha256,
            bytes: size,
        });
        Ok(())
    }

    pub(super) fn finish(self, generation: &str) -> Result<ArchiveFileCounts, String> {
        let (images, others): (Vec<_>, Vec<_>) = self
            .items
            .into_iter()
            .partition(|file| file.kind == ArchiveFileKind::Image);
        let (drafts, files) = others
            .into_iter()
            .partition(|file| FilePart::of(file.kind) == FilePart::Desktop);
        let manifest = FileManifest {
            version: 1,
            generation: generation.into(),
            items: files,
            unreadable_drafts: 0,
            drafts_indexed: false,
            images_indexed: false,
        };
        let drafts = FileManifest {
            version: 1,
            generation: generation.into(),
            items: drafts,
            unreadable_drafts: self.unreadable_drafts,
            drafts_indexed: true,
            images_indexed: false,
        };
        write_new(
            &self.directory.join("files.dpapi"),
            &encode(&manifest, CIPHER_LIMIT)?,
        )?;
        write_new(
            &self.directory.join("draft-files.dpapi"),
            &encode(&drafts, CIPHER_LIMIT)?,
        )?;
        let images = FileManifest {
            version: 1,
            generation: generation.into(),
            items: images,
            unreadable_drafts: 0,
            drafts_indexed: false,
            images_indexed: true,
        };
        write_new(
            &self.directory.join("image-files.dpapi"),
            &encode(&images, CIPHER_LIMIT)?,
        )?;
        let files = manifest.counts();
        let drafts = drafts.counts();
        let images = images.counts();
        Ok(ArchiveFileCounts {
            saved: files.saved + drafts.saved + images.saved,
            unavailable: files.unavailable + drafts.unavailable + images.unavailable,
        })
    }
}

fn artifact_bytes(artifact: &Artifact, workspace: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    let path = Path::new(&artifact.path);
    safe(workspace)?;
    let root = fs::canonicalize(workspace).map_err(|_| error("ARCHIVE_PATH"))?;
    // Only touch paths relative to the configured workspace. Registered
    // results may carry either the original root or its canonical spelling.
    let relative = path
        .strip_prefix(workspace)
        .or_else(|_| path.strip_prefix(&root))
        .map_err(|_| error("ARCHIVE_SCOPE"))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(error("ARCHIVE_SCOPE"));
    }
    let scoped = root.join(relative);
    safe(&scoped)?;
    let real = fs::canonicalize(scoped).map_err(|_| error("ARCHIVE_PATH"))?;
    if real == root || !real.starts_with(&root) {
        return Err(error("ARCHIVE_SCOPE"));
    }
    let bytes = read_bounded(&real, FILE_LIMIT)?.ok_or_else(|| error("ARCHIVE_MISSING"))?;
    if bytes.len() as u64 != artifact.bytes {
        return Err(error("ARCHIVE_CHANGED"));
    }
    Ok(Zeroizing::new(bytes))
}

impl Store {
    pub(crate) fn backup_files(
        &self,
        backup_id: String,
        offset: usize,
    ) -> Result<ArchiveFilePage, String> {
        let directory = self.backup_directory(&backup_id)?;
        let pinned = PinnedArchive::read(&backup_store(&directory)?)?;
        let manifest = FileManifest::read(&directory, &pinned.index.generation)?;
        let indexed = manifest.is_some();
        let drafts_indexed = manifest
            .as_ref()
            .is_some_and(|manifest| manifest.drafts_indexed);
        let images_indexed = manifest
            .as_ref()
            .is_some_and(|manifest| manifest.images_indexed);
        let unreadable_drafts = manifest
            .as_ref()
            .map_or(0, |manifest| manifest.unreadable_drafts);
        let items = manifest.map_or_else(Vec::new, |manifest| manifest.items);
        let total = items.len();
        if offset > total {
            return Err(error("ARCHIVE_RANGE"));
        }
        let end = offset.saturating_add(PAGE_SIZE).min(total);
        Ok(ArchiveFilePage {
            backup_id,
            indexed,
            total,
            items: items.into_iter().skip(offset).take(end - offset).collect(),
            next_offset: (end < total).then_some(end),
            unreadable_drafts,
            drafts_indexed,
            images_indexed,
        })
    }

    fn archived_file(
        &self,
        backup_id: &str,
        file_id: &str,
    ) -> Result<(PathBuf, ArchiveFile, Zeroizing<Vec<u8>>), String> {
        if !uuid(file_id) {
            return Err(error("ARCHIVE_IDENTITY"));
        }
        let directory = self.backup_directory(backup_id)?;
        let pinned = PinnedArchive::read(&backup_store(&directory)?)?;
        let file = FileManifest::read(&directory, &pinned.index.generation)?
            .and_then(|manifest| manifest.items.into_iter().find(|entry| entry.id == file_id))
            .ok_or_else(|| error("ARCHIVE_RECORD_MISSING"))?;
        let size = file.bytes.ok_or_else(|| error("ARCHIVE_MISSING"))?;
        let cipher = read_bounded(&part_path(&directory, &file.id), CIPHER_LIMIT)?
            .ok_or_else(|| error("ARCHIVE_MISSING"))?;
        let bytes =
            crate::wechat_crypto::crypt(&cipher, false).map_err(|_| error("ARCHIVE_CRYPTO"))?;
        if bytes.len() != size || digest(&bytes) != file.sha256 {
            return Err(error("ARCHIVE_CHANGED"));
        }
        Ok((directory, file, bytes))
    }

    pub(crate) fn backup_file_recover(
        &self,
        backup_id: String,
        file_id: String,
    ) -> Result<ArchiveRecoveredFile, String> {
        let (directory, file, bytes) = self.archived_file(&backup_id, &file_id)?;
        let is_draft = file.kind == ArchiveFileKind::Draft;
        let recovered = directory.join(if is_draft {
            "recovered/desktop"
        } else {
            "recovered"
        });
        safe(&recovered)?;
        fs::create_dir_all(&recovered).map_err(|_| error("ARCHIVE_IO"))?;
        let extension = file
            .name
            .rsplit_once('.')
            .map(|(_, ext)| ext)
            .filter(|ext| {
                !ext.is_empty() && ext.len() <= 10 && ext.bytes().all(|b| b.is_ascii_alphanumeric())
            });
        let name = if is_draft {
            file.name.clone()
        } else {
            match extension {
                Some(ext) => format!("{}.{}", file.id, ext),
                None => file.id.clone(),
            }
        };
        let path = recovered.join(name);
        safe(&path)?;
        if path.try_exists().map_err(|_| error("ARCHIVE_IO"))? || !publish_new(&path, &bytes)? {
            let existing =
                read_bounded(&path, bytes.len())?.ok_or_else(|| error("ARCHIVE_MISSING"))?;
            if existing.len() != bytes.len() || digest(&existing) != file.sha256 {
                return Err(error("ARCHIVE_CHANGED"));
            }
        }
        Ok(ArchiveRecoveredFile {
            backup_id,
            file,
            path: path.to_string_lossy().into_owned(),
        })
    }
}

#[cfg(test)]
#[path = "assistant_state_store_backup_files_tests.rs"]
mod tests;
