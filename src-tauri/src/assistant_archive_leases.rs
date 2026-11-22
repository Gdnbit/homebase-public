//! Scoped protection for bodies restored for writable operations, not authority.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub(super) struct Registry(Arc<Mutex<BTreeMap<String, usize>>>);

#[must_use = "keep this protection until the operation finishes using the restored body"]
pub(super) struct Lease {
    registry: Registry,
    session: String,
}

impl Registry {
    pub(super) fn pin(&self, session: &str) -> Result<Lease, String> {
        let mut counts = self
            .0
            .lock()
            .map_err(|_| "ARCHIVE_BUSY: 正文使用保护无法核对，原状态保留")?;
        let count = counts.entry(session.into()).or_default();
        *count = count
            .checked_add(1)
            .ok_or("ARCHIVE_BUSY: 正文使用保护达到边界")?;
        Ok(Lease {
            registry: self.clone(),
            session: session.into(),
        })
    }

    pub(super) fn protected(&self) -> Result<Vec<String>, String> {
        self.0
            .lock()
            .map(|counts| counts.keys().cloned().collect())
            .map_err(|_| "ARCHIVE_BUSY: 正文使用保护无法核对，未驱逐记录".into())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // A poisoned registry remains unavailable to compaction. Never recover
        // it by assuming a still-borrowed body is safe to discard.
        if let Ok(mut counts) = self.registry.0.lock() {
            if let Some(count) = counts.get_mut(&self.session) {
                if *count > 1 {
                    *count -= 1;
                } else {
                    counts.remove(&self.session);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_and_unwound_uses_release_only_their_own_protection() {
        let registry = Registry::default();
        let first = registry.pin("original").unwrap();
        let second = registry.pin("original").unwrap();
        let other = registry.pin("other").unwrap();
        drop(first);
        assert_eq!(registry.protected().unwrap(), vec!["original", "other"]);
        let copy = registry.clone();
        let failed = std::panic::catch_unwind(move || {
            let _third = copy.pin("original").unwrap();
            panic!("synthetic operation failure");
        });
        assert!(failed.is_err());
        assert_eq!(registry.protected().unwrap(), vec!["original", "other"]);
        drop(second);
        assert_eq!(registry.protected().unwrap(), vec!["other"]);
        drop(other);
        assert!(registry.protected().unwrap().is_empty());
    }
}
