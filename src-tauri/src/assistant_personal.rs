//! Versioned personal memory and reminder state. Persisted only through the host's encrypted Save.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(test)]
#[path = "assistant_personal_tests.rs"]
mod tests;
#[path = "assistant_memory_search.rs"]
mod search;
#[path = "assistant_memory_view.rs"]
mod view;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Personal {
    pub revision: u64,
    pub memories: Vec<Memory>,
    pub reminders: Vec<Reminder>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub title: String,
    pub content: String,
    pub expires_at: Option<String>,
    pub updated_at: String,
    #[serde(default)]
    pub expiry_notified: bool,
    #[serde(default)]
    pub previous: Vec<MemoryRevision>,
    #[serde(default)]
    pub origin: Option<MemoryOrigin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<MemoryCorrection>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryRevision {
    pub content: String,
    pub expires_at: Option<String>,
    pub updated_at: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryOrigin {
    pub source: String,
    pub turn_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MemoryCorrection {
    pub reason: String,
    pub previous_title: String,
    pub previous_content: String,
    pub previous_expires_at: Option<String>,
    pub previous_origin: Option<MemoryOrigin>,
    pub corrected_at: String,
    pub origin: MemoryOrigin,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryCorrectionInput {
    pub revision: u64,
    pub id: String,
    pub expected_sha256: String,
    pub content: String,
    pub reason: String,
}
pub fn memory_fingerprint(memory: &Memory) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(serde_json::to_vec(&serde_json::json!({
        "id":memory.id,"title":memory.title,"content":memory.content,
        "expires_at":memory.expires_at,"updated_at":memory.updated_at,"origin":memory.origin
    })).expect("memory serializes")))
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Reminder {
    pub id: String,
    pub text: String,
    pub due_at: String,
    pub interval_minutes: Option<u32>,
    pub enabled: bool,
    pub delivery: String,
    pub last_due_at: Option<String>,
    pub source: String,
    pub source_key: String,
}
impl Reminder {
    pub(crate) fn notification_client_id(&self) -> String {
        format!(
            "hb-reminder-{}-{}",
            self.id,
            self.last_due_at.as_deref().unwrap_or("due")
        )
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryInput {
    pub revision: u64,
    pub id: Option<String>,
    pub title: String,
    pub content: String,
    pub expires_at: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReminderInput {
    pub revision: u64,
    pub id: Option<String>,
    pub text: String,
    pub due_at: String,
    pub interval_minutes: Option<u32>,
    pub enabled: bool,
}
pub fn timestamp(text: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| "日期必须包含时区并采用 RFC3339 格式".into())
}
pub fn skills() -> serde_json::Value {
    serde_json::json!([
        {"id":"homebase-operations","name":"HomeBase 能力与任务","description":"发现接口、查询原任务、返回校验产物","version":1},
        {"id":"personal-memory","name":"个人长期记忆","description":"相关检索、原记录纠错、来源历史、到期与遗忘","version":3},
        {"id":"reminders","name":"提醒与重复计划","description":"文字提醒、定时与事件办事、通知筛选和本人确认","version":3},
        {"id":"personal-workflows","name":"资料、文稿与快捷任务","description":"在对话中收录资料、保存文稿和调用快捷能力，复用现有业务与权限","version":1}
    ])
}
pub fn skill(id: &str) -> Result<&'static str, String> {
    match id {
        "personal-workflows" => Ok(include_str!("../../agent-runtime/skills/personal-workflows/SKILL.md")),
        "homebase-operations" => Ok(include_str!(
            "../../agent-runtime/skills/homebase-operations/SKILL.md"
        )),
        "personal-memory" => Ok(include_str!(
            "../../agent-runtime/skills/personal-memory/SKILL.md"
        )),
        "reminders" => Ok(include_str!(
            "../../agent-runtime/skills/reminders/SKILL.md"
        )),
        _ => Err("Skill 未登记，不读取任意路径或安装脚本".into()),
    }
}
pub(crate) fn text(value: &str, max: usize) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        return Err("内容为空或超过长度限制".into());
    }
    let lower = value.to_lowercase();
    if [
        "-----begin",
        "sk-",
        "sk_",
        "api_key=",
        "api_key:",
        "bearer ",
        "password=",
    ]
    .iter()
    .any(|p| lower.contains(p))
    {
        return Err("个人记忆与提醒不能保存凭据；请使用密钥管理".into());
    }
    Ok(value.to_string())
}
impl Personal {
    pub fn save_memory(
        &mut self,
        input: MemoryInput,
        now: DateTime<Utc>,
    ) -> Result<Memory, String> {
        if input.revision != self.revision {
            return Err("个人助理数据已更新，请刷新后重试".into());
        }
        let title = text(&input.title, 100)?;
        let content = text(&input.content, 2000)?;
        if self.memories.iter().any(|m| m.title.trim().to_lowercase() == title.to_lowercase() && input.id.as_ref().map(|id|m.id!=*id).unwrap_or(m.content != content)) {
            return Err(if input.id.is_some(){"MEMORY_CONFLICT:标题已被另一条记忆使用；请选其他标题或先明确核对合并，未覆盖任何记录"}else{"MEMORY_CONFLICT:已有同标题的不同记忆；先检索并核对原内容，更新时明确指定原id，不创建互相矛盾的副本"}.into());
        }
        if let Some(expiry) = &input.expires_at {
            if timestamp(expiry)? <= now {
                return Err("记忆到期时间须晚于当前时间".into());
            }
        }
        let index = if let Some(id) = &input.id {
            Some(
                self.memories
                    .iter()
                    .position(|m| &m.id == id)
                    .ok_or("记忆不存在，未创建替代记录")?,
            )
        } else {
            self.memories
                .iter()
                .position(|m| m.title.to_lowercase() == title.to_lowercase() && m.content == content)
        };
        if index.is_none() && self.memories.len() >= 200 {
            return Err("记忆数量达到上限，请先整理".into());
        }
        let mut previous = index.map(|i| self.memories[i].previous.clone()).unwrap_or_default();
        if let Some(i) = index {
            let old = &self.memories[i];
            if old.content != content || old.expires_at != input.expires_at {
                previous.push(MemoryRevision { content: old.content.clone(), expires_at: old.expires_at.clone(), updated_at: old.updated_at.clone() });
                if previous.len() > 5 { previous.remove(0); }
            }
        }
        let memory = Memory {
            id: index
                .map(|i| self.memories[i].id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            title,
            content,
            expires_at: input.expires_at,
            updated_at: now.to_rfc3339(),
            expiry_notified: false,
            previous,
            origin: index.and_then(|i| self.memories[i].origin.clone()),
            corrections: index.map(|i| self.memories[i].corrections.clone()).unwrap_or_default(),
        };
        if let Some(i) = index {
            self.memories[i] = memory.clone();
        } else {
            self.memories.push(memory.clone());
        }
        self.revision += 1;
        Ok(memory)
    }
    pub fn correction_input(&self, mut args: serde_json::Value, now: DateTime<Utc>) -> Result<MemoryCorrectionInput,String> {
        let fields=args.as_object_mut().ok_or("纠错参数须为对象")?;
        fields.remove("action");
        let title=fields.remove("title");let expiry=fields.remove("expires_at");
        for key in ["query","offset"] {
            if let Some(value)=fields.remove(key) {if !value.is_null() && value!=serde_json::json!("") && value!=serde_json::json!(0) {return Err(format!("纠错动作不使用{key}，请只传纠错字段"));}}
        }
        let input:MemoryCorrectionInput=serde_json::from_value(args).map_err(|_|"纠错须填revision、原id、expected_sha256、content和reason；不接受未登记字段")?;
        let index=self.validate_correction(&input,now)?;let old=&self.memories[index];
        if title.as_ref().is_some_and(|v|!v.is_null()&&v.as_str()!=Some(old.title.as_str())) {return Err("correct保留原标题；改标题请明确使用完整表单保存，不会忽略此修改要求".into());}
        if let Some(expiry)=expiry {if expiry!=serde_json::json!(old.expires_at) {return Err("correct保留原有效期；改变或清除有效期请明确使用完整表单保存，不会忽略此修改要求".into());}}
        Ok(input)
    }
    fn validate_correction(&self,input:&MemoryCorrectionInput,now:DateTime<Utc>)->Result<usize,String> {
        if input.revision != self.revision { return Err("MEMORY_STALE:记忆已更新，请重新检索并核对原内容，未覆盖任何记录".into()); }
        let index=self.memories.iter().position(|m|m.id==input.id).ok_or("记忆不存在，未创建替代记录")?;
        let old=&self.memories[index];
        if input.expected_sha256 != memory_fingerprint(&old) { return Err("MEMORY_STALE:原记忆指纹不符，请重新核对，未覆盖任何记录".into()); }
        if old.expires_at.as_ref().is_some_and(|d|timestamp(d).map_or(true,|d|d<=now)) { return Err("记忆已到期，纠错不会擅自恢复；请明确新的有效期后保存".into()); }
        text(&input.content,2000)?;text(&input.reason,500)?;
        Ok(index)
    }
    pub fn correct_memory(&mut self, input: MemoryCorrectionInput, origin: MemoryOrigin, now: DateTime<Utc>) -> Result<Memory, String> {
        let index=self.validate_correction(&input,now)?;let old=self.memories[index].clone();
        let content=text(&input.content,2000)?;
        let reason=text(&input.reason,500)?;
        if content==old.content { return Ok(old); }
        let correction=MemoryCorrection {reason,previous_title:old.title.clone(),previous_content:old.content.clone(),previous_expires_at:old.expires_at.clone(),previous_origin:old.origin.clone(),corrected_at:now.to_rfc3339(),origin:origin.clone()};
        let mut memory=self.save_memory(MemoryInput {revision:input.revision,id:Some(old.id),title:old.title,content,expires_at:old.expires_at},now)?;
        memory.origin=Some(origin);
        memory.corrections.push(correction);
        if memory.corrections.len()>10 {memory.corrections.remove(0);}
        self.memories[index]=memory.clone();
        Ok(memory)
    }
    pub fn search(&self, query: &str, now: DateTime<Utc>) -> Vec<Memory> {
        self.search_refs(query, now).into_iter().cloned().collect()
    }
    /// Current-record borrowers avoid cloning correction/update archives for a
    /// read-only projection or search page. Owned callers keep search().
    pub(crate) fn search_refs(&self, query: &str, now: DateTime<Utc>) -> Vec<&Memory> {
        search::ranked(self.memories.iter().filter(|m| m.expires_at.as_ref().is_none_or(|d| timestamp(d).is_ok_and(|d| d > now))), query)
            .into_iter().map(|(m,_)|m).collect()
    }
    pub fn save_reminder(
        &mut self,
        input: ReminderInput,
        source: &str,
        key: &str,
    ) -> Result<Reminder, String> {
        if input.revision != self.revision {
            return Err("个人助理数据已更新，请刷新后重试".into());
        }
        let text = text(&input.text, 1000)?;
        timestamp(&input.due_at)?;
        if input
            .interval_minutes
            .is_some_and(|n| !(1..=525600).contains(&n))
        {
            return Err("重复间隔须为 1–525600 分钟".into());
        }
        let index = if let Some(id) = &input.id {
            Some(
                self.reminders
                    .iter()
                    .position(|r| &r.id == id)
                    .ok_or("提醒不存在")?,
            )
        } else {
            None
        };
        if index.is_none() && self.reminders.len() >= 100 {
            return Err("提醒数量达到上限".into());
        }
        let reminder = Reminder {
            id: index
                .map(|i| self.reminders[i].id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            text,
            due_at: input.due_at,
            interval_minutes: input.interval_minutes,
            enabled: input.enabled,
            delivery: "scheduled".into(),
            last_due_at: None,
            source: source.into(),
            source_key: key.into(),
        };
        // Desktop editing preserves the original authenticated channel binding.
        let reminder = if let Some(i) = index {
            let mut r = reminder;
            r.source = self.reminders[i].source.clone();
            r.source_key = self.reminders[i].source_key.clone();
            self.reminders[i] = r.clone();
            r
        } else {
            self.reminders.push(reminder.clone());
            reminder
        };
        self.revision += 1;
        Ok(reminder)
    }
    pub fn tick(&mut self, now: DateTime<Utc>) -> bool {
        let mut changed = false;
        for memory in &mut self.memories {
            if !memory.expiry_notified
                && memory
                    .expires_at
                    .as_ref()
                    .is_some_and(|s| timestamp(s).is_ok_and(|d| d <= now))
            {
                memory.expiry_notified = true;
                changed = true;
            }
        }
        for r in &mut self.reminders {
            if r.enabled
                && r.delivery == "scheduled"
                && timestamp(&r.due_at).is_ok_and(|d| d <= now)
            {
                r.last_due_at = Some(r.due_at.clone());
                r.delivery = if r.source == "wechat" {
                    "pending"
                } else {
                    "desktop_due"
                }
                .into();
                changed = true;
            }
        }
        if changed {
            self.revision += 1;
        }
        changed
    }
    pub fn acknowledge(
        &mut self,
        id: &str,
        delivery: &str,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        let r = self
            .reminders
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("提醒不存在")?;
        if !matches!(
            delivery,
            "accepted" | "unknown" | "dismissed" | "canceled" | "claimed"
        ) {
            return Err("提醒状态无效".into());
        }
        r.delivery = delivery.into();
        if matches!(delivery, "accepted" | "dismissed") {
            if let Some(n) = r.interval_minutes {
                let due = timestamp(&r.due_at)?;
                let elapsed = (now - due).num_minutes().max(0);
                let steps = elapsed / i64::from(n) + 1;
                r.due_at = (due + chrono::Duration::minutes(steps * i64::from(n))).to_rfc3339();
                r.delivery = "scheduled".into();
            } else {
                r.enabled = false;
            }
        }
        if delivery == "canceled" {
            r.enabled = false;
        }
        self.revision += 1;
        Ok(())
    }
}
