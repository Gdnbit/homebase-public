//! A bounded, credential-free one-shot document process. No model or business
//! task is created here. The owning document store will supply all paths.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Notify,
};
const INPUT_LIMIT: usize = 6 * 1024 * 1024;
const OUTPUT_LIMIT: u64 = 2 * 1024 * 1024;

#[cfg(windows)]
use crate::process_lifetime::ProcessLifetime;

#[derive(Default)]
pub struct Cancellation {
    cancelled: AtomicBool,
    notify: Notify,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }
    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.cancelled.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeError {
    InputLimit,
    RuntimeMissing,
    Transport,
    OutputLimit,
    InvalidOutput,
    ProcessFailed,
    Timeout,
    Cancelled,
    CleanupUnknown,
    SourceUnavailable,
    WorkspaceUnavailable,
}
impl RuntimeError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::InputLimit => "文档请求超过传输上限，未启动解析",
            Self::RuntimeMissing => "本地文档运行包不可用，未回退到全局程序或云端",
            Self::Transport => "本地解析进程通信失败，未按空内容处理",
            Self::OutputLimit => "解析结果超过传输上限，未截断或按成功处理",
            Self::InvalidOutput => "文档解析结果格式不可确认，未按成功处理",
            Self::ProcessFailed => "本地解析进程异常退出，未按空文档处理",
            Self::Timeout => "本地解析超时，原解析进程已停止；可缩小页范围后显式重试",
            Self::Cancelled => "本次文档解析已停止，没有发送给模型",
            Self::CleanupUnknown => "未能确认原解析进程已停止；保留原状态，不自动重新解析",
            Self::SourceUnavailable => "原文档密文或来源摘要无法核对，未启动解析；原记录保留",
            Self::WorkspaceUnavailable => "文档工作目录无法保护，未启动解析；原记录保留",
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentPage {
    pub number: u32,
    pub text: String,
    pub bytes: usize,
    pub empty: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentResult {
    pub version: String,
    pub status: String,
    pub code: Option<String>,
    pub message: String,
    pub method: Option<String>,
    pub source_sha256: Option<String>,
    pub source_bytes: Option<usize>,
    pub total_pages: Option<u32>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
    pub pages: Option<Vec<DocumentPage>>,
    pub text_bytes: Option<usize>,
    pub missing_text_pages: Option<Vec<u32>>,
    pub partial: Option<bool>,
}
impl DocumentResult {
    pub(crate) fn validate(self) -> Result<Self, RuntimeError> {
        let invalid = || RuntimeError::InvalidOutput;
        if self.version != "homebase-document-v1"
            || self.message.is_empty()
            || self.message.len() > 2048
        {
            return Err(invalid());
        }
        if let Some(hash) = &self.source_sha256 {
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid());
            }
        }
        if self.code.as_ref().is_some_and(|code| {
            code.is_empty()
                || code.len() > 64
                || !code
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        }) {
            return Err(invalid());
        }
        match self.status.as_str() {
            "ready" | "no_text" => {
                if (self.status == "ready" && self.code.is_some())
                    || (self.status == "no_text" && self.code.as_deref() != Some("NO_TEXT"))
                {
                    return Err(invalid());
                }
                if self.method.as_deref() != Some("pdf_text")
                    || self.source_sha256.is_none()
                    || self
                        .source_bytes
                        .map_or(true, |n| n == 0 || n > 4 * 1024 * 1024)
                {
                    return Err(invalid());
                }
                let (
                    Some(from),
                    Some(to),
                    Some(total),
                    Some(pages),
                    Some(bytes),
                    Some(missing),
                    Some(partial),
                ) = (
                    self.page_from,
                    self.page_to,
                    self.total_pages,
                    &self.pages,
                    self.text_bytes,
                    &self.missing_text_pages,
                    self.partial,
                )
                else {
                    return Err(invalid());
                };
                if from == 0
                    || to < from
                    || to > total
                    || to - from + 1 > 40
                    || pages.len() != (to - from + 1) as usize
                    || bytes > 256 * 1024
                {
                    return Err(invalid());
                }
                let mut sum = 0;
                let mut empty = Vec::new();
                for (index, page) in pages.iter().enumerate() {
                    if page.number != from + index as u32
                        || page.bytes != page.text.len()
                        || page.empty != page.text.is_empty()
                    {
                        return Err(invalid());
                    }
                    sum += page.bytes;
                    if page.empty {
                        empty.push(page.number);
                    }
                }
                if sum != bytes
                    || missing != &empty
                    || partial != (from != 1 || to != total || !empty.is_empty())
                    || (self.status == "ready") != (bytes > 0)
                {
                    return Err(invalid());
                }
            }
            "page_selection_required" => {
                if self.code.as_deref() != Some("PAGE_SELECTION_REQUIRED")
                    || self.method.as_deref() != Some("pdf_text")
                    || self.source_sha256.is_none()
                    || self
                        .source_bytes
                        .map_or(true, |n| n == 0 || n > 4 * 1024 * 1024)
                    || self.total_pages.map_or(true, |n| n <= 40)
                    || self.pages.is_some()
                {
                    return Err(invalid());
                }
            }
            "failed" => {
                if self.code.as_ref().map_or(true, |s| s.is_empty()) || self.pages.is_some() {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        Ok(self)
    }
}

pub async fn run(
    runtime: &Path,
    node: &Path,
    profile: &Path,
    request: &Value,
    cancel: Arc<Cancellation>,
    deadline: Duration,
) -> Result<DocumentResult, RuntimeError> {
    let input =
        zeroize::Zeroizing::new(serde_json::to_vec(request).map_err(|_| RuntimeError::InputLimit)?);
    if input.len() > INPUT_LIMIT {
        return Err(RuntimeError::InputLimit);
    }
    if cancel.cancelled.load(Ordering::SeqCst) {
        return Err(RuntimeError::Cancelled);
    }
    let worker = runtime.join("worker.mjs");
    if !runtime.is_absolute()
        || !node.is_absolute()
        || !profile.is_absolute()
        || !worker.is_file()
        || !node.is_file()
        || !profile.is_dir()
    {
        return Err(RuntimeError::RuntimeMissing);
    }
    let mut command = tokio::process::Command::new(node);
    command
        .arg("--permission")
        .arg(format!("--allow-fs-read={}", runtime.display()))
        .arg("--allow-addons")
        .arg("--max-old-space-size=192")
        .arg(worker)
        .current_dir(profile)
        .env_clear()
        .env("HOME", profile)
        .env("USERPROFILE", profile)
        .env("TEMP", profile)
        .env("TMP", profile)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in ["SystemRoot", "WINDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|_| RuntimeError::RuntimeMissing)?;
    // Bind before writing any PDF bytes. If the host is killed, closing its only
    // Job handle stops the parser, so a restart cannot race an orphaned decode.
    #[cfg(windows)]
    let _lifetime = match ProcessLifetime::attach(&child) {
        Ok(job) => job,
        Err(_) => {
            let _ = child.kill().await;
            return Err(RuntimeError::RuntimeMissing);
        }
    };
    let mut stdin = child.stdin.take().ok_or(RuntimeError::Transport)?;
    let stdout = child.stdout.take().ok_or(RuntimeError::Transport)?;
    let execution = async {
        let write = async move {
            stdin
                .write_all(&input)
                .await
                .map_err(|_| RuntimeError::Transport)?;
            stdin
                .shutdown()
                .await
                .map_err(|_| RuntimeError::Transport)?;
            drop(stdin);
            Ok::<(), RuntimeError>(())
        };
        let read = async {
            let mut output = Vec::new();
            stdout
                .take(OUTPUT_LIMIT + 1)
                .read_to_end(&mut output)
                .await
                .map_err(|_| RuntimeError::Transport)?;
            if output.len() as u64 > OUTPUT_LIMIT {
                return Err(RuntimeError::OutputLimit);
            }
            Ok(output)
        };
        let wait = async {
            let status = child.wait().await.map_err(|_| RuntimeError::Transport)?;
            if !status.success() {
                return Err(RuntimeError::ProcessFailed);
            }
            Ok(())
        };
        let (_, output, _) = tokio::try_join!(write, read, wait)?;
        serde_json::from_slice::<DocumentResult>(&output)
            .map_err(|_| RuntimeError::InvalidOutput)?
            .validate()
    };
    let result = tokio::select! { biased;
        _=cancel.wait()=>Err(RuntimeError::Cancelled),
        _=tokio::time::sleep(deadline)=>Err(RuntimeError::Timeout),
        outcome=execution=>outcome,
    };
    if result.is_err() {
        let stopped = tokio::time::timeout(Duration::from_secs(3), async {
            if child.try_wait().map_err(|_| ())?.is_none() {
                child.kill().await.map_err(|_| ())?;
            }
            child.wait().await.map_err(|_| ())?;
            Ok::<(), ()>(())
        })
        .await;
        if !matches!(stopped, Ok(Ok(()))) {
            return Err(RuntimeError::CleanupUnknown);
        }
    }
    result
}
