//! Same-owner authorization replacement, preserving the original logical channel.
use super::*;

impl Stored {
    pub(super) fn logical_key(&self) -> String {
        if self.channel_key.is_empty() {
            format!(
                "{:x}",
                Sha256::digest(format!("{}:{}", self.bot, self.owner))
            )
        } else {
            self.channel_key.clone()
        }
    }
}
impl Bridge {
    pub(super) async fn commit_pairing(
        &self,
        generation: u64,
        base: String,
        token: &str,
        bot: &str,
        owner: &str,
    ) -> Result<(), String> {
        let _submit = self.submit_lock.lock().await;
        let mut state = self.state.lock().await;
        if self.generation.load(Ordering::SeqCst) != generation
            || !state
                .pairing
                .as_ref()
                .is_some_and(|p| p.generation == generation)
        {
            return Err("扫码已取消或被替换，未保存迟到的授权".into());
        }
        if !state.stored.token.is_empty() && state.stored.owner != owner {
            return Err("扫码微信与原绑定本人不一致，未替换授权；原绑定与任务已保留".into());
        }
        let mut next = state.stored.clone();
        next.channel_key = if next.token.is_empty() {
            format!("{:x}", Sha256::digest(format!("{bot}:{owner}")))
        } else {
            next.logical_key()
        };
        next.enabled = false;
        next.token = token.into();
        next.bot = bot.into();
        next.owner = owner.into();
        next.base = base;
        next.cursor.clear();
        next.reauth_required = false;
        // Old task results remain queryable, but their old reply context is not
        // evidence that this new authorization can deliver proactive messages.
        next.reply_context_pending = true;
        let source = next.logical_key();
        for record in &mut next.records {
            if (record.local_media_kind().is_some() && record.status == "running")
                || record.watches_submitted_work(&source)
            {
                record.args["delivery_allowed"] = json!(false);
            }
            let pending_reference = record.status == "received"
                && (record.command == "wechat_pdf"
                    || record.command == "assistant"
                        && (record.args["pdf_receipt"].is_string()
                            || record.args["video_receipt"].is_string()
                            || record.args["voice_receipt"].is_string()
                            || record.args["repository_install"].is_object()));
            if record.pending_completion() {
                record.mark_old_reply_unsent();
            } else if record.pending_query().is_some() || pending_reference {
                record.status = "canceled".into();
                record.delivery = "not_sent".into();
                record.delivery_error = Some("微信授权已更新，旧查询没有发送。请用原任务编号再查一次。".into());
            }
        }
        (self.save)(next.clone())
            .await
            .map_err(|_| "新授权加密保存失败，未替换原绑定或任务".to_string())?;
        state.stored = next;
        state.pairing = Some(Pairing {
            code: String::new(),
            base: String::new(),
            pin: None,
            generation,
            view: PairingView {
                state: "confirmed".into(),
                message: "绑定成功，正在连接微信；原任务保留，首次收到新消息后恢复主动通知".into(),
                qr_data_url: None,
            },
        });
        Ok(())
    }
    pub(super) async fn mark_stale(&self, generation: u64) {
        let mut state = self.state.lock().await;
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cancel.notify_waiters();
        let source = state.stored.logical_key();
        let mut next = state.stored.clone();
        next.enabled = false;
        next.reauth_required = true;
        match (self.save)(next.clone()).await {
            Ok(()) => {
                state.stored = next;
                state.state = "expired".into();
                state.message =
                    "微信授权已失效，请重新扫码；原绑定与任务保留，不会重提或重发".into();
            }
            Err(_) => {
                state.state = "error".into();
                state.message =
                    "微信授权已失效且停接收状态保存失败，本次接收已停止；请核对存储，原账本未覆盖"
                        .into();
            }
        }
        drop(state);
        if let Some(agent) = &self.agent {
            let _ =
                agent(json!({"operation":"notifications","source_key":source,"connected":false}))
                    .await;
        }
    }
}
