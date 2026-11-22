//! iLink may describe the same delivery with an envelope ID or an item ID.
use super::*;

const DELIVERY_METADATA: &str = "assistant_delivery_metadata_v1";
const INBOUND_METADATA: &str = "wechat_inbound_keys_v1";
pub(super) const OUTBOUND_QUOTE_LIMIT: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryMetadata {
    #[serde(default)]
    outbound_quote_keys: Vec<String>,
    #[serde(default)]
    confirmation_quotes: Vec<ConfirmationQuote>,
    #[serde(default)]
    file_quotes: Vec<file_delivery::FileQuote>,
    #[serde(default)]
    attachment_receipts: Option<Vec<file_delivery::Receipt>>,
}

fn merge_recent<T: Eq>(mut saved: Vec<T>, current: Vec<T>) -> Vec<T> {
    for entry in current {
        if !saved.contains(&entry) {
            saved.push(entry);
        }
    }
    let excess = saved.len().saturating_sub(OUTBOUND_QUOTE_LIMIT);
    saved.drain(..excess);
    saved
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct InboundKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item: Option<String>,
}

impl InboundKeys {
    pub(super) fn is_empty(&self) -> bool {
        self.message.is_none() && self.item.is_none()
    }

    fn contains(&self, key: &str) -> bool {
        self.message.as_deref() == Some(key) || self.item.as_deref() == Some(key)
    }
}

pub(super) struct InboundIdentity {
    pub(super) receipt: String,
    pub(super) keys: InboundKeys,
    legacy_item: Option<String>,
}

pub(super) enum InboundMatch {
    New,
    Known(usize),
    Ambiguous,
}

fn wire_id(value: &Value) -> Option<String> {
    if let Some(number) = value.as_u64() {
        return Some(number.to_string());
    }
    let id = value.as_str()?.trim();
    (!id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)).then(|| id.to_owned())
}

// Quoted media uses the original numeric ID, including values beyond JavaScript's
// safe integer range. Keep this validation independent from fallback delivery IDs.
pub(super) fn reference_id(value: &Value) -> Option<String> {
    let id = wire_id(value)?;
    id.parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == id)
        .map(|_| id)
}

pub(super) fn legacy_receipt_id(account: &Stored, id: &str) -> String {
    format!(
        "wx-{:x}",
        Sha256::digest(format!("{}:{}:{id}", account.bot, account.owner))
    )
}

pub(super) fn source_key(logical_channel: &str, id: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("homebase-wechat-inbound-v1:{logical_channel}:{id}"))
    )
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ConfirmationQuote {
    key: String,
    confirmation: uuid::Uuid,
}

impl ConfirmationQuote {
    pub(super) fn new(key: String, confirmation: uuid::Uuid) -> Self {
        Self { key, confirmation }
    }
}

impl Record {
    pub(super) fn with_persisted_inbound_keys(mut self) -> Self {
        self.persist_inbound_keys();
        self
    }

    pub(super) fn persist_inbound_keys(&mut self) {
        if self.inbound_keys.is_empty() {
            return;
        }
        if self.args.is_null() {
            self.args = json!({});
        }
        if let Some(args) = self.args.as_object_mut() {
            args.insert(INBOUND_METADATA.into(), json!(self.inbound_keys));
        }
    }

    pub(super) fn restore_inbound_keys(&mut self) {
        if let Some(saved) = self.args.get(INBOUND_METADATA).cloned()
            .and_then(|value| serde_json::from_value::<InboundKeys>(value).ok())
        {
            let hash = |value: &String| value.len() == 64
                && value.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if self.inbound_keys.message.is_none() {
                self.inbound_keys.message = saved.message.filter(hash);
            }
            if self.inbound_keys.item.is_none() {
                self.inbound_keys.item = saved.item.filter(hash);
            }
        }
        self.persist_inbound_keys();
    }

