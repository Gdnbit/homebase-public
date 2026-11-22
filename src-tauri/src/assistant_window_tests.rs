//! Synthetic long timelines; all reads must remain complete, bounded and passive.
use super::*;
use window::TimelineRequest;

#[test]
fn window_public_error_codes_are_specific_and_never_echo_private_cursor_or_content() {
    for code in [
        "WINDOW_BAD_ARGUMENT",
        "WINDOW_SESSION_NOT_FOUND",
        "WINDOW_IDENTITY_CONFLICT",
        "WINDOW_CURSOR_NOT_FOUND",
        "WINDOW_INVALID_DATA",
        "WINDOW_ENTRY_TOO_LARGE",
    ] {
        let (actual, message) =
            window::public_error(&format!("{code}: private-sentinel-content")).unwrap();
        assert_eq!(actual, code);
        assert!(!message.contains("private-sentinel"));
        assert!(!message.is_empty());
    }
    assert!(window::public_error("other: WINDOW_CURSOR_NOT_FOUND: private").is_none());
}

fn window_request(id: &str, before: Option<String>) -> TimelineRequest {
    TimelineRequest {
        session_id: id.into(),
        before_message_id: before,
        limit: 40,
    }
}

#[tokio::test]
async fn window_long_unicode_pages_return_full_text_once_without_whole_checkpoint_or_state_writes()
{
    let (e, models, effects, saved, _root) = fixture(false, vec![]).await;
    let ack = e.send(request("窗口长历史", "window-long")).await.unwrap();
    e.change_turn(&ack.turn_id, |s, t| {
        s.messages = (0..180)
            .map(|n| Message {
                id: format!("window-msg-{n}"),
                role: if n % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("{n} 中文🙂引号\"反斜杠\\换行\n").repeat(600),
                created_at: now(),
                turn_id: if n % 2 == 1 {
                    Some(ack.turn_id.clone())
                } else {
                    None
                },
                attachments: vec![],
            })
            .collect();
        s.turns[t].steps.push(Step {
            id: "hidden-checkpoint".into(),
            name: "homebase_call".into(),
            state: "completed".into(),
            arguments: json!({"private-checkpoint":"hidden-body".repeat(30000)}),
            result: Value::Null,
            job_id: None,
        });
        Ok(())
    })
    .await
    .unwrap();
    let before = serde_json::to_value(saved.lock().await.clone()).unwrap();
    let original = e.session(&ack.session_id).await.unwrap();
    let mut cursor = None;
    let mut pages = vec![];
    loop {
        let result = e
            .session_window(window_request(&ack.session_id, cursor))
            .await
            .unwrap();
        let bytes = serde_json::to_vec(&result).unwrap();
        assert!(bytes.len() <= 256 * 1024);
        assert!(!String::from_utf8_lossy(&bytes).contains("private-checkpoint"));
        assert_eq!(result.page.total_messages, 180);
        assert_eq!(result.page.returned, result.messages.len());
        assert_eq!(
            result.page.has_more,
            result.page.before_message_id.is_some()
        );
        for m in &result.messages {
            assert_eq!(
                m.content,
                original
                    .messages
                    .iter()
                    .find(|old| old.id == m.id)
                    .unwrap()
                    .content
            );
        }
        cursor = result.page.before_message_id.clone();
        pages.push(result.messages);
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        pages.len() > 5,
        "actual byte budget may return fewer than40 full messages"
    );
    let ids: Vec<_> = pages.into_iter().rev().flatten().map(|m| m.id).collect();
    assert_eq!(
        ids,
        original
            .messages
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        serde_json::to_value(saved.lock().await.clone()).unwrap(),
        before
    );
    assert_eq!(models.load(Ordering::SeqCst), 0);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn window_active_task_outside_page_keeps_identity_but_never_exposes_partial_confirmation() {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let ack = e
        .send(request("窗口原审批", "window-pending"))
        .await
        .unwrap();
    e.change_turn(&ack.turn_id, |s, t| {
        s.turns[t].state = "awaiting_confirmation".into();
        s.turns[t].pending = Some(Pending {
            id: "approval-original".into(),
            tool_id: "tool-original".into(),
            description: "原审批".into(),
            arguments: json!({"original_parameters":"private-confirmation-sentinel"}),
            approved: false,
        });
        s.turns[t].steps.push(Step {
            id: "step-original".into(),
            name: "homebase_files".into(),
            state: "running".into(),
            arguments: json!({"original":true}),
            result: Value::Null,
            job_id: None,
        });
        for n in 0..60 {
            s.messages.push(Message {
                id: format!("later-{n}"),
                role: "user".into(),
                content: format!("晚些记录{n}"),
                created_at: now(),
                turn_id: None,
                attachments: vec![],
            });
        }
        Ok(())
    })
    .await
    .unwrap();
    let result = e
        .session_window(TimelineRequest {
            limit: 2,
            ..window_request(&ack.session_id, None)
        })
        .await
        .unwrap();
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.turns[0].turn.id, ack.turn_id);
    assert_eq!(result.turns[0].step_count, 1);
    assert!(result.turns[0].details_omitted);
    assert!(result.turns[0].turn.pending.is_none());
    assert!(result.turns[0].turn.steps.is_empty());
    assert!(!serde_json::to_string(&result)
        .unwrap()
        .contains("private-confirmation-sentinel"));
    let full = e.turn(&ack.turn_id).await.unwrap();
    assert_eq!(
        full.pending.unwrap().arguments["original_parameters"],
        "private-confirmation-sentinel"
    );
    assert_eq!(full.steps.len(), 1);
    assert!(
        !saved.lock().await.sessions[0].turns[0]
            .pending
            .as_ref()
            .unwrap()
            .approved
    );
}

#[tokio::test]
async fn window_missing_cursor_bad_scope_duplicate_ids_and_oversized_records_are_explicit() {
    let (e, _, _, saved, _root) = fixture(false, vec![]).await;
    let ack = e.send(request("窗口边界", "window-errors")).await.unwrap();
    assert!(e
        .session_window(window_request("missing", None))
        .await
        .err()
        .unwrap()
        .starts_with("WINDOW_SESSION_NOT_FOUND:"));
    assert!(e
        .session_window(window_request(&ack.session_id, Some("missing".into())))
        .await
        .err()
        .unwrap()
        .starts_with("WINDOW_CURSOR_NOT_FOUND:"));
    assert!(e
        .session_window(TimelineRequest {
            limit: 0,
            ..window_request(&ack.session_id, None)
        })
        .await
        .err()
        .unwrap()
        .starts_with("WINDOW_BAD_ARGUMENT:"));
    e.change_turn(&ack.turn_id, |s, _| {
        let old = s.messages[0].clone();
        s.messages.push(old);
        Ok(())
    })
    .await
    .unwrap();
    assert!(e
        .session_window(window_request(&ack.session_id, None))
        .await
        .err()
        .unwrap()
        .starts_with("WINDOW_IDENTITY_CONFLICT:"));
    e.change_turn(&ack.turn_id, |s, _| {
        s.messages = s.messages.iter().take(2).cloned().collect();
        s.messages[1].content = "合成大正文".repeat(100000);
        Ok(())
    })
    .await
    .unwrap();
    let before = serde_json::to_value(saved.lock().await.clone()).unwrap();
    assert!(e
        .session_window(window_request(&ack.session_id, None))
        .await
        .err()
        .unwrap()
        .starts_with("WINDOW_ENTRY_TOO_LARGE:"));
    assert_eq!(
        serde_json::to_value(saved.lock().await.clone()).unwrap(),
        before
    );
}
