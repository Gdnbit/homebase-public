//! Private editor/outbox only. Loading or archiving never submits development.
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

// Leave room for the control service's 2 MiB result envelope as well as IPC.
const MAX_PLAIN: usize = 1024 * 1024;
const MAX_CIPHER: usize = 2 * 1024 * 1024;
const MAX_REQUESTS: usize = 128;

pub fn public_error(value: &str) -> Option<(&'static str, &'static str)> {
    let (code, supplied) = value.split_once(": ").unwrap_or((value, ""));
    let message = match code {
        "DEVELOPMENT_WORKSPACE_READ" => {
            "开发草稿无法完整读取或解密，原记录保留，不能按空草稿处理。"
        }
        "DEVELOPMENT_WORKSPACE_SAVE" => "开发草稿或原请求尚未确认保存，输入保留，未启动开发。",
        "DEVELOPMENT_WORKSPACE_CONFLICT" => {
            "开发草稿已被另一窗口更新，当前输入与已保存版本均保留，请先核对。"
        }
        "DEVELOPMENT_WORKSPACE_PENDING" => {
            "已有原开发请求需要核对；请查询原编号，或明确新建另一个任务。"
        }
        "DEVELOPMENT_WORKSPACE_ID" => "原开发请求编号或内容不一致，未替换或执行。",
        "DEVELOPMENT_WORKSPACE_INVALID" => "开发输入字段或原请求无效，原记录保持，未执行。",
        "DEVELOPMENT_WORKSPACE_LIMIT" => {
            "开发草稿或保留请求已到安全上限，未截断、删除或覆盖原记录。"
        }
        _ => return None,
    };
    if !supplied.is_empty() && supplied != message {
        return None;
    }
    Some((
        match code {
            "DEVELOPMENT_WORKSPACE_READ" => "DEVELOPMENT_WORKSPACE_READ",
            "DEVELOPMENT_WORKSPACE_SAVE" => "DEVELOPMENT_WORKSPACE_SAVE",
            "DEVELOPMENT_WORKSPACE_CONFLICT" => "DEVELOPMENT_WORKSPACE_CONFLICT",
            "DEVELOPMENT_WORKSPACE_PENDING" => "DEVELOPMENT_WORKSPACE_PENDING",
            "DEVELOPMENT_WORKSPACE_ID" => "DEVELOPMENT_WORKSPACE_ID",
            "DEVELOPMENT_WORKSPACE_INVALID" => "DEVELOPMENT_WORKSPACE_INVALID",
            _ => "DEVELOPMENT_WORKSPACE_LIMIT",
        },
        message,
    ))
}
fn error(code: &str) -> String {
    let (code, message) = public_error(code).unwrap();
    format!("{code}: {message}")
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentDraft {
    pub workspace: String,
    pub brief: String,
    pub workflow: String,
    pub checks_json: String,
    pub protected_files_input: String,
}
impl Default for DevelopmentDraft {
    fn default() -> Self {
        Self {
            workspace: String::new(),
            brief: String::new(),
            workflow: "development-local".into(),
            checks_json: "[]".into(),
            protected_files_input: String::new(),
        }
    }
}
impl DevelopmentDraft {
    fn validate(&self) -> Result<(), String> {
        if [
            &self.workspace,
            &self.brief,
            &self.workflow,
            &self.checks_json,
            &self.protected_files_input,
        ]
        .iter()
        .any(|v| v.len() > 256 * 1024)
        {
            return Err(error("DEVELOPMENT_WORKSPACE_LIMIT"));
        }
        Ok(())
    }
    fn snapshot(&self, request_id: String) -> Result<DevelopmentSnapshot, String> {
        let checks = serde_json::from_str(&self.checks_json)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_INVALID"))?;
        let request = DevelopmentSnapshot {
            request_id,
            workspace: self.workspace.trim().into(),
            brief: self.brief.clone(),
            workflow: self.workflow.clone(),
            checks,
            protected_files: self
                .protected_files_input
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        };
        request.validate()?;
        Ok(request)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentSnapshot {
    pub request_id: String,
    pub workspace: String,
    pub brief: String,
    pub workflow: String,
    pub checks: Vec<Vec<String>>,
    pub protected_files: Vec<String>,
}
impl DevelopmentSnapshot {
    fn validate(&self) -> Result<(), String> {
        if self.request_id.is_empty()
            || self.request_id.len() > 160
            || !self
                .request_id
                .bytes()
                .enumerate()
                .all(|(i, b)| b.is_ascii_alphanumeric() || (i > 0 && b"._:-".contains(&b)))
            || self.workspace.trim().is_empty()
            || self.workspace.len() > 2000
            || self.workspace.contains('\0')
            || self.brief.trim().is_empty()
            || self.brief.len() > 16000
            || self.brief.contains('\0')
            || !["development-local", "execution-checked", "development"]
                .contains(&self.workflow.as_str())
            || self.checks.len() > 8
            || self.checks.iter().any(|a| {
                a.is_empty()
                    || a.len() > 40
                    || a[0].trim().is_empty()
                    || a.iter().any(|v| v.len() > 2000 || v.contains('\0'))
            })
            || self.protected_files.len() > 20
            || self
                .protected_files
                .iter()
                .any(|v| v.trim().is_empty() || v.len() > 2000 || v.contains('\0'))
        {
            return Err(error("DEVELOPMENT_WORKSPACE_INVALID"));
        }
        if serde_json::to_vec(self)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_INVALID"))?
            .len()
            > 60000
        {
            return Err(error("DEVELOPMENT_WORKSPACE_LIMIT"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentWorkspaceEntry {
    pub schema_version: u32,
    pub revision: u64,
    pub draft: DevelopmentDraft,
    pub pending: Option<DevelopmentSnapshot>,
    pub previous: Vec<DevelopmentSnapshot>,
}
impl Default for DevelopmentWorkspaceEntry {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            draft: Default::default(),
            pending: None,
            previous: vec![],
        }
    }
}
pub struct DevelopmentWorkspaceStore {
    directory: PathBuf,
    gate: Mutex<()>,
    #[cfg(test)]
    fail_publish: std::sync::atomic::AtomicBool,
}
impl DevelopmentWorkspaceStore {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            gate: Mutex::new(()),
            #[cfg(test)]
            fail_publish: std::sync::atomic::AtomicBool::new(false),
        }
    }
    fn path(&self) -> PathBuf {
        self.directory.join("editor.workspace.dpapi")
    }
    pub fn get(&self) -> Result<DevelopmentWorkspaceEntry, String> {
        let Some(cipher) = crate::assistant_state_store::read_bounded(&self.path(), MAX_CIPHER)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_READ"))?
        else {
            return Ok(Default::default());
        };
        let plain = crate::wechat_crypto::crypt(&cipher, false)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_READ"))?;
        if plain.len() > MAX_PLAIN {
            return Err(error("DEVELOPMENT_WORKSPACE_LIMIT"));
        }
        let e: DevelopmentWorkspaceEntry =
            serde_json::from_slice(&plain).map_err(|_| error("DEVELOPMENT_WORKSPACE_READ"))?;
        if e.schema_version != 1
            || e.revision == 0
            || e.previous.len() + usize::from(e.pending.is_some()) > MAX_REQUESTS
        {
            return Err(error("DEVELOPMENT_WORKSPACE_READ"));
        }
        e.draft.validate()?;
        let mut ids = std::collections::HashSet::new();
        for request in e.pending.iter().chain(e.previous.iter()) {
            request.validate()?;
            if !ids.insert(&request.request_id) {
                return Err(error("DEVELOPMENT_WORKSPACE_ID"));
            }
        }
        Ok(e)
    }
    async fn lock(&self) -> Result<fs::File, String> {
        crate::assistant_state_store::safe(&self.directory)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?;
        crate::legacy_isolation::ensure_test_path(&self.directory)?;
        crate::control_protocol::private_directory(&self.directory)
            .await
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?;
        let path = self.directory.join("editor.writer.lock");
        crate::assistant_state_store::safe(&path)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?;
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?;
        crate::assistant_state_store::ordinary(&file)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?;
        fs2::FileExt::try_lock_exclusive(&file)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_CONFLICT"))?;
        Ok(file)
    }
    async fn publish(
        &self,
        mut e: DevelopmentWorkspaceEntry,
    ) -> Result<DevelopmentWorkspaceEntry, String> {
        e.revision = e
            .revision
            .checked_add(1)
            .ok_or_else(|| error("DEVELOPMENT_WORKSPACE_LIMIT"))?;
        let plain = Zeroizing::new(
            serde_json::to_vec(&e).map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?,
        );
        if plain.len() > MAX_PLAIN {
            return Err(error("DEVELOPMENT_WORKSPACE_LIMIT"));
        }
        #[cfg(test)]
        if self.fail_publish.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(error("DEVELOPMENT_WORKSPACE_SAVE"));
        }
        let cipher = crate::wechat_crypto::crypt(&plain, true)
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?;
        crate::control_protocol::publish_private(&self.path(), &cipher)
            .await
            .map_err(|_| error("DEVELOPMENT_WORKSPACE_SAVE"))?;
        Ok(e)
    }
    pub async fn save(
        &self,
        revision: u64,
        draft: DevelopmentDraft,
    ) -> Result<DevelopmentWorkspaceEntry, String> {
        draft.validate()?;
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut e = self.get()?;
        if e.draft == draft {
            return Ok(e);
        }
        if e.revision != revision {
            return Err(error("DEVELOPMENT_WORKSPACE_CONFLICT"));
        }
        e.draft = draft;
        self.publish(e).await
    }
    pub async fn prepare(
        &self,
        revision: u64,
        draft: DevelopmentDraft,
        request: DevelopmentSnapshot,
    ) -> Result<DevelopmentWorkspaceEntry, String> {
        draft.validate()?;
        request.validate()?;
        if draft.snapshot(request.request_id.clone())? != request {
            return Err(error("DEVELOPMENT_WORKSPACE_ID"));
        }
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut e = self.get()?;
        if let Some(p) = &e.pending {
            return if p == &request {
                Ok(e)
            } else {
                Err(error("DEVELOPMENT_WORKSPACE_PENDING"))
            };
        }
        if e.previous
            .iter()
            .any(|p| p.request_id == request.request_id)
        {
            return Err(error("DEVELOPMENT_WORKSPACE_ID"));
        }
        if e.previous.len() >= MAX_REQUESTS {
            return Err(error("DEVELOPMENT_WORKSPACE_LIMIT"));
        }
        if e.revision != revision {
            return Err(error("DEVELOPMENT_WORKSPACE_CONFLICT"));
        }
        e.draft = draft;
        e.pending = Some(request);
        self.publish(e).await
    }
    pub async fn fresh(
        &self,
        revision: u64,
        request_id: Option<String>,
    ) -> Result<DevelopmentWorkspaceEntry, String> {
        let _gate = self.gate.lock().await;
        let _lock = self.lock().await?;
        let mut e = self.get()?;
        if e.revision != revision {
            return Err(error("DEVELOPMENT_WORKSPACE_CONFLICT"));
        }
        if e.pending.as_ref().map(|p| &p.request_id) != request_id.as_ref() {
            return Err(error("DEVELOPMENT_WORKSPACE_ID"));
        }
        let Some(pending) = e.pending.take() else {
            return Ok(e);
        };
        if e.previous.len() >= MAX_REQUESTS {
            return Err(error("DEVELOPMENT_WORKSPACE_LIMIT"));
        }
        e.previous.push(pending);
        self.publish(e).await
    }
}

#[cfg(test)]
#[path = "development_workspace_tests.rs"]
mod tests;
