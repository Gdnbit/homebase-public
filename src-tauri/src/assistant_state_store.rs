//! Private immutable history segments. Not permission, execution state or an eviction policy.
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

const CIPHER_LIMIT: usize = 32 * 1024 * 1024;
const SEGMENT_LIMIT: usize = 24 * 1024 * 1024;
const MATERIALIZE_LIMIT: usize = 64 * 1024 * 1024;
const RECORDS: usize = 32;
pub(crate) const ARCHIVE_JOB_TURNS_KEY: &str = "archive_job_turns";
#[path = "assistant_state_store_overlay.rs"]
mod overlay;
pub(crate) use overlay::{RecordSlot, SeriesOverlay};
#[path = "assistant_state_store_view.rs"]
mod read_view;
pub(crate) use read_view::RecordView;
#[path = "assistant_state_store_backup.rs"]
mod backup;
pub(crate) use backup::{ArchiveBackup, ArchiveDraftView, ArchiveFilePage, ArchiveRecoveredFile};
pub(crate) fn initialization_allowed(
    mode: Option<&std::ffi::OsStr>,
    test_home: Option<&std::ffi::OsStr>,
) -> Result<bool, String> {
    match mode.and_then(std::ffi::OsStr::to_str) {
        Some("1") => {
            if test_home.is_some_and(|v| !v.is_empty() && Path::new(v).is_absolute()) {
                Ok(true)
            } else {
                Err("新分段存储仅可在显式隔离测试home中初始化，原文件未修改".into())
            }
        }
        Some("0") => Ok(false),
        None if mode.is_none() => Ok(false),
        _ => Err("历史存储模式未知，原文件未修改".into()),
    }
}
fn uuid(text: &str) -> bool {
    uuid::Uuid::parse_str(text).is_ok_and(|id| id.to_string() == text)
}
fn budget<T: Serialize>(value: &T, limit: usize) -> Result<(), String> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter { bytes: 0, limit }, value).map_err(|_| error("ARCHIVE_LIMIT"))
}
fn error(code: &str) -> String {
    format!("{code}: 历史存储未完成校验，原记录保留；不回退为空或重新执行任务")
}
fn io_failure(stage: &str, os_code: Option<i64>) -> String {
    let message = error("ARCHIVE_IO");
    match os_code {
        Some(code) => format!("{message}（{stage}，系统代码 {code}）"),
        None => format!("{message}（{stage}）"),
    }
}
fn publication_failure(record: &str, failure: crate::control_protocol::ControlError) -> String {
    // Diagnostics carry only a known stage and an optional system error code.
    let detail = failure.details.as_ref();
    let stage = if failure.code == "STATE_ACL" {
        "保护文件权限"
    } else {
        match detail.and_then(|value| value["stage"].as_str()) {
            Some("parent") => "定位目录",
            Some("staging") => "建立暂存文件",
            Some("write") => "写入并同步暂存文件",
            Some("replace") => "原子替换",
            Some("directory_sync") => "同步目录",
            _ => "发布文件",
        }
    };
    io_failure(
        &format!("{record} · {stage}"),
        detail.and_then(|value| value["os_code"].as_i64()),
    )
}

