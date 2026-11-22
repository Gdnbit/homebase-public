use super::*;

#[tokio::test]
async fn record_engine_transactions_keep_old_snapshots_on_update_and_save_failure() {
    let (engine, models, executions, saved, _root) =
        fixture(true, vec![reply(vec![]), reply(vec![])]).await;
    let first = engine
        .send(request("合成旧记录一", "records-original-a"))
        .await
        .unwrap();
    wait(&engine, &first.turn_id, "completed").await;
    let mut next = request("合成旧记录二", "records-original-b");
    next.session_id = Some(first.session_id.clone());
    let second = engine.send(next).await.unwrap();
    wait(&engine, &second.turn_id, "completed").await;
    engine
        .change_turn(&first.turn_id, |s, _| {
            for (id, turn) in [(&"job-a", &first.turn_id), (&"job-b", &second.turn_id)] {
                s.jobs.push(Job {
                    archived_result_bytes: None,
                    id: (*id).into(),
                    command: "system_info".into(),
                    turn_id: turn.clone(),
                    state: "succeeded".into(),
                    result: json!({"status":"succeeded","fixture":"original","text":"合成🙂".repeat(16 * 1024)}),
                    owned: true,
                    deliver: false,
                    artifacts: vec![],
                    remote: None,
                });
            }
            Ok(())
        })
        .await
        .unwrap();
    let before = saved.lock().await.clone();
    let bytes = serde_json::to_vec(&before).unwrap();
    let original_view = engine.turn(&first.turn_id).await.unwrap();
    assert_eq!(original_view.jobs.len(), 1);
    assert_eq!(original_view.jobs[0].id, "job-a");
    assert!(std::ptr::eq(
        &original_view.jobs[0],
        &before.sessions[0].jobs[0]
    ));
    let serialized = serde_json::to_value(&original_view).unwrap();
    assert_eq!(serialized["jobs"], json!([&before.sessions[0].jobs[0]]));
    let summary_view = view_summary_at(
        &before.sessions[0],
        &before.sessions[0].turns[0],
        chrono::Utc::now(),
    );
    assert_eq!(summary_view.jobs.len(), 1);
    assert!(summary_view.jobs[0].result.is_null());
    let failed: Result<(), String> = engine
        .change_turn(&first.turn_id, |s, t| {
            s.turns[t].text = "not-committed".into();
            s.messages[0].content = "not-committed".into();
            s.jobs[0].result["fixture"] = json!("not-committed");
            Err("synthetic update rejected".into())
        })
        .await;
    assert!(failed.is_err());
    assert_eq!(
        serde_json::to_vec(&*engine.state.lock().await).unwrap(),
        bytes
    );
    assert_eq!(serde_json::to_vec(&*saved.lock().await).unwrap(), bytes);

    let fail_save: Save =
        Arc::new(|_| Box::pin(async { Err("synthetic persistence rejected".into()) }));
    let restarted = Engine::new(
        before.clone(),
        fail_save,
        engine.model.clone(),
        engine.control.clone(),
        engine.workspace.clone(),
    )
    .await
    .unwrap();
    assert!(restarted
        .change_turn(&first.turn_id, |s, t| {
            s.turns[t].text = "failed-save".into();
            s.messages[0].content = "failed-save".into();
            s.jobs[0].result["fixture"] = json!("failed-save");
            Ok(())
        })
        .await
        .is_err());
    assert_eq!(
        serde_json::to_vec(&*restarted.state.lock().await).unwrap(),
        bytes
    );
    assert_eq!(
        serde_json::to_vec(&*engine.state.lock().await).unwrap(),
        bytes
    );
    assert_eq!(serde_json::to_vec(&*saved.lock().await).unwrap(), bytes);

    engine
        .change_turn(&first.turn_id, |s, t| {
            s.turns[t].text = "已提交的隔离更新🙂".into();
            s.messages
                .find_mut(|m| {
                    m.role == "assistant" && m.turn_id.as_deref() == Some(first.turn_id.as_str())
                })
                .unwrap()
                .content = "已提交的隔离更新🙂".into();
            s.jobs.find_mut(|job| job.id == "job-a").unwrap().result["fixture"] =
                json!("committed");
            Ok(())
        })
        .await
        .unwrap();
    let after = saved.lock().await.clone();
    assert_eq!(after.sessions[0].turns[0].text, "已提交的隔离更新🙂");
    assert_eq!(after.sessions[0].jobs[0].result["fixture"], "committed");
    let updated_view = engine.turn(&first.turn_id).await.unwrap();
    assert!(std::ptr::eq(
        &updated_view.jobs[0],
        &after.sessions[0].jobs[0]
    ));
    assert_eq!(updated_view.jobs[0].result["fixture"], "committed");
    assert_eq!(serde_json::to_value(&original_view).unwrap(), serialized);
    assert_eq!(
        serde_json::to_vec(&before).unwrap(),
        bytes,
        "previous snapshots must remain immutable after a successful later transaction"
    );
    assert!(
        std::ptr::eq(&before.sessions[0].turns[1], &after.sessions[0].turns[1]),
        "untouched history should not be copied"
    );
    assert!(std::ptr::eq(
        &before.sessions[0].jobs[1],
        &after.sessions[0].jobs[1]
    ));
    assert!(std::ptr::eq(
        &before.sessions[0].messages[3],
        &after.sessions[0].messages[3]
    ));
    assert_eq!(models.load(Ordering::SeqCst), 2);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
