use super::*;
use crate::control_protocol::{ControlError, Request};
use crate::control_state::{Controller, Runner};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn catalog() -> Value {
    serde_json::from_str(include_str!("../../control/commands.json")).unwrap()
}

fn request(method: &str, job_id: Option<&str>) -> Request {
    Request {
        protocol: 1,
        id: uuid::Uuid::new_v4().to_string(),
        token: "synthetic-cover-fixture".into(),
        method: method.into(),
        command: (method == "call").then(|| COMMAND.into()),
        args: if method == "call" {
            json!({"source":{
            "receipt_id":format!("wx-{}", "b".repeat(64)),
            "session_id":"bba9063a-4b64-4b4a-901d-77933f562801","sha256":"a".repeat(64)
        },"atMs":1250,"title":"中文封面 100% %{pts} '原片': 录制,剪辑。"})
        } else {
            json!({})
        },
        request_id: Some("synthetic-cover-request".into()),
        job_id: job_id.map(str::to_owned),
        catalog_hash: catalog()["catalog_hash"].as_str().map(str::to_owned),
        grants: vec!["write".into(), "private".into()],
    }
}

#[test]
fn title_and_timestamp_are_bounded() {
    assert_eq!(
        cover_title(119_999, &"你".repeat(32)).unwrap(),
        format!("{}\n{}", "你".repeat(16), "你".repeat(16))
    );
    for (ms, title) in [
        (120_000, "正常标题"),
        (0, "  "),
        (0, "第一行\n第二行"),
        (0, &"你".repeat(33)),
    ] {
        assert!(cover_title(ms, title)
            .unwrap_err()
            .starts_with("COVER_INPUT_INVALID"));
    }
}

#[test]
fn wechat_plain_language_can_bind_original_for_a_cover() {
    assert!(is_cover_request("帮我从这段视频生成三张封面"));
    assert_eq!(
        clip_video::recent_clip_intent("把刚发的视频做三张封面"),
        Some(clip_video::RecentClip::Latest)
    );
    assert_eq!(
        clip_video::recent_clip_intent("用这段原片第2秒做封面"),
        Some(clip_video::RecentClip::Unique)
    );
    assert_eq!(
        clip_video::recent_clip_intent("帮我解释刚发的视频提到的做封面技巧"),
        None
    );
    assert_eq!(
        clip_video::recent_clip_intent("先别给刚发的视频做封面"),
        None
    );
}

#[tokio::test]
#[ignore = "requires the pinned Windows FFmpeg at HOMEBASE_VIDEO_FFMPEG"]
async fn native_cover_set_is_idempotent_and_reopens_only_intact_files() {
    let ffmpeg = PathBuf::from(std::env::var_os("HOMEBASE_VIDEO_FFMPEG").unwrap());
    assert!(ffmpeg.is_absolute());
    let lab = tempfile::tempdir().unwrap();
    let source = lab.path().join("original.mp4");
    assert!(std::process::Command::new(&ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=320x180:r=25:d=1",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=320x180:r=25:d=1",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0",
            "-c:v",
            "libx264",
            "-threads",
            "1",
        ])
        .arg(&source)
        .status()
        .unwrap()
        .success());
    let bytes = std::fs::read(&source).unwrap();
    let directory = lab.path().join("control");
    std::fs::create_dir(&directory).unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let runner: Runner = {
        let (ffmpeg, root, bytes, runs) = (
            ffmpeg.clone(),
            lab.path().to_owned(),
            bytes.clone(),
            runs.clone(),
        );
        Arc::new(move |command, args| {
            let (ffmpeg, root, bytes, runs) =
                (ffmpeg.clone(), root.clone(), bytes.clone(), runs.clone());
            Box::pin(async move {
                assert_eq!(command, COMMAND);
                runs.fetch_add(1, Ordering::SeqCst);
                let ms = args["atMs"].as_u64().unwrap();
                let title = cover_title(ms, args["title"].as_str().unwrap()).unwrap();
                render(&ffmpeg, &root, &bytes, ms, &title)
                    .await
                    .map_err(|error| ControlError::new("COVER_RENDER_FAILED", &error))
            })
        })
    };
    let controller = Controller::new(catalog(), &directory, runner.clone()).unwrap();
    let accepted = controller
        .clone()
        .handle(request("call", None))
        .await
        .unwrap();
    let id = accepted["job"]["id"].as_str().unwrap();
    let original = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let result = controller
                .clone()
                .handle(request("jobs.get", Some(id)))
                .await
                .unwrap();
            if !matches!(result["status"].as_str(), Some("queued" | "running")) {
                assert_eq!(result["status"], "succeeded", "{result:?}");
                break result["result"].clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(valid_receipt(&original, true));
    let artifacts = original["artifacts"].as_array().unwrap();
    for (artifact, (_, w, h)) in artifacts.iter().zip(COVERS) {
        let decoded = std::process::Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-i",
                artifact["path"].as_str().unwrap(),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-",
            ])
            .output()
            .unwrap();
        assert!(decoded.status.success());
        assert_eq!(decoded.stdout.len(), (w * h * 3) as usize);
        let pixel = ((h / 2 * w + w / 2) * 3) as usize;
        assert!(decoded.stdout[pixel] < 16 && decoded.stdout[pixel + 2] > 230);
        let start = (h * 73 / 100 * w * 3) as usize;
        assert!(decoded.stdout[start..]
            .chunks_exact(3)
            .any(|px| px.iter().all(|value| *value > 238)));
    }
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert!(render(&ffmpeg, lab.path(), &bytes, 5_000, "超出原片")
        .await
        .unwrap_err()
        .starts_with("COVER_RENDER_FAILED"));
    assert_eq!(
        std::fs::read_dir(lab.path().join("video-covers"))
            .unwrap()
            .count(),
        1
    );

    drop(controller);
    let reopened = Controller::new(catalog(), &directory, runner).unwrap();
    let repeat = reopened
        .clone()
        .handle(request("call", None))
        .await
        .unwrap();
    assert_eq!(repeat["job"]["id"], id);
    let fetched = reopened
        .clone()
        .handle(request("jobs.get", Some(id)))
        .await
        .unwrap();
    assert_eq!(fetched["result"], original);
    let mut ungranted = request("jobs.get", Some(id));
    ungranted.grants.clear();
    assert!(reopened.clone().handle(ungranted).await.is_err());
    std::fs::write(artifacts[0]["path"].as_str().unwrap(), b"altered").unwrap();
    let fetched = reopened
        .handle(request("jobs.get", Some(id)))
        .await
        .unwrap();
    assert_eq!(fetched["error"]["code"], "RESULT_UNAVAILABLE");
    assert!(fetched["result"].is_null());
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}
