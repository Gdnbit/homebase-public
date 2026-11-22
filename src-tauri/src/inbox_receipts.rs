//! Durable business identities for Inbox. Observations never publish a note.
use crate::inbox_document::{render_inbox_md, InboxSubmitPayload, InboxSubmitResult};
use chrono::{DateTime, FixedOffset, Local};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const SCHEMA: u32 = 1;
pub(crate) const MAX_TEXT: usize = 512 * 1024;
const MAX_PLAIN: usize = 8 * 1024 * 1024;
const MAX_CIPHER: usize = 16 * 1024 * 1024;
const MAX_NOTE: usize = 2 * 1024 * 1024;
const MAX_RECEIPTS: usize = 10_000;

pub fn public_error(error: &str) -> Option<(&'static str, &'static str)> {
    let (code, supplied) = error.split_once(": ").unwrap_or((error, ""));
    let pair = match code {
        "INBOX_SUBMISSION_INVALID" => (
            "INBOX_SUBMISSION_INVALID",
            "收录意图或内容格式无效，未执行；请保留原编号核对。",
        ),
        "INBOX_SUBMISSION_LIMIT" => (
            "INBOX_SUBMISSION_LIMIT",
            "收录内容或回执数量超过安全上限，原数据保留，未截断或清理。",
        ),
        "INBOX_SUBMISSION_CONFLICT" => (
            "INBOX_SUBMISSION_CONFLICT",
            "原收录编号已用于不同内容或目标，未覆盖或重复执行。",
        ),
        "INBOX_SUBMISSION_READ" => (
            "INBOX_SUBMISSION_READ",
            "原收录回执无法完整读取或解密，不能按未执行或空数据处理。",
        ),
        "INBOX_SUBMISSION_SAVE" => (
            "INBOX_SUBMISSION_SAVE",
            "收录回执未确认安全保存，原编号和输入保留，不自动换编号执行。",
        ),
        "INBOX_SUBMISSION_BUSY" => (
            "INBOX_SUBMISSION_BUSY",
            "另一窗口正在处理资料，请核对原收录；未重复执行。",
        ),
        "INBOX_SUBMISSION_SOURCE_CHANGED" => (
            "INBOX_SUBMISSION_SOURCE_CHANGED",
            "原收录目录归属已变化，未回退其他目录或重新写入。",
        ),
        "INBOX_SUBMISSION_NOT_FOUND" => (
            "INBOX_SUBMISSION_NOT_FOUND",
            "未找到原收录回执，不代表未执行；请核对原任务，不自动另建记录。",
        ),
        "INBOX_SUBMISSION_UNCONFIRMED" => (
            "INBOX_SUBMISSION_UNCONFIRMED",
            "原收录已越过发布边界，结果待核对；只查询原编号，不重新写入。",
        ),
        "INBOX_SUBMISSION_NOT_PUBLISHED" => (
            "INBOX_SUBMISSION_NOT_PUBLISHED",
            "笔记暂存或预检失败，尚未发布；保留原意图，修复后只能明确继续原编号。",
        ),
        "INBOX_SUBMISSION_CANCELED" => (
            "INBOX_SUBMISSION_CANCELED",
            "这条未执行的原收录已取消，不会再次发布。",
        ),
        "INBOX_SUBMISSION_CANCEL_BLOCKED" => (
            "INBOX_SUBMISSION_CANCEL_BLOCKED",
            "原收录已发布或结果未知，不能当作未执行而取消；原文件和回执保持。",
        ),
        "INBOX_SUBMISSION_FILE_MISSING" => (
            "INBOX_SUBMISSION_FILE_MISSING",
            "原收录已完成，但文件当前不存在；未自动重建，请查询原回执。",
        ),
        "INBOX_SUBMISSION_FILE_CHANGED" => (
            "INBOX_SUBMISSION_FILE_CHANGED",
            "原收录已完成，但文件当前已修改或无法核对；未覆盖，请查询原回执。",
        ),
        _ => return None,
    };
    if supplied.is_empty() || supplied == pair.1 {
        Some(pair)
    } else {
        None
    }
}
pub(crate) fn error(code: &str) -> String {
    public_error(code)
        .map(|(code, text)| format!("{code}: {text}"))
        .unwrap_or_else(|| "INBOX_SUBMISSION_INVALID".into())
}
pub(crate) fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn fingerprint(payload: &InboxSubmitPayload) -> Result<String, String> {
    serde_json::to_vec(payload)
        .map(|v| sha(&v))
        .map_err(|_| error("INBOX_SUBMISSION_INVALID"))
}
pub(crate) fn validate_bounds(p: &InboxSubmitPayload) -> Result<(), String> {
    if p.text.len() > MAX_TEXT
        || p.title.as_ref().is_some_and(|v| v.len() > 4096)
        || p.source_hint.as_ref().is_some_and(|v| v.len() > 2048)
        || p.extra_tags.len() > 100
        || p.extra_tags.iter().any(|v| v.len() > 512)
        || p.attachments.len() > 100
        || p.attachments.iter().any(|v| v.len() > 1024)
    {
        return Err(error("INBOX_SUBMISSION_LIMIT"));
    }
    Ok(())
}
pub(crate) fn validate_payload(p: &InboxSubmitPayload) -> Result<(), String> {
    validate_bounds(p)?;
    if p.text.trim().is_empty()
        && p.attachments.is_empty()
        && p.title.as_ref().is_none_or(|v| v.trim().is_empty())
        || p.submission_id.as_deref().is_none_or(|v| !valid_id(v))
    {
        return Err(error("INBOX_SUBMISSION_INVALID"));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboxSubmissionState {
    NotFound,
    Prepared,
    Unknown,
    Completed,
    Canceled,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InboxSubmissionFileState {
    Present,
    Modified,
    Missing,
    Unreadable,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InboxSubmissionView {
    pub submission_id: String,
    pub state: InboxSubmissionState,
    /// Publication state and artifact availability are separate facts.
    pub file_state: Option<InboxSubmissionFileState>,
    pub result: Option<InboxSubmitResult>,
    pub message: String,
}

pub(crate) fn confirmed_result(view: InboxSubmissionView) -> Result<InboxSubmitResult, String> {
    match (view.state, view.file_state, view.result) {
        (
            InboxSubmissionState::Completed,
            Some(InboxSubmissionFileState::Present),
            Some(result),
        ) => Ok(result),
        (InboxSubmissionState::Completed, Some(InboxSubmissionFileState::Missing), _) => {
            Err(error("INBOX_SUBMISSION_FILE_MISSING"))
        }
        (InboxSubmissionState::Completed, _, _) => Err(error("INBOX_SUBMISSION_FILE_CHANGED")),
        (InboxSubmissionState::Canceled, _, _) => Err(error("INBOX_SUBMISSION_CANCELED")),
        (InboxSubmissionState::NotFound, _, _) => Err(error("INBOX_SUBMISSION_NOT_FOUND")),
        (InboxSubmissionState::Prepared, _, _) => Err(error("INBOX_SUBMISSION_NOT_PUBLISHED")),
        _ => Err(error("INBOX_SUBMISSION_UNCONFIRMED")),
    }
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Publishing,
    Completed,
    Canceled,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    id: String,
    vault: PathBuf,
    created_at: String,
    fingerprint: String,
    phase: Phase,
    // Dropped only after completion/cancellation is durably recorded. Workspace
    // snapshots are independent, so losing an ACK cannot lose the source input.
    payload: Option<InboxSubmitPayload>,
    result: InboxSubmitResult,
    note_sha256: String,
    note_bytes: usize,
}

pub struct ReceiptStore {
    directory: PathBuf,
    vault: PathBuf,
    gate: Mutex<()>,
    #[cfg(test)]
    fault: std::sync::atomic::AtomicU8,
}
impl ReceiptStore {
    pub fn new(directory: PathBuf, vault: PathBuf) -> Self {
        Self {
            directory,
            vault,
            gate: Mutex::new(()),
            #[cfg(test)]
            fault: std::sync::atomic::AtomicU8::new(0),
        }
    }
    fn path(&self, id: &str) -> Result<PathBuf, String> {
        if !valid_id(id) {
            return Err(error("INBOX_SUBMISSION_INVALID"));
        }
        Ok(self.directory.join(format!("{id}.receipt.dpapi")))
    }
    fn target(&self, id: &str, at: &DateTime<FixedOffset>, char_count: usize) -> InboxSubmitResult {
        let day = at.format("%Y-%m-%d").to_string();
        let filename = format!("{}-{id}.md", at.format("%H%M%S"));
        InboxSubmitResult {
            path: self
                .vault
                .join("raw/inbox")
                .join(&day)
                .join(&filename)
                .to_string_lossy()
                .into(),
            filename,
            day,
            char_count,
        }
    }
    async fn lock(&self) -> Result<fs::File, String> {
        crate::assistant_state_store::safe(&self.directory)
            .map_err(|_| error("INBOX_SUBMISSION_SAVE"))?;
        crate::legacy_isolation::ensure_test_path(&self.directory)?;
        crate::control_protocol::private_directory(&self.directory)
            .await
            .map_err(|_| error("INBOX_SUBMISSION_SAVE"))?;
        let path = self.directory.join("writer.lock");
        crate::assistant_state_store::safe(&path).map_err(|_| error("INBOX_SUBMISSION_SAVE"))?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|_| error("INBOX_SUBMISSION_SAVE"))?;
        crate::assistant_state_store::ordinary(&file)
            .map_err(|_| error("INBOX_SUBMISSION_SAVE"))?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| error("INBOX_SUBMISSION_BUSY"))?;
        Ok(file)
    }
    fn load(&self, id: &str) -> Result<Option<Receipt>, String> {
        let Some(bytes) = crate::assistant_state_store::read_bounded(&self.path(id)?, MAX_CIPHER)
            .map_err(|_| error("INBOX_SUBMISSION_READ"))?
        else {
            return Ok(None);
        };
        let plain = crate::wechat_crypto::crypt(&bytes, false)
            .map_err(|_| error("INBOX_SUBMISSION_READ"))?;
        if plain.len() > MAX_PLAIN {
            return Err(error("INBOX_SUBMISSION_LIMIT"));
        }
        let r: Receipt =
            serde_json::from_slice(&plain).map_err(|_| error("INBOX_SUBMISSION_READ"))?;
        if r.schema_version != SCHEMA || r.id != id {
            return Err(error("INBOX_SUBMISSION_READ"));
        }
        if r.vault != self.vault {
            return Err(error("INBOX_SUBMISSION_SOURCE_CHANGED"));
        }
        let at = DateTime::parse_from_rfc3339(&r.created_at)
            .map_err(|_| error("INBOX_SUBMISSION_READ"))?;
        let expected = self.target(id, &at, r.result.char_count);
        if Path::new(&expected.path) != Path::new(&r.result.path)
            || expected.filename != r.result.filename
            || expected.day != r.result.day
            || r.note_bytes > MAX_NOTE
            || r.result.char_count > MAX_TEXT
            || ![&r.fingerprint, &r.note_sha256].iter().all(|v| {
                v.len() == 64
                    && v.bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err(error("INBOX_SUBMISSION_READ"));
        }
        if let Some(p) = &r.payload {
            validate_payload(p)?;
            let md = render_inbox_md(p, &at);
            if p.submission_id.as_deref() != Some(id)
                || fingerprint(p)? != r.fingerprint
                || sha(md.as_bytes()) != r.note_sha256
                || md.len() != r.note_bytes
            {
                return Err(error("INBOX_SUBMISSION_READ"));
            }
        } else if matches!(r.phase, Phase::Prepared | Phase::Publishing) {
            return Err(error("INBOX_SUBMISSION_READ"));
        }
        Ok(Some(r))
    }
    async fn publish(&self, r: &Receipt) -> Result<(), String> {
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 1
            || (r.phase == Phase::Completed
                && self.fault.load(std::sync::atomic::Ordering::SeqCst) == 5)
        {
            return Err(error("INBOX_SUBMISSION_SAVE"));
        }
        let plain =
            Zeroizing::new(serde_json::to_vec(r).map_err(|_| error("INBOX_SUBMISSION_SAVE"))?);
        if plain.len() > MAX_PLAIN {
            return Err(error("INBOX_SUBMISSION_LIMIT"));
        }
        let bytes = crate::wechat_crypto::crypt(&plain, true)
            .map_err(|_| error("INBOX_SUBMISSION_SAVE"))?;
        crate::control_protocol::publish_private(&self.path(&r.id)?, &bytes)
            .await
            .map_err(|_| error("INBOX_SUBMISSION_SAVE"))
    }
    fn observe(&self, r: &Receipt) -> InboxSubmissionView {
        let mut view = InboxSubmissionView {
            submission_id: r.id.clone(),
            state: InboxSubmissionState::Prepared,
            file_state: None,
            result: None,
            message: "原收录意图已保存，尚未发布；恢复不会自动执行。".into(),
        };
        if r.phase == Phase::Canceled {
            view.state = InboxSubmissionState::Canceled;
            view.message = "这条原收录已在发布前取消，未删除任何笔记。".into();
            return view;
        }
        if r.phase == Phase::Prepared {
            return view;
        }
        let file =
            match crate::assistant_state_store::read_bounded(Path::new(&r.result.path), MAX_NOTE) {
                Ok(Some(bytes)) if bytes.len() == r.note_bytes && sha(&bytes) == r.note_sha256 => {
                    InboxSubmissionFileState::Present
                }
                Ok(Some(_)) => InboxSubmissionFileState::Modified,
                Ok(None) => InboxSubmissionFileState::Missing,
                Err(_) => InboxSubmissionFileState::Unreadable,
            };
        let confirmed = r.phase == Phase::Completed || file == InboxSubmissionFileState::Present;
        view.state = if confirmed {
            InboxSubmissionState::Completed
        } else {
            InboxSubmissionState::Unknown
        };
        view.file_state = Some(file.clone());
        if confirmed {
            view.result = Some(r.result.clone());
        }
        view.message = match (confirmed, file) {
            (true, InboxSubmissionFileState::Present) => "已核对原笔记，不会重新收录。",
            (true, InboxSubmissionFileState::Missing) => {
                "原收录已完成，但笔记目前不存在；未重新创建。"
            }
            (true, InboxSubmissionFileState::Modified) => {
                "原收录已完成，笔记后来已修改；未覆盖新内容。"
            }
            (true, InboxSubmissionFileState::Unreadable) => {
                "原收录已完成，但原文件当前无法核对；不把它当作可取结果。"
            }
            _ => "原收录已越过发布边界，结果尚未确认；保留原编号，只查询、不重复写入。",
        }
        .into();
        view
    }
    pub fn get(&self, id: &str) -> Result<InboxSubmissionView, String> {
        self.get_checked(id, None)
    }
    fn check_binding(r: &Receipt, expected: Option<&InboxSubmitPayload>) -> Result<(), String> {
        if let Some(payload) = expected {
            validate_payload(payload)?;
            if payload.submission_id.as_deref() != Some(&r.id)
                || fingerprint(payload)? != r.fingerprint
            {
                return Err(error("INBOX_SUBMISSION_CONFLICT"));
            }
        }
        Ok(())
    }
    pub fn get_for(
        &self,
        id: &str,
        payload: &InboxSubmitPayload,
    ) -> Result<InboxSubmissionView, String> {
        self.get_checked(id, Some(payload))
    }
    fn get_checked(
        &self,
        id: &str,
        expected: Option<&InboxSubmitPayload>,
    ) -> Result<InboxSubmissionView, String> {
        Ok(match self.load(id)? {
            Some(r) => {
                Self::check_binding(&r, expected)?;
                self.observe(&r)
            }
            None => InboxSubmissionView {
                submission_id: id.into(),
                state: InboxSubmissionState::NotFound,
                file_state: None,
                result: None,
                message: "未找到原收录回执，不代表未执行；不能自动换编号重做。".into(),
            },
        })
    }
    pub async fn prepare(
        &self,
        id: &str,
        payload: InboxSubmitPayload,
    ) -> Result<InboxSubmissionView, String> {
        validate_payload(&payload)?;
        crate::assistant_state_store::safe(&self.vault)
            .map_err(|_| error("INBOX_SUBMISSION_SOURCE_CHANGED"))?;
        if payload.submission_id.as_deref() != Some(id) {
            return Err(error("INBOX_SUBMISSION_CONFLICT"));
        }
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        if let Some(old) = self.load(id)? {
            if fingerprint(&payload)? != old.fingerprint {
                return Err(error("INBOX_SUBMISSION_CONFLICT"));
            }
            return Ok(self.observe(&old));
        }
        let mut count = 0;
        for entry in fs::read_dir(&self.directory).map_err(|_| error("INBOX_SUBMISSION_READ"))? {
            if entry
                .map_err(|_| error("INBOX_SUBMISSION_READ"))?
                .file_name()
                .to_string_lossy()
                .ends_with(".receipt.dpapi")
            {
                count += 1;
                if count >= MAX_RECEIPTS {
                    return Err(error("INBOX_SUBMISSION_LIMIT"));
                }
            }
        }
        let at = Local::now().fixed_offset();
        let md = render_inbox_md(&payload, &at);
        if md.len() > MAX_NOTE {
            return Err(error("INBOX_SUBMISSION_LIMIT"));
        }
        let r = Receipt {
            schema_version: SCHEMA,
            id: id.into(),
            vault: self.vault.clone(),
            created_at: at.to_rfc3339(),
            fingerprint: fingerprint(&payload)?,
            phase: Phase::Prepared,
            result: self.target(id, &at, payload.text.chars().count()),
            note_sha256: sha(md.as_bytes()),
            note_bytes: md.len(),
            payload: Some(payload),
        };
        self.publish(&r).await?;
        Ok(self.observe(&r))
    }
    pub async fn commit(&self, id: &str) -> Result<InboxSubmissionView, String> {
        self.commit_checked(id, None).await
    }
    pub async fn commit_for(
        &self,
        id: &str,
        payload: &InboxSubmitPayload,
    ) -> Result<InboxSubmissionView, String> {
        self.commit_checked(id, Some(payload)).await
    }
    async fn commit_checked(
        &self,
        id: &str,
        expected: Option<&InboxSubmitPayload>,
    ) -> Result<InboxSubmissionView, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut r = self
            .load(id)?
            .ok_or_else(|| error("INBOX_SUBMISSION_NOT_FOUND"))?;
        Self::check_binding(&r, expected)?;
        if r.phase != Phase::Prepared {
            let view = self.observe(&r);
            if r.phase == Phase::Publishing && view.state == InboxSubmissionState::Completed {
                r.phase = Phase::Completed;
                r.payload = None;
                self.publish(&r).await?;
            }
            return Ok(view);
        }
        let p = r
            .payload
            .as_ref()
            .ok_or_else(|| error("INBOX_SUBMISSION_READ"))?;
        crate::inbox_storage::references::validate_references(&self.vault, &p.attachments)
            .map_err(crate::inbox_storage::references::display_error)?;
        let path = Path::new(&r.result.path);
        crate::legacy_isolation::ensure_test_path(path)?;
        crate::assistant_state_store::safe(path)
            .map_err(|_| error("INBOX_SUBMISSION_NOT_PUBLISHED"))?;
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(error("INBOX_SUBMISSION_CONFLICT")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(error("INBOX_SUBMISSION_NOT_PUBLISHED")),
        }
        let parent = path
            .parent()
            .ok_or_else(|| error("INBOX_SUBMISSION_INVALID"))?;
        fs::create_dir_all(parent).map_err(|_| error("INBOX_SUBMISSION_NOT_PUBLISHED"))?;
        crate::assistant_state_store::safe(path)
            .map_err(|_| error("INBOX_SUBMISSION_NOT_PUBLISHED"))?;
        let parent =
            fs::canonicalize(parent).map_err(|_| error("INBOX_SUBMISSION_NOT_PUBLISHED"))?;
        let destination = parent.join(&r.result.filename);
        let at = DateTime::parse_from_rfc3339(&r.created_at)
            .map_err(|_| error("INBOX_SUBMISSION_READ"))?;
        let md = Zeroizing::new(render_inbox_md(p, &at));
        let mut temp = tempfile::NamedTempFile::new_in(&parent)
            .map_err(|_| error("INBOX_SUBMISSION_NOT_PUBLISHED"))?;
        temp.write_all(md.as_bytes())
            .and_then(|_| temp.as_file().sync_all())
            .map_err(|_| error("INBOX_SUBMISSION_NOT_PUBLISHED"))?;
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 2 {
            return Err(error("INBOX_SUBMISSION_NOT_PUBLISHED"));
        }
        r.phase = Phase::Publishing;
        self.publish(&r).await?;
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 3 {
            return Err(error("INBOX_SUBMISSION_UNCONFIRMED"));
        }
        // Never replace a destination, even if a different actor created it since
        // preflight. An error after this boundary stays unknown until queried.
        temp.persist_noclobber(&destination)
            .map_err(|_| error("INBOX_SUBMISSION_UNCONFIRMED"))?;
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 4 {
            return Err(error("INBOX_SUBMISSION_UNCONFIRMED"));
        }
        r.phase = Phase::Completed;
        r.payload = None;
        self.publish(&r).await?;
        Ok(self.observe(&r))
    }
    pub async fn submit(&self, payload: InboxSubmitPayload) -> Result<InboxSubmissionView, String> {
        let id = payload
            .submission_id
            .clone()
            .ok_or_else(|| error("INBOX_SUBMISSION_INVALID"))?;
        self.prepare(&id, payload.clone()).await?;
        self.commit_for(&id, &payload).await
    }
    pub async fn cancel(&self, id: &str) -> Result<InboxSubmissionView, String> {
        self.cancel_checked(id, None).await
    }
    pub async fn cancel_for(
        &self,
        id: &str,
        payload: &InboxSubmitPayload,
    ) -> Result<InboxSubmissionView, String> {
        self.cancel_checked(id, Some(payload)).await
    }
    async fn cancel_checked(
        &self,
        id: &str,
        expected: Option<&InboxSubmitPayload>,
    ) -> Result<InboxSubmissionView, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut r = self
            .load(id)?
            .ok_or_else(|| error("INBOX_SUBMISSION_NOT_FOUND"))?;
        Self::check_binding(&r, expected)?;
        if r.phase == Phase::Canceled {
            return Ok(self.observe(&r));
        }
        if r.phase != Phase::Prepared {
            return Err(error("INBOX_SUBMISSION_CANCEL_BLOCKED"));
        }
        r.phase = Phase::Canceled;
        r.payload = None;
        self.publish(&r).await?;
        Ok(self.observe(&r))
    }
    /// Persist only a verified observation; prepared never advances to publish.
    pub async fn resolve(&self, id: &str) -> Result<InboxSubmissionView, String> {
        self.resolve_checked(id, None).await
    }
    pub async fn resolve_for(
        &self,
        id: &str,
        payload: &InboxSubmitPayload,
    ) -> Result<InboxSubmissionView, String> {
        self.resolve_checked(id, Some(payload)).await
    }
    async fn resolve_checked(
        &self,
        id: &str,
        expected: Option<&InboxSubmitPayload>,
    ) -> Result<InboxSubmissionView, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut r = self
            .load(id)?
            .ok_or_else(|| error("INBOX_SUBMISSION_NOT_FOUND"))?;
        Self::check_binding(&r, expected)?;
        let view = self.observe(&r);
        if r.phase == Phase::Publishing && view.state == InboxSubmissionState::Completed {
            r.phase = Phase::Completed;
            r.payload = None;
            self.publish(&r).await?;
        }
        Ok(view)
    }
}

#[cfg(test)]
#[path = "inbox_receipt_tests.rs"]
mod tests;