    pub(super) fn control_args(&self) -> Value {
        let mut args = self.args.clone();
        if let Some(args) = args.as_object_mut() {
            args.remove(INBOUND_METADATA);
            args.remove(DELIVERY_METADATA);
            args.remove(file_delivery::DELIVERY_V2);
        }
        args
    }

    pub(super) fn restore_delivery_metadata(&mut self) {
        if !matches!(self.command.as_str(), "assistant" | "message" | voice::COMMAND) {
            return;
        }
        if let Some(saved) = self.args.get(DELIVERY_METADATA).cloned()
            .and_then(|value| serde_json::from_value::<DeliveryMetadata>(value).ok())
        {
            self.outbound_quote_keys = merge_recent(saved.outbound_quote_keys, std::mem::take(&mut self.outbound_quote_keys));
            self.confirmation_quotes = merge_recent(saved.confirmation_quotes, std::mem::take(&mut self.confirmation_quotes));
            self.file_quotes = merge_recent(saved.file_quotes, std::mem::take(&mut self.file_quotes));
            if self.attachment_receipts.is_none() {
                self.attachment_receipts = saved.attachment_receipts;
            }
        }
        self.restore_exact_file_delivery();
        self.persist_delivery_metadata();
    }

    pub(super) fn persist_delivery_metadata(&mut self) {
        if !matches!(self.command.as_str(), "assistant" | "message" | voice::COMMAND) || (
            self.outbound_quote_keys.is_empty() && self.confirmation_quotes.is_empty()
                && self.file_quotes.is_empty() && self.attachment_receipts.is_none()
        ) {
            return;
        }
        // Old builds discard new Record fields when saving the DPAPI ledger.
        // They retain arbitrary args on these host-owned assistant receipts.
        if self.args.is_null() {
            self.args = json!({});
        }
        if let Some(args) = self.args.as_object_mut() {
            args.insert(DELIVERY_METADATA.into(), json!({
                "outbound_quote_keys": self.outbound_quote_keys,
                "confirmation_quotes": self.confirmation_quotes,
                "file_quotes": self.file_quotes,
                "attachment_receipts": self.attachment_receipts,
            }));
        }
    }

    pub(super) fn remember_confirmation_quote(&mut self, quote: ConfirmationQuote) {
        if self.confirmation_quotes.contains(&quote) {
            return;
        }
        self.confirmation_quotes.push(quote);
        let excess = self.confirmation_quotes.len().saturating_sub(OUTBOUND_QUOTE_LIMIT);
        self.confirmation_quotes.drain(..excess);
        self.persist_delivery_metadata();
    }
}

pub(super) fn message_references(message: &Value) -> impl Iterator<Item = &Value> {
    message["item_list"].as_array().into_iter().flatten()
        .filter(|item| item["type"] == 1)
        .filter_map(|item| item.get("ref_msg").filter(|value| !value.is_null()))
}

pub(super) fn reference_keys(account: &Stored, reference: &Value) -> Vec<(String, String)> {
    let source = account.logical_key();
    [&reference["svr_id"], &reference["message_item"]["msg_id"]].into_iter()
        .filter_map(reference_id)
        .map(|id| (legacy_receipt_id(account, &id), source_key(&source, &id)))
        .collect()
}

pub(super) fn matches_reference(record: &Record, legacy: &str, stable: &str) -> bool {
    record.id == legacy
        || record.args["quote_id"] == legacy
        || record.args["quote_source"] == stable
        || record.inbound_keys.contains(stable)
}

pub(super) fn matches_task_reference(record: &Record, source: &str, legacy: &str, stable: &str) -> bool {
    let media = record.command == "wechat_pdf" || record.command == voice::COMMAND
        || record.local_media_kind().is_some();
    if media && record.args["source"] != source {
        return false;
    }
    record.outbound_quote_keys.iter().any(|key| key == stable)
        || record.confirmation_quotes.iter().any(|quote| quote.key == stable)
        || record.file_quotes.iter().any(|quote| quote.key == stable)
        || if media {
            matches_reference(record, legacy, stable)
        } else {
            record.id == legacy || record.inbound_keys.contains(stable)
        }
}

