//! Per-file receipts on the original assistant turn, inside the existing WeChat ledger.
use super::*;

pub(super) const UNCONFIRMED: &str = "文件仍在原任务。微信的附件发送回执没有核实，我先不重复发送。确认手机没有收到时，可以引用原任务回复并按文件名重发。";
pub(super) const NOT_SENT: &str =
    "这次附件没有进入微信消息发送，文件仍在原任务。可以再说「把结果发给我」。";
pub(super) const FILE_UNAVAILABLE: &str = "没能核对这份原附件。文件可能已移除或任务结果发生变化，请在 HomeBase 查看原任务。";
const RECEIPT_LIMIT: usize = 12;
pub(super) const DELIVERY_V2: &str = "assistant_file_receipts_v2";
const FILE_LIST: &str = "assistant_attachment_list_v1";
const SELECT_HINT: &str = "我没法核对你说的是哪份附件。请引用带编号的附件列表，或直接引用那份附件。";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    Unknown,
    NotSent,
    Accepted,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Receipt {
    digest: String,
    outcome: Outcome,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ExactReceipt {
    file: FileSelection,
    outcome: Outcome,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ExactDelivery {
    inherited: Vec<Receipt>,
    exact: Vec<ExactReceipt>,
    legacy: Vec<Receipt>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Resend {
    Task,
    File,
    RequestFile,
}

pub(super) fn resend_intent(text: &str) -> Option<Resend> {
    match text.trim().trim_end_matches(['。', '！', '!']).trim_end() {
        "重新发送附件" | "重发附件" | "请重发附件" => Some(Resend::Task),
        "重发这份" | "重发这份文件" | "重发这个附件" | "请重发这份" | "请重发这份文件" | "请把这份再发给我"
        | "这张没收到，重发一下" | "这份没收到，重发一下" => Some(Resend::File),
        "发给我刚才那份" | "把这份发给我" | "把这份附件发给我" | "只发这份" => Some(Resend::RequestFile),
        _ => None,
    }
}

pub(super) fn ordinal_intent(text: &str) -> Option<(usize, bool)> {
    let text = text.trim().trim_end_matches(['。', '！', '!']).trim_end();
    let (prefix, suffix) = ["只发给我", "发给我", "发一下", "请发", "只发", "重发"]
        .into_iter().find_map(|prefix| text.strip_prefix(prefix).map(|suffix| (prefix, suffix.trim_start())))?;
    let slot = match suffix {
        "第一份" | "第1份" | "第一个附件" | "第1个附件" => 0,
        "第二份" | "第2份" | "第二个附件" | "第2个附件" => 1,
        "第三份" | "第3份" | "第三个附件" | "第3个附件" => 2,
        _ => return None,
    };
    Some((slot, prefix == "重发"))
}

pub(super) fn supports_attachment(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some(super::publish_package::FILENAME)
        || path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
        ["mp4", "mp3", "wav", "png", "jpg", "jpeg", "webp", "json", "txt", "md", "markdown", "csv", "srt"]
            .iter().any(|allowed| ext.eq_ignore_ascii_case(allowed))
    })
}

pub(crate) fn valid_attachment_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && name == name.trim()
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        && !name.chars().any(char::is_control) && supports_attachment(Path::new(name))
}