fn index_replacement_may_be_blocked(failure: &crate::control_protocol::ControlError) -> bool {
    // MoveFileExW can also report access denied for a held destination.
    cfg!(windows)
        && failure.code == "STATE_IO"
        && failure.details.as_ref().is_some_and(|detail| {
            detail["stage"].as_str() == Some("replace")
                && matches!(detail["os_code"].as_i64(), Some(5 | 32 | 33))
        })
}
/// Only known codes cross the control boundary; never echo private I/O details.
pub fn public_error(message: &str) -> Option<(&'static str, &'static str)> {
    [
        ("ARCHIVE_CHANGED", "历史分段校验不一致，已停止读取；原记录保留，未回退为空或重新执行任务"),
        ("ARCHIVE_LEGACY_CONFLICT", "旧格式状态已被其他写入器修改，已停止读写；请先核对两个版本的原记录，不能直接重试业务操作"),
        ("ARCHIVE_CONFLICT", "历史索引已被其他写入器更新，本次保存未提交；请先重新读取原记录，不能重做结果未知的任务"),
        ("ARCHIVE_BUSY", "历史存储正由其他写入器持有，本次保存未提交；请先查询原任务，不能重新发起业务操作"),
        ("ARCHIVE_INCOMPLETE", "首次历史提交尚未完成，原分段保留；未建立空配置或回退旧快照，请核对后恢复"),
        ("ARCHIVE_MISSING", "原历史分段缺失，已停止读取；未替换为其他记录或重新执行任务"),
        ("ARCHIVE_RECORD_MISSING", "原编号不在指定历史范围内，未读取其他会话或创建新任务"),
        ("ARCHIVE_PATH", "历史存储路径、链接或文件类型不符合保护要求，已停止读写；原记录保留"),
        ("ARCHIVE_LIMIT", "历史存储超过当前安全读取或保存边界，已停止处理；未截断、删除或重做任务"),
        ("ARCHIVE_CRYPTO", "历史密文无法解密，已停止读取；未回退为空配置或覆盖原文件"),
        ("ARCHIVE_INVALID", "历史索引或分段格式无效，已停止处理；原记录保留，未静默忽略"),
        ("ARCHIVE_IDENTITY", "历史记录编号缺失、重复或无效，保存未提交；原记录保留"),
        ("ARCHIVE_SCOPE", "历史记录与原会话、来源或顺序不一致，已拒绝读取；未跨来源查找"),
        ("ARCHIVE_RANGE", "历史读取范围无效，请使用原快照及返回的范围继续读取"),
        ("ARCHIVE_NOT_LOADED", "历史存储尚未完成加载，本次保存未提交；不能据此重新执行业务任务"),
        ("ARCHIVE_IO", "历史存储无法完整读取或保存，请检查权限与磁盘状态；原记录未被空配置替换"),
        ("ARCHIVE_BACKUP_EXISTS", "备份编号已使用，请先查询该编号的备份状态"),
        ("ARCHIVE_BACKUP_NO_INDEX", "当前会话档案没有已发布的分段索引"),
    ]
    .into_iter()
    .find(|(code, _)| message.strip_prefix(code).is_some_and(|rest| rest.starts_with(':')))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn identifier(text: &str, limit: usize, empty: bool) -> bool {
    (empty || !text.is_empty()) && text.len() <= limit && !text.chars().any(char::is_control)
}
fn sha(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn safe(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(error("ARCHIVE_PATH"));
    }
    // Reject a linked ancestor before asking the filesystem about its children.
    let ancestors: Vec<_> = path.ancestors().collect();
    for part in ancestors.into_iter().rev() {
        match fs::symlink_metadata(part) {
            Ok(meta) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        return Err(error("ARCHIVE_PATH"));
                    }
                }
                if meta.file_type().is_symlink() {
                    return Err(error("ARCHIVE_PATH"));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(failure) => return Err(io_failure("核对路径", failure.raw_os_error().map(i64::from))),
        }
    }
    Ok(())
}
pub(crate) fn ordinary(file: &fs::File) -> Result<fs::Metadata, String> {
    let meta = file.metadata()
        .map_err(|failure| io_failure("读取文件属性", failure.raw_os_error().map(i64::from)))?;
    if !meta.is_file() {
        return Err(error("ARCHIVE_PATH"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() != 1 {
            return Err(error("ARCHIVE_PATH"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0
            || info.nNumberOfLinks != 1
        {
            return Err(error("ARCHIVE_PATH"));
        }
    }
    Ok(meta)
}
pub(crate) fn read_bounded(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, String> {
    safe(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    let file = match options.open(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(failure) => return Err(io_failure("打开原文件", failure.raw_os_error().map(i64::from))),
    };
    let meta = ordinary(&file)?;
    if meta.len() > limit as u64 {
        return Err(error("ARCHIVE_LIMIT"));
    }
    let mut bytes = Vec::with_capacity((meta.len() as usize).min(limit));
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|failure| io_failure("读取原文件", failure.raw_os_error().map(i64::from)))?;
    if bytes.len() > limit {
        return Err(error("ARCHIVE_LIMIT"));
    }
    Ok(Some(bytes))
}
pub(crate) fn read_legacy(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
    let Some(cipher) = read_bounded(path, CIPHER_LIMIT)? else {
        return Ok(None);
    };
    let plain = crate::wechat_crypto::crypt(&cipher, false).map_err(|_| error("ARCHIVE_CRYPTO"))?;
    if plain.len() > CIPHER_LIMIT {
        return Err(error("ARCHIVE_LIMIT"));
    }
    Ok(Some(plain))
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    let plain = crate::wechat_crypto::crypt(bytes, false).map_err(|_| error("ARCHIVE_CRYPTO"))?;
    if plain.len() > CIPHER_LIMIT {
        return Err(error("ARCHIVE_LIMIT"));
    }
    serde_json::from_slice(&plain).map_err(|_| error("ARCHIVE_INVALID"))
}
fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Zeroizing<Vec<u8>>, String> {
    budget(value, limit)?;
    let plain = Zeroizing::new(serde_json::to_vec(value).map_err(|_| error("ARCHIVE_INVALID"))?);
    if plain.len() > limit {
        return Err(error("ARCHIVE_LIMIT"));
    }
    let cipher = crate::wechat_crypto::crypt(&plain, true).map_err(|_| error("ARCHIVE_CRYPTO"))?;
    if cipher.len() > CIPHER_LIMIT {
        return Err(error("ARCHIVE_LIMIT"));
    }
    Ok(cipher)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Messages,
    Turns,
    Jobs,
    Context,
}
impl Kind {
    fn key(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Turns => "turns",
            Self::Jobs => "jobs",
            Self::Context => "history",
        }
    }
}
const KINDS: [Kind; 3] = [Kind::Messages, Kind::Turns, Kind::Jobs];
const CONTEXT_KINDS: [Kind; 4] = [Kind::Messages, Kind::Turns, Kind::Jobs, Kind::Context];
fn kinds(format: u32) -> &'static [Kind] {
    if matches!(format, 2 | 3) {
        &CONTEXT_KINDS
    } else {
        &KINDS
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SegmentRef {
    file: String,
    cipher_sha256: String,
    plain_sha256: String,
    plain_bytes: usize,
    cipher_bytes: usize,
    start: usize,
    count: usize,
    item_ids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Series {
    session_id: String,
    source_digest: String,
    kind: Kind,
    count: usize,
    segments: Vec<SegmentRef>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    format: u32,
    generation: String,
    anchor_sha256: Option<String>,
    metadata: Value,
    series: Vec<Series>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Segment {
    format: u32,
    session_id: String,
    source_digest: String,
    kind: Kind,
    start: usize,
    items: Vec<Value>,
}

fn segment_cipher(path: &Path, reference: &SegmentRef) -> Result<Vec<u8>, String> {
    let bytes = read_bounded(path, reference.cipher_bytes)?
        .ok_or_else(|| error("ARCHIVE_MISSING"))?;
    if bytes.len() != reference.cipher_bytes || digest(&bytes) != reference.cipher_sha256 {
        return Err(error("ARCHIVE_CHANGED"));
    }
    Ok(bytes)
}

fn decode_segment(bytes: &[u8], reference: &SegmentRef) -> Result<Segment, String> {
    let plain = crate::wechat_crypto::crypt(bytes, false).map_err(|_| error("ARCHIVE_CRYPTO"))?;
    if plain.len() != reference.plain_bytes || digest(&plain) != reference.plain_sha256 {
        return Err(error("ARCHIVE_CHANGED"));
    }
    serde_json::from_slice(&plain).map_err(|_| error("ARCHIVE_INVALID"))
}

fn validate_segment(segment: &Segment, series: &Series, reference: &SegmentRef) -> Result<(), String> {
    if segment.format != 1
        || segment.session_id != series.session_id
        || segment.source_digest != series.source_digest
        || segment.kind != series.kind
        || segment.start != reference.start
        || segment.items.len() != reference.count
        || segment.items.iter().zip(&reference.item_ids).any(|(item, id)| item["id"].as_str() != Some(id.as_str()))
    {
        return Err(error("ARCHIVE_SCOPE"));
    }
    if series.kind == Kind::Context
        && segment.items.iter().any(|item| {
            item.as_object().is_none_or(|map| map.len() != 2) || !item["messages"].is_array()
        })
    {
        return Err(error("ARCHIVE_INVALID"));
    }
    Ok(())
}
#[derive(Clone)]
pub struct Snapshot {
    root: PathBuf,
    index: Arc<Index>,
}
impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("generation", &self.index.generation)
            .finish_non_exhaustive()
    }
}
impl Snapshot {
    pub(crate) fn owner_matches(&self, session: &str, source: &str, key: &str) -> bool {
        self.index.metadata["sessions"].as_array().is_some_and(|rows|rows.iter().any(|row|
            row["id"].as_str()==Some(session) && row["source"].as_str()==Some(source)
                && row["source_key"].as_str()==Some(key)))
    }
    pub fn context_segmented(&self) -> bool {
        matches!(self.index.format, 2 | 3)
    }
    pub fn capacity_segmented(&self) -> bool {
        self.index.format == 3
    }
    pub fn metadata(&self) -> Value {
        self.index.metadata.clone()
    }
    pub(crate) fn archive_job_turns(&self) -> Option<&Value> {
        self.index.metadata.get(ARCHIVE_JOB_TURNS_KEY)
    }
    pub fn record_ids(&self, session: &str, kind: Kind) -> Result<Vec<String>, String> {
        let series = self
            .index
            .series
            .iter()
            .find(|s| s.session_id == session && s.kind == kind)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        Ok(series
            .segments
            .iter()
            .flat_map(|r| r.item_ids.iter().cloned())
            .collect())
    }
    pub fn generation(&self) -> &str {
        &self.index.generation
    }
    pub fn count(&self, session: &str, kind: Kind) -> Result<usize, String> {
        self.index
            .series
            .iter()
            .find(|s| s.session_id == session && s.kind == kind)
            .map(|s| s.count)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))
    }
}
#[derive(Clone, Copy, Default, Debug)]
pub struct Metrics {
    pub index_reads: usize,
    pub index_decryptions: usize,
    pub segment_reads: usize,
    #[cfg(test)]
    pub(crate) job_range_bytes: usize,
    #[cfg(test)]
    pub(crate) turn_range_bytes: usize,
}
#[derive(Clone, Debug)]
pub struct Saved {
    pub snapshot: Snapshot,
    pub generation: String,
    pub segments_written: usize,
    pub segments_reused: usize,
    pub index_written: bool,
}
struct Cached {
    reference: SegmentRef,
    value: Arc<Segment>,
}
pub struct Store {
    context_enabled: bool,
    capacity_enabled: bool,
    root: PathBuf,
    legacy: PathBuf,
    observed: Option<String>,
    anchor: Option<String>,
    loaded: bool,
    current: Option<Arc<Index>>,
    metrics: Metrics,
    cache: Arc<std::sync::Mutex<VecDeque<Cached>>>,
    record_views: BTreeMap<(String, String), std::sync::Weak<RecordView>>,
    view_reads: Arc<std::sync::atomic::AtomicUsize>,
    read_observer: Option<Arc<std::sync::atomic::AtomicUsize>>,
    #[cfg(test)]
    fail_before_head: bool,
    #[cfg(all(test, windows))]
    before_index_retry: Option<Box<dyn FnOnce() + Send + Sync>>,
}
fn scope(session: &Value) -> Result<(String, String), String> {
    let id = session["id"]
        .as_str()
        .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
    let source = session["source"]
        .as_str()
        .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
    let key = session["source_key"]
        .as_str()
        .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
    if !identifier(id, 256, false) || !identifier(source, 64, false) || !identifier(key, 2048, true)
    {
        return Err(error("ARCHIVE_SCOPE"));
    }
    Ok((
        id.into(),
        digest(&serde_json::to_vec(&(id, source, key)).unwrap()),
    ))
}
fn validate(index: &Index) -> Result<(), String> {
    if !matches!(index.format, 1 | 2 | 3)
        || !uuid(&index.generation)
        || index.anchor_sha256.as_ref().is_some_and(|v| !sha(v))
    {
        return Err(error("ARCHIVE_INVALID"));
    }
    let capacity = index.metadata["archive_capacity_version"].as_u64().unwrap_or(0);
    if (index.format == 3 && capacity != 1) || (index.format != 3 && capacity != 0) {
        return Err(error("ARCHIVE_MODE"));
    }
    let sessions = index.metadata["sessions"]
        .as_array()
        .ok_or_else(|| error("ARCHIVE_INVALID"))?;
    if sessions.len() > 10000 || index.series.len() != sessions.len() * kinds(index.format).len() {
        return Err(error("ARCHIVE_LIMIT"));
    }
    let mut owners = BTreeSet::new();
    let mut expected = BTreeMap::new();
    for session in sessions {
        let (id, source) = scope(session)?;
        if !owners.insert(id.clone()) {
            return Err(error("ARCHIVE_IDENTITY"));
        }
        for &kind in kinds(index.format) {
            if session[kind.key()].as_array().is_none_or(|v| !v.is_empty()) {
                return Err(error("ARCHIVE_INVALID"));
            }
            expected.insert((id.clone(), kind), source.clone());
        }
    }
    let mut files = BTreeSet::new();
    for series in &index.series {
        if expected
            .remove(&(series.session_id.clone(), series.kind))
            .as_ref()
            != Some(&series.source_digest)
        {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let mut end = 0usize;
        let mut record_ids = BTreeSet::new();
        for r in &series.segments {
            if !uuid(&r.file)
                || !files.insert(&r.file)
                || !sha(&r.cipher_sha256)
                || !sha(&r.plain_sha256)
                || r.plain_bytes > SEGMENT_LIMIT
                || r.cipher_bytes > CIPHER_LIMIT
                || r.cipher_bytes == 0
                || r.start != end
                || r.count == 0
                || r.count > RECORDS
                || r.item_ids.len() != r.count
                || r.item_ids
                    .iter()
                    .any(|id| !identifier(id, 256, false) || !record_ids.insert(id))
            {
                return Err(error("ARCHIVE_INVALID"));
            }
            if series.kind == Kind::Context
                && r.item_ids
                    .iter()
                    .enumerate()
                    .any(|(i, id)| *id != format!("context-{}", r.start + i))
            {
                return Err(error("ARCHIVE_INVALID"));
            }
            end = end
                .checked_add(r.count)
                .ok_or_else(|| error("ARCHIVE_LIMIT"))?;
        }
        if end != series.count {
            return Err(error("ARCHIVE_INVALID"));
        }
    }
    Ok(())
}
impl Store {
    pub fn new(root: PathBuf, legacy: PathBuf) -> Result<Self, String> {
        safe(&root)?;
        safe(&legacy)?;
        if root.file_name().and_then(|v| v.to_str()) != Some("history-v1")
            || root.parent() != legacy.parent()
        {
            return Err(error("ARCHIVE_PATH"));
        }
        Ok(Self::for_paths(root, legacy))
    }
    fn for_paths(root: PathBuf, legacy: PathBuf) -> Self {
        Self {
            context_enabled: false,
            capacity_enabled: false,
            root,
            legacy,
            observed: None,
            anchor: None,
            loaded: false,
            current: None,
            metrics: Metrics::default(),
            cache: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            record_views: BTreeMap::new(),
            view_reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            read_observer: None,
            #[cfg(test)]
            fail_before_head: false,
            #[cfg(all(test, windows))]
            before_index_retry: None,
        }
    }
    pub(crate) fn pinned_reader(&self) -> Self {
        let mut reader = Self::for_paths(self.root.clone(), self.legacy.clone());
        reader.cache = self.cache.clone();
        reader.read_observer = Some(self.view_reads.clone());
        reader
    }
    fn head(&self) -> PathBuf {
        self.root.join("head.dpapi")
    }
    fn segment(&self, id: &str) -> Result<PathBuf, String> {
        if !uuid(id) {
            return Err(error("ARCHIVE_PATH"));
        }
        let path = self.root.join("segments").join(format!("{id}.dpapi"));
        safe(&path)?;
        Ok(path)
    }
    fn anchor_now(&self) -> Result<Option<String>, String> {
        Ok(read_bounded(&self.legacy, CIPHER_LIMIT)?.map(|v| digest(&v)))
    }
    pub fn metrics(&self) -> Metrics {
        let mut metrics = self.metrics;
        metrics.segment_reads = metrics.segment_reads.saturating_add(
            self.view_reads.load(std::sync::atomic::Ordering::Relaxed),
        );
        metrics
    }
    #[cfg(test)]
    pub(crate) fn set_fail_before_head(&mut self, enabled: bool) { self.fail_before_head = enabled; }
    pub fn enable_context_segments(&mut self) {
        self.context_enabled = true;
    }
    pub fn context_segments(&self) -> bool {
        self.context_enabled || self.capacity_segments()
            || self.current.as_ref().is_some_and(|index| index.format == 2)
    }
    pub fn enable_capacity_segments(&mut self) {
        self.capacity_enabled = true;
        self.context_enabled = true;
    }
    pub fn capacity_segments(&self) -> bool {
        self.capacity_enabled || self.current.as_ref().is_some_and(|index| index.format == 3)
    }
    pub fn read_record(
        &mut self,
        snapshot: &Snapshot,
        session: &str,
        kind: Kind,
        id: &str,
    ) -> Result<Value, String> {
        if snapshot.root != self.root {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let series = snapshot
            .index
            .series
            .iter()
            .find(|s| s.session_id == session && s.kind == kind)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        let offset = series
            .segments
            .iter()
            .find_map(|r| {
                r.item_ids
                    .iter()
                    .position(|item| item == id)
                    .map(|i| r.start + i)
            })
            .ok_or_else(|| error("ARCHIVE_RECORD_MISSING"))?;
        self.read_range(snapshot, session, kind, offset, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| error("ARCHIVE_RECORD_MISSING"))
    }
    pub fn load_index(&mut self) -> Result<Option<Snapshot>, String> {
        let bytes = read_bounded(&self.head(), CIPHER_LIMIT)?;
        let anchor = self.anchor_now()?;
        let Some(bytes) = bytes else {
            let segments = self.root.join("segments");
            safe(&segments)?;
            if segments.exists()
                && fs::read_dir(segments)
                    .map_err(|_| error("ARCHIVE_IO"))?
                    .next()
                    .is_some()
            {
                return Err(error("ARCHIVE_INCOMPLETE"));
            }
            self.anchor = anchor;
            self.observed = None;
            self.loaded = true;
            self.current = None;
            return Ok(None);
        };
        self.metrics.index_reads += 1;
        let fingerprint = digest(&bytes);
        if self.loaded && self.observed.as_ref() == Some(&fingerprint) {
            if self.anchor != anchor {
                return Err(error("ARCHIVE_LEGACY_CONFLICT"));
            }
            if let Some(index) = &self.current {
                return Ok(Some(Snapshot {
                    root: self.root.clone(),
                    index: index.clone(),
                }));
            }
        }
        self.metrics.index_decryptions += 1;
        let index: Index = decode(&bytes)?;
        validate(&index)?;
        if index.anchor_sha256 != anchor {
            return Err(error("ARCHIVE_LEGACY_CONFLICT"));
        }
        self.observed = Some(fingerprint);
        self.anchor = anchor;
        self.loaded = true;
        let index = Arc::new(index);
        self.current = Some(index.clone());
        Ok(Some(Snapshot {
            root: self.root.clone(),
            index,
        }))
    }
    fn read_verified_segment(
        &mut self,
        series: &Series,
        reference: &SegmentRef,
    ) -> Result<Segment, String> {
        let bytes = segment_cipher(&self.segment(&reference.file)?, reference)?;
        self.metrics.segment_reads += 1;
        if let Some(observer) = &self.read_observer {
            observer.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let segment = decode_segment(&bytes, reference)?;
        validate_segment(&segment, series, reference)?;
        Ok(segment)
    }

    /// Sequential startup consumes owned rows without retaining a second copy
    /// of the decoded turn bodies in the shared range cache.
    pub(crate) fn visit_turns(
        &mut self,
        snapshot: &Snapshot,
        session: &str,
        mut visit: impl FnMut(Value) -> Result<(), String>,
    ) -> Result<(), String> {
        if snapshot.root != self.root {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let series = snapshot
            .index
            .series
            .iter()
            .find(|series| series.session_id == session && series.kind == Kind::Turns)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        for reference in &series.segments {
            let cached = self
                .cache
                .lock()
                .map_err(|_| error("ARCHIVE_INVALID"))?
                .iter()
                .find(|entry| entry.reference == *reference)
                .map(|entry| Arc::clone(&entry.value));
            if let Some(segment) = cached {
                validate_segment(&segment, series, reference)?;
                for item in &segment.items {
                    visit(item.clone())?;
                }
            } else {
                for item in self.read_verified_segment(series, reference)?.items {
                    visit(item)?;
                }
            }
        }
        Ok(())
    }

    fn cached_segment(
        &mut self,
        series: &Series,
        reference: &SegmentRef,
    ) -> Result<Arc<Segment>, String> {
        let shared_cache = Arc::clone(&self.cache);
        let mut cache = shared_cache.lock().map_err(|_| error("ARCHIVE_INVALID"))?;
        if let Some(position) = cache.iter().position(|entry| entry.reference == *reference) {
            let entry = cache.remove(position).unwrap();
            validate_segment(&entry.value, series, reference)?;
            let value = entry.value.clone();
            cache.push_back(entry);
            return Ok(value);
        }
        let value = Arc::new(self.read_verified_segment(series, reference)?);
        while cache.len() >= 8
            || cache.iter().map(|entry| entry.reference.plain_bytes).sum::<usize>()
                + reference.plain_bytes
                > CIPHER_LIMIT
        {
            if cache.pop_front().is_none() {
                break;
            }
        }
        cache.push_back(Cached {
            reference: reference.clone(),
            value: value.clone(),
        });
        Ok(value)
    }

    /// Borrow one row from its verified, size-bounded segment. The callback can
    /// return a read-only projection without copying the entire row.
    pub(crate) fn inspect_record_at<T>(
        &mut self,
        snapshot: &Snapshot,
        session: &str,
        kind: Kind,
        offset: usize,
        inspect: impl FnOnce(&Value) -> Result<T, String>,
    ) -> Result<T, String> {
        if snapshot.root != self.root {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let series = snapshot
            .index
            .series
            .iter()
            .find(|series| series.session_id == session && series.kind == kind)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        let reference = series
            .segments
            .iter()
            .find(|segment| (segment.start..segment.start + segment.count).contains(&offset))
            .ok_or_else(|| error("ARCHIVE_RANGE"))?;
        let segment = self.cached_segment(series, reference)?;
        inspect(&segment.items[offset - reference.start])
    }

    pub fn read_range(
        &mut self,
        snapshot: &Snapshot,
        session: &str,
        kind: Kind,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Value>, String> {
        if snapshot.root != self.root || limit == 0 || limit > 64 {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let series = snapshot
            .index
            .series
            .iter()
            .find(|s| s.session_id == session && s.kind == kind)
            .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
        if offset > series.count {
            return Err(error("ARCHIVE_RANGE"));
        }
        let end = offset.saturating_add(limit).min(series.count);
        let mut result = vec![];
        let mut size = 0;
        for r in series
            .segments
            .iter()
            .filter(|r| r.start < end && r.start + r.count > offset)
        {
            let segment = self.cached_segment(series, r)?;
            for item in &segment.items
                [(offset.max(r.start) - r.start)..(end.min(r.start + r.count) - r.start)]
            {
                let item_size = serde_json::to_vec(item)
                    .map_err(|_| error("ARCHIVE_INVALID"))?
                    .len();
                #[cfg(test)]
                match kind {
                    Kind::Jobs => self.metrics.job_range_bytes += item_size,
                    Kind::Turns => self.metrics.turn_range_bytes += item_size,
                    _ => (),
                }
                size += item_size;
                if size > CIPHER_LIMIT {
                    return Err(error("ARCHIVE_LIMIT"));
                }
                result.push(if kind == Kind::Context {
                    item["messages"].clone()
                } else {
                    item.clone()
                });
            }
        }
        Ok(result)
    }
    pub fn materialize(&mut self, snapshot: &Snapshot) -> Result<Value, String> {
        let metadata_bytes = serde_json::to_vec(&snapshot.index.metadata)
            .map_err(|_| error("ARCHIVE_INVALID"))?
            .len();
        let estimate = snapshot
            .index
            .series
            .iter()
            .flat_map(|s| &s.segments)
            .try_fold(metadata_bytes, |n, s| n.checked_add(s.plain_bytes))
            .ok_or_else(|| error("ARCHIVE_LIMIT"))?;
        if estimate > MATERIALIZE_LIMIT {
            return Err(error("ARCHIVE_LIMIT"));
        }
        let mut result = snapshot.index.metadata.clone();
        for session in result["sessions"]
            .as_array_mut()
            .ok_or_else(|| error("ARCHIVE_INVALID"))?
        {
            let id = session["id"].as_str().unwrap().to_string();
            for &kind in kinds(snapshot.index.format) {
                let count = snapshot.count(&id, kind)?;
                let mut items = vec![];
                for offset in (0..count).step_by(64) {
                    items.extend(self.read_range(snapshot, &id, kind, offset, 64)?);
                }
                session[kind.key()] = Value::Array(items);
            }
        }
        Ok(result)
    }
    pub async fn save(&mut self, value: Value) -> Result<Saved, String> {
        self.save_partition(value, &[]).await
    }
    /// Cold bodies may only remain unchanged from their trusted original snapshot.
    pub async fn save_partition(
        &mut self,
        value: Value,
        cold: &[(String, Snapshot)],
    ) -> Result<Saved, String> {
        self.save_combined(value, cold, None).await
    }
    /// Storage foundation only: callers must retain original identities and a fresh global base.
    /// The Engine does not yet use sparse per-record residency.
    pub(crate) async fn save_record_partition(
        &mut self,
        value: Value,
        base: &Snapshot,
        cold: &[(String, Snapshot)],
        records: &[SeriesOverlay],
    ) -> Result<Saved, String> {
        self.save_combined(value, cold, Some((base, records))).await
    }
    async fn save_combined(
        &mut self,
        mut value: Value,
        cold: &[(String, Snapshot)],
        records: Option<(&Snapshot, &[SeriesOverlay])>,
    ) -> Result<Saved, String> {
        if !self.loaded {
            return Err(error("ARCHIVE_NOT_LOADED"));
        }
        let format = if self.capacity_segments() { 3 } else if self.context_segments() { 2 } else { 1 };
        let check_budget = |value: &Value| match records {
            Some((_, overlays)) => budget(&(value, overlays), MATERIALIZE_LIMIT),
            None => budget(value, MATERIALIZE_LIMIT),
        };
        if let Err(error) = check_budget(&value) {
            if !discard_job_turns(&mut value) {
                return Err(error);
            }
            check_budget(&value)?;
        }
        safe(&self.root)?;
        fs::create_dir_all(self.root.join("segments"))
            .map_err(|failure| io_failure("准备历史分段目录", failure.raw_os_error().map(i64::from)))?;
        safe(&self.root.join("segments"))?;
        let lock_path = self.root.join("writer.lock");
        safe(&lock_path)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|failure| io_failure("打开历史写入锁", failure.raw_os_error().map(i64::from)))?;
        ordinary(&lock)?;
        lock.try_lock_exclusive()
            .map_err(|_| error("ARCHIVE_BUSY"))?;
        self.check_current()?;
        let mut cold_by_id = BTreeMap::new();
        for (session, snapshot) in cold {
            if snapshot.root != self.root || cold_by_id.insert(session.clone(), snapshot).is_some()
            {
                return Err(error("ARCHIVE_SCOPE"));
            }
            let current = self
                .current
                .as_ref()
                .ok_or_else(|| error("ARCHIVE_CONFLICT"))?;
            if matches!(format, 2 | 3) && !snapshot.context_segmented() {
                return Err(error("ARCHIVE_SCOPE"));
            }
            for &kind in kinds(format) {
                let previous = snapshot
                    .index
                    .series
                    .iter()
                    .find(|s| s.session_id == *session && s.kind == kind)
                    .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
                if current
                    .series
                    .iter()
                    .find(|s| s.session_id == *session && s.kind == kind)
                    != Some(previous)
                {
                    return Err(error("ARCHIVE_CONFLICT"));
                }
            }
        }
        let overlays = if let Some((base, records)) = records {
            overlay::validate(self, &value, base, &cold_by_id, records)?
        } else {
            BTreeMap::new()
        };
        let sessions = value
            .as_object_mut()
            .and_then(|root| root.get_mut("sessions"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| error("ARCHIVE_INVALID"))?;
        if sessions.len() > 10000 {
            return Err(error("ARCHIVE_LIMIT"));
        }
        let mut owners = BTreeSet::new();
        let mut series = vec![];
        let mut pending = vec![];
        let mut pending_bytes = 0;
        let mut reused = 0;
        for session in sessions {
            let (id, source) = scope(session)?;
            if !owners.insert(id.clone()) {
                return Err(error("ARCHIVE_IDENTITY"));
            }
            for &kind in kinds(format) {
                let mut items = std::mem::take(
                    session[kind.key()]
                        .as_array_mut()
                        .ok_or_else(|| error("ARCHIVE_INVALID"))?,
                );
                if let Some(snapshot) = cold_by_id.get(&id) {
                    let previous = snapshot
                        .index
                        .series
                        .iter()
                        .find(|s| s.session_id == id && s.kind == kind)
                        .ok_or_else(|| error("ARCHIVE_SCOPE"))?;
                    if !items.is_empty() || previous.source_digest != source {
                        return Err(error("ARCHIVE_SCOPE"));
                    }
                    reused += previous.segments.len();
                    series.push(previous.clone());
                    continue;
                }
                if let Some(records) = overlays.get(&(id.clone(), kind)) {
                    if !items.is_empty() {
                        return Err(error("ARCHIVE_SCOPE"));
                    }
                    series.push(overlay::prepare(
                        self, records, &mut pending, &mut pending_bytes, &mut reused,
                    )?);
                    continue;
                }
                if kind == Kind::Context {
                    if items.iter().any(|item| !item.is_array()) {
                        return Err(error("ARCHIVE_INVALID"));
                    }
                    items = items.into_iter().enumerate().map(|(i, messages)| serde_json::json!({"id":format!("context-{i}"),"messages":messages})).collect();
                }
                let mut seen = BTreeSet::new();
                for item in &items {
                    let item_id = item["id"]
                        .as_str()
                        .ok_or_else(|| error("ARCHIVE_IDENTITY"))?;
                    if !identifier(item_id, 256, false) || !seen.insert(item_id) {
                        return Err(error("ARCHIVE_IDENTITY"));
                    }
                }
                let mut refs = vec![];
                let mut start = 0;
                while start < items.len() {
                    let mut count = RECORDS.min(items.len() - start);
                    let plain = loop {
                        let segment = Segment {
                            format: 1,
                            session_id: id.clone(),
                            source_digest: source.clone(),
                            kind,
                            start,
                            items: items[start..start + count].to_vec(),
                        };
                        let plain = Zeroizing::new(
                            serde_json::to_vec(&segment).map_err(|_| error("ARCHIVE_INVALID"))?,
                        );
                        if plain.len() <= SEGMENT_LIMIT {
                            break plain;
                        }
                        if count == 1 {
                            return Err(error("ARCHIVE_LIMIT"));
                        }
                        count = (count + 1) / 2;
                    };
                    let plain_sha = digest(&plain);
                    let old = self
                        .current
                        .as_ref()
                        .and_then(|i| {
                            i.series.iter().find(|s| {
                                s.session_id == id && s.source_digest == source && s.kind == kind
                            })
                        })
                        .and_then(|s| {
                            s.segments.iter().find(|r| {
                                r.start == start && r.count == count && r.plain_sha256 == plain_sha
                            })
                        });
                    if let Some(old) = old {
                        refs.push(old.clone());
                        reused += 1;
                    } else {
                        let cipher = crate::wechat_crypto::crypt(&plain, true)
                            .map_err(|_| error("ARCHIVE_CRYPTO"))?;
                        if cipher.len() > CIPHER_LIMIT {
                            return Err(error("ARCHIVE_LIMIT"));
                        }
                        let r = SegmentRef {
                            file: uuid::Uuid::new_v4().to_string(),
                            cipher_sha256: digest(&cipher),
                            plain_sha256: plain_sha,
                            plain_bytes: plain.len(),
                            cipher_bytes: cipher.len(),
                            start,
                            count,
                            item_ids: items[start..start + count]
                                .iter()
                                .map(|item| item["id"].as_str().unwrap().to_string())
                                .collect(),
                        };
                        overlay::push_pending(
                            &mut pending, &mut pending_bytes, records.is_some(),
                            r.file.clone(), cipher,
                        )?;
                        refs.push(r);
                    }
                    start += count;
                }
                series.push(Series {
                    session_id: id.clone(),
                    source_digest: source.clone(),
                    kind,
                    count: items.len(),
                    segments: refs,
                });
            }
        }
        if cold_by_id.keys().any(|id| !owners.contains(id)) {
            return Err(error("ARCHIVE_SCOPE"));
        }
        if let Some(old) = &self.current {
            if old.format == format && old.metadata == value && old.series == series {
                return Ok(Saved {
                    snapshot: Snapshot { root: self.root.clone(), index: old.clone() },
                    generation: old.generation.clone(),
                    segments_written: 0,
                    segments_reused: reused,
                    index_written: false,
                });
            }
        }
        let mut next = Index {
            format,
            generation: uuid::Uuid::new_v4().to_string(),
            anchor_sha256: self.anchor.clone(),
            metadata: value,
            series,
        };
        validate(&next)?;
        let cipher = match encode(&next, CIPHER_LIMIT) {
            Err(err) if err == error("ARCHIVE_LIMIT") && discard_job_turns(&mut next.metadata) => {
                // The optional lookup data cannot reduce the canonical archive capacity.
                encode(&next, CIPHER_LIMIT)?
            }
            result => result?,
        };
        let written = pending.len();
        for (id, bytes) in pending {
            let file = self.segment(&id)?;
            if file.exists() {
                return Err(error("ARCHIVE_CONFLICT"));
            }
            crate::control_protocol::publish_private(&file, &bytes)
                .await
                .map_err(|failure| publication_failure("保存历史分段", failure))?;
        }
        #[cfg(test)]
        if self.fail_before_head {
            return Err(error("ARCHIVE_INJECTED"));
        }
        let head = self.head();
        for next_delay_ms in [Some(40), Some(120), None] {
            self.check_current()?;
            match crate::control_protocol::publish_private(&head, &cipher).await {
                Ok(()) => break,
                Err(failure) => {
                    let Some(delay) = next_delay_ms.filter(|_| index_replacement_may_be_blocked(&failure)) else {
                        return Err(publication_failure("提交历史索引", failure));
                    };
                    #[cfg(all(test, windows))]
                    if let Some(release) = self.before_index_retry.take() {
                        release();
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
            }
        }
        self.observed = Some(digest(&cipher));
        let generation = next.generation.clone();
        let index = Arc::new(next);
        self.current = Some(index.clone());
        Ok(Saved {
            snapshot: Snapshot { root: self.root.clone(), index },
            generation,
            segments_written: written,
            segments_reused: reused,
            index_written: true,
        })
    }
    /// Complete current snapshot into a fresh independent format-1 store.
    /// The caller must not activate a target unless this whole operation succeeds.
    pub async fn export_v1(
        &mut self,
        snapshot: &Snapshot,
        target: &mut Store,
    ) -> Result<Saved, String> {
        if snapshot.root != self.root || target.root == self.root || target.context_enabled {
            return Err(error("ARCHIVE_SCOPE"));
        }
        let current = self
            .load_index()?
            .ok_or_else(|| error("ARCHIVE_CONFLICT"))?;
        if current.generation() != snapshot.generation() {
            return Err(error("ARCHIVE_CONFLICT"));
        }
        if target.load_index()?.is_some() || target.anchor.is_some() {
            return Err(error("ARCHIVE_CONFLICT"));
        }
        let mut complete = self.materialize(snapshot)?;
        if snapshot.capacity_segmented() {
            // A compatible legacy export must fit the legacy Engine, not only
            // the Store materialization guard. Never publish a misleading copy.
            budget(&complete, 24 * 1024 * 1024)?;
            complete.as_object_mut().ok_or_else(|| error("ARCHIVE_INVALID"))?
                .remove("archive_capacity_version");
        }
        complete
            .as_object_mut()
            .ok_or_else(|| error("ARCHIVE_INVALID"))?
            .remove("archive_index");
        self.check_current()?;
        let saved = target.save(complete).await?;
        self.check_current()?;
        Ok(saved)
    }
    fn check_current(&self) -> Result<(), String> {
        if self.anchor_now()? != self.anchor {
            return Err(error("ARCHIVE_LEGACY_CONFLICT"));
        }
        if read_bounded(&self.head(), CIPHER_LIMIT)?.map(|v| digest(&v)) != self.observed {
            return Err(error("ARCHIVE_CONFLICT"));
        }
        Ok(())
    }
}

fn discard_job_turns(value: &mut Value) -> bool {
    value
        .as_object_mut()
        .and_then(|metadata| metadata.remove(ARCHIVE_JOB_TURNS_KEY))
        .is_some()
}

#[cfg(test)]
#[path = "assistant_state_store_tests.rs"]
mod tests;
