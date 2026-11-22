use super::*;
use crate::control_state::{Controller, Runner};
use std::time::Instant;

const COMMAND: &str = "capability_assemble_video";
const REQUEST_ID: &str = "synthetic-clip-crash-request";
const CRASH_DIR: &str = "HOMEBASE_CLIP_CRASH_DIR";
const CRASH_STAGE: &str = "HOMEBASE_CLIP_CRASH_STAGE";
const TEST_NAME: &str =
    "wechat::clip_video::recovery_tests::real_encoder_crash_preserves_only_committed_results";

fn worker_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os(CRASH_DIR)?);
    assert!(dir.is_absolute());
    assert!(dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("clip-crash-"));
    assert_eq!(
        std::fs::read(dir.join("synthetic-owner")).unwrap(),
        b"homebase-clip-crash-fixture"
    );
    Some(dir)
}

pub(super) fn slow_encoding() -> bool {
    std::env::var(CRASH_STAGE).as_deref() == Ok("encoding") && worker_dir().is_some()
}

pub(super) fn record_encoder(child: &tokio::process::Child) {
    if slow_encoding() {
        let dir = worker_dir().unwrap();
        let Some(pid) = child.id() else { return };
        std::fs::write(dir.join("encoder.pid"), pid.to_string()).unwrap();
    }
}

fn input() -> Value {
    json!({
        "source":{"receipt_id":"wx-9a9896e6-9e49-4518-b282-2023e4af646b",
        "session_id":"bba9063a-4b64-4b4a-901d-77933f562801","sha256":"a".repeat(64)},
        "segments":[{"startMs":0,"durationMs":6000},{"startMs":9000,"durationMs":500}]
    })
}

fn catalog() -> Value {
    serde_json::from_str(include_str!("../../control/commands.json")).unwrap()
}

fn request(method: &str, job_id: Option<&str>) -> Request {
    Request {
        protocol: 1,
        id: format!("synthetic-{}", uuid::Uuid::new_v4()),
        token: "synthetic-local-test".into(),
        method: method.into(),
        command: Some(COMMAND.into()),
        args: if method == "call" { input() } else { json!({}) },
        request_id: Some(REQUEST_ID.into()),
        job_id: job_id.map(str::to_owned),
        catalog_hash: catalog()["catalog_hash"].as_str().map(str::to_owned),
        grants: vec!["write".into(), "private".into()],
    }
}

fn save_marker(dir: &Path, name: &str, value: &Value) {
    let path = dir.join(name);
    let staging = path.with_extension("pending");
    std::fs::write(&staging, serde_json::to_vec(value).unwrap()).unwrap();
    std::fs::rename(staging, path).unwrap();
}

