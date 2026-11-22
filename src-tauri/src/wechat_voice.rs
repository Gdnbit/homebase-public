//! Retain received voice as data until the owner explicitly puts a verified transcript to work.
use super::*;

pub(super) const COMMAND: &str = "wechat_voice_transcript";
const UNAVAILABLE: &str = "微信没有提供可用的语音转写。可以发文字，或在桌面用本地录音转文字。";
const INVALID: &str = "微信附带的语音转写格式无效或超出上限，请用文字发送。";
pub(super) const TASK_HINT: &str = "核对文字后，可以引用这条语音或转写回复，说「按这条语音说的办」。";

pub(super) enum InboundVoice<'a> {
    Partial,
    Complete(Result<VoiceContent<'a>, &'static str>),
}

pub(super) enum VoiceContent<'a> {
    Text(&'a str),
    Audio(&'a Value),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceTaskIntent {
    receipt: String,
    source: String,
    scope: String,
    transcript_sha256: String,
}

pub(super) enum VoiceTaskAdmission {
    Submit(Value),
    Existing(String),
    Recovered(Value),
}

fn valid_text(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 6000
        && text.chars().count() <= 2800
        && !text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r'))
}

pub(crate) fn task_request_id(receipt: &str) -> Result<String, String> {
    if !video::receipt_id(receipt) {
        return Err("原语音收件编号无法核对。".into());
    }
    Ok(format!("voice-task-{receipt}"))
}

pub(super) fn inbound(message: &Value) -> Option<InboundVoice<'_>> {
    let items = message["item_list"].as_array()?;
    let voice = items.iter().find(|item| item["type"] == 3)?;
    if voice["is_completed"] == false {
        return Some(InboundVoice::Partial);
    }
    let result = if items
        .iter()
        .filter(|item| {
            matches!(item["type"].as_u64(), Some(2..=5))
                || item["type"] == 1
                    && item["text_item"]["text"]
                        .as_str()
                        .is_some_and(|text| !text.trim().is_empty())
        })
        .count()
        != 1
    {
        Err("请一次单独发送一条语音。")
    } else {
        let audio = || {
            if voice["voice_item"]["media"].is_object() {
                Ok(VoiceContent::Audio(voice))
            } else {
                Err(UNAVAILABLE)
            }
        };
        match voice["voice_item"].get("text") {
            None | Some(Value::Null) => audio(),
            Some(Value::String(value)) if value.trim().is_empty() => audio(),
            Some(Value::String(value)) if valid_text(value.trim()) => {
                Ok(VoiceContent::Text(value.trim()))
            }
            _ => Err(INVALID),
        }
    };
    Some(InboundVoice::Complete(result))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceQuestion {
    pub receipt_id: String,
    pub scope: String,
    pub transcript: String,
    #[serde(default)]
    pub local: bool,
}

impl VoiceQuestion {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !video::receipt_id(&self.receipt_id) || !valid_text(&self.transcript) {
            return Err("原语音转写无法核对。".into());
        }
        Ok(())
    }

    pub(crate) fn excerpt(&self) -> Result<crate::assistant::TextAttachment, String> {
        self.validate()?;
        let origin = if self.local {
            "本机语音转写的文字预览，可能有识别误差。"
        } else {
            "微信附带的语音转写，可能有识别误差。"
        };
        Ok(crate::assistant::TextAttachment {
            name: "微信语音转写.txt".into(),
            content: format!("{origin}\n{}", self.transcript),
            document: None,
        })
    }
}

fn task_intent(record: &Record, source: &str) -> Option<VoiceTaskIntent> {
    if record.command != COMMAND || record.status != "completed" || record.args["source"] != source
        || !video::receipt_id(&record.id) {
        return None;
    }
    let result = record.result.as_ref()?;
    let transcript_sha256 = if record.local_media_kind().is_some() {
        result["artifact"]["sha256"].as_str()?.into()
    } else {
        let text = result["text"].as_str().filter(|text| valid_text(text))?;
        format!("{:x}", Sha256::digest(text.as_bytes()))
    };
    Some(VoiceTaskIntent {
        receipt: record.id.clone(), source: source.into(), scope: record.args["scope"].as_str()?.into(), transcript_sha256,
    })
}

