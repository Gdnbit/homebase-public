//! Stable, bounded history reads. A snapshot is data, never execution authority.
use crate::assistant::{view_at, Progress, Session};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const PAGE_BYTES: usize = 9000;
const CHUNK_BYTES: usize = 8192;
const DIRECT_CHUNK_BYTES: usize = 512 * 1024;
const DIRECT_RESPONSE_BYTES: usize = 1024 * 1024;
const SOURCE_BYTES: usize = 32 * 1024 * 1024;
const CACHE_BYTES: usize = 64 * 1024 * 1024;
const SNAPSHOTS: usize = 4;
const TTL: Duration = Duration::from_secs(300);
const MAX_AGE: Duration = Duration::from_secs(3600);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    #[default]
    Messages,
    Turns,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    pub session_id: String,
    #[serde(default)]
    pub kind: HistoryKind,
    #[serde(default)]
    pub query: String,
    pub snapshot_id: Option<String>,
    pub through_turn_id: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "page_limit")]
    pub limit: usize,
}
fn page_limit() -> usize {
    8
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryRead {
    pub session_id: String,
    pub snapshot_id: String,
    pub item_id: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "chunk_limit")]
    pub limit: usize,
}
fn chunk_limit() -> usize {
    4096
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub turn_id: Option<String>,
    pub role: Option<String>,
    pub state: Option<String>,
    pub created_at: String,
    pub preview: String,
    pub preview_only: bool,
    pub json_bytes: usize,
    pub json_sha256: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryPage {
    pub version: String,
    pub session_id: String,
    pub snapshot_id: String,
    pub kind: HistoryKind,
    pub query: String,
    pub through_turn_id: Option<String>,
    pub captured_at: String,
    pub expires_in_seconds: u64,
    pub total: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub complete: bool,
    pub omitted_unassigned_messages: usize,
    pub items: Vec<HistoryItem>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryChunk {
    pub version: String,
    pub session_id: String,
    pub snapshot_id: String,
    pub kind: HistoryKind,
    pub item_id: String,
    pub format: String,
    pub json_sha256: String,
    pub total_bytes: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub complete: bool,
    pub content: String,
}

pub(crate) struct HistorySource {
    pub session: Session,
    pub source_key: String,
    pub kind: HistoryKind,
    pub query: String,
    pub through_turn_id: Option<String>,
    pub omitted_unassigned_messages: usize,
    pub captured_at: DateTime<Utc>,
    pub records: Option<Arc<dyn HistoryRecords>>,
    pub direct_turn: Option<DirectTurnContext>,
}
#[derive(Serialize)]
pub(crate) struct DirectTurnContext {
    pub progress: Progress,
    pub resumable: bool,
}
/// A single row projection is read-only history data, never adopted by the Engine.
pub(crate) trait HistoryRecords: Send + Sync {
    fn ids(&self) -> &[String];
    fn charge_bytes(&self) -> Result<usize, String>;
    fn row(&self, index: usize) -> Result<Session, String>;
}
struct Snapshot {
    id: String,
    source: HistorySource,
    born: Instant,
    touched: Instant,
    bytes: usize,
}
struct Body {
    text: Zeroizing<String>,
    sha256: String,
    item: HistoryItem,
    bytes: usize,
}
pub struct HistoryStore {
    snapshots: VecDeque<Snapshot>,
    cache: BTreeMap<(String, String), Arc<Body>>,
    cache_order: VecDeque<(String, String)>,
    cache_bytes: usize,
    #[cfg(test)]
    encodes: usize,
}
impl Default for HistoryStore {
    fn default() -> Self {
        Self {
            snapshots: VecDeque::new(),
            cache: BTreeMap::new(),
            cache_order: VecDeque::new(),
            cache_bytes: 0,
            #[cfg(test)]
            encodes: 0,
        }
    }
}
fn fail(code: &str, message: &str) -> String {
    format!("{code}: {message}")
}
fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn check_snapshot(value: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(value).is_err() {
        return Err(fail("HISTORY_BAD_ARGUMENT", "快照编号无效，未查询其他记录"));
    }
    Ok(())
}
impl HistoryQuery {
    pub(crate) fn normalized_query(&self) -> String {
        self.query.trim().to_lowercase()
    }
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.query.len() > 240 || self.query.chars().any(char::is_control) {
            return Err(fail(
                "HISTORY_BAD_ARGUMENT",
                "历史关键词超过240字节或包含控制字符",
            ));
        }
        if !valid_id(&self.session_id)
            || self.through_turn_id.as_ref().is_some_and(|v| !valid_id(v))
            || !(1..=20).contains(&self.limit)
            || self.offset > 1_000_000
        {
            return Err(fail("HISTORY_BAD_ARGUMENT", "历史查询参数超出范围"));
        }
        if let Some(id) = &self.snapshot_id {
            check_snapshot(id)?;
        } else if self.offset != 0 {
            return Err(fail("HISTORY_BAD_ARGUMENT", "续页必须指定原快照"));
        }
        Ok(())
    }
}
impl HistoryRead {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.session_id)
            || !valid_id(&self.item_id)
            || !(4..=DIRECT_CHUNK_BYTES).contains(&self.limit)
        {
            return Err(fail("HISTORY_BAD_ARGUMENT", "历史正文参数超出范围"));
        }
        check_snapshot(&self.snapshot_id)
    }
}
struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        if self.0 > SOURCE_BYTES {
            return Err(std::io::Error::other("history capacity"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl HistorySource {
    fn total(&self) -> usize {
        if let Some(records) = &self.records { return records.ids().len(); }
        match self.kind {
            HistoryKind::Messages => self.session.messages.len(),
            HistoryKind::Turns => self.session.turns.len(),
        }
    }
    fn item_id(&self, index: usize) -> Result<&str, String> {
        let id = if let Some(records) = &self.records { records.ids().get(index).map(String::as_str) }
            else { match self.kind {
                HistoryKind::Messages => self.session.messages.get(index).map(|m|m.id.as_str()),
                HistoryKind::Turns => self.session.turns.get(index).map(|t|t.id.as_str()),
            }};
        id.filter(|id|valid_id(id)).ok_or_else(||fail("HISTORY_INVALID_DATA","历史编号无效，原记录保留"))
    }
    fn identity(&self, index: usize) -> Result<HistoryItem, String> {
        Self::identity_at(&self.session, self.kind, index)
    }
    fn identity_at(session: &Session, kind: HistoryKind, index: usize) -> Result<HistoryItem, String> {
        let (id, turn_id, role, state, created, content) = match kind {
            HistoryKind::Messages => {
                let m = session
                    .messages
                    .get(index)
                    .ok_or_else(|| fail("HISTORY_ITEM_NOT_FOUND", "原消息不存在"))?;
                (
                    m.id.clone(),
                    m.turn_id.clone(),
                    Some(m.role.clone()),
                    None,
                    m.created_at.clone(),
                    &m.content,
                )
            }
            HistoryKind::Turns => {
                let t = session
                    .turns
                    .get(index)
                    .ok_or_else(|| fail("HISTORY_ITEM_NOT_FOUND", "原轮次不存在"))?;
                (
                    t.id.clone(),
                    Some(t.id.clone()),
                    None,
                    Some(t.state.clone()),
                    t.created_at.clone(),
                    &t.text,
                )
            }
        };
        if !valid_id(&id)
            || turn_id.as_ref().is_some_and(|v| !valid_id(v))
            || role.as_ref().is_some_and(|v| v.len() > 64)
            || state.as_ref().is_some_and(|v| v.len() > 64)
            || created.len() > 128
        {
            return Err(fail("HISTORY_INVALID_DATA", "历史元数据无效，原记录保留"));
        }
        Ok(HistoryItem {
            id,
            turn_id,
            role,
            state,
            created_at: created,
            preview: content.chars().take(160).collect(),
            preview_only: true,
            json_bytes: 0,
            json_sha256: String::new(),
        })
    }
    fn body(&self, id: &str) -> Result<Body, String> {
        let owned;
        let session = if let Some(records) = &self.records {
            let position = records.ids().iter().position(|item|item == id)
                .ok_or_else(||fail("HISTORY_ITEM_NOT_FOUND","快照中没有这条记录"))?;
            owned = records.row(position)?;
            &owned
        } else { &self.session };
        let position = match self.kind {
            HistoryKind::Messages => session.messages.iter().position(|m|m.id == id),
            HistoryKind::Turns => session.turns.iter().position(|t|t.id == id),
        }.ok_or_else(||fail("HISTORY_ITEM_NOT_FOUND","快照中没有这条记录"))?;
        let item = Self::identity_at(session,self.kind,position)?;
        let text = match self.kind {
            HistoryKind::Messages => serde_json::to_string(
                session
                    .messages
                    .iter()
                    .find(|m| m.id == id)
                    .ok_or_else(|| fail("HISTORY_ITEM_NOT_FOUND", "快照中没有这条消息"))?,
            ),
            HistoryKind::Turns => {
                let mut detail = view_at(
                    session,
                    session.turns.iter().find(|t| t.id == id)
                        .ok_or_else(|| fail("HISTORY_ITEM_NOT_FOUND", "快照中没有这个轮次"))?,
                    self.captured_at,
                );
                session.restore_job_results(&mut detail.jobs)?;
                if let Some(context) = &self.direct_turn {
                    detail.progress = context.progress.clone();
                    detail.resumable = context.resumable;
                    let mut value = serde_json::to_value(&detail)
                        .map_err(|_| fail("HISTORY_INVALID_DATA", "原轮次无法编码"))?;
                    crate::control_state::redact(&mut value);
                    serde_json::to_string(&value)
                } else {
                    serde_json::to_string(&detail)
                }
            }
        }
        .map_err(|_| fail("HISTORY_INVALID_DATA", "历史正文无法编码，原记录保留"))?;
        if text.len() > CACHE_BYTES {
            return Err(fail(
                "HISTORY_CAPACITY",
                "单条历史正文超过安全上限，未返回截断成功",
            ));
        }
        let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
        let bytes = text.len().checked_add(serde_json::to_vec(&item)
            .map_err(|_|fail("HISTORY_INVALID_DATA","历史元数据无法编码"))?.len())
            .filter(|n|*n <= CACHE_BYTES)
            .ok_or_else(||fail("HISTORY_CAPACITY","单条历史正文与元数据超过缓存边界"))?;
        Ok(Body {
            text: Zeroizing::new(text),
            sha256,
            item,
            bytes,
        })
    }
}
impl HistoryStore {
    fn maintain(&mut self, now: Instant) {
        let before = self.snapshots.len();
        self.snapshots.retain(|s| {
            now.saturating_duration_since(s.touched) < TTL
                && now.saturating_duration_since(s.born) < MAX_AGE
        });
        if self.snapshots.len() != before {
            self.prune_cache();
        }
    }
    fn prune_cache(&mut self) {
        let live: BTreeSet<_> = self.snapshots.iter().map(|s| s.id.as_str()).collect();
        self.cache.retain(|(id, _), _| live.contains(id.as_str()));
        self.cache_order.retain(|key| self.cache.contains_key(key));
        self.cache_bytes = self.cache.values().map(|b| b.bytes).sum();
    }
    fn touch(&mut self, id: &str, now: Instant) {
        if let Some(s) = self.snapshots.iter_mut().find(|s| s.id == id) {
            s.touched = s.touched.max(now);
        }
    }
    pub(crate) fn insert(&mut self, source: HistorySource, now: Instant) -> Result<String, String> {
        let mut counter = Counter(0);
        serde_json::to_writer(&mut counter, &source.session)
            .map_err(|_| fail("HISTORY_CAPACITY", "历史快照超过安全容量，原记录未修改"))?;
        if let Some(context) = &source.direct_turn {
            serde_json::to_writer(&mut counter, context)
                .map_err(|_| fail("HISTORY_CAPACITY", "历史快照超过安全容量，原记录未修改"))?;
        }
        if let Some(records) = &source.records {
            counter.0 = counter.0.checked_add(records.charge_bytes()?)
                .filter(|n|*n <= SOURCE_BYTES)
                .ok_or_else(||fail("HISTORY_CAPACITY","历史来源与只读缓存超过安全容量"))?;
        }
        let mut ids = BTreeSet::new();
        for index in 0..source.total() {
            let id = if source.records.is_some() { source.item_id(index)?.to_string() }
                else { source.identity(index)?.id };
            if !ids.insert(id) {
                return Err(fail("HISTORY_INVALID_DATA", "历史编号重复，未猜测记录归属"));
            }
        }
        self.maintain(now);
        while self.snapshots.len() >= SNAPSHOTS
            || self.snapshots.iter().map(|s| s.bytes).sum::<usize>() + counter.0 > SOURCE_BYTES
        {
            self.snapshots.pop_front();
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.snapshots.push_back(Snapshot {
            id: id.clone(),
            source,
            born: now,
            touched: now,
            bytes: counter.0,
        });
        self.prune_cache();
        Ok(id)
    }
    fn checked(
        &self,
        id: &str,
        session: &str,
        key: &str,
        bound: Option<&str>,
    ) -> Result<&Snapshot, String> {
        let snap = self.snapshots.iter().find(|s| s.id == id).ok_or_else(|| {
            fail(
                "HISTORY_SNAPSHOT_EXPIRED",
                "历史快照已过期或程序已重启，请重新查询",
            )
        })?;
        if snap.source.session.id != session
            || snap.source.source_key != key
            || bound.is_some_and(|v| snap.source.through_turn_id.as_deref() != Some(v))
        {
            return Err(fail(
                "HISTORY_SCOPE_MISMATCH",
                "历史快照不属于当前会话或轮次，请重新查询",
            ));
        }
        Ok(snap)
    }
    fn body(&mut self, snapshot: &str, id: &str) -> Result<Arc<Body>, String> {
        let key = (snapshot.to_string(), id.to_string());
        if let Some(body) = self.cache.get(&key) {
            return Ok(body.clone());
        }
        let body = Arc::new(
            self.snapshots
                .iter()
                .find(|s| s.id == snapshot)
                .ok_or_else(|| fail("HISTORY_SNAPSHOT_EXPIRED", "历史快照已过期，请重新查询"))?
                .source
                .body(id)?,
        );
        #[cfg(test)]
        {
            self.encodes += 1;
        }
        while self.cache_bytes + body.bytes > CACHE_BYTES {
            if let Some(old) = self.cache_order.pop_front() {
                if let Some(value) = self.cache.remove(&old) {
                    self.cache_bytes -= value.bytes;
                }
            }
        }
        self.cache_bytes += body.bytes;
        self.cache_order.push_back(key.clone());
        self.cache.insert(key, body.clone());
        Ok(body)
    }
    pub(crate) fn query(
        &mut self,
        request: &HistoryQuery,
        key: &str,
        bound: Option<&str>,
        now: Instant,
    ) -> Result<HistoryPage, String> {
        request.validate()?;
        self.maintain(now);
        let id = request
            .snapshot_id
            .as_deref()
            .ok_or_else(|| fail("HISTORY_BAD_ARGUMENT", "必须先创建历史快照"))?;
        let snap = self.checked(id, &request.session_id, key, bound)?;
        if snap.source.kind != request.kind
            || snap.source.query != request.normalized_query()
            || snap.source.through_turn_id != request.through_turn_id
        {
            return Err(fail("HISTORY_SCOPE_MISMATCH", "续页条件与原快照不一致"));
        }
        let total = snap.source.total();
        if request.offset > total {
            return Err(fail("HISTORY_BAD_OFFSET", "历史页偏移超出原快照"));
        }
        let mut page = HistoryPage {
            version: "homebase-history-v1".into(),
            session_id: request.session_id.clone(),
            snapshot_id: id.into(),
            kind: request.kind,
            query: snap.source.query.clone(),
            through_turn_id: request.through_turn_id.clone(),
            captured_at: snap.source.captured_at.to_rfc3339(),
            expires_in_seconds: TTL
                .min(MAX_AGE.saturating_sub(now.saturating_duration_since(snap.born)))
                .as_secs(),
            total,
            offset: request.offset,
            next_offset: None,
            complete: false,
            omitted_unassigned_messages: snap.source.omitted_unassigned_messages,
            items: vec![],
        };
        for index in request.offset..total.min(request.offset + request.limit) {
            let item_id = self
                .checked(id, &request.session_id, key, bound)?
                .source
                .item_id(index)?.to_string();
            let body = self.body(id, &item_id)?;
            let mut item = body.item.clone();
            item.json_bytes = body.text.len();
            item.json_sha256 = body.sha256.clone();
            page.items.push(item);
            if serde_json::to_vec(&page)
                .map_err(|_| fail("HISTORY_INVALID_DATA", "历史页无法编码"))?
                .len()
                > PAGE_BYTES - 100
            {
                page.items.pop();
                break;
            }
        }
        let next = request.offset + page.items.len();
        if next < total {
            if page.items.is_empty() {
                return Err(fail("HISTORY_CAPACITY", "单条历史元数据超出页面预算"));
            }
            page.next_offset = Some(next);
        } else {
            page.complete = true;
        }
        if serde_json::to_vec(&page)
            .map_err(|_| fail("HISTORY_INVALID_DATA", "历史页无法编码"))?
            .len()
            > PAGE_BYTES
        {
            return Err(fail("HISTORY_CAPACITY", "历史页超出预算，未返回截断成功"));
        }
        self.touch(id, now);
        Ok(page)
    }
    pub(crate) fn read(
        &mut self,
        request: &HistoryRead,
        key: &str,
        bound: Option<&str>,
        now: Instant,
    ) -> Result<HistoryChunk, String> {
        request.validate()?;
        self.maintain(now);
        let snap = self.checked(&request.snapshot_id, &request.session_id, key, bound)?;
        let kind = snap.source.kind;
        let direct = snap.source.direct_turn.is_some() && bound.is_none();
        if !direct && request.limit > CHUNK_BYTES {
            return Err(fail("HISTORY_BAD_ARGUMENT", "大分块只适用于按原轮次创建的私人快照"));
        }
        let budget = if direct && request.limit > CHUNK_BYTES { DIRECT_RESPONSE_BYTES } else { PAGE_BYTES };
        let body = self.body(&request.snapshot_id, &request.item_id)?;
        let text = body.text.as_str();
        if request.offset > text.len() || !text.is_char_boundary(request.offset) {
            return Err(fail("HISTORY_BAD_OFFSET", "正文偏移不是有效UTF-8边界"));
        }
        let mut end = text.len().min(request.offset.saturating_add(request.limit));
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        loop {
            let chunk = HistoryChunk {
                version: "homebase-history-v1".into(),
                session_id: request.session_id.clone(),
                snapshot_id: request.snapshot_id.clone(),
                kind,
                item_id: request.item_id.clone(),
                format: match kind {
                    HistoryKind::Messages => "message_json",
                    HistoryKind::Turns => "turn_view_json",
                }
                .into(),
                json_sha256: body.sha256.clone(),
                total_bytes: text.len(),
                offset: request.offset,
                next_offset: if end < text.len() { Some(end) } else { None },
                complete: end == text.len(),
                content: text[request.offset..end].into(),
            };
            if serde_json::to_vec(&chunk)
                .map_err(|_| fail("HISTORY_INVALID_DATA", "历史分块无法编码"))?
                .len()
                <= budget
            {
                self.touch(&request.snapshot_id, now);
                return Ok(chunk);
            }
            if end == request.offset {
                return Err(fail("HISTORY_CAPACITY", "历史分块元数据超出预算"));
            }
            end = request.offset + (end - request.offset) / 2;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end == request.offset && request.offset < text.len() {
                return Err(fail("HISTORY_CAPACITY", "历史正文无法在预算内前进"));
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn encoded_items(&self) -> usize {
        self.encodes
    }
    #[cfg(test)]
    pub(crate) fn clear_serialized_cache(&mut self) {
        self.cache.clear();
        self.cache_order.clear();
        self.cache_bytes = 0;
    }
}

pub fn public_error(message: &str) -> Option<(&'static str, &'static str)> {
    [
        (
            "HISTORY_BAD_ARGUMENT",
            "历史参数无效：历史页上限20，常规正文上限8192字节，指定原轮次的私人快照可选至524288字节；续读使用原编号和偏移",
        ),
        (
            "HISTORY_SNAPSHOT_EXPIRED",
            "历史快照已过期或程序已重启，请重新查询",
        ),
        (
            "HISTORY_SCOPE_MISMATCH",
            "历史快照与当前会话、轮次或筛选不一致，请重新查询",
        ),
        (
            "HISTORY_ITEM_NOT_FOUND",
            "原快照没有该历史记录，未读取其他内容",
        ),
        (
            "HISTORY_BAD_OFFSET",
            "历史偏移无效，请使用原响应的next_offset",
        ),
        (
            "HISTORY_CAPACITY",
            "历史读取超出安全容量，原数据保留，未返回截断成功",
        ),
        ("HISTORY_INVALID_DATA", "历史记录无法安全读取，原数据保留"),
        ("HISTORY_SESSION_NOT_FOUND", "原会话不存在，未改读其他会话"),
        ("HISTORY_TURN_NOT_FOUND", "原轮次不存在或不属于指定会话"),
        ("HISTORY_STEP_NOT_FOUND", "原轮次没有该步骤，未读取其他结果"),
    ]
    .into_iter()
    .find(|(code, _)| message.starts_with(&format!("{code}:")))
}
