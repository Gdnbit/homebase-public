//! Array-compatible snapshots with copy-on-write at the record boundary.
//! Cloning a State shares immutable history; writing detaches only that record.
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    io::{self, Write},
    ops::{Index, IndexMut},
    sync::{Arc, OnceLock},
};

#[derive(Clone)]
pub struct Record<T> {
    value: T,
    json_bytes: OnceLock<usize>,
}

impl<T> Record<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            json_bytes: OnceLock::new(),
        }
    }
}

pub struct Records<T>(Vec<Arc<Record<T>>>);
pub type Iter<'a, T> =
    std::iter::Map<std::slice::Iter<'a, Arc<Record<T>>>, fn(&'a Arc<Record<T>>) -> &'a T>;
pub type IterMut<'a, T> = std::iter::Map<
    std::slice::IterMut<'a, Arc<Record<T>>>,
    fn(&'a mut Arc<Record<T>>) -> &'a mut T,
>;

fn record_ref<T>(record: &Arc<Record<T>>) -> &T {
    &record.value
}
fn record_mut<T: Clone>(record: &mut Arc<Record<T>>) -> &mut T {
    let record = Arc::make_mut(record);
    record.json_bytes.take();
    &mut record.value
}

impl<T> Clone for Records<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Default for Records<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}
impl<T> From<Vec<T>> for Records<T> {
    fn from(value: Vec<T>) -> Self {
        value.into_iter().collect()
    }
}
impl<T> FromIterator<T> for Records<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|value| Arc::new(Record::new(value)))
                .collect(),
        )
    }
}
impl<T> Index<usize> for Records<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        record_ref(&self.0[index])
    }
}
impl<T: Clone> IndexMut<usize> for Records<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        record_mut(&mut self.0[index])
    }
}
impl<T> Records<T> {
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn select(&self, mut predicate: impl FnMut(&T) -> bool) -> Self {
        Self(
            self.0
                .iter()
                .filter(|record| predicate(record_ref(record)))
                .cloned()
                .collect(),
        )
    }
    pub fn iter(&self) -> Iter<'_, T> {
        self.0.iter().map(record_ref::<T>)
    }
    pub fn push(&mut self, value: T) {
        self.0.push(Arc::new(Record::new(value)));
    }
    pub(crate) fn discard_last(&mut self) {
        drop(self.0.pop());
    }
    pub(crate) fn drop_prefix(&mut self, count: usize) {
        self.0.drain(..count);
    }
    pub fn get(&self, index: usize) -> Option<&T> {
        self.0.get(index).map(record_ref)
    }
    pub(crate) fn shares_record(&self, other: &Self, index: usize) -> bool {
        self.0
            .get(index)
            .zip(other.0.get(index))
            .is_some_and(|(current, saved)| Arc::ptr_eq(current, saved))
    }
    pub fn first(&self) -> Option<&T> {
        self.0.first().map(record_ref)
    }
    pub fn last(&self) -> Option<&T> {
        self.0.last().map(record_ref)
    }
}
impl<T: Clone> Records<T> {
    /// Detaches every yielded record. Prefer matching_mut for selective writes.
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        self.0.iter_mut().map(record_mut::<T>)
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        self.0.get_mut(index).map(record_mut)
    }
    pub fn find_mut(&mut self, mut predicate: impl FnMut(&T) -> bool) -> Option<&mut T> {
        let index = self
            .0
            .iter()
            .position(|record| predicate(record_ref(record)))?;
        Some(record_mut(&mut self.0[index]))
    }
    pub fn matching_mut<'a>(
        &'a mut self,
        mut predicate: impl FnMut(&T) -> bool + 'a,
    ) -> impl Iterator<Item = &'a mut T> + 'a {
        self.0
            .iter_mut()
            .filter(move |record| predicate(record_ref(record)))
            .map(record_mut)
    }
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }
}
impl<T: Serialize> Serialize for Records<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for Records<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<T>::deserialize(deserializer).map(Into::into)
    }
}
impl<'a, T> IntoIterator for &'a Records<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(record_ref::<T>)
    }
}
impl<'a, T: Clone> IntoIterator for &'a mut Records<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

#[derive(Default)]
struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.checked_add(bytes.len()).ok_or_else(size_overflow)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn size_overflow() -> io::Error {
    io::Error::other("JSON size overflow")
}
pub(crate) fn encoded_bytes(value: &impl Serialize) -> Result<usize, serde_json::Error> {
    let mut counter = Counter::default();
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}
impl<T: Serialize> Records<T> {
    /// Exact bytes added to an empty compact JSON array, including commas.
    pub(crate) fn encoded_extra_bytes(&self) -> Result<usize, serde_json::Error> {
        let mut total = self.len().saturating_sub(1);
        for record in &self.0 {
            let bytes = if let Some(&cached) = record.json_bytes.get() {
                cached
            } else {
                let bytes = encoded_bytes(&record.value)?;
                let _ = record.json_bytes.set(bytes);
                bytes
            };
            total = total
                .checked_add(bytes)
                .ok_or_else(|| serde_json::Error::io(size_overflow()))?;
        }
        Ok(total)
    }