impl InboundIdentity {
    pub(super) fn from_message(account: &Stored, value: &Value) -> Option<Self> {
        let message = wire_id(&value["message_id"]);
        let item = value["item_list"]
            .as_array()
            .and_then(|items| items.iter().find_map(|item| wire_id(&item["msg_id"])));
        let preferred = message.as_deref().or(item.as_deref())?;
        let logical_channel = account.logical_key();
        Some(Self {
            receipt: legacy_receipt_id(account, preferred),
            legacy_item: item.as_deref().map(|id| legacy_receipt_id(account, id)),
            keys: InboundKeys {
                message: message
                    .as_deref()
                    .map(|id| source_key(&logical_channel, id)),
                item: item.as_deref().map(|id| source_key(&logical_channel, id)),
            },
        })
    }

    fn matches(&self, record: &Record) -> bool {
        let media_receipt = record.command == "wechat_pdf" || record.local_media_kind().is_some();
        let matches_legacy =
            |id: &str| record.id == id || (media_receipt && record.args["quote_id"] == id);
        matches_legacy(&self.receipt)
            || self.legacy_item.as_deref().is_some_and(matches_legacy)
            || [self.keys.message.as_deref(), self.keys.item.as_deref()]
                .into_iter()
                .flatten()
                .any(|key| {
                    record.inbound_keys.contains(key)
                        || (media_receipt && record.args["quote_source"] == key)
                })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_quote_aliases_belong_only_to_media_receipts() {
        let account = Stored {
            bot: "synthetic-bot".into(),
            owner: "synthetic-owner".into(),
            ..Stored::default()
        };
        let identity =
            InboundIdentity::from_message(&account, &json!({"message_id":"21"})).unwrap();
        let mut record = Record {
            id: legacy_receipt_id(&account, "20"),
            command: "system_info".into(),
            args: json!({
                "quote_id": identity.receipt,
                "quote_source": source_key(&account.logical_key(), "21"),
            }),
            ..Record::default()
        };
        assert!(!identity.matches(&record));
        record.command = "wechat_pdf".into();
        assert!(identity.matches(&record));
    }

    #[test]
    fn outbound_quote_keys_never_deduplicate_inbound_messages() {
        let account = Stored {
            bot: "synthetic-bot".into(), owner: "synthetic-owner".into(), ..Stored::default()
        };
        let inbound = InboundIdentity::from_message(&account, &json!({"message_id":"42"})).unwrap();
        let mut original = Record {
            id: "original-receipt".into(), command: "assistant".into(), ..Default::default()
        };
        original.remember_outbound_keys([source_key(&account.logical_key(), "42")]);
        assert_eq!(original.outbound_quote_keys.len(), 1);
        assert!(!inbound.matches(&original));
    }
}

impl Bridge {
    pub(super) async fn quoted_resend(
        &self,
        account: &Stored,
        message: &Value,
        mode: file_delivery::Resend,
    ) -> Option<Result<Action, String>> {
        let mut references = message_references(message);
        let mismatch = || Ok(Action::Reply("我没能定位到一份可核对的附件。请单独引用那份文件，或在 HomeBase 查看原任务。".into()));
        let Some(reference) = references.next() else {
            return (mode != file_delivery::Resend::Task).then(|| Ok(Action::Reply("请引用你想重发的那份文件，再说「重发这份」或「把这份发给我」。".into())));
        };
        if references.next().is_some() {
            return Some(mismatch());
        }
        let source = account.logical_key();
        let keys = reference_keys(account, reference);
        let state = self.state.lock().await;
        let mut records = state.stored.records.iter().filter(|record| {
            keys.iter().any(|(legacy, key)| matches_task_reference(record, &source, legacy, key))
        });
        let Some(record) = records.next() else { return Some(mismatch()); };
        if records.next().is_some() {
            return Some(mismatch());
        }
        let mut selection = None;
        let mut other_message = false;
        for (legacy, key) in &keys {
            let mut matches = record.file_quotes.iter().filter(|quote| &quote.key == key);
            if let Some(quote) = matches.next() {
                if matches.next().is_some() || selection.is_some_and(|file| file != &quote.selection) {
                    return Some(mismatch());
                }
                selection = Some(&quote.selection);
            } else if matches_task_reference(record, &source, legacy, key) {
                other_message = true;
            }
        }
        if let Some(file) = selection {
            if !other_message && record.command == "assistant" {
                if let Some(turn) = record.job_id.as_ref().filter(|turn| valid_task_id(turn)) {
                    return Some(Ok(if mode == file_delivery::Resend::RequestFile {
                        Action::FileRequest { turn:turn.clone(), file:file.clone(), resend:false }
                    } else { Action::FileQuery { turn:turn.clone(), file:file.clone() } }));
                }
            }
            return Some(mismatch());
        }
        // Legacy attachment aliases do not identify a particular file. A task-level
        // resend can still use a known text reference, when iLink provides its type.
        if mode == file_delivery::Resend::Task && reference["message_item"]["type"] == 1
            && reference["message_item"]["file_item"].is_null()
        {
            drop(state);
            return self.quoted_task(account, message, true).await;
        }
        Some(mismatch())
    }

