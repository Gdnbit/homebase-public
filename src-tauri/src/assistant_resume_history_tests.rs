use super::*;

#[tokio::test]
async fn resume_history_original_task_after_later_chat_and_restart_preserves_the_later_group() {
    original_after_later_chat("desktop").await;
}

#[tokio::test]
async fn resume_history_wechat_repeated_continue_keeps_original_task_and_later_chat() {
    original_after_later_chat("wechat").await;
}

async fn original_after_later_chat(source: &str) {
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let model: Model = Arc::new(move |_| {
        let calls = calls.clone();
        Box::pin(async move {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(tool_reply(
                    1,
                    "homebase_files",
                    json!({"action":"write","path":"history-once.txt","content":"synthetic"}),
                )),
                1 => Err(Failure::http(503).encode()),
                2 => {
                    let mut answer = reply(vec![]);
                    answer.text = "后续聊天的独立回复🙂".into();
                    answer.message = json!({"role":"assistant","content":answer.text});
                    Ok(answer)
                }
                3 => Ok(reply(vec![])),
                _ => panic!("Original operation or unrelated chat was replayed"),
            }
        })
    });
    let (engine, effects, saved, _root) = setup(model).await;
    let original = engine
        .send_from(
            request("保存一份合成文件，只执行一次", "history-original"),
            source,
            "synthetic-history-owner",
        )
        .await
        .unwrap();
    let stopped = task_wait(&engine, &original.turn_id, "failed").await;
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    let mut chat = request("先聊一个无关的小问题", "history-later-chat");
    chat.session_id = Some(original.session_id.clone());
    chat.mode = Some("chat".into());
    let later = engine
        .send_from(chat, source, "synthetic-history-owner")
        .await
        .unwrap();
    let later_view = task_wait(&engine, &later.turn_id, "completed").await;
    let state = saved.lock().await.clone();
    let later_group = state.sessions[0].history.last().unwrap().clone();
    assert_eq!(
        later_group.last().unwrap()["content"],
        "后续聊天的独立回复🙂"
    );
    let restored = Engine::new(
        state,
        engine.save.clone(),
        engine.model.clone(),
        engine.control.clone(),
        engine.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(
        restored.turn(&original.turn_id).await.unwrap().resumable,
        "A completed later chat must not permanently block the original task"
    );
    let control = json!({"operation":"submit","source_key":"synthetic-history-owner","request_id":"original-history-continue","text":format!("继续 {}",original.turn_id)});
    let channel_receipt = if source == "wechat" {
        let receipt = restored.channel(control.clone()).await.unwrap();
        assert_eq!(receipt["turn_id"], original.turn_id);
        Some(receipt)
    } else {
        assert_eq!(
            restored.resume(&original.turn_id).await.unwrap().turn_id,
            original.turn_id
        );
        None
    };
    let done = task_wait(&restored, &original.turn_id, "completed").await;
    assert_eq!(done.jobs[0].id, stopped.jobs[0].id);
    assert_eq!(done.steps[0].id, stopped.steps[0].id);
    assert_eq!(
        serde_json::to_value(restored.turn(&later.turn_id).await.unwrap()).unwrap(),
        serde_json::to_value(later_view).unwrap()
    );
    if let Some(receipt) = channel_receipt {
        assert_eq!(
            restored.channel(control).await.unwrap(),
            receipt,
            "A replayed WeChat message never grants another continuation"
        );
    }
    let state = restored.state.lock().await;
    let session = &state.sessions[0];
    assert_eq!(session.history.len(), 2);
    assert_eq!(
        session.history[0], later_group,
        "Never pop the unrelated latest group"
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(count.load(Ordering::SeqCst), 4);
}

async fn legacy_history_fixture() -> (
    Arc<Engine>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Ack,
    tempfile::TempDir,
) {
    let (engine, models, effects, _, root) = fixture(false, vec![]).await;
    let ack = engine
        .send(request("旧格式合成历史", "history-legacy"))
        .await
        .unwrap();
    engine
        .mutate(
            Change {
                session_id: Some(ack.session_id.clone()),
                ..Change::default()
            },
            |state| {
                state.settings.enabled = true;
                let (s, t) = locate(state, &ack.turn_id)?;
                let session = &mut state.sessions[s];
                let turn = &mut session.turns[t];
                turn.state = "failed".into();
                turn.settings.runtime = "pi".into();
                turn.runtime_version = RUNTIME_VERSION.into();
                turn.runtime_operations = vec![json!({"synthetic_check_only":true})].into();
                turn.text = "合成拒绝结果".into();
                turn.history_committed = true;
                session.history = vec![vec![
                    turn.protocol[0].clone(),
                    json!({"role":"assistant","content":turn.text}),
                ]];
                Ok(())
            },
        )
        .await
        .unwrap();
    (engine, models, effects, ack, root)
}

#[tokio::test]
async fn resume_history_missing_duplicate_or_shared_legacy_group_fails_before_state_change() {
    for case in ["missing", "duplicate", "shared"] {
        let (engine, models, effects, ack, _root) = legacy_history_fixture().await;
        engine
            .change_turn(&ack.turn_id, |session, index| {
                match case {
                    "missing" => session.history.clear(),
                    "duplicate" => session.history.push(session.history[0].clone()),
                    _ => {
                        let mut other = session.turns[index].clone();
                        other.id = id();
                        other.request_id = "same-content-other-task".into();
                        session.turns.push(other);
                    }
                }
                Ok(())
            })
            .await
            .unwrap();
        let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
        let view = engine.turn(&ack.turn_id).await.unwrap();
        assert!(!view.resumable, "{case}");
        assert!(view.progress.next.unwrap().contains("历史归属"));
        assert!(engine
            .resume(&ack.turn_id)
            .await
            .err()
            .unwrap()
            .contains("历史归属"));
        assert_eq!(
            serde_json::to_value(&*engine.state.lock().await).unwrap(),
            before,
            "{case}"
        );
        assert_eq!(models.load(Ordering::SeqCst), 0);
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn resume_history_failed_persistence_does_not_remove_any_group_or_schedule_work() {
    let (engine, models, effects, ack, _root) = legacy_history_fixture().await;
    let state = engine.state.lock().await.clone();
    let before = serde_json::to_value(&state).unwrap();
    let save: Save =
        Arc::new(|_| Box::pin(async { Err("synthetic-persistence-unavailable".into()) }));
    let restored = Engine::new(
        state,
        save,
        engine.model.clone(),
        engine.control.clone(),
        engine.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(restored.turn(&ack.turn_id).await.unwrap().resumable);
    assert!(restored.resume(&ack.turn_id).await.is_err());
    assert_eq!(
        serde_json::to_value(&*restored.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