    #[cfg(test)]
    pub(crate) fn cached_byte_lengths(&self) -> usize {
        self.0
            .iter()
            .filter(|row| row.json_bytes.get().is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counted {
        id: usize,
        body: String,
        clones: Arc<AtomicUsize>,
    }
    #[derive(Clone)]
    struct CountedEncoding {
        body: String,
        serializations: Arc<AtomicUsize>,
    }
    impl Serialize for CountedEncoding {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.serializations.fetch_add(1, Ordering::SeqCst);
            self.body.serialize(serializer)
        }
    }
    #[test]
    fn cached_compact_counts_follow_copy_on_write_and_mutation() {
        let serializations = Arc::new(AtomicUsize::new(0));
        let original: Records<_> = ["中文🙂", "quote\"", "line\n", "\u{1f642}"]
            .into_iter()
            .map(|body| CountedEncoding {
                body: body.into(),
                serializations: serializations.clone(),
            })
            .collect();
        let expected = encoded_bytes(&original).unwrap() - 2;
        serializations.store(0, Ordering::SeqCst);
        assert_eq!(original.encoded_extra_bytes().unwrap(), expected);
        assert_eq!(original.clone().encoded_extra_bytes().unwrap(), expected);
        assert_eq!(serializations.load(Ordering::SeqCst), 4);
        let mut changed = original.clone();
        changed[0].body.push('!');
        assert_eq!(changed.encoded_extra_bytes().unwrap(), expected + 1);
        changed.get_mut(1).unwrap().body.push('"');
        assert_eq!(changed.encoded_extra_bytes().unwrap(), expected + 3);
        changed
            .find_mut(|r| r.body.contains('\n'))
            .unwrap()
            .body
            .push('\n');
        assert_eq!(changed.encoded_extra_bytes().unwrap(), expected + 5);
        for row in changed.matching_mut(|r| r.body == "🙂") {
            row.body.push('!');
        }
        assert_eq!(changed.encoded_extra_bytes().unwrap(), expected + 6);
        assert_eq!(serializations.load(Ordering::SeqCst), 8);
        assert_eq!(original.encoded_extra_bytes().unwrap(), expected);
    }
    impl Clone for Counted {
        fn clone(&self) -> Self {
            self.clones.fetch_add(1, Ordering::SeqCst);
            Self {
                id: self.id,
                body: self.body.clone(),
                clones: self.clones.clone(),
            }
        }
    }
    #[test]
    fn record_snapshots_detach_only_actual_writes_and_leave_originals_intact() {
        let clones = Arc::new(AtomicUsize::new(0));
        let original: Records<_> = (0..100)
            .map(|id| Counted {
                id,
                body: "original".into(),
                clones: clones.clone(),
            })
            .collect();
        let mut next = original.clone();
        let selected = original.select(|r| r.id == 20 || r.id == 80);
        assert_eq!(selected.len(), 2);
        assert_eq!(
            clones.load(Ordering::SeqCst),
            0,
            "selecting a snapshot must not clone record payloads"
        );
        drop(selected);
        assert_eq!(clones.load(Ordering::SeqCst), 0);
        assert!(next.find_mut(|r| r.id == 1000).is_none());
        assert_eq!(clones.load(Ordering::SeqCst), 0);
        next.find_mut(|r| r.id == 95).unwrap().body = "updated".into();
        next[95].body.push('!');
        assert_eq!(clones.load(Ordering::SeqCst), 1);
        assert_eq!(original[95].body, "original");
        assert_eq!(next[95].body, "updated!");
        for record in next.matching_mut(|r| r.id == 20 || r.id == 80) {
            record.body = "selected".into();
        }
        assert_eq!(clones.load(Ordering::SeqCst), 3);
        assert_eq!(original[20].body, "original");
        assert_eq!(next.iter().filter(|r| r.body == "selected").count(), 2);
        drop(next);
        assert_eq!(original.len(), 100);
        assert!(original.iter().all(|r| r.body == "original"));
    }
    #[test]
    fn old_json_arrays_keep_order_values_and_encoding_without_a_migration() {
        let old = serde_json::json!([{"id":"a","result":{"text":"中文🙂"}},null,{"id":"b","result":[1,false]}]);
        let records: Records<serde_json::Value> = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(
            serde_json::to_vec(&records).unwrap(),
            serde_json::to_vec(&old).unwrap()
        );
        assert_eq!(records.first().unwrap()["id"], "a");
        let mut changed = records.clone();
        changed[0]["result"]["text"] = serde_json::json!("更新");
        assert_eq!(records[0]["result"]["text"], "中文🙂");
        assert_eq!(changed.last().unwrap()["id"], "b");
        assert_eq!(records.to_vec(), old.as_array().unwrap().clone());
    }
}