async fn worker(dir: PathBuf, ffmpeg: PathBuf, stage: String) {
    let root = dir.clone();
    let mode = stage.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let (root, ffmpeg, mode) = (root.clone(), ffmpeg.clone(), mode.clone());
        Box::pin(async move {
            assert_eq!(command, COMMAND);
            assert_eq!(args["segments"], input()["segments"]);
            let ranges: Vec<ClipRange> = serde_json::from_value(args["segments"].clone()).unwrap();
            let bytes = std::fs::read(root.join("original.mp4")).unwrap();
            let result = render_clip(&ffmpeg, &root, &bytes, &ranges)
                .await
                .map_err(|error| ControlError::new("CLIP_FAILED", &error))?;
            if mode == "published" {
                save_marker(&root, "published.json", &result);
                std::future::pending::<()>().await;
            }
            Ok(result)
        })
    });
    let controller = Controller::new(catalog(), &dir, runner).unwrap();
    let response = controller
        .clone()
        .handle(request("call", None))
        .await
        .unwrap();
    save_marker(&dir, "accepted.json", &response);
    if stage == "committed" {
        let id = response["job"]["id"].as_str().unwrap();
        let value = tokio::time::timeout(Duration::from_secs(35), async {
            loop {
                let v = controller
                    .clone()
                    .handle(request("jobs.get", Some(id)))
                    .await
                    .unwrap();
                if v["status"] != "running" {
                    assert_eq!(v["status"], "succeeded", "{v:?}");
                    break v["result"].clone();
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        save_marker(&dir, "committed.json", &value);
    }
    std::future::pending::<()>().await;
}

fn encoder_running(pid: u32, ffmpeg: &Path) -> bool {
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    let tasks = std::process::Command::new(system.join("System32/tasklist.exe"))
        .args(["/FO", "CSV", "/NH", "/FI", &format!("PID eq {pid}")])
        .output()
        .unwrap();
    assert!(tasks.status.success());
    let needle = format!(
        "\"{}\",\"{pid}\",",
        ffmpeg.file_name().unwrap().to_string_lossy()
    );
    String::from_utf8_lossy(&tasks.stdout).lines().any(|line| {
        line.to_ascii_lowercase()
            .starts_with(&needle.to_ascii_lowercase())
    })
}

async fn wait_for<T>(
    child: &mut tokio::process::Child,
    mut observe: impl FnMut() -> Option<T>,
) -> T {
    let start = Instant::now();
    loop {
        assert!(
            child.try_wait().unwrap().is_none(),
            "test controller exited before the crash point"
        );
        if let Some(result) = observe() {
            return result;
        }
        assert!(
            start.elapsed() < Duration::from_secs(40),
            "test controller did not reach the requested crash point"
        );
        tokio::time::sleep(Duration::from_millis(35)).await;
    }
}

fn read_marker(dir: &Path, name: &str) -> Option<Value> {
    std::fs::read(dir.join(name))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

#[tokio::test]
#[ignore = "force-terminates only owned synthetic controller subprocesses and requires pinned FFmpeg"]
async fn real_encoder_crash_preserves_only_committed_results() {
    let ffmpeg =
        PathBuf::from(std::env::var_os("HOMEBASE_VIDEO_FFMPEG").expect("pinned FFmpeg required"));
    assert!(ffmpeg.is_absolute());
    if let Some(dir) = worker_dir() {
        worker(dir, ffmpeg, std::env::var(CRASH_STAGE).unwrap()).await;
        return;
    }
    let lab = tempfile::tempdir().unwrap();
    let fixture = lab.path().join("fixture.mp4");
    let generate = std::process::Command::new(&ffmpeg)
        .args([
            "-nostdin",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x240:r=30:d=12",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        generate.status.success(),
        "{}",
        String::from_utf8_lossy(&generate.stderr)
    );
    let original = std::fs::read(&fixture).unwrap();
    for stage in ["encoding", "published", "committed"] {
        let owned = tempfile::Builder::new()
            .prefix("clip-crash-")
            .tempdir_in(lab.path())
            .unwrap();
        let dir = owned.path();
        std::fs::write(dir.join("synthetic-owner"), b"homebase-clip-crash-fixture").unwrap();
        std::fs::write(dir.join("original.mp4"), &original).unwrap();
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST_NAME, "--ignored", "--nocapture"])
            .env(CRASH_DIR, dir)
            .env(CRASH_STAGE, stage)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let accepted = wait_for(&mut child, || read_marker(dir, "accepted.json")).await;
        let mut encoder = None;
        let expected = if stage == "encoding" {
            let pid = wait_for(&mut child, || {
                let pid: u32 = std::fs::read_to_string(dir.join("encoder.pid"))
                    .ok()?
                    .parse()
                    .ok()?;
                let clips = std::fs::read_dir(dir.join("clips")).ok()?;
                clips
                    .filter_map(Result::ok)
                    .any(|work| {
                        std::fs::metadata(work.path().join("part-0.mov"))
                            .is_ok_and(|info| info.len() > 512)
                    })
                    .then_some(pid)
            })
            .await;
            assert!(
                encoder_running(pid, &ffmpeg),
                "the real FFmpeg must still be encoding"
            );
            encoder = Some(pid);
            None
        } else {
            Some(wait_for(&mut child, || read_marker(dir, &format!("{stage}.json"))).await)
        };
        let terminal = child.kill().await;
        assert!(
            terminal.is_ok(),
            "owned test controller did not terminate: {terminal:?}"
        );
        if let Some(pid) = encoder {
            tokio::time::timeout(Duration::from_secs(6), async {
                while encoder_running(pid, &ffmpeg) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .expect("FFmpeg outlived its terminated owner");
        }
        let preserved = std::fs::read(dir.join("original.mp4")).unwrap();
        assert_eq!(preserved, original);
        let repeated: Runner =
            Arc::new(|_, _| panic!("crash recovery dispatched the old clip again"));
        let controller = Controller::new(catalog(), dir, repeated).unwrap();
        let id = accepted["job"]["id"].as_str().unwrap();
        let retry = controller
            .clone()
            .handle(request("call", None))
            .await
            .unwrap();
        assert_eq!(retry["job"]["id"], id);
        assert_eq!(retry["reused"], true);
        let result = controller
            .clone()
            .handle(request("jobs.get", Some(id)))
            .await
            .unwrap();
        if stage == "committed" {
            let expected = expected.unwrap();
            assert_eq!(result["status"], "succeeded");
            assert_eq!(result["result_storage"], "protected");
            assert_eq!(result["result"], expected);
            let mp4 = expected["artifact"]["path"].as_str().unwrap();
            let ledger = std::fs::read_to_string(dir.join("operations.jsonl")).unwrap();
            assert!(
                !ledger.contains("clip.mp4")
                    && !ledger.contains(expected["artifact"]["sha256"].as_str().unwrap())
            );
            let mut denied = request("jobs.get", Some(id));
            denied.grants = vec!["write".into()];
            assert_eq!(
                controller.clone().handle(denied).await.unwrap_err().code,
                "SCOPE_REQUIRED"
            );
            let mut changed = std::fs::read(mp4).unwrap();
            let last = changed.last_mut().unwrap();
            *last ^= 1;
            std::fs::write(mp4, changed).unwrap();
            let tampered = controller
                .handle(request("jobs.get", Some(id)))
                .await
                .unwrap();
            assert_eq!(tampered["error"]["code"], "RESULT_UNAVAILABLE");
            assert!(tampered["result"].is_null());
        } else {
            assert_eq!(result["status"], "interrupted_unknown");
            assert!(result["result"].is_null());
            assert_eq!(result["result_available"], false);
            if let Some(published) = expected {
                assert!(Path::new(published["artifact"]["path"].as_str().unwrap()).is_file());
            } else {
                assert!(!std::fs::read_dir(dir.join("clips"))
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(
                        |entry| entry.file_name().to_string_lossy().starts_with("clip-")
                            && !entry
                                .file_name()
                                .to_string_lossy()
                                .starts_with("clip-work-")
                    ));
            }
        }
        println!(
            "Crash phase {stage}: original job {id}, state {}",
            result["status"]
        );
    }
}
