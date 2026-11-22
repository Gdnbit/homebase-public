use super::*;

#[test]
#[ignore = "explicit local host, Pi worker and pinned FFmpeg for isolated media required"]
fn pi_wechat_uses_installed_skill_and_recovers_the_original_video_delivery() {
    // The full debug fixture holds the local host and media futures across restarts.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(exercise_video_delivery("pi"));
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn exercise_video_delivery(harness: &str) {
    use crate::assistant::{Engine as Core, Model, Save as AgentSave, State};
    use crate::assistant_local_host::LocalHost;
    use crate::capability_discovery::{self as discovery, DiscoveryRequest, DiscoveryStore};
    use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyInit};

    const SKILL: &str = "homebase-fixture-cut-v1";
    const EDIT: &str = "把倒数第二段视频照旧剪一下。";
    const ORIGINAL_REPLY: &str = "9007199254741113";
    const REVISION: &str = "这次只留原片前半秒";
    const RENDERED_REVISION: &str = "只留上一版成片的前半秒";
    const IMPLICIT_REVISION: &str = "只留这一版成片的前0.3秒";
    const BACK_TO_ORIGINAL: &str = "这次只留原片前0.7秒";
    const COVER: &str = "帮我从这段原片第1.25秒做三张封面，标题叫蓝色画面";
    const TIMED_EDIT: &str = "帮我拼接这段原片的2秒到2.6秒和0.1秒到0.6秒，按这个顺序配字幕";
    const PACKAGE: &str = "帮我从这段原片做发布包，沿用刚才的片段顺序和封面时间";
    let ffmpeg = PathBuf::from(std::env::var_os("HOMEBASE_VIDEO_FFMPEG").unwrap());
    assert!(ffmpeg.is_absolute());
    let runtime = PathBuf::from(std::env::var_os("HOMEBASE_LOCAL_RUNTIME").unwrap());
    assert!(runtime.is_absolute());
    let lab = tempfile::tempdir().unwrap();
    let workspace = lab.path().join("workspace");
    let control_root = lab.path().join("control");
    std::fs::create_dir(&control_root).unwrap();
    let skills_root = lab.path().join("installed-skills");
    let skill = skills_root.join(SKILL);
    let description = "---\nname: 合成剪片流程\ndescription: 用于测试蓝色画面剪辑\n---\npreset: references/preset.json\n";
    std::fs::create_dir_all(skill.join("references")).unwrap();
    std::fs::write(skill.join("SKILL.md"), description).unwrap();
    std::fs::write(
        skill.join("references/preset.json"),
        r#"{"segments":[{"startMs":2600,"durationMs":750},{"startMs":100,"durationMs":800}]}"#,
    )
    .unwrap();
    let unavailable = lab.path().join("not-a-skill-directory");
    std::fs::write(&unavailable, b"synthetic sentinel").unwrap();
    let fixture_context = json!({"skill_roots":[
        {"id":"codex","path":skills_root},
        {"id":"claude","path":unavailable}
    ]});
    let host = LocalHost::start(&runtime, &lab.path().join("host-profile"))
        .await
        .unwrap();
    let index_host = host.clone();
    let index_context = fixture_context.clone();
    let lookups = Arc::new(AtomicUsize::new(0));
    let skill_reads = lookups.clone();
    let fixture = lab.path().join("fixture.mp4");
    let run = std::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=160x120:d=1:r=30",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=160x120:d=3:r=30",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2.15",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0[color]",
            "-map",
            "[color]",
            "-map",
            "2:a",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
        ])
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let original = std::fs::read(&fixture).unwrap();
    let raw_sha = format!("{:x}", Sha256::digest(&original));
    let key = [9u8; 16];
    let cipher = ecb::Encryptor::<aes::Aes128>::new_from_slice(&key)
        .unwrap()
        .encrypt_padded_vec_mut::<Pkcs7>(&original);
    let (base, sent, server) = fake_api(
        200,
        json!({"__pdf_cipher":STANDARD.encode(&cipher),"__media":true,"__drop_first_file_ack":true,"__send_text_prefix_once":true,
            "__send_text_prefix_ids":{"已按原来的范围剪好":ORIGINAL_REPLY}}),
    )
    .await;
    let generation = Arc::new(AtomicUsize::new(0));
    let submitted = generation.clone();
    let handle = Arc::new(std::sync::Mutex::new(std::sync::Weak::<Bridge>::new()));
    let runner_handle = handle.clone();
    let selected = Arc::new(std::sync::OnceLock::<String>::new());
    let expected = selected.clone();
    let original_output = Arc::new(std::sync::OnceLock::<clip_video::ClipArtifactSource>::new());
    let expected_output = original_output.clone();
    let chain_output = Arc::new(std::sync::OnceLock::<clip_video::ClipArtifactSource>::new());
    let expected_chain = chain_output.clone();
    let clip_ffmpeg = ffmpeg.clone();
    let runner: Runner = Arc::new(move |command, args| {
        let (handle, ffmpeg, submitted, expected) = (
            runner_handle.clone(),
            clip_ffmpeg.clone(),
            submitted.clone(),
            expected.clone(),
        );
        let (host, index_context) = (index_host.clone(), index_context.clone());
        let expected_output = expected_output.clone();
        let expected_chain = expected_chain.clone();
        Box::pin(async move {
            if command == "capability_index_query" {
                let mut request: DiscoveryRequest =
                    serde_json::from_value(args["request"].clone()).unwrap();
                assert_eq!(request.query, "蓝色画面剪辑");
                let mut entries = vec![];
                let mut issues = vec![];
                discovery::append_installed_skills(
                    host.execute(
                        "fixture-skill-index",
                        "skill_index",
                        index_context,
                        json!({}),
                    )
                    .await,
                    &mut entries,
                    &mut issues,
                );
                assert_eq!(issues.len(), 1, "{issues:?}");
                let clock = std::time::Instant::now();
                let mut store = DiscoveryStore::new();
                request.catalog_id = Some(
                    store
                        .insert(discovery::normalize(entries, issues), clock)
                        .unwrap(),
                );
                return Ok(serde_json::to_value(store.page(&request, clock).unwrap()).unwrap());
            }
            let bridge = handle.lock().unwrap().upgrade().unwrap();
            if command == clip_video::TRIM_ARTIFACT_COMMAND {
                let (expected, duration) = match submitted.fetch_add(1, Ordering::SeqCst) {
                    2 => (expected_output.get().unwrap(), 500),
                    3 => (expected_chain.get().unwrap(), 300),
                    _ => panic!("unexpected repeated trim"),
                };
                let source: clip_video::ClipArtifactSource = serde_json::from_value(args["source"].clone()).unwrap();
                assert_eq!(args["source"], json!(expected));
                assert_eq!(args["startMs"], 0);
                assert_eq!(args["durationMs"], duration);
                return bridge.trim_clip_artifact(&ffmpeg, source, 0, duration).await
                    .map_err(|error| ControlError::new("CLIP_FIXTURE_ERROR", &error));
            }
            let source: clip_video::ClipSource =
                serde_json::from_value(args["source"].clone()).unwrap();
            assert_eq!(&source.receipt_id, expected.get().unwrap());
            let result = match submitted.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    assert_eq!(command, "capability_assemble_video");
                    assert_eq!(args["segments"], json!([{"startMs":2600,"durationMs":750},{"startMs":100,"durationMs":800}]));
                    let segments: Vec<clip_video::ClipRange> = serde_json::from_value(args["segments"].clone()).unwrap();
                    bridge.assemble_original(&ffmpeg, source, &segments).await
                }
                1 => {
                    assert_eq!(command, "capability_clip_video");
                    assert_eq!(args["startMs"], 0);
                    assert_eq!(args["durationMs"], 500);
                    bridge.clip_original(&ffmpeg, source, 0, 500).await
                }
                4 => {
                    assert_eq!(command, "capability_clip_video");
                    assert_eq!(args["startMs"], 0);
                    assert_eq!(args["durationMs"], 700);
                    bridge.clip_original(&ffmpeg, source, 0, 700).await
                }
                5 => {
                    assert_eq!(command, video_cover::COMMAND);
                    assert_eq!(args["atMs"], 1250);
                    assert_eq!(args["title"], "蓝色画面");
                    bridge.cover_original(&ffmpeg, source, 1250, "蓝色画面").await
                }
                6 => {
                    assert_eq!(command, "capability_assemble_video");
                    assert_eq!(args["includeSubtitles"], true);
                    let ranges: Vec<clip_video::ClipRange> = serde_json::from_value(args["segments"].clone()).unwrap();
                    bridge.assemble_subtitled_original(&ffmpeg, source, &ranges).await
                }
                7 => {
                    assert_eq!(command, publish_package::COMMAND);
                    let ranges: Vec<clip_video::ClipRange> = serde_json::from_value(args["segments"].clone()).unwrap();
                    bridge.publish_package_original(&ffmpeg, source, &ranges, args["coverAtMs"].as_u64().unwrap(),
                        args["title"].as_str().unwrap(), args["description"].as_str().unwrap()).await
                }
                _ => panic!("duplicate replay started another edit"),
            };
            result.map_err(|error| ControlError::new("CLIP_FIXTURE_ERROR", &error))
        })
    });
    let contract = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let ctl = Controller::new(contract, &control_root, runner.clone()).unwrap();
    let connect_control = |ctl: Arc<Controller>| -> Control {
        let (host, context, reads) = (host.clone(), fixture_context.clone(), skill_reads.clone());
        Arc::new(move |request| {
            let (ctl, host, context, reads) =
                (ctl.clone(), host.clone(), context.clone(), reads.clone());
            Box::pin(async move {
                if request.method == "assistant.local_read" {
                    let input = &request.args["request"];
                    assert!(request.grants.iter().any(|grant| grant == "private"));
                    assert_eq!(request.args["operation"], "skill");
                    assert!(matches!(
                        input["action"].as_str(),
                        Some("read" | "resource")
                    ));
                    reads.fetch_add(1, Ordering::SeqCst);
                    return host
                        .execute(
                            request.args["operationId"].as_str().unwrap(),
                            "skill",
                            context,
                            input.clone(),
                        )
                        .await
                        .map_err(|error| ControlError::new("LOCAL_SKILL_FIXTURE", &error));
                }
                ctl.handle(request).await
            })
        })
    };
    let control = connect_control(ctl.clone());
    let history = Arc::new(AtomicUsize::new(0));
    let rounds = history.clone();
    let model: Model = Arc::new(move |request| {
        let rounds = rounds.clone();
        Box::pin(async move {
            if request.messages.last().is_some_and(|message| message["content"] == "nihaoya") {
                assert!(request.definitions.as_ref().is_some_and(Vec::is_empty));
                return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"你好呀，随时可以把视频发给我。"}}]}));
            }
            if request.messages[0]["content"].as_str().is_some_and(|text| text.starts_with("HomeBase Pi 自动摘要")) {
                assert!(request.definitions.as_ref().is_some_and(Vec::is_empty));
                return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"用户已对同一合成原片创建多个剪辑版本，后续仍要区分原片与各版成片。"}}]}));
            }
            let round = rounds.fetch_add(1, Ordering::SeqCst);
            let last_result = || -> Value {
                let msg = request
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m["role"] == "tool")
                    .unwrap();
                serde_json::from_str(msg["content"].as_str().unwrap()).unwrap()
            };
            let (name, args) = match round {
                0 => {
                    let context = request
                        .messages
                        .iter()
                        .filter_map(|m| m["content"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    assert!(context.contains("蓝色画面剪辑"), "recall uses the bound clip intent");
                    assert!(!context.contains(SKILL), "selection must not rely on a known installation name");
                    assert!(
                        context.contains("本轮已绑定"),
                        "request has a verified original"
                    );
                    assert!(!context.contains("original.mp4"));
                    assert!(!context.contains("synthetic-image-style"));
                    assert!(!context.contains("references/preset.json"));
                    assert_eq!(
                        request.messages.last().unwrap()["content"],
                        EDIT
                    );
                    (
                        "homebase_call",
                        json!({"command":"capability_index_query","args":{"request":{"kind":"skill","query":"蓝色画面剪辑","limit":3}}}),
                    )
                }
                1 => {
                    let page = last_result();
                    let skills = page["result"]["entries"].as_array().unwrap();
                    assert_eq!(page["result"]["source_complete"], false);
                    assert!(!page.to_string().contains("not-a-skill-directory"));
                    assert_eq!(skills.len(), 1);
                    assert!(skills[0]["name"].as_str().unwrap().contains(SKILL));
                    assert!(skills[0]["target"]
                        .as_str()
                        .unwrap()
                        .starts_with("installed:codex:"));
                    (
                        "homebase_installed_skill",
                        json!({"action":"read","id":skills[0]["target"]}),
                    )
                }
                2 => {
                    let skill = last_result();
                    let path = skill["content"]
                        .as_str()
                        .unwrap()
                        .lines()
                        .find_map(|line| line.strip_prefix("preset: "))
                        .unwrap();
                    (
                        "homebase_installed_skill",
                        json!({"action":"resource","id":skill["id"],"path":path}),
                    )
                }
                3 => {
                    assert_eq!(last_result()["path"], "references/preset.json");
                    (
                        "homebase_describe",
                        json!({"command":"capability_assemble_video"}),
                    )
                }
                4 => {
                    let description = last_result();
                    assert_eq!(description["input_schema"]["required"], json!(["segments"]));
                    assert!(description["input_schema"]["properties"]
                        .get("source")
                        .is_none());
                    let preset = request
                        .messages
                        .iter()
                        .filter(|m| m["role"] == "tool")
                        .filter_map(|m| serde_json::from_str::<Value>(m["content"].as_str()?).ok())
                        .find(|result| result["path"] == "references/preset.json")
                        .unwrap();
                    let range: Value =
                        serde_json::from_str(preset["content"].as_str().unwrap()).unwrap();
                    (
                        "homebase_call",
                        json!({"command":"capability_assemble_video","args":range}),
                    )
                }
                5 => {
                    let result = last_result();
                    assert_eq!(result["result"]["duration_ms"], 1566);
                    assert_eq!(result["result"]["segments"][0]["startMs"], 2600);
                    assert_eq!(result["result"]["segments"][1]["startMs"], 100);
                    assert_eq!(result["result"]["artifact"]["mime_type"], "video/mp4");
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                6 => {
                    return crate::assistant_protocol::parse(
                        "chat",
                        &json!({"choices":[{"message":{"content":"已按原来的范围剪好，正在返回片段。"}}]}),
                    )
                }
                7 => {
                    assert_eq!(request.messages.last().unwrap()["content"], REVISION);
                    let context = request.messages.iter().filter_map(|m| m["content"].as_str()).collect::<Vec<_>>().join("\n");
                    assert!(context.contains("本次明确要求改剪原片"));
                    assert!(context.contains("\"previous_clip\"") && context.contains("\"duration_ms\":1566"), "{context}");
                    ("homebase_describe", json!({"command":"capability_clip_video"}))
                }
                8 => {
                    let description = last_result();
                    assert!(description["input_schema"]["properties"].get("source").is_none());
                    ("homebase_call", json!({"command":"capability_clip_video","args":{"startMs":0,"durationMs":500}}))
                }
                9 => {
                    let result = last_result();
                    assert_eq!(result["result"]["duration_ms"], 500);
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                10 => {
                    return crate::assistant_protocol::parse(
                        "chat",
                        &json!({"choices":[{"message":{"content":"已取原片前半秒并保存为新版。"}}]}),
                    )
                }
                11 => {
                    assert_eq!(request.messages.last().unwrap()["content"], RENDERED_REVISION);
                    let context = request.messages.iter().filter_map(|m| m["content"].as_str()).collect::<Vec<_>>().join("\n");
                    assert!(context.contains("按已核对的上一版成片时间剪辑"));
                    assert!(!context.contains("本轮已绑定本人在当前微信会话发送的原 MP4"));
                    ("homebase_describe", json!({"command":clip_video::TRIM_ARTIFACT_COMMAND}))
                }
                12 => {
                    let description = last_result();
                    assert!(description["input_schema"]["properties"].get("source").is_none());
                    ("homebase_call", json!({"command":clip_video::TRIM_ARTIFACT_COMMAND,"args":{"startMs":0,"durationMs":500}}))
                }
                13 => {
                    let result = last_result();
                    assert_eq!(result["result"]["duration_ms"], 500);
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                14 => {
                    return crate::assistant_protocol::parse(
                        "chat",
                        &json!({"choices":[{"message":{"content":"已取上一版成片的前半秒，已生成新版。"}}]}),
                    )
                }
                15 => {
                    assert_eq!(request.messages.last().unwrap()["content"], IMPLICIT_REVISION);
                    ("homebase_describe", json!({"command":clip_video::TRIM_ARTIFACT_COMMAND}))
                }
                16 => {
                    assert!(last_result()["input_schema"]["properties"].get("source").is_none());
                    ("homebase_call", json!({"command":clip_video::TRIM_ARTIFACT_COMMAND,"args":{"startMs":0,"durationMs":300}}))
                }
                17 => {
                    let result = last_result();
                    assert_eq!(result["result"]["duration_ms"], 300);
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                18 => {
                    return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"已从这版成片再剪出前 0.3 秒。"}}]}));
                }
                19 => {
                    assert_eq!(request.messages.last().unwrap()["content"], BACK_TO_ORIGINAL);
                    let context = request.messages.iter().filter_map(|m| m["content"].as_str()).collect::<Vec<_>>().join("\n");
                    assert!(context.contains("本次明确要求改剪原片"));
                    ("homebase_call", json!({"command":"capability_clip_video","args":{"startMs":0,"durationMs":700}}))
                }
                20 => {
                    let result = last_result();
                    assert_eq!(result["result"]["duration_ms"], 700);
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                21 => {
                    return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"已从最初原片剪出前 0.7 秒。"}}]}));
                }
                22 => {
                    assert_eq!(request.messages.last().unwrap()["content"], "列一个室内活动清单");
                    return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"可以读书、拼图或拉伸。"}}]}));
                }
                23 => {
                    assert_eq!(request.messages.last().unwrap()["content"], COVER);
                    assert!(request.messages.iter().filter_map(|m| m["content"].as_str())
                        .any(|message| message.contains(video_cover::COMMAND)));
                    ("homebase_describe", json!({"command":video_cover::COMMAND}))
                }
                24 => {
                    let description = last_result();
                    assert!(description["input_schema"]["properties"].get("source").is_none());
                    ("homebase_call", json!({"command":video_cover::COMMAND,"args":{"atMs":1250,"title":"蓝色画面"}}))
                }
                25 => {
                    let result = last_result();
                    assert_eq!(result["files"].as_array().unwrap().len(), 3);
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                26 => return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"从原片 1.25 秒生成了三张封面。"}}]})),
                27 => {
                    assert_eq!(request.messages.last().unwrap()["content"], TIMED_EDIT);
                    ("homebase_call", json!({"command":"capability_assemble_video","args":{"includeSubtitles":true,"segments":[{"startMs":2000,"durationMs":600},{"startMs":100,"durationMs":500}]}}))
                }
                28 => {
                    let result = last_result();
                    assert_eq!(result["files"].as_array().unwrap().len(), 2);
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                29 => return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"成片和对应的独立字幕已生成。"}}]})),
                30 => {
                    assert_eq!(request.messages.last().unwrap()["content"], PACKAGE);
                    assert!(request.messages.iter().filter_map(|m| m["content"].as_str())
                        .any(|message| message.contains(publish_package::COMMAND)));
                    ("homebase_describe", json!({"command":publish_package::COMMAND}))
                }
                31 => {
                    assert!(last_result()["input_schema"]["properties"].get("source").is_none());
                    ("homebase_call", json!({"command":publish_package::COMMAND,"args":{
                        "segments":[{"startMs":2000,"durationMs":600},{"startMs":100,"durationMs":500}],
                        "coverAtMs":1250,"title":"合成蓝色标题","description":"隔离发布简介"}}))
                }
                32 => {
                    let result = last_result();
                    assert_eq!(result["files"].as_array().unwrap().len(), 1);
                    ("homebase_deliver", json!({"job_id":result["job_id"]}))
                }
                33 => return crate::assistant_protocol::parse("chat", &json!({"choices":[{"message":{"content":"发布包已生成。"}}]})),
                _ => panic!("replay must not create a new model request"),
            };
            crate::assistant_protocol::parse(
                "chat",
                &json!({"choices":[{"message":{"tool_calls":[{"id":format!("fixture-clip-{round}"),"function":{"name":name,"arguments":args.to_string()}}]}}]}),
            )
        })
    });
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.runtime = harness.into();
    state.settings.route_id = "openrouter::synthetic/model:free".into();
    let saved_core = Arc::new(Mutex::new(state.clone()));
    let store = saved_core.clone();
    let save: AgentSave = Arc::new(move |state| {
        let store = store.clone();
        Box::pin(async move {
            *store.lock().await = state;
            Ok(())
        })
    });
    let (template, saved_wx) = test_bridge(account(&base), &workspace, ctl);
    let core = Core::new(
        state,
        save.clone(),
        model.clone(),
        control.clone(),
        workspace.clone(),
    )
    .await
    .unwrap();
    core.memory_save(crate::assistant_personal::MemoryInput {
        revision: 0,
        id: None,
        title: "默认剪片".into(),
        content: "视频剪辑沿用蓝色画面剪辑的已安装工作流。".into(),
        expires_at: None,
    })
    .await
    .unwrap();
    core.memory_save(crate::assistant_personal::MemoryInput {
        revision: 1,
        id: None,
        title: "图片生成".into(),
        content: "synthetic-image-style".into(),
        expires_at: None,
    })
    .await
    .unwrap();
    let connect = |stored: Stored, core: Arc<Core>, control: Control| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(
            stored,
            api,
            template.save.clone(),
            control,
            workspace.clone(),
            Some(Arc::new(move |request| {
                let core = core.clone();
                Box::pin(async move { core.channel(request).await })
            })),
        )
    };
    let bridge = connect(account(&base), core.clone(), control);
    *handle.lock().unwrap() = Arc::downgrade(&bridge);
    let media_root = workspace.clone();
    bridge.configure_video(Arc::new(move |request| {
        let root = media_root.clone();
        Box::pin(async move {
            let video::VideoRequest::Start { id, bytes } = request else {
                panic!("no duplicate download or transcribe")
            };
            let path = root.join("video-transcripts").join(id).join("original.mp4");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes.as_slice()).unwrap();
            video::VideoResult::Failed("合成转写未运行，原视频已保存。".into())
        })
    }));
    bridge.start().await.unwrap();
    let greeting = msg("synthetic-clip-greeting", "nihaoya");
    bridge.ingest(&greeting).await.unwrap();
    bridge.ingest(&greeting).await.unwrap();
    let model_question = msg("synthetic-clip-model", "你是什么模型？");
    bridge.ingest(&model_question).await.unwrap();
    bridge.ingest(&model_question).await.unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let ready = {
                let rows = sent.lock().await;
                let texts: Vec<_> = rows.iter().filter_map(|r| r["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
                assert!(texts.len() <= 2, "duplicate or premature progress: {texts:?}");
                texts.contains(&"你好呀，随时可以把视频发给我。")
                    && texts.contains(&"我现在用的是 synthetic/model:free，通过 openrouter 调用。")
            };
            if ready { break; }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    let mut incoming = msg("synthetic-clip-original", "");
    incoming["item_list"] = json!([{"type":5,"msg_id":"9007199254741013","video_item":{"video_size":cipher.len(),
        "video_md5":format!("{:x}",md5::Md5::digest(&original)),"media":{"aes_key":STANDARD.encode(key),"encrypt_query_param":"synthetic-clip-input"}}}]);
    let mut earlier = incoming.clone();
    earlier["message_id"] = json!("synthetic-clip-other");
    earlier["item_list"][0]["msg_id"] = json!("9007199254741012");
    bridge.ingest(&earlier).await.unwrap();
    let mut rejected = incoming.clone();
    rejected["message_id"] = json!("synthetic-clip-oversize");
    rejected["item_list"][0]["msg_id"] = json!("9007199254741010");
    rejected["item_list"][0]["video_item"]["video_size"] = json!(50_000_000);
    bridge.ingest(&rejected).await.unwrap();
    bridge
        .ingest(&msg(
            "synthetic-clip-rejected-reference",
            "把我刚发的视频剪一下",
        ))
        .await
        .unwrap();
    assert!(sent.lock().await.iter().any(|r| {
        let text = r["body"]["msg"]["item_list"].to_string();
        text.contains("这段原片没有接收成功") && !text.contains("查看指令")
    }));
    let replies_before_corruption = sent.lock().await.len();
    let mut failed = incoming.clone();
    failed["message_id"] = json!("synthetic-clip-bad-source");
    failed["item_list"][0]["msg_id"] = json!("9007199254741011");
    failed["item_list"][0]["video_item"]["video_md5"] = json!("ffffffffffffffffffffffffffffffff");
    bridge.ingest(&failed).await.unwrap();
    bridge
        .ingest(&msg(
            "synthetic-clip-failed-reference",
            "把我刚发的视频剪一下",
        ))
        .await
        .unwrap();
    assert!(sent.lock().await[replies_before_corruption..]
        .iter()
        .any(|r| r["body"]["msg"]["item_list"]
            .to_string()
            .contains("这段原片没有接收成功")));
    bridge.ingest(&incoming).await.unwrap();
    let source = {
        let state = saved_wx.lock().await;
        let row = state.records.last().unwrap();
        assert_eq!(row.args["original_sha256"], raw_sha);
        clip_video::ClipSource {
            receipt_id: row.id.clone(),
            session_id: row.args["scope"].as_str().unwrap().into(),
            sha256: raw_sha,
        }
    };
    selected.set(source.receipt_id.clone()).unwrap();
    let mut explicitly_earlier = msg("synthetic-clip-quoted", "把我刚发的视频剪一下");
    explicitly_earlier["item_list"][0]["ref_msg"] =
        json!({"message_item":{"type":5,"msg_id":"9007199254741012"}});
    let original_earlier = identity::legacy_receipt_id(&account(&base), "synthetic-clip-other");
    assert!(matches!(
        bridge.video_question(&explicitly_earlier, &account(&base), "把我刚发的视频剪一下").await,
        Some(Ok(Action::Call(command, args))) if command == "assistant" && args["video_clip_receipt"] == original_earlier
    ));
    bridge
        .ingest(&msg("synthetic-clip-ambiguous", "照旧，剪一下"))
        .await
        .unwrap();
    assert!(sent
        .lock()
        .await
        .iter()
        .any(|r| r["body"]["msg"]["item_list"]
            .to_string()
            .contains("收到了不止一段视频")));
    assert_eq!(history.load(Ordering::SeqCst), 0);
    let untrusted = clip_video::ClipSource {
        session_id: uuid::Uuid::new_v4().to_string(),
        ..source.clone()
    };
    assert!(bridge
        .clip_original(&ffmpeg, source.clone(), 120_000, 1000)
        .await
        .unwrap_err()
        .contains("前 120 秒"));
    assert!(bridge
        .clip_original(&ffmpeg, untrusted, 1500, 1000)
        .await
        .unwrap_err()
        .contains("绑定"));
    let source_path = workspace
        .join("video-transcripts")
        .join(&source.receipt_id)
        .join("original.mp4");
    std::fs::write(&source_path, b"synthetic modified bytes").unwrap();
    assert!(bridge
        .clip_original(&ffmpeg, source.clone(), 1500, 1000)
        .await
        .unwrap_err()
        .contains("摘要"));
    assert!(!workspace.join("clips").exists());
    std::fs::write(&source_path, &original).unwrap();
    let unexpected = workspace.join("clips");
    std::fs::write(&unexpected, b"existing isolated sentinel").unwrap();
    assert!(bridge
        .clip_original(&ffmpeg, source.clone(), 1500, 1000)
        .await
        .unwrap_err()
        .contains("普通目录"));
    assert_eq!(
        std::fs::read(&unexpected).unwrap(),
        b"existing isolated sentinel"
    );
    std::fs::remove_file(&unexpected).unwrap();
    let parts = [
        clip_video::ClipRange {
            start_ms: 100,
            duration_ms: 800,
        },
        clip_video::ClipRange {
            start_ms: 3700,
            duration_ms: 900,
        },
    ];
    assert!(bridge
        .assemble_original(&ffmpeg, source.clone(), &parts[..1])
        .await
        .unwrap_err()
        .contains("2 至 6"));
    assert!(bridge
        .assemble_original(&ffmpeg, source.clone(), &parts)
        .await
        .unwrap_err()
        .contains("视频长度不足"));
    assert_eq!(
        std::fs::read_dir(workspace.join("clips")).unwrap().count(),
        0
    );
    let expired = (chrono::Utc::now() - chrono::TimeDelta::minutes(16)).to_rfc3339();
    let older_receipts = saved_wx.lock().await.records.iter()
        .filter(|row| (row.is_video() || row.args["video_rejected"] == true) && row.id != source.receipt_id)
        .map(|row| row.id.clone())
        .collect::<Vec<_>>();
    assert!(older_receipts.contains(&original_earlier));
    for id in older_receipts {
        bridge.change(&id, |record| record.created_at = expired.clone()).await.unwrap();
    }
    let mut latest = incoming.clone();
    latest["message_id"] = json!("synthetic-clip-later");
    latest["item_list"][0]["msg_id"] = json!("9007199254741014");
    bridge.ingest(&latest).await.unwrap();
    {
        let ledger = saved_wx.lock().await;
        let next = ledger.records.last().unwrap();
        assert_ne!(next.id, source.receipt_id);
        assert_eq!(next.args["scope"], source.session_id);
        assert_eq!(next.args["original_sha256"], source.sha256);
    }
    let started_at = tokio::time::Instant::now();
    let natural = msg("synthetic-clip-request", EDIT);
    bridge.ingest(&natural).await.unwrap();
    bridge.ingest(&natural).await.unwrap();
    let clip_receipt = identity::legacy_receipt_id(&account(&base), "synthetic-clip-request");
    let turn = bridge
        .status()
        .await
        .tasks
        .iter()
        .find(|row| row.id == clip_receipt && row.command == "assistant")
        .unwrap()
        .job_id
        .clone()
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let value = core.turn(&turn).await.unwrap();
            assert_ne!(value.state, "failed", "{}", value.text);
            if value.state == "completed" {
                break value;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let completed_at = tokio::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(15), async {
        while !saved_wx
            .lock()
            .await
            .records
            .iter()
            .any(|row| row.job_id.as_deref() == Some(&turn) && row.delivery == "attachment_unknown")
        {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    let into_initial_delivery_ms = completed_at.elapsed().as_millis();
    let [artifact] = result.artifacts.as_slice() else {
        panic!("expected one result video")
    };
    let edited = std::fs::read(&artifact.path).unwrap();
    assert_eq!(artifact.sha256, format!("{:x}", Sha256::digest(&edited)));
    original_output.set(clip_video::ClipArtifactSource {
        session_id: result.session_id.clone(), turn_id: turn.clone(),
        job_id: artifact.job_id.clone(), sha256: artifact.sha256.clone(),
    }).unwrap();
    let pixels = std::process::Command::new(&ffmpeg)
        .args(["-loglevel", "error", "-i"])
        .arg(&artifact.path)
        .args([
            "-map",
            "0:v:0",
            "-vf",
            "scale=1:1",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(pixels.status.success());
    let colors = pixels.stdout.chunks_exact(3).collect::<Vec<_>>();
    assert_eq!(colors.len(), 47);
    assert!(colors[..23].iter().all(|p| p[2] > 200 && p[0] < 40));
    assert!(
        colors[23..].iter().all(|p| p[0] > 200 && p[2] < 40),
        "red follows blue as specified"
    );
    let decoded = std::process::Command::new(&ffmpeg)
        .args(["-loglevel", "error", "-xerror", "-i"])
        .arg(&artifact.path)
        .args(["-map", "0:v:0", "-map", "0:a:0", "-f", "null", "-"])
        .output()
        .unwrap();
    assert!(
        decoded.status.success(),
        "the entire output decodes, including audio"
    );
    let pcm = std::process::Command::new(&ffmpeg)
        .args(["-loglevel", "error", "-i"])
        .arg(&artifact.path)
        .args([
            "-map",
            "0:a:0",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(pcm.status.success());
    assert!(
        pcm.stdout[..96000].iter().all(|byte| *byte == 0),
        "first segment is after the original audio ended"
    );
    assert!(
        pcm.stdout[163200..240000].iter().any(|byte| *byte != 0),
        "second segment preserves its sound"
    );
    assert_eq!(std::fs::read(&source_path).unwrap(), original);
    let mut latest = incoming;
    latest["message_id"] = json!("synthetic-clip-later");
    latest["item_list"][0]["msg_id"] = json!("9007199254741014");
    bridge.ingest(&latest).await.unwrap();
    let state = saved_wx.lock().await.clone();
    #[cfg(windows)]
    let state = {
        let bytes = serde_json::to_vec(&state).unwrap();
        let encrypted = crate::wechat_crypto::crypt(&bytes, true).unwrap();
        let plain = crate::wechat_crypto::crypt(&encrypted, false).unwrap();
        serde_json::from_slice::<Stored>(&plain).unwrap()
    };
    bridge.stop().await.unwrap();
    let contract = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
    let controller = Controller::new(contract, &control_root, runner.clone()).unwrap();
    let control = connect_control(controller);
    let old = saved_core.lock().await.clone();
    let restored = Core::new(old, save.clone(), model.clone(), control.clone(), workspace.clone())
        .await
        .unwrap();
    let bridge = connect(state, restored.clone(), control.clone());
    *handle.lock().unwrap() = Arc::downgrade(&bridge);
    bridge.start().await.unwrap();
    bridge.ingest(&greeting).await.unwrap();
    bridge.ingest(&model_question).await.unwrap();
    bridge.ingest(&natural).await.unwrap();
    let count_replies = |traffic: &[Value]| {
        traffic.iter().filter(|row| {
            row["head"].as_str().is_some_and(|head| head.contains("/sendmessage "))
        }).count()
    };
    let status_query = msg("synthetic-clip-status", "好了吗？");
    let result_query = msg("synthetic-clip-result", "把结果发给我");
    let query_started = tokio::time::Instant::now();
    bridge.ingest(&status_query).await.unwrap();
    let status_ms = query_started.elapsed().as_millis();
    let result_started = tokio::time::Instant::now();
    bridge.ingest(&result_query).await.unwrap();
    let result_ms = result_started.elapsed().as_millis();
    let previously_sent = count_replies(sent.lock().await.as_slice());
    bridge.ingest(&status_query).await.unwrap();
    bridge.ingest(&result_query).await.unwrap();
    assert_eq!(count_replies(sent.lock().await.as_slice()), previously_sent,
        "replaying the same status and result messages must not send another reply");
    let saved = saved_wx.lock().await;
    let queries: Vec<_> = saved
        .records
        .iter()
        .filter(|record| record.args["current_query"]["target"] == turn)
        .map(|record| record.args["current_query"]["files"].as_bool().unwrap())
        .collect();
    assert_eq!(queries, [false, true]);
    drop(saved);
    let file_messages = |traffic: &[Value]| {
        traffic
            .iter()
            .filter(|row| {
                row["body"]["msg"]["item_list"][0]["file_item"]["file_name"] == "clip.mp4"
            })
            .count()
    };
    {
        let traffic = sent.lock().await;
        assert_eq!(
            file_messages(&traffic),
            1,
            "status and result must not duplicate an uncertain send"
        );
        assert!(
            traffic
                .iter()
                .filter(|row| row["body"]["msg"]["item_list"]
                    .to_string()
                    .contains("微信的附件发送回执没有核实"))
                .count()
                >= 3
        );
    }
    let resend = msg("synthetic-clip-resend", "重新发送附件");
    let resend_started = tokio::time::Instant::now();
    bridge.ingest(&resend).await.unwrap();
    let resend_ms = resend_started.elapsed().as_millis();
    let previously_sent = count_replies(sent.lock().await.as_slice());
    bridge.ingest(&resend).await.unwrap();
    assert_eq!(count_replies(sent.lock().await.as_slice()), previously_sent,
        "replaying the same resend request must not send another reply or file");
    let settled = bridge
        .status()
        .await
        .tasks
        .into_iter()
        .find(|task| task.job_id.as_deref() == Some(&turn))
        .unwrap();
    assert_eq!(settled.delivery, "accepted");
    assert!(settled.delivery_error.is_none());
    assert_eq!(
        restored.turn(&turn).await.unwrap().artifacts[0].sha256,
        artifact.sha256
    );
    println!("wechat_clip_evidence {}", json!({
        "native_render_ready_ms": completed_at.duration_since(started_at).as_millis(),
        "ready_to_initial_delivery_attempt_ms": into_initial_delivery_ms,
        "status_reply_ms": status_ms,
        "result_lookup_reply_ms": result_ms,
        "explicit_attachment_resend_ms": resend_ms,
        "original_render_calls": generation.load(Ordering::SeqCst),
        "identical_status_result_resend_messages_replied_again": false,
    }));
    let quoted = |message_id, text| {
        let mut message = msg(message_id, text);
        message["item_list"][0]["ref_msg"] = json!({"svr_id":ORIGINAL_REPLY});
        message
    };
    bridge.ingest(&msg("synthetic-clip-ambiguous-revision", "再短一点")).await.unwrap();
    assert!(sent.lock().await.iter().any(|row| row["body"]["msg"]["item_list"].to_string().contains("按原片时间剪，还是按上一版成片时间剪")));
    assert!(saved_wx.lock().await.records.iter().any(|row| row.args["quoted_assistant_v1"]["text"] == "再短一点"
        && row.status == "completed" && row.job_id.is_none() && row.args.get("clip_clarification_file_v1").is_none()));
    assert_eq!(history.load(Ordering::SeqCst), 7);
    assert_eq!(generation.load(Ordering::SeqCst), 1);
    let revision = quoted("synthetic-clip-revision", REVISION);
    bridge.ingest(&revision).await.unwrap();
    bridge.ingest(&revision).await.unwrap();
    let await_revision = |assistant: Arc<Core>, text: &'static str| {
        let ledger = saved_wx.clone();
        async move {
            loop {
                let target = {
                    let snapshot = ledger.lock().await;
                    let record = snapshot.records.iter().find(|row| row.args["quoted_assistant_v1"]["text"] == text)
                        .expect("revision should have an inbound receipt");
                    assert_ne!(record.status, "failed", "revision was not accepted: {:?}", record.result);
                    (record.delivery == "accepted" && record.status == "succeeded").then(|| record.job_id.clone().unwrap())
                };
                if let Some(id) = target { break assistant.turn(&id).await.unwrap(); }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }
    };
    let revised = tokio::time::timeout(Duration::from_secs(45), await_revision(restored.clone(), REVISION)).await.unwrap();
    assert_ne!(revised.id, turn);
    assert_eq!(revised.parent_turn_id.as_deref(), Some(turn.as_str()));
    let [revision_artifact] = revised.artifacts.as_slice() else { panic!("expected one new clip"); };
    let new_clip = std::fs::read(&revision_artifact.path).unwrap();
    assert_ne!(revision_artifact.sha256, artifact.sha256);
    assert_eq!(std::fs::read(&artifact.path).unwrap(), edited);
    assert_eq!(std::fs::read(&source_path).unwrap(), original);
    let binding = original_output.get().unwrap();
    let mut wrong = binding.clone();
    wrong.sha256 = "9".repeat(64);
    assert!(bridge.trim_clip_artifact(&ffmpeg, wrong, 0, 500).await.unwrap_err().starts_with("CLIP_SOURCE_UNAVAILABLE:"));
    assert!(restored.channel(json!({"operation":"resolve_clip_artifact","source_key":"other-binding","clip":binding})).await.is_err());
    assert!(bridge.trim_clip_artifact(&ffmpeg, binding.clone(), 0, 2000).await.unwrap_err().starts_with("CLIP_RANGE_INVALID:"));
    std::fs::write(&artifact.path, &new_clip).unwrap();
    assert!(bridge.trim_clip_artifact(&ffmpeg, binding.clone(), 0, 500).await.unwrap_err().starts_with("CLIP_SOURCE_CHANGED:"));
    std::fs::write(&artifact.path, &edited).unwrap();
    let question = quoted("synthetic-rendered-clip-question", "只留前半秒");
    bridge.ingest(&question).await.unwrap();
    assert_eq!(generation.load(Ordering::SeqCst), 2);
    let stored = saved_wx.lock().await.clone();
    assert_eq!(stored.records.last().unwrap().delivery, "accepted");
    assert_eq!(stored.records.last().unwrap().args["clip_clarification_file_v1"], json!(
        file_delivery::FileSelection::from_path(&artifact.path, &artifact.sha256).unwrap()
    ));
    #[cfg(windows)]
    let stored = {
        let ciphertext = crate::wechat_crypto::crypt(&serde_json::to_vec(&stored).unwrap(), true).unwrap();
        let plaintext = crate::wechat_crypto::crypt(&ciphertext, false).unwrap();
        serde_json::from_slice::<Stored>(&plaintext).unwrap()
    };
    bridge.stop().await.unwrap();
    let bridge = connect(stored, restored.clone(), bridge.control.clone());
    *handle.lock().unwrap() = Arc::downgrade(&bridge);
    bridge.start().await.unwrap();
    let rendered = msg("synthetic-rendered-clip", "按成片");
    bridge.ingest(&rendered).await.unwrap();
    bridge.ingest(&rendered).await.unwrap();
    bridge.ingest(&msg("synthetic-rendered-clip-second-answer", "按成片")).await.unwrap();
    let rendered = tokio::time::timeout(Duration::from_secs(45), await_revision(restored.clone(), RENDERED_REVISION)).await.unwrap();
    assert_eq!(rendered.parent_turn_id.as_deref(), Some(turn.as_str()));
    let [rendered_artifact] = rendered.artifacts.as_slice() else { panic!("expected one trimmed version"); };
    let trimmed = std::fs::read(&rendered_artifact.path).unwrap();
    assert!(rendered_artifact.sha256 != artifact.sha256 && rendered_artifact.sha256 != revision_artifact.sha256);
    let pixels = std::process::Command::new(&ffmpeg).args(["-loglevel","error","-i"]).arg(&rendered_artifact.path)
        .args(["-an","-vf","scale=1:1","-pix_fmt","rgb24","-f","rawvideo","pipe:1"]).output().unwrap();
    assert!(pixels.status.success());
    assert_eq!(pixels.stdout.len(), 15 * 3);
    assert!(pixels.stdout.chunks_exact(3).all(|p| p[2] > 200 && p[0] < 40), "trim starts in the blue section of the assembly, not the red original");
    assert_eq!(std::fs::read(&artifact.path).unwrap(), edited);
    assert_eq!(std::fs::read(&revision_artifact.path).unwrap(), new_clip);
    assert_eq!(std::fs::read(&source_path).unwrap(), original);
    chain_output.set(clip_video::ClipArtifactSource {
        session_id: rendered.session_id.clone(), turn_id: rendered.id.clone(),
        job_id: rendered_artifact.job_id.clone(), sha256: rendered_artifact.sha256.clone(),
    }).unwrap();
    let implicit = msg("synthetic-clip-implicit-revision", IMPLICIT_REVISION);
    bridge.ingest(&implicit).await.unwrap();
    bridge.ingest(&implicit).await.unwrap();
    let chained = tokio::time::timeout(Duration::from_secs(45), await_revision(restored.clone(), IMPLICIT_REVISION)).await.unwrap();
    assert_eq!(chained.parent_turn_id.as_deref(), Some(rendered.id.as_str()));
    let [chained_artifact] = chained.artifacts.as_slice() else { panic!("expected one additional clip"); };
    let chained_bytes = std::fs::read(&chained_artifact.path).unwrap();
    assert_ne!(chained_artifact.sha256, rendered_artifact.sha256);
    assert_eq!(std::fs::read(&rendered_artifact.path).unwrap(), trimmed);
    let durable_assistant = serde_json::from_slice(&serde_json::to_vec(&*saved_core.lock().await).unwrap()).unwrap();
    let durable_wechat = serde_json::from_slice(&serde_json::to_vec(&*saved_wx.lock().await).unwrap()).unwrap();
    bridge.stop().await.unwrap();
    let restored = Core::new(durable_assistant, save, model, control.clone(), workspace.clone()).await.unwrap();
    let bridge = connect(durable_wechat, restored.clone(), bridge.control.clone());
    *handle.lock().unwrap() = Arc::downgrade(&bridge);
    bridge.start().await.unwrap();
    let back = msg("synthetic-clip-back-to-original", BACK_TO_ORIGINAL);
    bridge.ingest(&back).await.unwrap();
    {
        let ledger = saved_wx.lock().await;
        let record = ledger.records.last().unwrap();
        assert_eq!(record.command, "assistant", "the current version must resolve the original timeline");
        assert!(record.job_id.is_some());
    }
    bridge.ingest(&back).await.unwrap();
    let back = tokio::time::timeout(Duration::from_secs(45), await_revision(restored.clone(), BACK_TO_ORIGINAL)).await.unwrap();
    assert_eq!(back.parent_turn_id.as_deref(), Some(chained.id.as_str()));
    let [back_artifact] = back.artifacts.as_slice() else { panic!("expected a new original-timeline clip"); };
    let back_bytes = std::fs::read(&back_artifact.path).unwrap();
    let pixels = std::process::Command::new(&ffmpeg).args(["-loglevel","error","-i"]).arg(&back_artifact.path)
        .args(["-an","-vf","scale=1:1","-pix_fmt","rgb24","-f","rawvideo","pipe:1"]).output().unwrap();
    assert!(pixels.status.success());
    assert_eq!(pixels.stdout.len(), 21 * 3);
    assert!(pixels.stdout.chunks_exact(3).all(|p| p[0] > 200 && p[2] < 40), "new clip uses the original red frames");
    assert_eq!(std::fs::read(&chained_artifact.path).unwrap(), chained_bytes);
    assert_eq!(std::fs::read(&source_path).unwrap(), original);
    let unrelated = restored.channel(json!({"operation":"submit","source_key":bridge.source_key().await,
        "request_id":"synthetic-unrelated-current","text":"列一个室内活动清单"})).await.unwrap();
    let unrelated_id = unrelated["turn_id"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        while restored.turn(unrelated_id).await.unwrap().state != "completed" {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    bridge.ingest(&msg("synthetic-clip-after-unrelated", IMPLICIT_REVISION)).await.unwrap();
    let mut cover = msg("synthetic-video-cover", COVER);
    cover["item_list"][0]["ref_msg"] = json!({"message_item":{"type":5,"msg_id":"9007199254741013"}});
    bridge.ingest(&cover).await.unwrap();
    bridge.ingest(&cover).await.unwrap();
    let cover_id = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let ledger = saved_wx.lock().await;
            if let Some(record) = ledger.records.iter().find(|row| row.args["text"] == COVER) {
                assert_ne!(record.status, "failed", "{:?}", record.result);
                if record.status == "succeeded" && record.delivery == "accepted" {
                    break record.job_id.clone().unwrap();
                }
            }
            drop(ledger);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    let files = restored.turn(&cover_id).await.unwrap().artifacts;
    assert_eq!(files.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["cover-wide.png", "cover-portrait.png", "cover-vertical.png"]);
    assert!(files.iter().all(|a| a.job_id == files[0].job_id));
    let covers = files.iter().map(|a| std::fs::read(&a.path).unwrap()).collect::<Vec<_>>();
    bridge.ingest(&msg("synthetic-cover-status", "好了吗？")).await.unwrap();
    assert_eq!(std::fs::read(&source_path).unwrap(), original);
    let transcript_root = source_path.parent().unwrap().join("transcripts");
    let task = transcript_root.join("fixture");
    let input_caption = "1\n00:00:00,200 --> 00:00:00,800\n前段\n\n2\n00:00:01,800 --> 00:00:02,650\n跨越切点\n";
    let input_caption_path = task.join("runs").join(subtitles::WHISPER_ENGINE).join(subtitles::NAME);
    std::fs::create_dir_all(input_caption_path.parent().unwrap()).unwrap();
    std::fs::write(&input_caption_path, input_caption).unwrap();
    let (caption_kind, caption_sha) = subtitles::observe_timed_source(&task, &transcript_root).unwrap();
    assert_eq!(caption_kind, "whisper_run");
    std::fs::write(&input_caption_path, "损坏字幕").unwrap();
    bridge.change(&source.receipt_id, |row| {
        row.status = "completed".into();
        row.result = Some(json!({"text":"合成转写","artifact":{"path":task.join("exports/transcript.txt"),
            "timed_subtitle_kind":caption_kind,"timed_subtitle_sha256":caption_sha}}));
    }).await.unwrap();
    let ranges = [clip_video::ClipRange { start_ms: 2000, duration_ms: 600 }, clip_video::ClipRange { start_ms: 100, duration_ms: 500 }];
    assert!(bridge.assemble_subtitled_original(&ffmpeg, source.clone(), &ranges).await.unwrap_err().contains("摘要"));
    std::fs::write(&input_caption_path, input_caption).unwrap();
    let mut timed_request = msg("synthetic-video-timed-assembly", TIMED_EDIT);
    timed_request["item_list"][0]["ref_msg"] = cover["item_list"][0]["ref_msg"].clone();
    bridge.ingest(&timed_request).await.unwrap();
    bridge.ingest(&timed_request).await.unwrap();
    let timed_id = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let ledger = saved_wx.lock().await;
            if let Some(row) = ledger.records.iter().find(|row| row.args["text"] == TIMED_EDIT) {
                assert_ne!(row.status, "failed", "{:?}", row.result);
                if row.status == "succeeded" && row.delivery == "accepted" {
                    break row.job_id.clone().unwrap();
                }
            }
            drop(ledger);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    let timed_files = restored.turn(&timed_id).await.unwrap().artifacts;
    assert_eq!(timed_files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["clip.mp4", "subtitles.srt"]);
    assert_eq!(timed_files[0].job_id, timed_files[1].job_id);
    let timed_bytes = timed_files.iter().map(|f| std::fs::read(&f.path).unwrap()).collect::<Vec<_>>();
    let (_, preview) = restored.artifact_content(&timed_files[1].id, true).await.unwrap();
    assert_eq!(preview, timed_bytes[1]);
    assert_eq!(std::str::from_utf8(&timed_bytes[1]).unwrap(), "1\n00:00:00,000 --> 00:00:00,600\n跨越切点\n\n2\n00:00:00,700 --> 00:00:01,100\n前段\n\n");
    assert_eq!(std::fs::read_to_string(&input_caption_path).unwrap(), input_caption);
    let mut package_request = msg("synthetic-video-package", PACKAGE);
    package_request["item_list"][0]["ref_msg"] = cover["item_list"][0]["ref_msg"].clone();
    bridge.ingest(&package_request).await.unwrap();
    bridge.ingest(&package_request).await.unwrap();
    let package_turn = tokio::time::timeout(Duration::from_secs(55), async {
        loop {
            let ledger = saved_wx.lock().await;
            if let Some(row) = ledger.records.iter().find(|row| row.args["text"] == PACKAGE) {
                assert_ne!(row.status, "failed", "{:?}", row.result);
                if row.status == "succeeded" && row.delivery == "accepted" {
                    break row.job_id.clone().unwrap();
                }
            }
            drop(ledger);
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }).await.unwrap();
    let package = restored.turn(&package_turn).await.unwrap().artifacts[0].clone();
    assert_eq!(package.name, publish_package::FILENAME);
    let (_, package_data) = restored.artifact_content(&package.id, false).await.unwrap();
    {
        use std::io::{Cursor, Read};
        let mut zip = zip::ZipArchive::new(Cursor::new(&package_data)).unwrap();
        assert_eq!(zip.len(), 7);
        let manifest: Value = serde_json::from_reader(zip.by_name("manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["source"]["sha256"], source.sha256);
        assert_eq!(manifest["source"]["wechat_receipt_id"], source.receipt_id);
        let files = manifest["files"].as_array().unwrap();
        assert_eq!(files.len(), 6);
        for entry in files {
            let name = entry["name"].as_str().unwrap();
            let mut bytes = Vec::new();
            zip.by_name(name).unwrap().read_to_end(&mut bytes).unwrap();
            assert_eq!(entry["sha256"], format!("{:x}", Sha256::digest(&bytes)));
            match name {
                "video-vertical.mp4" => {
                    let video = lab.path().join("verified-vertical.mp4");
                    std::fs::write(&video, bytes).unwrap();
                    let decoded = std::process::Command::new(&ffmpeg)
                        .args(["-hide_banner", "-i"]).arg(video)
                        .args(["-an", "-vf", "crop=2:2,scale=1:1", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"])
                        .output().unwrap();
                    assert!(decoded.status.success());
                    assert!(String::from_utf8_lossy(&decoded.stderr).contains("720x1280"));
                    assert_eq!(decoded.stdout.len(), 33 * 3);
                    assert!(decoded.stdout[..18 * 3].chunks_exact(3).all(|px| px[2] > 200 && px[0] < 40));
                    assert!(decoded.stdout[18 * 3..].chunks_exact(3).all(|px| px[0] > 200 && px[2] < 40));
                }
                "subtitles.srt" => assert_eq!(bytes, timed_bytes[1]),
                "post.md" => assert_eq!(String::from_utf8(bytes).unwrap(), "# 合成蓝色标题\n\n隔离发布简介\n\n![封面](cover-vertical.png)\n"),
                "cover-wide.png" => assert_eq!(&bytes[..24], &covers[0][..24]),
                "cover-portrait.png" => assert_eq!(&bytes[..24], &covers[1][..24]),
                "cover-vertical.png" => assert_eq!(&bytes[..24], &covers[2][..24]),
                _ => panic!("unexpected ZIP entry"),
            }
        }
    }
    assert_eq!(std::fs::read(&source_path).unwrap(), original);
    bridge.ingest(&msg("synthetic-package-status", "好了吗？")).await.unwrap();
    bridge
        .ingest(&msg("synthetic-clip-new-session", "新对话"))
        .await
        .unwrap();
    bridge
        .ingest(&msg(
            "synthetic-clip-new-session-request",
            "把刚发的视频剪一下",
        ))
        .await
        .unwrap();
    let mut quote = msg("synthetic-clip-foreign-session", "帮我按之前的方式剪辑");
    quote["item_list"][0]["ref_msg"] =
        json!({"message_item":{"type":5,"msg_id":"9007199254741013"}});
    bridge.ingest(&quote).await.unwrap();
    bridge.ingest(&quoted("synthetic-clip-cross-session-revision", REVISION)).await.unwrap();
    bridge.ingest(&quoted("synthetic-clip-cross-session-rendered", RENDERED_REVISION)).await.unwrap();
    bridge.ingest(&msg("synthetic-clip-cross-session-implicit", IMPLICIT_REVISION)).await.unwrap();
    assert_eq!(generation.load(Ordering::SeqCst), 8);
    assert_eq!(history.load(Ordering::SeqCst), 34);
    assert_eq!(lookups.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        description
    );
    let traffic = sent.lock().await;
    assert_eq!(file_messages(&traffic), 7);
    let uploads = traffic
        .iter()
        .filter(|r| {
            r["head"]
                .as_str()
                .unwrap_or_default()
                .contains("/c2c/upload?")
        })
        .collect::<Vec<_>>();
    let keys = traffic
        .iter()
        .filter(|r| {
            r["head"]
                .as_str()
                .unwrap_or_default()
                .contains("/getuploadurl ")
        })
        .map(|r| r["body"]["aeskey"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!((uploads.len(), keys.len()), (12, 12));
    for ((upload, key), expected) in uploads.into_iter().zip(keys).zip([&edited, &edited, &new_clip, &trimmed, &chained_bytes, &back_bytes].into_iter().chain(&covers).chain(&timed_bytes).chain([&package_data])) {
        let key = (0..32)
            .step_by(2)
            .map(|i| u8::from_str_radix(&key[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let encrypted = STANDARD
            .decode(upload["raw_base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            ecb::Decryptor::<aes::Aes128>::new_from_slice(&key)
                .unwrap()
                .decrypt_padded_vec_mut::<Pkcs7>(&encrypted)
                .unwrap(),
            expected.as_slice()
        );
    }
    assert!(traffic.iter().any(|r| r["body"]["msg"]["item_list"]
        .to_string()
        .contains("原视频属于另一会话")));
    assert!(traffic.iter().any(|r| r["body"]["msg"]["item_list"]
        .to_string()
        .contains("当前对话最近没有可选的原视频")));
    assert_eq!(traffic.iter().filter(|r| r["body"]["msg"]["item_list"]
        .to_string()
        .contains("我暂时无法确定要修改的版本")).count(), 2);
    for expected in ["你好呀，随时可以把视频发给我。", "我现在用的是 synthetic/model:free，通过 openrouter 调用。"] {
        assert_eq!(traffic.iter().filter(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"] == expected).count(), 1);
    }
    drop(traffic);
    bridge.stop().await.unwrap();
    #[cfg(windows)]
    {
        let contract: Value = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
        let catalog_hash = contract["catalog_hash"].as_str().map(str::to_owned);
        let lookup = |job_id: &str| Request {
            protocol: 1, id: "fixture-reopen-trim".into(), token: "isolated".into(),
            method: "jobs.get".into(), command: None, args: json!({}), request_id: None,
            job_id: Some(job_id.to_string()),
            catalog_hash: catalog_hash.clone(),
            grants: vec!["write".into(), "private".into()],
        };
        let opened = Controller::new(contract, &control_root, runner).unwrap();
        let result = opened.clone().handle(lookup(&rendered_artifact.job_id)).await.unwrap();
        assert_eq!(result["status"], "succeeded");
        assert_eq!(result["result"]["artifact"]["sha256"], rendered_artifact.sha256);
        let result = opened.clone().handle(lookup(&timed_files[0].job_id)).await.unwrap();
        assert_eq!(result["result"]["subtitles"]["sha256"], timed_files[1].sha256);
        std::fs::write(&timed_files[1].path, "字幕产物替换").unwrap();
        let result = opened.clone().handle(lookup(&timed_files[0].job_id)).await.unwrap();
        assert_eq!(result["error"]["code"], "RESULT_UNAVAILABLE");
        assert!(result["result"].is_null());
        let result = opened.clone().handle(lookup(&package.job_id)).await.unwrap();
        assert_eq!(result["result"]["artifact"]["sha256"], package.sha256);
        std::fs::write(&package.path, "发布包被替换").unwrap();
        let result = opened.handle(lookup(&package.job_id)).await.unwrap();
        assert_eq!(result["error"]["code"], "RESULT_UNAVAILABLE");
        assert!(result["result"].is_null());
    }
    server.abort();
}

#[test]
fn clip_timeline_choice_requires_the_current_accepted_prompt_with_a_range() {
    assert_eq!(clip_video::timeline_answer("按成片。"), Some("上一版成片的"));
    assert_eq!(clip_video::timeline_answer("按成片？"), None);
    assert_eq!(clip_video::timeline_answer("按原片，顺便再发到群里"), None);
    let now = chrono::Utc::now();
    let mut stored = account("https://example.invalid");
    let source = stored.logical_key();
    let turn = uuid::Uuid::new_v4().to_string();
    let file = file_delivery::FileSelection::from_path("fixture.mp4", &"d".repeat(64)).unwrap();
    let prompt = Record {
        command: "message".into(), status: "completed".into(), delivery: "accepted".into(),
        created_at: now.to_rfc3339(), args: json!({
            "quoted_assistant_v1":{"source":source,"turn":turn,"text":"只留前0.3秒"},
            "clip_clarification_file_v1":file
        }), ..Record::default()
    };
    stored.records.push(prompt.clone());
    let choice = |stored: &Stored, source: &str, at| Bridge::clip_timeline_followup(stored, source, "原片", at);
    assert!(matches!(choice(&stored, &source, now), Some(Action::AssistantFollowup { turn: id, file: Some(selection), text })
        if id == turn && selection == file && text == "只留原片前0.3秒"));
    assert!(choice(&stored, "other-binding", now).is_none());
    assert!(choice(&stored, &source, now + chrono::TimeDelta::minutes(16)).is_none());
    assert!(choice(&stored, &source, now - chrono::TimeDelta::seconds(1)).is_none());
    for status in ["pending", "unknown"] {
        stored.records[0].delivery = status.into();
        assert!(choice(&stored, &source, now).is_none());
    }
    stored.records[0] = prompt;
    stored.records[0].args["quoted_assistant_v1"]["text"] = json!("只留前几秒，再接后面两秒");
    assert!(choice(&stored, &source, now).is_none());
    stored.records[0].args["quoted_assistant_v1"]["text"] = json!("只留前半秒");
    stored.records.push(Record::default());
    assert!(choice(&stored, &source, now).is_none());
}

#[tokio::test]
async fn implicit_clip_followup_keeps_the_file_selected_before_restart() {
    let lab = tempfile::tempdir().unwrap();
    let (base, traffic, server) = fake_api(200, json!({})).await;
    let stored = account(&base);
    let effects = Arc::new(AtomicUsize::new(0));
    let (template, saved) = test_bridge(stored.clone(), lab.path(), controller(lab.path(), effects.clone()));
    let selected = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let file = file_delivery::FileSelection::from_path("fixture-first.mp4", &"d".repeat(64)).unwrap();
    let entered = Arc::new(Notify::new());
    let hold: Agent = Arc::new({
        let (entered, selected, file) = (entered.clone(), selected.clone(), file.clone());
        move |request| {
            let (entered, selected, file) = (entered.clone(), selected.clone(), file.clone());
            Box::pin(async move {
                match request["operation"].as_str() {
                    Some("resolve_current_clip") => Ok(json!({"target":selected,"file":file})),
                    Some("submit_quoted_task") => {
                        assert_eq!(request["turn_id"], selected);
                        assert_eq!(request["file"], json!(file));
                        entered.notify_one();
                        std::future::pending().await
                    }
                    _ => panic!("unexpected operation: {request}"),
                }
            })
        }
    });
    let make_bridge = |stored, agent| {
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), lab.path().into(), Some(agent))
    };
    let bridge = make_bridge(stored, hold);
    let incoming = msg("implicit-before-restart", "只留这版成片前半秒");
    let first = tokio::spawn({
        let incoming = incoming.clone();
        async move { bridge.ingest(&incoming).await }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await.unwrap();
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let durable = saved.lock().await.clone();
    let pending = durable.records.last().unwrap();
    assert_eq!(pending.command, "message");
    assert_eq!(pending.pending_assistant_followup().unwrap()["turn"], selected);
    assert_eq!(pending.pending_assistant_followup().unwrap()["file"], json!(file));
    let receipt = pending.id.clone();
    let resolves = Arc::new(AtomicUsize::new(0));
    let resumed = Arc::new(AtomicUsize::new(0));
    let after: Agent = Arc::new({
        let (resolves, resumed) = (resolves.clone(), resumed.clone());
        move |request| {
            let (resolves, resumed, selected, other, file) =
                (resolves.clone(), resumed.clone(), selected.clone(), other.clone(), file.clone());
            Box::pin(async move {
                match request["operation"].as_str() {
                    Some("resolve_current_clip") => {
                        resolves.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({"target":other,"file":file}))
                    }
                    Some("submit_quoted_task") => {
                        assert_eq!(request["turn_id"], selected);
                        assert_eq!(request["file"], json!(file));
                        resumed.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({"action":"message","text":"继续核对原成片"}))
                    }
                    _ => panic!("unexpected operation: {request}"),
                }
            })
        }
    });
    let reopened = make_bridge(serde_json::from_slice(&serde_json::to_vec(&durable).unwrap()).unwrap(), after);
    reopened.submit(&receipt).await.unwrap();
    reopened.ingest(&incoming).await.unwrap();
    assert_eq!(resolves.load(Ordering::SeqCst), 0);
    assert_eq!(resumed.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(traffic.lock().await.iter().filter(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"] == "继续核对原成片").count(), 1);
    server.abort();
}

#[tokio::test]
async fn recent_video_ignores_expired_future_and_foreign_receipts() {
    let lab = tempfile::tempdir().unwrap();
    let scope = uuid::Uuid::new_v4().to_string();
    let mut stored = account("https://example.invalid");
    let source = stored.logical_key();
    let now = chrono::Utc::now();
    let row = |source: &str, scope: &str, minutes: i64| Record {
        id: format!("wx-{}", uuid::Uuid::new_v4()),
        command: "wechat_video_transcript".into(),
        status: "failed".into(),
        created_at: (now + chrono::TimeDelta::minutes(minutes)).to_rfc3339(),
        args: json!({"source":source,"scope":scope,"original_sha256":"a".repeat(64)}),
        ..Record::default()
    };
    stored.records = vec![
        row(&source, &scope, -16),
        row(&source, &uuid::Uuid::new_v4().to_string(), -1),
        row("another-binding", &scope, -1),
        row(&source, &scope, 1),
    ];
    let account = stored.account();
    let agent_scope = scope.clone();
    let bridge = Bridge::new_with_agent(
        stored,
        Api::new().unwrap(),
        Arc::new(|_| Box::pin(async { panic!("selection must not write to the ledger") })),
        Arc::new(|_| Box::pin(async { panic!("selection must not dispatch work") })),
        lab.path().into(),
        Some(Arc::new(move |request| {
            assert_eq!(request["operation"], "current_intake_scope");
            let scope = agent_scope.clone();
            Box::pin(async move { Ok(json!({"session_id":scope})) })
        })),
    );
    let text = "把刚发的视频剪一下";
    assert!(matches!(
        bridge.recent_video_clip(&msg("synthetic-recent-bound", text), &account, 0, text, now).await,
        Some(Err(message)) if message.contains("当前对话最近没有可选的原视频")
    ));
    let implicit = "帮我按照之前的方式完成一下剪辑";
    assert!(bridge.recent_video_clip(&msg("synthetic-no-implicit-source", implicit), &account, 0, implicit, now).await.is_none());
    let previous = "请剪一下倒数第 2 段原片";
    let request = msg("synthetic-previous-material", previous);
    assert!(matches!(
        bridge.recent_video_clip(&request, &account, 0, previous, now).await,
        Some(Err(message)) if message.contains("倒数第二段")
    ));
    let older = row(&source, &scope, -3);
    let selected = row(&source, &scope, -2);
    let latest = row(&source, &scope, -1);
    bridge.state.lock().await.stored.records.push(latest.clone());
    assert!(matches!(
        bridge.recent_video_clip(&request, &account, 0, previous, now).await,
        Some(Err(message)) if message.contains("倒数第二段")
    ));
    {
        let mut state = bridge.state.lock().await;
        assert_eq!(state.stored.records.pop().unwrap().id, latest.id);
        state.stored.records.extend([older, selected.clone(), latest]);
    }
    assert!(matches!(
        bridge.recent_video_clip(&request, &account, 0, previous, now).await,
        Some(Ok(Action::Call(command, args))) if command == "assistant" && args["video_clip_receipt"] == selected.id
    ));
    {
        let mut state = bridge.state.lock().await;
        let target = state.stored.records.iter_mut().find(|record| record.id == selected.id).unwrap();
        target.args["original_sha256"] = Value::Null;
    }
    assert!(matches!(
        bridge.recent_video_clip(&request, &account, 0, previous, now).await,
        Some(Err(message)) if message.contains("没有接收成功")
    ));
    {
        let mut state = bridge.state.lock().await;
        state.stored.records.last_mut().unwrap().args["scope"] = Value::Null;
    }
    assert!(matches!(
        bridge.recent_video_clip(&request, &account, 0, previous, now).await,
        Some(Err(message)) if message.contains("不能确认")
    ));
}
