use super::*;

#[tokio::test]
async fn queue_feedback_queue_only_bare_continue_executes_once() {
    queue_feedback_case(false, false, false).await;
}

#[tokio::test]
async fn queue_feedback_lost_response_after_restart_reports_verification_not_zero_work() {
    queue_feedback_case(true, false, false).await;
}

#[tokio::test]
async fn queue_feedback_lost_response_never_claims_or_releases_a_new_paused_queue() {
    queue_feedback_case(true, true, false).await;
}

#[tokio::test]
async fn queue_settings_disabled_model_still_reconciles_an_old_lost_response() {
    queue_feedback_case(true, false, true).await;
}

#[tokio::test]
async fn queue_settings_disabled_model_reconciles_without_releasing_a_later_queue() {
    queue_feedback_case(true, true, true).await;
}

async fn queue_feedback_case(drop_response: bool, later_queue: bool, disable_after: bool) {
    let models = Arc::new(AtomicUsize::new(0));
    let calls = models.clone();
    let model: Model = Arc::new(move |_| {
        let calls = calls.clone();
        Box::pin(async move {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 | 3 => Ok(tool_reply(
                    1,
                    "homebase_memory",
                    json!({
                        "action":"remember","revision":0,"title":"合成确认","content":"不会保存"
                    }),
                )),
                1 => Ok(tool_reply(
                    1,
                    "homebase_files",
                    json!({
                        "action":"write","path":"queue-feedback-once.txt","content":"synthetic"
                    }),
                )),
                2 => Ok(reply(vec![])),
                _ => panic!("An old control released a new queue or repeated an effect"),
            }
        })
    });
    let (base, effects, saved, _root) = setup(model).await;
    let first = send_source(&base, request("仅准备合成确认", "feedback-first"), "wechat").await;
    let pending = task_wait(&base, &first.turn_id, "awaiting_confirmation").await;
    let mut next = request("之后写一份合成文件", "feedback-queued");
    next.session_id = Some(first.session_id.clone());
    next.mode = Some("task".into());
    let queued = send_source(&base, next, "wechat").await;
    assert_eq!(queued.state, "waiting_input_queue");
    base.confirm(&first.turn_id, &pending.pending.unwrap().id, false)
        .await
        .unwrap();
    drained(&base, &first.turn_id).await;
    assert_eq!(base.turn(&first.turn_id).await.unwrap().state, "rejected");

    // Fail the final control-response save, after the queue release is durable.
    // This exercises the actual write-ahead path, rather than editing a receipt.
    let fail = Arc::new(AtomicUsize::new(usize::from(drop_response)));
    let fail_once = fail.clone();
    let durable_save = base.save.clone();
    let save: Save = Arc::new(move |state| {
        let fail_once = fail_once.clone();
        let durable_save = durable_save.clone();
        Box::pin(async move {
            if state
                .channel_controls
                .iter()
                .any(|receipt| receipt.response.is_some())
                && fail_once.swap(0, Ordering::SeqCst) == 1
            {
                return Err("synthetic-control-response-save-lost".into());
            }
            durable_save(state).await
        })
    });
    let engine = Engine::new(
        saved.lock().await.clone(),
        save,
        base.model.clone(),
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let command = json!({"operation":"submit","source_key":"synthetic-queue-owner",
        "request_id":"feedback-continue","text":if drop_response {"继续排队"} else {"继续吧"}});
    let initial = engine.channel(command.clone()).await;
    if drop_response {
        assert!(initial
            .as_ref()
            .unwrap_err()
            .contains("synthetic-control-response-save-lost"));
    } else {
        assert!(initial.as_ref().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("后续 1 条"));
    }
    task_wait(&engine, &queued.turn_id, "completed").await;
    drained(&engine, &queued.turn_id).await;
    assert_eq!(models.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    assert_eq!(saved.lock().await.sessions[0].queue_resume_generation, 1);

    let mut later_id = None;
    if later_queue {
        let mut next = request("新一件合成确认", "feedback-new-first");
        next.session_id = Some(first.session_id.clone());
        let new_first = send_source(&engine, next, "wechat").await;
        let pending = task_wait(&engine, &new_first.turn_id, "awaiting_confirmation").await;
        let mut next = request("这条新要求仍需暂停", "feedback-new-queued");
        next.session_id = Some(first.session_id.clone());
        next.mode = Some("task".into());
        later_id = Some(send_source(&engine, next, "wechat").await.turn_id);
        engine
            .confirm(&new_first.turn_id, &pending.pending.unwrap().id, false)
            .await
            .unwrap();
        drained(&engine, &new_first.turn_id).await;
    }
    if disable_after {
        let mut settings = engine.settings().await;
        settings.enabled = false;
        engine.save_settings(settings).await.unwrap();
    }
    let state = saved.lock().await.clone();
    assert_eq!(state.channel_controls[0].response.is_none(), drop_response);
    let before = serde_json::to_value(&state.sessions[0]).unwrap();
    let restored = Engine::new(
        state,
        engine.save.clone(),
        engine.model.clone(),
        engine.control.clone(),
        engine.workspace.clone(),
    )
    .await
    .unwrap();
    if disable_after {
        let before_state = serde_json::to_value(&*saved.lock().await).unwrap();
        assert_eq!(
            restored
                .resume_queue_once(&first.session_id, Some(0))
                .await
                .unwrap()["resumed"],
            false
        );
        assert_eq!(
            serde_json::to_value(&*saved.lock().await).unwrap(),
            before_state
        );
        assert_eq!(
            serde_json::to_value(&*restored.state.lock().await).unwrap(),
            before_state
        );
    }
    let response = restored.channel(command.clone()).await.unwrap();
    if drop_response {
        let text = response["text"].as_str().unwrap();
        assert!(
            text.contains("没有再次启动"),
            "A replay must report verification: {text}"
        );
        assert!(
            !text.contains("已继续处理"),
            "A replay is not a new release: {text}"
        );
    } else {
        assert_eq!(response, initial.unwrap());
    }
    assert_eq!(restored.channel(command).await.unwrap(), response);
    assert_eq!(
        serde_json::to_value(&saved.lock().await.sessions[0]).unwrap(),
        before
    );
    if let Some(turn_id) = later_id {
        assert_eq!(
            restored.turn(&turn_id).await.unwrap().state,
            "waiting_input_queue"
        );
        assert!(saved.lock().await.sessions[0].queue_paused);
    }
    assert_eq!(
        models.load(Ordering::SeqCst),
        if later_queue { 4 } else { 3 }
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

async fn paused_settings_queue() -> (
    Arc<Engine>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Arc<Mutex<State>>,
    Ack,
    tempfile::TempDir,
) {
    let models = Arc::new(AtomicUsize::new(0));
    let calls = models.clone();
    let model: Model = Arc::new(move |_| {
        let calls = calls.clone();
        Box::pin(async move {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(tool_reply(
                    1,
                    "homebase_memory",
                    json!({"action":"remember","revision":0,"title":"合成开关确认","content":"不会保存"}),
                )),
                1 => Ok(tool_reply(
                    1,
                    "homebase_files",
                    json!({"action":"write","path":"queue-settings-once.txt","content":"synthetic"}),
                )),
                2 => Ok(reply(vec![])),
                _ => panic!("A disabled or replayed queue executed unexpectedly"),
            }
        })
    });
    let (engine, effects, saved, root) = setup(model).await;
    let first = send_source(&engine, request("准备合成确认", "settings-first"), "wechat").await;
    let pending = task_wait(&engine, &first.turn_id, "awaiting_confirmation").await;
    let mut next = request("之后写一份合成文件", "settings-queued");
    next.session_id = Some(first.session_id.clone());
    next.mode = Some("task".into());
    let queued = send_source(&engine, next, "wechat").await;
    assert_eq!(queued.state, "waiting_input_queue");
    engine
        .confirm(&first.turn_id, &pending.pending.unwrap().id, false)
        .await
        .unwrap();
    drained(&engine, &first.turn_id).await;
    assert!(saved.lock().await.sessions[0].queue_paused);
    (engine, models, effects, saved, queued, root)
}

#[tokio::test]
async fn queue_resume_old_snapshot_leaves_changed_waiting_messages_paused() {
    let (engine, models, effects, saved, queued, _root) = paused_settings_queue().await;
    let session_id = queued.session_id.clone();
    let earlier = engine.session(&session_id).await.unwrap().queue_resume_generation.unwrap();
    let mut next = request("再放入一条合成消息", "settings-later-queued");
    next.session_id = Some(session_id.clone());
    next.mode = Some("task".into());
    let added = send_source(&engine, next, "wechat").await;
    assert_eq!(added.state, "waiting_input_queue");
    let request_window = || window::TimelineRequest { session_id: session_id.clone(), before_message_id: None, limit: 1 };
    let changed = engine.session_window(request_window()).await.unwrap().queue_resume_generation.unwrap();
    assert_ne!(earlier, changed);
    let before = serde_json::to_value(&*saved.lock().await).unwrap();
    assert_eq!(engine.resume_queue_once(&session_id, Some(earlier)).await.unwrap(), json!({"queued":2,"resumed":false}));
    assert_eq!(before, serde_json::to_value(&*saved.lock().await).unwrap());
    engine.cancel(&added.turn_id).await.unwrap();
    let current = engine.session_window(request_window()).await.unwrap().queue_resume_generation.unwrap();
    assert_ne!(changed, current);
    assert_eq!(engine.resume_queue_once(&session_id, Some(changed)).await.unwrap(), json!({"queued":1,"resumed":false}));
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(engine.resume_queue_once(&session_id, Some(current)).await.unwrap(), json!({"queued":1,"resumed":true}));
    task_wait(&engine, &queued.turn_id, "completed").await;
    drained(&engine, &queued.turn_id).await;
    assert_eq!(engine.session_window(request_window()).await.unwrap().queue_resume_generation, None);
    assert_eq!(engine.turn(&added.turn_id).await.unwrap().state,"canceled");
    assert_eq!(models.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn queue_settings_disabled_new_release_preserves_state_and_does_not_call_model() {
    let (engine, models, effects, saved, queued, _root) = paused_settings_queue().await;
    let mut settings = engine.settings().await;
    settings.enabled = false;
    engine.save_settings(settings).await.unwrap();
    let before = serde_json::to_value(&*saved.lock().await).unwrap();
    for expected in [None, Some(0)] {
        assert!(engine
            .resume_queue_once(&queued.session_id, expected)
            .await
            .unwrap_err()
            .contains("模型已停用"));
    }
    assert_eq!(serde_json::to_value(&*saved.lock().await).unwrap(), before);
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

async fn poll_queue_future<F: std::future::Future>(
    mut future: std::pin::Pin<&mut F>,
) -> std::task::Poll<F::Output> {
    std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx))).await
}

#[tokio::test]
async fn queue_settings_disable_and_release_use_the_same_locked_state() {
    use std::task::Poll;
    let (engine, models, effects, saved, queued, _root) = paused_settings_queue().await;
    // Hold the dispatcher so the test observes only the competing state transactions.
    let _dispatch = engine.send_gate.lock().await;
    let mut disabled = engine.settings().await;
    disabled.enabled = false;
    let initial = engine.state.lock().await;
    let mut resume = std::pin::pin!(engine.resume_queue(&queued.session_id));
    assert!(poll_queue_future(resume.as_mut()).await.is_pending());
    let mut barrier = std::pin::pin!(engine.state.lock());
    assert!(poll_queue_future(barrier.as_mut()).await.is_pending());
    drop(initial);
    // FIFO mutex order: hydration, barrier, release decision, then disabling.
    assert!(poll_queue_future(resume.as_mut()).await.is_pending());
    let Poll::Ready(barrier) = poll_queue_future(barrier.as_mut()).await else {
        panic!("The queued state barrier must own its lock");
    };
    let mut disable = std::pin::pin!(engine.save_settings(disabled));
    assert!(poll_queue_future(disable.as_mut()).await.is_pending());
    drop(barrier);
    let release = poll_queue_future(resume.as_mut()).await;
    disable.await.unwrap();
    match release {
        Poll::Ready(result) => {
            // Release committed before disabling, so it may be accepted.
            assert_eq!(result.unwrap()["resumed"], true);
            assert!(!saved.lock().await.sessions[0].queue_paused);
        }
        Poll::Pending => {
            // Disabling committed first; a stale pre-read must not release afterward.
            assert!(resume.await.unwrap_err().contains("模型已停用"));
            assert!(saved.lock().await.sessions[0].queue_paused);
            assert_eq!(saved.lock().await.sessions[0].queue_resume_generation, 0);
        }
    }
    assert!(!saved.lock().await.settings.enabled);
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn queue_settings_failed_release_save_recovers_the_same_control_once_after_restart() {
    let (base, models, effects, saved, queued, _root) = paused_settings_queue().await;
    let fail = Arc::new(AtomicUsize::new(1));
    let fail_once = fail.clone();
    let durable_save = base.save.clone();
    let save: Save = Arc::new(move |state| {
        let fail_once = fail_once.clone();
        let durable_save = durable_save.clone();
        Box::pin(async move {
            if state
                .sessions
                .iter()
                .any(|s| s.queue_resume_generation == 1)
                && fail_once.swap(0, Ordering::SeqCst) == 1
            {
                return Err("synthetic-queue-release-save-failed".into());
            }
            durable_save(state).await
        })
    });
    let engine = Engine::new(
        saved.lock().await.clone(),
        save,
        base.model.clone(),
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let command = json!({"operation":"submit","source_key":"synthetic-queue-owner",
        "request_id":"settings-save-retry","text":"继续排队"});
    assert!(engine
        .channel(command.clone())
        .await
        .unwrap_err()
        .contains("synthetic-queue-release-save-failed"));
    let state = saved.lock().await.clone();
    assert!(state.sessions[0].queue_paused);
    assert_eq!(state.sessions[0].queue_resume_generation, 0);
    assert!(state.channel_controls.last().unwrap().response.is_none());
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let restored = Engine::new(
        state,
        engine.save.clone(),
        engine.model.clone(),
        engine.control.clone(),
        engine.workspace.clone(),
    )
    .await
    .unwrap();
    let response = restored.channel(command.clone()).await.unwrap();
    task_wait(&restored, &queued.turn_id, "completed").await;
    drained(&restored, &queued.turn_id).await;
    assert_eq!(restored.channel(command).await.unwrap(), response);
    assert_eq!(saved.lock().await.sessions[0].queue_resume_generation, 1);
    assert_eq!(models.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn queue_dispatch_stale_disabled_candidate_keeps_state() {
    stale_dispatch_candidate("disabled").await;
}

#[tokio::test]
async fn queue_dispatch_stale_canceled_candidate_keeps_state() {
    stale_dispatch_candidate("canceled").await;
}

#[tokio::test]
async fn queue_dispatch_stale_paused_candidate_keeps_state() {
    stale_dispatch_candidate("paused").await;
}

async fn stale_dispatch_candidate(condition: &str) {
    let (engine, models, effects, saved, queued, _root) = paused_settings_queue().await;
    let _dispatch = engine.send_gate.lock().await;
    if condition != "paused" {
        engine.resume_queue(&queued.session_id).await.unwrap();
        if condition == "disabled" {
            let mut settings = engine.settings().await;
            settings.enabled = false;
            engine.save_settings(settings).await.unwrap();
        } else {
            engine.cancel(&queued.turn_id).await.unwrap();
        }
    }
    let before = serde_json::to_value(&*saved.lock().await).unwrap();
    assert!(
        !engine
            .claim_queued_turn(&queued.turn_id, &[])
            .await
            .unwrap(),
        "An earlier candidate is no longer eligible: {condition}"
    );
    assert_eq!(serde_json::to_value(&*saved.lock().await).unwrap(), before);
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn queue_dispatch_stale_candidate_does_not_overtake_a_live_discussion() {
    let (base, _, effects, saved, queued, _root) = paused_settings_queue().await;
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (arrived, released, count) = (reached.clone(), release.clone(), calls.clone());
    let model: Model = Arc::new(move |_| {
        let (arrived, released, count) = (arrived.clone(), released.clone(), count.clone());
        Box::pin(async move {
            assert_eq!(count.fetch_add(1, Ordering::SeqCst), 0);
            arrived.notify_one();
            released.notified().await;
            Ok(reply(vec![]))
        })
    });
    let engine = Engine::new(
        saved.lock().await.clone(),
        base.save.clone(),
        model,
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let mut discussion = request("只讨论合成要求", "dispatch-live-discussion");
    discussion.session_id = Some(queued.session_id.clone());
    discussion.mode = Some("chat".into());
    let chat = send_source(&engine, discussion, "wechat").await;
    tokio::time::timeout(Duration::from_secs(10), reached.notified())
        .await
        .unwrap();
    let _dispatch = engine.send_gate.lock().await;
    engine.resume_queue(&queued.session_id).await.unwrap();
    let before = serde_json::to_value(&*saved.lock().await).unwrap();
    assert!(!engine
        .claim_queued_turn(&queued.turn_id, &[])
        .await
        .unwrap());
    assert_eq!(serde_json::to_value(&*saved.lock().await).unwrap(), before);
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    let mut settings = engine.settings().await;
    settings.enabled = false;
    engine.save_settings(settings).await.unwrap();
    release.notify_one();
    drained(&engine, &chat.turn_id).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn queue_dispatch_reenable_wakes_unpaused_work_with_original_route_and_receipt() {
    let (engine, models, effects, saved, queued, _root) = paused_settings_queue().await;
    let original_route = saved.lock().await.sessions[0]
        .turns
        .last()
        .unwrap()
        .settings
        .route_id
        .clone();
    let dispatch = engine.send_gate.lock().await;
    engine.resume_queue(&queued.session_id).await.unwrap();
    let mut settings = engine.settings().await;
    settings.enabled = false;
    engine.save_settings(settings).await.unwrap();
    // Observe the real dispatcher owning its gate while blocked on state. It
    // must consume the old wake while disabled before enabling is tested.
    let state = engine.state.lock().await;
    drop(dispatch);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match engine.send_gate.try_lock() {
                Ok(gate) => drop(gate),
                Err(_) => break,
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(state);
    drop(engine.send_gate.lock().await);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert!(!saved.lock().await.sessions[0].queue_paused);
    assert_eq!(
        engine.turn(&queued.turn_id).await.unwrap().state,
        "waiting_input_queue"
    );
    let mut settings = engine.settings().await;
    settings.enabled = true;
    settings.route_id = "synthetic-later-route".into();
    engine.save_settings(settings).await.unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            // Wakeup is the claim out of waiting_input_queue, not completion of
            // an actual Pi subprocess under unrelated build load.
            if engine.turn(&queued.turn_id).await.unwrap().state != "waiting_input_queue" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Re-enabling must wake work that was already released, without a new message");
    task_wait(&engine, &queued.turn_id, "completed").await;
    drained(&engine, &queued.turn_id).await;
    let state = saved.lock().await;
    assert_eq!(
        state.sessions[0].turns.last().unwrap().settings.route_id,
        original_route
    );
    assert_eq!(state.settings.route_id, "synthetic-later-route");
    assert_eq!(state.sessions[0].queue_resume_generation, 1);
    drop(state);
    let mut duplicate = request("之后写一份合成文件", "settings-queued");
    duplicate.session_id = Some(queued.session_id.clone());
    duplicate.mode = Some("task".into());
    assert_eq!(
        send_source(&engine, duplicate, "wechat").await.turn_id,
        queued.turn_id
    );
    assert_eq!(models.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn queue_dispatch_reenable_keeps_explicit_and_restart_pauses_until_wechat_continue() {
    for restart in [false, true] {
        let (base, models, effects, saved, queued, _root) = paused_settings_queue().await;
        let engine = if restart {
            Engine::new(
                saved.lock().await.clone(),
                base.save.clone(),
                base.model.clone(),
                base.control.clone(),
                base.workspace.clone(),
            )
            .await
            .unwrap()
        } else {
            base
        };
        let mut settings = engine.settings().await;
        settings.enabled = false;
        engine.save_settings(settings).await.unwrap();
        let mut settings = engine.settings().await;
        settings.enabled = true;
        engine.save_settings(settings).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(saved.lock().await.sessions[0].queue_paused);
        assert_eq!(
            engine.turn(&queued.turn_id).await.unwrap().state,
            "waiting_input_queue"
        );
        assert_eq!(models.load(Ordering::SeqCst), 1);
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        let command = json!({"operation":"submit","source_key":"synthetic-queue-owner",
            "request_id":"dispatch-explicit-continue","text":"继续排队"});
        let receipt = engine.channel(command.clone()).await.unwrap();
        task_wait(&engine, &queued.turn_id, "completed").await;
        drained(&engine, &queued.turn_id).await;
        assert_eq!(engine.channel(command).await.unwrap(), receipt);
        assert_eq!(models.load(Ordering::SeqCst), 3);
        assert_eq!(effects.load(Ordering::SeqCst), 1);
    }
}

async fn send_source(engine: &Arc<Engine>, request: SendRequest, source: &str) -> Ack {
    if source == "desktop" {
        engine.send(request).await.unwrap()
    } else {
        engine
            .send_from(request, "wechat", "synthetic-queue-owner")
            .await
            .unwrap()
    }
}

async fn drained(engine: &Engine, turn_id: &str) {
    for _ in 0..100 {
        if !engine.running.lock().await.contains_key(turn_id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("Owned test worker did not finish");
}

#[tokio::test]
async fn queue_resume_desktop_original_then_explicit_queue_after_restart_execute_once() {
    original_then_queue("desktop").await;
}

#[tokio::test]
async fn queue_resume_wechat_bare_continue_preserves_queue_and_original_receipt() {
    original_then_queue("wechat").await;
}

async fn original_then_queue(source: &str) {
    let count = Arc::new(AtomicUsize::new(0));
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (calls, arrived, released) = (count.clone(), reached.clone(), release.clone());
    let model: Model = Arc::new(move |_| {
        let (calls, arrived, released) = (calls.clone(), arrived.clone(), released.clone());
        Box::pin(async move {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(tool_reply(
                    1,
                    "homebase_files",
                    json!({"action":"write","path":"queue-original-once.txt","content":"synthetic"}),
                )),
                1 => {
                    arrived.notify_one();
                    released.notified().await;
                    Err(Failure::http(503).encode())
                }
                2 | 3 => Ok(reply(vec![])),
                _ => panic!("An original effect or queued input was replayed"),
            }
        })
    });
    let (engine, effects, saved, _root) = setup(model).await;
    let mut first = request("保存合成文件，只执行一次", "queue-resume-first");
    first.mode = Some("task".into());
    let original = send_source(&engine, first, source).await;
    tokio::time::timeout(Duration::from_secs(30), reached.notified())
        .await
        .unwrap();
    let mut next = request("原任务之后再处理这条独立要求", "queue-resume-next");
    next.session_id = Some(original.session_id.clone());
    next.mode = Some("task".into());
    let queued = send_source(&engine, next.clone(), source).await;
    assert_eq!(queued.state, "waiting_input_queue");
    assert_eq!(
        send_source(&engine, next, source).await.turn_id,
        queued.turn_id
    );
    release.notify_one();
    let stopped = task_wait(&engine, &original.turn_id, "failed").await;
    drained(&engine, &original.turn_id).await;
    assert!(stopped.queue_paused);
    let restored = Engine::new(
        saved.lock().await.clone(),
        engine.save.clone(),
        engine.model.clone(),
        engine.control.clone(),
        engine.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(
        restored.turn(&original.turn_id).await.unwrap().resumable,
        "An unstarted queue must not block continuation of the original checkpoint"
    );
    let queued_before =
        serde_json::to_value(restored.turn(&queued.turn_id).await.unwrap()).unwrap();
    let control = json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"queue-original-continue","text":"接着做"});
    let receipt = if source == "wechat" {
        let value = restored.channel(control.clone()).await.unwrap();
        assert_eq!(
            value["turn_id"], original.turn_id,
            "Bare continuation must target the paused task, not unlock its queue"
        );
        Some(value)
    } else {
        assert_eq!(
            restored.resume(&original.turn_id).await.unwrap().turn_id,
            original.turn_id
        );
        None
    };
    let done = task_wait(&restored, &original.turn_id, "completed").await;
    drained(&restored, &original.turn_id).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(done.jobs[0].id, stopped.jobs[0].id);
    assert_eq!(done.steps[0].id, stopped.steps[0].id);
    assert_eq!(
        serde_json::to_value(restored.turn(&queued.turn_id).await.unwrap()).unwrap(),
        queued_before
    );
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
    if let Some(value) = receipt {
        assert_eq!(restored.channel(control).await.unwrap(), value);
        let queue_control = json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"queue-explicit-release","text":"继续排队"});
        let accepted = restored.channel(queue_control.clone()).await.unwrap();
        task_wait(&restored, &queued.turn_id, "completed").await;
        assert_eq!(restored.channel(queue_control).await.unwrap(), accepted);
    } else {
        restored.resume_queue(&original.session_id).await.unwrap();
        task_wait(&restored, &queued.turn_id, "completed").await;
    }
    drained(&restored, &queued.turn_id).await;
    assert_eq!(count.load(Ordering::SeqCst), 4);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn queue_resume_running_worker_or_full_global_slots_never_changes_original_state() {
    for own_worker in [true, false] {
        let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
        let original = engine
            .send(request("合成暂停任务", "queue-slot-original"))
            .await
            .unwrap();
        engine
            .mutate(Change::default(), |state| {
                state.settings.enabled = true;
                let (s, t) = locate(state, &original.turn_id)?;
                let turn = &mut state.sessions[s].turns[t];
                turn.state = "failed".into();
                turn.settings.runtime = "pi".into();
                turn.runtime_version = RUNTIME_VERSION.into();
                turn.runtime_operations = vec![json!({"synthetic_guard_only":true})].into();
                Ok(())
            })
            .await
            .unwrap();
        for n in 0..if own_worker { 1 } else { 4 } {
            let (cancel, _) = watch::channel(false);
            engine.running.lock().await.insert(
                if own_worker {
                    original.turn_id.clone()
                } else {
                    format!("other-worker-{n}")
                },
                Running {
                    cancel,
                    approval: Arc::new(Notify::new()),
                },
            );
        }
        let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
        assert!(
            engine.resume(&original.turn_id).await.is_err(),
            "Never claim running while schedule would have no worker or no slot"
        );
        assert_eq!(
            serde_json::to_value(&*engine.state.lock().await).unwrap(),
            before
        );
        assert_eq!(models.load(Ordering::SeqCst), 0);
        assert_eq!(effects.load(Ordering::SeqCst), 0);
    }
}

async fn stopped_with_queue() -> (
    Arc<Engine>,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    Ack,
    String,
    tempfile::TempDir,
) {
    let (engine, models, effects, _, root) = fixture(false, vec![]).await;
    let original = engine
        .send_from(
            request("合成暂停事项", "bare-original"),
            "wechat",
            "synthetic-queue-owner",
        )
        .await
        .unwrap();
    let queue_id = id();
    engine
        .mutate(Change::default(), |state| {
            state.settings.enabled = true;
            let (s, t) = locate(state, &original.turn_id)?;
            let session = &mut state.sessions[s];
            let turn = &mut session.turns[t];
            turn.state = "failed".into();
            turn.settings.runtime = "pi".into();
            turn.runtime_version = RUNTIME_VERSION.into();
            turn.runtime_operations = vec![json!({"synthetic_selection_only":true})].into();
            let mut queued = turn.clone();
            queued.id = queue_id.clone();
            queued.request_id = "bare-queued".into();
            queued.state = "waiting_input_queue".into();
            queued.runtime_operations = Records::default();
            session.turns.push(queued);
            session.queue_paused = true;
            Ok(())
        })
        .await
        .unwrap();
    (engine, models, effects, original, queue_id, root)
}

#[tokio::test]
async fn queue_resume_unknown_original_never_falls_through_to_releasing_the_queue() {
    let (engine, models, effects, original, queue_id, _root) = stopped_with_queue().await;
    engine
        .change_turn(&original.turn_id, |session, index| {
            session.turns[index].runtime_uncertain_model = true;
            Ok(())
        })
        .await
        .unwrap();
    let queue_before = serde_json::to_value(engine.turn(&queue_id).await.unwrap()).unwrap();
    let error = engine.channel(json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"unknown-continue","text":"继续"})).await.err().unwrap();
    assert!(error.contains("未知"));
    assert_eq!(
        serde_json::to_value(engine.turn(&queue_id).await.unwrap()).unwrap(),
        queue_before
    );
    let state = engine.state.lock().await;
    let receipt = serde_json::to_value(state.channel_controls.last().unwrap()).unwrap();
    assert_eq!(receipt["action"], "继续");
    assert_eq!(receipt["turn_id"], original.turn_id);
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn queue_resume_ambiguous_or_new_dialogue_bare_continue_does_not_guess_an_old_target() {
    let (engine, models, effects, original, _, _root) = stopped_with_queue().await;
    engine
        .change_turn(&original.turn_id, |session, index| {
            let mut other = session.turns[index].clone();
            other.id = id();
            other.request_id = "another-stopped".into();
            session.turns.push(other);
            Ok(())
        })
        .await
        .unwrap();
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    assert!(engine.channel(json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"ambiguous-continue","text":"继续"})).await.err().unwrap().contains("多件暂停"));
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    engine.channel(json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"open-new-dialogue","text":"新对话"})).await.unwrap();
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    assert!(engine.channel(json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"new-dialogue-continue","text":"继续"})).await.is_err());
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn queue_resume_old_stored_control_receipt_keeps_its_original_action_without_reselection() {
    let (engine, models, effects, original, _, _root) = stopped_with_queue().await;
    let source = "synthetic-queue-owner";
    let request_id = "old-continue-receipt";
    let response = json!({"action":"message","text":"旧版本已受理的队列回执"});
    let receipt: channel_control::ControlReceipt = serde_json::from_value(json!({
        "key":format!("{:x}",Sha256::digest(serde_json::to_vec(&(source,request_id)).unwrap())),
        "fingerprint":format!("{:x}",Sha256::digest("继续".as_bytes())),
        "action":"继续排队","session_id":original.session_id,"turn_id":"","confirmation":"","generation":0,"response":response
    })).unwrap();
    engine
        .mutate(Change::default(), |state| {
            state.channel_controls.push(receipt);
            Ok(())
        })
        .await
        .unwrap();
    let before = serde_json::to_value(&*engine.state.lock().await).unwrap();
    assert_eq!(engine.channel(json!({"operation":"submit","source_key":source,"request_id":request_id,"text":"继续"})).await.unwrap(), response);
    assert_eq!(
        serde_json::to_value(&*engine.state.lock().await).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn queue_resume_explicit_old_turn_unlocks_only_its_own_session_queue() {
    let (engine, models, effects, _, _root) = fixture(
        true,
        vec![
            reply(vec![(
                "homebase_memory",
                json!({"action":"remember","revision":0,"title":"旧对话","content":"synthetic"}),
            )]),
            reply(vec![(
                "homebase_memory",
                json!({"action":"remember","revision":0,"title":"新对话","content":"synthetic"}),
            )]),
            reply(vec![]),
        ],
    )
    .await;
    let old = engine
        .send_from(
            request("合成旧事项", "old-session-task"),
            "wechat",
            "synthetic-queue-owner",
        )
        .await
        .unwrap();
    let pending = task_wait(&engine, &old.turn_id, "awaiting_confirmation").await;
    let mut next = request("旧会话的后续事项", "old-session-next");
    next.mode = Some("task".into());
    next.session_id = Some(old.session_id.clone());
    let old_queue = engine
        .send_from(next, "wechat", "synthetic-queue-owner")
        .await
        .unwrap();
    engine
        .confirm(&old.turn_id, &pending.pending.unwrap().id, false)
        .await
        .unwrap();
    drained(&engine, &old.turn_id).await;
    let new = engine.channel(json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"cross-new-dialogue","text":"新对话"})).await.unwrap();
    let mut first = request("合成新事项", "new-session-task");
    first.session_id = Some(new["session_id"].as_str().unwrap().into());
    let current = engine
        .send_from(first, "wechat", "synthetic-queue-owner")
        .await
        .unwrap();
    let pending = task_wait(&engine, &current.turn_id, "awaiting_confirmation").await;
    let mut next = request("新会话的后续事项", "new-session-next");
    next.mode = Some("task".into());
    next.session_id = Some(current.session_id.clone());
    let current_queue = engine
        .send_from(next, "wechat", "synthetic-queue-owner")
        .await
        .unwrap();
    engine
        .confirm(&current.turn_id, &pending.pending.unwrap().id, false)
        .await
        .unwrap();
    drained(&engine, &current.turn_id).await;
    let before = serde_json::to_value(engine.turn(&current_queue.turn_id).await.unwrap()).unwrap();
    let command = json!({"operation":"submit","source_key":"synthetic-queue-owner","request_id":"old-queue-release","text":format!("继续排队 {}",old.turn_id)});
    let receipt = engine.channel(command.clone()).await.unwrap();
    task_wait(&engine, &old_queue.turn_id, "completed").await;
    drained(&engine, &old_queue.turn_id).await;
    assert_eq!(engine.channel(command).await.unwrap(), receipt);
    assert_eq!(
        serde_json::to_value(engine.turn(&current_queue.turn_id).await.unwrap()).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 3);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}
