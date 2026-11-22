//! Shared desktop/CLI/model extraction supervisor; source authorization is
//! obtained from Engine before passing an Access, never from caller booleans.
use crate::{
    assistant_documents::{
        Access, Begin, DocumentStore, Extraction, Outcome, PageRange, Phase, RunLease,
    },
    document_runtime::{self, Cancellation},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};
#[derive(Clone, Serialize)]
pub struct StartReply {
    pub status: String,
    pub extraction: Option<Extraction>,
    pub message: String,
}
struct Running {
    lease: Arc<RunLease>,
    cancel: Arc<Cancellation>,
}
pub struct DocumentService {
    store: Arc<DocumentStore>,
    runtime: PathBuf,
    slots: Arc<Semaphore>,
    running: Mutex<BTreeMap<(String, String), Arc<Running>>>,
}
impl DocumentService {
    pub fn new(store: Arc<DocumentStore>, runtime: PathBuf) -> Self {
        Self {
            store,
            runtime,
            slots: Arc::new(Semaphore::new(2)),
            running: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn bundled(store: Arc<DocumentStore>) -> Self {
        let runtime = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("distribution/document-runtime")))
            .unwrap_or_default();
        Self::new(store, runtime)
    }
    pub async fn extract(
        self: &Arc<Self>,
        access: Access,
        pages: Option<PageRange>,
        retry: bool,
    ) -> Result<StartReply, String> {
        if let Some(cached) = self.store.lookup(&access, &pages)? {
            if !retry
                || !matches!(
                    cached.phase,
                    Phase::Unknown | Phase::Failed | Phase::Cancelled
                )
            {
                return Ok(Self::cached(cached));
            }
        }
        let permit = match self.slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                return Ok(StartReply {
                    status: "capacity".into(),
                    extraction: None,
                    message: "已有两个文档正在解析，本次尚未启动；请稍后显式重试".into(),
                })
            }
        };
        if !self.runtime.is_absolute()
            || !self.runtime.join("worker.mjs").is_file()
            || !self.runtime.join("node.exe").is_file()
        {
            return Ok(StartReply {
                status: "runtime_unavailable".into(),
                extraction: None,
                message: "固定本地文档运行包缺失，本次尚未启动；没有回退全局程序或云端".into(),
            });
        }
        let mut jobs = self.running.lock().await;
        let lease = match self.store.begin(&access, pages, retry).await? {
            Begin::Cached(value) => return Ok(Self::cached(value)),
            Begin::Run(lease) => Arc::new(lease),
        };
        let receipt = lease.receipt().clone();
        let key = (receipt.document_id.clone(), receipt.id.clone());
        let run = Arc::new(Running {
            lease,
            cancel: Arc::default(),
        });
        jobs.insert(key.clone(), run.clone());
        drop(jobs);
        let service = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result=async {
                let profile=service.store.process_directory(&run.lease).await.map_err(|_|document_runtime::RuntimeError::WorkspaceUnavailable)?;
                let bytes=service.store.source(&access).map_err(|_|document_runtime::RuntimeError::SourceUnavailable)?;
                let mut request=serde_json::json!({"version":1,"mime_type":"application/pdf","bytes_base64":STANDARD.encode(&bytes)});
                if let Some(pages)=&run.lease.receipt().pages{request["pages"]=serde_json::to_value(pages).map_err(|_|document_runtime::RuntimeError::InputLimit)?;}
                let parsed=document_runtime::run(&service.runtime,&service.runtime.join("node.exe"),&profile,&request,run.cancel.clone(),Duration::from_secs(20)).await;
                Ok::<_,document_runtime::RuntimeError>(match parsed{Ok(value)=>Outcome::Parsed(value),Err(error)=>Outcome::Runtime(error)})
            }.await;
            let outcome = result.unwrap_or_else(Outcome::Runtime);
            // Failed publication leaves the original receipt/immutable result;
            // reads expose unknown/recovered, never silently resubmit the source.
            let _ = service.store.finish(&run.lease, outcome).await;
            let mut jobs = service.running.lock().await;
            if jobs
                .get(&key)
                .is_some_and(|j| j.lease.receipt().attempt == run.lease.receipt().attempt)
            {
                jobs.remove(&key);
            }
        });
        Ok(StartReply {
            status: "started".into(),
            extraction: Some(receipt),
            message: "已开始本地文字提取，尚未发送给模型".into(),
        })
    }
    fn cached(extraction: Extraction) -> StartReply {
        StartReply {
            status: "cached".into(),
            message: extraction.message.clone(),
            extraction: Some(extraction),
        }
    }
    pub async fn cancel(&self, access: &Access, key: &str) -> Result<StartReply, String> {
        let status = self.store.summary(access, key)?;
        if !matches!(status.phase, Phase::Processing | Phase::CancelRequested) {
            return Ok(Self::cached(status));
        }
        let run = self
            .running
            .lock()
            .await
            .get(&(access.meta().id.clone(), key.into()))
            .cloned();
        let Some(run) = run else {
            return Ok(StartReply {
                status: "not_running_here".into(),
                extraction: Some(status),
                message: "原解析不由本进程监督，未声称已经停止；请回原窗口或查询原编号".into(),
            });
        };
        let requested = self.store.request_cancel(&run.lease).await?;
        if requested.phase == Phase::CancelRequested {
            run.cancel.cancel();
        }
        Ok(StartReply {
            status: "cancel_requested".into(),
            message: requested.message.clone(),
            extraction: Some(requested),
        })
    }
    pub async fn wait(&self, access: &Access, key: &str) -> Result<Extraction, String> {
        let until = tokio::time::Instant::now() + Duration::from_secs(23);
        loop {
            let status = self.store.summary(access, key)?;
            if !matches!(status.phase, Phase::Processing | Phase::CancelRequested)
                || tokio::time::Instant::now() >= until
            {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
}
