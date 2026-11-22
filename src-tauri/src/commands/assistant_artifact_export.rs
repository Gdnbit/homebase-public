use std::{io::Write, path::Path};

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

use crate::assistant::Artifact;

static EXPORT_DIALOG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Serialize)]
pub struct ExportReceipt {
    name: String,
    bytes: u64,
    sha256: String,
}

pub(super) async fn run(
    app: tauri::AppHandle,
    artifact_id: String,
) -> Result<Option<ExportReceipt>, String> {
    let _dialog = EXPORT_DIALOG
        .try_lock()
        .map_err(|_| "已有导出操作，请先完成或取消。")?;
    let window = app
        .get_webview_window("main")
        .ok_or("找不到 HomeBase 窗口。")?;
    let (send, receive) = tokio::sync::oneshot::channel();
    let mut picker = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("选择产物副本的保存目录");
    if let Some(test_home) = crate::legacy_isolation::test_path("") {
        crate::legacy_isolation::ensure_test_path(&test_home)?;
        picker = picker.set_directory(test_home);
    }
    picker.pick_folder(move |folder| {
        let _ = send.send(folder);
    });
    let Some(folder) = receive.await.map_err(|_| "未收到目录选择结果。")? else {
        return Ok(None);
    };
    let directory = folder
        .into_path()
        .map_err(|_| "请选择能够写入的本机目录。")?;
    crate::legacy_isolation::ensure_test_path(&directory)?;
    let (artifact, bytes) = super::service()
        .await?
        .artifact_content(&artifact_id, false)
        .await?;
    tauri::async_runtime::spawn_blocking(move || write_copy(&directory, artifact, &bytes))
        .await
        .map_err(|_| "未能确认导出完成，请检查所选目录。".to_owned())?
        .map(Some)
}

fn write_copy(
    directory: &Path,
    artifact: Artifact,
    verified_bytes: &[u8],
) -> Result<ExportReceipt, String> {
    let name = &artifact.name;
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    let device = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(*prefix)
            .is_some_and(|tail| tail.chars().count() == 1 && tail.chars().all(char::is_numeric))
    });
    if name.is_empty()
        || name.ends_with(['.', ' '])
        || device
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return Err("登记的文件名无法用于本机导出。".into());
    }
    let target = directory.join(name);
    crate::legacy_isolation::ensure_test_path(&target)?;
    crate::assistant_state_store::safe(directory).map_err(|_| "所选目录无法安全访问。")?;
    let failed = "无法写入副本，请检查所选目录、权限和可用空间。";
    let mut staged = tempfile::NamedTempFile::new_in(directory).map_err(|_| failed)?;
    staged
        .write_all(verified_bytes)
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|_| failed)?;
    staged.persist_noclobber(&target).map_err(|error| {
        if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            "所选目录已有同名文件，请选择其他目录。".to_owned()
        } else {
            failed.to_owned()
        }
    })?;
    Ok(ExportReceipt {
        name: artifact.name,
        bytes: artifact.bytes,
        sha256: artifact.sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::Barrier, thread};

    fn artifact(name: &str, bytes: u64) -> Artifact {
        Artifact {
            id: "synthetic-artifact".into(),
            job_id: "synthetic-job".into(),
            name: name.into(),
            path: String::new(),
            bytes,
            sha256: "fixture-digest".into(),
            mime_type: "text/plain".into(),
        }
    }

    #[test]
    fn concurrent_export_publishes_one_complete_copy_and_cleans_staged_files() {
        let directory = tempfile::tempdir().unwrap();
        let barrier = Barrier::new(2);
        let bytes = "隔离结果\n".as_bytes();
        let filename = "隔离报告.md";
        let start = || {
            barrier.wait();
            write_copy(
                directory.path(),
                artifact(filename, bytes.len() as u64),
                bytes,
            )
        };
        let results = thread::scope(|scope| {
            let first = scope.spawn(start);
            let second = scope.spawn(start);
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results
            .iter()
            .any(|result| matches!(result, Err(error) if error.contains("同名"))));
        assert_eq!(fs::read(directory.path().join(filename)).unwrap(), bytes);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        let receipt = results.into_iter().find_map(Result::ok).unwrap();
        assert_eq!(receipt.name, filename);
        assert_eq!(receipt.bytes, bytes.len() as u64);
        assert_eq!(receipt.sha256, "fixture-digest");
    }

    #[test]
    fn unsafe_export_names_leave_the_selected_directory_empty() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "",
            "..",
            "../out.md",
            "report.md:stream",
            "COM1.txt",
            "result.md ",
        ] {
            assert!(write_copy(directory.path(), artifact(name, 3), b"abc").is_err());
        }
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
