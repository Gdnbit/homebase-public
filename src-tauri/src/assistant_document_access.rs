//! Document ownership derives from the same atomically committed messages as
//! the accepted request. No separate post-ACK binding write is necessary.
use super::*;
use crate::assistant_documents::{Access, DocumentStore, Outcome, PageRange, Phase};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PdfQuestion {
    pub document_id: String,
    pub name: String,
}

pub(super) enum WechatQuestion {
    Pdf(PdfQuestion),
    Video(crate::wechat::video::VideoQuestion),
    Clip(crate::wechat::clip_video::ClipSource),
    Voice(crate::wechat::voice::VoiceQuestion),
    VoiceTask(crate::wechat::voice::VoiceQuestion),
    Task { turn: String, file: Option<crate::wechat::file_delivery::FileSelection> },
}

impl WechatQuestion {
    pub(super) fn fingerprint_parts(&self, original: &str) -> Value {
        match self {
            Self::Pdf(pdf) => json!(["wechat-pdf-question-v1", original, pdf.document_id, pdf.name]),
            Self::Video(video) => json!(["wechat-video-question-v1", original, video.receipt_id, video.scope, video.sha256]),
            Self::Clip(video) => json!(["wechat-video-clip-v1", original, video]),
            Self::Voice(voice) => json!([
                "wechat-voice-question-v1", original, voice.receipt_id, voice.scope, voice.local,
                format!("{:x}", Sha256::digest(voice.transcript.as_bytes()))
            ]),
            Self::VoiceTask(voice) => json!(["wechat-voice-task-v1", original, voice.receipt_id, voice.scope, voice.local,
                format!("{:x}", Sha256::digest(voice.transcript.as_bytes()))]),
            Self::Task { turn, file } => json!(["wechat-task-reference-v1", original, turn, file]),
        }
    }
}

impl Engine {
    /// Select only the latest conversation for new input. An older Pi chat
    /// never takes over implicitly when the current conversation is retired.
    pub(super) async fn writable_wechat_scope(&self, source: &str) -> Result<Option<String>, String> {
        let session = {
            let state = self.state.lock().await;
            let Some(session) = state.sessions.iter().rev()
                .find(|session| session.source == "wechat" && session.source_key == source)
            else { return Ok(None) };
            if state.settings.runtime != "pi" {
                return Ok(Some(session.id.clone()));
            }
            if session.cold.is_none() {
                return Ok(session.turns.last().is_none_or(|turn| turn.settings.runtime == "pi")
                    .then(|| session.id.clone()));
            }
            session.clone()
        };
        Ok(archive::cold_last_turn_uses_pi(&session).await?.then_some(session.id))
    }

    pub(super) async fn current_wechat_scope(&self, source: &str) -> Option<String> {
        self.state.lock().await.sessions.iter().rev()
            .find(|session| session.source == "wechat" && session.source_key == source)
            .map(|session| session.id.clone())
    }