pub(super) fn named_intent(text: &str) -> Option<(&str, bool)> {
    let question = text.trim_end().ends_with(['？', '?']);
    let text = text.trim().trim_end_matches(['。', '！', '？', '!', '?']).trim_end();
    let text = ["麻烦你", "麻烦", "帮我", "请你", "请"].into_iter()
        .find_map(|prefix| text.strip_prefix(prefix)).unwrap_or(text).trim_start();
    let (name, resend) = if let Some(rest) = text.strip_prefix("把") {
        [("再发给我", true), ("再发过来", true), ("发给我", false), ("发过来", false), ("发我", false)]
            .into_iter().find_map(|(suffix, resend)| rest.strip_suffix(suffix).map(|name| (name, resend)))?
    } else {
        [("重发", true), ("重新发送", true), ("发给我", false), ("只发", false), ("发一下", false)]
            .into_iter().find_map(|(prefix, resend)| text.strip_prefix(prefix).map(|name| (name, resend)))?
    };
    let name = name.trim();
    let quoted = [("「", "」"), ("“", "”"), ("\"", "\""), ("'", "'"), ("\u{60}", "\u{60}")].into_iter()
        .find_map(|(open, close)| name.strip_prefix(open)?.strip_suffix(close));
    if quoted.is_none() && (name.contains(['，', '。', '；', ';']) ||
        ["改成", "替换成", "剪成", "处理后", "然后", " 和 ", " 以及 "].into_iter().any(|word| name.contains(word))) {
        return None;
    }
    let name = quoted.unwrap_or(name);
    (valid_attachment_name(name) && !(question && resend)).then_some((name, resend))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSelection {
    digest: String,
    path_digest: String,
}

impl FileSelection {
    pub(crate) fn from_path(path: &str, sha256: &str) -> Option<Self> {
        if path.is_empty() || !valid_digest(sha256) {
            return None;
        }
        Some(Self {
            digest: sha256.into(),
            path_digest: format!("{:x}", Sha256::digest(path.as_bytes())),
        })
    }

    pub(crate) fn matches(&self, path: &str, sha256: &str) -> bool {
        self.digest == sha256 && self.path_digest == format!("{:x}", Sha256::digest(path.as_bytes()))
    }

    pub(super) fn from_artifact(artifact: &Value) -> Option<Self> {
        Self::from_path(artifact["path"].as_str()?, artifact["sha256"].as_str()?)
    }

    pub(super) fn find<'a>(&self, files: &'a [Value]) -> Option<(usize, &'a Value)> {
        let mut matches = files.iter().take(3).enumerate()
            .filter(|(_, file)| Self::from_artifact(file).as_ref() == Some(self));
        let selected = matches.next()?;
        matches.next().is_none().then_some(selected)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FileQuote {
    pub(super) key: String,
    pub(super) selection: FileSelection,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct FileList {
    source: String,
    turn: String,
    key: String,
    files: Vec<FileSelection>,
}

impl FileList {
    pub(super) fn for_reply(source: String, turn: &str, files: &[Value]) -> Option<(Self, String)> {
        if files.len() < 2 { return None; }
        let entries = files.iter().take(3).map(FileSelection::from_artifact).collect::<Option<Vec<_>>>()?;
        if entries.iter().enumerate().any(|(index, item)| entries[..index].contains(item)) { return None; }
        let lines = files.iter().take(3).enumerate().map(|(index, artifact)| {
            let path = Path::new(artifact["path"].as_str().unwrap_or_default());
            let name: String = path.file_name().and_then(|name| name.to_str()).unwrap_or("附件")
                .chars().filter(|c| !c.is_control()).take(100).collect();
            format!("{}. {name}", index + 1)
        }).collect::<Vec<_>>().join("\n");
        Some((Self { source, turn:turn.into(), key:String::new(), files:entries }, lines))
    }

    pub(super) fn remember(mut self, record: &mut Record, key: Option<&str>) {
        self.key = key.unwrap_or_default().into();
        if record.args.is_null() { record.args = json!({}); }
        if let Some(args) = record.args.as_object_mut() { args.insert(FILE_LIST.into(), json!(self)); }
    }
}

fn digest(value: &Value) -> Option<&str> {
    value["sha256"].as_str().filter(|hash| valid_digest(hash))
}

fn valid_digest(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn original_files(record: &Record) -> impl Iterator<Item = &Value> + Clone {
    record.result.as_ref().and_then(|value| value["artifacts"].as_array())
        .into_iter().flatten().take(3)
}

fn legacy_unknown(record: &Record) -> impl Iterator<Item = &str> {
    let maybe_sent = record.attachment_receipts.is_none()
        && record.status == "succeeded"
        && matches!(
            record.delivery.as_str(),
            "unknown" | "attachment_failed" | "attachment_unknown"
        );
    original_files(record)
        .filter_map(move |file| maybe_sent.then(|| digest(file)).flatten())
}

impl Record {
    fn remember_file_quote(&mut self, key: &str, selection: FileSelection) {
        let quote = FileQuote { key: key.into(), selection };
        if !self.file_quotes.contains(&quote) {
            self.file_quotes.push(quote);
            let excess = self.file_quotes.len().saturating_sub(identity::OUTBOUND_QUOTE_LIMIT);
            self.file_quotes.drain(..excess);
        }
    }

    pub(super) fn restore_exact_file_delivery(&mut self) {
        let exact = self.args.get(DELIVERY_V2).cloned()
            .and_then(|data| serde_json::from_value::<ExactDelivery>(data).ok());
        // Old versions can only update the digest-only projection. If that
        // projection changed, its ordering and delivery attempts are no longer
        // attributable to individual files. Apply the conservative legacy view.
        self.exact_file_delivery = exact.filter(|exact| Some(&exact.legacy) == self.attachment_receipts.as_ref());
        if self.exact_file_delivery.is_none() {
            if let Some(args) = self.args.as_object_mut() { args.remove(DELIVERY_V2); }
        }
    }

    pub(super) fn file_outcome(&self, artifact: &Value, files: &[Value]) -> Option<Outcome> {
        let selection = FileSelection::from_artifact(artifact)?;
        let prior = match &self.exact_file_delivery {
            Some(ledger) => {
                if let Some(exact) = ledger.exact.iter().find(|row| row.file == selection) {
                    return Some(exact.outcome);
                }
                Some(ledger.inherited.as_slice())
            }
            None => self.attachment_receipts.as_deref(),
        };
        if let Some(entries) = prior {
            let known = entries.iter().find(|r| r.digest == selection.digest)?.outcome;
            let ambiguous_acceptance = known == Outcome::Accepted
                && files.iter().take(3).chain(original_files(self)).any(|other| {
                    digest(other) == Some(selection.digest.as_str())
                        && FileSelection::from_artifact(other).as_ref() != Some(&selection)
                });
            return Some(if ambiguous_acceptance { Outcome::Unknown } else { known });
        }
        legacy_unknown(self).any(|hash| hash == selection.digest).then_some(Outcome::Unknown)
    }

    pub(super) fn init_file_receipts(&mut self) {
        if self.attachment_receipts.is_none() {
            self.attachment_receipts = Some(
                legacy_unknown(self)
                    .map(|digest| Receipt {
                        digest: digest.into(),
                        outcome: Outcome::Unknown,
                    })
                    .collect(),
            );
            self.persist_delivery_metadata();
        }
    }

    pub(super) fn file_delivery_view(&self) -> (&str, Option<String>) {
        let expected = self.result.as_ref().and_then(|v| v["artifacts"].as_array()).filter(|entries| !entries.is_empty());
        let has = |outcome| match expected {
            Some(files) => files.iter().take(3).any(|file| self.file_outcome(file, files) == Some(outcome)),
            None => self.attachment_receipts.as_ref().is_some_and(|rows| rows.iter().any(|r| r.outcome == outcome)),
        };
        if has(Outcome::Unknown) {
            return ("attachment_unknown", Some(UNCONFIRMED.into()));
        }
        if has(Outcome::NotSent) {
            return ("attachment_failed", Some(NOT_SENT.into()));
        }
        if self.command == "assistant"
            && expected.is_some_and(|files| {
                files.iter().take(3).all(|file| self.file_outcome(file, files) == Some(Outcome::Accepted))
            })
        {
            return ("accepted", None);
        }
        (&self.delivery, self.delivery_error.clone())
    }
}

impl Bridge {
    pub(super) async fn named_attachment(
        &self, account: &Stored, message: &Value, generation: u64, name: &str, resend: bool,
    ) -> Action {
        let (target, file) = if identity::message_references(message).next().is_some() {
            match self.quoted_assistant_followup(account, message, name).await {
                Some(Action::AssistantFollowup { turn, file, .. }) => (Some(turn), file),
                Some(Action::Reply(text)) => return Action::Reply(text),
                _ => return Action::Reply("没能核对这条引用。可以引用原任务回复或附件再问我。".into()),
            }
        } else {
            (None, None)
        };
        let Some(agent) = &self.agent else {
            return Action::Reply("暂时无法查询助理的原附件。".into());
        };
        let input = json!({"operation":"resolve_named_file","source_key":account.logical_key(),
            "name":name,"turn_id":target,"file":file});
        let Some(Ok(resolved)) = self.observe_query(generation, agent(input)).await else {
            return Action::Reply("暂时无法核对已保存的附件，可以稍后再问我。".into());
        };
        let notice = match resolved["state"].as_str() {
            Some("found") => {
                let turn = resolved["target"].as_str().filter(|turn| {
                    valid_task_id(turn) && target.as_deref().is_none_or(|target| target == *turn)
                });
                let selection: Option<FileSelection> = serde_json::from_value(resolved["file"].clone()).ok();
                if let (Some(turn), Some(selection)) = (turn, selection) {
                    if file.as_ref().is_none_or(|file| file == &selection) {
                        return Action::FileRequest { turn:turn.into(), file:selection, resend };
                    }
                }
                "暂时无法核对已保存的附件，可以稍后再问我。"
            }
            Some("ambiguous") => "找到了同名附件。请引用你要的那一笔任务回复，或直接引用那份附件。",
            Some("missing") if target.is_some() => "没在被引用的任务里找到这份可发送的附件。",
            Some("missing") => "没在当前对话的近期结果里找到这份附件。较早的文件可以引用原回复再问我。",
            _ => "暂时无法核对已保存的附件，可以稍后再问我。",
        };
        Action::Reply(notice.into())
    }

    pub(super) async fn numbered_attachment(
        &self, account: &Stored, message: &Value, generation: u64, index: usize, resend: bool,
    ) -> Action {
        let unavailable = || Action::Reply(SELECT_HINT.into());
        let mut references = identity::message_references(message);
        let reference = references.next();
        if references.next().is_some() { return unavailable(); }
        let source = account.logical_key();
        let turn = if reference.is_none() {
            let Some(agent) = &self.agent else { return unavailable(); };
            let Some(Ok(value)) = self.observe_query(generation, agent(json!({"operation":"resolve_current_query","source_key":source}))).await else {
                return unavailable();
            };
            let Some(turn) = value["target"].as_str().filter(|turn| valid_task_id(turn)) else { return unavailable(); };
            Some(turn.to_owned())
        } else { None };
        let state = self.state.lock().await;
        if !self.inbound_account_matches(&state, account, generation) { return unavailable(); }
        let mut lists = state.stored.records.iter().rev().filter_map(|record| {
            serde_json::from_value::<FileList>(record.args.get(FILE_LIST)?.clone()).ok()
        }).filter(|list| list.source == source && valid_task_id(&list.turn) && (2..=3).contains(&list.files.len()));
        let list = if let Some(reference) = reference {
            let keys = identity::reference_keys(account, reference);
            let mut listed = lists.filter(|list| keys.iter().any(|(_, key)| *key == list.key));
            let Some(selected) = listed.next().filter(|_| listed.next().is_none()) else { return unavailable(); };
            if keys.iter().any(|(legacy, key)| *key != selected.key && state.stored.records.iter().any(|record| {
                identity::matches_task_reference(record, &source, legacy, key)
            })) { return unavailable(); }
            selected
        } else {
            let Some(list) = lists.find(|list| Some(&list.turn) == turn.as_ref()) else { return unavailable(); };
            list
        };
        let Some(file) = list.files.get(index) else {
            return Action::Reply("这条附件列表没有这一项。请按原回复里显示的编号选择。".into());
        };
        Action::FileRequest { turn:list.turn, file:file.clone(), resend }
    }

    pub(super) async fn record_file_outcome(
        &self,
        origin: (&Stored, u64),
        assistant_id: &str,
        artifact: &Value,
        files: &[Value],
        outcome: Outcome,
        outbound_key: Option<&str>,
    ) -> Result<bool, String> {
        // An invalid digest cannot be bound to any original file.
        let Some(selection) = FileSelection::from_artifact(artifact) else {
            return if outcome == Outcome::NotSent { Ok(true) } else { Err("附件缺少可核对的摘要。".into()) };
        };
        let mut state = self.state.lock().await;
        if !self.inbound_account_matches(&state, origin.0, origin.1) {
            return Ok(false);
        }
        let mut next = state.stored.clone();
        let Some(record) = next
            .records
            .iter_mut()
            .find(|r| r.id == assistant_id && r.command == "assistant")
        else {
            return Ok(false);
        };
        record.init_file_receipts();
        // A failed resend upload must not erase an earlier confirmed or unknown send.
        if outcome == Outcome::NotSent && matches!(record.file_outcome(artifact, files), Some(Outcome::Accepted | Outcome::Unknown)) {
            return Ok(true);
        }
        let legacy = record.attachment_receipts.as_ref().unwrap();
        let ledger = record.exact_file_delivery.get_or_insert_with(|| ExactDelivery {
            inherited: legacy.clone(), exact:Vec::new(), legacy:legacy.clone(),
        });
        if let Some(old) = ledger.exact.iter_mut().find(|r| r.file == selection) {
            old.outcome = outcome;
        } else {
            if ledger.exact.len() >= RECEIPT_LIMIT {
                return Err("原任务的附件回执已达上限，未发送新附件。".into());
            }
            ledger.exact.push(ExactReceipt { file: selection.clone(), outcome });
        }
        let legacy_outcome = {
            let mut siblings = files.iter().take(3).chain(original_files(record))
                .filter(|file| digest(file) == Some(selection.digest.as_str()));
            if siblings.clone().all(|file| record.file_outcome(file, files) == Some(Outcome::Accepted)) {
                Outcome::Accepted
            } else if siblings.any(|file| matches!(record.file_outcome(file, files), Some(Outcome::Accepted | Outcome::Unknown))) {
                Outcome::Unknown
            } else { Outcome::NotSent }
        };
        let ledger = record.exact_file_delivery.as_mut().unwrap();
        if let Some(legacy) = ledger.legacy.iter_mut().find(|row| row.digest == selection.digest) {
            legacy.outcome = legacy_outcome;
        } else {
            if ledger.legacy.len() >= RECEIPT_LIMIT { return Err("原任务的附件回执已达上限，未发送新附件。".into()); }
            ledger.legacy.push(Receipt { digest: selection.digest.clone(), outcome:legacy_outcome });
        }
        record.attachment_receipts = Some(ledger.legacy.clone());
        record.args[DELIVERY_V2] = json!(ledger);
        if let Some(key) = outbound_key {
            record.remember_file_quote(key, selection);
        }
        record.persist_delivery_metadata();
        (self.save)(next.clone()).await?;
        state.stored = next;
        Ok(true)
    }

    pub(super) async fn remember_file_notice(
        &self, origin: (&Stored, u64), assistant_id: &str, selection: FileSelection, key: &str,
    ) -> Result<bool, String> {
        self.change_for(assistant_id, Some(origin), |record| {
            if record.command == "assistant" {
                record.remember_file_quote(key, selection);
                record.persist_delivery_metadata();
            }
        }).await
    }
}