    pub(super) async fn quoted_confirmation(
        &self,
        account: &Stored,
        message: &Value,
        text: &str,
    ) -> Option<Action> {
        let phrase = text.trim().trim_end_matches(['。', '！', '!']).trim_end();
        if !matches!(phrase, "确认" | "确认执行" | "同意执行" | "就这么办" | "按这个来") {
            return None;
        }
        let mut references = message_references(message);
        let reference = references.next()?;
        let mismatch = || Action::Reply("这条引用无法对应一份已发出的确认方案。请查看并引用最新的确认提示。".into());
        if references.next().is_some() {
            return Some(mismatch());
        }
        let source = account.logical_key();
        let keys = reference_keys(account, reference);
        let state = self.state.lock().await;
        let mut records = state.stored.records.iter().filter(|record| {
            keys.iter().any(|(legacy, key)| matches_task_reference(record, &source, legacy, key))
        });
        let Some(record) = records.next() else { return Some(mismatch()); };
        if records.next().is_some() {
            return Some(mismatch());
        }
        let mut confirmation = None;
        for (legacy, key) in &keys {
            let mut matches = record.confirmation_quotes.iter().filter(|quote| &quote.key == key);
            if let Some(quote) = matches.next() {
                if matches.next().is_some() || confirmation.is_some_and(|id| id != quote.confirmation) {
                    return Some(mismatch());
                }
                confirmation = Some(quote.confirmation);
            } else if matches_task_reference(record, &source, legacy, key) {
                return Some(mismatch());
            }
        }
        Some(confirmation.map_or_else(mismatch, |confirmation| {
            // The durable channel-control path validates the exact pending ID and source again.
            Action::Call("assistant".into(), json!({"text":format!("确认 {confirmation}")}))
        }))
    }

    pub(super) async fn quoted_task(
        &self,
        account: &Stored,
        message: &Value,
        files: bool,
    ) -> Option<Result<Action, String>> {
        let mut references = message_references(message);
        let reference = references.next()?;
        let ambiguous = "这条引用对应多个任务，请用 /状态 或 /结果 加原任务编号。";
        if references.next().is_some() {
            return Some(Err(ambiguous.into()));
        }

        let source = account.logical_key();
        let keys = reference_keys(account, reference);
        let state = self.state.lock().await;
        let mut records = state.stored.records.iter().filter(|record| {
            keys.iter().any(|(legacy, stable)| matches_task_reference(record, &source, legacy, stable))
        });
        let original = records.next();
        if records.next().is_some() {
            return Some(Err(ambiguous.into()));
        }
        let action = original.and_then(|record| match record.query() {
            Some((QueryTarget::Numbered(target), _)) => Some(Action::Query(target.into(), files, false)),
            Some((QueryTarget::Turn(turn), _)) => Some(Action::CurrentQuery { files, target: Some(turn.into()) }),
            None if !matches!(record.command.as_str(), "" | "message") && valid_task_id(&record.id) => {
                Some(Action::Query(record.id.clone(), files, false))
            }
            _ => None,
        });
        Some(action.ok_or_else(|| "没找到这条引用对应的原任务，请用 /状态 或 /结果 加原任务编号。".into()))
    }

