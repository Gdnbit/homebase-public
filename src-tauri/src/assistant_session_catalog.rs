//! Ephemeral metadata snapshots. Discovery is never task or source authority.
use super::{Engine, SessionMeta};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant};

const PAGE_BYTES: usize = 9000;
const SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
const STORE_BYTES: usize = 32 * 1024 * 1024;
const SNAPSHOTS: usize = 4;
const MAX_ROWS: usize = 10_000;
const TTL: Duration = Duration::from_secs(300);
const MAX_AGE: Duration = Duration::from_secs(3600);

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    Desktop,
    Wechat,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionQuery {
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub query: String,
    pub source: Option<SessionSource>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    20
}
impl SessionQuery {
    fn validate(mut self) -> Result<Self, String> {
        if self.query.len() > 240
            || self.query.chars().any(char::is_control)
            || !(1..=40).contains(&self.limit)
            || self.offset > MAX_ROWS
            || self
                .snapshot_id
                .as_ref()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_err())
        {
            return Err(error("ARGUMENT_INVALID"));
        }
        if self.snapshot_id.is_none() && self.offset != 0 {
            return Err(error("SNAPSHOT_REQUIRED"));
        }
        self.query = self.query.trim().to_lowercase();
        if self.query.len() > 240 {
            return Err(error("ARGUMENT_INVALID"));
        }
        Ok(self)
    }
}
#[derive(Serialize)]
pub struct SessionPage {
    pub version: String,
    pub snapshot_id: String,
    pub query: String,
    pub source: Option<SessionSource>,
    pub captured_at: String,
    pub captured_revision: u64,
    pub expires_in_seconds: u64,
    pub total: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub complete: bool,
    pub items: Vec<SessionMeta>,
}
struct Snapshot {
    id: String,
    query: String,
    source: Option<SessionSource>,
    revision: u64,
    captured_at: String,
    created: Instant,
    accessed: Instant,
    bytes: usize,
    rows: Vec<SessionMeta>,
    issued: BTreeSet<usize>,
}
#[derive(Default)]
pub(super) struct Catalog {
    snapshots: VecDeque<Snapshot>,
}
fn error(code: &str) -> String {
    format!("SESSION_CATALOG_{code}: 会话目录未执行任务或改写原记录")
}
pub fn public_error(value: &str) -> Option<(&'static str, &'static str)> {
    Some(match value.split_once(':')?.0 {
        "SESSION_CATALOG_ARGUMENT_INVALID" => (
            "SESSION_CATALOG_ARGUMENT_INVALID",
            "会话目录参数无效，请核对关键词、来源、limit和快照编号。",
        ),
        "SESSION_CATALOG_SNAPSHOT_REQUIRED" => (
            "SESSION_CATALOG_SNAPSHOT_REQUIRED",
            "续页必须保留原snapshot_id、筛选与实际next_offset；未重新扫描。",
        ),
        "SESSION_CATALOG_SNAPSHOT_EXPIRED" => (
            "SESSION_CATALOG_SNAPSHOT_EXPIRED",
            "会话目录快照已过期、重启失效或淘汰；请显式从offset=0重查，不混合新旧页。",
        ),
        "SESSION_CATALOG_FILTER_MISMATCH" => (
            "SESSION_CATALOG_FILTER_MISMATCH",
            "续页筛选与原会话快照不同；保留原筛选，或从offset=0另开查询。",
        ),
        "SESSION_CATALOG_OFFSET_INVALID" => (
            "SESSION_CATALOG_OFFSET_INVALID",
            "不是原快照实际返回的续页位置，未猜测或跳过会话。",
        ),
        "SESSION_CATALOG_LIMIT" => (
            "SESSION_CATALOG_LIMIT",
            "会话元数据超过目录预算，未静默截断；可缩小筛选并核对原会话。",
        ),
        "SESSION_CATALOG_INVALID_DATA" => (
            "SESSION_CATALOG_INVALID_DATA",
            "会话编号重复或元数据无法核对，未当空目录或猜测原会话。",
        ),
        "SESSION_CATALOG_ENTRY_LIMIT" => (
            "SESSION_CATALOG_ENTRY_LIMIT",
            "单条会话元数据超过页预算，未截断编号或返回损坏JSON。",
        ),
        _ => return None,
    })
}
impl Catalog {
    fn prune(&mut self, now: Instant) {
        self.snapshots.retain(|s| {
            now.duration_since(s.accessed) < TTL && now.duration_since(s.created) < MAX_AGE
        });
    }
    fn current(&mut self, request: &SessionQuery, revision: u64, now: Instant) -> Option<String> {
        self.prune(now);
        self.snapshots
            .iter()
            .rev()
            .find(|s| {
                s.revision == revision && s.query == request.query && s.source == request.source
            })
            .map(|s| s.id.clone())
    }
    fn capture(
        &mut self,
        request: &SessionQuery,
        mut rows: Vec<SessionMeta>,
        revision: u64,
        now: Instant,
    ) -> Result<String, String> {
        self.prune(now);
        if rows.len() > MAX_ROWS {
            return Err(error("LIMIT"));
        }
        let mut ids = BTreeSet::new();
        for row in &rows {
            if row.id.is_empty()
                || row.id.len() > 256
                || row.id.chars().any(char::is_control)
                || !ids.insert(&row.id)
            {
                return Err(error("INVALID_DATA"));
            }
        }
        rows.retain(|row| {
            request.source.is_none_or(|source| {
                row.source
                    == match source {
                        SessionSource::Desktop => "desktop",
                        SessionSource::Wechat => "wechat",
                    }
            }) && (request.query.is_empty()
                || [&row.id, &row.title, &row.source]
                    .iter()
                    .any(|value| value.to_lowercase().contains(&request.query)))
        });
        rows.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let bytes = serde_json::to_vec(&rows)
            .map_err(|_| error("INVALID_DATA"))?
            .len();
        if bytes > SNAPSHOT_BYTES {
            return Err(error("LIMIT"));
        }
        // Check the first page before evicting another caller's usable snapshot.
        let id = uuid::Uuid::new_v4().to_string();
        let mut snapshot = Snapshot {
            id: id.clone(),
            query: request.query.clone(),
            source: request.source,
            revision,
            captured_at: chrono::Utc::now().to_rfc3339(),
            created: now,
            accessed: now,
            bytes,
            rows,
            issued: BTreeSet::from([0]),
        };
        Self::render(&mut snapshot, request, now)?;
        while self.snapshots.len() >= SNAPSHOTS
            || self.snapshots.iter().map(|s| s.bytes).sum::<usize>() + bytes > STORE_BYTES
        {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(snapshot);
        Ok(id)
    }
    fn render(
        snapshot: &mut Snapshot,
        request: &SessionQuery,
        now: Instant,
    ) -> Result<SessionPage, String> {
        if request.query != snapshot.query || request.source != snapshot.source {
            return Err(error("FILTER_MISMATCH"));
        }
        if !snapshot.issued.contains(&request.offset) || request.offset > snapshot.rows.len() {
            return Err(error("OFFSET_INVALID"));
        }
        let mut page = SessionPage {
            version: "homebase-session-catalog-v1".into(),
            snapshot_id: snapshot.id.clone(),
            query: snapshot.query.clone(),
            source: snapshot.source,
            captured_at: snapshot.captured_at.clone(),
            captured_revision: snapshot.revision,
            expires_in_seconds: TTL
                .min(MAX_AGE.saturating_sub(now.duration_since(snapshot.created)))
                .as_secs(),
            total: snapshot.rows.len(),
            offset: request.offset,
            next_offset: None,
            complete: false,
            items: Vec::new(),
        };
        let end = snapshot.rows.len().min(request.offset + request.limit);
        for row in &snapshot.rows[request.offset..end] {
            page.items.push(row.clone());
        }
        loop {
            let next = request.offset + page.items.len();
            page.next_offset = (next < snapshot.rows.len()).then_some(next);
            page.complete = page.next_offset.is_none();
            if serde_json::to_vec(&page)
                .map_err(|_| error("INVALID_DATA"))?
                .len()
                <= PAGE_BYTES
            {
                break;
            }
            if page.items.pop().is_none() {
                return Err(error("ENTRY_LIMIT"));
            }
        }
        if page.items.is_empty() && request.offset < snapshot.rows.len() {
            return Err(error("ENTRY_LIMIT"));
        }
        if let Some(next) = page.next_offset {
            snapshot.issued.insert(next);
        }
        snapshot.accessed = now;
        Ok(page)
    }
    fn page(&mut self, request: &SessionQuery, now: Instant) -> Result<SessionPage, String> {
        self.prune(now);
        let id = request
            .snapshot_id
            .as_deref()
            .ok_or_else(|| error("SNAPSHOT_REQUIRED"))?;
        let snapshot = self
            .snapshots
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| error("SNAPSHOT_EXPIRED"))?;
        Self::render(snapshot, request, now)
    }
}
impl Engine {
    pub async fn sessions_query(&self, request: SessionQuery) -> Result<SessionPage, String> {
        let mut request = request.validate()?;
        let now = Instant::now();
        if request.snapshot_id.is_some() {
            return self.session_catalog.lock().await.page(&request, now);
        }
        // No inverse lock path: continuation pages touch only the derived catalog.
        let state = self.state.lock().await;
        let mut catalog = self.session_catalog.lock().await;
        let id = match catalog.current(&request, state.revision, now) {
            Some(id) => id,
            None => catalog.capture(
                &request,
                state.sessions.iter().map(super::meta).collect(),
                state.revision,
                now,
            )?,
        };
        request.snapshot_id = Some(id);
        catalog.page(&request, now)
    }
}

#[cfg(test)]
#[path = "assistant_session_catalog_tests.rs"]
mod tests;
