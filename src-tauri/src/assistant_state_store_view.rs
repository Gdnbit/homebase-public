//! Scoped immutable data access. No writable Store or full-profile snapshot escapes this type.
use super::*;

pub(crate) struct RecordView {
    reader: std::sync::Mutex<Store>,
    snapshot: Snapshot,
    session: String,
    charge: usize,
}
impl Store {
    pub(crate) fn record_view(
        &mut self,
        snapshot: &Snapshot,
        session: &str,
    ) -> Result<Arc<RecordView>, String> {
        if !self.loaded || snapshot.root != self.root {
            return Err(error("ARCHIVE_SCOPE"));
        }
        self.check_current()?;
        let owner = snapshot.index.metadata["sessions"]
            .as_array()
            .and_then(|rows| rows.iter().find(|s| s["id"].as_str() == Some(session)))
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        let metadata = serde_json::json!({
            "archive_capacity_version": if snapshot.capacity_segmented() {1} else {0},
            "sessions":[{"id":owner["id"],"source":owner["source"],"source_key":owner["source_key"],
                "messages":[],"turns":[],"jobs":[],"history":[]}]
        });
        let index = Index {
            format: snapshot.index.format,
            generation: snapshot.index.generation.clone(),
            anchor_sha256: snapshot.index.anchor_sha256.clone(),
            metadata,
            series: snapshot
                .index
                .series
                .iter()
                .filter(|s| s.session_id == session)
                .cloned()
                .collect(),
        };
        validate(&index)?;
        self.record_views.retain(|_, view| view.strong_count() > 0);
        let key = (snapshot.generation().to_string(), session.to_string());
        if let Some(view) = self
            .record_views
            .get(&key)
            .and_then(std::sync::Weak::upgrade)
        {
            if view.snapshot.index.as_ref() != &index {
                return Err(error("ARCHIVE_SCOPE"));
            }
            return Ok(view);
        }
        budget(&index, CIPHER_LIMIT)?;
        let charge = serde_json::to_vec(&index)
            .map_err(|_| error("ARCHIVE_INVALID"))?
            .len();
        let mut reader = Store::new(self.root.clone(), self.legacy.clone())?;
        // All views share the original verified 32MiB/8-segment cache. No per-view body pool.
        reader.cache = self.cache.clone();
        reader.read_observer = Some(self.view_reads.clone());
        let view = Arc::new(RecordView {
            reader: std::sync::Mutex::new(reader),
            snapshot: Snapshot {
                root: self.root.clone(),
                index: Arc::new(index),
            },
            session: session.into(),
            charge,
        });
        self.record_views.insert(key, Arc::downgrade(&view));
        Ok(view)
    }
}
impl RecordView {
    pub(crate) fn ids(&self, kind: Kind) -> Result<Vec<String>, String> {
        self.snapshot.record_ids(&self.session, kind)
    }
    pub(crate) fn count(&self, kind: Kind) -> Result<usize, String> {
        self.snapshot.count(&self.session, kind)
    }
    pub(crate) fn range(
        &self,
        kind: Kind,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        self.reader
            .lock()
            .map_err(|_| error("ARCHIVE_INVALID"))?
            .read_range(&self.snapshot, &self.session, kind, offset, limit)
    }
    pub(crate) fn record(&self, kind: Kind, id: &str) -> Result<Value, String> {
        self.reader
            .lock()
            .map_err(|_| error("ARCHIVE_INVALID"))?
            .read_record(&self.snapshot, &self.session, kind, id)
    }
    pub(crate) fn charge_bytes(&self) -> usize {
        self.charge
    }
    #[cfg(test)]
    pub(crate) fn segment_reads(&self) -> usize {
        self.reader.lock().unwrap().metrics.segment_reads
    }
}
