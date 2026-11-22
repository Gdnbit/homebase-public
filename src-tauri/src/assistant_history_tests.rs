use super::*;
use crate::assistant_history::{HistoryKind, HistoryQuery, HistoryRead, HistoryStore};
use std::time::Instant;

fn query(session: &str) -> HistoryQuery {
    HistoryQuery {
        session_id: session.into(),
        kind: HistoryKind::Messages,
        query: String::new(),
        snapshot_id: None,
        through_turn_id: None,
        offset: 0,
        limit: 20,
    }
}

#[tokio::test]
async fn exact_turn_snapshot_keeps_frozen_redacted_results_and_bounds_private_chunks() {
    let (engine, models, effects, _, _root) = fixture(false, vec![]).await;
    let original = engine.send(request("合成原任务", "direct-history")).await.unwrap();
    let other = engine.send(request("另一个会话", "other-direct-history")).await.unwrap();
    engine.change_turn(&original.turn_id, |s, index| {
        s.turns[index].steps.push(Step {
            id: "isolated-step".into(),
            name: "homebase_call".into(),
            state: "completed".into(),
            arguments: json!({"access_token":"SYNTHETIC-REDACTION-SENTINEL"}),
            result: json!({"text":"合🙂\"\\\n".repeat(280_000),"env":{"EXAMPLE":"SYNTHETIC-ENV-SENTINEL"}}),
            job_id: None,
        });
        Ok(())
    }).await.unwrap();
    assert!(engine.history_turn_snapshot(&other.session_id, &original.turn_id)
        .await.unwrap_err().starts_with("HISTORY_SCOPE_MISMATCH:"));
    let snap = engine.history_turn_snapshot(&original.session_id, &original.turn_id).await.unwrap();
    assert_eq!(snap.total, 1);
    let item = &snap.items[0];
    assert_eq!(item.id, original.turn_id);
    assert!(item.json_bytes > 2 * 1024 * 1024);
    engine.change_turn(&original.turn_id, |s, index| {
        s.turns[index].steps[0].result = json!({"changed":true});
        Ok(())
    }).await.unwrap();

    let mut read = HistoryRead {
        session_id: original.session_id.clone(),
        snapshot_id: snap.snapshot_id.clone(),
        item_id: original.turn_id.clone(),
        offset: 0,
        limit: 512 * 1024,
    };
    assert!(engine.history.lock().await.read(&read, "desktop", Some(&original.turn_id), Instant::now())
        .unwrap_err().starts_with("HISTORY_SCOPE_MISMATCH:"));
    let mut joined = String::new();
    for _ in 0..30 {
        let part = engine.history_read(read.clone()).await.unwrap();
        assert!(serde_json::to_vec(&part).unwrap().len() <= 1024 * 1024);
        assert_eq!(part.offset, joined.len());
        assert_eq!(part.json_sha256, item.json_sha256);
        joined.push_str(&part.content);
        if part.complete {
            assert_eq!(part.next_offset, None);
            break;
        }
        read.offset = part.next_offset.unwrap();
    }
    assert_eq!(joined.len(), item.json_bytes);
    assert_eq!(format!("{:x}", Sha256::digest(joined.as_bytes())), item.json_sha256);
    let value: Value = serde_json::from_str(&joined).unwrap();
    assert_eq!(value["id"], original.turn_id);
    assert_eq!(value["steps"][0]["arguments"]["access_token"], "[redacted]");
    assert_eq!(value["steps"][0]["result"]["env"]["EXAMPLE"], "[redacted]");
    assert!(value["steps"][0]["result"]["text"].as_str().unwrap().starts_with("合🙂"));
    assert!(!joined.contains("SYNTHETIC-"));
    read.offset = joined.find('合').unwrap() + 1;
    assert!(engine.history_read(read.clone()).await.unwrap_err().starts_with("HISTORY_BAD_OFFSET:"));
    let mut normal = query(&original.session_id);
    normal.kind = HistoryKind::Turns;
    let ordinary = engine.history_query(normal).await.unwrap();
    read.snapshot_id = ordinary.snapshot_id;
    read.offset = 0;
    assert!(engine.history_read(read).await.unwrap_err().starts_with("HISTORY_BAD_ARGUMENT:"));
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn history_active_reads_renew_idle_lease_but_not_absolute_lifetime_or_other_scope() {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let ack = e.send(request("历史租约", "history-lease")).await.unwrap();
    let session = saved.lock().await.sessions[0].clone();
    let mut q = query(&ack.session_id);
    let now = Instant::now();
    let mut store = HistoryStore::default();
    q.snapshot_id = Some(
        store
            .insert(history_ops::history_source(&session, &q).unwrap(), now)
            .unwrap(),
    );
    assert!(store
        .query(&q, "wrong-source", None, now + Duration::from_secs(299))
        .is_err());
    assert!(store
        .query(&q, "desktop", None, now + Duration::from_secs(301))
        .unwrap_err()
        .starts_with("HISTORY_SNAPSHOT_EXPIRED:"));
    q.snapshot_id = None;
    let mut store = HistoryStore::default();
    q.snapshot_id = Some(
        store
            .insert(history_ops::history_source(&session, &q).unwrap(), now)
            .unwrap(),
    );
    for seconds in (240..3600).step_by(240) {
        assert!(store
            .query(&q, "desktop", None, now + Duration::from_secs(seconds))
            .is_ok());
    }
    assert!(store
        .query(&q, "desktop", None, now + Duration::from_secs(3601))
        .unwrap_err()
        .starts_with("HISTORY_SNAPSHOT_EXPIRED:"));
}
async fn message(
    engine: &Engine,
    session: &str,
    snapshot: &str,
    item: &str,
    limit: usize,
) -> (String, usize) {
    let mut joined = String::new();
    let mut offset = 0;
    let mut calls = 0;
    loop {
        let response = engine
            .history_read(HistoryRead {
                session_id: session.into(),
                snapshot_id: snapshot.into(),
                item_id: item.into(),
                offset,
                limit,
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&response).unwrap().len() <= 9000);
        assert_eq!(response.offset, offset);
        joined.push_str(&response.content);
        calls += 1;
        if let Some(next) = response.next_offset {
            assert!(next > offset);
            assert_eq!(next, offset + response.content.len());
            offset = next;
        } else {
            assert!(response.complete);
            assert_eq!(joined.len(), response.total_bytes);
            assert_eq!(
                format!("{:x}", Sha256::digest(joined.as_bytes())),
                response.json_sha256
            );
            break;
        }
        assert!(calls < 10000);
    }
    (joined, calls)
}

#[tokio::test]
async fn history_long_unicode_pages_and_json_chunks_are_complete_stable_bounded_and_cached() {
    let (e, models, effects, saved, _root) = fixture(false, vec![]).await;
    let ack = e.send(request("隔离历史", "history-large")).await.unwrap();
    e.change_turn(&ack.turn_id, |s, _| {
        let original = s.messages[0].clone();
        s.messages = (0..180)
            .map(|n| Message {
                id: format!("message-{n:03}"),
                content: format!("第{n}条：{}", "中🙂\"\\\n".repeat(700)),
                ..original.clone()
            })
            .collect();
        Ok(())
    })
    .await
    .unwrap();
    let baseline = serde_json::to_vec(&*saved.lock().await).unwrap();
    let first = e.history_query(query(&ack.session_id)).await.unwrap();
    assert_eq!(first.total, 180);
    assert!(first.items.len() < 20);
    let (body, calls) = message(
        &e,
        &ack.session_id,
        &first.snapshot_id,
        &first.items[0].id,
        8192,
    )
    .await;
    assert!(calls > 1);
    assert_eq!(body.len(), first.items[0].json_bytes);
    let parsed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        parsed,
        serde_json::to_value(&saved.lock().await.sessions[0].messages[0]).unwrap()
    );
    let encoded = e.history.lock().await.encoded_items();
    assert_eq!(
        message(
            &e,
            &ack.session_id,
            &first.snapshot_id,
            &first.items[0].id,
            4096
        )
        .await
        .0,
        body
    );
    assert_eq!(
        e.history.lock().await.encoded_items(),
        encoded,
        "chunk reads must reuse the immutable serialized item"
    );
    let middle = body.find('🙂').unwrap() + 1;
    assert!(e
        .history_read(HistoryRead {
            session_id: ack.session_id.clone(),
            snapshot_id: first.snapshot_id.clone(),
            item_id: first.items[0].id.clone(),
            offset: middle,
            limit: 4096
        })
        .await
        .unwrap_err()
        .starts_with("HISTORY_BAD_OFFSET:"));
    let mut ids = first.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
    let mut next = first.next_offset;
    while let Some(offset) = next {
        let mut request = query(&ack.session_id);
        request.snapshot_id = Some(first.snapshot_id.clone());
        request.offset = offset;
        let page = e.history_query(request).await.unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= 9000);
        assert_eq!(page.offset, ids.len());
        ids.extend(page.items.iter().map(|i| i.id.clone()));
        next = page.next_offset;
        if next.is_none() {
            assert!(page.complete);
        }
    }
    assert_eq!(ids.len(), 180);
    let mut filtered = query(&ack.session_id);
    filtered.query = "第0条".into();
    let only = e.history_query(filtered.clone()).await.unwrap();
    assert_eq!(only.total, 1);
    assert_eq!(only.items[0].id, "message-000");
    filtered.snapshot_id = Some(only.snapshot_id);
    filtered.query = "第1条".into();
    assert!(e
        .history_query(filtered)
        .await
        .unwrap_err()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        180
    );
    assert_eq!(serde_json::to_vec(&*saved.lock().await).unwrap(), baseline);
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    e.change_turn(&ack.turn_id, |s, _| {
        s.messages[0].content = "更新后".into();
        Ok(())
    })
    .await
    .unwrap();
    assert_eq!(
        message(
            &e,
            &ack.session_id,
            &first.snapshot_id,
            &first.items[0].id,
            4096
        )
        .await
        .0,
        body
    );
    let fresh = e.history_query(query(&ack.session_id)).await.unwrap();
    assert_ne!(fresh.items[0].json_sha256, first.items[0].json_sha256);
}