impl Bridge {
    pub(super) async fn voice_task(&self, account: &Stored, message: &Value, text: &str) -> Option<Action> {
        let phrase = text.trim().trim_end_matches(['。', '！', '!']).trim_end();
        if !matches!(phrase, "按这条语音说的办" | "就按这条语音办" | "按这条语音办" | "请按这条语音说的办") {
            return None;
        }
        let mut references = identity::message_references(message);
        let Some(reference) = references.next() else {
            return Some(Action::Reply("请引用你想交办的那条语音或转写回复，再说「按这条语音说的办」。".into()));
        };
        let unavailable = || Action::Reply("我还无法核对这条语音的可用转写。请引用一条已转写的原语音或转写回复。".into());
        if references.next().is_some() { return Some(unavailable()); }
        let source = account.logical_key();
        let keys = identity::reference_keys(account, reference);
        let state = self.state.lock().await;
        let mut records = state.stored.records.iter().filter(|record| {
            keys.iter().any(|(legacy, key)| identity::matches_task_reference(record, &source, legacy, key))
        });
        let Some(original) = records.next().filter(|_| records.next().is_none()) else { return Some(unavailable()); };
        Some(task_intent(original, &source).map_or_else(unavailable, Action::VoiceTask))
    }

    pub(super) async fn prepare_voice_task(&self, saved: &Value, recovering: bool) -> Result<VoiceTaskAdmission, String> {
        let intent: VoiceTaskIntent = serde_json::from_value(saved.clone()).map_err(|_| "语音交办记录无法核对。")?;
        let receipt = {
            let state = self.state.lock().await;
            if state.stored.logical_key() != intent.source {
                return Err("这条语音属于另一微信绑定，未交办。".into());
            }
            let mut prior = state.stored.records.iter().filter(|item| {
                item.command == "assistant" && item.args["voice_task_v1"]["source"] == intent.source
                    && item.args["voice_task_v1"]["receipt"] == intent.receipt
            }).filter_map(|item| item.job_id.as_deref().map(|turn| (turn, &item.args["voice_task_v1"]["scope"])));
            if let Some((turn, scope)) = prior.next() {
                if !valid_task_id(turn) || *scope != intent.scope || prior.any(|other| other != (turn, scope)) {
                    return Err("原语音交办的任务或会话无法核对，请先查看原收件。".into());
                }
                return Ok(VoiceTaskAdmission::Existing(turn.into()));
            }
            state.stored.records.iter().find(|item| item.id == intent.receipt).cloned()
        };
        if recovering {
            if let Some(ack) = self.recover_voice_acceptance(&intent).await? {
                return Ok(VoiceTaskAdmission::Recovered(ack));
            }
        }
        match self.voice_task_request(&intent, receipt.as_ref()) {
            Ok(input) => Ok(VoiceTaskAdmission::Submit(input)),
            Err(error) if !recovering => match self.recover_voice_acceptance(&intent).await? {
                Some(ack) => Ok(VoiceTaskAdmission::Recovered(ack)),
                None => Err(error),
            },
            Err(error) => Err(error),
        }
    }

    async fn recover_voice_acceptance(&self, intent: &VoiceTaskIntent) -> Result<Option<Value>, String> {
        let Some(agent) = &self.agent else { return Ok(None); };
        let value = agent(json!({"operation":"lookup_voice_task","source_key":intent.source,
            "receipt_id":intent.receipt,"scope":intent.scope})).await?;
        match value.get("receipt") {
            Some(Value::Null) => Ok(None),
            Some(ack) if ack["session_id"] == intent.scope
                && ack["turn_id"].as_str().is_some_and(valid_task_id)
                && ack["state"].is_string() && ack["interaction"] == "task" => Ok(Some(ack.clone())),
            _ => Err("原语音交办回执暂时无法核对，请保留语音，稍后再试。".into()),
        }
    }

