use crate::task_event_store::{Journal, TaskEventPage};
use serde::Serialize;
use std::sync::{Mutex, OnceLock};
use tauri::Emitter;
fn journal() -> &'static Mutex<Journal> {
    static STORE: OnceLock<Mutex<Journal>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Journal::default()))
}
pub trait TrackedEmitter {
    fn emit_tracked<S: Serialize + Clone>(&self, event: &str, payload: S) -> tauri::Result<()>;
}
impl TrackedEmitter for tauri::AppHandle {
    fn emit_tracked<S: Serialize + Clone>(&self, event: &str, payload: S) -> tauri::Result<()> {
        if let Ok(value) = serde_json::to_value(&payload) {
            if let Ok(mut store) = journal().lock() {
                store.record(event, value);
            }
        }
        self.emit(event, payload)
    }
}
pub fn stream_closed(topic: &str, id: &str) {
    if let Ok(mut store) = journal().lock() {
        store.stream_closed(topic, id);
    }
}
pub fn stream_error(topic: &str, id: &str) {
    if let Ok(mut store) = journal().lock() {
        store.stream_error(topic, id);
    }
}
pub fn page(topic: &str, id: &str, after: u64, limit: usize) -> Result<TaskEventPage, String> {
    if !["studio:event", "wiki-agent:event", "gdnbit:event"].contains(&topic)
        || id.is_empty()
        || id.len() > 160
    {
        return Err("事件主题或任务编号无效".into());
    }
    journal()
        .lock()
        .map_err(|_| "任务事件存储不可读取".into())
        .map(|store| store.page(topic, id, after, limit))
}