    pub(super) async fn quoted_assistant_followup(&self, account: &Stored, message: &Value, text: &str) -> Option<Action> {
        let references: Vec<_> = message_references(message).collect();
        let reference = references.first()?;
        let source = account.logical_key();
        let keys: Vec<_> = references.iter().flat_map(|reference| reference_keys(account, reference)).collect();
        let state = self.state.lock().await;
        let records: Vec<_> = state.stored.records.iter().filter(|record| {
            keys.iter().any(|(legacy, key)| matches_task_reference(record, &source, legacy, key))
        }).collect();
        if !records.iter().any(|record| record.command == "assistant"
            || matches!(record.query(), Some((QueryTarget::Turn(_), _)))) {
            return None;
        }
        let unmatched = || Action::Reply("我没能核对这条引用。请单独引用 HomeBase 的原任务回复或那份附件。".into());
        let [record] = records.as_slice() else { return Some(unmatched()); };
        if references.len() != 1 { return Some(unmatched()); }
        let turn = match record.query() {
            Some((QueryTarget::Turn(turn), _)) => Some(turn),
            None if record.command == "assistant" => record.job_id.as_deref(),
            _ => None,
        };
        let Some(turn) = turn.filter(|turn| valid_task_id(turn)) else { return Some(unmatched()); };

        let mut file = None;
        let mut matched_other_message = false;
        for (legacy, key) in &keys {
            let mut matches = record.file_quotes.iter().filter(|quote| &quote.key == key);
            if let Some(quote) = matches.next() {
                if matches.next().is_some() || file.is_some_and(|old| old != &quote.selection) { return Some(unmatched()); }
                file = Some(&quote.selection);
            } else if matches_task_reference(record, &source, legacy, key) {
                matched_other_message = true;
            }
        }
        let is_file = matches!(reference["message_item"]["type"].as_u64(), Some(2..=5));
        if (is_file && file.is_none()) || (file.is_some() && matched_other_message) {
            return Some(unmatched());
        }
        Some(Action::AssistantFollowup { turn: turn.into(), file: file.cloned(), text: text.into() })
    }

    pub(super) async fn existing_inbound(
        &self,
        state: &mut State,
        identity: &InboundIdentity,
    ) -> Result<InboundMatch, String> {
        let mut matches = state
            .stored
            .records
            .iter()
            .enumerate()
            .filter(|(_, row)| identity.matches(row));
        let Some((index, original)) = matches.next() else {
            return Ok(InboundMatch::New);
        };
        if matches.next().is_some() {
            return Ok(InboundMatch::Ambiguous);
        }
        let keys = &original.inbound_keys;
        // Record newly observed forms before advancing the polling cursor. Keep
        // the original receipt, operation, context and any existing aliases.
        if (keys.message.is_none() && identity.keys.message.is_some())
            || (keys.item.is_none() && identity.keys.item.is_some())
        {
            let mut next = state.stored.clone();
            let keys = &mut next.records[index].inbound_keys;
            if keys.message.is_none() {
                keys.message = identity.keys.message.clone();
            }
            if keys.item.is_none() {
                keys.item = identity.keys.item.clone();
            }
            next.records[index].persist_inbound_keys();
            (self.save)(next.clone()).await?;
            state.stored = next;
        }
        Ok(InboundMatch::Known(index))
    }
}
