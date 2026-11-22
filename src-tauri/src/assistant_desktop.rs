//! Client drafts and submission receipts, independent of business execution state.
//! Each scope has one bounded encrypted file; typing never rewrites all sessions.
use crate::assistant::{request_fingerprint, Ack, ImageReference, SendRequest, TextAttachment};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

const SCHEMA: u32 = 1;
pub(crate) const MAX_SCOPES: usize = 200;
const MAX_CIPHER: u64 = 1024 * 1024;
const MAX_PLAIN: usize = 512 * 1024;
// A private, unsent draft is not an execution request. Keep the two budgets
// separate so multi-byte text can be saved before the user splits the message.
pub(super) const MAX_DRAFT_BYTES: usize = 64 * 1024;
const SUFFIX: &str = ".draft.dpapi";
#[path = "assistant_voice_state.rs"]
mod voice_state;
pub use voice_state::{VoiceJob, VoicePhase, VoiceRecord};
#[path = "assistant_native_state.rs"]
mod native_state;
pub use native_state::NativeMessage;
#[path = "assistant_handoff_store.rs"]
mod handoff_store;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DesktopBody {
    pub text: String,
    pub route: String,
    pub mode: String,
    pub files: Vec<TextAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageReference>,
}
impl Default for DesktopBody {
    fn default() -> Self {
        Self {
            text: String::new(),
            route: String::new(),
            mode: "auto".into(),
            files: vec![],
            image: None,
        }
    }
}
impl DesktopBody {
    fn validate(&self, scope: &str) -> Result<(), String> {
        if self.text.len() > MAX_DRAFT_BYTES {
            return Err("草稿正文超过64 KiB，尚未保存；已保存的原稿保留，请先缩短内容".into());
        }
        if self.route.len() > 300
            || self.route.chars().any(char::is_control)
            || !["auto", "chat", "task"].contains(&self.mode.as_str())
        {
            return Err("草稿正文、模型或消息方式无效，未覆盖已保存内容".into());
        }
        crate::assistant::input::prepare(&self.text, &self.files)?;
        if let Some(image) = &self.image {
            image.validate(scope)?;
            if self.mode != "chat" || !self.files.is_empty() {
                return Err("本机图片只可用于只聊聊模式，并须与文本附件分别发送".into());
            }
        }
        Ok(())
    }
    fn from_request(request: &SendRequest) -> Self {
        Self {
            text: request.text.clone(),
            route: request.route_id.clone().unwrap_or_default(),
            mode: request.mode.clone().unwrap_or_else(|| "auto".into()),
            files: request.attachments.clone(),
            image: request.image.clone(),
        }
    }
    fn matches(&self, request: &SendRequest) -> bool {
        self.text == request.text
            && self.route == request.route_id.as_deref().unwrap_or("")
            && self.mode == request.mode.as_deref().unwrap_or("auto")
            && self.image == request.image
            && self.files.len() == request.attachments.len()
            && self
                .files
                .iter()
                .zip(&request.attachments)
                .all(|(a, b)| a == b)
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopPending {
    pub request: SendRequest,
    /// prepared means acceptance may be unknown, never automatically resubmit.
    pub state: String,
    pub error: Option<String>,
    fingerprint: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesktopEntry {
    pub schema_version: u32,
    pub scope: String,
    pub revision: u64,
    pub body: DesktopBody,
    pub pending: Option<DesktopPending>,
    #[serde(default)]
    pub native_pending: Option<NativeMessage>,
    #[serde(default)]
    pub voices: Vec<VoiceRecord>,
    pub updated_at: Option<String>,
}

pub(crate) struct ArchivedDraft {
    pub name: String,
    pub contents: Option<(DesktopEntry, Vec<u8>)>,
}
impl DesktopEntry {
    fn empty(scope: &str) -> Self {
        Self {
            schema_version: SCHEMA,
            scope: scope.into(),
            revision: 0,
            body: DesktopBody::default(),
            pending: None,
            native_pending: None,
            voices: vec![],
            updated_at: None,
        }
    }
    fn changed(&mut self) -> Result<(), String> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or("草稿版本超出范围，原文件保留")?;
        self.updated_at = Some(chrono::Utc::now().to_rfc3339());
        Ok(())
    }
}

#[derive(Serialize)]
pub struct DesktopMeta {
    pub scope: String,
    pub revision: u64,
    pub has_draft: bool,
    pub pending_state: Option<String>,
    pub voice_pending_count: usize,
    pub route: String,
    pub updated_at: Option<String>,
}
#[derive(Serialize)]
pub struct DesktopIndex {
    pub entries: Vec<DesktopMeta>,
    pub unreadable: usize,
    pub complete: bool,
}

#[derive(Serialize)]
pub struct DesktopReply {
    pub status: String,
    pub code: Option<String>,
    pub message: String,
    pub entry: Option<DesktopEntry>,
    pub index: Option<DesktopIndex>,
    pub ack: Option<Ack>,
    pub native_result: Option<serde_json::Value>,
    pub warning: Option<String>,
}
impl DesktopReply {
    pub fn entry(status: &str, entry: DesktopEntry) -> Self {
        Self {
            status: status.into(),
            entry: Some(entry),
            index: None,
            ack: None,
            native_result: None,
            code: None,
            message: String::new(),
            warning: None,
        }
    }
    pub fn failure(code: &str, message: String) -> Self {
        Self {
            status: "failed".into(),
            code: Some(code.into()),
            message,
            entry: None,
            index: None,
            ack: None,
            native_result: None,
            warning: None,
        }
    }
    pub fn index(index: DesktopIndex) -> Self {
        let incomplete = !index.complete;
        Self {
            status: if incomplete { "partial" } else { "succeeded" }.into(),
            code: if incomplete {
                Some("DRAFTS_PARTIAL".into())
            } else {
                None
            },
            message: if incomplete {
                "有草稿无法读取，原文件保留；这不是空数据"
            } else {
                ""
            }
            .into(),
            index: Some(index),
            entry: None,
            ack: None,
            native_result: None,
            warning: None,
        }
    }
}

pub struct DesktopStore {
    directory: PathBuf,
    gate: Mutex<()>,
    #[cfg(test)]
    fail_publish: std::sync::atomic::AtomicBool,
}
impl DesktopStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            gate: Mutex::new(()),
            #[cfg(test)]
            fail_publish: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub(crate) fn validate_scope(scope: &str) -> Result<(), String> {
        if let Some(id)=scope.strip_prefix("handoff:") {
            return if uuid::Uuid::parse_str(id).is_ok_and(|value|value.to_string()==id) { Ok(()) }
                else { Err("交接暂存编号无效".into()) };
        }
        if let Some(native) = scope.strip_prefix("native:") {
            if let Some((provider, id)) = native.split_once(':') {
                if ["codex", "claude-code", "gemini-cli"].contains(&provider)
                    && !id.is_empty()
                    && id.len() <= 120
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
                {
                    return Ok(());
                }
            }
            return Err("原生草稿会话编号无效".into());
        }
        if scope == "new" || uuid::Uuid::parse_str(scope).is_ok_and(|id| id.to_string() == scope) {
            Ok(())
        } else {
            Err("草稿会话编号无效".into())
        }
    }
    fn path(&self, scope: &str) -> Result<PathBuf, String> {
        Self::validate_scope(scope)?;
        Ok(self
            .directory
            .join(format!("{:x}{SUFFIX}", Sha256::digest(scope.as_bytes()))))
    }
    fn read_file(&self, path: &std::path::Path) -> Result<DesktopEntry, String> {
        let mut file = fs::File::open(path).map_err(|_| "草稿密文不可读，原文件未改动")?;
        let mut cipher = Vec::new();
        file.by_ref()
            .take(MAX_CIPHER + 1)
            .read_to_end(&mut cipher)
            .map_err(|_| "草稿密文读取失败")?;
        if cipher.len() as u64 > MAX_CIPHER {
            return Err("草稿密文超过上限，未读取或覆盖".into());
        }
        self.decode_file(path, &cipher)
    }
    fn decode_file(&self, path: &std::path::Path, cipher: &[u8]) -> Result<DesktopEntry, String> {
        let plain = crate::wechat_crypto::crypt(cipher, false)
            .map_err(|_| "草稿无法解密，未回退为空或覆盖原文件")?;
        if plain.len() > MAX_PLAIN {
            return Err("草稿内容超过上限，原文件保留".into());
        }
        let entry: DesktopEntry =
            serde_json::from_slice(&plain).map_err(|_| "草稿格式无效，原文件保留")?;
        if entry.schema_version != SCHEMA || entry.revision == 0 {
            return Err("草稿版本不受支持，原文件保留".into());
        }
        if entry.scope.starts_with("handoff:") && (entry.pending.is_some() || entry.native_pending.is_some() || !entry.voices.is_empty() || !entry.body.route.is_empty() || entry.body.mode != "auto" || !entry.body.files.is_empty()) {
            return Err("交接暂存只能保存文字，不能包含执行回执、模型或录音状态".into());
        }
        if self.path(&entry.scope)? != path
            && (!Self::completed_handoff(&entry)
                || self.completed_handoff_path(&entry.scope)? != path)
        {
            return Err("草稿归属不匹配，未关联其他会话内容".into());
        }
        entry.body.validate(&entry.scope)?;
        voice_state::validate(&entry.voices)?;
        if let Some(request) = &entry.native_pending {
            request.validate()?;
            if request.scope() != entry.scope || entry.pending.is_some() {
                return Err("原生待核对请求归属无效，原文件保留".into());
            }
        }
        if let Some(pending) = &entry.pending {
            Self::validate_request(&entry.scope, &pending.request)?;
            if !["prepared", "rejected"].contains(&pending.state.as_str())
                || pending.fingerprint != request_fingerprint(&pending.request, "desktop")?
            {
                return Err("待核对请求格式无效，原文件保留".into());
            }
        }
        Ok(entry)
    }

    pub(crate) fn archive_filename(name: &str) -> bool {
        name.strip_suffix(SUFFIX).is_some_and(|hash| {
            hash.len() == 64
                && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
    }

    pub(crate) fn read_archived(&self, name: &str, cipher: &[u8]) -> Result<DesktopEntry, String> {
        if !Self::archive_filename(name) || cipher.len() as u64 > MAX_CIPHER {
            return Err("草稿备份文件格式无效".into());
        }
        self.decode_file(&self.directory.join(name), cipher)
    }

    pub(crate) fn archive_drafts(
        &self,
        mut visit: impl FnMut(ArchivedDraft) -> Result<(), String>,
    ) -> Result<(), String> {
        use crate::assistant_state_store::{read_bounded, safe};
        safe(&self.directory)?;
        let mut paths = self.files()?;
        paths.sort();
        for path in paths {
            let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("");
            let contents = if Self::archive_filename(name) {
                read_bounded(&path, MAX_CIPHER as usize)
                    .ok()
                    .flatten()
                    .and_then(|cipher| self.read_archived(name, &cipher).ok().map(|entry| (entry, cipher)))
            } else {
                None
            };
            visit(ArchivedDraft {
                name: if Self::archive_filename(name) { name.to_owned() } else { "无法识别的草稿".into() },
                contents,
            })?;
        }
        Ok(())
    }
    pub fn get(&self, scope: &str) -> Result<DesktopEntry, String> {
        let path = self.path(scope)?;
        match fs::metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(self.read_completed_handoff(scope)?.unwrap_or_else(|| DesktopEntry::empty(scope)))
            },
            Err(_) => Err("草稿状态不可读取，未回退为空".into()),
            Ok(_) => {
                let entry = self.read_file(&path)?;
                if let Some(completed) = self.read_completed_handoff(scope)? {
                    if completed.revision >= entry.revision {
                        return Err("活跃交接版本未晚于已完成记录，未采用过时副本或重置版本".into());
                    }
                }
                Ok(entry)
            },
        }
    }
    fn files(&self) -> Result<Vec<PathBuf>, String> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err("草稿目录不可读，未返回空列表".into()),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| "草稿目录读取不完整")?;
            if entry.file_name().to_string_lossy().ends_with(SUFFIX) {
                paths.push(entry.path());
                if paths.len() > MAX_SCOPES {
                    return Err("草稿目录超过安全上限，未截断或删除原数据".into());
                }
            }
        }
        Ok(paths)
    }
    pub fn index(&self) -> Result<DesktopIndex, String> {
        let mut entries = Vec::new();
        let mut unreadable = 0;
        for path in self.files()? {
            match self.read_file(&path) {
                Ok(entry) => entries.push(DesktopMeta {
                    scope: entry.scope,
                    revision: entry.revision,
                    has_draft: !entry.body.text.is_empty() || !entry.body.files.is_empty() || entry.body.image.is_some(),
                    pending_state: entry
                        .pending
                        .map(|p| p.state)
                        .or_else(|| entry.native_pending.map(|_| "prepared".into())),
                    voice_pending_count: entry
                        .voices
                        .iter()
                        .filter(|v| !matches!(v.phase, VoicePhase::Applied | VoicePhase::Dismissed))
                        .count(),
                    route: entry.body.route,
                    updated_at: entry.updated_at,
                }),
                Err(_) => unreadable += 1,
            }
        }
        entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(DesktopIndex {
            entries,
            unreadable,
            complete: unreadable == 0,
        })
    }
    fn lock(&self) -> Result<fs::File, String> {
        fs::create_dir_all(&self.directory).map_err(|_| "草稿目录不可创建")?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join("workspace.lock"))
            .map_err(|_| "草稿写入锁不可用")?;
        fs2::FileExt::try_lock_exclusive(&file)
            .map_err(|_| "另一窗口正在保存，请稍后重试；原稿未覆盖")?;
        Ok(file)
    }
    async fn publish(&self, entry: &DesktopEntry) -> Result<(), String> {
        let path = self.path(&entry.scope)?;
        if !path.exists() && self.files()?.len() >= MAX_SCOPES {
            return Err("草稿数量已达上限，未删除原内容".into());
        }
        let plain = Zeroizing::new(serde_json::to_vec(entry).map_err(|_| "草稿编码失败")?);
        if plain.len() > MAX_PLAIN {
            return Err("草稿编码内容超过上限，未覆盖原稿".into());
        }
        let cipher =
            crate::wechat_crypto::crypt(&plain, true).map_err(|_| "草稿加密失败，未保存")?;
        #[cfg(test)]
        if self.fail_publish.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("合成草稿写入失败".into());
        }
        crate::control_protocol::publish_private(&path, &cipher)
            .await
            .map_err(|_| "草稿安全保存失败，原文件保留".into())
    }
    pub async fn save(
        &self,
        scope: &str,
        revision: u64,
        body: DesktopBody,
    ) -> Result<DesktopEntry, String> {
        body.validate(scope)?;
        if scope.starts_with("handoff:") && (!body.route.is_empty() || body.mode != "auto" || !body.files.is_empty()) {
            return Err("交接暂存只支持文字，不接受模型、附件或执行方式".into());
        }
        if scope.starts_with("native:")
            && (!body.files.is_empty() || !body.route.is_empty() || body.mode != "auto")
        {
            return Err("原生输入当前只支持文字草稿；附件、模型和消息方式须由原生适配器支持，未保存或静默忽略这些字段".into());
        }
        let _gate = self.gate.lock().await;
        let _file_lock = self.lock()?;
        let mut entry = self.get(scope)?;
        if entry.body == body {
            self.finish_completed_handoff(&entry)?;
            return Ok(entry);
        }
        if entry.revision != revision {
            return Err("草稿已被另一窗口更新，未覆盖；请先核对已保存内容".into());
        }
        entry.body = body;
        entry.changed()?;
        self.publish(&entry).await?;
        self.finish_completed_handoff(&entry)?;
        Ok(entry)
    }
    fn validate_request(scope: &str, request: &SendRequest) -> Result<(), String> {
        Self::validate_scope(scope)?;
        if scope.starts_with("handoff:") {
            return Err("交接暂存不能发送或执行任务，请先转入真实桌面会话".into());
        }
        if request.text.len() > 8000 {
            return Err("消息不能超过8000字节；长草稿仍保留，请缩短或改为文本附件后发送".into());
        }
        if request.session_id.as_deref().unwrap_or("new") != scope
            || request.request_id.is_empty()
            || request.request_id.len() > 120
            || request.request_id.chars().any(char::is_control)
            || (request.text.trim().is_empty() && request.attachments.is_empty())
        {
            return Err("发送回执的会话、内容或请求编号无效".into());
        }
        DesktopBody::from_request(request).validate(scope)
    }
    pub async fn prepare(
        &self,
        scope: &str,
        revision: u64,
        request: SendRequest,
    ) -> Result<DesktopEntry, String> {
        Self::validate_request(scope, &request)?;
        let fingerprint = request_fingerprint(&request, "desktop")?;
        let _gate = self.gate.lock().await;
        let _file_lock = self.lock()?;
        let mut entry = self.get(scope)?;
        if let Some(old) = &entry.pending {
            if old.request.request_id == request.request_id && old.fingerprint != fingerprint {
                return Err("同一请求编号不能更换消息、模式、模型或附件".into());
            }
            if old.state == "prepared" {
                if old.request.request_id == request.request_id && old.fingerprint == fingerprint {
                    return Ok(entry);
                }
                return Err("上一条发送尚待核对，请查询原请求或新建对话；原请求不会被丢弃".into());
            }
        }
        if entry.revision != revision {
            return Err("草稿版本已更新，未执行发送".into());
        }
        entry.body = DesktopBody::from_request(&request);
        entry.pending = Some(DesktopPending {
            request,
            state: "prepared".into(),
            error: None,
            fingerprint,
        });
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }
    pub async fn rejected(
        &self,
        scope: &str,
        request_id: &str,
        message: &str,
    ) -> Result<DesktopEntry, String> {
        let _gate = self.gate.lock().await;
        let _file_lock = self.lock()?;
        let mut entry = self.get(scope)?;
        let pending = entry
            .pending
            .as_mut()
            .ok_or("发送回执不存在，未清理任何内容")?;
        if pending.request.request_id != request_id {
            return Err("发送回执已变化，原回执保留".into());
        }
        pending.state = "rejected".into();
        pending.error = Some(message.chars().take(500).collect());
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }
    /// Only call with a verified Engine receipt; callers cannot supply an ACK.
    pub async fn acknowledged(
        &self,
        scope: &str,
        request_id: &str,
        ack: &Ack,
    ) -> Result<DesktopEntry, String> {
        Self::validate_scope(&ack.session_id)?;
        let _gate = self.gate.lock().await;
        let _file_lock = self.lock()?;
        let mut entry = self.get(scope)?;
        let Some(pending) = entry.pending.as_ref() else {
            return Ok(entry);
        };
        if pending.request.request_id != request_id
            || pending
                .request
                .session_id
                .as_ref()
                .is_some_and(|s| s != &ack.session_id)
        {
            return Err("原请求与实际受理会话不一致，未清理草稿".into());
        }
        if scope == "new" {
            let mut destination = self.get(&ack.session_id)?;
            if destination.revision == 0 {
                destination.body.route = pending.request.route_id.clone().unwrap_or_default();
                destination.body.mode = pending
                    .request
                    .mode
                    .clone()
                    .unwrap_or_else(|| "auto".into());
                destination.changed()?;
                // Publish preferences first. A crash leaves the original pending
                // receipt intact, so recovery still checks rather than resends.
                self.publish(&destination).await?;
            }
        }
        if entry.body.matches(&pending.request) {
            entry.body.text.clear();
            entry.body.files.clear();
            entry.body.image = None;
        }
        entry.pending = None;
        entry.changed()?;
        self.publish(&entry).await?;
        Ok(entry)
    }

    pub(crate) async fn accept(
        &self,
        scope: &str,
        request: &SendRequest,
        ack: Ack,
    ) -> DesktopReply {
        let mut reply = match self.acknowledged(scope, &request.request_id, &ack).await {
            Ok(entry) => DesktopReply::entry("accepted", entry),
            Err(message) => {
                let mut reply = DesktopReply::failure(
                    "LOCAL_ACK_SAVE_FAILED",
                    "消息已受理，但本地草稿清理未确认；请查询原任务，不要换编号重做".into(),
                );
                reply.status = "partial".into();
                reply.warning = Some(message);
                reply.entry = self.get(scope).ok();
                reply
            }
        };
        reply.ack = Some(ack);
        reply
    }
    /// Check a known receipt only. A missing receipt is never a submission.
    pub async fn recover(&self, engine: &crate::assistant::Engine, scope: &str) -> DesktopReply {
        if let Err(message) = engine.desktop_scope(scope).await {
            return DesktopReply::failure("SCOPE_INVALID", message);
        }
        let entry = match self.get(scope) {
            Ok(entry) => entry,
            Err(message) => return DesktopReply::failure("DRAFT_READ_FAILED", message),
        };
        let Some(pending) = &entry.pending else {
            return DesktopReply::entry("loaded", entry);
        };
        match engine.desktop_receipt(&pending.request).await {
            Ok(Some(ack)) => self.accept(scope, &pending.request, ack).await,
            Ok(None) => {
                let mut reply = DesktopReply::entry("not_found", entry);
                reply.message =
                    "尚未查到原请求；不代表已取消。可按原编号重试，或新建对话，旧请求会保留".into();
                reply
            }
            Err(message) => DesktopReply::failure("RECEIPT_CONFLICT", message),
        }
    }
    /// Persist the original request before the shared engine may execute it.
    pub async fn transmit(
        &self,
        engine: &std::sync::Arc<crate::assistant::Engine>,
        revision: u64,
        request: SendRequest,
    ) -> DesktopReply {
        let scope = request.session_id.as_deref().unwrap_or("new").to_string();
        if let Err(message) = engine.desktop_scope(&scope).await {
            return DesktopReply::failure("SCOPE_INVALID", message);
        }
        // The previous response may have been lost after both durable ACKs. Read
        // the engine first, so an old client revision cannot cause a fresh send.
        match engine.desktop_receipt(&request).await {
            Ok(Some(ack)) => return self.accept(&scope, &request, ack).await,
            Err(message) => return DesktopReply::failure("RECEIPT_CONFLICT", message),
            Ok(None) => {}
        }
        let prepared = match self.prepare(&scope, revision, request.clone()).await {
            Ok(entry) => entry,
            Err(message) => return DesktopReply::failure("DRAFT_PREPARE_FAILED", message),
        };
        match engine.send(request.clone()).await {
            Ok(ack) => self.accept(&scope, &request, ack).await,
            Err(message) => {
                match engine.desktop_receipt(&request).await {
                    Ok(Some(ack)) => self.accept(&scope, &request, ack).await,
                    Ok(None) => {
                        // Engine::send has finished without an accepted receipt.
                        // Mark this specific attempt rejected, without deleting it.
                        let mut reply = DesktopReply::failure("SEND_NOT_ACCEPTED", message.clone());
                        match self.rejected(&scope, &request.request_id, &message).await {
                            Ok(entry) => reply.entry = Some(entry),
                            Err(error) => {
                                reply.entry = Some(prepared);
                                reply.warning = Some(error);
                            }
                        }
                        reply
                    }
                    Err(error) => {
                        let mut reply = DesktopReply::failure(
                            "RECEIPT_UNKNOWN",
                            "发送受理情况尚未确认，原请求已保留，未重新执行".into(),
                        );
                        reply.entry = Some(prepared);
                        reply.warning = Some(error);
                        reply
                    }
                }
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn fail_writes(&self, fail: bool) {
        self.fail_publish
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    fn body(text: &str) -> DesktopBody {
        DesktopBody {
            image: None,
            text: text.into(),
            ..DesktopBody::default()
        }
    }
    fn request(scope: &str, id: &str) -> SendRequest {
        SendRequest {
            image: None,
            session_id: if scope == "new" {
                None
            } else {
                Some(scope.into())
            },
            text: "隔离草稿".into(),
            request_id: id.into(),
            mode: None,
            route_id: Some("openrouter::fixture:free".into()),
            attachments: vec![TextAttachment {
                name: "test.md".into(),
                content: "隔离附件".into(),
                document: None,
            }],
        }
    }
    #[tokio::test]
    async fn handoff_outbox_is_encrypted_restartable_data_and_cannot_prepare_execution() {
        let root = tempfile::tempdir().unwrap();
        let scope = format!("handoff:{}", uuid::Uuid::new_v4());
        let store = DesktopStore::new(root.path().join("drafts"));
        assert_eq!(store.get(&scope).unwrap().revision, 0);
        assert!(!store.directory.exists());
        let saved = store.save(&scope, 0, body("仅用于隔离测试的交接原文")).await.unwrap();
        let cipher = fs::read(store.path(&scope).unwrap()).unwrap();
        assert!(!cipher.windows("交接原文".len()).any(|w| w == "交接原文".as_bytes()));
        let restarted = DesktopStore::new(store.directory.clone());
        assert_eq!(restarted.get(&scope).unwrap().body.text, saved.body.text);
        assert!(restarted.index().unwrap().entries.iter().any(|v| v.scope == scope && v.has_draft));
        assert!(restarted.prepare(&scope, saved.revision, request(&scope, "must-not-run")).await.is_err());
        let mut configured = body("不能混入执行选项");
        configured.mode = "task".into();
        assert!(restarted.save(&scope, saved.revision, configured).await.is_err());
        assert!(restarted.save(&scope, 0, body("过时覆盖")).await.is_err());
        assert_eq!(fs::read(restarted.path(&scope).unwrap()).unwrap(), cipher);
        for invalid in ["handoff:../escape", "handoff:not-an-id", "handoff:12345678-ABCD-1234-1234-123456789012"] {
            assert!(restarted.get(invalid).is_err());
        }
        let mut entry = restarted.get(&scope).unwrap();
        entry.body.route = "unexpected-execution-route".into();
        let forged = crate::wechat_crypto::crypt(&serde_json::to_vec(&entry).unwrap(), true).unwrap();
        fs::write(restarted.path(&scope).unwrap(), &forged).unwrap();
        assert!(restarted.get(&scope).is_err());
        assert!(!restarted.index().unwrap().complete);
        assert_eq!(fs::read(restarted.path(&scope).unwrap()).unwrap().as_slice(), forged.as_slice());
    }

    #[tokio::test]
    async fn desktop_drafts_encrypt_restart_and_cas_without_creating_state_on_read() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("desktop");
        let store = DesktopStore::new(directory.clone());
        assert_eq!(store.get("new").unwrap().revision, 0);
        assert!(store.index().unwrap().complete);
        assert!(!directory.exists());
        let first = store
            .save("new", 0, body("不能出现在明文中"))
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        let bytes = fs::read(store.path("new").unwrap()).unwrap();
        assert!(!bytes
            .windows("不能出现在明文中".len())
            .any(|w| w == "不能出现在明文中".as_bytes()));
        let restored = DesktopStore::new(directory);
        assert_eq!(restored.get("new").unwrap().body.text, first.body.text);
        assert_eq!(
            restored
                .save("new", 0, body("不能出现在明文中"))
                .await
                .unwrap()
                .revision,
            1
        );
        assert!(restored.save("new", 0, body("过时覆盖")).await.is_err());
        assert_eq!(restored.get("new").unwrap().body.text, first.body.text);
        assert!(store.get("../not-owned").is_err());
    }
    #[tokio::test]
    async fn desktop_long_drafts_are_distinct_from_execution_budget() {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().to_owned());
        let long = "中文草稿🙂".repeat(1500);
        assert!(long.len() > 8000 && long.len() < MAX_DRAFT_BYTES);
        let saved = store.save("new", 0, body(&long)).await.unwrap();
        let before = fs::read(store.path("new").unwrap()).unwrap();
        let restarted = DesktopStore::new(root.path().to_owned());
        assert_eq!(restarted.get("new").unwrap().body.text, long);
        let mut too_large = request("new", "must-not-prepare");
        too_large.text = long.clone();
        assert!(restarted
            .prepare("new", saved.revision, too_large)
            .await
            .err()
            .unwrap()
            .contains("8000"));
        assert_eq!(fs::read(store.path("new").unwrap()).unwrap(), before);
        assert!(restarted.get("new").unwrap().pending.is_none());
        assert!(restarted
            .save(
                "new",
                saved.revision,
                body(&"a".repeat(MAX_DRAFT_BYTES + 1))
            )
            .await
            .is_err());
        assert_eq!(fs::read(store.path("new").unwrap()).unwrap(), before);
        let scope = "native:claude-code:opaque_test_thread";
        restarted.save(scope, 0, body(&long)).await.unwrap();
        assert_eq!(
            DesktopStore::new(root.path().to_owned())
                .get(scope)
                .unwrap()
                .body
                .text,
            long
        );
        // The precise byte boundary is inclusive, independently of code points.
        restarted
            .save("new", saved.revision, body(&"a".repeat(MAX_DRAFT_BYTES)))
            .await
            .unwrap();
        assert_eq!(
            restarted.get("new").unwrap().body.text.len(),
            MAX_DRAFT_BYTES
        );
    }
    #[tokio::test]
    async fn desktop_pending_never_expires_or_replaces_an_unknown_request() {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().to_owned());
        let req = request("new", "original");
        let prepared = store.prepare("new", 0, req.clone()).await.unwrap();
        assert_eq!(
            store.prepare("new", 0, req.clone()).await.unwrap().revision,
            prepared.revision
        );
        let mut changed = req.clone();
        changed.text.push_str("修改");
        assert!(store
            .prepare("new", prepared.revision, changed)
            .await
            .is_err());
        assert!(store
            .prepare("new", prepared.revision, request("new", "different-id"))
            .await
            .is_err());
        let restarted = DesktopStore::new(root.path().to_owned());
        assert_eq!(
            restarted
                .get("new")
                .unwrap()
                .pending
                .unwrap()
                .request
                .request_id,
            "original"
        );
        let rejected = store
            .rejected("new", "original", "明确未受理")
            .await
            .unwrap();
        assert!(store
            .prepare("new", rejected.revision, request("new", "corrected"))
            .await
            .is_ok());
    }
    #[tokio::test]
    async fn desktop_ack_preserves_newer_edits_and_moves_model_choice_to_actual_session() {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().to_owned());
        let req = request("new", "original");
        let prepared = store.prepare("new", 0, req.clone()).await.unwrap();
        store
            .save("new", prepared.revision, body("另一窗口的新草稿"))
            .await
            .unwrap();
        let target = uuid::Uuid::new_v4().to_string();
        let ack = Ack {
            session_id: target.clone(),
            turn_id: "original-turn".into(),
            state: "queued".into(),
        };
        let entry = store.acknowledged("new", "original", &ack).await.unwrap();
        assert!(entry.pending.is_none());
        assert_eq!(entry.body.text, "另一窗口的新草稿");
        assert_eq!(
            store.get(&target).unwrap().body.route,
            req.route_id.unwrap()
        );
        assert!(store.get(&target).unwrap().body.text.is_empty());
    }
    #[tokio::test]
    async fn desktop_corrupt_cipher_and_publish_failure_preserve_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = DesktopStore::new(root.path().to_owned());
        store.save("new", 0, body("原内容")).await.unwrap();
        let original = fs::read(store.path("new").unwrap()).unwrap();
        store
            .fail_publish
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(store
            .prepare("new", 1, request("new", "not-sent"))
            .await
            .is_err());
        assert_eq!(fs::read(store.path("new").unwrap()).unwrap(), original);
        fs::write(store.path("new").unwrap(), b"broken cipher").unwrap();
        assert!(store.get("new").is_err());
        assert!(store.save("new", 1, body("不能覆盖")).await.is_err());
        let index = store.index().unwrap();
        assert_eq!(index.unreadable, 1);
        assert!(!index.complete);
        assert_eq!(
            fs::read(store.path("new").unwrap()).unwrap(),
            b"broken cipher"
        );
    }
    #[tokio::test]
    async fn desktop_two_stores_cannot_overwrite_the_same_revision() {
        let root = tempfile::tempdir().unwrap();
        let one = DesktopStore::new(root.path().to_owned());
        let two = DesktopStore::new(root.path().to_owned());
        one.save("new", 0, body("最初")).await.unwrap();
        let (a, b) = tokio::join!(
            one.save("new", 1, body("版本A")),
            two.save("new", 1, body("版本B"))
        );
        assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
        assert_eq!(one.get("new").unwrap().revision, 2);
    }
}
