//! Context is a bounded view of current work, never a mutation or new authority.
use super::*;

#[tokio::test]
async fn original_clip_lineage_requires_verified_prior_artifacts_in_the_same_session() {
    use crate::wechat::clip_video::{ClipArtifactSource, ClipSource, TRIM_ARTIFACT_COMMAND};

    let (engine, models, effects, _, lab) = fixture(false, vec![]).await;
    let mut links: Vec<ClipArtifactSource> = Vec::new();
    let mut source = None;
    for index in 0..3 {
        let ack = engine.send_from(request("剪辑测试", &format!("lineage-{index}")), "wechat", "fixture-lineage")
            .await.unwrap();
        let binding = ClipArtifactSource {
            session_id: ack.session_id.clone(), turn_id: ack.turn_id.clone(), job_id: id(),
            sha256: format!("{:064x}", index + 1),
        };
        let original = ClipSource {
            receipt_id: format!("wx-{}", "a".repeat(64)),
            session_id: ack.session_id.clone(), sha256: "b".repeat(64),
        };
        engine.change_turn(&ack.turn_id, |session, position| {
            let turn = &mut session.turns[position];
            turn.state = "completed".into();
            let command = if let Some(parent) = links.last() {
                turn.parent_turn_id = Some(parent.turn_id.clone());
                turn.runtime_models.insert(CLIP_ARTIFACT_CHECKPOINT.into(), json!(parent));
                TRIM_ARTIFACT_COMMAND
            } else {
                turn.clip_source = Some(original.clone());
                "capability_assemble_video"
            };
            session.jobs.push(Job {
                id: binding.job_id.clone(), command: command.into(), turn_id: binding.turn_id.clone(),
                state: "succeeded".into(), result: json!({"status":"succeeded"}), owned: true,
                deliver: true, remote: None, archived_result_bytes: None,
                artifacts: vec![Artifact {
                    id: format!("file-{}", binding.job_id), job_id: binding.job_id.clone(), name: "clip.mp4".into(),
                    path: lab.path().join("clips").join(format!("clip-{}", binding.job_id)).join("clip.mp4").to_string_lossy().into_owned(),
                    bytes: 6, sha256: binding.sha256.clone(), mime_type: "video/mp4".into(),
                }],
            });
            Ok(())
        }).await.unwrap();
        if index == 0 { source = Some(original); }
        links.push(binding);
    }

    let pristine = engine.state.lock().await.sessions[0].clone();
    let resolve = |session: &Session| {
        context::original_clip_source(session, &session.turns[2], &session.jobs[2])
            .map(|original| original.receipt_id.clone())
    };
    assert_eq!(resolve(&pristine), source.map(|original| original.receipt_id));

    let mut changed = pristine.clone();
    changed.jobs[0].artifacts[0].sha256 = "f".repeat(64);
    assert!(resolve(&changed).is_none(), "an earlier artifact digest changed");
    let mut changed = pristine.clone();
    changed.jobs[1].owned = false;
    assert!(resolve(&changed).is_none(), "the parent job is not owned");
    let mut changed = pristine.clone();
    changed.turns[1].state = "failed".into();
    assert!(resolve(&changed).is_none(), "an earlier turn is not completed");
    let mut changed = pristine.clone();
    let foreign = ClipArtifactSource { session_id: id(), ..links[1].clone() };
    changed.turns[2].runtime_models.insert(CLIP_ARTIFACT_CHECKPOINT.into(), json!(foreign));
    assert!(resolve(&changed).is_none(), "the parent references another session");
    let mut changed = pristine;
    changed.turns[2].parent_turn_id = Some(links[2].turn_id.clone());
    changed.turns[2].runtime_models.insert(CLIP_ARTIFACT_CHECKPOINT.into(), json!(links[2]));
    assert!(resolve(&changed).is_none(), "a self reference cannot be followed");
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[test]
fn output_revision_requests_keep_workflow_scope_for_clear_changes() {
    for text in [
        "照旧，这次把封面配色换成绿色。",
        "按之前的做法再做一版，这次改成浅绿色。",
        "照旧，这一版画幅改成 16:9。",
        "再照上次的流程做一版，新版格式改成 PNG。",
        "再来一版。",
        "麻烦你就照咱们上回那套再来一版，这次改成浅绿色。",
    ] {
        assert!(continuity::workflow_reuse_shorthand(text), "{text}");
    }
    for text in [
        "照旧，这次剪一条新视频。",
        "照旧，这次把另一张图片替换成绿色。",
        "照旧，这次把封面配色换成绿色，再发到群里。",
        "再来一版？",
        "再做一次。",
        "帮我再来一版新视频。",
        "照咱们上回那套再来一版，再发到群里。",
        "再来一版，这次颜色改成绿色然后发到群里。",
    ] {
        assert!(!continuity::workflow_reuse_shorthand(text), "{text}");
    }
}

#[tokio::test]
async fn successive_workflow_reuse_keeps_the_latest_result_and_the_original_topic() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    engine.memory_save(crate::assistant_personal::MemoryInput {
        revision: 0, id: None, title: "默认出图".into(),
        content: "fixture-multi-version-image-style".into(), expires_at: None,
    }).await.unwrap();
    const ORIGINAL: &str = "帮我生成一张山水封面图片。";
    const DIFFERENT_COLOR: &str = "照旧，这次把封面配色换成绿色。";
    const DIFFERENT_FORMAT: &str = "再来一版，新版格式改成 PNG。";
    const AGAIN: &str = "再来一版。";
    let mut previous: Option<String> = None;
    for (index, text) in [ORIGINAL, DIFFERENT_COLOR, DIFFERENT_FORMAT, AGAIN, AGAIN].into_iter().enumerate() {
        let ack = engine.send_from(request(text, &format!("fixture-version-{index}")), "wechat", "version-owner")
            .await.unwrap();
        engine.prepare_context(&ack.turn_id).await.unwrap();
        engine.change_turn(&ack.turn_id, |session, position| {
            let turn = &mut session.turns[position];
            assert_eq!(turn.parent_turn_id, previous);
            assert_eq!(turn.protocol[0]["content"], text);
            assert!(turn.runtime_prompt.contains("fixture-multi-version-image-style"));
            if index == 4 {
                assert!(turn.runtime_prompt.contains("cover-3.png"));
                assert!(turn.runtime_prompt.contains(ORIGINAL));
                assert!(turn.runtime_prompt.contains(
                    &format!("\"workflow_revision_requests\":{}", json!([
                        {"request":DIFFERENT_FORMAT,"request_truncated":false},
                        {"request":DIFFERENT_COLOR,"request_truncated":false},
                    ]))
                ));
                assert!(!turn.runtime_prompt.contains("fixture-private/"));
                // A corrupt forward/self link must never resolve a workflow topic.
                turn.parent_turn_id = Some(ack.turn_id.clone());
            }
            turn.state = "completed".into();
            let job_id = format!("version-job-{index}");
            session.jobs.push(Job {
                id: job_id.clone(), command: "model_capability_call".into(), turn_id: ack.turn_id.clone(),
                state: "succeeded".into(), result: json!({"status":"succeeded"}), owned: true,
                deliver: true, remote: None, archived_result_bytes: None,
                artifacts: vec![Artifact {
                    id: format!("version-file-{index}"), job_id, name: format!("cover-{index}.png"),
                    path: format!("fixture-private/cover-{index}.png"), bytes: 3,
                    sha256: "1".repeat(64), mime_type: "image/png".into(),
                }],
            });
            Ok(())
        }).await.unwrap();
        previous = Some(ack.turn_id);
    }
    let next = engine.send_from(request(AGAIN, "fixture-version-invalid-link"), "wechat", "version-owner")
        .await.unwrap();
    engine.prepare_context(&next.turn_id).await.unwrap();
    let state = engine.state.lock().await;
    let (s, t) = locate(&state, &next.turn_id).unwrap();
    let turn = &state.sessions[s].turns[t];
    assert!(turn.parent_turn_id.is_none());
    assert!(!turn.runtime_prompt.contains("fixture-multi-version-image-style"));
    assert!(turn.runtime_prompt.contains("目前没有可核对的对象"));
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unquoted_workflow_reuse_binds_only_the_immediately_preceding_verified_result() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    engine.memory_save(crate::assistant_personal::MemoryInput {
        revision: 0, id: None, title: "默认出图".into(),
        content: "fixture-verified-image-preset".into(), expires_at: None,
    }).await.unwrap();
    const REUSE: &str = "再来一版。";
    const ORIGINAL: &str = "帮我生成一张山水封面图片。";
    for (source, owner, previous_state, intervening, new_session) in [
        ("desktop", "desktop", "completed", false, false),
        ("wechat", "fixture-followup-same-chat", "completed", false, false),
        ("wechat", "fixture-followup-intervening", "completed", true, false),
        ("wechat", "fixture-followup-failed", "failed", false, false),
        ("wechat", "fixture-followup-late", "waiting_input_queue", false, false),
        ("wechat", "fixture-followup-new-chat", "completed", false, true),
    ] {
        let first = engine.send_from(request(ORIGINAL, &format!("first-{owner}")), source, owner)
            .await.unwrap();
        engine.change_turn(&first.turn_id, |session, index| {
            session.turns[index].state = previous_state.into();
            let job_id = format!("image-job-{owner}");
            session.jobs.push(Job {
                id: job_id.clone(), command: "model_capability_call".into(), turn_id: first.turn_id.clone(),
                state: "succeeded".into(), result: json!({"status":"succeeded"}), owned: true,
                deliver: true, remote: None, archived_result_bytes: None,
                artifacts: vec![Artifact {
                    id: format!("image-artifact-{owner}"), job_id, name: "cover.png".into(),
                    path: "fixture-private/cover.png".into(), bytes: 6,
                    sha256: "1".repeat(64), mime_type: "image/png".into(),
                }],
            });
            Ok(())
        }).await.unwrap();
        if intervening {
            let mut other = request("聊聊另一个事项", &format!("intervened-{owner}"));
            other.session_id = Some(first.session_id.clone());
            engine.send_from(other, source, owner).await.unwrap();
        }
        let mut short = request(REUSE, &format!("reuse-{owner}"));
        if new_session {
            engine.channel(json!({"operation":"submit","source_key":owner,
                "request_id":"new-chat-before-short-reuse","text":"新对话"})).await.unwrap();
        } else {
            short.session_id = Some(first.session_id.clone());
        }
        let next = engine.send_from(short.clone(), source, owner).await.unwrap();
        if previous_state == "waiting_input_queue" {
            engine.change_turn(&first.turn_id, |session, index| {
                session.turns[index].state = "completed".into();
                Ok(())
            }).await.unwrap();
        }
        // Later traffic cannot replace the result bound at acceptance.
        let mut later = request("帮我剪辑一段视频", &format!("later-{owner}"));
        later.session_id = Some(next.session_id.clone());
        engine.send_from(later, source, owner).await.unwrap();
        assert_eq!(engine.send_from(short, source, owner).await.unwrap().turn_id, next.turn_id);
        engine.prepare_context(&next.turn_id).await.unwrap();
        let state = engine.state.lock().await;
        let (session, turn) = locate(&state, &next.turn_id).unwrap();
        let followup = &state.sessions[session].turns[turn];
        let expected = previous_state == "completed" && !intervening && !new_session;
        assert_eq!(followup.parent_turn_id.as_deref(), expected.then_some(first.turn_id.as_str()));
        assert_eq!(followup.runtime_prompt.contains("fixture-verified-image-preset"), expected);
        assert_eq!(followup.runtime_prompt.contains("目前没有可核对的对象"), !expected);
        assert!(!followup.runtime_prompt.contains("fixture-private/"));
        assert_eq!(followup.protocol[0]["content"], REUSE);
    }
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn quoted_workflow_reuse_recalls_current_preferences_for_the_original_task() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    let owner = "workflow-reference-owner";
    let submit = |id: &str, text: &str| json!({"operation":"submit",
        "source_key":owner,"request_id":id,"text":text});
    let quoted = |id: &str, source: &str, text: &str, target: &str| json!({
        "operation":"submit_quoted_task","source_key":source,"request_id":id,
        "text":text,"turn_id":target
    });
    let save = |revision, id, title: &str, content: &str| crate::assistant_personal::MemoryInput {
        revision, id, title: title.into(), content: content.into(), expires_at: None,
    };
    let original = engine.channel(submit("workflow-origin", "帮我生成一张图片，做成蓝色山水封面。"))
        .await.unwrap();
    let original_turn = original["turn_id"].as_str().unwrap();
    engine.change_turn(original_turn, |session, index| {
        session.turns[index].state = "completed".into();
        Ok(())
    }).await.unwrap();
    engine.channel(submit("workflow-new-chat", "新对话")).await.unwrap();
    engine.channel(submit("workflow-other-job", "把这个视频剪成三段")).await.unwrap();

    let first = engine.memory_save(save(0, None, "默认出图", "fixture-old-image")).await.unwrap();
    engine.memory_save(save(1, None, "常用剪片", "fixture-clip-workflow")).await.unwrap();
    engine.memory_save(save(2, None, "账单归档", "再照上次的做法做一版 fixture-unrelated-invoice")).await.unwrap();
    const REUSE: &str = "再照上次的做法做一版。";
    assert!(engine.channel(quoted("workflow-wrong-owner", "different-binding", REUSE, original_turn)).await.is_err());
    let request = quoted("workflow-repeat", owner, REUSE, original_turn);
    let replay = engine.channel(request.clone()).await.unwrap();
    assert_eq!(replay["session_id"], original["session_id"]);
    assert_eq!(engine.channel(request).await.unwrap()["turn_id"], replay["turn_id"]);
    engine.memory_save(save(3, first["id"].as_str().map(str::to_owned), "默认出图", "fixture-current-image")).await.unwrap();

    let plain = engine.channel(submit("workflow-without-reference", REUSE)).await.unwrap();
    let changed = engine.channel(quoted("workflow-new-topic", owner, "帮我按上次的做法剪辑视频。", original_turn)).await.unwrap();
    for (receipt, expected) in [(&replay, Some("fixture-current-image")), (&plain, None), (&changed, Some("fixture-clip-workflow"))] {
        let id = receipt["turn_id"].as_str().unwrap();
        engine.prepare_context(id).await.unwrap();
        let state = engine.state.lock().await;
        let (session, turn) = locate(&state, id).unwrap();
        let turn = &state.sessions[session].turns[turn];
        if let Some(expected) = expected {
            assert!(turn.runtime_prompt.contains(expected), "{expected} missing");
        }
        assert!(!turn.runtime_prompt.contains("fixture-old-image"));
        assert_eq!(turn.runtime_prompt.contains("fixture-current-image"), receipt == &replay);
        assert_eq!(turn.runtime_prompt.contains("fixture-clip-workflow"), receipt == &changed);
        assert!(!turn.runtime_prompt.contains("fixture-unrelated-invoice"));
        if receipt == &replay {
            assert_eq!(turn.protocol[0]["content"], REUSE);
        }
    }
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn wechat_named_attachment_stays_in_the_current_or_explicit_owner_scope() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    let owner = "named-file-owner";
    let resolve = |target: Option<&str>, source: &str| json!({
        "operation":"resolve_named_file", "source_key":source,
        "name":"封面.PNG", "turn_id":target, "file":null
    });
    let mut turns = Vec::new();
    for index in 0..2 {
        let ack = engine.send_from(request("制作封面", &format!("named-file-{index}")), "wechat", owner)
            .await.unwrap();
        engine.change_turn(&ack.turn_id, |session, turn_index| {
            session.turns[turn_index].state = "completed".into();
            let job_id = format!("fixture-job-{index}");
            session.jobs.push(Job {
                id: job_id.clone(), command: "model_capability_call".into(), turn_id: ack.turn_id.clone(),
                state: "succeeded".into(), result: json!({"status":"succeeded"}), owned: true,
                deliver: true, remote: None, archived_result_bytes: None,
                artifacts: vec![Artifact {
                    id: format!("fixture-file-{index}"), job_id, name: "封面.png".into(),
                    path: format!("fixtures/{index}/封面.png"), bytes: 6,
                    sha256: "1".repeat(64), mime_type: "image/png".into(),
                }],
            });
            Ok(())
        }).await.unwrap();
        turns.push(ack.turn_id);
        let revision = engine.state.lock().await.revision;
        let found = engine.channel(resolve(None, owner)).await.unwrap();
        if index == 0 {
            assert_eq!(found["state"], "found");
            assert_eq!(found["target"], turns[0]);
            assert!(found["file"].is_object());
            assert!(!found.to_string().contains("fixtures/"));
        } else {
            assert_eq!(found, json!({"state":"ambiguous"}));
        }
        assert_eq!(engine.state.lock().await.revision, revision);
    }
    let first = engine.channel(resolve(Some(&turns[0]), owner)).await.unwrap();
    let second = engine.channel(resolve(Some(&turns[1]), owner)).await.unwrap();
    assert_ne!(first["file"], second["file"]);
    assert!(engine.channel(resolve(Some(&turns[0]), "another-owner")).await.is_err());
    engine.channel(json!({"operation":"submit","source_key":owner,
        "request_id":"named-file-new-chat","text":"新对话"})).await.unwrap();
    assert_eq!(engine.channel(resolve(None, owner)).await.unwrap(), json!({"state":"missing"}));
    assert_eq!(engine.channel(resolve(Some(&turns[0]), owner)).await.unwrap(), first);
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn wechat_natural_requests_recall_matching_defaults_even_with_different_words() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    engine.mutate(Change::default(), |state| {
        for (title, content, expires_at) in [
            ("常用剪片", "测试工作流 synthetic-edit-v1", None),
            ("默认出图", "测试画幅 synthetic-image-square", None),
            ("临时出图", "synthetic-expired-image", Some((chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339())),
            ("旧剪片设置", "synthetic-expired", Some((chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339())),
        ] {
            let revision = state.personal.revision;
            let saved_at = if expires_at.is_some() { chrono::Utc::now() - chrono::Duration::days(2) } else { chrono::Utc::now() };
            state.personal.save_memory(crate::assistant_personal::MemoryInput {
                revision, id: None, title: title.into(), content: content.into(), expires_at,
            }, saved_at)?;
        }
        Ok(())
    }).await.unwrap();

    for (request_id, text, expected, unrelated) in [
        ("edit-original", "帮我按照之前的方式完成一下剪辑", "synthetic-edit-v1", "synthetic-image-square"),
        ("image-original", "帮我生成一个图片", "synthetic-image-square", "synthetic-edit-v1"),
        ("image-colloquial", "照上次的做法帮我画张图", "synthetic-image-square", "synthetic-edit-v1"),
        ("image-described-cover", "帮我画一张蓝色封面", "synthetic-image-square", "synthetic-edit-v1"),
        ("image-described-photo", "麻烦生成一幅山水黑白插画。", "synthetic-image-square", "synthetic-edit-v1"),
    ] {
        let receipt = engine.channel(json!({"operation":"submit","source_key":"natural-capabilities","request_id":request_id,"text":text})).await.unwrap();
        let turn_id = receipt["turn_id"].as_str().unwrap();
        engine.prepare_context(turn_id).await.unwrap();
        let state = engine.state.lock().await;
        let (s, t) = locate(&state, turn_id).unwrap();
        let turn = &state.sessions[s].turns[t];
        assert_eq!(turn.interaction, "task");
        assert_eq!(turn.protocol[0]["content"], text);
        assert!(turn.runtime_prompt.contains(expected));
        assert!(turn.runtime_prompt.contains("background_not_authority"));
        assert!(turn.runtime_prompt.contains("自然语言办事"));
        assert!(!turn.runtime_prompt.contains(unrelated));
        assert!(!turn.runtime_prompt.contains("synthetic-expired"));
    }
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn conversational_shortcuts_recall_the_workflow_instead_of_unrelated_notes() {
    let (engine, _, _, _, _root) = fixture(false, vec![]).await;
    for (revision, (title, content)) in [
        ("默认出图", "fixture-cover-preference"),
        ("常用剪片", "fixture-clip-workflow"),
        ("账单归档", "fixture-unrelated 按照之前的方式完成一下"),
        ("会议纪要", "fixture-unrelated 按照之前的方式完成一下"),
        ("采购清单", "fixture-unrelated 按照之前的方式完成一下"),
        ("文档整理", "fixture-unrelated 按照之前的方式完成一下"),
    ].into_iter().enumerate() {
        engine.memory_save(crate::assistant_personal::MemoryInput {
            revision: revision as u64,
            id: None,
            title: title.into(),
            content: content.into(),
            expires_at: None,
        }).await.unwrap();
    }
    for (request, expected) in [
        ("帮我给这篇文章配张头图。", "fixture-cover-preference"),
        ("帮我按照之前的方式完成一下剪辑。", "fixture-clip-workflow"),
        ("就照咱们上回的做法剪个片吧。", "fixture-clip-workflow"),
    ] {
        let context = engine.context(None, request).await.unwrap();
        let text = context["memories"].to_string();
        assert!(text.contains(expected), "{request}: {text}");
        assert!(!text.contains("fixture-unrelated"), "{request}: {text}");
    }
    let context = engine.context(None, "账单归档").await.unwrap();
    assert_eq!(context["memories"][0]["title"], "账单归档");
}

#[tokio::test]
async fn workflow_filler_does_not_recall_unrelated_defaults_on_desktop_or_wechat() {
    let (engine, _, _, _, _root) = fixture(false, vec![]).await;
    for (revision, (title, content)) in [
        ("例会纪要", "fixture-meeting-workflow"),
        ("账单归档", "fixture-private-billing 按照之前的方式完成一下"),
        ("出图设置", "fixture-private-image 照咱们上回那套"),
    ].into_iter().enumerate() {
        engine.memory_save(crate::assistant_personal::MemoryInput {
            revision: revision as u64,
            id: None,
            title: title.into(),
            content: content.into(),
            expires_at: None,
        }).await.unwrap();
    }

    for (source, text) in [
        ("desktop", "帮我按照之前的方式完成一下例会纪要。"),
        ("wechat", "就照咱们上回那套整理例会纪要吧。"),
    ] {
        let ack = engine.send_from(request(text, &format!("meeting-{source}")), source, "meeting-owner")
            .await.unwrap();
        engine.prepare_context(&ack.turn_id).await.unwrap();
        let state = engine.state.lock().await;
        let (s, t) = locate(&state, &ack.turn_id).unwrap();
        let turn = &state.sessions[s].turns[t];
        assert_eq!(turn.protocol[0]["content"], text);
        assert!(turn.runtime_prompt.contains("fixture-meeting-workflow"), "{source}");
        assert!(!turn.runtime_prompt.contains("fixture-private-"), "{source}");
    }

    let missing = engine.context(None, "帮我按照之前的方式完成一下产品盘点。")
        .await.unwrap();
    assert_eq!(missing["memories"], json!([]));
    let exact = engine.context(None, "账单归档").await.unwrap();
    assert_eq!(exact["memories"][0]["title"], "账单归档");
}

#[tokio::test]
async fn queued_natural_tasks_use_current_preferences_once_and_ignore_attachment_topics() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    let save = |revision, id, title: &str, content: &str, expires_at| {
        crate::assistant_personal::MemoryInput {
            revision,
            id,
            title: title.into(),
            content: content.into(),
            expires_at,
        }
    };
    let old = engine.memory_save(save(0, None, "默认出图", "fixture-previous-blue", None)).await.unwrap();
    let mut desktop_request = request("帮我画张图", "queued-preference-desktop");
    desktop_request.attachments.push(input::TextAttachment {
        name: "sample.md".into(),
        content: "合成附件，包含完整题目：指定代码仓库规范".into(),
        document: None,
    });
    let desktop = engine.send(desktop_request).await.unwrap();
    let wechat = engine.channel(json!({"operation":"submit","source_key":"fixture-owner","request_id":"queued-preference-wechat","text":"照之前的办法画个封面"})).await.unwrap();
    let wechat_id = wechat["turn_id"].as_str().unwrap();
    // A queued turn received by an older version already has a memory snapshot.
    let old_projection = engine.context(Some(&desktop.session_id), "出图").await.unwrap();
    assert!(old_projection.to_string().contains("fixture-previous-blue"));
    for id in [&*desktop.turn_id, wechat_id] {
        engine.change_turn(id, |session, index| {
            session.queue_paused = true;
            let turn = &mut session.turns[index];
            turn.state = "waiting_input_queue".into();
            assert!(!turn.runtime_prompt.contains("fixture-previous-blue"));
            if id == wechat_id {
                let (head, _) = turn.runtime_prompt.split_once(context::SUBMISSION_CONTEXT).unwrap();
                turn.runtime_prompt = format!("{head}{}{old_projection}\nfixture-later-context", context::SUBMISSION_CONTEXT);
            }
            Ok(())
        }).await.unwrap();
    }
    engine.memory_save(save(1, old["id"].as_str().map(str::to_owned), "默认出图", "fixture-current-green", None)).await.unwrap();
    engine.memory_save(save(2, None, "指定代码仓库规范", "fixture-private-unrelated", None)).await.unwrap();
    engine.mutate(Change::default(), |state| {
        let now = chrono::Utc::now();
        state.personal.save_memory(save(3, None, "临时出图", "fixture-expired", Some((now - chrono::Duration::days(1)).to_rfc3339())), now - chrono::Duration::days(2))?;
        Ok(())
    }).await.unwrap();
    for id in [&*desktop.turn_id, wechat_id] {
        engine.prepare_context(id).await.unwrap();
        let before = engine.state.lock().await.revision;
        engine.prepare_context(id).await.unwrap();
        let state = engine.state.lock().await;
        assert_eq!(state.revision, before);
        let (s, t) = locate(&state, id).unwrap();
        let prompt = &state.sessions[s].turns[t].runtime_prompt;
        assert_eq!(prompt.matches("fixture-current-green").count(), 1);
        assert!(!prompt.contains("fixture-previous-blue"));
        assert!(!prompt.contains("fixture-private-unrelated"));
        assert!(!prompt.contains("fixture-expired"));
        assert_eq!(prompt.contains("fixture-later-context"), id == wechat_id);
    }
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn context_keeps_original_active_work_after_many_discussions_and_queued_messages() {
    let (e, models, effects, _, _root) = fixture(false, vec![]).await;
    let main = e
        .send(request("制作蓝色圆点动画", "context-primary"))
        .await
        .unwrap();
    let mut queued = vec![];
    for i in 0..8 {
        let mut req = request(&format!("后续任务{i}"), &format!("context-queue-{i}"));
        req.session_id = Some(main.session_id.clone());
        queued.push(e.send(req).await.unwrap().turn_id);
    }
    for i in 0..8 {
        let mut req = request(&format!("只讨论{i}"), &format!("context-chat-{i}"));
        req.session_id = Some(main.session_id.clone());
        req.mode = Some("chat".into());
        e.send(req).await.unwrap();
    }
    e.change_turn(&main.turn_id, |s, t| {
        for turn in &mut s.turns {
            if queued.contains(&turn.id) {
                turn.state = "waiting_input_queue".into();
            }
        }
        s.turns[t].state = "awaiting_confirmation".into();
        s.turns[t].requirements_revision = 1;
        s.turns[t].requirements.push(Requirement {
            revision: 1,
            request_id: "context-change".into(),
            text: "这次字幕用两句简短中文".into(),
            received_at: now(),
        });
        Ok(())
    })
    .await
    .unwrap();
    let before = serde_json::to_value(e.state.lock().await.clone()).unwrap();
    let ctx = e.context(Some(&main.session_id), "").await.unwrap();
    assert_eq!(ctx["tasks"][0]["id"], main.turn_id);
    assert_eq!(
        ctx["tasks"][0]["requirements"][0]["text"],
        "这次字幕用两句简短中文"
    );
    assert_eq!(ctx["tasks"][1]["id"], queued[0]);
    assert_eq!(ctx["tasks_total"], 9);
    assert_eq!(ctx["discussion_turns"], 8);
    assert_eq!(ctx["tasks_has_more"], true);
    assert!(ctx["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| !v["progress"].is_null()));
    let typed: ContextProjection = serde_json::from_value(ctx.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(typed).unwrap(),
        ctx,
        "public typed schema must preserve all projected fields"
    );
    assert_eq!(
        serde_json::to_value(e.state.lock().await.clone()).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn context_has_a_serialized_byte_budget_and_never_injects_memory_history_or_workspace_paths()
{
    let (e, models, effects, _, _root) = fixture(false, vec![]).await;
    let first = e
        .send(request("动画偏好", "context-budget-first"))
        .await
        .unwrap();
    for i in 0..6 {
        let mut req = request("动画偏好", &format!("context-budget-{i}"));
        req.session_id = Some(first.session_id.clone());
        e.send(req).await.unwrap();
    }
    let mut state = e.state.lock().await;
    for i in 0..4 {
        let revision = state.personal.revision;
        state
            .personal
            .save_memory(
                crate::assistant_personal::MemoryInput {
                    revision,
                    id: None,
                    title: format!("动画偏好{i}"),
                    content: format!("动画偏好 {}", "\"\\\n🙂".repeat(250)),
                    expires_at: None,
                },
                chrono::Utc::now(),
            )
            .unwrap();
        state.personal.memories.last_mut().unwrap().previous.push(
            crate::assistant_personal::MemoryRevision {
                content: "PRIVATE-OLD-HISTORY-SENTINEL".repeat(1000),
                expires_at: None,
                updated_at: now(),
            },
        );
    }
    let (s, _) = locate(&state, &first.turn_id).unwrap();
    for turn in &mut state.sessions[s].turns {
        turn.text = "\"\\\n🙂".repeat(2000);
        turn.state = "completed".into();
    }
    state.local.workspaces = (0..14)
        .map(|i| crate::assistant_local::WorkspaceGrant {
            id: format!("workspace-{i}"),
            path: format!("C:/HomeBase/Private-WORKSPACE-PATH-{i}"),
            access: "read".into(),
            auto_write: false,
        })
        .collect();
    drop(state);
    let before = serde_json::to_value(e.state.lock().await.clone()).unwrap();
    let ctx = e
        .context(Some(&first.session_id), "动画偏好")
        .await
        .unwrap();
    assert!(
        ctx.to_string().len() <= 9000,
        "serialized context exceeded its own transport budget"
    );
    assert_eq!(ctx["context_has_more"], true);
    assert_eq!(ctx["summary_only"], true);
    let typed: ContextProjection = serde_json::from_value(ctx.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), ctx);
    assert!(!ctx.to_string().contains("PRIVATE-OLD-HISTORY-SENTINEL"));
    assert!(!ctx.to_string().contains("PRIVATE-WORKSPACE-PATH"));
    assert!(!ctx.to_string().contains('\u{fffd}'));
    assert_eq!(
        serde_json::to_value(e.state.lock().await.clone()).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn context_fingerprint_tracks_visible_facts_not_an_unrelated_conversation_or_read_clock() {
    let (e, _, _, _, _root) = fixture(false, vec![]).await;
    let main = e
        .send(request("自己的任务", "context-fp-main"))
        .await
        .unwrap();
    let first = e.context(Some(&main.session_id), "").await.unwrap();
    e.send(request(
        "OTHER-SESSION-PRIVATE-SENTINEL",
        "context-fp-other",
    ))
    .await
    .unwrap();
    let second = e.context(Some(&main.session_id), "").await.unwrap();
    assert_ne!(first["state_revision"], second["state_revision"]);
    assert_eq!(first["fingerprint"], second["fingerprint"]);
    assert!(!second
        .to_string()
        .contains("OTHER-SESSION-PRIVATE-SENTINEL"));
    e.change_turn(&main.turn_id, |s, t| {
        s.turns[t].text = "本会话已有新结果".into();
        Ok(())
    })
    .await
    .unwrap();
    let changed = e.context(Some(&main.session_id), "").await.unwrap();
    assert_ne!(second["fingerprint"], changed["fingerprint"]);
}
