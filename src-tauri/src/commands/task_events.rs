/// 查询当前实例捕获的流式任务事件；只读，不恢复或改写任务。
#[tauri::command]
pub fn task_events_get(
    topic: String,
    task_id: String,
    after_seq: Option<u64>,
    limit: Option<usize>,
) -> Result<crate::task_event_store::TaskEventPage, String> {
    crate::task_events::page(
        &topic,
        &task_id,
        after_seq.unwrap_or(0),
        limit.unwrap_or(100),
    )
}
