//! One private source/receipt implementation for desktop and public CLI.
use crate::{
    assistant_documents::{DocumentMeta, Extraction, PageRange, UploadPart, UploadStatus},
    document_runtime::{DocumentResult, RuntimeError},
    document_service::StartReply,
};
use serde::Serialize;
#[derive(Serialize)]
pub struct DocumentIndex {
    pub documents: Vec<DocumentMeta>,
    pub uploads: Vec<UploadStatus>,
    pub unreadable: usize,
    pub complete: bool,
}
#[derive(Serialize)]
pub struct DocumentDetails {
    pub document: DocumentMeta,
    pub extractions: Vec<Extraction>,
}
#[derive(Serialize)]
pub struct DocumentResultReply {
    pub extraction: Extraction,
    pub result: Option<DocumentResult>,
    pub error: Option<RuntimeError>,
    pub message: String,
}
async fn write_scope(
    scope: &str,
) -> Result<&'static std::sync::Arc<crate::assistant::Engine>, String> {
    let engine = super::assistant::service().await?;
    engine.desktop_scope(scope).await?;
    Ok(engine)
}
async fn upload_scope(
    scope: &str,
    id: &str,
) -> Result<&'static std::sync::Arc<crate::assistant::Engine>, String> {
    let engine = write_scope(scope).await?;
    if engine.documents().registered(id)? {
        engine.document_access(id, scope).await?;
    }
    Ok(engine)
}
#[tauri::command]
pub async fn assistant_document_import(request: UploadPart) -> Result<UploadStatus, String> {
    upload_scope(&request.scope, &request.document_id)
        .await?
        .documents()
        .upload_part(request)
        .await
}
#[tauri::command]
pub async fn assistant_document_import_status(
    scope: String,
    document_id: String,
) -> Result<UploadStatus, String> {
    upload_scope(&scope, &document_id)
        .await?
        .documents()
        .upload_status(&document_id, &scope)
}
#[tauri::command]
pub async fn assistant_document_import_commit(
    scope: String,
    document_id: String,
) -> Result<DocumentMeta, String> {
    upload_scope(&scope, &document_id)
        .await?
        .documents()
        .upload_commit(&document_id, &scope)
        .await
}
#[tauri::command]
pub async fn assistant_document_index(scope: String) -> Result<DocumentIndex, String> {
    let engine = super::assistant::service().await?;
    let (documents, unreadable) = engine.document_index(&scope).await?;
    let uploads = if engine.desktop_scope(&scope).await.is_ok() {
        engine.documents().upload_index(&scope)?
    } else {
        Vec::new()
    };
    Ok(DocumentIndex {
        documents,
        uploads,
        unreadable,
        complete: unreadable == 0,
    })
}
#[tauri::command]
pub async fn assistant_document_get(
    scope: String,
    document_id: String,
) -> Result<DocumentDetails, String> {
    let engine = super::assistant::service().await?;
    let access = engine.document_access(&document_id, &scope).await?;
    Ok(DocumentDetails {
        document: access.meta().clone(),
        extractions: engine.documents().extraction_index(&access)?,
    })
}
#[tauri::command]
pub async fn assistant_document_extract(
    scope: String,
    document_id: String,
    pages: Option<PageRange>,
    retry: Option<bool>,
) -> Result<StartReply, String> {
    let engine = write_scope(&scope).await?;
    let access = engine.document_access(&document_id, &scope).await?;
    engine
        .document_service()
        .extract(access, pages, retry.unwrap_or(false))
        .await
}
#[tauri::command]
pub async fn assistant_document_status(
    scope: String,
    document_id: String,
    extraction_id: String,
) -> Result<Extraction, String> {
    let engine = super::assistant::service().await?;
    let access = engine.document_access(&document_id, &scope).await?;
    engine.documents().summary(&access, &extraction_id)
}
#[tauri::command]
pub async fn assistant_document_result(
    scope: String,
    document_id: String,
    extraction_id: String,
) -> Result<DocumentResultReply, String> {
    use crate::assistant_documents::{Outcome, Phase};
    let engine = super::assistant::service().await?;
    let access = engine.document_access(&document_id, &scope).await?;
    let extraction = engine.documents().summary(&access, &extraction_id)?;
    let message = extraction.message.clone();
    let (result, error) = if matches!(
        extraction.phase,
        Phase::Processing | Phase::CancelRequested | Phase::Unknown
    ) {
        (None, None)
    } else {
        match engine.documents().result(&access, &extraction_id)? {
            Outcome::Parsed(value) => (Some(value), None),
            Outcome::Runtime(error) => (None, Some(error)),
        }
    };
    Ok(DocumentResultReply {
        extraction,
        result,
        error,
        message,
    })
}
#[tauri::command]
pub async fn assistant_document_excerpt(
    scope: String,
    document_id: String,
    extraction_id: String,
    pages: PageRange,
    text_from: Option<usize>,
    text_to: Option<usize>,
) -> Result<crate::assistant_documents::Excerpt, String> {
    let engine = super::assistant::service().await?;
    let access = engine.document_access(&document_id, &scope).await?;
    let range = match (text_from, text_to) {
        (None, None) => None,
        (Some(from), Some(to)) => Some((from, to)),
        _ => return Err("文字选择须同时给出text_from和text_to".into()),
    };
    engine
        .documents()
        .excerpt(&access, &extraction_id, pages, range)
}
#[tauri::command]
pub async fn assistant_document_cancel(
    scope: String,
    document_id: String,
    extraction_id: String,
) -> Result<StartReply, String> {
    let engine = write_scope(&scope).await?;
    let access = engine.document_access(&document_id, &scope).await?;
    engine
        .document_service()
        .cancel(&access, &extraction_id)
        .await
}
