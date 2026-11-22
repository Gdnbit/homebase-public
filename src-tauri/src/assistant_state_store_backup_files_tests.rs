use super::*;
use crate::assistant::SendRequest;
use crate::assistant_desktop::{DesktopBody, DesktopStore};
use serde_json::json;

const PDF: &[u8] = b"%PDF-1.4\n% HomeBase isolated backup fixture\n%%EOF\n";
const RESULT: &[u8] = b"original synthetic result\n";
const ARTIFACT: &str = "fixture-artifact";

fn synthetic_image(pixel: [u8; 3]) -> String {
    use base64::Engine as _;
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 1, 1);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&pixel).unwrap();
        writer.finish().unwrap();
    }
    base64::engine::general_purpose::STANDARD.encode(png)
}

#[test]
fn image_backup_rejects_foreign_scope_and_non_desktop_turns() {
    let root = tempfile::tempdir().unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let foreign = ImageReference {
        scope: uuid::Uuid::new_v4().to_string(),
        name: "foreign.png".into(),
        sha256: "a".repeat(64),
        bytes: 70,
    };
    let mut files = FileCollector::new(root.path(), root.path(), root.path().join("documents"));
    assert!(files
        .image(&owner, &foreign, false)
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE:"));
    let segment = Segment {
        format: 1,
        session_id: owner.clone(),
        source_digest: "b".repeat(64),
        kind: Kind::Turns,
        start: 0,
        items: vec![json!({"id":"test-turn", "image":foreign})],
    };
    assert!(files
        .collect(&segment, true)
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE:"));
    let original_new = ImageReference {
        scope: "new".into(),
        ..foreign
    };
    let from_other_channel = Segment {
        items: vec![json!({"id":"test-turn", "image":original_new})],
        ..segment
    };
    assert!(files
        .collect(&from_other_channel, false)
        .unwrap_err()
        .starts_with("ARCHIVE_SCOPE:"));
    assert!(!root.path().join("files").exists());
}

#[tokio::test]
async fn png_backup_restores_draft_pending_and_accepted_images_from_detached_files() {
    let source = tempfile::tempdir().unwrap();
    let assistant = source.path().join("assistant");
    let root = assistant.join("chat-images");
    let images = ImageStore::new(root.clone());
    let original = images
        .stage(
            "new".into(),
            "original.png".into(),
            &synthetic_image([1, 2, 3]),
        )
        .await
        .unwrap();
    let latest = images
        .stage(
            "new".into(),
            "latest.png".into(),
            &synthetic_image([4, 5, 6]),
        )
        .await
        .unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let changed = images
        .stage(
            owner.clone(),
            "changed.png".into(),
            &synthetic_image([7, 8, 9]),
        )
        .await
        .unwrap();
    let changed_path = root.join(format!(
        "{}-{}.png.dpapi",
        digest(owner.as_bytes()),
        changed.sha256
    ));
    fs::write(&changed_path, b"changed fixture ciphertext").unwrap();
    let missing = ImageReference {
        scope: owner.clone(),
        name: "missing.png".into(),
        sha256: "a".repeat(64),
        bytes: 5,
    };
    let captured_original = images.load(&original).unwrap();
    let captured_latest = images.load(&latest).unwrap();
    let drafts = DesktopStore::new(assistant.join("desktop"));
    let prepared = drafts
        .prepare(
            "new",
            0,
            SendRequest {
                image: Some(original.clone()),
                session_id: None,
                text: "原问题".into(),
                request_id: "original-picture".into(),
                mode: Some("chat".into()),
                route_id: Some("openrouter::fixture:free".into()),
                attachments: vec![],
            },
        )
        .await
        .unwrap();
    let saved = drafts
        .save(
            "new",
            prepared.revision,
            DesktopBody {
                image: Some(latest.clone()),
                text: "新问题".into(),
                route: "openrouter::other:free".into(),
                mode: "chat".into(),
                files: vec![],
            },
        )
        .await
        .unwrap();

    let mut writer = backup_store(&assistant).unwrap();
    writer.load_index().unwrap();
    writer
        .save(json!({"sessions":[{
            "id":owner, "source":"desktop", "source_key":"desktop", "messages":[], "jobs":[],
            "turns":[{"id":"accepted-1","image":original}, {"id":"accepted-2","image":original},
                {"id":"accepted-3","image":changed}, {"id":"accepted-4","image":missing}]
        }]}))
        .await
        .unwrap();
    let backup_id = uuid::Uuid::new_v4().to_string();
    let source_backup = writer.backup_directory(&backup_id).unwrap();
    let receipt = writer
        .backup(backup_id.clone(), source.path().join("workspace"))
        .await
        .unwrap();
    let counts = receipt.files.unwrap();
    assert_eq!((counts.saved, counts.unavailable), (4, 2));

    let detached = tempfile::tempdir().unwrap();
    let detached_assistant = detached.path().join("assistant");
    let reader = backup_store(&detached_assistant).unwrap();
    let copy = reader.backup_directory(&backup_id).unwrap();
    for folder in ["history-v1", "history-v1/segments", "files"] {
        fs::create_dir_all(copy.join(folder)).unwrap();
        for file in fs::read_dir(source_backup.join(folder)).unwrap() {
            let file = file.unwrap();
            if file.file_type().unwrap().is_file() {
                fs::copy(file.path(), copy.join(folder).join(file.file_name())).unwrap();
            }
        }
    }
    for manifest in ["files.dpapi", "draft-files.dpapi", "image-files.dpapi"] {
        fs::copy(source_backup.join(manifest), copy.join(manifest)).unwrap();
    }
    let page = reader.backup_files(backup_id.clone(), 0).unwrap();
    assert!(page.images_indexed);
    let draft = page
        .items
        .iter()
        .find(|file| file.kind == ArchiveFileKind::Draft)
        .unwrap();
    let restored = reader
        .backup_draft_get(backup_id.clone(), draft.id.clone())
        .unwrap()
        .entry;
    assert_eq!(restored.revision, saved.revision);
    assert!(restored.body.image.unwrap() == latest);
    let pending = restored.pending.unwrap();
    assert_eq!(pending.request.request_id, "original-picture");
    assert!(pending.request.image.unwrap() == original);

    for image in page
        .items
        .iter()
        .filter(|file| file.kind == ArchiveFileKind::Image)
    {
        if image.state != ArchiveFileState::Saved {
            assert!(
                image.state
                    == if image.name == "changed.png" {
                        ArchiveFileState::Changed
                    } else {
                        ArchiveFileState::Unavailable
                    }
            );
            assert!(reader
                .backup_file_recover(backup_id.clone(), image.id.clone())
                .is_err());
            continue;
        }
        let pixels = if image.sha256 == original.sha256 {
            &captured_original
        } else {
            &captured_latest
        };
        assert!(!fs::read(part_path(&copy, &image.id))
            .unwrap()
            .starts_with(b"\x89PNG\r\n\x1a\n"));
        let recovered = reader
            .backup_file_recover(backup_id.clone(), image.id.clone())
            .unwrap();
        assert_eq!(Path::new(&recovered.path).extension().unwrap(), "png");
        assert_eq!(fs::read(&recovered.path).unwrap(), **pixels);
        let again = reader
            .backup_file_recover(backup_id.clone(), image.id.clone())
            .unwrap();
        assert_eq!(again.path, recovered.path);
    }
    assert_eq!(
        page.items
            .iter()
            .filter(|file| file.session_id == owner && file.sha256 == original.sha256)
            .count(),
        1
    );
    assert!(!detached_assistant.join("desktop").exists());
    assert!(!detached_assistant.join("chat-images").exists());
    assert_eq!(drafts.get("new").unwrap().revision, saved.revision);
    fs::rename(
        copy.join("image-files.dpapi"),
        copy.join("held-image-files.dpapi"),
    )
    .unwrap();
    let legacy = reader.backup_files(backup_id, 0).unwrap();
    assert!(!legacy.images_indexed);
    assert_eq!(legacy.total, 1);
}

async fn source_files(
    assistant: &Path,
    workspace: &Path,
    owner: &str,
    job: &str,
) -> (Value, Vec<Value>) {
    fs::create_dir_all(workspace).unwrap();
    let document = uuid::Uuid::new_v4().to_string();
    let meta = DocumentStore::new(assistant.join("documents"))
        .stage(&document, owner, "原始资料.pdf", PDF)
        .await
        .unwrap();
    let text = "测试摘录";
    let attachment = json!({
        "name":"excerpt.txt","bytes":text.len(),"sha256":digest(text.as_bytes()),
        "document":{"document_id":document, "extraction_id":"1".repeat(64),
            "source_name":meta.name,"source_sha256":meta.source_sha256,"page_from":1,"page_to":1,
            "text_from":0,"text_to":text.len(),"content_sha256":digest(text.as_bytes())}
    });
    let regular = workspace.join("report.txt");
    let changed = workspace.join("changed.txt");
    let outside = assistant.join("outside.txt");
    fs::write(&regular, RESULT).unwrap();
    fs::write(&changed, b"different").unwrap();
    fs::write(&outside, RESULT).unwrap();
    let artifacts = [
        (ARTIFACT, fs::canonicalize(&regular).unwrap()),
        ("fixture-changed", changed),
        ("fixture-missing", workspace.join("missing.txt")),
        ("fixture-outside", outside),
    ]
    .into_iter()
    .map(|(id, path)| {
        json!({
            "id":id,"job_id":job,"name":path.file_name().unwrap().to_string_lossy(),
            "path":path,"bytes":RESULT.len(),"sha256":digest(RESULT),"mime_type":"text/plain"
        })
    })
    .collect();
    (attachment, artifacts)
}

#[tokio::test]
async fn referenced_pdf_and_owned_results_survive_even_after_the_sources_go_away() {
    let home = tempfile::tempdir().unwrap();
    let assistant = home.path().join("assistant");
    let workspace = home.path().join("workspace");
    let owner = uuid::Uuid::new_v4().to_string();
    let (attachment, artifacts) =
        source_files(&assistant, &workspace, &owner, "original-job").await;
    let doc_id = attachment["document"]["document_id"].as_str().unwrap();
    let mut writer = backup_store(&assistant).unwrap();
    writer.load_index().unwrap();
    let saved = writer.save(json!({"sessions":[{
        "id":owner,"source":"desktop","source_key":"desktop","history":[],"turns":[],
        "messages":[{"id":"original-message","role":"user","attachments":[attachment,attachment]}],
        "jobs":[{"id":"original-job","owned":true,"artifacts":artifacts},
            {"id":"observed-job","owned":false,"artifacts":[{"id":"out-of-scope"}]}]
    }]})).await.unwrap();
    let head_before = fs::read(writer.head()).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let receipt = writer.backup(id.clone(), workspace.clone()).await.unwrap();
    let counts = receipt.files.unwrap();
    assert_eq!((counts.saved, counts.unavailable), (2, 3));
    assert_eq!(
        receipt.generation.as_deref(),
        Some(saved.generation.as_str())
    );
    let reader = backup_store(&assistant).unwrap();
    let folder = reader.backup_directory(&id).unwrap();
    let list = reader.backup_files(id.clone(), 0).unwrap();
    assert!(list.indexed);
    assert_eq!(list.total, 5);
    assert_eq!(fs::read_dir(folder.join("files")).unwrap().count(), 2);
    let status = reader.backup_status(id.clone()).unwrap().files.unwrap();
    assert_eq!((status.saved, status.unavailable), (2, 3));

    fs::rename(workspace.join("report.txt"), workspace.join("held.txt")).unwrap();
    let source = assistant
        .join("documents")
        .join(doc_id)
        .join("source.dpapi");
    fs::rename(&source, source.with_extension("held")).unwrap();
    for entry in list.items.iter().filter(|item| item.bytes.is_some()) {
        let expected = if entry.kind == ArchiveFileKind::Document {
            PDF
        } else {
            RESULT
        };
        assert_eq!(entry.sha256, digest(expected));
        assert_ne!(fs::read(part_path(&folder, &entry.id)).unwrap(), expected);
        let recovered = reader
            .backup_file_recover(id.clone(), entry.id.clone())
            .unwrap();
        assert_eq!(fs::read(&recovered.path).unwrap(), expected);
        assert_eq!(
            reader
                .backup_file_recover(id.clone(), entry.id.clone())
                .unwrap()
                .path,
            recovered.path
        );
        if entry.kind == ArchiveFileKind::Artifact {
            fs::write(&recovered.path, vec![b'Z'; expected.len()]).unwrap();
            let failed = reader
                .backup_file_recover(id.clone(), entry.id.clone())
                .err()
                .unwrap();
            assert!(failed.starts_with("ARCHIVE_CHANGED:"));
            assert_eq!(
                fs::read(&recovered.path).unwrap(),
                vec![b'Z'; expected.len()]
            );
        }
    }
    for entry in list.items.iter().filter(|item| item.bytes.is_none()) {
        assert!(reader
            .backup_file_recover(id.clone(), entry.id.clone())
            .err()
            .unwrap()
            .starts_with("ARCHIVE_MISSING:"));
    }
    let changed = list
        .items
        .iter()
        .find(|item| item.original_id == "fixture-changed")
        .unwrap();
    assert!(changed.state == ArchiveFileState::Changed);
    let outside = list
        .items
        .iter()
        .find(|item| item.original_id == "fixture-outside")
        .unwrap();
    assert!(outside.state == ArchiveFileState::Unavailable);
    let document = list
        .items
        .iter()
        .find(|item| item.original_id == doc_id)
        .unwrap();
    let blob = part_path(&folder, &document.id);
    fs::write(
        &blob,
        crate::wechat_crypto::crypt(b"altered", true).unwrap(),
    )
    .unwrap();
    assert!(reader
        .backup_file_recover(id.clone(), document.id.clone())
        .err()
        .unwrap()
        .starts_with("ARCHIVE_CHANGED:"));
    assert_eq!(fs::read(reader.head()).unwrap(), head_before);
    fs::rename(
        folder.join("files.dpapi"),
        folder.join("held-manifest.dpapi"),
    )
    .unwrap();
    fs::rename(
        folder.join("draft-files.dpapi"),
        folder.join("held-draft-manifest.dpapi"),
    )
    .unwrap();
    fs::rename(
        folder.join("image-files.dpapi"),
        folder.join("held-image-manifest.dpapi"),
    )
    .unwrap();
    assert!(!reader.backup_files(id.clone(), 0).unwrap().indexed);
    assert!(reader.backup_status(id).unwrap().files.is_none());
}

#[tokio::test]
#[ignore = "explicit synthetic candidate profile cloned under F lab"]
async fn seed_archive_file_native_profile() {
    let home = PathBuf::from(std::env::var_os("HOMEBASE_FILE_BACKUP_SEED_HOME").unwrap());
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
        .starts_with("archive-file-profile-"));
    safe(&home).unwrap();
    let assistant = home.join(".homebase/assistant");
    let workspace = home.join(".homebase/wechat/workspace");
    let mut store = backup_store(&assistant).unwrap();
    let previous = store.load_index().unwrap().unwrap();
    let mut value = store.materialize(&previous).unwrap();
    value.as_object_mut().unwrap().remove("archive_index");
    let row = &mut value["sessions"][0];
    let owner = row["id"].as_str().unwrap().to_owned();
    let job = row["jobs"][0]["id"].as_str().unwrap().to_owned();
    let (attachment, artifacts) = source_files(&assistant, &workspace, &owner, &job).await;
    let message = row["messages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|message| message["role"] == "user")
        .unwrap();
    message["attachments"] = json!([attachment]);
    row["jobs"][0]["artifacts"] = json!(artifacts);
    let saved = store.save(value).await.unwrap();
    println!(
        "ARCHIVE_FILE_FIXTURE {}",
        json!({
            "generation":saved.generation,"original_document_id":attachment["document"]["document_id"],
            "raw_pdf_sha256":digest(PDF),"raw_result_sha256":digest(RESULT),"artifact_id":ARTIFACT
        })
    );
}
