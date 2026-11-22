//! Official iLink channel. No shell execution, no arbitrary command dispatch.
use crate::control_protocol::{ControlError, Request};
#[path = "wechat_file_delivery.rs"]
pub(crate) mod file_delivery;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify};
use zeroize::Zeroizing;

const ORIGIN: &str = "https://ilinkai.weixin.qq.com";
// Protocol generation follows the official main/beta owner-binding + PIN contract.
const VERSION: &str = "3.0.0/homebase-0.1.0";
const MAX_BODY: usize = 2 * 1024 * 1024;
#[path = "wechat_reauthorization.rs"]
mod reauthorization;
#[path = "wechat_pdf.rs"]
mod pdf;
pub(crate) use pdf::{PdfInbox, PdfRequest};
#[path = "wechat_cdn.rs"]
mod cdn;
#[path = "wechat_video.rs"]
pub(crate) mod video;
#[path = "wechat_clip.rs"]
pub(crate) mod clip_video;
#[path = "wechat_video_cover.rs"]
pub(crate) mod video_cover;
#[path = "wechat_publish_package.rs"]
pub(crate) mod publish_package;
#[path = "wechat_subtitles.rs"]
pub(crate) mod subtitles;
#[path = "wechat_repository_search.rs"]
pub(crate) mod repository_search;
#[path = "wechat_voice.rs"]
pub(crate) mod voice;
#[path = "wechat_identity.rs"]
mod identity;
#[path = "wechat_capabilities.rs"]
mod capability_browse;
use identity::{InboundIdentity, InboundKeys, InboundMatch};
pub const HELP: &str = concat!(
    "HomeBase 微信助理\n",
    "直接描述你要做的事情，和桌面共用模型、能力、记忆与提醒。\n",
    "能力 / 能力 MCP / 能力 Skill / 能力 API：浏览统一能力库；用「找能力 转写」搜索。\n",
    "帮我收录：分享内容（或 /收录 分享内容）：把文案或链接原样保存到资料库。\n",
    "进度呢？ / 结果呢？：查看当前任务，或引用已关联的任务消息和回复\n",
    "单独发送一份 4 MiB 以内的 PDF：在本机保存并预览首页。引用该文件提问，或发 /问PDF 收件编号 你的问题\n",
    "直接发送一段不超过 10 MiB 的 MP4：本地转写最多前 120 秒，返回文字稿及原文中的 GitHub 仓库链接。引用原视频提问，或发 /问视频 收件编号 你的问题\n",
    "分析仓库 https://github.com/markedjs/marked：检查公开仓库是否能接入。支持时可引用分析回复说「安装这个项目」，再确认。\n",
    "/安装项目 原分析任务编号：用原分析准备安装和验收，回复确认才开始。\n",
    "分析视频中的仓库：引用原视频回复「分析第 1 个项目」，或发送 /分析项目 原收件编号 1。\n",
    "视频只提到项目名时：引用视频回复「找第 1 个项目」或「找项目 marked」。\n",
    "单独发送一条微信语音：可查看微信附带的文字，或本机转写 60 秒以内的 SILK 语音。也可以引用原语音，或发 /问语音 收件编号 你的问题\n",
    "确认 / 拒绝：处理本人唯一待确认操作\n",
    "停止：停止助理并暂停后续消息；已受理业务任务可能继续\n",
    "继续 / 继续排队：明确恢复可继续的轮次或后续消息\n",
    "新对话：开启新上下文，原任务与长期记忆保留\n",
    "工作中可以边聊边补充原要求，旧确认会失效；另一件新事情会排队。失败或重启后需核对原任务再明确继续。\n",
    "兼容：系统、能力、视频 标题、/状态 编号、/结果 编号、/模型状态 编号、/调用 命令名 JSON参数。\n",
    "只接收绑定本人私聊，执行已接入能力；不导出密钥，本机工具仍受已配置工作区、隔离与确认限制。"
);

pub type Control = Arc<
    dyn Fn(Request) -> Pin<Box<dyn Future<Output = Result<Value, ControlError>> + Send>>
        + Send
        + Sync,
>;
pub type Agent =
    Arc<dyn Fn(Value) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>> + Send + Sync>;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    #[serde(default, skip_serializing_if = "InboundKeys::is_empty")]
    inbound_keys: InboundKeys,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    outbound_quote_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    confirmation_quotes: Vec<identity::ConfirmationQuote>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    file_quotes: Vec<file_delivery::FileQuote>,
    pub command: String,
    pub status: String,
    pub job_id: Option<String>,
    pub delivery: String,
    #[serde(default)]
    pub delivery_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attachment_receipts: Option<Vec<file_delivery::Receipt>>,
    #[serde(skip)]
    exact_file_delivery: Option<file_delivery::ExactDelivery>,
    pub created_at: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    context: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    progress_signature: String,
    #[serde(default)]
    progress_at: Option<String>,
    #[serde(default)]
    progress_count: u32,
    #[serde(default)]
    progress_delivery: String,
    #[serde(default)]
    query_failures: u32,
    #[serde(default)]
    query_retry_at: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    notice_fingerprint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    confirmation_signature: String,
}

#[derive(Debug, PartialEq, Eq)]
enum QueryTarget<'a> {
    Numbered(&'a str),
    Turn(&'a str),
    Empty,
    Unresolved,
}

#[derive(Clone, Copy)]
struct AssistantReply<'a> {
    turn: &'a str,
    confirmation: Option<uuid::Uuid>,
}

const COMPLETION_REPLY: &str = "_homebase_completion_reply";
const UNREADABLE_SHARE: &str = "wechat_unreadable_share";
const UNREADABLE_SHARE_REPLY: &str = "我读不到这条分享的内容。如果你想转写视频，可以把原 MP4 发给我。也可以粘贴链接并说‘帮我收录’，我会先把链接存下来。";
const MAX_CONTROL_LOOKUP_FAILURES: u32 = 4;

fn control_job_status(value: &Value) -> Option<&str> {
    value["status"].as_str().filter(|status| {
        matches!(
            *status,
            "queued" | "running" | "succeeded" | "failed" | "canceled" | "timed_out"
                | "uncertain" | "interrupted_unknown"
        )
    })
}

fn valid_assistant_observation(value: &Value, turn: &str) -> bool {
    let Some(status) = value["status"].as_str() else {
        return false;
    };
    let result = &value["result"];
    if result["turn_id"].as_str() != Some(turn)
        || result.get("status").is_some_and(|nested| nested.as_str() != Some(status))
    {
        return false;
    }
    match status {
        "queued" | "running" | "waiting_jobs" => true,
        "waiting_input_queue" | "awaiting_confirmation" | "succeeded" | "failed" | "canceled"
        | "rejected" | "disabled" | "interrupted_unknown" | "paused_budget" | "needs_input"
        | "unknown" => result["text"].as_str().is_some_and(|text| !text.trim().is_empty()),
        _ => false,
    }
}

fn query_backoff_deadline(failures: u32) -> i64 {
    chrono::Utc::now().timestamp() + (5_i64 * (1_i64 << failures.min(6))).min(180)
}

impl Record {
    fn remember_outbound_keys(&mut self, keys: impl IntoIterator<Item = String>) {
        if self.command == "message"
            && !matches!(self.query(), Some((QueryTarget::Numbered(_) | QueryTarget::Turn(_), _)))
        {
            return;
        }
        let mut changed = false;
        for key in keys {
            if !self.outbound_quote_keys.contains(&key) {
                self.outbound_quote_keys.push(key);
                changed = true;
            }
        }
        if changed {
            let excess = self.outbound_quote_keys.len().saturating_sub(identity::OUTBOUND_QUOTE_LIMIT);
            self.outbound_quote_keys.drain(..excess);
            self.persist_delivery_metadata();
        }
    }

    fn numbered_query(&self) -> Option<(&str, bool)> {
        if self.command != "message" {
            return None;
        }
        let query = self.args.get("numbered_query")?;
        let target = query["target"].as_str().filter(|target| valid_task_id(target))?;
        Some((target, query["files"].as_bool()?))
    }

    fn query(&self) -> Option<(QueryTarget<'_>, bool)> {
        if let Some((target, files)) = self.numbered_query() {
            return Some((QueryTarget::Numbered(target), files));
        }
        if self.command != "message" {
            return None;
        }
        if let Some(query) = self.args.get("file_query") {
            let target = query["target"].as_str().filter(|target| valid_task_id(target))?;
            return Some((QueryTarget::Turn(target), true));
        }
        if let Some(query) = self.args.get("file_request_v1") {
            let target = query["target"].as_str().filter(|target| valid_task_id(target))?;
            return Some((QueryTarget::Turn(target), true));
        }
        let query = self.args.get("current_query")?;
        let target = match query.get("target") {
            None => QueryTarget::Unresolved,
            Some(Value::Null) => QueryTarget::Empty,
            Some(Value::String(turn)) if valid_task_id(turn) => QueryTarget::Turn(turn),
            _ => return None,
        };
        Some((target, query["files"].as_bool()?))
    }

    fn pending_query(&self) -> Option<(QueryTarget<'_>, bool)> {
        if self.status == "received" && self.delivery == "pending" {
            self.query()
        } else {
            None
        }
    }

    fn pending_assistant_followup(&self) -> Option<&Value> {
        self.pending_bound_assistant("quoted_assistant_v1")
    }

    fn pending_voice_task(&self) -> Option<&Value> {
        self.pending_bound_assistant("voice_task_v1").filter(|_| self.pending_query().is_none())
    }

    fn pending_bound_assistant(&self, key: &str) -> Option<&Value> {
        if self.command != "message" || self.status != "received" || self.delivery != "pending" {
            return None;
        }
        self.args.get(key)
    }

    fn pending_assistant_completion(&self) -> bool {
        self.command == "assistant"
            && self.delivery == "pending"
            && self.job_id.is_some()
            && self.result.is_some()
            && !matches!(
                self.status.as_str(),
                "received" | "queued" | "running" | "waiting_jobs"
                    | "awaiting_confirmation" | "waiting_input_queue" | "continued"
            )
    }

    fn same_assistant_watch(&self, expected: &Self) -> bool {
        self.command == "assistant"
            && self.id == expected.id
            && self.job_id == expected.job_id
            && self.status == expected.status
            && self.client_id == expected.client_id
            && self.context == expected.context
            && self.delivery == expected.delivery
    }

    fn has_control_job(&self) -> bool {
        !matches!(self.command.as_str(), "assistant" | "message" | "wechat_pdf")
            && self.command != voice::COMMAND
            && !self.is_video()
            && self.job_id.as_deref().is_some_and(valid_task_id)
    }

    fn pending_control_completion(&self) -> Option<&str> {
        if self.delivery != "pending"
            || matches!(self.status.as_str(), "received" | "queued" | "running")
            || !self.has_control_job()
        {
            return None;
        }
        self.args[COMPLETION_REPLY].as_str().filter(|text| !text.is_empty())
    }

    fn has_saved_control_result(&self) -> bool {
        self.has_control_job() && self.result.is_some() && matches!(
            self.status.as_str(), "succeeded" | "failed" | "canceled" | "timed_out"
        )
    }

    fn clear_query_retry(&mut self) {
        self.query_failures = 0;
        self.query_retry_at = 0;
    }

    fn pending_completion(&self) -> bool {
        self.pending_assistant_completion()
            || self.pending_control_completion().is_some()
            || self.pending_media_completion()
            || self.pending_pdf_completion()
            || self.pending_share_notice()
    }

    fn pending_share_notice(&self) -> bool {
        self.command == UNREADABLE_SHARE && self.delivery == "pending"
    }

    fn clear_completion_reply(&mut self) {
        if let Some(args) = self.args.as_object_mut() {
            args.remove(COMPLETION_REPLY);
        }
    }

    fn watches_submitted_work(&self, source: &str) -> bool {
        if !self.args.is_object() && !self.args.is_null() {
            return false;
        }
        if self.command == "assistant" {
            return self.job_id.as_deref().is_some_and(valid_task_id)
                && matches!(self.status.as_str(), "running" | "awaiting_confirmation" | "waiting_input_queue");
        }
        if self.status != "running" {
            return false;
        }
        if self.local_media_kind().is_some() {
            return self.args["phase"] == "submitted" && self.args["source"] == source;
        }
        self.has_control_job()
    }

    fn reply_context_retired(&self) -> bool {
        self.args["delivery_allowed"] == false
    }

    fn mark_old_reply_unsent(&mut self) {
        self.clear_completion_reply();
        self.delivery = "not_sent".into();
        self.delivery_error = Some("微信授权已更新，旧通知未发送。请用原任务编号查询。".into());
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Stored {
    pub enabled: bool,
    token: String,
    bot: String,
    owner: String,
    base: String,
    cursor: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    reauth_required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    channel_key: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    reply_context_pending: bool,
    #[serde(default)]
    records: Vec<Record>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notices: Vec<NoticeReceipt>,
}
#[derive(Clone, Serialize, Deserialize)]
struct NoticeReceipt { owner:String, group:String, fingerprint:String }
impl Stored {
    fn account(&self) -> Self {
        Self {
            enabled: self.enabled,
            token: self.token.clone(),
            bot: self.bot.clone(),
            owner: self.owner.clone(),
            base: self.base.clone(),
            cursor: self.cursor.clone(),
            reauth_required: self.reauth_required,
            channel_key: self.channel_key.clone(),
            reply_context_pending: self.reply_context_pending,
            records: Vec::new(),
            notices: Vec::new(),
        }
    }
}
pub type Save =
    Arc<dyn Fn(Stored) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;

#[derive(Clone, Serialize)]
pub struct TaskView {
    pub id: String,
    pub command: String,
    pub status: String,
    pub job_id: Option<String>,
    pub delivery: String,
    pub delivery_error: Option<String>,
    pub created_at: String,
    pub progress_delivery: String,
    pub progress_count: u32,
}
#[derive(Clone, Serialize)]
pub struct Status {
    pub bound: bool,
    pub enabled: bool,
    pub state: String,
    pub message: String,
    pub tasks: Vec<TaskView>,
    pub local_transcriptions: LocalTranscriptions,
    pub protocol_version: String,
    pub pairing_active: bool,
}
#[derive(Clone, Default, Serialize)]
pub struct LocalTranscriptions {
    pub video: TranscriptionCounts,
    pub voice: TranscriptionCounts,
}
#[derive(Clone, Default, Serialize)]
pub struct TranscriptionCounts {
    pub processing: usize,
    pub unverified: usize,
}

/// Counts only. The persisted message fields and account never cross this boundary.
#[derive(Default, Serialize)]
pub(crate) struct RolloutActivity {
    records: usize,
    pending_work: usize,
    work_to_review: usize,
    pending_delivery: usize,
    delivery_to_review: usize,
    receiver_enabled: bool,
    pairing_active: bool,
    submit_busy: bool,
    attachment_busy: bool,
}

fn rollout_work_status(status: &str) -> (bool, bool) {
    match status {
        "received" | "queued" | "running" | "waiting_jobs" | "waiting_remote"
        | "waiting_input_queue" | "awaiting_confirmation" | "pending"
        | "processing" | "submitted" | "preflighting" => (true, false),
        "completed" | "succeeded" | "success" | "ready" | "failed" | "timed_out"
        | "canceled" | "cancelled" | "rejected" | "disabled" | "error"
        | "continued" | "superseded" => (false, false),
        _ => (false, true),
    }
}
#[derive(Clone, Serialize)]
pub struct PairingView {
    pub state: String,
    pub message: String,
    pub qr_data_url: Option<String>,
}
struct Pairing {
    code: String,
    base: String,
    view: PairingView,
    pin: Option<Zeroizing<String>>,
    generation: u64,
}
struct State {
    stored: Stored,
    state: String,
    message: String,
    pairing: Option<Pairing>,
}

#[derive(Clone)]
pub struct Api {
    client: reqwest::Client,
    #[cfg(test)]
    test_origin: Option<String>,
}
impl Api {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(40))
                .build()
                .map_err(|_| "微信网络客户端初始化失败")?,
            #[cfg(test)]
            test_origin: None,
        })
    }
    pub async fn request(
        &self,
        base: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let url = self.target_url(format!("{base}{path}"));
        let mut req = if body.is_some() {
            self.client.post(url)
        } else {
            self.client.get(url)
        };
        req = req
            .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", "196608");
        if let Some(mut body) = body {
            req = req.header("AuthorizationType", "ilink_bot_token").header(
                "X-WECHAT-UIN",
                STANDARD.encode(
                    u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap())
                        .to_string(),
                ),
            );
            if let Some(token) = token {
                req = req.bearer_auth(token);
                body["base_info"] = json!({"channel_version":VERSION,"bot_agent":"HomeBase/0.1.0"});
            }
            req = req.json(&body);
        }
        let response = req
            .send()
            .await
            .map_err(|_| "微信网络请求失败或超时；未回显认证内容")?;
        let code = response.status().as_u16();
        if !response.status().is_success() {
            return Err(format!("微信接口返回 HTTP {code}"));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err("微信响应过大".into());
        }
        let mut response = response;
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| "微信响应读取失败")? {
            if bytes.len() + chunk.len() > MAX_BODY {
                return Err("微信响应过大".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| "微信接口返回格式无效")?;
        check_business(&value)?;
        Ok(value)
    }
    fn target_url(&self, url: String) -> String {
        #[cfg(test)]
        if let Some(origin) = &self.test_origin {
            let parsed = reqwest::Url::parse(&url).unwrap();
            return format!(
                "{origin}{}{}",
                parsed.path(),
                parsed.query().map(|q| format!("?{q}")).unwrap_or_default()
            );
        }
        url
    }
    async fn text(
        &self,
        account: &Stored,
        context: &str,
        client_id: &str,
        text: &str,
    ) -> Result<Option<String>, String> {
        self.send(
            account,
            context,
            client_id,
            json!({"type":1,"text_item":{"text":clip(text,3500)}}),
        )
        .await
    }
    async fn send(
        &self,
        account: &Stored,
        context: &str,
        client_id: &str,
        item: Value,
    ) -> Result<Option<String>, String> {
        if context.is_empty() {
            return Err("缺少会话上下文，请在微信重新发送查询".into());
        }
        let v = self
            .request(
                &account.base,
                "/ilink/bot/sendmessage",
                Some(&account.token),
                Some(json!({"msg":{
                    "from_user_id":"", "to_user_id":account.owner,"client_id":client_id,
                    "message_type":2,"message_state":2,"context_token":context,"item_list":[item]
                }})),
            )
            .await?;
        // Official SendMessageResp uses protobuf defaults: an omitted ret is 0.
        // request() already rejects HTTP failures and explicit ret/errcode errors.
        if !v.is_object() {
            return Err("微信发送响应格式无效；发送状态未知".into());
        }
        Ok(identity::reference_id(&v["message_id"])
            .filter(|id| id != "0")
            .map(|id| identity::source_key(&account.logical_key(), &id)))
    }
    async fn prepare_file(
        &self,
        account: &Stored,
        path: &Path,
        workspace: &Path,
        expected_hash: &str,
    ) -> Result<Value, String> {
        let plain = artifact_bytes(path, workspace)?;
        if expected_hash.len() != 64 || format!("{:x}", Sha256::digest(&plain)) != expected_hash {
            return Err("附件哈希与任务结果不一致，未上传；请在本机核对".into());
        }
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
        let key = Zeroizing::new(*uuid::Uuid::new_v4().as_bytes());
        let key_hex = key.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let cipher = ecb::Encryptor::<aes::Aes128>::new((&*key).into())
            .encrypt_padded_vec_mut::<Pkcs7>(&plain);
        let filekey = uuid::Uuid::new_v4().simple().to_string();
        let md5 = format!("{:x}", md5::Md5::digest(&plain));
        let params = self.request(&account.base,"/ilink/bot/getuploadurl",Some(&account.token),Some(json!({
            "filekey":filekey,"media_type":3,"to_user_id":account.owner,"rawsize":plain.len(),
            "rawfilemd5":md5,"filesize":cipher.len(),"no_need_thumb":true,"aeskey":key_hex
        }))).await?;
        let url = if let Some(full) = params["upload_full_url"].as_str().filter(|s| !s.is_empty()) {
            validate_cdn(full)?;
            full.to_string()
        } else {
            let param = params["upload_param"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("微信未返回上传参数")?;
            let mut url =
                reqwest::Url::parse("https://novac2c.cdn.weixin.qq.com/c2c/upload").unwrap();
            url.query_pairs_mut()
                .append_pair("encrypted_query_param", param)
                .append_pair("filekey", &filekey);
            url.to_string()
        };
        // No Bot token is sent to CDN. No automatic resend after ambiguous upload.
        let response = self
            .client
            .post(self.target_url(url))
            .header("Content-Type", "application/octet-stream")
            .body(cipher)
            .send()
            .await
            .map_err(|_| "微信附件上传失败或状态未知")?;
        if response.status().as_u16() != 200 {
            return Err("微信附件上传未成功".into());
        }
        let param = response
            .headers()
            .get("x-encrypted-param")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            .ok_or("微信附件缺少下载参数")?;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("附件名无效")?;
        Ok(json!({"type":4,"file_item":{
            "file_name":filename,"len":plain.len().to_string(),"md5":md5,
            "media":{"encrypt_query_param":param,"aes_key":STANDARD.encode(key_hex.as_bytes()),"encrypt_type":1}
        }}))
    }
    #[cfg(test)]
    async fn file(
        &self, account: &Stored, context: &str, client_id: &str,
        path: &Path, workspace: &Path, expected_hash: &str,
    ) -> Result<Option<String>, String> {
        let item = self.prepare_file(account, path, workspace, expected_hash).await?;
        self.send(account, context, client_id, item).await
    }
}
pub fn check_business(v: &Value) -> Result<(), String> {
    for field in ["ret", "errcode"] {
        if let Some(code) = v.get(field) {
            if code.as_i64() == Some(-14) {
                return Err("SESSION_EXPIRED：微信会话已失效，请重新绑定".into());
            }
            if code.as_i64() == Some(-2) {
                return Err("微信暂不接受这次发送（ret/errcode=-2）；可能需要新的会话上下文，请在微信发一条消息后查询原结果。未自动重发。".into());
            }
            if let Some(number) = code.as_i64().filter(|n| *n != 0) {
                return Err(format!("微信业务返回失败（{field}={number}）；未回显响应正文"));
            }
            if code.as_i64().is_none() {
                return Err("微信业务状态格式无效；未回显响应正文".into());
            }
        }
    }
    Ok(())
}

// Protobuf JSON may omit default scalar/repeated fields on an empty long poll.
// An absent list is empty; an explicitly malformed list is a protocol error.
fn updates(v: &Value) -> Result<&[Value], String> {
    if !v.is_object() {
        return Err("微信消息响应不是对象，未推进游标".into());
    }
    check_business(v)?;
    match v.get("msgs") {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(messages)) => Ok(messages),
        Some(_) => Err("微信消息列表格式无效，未推进游标".into()),
    }
}
fn clip(text: &str, count: usize) -> String {
    let clipped: String = text.chars().take(count).collect();
    if text.chars().count() > count {
        format!("{clipped}\n（内容较长，请按具体任务查询）")
    } else {
        clipped
    }
}
pub fn official_base(value: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "微信返回的服务地址无效")?;
    let host = url.host_str().unwrap_or("");
    if url.scheme() != "https"
        || !(host == "ilinkai.weixin.qq.com" || host.ends_with(".ilinkai.weixin.qq.com"))
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("微信返回了尚未核实的服务地址，已停止以保护凭据".into());
    }
    Ok(url.origin().ascii_serialization())
}
fn validate_cdn(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|_| "微信 CDN 地址无效")?;
    if url.scheme() != "https"
        || url.host_str() != Some("novac2c.cdn.weixin.qq.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("微信返回了尚未核实的 CDN，附件未上传".into());
    }
    Ok(())
}
pub fn artifact_bytes(path: &Path, root: &Path) -> Result<Vec<u8>, String> {
    artifact_bytes_limited(path,root,20*1024*1024)
}
pub(crate) fn artifact_bytes_limited(path: &Path, root: &Path, limit:usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if limit==0 || limit>20*1024*1024 {return Err("附件读取上限无效".into());}
    let real_root = root.canonicalize().map_err(|_| "微信输出目录不存在")?;
    let real = path.canonicalize().map_err(|_| "任务附件不存在")?;
    if !real.starts_with(&real_root) || real == real_root {
        return Err("仅允许获取本微信通道生成的附件".into());
    }
    let mut cursor = path.to_path_buf();
    while cursor.starts_with(root) {
        let meta = std::fs::symlink_metadata(&cursor).map_err(|_| "附件元数据读取失败")?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err("附件路径含链接，未上传".into());
            }
        }
        if meta.file_type().is_symlink() {
            return Err("附件路径含链接，未上传".into());
        }
        if !cursor.pop() {
            break;
        }
    }
    if !file_delivery::supports_attachment(&real) {
        return Err("该附件格式暂不通过微信发送".into());
    }
    let mut options=std::fs::OpenOptions::new();options.read(true);
    #[cfg(windows)] {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT,FILE_SHARE_READ};
        options.share_mode(FILE_SHARE_READ).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file=options.open(&real).map_err(|_|"附件无法安全打开，请核对文件占用或权限")?;
    let meta=file.metadata().map_err(|_|"附件读取失败")?;
    #[cfg(windows)] {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes()&0x400!=0 {return Err("附件路径含链接，未读取".into());}
    }
    if !meta.is_file() || meta.len()==0 || meta.len()>limit as u64 {
        return Err(if limit==20*1024*1024 {"附件为空或超过 20 MiB；请在本机取结果"}else{"附件为空或超过本次读取上限；请在本机取结果"}.into());
    }
    let mut bytes=Vec::with_capacity(meta.len() as usize);
    file.take(limit as u64+1).read_to_end(&mut bytes).map_err(|_|"附件读取失败")?;
    if bytes.len()>limit || bytes.len()!=meta.len() as usize {return Err("附件读取期间大小变化，未采纳结果".into());}
    Ok(bytes)
}

