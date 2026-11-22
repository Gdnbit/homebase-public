use super::*;
use crate::assistant::{SendRequest, TextAttachment};
use crate::assistant_desktop::{DesktopBody, NativeMessage, VoiceJob, VoicePhase};
use serde_json::json;

const ORIGINAL: &str = "原发送正文，等待核对";
const CURRENT: &str = "发送后继续编辑的新草稿";
const SAMPLE: &[u8] = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x40\x1f\0\0\x80\x3e\0\0\x02\0\x10\0data\x04\0\0\0\0\0\0\0";
const PDF: &[u8] = b"%PDF-1.4\n% local synthetic draft source\n%%EOF";

async fn draft_fixture(assistant: &Path) -> (DesktopEntry, String) {
    let store = DesktopStore::new(assistant.join("desktop"));
    let documents = DocumentStore::new(assistant.join("documents"));
    let origin_id = uuid::Uuid::new_v4().to_string();
    let foreign_id = uuid::Uuid::new_v4().to_string();
    let foreign_owner = uuid::Uuid::new_v4().to_string();
    let mut attachments = Vec::new();
    for (id, name, owner) in [
        (&origin_id, "草稿原件.pdf", "new"),
        (&foreign_id, "其他会话资料.pdf", foreign_owner.as_str()),
    ] {
        let meta = documents.stage(id, owner, name, PDF).await.unwrap();
        let text = "合成摘录";
        attachments.push(TextAttachment {
            name: "excerpt.txt".into(),
            content: text.into(),
            document: Some(DocumentRef {
                document_id: meta.id,
                extraction_id: "a".repeat(64),
                source_name: meta.name,
                source_sha256: meta.source_sha256,
                page_from: 1,
                page_to: 1,
                text_from: 0,
                text_to: text.len(),
                content_sha256: digest(text.as_bytes()),
            }),
        });
    }
    let prepared = store
        .prepare(
            "new",
            0,
            SendRequest {
                image: None,
                session_id: None,
                text: ORIGINAL.into(),
                request_id: "original-draft-intent".into(),
                mode: Some("task".into()),
                route_id: Some("synthetic-route".into()),
                attachments,
            },
        )
        .await
        .unwrap();
    store
        .save(
            "new",
            prepared.revision,
            DesktopBody {
                image: None,
                text: CURRENT.into(),
                ..DesktopBody::default()
            },
        )
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let directory = assistant.join("recordings").join(&id);
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("input.wav"), SAMPLE).unwrap();
    store
        .voice_begin("new", id.clone(), digest(SAMPLE))
        .await
        .unwrap();
    assert!(store.voice_prepare("new", &id).await.unwrap());
    let entry = store
        .voice_update(
            "new",
            &id,
            Some(VoiceJob {
                job_id: uuid::Uuid::new_v4().to_string(),
                task_dir: assistant
                    .join("never-read-this-journal")
                    .to_string_lossy()
                    .into_owned(),
            }),
            VoicePhase::Ready,
            Some("已经保全的测试转写".into()),
            "fixture".into(),
        )
        .await
        .unwrap();
    let native = NativeMessage {
        provider: "claude-code".into(),
        thread_id: "fixture-thread".into(),
        text: "合成原生待核对消息".into(),
        request_id: "original-native-request".into(),
    };
    store.native_prepare(0, native.clone()).await.unwrap();
    let missing = uuid::Uuid::new_v4().to_string();
    store
        .voice_begin(&native.scope(), missing, digest(SAMPLE))
        .await
        .unwrap();
    let damaged = assistant
        .join("desktop")
        .join(format!("{}.draft.dpapi", "0".repeat(64)));
    fs::write(damaged, b"unreadable synthetic ciphertext").unwrap();
    (entry, id)
}

