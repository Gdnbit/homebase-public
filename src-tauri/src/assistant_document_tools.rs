//! Models see only documents in committed user messages up to their own turn.
//! Desktop staging authority and model read authority are deliberately distinct.
use super::*;
use crate::assistant_documents::{DocumentRef, Outcome, PageRange};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    action: String,
    document_id: Option<String>,
    extraction_id: Option<String>,
    pages: Option<PageRange>,
    offset: Option<usize>,
    limit: Option<usize>,
}
impl Engine {
    pub(super) async fn document_tool(&self, tid: &str, args: &Value) -> Result<Value, String> {
        let args: Args = serde_json::from_value(args.clone())
            .map_err(|_| "文档工具参数无效；不能指定路径、其他会话或授权标志")?;
        let (scope, visible) = {
            let state = self.state.lock().await;
            let (s, t) = locate(&state, tid)?;
            let session = &state.sessions[s];
            let prefix: std::collections::BTreeSet<_> =
                session.turns.iter().take(t+1).map(|v| v.id.as_str()).collect();
            let mut refs = Vec::<DocumentRef>::new();
            for message in session.messages.iter().rev() {
                if message.role != "user"
                    || !message
                        .turn_id
                        .as_deref()
                        .is_some_and(|id| prefix.contains(id))
                {
                    continue;
                }
                for info in &message.attachments {
                    if let Some(reference) = &info.document {
                        if reference.content_sha256 == info.sha256
                            && reference.validate().is_ok()
                            && !refs.contains(reference)
                        {
                            refs.push(reference.clone());
                        }
                    }
                }
            }
            (session.id.clone(), refs)
        };
        if args.action == "list" {
            if args.document_id.is_some() || args.extraction_id.is_some() || args.pages.is_some() {
                return Err("list只接受action、offset、limit；不会读取文件内容".into());
            }
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(10);
            if offset > visible.len() || limit == 0 || limit > 20 {
                return Err("list的offset须在引用总数内，limit为1至20".into());
            }
            let mut end = offset.saturating_add(limit).min(visible.len());
            loop {
                let value = json!({"status":"ready","documents":&visible[offset..end],"total":visible.len(),"offset":offset,"next_offset":if end<visible.len(){Some(end)}else{None},"message":"按最近发送顺序列出本轮及此前已发送的PDF引用；不含草稿或未来排队消息。","data_only":true});
                if value.to_string().len() <= 9000 {
                    return Ok(value);
                }
                if end <= offset + 1 {
                    return Err("引用元数据超过单次工具结果上限，未返回截断JSON".into());
                }
                end -= 1;
            }
        }
        let id = args
            .document_id
            .as_deref()
            .ok_or("先list发现本人已发送的document_id")?;
        let reference = visible
            .iter()
            .find(|r| r.document_id == id)
            .ok_or("文档尚未发送到本轮可见的对话中，未读取草稿或其他会话资料")?;
        let access = self.documents.access(id, &scope, true, true)?;
        if access.meta().source_sha256 != reference.source_sha256 {
            return Err("文档来源与已发送引用不一致，未读取".into());
        }
        match args.action.as_str() {
            "extract" => {
                if args.extraction_id.is_some() || args.offset.is_some() || args.limit.is_some() {
                    return Err("extract只接受document_id与明确的pages".into());
                }
                let pages = args
                    .pages
                    .ok_or("提取其他页必须明确指定pages，至多40个连续页")?;
                let service = self.document_service();
                let start = service.extract(access.clone(), Some(pages), false).await?;
                let Some(extraction) = start.extraction else {
                    return serde_json::to_value(start).map_err(|_| "文档启动结果无法编码".into());
                };
                let result = service.wait(&access, &extraction.id).await?;
                Ok(
                    json!({"status":result.phase,"extraction":result,"data_only":true,"message":"这是本地文字提取状态；读取原编号结果，不代表完成用户的其他任务。"}),
                )
            }
            "status" => {
                if args.pages.is_some() || args.offset.is_some() || args.limit.is_some() {
                    return Err("status只接受document_id和原extraction_id".into());
                }
                let key = args
                    .extraction_id
                    .as_deref()
                    .unwrap_or(&reference.extraction_id);
                let extraction = self.documents.summary(&access, key)?;
                Ok(json!({"status":extraction.phase,"extraction":extraction,"data_only":true}))
            }
            "read" => {
                let key = args
                    .extraction_id
                    .as_deref()
                    .unwrap_or(&reference.extraction_id);
                let status = self.documents.summary(&access, key)?;
                if status.phase != crate::assistant_documents::Phase::Ready {
                    return Ok(json!({"status":status.phase,"extraction":status,"data_only":true}));
                }
                let Outcome::Parsed(result) = self.documents.result(&access, key)? else {
                    return Err("原提取结果没有可读文字".into());
                };
                let explicit = args.extraction_id.is_some() || args.pages.is_some();
                let pages = args.pages.unwrap_or(PageRange {
                    from: reference.page_from,
                    to: reference.page_to,
                });
                if pages.from == 0
                    || pages.to < pages.from
                    || pages.to - pages.from >= 40
                    || pages.from < result.page_from.unwrap_or(u32::MAX)
                    || pages.to > result.page_to.unwrap_or(0)
                {
                    return Err("页范围尚未提取；先对本文件extract明确页范围".into());
                }
                let selected: Vec<_> = result
                    .pages
                    .as_ref()
                    .ok_or("原结果缺少页文字")?
                    .iter()
                    .filter(|p| p.number >= pages.from && p.number <= pages.to)
                    .collect();
                let body = selected
                    .iter()
                    .map(|p| format!("【第{}页】\n{}", p.number, p.text))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let (from, to) = if explicit {
                    (0, body.len())
                } else {
                    (reference.text_from, reference.text_to)
                };
                let offset = args.offset.unwrap_or(from);
                let limit = args.limit.unwrap_or(6000);
                if limit == 0
                    || limit > 8000
                    || from > to
                    || to > body.len()
                    || offset < from
                    || offset > to
                    || !body.is_char_boundary(offset)
                    || !body.is_char_boundary(to)
                {
                    return Err(
                        "文字窗口须在已选资料范围内，并使用有效UTF-8字节边界；limit为1至8000"
                            .into(),
                    );
                }
                let mut end = offset.saturating_add(limit).min(to);
                while end > offset && !body.is_char_boundary(end) {
                    end -= 1;
                }
                if end == offset && offset < to {
                    return Err("limit不足以容纳下一个完整字符，请增大limit".into());
                }
                loop {
                    let value = json!({"status":"ready","document_id":id,"extraction_id":key,"source_name":access.meta().name,"source_sha256":access.meta().source_sha256,"pages":pages,"text":&body[offset..end],"offset":offset,"next_offset":if end<to{Some(end)}else{None},"selected_text_to":to,"partial":result.partial.unwrap_or(true)||from!=0||to!=body.len()||end<to||offset>from||pages.from!=1||Some(pages.to)!=result.total_pages,"missing_text_pages":selected.iter().filter(|p|p.empty).map(|p|p.number).collect::<Vec<_>>(),"data_only":true,"method":"pdf_text_only"});
                    // The common tool boundary caps serialized JSON at 10KB.
                    // Quotes, backslashes and controls expand when encoded.
                    // Shrink the actual window and cursor together, never let
                    // bounded() replace unseen text with an advancing preview.
                    if value.to_string().len() <= 9000 {
                        return Ok(value);
                    }
                    let mut smaller = offset + (end - offset) * 3 / 4;
                    while smaller > offset && !body.is_char_boundary(smaller) {
                        smaller -= 1;
                    }
                    if smaller == offset {
                        return Err("文字窗口元数据超过工具上限，未跳过原文或伪造游标".into());
                    }
                    end = smaller;
                }
            }
            _ => Err("文档action仅支持list、read、extract、status".into()),
        }
    }
}