#[cfg(test)]
mod artifact_limit_tests {
    use super::*;
    #[test]
    fn artifact_custom_read_limit_is_checked_before_body_and_default_reader_remains_compatible() {
        let root=tempfile::tempdir().unwrap();let file=root.path().join("synthetic.txt");std::fs::write(&file,b"synthetic bytes").unwrap();
        assert_eq!(artifact_bytes(&file,root.path()).unwrap(),b"synthetic bytes");
        assert_eq!(artifact_bytes_limited(&file,root.path(),15).unwrap(),b"synthetic bytes");
        assert!(artifact_bytes_limited(&file,root.path(),3).is_err());assert!(artifact_bytes_limited(&file,root.path(),0).is_err());assert!(artifact_bytes_limited(&file,root.path(),20*1024*1024+1).is_err());
        let large=std::fs::OpenOptions::new().write(true).open(&file).unwrap();large.set_len(1024*1024).unwrap();drop(large);
        assert!(artifact_bytes_limited(&file,root.path(),256*1024).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn artifact_read_rejects_an_existing_writer_and_succeeds_after_the_writer_closes() {
        let root=tempfile::tempdir().unwrap();let file=root.path().join("synthetic.txt");std::fs::write(&file,b"closed result").unwrap();
        let writer=std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        assert!(artifact_bytes_limited(&file,root.path(),256*1024).is_err());drop(writer);
        assert_eq!(artifact_bytes_limited(&file,root.path(),256*1024).unwrap(),b"closed result");
    }
}

pub struct Bridge {
    state: Mutex<State>,
    api: Api,
    save: Save,
    control: Control,
    agent: Option<Agent>,
    pdf_inbox: Option<PdfInbox>,
    video_inbox: std::sync::OnceLock<video::VideoInbox>,
    workspace: PathBuf,
    generation: AtomicU64,
    cancel: Notify,
    submit_lock: Mutex<()>,
    attachment_lock: Mutex<()>,
}
impl Bridge {
    pub(crate) fn rollout_activity(&self) -> Option<RolloutActivity> {
        let state = self.state.try_lock().ok()?;
        let mut activity = RolloutActivity {
            records: state.stored.records.len(),
            receiver_enabled: state.stored.enabled,
            pairing_active: state.pairing.as_ref().is_some_and(|pairing| {
                matches!(pairing.view.state.as_str(), "wait" | "scaned" | "need_pin")
            }),
            submit_busy: self.submit_lock.try_lock().is_err(),
            attachment_busy: self.attachment_lock.try_lock().is_err(),
            ..RolloutActivity::default()
        };
        for record in &state.stored.records {
            let (mut pending, mut review) = rollout_work_status(&record.status);
            if let Some(result) = &record.result {
                // Retained job wrappers can report success before their business result is final.
                for status in [result.get("status"), result["result"].get("status")].into_iter().flatten() {
                    let (result_pending, result_review) = rollout_work_status(status.as_str().unwrap_or(""));
                    pending |= result_pending;
                    review |= result_review;
                }
            }
            activity.pending_work += usize::from(pending);
            activity.work_to_review += usize::from(review);
            activity.pending_delivery += usize::from(record.delivery == "pending");
            let delivery = record.file_delivery_view().0;
            activity.delivery_to_review += usize::from(
                matches!(record.delivery.as_str(), "unknown" | "attachment_unknown")
                || record.progress_delivery == "unknown"
                || !matches!(delivery, "pending" | "accepted" | "not_sent" | "suppressed"
                    | "attachment_failed" | "stale" | "retired"),
            );
        }
        Some(activity)
    }

    pub fn new(
        stored: Stored,
        api: Api,
        save: Save,
        control: Control,
        workspace: PathBuf,
    ) -> Arc<Self> {
        Self::new_with_agent(stored, api, save, control, workspace, None)
    }
    pub fn new_with_agent(
        stored: Stored,
        api: Api,
        save: Save,
        control: Control,
        workspace: PathBuf,
        agent: Option<Agent>,
    ) -> Arc<Self> {
        Self::new_with_pdf(stored, api, save, control, workspace, agent, None)
    }
    pub(crate) fn new_with_pdf(
        mut stored: Stored,
        api: Api,
        save: Save,
        control: Control,
        workspace: PathBuf,
        agent: Option<Agent>,
        pdf_inbox: Option<PdfInbox>,
    ) -> Arc<Self> {
        for record in &mut stored.records {
            record.restore_inbound_keys();
            record.restore_delivery_metadata();
        }
        let expired = stored.reauth_required;
        Arc::new(Self {
            state: Mutex::new(State {
                stored,
                state: if expired { "expired" } else { "stopped" }.into(),
                message: if expired { "微信授权已失效，请重新扫码；原任务与绑定记录已保留" } else { "尚未开始接收微信消息" }.into(),
                pairing: None,
            }),
            api,
            save,
            control,
            agent,
            pdf_inbox,
            video_inbox: std::sync::OnceLock::new(),
            workspace,
            generation: AtomicU64::new(0),
            cancel: Notify::new(),
            submit_lock: Mutex::new(()),
            attachment_lock: Mutex::new(()),
        })
    }
    pub(crate) fn configure_video(&self, inbox: video::VideoInbox) {
        let _ = self.video_inbox.set(inbox);
    }
    pub async fn status(&self) -> Status {
        let state = self.state.lock().await;
        let mut local_transcriptions = LocalTranscriptions::default();
        for record in &state.stored.records {
            let counts = match record.local_media_kind() {
                Some(video::MediaKind::Video) => &mut local_transcriptions.video,
                Some(video::MediaKind::Voice) => &mut local_transcriptions.voice,
                None => continue,
            };
            match record.status.as_str() {
                "received" | "running" => counts.processing += 1,
                "unknown" => counts.unverified += 1,
                _ => {}
            }
        }
        Status {
            bound: !state.stored.token.is_empty(),
            enabled: state.stored.enabled,
            state: state.state.clone(),
            message: state.message.clone(),
            protocol_version: VERSION.into(),
            pairing_active: state
                .pairing
                .as_ref()
                .is_some_and(|p| ["wait", "scaned", "need_pin"].contains(&p.view.state.as_str())),
            local_transcriptions,
            tasks: state
                .stored
                .records
                .iter()
                .rev()
                .take(50)
                .map(|r| {
                  let (delivery, delivery_error) = r.file_delivery_view();
                  TaskView {
                    id: r.id.clone(),
                    command: r.command.clone(),
                    status: match r.result.as_ref().and_then(|v| v["status"].as_str()) {
                        Some("failed") => "failed".into(),
                        Some("unknown") => "unknown".into(),
                        Some("queued") => "queued".into(),
                        Some("running") => "waiting_remote".into(),
                        _ => r.status.clone(),
                    },
                    job_id: r.job_id.clone(),
                    delivery: delivery.into(),
                    delivery_error,
                    created_at: r.created_at.clone(),
                    progress_delivery: r.progress_delivery.clone(),
                    progress_count: r.progress_count,
                  }
                })
                .collect(),
        }
    }
    pub async fn pairing(&self) -> PairingView {
        self.state
            .lock()
            .await
            .pairing
            .as_ref()
            .map(|p| p.view.clone())
            .unwrap_or(PairingView {
                state: "none".into(),
                message: "尚未获取二维码".into(),
                qr_data_url: None,
            })
    }
    pub async fn begin(self: &Arc<Self>) -> Result<(), String> {
        let _submit = self.submit_lock.lock().await;
        let mut state = self.state.lock().await;
        if !state.stored.token.is_empty() && state.stored.enabled {
            return Err("请先停止接收，再重新扫码；无需解除绑定，原记录会保留".into());
        }
        let source = state.stored.logical_key();
        let has_unresolved_work = state.stored.records.iter().any(|record| {
            !matches!(record.command.as_str(), "message" | "wechat_pdf")
                && matches!(
                    record.status.as_str(),
                    "received" | "running" | "awaiting_confirmation" | "waiting_input_queue"
                )
                && !(state.stored.reauth_required && record.watches_submitted_work(&source))
        });
        if has_unresolved_work {
            return Err("仍有未核对的微信任务，请先查询原任务；未清理记录或开始重新绑定".into());
        }
        if state
            .pairing
            .as_ref()
            .is_some_and(|p| ["wait", "scaned", "need_pin"].contains(&p.view.state.as_str()))
        {
            return Ok(());
        }
        // Serialize begin: repeated clicks cannot issue parallel QR sessions.
        let value = self
            .api
            .request(
                ORIGIN,
                "/ilink/bot/get_bot_qrcode?bot_type=3",
                None,
                Some(json!({"local_token_list":if state.stored.token.is_empty() {Vec::<&str>::new()} else {vec![state.stored.token.as_str()]}})),
            )
            .await?;
        let code = value["qrcode"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() < 8192)
            .ok_or("微信没有返回有效二维码")?
            .to_string();
        let content = value["qrcode_img_content"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() < 8192)
            .ok_or("微信二维码展示内容缺失")?;
        let qr = qrcode::QrCode::new(content.as_bytes()).map_err(|_| "微信二维码生成失败")?;
        let svg = qr
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .build();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        state.pairing = Some(Pairing {
            code,
            base: ORIGIN.into(),
            pin: None,
            generation,
            view: PairingView {
                state: "wait".into(),
                message: "请用本人微信扫码，并在手机确认绑定".into(),
                qr_data_url: Some(format!(
                    "data:image/svg+xml;base64,{}",
                    STANDARD.encode(svg)
                )),
            },
        });
        drop(state);
        let bridge = self.clone();
        tokio::spawn(async move {
            bridge.poll_pairing(generation).await;
        });
        Ok(())
    }
    pub async fn pin(&self, pin: String) -> Result<(), String> {
        let pin = Zeroizing::new(pin);
        if !(4..=12).contains(&pin.len()) || !pin.bytes().all(|b| b.is_ascii_digit()) {
            return Err("配对码须为手机显示的 4–12 位数字".into());
        }
        let mut state = self.state.lock().await;
        let pairing = state
            .pairing
            .as_mut()
            .filter(|p| p.view.state == "need_pin")
            .ok_or("当前不需要配对码")?;
        pairing.pin = Some(pin);
        pairing.view.message = "正在核对配对码…".into();
        Ok(())
    }
    pub async fn cancel_pairing(&self) -> Result<(), String> {
        let mut state = self.state.lock().await;
        let Some(pairing) = state.pairing.as_ref() else { return Ok(()); };
        if pairing.view.state == "confirmed" {
            return Err("新授权已经保存，未撤销或恢复旧凭据；请查看最新绑定状态并点击完成，需要暂停时使用停止接收".into());
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancel.notify_waiters();
        state.pairing = None;
        Ok(())
    }
    async fn poll_pairing(self: Arc<Self>, generation: u64) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(480);
        loop {
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            if tokio::time::Instant::now() > deadline {
                self.pair_error(generation, "expired", "扫码已超时，请重新获取二维码")
                    .await;
                return;
            }
            let (base, code, pin) = {
                let mut state = self.state.lock().await;
                let Some(p) = state
                    .pairing
                    .as_mut()
                    .filter(|p| p.generation == generation)
                else {
                    return;
                };
                (p.base.clone(), p.code.clone(), p.pin.take())
            };
            let mut url =
                reqwest::Url::parse(&format!("{base}/ilink/bot/get_qrcode_status")).unwrap();
            url.query_pairs_mut().append_pair("qrcode", &code);
            if let Some(pin) = &pin {
                url.query_pairs_mut().append_pair("verify_code", pin);
            }
            let path = format!("{}?{}", url.path(), url.query().unwrap());
            let result = tokio::select! {v=self.api.request(&base,&path,None,None)=>v,_=self.cancel.notified()=>return};
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            match result {
                Ok(v) => match v["status"].as_str().unwrap_or("") {
                    "confirmed" => {
                        let base = match official_base(v["baseurl"].as_str().unwrap_or("")) {
                            Ok(v) => v,
                            Err(e) => {
                                self.pair_error(generation, "error", &e).await;
                                return;
                            }
                        };
                        let token = v["bot_token"].as_str().unwrap_or("");
                        let bot = v["ilink_bot_id"].as_str().unwrap_or("");
                        let owner = v["ilink_user_id"].as_str().unwrap_or("");
                        if [token, bot, owner]
                            .iter()
                            .any(|s| s.is_empty() || s.len() > 8192)
                        {
                            self.pair_error(generation, "error", "绑定响应缺少必要字段，未保存凭据")
                                .await;
                            return;
                        }
                        if let Err(error) = self.commit_pairing(generation, base, token, bot, owner).await {
                            self.pair_error(generation, "error", &error).await;
                            return;
                        }
                        if self.start_checked(Some(generation)).await.is_err() {
                            self.pair_error(generation, "error", "授权已保存，但接收尚未启动；请关闭弹窗后重试开始接收，原任务保留").await;
                        }
                        return;
                    }
                    "expired" | "verify_code_blocked" => {
                        self.pair_error(generation, "expired", "二维码已过期或配对码被锁定，请重新扫码")
                            .await;
                        return;
                    }
                    "binded_redirect" => {
                        self.pair_error(
                            generation,
                            "error",
                            "服务端未签发新授权，请在手机核对当前连接后重试；本机原绑定与任务已保留",
                        )
                        .await;
                        return;
                    }
                    "scaned_but_redirect" => {
                        let host = v["redirect_host"].as_str().unwrap_or("");
                        match official_base(&format!("https://{host}")) {
                            Ok(base) => {
                                if let Some(p) = self.state.lock().await.pairing.as_mut().filter(|p|p.generation==generation && self.generation.load(Ordering::SeqCst)==generation) {
                                    p.base = base;
                                }
                            }
                            Err(e) => {
                                self.pair_error(generation, "error", &e).await;
                                return;
                            }
                        }
                    }
                    status if status.contains("need_verifycode") => {
                        if let Some(p) = self.state.lock().await.pairing.as_mut().filter(|p|p.generation==generation && self.generation.load(Ordering::SeqCst)==generation) {
                            p.view.state = "need_pin".into();
                            p.view.message = if pin.is_some() {
                                "配对码未通过，请输入手机上最新的数字"
                            } else {
                                "请输入微信手机端显示的配对码"
                            }
                            .into();
                        }
                    }
                    "scaned" => {
                        if let Some(p) = self.state.lock().await.pairing.as_mut().filter(|p|p.generation==generation && self.generation.load(Ordering::SeqCst)==generation) {
                            p.view.state = "scaned".into();
                            p.view.message = "已扫码，请在手机确认".into();
                        }
                    }
                    "wait" => (),
                    _ => {
                        self.pair_error(generation, "error", "微信返回未知绑定状态，请重新开始")
                            .await;
                        return;
                    }
                },
                Err(_) => {
                    if let Some(p) = self.state.lock().await.pairing.as_mut().filter(|p|p.generation==generation && self.generation.load(Ordering::SeqCst)==generation) {
                        p.view.message = "连接暂时中断，正在等待微信响应；可取消后重试".into();
                    }
                }
            }
            tokio::select! {_=tokio::time::sleep(Duration::from_secs(2))=>(),_=self.cancel.notified()=>return}
        }
    }
    async fn pair_error(&self, generation: u64, state: &str, message: &str) {
        if let Some(p) = self.state.lock().await.pairing.as_mut().filter(|p| p.generation == generation && self.generation.load(Ordering::SeqCst) == generation) {
            p.view = PairingView {
                state: state.into(),
                message: message.into(),
                qr_data_url: None,
            };
            p.pin = None;
        }
    }
    pub async fn start(self: &Arc<Self>) -> Result<(), String> {
        self.start_checked(None).await
    }
    async fn start_checked(self: &Arc<Self>, expected: Option<u64>) -> Result<(), String> {
        let mut state = self.state.lock().await;
        if expected.is_some_and(|g| self.generation.load(Ordering::SeqCst) != g || !state.pairing.as_ref().is_some_and(|p|p.generation == g && p.view.state == "confirmed")) {
            return Err("扫码已取消或被替换，未启动接收".into());
        }
        if state.stored.token.is_empty() {
            return Err("请先扫码绑定微信".into());
        }
        if state.stored.reauth_required {
            return Err("微信授权已失效，请重新扫码；无需解除绑定，原任务已保留".into());
        }
        if state.pairing.as_ref().is_some_and(|p| matches!(p.view.state.as_str(), "wait" | "scaned" | "need_pin")) {
            return Err("正在扫码，请先完成或取消当前绑定；未启动接收".into());
        }
        if state.stored.enabled
            && ["connected", "connecting", "reconnecting"].contains(&state.state.as_str())
        {
            return Ok(());
        }
        let mut next = state.stored.clone();
        next.enabled = true;
        (self.save)(next.clone()).await?;
        state.stored = next;
        state.state = "connecting".into();
        state.message = "正在连接微信…".into();
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.cancel.notify_waiters();
        drop(state);
        let bridge = self.clone();
        tokio::spawn(async move {
            bridge.receive(generation).await;
        });
        let bridge = self.clone();
        tokio::spawn(async move {
            bridge.monitor(generation).await;
        });
        Ok(())
    }
    pub async fn stop(&self) -> Result<(), String> {
        let source=self.source_key().await;
        let mut state = self.state.lock().await;
        let mut next = state.stored.clone();
        next.enabled = false;
        (self.save)(next.clone()).await?;
        state.stored = next;
        state.state = if state.stored.reauth_required { "expired" } else { "stopped" }.into();
        state.message = if state.stored.reauth_required { "微信授权已失效，请重新扫码；原任务与绑定记录已保留" } else { "已停止接收；已有任务继续运行，可稍后查询" }.into();
        state.pairing = None;
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancel.notify_waiters();
        drop(state);
        if let Some(agent)=&self.agent {let _=agent(json!({"operation":"notifications","source_key":source,"connected":false})).await;}
        Ok(())
    }
    pub async fn forget(&self) -> Result<(), String> {
        let source=self.source_key().await;
        let mut state = self.state.lock().await;
        (self.save)(Stored::default()).await?;
        state.stored = Stored::default();
        state.pairing = None;
        state.state = "unbound".into();
        state.message = "已解除本机绑定；微信端授权可在手机管理".into();
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancel.notify_waiters();
        drop(state);
        if let Some(agent)=&self.agent {let _=agent(json!({"operation":"notifications","source_key":source,"connected":false})).await;}
        Ok(())
    }
    async fn connection(&self, state: &str, message: &str) {
        self.connection_for(state, message, None).await;
    }
    async fn connection_for(
        &self, state: &str, message: &str, expected: Option<(&Stored, u64)>,
    ) {
        let source = {
            let mut current = self.state.lock().await;
            if expected.is_some_and(|(account, generation)| !self.inbound_account_matches(&current, account, generation)) {
                return;
            }
            current.state = state.into();
            current.message = message.into();
            if let Some(pairing) = current.pairing.as_mut().filter(|p| p.view.state == "confirmed") {
                pairing.view.message = if state == "connected" {
                    "绑定成功，微信已连接".into()
                } else {
                    message.into()
                };
            }
            (state != "connected" && self.agent.is_some()).then(|| current.stored.logical_key())
        };
        if let (Some(source), Some(agent)) = (source, &self.agent) {
            let _ = agent(json!({"operation":"notifications","source_key":source,"connected":false})).await;
        }
    }
    async fn receive(self: Arc<Self>, generation: u64) {
        while self.generation.load(Ordering::SeqCst) == generation {
            let account = self.state.lock().await.stored.account();
            let response = tokio::select! {r=self.api.request(&account.base,"/ilink/bot/getupdates",Some(&account.token),Some(json!({"get_updates_buf":account.cursor})))=>r,_=self.cancel.notified()=>return};
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            match response {
                Ok(value) => match updates(&value) {
                    Err(error) => {
                        self.connection("reconnecting", &error).await;
                    }
                    Ok(messages) => {
                        self.connection("connected", "已连接，仅接收绑定本人的私聊指令")
                            .await;
                        let mut ok = true;
                        for msg in messages {
                            if self.generation.load(Ordering::SeqCst) != generation {
                                return;
                            }
                            if self.ingest_for(msg, generation).await.is_err() {
                                if self.generation.load(Ordering::SeqCst) != generation {
                                    return;
                                }
                                ok = false;
                                break;
                            }
                        }
                        if ok {
                            if let Some(cursor) = value["get_updates_buf"]
                                .as_str()
                                .filter(|s| !s.is_empty() && s.len() < 65536)
                            {
                                let mut state = self.state.lock().await;
                                if self.generation.load(Ordering::SeqCst) != generation
                                    || !state.stored.enabled || state.stored.token != account.token {
                                    return;
                                }
                                if state.stored.cursor != cursor {
                                    let mut next = state.stored.clone();
                                    next.cursor = cursor.into();
                                    if (self.save)(next.clone()).await.is_ok() {
                                        state.stored = next;
                                    } else {
                                        state.state = "error".into();
                                        state.message =
                                            "微信游标保存失败，已停止接收以避免重复操作".into();
                                        return;
                                    }
                                }
                            }
                        } else {
                            self.connection(
                                "error",
                                "微信任务账本写入失败，已停止接收；请核对存储后重启",
                            )
                            .await;
                            return;
                        }
                    }
                },
                Err(error) => {
                    if error.starts_with("SESSION_EXPIRED") {
                        self.mark_stale(generation).await;
                        return;
                    }
                    self.connection("reconnecting", &error).await;
                    tokio::select! {_=tokio::time::sleep(Duration::from_secs(15))=>(),_=self.cancel.notified()=>return}
                }
            }
            tokio::select! {_=tokio::time::sleep(Duration::from_secs(1))=>(),_=self.cancel.notified()=>return}
        }
    }
    async fn request(
        &self,
        method: &str,
        command: Option<&str>,
        args: Value,
        id: &str,
        job: Option<&str>,
    ) -> Result<Value, String> {
        static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let hash = HASH.get_or_init(|| {
            let value: Value = serde_json::from_str(include_str!("../../control/commands.json"))
                .expect("build-verified control contract");
            value["catalog_hash"]
                .as_str()
                .expect("build-verified catalog hash")
                .to_string()
        });
        (self.control)(Request {
            protocol: 1,
            id: uuid::Uuid::new_v4().to_string(),
            token: String::new(),
            method: method.into(),
            catalog_hash: Some(hash.clone()),
            command: command.map(String::from),
            args,
            grants: vec!["private".into(), "write".into(), "external".into()],
            request_id: Some(id.into()),
            job_id: job.map(String::from),
        })
        .await
        .map_err(|e| format!("{}：{}", e.code, e.message))
    }
    /// Operator-only development probe. Reuses the bound owner's last reply context;
    /// never exports that context and never synthesizes an approval message.
    pub async fn self_test_message(&self, text: &str, request_id: &str) -> Result<Value, String> {
        if !cfg!(debug_assertions)
            || std::env::var("HOMEBASE_ENABLE_WECHAT_SELFTEST").as_deref() != Ok("1")
        {
            return Err("微信消息自测仅在显式开启的开发实例可用".into());
        }
        self.self_test_message_inner(text, request_id).await
    }
    async fn self_test_message_inner(&self, text: &str, request_id: &str) -> Result<Value, String> {
        if !request_id.starts_with("hb-selftest-")
            || request_id.len() > 128
            || !request_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            || text.len() > 8000
            || !(text.contains("HB-SELFTEST-") || matches!(text, "新对话" | "进度" | "结果"))
            || text.trim().starts_with("确认")
            || text.trim().starts_with("拒绝")
        {
            return Err("自测消息需要测试标记和稳定编号；不能模拟确认或拒绝".into());
        }
        let (message, id) = {
            let state = self.state.lock().await;
            let stored = &state.stored;
            if !stored.enabled || stored.owner.is_empty() || stored.bot.is_empty() || stored.reply_context_pending {
                return Err("本人微信尚未绑定或通道未启用".into());
            }
            let context = stored.records.iter().rev().find(|r| !r.context.is_empty())
                .ok_or("尚无可用的本人会话回复上下文")?;
            let id = format!("wx-{:x}", Sha256::digest(format!("{}:{}:{}",stored.bot,stored.owner,request_id).as_bytes()));
            if let Some(existing) = stored.records.iter().find(|r| r.id == id) {
                if existing.args["text"].as_str() != Some(text) {
                    return Err("自测编号已经使用；未发送不同内容或重复控制消息".into());
                }
                return Ok(json!({"id":id,"simulated_input":true,"duplicate":true}));
            }
            (json!({"from_user_id":stored.owner,"to_user_id":stored.bot,"message_type":1,
                "message_state":2,"context_token":context.context,"message_id":request_id,
                "item_list":[{"type":1,"text_item":{"text":text}}]}),id)
        };
        self.ingest(&message).await?;
        Ok(json!({"id":id,"simulated_input":true,"duplicate":false,"outbound":"bound_owner_wechat"}))
    }
    pub async fn ingest(&self, msg: &Value) -> Result<(), String> {
        self.ingest_for(msg, self.generation.load(Ordering::SeqCst)).await
    }
    fn inbound_account_matches(&self, state: &State, account: &Stored, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation && state.stored.enabled
            && !account.token.is_empty() && state.stored.token == account.token
            && state.stored.bot == account.bot && state.stored.owner == account.owner
    }
    async fn ingest_for(&self, msg: &Value, generation: u64) -> Result<(), String> {
        let received_at = chrono::Utc::now();
        let account = {
            let state = self.state.lock().await;
            if self.generation.load(Ordering::SeqCst) != generation || !state.stored.enabled {
                return Ok(());
            }
            state.stored.account()
        };
        if msg["from_user_id"] != account.owner
            || msg["to_user_id"] != account.bot
            || msg["message_type"] != 1
            || msg.get("message_state").is_some_and(|v| v != 0 && v != 2)
            || msg["delete_time_ms"].as_u64().unwrap_or(0) > 0
            || msg["group_id"].as_str().is_some_and(|s| !s.is_empty())
        {
            return Ok(());
        }
        let Some(context) = msg["context_token"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() < 8192)
        else {
            return Ok(());
        };
        let Some(identity) = InboundIdentity::from_message(&account, msg) else {
            return Ok(());
        };
        if let Some(voice) = voice::inbound(msg) {
            return match voice {
                voice::InboundVoice::Partial => Ok(()),
                voice::InboundVoice::Complete(Ok(voice::VoiceContent::Audio(value))) => {
                    let attachment = video::voice_attachment(value);
                    match attachment {
                        Ok(media) if self.video_inbox.get().is_some() => {
                            self.receive_media(&account, generation, &identity, context, &media).await
                        }
                        other => {
                            let reason = other.err().unwrap_or_else(|| "本机语音转写尚未就绪。".into());
                            self.receive_voice(&account, generation, &identity, context, Err(&reason)).await
                        }
                    }
                }
                voice::InboundVoice::Complete(Ok(voice::VoiceContent::Text(text))) => {
                    self.receive_voice(&account, generation, &identity, context, Ok(text)).await
                }
                voice::InboundVoice::Complete(Err(reason)) => {
                    self.receive_voice(&account, generation, &identity, context, Err(reason)).await
                }
            };
        }
        let video = video::inbound(msg);
        if let Some(Ok(attachment)) = video.as_ref() {
            if self.video_inbox.get().is_some() {
                return self.receive_media(&account, generation, &identity, context, attachment).await;
            }
        }
        let rejected_video = video.is_some();
        let media = if video.is_none() { pdf::inbound(msg) } else { None };
        if let Some(Ok(attachment)) = media.as_ref() {
            if self.pdf_inbox.is_some() {
                return self.receive_pdf(&account, generation, &identity, context, attachment).await;
            }
        }
        {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, &account, generation)
                || !matches!(self.existing_inbound(&mut state, &identity).await?, InboundMatch::New)
            {
                return Ok(());
            }
        }
        let text = msg["item_list"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter(|i| i["type"] == 1)
                    .filter_map(|i| i["text_item"]["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let resend = file_delivery::resend_intent(&text);
        let task_query = if let Some(files) = resend.map(|_| true).or_else(|| crate::assistant::current_query_files(&text)) {
            let quoted = if let Some(resend) = resend {
                self.quoted_resend(&account, msg, resend).await
            } else {
                self.quoted_task(&account, msg, files).await
            };
            match quoted {
                Some(query) => Some(query),
                None => self.agent.as_ref().map(|_| Ok(Action::CurrentQuery { files, target: None })),
            }
        } else {
            None
        };
        let timeline = clip_video::timeline_answer(&text).filter(|_| self.agent.is_some()
            && video.is_none() && media.is_none() && identity::message_references(msg).next().is_none());
        let followup_args = |turn: &str, file: &Option<file_delivery::FileSelection>, text: &str| json!({
            // Older bridges leave this record pending instead of sending an unbound request.
            "quoted_assistant_v1":{"source":account.logical_key(),"turn":turn,"file":file,"text":text}
        });
        let mut parsed = if let Some(video) = video {
            Ok(Action::Reply(video.err().unwrap_or("我暂时无法在本机接收视频。".into())))
        } else if let Some(media) = media {
            Err(media.err().unwrap_or("当前微信通道未配置本地 PDF 接收".into()))
        } else if text.trim().is_empty() && msg["item_list"].as_array().is_some_and(|items| {
            items.iter().any(|item| matches!(item["type"].as_u64(), Some(kind) if !matches!(kind, 0..=5 | 11 | 12)))
        }) {
            Ok(Action::UnreadableShare)
        } else if timeline.is_some() {
            Ok(Action::Reply("我找不到紧接着这句话的剪辑追问。可以引用要剪的视频，再告诉我按哪份时间留几秒。".into()))
        } else if let Some((index, resend)) = file_delivery::ordinal_intent(&text) {
            Ok(self.numbered_attachment(&account, msg, generation, index, resend).await)
        } else if let Some((name, repeat)) = file_delivery::named_intent(&text) {
            Ok(self.named_attachment(&account, msg, generation, name, repeat).await)
        } else if let Some(query) = task_query {
            query
        } else if let Some(confirmation) = self.quoted_confirmation(&account, msg, &text).await {
            Ok(confirmation)
        } else if let Some(action) = self.voice_task(&account, msg, &text).await {
            Ok(action)
        } else if let Some(capture) = parse_capture(&text) {
            capture
        } else if let Some(browse) = capability_browse::parse(&text) {
            browse
        } else if let Some(analysis) = video::analyze_repository_link(&text, &identity.receipt) {
            analysis
        } else if let Some(search) = self.search_video_repository(msg, &account, &text).await {
            search
        } else if let Some(analysis) = self.video_repository_analysis(msg, &account, generation, &text, &identity.receipt).await {
            analysis
        } else if let Some(install) = self.repository_install_request(msg, &account, generation, &text, &identity.receipt).await {
            install
        } else {
            let command = parse(&text, &self.workspace);
            let assistant = command.is_err() && self.agent.is_some()
                && !text.trim().starts_with('/') && !text.trim().is_empty();
            let quote = if assistant { self.quoted_assistant_followup(&account, msg, &text).await } else { None };
            match quote {
                Some(action) => Ok(action),
                None => {
                    let pdf = self.pdf_question(msg, &account, &text).await;
                    let video = self.video_question(msg, &account, &text).await;
                    let voice = self.voice_question(msg, &account, &text).await;
                    match (pdf, video, voice) {
                        (Some(question), None, None) | (None, Some(question), None) | (None, None, Some(question)) => question,
                        (None, None, None) => match self.recent_video_clip(msg, &account, generation, &text, received_at).await {
                            Some(action) => Ok(action.unwrap_or_else(Action::Reply)),
                            None if assistant && identity::message_references(msg).next().is_some() => Ok(Action::Reply(
                                "我没能核对这条引用。请引用 HomeBase 的原任务回复，或直接说明要处理的对象。".into())),
                            None if assistant => Ok(self.implicit_clip_followup(&account, generation, &text).await
                                .unwrap_or_else(|| Action::Call("assistant".into(), json!({"text":text})))),
                            None => command,
                        },
                        _ => Err("这条消息引用了不同类型的资料。请用 /问PDF、/问视频 或 /问语音 和原收件编号选定一份。".into()),
                    }
                }
            }
        };
        let (command, mut args) = match &parsed {
            Ok(Action::Call(c, a)) => (c.clone(), a.clone()),
            Ok(Action::AssistantFollowup { turn, file, text }) => ("message".into(), followup_args(turn, file, text)),
            Ok(Action::VoiceTask(intent)) => ("message".into(), json!({"voice_task_v1":intent})),
            Ok(Action::UnreadableShare) => (UNREADABLE_SHARE.into(), json!({})),
            Ok(Action::Query(target, files, false)) => {
                let mut args = json!({"numbered_query":{"target":target,"files":files}});
                if resend.is_some() { args["resend_attachment"] = json!(true); }
                ("message".into(), args)
            }
            Ok(Action::CurrentQuery { files, target }) => {
                let mut args = json!({"current_query":{"files":files}});
                if let Some(turn) = target {
                    args["current_query"]["target"] = json!(turn);
                }
                if resend.is_some() { args["resend_attachment"] = json!(true); }
                ("message".into(), args)
            }
            Ok(Action::FileQuery { turn, file }) => ("message".into(), json!({
                // Older clients must leave this query alone, not resend every file.
                "file_query":{"target":turn,"selection":file}
            })),
            Ok(Action::FileRequest { turn, file, resend }) => ("message".into(), json!({
                "file_request_v1":{"target":turn,"selection":file,"resend":resend}
            })),
            Ok(Action::Reply(_)) if rejected_video => {
                let source = account.logical_key();
                let scope = self.read_video_scope(&source).await.ok().flatten();
                ("message".into(), json!({"video_rejected":true,"source":source,"scope":scope}))
            }
            _ => ("message".into(), json!({})),
        };
        let id = identity.receipt.clone();
        {
            let mut state = self.state.lock().await;
            if !self.inbound_account_matches(&state, &account, generation) {
                return Ok(());
            }
            if !matches!(self.existing_inbound(&mut state, &identity).await?, InboundMatch::New) {
                return Ok(());
            }
            if state.stored.records.len() >= 5000 {
                return Err("微信账本已满，请归档后继续".into());
            }
            // Select and consume the prompt in the same ledger write as the answer.
            if let Some(Action::AssistantFollowup { turn, file, text }) = timeline.and_then(|timeline|
                Self::clip_timeline_followup(&state.stored, &account.logical_key(), timeline, received_at))
            {
                args = followup_args(&turn, &file, &text);
                parsed = Ok(Action::AssistantFollowup { turn, file, text });
            }
            let mut next = state.stored.clone();
            next.reply_context_pending = false;
            next.records.push(Record {
                id: id.clone(),
                inbound_keys: identity.keys,
                command: command.clone(),
                args: args.clone(),
                context: context.into(),
                status: if command == UNREADABLE_SHARE { "unsupported" } else { "received" }.into(),
                client_id: format!("hb-{}", uuid::Uuid::new_v4()),
                created_at: chrono::Utc::now().to_rfc3339(),
                delivery: "pending".into(),
                ..Record::default()
            }.with_persisted_inbound_keys());
            (self.save)(next.clone()).await?;
            state.stored = next;
        }
        match parsed {
            Ok(Action::Call(_, _) | Action::AssistantFollowup { .. }) => {
                self.submit(&id).await?;
            }
            Ok(Action::VoiceTask(_)) => self.submit_record(&id, false).await?,
            Ok(Action::Query(task, files, true)) => {
                self.respond_numbered_query(&id, &task, files, true, (&account, generation)).await?;
            }
            Ok(Action::Query(_, _, false) | Action::CurrentQuery { .. } | Action::FileQuery { .. } | Action::FileRequest { .. }) => self.resume_saved_query(&id, generation).await?,
            Ok(Action::Help) => self.reply(&id, HELP, None).await?,
            Ok(Action::Reply(text)) => self.reply(&id, &text, None).await?,
            Ok(Action::UnreadableShare) => {
                self.reply_files_with_claim(&id, UNREADABLE_SHARE_REPLY, &[], Some((&account, generation)), true, None, None).await?;
            }
            Err(error) => {
                self.reply(&id, &format!("{error}\n发送“帮助”查看指令。"), None)
                    .await?
            }
        }
        Ok(())
    }
    async fn observe_query<T>(
        &self, generation: u64, read: impl Future<Output = T>,
    ) -> Option<T> {
        let canceled = self.cancel.notified();
        tokio::pin!(canceled);
        canceled.as_mut().enable();
        if self.generation.load(Ordering::SeqCst) != generation {
            return None;
        }
        tokio::select! {
            biased;
            _ = &mut canceled => None,
            result = read => Some(result),
        }
    }

    async fn resume_saved_query(&self, id: &str, generation: u64) -> Result<(), String> {
        let Some(_guard) = self.observe_query(generation, self.submit_lock.lock()).await else {
            return Ok(());
        };
        let (account, record) = {
            let state = self.state.lock().await;
            let account = state.stored.account();
            if state.stored.reply_context_pending
                || !self.inbound_account_matches(&state, &account, generation)
            {
                return Ok(());
            }
            let record = state.stored.records.iter().find(|record| record.id == id).cloned();
            (account, record)
        };
        let origin = (&account, generation);
        let Some((target, files)) = record.as_ref().and_then(Record::pending_query) else { return Ok(()); };
        let selected;
        let turn = match target {
            QueryTarget::Numbered(target) => return self.respond_numbered_query(id, target, files, false, origin).await,
            QueryTarget::Turn(turn) => Some(turn),
            QueryTarget::Empty => None,
            QueryTarget::Unresolved => {
                let agent = self.agent.as_ref().ok_or("共享助理不可用")?;
                let read = agent(json!({"operation":"resolve_current_query","source_key":account.logical_key()}));
                let result = match self.observe_query(generation, read).await {
                    Some(Ok(result)) => result,
                    Some(Err(error)) => return self.reply_files_for(id, &error, &[], Some(origin)).await,
                    None => return Ok(()),
                };
                selected = match result.get("target").cloned() {
                    Some(Value::Null) => Value::Null,
                    Some(Value::String(turn)) if valid_task_id(&turn) => Value::String(turn),
                    _ => return self.reply_files_for(id, "暂时无法核对当前任务。请稍后再查。", &[], Some(origin)).await,
                };
                if !self.change_for(id, Some(origin), |record| {
                    record.args["current_query"]["target"] = selected.clone();
                }).await? {
                    return Ok(());
                }
                selected.as_str()
            }
        };
        match turn {
            Some(turn) => self.reply_assistant_turn(id, turn, files, origin).await,
            None => self.reply_files_for(id, "这次查询没有关联到任务。你可以直接告诉我想做什么。", &[], Some(origin)).await,
        }
    }

    async fn reply_assistant_turn(
        &self, id: &str, turn: &str, files: bool, origin: (&Stored, u64),
    ) -> Result<(), String> {
        let (account, generation) = origin;
        let read = self.agent_get_from(turn, files, account.logical_key());
        match self.observe_query(generation, read).await {
            Some(Ok(value)) => self.reply_assistant_for(id, &value, files, Some(origin), false).await,
            Some(Err(_)) => self.reply_files_for(id, "暂时无法核对原任务，请稍后再查。", &[], Some(origin)).await,
            None => Ok(()),
        }
    }

    async fn respond_numbered_query(
        &self, id: &str, task: &str, files: bool, remote: bool, origin: (&Stored, u64),
    ) -> Result<(), String> {
        let target = {
            let state = self.state.lock().await;
            if !self.inbound_account_matches(&state, origin.0, origin.1) {
                return Ok(());
            }
            state.stored.records.iter()
                .find(|record| record.id == task || record.job_id.as_deref() == Some(task))
                .cloned()
        };
        let Some(target) = target else {
            return self.reply_files_for(id, "未找到本微信通道的任务。请使用原消息中的完整编号。", &[], Some(origin)).await;
        };
        if target.local_media_kind().is_some() && !remote {
            return self.query_media(id, &target, files, origin).await;
        }
        if target.command == voice::COMMAND && !remote {
            return self.query_voice(id, &target, files, origin).await;
        }
        if target.command == "wechat_pdf" && !remote {
            return self.query_pdf(id, &target, files, origin).await;
        }
        if target.command == UNREADABLE_SHARE && !remote {
            return self.reply_files_for(id, UNREADABLE_SHARE_REPLY, &[], Some(origin)).await;
        }
        if target.command == "assistant" {
            let Some(turn) = &target.job_id else {
                return self.reply_files_for(id, "这件事还没有受理，可以稍后用原编号查询。", &[], Some(origin)).await;
            };
            return self.reply_assistant_turn(id, turn, files, origin).await;
        }
        if remote {
            let Some(value) = target.result else {
                return self.reply_files_for(id, "尚未取得服务商任务编号，请先查询控制任务状态。", &[], Some(origin)).await;
            };
            if target.command != "model_capability_call"
                || !["queued", "running"].contains(&value["status"].as_str().unwrap_or(""))
            {
                return self.reply_files_for(id, "该任务不是待查询的服务商异步任务。", &[], Some(origin)).await;
            }
            let task_id = value["task_id"].as_str().filter(|s| !s.is_empty()).ok_or("服务商任务编号缺失")?;
            let route = target.args["request"]["route_id"].as_str().ok_or("原渠道缺失")?;
            if !self.change_for(id, Some(origin), |record| {
                record.command = "model_capability_task".into();
                record.args = json!({
                    "routeId": route, "taskId": task_id,
                    "credentialSource": target.args["request"].get("credential_source").cloned().unwrap_or(json!("default")),
                    "outputRoot": self.workspace.join("models"),
                });
                record.persist_inbound_keys();
            }).await? {
                return Ok(());
            }
            return self.submit(id).await;
        }

        let value = if target.has_saved_control_result() {
            None
        } else if let Some(job) = &target.job_id {
            let read = self.request("jobs.get", None, json!({}), id, Some(job));
            match self.observe_query(origin.1, read).await {
                Some(Ok(value)) if control_job_status(&value).is_some() => Some(value),
                Some(Ok(_)) => return self.reply_files_for(id, "暂时无法核对原控制任务的状态。请稍后用原编号再查。", &[], Some(origin)).await,
                Some(Err(error)) => return self.reply_files_for(id, &format!("任务查询失败：{error}。请稍后用原编号查询。"), &[], Some(origin)).await,
                None => return Ok(()),
            }
        } else {
            None
        };
        let observed = value.as_ref().and_then(|value| value.get("result").filter(|value| !value.is_null()));
        let status = value.as_ref().and_then(control_job_status).unwrap_or(&target.status);
        if target.status == "unknown" && target.has_control_job()
            && matches!(status, "succeeded" | "failed" | "canceled" | "timed_out")
        {
            if let Some(result) = observed {
                let updated = self.change_for(&target.id, Some(origin), |record| {
                    if record.status == "unknown" && record.job_id == target.job_id {
                        if record.pending_control_completion().is_some() {
                            record.clear_completion_reply();
                            record.delivery = "not_sent".into();
                        }
                        record.status = status.into();
                        record.result = Some(result.clone());
                        record.clear_query_retry();
                    }
                }).await?;
                if !updated {
                    return Ok(());
                }
            }
        }
        let payload = if value.is_some() { observed } else { target.result.as_ref() };
        let problem = value.as_ref().and_then(|value| value["error"]["message"].as_str());
        let kind = if files { ControlReply::Result } else { ControlReply::Status };
        let response = control_reply(&target, status, payload, problem, kind);
        let artifact = if files {
            payload.and_then(|value| value.get("artifact")).filter(|value| !value.is_null())
        } else {
            None
        };
        let artifacts = artifact.cloned().into_iter().collect::<Vec<_>>();
        self.reply_files_for(id, &response, &artifacts, Some(origin)).await
    }

    async fn change(&self, id: &str, mutate: impl FnOnce(&mut Record)) -> Result<(), String> {
        self.change_for(id, None, mutate).await.map(|_| ())
    }
    async fn change_for(
        &self,
        id: &str,
        expected: Option<(&Stored, u64)>,
        mutate: impl FnOnce(&mut Record),
    ) -> Result<bool, String> {
        let mut state = self.state.lock().await;
        if expected.is_some_and(|(account, generation)| {
            !self.inbound_account_matches(&state, account, generation)
        }) {
            return Ok(false);
        }
        let mut next = state.stored.clone();
        let record = next
            .records
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("微信任务记录缺失")?;
        mutate(record);
        (self.save)(next.clone()).await?;
        state.stored = next;
        Ok(true)
    }
    async fn submit(&self, id: &str) -> Result<(), String> {
        self.submit_record(id, true).await
    }
    async fn submit_record(&self, id: &str, recovering: bool) -> Result<(), String> {
        let _guard = self.submit_lock.lock().await;
        let record = {
            self.state
                .lock()
                .await
                .stored
                .records
                .iter()
                .find(|r| r.id == id)
                .cloned()
                .ok_or("微信任务记录缺失")?
        };
        if record.job_id.is_some() {
            return Ok(());
        }
        let followup = record.pending_assistant_followup();
        let voice_task = record.pending_voice_task();
        if record.command == "message" && followup.is_none() && voice_task.is_none() { return Ok(()); }
        if record.command == "assistant" || followup.is_some() || voice_task.is_some() {
            let mut recovered_voice = None;
            let request = if let Some(voice_task) = voice_task {
                match self.prepare_voice_task(voice_task, recovering).await {
                    Ok(voice::VoiceTaskAdmission::Submit(request)) => Ok(Some(request)),
                    Ok(voice::VoiceTaskAdmission::Recovered(ack)) => {
                        recovered_voice = Some(ack);
                        Ok(None)
                    }
                    Ok(voice::VoiceTaskAdmission::Existing(turn)) => {
                        let generation = self.generation.load(Ordering::SeqCst);
                        let account = self.state.lock().await.stored.account();
                        if voice_task["source"] != account.logical_key() { return Ok(()); }
                        let saved = self.change_for(id, Some((&account, generation)), |r| {
                            r.args["current_query"] = json!({"target":&turn,"files":false});
                        }).await?;
                        if saved { self.reply_assistant_turn(id, &turn, false, (&account, generation)).await?; }
                        return Ok(());
                    }
                    Err(error) => Err(error),
                }
            } else if let Some(followup) = followup {
                let key = self.source_key().await;
                if followup["source"] != key { return Ok(()); }
                Ok(Some(json!({"operation":"submit_quoted_task","source_key":key,"request_id":id,
                    "text":followup["text"],"turn_id":followup["turn"],"file":followup["file"]})))
            } else if let Some(receipt) = record.args["pdf_receipt"].as_str() {
                self.pdf_question_request(&record, receipt).await
            } else if let Some(receipt) = record.args["video_receipt"].as_str() {
                self.video_question_request(&record, receipt).await
            } else if let Some(receipt) = record.args["video_clip_receipt"].as_str() {
                self.video_clip_request(&record, receipt).await
            } else if let Some(receipt) = record.args["voice_receipt"].as_str() {
                self.voice_question_request(&record, receipt).await
            } else if let Some(intent) = record.args.get("repository_install") {
                let key = self.source_key().await;
                Ok(Some(json!({"operation":"prepare_repository_install","source_key":key,"request_id":id,"intent":intent})))
            } else {
                let key = self.source_key().await;
                Ok(Some(json!({"operation":"submit","source_key":key,"request_id":id,"text":record.args["text"]})))
            };
            let result = match request {
                Ok(Some(input)) => match &self.agent {
                    Some(agent) => agent(input).await,
                    None => Err("共享助理不可用".into()),
                },
                Ok(None) => match recovered_voice { Some(ack) => Ok(ack), None => return Ok(()) },
                Err(error) => Err(error),
            };
            match result {
                Ok(v) => {
                    if v["action"] == "message" {
                        {
                            let mut state=self.state.lock().await;let mut next=state.stored.clone();
                            let source = next.logical_key();
                            if let Some(turn)=v["turn_id"].as_str(){
                                for r in &mut next.records {
                                    if r.id!=id&&r.command=="assistant"&&r.job_id.as_deref()==Some(turn)&&["running","awaiting_confirmation","waiting_input_queue"].contains(&r.status.as_str()) {r.status="continued".into();}
                                }
                            }
                            let r=next.records.iter_mut().find(|r|r.id==id).ok_or("控制回复记录不存在")?;
                            let clarification = followup.filter(|input| input["source"] == source)
                                .and_then(|input| clip_video::ambiguous_clip_range(input["text"].as_str()?))
                                .and_then(|_| serde_json::from_value::<file_delivery::FileSelection>(v["clip_clarification_file"].clone()).ok());
                            if let Some(file) = clarification {
                                r.args["clip_clarification_file_v1"] = json!(file);
                            }
                            r.command="message".into();r.status="completed".into();
                            (self.save)(next.clone()).await?;state.stored=next;
                        }
                        self.reply(id, v["text"].as_str().ok_or("助理控制回复缺失")?, None)
                            .await?;
                        return Ok(());
                    }
                    if v["action"] == "view" {
                        self.change(id, |r| {
                            r.command = "message".into();
                            r.status = "completed".into();
                            r.delivery = "unknown".into();
                        })
                        .await?;
                        self.reply_assistant(id, &v["view"], v["files"] == true)
                            .await?;
                        return Ok(());
                    }
                    let turn = v["turn_id"].as_str().ok_or("助理缺少轮次编号")?;
                    // A human confirmation transfers delivery to its latest message context.
                    {
                        let mut state=self.state.lock().await;
                        let mut next=state.stored.clone();
                        let progress=next.records.iter().filter(|r|r.id!=id&&r.command=="assistant"&&r.job_id.as_deref()==Some(turn)).max_by_key(|r|r.progress_count).cloned();
                        for r in &mut next.records {
                            if r.id!=id && r.command=="assistant" && r.job_id.as_deref()==Some(turn)
                                && ["running","awaiting_confirmation","waiting_input_queue"].contains(&r.status.as_str()) {
                                r.status="continued".into();
                            }
                        }
                        let r=next.records.iter_mut().find(|r|r.id==id).ok_or("接收记录不存在")?;
                        r.command="assistant".into();
                        r.job_id=Some(turn.into());
                        r.status=if v["state"]=="waiting_input_queue"{"waiting_input_queue"}else{"running"}.into();
                        if let Some(previous)=progress {
                            r.progress_count=previous.progress_count;r.progress_at=previous.progress_at;
                            r.progress_signature=previous.progress_signature;r.progress_delivery=previous.progress_delivery;
                        }
                        (self.save)(next.clone()).await?;state.stored=next;
                    }
                    if v["state"] == "disabled" {
                        let value = self.agent_get(turn, false).await?;
                        self.change(id, |r| r.status = "disabled".into()).await?;
                        self.reply_assistant(id, &value, false).await?;
                    } else if v["interaction"]!="chat" {
                        let acknowledgement = match v["state"].as_str() {
                            Some("queued" | "running" | "waiting_jobs") if v["action"] == "confirmation" => {
                                Some("收到，我会继续跟进这件事。")
                            }
                            // The watch will send the actual outcome or the next confirmation.
                            _ => None,
                        };
                        if let Some(text) = acknowledgement {
                            self.reply(id, text, None).await?;
                        }
                    }
                }
                Err(error) => {
                    self.change(id, |r| r.status = "failed".into()).await?;
                    self.reply(id, &error, None).await?;
                }
            }
            return Ok(());
        }
        let result = self
            .request("call", Some(&record.command), record.control_args(), id, None)
            .await;
        match result {
            Ok(v) => {
                let job = v["job"]["id"].as_str().ok_or("控制任务缺少编号")?;
                self.change(id, |r| {
                    r.job_id = Some(job.into());
                    r.status = "running".into();
                })
                .await?;
                self.reply(
                    id,
                    &format!(
                        "收到，我来处理「{}」。\n任务编号：{id}\n发 /状态 {id} 就能查进度。",
                        control_task_name(&record.command)
                    ),
                    None,
                )
                .await?;
            }
            Err(e) => {
                self.change(id, |r| r.status = "failed".into()).await?;
                let message = if record.command == capability_browse::COMMAND {
                    "暂时无法读取能力库，请稍后再查。"
                } else {
                    &e
                };
                self.reply(id, message, None).await?;
            }
        }
        Ok(())
    }
    async fn reply(&self, id: &str, text: &str, file: Option<&Value>) -> Result<(), String> {
        self.reply_files(id, text, &file.cloned().into_iter().collect::<Vec<_>>())
            .await
    }
    async fn source_key(&self) -> String {
        let state = self.state.lock().await;
        state.stored.logical_key()
    }
    async fn agent_get(&self, turn: &str, include_files: bool) -> Result<Value, String> {
        self.agent_get_from(turn, include_files, self.source_key().await).await
    }
    async fn agent_get_from(
        &self,
        turn: &str,
        include_files: bool,
        source_key: String,
    ) -> Result<Value, String> {
        let value = (self.agent.as_ref().ok_or("共享助理不可用")?)(
            json!({"operation":"get","source_key":source_key,"turn_id":turn,"include_files":include_files}),
        )
        .await?;
        if !valid_assistant_observation(&value, turn) {
            return Err("助理状态回复无效，暂时无法核对原任务".into());
        }
        Ok(value)
    }
    async fn reply_assistant(&self, id: &str, value: &Value, files: bool) -> Result<(), String> {
        self.reply_assistant_for(id, value, files, None, false).await
    }
    async fn reply_assistant_for(
        &self,
        id: &str,
        value: &Value,
        files: bool,
        expected: Option<(&Stored, u64)>,
        completion: bool,
    ) -> Result<(), String> {
        let payload = &value["result"];
        let text = payload["text"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("助理正在处理，请稍后查询状态。");
        let text = if payload["queue_paused"] == true
            && payload["queue_depth"].as_u64().unwrap_or(0) > 0
        {
            format!(
                "{text}\n\n还有 {} 条后续消息已暂停。说“继续排队”可处理，原任务不会自动重做。",
                payload["queue_depth"]
            )
        } else {
            text.to_string()
        };
        let artifacts = if files {
            payload["artifacts"].as_array().cloned().unwrap_or_default()
        } else {
            vec![]
        };
        let notice = if value["status"] == "succeeded" && artifacts.is_empty() && id.starts_with("hb-proactive-") {
            let notice = &payload["notification"];
            notice["group"].as_str().zip(notice["fingerprint"].as_str())
        } else {
            None
        };
        let assistant = payload["turn_id"].as_str().filter(|turn| valid_task_id(turn)).map(|turn| {
            let confirmation = if value["status"] == "awaiting_confirmation" && payload["status"] == "awaiting_confirmation" {
                payload["confirmation_id"].as_str().and_then(|id| uuid::Uuid::parse_str(id).ok())
            } else { None };
            AssistantReply { turn, confirmation }
        });
        self.reply_files_with_claim(id, &text, &artifacts, expected, completion, notice, assistant).await
    }
    async fn reply_files(&self, id: &str, text: &str, files: &[Value]) -> Result<(), String> {
        self.reply_files_for(id, text, files, None).await
    }

    async fn reply_files_for(
        &self, id: &str, text: &str, files: &[Value], expected: Option<(&Stored, u64)>,
    ) -> Result<(), String> {
        self.reply_files_with_claim(id, text, files, expected, false, None, None).await
    }

    async fn reply_files_with_claim(
        &self,
        id: &str,
        text: &str,
        files: &[Value],
        expected: Option<(&Stored, u64)>,
        completion: bool,
        notice: Option<(&str, &str)>,
        assistant: Option<AssistantReply<'_>>,
    ) -> Result<(), String> {
        let canceled = self.cancel.notified();
        tokio::pin!(canceled);
        if expected.is_some() {
            canceled.as_mut().enable();
        }
        let _attachment_guard = if files.is_empty() { None } else {
            Some(tokio::select! {
                biased;
                _ = &mut canceled, if expected.is_some() => return Ok(()),
                guard = self.attachment_lock.lock() => guard,
            })
        };
        let (account, generation, record, assistant_id, outgoing_files, file_note, file_list) = {
            let mut state = self.state.lock().await;
            let account = state.stored.account();
            if account.token.is_empty() || !account.enabled
                || expected.is_some_and(|(origin, generation)| !self.inbound_account_matches(&state, origin, generation))
            {
                return Ok(());
            }
            let index = state.stored.records.iter().position(|record| record.id == id).ok_or("微信记录缺失")?;
            let record = state.stored.records[index].clone();
            let query = record.query().is_some();
            let target_index = assistant.and_then(|reply| state.stored.records.iter().enumerate()
                .filter(|(_, task)| task.command == "assistant" && task.job_id.as_deref() == Some(reply.turn))
                .max_by_key(|(position, task)| (task.attachment_receipts.is_some(), *position))
                .map(|(position, _)| position));
            let target = target_index.map(|position| &state.stored.records[position]);
            let assistant_id = target.map(|task| task.id.clone());
            let requested_file = record.args.get("file_query").or_else(|| record.args.get("file_request_v1"));
            let allow_resend = query && (record.args["resend_attachment"] == true || record.args.get("file_query").is_some()
                || record.args["file_request_v1"]["resend"] == true);
            let saved_delivery = target.map(|task| task.file_delivery_view().0);
            let outcome = |file: &Value| target.and_then(|task| task.file_outcome(file, files));
            let has_unknown = saved_delivery == Some("attachment_unknown") || files.iter().take(3).any(|file| outcome(file) == Some(file_delivery::Outcome::Unknown));
            let has_unsettled = has_unknown || saved_delivery == Some("attachment_failed")
                || files.iter().take(3).any(|file| outcome(file) == Some(file_delivery::Outcome::NotSent));
            let mut unconfirmed = files.is_empty() && query && has_unknown;
            let outgoing_files = if let Some(file) = requested_file {
                let bound = matches!((record.query(), assistant),
                    (Some((QueryTarget::Turn(target), true)), Some(reply)) if target == reply.turn);
                if bound && target.is_some() {
                    serde_json::from_value::<file_delivery::FileSelection>(file["selection"].clone()).ok()
                        .and_then(|file| file.find(files)).filter(|(_, selected)| {
                            let held = !allow_resend && outcome(selected) == Some(file_delivery::Outcome::Unknown);
                            unconfirmed |= held;
                            !held
                        }).into_iter().collect::<Vec<_>>()
                } else { vec![] }
            } else {
                files.iter().take(3).enumerate().filter(|(_, file)| {
                    let delivery = outcome(file);
                    let hold_unconfirmed = !allow_resend
                        && delivery == Some(file_delivery::Outcome::Unknown);
                    unconfirmed |= hold_unconfirmed;
                    !hold_unconfirmed && !(has_unsettled && delivery == Some(file_delivery::Outcome::Accepted))
                }).collect::<Vec<_>>()
            };
            let file_note = if requested_file.is_some() && outgoing_files.is_empty() {
                Some(if unconfirmed { "这份附件的发送回执待核对。确认手机未收到时，可以按原附件列表说「重发第一份」等，或引用附件说「重发这份」。" }
                    else { file_delivery::FILE_UNAVAILABLE })
            } else if unconfirmed {
                Some(file_delivery::UNCONFIRMED)
            } else if files.is_empty() && query && target.is_some_and(|task| task.file_delivery_view().0 == "attachment_failed") {
                Some(file_delivery::NOT_SENT)
            } else {
                None
            };
            let foreign_local_completion = completion
                && (record.pending_media_completion() || record.pending_pdf_completion())
                && record.args["source"] != account.logical_key();
            if record.reply_context_retired()
                || foreign_local_completion
                || (query && (account.reply_context_pending || record.pending_query().is_none()))
                || (completion && (account.reply_context_pending || !record.pending_completion()))
            {
                return Ok(());
            }
            let file_list = if target.is_some() && requested_file.is_none() {
                assistant.and_then(|reply| file_delivery::FileList::for_reply(account.logical_key(), reply.turn, files))
            } else { None };
            if query || completion || notice.is_some() || (target_index.is_some() && !outgoing_files.is_empty()) {
                let mut next = state.stored.clone();
                if let Some(position) = target_index.filter(|_| !outgoing_files.is_empty()) {
                    next.records[position].init_file_receipts();
                }
                let outgoing = &mut next.records[index];
                let mut duplicate = false;
                if let Some((group, fingerprint)) = notice {
                    if record.notice_fingerprint == fingerprint
                        && matches!(record.delivery.as_str(), "suppressed" | "accepted" | "unknown")
                    {
                        return Ok(());
                    }
                    duplicate = next.notices.iter().any(|seen| {
                        seen.owner == next.owner && seen.group == group && seen.fingerprint == fingerprint
                    });
                    outgoing.notice_fingerprint = fingerprint.into();
                    if !duplicate {
                        next.notices.retain(|seen| seen.owner != next.owner || seen.group != group);
                        next.notices.push(NoticeReceipt {
                            owner: next.owner.clone(), group: group.into(), fingerprint: fingerprint.into(),
                        });
                        if next.notices.len() > 128 {
                            next.notices.remove(0);
                        }
                    }
                }
                if query {
                    outgoing.status = "completed".into();
                }
                if completion {
                    outgoing.clear_completion_reply();
                }
                outgoing.delivery = if duplicate { "suppressed" } else { "unknown" }.into();
                // The attempt and any notice fingerprint must survive a lost acknowledgement.
                (self.save)(next.clone()).await?;
                state.stored = next;
                if duplicate {
                    return Ok(());
                }
            }
            (account, self.generation.load(Ordering::SeqCst), record, assistant_id, outgoing_files, file_note, file_list)
        };
        let note;
        let text = if let Some((_, lines)) = file_list.as_ref() {
            note = format!("{}\n\n附件列表\n{lines}\n可以说「只发第二份」取其中一份。{}",
                clip(text, 2850), file_note.map_or_else(String::new, |message| format!("\n\n{message}")));
            note.as_str()
        } else if let Some(message) = file_note.as_ref() {
            note = format!("{}\n\n{message}", clip(text, 3150));
            note.as_str()
        } else { text };
        let send = async {
            self.api.text(&account, &record.context, &record.client_id, text).await
        };
        let sent = tokio::select! {
            biased;
            _ = &mut canceled, if expected.is_some() => return Ok(()),
            result = send => result,
        };
        let confirmation = assistant.and_then(|reply| reply.confirmation)
            .filter(|_| file_note.is_none() && text.chars().count() <= 3500);
        let confirmation_quote = sent.as_ref().ok().and_then(|key| key.as_ref()).zip(confirmation)
            .map(|(key, confirmation)| identity::ConfirmationQuote::new(key.clone(), confirmation));
        let remembered_list = file_list.zip(sent.as_ref().ok().cloned());
        let (mut delivery, mut delivery_error, mut outbound_keys) = match sent {
            Ok(key) => ("accepted", None, key.into_iter().collect::<Vec<_>>()),
            Err(error) => ("unknown", Some(error), Vec::new()),
        };
        if delivery_error.is_none() {
            for (index, artifact) in outgoing_files {
                let path = Path::new(artifact["path"].as_str().unwrap_or(""));
                let digest = artifact["sha256"].as_str().unwrap_or("");
                let file_client_id = format!("{}-file-{index}", record.client_id);
                let upload = self.api.prepare_file(
                        &account,
                        path,
                        &self.workspace,
                        digest,
                    );
                let prepared = tokio::select! {
                    biased;
                    _ = &mut canceled, if expected.is_some() => return Ok(()),
                    result = upload => result,
                };
                let origin = (&account, generation);
                let (outcome, error) = match prepared {
                    Ok(item) => {
                        if let Some(assistant) = assistant_id.as_deref() {
                            if !self.record_file_outcome(origin, assistant, artifact, files, file_delivery::Outcome::Unknown, None).await? { return Ok(()); }
                        }
                        let sent = tokio::select! {
                            biased;
                            _ = &mut canceled, if expected.is_some() => return Ok(()),
                            result = self.api.send(&account, &record.context, &file_client_id, item) => result,
                        };
                        match sent {
                            Ok(key) => {
                                if let Some(assistant) = assistant_id.as_deref() {
                                    if !self.record_file_outcome(origin, assistant, artifact, files, file_delivery::Outcome::Accepted, key.as_deref()).await? { return Ok(()); }
                                } else {
                                    outbound_keys.extend(key);
                                }
                                (file_delivery::Outcome::Accepted, None)
                            }
                            Err(error) => (file_delivery::Outcome::Unknown, Some(error)),
                        }
                    }
                    Err(error) => {
                        if let Some(assistant) = assistant_id.as_deref() {
                            if !self.record_file_outcome(origin, assistant, artifact, files, file_delivery::Outcome::NotSent, None).await? { return Ok(()); }
                        }
                        (file_delivery::Outcome::NotSent, Some(error))
                    }
                };
                if let Some(error) = error {
                    let unknown = outcome == file_delivery::Outcome::Unknown;
                    let message = match (unknown, assistant_id.is_some()) {
                        (true, true) => file_delivery::UNCONFIRMED,
                        (false, true) => file_delivery::NOT_SENT,
                        (true, false) => "微信附件发送回执待核对，未自动重试。",
                        (false, false) => "这次附件未进入微信消息发送，请查询原任务。",
                    };
                    if unknown || delivery != "attachment_unknown" {
                        delivery = if unknown { "attachment_unknown" } else { "attachment_failed" };
                        delivery_error = Some(format!("{message}\n{error}"));
                    }
                    let mut user_notice = if unknown { message.into() } else { format!("{message}\n{error}") };
                    let exact_notice = assistant_id.as_deref()
                        .zip(file_delivery::FileSelection::from_artifact(artifact));
                    if exact_notice.is_some() {
                        user_notice.push_str("\n也可以引用这条提示说「重发这份」。");
                        if let Some(name) = path.file_name().and_then(|name| name.to_str())
                            .filter(|name| file_delivery::valid_attachment_name(name))
                        {
                            user_notice.push_str(&format!("引用原任务回复时，可以说「重发 '{name}'」。"));
                        }
                    }
                    let error_client_id = format!("{}-file-{index}-error", record.client_id);
                    let notice = self.api.text(
                            &account,
                            &record.context,
                            &error_client_id,
                            &user_notice,
                        );
                    let notice = tokio::select! {
                        biased;
                        _ = &mut canceled, if expected.is_some() => return Ok(()),
                        result = notice => result,
                    };
                    if let (Some((assistant, selection)), Ok(Some(key))) = (exact_notice, notice) {
                        if !self.remember_file_notice(origin, assistant, selection, &key).await? {
                            return Ok(());
                        }
                    }
                }
            }
        }
        let account_guard = if confirmation_quote.is_some() || remembered_list.is_some() { Some((&account, generation)) } else { expected };
        self.change_for(id, account_guard, |r| {
            r.delivery = delivery.into();
            r.delivery_error = delivery_error;
            r.remember_outbound_keys(outbound_keys);
            if let Some(quote) = confirmation_quote { r.remember_confirmation_quote(quote); }
            if let Some(((list, _), key)) = remembered_list { list.remember(r, key.as_deref()); }
            if r.command == "message" {
                r.status = "completed".into();
            }
        })
        .await
        .map(|_| ())
    }
    async fn emit_progress(
        &self,
        id: &str,
        value: &Value,
        expected: Option<(&Stored, u64)>,
    ) -> Result<(), String> {
        let progress = &value["result"];
        let Some(text) = progress["progress"].as_str().map(str::trim).filter(|s|!s.is_empty()) else {
            return Ok(());
        };
        if progress["progress_has_execution_evidence"] != true { return Ok(()); }
        let signature = format!("{:x}", Sha256::digest(text.as_bytes()));
        let old_signature = progress["progress_signature"].as_str().filter(|s| !s.is_empty());
        let canceled = self.cancel.notified();
        let (account, record) = {
            let mut state = self.state.lock().await;
            if !state.stored.enabled || state.stored.token.is_empty()
                || expected.is_some_and(|(account, generation)| !self.inbound_account_matches(&state, account, generation))
            {
                return Ok(());
            }
            let current = state
                .stored
                .records
                .iter()
                .find(|r| r.id == id)
                .ok_or("进度记录不存在")?;
            if current.reply_context_retired() || current.progress_signature == signature
                || old_signature == Some(current.progress_signature.as_str()) {
                return Ok(());
            }
            let elapsed = current
                .progress_at
                .as_ref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| (chrono::Utc::now()-t.with_timezone(&chrono::Utc)).num_seconds());
            let age=chrono::DateTime::parse_from_rfc3339(&current.created_at).ok().map(|t|(chrono::Utc::now()-t.with_timezone(&chrono::Utc)).num_seconds()).unwrap_or(0);
            let due = elapsed.map_or(age >= 12, |seconds| seconds >= 45);
            if !due || current.progress_count >= 12 {
                return Ok(());
            }
            let mut next = state.stored.clone();
            let record = next.records.iter_mut().find(|r| r.id == id).unwrap();
            record.progress_signature = signature;
            record.progress_at = Some(chrono::Utc::now().to_rfc3339());
            record.progress_count += 1;
            record.progress_delivery = "unknown".into();
            let record = record.clone();
            (self.save)(next.clone()).await?;
            state.stored = next;
            (state.stored.account(), record)
        };
        let client_id = format!("{}-progress-{}", record.client_id, record.progress_count);
        let send = self.api.text(
            &account,
            &record.context,
            &client_id,
            text,
        );
        let sent = tokio::select! {
            biased;
            _ = canceled, if expected.is_some() => return Ok(()),
            result = send => result,
        };
        self.change_for(id, expected, |r| {
            r.progress_delivery = if sent.is_ok() { "accepted" } else { "unknown" }.into();
            if let Ok(Some(key)) = &sent {
                r.remember_outbound_keys([key.clone()]);
            }
        })
        .await?;
        sent.map(|_| ())
    }
    #[cfg(test)]
    async fn deliver_reminders(&self) {
        self.deliver_reminders_for(self.generation.load(Ordering::SeqCst)).await;
    }
    async fn deliver_reminders_for(&self, generation: u64) {
        let Some(agent) = &self.agent else {
            return;
        };
        let (account, context, connected) = {
            let state = self.state.lock().await;
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            (
                state.stored.account(),
                state
                    .stored
                    .records
                    .iter()
                    .rev()
                    .find(|r| !r.context.is_empty())
                    .map(|r| r.context.clone()),
                state.state=="connected",
            )
        };
        if !account.enabled || account.token.is_empty() || account.reply_context_pending {
            return;
        }
        let Some(context) = context else {
            return;
        };
        let key = account.logical_key();
        let is_current = |state: &State| {
            self.generation.load(Ordering::SeqCst) == generation
                && state.state == "connected"
                && state.stored.enabled
                && !state.stored.reauth_required
                && !state.stored.reply_context_pending
                && state.stored.owner == account.owner
                && state.stored.token == account.token
        };
        let Ok(value) = agent(json!({"operation":"notifications","source_key":key,"connected":connected})).await else {
            return;
        };
        for notification in value.as_array().into_iter().flatten() {
            if notification["kind"] == "agent_task" {
                let Some(turn)=notification["turn_id"].as_str() else {continue};
                let rid=format!("hb-proactive-{turn}");
                let mut state=self.state.lock().await;
                if !is_current(&state) || state.stored.records.iter().any(|r|r.id==rid) {continue;}
                if state.stored.records.len()>=4096 {continue;}
                let mut next=state.stored.clone();
                next.records.push(Record {id:rid.clone(),command:"assistant".into(),status:"running".into(),job_id:Some(turn.into()),
                    context:context.clone(),client_id:rid,created_at:chrono::Utc::now().to_rfc3339(),..Default::default()});
                // Save the watch before attempting any send; reconnect/replay never creates a second watch.
                if (self.save)(next.clone()).await.is_ok() {state.stored=next;}
                continue;
            }
            let Some(id) = notification["id"].as_str() else {
                continue;
            };
            let Some(client) = notification["client_id"].as_str() else {
                continue;
            };
            let text = if matches!(notification["kind"].as_str(),Some("followup_notice"|"matter_notice")) {notification["text"].as_str().unwrap_or("这件事需要你核对一下。").to_string()}
                else {format!("到时间了：{}", notification["text"].as_str().unwrap_or("请查看你的提醒"))};
            let current = {
                let state = self.state.lock().await;
                is_current(&state)
            };
            let delivery = if !current {
                // No request was made. The original occurrence can be claimed again.
                "not_sent"
            } else if self
                .api
                .text(&account, &context, client, &text)
                .await
                .is_ok()
            {
                "accepted"
            } else {
                "unknown"
            };
            // Persist unknown on failure; never silently resend an ambiguous external action.
            let _=agent(json!({"operation":"notification_ack","source_key":key,"reminder_id":id,"client_id":client,"delivery":delivery,"kind":notification["kind"]})).await;
        }
    }
    async fn monitor(self: Arc<Self>, generation: u64) {
        let mut reminder_check = tokio::time::Instant::now() - Duration::from_secs(30);
        while self.generation.load(Ordering::SeqCst) == generation {
            if reminder_check.elapsed() >= Duration::from_secs(15) {
                self.deliver_reminders_for(generation).await;
                reminder_check = tokio::time::Instant::now();
            }
            let records = self
                .state
                .lock()
                .await
                .stored
                .records
                .iter()
                .filter(|r| {
                    r.pending_query().is_some() || r.pending_assistant_followup().is_some() || r.pending_voice_task().is_some() || r.pending_completion()
                        || !matches!(r.command.as_str(), "message" | "wechat_pdf")
                        && [
                            "received",
                            "running",
                            "awaiting_confirmation",
                            "waiting_input_queue",
                        ]
                        .contains(&r.status.as_str())
                })
                .cloned()
                .collect::<Vec<_>>();
            for record in records {
                if self.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if record.pending_share_notice() {
                    let account = self.state.lock().await.stored.account();
                    let _ = self.reply_files_with_claim(&record.id, UNREADABLE_SHARE_REPLY, &[], Some((&account, generation)), true, None, None).await;
                    continue;
                }
                if record.pending_pdf_completion() {
                    let _ = self.send_pdf_completion(&record, generation).await;
                    continue;
                }
                if record.local_media_kind().is_some() {
                    let _ = self.update_media(&record, generation).await;
                    continue;
                }
                if record.status == "received" {
                    if record.pending_query().is_some() {
                        let _ = self.resume_saved_query(&record.id, generation).await;
                    } else {
                        let _ = self.submit(&record.id).await;
                    }
                    continue;
                }
                let Some(job) = &record.job_id else {
                    continue;
                };
                if record.command == "assistant" {
                    if record.query_retry_at>chrono::Utc::now().timestamp(){continue;}
                    let canceled = self.cancel.notified();
                    tokio::pin!(canceled);
                    canceled.as_mut().enable();
                    let account = {
                        let state = self.state.lock().await;
                        let account = state.stored.account();
                        if !self.inbound_account_matches(&state, &account, generation) {
                            return;
                        }
                        if !state.stored.records.iter().any(|r| r.same_assistant_watch(&record)) {
                            continue;
                        }
                        account
                    };
                    // Waiting for a status read must leave inbound control and edits available.
                    let observation = if record.pending_assistant_completion() {
                        None
                    } else {
                        Some(tokio::select! {
                            biased;
                            _ = &mut canceled => return,
                            result = self.agent_get_from(job, false, account.logical_key()) => result,
                        })
                    };
                    let _delivery_guard = tokio::select! {
                        biased;
                        _ = &mut canceled => return,
                        guard = self.submit_lock.lock() => guard,
                    };
                    let record = {
                        let state = self.state.lock().await;
                        if !self.inbound_account_matches(&state, &account, generation) {
                            return;
                        }
                        // An inbound request can transfer the watch to another record during the read.
                        let Some(current) = state.stored.records.iter()
                            .find(|r| r.same_assistant_watch(&record)) else { continue; };
                        current.clone()
                    };
                    let scope = Some((&account, generation));
                    let can_deliver = !record.reply_context_retired();
                    let Some(observation) = observation else {
                        if record.pending_assistant_completion() {
                            let value = json!({"status": record.status, "result": record.result});
                            let _ = self.reply_assistant_for(&record.id, &value, true, scope, true).await;
                        }
                        continue;
                    };
                    match observation {
                        Ok(value) => {
                            if record.query_failures > 0 {
                                let _ = self.change_for(&record.id, scope, |r| {
                                    r.clear_query_retry();
                                }).await;
                            }
                            let Some(status) = value["status"].as_str() else { continue; };
                            if status == "waiting_input_queue" {
                                if record.delivery=="pending" {
                                    if matches!(self.change_for(&record.id, scope, |r| {
                                        r.status = "waiting_input_queue".into();
                                        if can_deliver {
                                            r.delivery = "unknown".into();
                                            r.client_id = format!("{}-queued", r.client_id);
                                        } else {
                                            r.mark_old_reply_unsent();
                                        }
                                    }).await, Ok(true)) && can_deliver {
                                        let _ = self.reply_assistant_for(&record.id, &value, false, scope, false).await;
                                    }
                                }
                                continue;
                            }
                            if matches!(status, "queued" | "running" | "waiting_jobs") {
                                if record.status == "waiting_input_queue" {
                                    let _ = self
                                        .change_for(&record.id, scope, |r| r.status = "running".into())
                                        .await;
                                }
                                let _ = self.emit_progress(&record.id, &value, scope).await;
                                continue;
                            }
                            let confirmation_signature=value["result"]["progress_signature"].as_str().filter(|s|!s.is_empty()).map(str::to_string)
                                .unwrap_or_else(||format!("{:x}",Sha256::digest(value["result"]["text"].as_str().unwrap_or("").as_bytes())));
                            if status == "awaiting_confirmation" && record.status == status && record.confirmation_signature==confirmation_signature {
                                continue;
                            }
                            let completion = status != "awaiting_confirmation";
                            if matches!(self
                                .change_for(&record.id, scope, |r| {
                                    r.status = status.into();
                                    r.result = Some(value["result"].clone());
                                    if can_deliver {
                                        r.client_id = format!("{}-update", r.client_id);
                                        r.delivery = if completion { "pending" } else { "unknown" }.into();
                                        if completion {
                                            r.delivery_error = None;
                                        }
                                    } else {
                                        r.mark_old_reply_unsent();
                                    }
                                    if !completion {
                                        r.confirmation_signature = confirmation_signature.clone();
                                    }
                                })
                                .await, Ok(true)) && can_deliver
                            {
                                let _ = self
                                    .reply_assistant_for(
                                        &record.id,
                                        &value,
                                        completion,
                                        scope,
                                        completion,
                                    )
                                    .await;
                            }
                        }
                        Err(_) => {
                            let failures=record.query_failures.saturating_add(1);
                            let exhausted=failures>=8;
                            let recorded = self.change_for(&record.id, scope, |r|{
                                r.query_failures=failures;
                                r.query_retry_at=query_backoff_deadline(failures);
                                if exhausted {
                                    r.status = "unknown".into();
                                    if !can_deliver {
                                        r.mark_old_reply_unsent();
                                    }
                                }
                            }).await;
                            if can_deliver && exhausted && matches!(recorded, Ok(true)) {
                                let _ = self.reply_files_for(&record.id, "暂时查不到这件事的最新进展，结果还不能确定。我保留了原任务，没有重新执行；稍后说“进度”可以继续核对。", &[], scope).await;
                            }
                        }
                    }
                    continue;
                }
                let canceled = self.cancel.notified();
                tokio::pin!(canceled);
                canceled.as_mut().enable();
                let account = {
                    let state = self.state.lock().await;
                    let account = state.stored.account();
                    if !self.inbound_account_matches(&state, &account, generation) {
                        return;
                    }
                    account
                };
                let scope = Some((&account, generation));
                if let Some(text) = record.pending_control_completion() {
                    let _ = self.reply_files_with_claim(&record.id, text, &[], scope, true, None, None).await;
                    continue;
                }
                if record.query_retry_at > chrono::Utc::now().timestamp() {
                    continue;
                }
                let value = tokio::select! {
                    biased;
                    _ = &mut canceled => return,
                    result = self.request("jobs.get", None, json!({}), &record.id, Some(job)) => result,
                };
                let observed = value.as_ref().ok().and_then(|value| {
                    control_job_status(value).map(|status| (status, value))
                });
                let (status, payload, problem, failures) = match observed {
                    Some(("running" | "queued", _)) => {
                        if record.query_failures != 0 {
                            let _ = self.change_for(&record.id, scope, Record::clear_query_retry).await;
                        }
                        continue;
                    }
                    Some((status, value)) => {
                        let payload = value.get("result").filter(|v| !v.is_null()).cloned();
                        let problem = value["error"]["message"].as_str().map(str::to_owned);
                        (status, payload, problem, 0)
                    }
                    None => {
                        let failures = record.query_failures.saturating_add(1).min(MAX_CONTROL_LOOKUP_FAILURES);
                        if failures < MAX_CONTROL_LOOKUP_FAILURES {
                            let _ = self.change_for(&record.id, scope, |r| {
                                r.query_failures = failures;
                                r.query_retry_at = query_backoff_deadline(failures);
                            }).await;
                            continue;
                        }
                        let detail = "发 /状态 或 /结果 加上原编号，可以继续核对。";
                        ("unknown", None, Some(detail.into()), failures)
                    }
                };
                let retired = record.reply_context_retired();
                let notice = (!retired).then(|| {
                    control_reply(&record, status, payload.as_ref(), problem.as_deref(), ControlReply::Update)
                });
                match self.change_for(&record.id, scope, |r| {
                    r.status = status.into();
                    r.query_failures = failures;
                    r.query_retry_at = 0;
                    if payload.is_some() {
                        r.result = payload.clone();
                    }
                    if let Some(text) = &notice {
                        if r.args.is_null() {
                            r.args = json!({});
                        }
                        if let Some(args) = r.args.as_object_mut() {
                            args.insert(COMPLETION_REPLY.into(), json!(text));
                            r.client_id = format!("{}-done", r.client_id);
                            r.delivery = "pending".into();
                            r.delivery_error = None;
                        } else {
                            r.delivery = "not_sent".into();
                            r.delivery_error = Some("原通知无法安全保存。请用原任务编号查询。".into());
                        }
                    } else {
                        r.mark_old_reply_unsent();
                    }
                }).await {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(_) => {
                        self.connection_for("error", "微信任务结果保存失败，已暂停回复", scope).await;
                        return;
                    }
                }
                if let Some(text) = &notice {
                    // Attachments are returned by an explicit result query.
                    let _ = self.reply_files_with_claim(&record.id, text, &[], scope, true, None, None).await;
                }
            }
            let canceled = self.cancel.notified();
            tokio::pin!(canceled);
            canceled.as_mut().enable();
            if self.generation.load(Ordering::SeqCst) != generation {
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => (),
                _ = &mut canceled => return,
            }
        }
    }
}
#[cfg(test)]
fn artifact_path(value: &Value) -> Option<PathBuf> {
    value["artifact"]["path"].as_str().map(PathBuf::from)
}
fn control_task_name(command: &str) -> &str {
    match command {
        "system_info" => "系统信息",
        "system_gpu" => "显卡状态",
        "model_capabilities_catalog" => "模型目录",
        "model_capabilities_refresh" => "更新模型目录",
        "model_capability_resolve" => "检查模型调用条件",
        "model_capability_call" => "模型调用",
        "model_capability_task" => "查询模型任务",
        capability_browse::COMMAND => "能力库",
        "inbox_submit" => "微信资料收录",
        "repo_project_analyze" => "仓库分析",
        repository_search::COMMAND => "GitHub 仓库搜索",
        "capabilities_catalog" => "本机视频能力",
        "capability_install" => "安装视频工具",
        "capability_render" => "视频生成",
        _ => "任务",
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControlReply {
    Status,
    Result,
    Update,
}

fn control_reply(
    record: &Record, status: &str, result: Option<&Value>, problem: Option<&str>, kind: ControlReply,
) -> String {
    let business = result.and_then(|value| result_text(value, "status"));
    let attachment = result.is_some_and(|value| {
        result_text(&value["artifact"], "path").is_some()
            && result_text(&value["artifact"], "sha256").is_some()
    });
    let heading = match (status, business) {
        ("received", _) => "收到，正在等待受理。",
        ("queued", _) => "任务正在排队。",
        ("running", _) => "任务还在处理中。",
        ("failed", _) | ("succeeded", Some("failed")) if record.command == "capability_render" => "视频未能生成。",
        ("failed", _) | ("succeeded", Some("failed")) => "这次任务没有完成。",
        ("canceled", _) | ("succeeded", Some("canceled")) => "任务已取消。",
        ("timed_out", _) | ("succeeded", Some("timed_out")) => "任务等待超时。",
        ("interrupted_unknown" | "uncertain" | "unknown", _) | ("succeeded", Some("unknown" | "interrupted_unknown" | "uncertain")) => {
            "原任务的结果待核对。"
        }
        ("succeeded", Some("queued")) => "服务商正在排队。",
        ("succeeded", Some("running")) => "服务商还在处理。",
        ("succeeded", Some(other)) if other != "succeeded" => "暂时无法确认业务结果。",
        ("succeeded", _) if result.is_none() => "查到了完成记录，原结果暂时无法取回。",
        ("succeeded", _) if record.command == "capability_render" && attachment => "视频已生成。",
        ("succeeded", _) if record.command == "capability_render" => "视频任务已结束，暂时没有可取回的文件。",
        ("succeeded", _) => "已处理完毕。",
        _ => "暂时无法确认原任务状态。",
    };
    let mut text = format!("{}\n{heading}", control_task_name(&record.command));
    let result_error = result.and_then(|value| result_text(value, "error"));
    if let Some(error) = problem.or(result_error).filter(|_| record.command != capability_browse::COMMAND) {
        text.push('\n');
        text.push_str(&clip(error, 360));
    }
    let result_ready = status == "succeeded" && matches!(business, None | Some("succeeded"));
    if record.command == repository_search::COMMAND && status == "succeeded" && kind != ControlReply::Status {
        if let Some(value) = result {
            text.push('\n');
            text.push_str(&repository_search::summary(value));
        }
    } else if record.command == "repo_project_analyze" && status == "succeeded" && kind != ControlReply::Status {
        if let Some(value) = result {
            text.push('\n');
            text.push_str(&repository_analysis_summary(record, value));
        }
    } else if result_ready && kind != ControlReply::Status {
        if let Some(value) = result {
            let detail = summary(&record.command, value);
            if !detail.is_empty() {
                text.push('\n');
                text.push_str(&detail);
            }
        }
    }
    text.push_str(&format!("\n任务编号：{}", record.id));
    if record.command == "model_capability_call"
        && status == "succeeded"
        && matches!(business, Some("queued" | "running"))
        && result.is_some_and(|value| result_text(value, "task_id").is_some())
    {
        text.push_str(&format!("\n发 /模型状态 {} 可继续查询。", record.id));
    } else if result_ready && result.is_some()
        && (record.command != "capability_render" || attachment)
        && (kind == ControlReply::Status || kind == ControlReply::Update && attachment)
    {
        text.push_str(&format!("\n发 /结果 {} 可取回结果。", record.id));
    }
    text
}

fn repository_analysis_summary(record: &Record, value: &Value) -> String {
    let repository = result_text(&record.args["request"], "repository_url").unwrap_or("原仓库");
    let project = &value["project"];
    let outcome = match project["phase"].as_str() {
        Some("analyzed") => "现有驱动支持。可以引用这条回复说「安装这个项目」，或在能力库继续。",
        Some("registered") => "该仓库已有登记能力，可在能力库查看。",
        Some("needs_adapter") => video::REPOSITORY_ADAPTER_REQUIRED,
        Some("failed") => "仓库分析未完成，可在能力库查看原项目。",
        Some("canceled") => "仓库分析已取消。",
        _ => "原项目结果待核对，可在能力库查看。",
    };
    let mut detail = format!("{}\n{outcome}", clip(repository, 240));
    if project["phase"] == "analyzed" {
        detail.push_str(&format!("\n也可发送以下命令\n/安装项目 {}", record.id));
    }
    if let Some(id) = result_text(project, "id").filter(|id| id.starts_with("project_") && id.len() <= 160 && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))) {
        detail.push_str(&format!("\n项目编号：{id}"));
    }
    detail
}

fn result_text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value[field].as_str().map(str::trim).filter(|text| !text.is_empty())
}

fn system_result_summary(command: &str, value: &Value) -> String {
    let mut lines = Vec::new();
    if command == "system_info" {
        for (field, label) in [("os", "系统"), ("cpu_brand", "处理器")] {
            if let Some(text) = result_text(value, field) {
                lines.push(format!("{label}：{}", clip(text, 140)));
            }
        }
        if let Some(memory) = value["memory_total_gb"].as_f64().filter(|amount| *amount > 0.0) {
            lines.push(format!("内存：{memory:.1} GiB"));
        }
    }
    match (result_text(value, "gpu_status"), result_text(value, "gpu_name")) {
        (Some("detected"), Some(name)) => {
            lines.push(format!("显卡：{}", clip(name, 140)));
            if command == "system_gpu" {
                if let Some(percent) = value["gpu_percent"].as_f64().filter(|percent| (0.0..=100.0).contains(percent)) {
                    lines.push(format!("显卡使用率：{percent:.1}%"));
                } else {
                    lines.push("暂时无法核对显卡使用率。".into());
                }
            }
        }
        (Some("not_detected"), _) => lines.push("未识别到物理显卡。".into()),
        _ => lines.push("暂时无法核对显卡信息。".into()),
    }
    if value["issues"].as_array().is_some_and(|issues| !issues.is_empty()) {
        lines.push("部分系统信息暂时无法核对。".into());
    }
    lines.join("\n")
}

fn summary(command: &str, value: &Value) -> String {
    if command == capability_browse::COMMAND {
        return capability_browse::summary(value);
    }
    if command == "model_capability_call" && value["artifact"]["mime_type"] == "image/png" {
        return "图片已校验并保存为 PNG，可按原任务取回。".into();
    }
    if matches!(command, "system_info" | "system_gpu") {
        return system_result_summary(command, value);
    }
    if command == "inbox_submit" {
        return result_text(value, "filename")
            .filter(|name| Path::new(name).file_name().and_then(|leaf| leaf.to_str()) == Some(*name))
            .map(|name| format!("分享内容已保存到资料库。收录文件：{}", clip(name, 140)))
            .unwrap_or_else(|| "资料收录任务结束，暂时无法核对原文件名。".into());
    }
    if command == "capability_render" {
        if let Some(seconds) = value["verification"]["seconds"].as_f64().filter(|seconds| *seconds > 0.0) {
            return format!("时长约 {seconds} 秒。");
        }
        return result_text(value, "text").map(|text| clip(text, 600)).unwrap_or_default();
    }
    if command == "capability_install" {
        return result_text(value, "version")
            .map(|version| format!("本机视频工具版本 {}。发「视频 标题」可开始生成。", clip(version, 80)))
            .unwrap_or_default();
    }
    if command == "model_capabilities_catalog" {
        let mut text = format!("模型/渠道总记录：{}\n功能：", value["total_models"]);
        if let Some(functions) = value["functions"].as_array() {
            text.push_str(
                &functions
                    .iter()
                    .filter_map(|f| f["name"].as_str())
                    .collect::<Vec<_>>()
                    .join("、"),
            );
        }
        text.push_str("\n本页模型：\n");
        if let Some(models) = value["models"].as_array() {
            for model in models.iter().take(12) {
                text.push_str(&format!(
                    "{}\n  {}\n",
                    model["name"].as_str().unwrap_or("未知"),
                    model["route_id"].as_str().unwrap_or("未知")
                ));
            }
        }
        text.push_str(&format!(
            "下一页 offset：{}\n目录不代表生成权限已验证。",
            value["next_offset"]
        ));
        return clip(&text, 2800);
    }
    clip(
        &serde_json::to_string_pretty(value).unwrap_or_else(|_| "结果编码失败".into()),
        2800,
    )
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Help,
    UnreadableShare,
    Reply(String),
    Call(String, Value),
    AssistantFollowup { turn: String, file: Option<file_delivery::FileSelection>, text: String },
    VoiceTask(voice::VoiceTaskIntent),
    Query(String, bool, bool),
    CurrentQuery { files: bool, target: Option<String> },
    FileQuery { turn: String, file: file_delivery::FileSelection },
    FileRequest { turn: String, file: file_delivery::FileSelection, resend: bool },
}
fn valid_task_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn parse_capture(text: &str) -> Option<Result<Action, String>> {
    let text = text.trim();
    let rest = ["/收录", "帮我收录", "请收录", "收录"]
        .into_iter()
        .find_map(|prefix| {
            text.strip_prefix(prefix).filter(|rest| {
                rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || matches!(c, ':' | '：'))
            })
        })?;
    let rest = rest.trim_start();
    let content = rest.strip_prefix([':', '：']).unwrap_or(rest).trim_start();
    if content.is_empty() {
        return Some(Err("请在「帮我收录：」后粘贴分享文案或链接。".into()));
    }
    if text.len() > 16_000 {
        return Some(Err("分享消息超过 16 KB，未收录。".into()));
    }
    Some(Ok(Action::Call(
        "inbox_submit".into(),
        json!({"payload":{"text":content,"title":"微信分享","source_hint":"wechat-share","extra_tags":["wechat"]}}),
    )))
}

pub fn parse(text: &str, root: &Path) -> Result<Action, String> {
    let text = text.trim();
    if text.len() > 16000 {
        return Err("消息超过 16 KB，未执行".into());
    }
    if ["帮助", "help", "/帮助", "/help"].contains(&text) {
        return Ok(Action::Help);
    }
    if ["系统", "/系统"].contains(&text) {
        return Ok(Action::Call("system_info".into(), json!({})));
    }
    if ["模型目录", "/模型目录"].contains(&text) {
        return Ok(Action::Call(
            "model_capabilities_catalog".into(),
            json!({"offset":0,"limit":12}),
        ));
    }
    if ["安装视频", "/安装视频"].contains(&text) {
        return Ok(Action::Call(
            "capability_install".into(),
            json!({"rootPath":root.join("hyperframes")}),
        ));
    }
    if let Some(title) = text
        .strip_prefix("视频 ")
        .or_else(|| text.strip_prefix("/视频 "))
    {
        return Ok(Action::Call(
            "capability_render".into(),
            json!({"rootPath":root.join("hyperframes"),"input":{"title":title.trim(),"seconds":6}}),
        ));
    }
    for (prefix, result, remote) in [
        ("/状态 ", false, false),
        ("状态 ", false, false),
        ("/结果 ", true, false),
        ("结果 ", true, false),
        ("/模型状态 ", true, true),
    ] {
        if let Some(id) = text.strip_prefix(prefix) {
            let id = id.trim();
            if !valid_task_id(id) {
                return Err("任务编号无效".into());
            }
            return Ok(Action::Query(id.into(), result, remote));
        }
    }
    if let Some(rest) = text.strip_prefix("/调用 ") {
        let (command, args) = rest
            .split_once(char::is_whitespace)
            .ok_or("格式：/调用 命令名 JSON参数")?;
        if ![
            "system_info",
            "system_gpu",
            "model_capabilities_catalog",
            "model_capabilities_refresh",
            "model_capability_resolve",
            "model_capability_call",
            "capabilities_catalog",
            "capability_install",
            "capability_render",
        ]
        .contains(&command)
        {
            return Err("该接口未开放给微信；不执行任意命令、秘密读取或已有任务写入".into());
        }
        let mut value: Value =
            serde_json::from_str(args.trim()).map_err(|_| "调用参数必须是有效 JSON")?;
        let object = value.as_object_mut().ok_or("调用参数必须为 JSON 对象")?;
        if command.starts_with("capabilit") {
            if object.contains_key("root_path") || object.contains_key("rootPath") {
                return Err("微信能力目录由系统隔离管理，不接受外部路径".into());
            }
            object.insert("rootPath".into(), json!(root.join("hyperframes")));
        }
        if command == "model_capability_call" || command == "model_capability_resolve" {
            let request = object
                .get_mut("request")
                .and_then(Value::as_object_mut)
                .ok_or("缺少 request 对象")?;
            if request.contains_key("output_root") {
                return Err("微信模型输出目录由系统管理".into());
            }
            request.insert("output_root".into(), json!(root.join("models")));
        }
        return Ok(Action::Call(command.into(), value));
    }
    Err("未识别该指令，未执行任何任务".into())
}

#[cfg(test)]
mod tests {
    mod voice_tests { include!("wechat_voice_tests.rs"); }
    mod reauthorization_tests { include!("wechat_reauthorization_tests.rs"); }
    mod capability_handoff_tests { include!("wechat_capability_handoff_tests.rs"); }
    mod capability_browse_tests { include!("wechat_capability_browse_tests.rs"); }
    mod image_delivery_tests { include!("wechat_image_delivery_tests.rs"); }
    mod clip_delivery_tests { include!("wechat_clip_delivery_tests.rs"); }
    mod exact_resend_tests { include!("wechat_exact_resend_tests.rs"); }
    mod workflow_context_tests { include!("wechat_workflow_context_tests.rs"); }
    mod workflow_tests { include!("wechat_workflow_tests.rs"); }
    mod repository_tests { include!("wechat_repository_tests.rs"); }
    use super::*;
    mod personal_followup_tests { include!("wechat_personal_followup_tests.rs"); }
    mod personal_dialogue_tests { include!("wechat_personal_dialogue_tests.rs"); }
    mod adaptation_tests { include!("wechat_adaptation_tests.rs"); }
    mod live_adaptation_tests { include!("wechat_live_adaptation_tests.rs"); }
    use crate::control_state::{Controller, Runner};
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn wechat_natural_messages_use_shared_disabled_assistant_and_keep_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count.clone());
        let control: Control = Arc::new(move |r| {
            let c = ctl.clone();
            Box::pin(async move { c.handle(r).await })
        });
        let models = Arc::new(AtomicUsize::new(0));
        let calls = models.clone();
        let core = crate::assistant::Engine::new(
            crate::assistant::State::default(),
            Arc::new(|_| Box::pin(async { Ok(()) })),
            Arc::new(move |_| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err("must never call a model".into()) })
            }),
            control.clone(),
            tmp.path().join("workspace"),
        )
        .await
        .unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let bridge = Bridge::new_with_agent(
            account(&base),
            api,
            Arc::new(|_| Box::pin(async { Ok(()) })),
            control,
            tmp.path().join("workspace"),
            Some(Arc::new(move |r| {
                let core = core.clone();
                Box::pin(async move { core.channel(r).await })
            })),
        );
        bridge
            .ingest(&msg("natural-1", "你好，帮我看看系统"))
            .await
            .unwrap();
        bridge
            .ingest(&msg("natural-1", "你好，帮我看看系统"))
            .await
            .unwrap();
        assert_eq!(bridge.status().await.tasks.len(), 1);
        assert_eq!(bridge.status().await.tasks[0].status, "disabled");
        let probe_text = "HB-SELFTEST-fixture 你好";
        let probe = bridge.self_test_message_inner(probe_text, "hb-selftest-fixture-1").await.unwrap();
        assert_eq!(probe["simulated_input"], true);
        assert_eq!(bridge.status().await.tasks.len(), 2);
        let sends = seen.lock().await.len();
        let repeated = bridge.self_test_message_inner(probe_text, "hb-selftest-fixture-1").await.unwrap();
        assert_eq!(repeated["duplicate"], true);
        assert_eq!(seen.lock().await.len(), sends);
        assert!(bridge.self_test_message_inner("HB-SELFTEST-fixture different", "hb-selftest-fixture-1").await.is_err());
        assert!(bridge.self_test_message_inner("确认 HB-SELFTEST-fixture", "hb-selftest-fixture-2").await.is_err());
        assert!(bridge.self_test_message_inner("没有测试标记", "hb-selftest-fixture-2").await.is_err());
        assert!(bridge.self_test_message_inner(probe_text, "arbitrary-id").await.is_err());
        assert_eq!(models.load(Ordering::SeqCst), 0);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        assert!(seen
            .lock()
            .await
            .iter()
            .any(|v| v["body"]["msg"]["item_list"][0]["text_item"]["text"]
                .as_str()
                .is_some_and(|s| s.contains("尚未启用"))));
        bridge.ingest(&msg("explicit-1", "系统")).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        bridge.ingest(&msg("short-new-1", "新对话")).await.unwrap();
        bridge.ingest(&msg("short-new-1", "新对话")).await.unwrap();
        assert_eq!(models.load(Ordering::SeqCst), 0);
        assert!(seen
            .lock()
            .await
            .iter()
            .any(|v| v["body"]["msg"]["item_list"][0]["text_item"]["text"]
                .as_str()
                .is_some_and(|s| s.contains("已开启新对话"))));
        server.abort();
    }

    #[tokio::test]
    async fn wechat_delivery_failure_keeps_safe_reason_and_does_not_retry() {
        let root=tempfile::tempdir().unwrap();
        let(base,seen,server)=fake_api(200,json!({"ret":-2,"errmsg":"synthetic-private-context"})).await;
        let mut stored=account(&base);
        stored.records.push(Record{id:"delivery-error".into(),command:"message".into(),status:"received".into(),context:"synthetic-context".into(),client_id:"fixture-send".into(),..Default::default()});
        let count=Arc::new(AtomicUsize::new(0));
        let(bridge,saved)=test_bridge(stored,root.path(),controller(root.path(),count));
        bridge.reply("delivery-error","synthetic reply",None).await.unwrap();
        let task=bridge.status().await.tasks.remove(0);
        assert_eq!(task.delivery,"unknown");
        let reason=task.delivery_error.unwrap();assert!(reason.contains("=-2"));assert!(!reason.contains("synthetic-private-context"));
        assert_eq!(saved.lock().await.records[0].delivery_error.as_deref(),Some(reason.as_str()));
        assert_eq!(seen.lock().await.len(),1);
        server.abort();
    }
    async fn fake_api(
        code: u16,
        body: Value,
    ) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let logged = seen.clone();
        let dropped_file_ack = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sent_prefix_ack = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let seen = logged.clone();
                let body = body.clone();
                let dropped_file_ack = dropped_file_ack.clone();
                let sent_prefix_ack = sent_prefix_ack.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buf = [0u8; 4096];
                    let split = loop {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(i) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                            break i + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&bytes[..split]).to_string();
                    let size = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    while bytes.len() < split + size {
                        let n = socket.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..n]);
                    }
                    let payload = serde_json::from_slice::<Value>(&bytes[split..split + size])
                        .unwrap_or(Value::Null);
                    let first = payload["get_updates_buf"] == "";
                    let send_id = payload["msg"]["client_id"].as_str()
                        .and_then(|client| body["__send_message_ids"].get(client))
                        .or_else(|| {
                            let text = payload["msg"]["item_list"][0]["text_item"]["text"].as_str()?;
                            let matched = body["__send_text_prefix_ids"].as_object()?.iter()
                                .find_map(|(prefix, id)| text.starts_with(prefix.as_str()).then_some(id))
                                ?;
                            (body["__send_text_prefix_once"] != true || !sent_prefix_ack.swap(true, Ordering::SeqCst)).then_some(matched)
                        });
                    let drop_ack = body["__drop_first_file_ack"] == true
                        && head.contains("/sendmessage ")
                        && payload["msg"]["item_list"][0]["type"] == 4
                        && !dropped_file_ack.swap(true, Ordering::SeqCst);
                    seen.lock().await.push(json!({"head":head,"body":payload,"raw_base64":STANDARD.encode(&bytes[split..split+size])}));
                    if drop_ack {
                        return;
                    }
                    let pdf_cipher = body["__pdf_cipher"].as_str().map(std::borrow::ToOwned::to_owned);
                    let response = if let Some(id) = send_id.filter(|_| head.contains("/sendmessage ")) {
                        json!({"message_id":id})
                    } else if head.contains("/getupdates ") {
                        if let Some(sparse) = body.get("__updates_response") {
                            sparse.clone()
                        } else {
                            json!({"ret":0,"msgs":if first {body.get("__incoming").cloned().unwrap_or(json!([]))}else{json!([])},"get_updates_buf":"synthetic-cursor"})
                        }
                    } else if body["__media"] == true && head.contains("/getuploadurl ") {
                        json!({"upload_full_url":"https://novac2c.cdn.weixin.qq.com/c2c/upload?synthetic=1"})
                    } else if body["__pairing"] == true {
                        if head.contains("/get_bot_qrcode?") {
                            json!({"qrcode":"synthetic-qr","qrcode_img_content":"https://example.test/synthetic-pairing"})
                        } else if head.contains("/get_qrcode_status?") {
                            if head.contains("verify_code=123456") {
                                body.get("__pairing_confirmation").cloned().unwrap_or_else(||json!({"status":"confirmed","bot_token":"synthetic-token","ilink_bot_id":"synthetic-bot","ilink_user_id":"synthetic-owner","baseurl":ORIGIN}))
                            } else {
                                json!({"status":"need_verifycode"})
                            }
                        } else {
                            json!({"ret":0})
                        }
                    } else if body["__media"] == true {
                        json!({})
                    } else {
                        body
                    };
                    let encoded = if head.starts_with("GET /c2c/download?") {
                        pdf_cipher.as_deref().map(|cipher| STANDARD.decode(cipher).unwrap())
                            .unwrap_or_else(|| serde_json::to_vec(&response).unwrap())
                    } else {
                        serde_json::to_vec(&response).unwrap()
                    };
                    let header=format!("HTTP/1.1 {code} Test\r\nContent-Type: application/json\r\nx-encrypted-param: synthetic-download\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",encoded.len());
                    socket.write_all(header.as_bytes()).await.unwrap();
                    socket.write_all(&encoded).await.unwrap();
                });
            }
        });
        (base, seen, worker)
    }
    fn msg(id: &str, text: &str) -> Value {
        json!({"message_id":id,"from_user_id":"synthetic-owner","to_user_id":"synthetic-bot","message_type":1,"message_state":0,"context_token":"synthetic-context","item_list":[{"type":1,"text_item":{"text":text}}]})
    }
    #[tokio::test]
    async fn wechat_pdf_downloads_once_and_recovers_cached_source_after_lost_claim() {
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
        let temp = tempfile::tempdir().unwrap();
        let key = [7u8; 16];
        let document = b"%PDF-1.7\nsynthetic";
        let cipher = ecb::Encryptor::<aes::Aes128>::new((&key).into()).encrypt_padded_vec_mut::<Pkcs7>(document);
        let (base, seen, server) = fake_api(200, json!({"__pdf_cipher":STANDARD.encode(cipher)})).await;
        let stored = account(&base);
        let control: Control = Arc::new(|_| Box::pin(async { panic!("PDF must not dispatch a control task") }));
        let received = Arc::new(Mutex::new(None::<(String, String)>));
        let process = received.clone();
        let inbox_calls = Arc::new(AtomicUsize::new(0));
        let inbox_count = inbox_calls.clone();
        let inbox: PdfInbox = Arc::new(move |request| {
            let process = process.clone();
            let inbox_count = inbox_count.clone();
            Box::pin(async move {
                inbox_count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(request.name, "合成.pdf");
                let mut saved = process.lock().await;
                if let Some(bytes) = request.bytes {
                    assert!(!request.read_only);
                    assert_eq!(&**bytes, document);
                    assert!(saved.is_none());
                    *saved = Some((request.id.clone(), request.source.clone()));
                }
                Ok(saved.as_ref().filter(|(doc, owner)| doc == &request.id && owner == &request.source)
                    .map(|_| "本机 PDF 文字".into()))
            })
        });
        let ledger = Arc::new(Mutex::new(stored.clone()));
        let output = ledger.clone();
        let denied = Arc::new(AtomicUsize::new(0));
        let save: Save = Arc::new(move |next| {
            let output = output.clone();
            let denied = denied.clone();
            Box::pin(async move {
                if next.records.first().is_some_and(|r| r.delivery == "unknown") && denied.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("synthetic claim publication failure".into());
                }
                *output.lock().await = next;
                Ok(())
            })
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new_with_pdf(stored, api.clone(), save.clone(), control.clone(), temp.path().into(), None, Some(inbox.clone()));
        let mut attachment = msg("pdf-1", "");
        attachment["item_list"] = json!([{"type":4,"msg_id":"9007199254741011","file_item":{"file_name":"合成.pdf","len":document.len().to_string(),"media":{
            "aes_key":STANDARD.encode(key),"encrypt_query_param":"synthetic+percent%","encrypt_type":1}}}]);
        assert!(bridge.ingest(&attachment).await.is_err());
        bridge.stop().await.unwrap();
        let journal = serde_json::to_vec(&*ledger.lock().await).unwrap();
        let cipher = crate::wechat_crypto::crypt(&journal, true).unwrap();
        let plain = crate::wechat_crypto::crypt(&cipher, false).unwrap();
        let mut saved: Stored = serde_json::from_slice(&plain).unwrap();
        assert_eq!(saved.records[0].status, "completed");
        assert_eq!(saved.records[0].delivery, "pending");
        assert_eq!(saved.records[0].result.as_ref().unwrap()["text"], "本机 PDF 文字");
        assert_eq!(seen.lock().await.len(), 1, "only the encrypted download occurred");
        assert!(!serde_json::to_string(&saved).unwrap().contains("synthetic+percent%"));
        assert!(!serde_json::to_string(&saved).unwrap().contains(&STANDARD.encode(key)));
        let original_id = saved.records[0].id.clone();
        saved.enabled = true;
        let bridge = Bridge::new_with_pdf(saved, api, save, control, temp.path().into(), None, Some(inbox));
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if ledger.lock().await.records[0].delivery == "accepted" {break;}
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        let mut item_only = attachment.clone();
        item_only.as_object_mut().unwrap().remove("message_id");
        bridge.ingest(&item_only).await.unwrap();
        bridge.ingest(&attachment).await.unwrap();
        assert_eq!(inbox_calls.load(Ordering::SeqCst), 2, "restart must not reopen the PDF inbox");
        let id = {
            let saved = ledger.lock().await;
            assert_eq!(saved.records.len(), 1);
            assert_eq!(saved.records[0].id, original_id);
            saved.records[0].id.clone()
        };
        bridge.ingest(&msg("pdf-query", &format!("/结果 {id}"))).await.unwrap();
        assert_eq!(ledger.lock().await.records[0].delivery, "accepted");
        *received.lock().await = None;
        bridge.ingest(&msg("pdf-unavailable", &format!("/结果 {id}"))).await.unwrap();
        let traffic = seen.lock().await;
        assert_eq!(traffic.len(), 4);
        let get = traffic[0]["head"].as_str().unwrap();
        assert!(get.starts_with("GET /c2c/download?encrypted_query_param=synthetic%2Bpercent%25 "));
        assert!(!get.to_ascii_lowercase().contains("authorization"));
        for message in &traffic[1..3] { assert!(message["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap().contains("本机 PDF 文字")); }
        let unavailable = traffic[3]["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap();
        assert!(unavailable.contains("暂无法核对"));
        assert!(!unavailable.contains("本机 PDF 文字"));
        drop(traffic);
        bridge.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn wechat_pdf_read_only_query_repairs_original_receipt_without_redownloading() {
        for initial in ["received", "canceled"] {
            let root = tempfile::tempdir().unwrap();
            let (base, seen, server) = fake_api(200, json!({})).await;
            let mut stored = account(&base);
            let source = stored.logical_key();
            let receipt = format!("wx-{}", "b".repeat(64));
            stored.records.push(Record {
                id: receipt.clone(),
                command: "wechat_pdf".into(),
                status: initial.into(),
                delivery: if initial == "received" { "pending" } else { "not_sent" }.into(),
                args: json!({"source":source,"name":"合成.pdf"}),
                context: "original-pdf-context".into(),
                client_id: "original-pdf-client".into(),
                ..Default::default()
            });
            let control: Control = Arc::new(|_| Box::pin(async { panic!("PDF lookup must stay local") }));
            let reads = Arc::new(AtomicUsize::new(0));
            let inbox: PdfInbox = {
                let (source, reads) = (source.clone(), reads.clone());
                Arc::new(move |request| {
                    let (source, reads) = (source.clone(), reads.clone());
                    Box::pin(async move {
                        assert!(request.read_only && request.bytes.is_none());
                        assert_eq!(request.source, source);
                        assert_eq!(request.name, "合成.pdf");
                        reads.fetch_add(1, Ordering::SeqCst);
                        Ok(Some("本机已核对的原 PDF 文字".into()))
                    })
                })
            };
            let (template, saved) = test_bridge(
                stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
            );
            let mut api = Api::new().unwrap();
            api.test_origin = Some(base);
            let bridge = Bridge::new_with_pdf(stored, api, template.save.clone(), control, root.path().into(), None, Some(inbox));
            bridge.ingest(&msg("lookup-original-pdf", &format!("/结果 {receipt}"))).await.unwrap();
            {
                let snapshot = saved.lock().await;
                let record = &snapshot.records[0];
                assert_eq!(record.id, receipt);
                assert_eq!(record.status, "completed");
                assert_eq!(record.delivery, "not_sent");
                assert_eq!(record.result.as_ref().unwrap()["text"], "本机已核对的原 PDF 文字");
            }
            let account = bridge.state.lock().await.stored.account();
            let question = format!("/问PDF {receipt} 请概括首页");
            let action = bridge.pdf_question(&msg("question-about-original", &question), &account, &question).await.unwrap().unwrap();
            assert_eq!(action, Action::Call("assistant".into(), json!({"text":"请概括首页","pdf_receipt":receipt})));
            let traffic = seen.lock().await;
            assert_eq!(traffic.len(), 1);
            assert!(traffic[0]["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap()
                .contains("本机已核对的原 PDF 文字"));
            assert_eq!(reads.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }

    #[tokio::test]
    async fn wechat_pdf_paused_lookup_resumes_only_in_the_current_connection() {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        let source = stored.logical_key();
        let receipt = format!("wx-{}", "f".repeat(64));
        stored.records.push(Record {
            id: receipt.clone(),
            command: "wechat_pdf".into(),
            status: "received".into(),
            delivery: "pending".into(),
            args: json!({"source":source,"name":"合成.pdf"}),
            ..Default::default()
        });
        let reached = Arc::new(Notify::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let inbox: PdfInbox = {
            let (reached, reads) = (reached.clone(), reads.clone());
            Arc::new(move |request| {
                let (reached, reads) = (reached.clone(), reads.clone());
                Box::pin(async move {
                    assert!(request.read_only && request.bytes.is_none());
                    if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                        reached.notify_one();
                        return std::future::pending().await;
                    }
                    Ok(Some("从当前授权取回原 PDF".into()))
                })
            })
        };
        let control: Control = Arc::new(|_| Box::pin(async { panic!("PDF lookup must stay local") }));
        let (template, saved) = test_bridge(
            stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
        );
        let make_bridge = |stored| {
            let mut api = Api::new().unwrap();
            api.test_origin = Some(base.clone());
            Bridge::new_with_pdf(stored, api, template.save.clone(), control.clone(), root.path().into(), None, Some(inbox.clone()))
        };
        let bridge = make_bridge(stored);
        let request = msg("lookup-after-pause", &format!("/结果 {receipt}"));
        let caller = bridge.clone();
        let query = tokio::spawn(async move { caller.ingest(&request).await });
        tokio::time::timeout(Duration::from_secs(5), reached.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), query).await.unwrap().unwrap().unwrap();
        assert!(seen.lock().await.is_empty());
        let mut snapshot = saved.lock().await.clone();
        assert_eq!(snapshot.records[0].status, "received");
        assert_eq!(snapshot.records[1].pending_query(), Some((QueryTarget::Numbered(&receipt), true)));
        snapshot.enabled = true;
        let reopened = make_bridge(snapshot);
        let monitor = tokio::spawn(reopened.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if saved.lock().await.records[1].delivery == "accepted" {break;}
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        reopened.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        let state = saved.lock().await;
        assert_eq!(state.records[0].status, "completed");
        assert_eq!(state.records[0].delivery, "not_sent");
        assert_eq!(state.records[0].id, receipt);
        assert_eq!(state.records.len(), 2);
        assert_eq!(seen.lock().await.len(), 1);
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_pdf_pause_cancels_pending_fetch_and_discards_stale_poll_generation() {
        let temp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let stored = account(&base);
        let table = Arc::new(Mutex::new(stored.clone()));
        let output = table.clone();
        let save: Save = Arc::new(move |next| { let output = output.clone(); Box::pin(async move { *output.lock().await = next; Ok(()) }) });
        let control: Control = Arc::new(|_| Box::pin(async { panic!("a pending PDF is not a control action") }));
        let loading = Arc::new(Notify::new());
        let open = loading.clone();
        let inbox: PdfInbox = Arc::new(move |request| {
            let open = open.clone();
            Box::pin(async move {
                assert!(request.bytes.is_none());
                open.notify_one();
                std::future::pending().await
            })
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new_with_pdf(stored, api, save, control, temp.path().into(), None, Some(inbox));
        let mut attachment = msg("paused-pdf", "");
        attachment["item_list"] = json!([{"type":4,"file_item":{"file_name":"合成.pdf","media":{
            "aes_key":STANDARD.encode([7u8; 16]),"encrypt_query_param":"synthetic-param"}}}]);
        let receiver = bridge.clone();
        let pending = attachment.clone();
        let work = tokio::spawn(async move { receiver.ingest_for(&pending, 0).await });
        tokio::time::timeout(Duration::from_secs(2), loading.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), work).await.unwrap().unwrap().unwrap();
        assert!(seen.lock().await.is_empty());
        assert_eq!(table.lock().await.records[0].delivery, "pending");
        let original = table.lock().await.records.len();
        bridge.state.lock().await.stored.enabled = true;
        attachment["message_id"] = json!("stale-second-message");
        bridge.ingest_for(&attachment, 0).await.unwrap();
        assert_eq!(table.lock().await.records.len(), original);
        assert!(seen.lock().await.is_empty());
        server.abort();
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "explicit installed PDF parser and synthetic fixture paths required"]
    async fn wechat_pdf_encrypted_wire_to_original_local_result_without_model() {
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
        let runtime = PathBuf::from(std::env::var_os("HOMEBASE_DOCUMENT_RUNTIME").expect("fixed local runtime"));
        let file = PathBuf::from(std::env::var_os("HOMEBASE_DOCUMENT_FIXTURE").expect("synthetic PDF fixture"));
        assert!(runtime.is_absolute() && file.is_absolute());
        let bytes = std::fs::read(file).unwrap();
        let root = tempfile::tempdir().unwrap();
        let control: Control = Arc::new(|_| Box::pin(async { panic!("PDF intake must not invoke a control job") }));
        let engine = crate::assistant::Engine::new(crate::assistant::State::default(), Arc::new(|_| Box::pin(async { Ok(()) })),
            Arc::new(|_| Box::pin(async { panic!("PDF intake must remain local") })), control.clone(), root.path().join("workspace")).await.unwrap();
        engine.configure_document_runtime(runtime).unwrap();
        let documents = engine.clone();
        let inbox: PdfInbox = Arc::new(move |request| { let documents = documents.clone(); Box::pin(async move { documents.channel_pdf(request).await }) });
        let key = [9u8; 16];
        let cipher = ecb::Encryptor::<aes::Aes128>::new((&key).into()).encrypt_padded_vec_mut::<Pkcs7>(&bytes);
        let (base, seen, server) = fake_api(200, json!({"__pdf_cipher":STANDARD.encode(cipher)})).await;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let bridge = Bridge::new_with_pdf(account(&base), api, Arc::new(|_| Box::pin(async { Ok(()) })), control, root.path().into(), None, Some(inbox));
        let mut attachment = msg("integrated-pdf", "");
        attachment["item_list"] = json!([{"type":4,"file_item":{"file_name":"合成.pdf","len":bytes.len().to_string(),
            "md5":format!("{:x}",md5::Md5::digest(&bytes)),"media":{
                "aes_key":STANDARD.encode(key),"encrypt_query_param":"synthetic-processed-pdf","encrypt_type":1}}}]);
        let mut unowned = attachment.clone();
        unowned["from_user_id"] = json!("unbound-user");
        bridge.ingest(&unowned).await.unwrap();
        assert!(seen.lock().await.is_empty());
        bridge.ingest(&attachment).await.unwrap();
        bridge.ingest(&attachment).await.unwrap();
        let tasks = bridge.status().await.tasks;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "completed");
        assert_eq!(tasks[0].delivery, "accepted");
        bridge.ingest(&msg("integrated-pdf-query", &format!("/结果 {}", tasks[0].id))).await.unwrap();
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 3);
        assert!(requests[0]["head"].as_str().unwrap().starts_with("GET /c2c/download?"));
        for reply in &requests[1..] {
            assert!(reply["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap().contains("中文🙂"));
        }
        let sessions = engine.sessions().await;
        assert_eq!(sessions.len(), 1);
        let (documents, unreadable) = engine.document_index(&sessions[0].id).await.unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(unreadable, 0);
        assert!(engine.session(&sessions[0].id).await.unwrap().turns.is_empty());
        server.abort();
    }
    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "explicit installed PDF parser and synthetic fixture paths required"]
    async fn wechat_pdf_question_replays_original_receipt_and_keeps_other_sessions_private() {
        use crate::assistant_protocol::ModelReply;
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};

        let runtime = PathBuf::from(std::env::var_os("HOMEBASE_DOCUMENT_RUNTIME").unwrap());
        let file = PathBuf::from(std::env::var_os("HOMEBASE_DOCUMENT_FIXTURE").unwrap());
        assert!(runtime.is_absolute() && file.is_absolute());
        let bytes = std::fs::read(file).unwrap();
        let root = tempfile::tempdir().unwrap();
        let control: Control = Arc::new(|_| Box::pin(async { panic!("quoted PDF cannot call business tools") }));
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = received.clone();
        let mut initial = crate::assistant::State::default();
        initial.settings.enabled = true;
        let engine = crate::assistant::Engine::new(
            initial, Arc::new(|_| Box::pin(async { Ok(()) })),
            Arc::new(move |request| {
                let captured = captured.clone();
                Box::pin(async move {
                    assert_eq!(request.definitions, Some(vec![]));
                    captured.lock().await.push(request.messages);
                    Ok(ModelReply {
                        text: "第 1 页含中文样例。".into(),
                        calls: vec![],
                        message: json!({"role":"assistant","content":"第 1 页含中文样例。"}),
                        usage: None,
                        diagnostics: None,
                    })
                })
            }), control.clone(), root.path().join("workspace"),
        ).await.unwrap();
        engine.configure_document_runtime(runtime).unwrap();
        let documents = engine.clone();
        let inbox: PdfInbox = Arc::new(move |request| {
            let documents = documents.clone();
            Box::pin(async move { documents.channel_pdf(request).await })
        });
        let agent_engine = engine.clone();
        let agent: Agent = Arc::new(move |request| {
            let agent_engine = agent_engine.clone();
            Box::pin(async move { agent_engine.channel(request).await })
        });
        let key = [8u8; 16];
        let cipher = ecb::Encryptor::<aes::Aes128>::new((&key).into()).encrypt_padded_vec_mut::<Pkcs7>(&bytes);
        let (base, seen, server) = fake_api(200, json!({"__pdf_cipher":STANDARD.encode(cipher)})).await;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let stored = account(&base);
        let persisted = Arc::new(Mutex::new(stored.clone()));
        let ledger = persisted.clone();
        let fail_once = Arc::new(AtomicUsize::new(0));
        let save: Save = Arc::new(move |state| {
            let ledger = ledger.clone();
            let fail_once = fail_once.clone();
            Box::pin(async move {
                if state.records.iter().any(|r| r.args["pdf_receipt"].is_string() && r.job_id.is_some())
                    && fail_once.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("simulated loss of question acknowledgement".into());
                }
                *ledger.lock().await = state;
                Ok(())
            })
        });
        let bridge = Bridge::new_with_pdf(stored, api.clone(), save.clone(), control.clone(),
            root.path().into(), Some(agent.clone()), Some(inbox.clone()));
        let mut attachment = msg("9007199254740993", "");
        attachment["item_list"] = json!([{"type":4,"msg_id":"9007199254740994",
            "file_item":{"file_name":"合成.pdf","len":bytes.len().to_string(),
                "media":{"aes_key":STANDARD.encode(key),"encrypt_query_param":"synthetic-pdf-question"}}}]);
        bridge.ingest(&attachment).await.unwrap();
        let pdf = bridge.status().await.tasks[0].id.clone();
        assert!(received.lock().await.is_empty());
        assert!(engine.session(&engine.sessions().await[0].id).await.unwrap().turns.is_empty());

        let mut question = msg("question", "这份 PDF 的首页讲了什么？");
        question["item_list"][0]["ref_msg"] = json!({"svr_id":"9007199254740994"});
        let mut foreign = question.clone();
        foreign["from_user_id"] = json!("foreign-user");
        bridge.ingest(&foreign).await.unwrap();
        bridge.ingest(&msg("ordinary", "进度")).await.unwrap();
        assert!(received.lock().await.is_empty());
        assert!(bridge.ingest(&question).await.is_err());
        let after_lost_ack = persisted.lock().await.clone();
        let original = after_lost_ack.records.last().unwrap();
        assert_eq!(original.args["pdf_receipt"], pdf);
        assert_eq!(original.status, "received");
        assert!(original.job_id.is_none());
        let interrupted_id = original.id.clone();
        let bridge = Bridge::new_with_pdf(after_lost_ack, api, save, control, root.path().into(), Some(agent), Some(inbox));
        bridge.submit(&interrupted_id).await.unwrap();
        bridge.ingest(&question).await.unwrap();
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if persisted.lock().await.records.iter().any(|r| r.id == interrupted_id && r.delivery == "accepted") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.unwrap();
        monitor.abort();
        monitor.await.unwrap_err();
        assert_eq!(received.lock().await.len(), 1);
        let prompt = serde_json::to_string(&received.lock().await[0]).unwrap();
        assert!(prompt.contains("中文🙂"));
        assert!(prompt.contains("首页摘录"));
        assert!(!prompt.contains("synthetic-pdf-question"));
        let old_scope = engine.sessions().await[0].id.clone();
        assert_eq!(engine.session(&old_scope).await.unwrap().turns.len(), 1);
        let precise = format!("/问PDF {pdf} 请标明这条原收件的首页。");
        let explicit = msg("explicit-pdf-question", &precise);
        bridge.ingest(&explicit).await.unwrap();
        let explicit_turn = persisted.lock().await.records.last().unwrap().job_id.clone().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let scope = engine.sessions().await[0].id.clone();
                if engine.session(&scope).await.unwrap().turns.iter().any(|turn|
                    turn.id == explicit_turn && turn.state == "completed") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.unwrap();
        assert_eq!(received.lock().await.len(), 2);
        assert_eq!(engine.session(&old_scope).await.unwrap().turns.len(), 2);
        bridge.ingest(&msg("new-session", "新对话")).await.unwrap();
        bridge.ingest(&msg("out-of-scope", &format!("/问PDF {pdf} 还记得这份文件吗？"))).await.unwrap();
        let mut missing = msg("missing-quote", "看看这个文件");
        missing["item_list"][0]["ref_msg"] = json!({"svr_id":"9007199254740995",
            "message_item":{"type":4,"file_item":{"file_name":"陌生.pdf","media":{"full_url":"https://invalid.example/private"}}}});
        bridge.ingest(&missing).await.unwrap();
        assert_eq!(received.lock().await.len(), 2);
        let traffic = seen.lock().await;
        assert_eq!(traffic.iter().filter(|r| r["head"].as_str().unwrap().starts_with("GET /c2c/download?")).count(), 1);
        let text: Vec<_> = traffic.iter().filter_map(|r| r["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
        assert!(text.iter().any(|reply| reply.contains("第 1 页含中文样例")));
        assert!(text.iter().any(|reply| reply.contains("当前微信会话无法核对这份 PDF")));
        assert!(text.iter().any(|reply| reply.contains("没有找到引用的 PDF 收件")));
        server.abort();
    }
    #[tokio::test]
    async fn wechat_video_receipt_restarts_as_observation_only_and_delivers_transcript() {
        verify_video_receipt_restart(false).await;
    }

    #[tokio::test]
    async fn wechat_mp4_file_restarts_as_video_and_checks_decrypted_length() {
        verify_video_receipt_restart(true).await;
    }

    async fn verify_video_receipt_restart(file_item: bool) {
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("transcript.txt");
        let transcript = "合成视频已经转写。";
        let original = format!("{transcript}\nhttps://github.com/fixtures/video-project.git。 另一个项目叫 Marked。");
        std::fs::write(&file, &original).unwrap();
        let artifact = json!({"path":file.to_string_lossy(),"sha256":format!("{:x}", Sha256::digest(original.as_bytes()))});
        let repositories = video::RepositoryHints::from_transcript(&original);
        let key = [7u8; 16];
        let bytes = b"\x00\x00\x00\x18ftypisomsynthetic";
        let cipher = ecb::Encryptor::<aes::Aes128>::new((&key).into()).encrypt_padded_vec_mut::<Pkcs7>(bytes);
        let (base, seen, server) = fake_api(200, json!({"__pdf_cipher":STANDARD.encode(&cipher),"__media":true})).await;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let control: Control = Arc::new(|_| Box::pin(async { panic!("video must not invoke a general control job") }));
        let ready = Arc::new(AtomicUsize::new(0));
        let starts = Arc::new(AtomicUsize::new(0));
        let count = starts.clone();
        let model = ready.clone();
        let inbox: video::VideoInbox = Arc::new(move |request| {
            let artifact = artifact.clone();
            let count = count.clone();
            let model = model.clone();
            let repositories = repositories.clone();
            Box::pin(async move {
                match request {
                    video::VideoRequest::Start { id, bytes: pulled } => {
                        assert!(video::receipt_id(&id));
                        assert_eq!(&**pulled, bytes);
                        count.fetch_add(1, Ordering::SeqCst);
                        video::VideoResult::Running
                    }
                    video::VideoRequest::Observe { id } => {
                        assert!(video::receipt_id(&id));
                        if model.load(Ordering::SeqCst) == 0 {
                            video::VideoResult::Running
                        } else {
                            video::VideoResult::Ready { preview: transcript.into(), artifact, repositories }
                        }
                    }
                    _ => panic!("video test received a voice request"),
                }
            })
        });
        let stored = account(&base);
        let state = Arc::new(Mutex::new(stored.clone()));
        let output = state.clone();
        let save: Save = Arc::new(move |next| {
            let output = output.clone();
            Box::pin(async move { *output.lock().await = next; Ok(()) })
        });
        let bridge = Bridge::new(stored, api.clone(), save.clone(), control.clone(), folder.path().into());
        bridge.configure_video(inbox.clone());
        let mut message = msg("native-video", "");
        let media = json!({"aes_key":STANDARD.encode(key),"encrypt_query_param":"synthetic-video-download"});
        let digest = format!("{:x}", md5::Md5::digest(bytes));
        message["item_list"] = if file_item {
            json!([{"type":4,"msg_id":"9007199254741013","file_item":{
                "file_name":"合成.MP4","len":bytes.len().to_string(),"md5":digest.to_uppercase(),"media":media}}])
        } else {
            json!([{"type":5,"msg_id":"9007199254741013","video_item":{
                "video_size":cipher.len(),"video_md5":digest,"media":media}}])
        };
        let mut foreign = message.clone();
        foreign["from_user_id"] = json!("foreign");
        bridge.ingest(&foreign).await.unwrap();
        let mut item_only = message.clone();
        item_only.as_object_mut().unwrap().remove("message_id");
        bridge.ingest(&item_only).await.unwrap();
        bridge.ingest(&message).await.unwrap();
        assert_eq!(state.lock().await.records.len(), 1);
        let id = bridge.status().await.tasks[0].id.clone();
        bridge.ingest(&msg("video-status", &format!("/状态 {id}"))).await.unwrap();
        let snapshot = state.lock().await.clone();
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-video-download"));
        assert_eq!(starts.load(Ordering::SeqCst), 1);

        let restarted = Bridge::new(snapshot, api, save, control, folder.path().into());
        restarted.configure_video(inbox);
        ready.store(1, Ordering::SeqCst);
        let mut record = state.lock().await.records[0].clone();
        record.args["phase"] = json!("receiving");
        record.created_at = (chrono::Utc::now() - chrono::Duration::seconds(45)).to_rfc3339();
        restarted.update_media(&record, 0).await.unwrap();
        restarted.update_media(&record, 0).await.unwrap();
        restarted.ingest(&message).await.unwrap();
        let result = &restarted.status().await.tasks[0];
        assert_eq!(result.status, "completed");
        assert_eq!(result.delivery, "accepted");
        restarted.ingest(&msg("video-result", &format!("/结果 {id}"))).await.unwrap();
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        if file_item {
            let mut mismatch = msg("bad-video-length", "");
            let mut item = message["item_list"][0].clone();
            item["msg_id"] = json!("9007199254741021");
            item["file_item"]["len"] = json!(cipher.len().to_string());
            mismatch["item_list"] = json!([item]);
            restarted.ingest(&mismatch).await.unwrap();
            let status = restarted.status().await;
            assert!(status.tasks.iter().any(|task| task.command == "wechat_video_transcript" && task.status == "failed"));
            assert_eq!(starts.load(Ordering::SeqCst), 1);
        }
        let traffic = seen.lock().await;
        let contains = |fragment: &str| traffic.iter().filter(|r| r["head"].as_str().unwrap().contains(fragment)).count();
        assert_eq!(contains("GET /c2c/download?"), if file_item { 2 } else { 1 });
        assert_eq!(contains("/getuploadurl "), 2);
        let texts: Vec<_> = traffic.iter().filter_map(|r| r["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
        assert!(texts.iter().any(|text| text.contains("正在处理原视频")));
        if file_item {
            assert!(texts.iter().any(|text| text.contains("文件长度或原校验值不符")));
        }
        assert_eq!(texts.iter().filter(|text| text.contains(transcript)).count(), 2);
        assert_eq!(texts.iter().filter(|text| text.contains("https://github.com/fixtures/video-project")).count(), 2);
        assert_eq!(texts.iter().filter(|text| text.contains("1. Marked") && text.contains("找第 1 个项目")).count(), 2);
        let saved = state.lock().await;
        assert_eq!(saved.records[0].result.as_ref().unwrap()["repository_refs"], json!(["https://github.com/fixtures/video-project"]));
        assert_eq!(saved.records[0].result.as_ref().unwrap()["project_names"], json!(["Marked"]));
        server.abort();
    }

    #[tokio::test]
    async fn unreadable_wechat_share_preserves_only_a_notice_and_recovers_once() {
        let root = tempfile::tempdir().unwrap();
        let (base, traffic, server) = fake_api(200, json!({})).await;
        let stored = account(&base);
        let saved = Arc::new(Mutex::new(stored.clone()));
        let allow_send = Arc::new(AtomicBool::new(false));
        let save: Save = {
            let (saved, allow_send) = (saved.clone(), allow_send.clone());
            Arc::new(move |next| {
                let (saved, allow_send) = (saved.clone(), allow_send.clone());
                Box::pin(async move {
                    if !allow_send.load(Ordering::SeqCst) && next.records[0].delivery == "unknown" {
                        return Err("synthetic stop before send claim".into());
                    }
                    *saved.lock().await = next;
                    Ok(())
                })
            })
        };
        let control: Control = Arc::new(|_| Box::pin(async { panic!("unreadable share must not execute a task") }));
        let make_bridge = |stored| {
            let mut api = Api::new().unwrap();
            api.test_origin = Some(base.clone());
            Bridge::new(stored, api, save.clone(), control.clone(), root.path().into())
        };
        let mut message = msg("original-unreadable-share", "");
        message["item_list"] = json!([{"type":47,"msg_id":"9734","opaque":{"value":"synthetic-opaque-share-payload"}}]);
        let bridge = make_bridge(stored);
        assert!(bridge.ingest(&message).await.is_err());
        assert!(traffic.lock().await.is_empty());
        let snapshot = saved.lock().await.clone();
        let id = snapshot.records[0].id.clone();
        assert!(snapshot.records[0].pending_share_notice());
        assert_eq!(snapshot.records[0].status, "unsupported");
        assert!(!serde_json::to_string(&snapshot).unwrap().contains("synthetic-opaque-share-payload"));

        allow_send.store(true, Ordering::SeqCst);
        let reopened = make_bridge(snapshot);
        let monitor = tokio::spawn(reopened.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), async {
            while saved.lock().await.records[0].delivery != "accepted" {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        monitor.abort();
        reopened.ingest(&message).await.unwrap();
        assert_eq!(saved.lock().await.records.len(), 1);
        assert_eq!(traffic.lock().await.len(), 1);
        reopened.ingest(&msg("query-original-share", &format!("/结果 {id}"))).await.unwrap();
        let requests = traffic.lock().await;
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(request["body"]["msg"]["item_list"][0]["text_item"]["text"], UNREADABLE_SHARE_REPLY);
        }
        server.abort();
    }
    #[tokio::test]
    async fn wechat_video_result_lists_repositories_from_a_verified_legacy_transcript() {
        let folder = tempfile::tempdir().unwrap();
        let (base, traffic, server) = fake_api(200, json!({"__media":true})).await;
        let mut stored = account(&base);
        let source = stored.logical_key();
        let receipt = format!("wx-{}", "a".repeat(64));
        let file = folder.path().join("video-transcripts").join(&receipt).join("transcripts")
            .join("job").join("exports").join("transcript.txt");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = format!("{}\n源代码（https://github.com/markedjs/marked.git）。\nhttps://github.com/MarkedJS/Marked\ngithub.com/next/project\nhttps://github.com/hidden/repo?token=synthetic。下一项项目名叫 HyperFrames。", "省略".repeat(700));
        std::fs::write(&file, &text).unwrap();
        stored.records.push(Record {
            id: receipt.clone(), command: "wechat_video_transcript".into(), status: "completed".into(),
            delivery: "accepted".into(), args: json!({"source":source,"scope":"synthetic-wechat-scope"}),
            result: Some(json!({"text":"隔离旧转写预览。","artifact":{"path":file,"sha256":format!("{:x}",Sha256::digest(text.as_bytes()))}})),
            ..Record::default()
        });
        let effects = Arc::new(AtomicUsize::new(0));
        let (bridge, memory) = test_bridge(stored, folder.path(), controller(folder.path(), effects.clone()));
        bridge.ingest(&msg("old-status", &format!("/状态 {receipt}"))).await.unwrap();
        bridge.ingest(&msg("old-result", &format!("/结果 {receipt}"))).await.unwrap();
        let requested = msg("legacy-name", &format!("/找项目 {receipt} #1"));
        let account = memory.lock().await.account();
        assert!(matches!(bridge.search_video_repository(&requested, &account, &format!("/找项目 {receipt} #1")).await,
            Some(Ok(Action::Call(command, args))) if command == repository_search::COMMAND && args["query"] == "HyperFrames"));
        std::fs::write(&file, "https://github.com/not-original/project").unwrap();
        bridge.ingest(&msg("tampered-result", &format!("/结果 {receipt}"))).await.unwrap();
        assert!(matches!(bridge.search_video_repository(&requested, &account, &format!("/找项目 {receipt} #1")).await, Some(Err(_))));
        let traffic = traffic.lock().await;
        let replies: Vec<_> = traffic.iter().filter_map(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
        assert!(!replies[0].contains("github.com"));
        assert_eq!(replies[1].matches("https://github.com/markedjs/marked").count(), 1);
        assert!(replies[1].contains("https://github.com/next/project"));
        assert!(replies[1].contains("1. HyperFrames"));
        assert!(!replies[1].contains("synthetic"));
        assert!(!replies[2].contains("not-original") && !replies[2].contains("github.com/markedjs"));
        assert_eq!(traffic.iter().filter(|row| row["body"]["msg"]["item_list"][0]["type"] == 4).count(), 1);
        assert!(memory.lock().await.records[0].result.as_ref().unwrap().get("repository_refs").is_none());
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_video_quotes_resolve_the_same_receipt_after_a_result_reply() {
        let root = tempfile::tempdir().unwrap();
        let (base, traffic, server) = fake_api(200, json!({"__media":true, "__send_message_ids":{
            "synthetic-result":"803", "synthetic-result-file-0":"804"
        }})).await;
        let mut stored = account(&base);
        let source = stored.logical_key();
        let receipt = identity::legacy_receipt_id(&stored, "701");
        let other = identity::legacy_receipt_id(&stored, "702");
        let transcript = b"Synthetic transcript";
        let path = root.path().join("transcript.txt");
        std::fs::write(&path, transcript).unwrap();
        for (id, key, outgoing) in [(&receipt, "701", "801"), (&other, "702", "802")] {
            stored.records.push(Record {
                id: id.clone(),
                command: "wechat_video_transcript".into(),
                status: "completed".into(),
                args: json!({"source":source,"scope":"original-session","quote_source":identity::source_key(&source, key)}),
                result: Some(json!({"text":"合成文字稿","artifact":{"path":path,"sha256":format!("{:x}", Sha256::digest(transcript))},
                    "repository_refs":["https://github.com/example/video"],"project_names":["Marked"]})),
                outbound_quote_keys: vec![identity::source_key(&source, outgoing)],
                ..Record::default()
            });
        }
        stored.records.push(Record {
            id: "numbered-result-message".into(),
            command: "message".into(),
            status: "received".into(),
            delivery: "pending".into(),
            client_id: "synthetic-result".into(),
            context: "synthetic-context".into(),
            args: json!({"numbered_query":{"target":receipt,"files":true}}),
            ..Record::default()
        });
        stored.records.push(Record {
            id: identity::legacy_receipt_id(&stored, "900"),
            command: "system_info".into(),
            ..Record::default()
        });
        let account = stored.account();
        let effects = Arc::new(AtomicUsize::new(0));
        let control = controller(root.path(), effects.clone());
        let (bridge, memory) = test_bridge(stored, root.path(), control.clone());
        bridge.resume_saved_query("numbered-result-message", 0).await.unwrap();
        let saved = memory.lock().await.clone();
        assert_eq!(saved.records[2].delivery, "accepted");
        assert_eq!(saved.records[2].outbound_quote_keys, ["803", "804"].map(|key| identity::source_key(&source, key)));
        let snapshot = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        let (bridge, _) = test_bridge(snapshot, root.path(), control);
        let question = "帮我总结这段视频";
        let mut message = msg("reuse-video", question);
        for reference in [
            json!({"message_item":{"type":4,"msg_id":"701","file_item":{"file_name":"测试.MP4"}}}),
            json!({"svr_id":"801"}),
            json!({"message_item":{"type":4,"msg_id":"804","file_item":{"file_name":"transcript.txt"}}}),
            json!({"svr_id":"801","message_item":{"msg_id":"803"}}),
        ] {
            message["item_list"][0]["ref_msg"] = reference;
            assert!(matches!(bridge.video_question(&message, &account, question).await,
                Some(Ok(Action::Call(command, args))) if command == "assistant" && args["video_receipt"] == receipt));
        }
        message["item_list"][0]["ref_msg"] = json!({"svr_id":"801","message_item":{"msg_id":"802"}});
        assert!(matches!(bridge.video_question(&message, &account, question).await,
            Some(Err(error)) if error.contains("多段视频")));
        message["item_list"][0]["ref_msg"] = json!({"svr_id":"801","message_item":{"msg_id":"900"}});
        assert!(matches!(bridge.video_question(&message, &account, question).await,
            Some(Err(error)) if error.contains("多个任务")));
        message["item_list"][0]["ref_msg"] = json!({"message_item":{"type":4,"msg_id":"999","file_item":{"file_name":"陌生.mp4",
            "media":{"full_url":"https://invalid.example/private"}}}});
        assert!(matches!(bridge.video_question(&message, &account, question).await,
            Some(Err(error)) if error.contains("没有找到引用的视频收件")));
        message["item_list"][0]["ref_msg"] = json!({"svr_id":"803"});
        assert!(matches!(bridge.video_question(&message, &account, "请参考这段视频的剪辑，帮我总结一下。").await,
            Some(Ok(Action::Call(command, args))) if command == "assistant"
                && args["video_receipt"] == receipt && args["video_clip_receipt"].is_null()));
        let mut foreign = account.clone();
        foreign.owner = "other-owner".into();
        assert!(bridge.video_question(&message, &foreign, question).await.is_none());
        assert!(matches!(bridge.quoted_task(&account, &message, true).await,
            Some(Ok(Action::Query(id, true, false))) if id == receipt));
        assert!(matches!(bridge.search_video_repository(&message, &account, "找第 1 个项目").await,
            Some(Ok(Action::Call(command, args))) if command == repository_search::COMMAND && args["query"] == "Marked"));
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        assert!(!traffic.lock().await.iter().any(|entry| entry["head"].as_str().unwrap().starts_with("GET ")));
        server.abort();
    }

    #[tokio::test]
    async fn wechat_video_question_uses_the_original_transcript_once_in_its_session() {
        use crate::assistant_protocol::ModelReply;
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyInit};

        let folder = tempfile::tempdir().unwrap();
        let control: Control = Arc::new(|_| Box::pin(async { panic!("video questions are read-only") }));
        let captures = Arc::new(Mutex::new(Vec::new()));
        let prompts = captures.clone();
        let mut initial = crate::assistant::State::default();
        initial.settings.enabled = true;
        let engine_file = folder.path().join("assistant.dpapi");
        let destination = engine_file.clone();
        let persist: crate::assistant::Save = Arc::new(move |state| {
            let file = destination.clone();
            Box::pin(async move {
                let plain = Zeroizing::new(serde_json::to_vec(&state).map_err(|err| err.to_string())?);
                let sealed = crate::wechat_crypto::crypt(&plain, true)?;
                crate::control_protocol::publish_private(&file, &sealed).await.map_err(|err| err.message)
            })
        });
        let model: crate::assistant::Model = Arc::new(move |request| {
            let prompts = prompts.clone();
            Box::pin(async move {
                prompts.lock().await.push((request.definitions, request.messages));
                Ok(ModelReply {
                    text: "视频提到了蓝色圆点。".into(), calls: vec![],
                    message: json!({"role":"assistant","content":"视频提到了蓝色圆点。"}),
                    usage: None, diagnostics: None,
                })
            })
        });
        let assistant_workspace = folder.path().join("assistant");
        let engine = crate::assistant::Engine::new(
            initial, persist.clone(), model.clone(), control.clone(), assistant_workspace.clone(),
        ).await.unwrap();
        let shared = engine.clone();
        let agent: Agent = Arc::new(move |request| {
            let shared = shared.clone();
            Box::pin(async move { shared.channel(request).await })
        });
        let key = [7u8; 16];
        let raw = b"\x00\x00\x00\x18ftypisomsynthetic";
        let cipher = ecb::Encryptor::<aes::Aes128>::new((&key).into()).encrypt_padded_vec_mut::<Pkcs7>(raw);
        let (base, traffic, server) = fake_api(200, json!({"__pdf_cipher":STANDARD.encode(&cipher),"__media":true})).await;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let stored = account(&base);
        let persisted = Arc::new(Mutex::new(stored.clone()));
        let ledger = persisted.clone();
        let lose_ack = Arc::new(AtomicUsize::new(0));
        let save: Save = Arc::new(move |next| {
            let ledger = ledger.clone();
            let lose_ack = lose_ack.clone();
            Box::pin(async move {
                if next.records.iter().any(|r| r.args["video_receipt"].is_string() && r.job_id.is_some())
                    && lose_ack.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err("simulated lost video question acknowledgement".into());
                }
                *ledger.lock().await = next;
                Ok(())
            })
        });
        let workspace = folder.path().join("video-inbox");
        let output = workspace.clone();
        let starts = Arc::new(AtomicUsize::new(0));
        let count = starts.clone();
        let transcript = format!("蓝色圆点\n{}不得传输的末尾", "😀".repeat(1700));
        let content = transcript.clone();
        let inbox: video::VideoInbox = Arc::new(move |request| {
            let output = output.clone();
            let count = count.clone();
            let content = content.clone();
            Box::pin(async move {
                let video::VideoRequest::Start { id, .. } = request else { panic!("already completed") };
                count.fetch_add(1, Ordering::SeqCst);
                let path = output.join("video-transcripts").join(id).join("transcripts")
                    .join("job").join("exports").join("transcript.txt");
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, &content).unwrap();
                let canonical = path.canonicalize().unwrap();
                video::VideoResult::Ready {
                    preview: "蓝色圆点".into(),
                    artifact: json!({"path":canonical, "sha256":format!("{:x}", Sha256::digest(content.as_bytes()))}),
                    repositories: Default::default(),
                }
            })
        });
        let bridge = Bridge::new_with_pdf(stored, api.clone(), save.clone(), control.clone(),
            workspace.clone(), Some(agent.clone()), None);
        bridge.configure_video(inbox.clone());
        let mut video = msg("9007199254740993", "");
        video["item_list"] = json!([{"type":4,"msg_id":"9007199254740994",
            "file_item":{"file_name":"合成.MP4","len":raw.len().to_string(),
                "media":{"aes_key":STANDARD.encode(key),"encrypt_query_param":"synthetic-video"}}}]);
        bridge.ingest(&video).await.unwrap();
        bridge.ingest(&video).await.unwrap();
        assert!(captures.lock().await.is_empty());
        let original = persisted.lock().await.records[0].clone();
        assert_eq!(original.status, "completed");
        let sessions = engine.sessions().await;
        assert_eq!(sessions.len(), 1, "the received video belongs to a shared conversation before any question");
        assert_eq!(original.args["scope"], sessions[0].id);

        let mut quote = msg("quoted-video", "提取视频里的关键信息");
        quote["item_list"][0]["ref_msg"] = json!({"message_item":{
            "type":4,"msg_id":"9007199254740994","file_item":{"file_name":"合成.MP4"}}});
        let mut foreign = quote.clone();
        foreign["from_user_id"] = json!("foreign");
        bridge.ingest(&foreign).await.unwrap();
        assert!(captures.lock().await.is_empty());
        assert!(bridge.ingest(&quote).await.is_err());
        let saved = persisted.lock().await.clone();
        let question = saved.records.last().unwrap();
        assert_eq!(question.args["video_receipt"], original.id);
        assert!(question.job_id.is_none());
        let interrupted = question.id.clone();
        let session_id = sessions[0].id.clone();
        let original_turn = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(turn) = engine.session(&session_id).await.unwrap().turns.into_iter()
                    .find(|turn| turn.state == "completed") {
                    break turn.id;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.unwrap();
        drop(bridge);
        drop(agent);
        drop(engine);
        let encrypted = std::fs::read(&engine_file).unwrap();
        let reopened = crate::wechat_crypto::crypt(&encrypted, false).unwrap();
        let state = serde_json::from_slice(&reopened).unwrap();
        let engine = crate::assistant::Engine::new(
            state, persist, model, control.clone(), assistant_workspace,
        ).await.unwrap();
        let shared = engine.clone();
        let agent: Agent = Arc::new(move |request| {
            let shared = shared.clone();
            Box::pin(async move { shared.channel(request).await })
        });
        let bridge = Bridge::new_with_pdf(saved, api, save, control, workspace, Some(agent), None);
        bridge.configure_video(inbox);
        bridge.submit(&interrupted).await.unwrap();
        assert_eq!(persisted.lock().await.records.last().unwrap().job_id.as_deref(), Some(original_turn.as_str()));
        bridge.ingest(&quote).await.unwrap();
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if persisted.lock().await.records.iter().any(|r| r.id == interrupted && r.delivery == "accepted") { break; }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.unwrap();
        monitor.abort();
        monitor.await.unwrap_err();
        let prompts = captures.lock().await;
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].0, Some(vec![]));
        let prompt = serde_json::to_string(&prompts[0].1).unwrap();
        assert!(prompt.contains("蓝色圆点"));
        assert!(prompt.contains("最多"));
        assert!(!prompt.contains("不得传输的末尾"));
        assert!(!prompt.contains("video-inbox"));
        drop(prompts);

        let source = PathBuf::from(original.result.unwrap()["artifact"]["path"].as_str().unwrap());
        std::fs::write(&source, "被替换的本地文字").unwrap();
        let command = format!("/问视频 {} 这段话可信吗？", original.id);
        bridge.ingest(&msg("tampered", &command)).await.unwrap();
        assert_eq!(captures.lock().await.len(), 1);
        std::fs::write(&source, &transcript).unwrap();
        bridge.ingest(&msg("explicit-video-question", &command)).await.unwrap();
        let explicit = persisted.lock().await.records.last().unwrap().job_id.clone().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if engine.session(&sessions[0].id).await.unwrap().turns.iter()
                    .any(|turn| turn.id == explicit && turn.state == "completed") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.unwrap();
        bridge.ingest(&msg("new-video-session", "新对话")).await.unwrap();
        bridge.ingest(&msg("wrong-video-session", &command)).await.unwrap();
        assert_eq!(captures.lock().await.len(), 2);
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        let traffic = traffic.lock().await;
        assert_eq!(traffic.iter().filter(|entry| entry["head"].as_str().unwrap().starts_with("GET /c2c/download?")).count(), 1);
        let replies: Vec<_> = traffic.iter().filter_map(|r| r["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
        assert!(replies.iter().any(|reply| reply.contains("蓝色圆点")));
        assert!(replies.iter().any(|reply| reply.contains("摘要不一致")));
        assert!(replies.iter().any(|reply| reply.contains("当前微信会话无法核对")));
        server.abort();
    }

    fn account(base: &str) -> Stored {
        Stored {
            enabled: true,
            token: "synthetic-token".into(),
            owner: "synthetic-owner".into(),
            bot: "synthetic-bot".into(),
            base: base.into(),
            ..Stored::default()
        }
    }

    #[tokio::test]
    async fn rollout_activity_counts_old_work_and_reports_unknown_receipts_without_payload() {
        let lab = tempfile::tempdir().unwrap();
        let mut stored = account("https://example.invalid");
        stored.token = "synthetic-private-value".into();
        stored.records = vec![
            Record { status:"running".into(), delivery:"pending".into(),
                args:json!({"context":"synthetic-private-value"}), ..Record::default() },
            Record { status:"completed".into(), delivery:"accepted".into(),
                result:Some(json!({"status":"future-state","body":"synthetic-private-value"})),
                attachment_receipts:Some(serde_json::from_value(json!([{
                    "digest":"a".repeat(64), "outcome":"unknown"
                }])).unwrap()), ..Record::default() },
            Record { status:"completed".into(), delivery:"unknown".into(), progress_delivery:"unknown".into(),
                ..Record::default() },
            Record { status:"completed".into(), delivery:"accepted".into(),
                result:Some(json!({"status":"succeeded","result":{"status":"running"}})),
                ..Record::default() },
        ];
        stored.records.extend((0..55).map(|_| Record {
            status:"completed".into(), delivery:"accepted".into(), ..Record::default()
        }));
        let bridge = Bridge::new(stored, Api::new().unwrap(),
            Arc::new(|_| Box::pin(async { panic!("an activity read must not persist") })),
            Arc::new(|_| Box::pin(async { panic!("an activity read must not dispatch") })),
            lab.path().into());
        assert_eq!(bridge.status().await.tasks.len(), 50);
        let submit = bridge.submit_lock.lock().await;
        let attachment = bridge.attachment_lock.lock().await;
        let report = serde_json::to_value(bridge.rollout_activity()).unwrap();
        assert_eq!(report["records"], 59);
        assert_eq!(report["pending_work"], 2);
        assert_eq!(report["work_to_review"], 1);
        assert_eq!(report["pending_delivery"], 1);
        assert_eq!(report["delivery_to_review"], 2);
        assert_eq!(report["submit_busy"], true);
        assert_eq!(report["attachment_busy"], true);
        assert!(!report.to_string().contains("synthetic-private-value"));
        drop((submit, attachment));
        let state = bridge.state.lock().await;
        assert!(bridge.rollout_activity().is_none());
        assert_eq!(state.stored.records.len(), 59);
    }
    fn controller(dir: &Path, counter: Arc<AtomicUsize>) -> Arc<Controller> {
        let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
        let runner: Runner = Arc::new(move |name, args| {
            let counter = counter.clone();
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"status":"succeeded","command":name,"input":args,"synthetic":true}))
            })
        });
        Controller::new(catalog, dir, runner).unwrap()
    }
    fn test_bridge(
        stored: Stored,
        root: &Path,
        controller: Arc<Controller>,
    ) -> (Arc<Bridge>, Arc<Mutex<Stored>>) {
        let memory = Arc::new(Mutex::new(stored.clone()));
        let output = memory.clone();
        let save: Save = Arc::new(move |value| {
            let output = output.clone();
            Box::pin(async move {
                *output.lock().await = value;
                Ok(())
            })
        });
        let control: Control = Arc::new(move |request| {
            let controller = controller.clone();
            Box::pin(async move { controller.handle(request).await })
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(stored.base.clone());
        (
            Bridge::new(stored, api, save, control, root.to_path_buf()),
            memory,
        )
    }
    #[tokio::test]
    async fn wechat_stop_control_claims_original_watch_and_sends_one_natural_ack(){
        let root=tempfile::tempdir().unwrap();let(base,seen,server)=fake_api(200,json!({})).await;
        let mut stored=account(&base);stored.records=vec![Record{id:"original".into(),command:"assistant".into(),status:"running".into(),job_id:Some("synthetic-turn".into()),context:"synthetic".into(),client_id:"old".into(),..Default::default()},Record{id:"stop".into(),command:"assistant".into(),status:"received".into(),context:"synthetic".into(),client_id:"stop".into(),args:json!({"text":"停一下"}),..Default::default()}];
        let(template,_)=test_bridge(stored.clone(),root.path(),controller(root.path(),Arc::new(AtomicUsize::new(0))));
        let agent:Agent=Arc::new(|r|Box::pin(async move{if r["operation"]=="notifications"{return Ok(json!([]));}assert_eq!(r["operation"],"submit");Ok(json!({"action":"message","text":"这件事我先停下了。","turn_id":"synthetic-turn"}))}));
        let mut api=Api::new().unwrap();api.test_origin=Some(base);
        let bridge=Bridge::new_with_agent(stored,api,template.save.clone(),template.control.clone(),root.path().into(),Some(agent));
        bridge.submit("stop").await.unwrap();assert_eq!(bridge.state.lock().await.stored.records[0].status,"continued");
        let monitor=tokio::spawn(bridge.clone().monitor(0));tokio::time::sleep(Duration::from_millis(100)).await;assert_eq!(seen.lock().await.len(),1);monitor.abort();server.abort();
    }
    #[tokio::test]
    async fn wechat_new_requirements_are_accepted_during_a_slow_watch() {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        stored.records = vec![
            Record {
                id: "original".into(),
                command: "assistant".into(),
                status: "running".into(),
                job_id: Some("synthetic-turn".into()),
                context: "original-context".into(),
                client_id: "old-client".into(),
                ..Default::default()
            },
            Record {
                id: "change-request".into(),
                command: "assistant".into(),
                status: "received".into(),
                delivery: "pending".into(),
                context: "updated-context".into(),
                client_id: "new-client".into(),
                args: json!({"text":"把圆点改成蓝色"}),
                ..Default::default()
            },
        ];
        let (template, _) = test_bridge(
            stored.clone(),
            root.path(),
            controller(root.path(), Arc::new(AtomicUsize::new(0))),
        );
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let agent: Agent = Arc::new({
            let (started, release, reads) = (started.clone(), release.clone(), reads.clone());
            move |request| {
                let (started, release, reads) = (started.clone(), release.clone(), reads.clone());
                Box::pin(async move {
                    match request["operation"].as_str() {
                        Some("notifications") => Ok(json!([])),
                        Some("submit") => {
                            assert_eq!(request["request_id"], "change-request");
                            Ok(json!({"turn_id":"synthetic-turn","state":"running"}))
                        }
                        Some("get") => {
                            assert_eq!(request["turn_id"], "synthetic-turn");
                            let text = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                                started.notify_one();
                                release.notified().await;
                                "这是旧要求的结果"
                            } else {
                                "已按新要求完成"
                            };
                            Ok(json!({"status":"succeeded","result":{"turn_id":"synthetic-turn","text":text}}))
                        }
                        _ => panic!("unexpected synthetic operation"),
                    }
                })
            }
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new_with_agent(
            stored,
            api,
            template.save.clone(),
            template.control.clone(),
            root.path().into(),
            Some(agent),
        );
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(3), started.notified())
            .await
            .expect("background status lookup did not start");
        tokio::time::timeout(Duration::from_secs(3), bridge.submit("change-request"))
            .await
            .expect("the background lookup blocked the user's change")
            .unwrap();
        assert_eq!(bridge.state.lock().await.stored.records[0].status, "continued");
        release.notify_one();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if !seen.lock().await.is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the updated watch did not deliver a result");
        let traffic = seen.lock().await;
        let text = |index: usize| -> String {
            traffic[index]["body"]["msg"]["item_list"][0]["text_item"]["text"]
                .as_str()
                .unwrap()
                .into()
        };
        assert_eq!(traffic.len(), 1);
        assert_eq!(text(0), "已按新要求完成");
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        drop(traffic);
        let state = bridge.state.lock().await;
        assert_eq!(state.stored.records[0].status, "continued");
        assert_eq!(state.stored.records[1].status, "succeeded");
        drop(state);
        monitor.abort();
        server.abort();
    }
    #[tokio::test]
    async fn wechat_short_tasks_are_quiet_and_identical_progress_never_repeats(){
        let root=tempfile::tempdir().unwrap();let(base,seen,server)=fake_api(200,json!({})).await;
        let mut stored=account(&base);stored.records.push(Record{id:"quiet".into(),command:"assistant".into(),status:"running".into(),created_at:chrono::Utc::now().to_rfc3339(),client_id:"quiet-client".into(),context:"synthetic".into(),..Default::default()});
        let(bridge,_)=test_bridge(stored,root.path(),controller(root.path(),Arc::new(AtomicUsize::new(0))));
        let phase=json!({"result":{"progress_signature":"same-stage","progress":"我在核对合成文件。","progress_has_execution_evidence":true}});
        bridge.emit_progress("quiet", &phase, None).await.unwrap();assert!(seen.lock().await.is_empty());
        bridge.change("quiet",|r|r.created_at=(chrono::Utc::now()-chrono::Duration::seconds(15)).to_rfc3339()).await.unwrap();
        bridge.emit_progress("quiet", &phase, None).await.unwrap();assert_eq!(seen.lock().await.len(),1);
        bridge.emit_progress("quiet", &phase, None).await.unwrap();assert_eq!(seen.lock().await.len(),1);
        bridge.change("quiet",|r|r.progress_at=Some((chrono::Utc::now()-chrono::Duration::seconds(181)).to_rfc3339())).await.unwrap();
        let mut later = phase.clone();
        later["result"]["progress_signature"] = json!("internal-state-changed");
        bridge.emit_progress("quiet", &later, None).await.unwrap();assert_eq!(seen.lock().await.len(),1);
        later["result"]["progress"] = json!("文件已核对，正在生成字幕。");
        bridge.emit_progress("quiet", &later, None).await.unwrap();assert_eq!(seen.lock().await.len(),2);server.abort();
    }
    #[tokio::test]
    async fn wechat_transient_assistant_query_failure_keeps_original_watch_and_returns_final(){
        let root=tempfile::tempdir().unwrap();let(base,seen,server)=fake_api(200,json!({})).await;
        let mut stored=account(&base);stored.records.push(Record{id:"watch".into(),command:"assistant".into(),status:"running".into(),job_id:Some("original-turn".into()),client_id:"watch-client".into(),context:"synthetic".into(),..Default::default()});
        let(template,saved)=test_bridge(stored.clone(),root.path(),controller(root.path(),Arc::new(AtomicUsize::new(0))));
        let calls=Arc::new(AtomicUsize::new(0));let count=calls.clone();
        let agent:Agent=Arc::new(move|r|{let count=count.clone();Box::pin(async move{
            if r["operation"]=="notifications"{return Ok(json!([]));}
            assert_eq!(r["operation"],"get");assert_eq!(r["turn_id"],"original-turn");
            if count.fetch_add(1,Ordering::SeqCst)==0{return Err("synthetic unavailable".into());}
            Ok(json!({"status":"succeeded","result":{"turn_id":"original-turn","text":"合成任务完成","artifacts":[]}}))
        })});
        let mut api=Api::new().unwrap();api.test_origin=Some(base);
        let bridge=Bridge::new_with_agent(stored,api,template.save.clone(),template.control.clone(),root.path().into(),Some(agent));
        let monitor=tokio::spawn(bridge.clone().monitor(0));
        let deadline=tokio::time::Instant::now()+Duration::from_secs(8);
        loop{if saved.lock().await.records[0].query_failures==1{break;}assert!(tokio::time::Instant::now()<deadline);tokio::time::sleep(Duration::from_millis(20)).await;}
        {let state=bridge.state.lock().await;assert_eq!(state.stored.records[0].status,"running");assert_eq!(state.stored.records[0].job_id.as_deref(),Some("original-turn"));}
        bridge.change("watch",|r|r.query_retry_at=0).await.unwrap();
        loop{if bridge.status().await.tasks[0].status=="succeeded"{break;}assert!(tokio::time::Instant::now()<deadline);tokio::time::sleep(Duration::from_millis(20)).await;}
        assert_eq!(calls.load(Ordering::SeqCst),2);
        for _ in 0..50 {if !seen.lock().await.is_empty(){break;}tokio::time::sleep(Duration::from_millis(20)).await;}
        assert!(seen.lock().await.iter().any(|v|v.to_string().contains("合成任务完成")));monitor.abort();server.abort();
    }
    #[tokio::test]
    async fn wechat_invalid_assistant_observation_retries_the_same_task_after_restart() {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        let previous_result = json!({"text":"上次已核对的状态"});
        stored.records.push(Record {
            id: "invalid-watch".into(), command: "assistant".into(), status: "running".into(),
            job_id: Some("original-turn".into()), context: "synthetic-watch".into(),
            client_id: "synthetic-watch-id".into(), result: Some(previous_result.clone()), ..Default::default()
        });
        stored.records.push(Record {
            id: "manual-check".into(), command: "message".into(), status: "completed".into(),
            context: "synthetic-manual".into(), client_id: "synthetic-manual-id".into(), ..Default::default()
        });
        let (template, saved) = test_bridge(stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))));
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let agent: Agent = Arc::new(move |request| {
            let calls = observed.clone();
            Box::pin(async move {
                if request["operation"] == "notifications" { return Ok(json!([])); }
                assert_eq!(request["operation"], "get");
                assert_eq!(request["turn_id"], "original-turn");
                Ok(match calls.fetch_add(1, Ordering::SeqCst) {
                    0 => json!({"status":"unrecognized", "result":{"turn_id":"original-turn", "text":"不应发送的未知状态"}}),
                    1 => json!({"status":"succeeded", "result":{"turn_id":"another-turn", "text":"不应发送的其他任务内容"}}),
                    _ => json!({"status":"succeeded", "result":{"status":"succeeded", "turn_id":"original-turn", "text":"只送达原任务结果"}}),
                })
            })
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let bridge = Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent.clone()));
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                {
                    let snapshot = saved.lock().await;
                    let watch = &snapshot.records[0];
                    assert_eq!(watch.status,"running");
                    assert_eq!(watch.result.as_ref(), Some(&previous_result));
                    if watch.query_failures == 1 {
                        assert!(watch.query_retry_at > chrono::Utc::now().timestamp());
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert!(seen.lock().await.is_empty());
        let account = bridge.state.lock().await.stored.account();
        bridge.reply_assistant_turn("manual-check", "original-turn", true, (&account,0)).await.unwrap();
        monitor.abort();
        assert!(monitor.await.unwrap_err().is_cancelled());
        let mut persisted = saved.lock().await.clone();
        assert_eq!(persisted.records[0].query_failures, 1);
        persisted.records[0].query_retry_at = 0;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let restored = Bridge::new_with_agent(persisted, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent));
        let monitor = tokio::spawn(restored.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if saved.lock().await.records[0].delivery == "accepted" { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let traffic = seen.lock().await;
        let messages = traffic.iter().map(|row| row["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap()).collect::<Vec<_>>();
        assert_eq!(messages, ["暂时无法核对原任务，请稍后再查。", "只送达原任务结果"]);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let state = saved.lock().await;
        assert_eq!(state.records[0].status,"succeeded");
        assert_eq!(state.records[0].query_failures,0);
        assert_eq!(state.records[0].job_id.as_deref(), Some("original-turn"));
        drop(state);
        drop(traffic);
        monitor.abort();
        server.abort();
    }

    #[test]
    fn wechat_assistant_observation_requires_original_identity_and_consistent_status() {
        for invalid in [
            json!(null),
            json!({"result":{"turn_id":"original","text":"合成内容"}}),
            json!({"status":"succeeded","result":null}),
            json!({"status":"succeeded","result":{"text":"缺少编号"}}),
            json!({"status":"succeeded","result":{"turn_id":"different","text":"归属不同"}}),
            json!({"status":"succeeded","result":{"turn_id":"original","status":"running","text":"状态冲突"}}),
            json!({"status":"awaiting_confirmation","result":{"turn_id":"original","text":" "}}),
        ] {
            assert!(!valid_assistant_observation(&invalid, "original"));
        }
        assert!(valid_assistant_observation(
            &json!({"status":"unknown","result":{"turn_id":"original","status":"unknown","text":"原任务的结果仍待确认。"}}),
            "original",
        ));
        assert!(valid_assistant_observation(
            &json!({"status":"running","result":{"turn_id":"original","status":"running"}}),
            "original",
        ));
    }

    #[tokio::test]
    async fn wechat_pause_before_assistant_observation_returns_preserves_original_watch() {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        stored.records.push(Record {
            id: "paused-watch".into(),
            command: "assistant".into(),
            status: "running".into(),
            job_id: Some("original-turn".into()),
            client_id: "original-client".into(),
            context: "synthetic".into(),
            delivery: "pending".into(),
            ..Default::default()
        });
        let (template, saved) = test_bridge(
            stored.clone(),
            root.path(),
            controller(root.path(), Arc::new(AtomicUsize::new(0))),
        );
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let agent: Agent = {
            let (entered, release, reads) = (entered.clone(), release.clone(), reads.clone());
            Arc::new(move |request| {
                let (entered, release, reads) = (entered.clone(), release.clone(), reads.clone());
                Box::pin(async move {
                    if request["operation"] == "notifications" {
                        return Ok(json!([]));
                    }
                    assert_eq!(request["operation"], "get");
                    assert_eq!(request["turn_id"], "original-turn");
                    if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                        entered.notify_one();
                        release.notified().await;
                    }
                    Ok(json!({"status":"succeeded","result":{"turn_id":"original-turn","text":"合成原任务已完成","artifacts":[]}}))
                })
            })
        };
        let make_bridge = |stored| {
            let mut api = Api::new().unwrap();
            api.test_origin = Some(base.clone());
            Bridge::new_with_agent(
                stored,
                api,
                template.save.clone(),
                template.control.clone(),
                root.path().into(),
                Some(agent.clone()),
            )
        };
        let bridge = make_bridge(stored);
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), entered.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        let original = serde_json::to_value(&saved.lock().await.records).unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        release.notify_one();
        let mut reloaded = saved.lock().await.clone();
        assert!(!reloaded.enabled);
        assert!(seen.lock().await.is_empty());
        assert_eq!(serde_json::to_value(&reloaded.records).unwrap(), original);

        reloaded.enabled = true;
        let resumed = make_bridge(reloaded);
        let monitor = tokio::spawn(resumed.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if saved.lock().await.records[0].delivery == "accepted" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        let state = saved.lock().await;
        assert_eq!(state.records.len(), 1);
        assert_eq!(state.records[0].job_id.as_deref(), Some("original-turn"));
        assert_eq!(state.records[0].status, "succeeded");
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        let sent = seen.lock().await;
        assert_eq!(sent.len(), 1);
        assert!(sent[0]["body"].to_string().contains("合成原任务已完成"));
        monitor.abort();
        server.abort();
    }
    #[tokio::test]
    async fn wechat_saved_assistant_completion_resumes_without_repeating_observation_or_send() {
        for code in [200, 500] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("original-output.txt");
            let bytes = b"isolated original output";
            std::fs::write(&path, bytes).unwrap();
            let artifact = json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes))});
            let (base, seen, server) = fake_api(code, json!({
                "__media":true,
                "__send_message_ids":{"original-client-update":"194","original-client-update-file-0":"195"}
            })).await;
            let mut stored = account(&base);
            stored.records.push(Record {
                id: "completion-watch".into(),
                command: "assistant".into(),
                status: "running".into(),
                job_id: Some("original-turn".into()),
                client_id: "original-client".into(),
                context: "synthetic-context".into(),
                delivery: "pending".into(),
                ..Default::default()
            });
            let dispatched = Arc::new(AtomicUsize::new(0));
            let (template, saved) = test_bridge(
                stored.clone(), root.path(), controller(root.path(), dispatched.clone()),
            );
            let blocked = Arc::new(std::sync::atomic::AtomicBool::new(true));
            let attempted = Arc::new(tokio::sync::Notify::new());
            let save: Save = {
                let (blocked, attempted, persist) =
                    (blocked.clone(), attempted.clone(), template.save.clone());
                Arc::new(move |value| {
                    let (blocked, attempted, persist) =
                        (blocked.clone(), attempted.clone(), persist.clone());
                    Box::pin(async move {
                        let record = &value.records[0];
                        if value.enabled && record.status == "succeeded"
                            && record.delivery == "unknown" && blocked.load(Ordering::SeqCst)
                        {
                            attempted.notify_one();
                            return Err("synthetic pre-send persistence failure".into());
                        }
                        persist(value).await
                    })
                })
            };
            let reads = Arc::new(AtomicUsize::new(0));
            let payload = json!({"turn_id":"original-turn","text":"合成任务已完成","artifacts":[artifact]});
            let agent: Agent = {
                let (reads, payload) = (reads.clone(), payload.clone());
                Arc::new(move |request| {
                    let (reads, payload) = (reads.clone(), payload.clone());
                    Box::pin(async move {
                        if request["operation"] == "notifications" {
                            return Ok(json!([]));
                        }
                        assert_eq!(request["operation"], "get");
                        assert_eq!(request["turn_id"], "original-turn");
                        reads.fetch_add(1, Ordering::SeqCst);
                        Ok(json!({"status":"succeeded","result":payload}))
                    })
                })
            };
            let make_bridge = |stored| {
                let mut api = Api::new().unwrap();
                api.test_origin = Some(base.clone());
                Bridge::new_with_agent(
                    stored, api, save.clone(), template.control.clone(), root.path().into(),
                    Some(agent.clone()),
                )
            };
            let bridge = make_bridge(stored);
            let monitor = tokio::spawn(bridge.clone().monitor(0));
            tokio::time::timeout(Duration::from_secs(5), attempted.notified()).await.unwrap();
            bridge.stop().await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), monitor).await.unwrap().unwrap();
            let snapshot = serde_json::to_vec(&*saved.lock().await).unwrap();
            let encrypted = crate::wechat_crypto::crypt(&snapshot, true).unwrap();
            let restored = crate::wechat_crypto::crypt(&encrypted, false).unwrap();
            let mut reloaded: Stored = serde_json::from_slice(&restored).unwrap();
            assert_eq!(reloaded.records.len(), 1);
            assert!(!reloaded.enabled);
            assert!(seen.lock().await.is_empty());
            let record = &reloaded.records[0];
            assert_eq!(record.status, "succeeded");
            assert_eq!(record.delivery, "pending");
            assert_eq!(record.result.as_ref(), Some(&payload));
            assert_eq!(record.client_id, "original-client-update");

            blocked.store(false, Ordering::SeqCst);
            reloaded.enabled = true;
            let resumed = make_bridge(reloaded);
            let monitor = tokio::spawn(resumed.clone().monitor(0));
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let settled = {
                        let state = saved.lock().await;
                        let record = &state.records[0];
                        record.delivery == "accepted" || record.delivery_error.is_some()
                    };
                    if settled {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.unwrap();
            resumed.stop().await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), monitor).await.unwrap().unwrap();
            let mut final_state = saved.lock().await.clone();
            let record = &final_state.records[0];
            assert_eq!(record.delivery, if code == 200 { "accepted" } else { "unknown" });
            assert_eq!(record.result.as_ref(), Some(&payload));
            assert_eq!(record.job_id.as_deref(), Some("original-turn"));
            assert_eq!(record.client_id, "original-client-update");
            final_state.enabled = true;
            let recovered = make_bridge(final_state);
            let monitor = tokio::spawn(recovered.clone().monitor(0));
            tokio::time::sleep(Duration::from_millis(100)).await;
            recovered.stop().await.unwrap();
            tokio::time::timeout(Duration::from_secs(5), monitor).await.unwrap().unwrap();
            let traffic = seen.lock().await;
            let sent: Vec<_> = traffic.iter().filter(|call| {
                call["head"].as_str().unwrap().contains("/sendmessage ")
            }).collect();
            assert_eq!(traffic.len(), if code == 200 { 4 } else { 1 });
            assert_eq!(sent.len(), if code == 200 { 2 } else { 1 });
            let text = &sent[0]["body"]["msg"];
            assert_eq!(text["client_id"], "original-client-update");
            assert_eq!(text["item_list"][0]["text_item"]["text"], "合成任务已完成");
            if code == 200 {
                let file = &sent[1]["body"]["msg"];
                assert_eq!(file["client_id"], "original-client-update-file-0");
                assert_eq!(file["item_list"][0]["type"], 4);
            }
            assert_eq!(reads.load(Ordering::SeqCst), 1);
            assert_eq!(dispatched.load(Ordering::SeqCst), 0);
            server.abort();
        }
    }
    #[tokio::test]
    async fn wechat_progress_is_atomic_throttled_bounded_and_not_retried_when_unknown() {
        for code in [200, 500] {
            let root = tempfile::tempdir().unwrap();
            let (base, seen, server) = fake_api(code, json!({})).await;
            let mut stored = account(&base);
            stored.records.push(Record {
                id: "progress-fixture".into(),
                command: "assistant".into(),
                status: "running".into(),
                client_id: "fixture-client".into(),
                context: "synthetic-context".into(),
                created_at:(chrono::Utc::now()-chrono::Duration::seconds(13)).to_rfc3339(),
                ..Default::default()
            });
            let (bridge, _) = test_bridge(
                stored,
                root.path(),
                controller(root.path(), Arc::new(AtomicUsize::new(0))),
            );
            let model_only = json!({"result":{"progress_signature":"model-only",
                "progress":"正在做：整理处理步骤。","progress_has_execution_evidence":false}});
            bridge.emit_progress("progress-fixture", &model_only, None).await.unwrap();
            assert!(seen.lock().await.is_empty());
            let first =
                json!({"result":{"progress_signature":"stage-a","progress":"正在查询隔离任务","progress_has_execution_evidence":true}});
            let (a, b) = tokio::join!(
                bridge.emit_progress("progress-fixture", &first, None),
                bridge.emit_progress("progress-fixture", &first, None)
            );
            assert!(a.is_ok() || b.is_ok());
            assert_eq!(seen.lock().await.len(), 1);
            assert_eq!(
                bridge.status().await.tasks[0].progress_delivery,
                if code == 200 { "accepted" } else { "unknown" }
            );
            let next =
                json!({"result":{"progress_signature":"stage-b","progress":"正在生成隔离任务","progress_has_execution_evidence":true}});
            bridge
                .emit_progress("progress-fixture", &next, None)
                .await
                .unwrap();
            assert_eq!(seen.lock().await.len(), 1);
            bridge
                .change("progress-fixture", |r| {
                    r.progress_at = Some("2026-01-01T00:00:00Z".into());
                    r.progress_count = 12;
                })
                .await
                .unwrap();
            bridge
                .emit_progress("progress-fixture", &next, None)
                .await
                .unwrap();
            assert_eq!(seen.lock().await.len(), 1);
            {
                let mut state = bridge.state.lock().await;
                state.stored.enabled = false;
                let record = &mut state.stored.records[0];
                record.progress_count = 0;
                record.progress_signature.clear();
                record.progress_at = None;
            }
            bridge
                .emit_progress("progress-fixture", &next, None)
                .await
                .unwrap();
            assert_eq!(seen.lock().await.len(), 1);
            server.abort();
        }
    }
    #[test]
    fn wechat_commands_are_scoped_and_output_paths_are_owned() {
        let root = Path::new("C:/synthetic/wechat");
        assert!(parse("/调用 secrets_list {}", root).is_err());
        assert!(parse("/调用 studio_chat_send {}", root).is_err());
        assert!(parse("/调用 capability_render {\"rootPath\":\"C:/real\"}", root).is_err());
        assert!(parse(
            "/调用 model_capability_call {\"request\":{\"output_root\":\"C:/real\"}}",
            root
        )
        .is_err());
        let Action::Call(name, input) = parse("视频 隔离动画", root).unwrap() else {
            panic!()
        };
        assert_eq!(name, "capability_render");
        assert_eq!(input["rootPath"], json!(root.join("hyperframes")));
        assert_eq!(input["input"]["title"], "隔离动画");
        assert!(parse("/状态 ../other", root).is_err());
        assert!(parse("任意自然语言不是执行许可", root).is_err());
        let Action::Call(_,input)=parse("/调用 model_capability_call {\"request\":{\"route_id\":\"fixture::model\",\"input\":{}}}",root).unwrap() else{panic!()};
        assert_eq!(input["request"]["output_root"], json!(root.join("models")));
    }
    #[test]
    fn wechat_fixed_hosts_and_business_errors_fail_closed() {
        assert!(official_base("https://ilinkai.weixin.qq.com").is_ok());
        for bad in [
            "http://ilinkai.weixin.qq.com",
            "https://ilinkai.weixin.qq.com.evil.test",
            "https://user:pass@ilinkai.weixin.qq.com",
            "https://127.0.0.1",
            "https://ilinkai.weixin.qq.com/path",
        ] {
            assert!(official_base(bad).is_err());
        }
        assert!(validate_cdn("https://novac2c.cdn.weixin.qq.com/c2c/upload?x=1").is_ok());
        assert!(validate_cdn("https://evil.test/upload").is_err());
        assert!(check_business(&json!({"ret":0,"errcode":-14}))
            .unwrap_err()
            .starts_with("SESSION_EXPIRED"));
        let error = check_business(&json!({"ret":12,"errmsg":"synthetic-secret"})).unwrap_err();
        assert!(!error.contains("synthetic-secret"));
        assert!(check_business(&json!({"ret":"0"})).is_err());
    }

    #[test]
    fn wechat_sparse_protobuf_updates_are_empty_not_connection_failures() {
        for v in [
            json!({}),
            json!({"ret":0}),
            json!({"get_updates_buf":"synthetic-cursor"}),
            json!({"ret":0,"msgs":null}),
            json!({"msgs":[]}),
        ] {
            assert!(updates(&v).unwrap().is_empty());
        }
        for v in [
            Value::Null,
            json!([]),
            json!({"ret":-14}),
            json!({"errcode":3}),
            json!({"msgs":{}}),
            json!({"msgs":"invalid"}),
            json!({"ret":"0"}),
        ] {
            assert!(updates(&v).is_err());
        }
    }

    #[tokio::test]
    async fn wechat_confirmation_dialog_tracks_connection_instead_of_staying_connecting() {
        let tmp = tempfile::tempdir().unwrap();
        let ctl = controller(tmp.path(), Arc::new(AtomicUsize::new(0)));
        let (bridge, _) = test_bridge(account("http://127.0.0.1:1"), tmp.path(), ctl);
        bridge.state.lock().await.pairing = Some(Pairing {
            code: String::new(),
            base: String::new(),
            pin: None,
            generation: 0,
            view: PairingView {
                state: "confirmed".into(),
                message: "绑定成功，正在连接微信".into(),
                qr_data_url: None,
            },
        });
        bridge.connection("connected", "已连接").await;
        assert_eq!(bridge.pairing().await.message, "绑定成功，微信已连接");
        bridge
            .connection("reconnecting", "合成连接中断，正在重连")
            .await;
        assert_eq!(bridge.pairing().await.message, "合成连接中断，正在重连");
    }

    #[tokio::test]
    async fn wechat_sparse_empty_http_poll_connects_without_creating_tasks() {
        for sparse in [
            json!({"ret":0,"get_updates_buf":"synthetic-cursor"}),
            json!({"msgs":[],"get_updates_buf":"synthetic-cursor"}),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let count = Arc::new(AtomicUsize::new(0));
            let ctl = controller(tmp.path(), count.clone());
            let (base, _, server) = fake_api(200, json!({"__updates_response":sparse})).await;
            let (bridge, memory) = test_bridge(account(&base), tmp.path(), ctl);
            bridge.start().await.unwrap();
            for _ in 0..100 {
                if bridge.status().await.state == "connected" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(bridge.status().await.state, "connected");
            assert_eq!(count.load(Ordering::SeqCst), 0);
            assert!(bridge.status().await.tasks.is_empty());
            for _ in 0..100 {
                if memory.lock().await.cursor == "synthetic-cursor" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert_eq!(memory.lock().await.cursor, "synthetic-cursor");
            bridge.stop().await.unwrap();
            server.abort();
        }
    }

    #[tokio::test]
    #[ignore = "explicit authorized read-only live protocol shape; no token or message content output"]
    async fn wechat_live_response_shape_only() {
        let file = std::env::var_os("HOMEBASE_WECHAT_PROBE_FILE")
            .expect("explicit encrypted channel file required");
        let encrypted = std::fs::read(file).unwrap();
        let plain = crate::wechat_crypto::crypt(&encrypted, false).unwrap();
        let stored: Stored = serde_json::from_slice(&plain).unwrap();
        let base = official_base(&stored.base).unwrap();
        let value = Api::new()
            .unwrap()
            .request(
                &base,
                "/ilink/bot/getupdates",
                Some(&stored.token),
                Some(json!({"get_updates_buf":stored.cursor})),
            )
            .await
            .unwrap();
        let shape=value.as_object().unwrap().iter().map(|(k,v)|json!({"field":k,"type":match v{Value::Null=>"null",Value::Array(_)=>"array",Value::Object(_)=>"object",Value::Number(_)=>"number",Value::String(_)=>"string",Value::Bool(_)=>"bool"}})).collect::<Vec<_>>();
        println!(
            "Live response shape only: {}",
            json!({"fields":shape,"ret":value.get("ret"),"errcode":value.get("errcode"),"message_count":value["msgs"].as_array().map(Vec::len),"parser_accepts":updates(&value).is_ok()})
        );
    }
    #[tokio::test]
    async fn wechat_auth_headers_and_reply_context_match_official_contract() {
        let (base, seen, server) = fake_api(200, json!({"ret":0})).await;
        let api = Api::new().unwrap();
        api.text(
            &account(&base),
            "synthetic-context",
            "stable-client-id",
            "隔离回复",
        )
        .await
        .unwrap();
        let calls = seen.lock().await;
        let call = &calls[0];
        let head = call["head"].as_str().unwrap().to_ascii_lowercase();
        assert!(head.contains("authorization: bearer synthetic-token"));
        assert!(head.contains("authorizationtype: ilink_bot_token"));
        assert!(head.contains("x-wechat-uin:"));
        assert_eq!(call["body"]["base_info"]["channel_version"], VERSION);
        assert_eq!(call["body"]["msg"]["context_token"], "synthetic-context");
        assert_eq!(call["body"]["msg"]["to_user_id"], "synthetic-owner");
        assert_eq!(call["body"]["msg"]["client_id"], "stable-client-id");
        server.abort();
    }
    #[tokio::test]
    async fn wechat_inbound_keys_survive_an_older_writer_without_redispatching() {
        const FIRST_ENVELOPE: &str = "9007199254741100";
        const FIRST_ITEM: &str = "9007199254741101";
        const SECOND_ENVELOPE: &str = "9007199254741102";
        const SECOND_ITEM: &str = "9007199254741103";
        let root = tempfile::tempdir().unwrap();
        let (base, sent, server) = fake_api(200, json!({})).await;
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(root.path(), count.clone());
        let (bridge, saved) = test_bridge(account(&base), root.path(), ctl.clone());
        let with_item = |envelope: Option<&str>, item: &str| {
            let mut message = msg("placeholder", "系统");
            message.as_object_mut().unwrap().remove("message_id");
            if let Some(envelope) = envelope {
                message["message_id"] = json!(envelope);
            }
            message["item_list"][0]["msg_id"] = json!(item);
            message
        };
        bridge.ingest(&with_item(Some(FIRST_ENVELOPE), FIRST_ITEM)).await.unwrap();
        bridge.ingest(&with_item(None, SECOND_ITEM)).await.unwrap();
        bridge.ingest(&with_item(Some(SECOND_ENVELOPE), SECOND_ITEM)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(4), async {
            while count.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        let recorded = saved.lock().await.clone();
        assert_eq!(recorded.records.len(), 2);
        assert!(recorded.records.iter().all(|record| record.job_id.is_some() && record.control_args() == json!({})));
        let original_jobs = recorded.records.iter().map(|record| record.job_id.clone()).collect::<Vec<_>>();
        let mut legacy = serde_json::to_value(recorded).unwrap();
        for record in legacy["records"].as_array_mut().unwrap() {
            record.as_object_mut().unwrap().remove("inbound_keys");
        }
        let encoded = serde_json::to_vec(&legacy).unwrap();
        #[cfg(windows)]
        let encoded = {
            let sealed = crate::wechat_crypto::crypt(&encoded, true).unwrap();
            crate::wechat_crypto::crypt(&sealed, false).unwrap()
        };
        let persisted = serde_json::from_slice(&encoded).unwrap();
        let (reopened, _) = test_bridge(persisted, root.path(), ctl);
        let count_before = sent.lock().await.len();
        reopened.ingest(&with_item(None, FIRST_ITEM)).await.unwrap();
        reopened.ingest(&msg(SECOND_ENVELOPE, "系统")).await.unwrap();
        reopened.ingest(&with_item(Some(FIRST_ENVELOPE), SECOND_ITEM)).await.unwrap();
        let state = reopened.state.lock().await;
        assert_eq!(state.stored.records.iter().map(|record| record.job_id.clone()).collect::<Vec<_>>(), original_jobs);
        assert_eq!(sent.lock().await.len(), count_before);
        assert_eq!(count.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_owner_group_filter_and_duplicate_delivery_never_repeat_work() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count.clone());
        let (base, _, server) = fake_api(200, json!({"ret":0})).await;
        let (bridge, _) = test_bridge(account(&base), tmp.path(), ctl);
        let mut other = msg("other", "系统");
        other["from_user_id"] = json!("outsider");
        bridge.ingest(&other).await.unwrap();
        let mut group = msg("group", "系统");
        group["group_id"] = json!("group");
        bridge.ingest(&group).await.unwrap();
        let mut echo = msg("echo", "系统");
        echo["message_type"] = json!(2);
        bridge.ingest(&echo).await.unwrap();
        let input = msg("1", "系统");
        let (a, b) = tokio::join!(bridge.ingest(&input), bridge.ingest(&input));
        a.unwrap();
        b.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(bridge.status().await.tasks.len(), 1);
        assert!(bridge.status().await.tasks[0].job_id.is_some());
        let safe = serde_json::to_string(&bridge.status().await).unwrap();
        assert!(!safe.contains("synthetic-token"));
        assert!(!safe.contains("synthetic-context"));
        assert!(!safe.contains("synthetic-owner"));
        server.abort();
    }
    #[tokio::test]
    async fn wechat_crash_after_control_acceptance_reuses_durable_request() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count.clone());
        let (base, _, server) = fake_api(200, json!({"ret":0})).await;
        let (bridge, memory) = test_bridge(account(&base), tmp.path(), ctl.clone());
        bridge.ingest(&msg("1", "系统")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut snapshot = memory.lock().await.clone();
        snapshot.records[0].job_id = None;
        snapshot.records[0].status = "received".into();
        drop(bridge);
        drop(ctl);
        let restarted = controller(tmp.path(), count.clone());
        let (bridge, _) = test_bridge(snapshot, tmp.path(), restarted);
        let id = bridge.status().await.tasks[0].id.clone();
        bridge.submit(&id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(bridge.status().await.tasks[0].job_id.is_some());
        server.abort();
    }

    #[tokio::test]
    async fn wechat_capture_saves_exact_share_and_recovers_the_original_receipt() {
        let root = match std::env::var_os("HOMEBASE_TEST_HOME") {
            Some(parent) => {
                std::fs::create_dir_all(&parent).unwrap();
                tempfile::tempdir_in(parent).unwrap()
            }
            None => tempfile::tempdir().unwrap(),
        };
        let vault = root.path().join("Wiki");
        let control_root = root.path().join("control");
        std::fs::create_dir(&control_root).unwrap();
        let receipts = Arc::new(crate::inbox_receipts::ReceiptStore::new(
            root.path().join("inbox-receipts"),
            vault.clone(),
        ));
        let writes = Arc::new(AtomicUsize::new(0));
        let runner: Runner = {
            let writes = writes.clone();
            let receipts = receipts.clone();
            Arc::new(move |name, args| {
                let writes = writes.clone();
                let receipts = receipts.clone();
                Box::pin(async move {
                    assert_eq!(name, "inbox_submit");
                    let payload: crate::inbox_document::InboxSubmitPayload =
                        serde_json::from_value(args["payload"].clone()).unwrap();
                    assert_eq!(payload.source_hint.as_deref(), Some("wechat-share"));
                    assert!(payload.submission_id.is_some());
                    writes.fetch_add(1, Ordering::SeqCst);
                    Ok(json!(crate::inbox_receipts::confirmed_result(
                        receipts.submit(payload).await.unwrap()
                    ).unwrap()))
                })
            })
        };
        let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
        let ctl = Controller::new(catalog, &control_root, runner.clone()).unwrap();
        let (base, sent, server) = fake_api(200, json!({"ret":0})).await;
        let (bridge, memory) = test_bridge(account(&base), root.path(), ctl.clone());
        let share = "一段公开演示🙂\nhttps://example.invalid/video?id=42&view=share\n/调用 capability_install {}";
        let message = msg("share-capture", &format!("帮我收录：\n{share}"));
        let mut visitor = msg("not-the-owner", &format!("/收录 {share}"));
        visitor["from_user_id"] = json!("not-the-owner");
        bridge.ingest(&visitor).await.unwrap();
        assert!(bridge.status().await.tasks.is_empty());
        bridge.ingest(&message).await.unwrap();
        bridge.ingest(&message).await.unwrap();
        let task = bridge.status().await.tasks.remove(0);
        assert_eq!(task.command, "inbox_submit");
        let job = task.job_id.unwrap();
        let mut snapshot = memory.lock().await.clone();
        assert_eq!(snapshot.records.len(), 1);
        let completed = tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let value = bridge.request("jobs.get", None, json!({}), "test", Some(&job)).await.unwrap();
                if !matches!(value["status"].as_str(), Some("running" | "queued")) {
                    assert_eq!(value["status"], "succeeded", "{value}");
                    break value;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let result = &completed["result"];
        let note = PathBuf::from(result["path"].as_str().unwrap());
        let filename = result["filename"].as_str().unwrap();
        assert!(note.starts_with(vault.join("raw/inbox")));
        let original = std::fs::read_to_string(&note).unwrap();
        assert!(original.contains(share));
        assert_eq!(original.matches("https://example.invalid/video").count(), 1);
        assert_eq!(writes.load(Ordering::SeqCst), 1);

        // Recreate a crash after control acceptance but before the channel saved the returned job ID.
        snapshot.records[0].job_id = None;
        snapshot.records[0].status = "received".into();
        let bytes = serde_json::to_vec(&snapshot).unwrap();
        let sealed = crate::wechat_crypto::crypt(&bytes, true).unwrap();
        assert!(!sealed.windows(share.len()).any(|window| window == share.as_bytes()));
        let snapshot = serde_json::from_slice(&crate::wechat_crypto::crypt(&sealed, false).unwrap()).unwrap();
        drop(bridge);
        drop(ctl);
        let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
        let restarted = Controller::new(catalog, &control_root, runner).unwrap();
        let (bridge, _) = test_bridge(snapshot, root.path(), restarted);
        bridge.submit(&task.id).await.unwrap();
        assert_eq!(bridge.status().await.tasks[0].job_id.as_deref(), Some(job.as_str()));
        bridge.ingest(&msg("capture-status", &format!("/状态 {}", task.id))).await.unwrap();
        bridge.ingest(&msg("capture-result", &format!("/结果 {}", task.id))).await.unwrap();
        bridge.ingest(&msg("capture-unapproved", "/调用 inbox_submit {\"payload\":{\"text\":\"禁止\"}}")).await.unwrap();
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read_to_string(&note).unwrap(), original);
        let sent = sent.lock().await;
        let replies: Vec<_> = sent.iter().filter_map(|reply| reply["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect();
        assert!(replies.iter().any(|text| text.contains("微信资料收录") && text.contains("已处理完毕")));
        assert!(replies.iter().any(|text| text.contains(filename) && text.contains(&task.id)));
        assert!(replies.iter().any(|text| text.contains("该接口未开放给微信")));
        assert!(replies.iter().all(|text| !text.contains(note.to_str().unwrap()) && !text.contains("example.invalid")));
        server.abort();
    }

    #[tokio::test]
    async fn wechat_control_reply_describes_system_read_without_dumping_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
        let runner: Runner = Arc::new(|name, _| Box::pin(async move {
            assert_eq!(name, "system_info");
            Ok(json!({
                "os":"Windows 11", "cpu_brand":"合成处理器", "memory_total_gb":16,
                "gpu_status":"not_detected", "gpu_name":null,
                "hostname":"synthetic-private-machine", "username":"synthetic-private-user"
            }))
        }));
        let controller = Controller::new(catalog, root.path(), runner).unwrap();
        let (bridge, saved) = test_bridge(account(&base), root.path(), controller);
        bridge.ingest(&msg("plain-system-read", "系统")).await.unwrap();
        let monitor = tokio::spawn(bridge.clone().monitor(0));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let ready = {
                    let state = saved.lock().await;
                    state.records[0].status == "succeeded" && state.records[0].delivery == "accepted"
                };
                if ready { break; }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        bridge.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        let traffic = seen.lock().await;
        assert_eq!(traffic.len(), 2);
        let accepted = traffic[0]["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap();
        let result = traffic[1]["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap();
        assert!(accepted.contains("系统信息") && accepted.contains("/状态") && !accepted.contains("system_info"));
        assert!(result.contains("Windows 11") && result.contains("合成处理器") && result.contains("16") && result.contains("未识别到物理显卡"));
        assert!(!result.contains("synthetic-private") && !result.contains("gpu_status") && !result.contains("控制状态"));
        server.abort();
    }

    #[tokio::test]
    async fn wechat_control_reply_distinguishes_video_and_model_outcomes_and_returns_original_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original.mp4");
        let bytes = b"synthetic video payload";
        std::fs::write(&path, bytes).unwrap();
        let artifact = json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes)),"mime_type":"video/mp4"});
        let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
        let mut stored = account(&base);
        for (id, name, outcome) in [
            ("failed-video", "capability_render", json!({"status":"failed","error":"缺少本机渲染器"})),
            ("queued-model", "model_capability_call", json!({"status":"queued","task_id":"synthetic-service-id"})),
            ("ready-video", "capability_render", json!({"status":"succeeded","artifact":artifact,"verification":{"seconds":6}})),
        ] {
            stored.records.push(Record {
                id: id.into(), command: name.into(), status: "succeeded".into(),
                job_id: Some(format!("control-{id}")), result: Some(outcome),
                delivery: "accepted".into(), ..Default::default()
            });
        }
        let ctl: Control = Arc::new(|_| Box::pin(async { panic!("verified original receipt must not be queried or rerun") }));
        let (template, _) = test_bridge(stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))));
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new(stored, api, template.save.clone(), ctl, root.path().into());
        bridge.ingest(&msg("failed-video-status", "/状态 failed-video")).await.unwrap();
        bridge.ingest(&msg("queued-model-status", "/状态 queued-model")).await.unwrap();
        bridge.ingest(&msg("ready-video-result", "/结果 ready-video")).await.unwrap();
        let traffic = seen.lock().await;
        let posts = traffic.iter().filter_map(|call| call["body"]["msg"]["item_list"][0].as_object()).collect::<Vec<_>>();
        let texts = posts.iter().filter_map(|item| item.get("text_item")?.get("text")?.as_str()).collect::<Vec<_>>();
        assert_eq!(texts.len(), 3);
        assert!(texts[0].contains("视频未能生成") && texts[0].contains("缺少本机渲染器"));
        assert!(texts[1].contains("服务商正在排队") && texts[1].contains("/模型状态 queued-model"));
        assert!(texts[2].contains("视频已生成") && texts[2].contains("6 秒") && texts[2].contains("ready-video"));
        assert!(texts.iter().all(|text| !text.contains("succeeded") && !text.contains("control-") && !text.contains("sha256")));
        assert_eq!(posts.iter().filter(|item| item.get("type").and_then(Value::as_u64) == Some(4)).count(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_query_and_result_use_only_this_channels_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count);
        let (base, seen, server) = fake_api(200, json!({"ret":0})).await;
        let (bridge, _) = test_bridge(account(&base), tmp.path(), ctl);
        bridge.ingest(&msg("1", "系统")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let id = bridge.status().await.tasks[0].id.clone();
        bridge
            .ingest(&msg("2", &format!("/结果 {id}")))
            .await
            .unwrap();
        bridge
            .ingest(&msg("3", "/结果 external-job-id"))
            .await
            .unwrap();
        let calls = seen.lock().await;
        let texts = calls
            .iter()
            .filter_map(|c| c["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str())
            .collect::<Vec<_>>();
        assert!(texts
            .iter()
            .any(|t| t.contains(id.as_str()) && t.contains("系统信息") && t.contains("已处理完毕")));
        assert!(texts.iter().any(|t| t.contains("未找到本微信通道")));
        server.abort();
    }

    #[tokio::test]
    async fn wechat_numbered_result_resumes_before_reply_and_claims_delivery_before_post() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("output.mp4");
        let bytes = b"synthetic-isolated-video";
        std::fs::write(&file, bytes).unwrap();
        let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
        let mut stored = account(&base);
        stored.records.push(Record {
            id: "original-result".into(), command: "capability_render".into(),
            status: "succeeded".into(), delivery: "accepted".into(),
            result: Some(json!({"status":"succeeded","artifact":{"path":file,"sha256":format!("{:x}",Sha256::digest(bytes))}})),
            ..Default::default()
        });
        let template = test_bridge(stored.clone(), tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0)))).0;
        let persisted = Arc::new(Mutex::new(stored.clone()));
        let save_count = Arc::new(AtomicUsize::new(0));
        let save: Save = {
            let persisted = persisted.clone();
            let save_count = save_count.clone();
            Arc::new(move |value| {
                let persisted = persisted.clone();
                let count = save_count.clone();
                Box::pin(async move {
                    if count.fetch_add(1, Ordering::SeqCst) > 0 {
                        return Err("synthetic interruption after inbound acceptance".into());
                    }
                    *persisted.lock().await = value;
                    Ok(())
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let bridge = Bridge::new(stored, api, save, template.control.clone(), tmp.path().into());
        let incoming = msg("recover-numbered-query", "/结果 original-result");
        assert!(bridge.ingest(&incoming).await.is_err());
        assert!(seen.lock().await.is_empty(), "claim must be saved before posting");

        let snapshot = persisted.lock().await.clone();
        assert_eq!(snapshot.records[1].status, "received");
        assert_eq!(snapshot.records[1].delivery, "pending");
        assert_eq!(snapshot.records[1].pending_query(), Some((QueryTarget::Numbered("original-result"), true)));
        #[cfg(windows)]
        let snapshot: Stored = {
            let value = serde_json::to_vec(&snapshot).unwrap();
            let cipher = crate::wechat_crypto::crypt(&value, true).unwrap();
            let file = tmp.path().join("isolated-channel.dpapi");
            std::fs::write(&file, &cipher).unwrap();
            let reopened = std::fs::read(file).unwrap();
            assert!(!String::from_utf8_lossy(&reopened).contains("original-result"));
            serde_json::from_slice(&crate::wechat_crypto::crypt(&reopened, false).unwrap()).unwrap()
        };
        let (restored, after) = test_bridge(snapshot, tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        let monitor = tokio::spawn(restored.clone().monitor(0));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while after.lock().await.records[1].delivery != "accepted" {
            assert!(tokio::time::Instant::now() < deadline, "original query was not recovered");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        restored.ingest(&incoming).await.unwrap();
        let requests = seen.lock().await;
        let replies = requests.iter().filter_map(|v| v["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str()).collect::<Vec<_>>();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].contains("original-result"));
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[3]["body"]["msg"]["item_list"][0]["type"], 4);
        drop(requests);
        monitor.abort();
        server.abort();
    }

    #[tokio::test]
    async fn wechat_numbered_query_with_unconfirmed_ack_is_not_posted_again() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let stored = account(&base);
        let template = test_bridge(stored.clone(), tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0)))).0;
        let persisted = Arc::new(Mutex::new(stored.clone()));
        let save: Save = {
            let persisted = persisted.clone();
            Arc::new(move |value| {
                let persisted = persisted.clone();
                Box::pin(async move {
                    if value.records[0].delivery == "accepted" {
                        return Err("synthetic acknowledgement save failure".into());
                    }
                    *persisted.lock().await = value;
                    Ok(())
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new(stored, api, save, template.control.clone(), tmp.path().into());
        let incoming = msg("unknown-query-ack", "/状态 missing-original-task");
        assert!(bridge.ingest(&incoming).await.is_err());
        let snapshot = persisted.lock().await.clone();
        assert_eq!(snapshot.records[0].status, "completed");
        assert_eq!(snapshot.records[0].delivery, "unknown");
        assert_eq!(seen.lock().await.len(), 1);
        let id = snapshot.records[0].id.clone();
        let (restored, after) = test_bridge(snapshot, tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        let monitor = tokio::spawn(restored.clone().monitor(0));
        restored.ingest(&incoming).await.unwrap();
        restored.resume_saved_query(&id, 0).await.unwrap();
        assert_eq!(seen.lock().await.len(), 1);
        assert_eq!(after.lock().await.records[0].delivery, "unknown");
        monitor.abort();
        server.abort();
    }

    #[tokio::test]
    async fn wechat_paused_current_query_keeps_the_selected_turn_and_releases_the_lock() {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let stored = account(&base);
        let (template, saved) = test_bridge(
            stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
        );
        let started = Arc::new(Notify::new());
        let gets = Arc::new(AtomicUsize::new(0));
        let resolutions = Arc::new(AtomicUsize::new(0));
        let agent: Agent = {
            let (started, gets, resolutions) = (started.clone(), gets.clone(), resolutions.clone());
            Arc::new(move |request| {
                let (started, gets, resolutions) = (started.clone(), gets.clone(), resolutions.clone());
                Box::pin(async move {
                    match request["operation"].as_str() {
                        Some("notifications") => Ok(json!([])),
                        Some("resolve_current_query") => {
                            assert_eq!(resolutions.fetch_add(1, Ordering::SeqCst), 0);
                            Ok(json!({"target":"original-turn"}))
                        }
                        Some("get") => {
                            assert_eq!(request["turn_id"], "original-turn");
                            assert_eq!(request["include_files"], false);
                            if gets.fetch_add(1, Ordering::SeqCst) == 0 {
                                started.notify_one();
                                return std::future::pending().await;
                            }
                            Ok(json!({"status":"succeeded","result":{"turn_id":"original-turn","text":"原任务已整理好"}}))
                        }
                        _ => panic!("query must not submit or execute work"),
                    }
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new_with_agent(
            stored, api, template.save.clone(), template.control.clone(), root.path().into(), Some(agent),
        );
        let incoming = msg("pause-original-progress", "进度呢");
        let caller = bridge.clone();
        let query_message = incoming.clone();
        let mut call = tokio::spawn(async move { caller.ingest(&query_message).await });
        tokio::time::timeout(Duration::from_secs(3), started.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), &mut call).await;
        if result.is_err() {
            call.abort();
        }
        result.expect("pause must drop the old read and release the query lock").unwrap().unwrap();
        assert!(seen.lock().await.is_empty());
        assert_eq!(saved.lock().await.records[0].pending_query(), Some((QueryTarget::Turn("original-turn"), false)));

        let generation = bridge.generation.load(Ordering::SeqCst);
        bridge.state.lock().await.stored.enabled = true;
        let monitor = tokio::spawn(bridge.clone().monitor(generation));
        tokio::time::timeout(Duration::from_secs(4), async {
            while saved.lock().await.records[0].delivery != "accepted" {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }).await.unwrap();
        bridge.ingest(&incoming).await.unwrap();
        bridge.stop().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), monitor).await.unwrap().unwrap();
        assert_eq!(gets.load(Ordering::SeqCst), 2);
        assert_eq!(resolutions.load(Ordering::SeqCst), 1);
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["body"]["msg"]["item_list"][0]["text_item"]["text"], "原任务已整理好");
        server.abort();
    }

    #[tokio::test]
    async fn wechat_paused_numbered_query_reuses_the_original_control_job() {
        let root = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        stored.records.push(Record {
            id: "original-receipt".into(), command: "capability_render".into(),
            status: "running".into(), job_id: Some("original-control-job".into()),
            ..Default::default()
        });
        let (template, saved) = test_bridge(
            stored.clone(), root.path(), controller(root.path(), Arc::new(AtomicUsize::new(0))),
        );
        let started = Arc::new(Notify::new());
        let reads = Arc::new(AtomicUsize::new(0));
        let control: Control = {
            let (started, reads) = (started.clone(), reads.clone());
            Arc::new(move |request| {
                let (started, reads) = (started.clone(), reads.clone());
                Box::pin(async move {
                    assert_eq!(request.method, "jobs.get");
                    assert_eq!(request.job_id.as_deref(), Some("original-control-job"));
                    if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                        started.notify_one();
                        return std::future::pending().await;
                    }
                    Ok(json!({"status":"succeeded","result":{"status":"succeeded"}}))
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new(stored, api, template.save.clone(), control, root.path().into());
        let incoming = msg("pause-numbered-progress", "/状态 original-receipt");
        let caller = bridge.clone();
        let query_message = incoming.clone();
        let mut call = tokio::spawn(async move { caller.ingest(&query_message).await });
        tokio::time::timeout(Duration::from_secs(3), started.notified()).await.unwrap();
        bridge.stop().await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), &mut call).await;
        if result.is_err() {
            call.abort();
        }
        result.expect("pause must drop the old control read").unwrap().unwrap();
        assert!(seen.lock().await.is_empty());
        assert_eq!(saved.lock().await.records[1].pending_query(), Some((QueryTarget::Numbered("original-receipt"), false)));

        bridge.state.lock().await.stored.enabled = true;
        bridge.ingest(&incoming).await.unwrap();
        let query_id = saved.lock().await.records[1].id.clone();
        bridge.resume_saved_query(&query_id, bridge.generation.load(Ordering::SeqCst)).await.unwrap();
        bridge.stop().await.unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 2);
        let state = saved.lock().await;
        assert_eq!(state.records[0].status, "running");
        assert_eq!(state.records[1].delivery, "accepted");
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 1);
        let text = requests[0]["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap();
        assert!(text.contains("original-receipt") && text.contains("暂时没有可取回的文件"));
        server.abort();
    }

    #[tokio::test]
    async fn wechat_current_progress_pins_original_turn_before_read_and_reuses_after_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let stored = account(&base);
        let (template, saved) = test_bridge(stored.clone(), tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        let read_started = Arc::new(Notify::new());
        let first_agent: Agent = {
            let started = read_started.clone();
            Arc::new(move |request| {
                let started = started.clone();
                Box::pin(async move {
                    if request["operation"] == "resolve_current_query" {
                        return Ok(json!({"target":"original-turn"}));
                    }
                    assert_eq!(request["operation"], "get");
                    started.notify_one();
                    std::future::pending().await
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new_with_agent(stored, api.clone(), template.save.clone(), template.control.clone(), tmp.path().into(), Some(first_agent));
        let incoming = msg("progress-original", "帮我看看刚才那个做到哪一步了？");
        let ingress = {
            let bridge = bridge.clone();
            let incoming = incoming.clone();
            tokio::spawn(async move { bridge.ingest(&incoming).await })
        };
        tokio::time::timeout(Duration::from_secs(3), read_started.notified()).await.unwrap();
        ingress.abort();
        assert!(ingress.await.unwrap_err().is_cancelled());
        let snapshot = saved.lock().await.clone();
        assert_eq!(snapshot.records[0].args["current_query"]["target"], "original-turn");
        assert_eq!(snapshot.records[0].delivery, "pending");

        let restarted_agent: Agent = Arc::new(|request| Box::pin(async move {
            match request["operation"].as_str() {
                Some("notifications") => Ok(json!([])),
                Some("resolve_current_query" | "submit") => panic!("recovery must not select the new current task"),
                Some("get") => {
                    assert_eq!(request["turn_id"], "original-turn");
                    assert_eq!(request["include_files"], false);
                    Ok(json!({"status":"succeeded","result":{"turn_id":"original-turn","text":"original-progress"}}))
                }
                _ => panic!("unexpected operation"),
            }
        }));
        let restored = Bridge::new_with_agent(snapshot, api, template.save.clone(), template.control.clone(), tmp.path().into(), Some(restarted_agent));
        let monitor = tokio::spawn(restored.clone().monitor(0));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while saved.lock().await.records[0].delivery != "accepted" {
            assert!(tokio::time::Instant::now() < deadline, "original progress was not recovered");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        restored.ingest(&incoming).await.unwrap();
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["body"]["msg"]["item_list"][0]["text_item"]["text"], "original-progress");
        drop(requests);
        monitor.abort();
        server.abort();
    }
    #[tokio::test]
    async fn wechat_quoted_outbound_text_and_attachment_read_original_task_after_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({"__media":true,"__send_message_ids":{
            "original-client":"9007199254740994", "original-client-file-0":9007199254740995_u64
        }})).await;
        let mut stored = account(&base);
        stored.records.push(Record {
            id: "original-receipt".into(), client_id: "original-client".into(),
            command: "assistant".into(), status: "succeeded".into(),
            job_id: Some("original-turn".into()), context: "synthetic-context".into(), ..Default::default()
        });
        let (bridge, saved) = test_bridge(stored, tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        let path = tmp.path().join("quoted-output.txt");
        let bytes = b"verified original output";
        std::fs::write(&path, bytes).unwrap();
        let artifact = json!({"path":path,"sha256":format!("{:x}",Sha256::digest(bytes))});
        bridge.reply("original-receipt", "旧任务的原回复", Some(&artifact)).await.unwrap();
        let mut snapshot = saved.lock().await.clone();
        assert_eq!(snapshot.records[0].delivery, "accepted");
        snapshot.records.push(Record {
            id: "new-receipt".into(), command: "assistant".into(), status: "succeeded".into(),
            job_id: Some("new-turn".into()), ..Default::default()
        });
        let encoded = serde_json::to_vec(&snapshot).unwrap();
        for id in ["9007199254740994", "9007199254740995"] {
            assert!(!String::from_utf8_lossy(&encoded).contains(id));
        }
        let snapshot = serde_json::from_slice(&encoded).unwrap();
        let confirmation = Arc::new(Mutex::new(Vec::new()));
        let agent: Agent = Arc::new({
            let confirmation = confirmation.clone();
            move |request| {
                let confirmation = confirmation.clone();
                let artifact = artifact.clone();
                Box::pin(async move {
                    assert_eq!(request["operation"], "get");
                    assert_eq!(request["turn_id"], "original-turn");
                    confirmation.lock().await.push(request["include_files"].as_bool().unwrap());
                    Ok(json!({"status":"succeeded","result":{"turn_id":"original-turn","text":"从原任务读取的结果","artifacts":[artifact]}}))
                })
            }
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let restored = Bridge::new_with_agent(snapshot, api, bridge.save.clone(), bridge.control.clone(), tmp.path().into(), Some(agent));
        for (message, quote, text) in [
            ("quote-outbound-text", json!({"svr_id":"9007199254740994"}), "刚才那件事做到哪了？"),
            ("quote-outbound-file", json!({"message_item":{"msg_id":"9007199254740995"}}), "把刚才的结果发过来。"),
        ] {
            let mut inbound = msg(message, text);
            inbound["item_list"][0]["ref_msg"] = quote;
            restored.ingest(&inbound).await.unwrap();
            restored.ingest(&inbound).await.unwrap();
        }
        assert_eq!(*confirmation.lock().await, vec![false, true]);
        let calls = seen.lock().await;
        assert_eq!(calls.len(), 9);
        assert_eq!(calls[4]["body"]["msg"]["item_list"][0]["text_item"]["text"], "从原任务读取的结果");
        assert_eq!(calls[8]["body"]["msg"]["item_list"][0]["type"], 4);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_quoted_outbound_query_reply_keeps_the_saved_turn_without_reselecting() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({"__send_message_ids":{"old-status-reply":"103"}})).await;
        let mut stored = account(&base);
        stored.records.push(Record {
            id: "old-query".into(), command: "message".into(), status: "received".into(),
            delivery: "pending".into(), context: "synthetic-context".into(), client_id: "old-status-reply".into(),
            args: json!({"current_query":{"target":"known-old-turn","files":false}}), ..Default::default()
        });
        let (bridge, persisted) = test_bridge(stored, tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        bridge.reply("old-query", "旧任务进度", None).await.unwrap();
        let snapshot = persisted.lock().await.clone();
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let agent: Agent = Arc::new(|request| Box::pin(async move {
            assert_eq!(request["operation"], "get");
            assert_eq!(request["turn_id"], "known-old-turn");
            assert_eq!(request["include_files"], true);
            Ok(json!({"status":"succeeded","result":{"turn_id":"known-old-turn","text":"复用旧查询的目标。"}}))
        }));
        let restored = Bridge::new_with_agent(snapshot, api, bridge.save.clone(), bridge.control.clone(), tmp.path().into(), Some(agent));
        let mut quoted = msg("quote-old-query-response", "结果呢？");
        quoted["item_list"][0]["ref_msg"] = json!({"svr_id":"103"});
        restored.ingest(&quoted).await.unwrap();
        assert_eq!(persisted.lock().await.records[1].query(), Some((QueryTarget::Turn("known-old-turn"), true)));
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["body"]["msg"]["item_list"][0]["text_item"]["text"], "复用旧查询的目标。");
        server.abort();
    }

    #[tokio::test]
    async fn wechat_quoted_result_reopens_original_task_and_delivers_attachment_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
        let mut stored = account(&base);
        let mut original_msg = msg("original-envelope", "合成旧任务");
        original_msg["item_list"][0]["msg_id"] = json!("9007199254740994");
        let original = InboundIdentity::from_message(&stored, &original_msg).unwrap();
        let original_id = original.receipt.clone();
        stored.records.push(Record {
            id: original.receipt, inbound_keys: original.keys, command: "assistant".into(),
            status: "succeeded".into(), job_id: Some("turn-before-new-chat".into()), ..Default::default()
        });
        stored.records.push(Record {
            id: "new-current-task".into(), command: "assistant".into(), status: "succeeded".into(),
            job_id: Some("current-turn".into()), ..Default::default()
        });
        let (template, saved) = test_bridge(stored.clone(), tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        let read_started = Arc::new(Notify::new());
        let first_agent: Agent = {
            let started = read_started.clone();
            Arc::new(move |_| {
                let started = started.clone();
                Box::pin(async move {
                    started.notify_one();
                    std::future::pending().await
                })
            })
        };
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new_with_agent(stored, api.clone(), template.save.clone(), template.control.clone(), tmp.path().into(), Some(first_agent));
        let mut inbound = msg("quoted-old-result", "结果呢？");
        inbound["item_list"][0]["ref_msg"] = json!({"svr_id":"9007199254740994"});
        let ingress = {
            let bridge = bridge.clone();
            let inbound = inbound.clone();
            tokio::spawn(async move { bridge.ingest(&inbound).await })
        };
        tokio::time::timeout(Duration::from_secs(3), read_started.notified()).await.unwrap();
        ingress.abort();
        assert!(ingress.await.unwrap_err().is_cancelled());
        let snapshot = saved.lock().await.clone();
        assert_eq!(snapshot.records[2].pending_query(), Some((QueryTarget::Numbered(&original_id), true)));
        assert!(seen.lock().await.is_empty());

        let path = tmp.path().join("quoted-original.txt");
        let text = b"old result after a new conversation";
        std::fs::write(&path, text).unwrap();
        let artifact = json!({"path":path,"sha256":format!("{:x}", Sha256::digest(text))});
        let reads = Arc::new(AtomicUsize::new(0));
        let restarted: Agent = Arc::new({
            let reads = reads.clone();
            move |request| {
                let artifact = artifact.clone();
                let reads = reads.clone();
                Box::pin(async move {
                    assert_eq!(request["operation"], "get");
                    assert_eq!(request["turn_id"], "turn-before-new-chat");
                    assert_eq!(request["include_files"], true);
                    reads.fetch_add(1, Ordering::SeqCst);
                    Ok(json!({"status":"succeeded","result":{"turn_id":"turn-before-new-chat","text":"原任务已完成。","artifacts":[artifact]}}))
                })
            }
        });
        let query_id = snapshot.records[2].id.clone();
        let restored = Bridge::new_with_agent(snapshot, api, template.save.clone(), template.control.clone(), tmp.path().into(), Some(restarted));
        restored.resume_saved_query(&query_id, 0).await.unwrap();
        restored.ingest(&inbound).await.unwrap();
        restored.resume_saved_query(&query_id, 0).await.unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        {
            let saved = saved.lock().await;
            assert_eq!(saved.records[2].delivery, "accepted", "{:?}", saved.records[2].delivery_error);
        }
        let messages = seen.lock().await;
        assert_eq!(messages.len(), 4, "one text and one uploaded attachment");
        assert_eq!(messages[0]["body"]["msg"]["item_list"][0]["text_item"]["text"], "原任务已完成。");
        assert_eq!(messages[3]["body"]["msg"]["item_list"][0]["type"], 4);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_unknown_or_conflicting_quote_never_selects_the_current_task() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        for number in ["21", "22"] {
            stored.records.push(Record {
                id: identity::legacy_receipt_id(&stored, number), command: "assistant".into(),
                status: "succeeded".into(), job_id: Some(format!("original-{number}")), ..Default::default()
            });
        }
        let (template, _) = test_bridge(stored.clone(), tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let agent: Agent = Arc::new(|_| Box::pin(async { panic!("an invalid quote must not reach the shared agent") }));
        let bridge = Bridge::new_with_agent(stored, api, template.save.clone(), template.control.clone(), tmp.path().into(), Some(agent));
        for (id, quote, reason) in [
            ("unknown-quote", json!({"svr_id":"23"}), "没找到这条引用对应的原任务"),
            ("conflicting-quote", json!({"svr_id":"21", "message_item":{"msg_id":"22"}}), "引用对应多个任务"),
        ] {
            let mut inbound = msg(id, "好了吗？");
            inbound["item_list"][0]["ref_msg"] = quote;
            bridge.ingest(&inbound).await.unwrap();
            bridge.ingest(&inbound).await.unwrap();
            let messages = seen.lock().await;
            assert!(messages.last().unwrap()["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap().contains(reason));
        }
        assert_eq!(seen.lock().await.len(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_current_result_keeps_resolved_empty_target_after_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let (base, seen, server) = fake_api(200, json!({})).await;
        let mut stored = account(&base);
        stored.records.push(Record {
            id: "empty-query".into(), command: "message".into(), status: "received".into(),
            delivery: "pending".into(), client_id: "empty-reply".into(),
            context: "synthetic-context".into(), args: json!({"current_query":{"target":null,"files":true}}),
            ..Default::default()
        });
        let (bridge, saved) = test_bridge(stored, tmp.path(), controller(tmp.path(), Arc::new(AtomicUsize::new(0))));
        // Deliberately omit the agent. A resolved empty query must not ask it to select again.
        bridge.resume_saved_query("empty-query", 0).await.unwrap();
        bridge.resume_saved_query("empty-query", 0).await.unwrap();
        assert_eq!(saved.lock().await.records[0].delivery, "accepted");
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(requests[0]["body"]["msg"]["item_list"][0]["text_item"]["text"].as_str().unwrap().contains("没有关联到任务"));
        drop(requests);
        server.abort();
    }

    #[tokio::test]
    async fn wechat_storage_failure_and_ambiguous_reply_do_not_reexecute() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count.clone());
        let (base, _, server) = fake_api(500, json!({"errmsg":"synthetic-token"})).await;
        let (bridge, _) = test_bridge(account(&base), tmp.path(), ctl.clone());
        bridge.ingest(&msg("1", "系统")).await.unwrap();
        bridge.ingest(&msg("1", "系统")).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(bridge.status().await.tasks[0].delivery, "unknown");
        let save: Save = Arc::new(|_| Box::pin(async { Err("synthetic disk failure".into()) }));
        let control: Control = Arc::new(move |r| {
            let c = ctl.clone();
            Box::pin(async move { c.handle(r).await })
        });
        let blocked = Bridge::new(
            account(&base),
            Api::new().unwrap(),
            save,
            control,
            tmp.path().into(),
        );
        assert!(blocked.ingest(&msg("2", "系统")).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        server.abort();
    }
    #[test]
    fn wechat_artifacts_cannot_read_existing_private_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("workspace");
        std::fs::create_dir(&root).unwrap();
        let outside = tmp.path().join("real-secret.txt");
        std::fs::write(&outside, "synthetic private").unwrap();
        assert!(artifact_bytes(&outside, &root).is_err());
        let file = root.join("output.mp4");
        std::fs::write(&file, "synthetic media").unwrap();
        assert_eq!(artifact_bytes(&file, &root).unwrap(), b"synthetic media");
        let exe = root.join("tool.exe");
        std::fs::write(&exe, "synthetic executable").unwrap();
        assert!(artifact_bytes(&exe, &root).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn wechat_dpapi_roundtrip_keeps_real_keys_out_of_storage() {
        let plain = b"synthetic-only-token-context-and-cursor";
        let cipher = crate::wechat_crypto::crypt(plain, true).unwrap();
        assert!(!cipher.windows(9).any(|w| w == b"synthetic"));
        assert_eq!(
            &*crate::wechat_crypto::crypt(&cipher, false).unwrap(),
            plain
        );
        assert!(crate::wechat_crypto::crypt(b"broken", false).is_err());
    }
    #[tokio::test]
    async fn wechat_qr_pin_confirmation_receive_stop_and_restart_use_fake_service_only() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count);
        let (base, seen, server) = fake_api(200, json!({"__pairing":true})).await;
        let memory = Arc::new(Mutex::new(Stored::default()));
        let dest = memory.clone();
        let save: Save = Arc::new(move |s| {
            let dest = dest.clone();
            Box::pin(async move {
                *dest.lock().await = s;
                Ok(())
            })
        });
        let control: Control = Arc::new(move |r| {
            let ctl = ctl.clone();
            Box::pin(async move { ctl.handle(r).await })
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base);
        let bridge = Bridge::new(Stored::default(), api, save, control, tmp.path().into());
        let (a, b) = tokio::join!(bridge.begin(), bridge.begin());
        a.unwrap();
        b.unwrap();
        assert!(bridge
            .pairing()
            .await
            .qr_data_url
            .unwrap()
            .starts_with("data:image/svg+xml;base64,"));
        for _ in 0..50 {
            if bridge.pairing().await.state == "need_pin" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(bridge.pairing().await.state, "need_pin");
        assert!(bridge.pin("bad".into()).await.is_err());
        bridge.pin("123456".into()).await.unwrap();
        for _ in 0..180 {
            if bridge.status().await.state == "connected" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(bridge.status().await.bound);
        assert_eq!(bridge.status().await.state, "connected");
        assert!(bridge.pairing().await.qr_data_url.is_none());
        bridge.stop().await.unwrap();
        assert!(!bridge.status().await.enabled);
        bridge.start().await.unwrap();
        bridge.stop().await.unwrap();
        assert!(memory.lock().await.token == "synthetic-token");
        let requests = seen.lock().await;
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["head"].as_str().unwrap().contains("/get_bot_qrcode?"))
                .count(),
            1
        );
        let qr = requests
            .iter()
            .find(|r| r["head"].as_str().unwrap().contains("/get_bot_qrcode?"))
            .unwrap();
        assert_eq!(qr["body"], json!({"local_token_list":[]}));
        assert!(!qr["head"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization:"));
        drop(requests);
        bridge.forget().await.unwrap();
        assert!(!bridge.status().await.bound);
        assert!(memory.lock().await.token.is_empty());
        server.abort();
    }
    #[tokio::test]
    async fn wechat_generated_file_upload_is_encrypted_and_has_no_bot_auth_on_cdn() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("output.mp4");
        std::fs::write(&file, b"synthetic-video-bytes").unwrap();
        let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        api.file(
            &account(&base),
            "synthetic-context",
            "stable-file-client",
            &file,
            tmp.path(),
            &format!("{:x}", Sha256::digest(b"synthetic-video-bytes")),
        )
        .await
        .unwrap();
        assert!(api
            .file(
                &account(&base),
                "synthetic-context",
                "tampered",
                &file,
                tmp.path(),
                &"0".repeat(64)
            )
            .await
            .is_err());
        let requests = seen.lock().await;
        assert_eq!(requests.len(), 3);
        let params = &requests[0]["body"];
        assert_eq!(params["media_type"], 3);
        assert_eq!(params["no_need_thumb"], true);
        assert_eq!(params["rawsize"], 21);
        assert_eq!(params["filesize"], 32);
        let upload = &requests[1];
        assert!(!upload["head"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization:"));
        let encrypted = STANDARD
            .decode(upload["raw_base64"].as_str().unwrap())
            .unwrap();
        assert_ne!(encrypted, b"synthetic-video-bytes");
        use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyInit};
        let hex = params["aeskey"].as_str().unwrap();
        let key = (0..32)
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect::<Vec<_>>();
        let plain = ecb::Decryptor::<aes::Aes128>::new_from_slice(&key)
            .unwrap()
            .decrypt_padded_vec_mut::<Pkcs7>(&encrypted)
            .unwrap();
        assert_eq!(plain, b"synthetic-video-bytes");
        let item = &requests[2]["body"]["msg"]["item_list"][0];
        assert_eq!(item["type"], 4);
        assert_eq!(item["file_item"]["file_name"], "output.mp4");
        assert_eq!(
            item["file_item"]["media"]["encrypt_query_param"],
            "synthetic-download"
        );
        server.abort();
    }

    #[tokio::test]
    async fn wechat_send_uses_official_default_success_but_rejects_errors_and_nonobjects() {
        for body in [
            json!({}),
            json!({"ret":0}),
            json!({"message_id":"synthetic-ack"}),
        ] {
            let (base, _, server) = fake_api(200, body).await;
            Api::new()
                .unwrap()
                .text(
                    &account(&base),
                    "synthetic-context",
                    "synthetic-client",
                    "test",
                )
                .await
                .unwrap();
            server.abort();
        }
        for body in [
            Value::Null,
            json!([]),
            json!({"ret":1,"errmsg":"synthetic-secret"}),
            json!({"errcode":-14}),
            json!({"ret":"0"}),
        ] {
            let (base, _, server) = fake_api(200, body).await;
            let error = Api::new()
                .unwrap()
                .text(
                    &account(&base),
                    "synthetic-context",
                    "synthetic-client",
                    "test",
                )
                .await
                .unwrap_err();
            assert!(!error.contains("synthetic-secret"));
            server.abort();
        }
    }

    #[tokio::test]
    async fn wechat_result_uploads_attachment_when_text_and_file_acks_omit_ret() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("output.mp4");
        let bytes = b"synthetic-isolated-video";
        std::fs::write(&file, bytes).unwrap();
        let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
        let mut stored = account(&base);
        stored.records.push(Record{id:"wx-synthetic-video".into(),command:"capability_render".into(),status:"succeeded".into(),result:Some(json!({"status":"succeeded","artifact":{"path":file,"sha256":format!("{:x}",Sha256::digest(bytes)),"bytes":bytes.len()}})),..Record::default()});
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count.clone());
        let (bridge, _) = test_bridge(stored, tmp.path(), ctl);
        bridge
            .ingest(&msg("explicit-result", "/结果 wx-synthetic-video"))
            .await
            .unwrap();
        assert_eq!(bridge.status().await.tasks[0].delivery, "accepted");
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let calls = seen.lock().await;
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0]["body"]["msg"]["item_list"][0]["type"], 1);
        assert!(calls[1]["head"]
            .as_str()
            .unwrap()
            .contains("/getuploadurl "));
        assert_eq!(calls[3]["body"]["msg"]["item_list"][0]["type"], 4);
        server.abort();
    }
    #[tokio::test]
    async fn wechat_partial_file_delivery_keeps_accepted_siblings_and_retries_only_the_missing_one() {
        for ambiguous in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let first = tmp.path().join("first.mp4");
            let second = tmp.path().join("second.mp4");
            let bytes: &[u8] = b"synthetic first";
            std::fs::write(&first, if ambiguous { bytes } else { b"changed bytes" }).unwrap();
            std::fs::write(&second, b"synthetic second").unwrap();
            let (base, seen, server) = fake_api(200, json!({"__media":true,"__drop_first_file_ack":ambiguous})).await;
            let outcome = json!({"turn_id":"turn-partial","status":"succeeded","text":"已生成两份文件。","artifacts":[
                {"path":first,"sha256":format!("{:x}",Sha256::digest(bytes))},
                {"path":second,"sha256":format!("{:x}",Sha256::digest(b"synthetic second"))}
            ]});
            let value = json!({"status":"succeeded","result":outcome});
            let mut stored = account(&base);
            stored.records.push(Record {
                id:"origin".into(), command:"assistant".into(), job_id:Some("turn-partial".into()),
                status:"succeeded".into(), result:Some(outcome), delivery:"pending".into(),
                context:"synthetic-context".into(), client_id:"origin-client".into(),
                ..Record::default()
            });
            for (id, resend) in [("ordinary",false),("explicit",true)] {
                stored.records.push(Record {
                    id:id.into(), command:"message".into(), status:"received".into(),
                    args:json!({"current_query":{"target":"turn-partial","files":true},"resend_attachment":resend}),
                    delivery:"pending".into(), context:"synthetic-context".into(), client_id:id.into(),
                    ..Record::default()
                });
            }
            let count = Arc::new(AtomicUsize::new(0));
            let ctl = controller(tmp.path(), count.clone());
            let (bridge, saved) = test_bridge(stored, tmp.path(), ctl);
            bridge.reply_assistant_for("origin", &value, true, None, true).await.unwrap();
            let original = saved.lock().await.records[0].clone();
            let files = value["result"]["artifacts"].as_array().unwrap();
            assert_eq!(original.file_outcome(&files[1], files), Some(file_delivery::Outcome::Accepted));
            assert_eq!(original.file_delivery_view().0, if ambiguous {"attachment_unknown"} else {"attachment_failed"});
            std::fs::write(&first, bytes).unwrap();
            bridge.reply_assistant_for("ordinary", &value, true, None, false).await.unwrap();
            if ambiguous {
                bridge.reply_assistant_for("explicit", &value, true, None, false).await.unwrap();
            }
            let settled = saved.lock().await.records[0].clone();
            assert_eq!(settled.file_delivery_view().0, "accepted");
            let calls = seen.lock().await;
            let sent: Vec<&str> = calls.iter().filter_map(|r| r["body"]["msg"]["item_list"][0]["file_item"]["file_name"].as_str()).collect();
            assert_eq!(sent, if ambiguous { vec!["first.mp4","second.mp4","first.mp4"] } else { vec!["second.mp4","first.mp4"] });
            assert_eq!(count.load(Ordering::SeqCst), 0);
            server.abort();
        }
    }
    #[tokio::test]
    async fn wechat_receive_http_advances_cursor_after_durable_acceptance() {
        let tmp = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let ctl = controller(tmp.path(), count.clone());
        let (base, seen, server) = fake_api(
            200,
            json!({"ret":0,"__incoming":[msg("http-inbound","系统")]}),
        )
        .await;
        let (bridge, memory) = test_bridge(account(&base), tmp.path(), ctl);
        bridge.start().await.unwrap();
        for _ in 0..100 {
            if memory.lock().await.cursor == "synthetic-cursor" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(memory.lock().await.cursor, "synthetic-cursor");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(bridge.status().await.tasks[0].job_id.is_some());
        bridge.stop().await.unwrap();
        assert!(seen
            .lock()
            .await
            .iter()
            .any(|r| r["head"].as_str().unwrap().contains("/sendmessage ")));
        server.abort();
    }
    #[tokio::test]
    #[ignore = "explicit isolated installation/render; never uses real WeChat or model keys"]
    async fn wechat_real_video_end_to_end_with_isolated_runtime() {
        use std::process::Stdio;
        let parent = std::env::var_os("HOMEBASE_WECHAT_LAB_ROOT")
            .expect("set an explicit isolated lab root");
        std::fs::create_dir_all(&parent).unwrap();
        let tmp = tempfile::Builder::new()
            .prefix("wechat-video-")
            .tempdir_in(parent)
            .unwrap();
        let root = tmp.path().to_path_buf();
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("scripts/capability-runtime.mjs");
        let runner: Runner = Arc::new(move |name, args| {
            let script = script.clone();
            Box::pin(async move {
                let (action, input) = match name.as_str() {
                    "capability_install" => ("install", json!({"root_path":args["rootPath"]})),
                    "capability_render" => (
                        "render",
                        json!({"root_path":args["rootPath"],"input":args["input"]}),
                    ),
                    _ => {
                        return Err(ControlError::new(
                            "TEST_BLOCKED",
                            "only isolated video allowed",
                        ))
                    }
                };
                let mut cmd = tokio::process::Command::new("node");
                cmd.arg(script).arg(action).env_clear();
                for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATH", "PATHEXT"] {
                    if let Some(v) = std::env::var_os(name) {
                        cmd.env(name, v);
                    }
                }
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                #[cfg(windows)]
                cmd.creation_flags(0x08000000);
                let mut child = cmd
                    .spawn()
                    .map_err(|_| ControlError::new("SPAWN", "isolated runtime unavailable"))?;
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(&serde_json::to_vec(&input).unwrap())
                    .await
                    .unwrap();
                let out = tokio::time::timeout(Duration::from_secs(1200), child.wait_with_output())
                    .await
                    .map_err(|_| ControlError::new("TIMEOUT", "isolated runtime timeout"))?
                    .unwrap();
                let value: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
                    ControlError::new(
                        "JSON",
                        &format!(
                            "isolated runtime result invalid, code={:?}, stderr={}",
                            out.status.code(),
                            clip(&String::from_utf8_lossy(&out.stderr), 800)
                        ),
                    )
                })?;
                Ok(value)
            })
        });
        let catalog = serde_json::from_str(include_str!("../../control/commands.json")).unwrap();
        let ctl = Controller::new(catalog, &root, runner).unwrap();
        let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
        let output = Arc::new(Mutex::new(account(&base)));
        let dest = output.clone();
        let save: Save = Arc::new(move |s| {
            let dest = dest.clone();
            Box::pin(async move {
                *dest.lock().await = s;
                Ok(())
            })
        });
        let control: Control = Arc::new(move |r| {
            let ctl = ctl.clone();
            Box::pin(async move { ctl.handle(r).await })
        });
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let bridge = Bridge::new(account(&base), api, save, control, root.join("workspace"));
        bridge.start().await.unwrap();
        async fn wait(bridge: &Arc<Bridge>, id: &str) {
            let start = tokio::time::Instant::now();
            loop {
                let status = bridge
                    .state
                    .lock()
                    .await
                    .stored
                    .records
                    .iter()
                    .find(|r| r.id == id)
                    .unwrap()
                    .status
                    .clone();
                if !["received", "running"].contains(&status.as_str()) {
                    let job = bridge
                        .state
                        .lock()
                        .await
                        .stored
                        .records
                        .iter()
                        .find(|r| r.id == id)
                        .unwrap()
                        .job_id
                        .clone()
                        .unwrap();
                    let detail = bridge
                        .request("jobs.get", None, json!({}), id, Some(&job))
                        .await
                        .unwrap();
                    assert_eq!(status, "succeeded", "{detail}");
                    return;
                }
                assert!(start.elapsed() < Duration::from_secs(1200));
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        bridge.ingest(&msg("install", "安装视频")).await.unwrap();
        let install = bridge.status().await.tasks[0].id.clone();
        wait(&bridge, &install).await;
        {
            let s = bridge.state.lock().await;
            let payload = s
                .stored
                .records
                .iter()
                .find(|r| r.id == install)
                .unwrap()
                .result
                .as_ref()
                .unwrap();
            assert_ne!(payload["status"], "failed", "{payload}");
        }
        bridge
            .ingest(&msg("render", "视频 微信隔离链路验证"))
            .await
            .unwrap();
        let render = bridge.status().await.tasks[0].id.clone();
        wait(&bridge, &render).await;
        let result = bridge
            .state
            .lock()
            .await
            .stored
            .records
            .iter()
            .find(|r| r.id == render)
            .unwrap()
            .result
            .clone()
            .unwrap();
        assert_eq!(result["status"], "succeeded", "{result}");
        let artifact = artifact_path(&result).unwrap();
        let bytes = artifact_bytes(&artifact, &root.join("workspace")).unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes)),
            result["artifact"]["sha256"]
        );
        bridge
            .ingest(&msg("status", &format!("/状态 {render}")))
            .await
            .unwrap();
        bridge
            .ingest(&msg("result", &format!("/结果 {render}")))
            .await
            .unwrap();
        assert_eq!(bridge.status().await.tasks[0].delivery, "accepted");
        let calls = seen.lock().await;
        let upload = calls
            .iter()
            .find(|r| r["head"].as_str().unwrap().contains("/c2c/upload?"))
            .expect("encrypted generated file uploaded to loopback");
        assert!(!upload["head"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization:"));
        assert!(calls
            .iter()
            .any(|r| r["body"]["msg"]["item_list"][0]["type"] == 4));
        drop(calls);
        bridge.stop().await.unwrap();
        server.abort();
        let report = json!({"lab":root,"installation_task":install,"render_task":render,"result":result,"wechat":"synthetic loopback only","real_model_calls":0,"real_wechat_messages":0});
        std::fs::write(
            root.join("validation.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        // Run the shared assistant through natural-language ingest, discovery, render,
        // status/result queries and encrypted media delivery using the same isolated runtime.
        let phases = Arc::new(AtomicUsize::new(0));
        let model_count = phases.clone();
        let model: crate::assistant::Model = Arc::new(move |request| {
            let phase = model_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                let (name, args) = match phase {
                    0 => ("homebase_discover", json!({"query":"capability"})),
                    1 => ("homebase_describe", json!({"command":"capability_render"})),
                    2 => (
                        "homebase_video",
                        json!({"title":"HomeBase 共享助理隔离验证","seconds":6}),
                    ),
                    3 => {
                        let tool = request
                            .messages
                            .iter()
                            .rev()
                            .find(|m| m["role"] == "tool")
                            .ok_or("fixture missing tool")?;
                        let value: Value =
                            serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
                        ("homebase_deliver", json!({"job_id":value["job_id"]}))
                    }
                    _ => {
                        return crate::assistant_protocol::parse(
                            "chat",
                            &json!({"choices":[{"message":{"content":"隔离代码动画已生成，正在返回已校验的附件。"}}]}),
                        )
                    }
                };
                crate::assistant_protocol::parse(
                    "chat",
                    &json!({"choices":[{"message":{"tool_calls":[{"id":format!("fixture-{phase}"),"function":{"name":name,"arguments":args.to_string()}}]}}]}),
                )
            })
        });
        let mut assistant_state = crate::assistant::State::default();
        assistant_state.settings.enabled = true;
        assistant_state.settings.route_id = "synthetic-no-key".into();
        assistant_state.settings.runtime = "pi".into();
        let core = crate::assistant::Engine::new(
            assistant_state,
            Arc::new(|_| Box::pin(async { Ok(()) })),
            model,
            bridge.control.clone(),
            root.join("workspace"),
        )
        .await
        .unwrap();
        let engine = core.clone();
        let (base, seen, server) = fake_api(200, json!({"__media":true})).await;
        let mut api = Api::new().unwrap();
        api.test_origin = Some(base.clone());
        let shared = Bridge::new_with_agent(
            account(&base),
            api,
            bridge.save.clone(),
            bridge.control.clone(),
            root.join("workspace"),
            Some(Arc::new(move |r| {
                let e = engine.clone();
                Box::pin(async move { e.channel(r).await })
            })),
        );
        shared.start().await.unwrap();
        shared
            .ingest(&msg("assistant-video", "请生成六秒动画并把文件发给我"))
            .await
            .unwrap();
        let turn_id = shared.status().await.tasks[0].job_id.clone().unwrap();
        let turn = tokio::time::timeout(Duration::from_secs(1200), async {
            loop {
                let t = core.turn(&turn_id).await.unwrap();
                if !["queued", "running", "waiting_jobs"].contains(&t.state.as_str()) {
                    break t;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(turn.state, "completed", "{}", turn.text);
        assert_eq!(turn.artifacts.len(), 1);
        assert_eq!(phases.load(Ordering::SeqCst), 5);
        for _ in 0..30 {
            if seen
                .lock()
                .await
                .iter()
                .any(|r| r["body"]["msg"]["item_list"][0]["type"] == 4)
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        shared
            .ingest(&msg("assistant-status", &format!("/状态 {turn_id}")))
            .await
            .unwrap();
        shared
            .ingest(&msg("assistant-result", &format!("/结果 {turn_id}")))
            .await
            .unwrap();
        assert_eq!(shared.status().await.tasks[0].delivery, "accepted");
        assert!(
            seen.lock()
                .await
                .iter()
                .filter(|r| r["body"]["msg"]["item_list"][0]["type"] == 4)
                .count()
                >= 2
        );
        assert_eq!(phases.load(Ordering::SeqCst), 5);
        shared.stop().await.unwrap();
        server.abort();
        std::fs::write(root.join("assistant-validation.json"),serde_json::to_vec_pretty(&json!({"lab":root,"turn":turn,"real_model_calls":0,"real_wechat_messages":0,"synthetic_model_rounds":5,"status_result_repeated_generation":false,"automatic_and_explicit_attachment_delivery":true})).unwrap()).unwrap();
        println!(
            "Isolated WeChat video evidence: {}",
            root.join("validation.json").display()
        );
        let _ = tmp.keep();
    }
}
