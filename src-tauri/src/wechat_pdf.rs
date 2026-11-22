//! Receive a bound owner's PDF using the documented iLink CDN wire format.
use super::*;
use crate::assistant_documents::{DocumentStore, SOURCE_LIMIT};
use super::cdn::CdnReference;

pub(crate) struct PdfRequest {
    pub source: String,
    pub id: String,
    pub name: String,
    pub bytes: Option<Zeroizing<Vec<u8>>>,
    pub read_only: bool,
}

pub(crate) type PdfInbox = Arc<
    dyn Fn(PdfRequest) -> Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send>>
        + Send
        + Sync,
>;

pub(super) struct PdfAttachment {
    name: String,
    media: CdnReference,
    size: Option<usize>,
    md5: Option<String>,
    fingerprint: String,
    item_id: Option<String>,
}

pub(super) fn inbound(message: &Value) -> Option<Result<PdfAttachment, String>> {
    let items = message["item_list"].as_array()?;
    let mut media = items
        .iter()
        .filter(|item| matches!(item["type"].as_u64(), Some(2..=5)));
    let first = media.next()?;
    if first["type"] != 4
        || media.next().is_some()
        || items.iter().any(|item| {
            item["type"] == 1
                && item["text_item"]["text"]
                    .as_str()
                    .is_some_and(|text| !text.trim().is_empty())
        })
    {
        return Some(Err("请单独发送一份不超过 4 MiB 的 PDF 文件。微信入口暂不处理图片、音视频或附带指令的文件。".into()));
    }
    Some(parse_file(&first["file_item"]).map(|mut attachment| {
        attachment.item_id = identity::reference_id(&first["msg_id"]);
        attachment
    }))
}

fn parse_file(file: &Value) -> Result<PdfAttachment, String> {
    let name = file["file_name"].as_str().ok_or("微信 PDF 缺少文件名")?;
    DocumentStore::name(name)?;
    let size = match file.get("len") {
        None | Some(Value::Null) => None,
        Some(n) => Some(
            n.as_str()
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|n| *n > 0 && *n <= SOURCE_LIMIT)
                .ok_or("微信 PDF 长度无效或超过 4 MiB，未下载")?,
        ),
    };
    let md5 = match file.get("md5") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) if s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Some(s.to_ascii_lowercase())
        }
        _ => return Err("微信 PDF 校验信息无效，未下载".into()),
    };
    let media = cdn::reference(&file["media"], "微信 PDF")?;
    let mut hash = Sha256::new();
    hash.update(
        serde_json::to_vec(&json!([name, media.url.as_str(), size, md5]))
            .map_err(|_| "微信 PDF 校验信息无效")?,
    );
    hash.update(&*media.key);
    Ok(PdfAttachment {
        name: name.into(),
        media,
        size,
        md5,
        fingerprint: format!("{:x}", hash.finalize()),
        item_id: None,
    })
}

impl Api {
    async fn download_pdf(&self, attachment: &PdfAttachment) -> Result<Zeroizing<Vec<u8>>, String> {
        let bytes = self.download_cdn(&attachment.media, SOURCE_LIMIT, None, "微信 PDF").await?;
        if !bytes.starts_with(b"%PDF-")
            || attachment.size.is_some_and(|size| size != bytes.len())
            || attachment.md5.as_ref().is_some_and(|md5| *md5 != format!("{:x}", md5::Md5::digest(&*bytes)))
        {
            return Err("微信 PDF 内容与文件声明不一致，未保存".into());
        }
        Ok(bytes)
    }
}