#[tokio::test]
async fn archived_drafts_keep_new_edits_original_intents_and_recordings_without_activation() {
    let source = tempfile::tempdir().unwrap();
    let assistant = source.path().join("assistant");
    let (entry, recording_id) = draft_fixture(&assistant).await;
    let name = format!("{}.draft.dpapi", digest(b"new"));
    let original_cipher = fs::read(assistant.join("desktop").join(&name)).unwrap();
    let mut writer = backup_store(&assistant).unwrap();
    writer.load_index().unwrap();
    writer.save(json!({"sessions":[]})).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let backup = writer
        .backup(id.clone(), source.path().join("workspace"))
        .await
        .unwrap();
    let counts = backup.files.unwrap();
    assert_eq!((counts.saved, counts.unavailable), (4, 3));
    let reader = backup_store(&assistant).unwrap();
    let directory = reader.backup_directory(&id).unwrap();
    let assets = reader.backup_files(id.clone(), 0).unwrap();
    assert!(assets.drafts_indexed);
    assert_eq!(assets.unreadable_drafts, 1);
    let new_draft = assets
        .items
        .iter()
        .find(|f| f.kind == ArchiveFileKind::Draft && f.original_id == "new")
        .unwrap();
    let native = assets
        .items
        .iter()
        .find(|f| f.kind == ArchiveFileKind::Draft && f.original_id.starts_with("native:"))
        .unwrap();
    let recording = assets
        .items
        .iter()
        .find(|f| f.original_id == recording_id)
        .unwrap();
    assert!(recording.kind == ArchiveFileKind::Recording);
    assert_eq!(recording.sha256, digest(SAMPLE));
    let expected_pdf = assets
        .items
        .iter()
        .find(|f| f.name == "草稿原件.pdf")
        .unwrap();
    assert!(expected_pdf.state == ArchiveFileState::Saved);
    let foreign_pdf = assets
        .items
        .iter()
        .find(|f| f.name == "其他会话资料.pdf")
        .unwrap();
    assert!(foreign_pdf.state == ArchiveFileState::Unavailable);
    // The original profile is available only to prove that reads/writes did not
    // target it. The following operations use the detached package.
    let detached = tempfile::tempdir().unwrap();
    let detached_assistant = detached.path().join("assistant");
    let separate = backup_store(&detached_assistant).unwrap();
    let copy = separate.backup_directory(&id).unwrap();
    fs::create_dir_all(copy.join("history-v1")).unwrap();
    fs::create_dir(copy.join("files")).unwrap();
    fs::copy(
        directory.join("history-v1/head.dpapi"),
        copy.join("history-v1/head.dpapi"),
    )
    .unwrap();
    for manifest in ["files.dpapi", "draft-files.dpapi"] {
        fs::copy(directory.join(manifest), copy.join(manifest)).unwrap();
    }
    for blob in fs::read_dir(directory.join("files")).unwrap() {
        let blob = blob.unwrap();
        fs::copy(blob.path(), copy.join("files").join(blob.file_name())).unwrap();
    }
    let restored = separate
        .backup_draft_get(id.clone(), new_draft.id.clone())
        .unwrap()
        .entry;
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&entry).unwrap()
    );
    assert_eq!(restored.body.text, CURRENT);
    let pending = restored.pending.unwrap();
    assert_eq!(pending.state, "prepared");
    assert_eq!(pending.request.text, ORIGINAL);
    assert_eq!(pending.request.request_id, "original-draft-intent");
    let native = separate
        .backup_draft_get(id.clone(), native.id.clone())
        .unwrap()
        .entry;
    assert_eq!(
        native.native_pending.unwrap().request_id,
        "original-native-request"
    );
    let file = separate
        .backup_file_recover(id.clone(), new_draft.id.clone())
        .unwrap();
    assert_eq!(
        Path::new(&file.path).file_name().unwrap().to_str(),
        Some(name.as_str())
    );
    assert_eq!(fs::read(&file.path).unwrap(), original_cipher);
    let detached_store = DesktopStore::new(copy.join("recovered/desktop"));
    assert_eq!(detached_store.get("new").unwrap().revision, entry.revision);
    let audio = separate
        .backup_file_recover(id.clone(), recording.id.clone())
        .unwrap();
    assert_eq!(fs::read(audio.path).unwrap(), SAMPLE);
    assert!(separate
        .backup_draft_get(id, recording.id.clone())
        .err()
        .unwrap()
        .starts_with("ARCHIVE_SCOPE:"));
    assert!(!detached_assistant.join("desktop").exists());
    assert!(!detached_assistant.join("recordings").exists());
    assert_eq!(
        fs::read(assistant.join("desktop").join(name)).unwrap(),
        original_cipher
    );
    assert!(!assistant.join("never-read-this-journal").exists());
}

#[tokio::test]
#[ignore = "explicit synthetic v53 candidate profile under F lab"]
async fn seed_native_archive_draft_profile() {
    let home = PathBuf::from(std::env::var_os("HOMEBASE_DRAFT_BACKUP_SEED_HOME").unwrap());
    assert!(home
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .starts_with("F:/HomeBase-Labs/"));
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("draft-archive-profile-"));
    safe(&home).unwrap();
    let assistant = home.join(".homebase/assistant");
    assert!(!assistant.join("desktop").exists());
    assert!(!assistant.join("recordings").exists());
    let (entry, recording_id) = draft_fixture(&assistant).await;
    println!(
        "DRAFT_ARCHIVE_FIXTURE {}",
        json!({
            "scope":entry.scope,"revision":entry.revision,"text":CURRENT,"pending_text":ORIGINAL,
            "pending_id":"original-draft-intent","voice_id":recording_id,"voice_sha":digest(SAMPLE),
            "job_id":entry.voices[0].job.as_ref().unwrap().job_id,
            "native_id":"original-native-request",
        })
    );
}
