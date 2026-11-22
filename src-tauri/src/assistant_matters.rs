//! Confirmed ongoing responsibilities. Reviews notify, never execute or grant permissions.
use super::*;
use crate::assistant_personal::{text, timestamp};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Matters {
    pub revision: u64,
    pub entries: Vec<Matter>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Matter {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub summary: String,
    pub next_action: String,
    pub state: String,
    pub review_at: Option<String>,
    pub delivery: String,
    pub updated_at: String,
    pub source: String,
    source_key: String,
    pub turns: Vec<String>,
    pub previous: Vec<MatterRevision>,
}
impl Matter {
    fn notification_client_id(&self) -> String {
        format!(
            "hb-matter-{}-{}",
            self.id,
            self.review_at.as_deref().unwrap_or("due")
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct MatterRevision {
    pub summary: String,
    pub next_action: String,
    pub state: String,
    pub updated_at: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatterInput {
    pub revision: u64,
    pub id: Option<String>,
    pub title: String,
    pub goal: String,
    pub summary: String,
    pub next_action: String,
    pub state: String,
    pub review_at: Option<String>,
}
fn optional(value: &str, max: usize) -> Result<String, String> {
    if value.trim().is_empty() {
        Ok(String::new())
    } else {
        text(value, max)
    }
}
impl Matters {
    fn save(&mut self, input: MatterInput, source: &str, key: &str) -> Result<String, String> {
        if self.revision != input.revision {
            return Err("事项已更新，先读取最新版本再保存。".into());
        }
        let title = text(&input.title, 100)?;
        let goal = text(&input.goal, 1000)?;
        let summary = optional(&input.summary, 1200)?;
        let next = optional(&input.next_action, 500)?;
        if !["active", "waiting", "paused", "completed", "canceled"].contains(&input.state.as_str())
        {
            return Err("事项状态无效。".into());
        }
        let existing = if let Some(id) = &input.id {
            Some(
                self.entries
                    .iter()
                    .position(|e| e.id == *id && (key == "desktop" || e.source_key == key))
                    .ok_or("事项不存在或不属于当前来源。")?,
            )
        } else {
            None
        };
        if self.entries.iter().any(|e| {
            (key == "desktop" || e.source_key == key)
                && e.title.to_lowercase() == title.to_lowercase()
                && input.id.as_deref() != Some(e.id.as_str())
        }) {
            return Err("MATTER_CONFLICT:已有同名事项，请核对原记录后指定id更新。".into());
        }
        if let Some(at) = &input.review_at {
            let due = timestamp(at)?;
            let unchanged =
                existing.is_some_and(|i| self.entries[i].review_at.as_ref() == Some(at));
            if !unchanged
                && (due <= chrono::Utc::now()
                    || due > chrono::Utc::now() + chrono::Duration::days(365))
            {
                return Err("复查时间须在未来一年内；更新时可回传原时间或明确null清除。".into());
            }
        }
        let stamp = now();
        let i = if let Some(i) = existing {
            i
        } else {
            if self.entries.len() >= 50 {
                return Err("已达50条事项上限，旧记录保留。".into());
            }
            self.entries.push(Matter {
                id: id(),
                title: String::new(),
                goal: String::new(),
                summary: String::new(),
                next_action: String::new(),
                state: String::new(),
                review_at: None,
                delivery: "none".into(),
                updated_at: stamp.clone(),
                source: source.into(),
                source_key: key.into(),
                turns: vec![],
                previous: vec![],
            });
            self.entries.len() - 1
        };
        let e = &mut self.entries[i];
        if existing.is_some()
            && (e.summary != summary || e.next_action != next || e.state != input.state)
        {
            e.previous.push(MatterRevision {
                summary: e.summary.clone(),
                next_action: e.next_action.clone(),
                state: e.state.clone(),
                updated_at: e.updated_at.clone(),
            });
            if e.previous.len() > 5 {
                e.previous.remove(0);
            }
        }
        let terminal = matches!(input.state.as_str(), "completed" | "canceled" | "paused");
        if terminal {
            e.delivery = "canceled".into();
        } else if e.review_at != input.review_at
            || matches!(e.delivery.as_str(), "none" | "canceled")
        {
            e.delivery = if input.review_at.is_some() {
                "scheduled"
            } else {
                "none"
            }
            .into();
        }
        e.title = title;
        e.goal = goal;
        e.summary = summary;
        e.next_action = next;
        e.state = input.state;
        e.review_at = input.review_at;
        e.updated_at = stamp;
        self.revision += 1;
        Ok(e.id.clone())
    }
    pub(super) fn tick(&mut self) -> bool {
        let mut changed = false;
        for e in &mut self.entries {
            if e.delivery == "scheduled"
                && matches!(e.state.as_str(), "active" | "waiting")
                && e.review_at
                    .as_deref()
                    .is_some_and(|d| timestamp(d).is_ok_and(|d| d <= chrono::Utc::now()))
            {
                e.delivery = if e.source == "wechat" {
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
}
impl Engine {
    fn matter_fingerprint(e: &Matter) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&e.title, &e.goal, &e.next_action)).unwrap())
        )
    }
    fn reserved_matter_slots(state: &State, id: &str) -> usize {
        state
            .followups
            .entries
            .iter()
            .filter(|e| e.enabled && e.matter_id.as_deref() == Some(id))
            .map(|e| {
                (e.max_runs as usize)
                    .saturating_sub(e.runs.iter().filter(|r| r.turn_id.is_some()).count())
            })
            .sum()
    }
    pub(super) fn followup_matter_fingerprint(
        state: &State,
        id: Option<&str>,
        key: &str,
        runs: u32,
    ) -> Result<Option<String>, String> {
        let Some(id) = id else {
            return Ok(None);
        };
        let e = state
            .matters
            .entries
            .iter()
            .find(|e| {
                e.id == id
                    && e.source_key == key
                    && matches!(e.state.as_str(), "active" | "waiting")
            })
            .ok_or("关联事项不存在、不属于当前来源或已暂停/关闭")?;
        if e.turns.len() + Self::reserved_matter_slots(state, id) + runs as usize > 64 {
            return Err(
                "事项的任务关联名额不足，未创建新计划；请保留旧记录并整理后另建后续事项。".into(),
            );
        }
        Ok(Some(Self::matter_fingerprint(e)))
    }
    pub(super) fn followup_matter_ready(state: &State, entry: &followup::Followup) -> bool {
        entry.matter_id.as_ref().is_none_or(|id| {
            state.matters.entries.iter().any(|e| {
                e.id == *id
                    && e.source_key == entry.source_key
                    && matches!(e.state.as_str(), "active" | "waiting")
                    && entry.matter_fingerprint.as_deref()
                        == Some(Self::matter_fingerprint(e).as_str())
            })
        })
    }
    pub(super) fn followup_matter_instruction(state: &State, entry: &followup::Followup) -> String {
        entry.matter_id.as_ref().and_then(|id|state.matters.entries.iter().find(|e|e.id==*id)).map(|e|format!("关联持续事项：{}。本次受理时会自动关联原轮次，不用再次attach或重新安排。先核对该事项的真实进展，执行已经约定的本次内容；单次检查完成不代表整个事项完成，进展笔记变更仍需本人确认。",e.title)).unwrap_or_default()
    }
    fn link_matter_turn(
        state: &mut State,
        id: &str,
        source: &str,
        tid: &str,
    ) -> Result<(), String> {
        let e = state
            .matters
            .entries
            .iter_mut()
            .find(|e| e.id == id && e.source_key == source)
            .ok_or("原事项不存在，未重建替代记录")?;
        if !e.turns.iter().any(|t| t == tid) {
            if e.turns.len() >= 64 {
                return Err("事项关联任务已达64条上限，原记录保留".into());
            }
            e.turns.push(tid.into());
            e.updated_at = now();
            state.matters.revision += 1;
        }
        Ok(())
    }
    pub(super) fn link_scheduled_matter(state: &mut State, tid: &str) -> Result<(), String> {
        let (si, ti) = locate(state, tid)?;
        let turn = &state.sessions[si].turns[ti];
        let link = state
            .followups
            .entries
            .iter()
            .find(|e| e.runs.iter().any(|r| r.request_id == turn.request_id))
            .and_then(|e| {
                e.matter_id
                    .as_ref()
                    .map(|id| (id.clone(), e.source_key.clone()))
            });
        if let Some((id, key)) = link {
            Self::link_matter_turn(state, &id, &key, tid)?;
        }
        Ok(())
    }
    pub(super) fn reconcile_matter_followups(state: &mut State) -> Result<(), String> {
        let links: Vec<_> = state
            .followups
            .entries
            .iter()
            .filter_map(|e| e.matter_id.as_ref().map(|id| (id, e)))
            .flat_map(|(id, e)| {
                e.runs.iter().filter_map(move |r| {
                    r.turn_id
                        .as_ref()
                        .map(|tid| (id.clone(), e.source_key.clone(), tid.clone()))
                })
            })
            .collect();
        for (id, key, tid) in links {
            Self::link_matter_turn(state, &id, &key, &tid)?;
        }
        Ok(())
    }
    fn linked_matter_task<'a>(
        state: &'a State,
        e: &Matter,
        action: &str,
    ) -> Option<(&'a Session, &'a Turn)> {
        let tasks: Vec<_> = e
            .turns
            .iter()
            .rev()
            .filter_map(|id| {
                state
                    .sessions
                    .iter()
                    .filter(|s| s.source_key == e.source_key)
                    .find_map(|s| {
                        s.turns
                            .iter()
                            .find(|t| t.id == *id && t.interaction != "chat")
                            .map(|t| (s, t))
                    })
            })
            .collect();
        if action == "结果" {
            if let Some(v) = tasks.iter().copied().find(|(s, t)| {
                s.jobs
                    .iter()
                    .any(|j| turn_has_job(t, j) && j.owned && !j.artifacts.is_empty())
            }) {
                return Some(v);
            }
        }
        tasks
            .iter()
            .copied()
            .find(|(_, t)| !t.request_id.starts_with("hb-followup-") && active(&t.state))
            .or_else(|| tasks.iter().copied().find(|(_, t)| active(&t.state)))
            .or_else(|| tasks.iter().copied().find(|(s, t)| view(s, t).resumable))
            .or_else(|| tasks.first().copied())
    }
    pub(super) fn named_matter_target(
        state: &State,
        source: &str,
        text: &str,
    ) -> Result<Option<(String, String)>, String> {
        let parsed = [
            ("继续跟进", "继续"),
            ("继续处理", "继续"),
            ("继续", "继续"),
            ("看看进度", "进度"),
            ("查询进度", "进度"),
            ("进度", "进度"),
            ("结果", "结果"),
        ]
        .iter()
        .find_map(|(prefix, action)| {
            text.strip_prefix(prefix)
                .map(|title| (*action, title.trim()))
        })
        .filter(|(_, title)| !title.is_empty());
        let Some((action, title)) = parsed else {
            return Ok(None);
        };
        let entries: Vec<_> = state
            .matters
            .entries
            .iter()
            .filter(|e| e.source_key == source && e.title == title)
            .collect();
        if entries.len() > 1 {
            return Err("有同名事项，请先核对具体目标；没有操作任何任务。".into());
        }
        let Some(e) = entries.first() else {
            return Ok(None);
        };
        let (_, target) = Self::linked_matter_task(state, e, action).ok_or(
            "这件事尚未关联执行任务；只有事项记录，没有可继续的原任务。请说明这次要做什么。",
        )?;
        let tid = &target.id;
        if !state.sessions.iter().any(|s| {
            s.source_key == source
                && s.turns
                    .iter()
                    .any(|t| t.id == *tid && t.interaction != "chat")
        }) {
            return Err("没有找到属于当前来源的关联任务；未创建替代任务。".into());
        }
        Ok(Some((action.into(), tid.clone())))
    }
    pub async fn matters(&self) -> Value {
        let state = self.state.lock().await;
        Self::matter_list(&state, "desktop", "")
    }
    fn matter_view(state: &State, e: &Matter) -> Value {
        let latest = Self::linked_matter_task(state, e, "进度").map(
            |(s, t)| json!({"turn_id":t.id,"state":t.state,"progress":progress::snapshot(s,t)}),
        );
        json!({"id":e.id,"title":e.title,"goal":e.goal,"summary":e.summary,"next_action":e.next_action,"state":e.state,"review_at":e.review_at,"delivery":e.delivery,"updated_at":e.updated_at,"source":e.source,"turn_ids":e.turns,"previous":e.previous,"latest_task":latest})
    }
    pub(super) fn matter_list(state: &State, key: &str, query: &str) -> Value {
        let q = query.trim().to_lowercase();
        let entries: Vec<_> = state
            .matters
            .entries
            .iter()
            .rev()
            .filter(|e| key == "desktop" || e.source_key == key)
            .filter(|e| {
                q.is_empty()
                    || format!("{} {} {} {}", e.title, e.goal, e.summary, e.next_action)
                        .to_lowercase()
                        .contains(&q)
            })
            .take(50)
            .map(|e| Self::matter_view(state, e))
            .collect();
        json!({"revision":state.matters.revision,"entries":entries,"notice":"事项是经确认的持续目标，进展笔记与原任务证据分别显示。复查只提醒，不执行；完成一个关联任务不自动完成整件事项。"})
    }
    pub async fn matter_save(&self, input: MatterInput) -> Result<Value, String> {
        self.matter_save_from(input, "desktop", "desktop").await
    }
    pub(super) async fn matter_save_from(
        &self,
        input: MatterInput,
        source: &str,
        key: &str,
    ) -> Result<Value, String> {
        let result = self
            .mutate(
                Change {
                    personal: true,
                    ..Default::default()
                },
                |s| {
                    let id = s.matters.save(input, source, key)?;
                    let e = s.matters.entries.iter().find(|e| e.id == id).unwrap();
                    Ok(Self::matter_view(s, e))
                },
            )
            .await?;
        self.personal_wake.notify_one();
        Ok(result)
    }
    pub async fn matter_review_ack(&self, id: &str, revision: u64) -> Result<(), String> {
        self.mutate(
            Change {
                personal: true,
                ..Default::default()
            },
            |s| {
                if s.matters.revision != revision {
                    return Err("事项已变化，请刷新。".into());
                }
                let e = s
                    .matters
                    .entries
                    .iter_mut()
                    .find(|e| e.id == id)
                    .ok_or("事项不存在")?;
                if !matches!(e.delivery.as_str(), "desktop_due" | "unknown") {
                    return Err("没有待知悉的复查提醒".into());
                }
                e.delivery = "dismissed".into();
                s.matters.revision += 1;
                Ok(())
            },
        )
        .await
    }
    pub(super) async fn matter_tick(&self) -> Result<(), String> {
        if !self.state.lock().await.matters.clone().tick() {
            return Ok(());
        }
        self.mutate(
            Change {
                personal: true,
                ..Default::default()
            },
            |s| {
                s.matters.tick();
                Ok(())
            },
        )
        .await
    }
    pub(super) fn matter_pending(state: &State, source: &str) -> bool {
        state
            .matters
            .entries
            .iter()
            .any(|e| e.source_key == source && e.delivery == "pending")
    }
    pub(super) fn matter_claim(state: &mut State, source: &str) -> Vec<Value> {
        let due:Vec<_>=state.matters.entries.iter().filter(|e|e.source_key==source && e.delivery=="pending").map(|e|{
            let task=Self::matter_view(state,e)["latest_task"].clone();
            let progress=task.get("progress").and_then(|p|p["current"].as_str()).map(|s|format!("\n原任务正在做：{s}。")).unwrap_or_default();
            json!({"kind":"matter_notice","id":e.id,"client_id":e.notification_client_id(),"text":format!("该回顾一下“{}”了。{}{}\n下一步记录：{}\n要继续处理，直接告诉我；这次只提醒，没有重新执行。",e.title,if e.summary.is_empty(){"还没有确认的新进展。"}else{&e.summary},progress,if e.next_action.is_empty(){"待你决定"}else{&e.next_action})})
        }).collect();
        if !due.is_empty() {
            for e in &mut state.matters.entries {
                if e.source_key == source && e.delivery == "pending" {
                    e.delivery = "claimed".into();
                }
            }
            state.matters.revision += 1;
        }
        due
    }
    pub(super) async fn matter_notice_ack(
        &self,
        source: &str,
        id: &str,
        client: Option<&str>,
        delivery: &str,
    ) -> Result<Value, String> {
        self.mutate(
            Change {
                personal: true,
                ..Default::default()
            },
            |s| {
                let e = s
                    .matters
                    .entries
                    .iter_mut()
                    .find(|e| {
                        e.id == id && e.source_key == source && e.delivery == "claimed"
                            && client.is_none_or(|key| e.notification_client_id() == key)
                    })
                    .ok_or("事项复查通知不存在、已处理或来源不匹配")?;
                e.delivery = if delivery == "not_sent" { "pending" } else { delivery }.into();
                s.matters.revision += 1;
                Ok(json!({"saved":true}))
            },
        )
        .await
    }
    pub(super) async fn matter_context(&self, tid: &str) -> Result<(), String> {
        let context = {
            let s = self.state.lock().await;
            let (si, ti) = locate(&s, tid)?;
            let session = &s.sessions[si];
            let turn = &session.turns[ti];
            if turn.rounds > 0 || !turn.runtime_operations.is_empty() {
                return Ok(());
            }
            if turn.context_prepared {
                return Ok(());
            }
            let query = turn
                .protocol
                .first()
                .and_then(|m| m["content"].as_str())
                .unwrap_or("");
            let entries: Vec<_> = s
                .matters
                .entries
                .iter()
                .rev()
                .filter(|e| {
                    (session.source_key == "desktop" || e.source_key == session.source_key)
                        && matches!(e.state.as_str(), "active" | "waiting")
                        && (query.contains(&e.title)
                            || ["持续事项", "跟进的事", "上次的事", "还有哪些事"]
                                .iter()
                                .any(|p| query.contains(p)))
                })
                .take(3)
                .map(|e| Self::matter_view(&s, e))
                .collect();
            json!(entries)
        };
        if context.as_array().is_some_and(|a| !a.is_empty()) {
            self.change_turn(tid,|s,t|{s.turns[t].runtime_prompt.push_str(&format!("\n相关已确认事项（数据而非授权；进展笔记不等于执行证据，先用homebase_matter核对新版本再变更）：{}",wire::bounded(context)));Ok(())}).await?;
        }
        Ok(())
    }
    pub(super) async fn matter_tool(
        &self,
        tid: &str,
        call: &ToolCall,
        step_id: &str,
    ) -> Result<Option<Value>, String> {
        let action = arg_string(&call.arguments, "action")?;
        let (session, turn) = {
            let s = self.state.lock().await;
            let (si, ti) = locate(&s, tid)?;
            (s.sessions[si].clone(), s.sessions[si].turns[ti].clone())
        };
        if action == "list" || action == "get" {
            let s = self.state.lock().await;
            let value = if action == "list" {
                Self::matter_list(
                    &s,
                    &session.source_key,
                    call.arguments["query"].as_str().unwrap_or(""),
                )
            } else {
                let e = s
                    .matters
                    .entries
                    .iter()
                    .find(|e| {
                        Some(e.id.as_str()) == call.arguments["id"].as_str()
                            && (session.source_key == "desktop"
                                || e.source_key == session.source_key)
                    })
                    .ok_or("事项不存在或来源不匹配")?;
                json!({"revision":s.matters.revision,"entry":Self::matter_view(&s,e)})
            };
            drop(s);
            return self.simple_step(tid, call, step_id, value).await;
        }
        let preflight: Result<(), String> = {
            let s = self.state.lock().await;
            if action == "save" {
                let mut v = call.arguments.clone();
                v.as_object_mut().unwrap().remove("action");
                serde_json::from_value::<MatterInput>(v)
                    .map_err(|_| {
                        "请提供完整事项表单，包括summary、next_action、state和review_at".to_string()
                    })
                    .and_then(|i| {
                        s.matters
                            .clone()
                            .save(i, &session.source, &session.source_key)
                            .map(|_| ())
                    })
            } else if action == "attach" {
                if call.arguments["revision"].as_u64() != Some(s.matters.revision) {
                    Err("事项版本已变化".into())
                } else if s.matters.entries.iter().any(|e| {
                    Some(e.id.as_str()) == call.arguments["id"].as_str()
                        && e.source_key == session.source_key
                        && matches!(e.state.as_str(), "active" | "waiting")
                }) {
                    Ok(())
                } else {
                    Err("只能关联当前来源的有效事项".into())
                }
            } else {
                Err("事项动作无效".into())
            }
        };
        if let Err(message) = preflight {
            return self.simple_step(tid,call,step_id,json!({"status":"failed","code":"MATTER_INVALID","message":message,"next_action":"先list/get核对原事项和revision，纠正后重新提出供本人确认；没有更改事项。"})).await;
        }
        if !turn
            .pending
            .as_ref()
            .is_some_and(|p| p.tool_id == call.id && p.approved && p.arguments == call.arguments)
        {
            self.change_turn(tid, |s, t| {
                s.turns[t].pending = Some(Pending {
                    id: id(),
                    tool_id: call.id.clone(),
                    description: format!("homebase_matter {action}"),
                    arguments: call.arguments.clone(),
                    approved: false,
                });
                s.turns[t].state = "awaiting_confirmation".into();
                s.turns[t].workflow.pause(chrono::Utc::now());
                Ok(())
            })
            .await?;
            return Ok(None);
        }
        let result = if action == "save" {
            let mut v = call.arguments.clone();
            v.as_object_mut().unwrap().remove("action");
            self.matter_save_from(
                serde_json::from_value(v).map_err(|_| "事项表单无效")?,
                &session.source,
                &session.source_key,
            )
            .await
        } else {
            self.mutate(Change{personal:true,..Default::default()},|s|{if call.arguments["revision"].as_u64()!=Some(s.matters.revision){return Err("事项已变化，请重新核对".into());}let mid=arg_string(&call.arguments,"id")?;let reserved=Self::reserved_matter_slots(s,mid);let e=s.matters.entries.iter().find(|e|e.id==mid && e.source_key==session.source_key).ok_or("事项不存在")?;if !e.turns.iter().any(|id|id==tid) && e.turns.len()+reserved>=64{return Err("事项的关联名额已满或已为确认的定时任务预留，原记录保留".into());}Self::link_matter_turn(s,mid,&session.source_key,tid)?;Ok(json!({"attached":true,"turn_id":tid,"notice":"仅关联原任务；没有重新执行，也不代表事项已完成。"}))}).await
        };
        self.simple_step(
            tid,
            call,
            step_id,
            result.unwrap_or_else(
                |message| json!({"status":"failed","code":"MATTER_NOT_SAVED","message":message}),
            ),
        )
        .await
    }
}
