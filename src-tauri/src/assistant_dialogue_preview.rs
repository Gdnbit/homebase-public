//! In-memory visible text scoped to an original model round.
use super::{DialogueNotice, TextDelta};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Preview {
    text: String,
    revision: u64,
    notified: Option<Instant>,
}

#[derive(Clone, Default)]
pub(super) struct Previews(Arc<Mutex<BTreeMap<String, Preview>>>);

pub(super) struct PreviewGuard {
    previews: Previews,
    id: String,
    session: String,
    notify: DialogueNotice,
}

impl Drop for PreviewGuard {
    fn drop(&mut self) {
        let visible = self.previews.0.lock().ok()
            .and_then(|mut previews| previews.remove(&self.id))
            .is_some_and(|preview| !preview.text.is_empty());
        if visible {
            (self.notify)(&self.session, &self.id);
        }
    }
}

impl Previews {
    pub(super) fn start(
        &self,
        session: &str,
        turn: &str,
        revision: u64,
        notify: DialogueNotice,
    ) -> (PreviewGuard, TextDelta) {
        if let Ok(mut previews) = self.0.lock() {
            previews.insert(turn.into(), Preview { revision, ..Preview::default() });
        }
        let guard = PreviewGuard {
            previews: self.clone(),
            id: turn.into(),
            session: session.into(),
            notify: notify.clone(),
        };
        let previews = self.clone();
        let session = session.to_string();
        let turn = turn.to_string();
        let delta: TextDelta = Arc::new(move |text| {
            let announced = {
                let Ok(mut previews) = previews.0.lock() else {
                    return;
                };
                let Some(preview) = previews.get_mut(&turn) else {
                    return;
                };
                if text.is_empty()
                    || preview.text.len() + text.len()
                        > crate::assistant_protocol::MAX_REPLY_BYTES
                {
                    return;
                }
                preview.text.push_str(text);
                let now = Instant::now();
                if preview
                    .notified
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(175))
                {
                    preview.notified = Some(now);
                    true
                } else {
                    false
                }
            };
            if announced {
                notify(&session, &turn);
            }
        });
        (guard, delta)
    }

    pub(super) fn text(&self, turn: &str, revision: u64) -> Option<String> {
        let previews = self.0.lock().ok()?;
        let preview = previews.get(turn)?;
        (preview.revision == revision && !preview.text.is_empty()).then(|| preview.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn model_fragments_are_hidden_after_requirements_change_and_removed_on_drop() {
        let previews = Previews::default();
        let updates = Arc::new(AtomicUsize::new(0));
        let count = updates.clone();
        let notify: DialogueNotice = Arc::new(move |session, turn| {
            assert_eq!((session, turn), ("synthetic-session", "synthetic-turn"));
            count.fetch_add(1, Ordering::SeqCst);
        });
        let (guard, delta) = previews.start("synthetic-session", "synthetic-turn", 3, notify);
        delta("合成旧要求的片段");
        assert_eq!(previews.text("synthetic-turn", 3).as_deref(), Some("合成旧要求的片段"));
        assert!(previews.text("synthetic-turn", 4).is_none());
        drop(guard);
        assert!(previews.text("synthetic-turn", 3).is_none());
        assert_eq!(updates.load(Ordering::SeqCst), 2);
    }
}