fn document_id(record_id: &str) -> String {
    let hash = Sha256::digest(format!("homebase-wechat-pdf:{record_id}"));
    let mut bytes: [u8; 16] = hash[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

#[cfg(test)]
#[test]
fn wechat_pdf_validates_download_origin_name_size_and_sdk_key_encodings() {
    use super::cdn::DOWNLOAD;
    let raw = [7u8; 16];
    let mut item = json!({"file_name":"本机.PDF","len":"48","media":{
        "aes_key":STANDARD.encode(raw),"encrypt_query_param":"a+b%","encrypt_type":1}});
    let first = parse_file(&item).unwrap();
    assert_eq!(*first.media.key, raw);
    assert_eq!(
        first.media.url.as_str(),
        format!("{DOWNLOAD}?encrypted_query_param=a%2Bb%25")
    );
    item["media"]["aes_key"] = json!(STANDARD.encode(b"07070707070707070707070707070707"));
    assert_eq!(parse_file(&item).unwrap().fingerprint, first.fingerprint);
    for bad in [
        "https://evil.test/c2c/download?a=1",
        "https://novac2c.cdn.weixin.qq.com/c2c/upload?a=1",
        "https://name@novac2c.cdn.weixin.qq.com/c2c/download?a=1",
        "https://novac2c.cdn.weixin.qq.com/c2c/download?a=1#fragment",
    ] {
        item["media"]["full_url"] = json!(bad);
        assert!(parse_file(&item).is_err());
    }
    item["media"]["full_url"] = json!(format!("{DOWNLOAD}?encrypted_query_param=official"));
    assert!(parse_file(&item).is_ok());
    item["len"] = json!((SOURCE_LIMIT + 1).to_string());
    assert!(parse_file(&item).is_err());
    item["len"] = json!("48");
    item["file_name"] = json!("../跨目录.pdf");
    assert!(parse_file(&item).is_err());
}

impl Record {
    pub(super) fn pending_pdf_completion(&self) -> bool {
        self.command == "wechat_pdf"
            && matches!(self.status.as_str(), "completed" | "failed")
            && self.delivery == "pending"
            && self.args[COMPLETION_REPLY] == true
            && self.result.as_ref().is_some_and(|value| value["text"].is_string())
    }
}

impl Bridge {
    pub(super) async fn pdf_question_request(
        &self,
        record: &Record,
        receipt_id: &str,
    ) -> Result<Option<Value>, String> {
        let state = self.state.lock().await;
        if !state.stored.enabled || record.status != "received" {
            return Ok(None);
        }
        let source = state.stored.logical_key();
        let receipt = state.stored.records.iter().find(|item| {
            item.id == receipt_id && item.command == "wechat_pdf"
                && item.status == "completed" && item.args["source"] == source
        }).ok_or("原 PDF 收件已无法核对，没有发起模型请求。")?;
        let name = receipt.args["name"].as_str().ok_or("原 PDF 收件缺少文件名。")?;
        Ok(Some(json!({"operation":"submit_pdf_question","source_key":source,
            "request_id":record.id,"text":record.args["text"],
            "pdf":{"document_id":document_id(receipt_id),"name":name}})))
    }

    pub(super) async fn pdf_question(
        &self,
        message: &Value,
        account: &Stored,
        text: &str,
    ) -> Option<Result<Action, String>> {
        let text = text.trim();
        let explicit = text.strip_prefix("/问PDF").filter(|rest| {
            rest.is_empty() || rest.starts_with(char::is_whitespace)
        });
        let (target, question) = match explicit {
            Some(rest) => {
                let Some((id, question)) = rest.trim().split_once(char::is_whitespace) else {
                    return Some(Err("请发送 /问PDF 收件编号 你的问题，或者在微信引用原 PDF 后提问。".into()));
                };
                let valid = id.strip_prefix("wx-").is_some_and(|value| value.len() == 64 && value.bytes().all(|n| n.is_ascii_hexdigit()));
                if !valid {
                    return Some(Err("请使用微信返回的原 PDF 收件编号。".into()));
                }
                (Some(id), question.trim())
            }
            None => {
                if text.is_empty() || text.starts_with('/') { return None; }
                (None, text)
            }
        };
        let references: Vec<_> = message["item_list"].as_array()?.iter()
            .filter(|item| item["type"] == 1)
            .filter_map(|item| item.get("ref_msg").filter(|value| value.is_object()))
            .collect();
        if target.is_none() && references.is_empty() { return None; }
        let source = account.logical_key();
        let state = self.state.lock().await;
        let records = state.stored.records.iter().filter(|record| {
            record.command == "wechat_pdf" && record.args["source"] == source
        });
        let mut chosen = None;
        let result = if let Some(id) = target {
            chosen = records.clone().find(|record| record.id == id);
            true
        } else {
            let mut mentions_pdf = false;
            for reference in references {
                let item = &reference["message_item"];
                mentions_pdf |= item["type"] == 4 && item["file_item"]["file_name"].as_str()
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with(".pdf"));
                for key in [&reference["svr_id"], &item["msg_id"]].into_iter().filter_map(identity::reference_id) {
                    let legacy = identity::legacy_receipt_id(account, &key);
                    let stable = identity::source_key(&source, &key);
                    for record in records.clone().filter(|r| identity::matches_reference(r, &legacy, &stable)) {
                        mentions_pdf = true;
                        if chosen.is_some_and(|other: &Record| other.id != record.id) {
                            return Some(Err("引用对应多份 PDF，请用 /问PDF 和原收件编号选定一份。".into()));
                        }
                        chosen = Some(record);
                    }
                }
            }
            mentions_pdf
        };
        if !result { return None; }
        let Some(record) = chosen else {
            return Some(Err("没有找到引用的 PDF 收件。可以在当前会话重新发送原文件，或用 /问PDF 加原收件编号提问。".into()));
        };
        if record.status != "completed" {
            return Some(Err("这份 PDF 的收件尚未完成，请先用 /结果 核对原收件编号。".into()));
        }
        if question.is_empty() || question.len() > 8000 {
            return Some(Err("请为这份 PDF 输入一个不超过 8000 字节的问题。".into()));
        }
        Some(Ok(Action::Call("assistant".into(), json!({"text":question,"pdf_receipt":record.id}))))
    }

    pub(super) async fn receive_pdf(
        &self,
        account: &Stored,
        generation: u64,
        identity: &InboundIdentity,
        context: &str,
        attachment: &PdfAttachment,
    ) -> Result<(), String> {
        let canceled = self.cancel.notified();
        tokio::pin!(canceled);
        canceled.as_mut().enable();
        let _serial = tokio::select! {
            biased;
            _ = &mut canceled => return Ok(()),
            lock = self.submit_lock.lock() => lock,
        };
        let source = account.logical_key();
        let id = {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation) {
                return Ok(());
            }
            match self.existing_inbound(&mut state, identity).await? {
                InboundMatch::Ambiguous => return Ok(()),
                InboundMatch::Known(index) => {
                    let record = &state.stored.records[index];
                    if record.command != "wechat_pdf"
                        || record.status != "received"
                        || record.context != context
                        || record.args["source"] != source
                        || record.args["fingerprint"] != attachment.fingerprint
                    {
                        return Ok(());
                    }
                    record.id.clone()
                }
                InboundMatch::New => {
                    if state.stored.records.len() >= 5000 {
                        return Err("微信账本已满，请归档后继续".into());
                    }
                    let mut next = state.stored.clone();
                    next.reply_context_pending = false;
                    let item_id = attachment.item_id.as_deref();
                    next.records.push(Record {
                        id: identity.receipt.clone(), inbound_keys: identity.keys.clone(),
                        command: "wechat_pdf".into(), status: "received".into(),
                        args: json!({"source":source,"name":attachment.name,"fingerprint":attachment.fingerprint,
                            "quote_id":item_id.map(|id| identity::legacy_receipt_id(account, id)),
                            "quote_source":item_id.map(|id| identity::source_key(&source, id))}),
                        context: context.into(), client_id: format!("hb-{}", uuid::Uuid::new_v4()),
                        created_at: chrono::Utc::now().to_rfc3339(), delivery: "pending".into(), ..Record::default()
                    }.with_persisted_inbound_keys());
                    (self.save)(next.clone()).await?;
                    state.stored = next;
                    identity.receipt.clone()
                }
            }
        };
        let id = id.as_str();

        if !self.inbound_account_matches(&*self.state.lock().await, account, generation) {
            return Ok(());
        }
        let inbox = self
            .pdf_inbox
            .as_ref()
            .ok_or("当前微信通道未配置本地 PDF 接收")?;
        let result = tokio::select! {
            biased;
            _ = &mut canceled => return Ok(()),
            result = async {
                let request = || PdfRequest { source: source.clone(), id: document_id(id),
                    name: attachment.name.clone(), bytes: None, read_only: false };
                if let Some(result) = inbox(request()).await? { return Ok(result); }
                let bytes = self.api.download_pdf(attachment).await?;
                if !self.inbound_account_matches(&*self.state.lock().await, account, generation) {
                    return Err("微信接收已暂停，未开始保存 PDF".into());
                }
                inbox(PdfRequest { bytes: Some(bytes), ..request() }).await?.ok_or("微信 PDF 未能核对本地保存结果".into())
            } => result,
        };

        let pending = {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation) {
                return Ok(());
            }
            let mut next = state.stored.clone();
            let record = next
                .records
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("微信 PDF 收件记录缺失")?;
            record.status = if result.is_ok() {
                "completed"
            } else {
                "failed"
            }
            .into();
            record.result = Some(json!({"text": result.as_ref().unwrap_or_else(|error| error)}));
            let can_send = !state.stored.reply_context_pending && !record.reply_context_retired();
            if can_send {
                record.args[COMPLETION_REPLY] = json!(true);
                record.delivery = "pending".into();
                record.delivery_error = None;
            } else {
                record.mark_old_reply_unsent();
            }
            let pending = can_send.then(|| record.clone());
            (self.save)(next.clone()).await?;
            state.stored = next;
            pending
        };
        if let Some(pending) = pending {
            self.send_pdf_completion(&pending, generation).await?;
        }
        Ok(())
    }

    pub(super) async fn send_pdf_completion(
        &self,
        record: &Record,
        generation: u64,
    ) -> Result<(), String> {
        let account = self.state.lock().await.stored.account();
        let text = record.result.as_ref().and_then(|value| value["text"].as_str())
            .ok_or("原 PDF 收件缺少回复摘要")?;
        let id = &record.id;
        let reply = if record.status == "completed" {
            format!("{text}\n\n用 /结果 {id} 查看原 PDF 的处理结果。")
        } else {
            format!("{text}。用 /结果 {id} 核对本次收件。")
        };
        self.reply_files_with_claim(id, &reply, &[], Some((&account, generation)), true, None, None).await
    }

    pub(super) async fn query_pdf(
        &self,
        id: &str,
        target: &Record,
        files: bool,
        origin: (&Stored, u64),
    ) -> Result<(), String> {
        let (account, generation) = origin;
        let scope = Some(origin);
        let source = account.logical_key();
        if target.args["source"] != source {
            return self.reply_files_for(id, "未找到本微信通道的 PDF 收件。", &[], scope).await;
        }
        let snapshot = target.result.as_ref().and_then(|r| r["text"].as_str());
        let mut receipt_status = target.status.as_str();
        let detail = if files {
            match (&self.pdf_inbox, target.args["name"].as_str()) {
                (Some(inbox), Some(name)) => {
                    let check = inbox(PdfRequest {
                        source,
                        id: document_id(&target.id),
                        name: name.into(),
                        bytes: None,
                        read_only: true,
                    });
                    let Some(result) = self.observe_query(generation, check).await else {
                        return Ok(());
                    };
                    match result {
                        Ok(Some(value)) => {
                            if matches!(receipt_status, "received" | "canceled") {
                                let updated = self.change_for(&target.id, scope, |record| {
                                    if matches!(record.status.as_str(), "received" | "canceled") {
                                        record.status = "completed".into();
                                        record.result = Some(json!({"text":value}));
                                        record.delivery = "not_sent".into();
                                    }
                                }).await?;
                                if !updated {
                                    return Ok(());
                                }
                                receipt_status = "completed";
                            }
                            Some(value)
                        }
                        Ok(None) if receipt_status != "completed" => snapshot.map(str::to_owned),
                        Ok(None) | Err(_) => Some("本机原文档或解析回执暂无法核对。".into()),
                    }
                }
                _ => Some("当前通道暂无法核对原 PDF。".into()),
            }
        } else {
            None
        };
        let status = match receipt_status {
            "completed" => "已处理",
            "failed" => "收件失败",
            _ => "待核对原收件",
        };
        let response = format!(
            "PDF 收件 {status}。{}",
            detail.map(|value| format!("\n{value}")).unwrap_or_default()
        );
        self.reply_files_for(id, &response, &[], scope).await
    }
}