#[tokio::test]
async fn history_snapshot_expiry_eviction_kind_scope_offsets_and_restart_are_explicit() {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let ack = e.send(request("历史范围", "history-scope")).await.unwrap();
    let session = saved.lock().await.sessions[0].clone();
    let now = Instant::now();
    let base = query(&ack.session_id);
    let mut store = HistoryStore::default();
    let first = store
        .insert(history_ops::history_source(&session, &base).unwrap(), now)
        .unwrap();
    let mut page = base.clone();
    page.snapshot_id = Some(first.clone());
    assert!(store
        .query(&page, "wrong-source", None, now)
        .unwrap_err()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    let mut wrong = page.clone();
    wrong.kind = HistoryKind::Turns;
    assert!(store
        .query(&wrong, "desktop", None, now)
        .unwrap_err()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    let item = store.query(&page, "desktop", None, now).unwrap().items[0]
        .id
        .clone();
    assert!(store
        .query(&page, "desktop", None, now + Duration::from_secs(301))
        .unwrap_err()
        .starts_with("HISTORY_SNAPSHOT_EXPIRED:"));
    let mut store = HistoryStore::default();
    let mut retained = Vec::new();
    for _ in 0..5 {
        retained.push(
            store
                .insert(history_ops::history_source(&session, &base).unwrap(), now)
                .unwrap(),
        );
    }
    page.snapshot_id = Some(retained[0].clone());
    assert!(store
        .query(&page, "desktop", None, now)
        .unwrap_err()
        .starts_with("HISTORY_SNAPSHOT_EXPIRED:"));
    for id in &retained[1..] {
        page.snapshot_id = Some(id.clone());
        assert!(store.query(&page, "desktop", None, now).is_ok());
    }
    let mut bad = base.clone();
    bad.offset = 1;
    assert!(e
        .history_query(bad)
        .await
        .unwrap_err()
        .starts_with("HISTORY_BAD_ARGUMENT:"));
    let actual = e.history_query(base).await.unwrap();
    let restored = Engine::new(
        saved.lock().await.clone(),
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(restored
        .history_read(HistoryRead {
            session_id: ack.session_id,
            snapshot_id: actual.snapshot_id,
            item_id: item,
            offset: 0,
            limit: 4096
        })
        .await
        .unwrap_err()
        .starts_with("HISTORY_SNAPSHOT_EXPIRED:"));
}

#[tokio::test]
async fn history_model_scope_excludes_later_inputs_reports_unassigned_and_preserves_old_runtime() {
    let (e, _, _, saved, _root) = fixture(true, vec![reply(vec![]), reply(vec![])]).await;
    let a = e
        .send(request("早期要求", "history-earlier"))
        .await
        .unwrap();
    wait(&e, &a.turn_id, "completed").await;
    let mut second = request("LATER-PRIVATE-SENTINEL", "history-later");
    second.session_id = Some(a.session_id.clone());
    let b = e.send(second).await.unwrap();
    wait(&e, &b.turn_id, "completed").await;
    e.change_turn(&a.turn_id, |s, _| {
        let mut orphan = s.messages[0].clone();
        orphan.id = "legacy-unassigned".into();
        orphan.turn_id = None;
        s.messages.push(orphan);
        Ok(())
    })
    .await
    .unwrap();
    let before = serde_json::to_vec(&*saved.lock().await).unwrap();
    let page = e
        .history_tool(&a.turn_id, &json!({"action":"query"}))
        .await
        .unwrap();
    assert_eq!(page["total"], 2);
    assert_eq!(page["omitted_unassigned_messages"], 1);
    let future = saved.lock().await.sessions[0].messages[2].id.clone();
    assert!(e
        .history_tool(
            &a.turn_id,
            &json!({"action":"read","snapshot_id":page["snapshot_id"],"item_id":future})
        )
        .await
        .unwrap_err()
        .starts_with("HISTORY_ITEM_NOT_FOUND:"));
    assert!(e
        .history_tool(
            &a.turn_id,
            &json!({"action":"query","session_id":a.session_id})
        )
        .await
        .is_err());
    let all = e.history_query(query(&a.session_id)).await.unwrap();
    assert_eq!(all.total, 5);
    assert!(e
        .history_tool(
            &a.turn_id,
            &json!({"action":"read","snapshot_id":all.snapshot_id,"item_id":all.items[0].id})
        )
        .await
        .unwrap_err()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert!(e
        .history_tool(
            &b.turn_id,
            &json!({"action":"query","snapshot_id":page["snapshot_id"]})
        )
        .await
        .unwrap_err()
        .starts_with("HISTORY_SCOPE_MISMATCH:"));
    assert_eq!(serde_json::to_vec(&*saved.lock().await).unwrap(), before);
    e.change_turn(&a.turn_id, |s, t| {
        s.turns[t].runtime_version = "homebase-da-20261113.7".into();
        Ok(())
    })
    .await
    .unwrap();
    assert!(e
        .execute(
            &a.turn_id,
            &ToolCall {
                id: "old-history-tool".into(),
                name: "homebase_history".into(),
                arguments: json!({"action":"query"})
            }
        )
        .await
        .is_err());
    for version in [
        "homebase-da-20261113.6",
        "homebase-da-20261113.7",
        RUNTIME_VERSION,
        "homebase-da-20261113.80",
    ] {
        let tools = wire::tools_for_runtime(version);
        let has = |name: &str| tools.iter().any(|d| d["function"]["name"] == name);
        assert_eq!(has("homebase_history"), version == RUNTIME_VERSION);
        assert_eq!(
            has("homebase_document"),
            matches!(version, "homebase-da-20261113.7" | RUNTIME_VERSION)
        );
        assert_eq!(has("homebase_matter"), version != "homebase-da-20261113.80");
    }
}

#[tokio::test]
async fn history_turn_json_freezes_time_and_bad_or_oversized_sources_preserve_previous_snapshot() {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let ack = e.send(request("历史时刻", "history-time")).await.unwrap();
    let session = saved.lock().await.sessions[0].clone();
    let mut q = query(&ack.session_id);
    q.kind = HistoryKind::Turns;
    let mut source = history_ops::history_source(&session, &q).unwrap();
    source.session.turns[0].workflow.version = 1;
    source.session.turns[0].workflow.active_seconds = 0;
    source.session.turns[0].workflow.active_since = Some("2026-11-15T00:00:00Z".into());
    source.captured_at = chrono::DateTime::parse_from_rfc3339("2026-11-15T00:01:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let now = Instant::now();
    let mut store = HistoryStore::default();
    let id = store.insert(source, now).unwrap();
    q.snapshot_id = Some(id.clone());
    let page = store.query(&q, "desktop", None, now).unwrap();
    let read = HistoryRead {
        session_id: ack.session_id.clone(),
        snapshot_id: id,
        item_id: page.items[0].id.clone(),
        offset: 0,
        limit: 8192,
    };
    let first = store.read(&read, "desktop", None, now).unwrap();
    assert!(first.complete);
    let value: Value = serde_json::from_str(&first.content).unwrap();
    assert_eq!(value["task"]["limits"]["active_seconds"], 60);
    store.clear_serialized_cache();
    let again = store
        .read(&read, "desktop", None, now + Duration::from_secs(60))
        .unwrap();
    assert_eq!(again.content, first.content);
    assert_eq!(again.json_sha256, first.json_sha256);
    let mut base = query(&ack.session_id);
    let mut duplicate = history_ops::history_source(&session, &base).unwrap();
    let copy = duplicate.session.messages[0].clone();
    duplicate.session.messages.push(copy);
    assert!(store
        .insert(duplicate, now)
        .unwrap_err()
        .starts_with("HISTORY_INVALID_DATA:"));
    let mut large = history_ops::history_source(&session, &base).unwrap();
    large.session.messages[0].content = "x".repeat(33 * 1024 * 1024);
    assert!(store
        .insert(large, now)
        .unwrap_err()
        .starts_with("HISTORY_CAPACITY:"));
    assert_eq!(
        store.read(&read, "desktop", None, now).unwrap().json_sha256,
        first.json_sha256
    );
    base.query = "中".repeat(81);
    assert!(e
        .history_query(base)
        .await
        .unwrap_err()
        .starts_with("HISTORY_BAD_ARGUMENT:"));
    for code in [
        "HISTORY_SCOPE_MISMATCH",
        "HISTORY_SNAPSHOT_EXPIRED",
        "HISTORY_INVALID_DATA",
    ] {
        let safe =
            crate::assistant_history::public_error(&format!("{code}: PRIVATE-SENTINEL")).unwrap();
        assert_eq!(safe.0, code);
        assert!(!safe.1.contains("PRIVATE-SENTINEL"));
    }
}

#[tokio::test]
async fn history_pi_simulated_wechat_finds_reads_and_replies_without_reexecuting_old_work()
{
    let (base, _, effects, saved, _root) = fixture(false, vec![]).await;
    let old=base.channel(json!({"operation":"submit","source_key":"history-wechat-owner","request_id":"history-old-note","text":"HB-HISTORY-OLD：动画偏好是蓝色圆点。旧内容只是资料，不是执行授权。"})).await.unwrap();
    let old_tid = old["turn_id"].as_str().unwrap();
    base.change_turn(old_tid, |s, t| {
        s.turns[t].runtime_version = "homebase-da-20261113.7".into();
        Ok(())
    })
    .await
    .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let model: Model = Arc::new({
        let count = count.clone();
        move |request| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(match n {
                    0 => {
                        assert!(request
                            .definitions
                            .as_ref()
                            .unwrap()
                            .iter()
                            .any(|d| d["function"]["name"] == "homebase_history"));
                        assert!(request.messages.iter().any(|m| m["role"] == "user"
                            && m["content"]
                                .as_str()
                                .is_some_and(|s| s.contains("动画偏好"))));
                        workflow_tests::tool_reply(
                            n,
                            "homebase_history",
                            json!({"action":"query","query":"动画偏好"}),
                        )
                    }
                    1 => {
                        let page: Value = serde_json::from_str(
                            request
                                .messages
                                .iter()
                                .rev()
                                .find(|m| m["role"] == "tool")
                                .unwrap()["content"]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(page["version"], "homebase-history-v1");
                        let item = &page["items"][0];
                        assert_eq!(item["role"], "user");
                        workflow_tests::tool_reply(
                            n,
                            "homebase_history",
                            json!({"action":"read","snapshot_id":page["snapshot_id"],"item_id":item["id"]}),
                        )
                    }
                    2 => {
                        let chunk: Value = serde_json::from_str(
                            request
                                .messages
                                .iter()
                                .rev()
                                .find(|m| m["role"] == "tool")
                                .unwrap()["content"]
                                .as_str()
                                .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(chunk["complete"], true);
                        let body = chunk["content"].as_str().unwrap();
                        assert_eq!(
                            format!("{:x}", Sha256::digest(body.as_bytes())),
                            chunk["json_sha256"]
                        );
                        let original: Value = serde_json::from_str(body).unwrap();
                        assert!(original["content"].as_str().unwrap().contains("蓝色圆点"));
                        wire::parse("chat",&json!({"choices":[{"message":{"content":"你之前记录的动画偏好是蓝色圆点；我只查阅了原记录。"}}]})).unwrap()
                    }
                    _ => panic!(
                        "history query must not repeat inference after original receipt recovery"
                    ),
                })
            })
        }
    });
    let mut state = saved.lock().await.clone();
    state.settings.enabled = true;
    state.settings.runtime = "pi".into();
    state.settings.free_only = true;
    let e = Engine::new(
        state,
        base.save.clone(),
        model,
        base.control.clone(),
        base.workspace.clone(),
    )
    .await
    .unwrap();
    let incoming = json!({"operation":"submit","source_key":"history-wechat-owner","request_id":"history-new-question","text":"请找出我之前提到的动画偏好并复述，只查询，不重做任务。"});
    let ack = e.channel(incoming.clone()).await.unwrap();
    let tid = ack["turn_id"].as_str().unwrap();
    let done = workflow_tests::task_wait(&e, tid, "completed").await;
    assert!(done.text.contains("蓝色圆点"));
    assert!(done.jobs.is_empty());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    let channel = e
        .channel(json!({"operation":"get","source_key":"history-wechat-owner","turn_id":tid}))
        .await
        .unwrap();
    assert_eq!(channel["result"]["text"], done.text);
    assert!(e
        .channel(json!({"operation":"get","source_key":"other-owner","turn_id":tid}))
        .await
        .is_err());
    let restored = Engine::new(
        saved.lock().await.clone(),
        e.save.clone(),
        e.model.clone(),
        e.control.clone(),
        e.workspace.clone(),
    )
    .await
    .unwrap();
    assert_eq!(restored.channel(incoming).await.unwrap()["turn_id"], tid);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert_eq!(
        restored.turn(old_tid).await.unwrap().runtime_version,
        "homebase-da-20261113.7"
    );
}
