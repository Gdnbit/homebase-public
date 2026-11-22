//! Every model, channel and persistence target in these tests is synthetic.
use super::*;
use crate::assistant_personal::MemoryInput;

fn input(revision: u64, title: &str, content: &str) -> MemoryInput {
    MemoryInput {
        revision,
        id: None,
        title: title.into(),
        content: content.into(),
        expires_at: None,
    }
}
fn scheduled(revision: u64) -> FollowupInput {
    FollowupInput {
        trigger:None,notification_policy:None,
        matter_id: None,
        revision,
        title: "隔离主动检查".into(),
        instruction: "读取隔离目录并报告实际结果，不修改既有文件".into(),
        due_at: (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339(),
        interval_minutes: None,
        max_runs: 1,
    }
}
async fn ready(
    replies: Vec<ModelReply>,
) -> (
    Arc<Engine>,
    Arc<AtomicUsize>,
    Arc<Mutex<State>>,
    tempfile::TempDir,
) {
    let (e, models, _, saved, root) = fixture(false, replies).await;
    let mut state = State::default();
    state.settings.enabled = true;
    state.settings.free_only = true;
    state.settings.runtime = "pi".into();
    state.settings.route_id = "openrouter::synthetic:free".into();
    let core = Engine::new(
        state,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    (core, models, saved, root)
}
async fn due(e: &Engine, fid: &str) {
    // The production clock is never changed; only this isolated schedule is made due.
    e.mutate(Change::default(), |s| {
        s.followups
            .entries
            .iter_mut()
            .find(|x| x.id == fid)
            .unwrap()
            .due_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        Ok(())
    })
    .await
    .unwrap();
}
async fn run_id(e: &Engine, fid: &str) -> String {
    e.state
        .lock()
        .await
        .followups
        .entries
        .iter()
        .find(|x| x.id == fid)
        .unwrap()
        .runs
        .last()
        .unwrap()
        .turn_id
        .clone()
        .unwrap()
}

#[test]
fn memory_ranking_matches_split_queries_and_excludes_unrelated_expired_or_invalid() {
    let mut p = crate::assistant_personal::Personal::default();
    let now = chrono::Utc::now();
    p.save_memory(input(0, "动画偏好", "蓝色圆点，字幕使用简短中文"), now)
        .unwrap();
    p.save_memory(
        input(
            1,
            "Repository convention",
            "HomeBase uses Rust and TypeScript",
        ),
        now,
    )
    .unwrap();
    p.save_memory(input(2, "饮食习惯", "早餐喜欢面包"), now)
        .unwrap();
    assert_eq!(
        p.search("帮我做动画，中文字幕怎么安排", now)[0].title,
        "动画偏好"
    );
    assert_eq!(
        p.search("repository Rust", now)[0].title,
        "Repository convention"
    );
    assert!(p.search("火星大气探测", now).is_empty());
    p.memories[0].expires_at = Some((now - chrono::Duration::seconds(1)).to_rfc3339());
    assert!(p.search("动画", now).is_empty());
    p.memories[1].expires_at = Some("invalid".into());
    assert!(p.search("Rust", now).is_empty());
}
#[test]
fn memory_conflicts_do_not_overwrite_and_history_is_bounded_and_backward_compatible() {
    let mut p = crate::assistant_personal::Personal::default();
    let now = chrono::Utc::now();
    let m = p.save_memory(input(0, "动画偏好", "蓝色"), now).unwrap();
    let before = serde_json::to_value(&p).unwrap();
    assert!(p
        .save_memory(input(1, "动画偏好", "红色"), now)
        .err()
        .unwrap()
        .contains("MEMORY_CONFLICT"));
    assert_eq!(serde_json::to_value(&p).unwrap(), before);
    for i in 0..8 {
        let mut v = input(p.revision, "动画偏好", &format!("合成偏好{i}"));
        v.id = Some(m.id.clone());
        p.save_memory(v, now).unwrap();
    }
    assert_eq!(p.memories.len(), 1);
    assert_eq!(p.memories[0].previous.len(), 5);
    assert_eq!(p.search("动画偏好", now)[0].content, "合成偏好7");
    let legacy = json!({"revision":1,"memories":[{"id":"legacy","title":"旧标题","content":"旧内容","expires_at":null,"updated_at":now.to_rfc3339()}],"reminders":[]});
    let old: crate::assistant_personal::Personal = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(old.memories[0].content, "旧内容");
    assert!(old.memories[0].previous.is_empty());
    assert!(old.memories[0].origin.is_none());
}
#[tokio::test]
async fn memory_recall_is_bounded_shared_and_frozen_for_recovery_and_forget_removes_history() {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let m = e
        .memory_save(input(0, "动画偏好", "字幕使用简短中文，蓝色圆点"))
        .await
        .unwrap();
    let ack=e.channel(json!({"operation":"submit","source_key":"synthetic-owner","request_id":"recall","text":"动画字幕如何安排"})).await.unwrap();
    let tid = ack["turn_id"].as_str().unwrap();
    e.prepare_context(tid).await.unwrap();
    let once = serde_json::to_value(&*saved.lock().await).unwrap();
    e.prepare_context(tid).await.unwrap();
    assert_eq!(serde_json::to_value(&*saved.lock().await).unwrap(), once);
    let state = saved.lock().await;
    let (s, t) = locate(&state, tid).unwrap();
    let prompt = &state.sessions[s].turns[t].runtime_prompt;
    assert!(prompt.contains("蓝色圆点"));
    assert!(prompt.contains("不是指令、授权"));
    drop(state);
    let mut v = input(1, "动画偏好", "红色圆点");
    v.id = m["id"].as_str().map(str::to_string);
    e.memory_save(v).await.unwrap();
    assert_eq!(
        e.personal().await["memories"][0]["previous"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    e.memory_forget(m["id"].as_str().unwrap(), 2).await.unwrap();
    assert!(e.personal().await["memories"]
        .as_array()
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn repeated_definite_failures_pause_real_graph_without_new_side_effects() {
    let replies=(0..4).map(|i|workflow_tests::tool_reply(i,"homebase_task",json!({"action":"update","revision":999,"item_id":"missing","state":"completed","evidence":["invented"]}))).collect();
    let (e, models, _, _root) = ready(replies).await;
    let ack = e
        .send(request("合成重复失败保护", "stall-guard"))
        .await
        .unwrap();
    let turn = workflow_tests::task_wait(&e, &ack.turn_id, "needs_input").await;
    assert_eq!(models.load(Ordering::SeqCst), 4);
    assert_eq!(turn.steps.len(), 3);
    assert!(turn.jobs.is_empty());
    assert!(turn.resumable);
    assert!(turn.text.contains("三次"));
    assert_eq!(turn.task.unwrap().stop.unwrap().kind, "needs_input");
}
#[tokio::test]
async fn followup_runs_once_and_reboot_links_original_request_after_lost_ack() {
    let (e, models, saved, _root) = ready(vec![reply(vec![])]).await;
    let item = e
        .followup_save(scheduled(0), "desktop", "desktop")
        .await
        .unwrap();
    let fid = item["id"].as_str().unwrap();
    due(&e, fid).await;
    e.followup_tick().await.unwrap();
    let tid = run_id(&e, fid).await;
    workflow_tests::task_wait(&e, &tid, "completed").await;
    e.followup_tick().await.unwrap();
    assert_eq!(models.load(Ordering::SeqCst), 1);
    // Simulate a persisted claim + accepted turn, but an interrupted link save.
    let mut state = saved.lock().await.clone();
    state.followups.entries[0].enabled = true;
    state.followups.entries[0].state = "running".into();
    state.followups.entries[0].runs[0].turn_id = None;
    state.followups.entries[0].runs[0].state = "prepared".into();
    let old_sessions = serde_json::to_value(&state.sessions).unwrap();
    let restored = Engine::new(
        state,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    restored.followup_tick().await.unwrap();
    assert_eq!(run_id(&restored, fid).await, tid);
    assert_eq!(models.load(Ordering::SeqCst), 1);
    assert_eq!(
        serde_json::to_value(&restored.state.lock().await.sessions).unwrap(),
        old_sessions
    );
    assert_eq!(
        restored.followups().await["entries"][0]["state"],
        "completed"
    );
}
#[tokio::test]
async fn followup_prepared_before_send_resumes_same_identity_and_save_failure_does_not_submit() {
    let (e, models, saved, _root) = ready(vec![reply(vec![])]).await;
    let item = e
        .followup_save(scheduled(0), "desktop", "desktop")
        .await
        .unwrap();
    let fid = item["id"].as_str().unwrap();
    let mut state = saved.lock().await.clone();
    let e0 = &mut state.followups.entries[0];
    e0.state = "running".into();
    e0.due_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    e0.runs.push(followup::FollowupRun {
        ordinal: 1,
        scheduled_at: e0.due_at.clone(),
        request_id: format!("hb-followup-{fid}-1"),
        turn_id: None,
        state: "prepared".into(),
    });
    let failing: Save = Arc::new(|_| Box::pin(async { Err("synthetic disk full".into()) }));
    let denied = Engine::new(
        state.clone(),
        failing,
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(denied.followup_tick().await.is_err());
    assert_eq!(models.load(Ordering::SeqCst), 0);
    let restored = Engine::new(
        state,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    restored.followup_tick().await.unwrap();
    let tid = run_id(&restored, fid).await;
    workflow_tests::task_wait(&restored, &tid, "completed").await;
    restored.followup_tick().await.unwrap();
    assert_eq!(
        restored.followups().await["entries"][0]["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(models.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn followup_disconnect_during_admission_waits_and_recovers_original_request() {
    use std::future::Future;
    use std::task::{Context, Waker};

    let (e, models, saved, _root) = ready(vec![reply(vec![])]).await;
    let item = e
        .followup_save(scheduled(0), "wechat", "owner-a")
        .await
        .unwrap();
    let fid = item["id"].as_str().unwrap();
    let heartbeat = |source, connected| json!({"operation":"notifications","source_key":source,"connected":connected});
    e.channel(heartbeat("owner-a", true)).await.unwrap();
    due(&e, fid).await;
    let send_gate = e.send_gate.lock().await;
    let mut tick = std::pin::pin!(e.followup_tick());
    assert!(tick
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending());
    let prepared = e.followups().await;
    let original = &prepared["entries"][0]["runs"][0];
    assert_eq!(original["state"], "prepared");
    assert!(original["turn_id"].is_null());
    e.channel(heartbeat("owner-a", false)).await.unwrap();
    drop(send_gate);
    let outcome = tick.await;
    assert_eq!(e.followups().await, prepared);
    outcome.unwrap();
    assert_eq!(models.load(Ordering::SeqCst), 0);

    let snapshot = saved.lock().await.clone();
    assert!(snapshot.sessions.is_empty());
    let restored = Engine::new(
        snapshot,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    restored
        .channel(heartbeat("other-owner", true))
        .await
        .unwrap();
    restored.followup_tick().await.unwrap();
    assert_eq!(restored.followups().await, prepared);
    restored.channel(heartbeat("owner-a", true)).await.unwrap();
    restored.followup_tick().await.unwrap();
    let tid = run_id(&restored, fid).await;
    workflow_tests::task_wait(&restored, &tid, "completed").await;
    restored.followup_tick().await.unwrap();
    let final_plan = restored.followups().await["entries"][0].clone();
    assert_eq!(final_plan["state"], "completed");
    let runs = final_plan["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["request_id"], original["request_id"]);
    assert_eq!(runs[0]["scheduled_at"], original["scheduled_at"]);
    assert_eq!(runs[0]["turn_id"], tid);
    assert_eq!(models.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn followup_is_bound_to_model_grants_time_and_connected_original_owner() {
    for mode in ["model", "grant", "offline", "owner", "disconnected"] {
        let (e, models, _, _root) = ready(vec![]).await;
        let item = e
            .followup_save(scheduled(0), "wechat", "owner-a")
            .await
            .unwrap();
        let fid = item["id"].as_str().unwrap();
        due(&e, fid).await;
        e.channel(json!({"operation":"notifications","source_key":if mode=="owner"{"owner-b"}else{"owner-a"}})).await.unwrap();
        if mode == "disconnected" {
            e.channel(
                json!({"operation":"notifications","source_key":"owner-a","connected":false}),
            )
            .await
            .unwrap();
        }
        e.mutate(Change::default(), |s| {
            match mode {
                "model" => s.settings.revision += 1,
                "grant" => s.local.revision += 1,
                "offline" => {
                    s.followups.entries[0].due_at =
                        (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339()
                }
                _ => (),
            };
            Ok(())
        })
        .await
        .unwrap();
        e.followup_tick().await.unwrap();
        let state = e.followups().await;
        assert_eq!(models.load(Ordering::SeqCst), 0);
        assert!(state["entries"][0]["runs"].as_array().unwrap().is_empty());
        assert_eq!(
            state["entries"][0]["state"],
            if matches!(mode, "owner" | "disconnected") {
                "scheduled"
            } else if mode == "offline" {
                "missed"
            } else {
                "blocked_model"
            }
        );
    }
}
#[tokio::test]
async fn offline_wechat_followup_does_not_schedule_recurring_idle_checks() {
    let (e, _, saved, _root) = ready(vec![]).await;
    e.followup_save(scheduled(0), "wechat", "owner-a")
        .await
        .unwrap();
    let mut state = saved.lock().await.clone();
    let now = chrono::Utc::now();
    let overdue = now - chrono::Duration::seconds(5);
    state.followups.entries[0].due_at = overdue.to_rfc3339();
    let mut presence = BTreeMap::new();
    assert_eq!(Engine::next_personal_check_at(&state, &presence, now), None);
    presence.insert("owner-b".into(), tokio::time::Instant::now());
    presence.insert(
        "owner-a".into(),
        tokio::time::Instant::now() - Duration::from_secs(90),
    );
    assert_eq!(Engine::next_personal_check_at(&state, &presence, now), None);
    presence.insert("owner-a".into(), tokio::time::Instant::now());
    assert_eq!(
        Engine::next_personal_check_at(&state, &presence, now),
        Some(overdue)
    );
    presence.clear();

    let desktop_due = now + chrono::Duration::minutes(20);
    let mut desktop = state.followups.entries[0].clone();
    desktop.source = "desktop".into();
    desktop.due_at = desktop_due.to_rfc3339();
    state.followups.entries.push(desktop);
    assert_eq!(
        Engine::next_personal_check_at(&state, &presence, now),
        Some(desktop_due)
    );

    let event_deadline = now + chrono::Duration::minutes(5);
    state.followups.entries[0].state = "waiting_event".into();
    state.followups.entries[0].due_at = event_deadline.to_rfc3339();
    assert_eq!(
        Engine::next_personal_check_at(&state, &presence, now),
        Some(event_deadline)
    );

    let mut entry = state.followups.entries[0].clone();
    entry.state = "running".into();
    entry.runs.push(followup::FollowupRun {
        ordinal: 1,
        scheduled_at: overdue.to_rfc3339(),
        request_id: "hb-followup-prepared-synthetic-1".into(),
        turn_id: None,
        state: "prepared".into(),
    });
    state.followups.entries[0] = entry;
    assert_eq!(
        Engine::next_personal_check_at(&state, &presence, now),
        Some(desktop_due)
    );
    let run = &mut state.followups.entries[0].runs[0];
    run.turn_id = Some("accepted-original-turn".into());
    run.state = "running".into();
    assert_eq!(
        Engine::next_personal_check_at(&state, &presence, now),
        Some(now + chrono::Duration::seconds(15))
    );
}

#[tokio::test]
async fn original_wechat_owner_wakes_overdue_followup_once() {
    let (e, models, _, _root) = ready(vec![reply(vec![])]).await;
    let plan = e
        .followup_save(scheduled(0), "wechat", "owner-a")
        .await
        .unwrap();
    let fid = plan["id"].as_str().unwrap();
    let heartbeat = |source, connected| json!({"operation":"notifications","source_key":source,"connected":connected});
    e.channel(heartbeat("owner-a", true)).await.unwrap();
    e.channel(heartbeat("owner-a", false)).await.unwrap();
    due(&e, fid).await;
    e.channel(heartbeat("owner-b", true)).await.unwrap();
    e.followup_tick().await.unwrap();
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(e.followups().await["entries"][0]["runs"], json!([]));

    e.channel(heartbeat("owner-a", true)).await.unwrap();
    let tid = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let followups = e.followups().await;
            if let Some(tid) = followups["entries"][0]["runs"][0]["turn_id"].as_str() {
                break tid.to_string();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("reconnection must wake the scheduler for the original owner");
    workflow_tests::task_wait(&e, &tid, "completed").await;
    let original = e.channel(heartbeat("owner-a", true)).await.unwrap();
    assert_eq!(original[0]["turn_id"], tid);
    assert_eq!(
        e.channel(heartbeat("owner-a", true)).await.unwrap(),
        original
    );
    e.followup_tick().await.unwrap();
    assert_eq!(run_id(&e, fid).await, tid);
    assert_eq!(e.followups().await["entries"][0]["state"], "completed");
    assert_eq!(models.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn followup_recurrence_skips_backlog_and_cancellation_and_failures_stop_future_runs() {
    let (e, models, _, _root) = ready(vec![reply(vec![])]).await;
    let mut input = scheduled(0);
    input.interval_minutes = Some(5);
    input.max_runs = 3;
    let entry = e.followup_save(input, "desktop", "desktop").await.unwrap();
    let fid = entry["id"].as_str().unwrap();
    due(&e, fid).await;
    e.followup_tick().await.unwrap();
    let tid = run_id(&e, fid).await;
    workflow_tests::task_wait(&e, &tid, "completed").await;
    e.mutate(Change::default(), |s| {
        s.followups.entries[0].runs[0].scheduled_at =
            (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        Ok(())
    })
    .await
    .unwrap();
    e.followup_tick().await.unwrap();
    let first = e.followups().await;
    assert_eq!(first["entries"][0]["state"], "scheduled");
    assert!(
        crate::assistant_personal::timestamp(first["entries"][0]["due_at"].as_str().unwrap())
            .unwrap()
            > chrono::Utc::now()
    );
    e.followup_tick().await.unwrap();
    assert_eq!(e.followups().await, first);
    assert_eq!(models.load(Ordering::SeqCst), 1);
    e.followup_cancel(fid, first["revision"].as_u64().unwrap())
        .await
        .unwrap();
    due(&e, fid).await;
    e.followup_tick().await.unwrap();
    assert_eq!(models.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_scheduled_model_pauses_repeat_without_replaying_uncertain_request() {
    let (e, models, _, _root) = ready(vec![]).await;
    let mut input = scheduled(0);
    input.interval_minutes = Some(5);
    input.max_runs = 3;
    let item = e.followup_save(input, "desktop", "desktop").await.unwrap();
    let fid = item["id"].as_str().unwrap();
    due(&e, fid).await;
    e.followup_tick().await.unwrap();
    let tid = run_id(&e, fid).await;
    workflow_tests::task_wait(&e, &tid, "failed").await;
    e.followup_tick().await.unwrap();
    assert_eq!(e.followups().await["entries"][0]["state"], "paused");
    assert!(!e.turn(&tid).await.unwrap().resumable);
    for _ in 0..3 {
        e.followup_tick().await.unwrap();
    }
    assert_eq!(models.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn followup_recovery_preserves_concurrent_wechat_notice_updates() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};

    let (e, models, saved, _root) = ready(vec![reply(vec![])]).await;
    let plan = e
        .followup_save(scheduled(0), "desktop", "desktop")
        .await
        .unwrap();
    let plan_id = plan["id"].as_str().unwrap();
    let notice = e
        .followup_save(scheduled(1), "wechat", "owner-a")
        .await
        .unwrap();
    let notice_id = notice["id"].as_str().unwrap();
    let client_id = "synthetic-wechat-notice";
    due(&e, plan_id).await;
    e.followup_tick().await.unwrap();
    let turn_id = run_id(&e, plan_id).await;
    workflow_tests::task_wait(&e, &turn_id, "completed").await;

    for (operation, expected) in [
        ("claim", "claimed"),
        ("accepted", "accepted"),
        ("unknown", "unknown"),
        ("not_sent", "pending"),
    ] {
        e.mutate(Change::default(), |state| {
            let recovered = &mut state.followups.entries[0];
            recovered.enabled = true;
            recovered.state = "running".into();
            recovered.runs[0].turn_id = None;
            recovered.runs[0].state = "prepared".into();
            let pending = &mut state.followups.entries[1];
            pending.enabled = false;
            pending.state = "paused".into();
            pending.notice_id = client_id.into();
            pending.notice_delivery = if operation == "claim" {
                "pending"
            } else {
                "claimed"
            }
            .into();
            state.followups.revision += 1;
            Ok(())
        })
        .await
        .unwrap();

        // Queue the channel write between the scheduler's reconcile read and commit.
        // Manual polling on the same thread makes the interleaving deterministic.
        let initial = e.state.lock().await;
        let mut tick = std::pin::pin!(e.followup_tick());
        let mut between_reads = std::pin::pin!(e.state.lock());
        let mut cx = Context::from_waker(Waker::noop());
        assert!(tick.as_mut().poll(&mut cx).is_pending());
        assert!(between_reads.as_mut().poll(&mut cx).is_pending());
        drop(initial);
        assert!(tick.as_mut().poll(&mut cx).is_pending());
        let Poll::Ready(before_reconcile) = between_reads.as_mut().poll(&mut cx) else {
            panic!("expected the gate between event and run reconciliation");
        };
        let mut channel_write = std::pin::pin!(async {
            if operation == "claim" {
                e.mutate(
                    Change {
                        personal: true,
                        ..Default::default()
                    },
                    |state| {
                        let claimed = Engine::claim_notices(state, "owner-a");
                        assert_eq!(claimed.len(), 1);
                        assert_eq!(claimed[0]["client_id"], client_id);
                        Ok(())
                    },
                )
                .await
            } else {
                e.channel(
                    json!({"operation":"notification_ack","kind":"followup_notice",
                    "source_key":"owner-a","reminder_id":notice_id,"client_id":client_id,
                    "delivery":operation}),
                )
                .await
                .map(|_| ())
            }
        });
        assert!(channel_write.as_mut().poll(&mut cx).is_pending());
        drop(before_reconcile);
        assert!(tick.as_mut().poll(&mut cx).is_pending());
        channel_write.await.unwrap();
        tick.await.unwrap();

        let state = saved.lock().await.clone();
        assert_eq!(
            state.followups.entries[1].notice_delivery, expected,
            "{operation}"
        );
        assert_eq!(state.followups.entries[1].notice_id, client_id);
        assert_eq!(state.followups.entries[0].state, "completed");
        assert_eq!(state.followups.entries[0].runs.len(), 1);
        assert_eq!(
            state.followups.entries[0].runs[0].turn_id.as_deref(),
            Some(turn_id.as_str())
        );
        e.followup_tick().await.unwrap();
        assert_eq!(
            saved.lock().await.followups.revision,
            state.followups.revision
        );
        assert_eq!(models.load(Ordering::SeqCst), 1);
        let notifications = e
            .channel(json!({"operation":"notifications","source_key":"owner-a"}))
            .await
            .unwrap();
        if operation == "not_sent" {
            assert_eq!(notifications.as_array().unwrap().len(), 1);
            assert_eq!(notifications[0]["client_id"], client_id);
        } else {
            assert_eq!(notifications, json!([]));
        }
    }
}

#[tokio::test]
async fn followup_notice_is_owner_bound_claimed_once_and_unknown_is_not_resent() {
    let (e, models, saved, _root) = ready(vec![]).await;
    let item = e
        .followup_save(scheduled(0), "wechat", "owner-a")
        .await
        .unwrap();
    let fid = item["id"].as_str().unwrap();
    due(&e, fid).await;
    e.channel(json!({"operation":"notifications","source_key":"owner-a"}))
        .await
        .unwrap();
    e.mutate(Change::default(), |s| {
        s.local.revision += 1;
        Ok(())
    })
    .await
    .unwrap();
    e.followup_tick().await.unwrap();
    assert_eq!(
        e.channel(json!({"operation":"notifications","source_key":"owner-b"}))
            .await
            .unwrap(),
        json!([])
    );
    let notices = e
        .channel(json!({"operation":"notifications","source_key":"owner-a"}))
        .await
        .unwrap();
    assert_eq!(notices.as_array().unwrap().len(), 1);
    assert_eq!(notices[0]["kind"], "followup_notice");
    assert_eq!(
        e.channel(json!({"operation":"notifications","source_key":"owner-a"}))
            .await
            .unwrap(),
        json!([])
    );
    assert!(e.channel(json!({"operation":"notification_ack","kind":"followup_notice","source_key":"owner-b","reminder_id":fid,"delivery":"accepted"})).await.is_err());
    let release = |client: Value| json!({"operation":"notification_ack","kind":"followup_notice",
        "source_key":"owner-a","reminder_id":fid,"client_id":client,"delivery":"not_sent"});
    assert!(e.channel(release(json!("older-followup-notice"))).await.is_err());
    e.channel(release(notices[0]["client_id"].clone())).await.unwrap();
    assert_eq!(e.channel(json!({"operation":"notifications","source_key":"owner-a"})).await.unwrap(), notices);
    let state = saved.lock().await.clone();
    let restored = Engine::new(
        state,
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        restored.followups().await["entries"][0]["notice_delivery"],
        "unknown"
    );
    assert_eq!(
        restored
            .channel(json!({"operation":"notifications","source_key":"owner-a"}))
            .await
            .unwrap(),
        json!([])
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn scheduled_work_requires_explicit_free_config_and_valid_finite_schedule() {
    let (e, models, _, _, _root) = fixture(false, vec![]).await;
    assert!(e
        .followup_save(scheduled(0), "desktop", "desktop")
        .await
        .is_err());
    assert_eq!(models.load(Ordering::SeqCst), 0);
    let (e, models, _, _root) = ready(vec![]).await;
    for mode in ["timezone", "past", "unbounded", "interval"] {
        let mut input = scheduled(0);
        match mode {
            "timezone" => input.due_at = "2026-11-13 12:00".into(),
            "past" => {
                input.due_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()
            }
            "unbounded" => input.max_runs = 999,
            "interval" => {
                input.interval_minutes = Some(1);
                input.max_runs = 2;
            }
            _ => (),
        }
        assert!(e.followup_save(input, "desktop", "desktop").await.is_err());
    }
    assert!(e.followups().await["entries"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(models.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn active_scheduled_turn_can_be_found_across_chats_but_ambiguous_control_is_rejected() {
    let (e, _, _, _, _root) = fixture(false, vec![]).await;
    let a=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"old-task","text":"合成旧任务"})).await.unwrap();
    let aid = a["turn_id"].as_str().unwrap();
    e.change_turn(aid, |s, t| {
        s.turns[t].state = "running".into();
        Ok(())
    })
    .await
    .unwrap();
    e.make_session_fixed("wechat", "owner-a", "new-empty-chat")
        .await
        .unwrap();
    {
        let state = e.state.lock().await;
        assert_eq!(
            Engine::channel_target(&state, "owner-a")
                .unwrap()
                .unwrap()
                .1
                .id,
            aid
        );
        assert!(Engine::channel_target(&state, "owner-b").unwrap().is_none());
    }
    let b=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"new-task","text":"合成另一任务"})).await.unwrap();
    e.change_turn(b["turn_id"].as_str().unwrap(), |s, t| {
        s.turns[t].state = "running".into();
        Ok(())
    })
    .await
    .unwrap();
    assert!(e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"ambiguous-stop","text":"停止"})).await.is_err());
    assert_eq!(e.turn(aid).await.unwrap().state, "running");
}

#[tokio::test]
async fn failed_mutating_call_is_not_reexecuted_but_new_reads_stay_fresh() {
    let (e, _, _, _, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("合成副作用检查", "effect-guard"))
        .await
        .unwrap();
    let write = ToolCall {
        id: "x".into(),
        name: "homebase_terminal".into(),
        arguments: json!({"command":"synthetic"}),
    };
    let mut result = json!({"state":"failed","result":{"exit_code":1}});
    e.record_tool_outcome(&ack.turn_id, "write-fingerprint", &write, &mut result)
        .await
        .unwrap();
    assert!(e
        .check_repeated_failure(&ack.turn_id, "write-fingerprint", &write)
        .await
        .unwrap_err()
        .starts_with("HB_STALLED_EFFECT:"));
    let read = ToolCall {
        id: "y".into(),
        name: "homebase_files".into(),
        arguments: json!({"action":"read","path":"synthetic"}),
    };
    for _ in 0..4 {
        e.check_repeated_failure(&ack.turn_id, "read-fingerprint", &read)
            .await
            .unwrap();
        e.record_tool_outcome(
            &ack.turn_id,
            "read-fingerprint",
            &read,
            &mut json!({"status":"succeeded","content":"fresh"}),
        )
        .await
        .unwrap();
    }
}

#[path = "assistant_failure_ledger_tests.rs"]
mod failure_ledger_tests;

#[tokio::test]
async fn fresh_scheduled_confirmation_is_reachable_but_new_chat_is_not_consent_to_old_work() {
    let (e, _, _, _, _root) = fixture(false, vec![]).await;
    let old=e.channel(json!({"operation":"submit","source_key":"owner-a","request_id":"old-approval","text":"旧任务"})).await.unwrap();
    let old_id = old["turn_id"].as_str().unwrap();
    let sid = old["session_id"].as_str().unwrap();
    e.change_turn(old_id, |s, t| {
        s.turns[t].state = "awaiting_confirmation".into();
        s.turns[t].pending = Some(Pending {
            id: id(),
            tool_id: "x".into(),
            description: "synthetic".into(),
            arguments: json!({}),
            approved: false,
        });
        Ok(())
    })
    .await
    .unwrap();
    e.make_session_fixed("wechat", "owner-a", "new-context-boundary")
        .await
        .unwrap();
    assert!(Engine::confirmation_targets(&*e.state.lock().await, "owner-a").is_empty());
    let fresh = e
        .send_from(
            SendRequest {
                image: None,
                route_id: None,
                attachments: vec![],
                mode: None,
                session_id: Some(sid.into()),
                text: "新到达的主动任务".into(),
                request_id: "synthetic-fresh-turn".into(),
            },
            "wechat",
            "owner-a",
        )
        .await
        .unwrap();
    e.change_turn(&fresh.turn_id, |s, t| {
        s.turns[t].request_id = "hb-followup-synthetic-1".into();
        s.turns[t].state = "awaiting_confirmation".into();
        s.turns[t].pending = Some(Pending {
            id: id(),
            tool_id: "y".into(),
            description: "synthetic".into(),
            arguments: json!({}),
            approved: false,
        });
        Ok(())
    })
    .await
    .unwrap();
    {
        let state = e.state.lock().await;
        let targets = Engine::confirmation_targets(&state, "owner-a");
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, fresh.turn_id);
        assert!(Engine::confirmation_targets(&state, "owner-b").is_empty());
    }
    e.make_session_fixed("wechat", "owner-a", "another-explicit-new-context")
        .await
        .unwrap();
    assert!(Engine::confirmation_targets(&*e.state.lock().await, "owner-a").is_empty());
}

#[tokio::test]
async fn scheduled_acceptance_rechecks_policy_atomically_and_never_uses_changed_paid_settings() {
    let (e, models, _, _root) = ready(vec![]).await;
    let entry = e
        .followup_save(scheduled(0), "desktop", "desktop")
        .await
        .unwrap();
    let fid = entry["id"].as_str().unwrap();
    let rid = format!("hb-followup-{fid}-1");
    e.mutate(Change::default(), |s| {
        let due = s.followups.entries[0].due_at.clone();
        s.followups.entries[0].runs.push(followup::FollowupRun {
            ordinal: 1,
            scheduled_at: due,
            request_id: rid.clone(),
            turn_id: None,
            state: "prepared".into(),
        });
        Ok(())
    })
    .await
    .unwrap();
    {
        let state = e.state.lock().await;
        assert!(Engine::scheduled_submission_allowed(
            &state,
            &rid,
            "desktop",
            "desktop",
            &state.settings
        ));
        assert!(!Engine::scheduled_submission_allowed(
            &state,
            &rid,
            "desktop",
            "other-owner",
            &state.settings
        ));
    }
    e.mutate(Change::default(), |s| {
        s.settings.free_only = false;
        s.settings.revision += 1;
        Ok(())
    })
    .await
    .unwrap();
    assert!(e
        .send_from(
            SendRequest {
                image: None,
                route_id: None,
                attachments: vec![],
                mode: None,
                session_id: None,
                text: "不应执行".into(),
                request_id: rid
            },
            "desktop",
            "desktop"
        )
        .await
        .err()
        .unwrap()
        .contains("配置或计划已变化"));
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert!(e
        .state
        .lock()
        .await
        .sessions
        .iter()
        .all(|s| s.turns.is_empty()));
}

#[tokio::test]
async fn manually_recovered_scheduled_turn_updates_result_without_reenabling_recurrence() {
    for recurring in [false, true] {
        let (e, models, _, _root) = ready(vec![reply(vec![])]).await;
        let mut input = scheduled(0);
        if recurring {
            input.interval_minutes = Some(5);
            input.max_runs = 3;
        }
        let entry = e.followup_save(input, "desktop", "desktop").await.unwrap();
        let fid = entry["id"].as_str().unwrap();
        due(&e, fid).await;
        e.followup_tick().await.unwrap();
        let tid = run_id(&e, fid).await;
        workflow_tests::task_wait(&e, &tid, "completed").await;
        e.mutate(Change::default(), |s| {
            let entry = &mut s.followups.entries[0];
            entry.enabled = false;
            entry.state = "paused".into();
            entry.notice = "旧暂停说明".into();
            entry.runs[0].state = "failed".into();
            Ok(())
        })
        .await
        .unwrap();
        e.followup_tick().await.unwrap();
        let result = e.followups().await;
        assert_eq!(
            result["entries"][0]["state"],
            if recurring { "paused" } else { "completed" }
        );
        assert!(result["entries"][0]["notice"]
            .as_str()
            .unwrap()
            .contains("恢复完成"));
        assert_eq!(result["entries"][0]["enabled"], false);
        e.followup_tick().await.unwrap();
        assert_eq!(models.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn incomplete_followup_proposal_returns_actionable_error_without_requesting_consent() {
    let (e, models, _, _, _root) = fixture(
        true,
        vec![
            reply(vec![(
                "homebase_followup",
                json!({"action":"create","title":"不完整的合成安排"}),
            )]),
            reply(vec![]),
        ],
    )
    .await;
    let a = e
        .send(request("帮我安排合成任务", "invalid-followup-proposal"))
        .await
        .unwrap();
    let turn = wait(&e, &a.turn_id, "completed").await;
    assert!(turn.pending.is_none());
    assert_eq!(turn.steps[0].result["code"], "FOLLOWUP_INVALID");
    assert_eq!(models.load(Ordering::SeqCst), 2);
    assert!(e.followups().await["entries"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn renaming_memory_cannot_create_a_conflicting_duplicate_title() {
    let mut p = crate::assistant_personal::Personal::default();
    let now = chrono::Utc::now();
    let first = p.save_memory(input(0, "动画偏好", "蓝色"), now).unwrap();
    p.save_memory(input(1, "字幕偏好", "中文"), now).unwrap();
    let baseline = serde_json::to_value(&p).unwrap();
    let mut rename = input(2, "字幕偏好", "红色");
    rename.id = Some(first.id);
    assert!(p
        .save_memory(rename, now)
        .err()
        .unwrap()
        .contains("MEMORY_CONFLICT"));
    assert_eq!(serde_json::to_value(&p).unwrap(), baseline);
}

#[test]
fn native_host_disconnect_keeps_unknown_separate_from_validation_failure() {
    for message in ["本机工具宿主断开，执行结果未知；未自动重新执行", "本机工具等待超时或断开；已请求停止，结果未知，未自动重试", "本机工具宿主已断开；未重试"] {
        let result=crate::assistant_local::operation_failure(message.into());assert_eq!(result["status"],"unknown");assert_eq!(result["code"],"LOCAL_RESULT_UNKNOWN");
    }
    assert_eq!(crate::assistant_local::operation_failure("工作区尚未授权".into())["status"],"failed");
}

#[tokio::test]
async fn unknown_local_result_stops_the_agent_instead_of_trying_another_execution() {
    let(e,models,_,saved,root)=fixture(true,vec![reply(vec![("homebase_terminal",json!({"command":"synthetic-uncertain"}))]),reply(vec![])]).await;
    let effects=Arc::new(AtomicUsize::new(0));let count=effects.clone();
    let runner:Runner=Arc::new(move|_,_|{let count=count.clone();Box::pin(async move{count.fetch_add(1,Ordering::SeqCst);Ok(json!({"status":"unknown","code":"EXECUTION_UNKNOWN","message":"合成未返回终态；不重跑"}))})});
    let ctl_root=root.path().join("unknown-control");std::fs::create_dir_all(&ctl_root).unwrap();let controller=Controller::new(catalog().clone(),&ctl_root,runner).unwrap();
    let control:Control=Arc::new(move|r|{let c=controller.clone();Box::pin(async move{c.handle(r).await})});
    let engine=Engine::new(saved.lock().await.clone(),e.save.clone(),e.model.clone(),control,e.workspace.clone()).await.unwrap();
    let request=request("合成未知结果保护","local-unknown-stop");let ack=engine.send(request.clone()).await.unwrap();let turn=wait(&engine,&ack.turn_id,"failed").await;
    assert_eq!(turn.jobs.len(),1);assert_eq!(turn.jobs[0].state,"unknown");assert_eq!(turn.jobs[0].result["code"],"EXECUTION_UNKNOWN");
    assert_eq!(engine.send(request).await.unwrap().turn_id,ack.turn_id);assert_eq!(models.load(Ordering::SeqCst),1);assert_eq!(effects.load(Ordering::SeqCst),1);
}

#[tokio::test]
async fn real_graph_memory_update_preserves_omitted_expiry_and_only_explicit_null_clears_it() {
    let mut personal = crate::assistant_personal::Personal::default();
    let expiry = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let mut seed = input(0, "字幕偏好", "合成旧偏好");
    seed.expires_at = Some(expiry.clone());
    let original = personal.save_memory(seed, chrono::Utc::now()).unwrap();
    let(e,models,_,_root)=ready(vec![workflow_tests::tool_reply(0,"homebase_memory",json!({"action":"remember","revision":1,"id":original.id,"title":"字幕偏好","content":"合成新偏好"})),reply(vec![]),workflow_tests::tool_reply(2,"homebase_memory",json!({"action":"remember","revision":2,"id":original.id,"title":"字幕偏好","content":"合成新偏好","expires_at":null})),reply(vec![])]).await;
    e.mutate(Change::default(), |s| {
        s.personal = personal;
        Ok(())
    })
    .await
    .unwrap();
    for i in 0..2 {
        let ack = e
            .send(request(
                if i == 0 {
                    "更新字幕偏好，保持原有效期"
                } else {
                    "明确清除这条记忆的到期时间"
                },
                &format!("memory-expiry-{i}"),
            ))
            .await
            .unwrap();
        let waiting = workflow_tests::task_wait(&e, &ack.turn_id, "awaiting_confirmation").await;
        e.confirm(&ack.turn_id, &waiting.pending.unwrap().id, true)
            .await
            .unwrap();
        workflow_tests::task_wait(&e, &ack.turn_id, "completed").await;
        let personal = e.personal().await;
        assert_eq!(
            personal["memories"][0]["expires_at"],
            if i == 0 { json!(expiry) } else { Value::Null }
        );
    }
    assert_eq!(models.load(Ordering::SeqCst), 4);
    let current = wire::tools_for_runtime(RUNTIME_VERSION);
    let current = current
        .iter()
        .find(|d| d["function"]["name"] == "homebase_memory")
        .unwrap();
    assert_eq!(
        current["function"]["parameters"]["properties"]["expires_at"]["type"],
        json!(["string", "null"])
    );
    for old in [
        "homebase-da-20261112.2",
        "homebase-da-20261113.2",
        "homebase-da-20261113.3",
    ] {
        let defs = wire::tools_for_runtime(old);
        let memory = defs
            .iter()
            .find(|d| d["function"]["name"] == "homebase_memory")
            .unwrap();
        assert_eq!(
            memory["function"]["parameters"]["properties"]["expires_at"]["type"],
            "string"
        );
    }
}