    pub(super) async fn wechat_pdf_excerpt(
        &self,
        source: &str,
        question: &PdfQuestion,
    ) -> Result<(String, TextAttachment), String> {
        let scope = self.current_wechat_scope(source).await
            .ok_or("当前微信会话没有此 PDF。请在当前会话发送该文件后再问。")?;
        let access = self.documents.access(&question.document_id, &scope, false, false)
            .map_err(|_| "当前微信会话无法核对这份 PDF。请在当前会话发送原文件后再问。")?;
        if access.meta().origin_scope != scope || access.meta().name != question.name {
            return Err("PDF 与原收件或当前微信会话不一致，未读取。".into());
        }
        let pages = PageRange { from: 1, to: 1 };
        let extraction = self.documents.lookup(&access, &Some(pages.clone()))?
            .ok_or("这份 PDF 尚无首页解析回执，没有发起模型请求。")?;
        if extraction.phase != Phase::Ready {
            return Err(match extraction.phase {
                Phase::NoText => "这份 PDF 的首页没有可提取文字。当前未启用 OCR。",
                Phase::Processing | Phase::CancelRequested | Phase::Unknown =>
                    "这份 PDF 的首页解析尚未确认，请先查询原收件结果。",
                _ => "这份 PDF 的首页尚未取得可用文字，请先查询原收件结果。",
            }.into());
        }
        let excerpt = self.documents.excerpt_prefix(&access, &extraction.id, pages, 6000)?;
        Ok((scope, TextAttachment {
            name: excerpt.name,
            content: excerpt.content,
            document: Some(excerpt.document),
        }))
    }
    pub(crate) async fn channel_pdf(
        &self,
        request: crate::wechat::PdfRequest,
    ) -> Result<Option<String>, String> {
        let scope = {
            let _submission = self.send_gate.lock().await;
            if self.documents.registered(&request.id)? {
                let meta = self.documents.metadata(&request.id)?;
                if meta.name != request.name
                    || !self.state.lock().await.sessions.iter().any(|session| {
                        session.id == meta.origin_scope
                            && session.source == "wechat"
                            && session.source_key == request.source
                    })
                {
                    return Err("PDF 不属于当前微信收件，未读取原文件".into());
                }
                if request.bytes.as_ref().is_some_and(|bytes| {
                    bytes.len() != meta.source_bytes
                        || format!("{:x}", Sha256::digest(bytes.as_slice())) != meta.source_sha256
                }) {
                    return Err("同一微信收件编号的 PDF 字节不一致，原文件保留".into());
                }
                meta.origin_scope
            } else {
                let Some(bytes) = request.bytes.as_ref().filter(|_| !request.read_only) else {
                    return Ok(None);
                };
                let current = self.writable_wechat_scope(&request.source).await?;
                let scope = match current {
                    Some(scope) => scope,
                    None => {
                        self.make_session_fixed("wechat", &request.source, &request.id)
                            .await?
                            .id
                    }
                };
                self.documents
                    .stage(&request.id, &scope, &request.name, bytes)
                    .await?;
                scope
            }
        };
        let access = self.document_access(&request.id, &scope).await?;
        let pages = Some(PageRange { from: 1, to: 1 });
        let receipt = if request.read_only {
            self.documents.lookup(&access, &pages)?
        } else {
            let service = self.document_service();
            let started = service.extract(access.clone(), pages, false).await?;
            match started.extraction {
                Some(extraction)
                    if matches!(extraction.phase, Phase::Processing | Phase::CancelRequested) =>
                {
                    Some(service.wait(&access, &extraction.id).await?)
                }
                Some(extraction) => Some(extraction),
                None => {
                    return Ok(Some(format!(
                        "已在本机保存 PDF「{}」。{}",
                        request.name, started.message
                    )))
                }
            }
        };
        let title = format!("已在本机保存 PDF「{}」。", request.name);
        let Some(receipt) = receipt else {
            return Ok(Some(format!("{title} 尚无首页解析回执。")));
        };
        let message = match receipt.phase {
            Phase::Ready => {
                let Outcome::Parsed(result) = self.documents.result(&access, &receipt.id)? else {
                    return Err("PDF 首页解析结果无法核对".into());
                };
                let page = result
                    .pages
                    .as_ref()
                    .and_then(|pages| pages.iter().find(|p| p.number == 1))
                    .ok_or("PDF 首页文字缺失")?;
                let preview = page.text.chars().take(1700).collect::<String>();
                let omitted = if preview.len() == page.text.len() {
                    ""
                } else {
                    "\n还有文字，微信仅展示本段预览。"
                };
                format!("{title}\n第 1 页本地文字预览\n{preview}{omitted}")
            }
            Phase::NoText => format!("{title} 首页没有可提取的文字。此入口未启用 OCR。"),
            Phase::Processing | Phase::CancelRequested | Phase::Unknown => {
                format!("{title} 首页解析结果待核对。")
            }
            _ => format!("{title} 首页未取得可用文字。{}", receipt.message),
        };
        Ok(Some(message))
    }
    pub async fn document_index(
        &self,
        scope: &str,
    ) -> Result<(Vec<crate::assistant_documents::DocumentMeta>, usize), String> {
        let (metas, unreadable) = self.documents.metadata_index()?;
        let state = self.state.lock().await;
        if scope != "new" && !state.sessions.iter().any(|s| s.id == scope) {
            return Err("文档会话不存在".into());
        }
        let mut claims = std::collections::BTreeSet::new();
        let mut links = std::collections::BTreeSet::new();
        for session in &state.sessions {
            for attachment in session.messages.iter().flat_map(|m| &m.attachments) {
                if let Some(reference) = &attachment.document {
                    claims.insert(reference.document_id.clone());
                    if session.id == scope && attachment.sha256 == reference.content_sha256 {
                        links.insert((
                            reference.document_id.clone(),
                            reference.source_sha256.clone(),
                        ));
                    }
                }
            }
        }
        let docs = metas
            .into_iter()
            .filter(|meta| {
                (meta.origin_scope == scope && (scope != "new" || !claims.contains(&meta.id)))
                    || links.contains(&(meta.id.clone(), meta.source_sha256.clone()))
            })
            .collect();
        Ok((docs, unreadable))
    }
    pub fn document_service(&self) -> &Arc<crate::document_service::DocumentService> {
        self.document_service.get_or_init(|| {
            Arc::new(crate::document_service::DocumentService::bundled(
                self.documents.clone(),
            ))
        })
    }
    pub fn configure_document_runtime(&self, runtime: std::path::PathBuf) -> Result<(), String> {
        self.document_service
            .set(Arc::new(crate::document_service::DocumentService::new(
                self.documents.clone(),
                runtime,
            )))
            .map_err(|_| "文档运行包已固定，未在执行中切换".into())
    }
    pub fn documents(&self) -> &Arc<DocumentStore> {
        &self.documents
    }
    pub async fn document_access(&self, document_id: &str, scope: &str) -> Result<Access, String> {
        let meta = self.documents.metadata(document_id)?;
        let state = self.state.lock().await;
        if scope != "new" && !state.sessions.iter().any(|s| s.id == scope) {
            return Err("文档会话不存在，未读取其他资料".into());
        }
        let mut claimed = false;
        let mut linked = false;
        for session in &state.sessions {
            for attachment in session.messages.iter().flat_map(|m| &m.attachments) {
                if let Some(reference) = &attachment.document {
                    if reference.document_id == document_id {
                        claimed = true;
                        if session.id == scope
                            && reference.source_sha256 == meta.source_sha256
                            && reference.content_sha256 == attachment.sha256
                        {
                            linked = true;
                        }
                    }
                }
            }
        }
        self.documents.access(document_id, scope, linked, claimed)
    }
    pub(super) async fn validate_document_input(
        &self,
        request: &SendRequest,
        source: &str,
    ) -> Result<(), String> {
        if !request.attachments.iter().any(|f| f.document.is_some()) {
            return Ok(());
        }
        if source != "desktop" {
            return Err("此入口尚未适配文档引用上传；不能使用其他入口的文档归属".into());
        }
        let scope = request.session_id.as_deref().unwrap_or("new");
        self.desktop_scope(scope).await?;
        for file in &request.attachments {
            if let Some(reference) = &file.document {
                let access = self.document_access(&reference.document_id, scope).await?;
                self.documents
                    .verify_excerpt(&access, reference, &file.name, &file.content)?;
            }
        }
        Ok(())
    }
}
