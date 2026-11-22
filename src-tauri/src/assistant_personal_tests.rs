use super::*;
fn moment(s: &str) -> DateTime<Utc> {
    timestamp(s).unwrap()
}
#[test]
fn correction_shared_tool_metadata_is_checked_before_approval_without_discarding_changes() {
    let at=moment("2026-11-13T10:00:00Z");let mut p=Personal::default();
    let old=p.save_memory(MemoryInput{revision:0,id:None,title:"测试偏好".into(),content:"红点".into(),expires_at:Some("2026-11-14T10:00:00Z".into())},at).unwrap();
    let args=serde_json::json!({"action":"correct","title":old.title,"revision":1,"id":old.id,"expected_sha256":memory_fingerprint(&old),"content":"蓝点","reason":"用户纠正"});
    assert!(p.correction_input(args.clone(),at).is_ok());assert_eq!(p.memories[0].content,"红点");assert_eq!(p.revision,1);
    let mut change=args.clone();change["title"]=serde_json::json!("其他标题");assert!(p.correction_input(change,at).is_err());
    let mut change=args.clone();change["expires_at"]=serde_json::Value::Null;assert!(p.correction_input(change,at).is_err());
    let mut change=args.clone();change["revision"]=serde_json::json!(0);assert!(p.correction_input(change,at).is_err());
    let mut change=args;change["hidden_change"]=serde_json::json!(true);assert!(p.correction_input(change,at).is_err());
}
#[test]
fn corrections_keep_identity_expiry_provenance_and_reject_stale_or_expired_bases() {
    let mut p=Personal::default();let at=moment("2026-11-13T10:00:00+08:00");
    let old=p.save_memory(MemoryInput{revision:0,id:None,title:"合成动画偏好".into(),content:"红点".into(),expires_at:Some("2026-11-14T10:00:00+08:00".into())},at).unwrap();
    let input=MemoryCorrectionInput{revision:1,id:old.id.clone(),expected_sha256:memory_fingerprint(&old),content:"蓝色圆点，简短中文字幕".into(),reason:"用户纠正了颜色".into()};
    let corrected=p.correct_memory(input.clone(),MemoryOrigin{source:"wechat".into(),turn_id:Some("synthetic-turn".into())},at+chrono::Duration::seconds(1)).unwrap();
    assert_eq!(corrected.id,old.id);assert_eq!(corrected.expires_at,old.expires_at);assert_eq!(p.memories.len(),1);
    assert_eq!(corrected.corrections[0].previous_content,"红点");assert_eq!(corrected.corrections[0].origin.source,"wechat");
    assert_eq!(p.search("蓝色",at)[0].content,corrected.content);
    assert!(p.correct_memory(input.clone(),MemoryOrigin{source:"desktop".into(),turn_id:None},at).err().unwrap().contains("MEMORY_STALE"));
    let mut stale=input;stale.revision=2;
    assert!(p.correct_memory(stale,MemoryOrigin{source:"desktop".into(),turn_id:None},at).err().unwrap().contains("MEMORY_STALE"));
    let expired=MemoryCorrectionInput{revision:2,id:corrected.id.clone(),expected_sha256:memory_fingerprint(&corrected),content:"绿点".into(),reason:"合成".into()};
    assert!(p.correct_memory(expired,MemoryOrigin{source:"wechat".into(),turn_id:None},at+chrono::Duration::days(2)).err().unwrap().contains("到期"));
    assert_eq!(p.revision,2);assert_eq!(p.memories[0].content,corrected.content);
    let restored:Personal=serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();assert_eq!(restored.memories[0].corrections.len(),1);
}
#[test]
fn personal_memory_expiry_revisions_and_credentials_are_explicit() {
    let mut state = Personal::default();
    let now = moment("2026-11-12T10:00:00+08:00");
    let input = MemoryInput {
        revision: 0,
        id: None,
        title: "回答风格".into(),
        content: "简短中文".into(),
        expires_at: Some("2026-11-12T10:01:00+08:00".into()),
    };
    state.save_memory(input.clone(), now).unwrap();
    assert_eq!(state.search("中文", now).len(), 1);
    assert!(state
        .search("", moment("2026-11-12T10:02:00+08:00"))
        .is_empty());
    assert!(state.tick(moment("2026-11-12T10:02:00+08:00")));
    assert!(!state.tick(moment("2026-11-12T10:03:00+08:00")));
    assert!(state.save_memory(input, now).is_err());
    assert!(state
        .save_memory(
            MemoryInput {
                revision: state.revision,
                id: None,
                title: "禁止".into(),
                content: "api_key=synthetic".into(),
                expires_at: None
            },
            now
        )
        .is_err());
    assert_eq!(state.memories.len(), 1);
}
#[test]
fn personal_reminders_fire_once_and_recurrence_skips_offline_backlog() {
    let mut state = Personal::default();
    let r = state
        .save_reminder(
            ReminderInput {
                revision: 0,
                id: None,
                text: "合成提醒".into(),
                due_at: "2026-11-12T10:00:00+08:00".into(),
                interval_minutes: Some(60),
                enabled: true,
            },
            "wechat",
            "synthetic-owner",
        )
        .unwrap();
    let now = moment("2026-11-12T13:30:00+08:00");
    assert!(state.tick(now));
    assert!(!state.tick(now));
    assert_eq!(state.reminders[0].delivery, "pending");
    state.acknowledge(&r.id, "claimed", now).unwrap();
    assert!(!state.tick(now));
    state.acknowledge(&r.id, "unknown", now).unwrap();
    assert!(!state.tick(now));
    state.acknowledge(&r.id, "dismissed", now).unwrap();
    assert_eq!(
        timestamp(&state.reminders[0].due_at).unwrap(),
        moment("2026-11-12T14:00:00+08:00")
    );
    assert!(!state.tick(now));
    state.acknowledge(&r.id, "canceled", now).unwrap();
    assert!(!state.tick(moment("2026-11-12T16:00:00+08:00")));
}
#[test]
fn personal_reminder_edit_keeps_original_channel_and_utc_offsets() {
    let mut state = Personal::default();
    let mut r = ReminderInput {
        revision: 0,
        id: None,
        text: "合成".into(),
        due_at: "2026-11-12T10:00:00+08:00".into(),
        interval_minutes: None,
        enabled: true,
    };
    let stored = state.save_reminder(r.clone(), "wechat", "owner-a").unwrap();
    r.revision = 1;
    r.id = Some(stored.id);
    r.text = "修改".into();
    state.save_reminder(r, "desktop", "desktop").unwrap();
    assert_eq!(state.reminders[0].source_key, "owner-a");
    assert_eq!(state.reminders[0].source, "wechat");
    assert_eq!(
        timestamp(&state.reminders[0].due_at).unwrap(),
        moment("2026-11-12T02:00:00Z")
    );
}
