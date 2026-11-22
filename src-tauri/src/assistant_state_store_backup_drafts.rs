//! Archive client input and original receipt identity without activating it.
use super::*;
use crate::assistant_desktop::{DesktopEntry, DesktopStore, VoiceRecord};

const RECORDING_LIMIT: usize = 10 * 1024 * 1024;

#[derive(Serialize)]
pub struct ArchiveDraftView {
    pub backup_id: String,
    pub file_id: String,
    pub entry: DesktopEntry,
}

impl FileCollector<'_> {
    pub(in super::super) fn collect_desktop(&mut self, assistant: &Path) -> Result<(), String> {
        let mut recordings = BTreeMap::new();
        DesktopStore::new(assistant.join("desktop")).archive_drafts(|captured| {
            let Some((entry, ciphertext)) = captured.contents else {
                self.unreadable_drafts += 1;
                return Ok(());
            };
            self.save_file(
                &entry.scope,
                ArchiveFileKind::Draft,
                entry.scope.clone(),
                captured.name,
                digest(&ciphertext),
                Ok(Zeroizing::new(ciphertext)),
            )?;
            let pending_files = entry
                .pending
                .iter()
                .flat_map(|pending| &pending.request.attachments);
            let mut references = BTreeSet::new();
            for attachment in entry.body.files.iter().chain(pending_files) {
                if let Some(reference) = &attachment.document {
                    if references.insert((&reference.document_id, &reference.source_sha256)) {
                        self.document(&entry.scope, reference.clone(), false)?;
                    }
                }
            }
            let pending_image = entry
                .pending
                .as_ref()
                .and_then(|pending| pending.request.image.as_ref());
            for reference in entry.body.image.iter().chain(pending_image) {
                self.image(&entry.scope, reference, false)?;
            }
            for voice in &entry.voices {
                if let Some(previous) = recordings.get(&voice.recording_id) {
                    if previous != &voice.audio_sha256 {
                        return Err(error("ARCHIVE_SCOPE"));
                    }
                    continue;
                }
                recordings.insert(voice.recording_id.clone(), voice.audio_sha256.clone());
                let (name, bytes) = match recording_input(assistant, voice) {
                    Ok((name, bytes)) => (name, Ok(bytes)),
                    Err(failure) => ("录音原件".into(), Err(failure)),
                };
                self.save_file(
                    &entry.scope,
                    ArchiveFileKind::Recording,
                    voice.recording_id.clone(),
                    name,
                    voice.audio_sha256.to_ascii_lowercase(),
                    bytes,
                )?;
            }
            Ok(())
        })
    }
}

fn recording_input(
    assistant: &Path,
    voice: &VoiceRecord,
) -> Result<(String, Zeroizing<Vec<u8>>), String> {
    if !uuid(&voice.recording_id) {
        return Err(error("ARCHIVE_IDENTITY"));
    }
    let directory = assistant.join("recordings").join(&voice.recording_id);
    let mut source = None;
    for name in ["input.webm", "input.ogg", "input.wav"] {
        let path = directory.join(name);
        safe(&path)?;
        if path.try_exists().map_err(|_| error("ARCHIVE_IO"))? {
            if source.is_some() {
                return Err(error("ARCHIVE_SCOPE"));
            }
            source = Some((name, path));
        }
    }
    let (name, path) = source.ok_or_else(|| error("ARCHIVE_MISSING"))?;
    let bytes = read_bounded(&path, RECORDING_LIMIT)?.ok_or_else(|| error("ARCHIVE_MISSING"))?;
    Ok((name.into(), Zeroizing::new(bytes)))
}

impl Store {
    pub(crate) fn backup_draft_get(
        &self,
        backup_id: String,
        file_id: String,
    ) -> Result<ArchiveDraftView, String> {
        let (directory, file, ciphertext) = self.archived_file(&backup_id, &file_id)?;
        if file.kind != ArchiveFileKind::Draft {
            return Err(error("ARCHIVE_SCOPE"));
        }
        // The original filename and scope still authenticate the recovered
        // draft. This operation never reads or writes the live DesktopStore.
        let entry = DesktopStore::new(directory.join("recovered/desktop"))
            .read_archived(&file.name, &ciphertext)
            .map_err(|_| error("ARCHIVE_INVALID"))?;
        if entry.scope != file.original_id || entry.scope != file.session_id {
            return Err(error("ARCHIVE_SCOPE"));
        }
        Ok(ArchiveDraftView {
            backup_id,
            file_id,
            entry,
        })
    }
}

#[cfg(test)]
#[path = "assistant_state_store_backup_drafts_tests.rs"]
mod tests;