    fn voice_task_request(&self, intent: &VoiceTaskIntent, receipt: Option<&Record>) -> Result<Value, String> {
        let receipt = receipt.ok_or("原语音收件已无法核对。")?;
        if task_intent(receipt, &intent.source).as_ref() != Some(intent) {
            return Err("原语音转写或会话归属已经变化。请重新发送语音。".into());
        }
        let local = receipt.local_media_kind().is_some();
        let result = receipt.result.as_ref().ok_or("原语音转写无法核对。")?;
        let transcript = if local {
            let path = result["artifact"]["path"].as_str().ok_or("原本机语音文字稿缺失。")?;
            let bytes = video::verified_transcript(video::MediaKind::Voice, &intent.receipt, &self.workspace, Path::new(path), &intent.transcript_sha256)?;
            std::str::from_utf8(&bytes).map_err(|_| "原语音转写无法读取为 UTF-8。")?.trim().to_owned()
        } else {
            result["text"].as_str().ok_or("原微信语音转写缺失。")?.trim().to_owned()
        };
        if !valid_text(&transcript) {
            return Err("语音转写为空、格式无效或太长。请把要办的内容用文字发给我。".into());
        }
        Ok(json!({"operation":"submit_voice_task","source_key":intent.source,
            "voice":{"receipt_id":intent.receipt,"scope":intent.scope,"transcript":transcript,"local":local}}))
    }
    pub(super) async fn receive_voice(
        &self,
        account: &Stored,
        generation: u64,
        identity: &InboundIdentity,
        context: &str,
        transcript: Result<&str, &str>,
    ) -> Result<(), String> {
        let _submit = self.submit_lock.lock().await;
        {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation)
                || !matches!(
                    self.existing_inbound(&mut state, identity).await?,
                    InboundMatch::New
                )
            {
                return Ok(());
            }
            if state.stored.records.len() >= 5000 {
                return Err("微信账本已满，请归档后继续。".into());
            }
        }
        let source = account.logical_key();
        let scope = match self.agent.as_ref().filter(|_| transcript.is_ok()) {
            Some(agent) => agent(json!({"operation":"intake_scope","source_key":source}))
                .await
                .ok()
                .and_then(|value| value["session_id"].as_str().map(str::to_owned)),
            None => None,
        };
        let (status, result) = match transcript {
            Ok(text) => ("completed", json!({"text":text})),
            Err(reason) => ("failed", json!({"reason":reason})),
        };
        let id = identity.receipt.as_str();
        let client_id = format!("hb-{}", uuid::Uuid::new_v4());
        {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, account, generation)
                || !matches!(
                    self.existing_inbound(&mut state, identity).await?,
                    InboundMatch::New
                )
            {
                return Ok(());
            }
            if state.stored.records.len() >= 5000 {
                return Err("微信账本已满，请归档后继续。".into());
            }
            let mut next = state.stored.clone();
            next.reply_context_pending = false;
            // A dropped reply after POST must never become an automatic resend.
            next.records.push(Record {
                id: id.into(),
                inbound_keys: identity.keys.clone(),
                command: COMMAND.into(),
                status: status.into(),
                args: json!({"source":source,"scope":scope}),
                context: context.into(),
                result: Some(result),
                delivery: "unknown".into(),
                client_id: client_id.clone(),
                created_at: chrono::Utc::now().to_rfc3339(),
                ..Record::default()
            }.with_persisted_inbound_keys());
            (self.save)(next.clone()).await?;
            state.stored = next;
        }
        let canceled = self.cancel.notified();
        tokio::pin!(canceled);
        canceled.as_mut().enable();
        if !self.inbound_account_matches(&*self.state.lock().await, account, generation) {
            return Ok(());
        }
        let text = match transcript {
            Ok(text) => format!(
                "收到语音。微信附带的转写是：\n{text}\n\n用 /结果 {id} 再看转写。{}",
                if scope.is_some() {
                    TASK_HINT
                } else {
                    ""
                }
            ),
            Err(reason) => format!("{reason}\n用 /结果 {id} 查看本次收件。"),
        };
        let sent = tokio::select! {
            biased;
            _ = &mut canceled => return Ok(()),
            sent = self.api.text(account, context, &client_id, &text) => sent,
        };
        if self.inbound_account_matches(&*self.state.lock().await, account, generation) {
            self.change(id, |record| {
                if sent.is_ok() {
                    record.delivery = "accepted".into();
                }
                if let Ok(Some(key)) = &sent {
                    record.remember_outbound_keys([key.clone()]);
                }
                record.delivery_error = sent.err();
            })
            .await?;
        }
        Ok(())
    }

    pub(super) async fn query_voice(
        &self,
        id: &str,
        receipt: &Record,
        files: bool,
        origin: (&Stored, u64),
    ) -> Result<(), String> {
        if receipt.args["source"] != origin.0.logical_key() {
            return self.reply_files_for(id, "未找到本微信通道的语音收件。", &[], Some(origin)).await;
        }
        let text = match receipt.result.as_ref() {
            Some(result) if receipt.status == "completed" => {
                match result["text"].as_str().filter(|text| valid_text(text)) {
                    Some(text) if files => format!("微信语音转写：\n{text}"),
                    Some(_) => "语音转写已保存。发送 /结果 和原收件编号可以查看。".into(),
                    None => "原语音转写无法核对。".into(),
                }
            }
            Some(result) if receipt.status == "failed" => result["reason"]
                .as_str()
                .unwrap_or("语音未取得可用文字。")
                .to_owned(),
            _ => "当前无法核对原语音收件。".into(),
        };
        self.reply_files_for(id, &text, &[], Some(origin)).await
    }

    pub(super) async fn voice_question(
        &self,
        message: &Value,
        account: &Stored,
        text: &str,
    ) -> Option<Result<Action, String>> {
        let text = text.trim();
        let explicit = text
            .strip_prefix("/问语音")
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace));
        let (target, question) = match explicit {
            Some(rest) => {
                let (id, question) = match rest.trim().split_once(char::is_whitespace) {
                    Some((id, question)) if video::receipt_id(id) => (id, question.trim()),
                    _ => {
                        return Some(Err(
                            "请发送 /问语音 收件编号 你的问题，或引用原语音后提问。".into(),
                        ))
                    }
                };
                (Some(id), question)
            }
            None if text.is_empty() || text.starts_with('/') => return None,
            None => (None, text),
        };
        let references = message["item_list"]
            .as_array()?
            .iter()
            .filter(|item| item["type"] == 1)
            .filter_map(|item| item.get("ref_msg").filter(|value| value.is_object()));
        if target.is_none() && references.clone().next().is_none() {
            return None;
        }
        let source = account.logical_key();
        let state = self.state.lock().await;
        let records = state
            .stored
            .records
            .iter()
            .filter(|record| record.command == COMMAND && record.args["source"] == source);
        let mut chosen = None;
        if let Some(id) = target {
            chosen = records.clone().find(|record| record.id == id);
        } else {
            let mut mentions_voice = false;
            for reference in references {
                let item = &reference["message_item"];
                mentions_voice |= item["type"] == 3;
                for key in [&reference["svr_id"], &item["msg_id"]]
                    .into_iter()
                    .filter_map(identity::reference_id)
                {
                    let legacy = identity::legacy_receipt_id(account, &key);
                    let stable = identity::source_key(&source, &key);
                    for record in records
                        .clone()
                        .filter(|record| identity::matches_task_reference(record, &source, &legacy, &stable))
                    {
                        mentions_voice = true;
                        if chosen.is_some_and(|other: &Record| other.id != record.id) {
                            return Some(Err(
                                "引用对应多条语音，请用 /问语音 和原编号选定一条。".into()
                            ));
                        }
                        chosen = Some(record);
                    }
                }
            }
            if !mentions_voice {
                return None;
            }
        }
        let Some(receipt) = chosen else {
            return Some(Err(
                "没有找到原语音收件。请引用当前对话收到的原语音。".into()
            ));
        };
        if receipt.status != "completed" || receipt.args["scope"].as_str().is_none() {
            return Some(Err(
                "这条语音没有可用于提问的已登记转写，请先用 /结果 核对。".into(),
            ));
        }
        if question.is_empty() || question.len() > 8000 {
            return Some(Err("请输入一个不超过 8000 字节的语音问题。".into()));
        }
        Some(Ok(Action::Call(
            "assistant".into(),
            json!({"text":question,"voice_receipt":receipt.id}),
        )))
    }

    pub(super) async fn voice_question_request(
        &self,
        record: &Record,
        receipt_id: &str,
    ) -> Result<Option<Value>, String> {
        let state = self.state.lock().await;
        if !state.stored.enabled || record.status != "received" {
            return Ok(None);
        }
        let source = state.stored.logical_key();
        let receipt = state
            .stored
            .records
            .iter()
            .find(|item| {
                item.id == receipt_id
                    && item.command == COMMAND
                    && item.status == "completed"
                    && item.args["source"] == source
            })
            .ok_or("原语音收件已无法核对。")?;
        let scope = receipt.args["scope"]
            .as_str()
            .ok_or("原语音收件缺少会话归属。")?;
        let local = receipt.local_media_kind().is_some();
        let field = if local { "transcript" } else { "text" };
        let text = receipt
            .result
            .as_ref()
            .and_then(|result| result[field].as_str())
            .filter(|text| valid_text(text))
            .ok_or("原语音转写无法核对。")?;
        Ok(Some(
            json!({"operation":"submit_voice_question","source_key":source,
            "request_id":record.id,"text":record.args["text"],
            "voice":{"receipt_id":receipt_id,"scope":scope,"transcript":text,"local":local}}),
        ))
    }
}
