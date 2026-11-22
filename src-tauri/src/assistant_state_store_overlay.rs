//! In-process references to verified immutable records, never a wire DTO or persisted shell.
use super::*;

#[derive(Clone, Debug, Serialize)]
pub(crate) enum RecordSlot {
    Retained { index: usize, id: String },
    Owned(Value),
}
impl RecordSlot {
    fn id(&self) -> Result<&str, String> {
        match self {
            Self::Retained { id, .. } => Ok(id),
            Self::Owned(value) => value["id"]
                .as_str()
                .ok_or_else(|| error("ARCHIVE_IDENTITY")),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SeriesOverlay {
    pub session_id: String,
    pub kind: Kind,
    #[serde(skip)]
    pub snapshot: Snapshot,
    pub records: Vec<RecordSlot>,
}

pub(super) fn validate<'a>(
    store: &Store,
    value: &Value,
    base: &Snapshot,
    cold: &BTreeMap<String, &Snapshot>,
    overlays: &'a [SeriesOverlay],
) -> Result<BTreeMap<(String, Kind), &'a SeriesOverlay>, String> {
    let current = store
        .current
        .as_ref()
        .ok_or_else(|| error("ARCHIVE_CONFLICT"))?;
    if !store.capacity_segments() || !base.capacity_segmented() {
        return Err(error("ARCHIVE_SCOPE"));
    }
    // Generation alone is insufficient: bind all metadata, scopes and series as well.
    if base.root != store.root || current.as_ref() != base.index.as_ref() {
        return Err(error("ARCHIVE_CONFLICT"));
    }
    let sessions = value["sessions"]
        .as_array()
        .ok_or_else(|| error("ARCHIVE_INVALID"))?;
    let previous = current.metadata["sessions"]
        .as_array()
        .ok_or_else(|| error("ARCHIVE_INVALID"))?;
    if sessions.len() < previous.len() {
        return Err(error("ARCHIVE_SCOPE"));
    }
    for (old, new) in previous.iter().zip(sessions) {
        if scope(old)? != scope(new)? {
            return Err(error("ARCHIVE_SCOPE"));
        }
    }
    let owners = sessions
        .iter()
        .map(|session| scope(session).map(|(id, source)| (id, (source, session))))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    if owners.len() != sessions.len() {
        return Err(error("ARCHIVE_IDENTITY"));
    }
    let old_series: BTreeMap<_, _> = current
        .series
        .iter()
        .map(|series| ((series.session_id.as_str(), series.kind), series))
        .collect();
    let mut result = BTreeMap::new();
    for overlay in overlays {
        let key = (overlay.session_id.clone(), overlay.kind);
        if overlay.kind == Kind::Context
            || overlay.snapshot.root != store.root
            || !overlay.snapshot.capacity_segmented()
            || cold.contains_key(&overlay.session_id)
            || result.insert(key, overlay).is_some()
        {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let (source, session) = owners
            .get(&overlay.session_id)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        if session[overlay.kind.key()]
            .as_array()
            .is_none_or(|items| !items.is_empty())
        {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let old = overlay
            .snapshot
            .index
            .series
            .iter()
            .find(|s| s.session_id == overlay.session_id && s.kind == overlay.kind)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        if source != &old.source_digest {
            return Err(error("ARCHIVE_SCOPE"));
        }
        if old_series
            .get(&(overlay.session_id.as_str(), overlay.kind))
            .copied()
            != Some(old)
        {
            return Err(error("ARCHIVE_CONFLICT"));
        }
        if overlay.records.len() < old.count {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let mut original_ids = old.segments.iter().flat_map(|r| &r.item_ids);
        let mut seen = BTreeSet::new();
        for (index, item) in overlay.records.iter().enumerate() {
            let id = item.id()?;
            if !identifier(id, 256, false) || !seen.insert(id) {
                return Err(error("ARCHIVE_IDENTITY"));
            }
            if index < old.count && original_ids.next().map(String::as_str) != Some(id) {
                return Err(error("ARCHIVE_SCOPE"));
            }
            if let RecordSlot::Retained {
                index: original, ..
            } = item
            {
                if *original != index || index >= old.count {
                    return Err(error("ARCHIVE_SCOPE"));
                }
            }
        }
    }
    // An omitted overlay must never silently erase a different series or session.
    for old in &current.series {
        if old.kind == Kind::Context
            || cold.contains_key(&old.session_id)
            || result.contains_key(&(old.session_id.clone(), old.kind))
        {
            continue;
        }
        let (_, session) = owners
            .get(&old.session_id)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        let items = session[old.kind.key()]
            .as_array()
            .ok_or_else(|| error("ARCHIVE_INVALID"))?;
        if items.len() < old.count
            || old
                .segments
                .iter()
                .flat_map(|r| &r.item_ids)
                .zip(items)
                .any(|(id, item)| item["id"].as_str() != Some(id.as_str()))
        {
            return Err(error("ARCHIVE_SCOPE"));
        }
    }
    Ok(result)
}

pub(super) fn push_pending(
    pending: &mut Vec<(String, Zeroizing<Vec<u8>>)>,
    bytes: &mut usize,
    limited: bool,
    file: String,
    cipher: Zeroizing<Vec<u8>>,
) -> Result<(), String> {
    if limited {
        *bytes = bytes
            .checked_add(cipher.len())
            .filter(|n| *n <= MATERIALIZE_LIMIT)
            .ok_or_else(|| error("ARCHIVE_LIMIT"))?;
    }
    pending.push((file, cipher));
    Ok(())
}

pub(super) fn prepare(
    store: &mut Store,
    overlay: &SeriesOverlay,
    pending: &mut Vec<(String, Zeroizing<Vec<u8>>)>,
    pending_bytes: &mut usize,
    reused: &mut usize,
) -> Result<Series, String> {
    // validate() has checked this immutable, source-bound series against the fresh head.
    let old = overlay
        .snapshot
        .index
        .series
        .iter()
        .find(|s| s.session_id == overlay.session_id && s.kind == overlay.kind)
        .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
    let mut refs = vec![];
    let mut start = 0;
    while start < overlay.records.len() {
        let position = old.segments.partition_point(|r| r.start < start);
        let original = old.segments.get(position).filter(|r| r.start == start);
        if let Some(original) = original {
            let end = start + original.count;
            let unchanged = overlay.records[start..end]
                .iter()
                .all(|slot| matches!(slot, RecordSlot::Retained { .. }));
            // Merge an appended suffix into the last short segment; internal short segments
            // may be byte-limited and remain reusable without decoding unrelated history.
            if unchanged
                && (original.count == RECORDS
                    || end < old.count
                    || overlay.records.len() == old.count)
            {
                refs.push(original.clone());
                *reused += 1;
                start = end;
                continue;
            }
        }
        let mut count = RECORDS.min(overlay.records.len() - start);
        // A smaller hot value must not repack a following untouched byte-limited segment.
        // If a growing record splits its old segment, resume at that segment's original end.
        let containing = old.segments.partition_point(|r| r.start <= start);
        if let Some(boundary) = containing.checked_sub(1).and_then(|n| old.segments.get(n)) {
            let end = boundary.start + boundary.count;
            if start < end && end < old.count {
                count = count.min(end - start);
            }
        }
        let (plain, items) = loop {
            let slots = &overlay.records[start..start + count];
            let first = slots
                .iter()
                .position(|v| matches!(v, RecordSlot::Retained { .. }));
            let last = slots
                .iter()
                .rposition(|v| matches!(v, RecordSlot::Retained { .. }));
            let retained = if let (Some(first), Some(last)) = (first, last) {
                match store.read_range(
                    &overlay.snapshot,
                    &overlay.session_id,
                    overlay.kind,
                    start + first,
                    last - first + 1,
                ) {
                    Ok(items) => items,
                    Err(e) if count > 1 && e.starts_with("ARCHIVE_LIMIT:") => {
                        count = (count + 1) / 2;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            } else {
                vec![]
            };
            let items = slots
                .iter()
                .enumerate()
                .map(|(offset, slot)| match slot {
                    RecordSlot::Owned(value) => Ok(value.clone()),
                    RecordSlot::Retained { id, .. } => {
                        let value = retained
                            .get(offset - first.unwrap())
                            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
                        if value["id"].as_str() != Some(id.as_str()) {
                            return Err(error("ARCHIVE_SCOPE"));
                        }
                        Ok(value.clone())
                    }
                })
                .collect::<Result<Vec<_>, String>>()?;
            let segment = Segment {
                format: 1,
                session_id: overlay.session_id.clone(),
                source_digest: old.source_digest.clone(),
                kind: overlay.kind,
                start,
                items,
            };
            let plain =
                Zeroizing::new(serde_json::to_vec(&segment).map_err(|_| error("ARCHIVE_INVALID"))?);
            if plain.len() <= SEGMENT_LIMIT {
                break (plain, segment.items);
            }
            if count == 1 {
                return Err(error("ARCHIVE_LIMIT"));
            }
            count = (count + 1) / 2;
        };
        let plain_sha = digest(&plain);
        if let Some(original) = original.filter(|r| r.count == count && r.plain_sha256 == plain_sha)
        {
            refs.push(original.clone());
            *reused += 1;
        } else {
            let cipher =
                crate::wechat_crypto::crypt(&plain, true).map_err(|_| error("ARCHIVE_CRYPTO"))?;
            if cipher.len() > CIPHER_LIMIT {
                return Err(error("ARCHIVE_LIMIT"));
            }
            let reference = SegmentRef {
                file: uuid::Uuid::new_v4().to_string(),
                cipher_sha256: digest(&cipher),
                plain_sha256: plain_sha,
                plain_bytes: plain.len(),
                cipher_bytes: cipher.len(),
                start,
                count,
                item_ids: items
                    .iter()
                    .map(|v| v["id"].as_str().unwrap().to_string())
                    .collect(),
            };
            push_pending(pending, pending_bytes, true, reference.file.clone(), cipher)?;
            refs.push(reference);
        }
        start += count;
    }
    Ok(Series {
        session_id: overlay.session_id.clone(),
        source_digest: old.source_digest.clone(),
        kind: overlay.kind,
        count: overlay.records.len(),
        segments: refs,
    })
}
