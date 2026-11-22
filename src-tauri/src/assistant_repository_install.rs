//! A versioned repository analysis becomes one host-prepared tool call.
use super::*;

const CHECKPOINT: &str = "wechat-repository-install-v1";

fn sandbox_startup_failure(code: &str) -> Option<&'static str> {
    Some(match code {
        "SANDBOX_NOT_READY" => "等待 WSL 隔离进程就绪超时。请先核对 Linux 发行版能否启动。",
        "SANDBOX_WSL_DISTRO_MISSING" => "WSL 未找到配置的 Linux 发行版。",
        "SANDBOX_WSL_PLATFORM_UNAVAILABLE" => "Windows 报告无法启动 WSL 2 的虚拟机平台。",
        "SANDBOX_WSL_COMPONENT_DISABLED" => "Windows 报告 WSL 所需组件未启用。",
        "SANDBOX_BOOT_EXITED" => "WSL 隔离监督进程未完成就绪检查便退出。",
        "SANDBOX_PROTOCOL_INVALID" => "暂时无法验证 WSL 隔离监督进程的就绪信息。",
        "SANDBOX_START_FAILED" => "Windows 无法启动 WSL 隔离进程。",
        "SANDBOX_RUNTIME_UNAVAILABLE" => "隔离环境缺少可运行的 Node.js 20 或更高版本。",
        "SANDBOX_ISOLATION_UNAVAILABLE" => "WSL 已响应，但无法启动受保护的隔离环境。",
        "SANDBOX_UNAVAILABLE" => "本机隔离执行器不可用。",
        "SANDBOX_CHANGED" => "本机隔离执行器校验失败。",
        _ => return None,
    })
}

pub(super) fn approval(pending: &Pending) -> Option<String> {
    (pending.tool_id == CHECKPOINT).then(|| {
        let examples = pending.arguments["request"]["examples"].as_array()
            .map(|examples| format!("这次会使用你提供的 {} 条验收示例。\n", examples.len()))
            .unwrap_or_default();
        format!(
            "我准备{}（分析版本 {}）。验收通过后会加入能力库。\n{examples}\n回复“确认”开始，或回复“拒绝”取消。有其他待确认操作时，可发送“确认 {}”。",
            pending.description, pending.arguments["request"]["revision"], pending.id,
        )
    })
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub(crate) repository: String,
    pub(crate) project_id: String,
    pub(crate) revision: u64,
    pub(crate) analysis_receipt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) examples: Option<Vec<Value>>,
}

pub(crate) fn validate_examples(examples: &[Value]) -> Result<(), String> {
    let valid = (1..=8).contains(&examples.len()) && examples.iter().all(|example| {
        example.as_object().is_some_and(|entry| entry.len() == 2)
            && example["input"].as_object().is_some_and(|input| input.len() == 1)
            && example["input"]["stdin"].as_str().is_some_and(|input| input.len() <= 16_000)
            && example["contains"].as_array().is_some_and(|parts| {
                (1..=8).contains(&parts.len()) && parts.iter().all(|part| {
                    part.as_str().is_some_and(|text| !text.is_empty() && text.len() <= 2000)
                })
            })
    });
    if !valid || serde_json::to_vec(examples).map_or(true, |data| data.len() > 16_000) {
        return Err("请提供 1 至 8 条验收示例。每条需要 input.stdin 文本和非空 contains 文本列表，合计不超过 16 KB。".into());
    }
    Ok(())
}

impl Intent {
    pub(crate) fn validate(&self) -> Result<(), String> {
        let project = &self.project_id;
        if !crate::wechat::video::receipt_id(&self.analysis_receipt)
            || self.revision == 0
            || !project.starts_with("project_")
            || project.len() <= "project_".len()
            || project.len() > 160
            || !project
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
            || crate::wechat::video::repository_refs(&self.repository).first()
                != Some(&self.repository)
        {
            return Err("原仓库、分析回执或项目版本无法核对，未准备安装。".into());
        }
        if let Some(examples) = &self.examples {
            validate_examples(examples)?;
        }
        Ok(())
    }

    fn call(&self) -> ToolCall {
        let mut request = json!({"project_id":self.project_id,"revision":self.revision});
        if let Some(examples) = &self.examples {
            request["examples"] = json!(examples);
        }
        ToolCall {
            id: CHECKPOINT.into(),
            name: "homebase_call".into(),
            arguments: json!({"command":"repo_project_install","args":{"request":request}}),
        }
    }

    pub(super) fn prepare(&self, turn: &mut Turn) {
        // Older builds reject this execution protocol before starting a graph.
        turn.runtime_version = CHECKPOINT.into();
        turn.settings.runtime = "pi".into();
        turn.runtime_models.insert(CHECKPOINT.into(), json!(self));
        turn.queued.push(self.call());
    }

    pub(super) fn outcome(&self, job: Option<(&str, &Value)>) -> (&'static str, String) {
        if let Some(("failed", value)) = job {
            if let Some(message) = value["code"].as_str().and_then(sandbox_startup_failure) {
                return ("failed", format!("{message} 本次没有运行仓库代码。"));
            }
            if let Some(message) = value["project"]["error"]["code"].as_str().and_then(sandbox_startup_failure) {
                return ("failed", format!("{message} 验收未完成，能力未登记。请按原任务编号查看。"));
            }
        }
        match job {
            Some(("succeeded", value))
                if value["status"] == "succeeded"
                    && value["registered"] == true
                    && value["project"]["id"] == self.project_id
                    && value["project"]["phase"] == "registered" =>
            {
                (
                    "completed",
                    format!("{} 已通过验收并加入能力库。", self.repository),
                )
            }
            Some(("failed", value))
                if value["code"] == "PROJECT_STALE"
                    || value["project"]["error"]["code"] == "PROJECT_STALE" =>
            {
                (
                    "failed",
                    format!(
                        "{} 的项目版本已更新。这次没有安装，请引用最新的分析结果。",
                        self.repository
                    ),
                )
            }
            Some(("failed", value)) if value["code"] == "ACCEPTANCE_REQUIRED" => (
                "failed",
                format!(
                    "{} 还需要验收示例。可以在能力库填写，或复制这条命令，把示例文字换成你的输入和预期输出。\n/安装项目 {} [{{\"input\":{{\"stdin\":\"输入文本\"}},\"contains\":[\"预期输出中的文字\"]}}]",
                    self.repository, self.analysis_receipt,
                ),
            ),
            Some((state, value))
                if matches!(state, "unknown" | "interrupted_unknown")
                    || value["code"] == "EXECUTION_UNKNOWN"
                    || value["project"]["error"]["code"] == "EXECUTION_UNKNOWN"
                    || value["project"]["phase"] == "interrupted_unknown" =>
            {
                (
                    "interrupted_unknown",
                    "隔离执行结果暂时无法确认。请按原任务编号查询，暂勿重新安装。".into(),
                )
            }
            Some(("failed", _)) => (
                "failed",
                format!(
                    "{} 的安装或验收失败。请在能力库查看原项目结果。",
                    self.repository
                ),
            ),
            Some(("canceled", _)) => (
                "canceled",
                "仓库安装已取消，请在能力库查看原项目状态。".into(),
            ),
            Some(("succeeded", value)) if value["status"] == "canceled" => (
                "canceled",
                "仓库安装已取消，请在能力库查看原项目状态。".into(),
            ),
            _ => (
                "failed",
                "未能核实仓库已注册，请按原任务查看实际结果。".into(),
            ),
        }
    }

    pub(super) fn result_fields(value: &Value) -> Value {
        let project = &value["project"];
        let code = value["code"]
            .as_str()
            .or_else(|| value["error"]["code"].as_str());
        json!({"status":value["status"],"code":code,"registered":value["registered"],"project":{
            "id":project["id"],"phase":project["phase"],"error":{"code":project["error"]["code"]}
        }})
    }
}

impl Turn {
    pub(super) fn has_repository_install(&self) -> bool {
        self.runtime_version == CHECKPOINT || self.runtime_models.contains_key(CHECKPOINT)
    }

    pub(super) fn repository_install(&self) -> Result<Option<Intent>, String> {
        let Some(value) = self.runtime_models.get(CHECKPOINT) else {
            return if self.runtime_version == CHECKPOINT {
                Err("原仓库安装回执缺失，未执行".into())
            } else {
                Ok(None)
            };
        };
        let intent: Intent =
            serde_json::from_value(value.clone()).map_err(|_| "原仓库安装确认无法读取")?;
        intent.validate()?;
        let call = intent.call();
        if !matches!(self.queued.as_slice(), [queued]
            if queued.id == call.id && queued.name == call.name && queued.arguments == call.arguments)
        {
            return Err("原仓库安装步骤发生变化，未执行".into());
        }
        Ok(Some(intent))
    }

    pub(super) fn safe_repository_restart(&self, jobs: &Records<Job>) -> bool {
        if !matches!(self.repository_install(), Ok(Some(_))) {
            return false;
        }
        match self.state.as_str() {
            "queued" | "running" => self.control_intents.is_empty() && self.steps.is_empty(),
            "waiting_jobs" => jobs.iter().any(|job| {
                turn_has_job(self, job)
                    && job.command == "repo_project_install"
                    && matches!(job.state.as_str(), "queued" | "running")
            }),
            _ => false,
        }
    }
}

const CANCEL_COMMAND: &str = "repo_operation_cancel";
const CANCEL_UNVERIFIED: &str = "暂时无法确认取消请求是否生效。可以继续问我这笔安装的进度。";

enum Cancellation {
    Reject(String),
    Request { project: String, operation: String },
    Report(&'static str),
}

fn cancellation(session: &Session, turn: &Turn) -> Result<Cancellation, String> {
    let Some(intent) = turn.repository_install()? else {
        return Ok(Cancellation::Report("这条回复没有关联仓库安装。请引用原安装任务。"));
    };
    if turn.state == "awaiting_confirmation" {
        if let Some(pending) = turn.pending.as_ref().filter(|p| p.tool_id == CHECKPOINT && !p.approved) {
            return Ok(Cancellation::Reject(pending.id.clone()));
        }
    }
    if turn.state == "rejected" {
        return Ok(Cancellation::Report("这次安装已取消，还没有开始下载。"));
    }
    let expected = intent.call().arguments["args"]["request"].clone();
    let mut steps = turn.steps.iter().filter_map(|step| {
        if step.name != "homebase_call" {
            return None;
        }
        let mut request = step.arguments["request"].clone();
        let operation = request.as_object_mut()?.remove("request_id")?.as_str()?.to_owned();
        let digest = operation.strip_prefix("assistant-")?;
        (request == expected && digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
            .then_some((step, operation))
    });
    let Some((step, operation)) = steps.next().filter(|_| steps.next().is_none()) else {
        return Ok(Cancellation::Report("暂时没有核对到可取消的安装操作。可以先问我进度。"));
    };
    match step.job_id.as_deref() {
        Some(id) => {
            let job = job_access::owned(&session.jobs, id)?.filter(|job| {
                job.turn_id == turn.id && job.command == "repo_project_install"
            }).ok_or("无法核对这笔安装的原任务，未提交取消请求。")?;
            if !matches!(job.state.as_str(), "queued" | "running") {
                return Ok(Cancellation::Report(match job.state.as_str() {
                    "canceled" => "这次安装已经取消。",
                    "succeeded" | "failed" => "这次安装已经结束。可以问我原任务结果。",
                    _ => CANCEL_UNVERIFIED,
                }));
            }
        }
        None if turn.control_intents.get(&step.id).map(String::as_str) == Some("repo_project_install") => {}
        None => return Ok(Cancellation::Report(CANCEL_UNVERIFIED)),
    }
    Ok(Cancellation::Request { project: intent.project_id, operation })
}

impl Engine {
    pub(super) async fn cancel_repository_install(
        self: &Arc<Self>,
        source: &str,
        session: &str,
        turn: &str,
        receipt: &str,
    ) -> Result<&'static str, String> {
        let target = self.inspect_scoped_turn(turn, Some(session), TurnProjection::Full, |s, t| {
            if s.source != "wechat" || s.source_key != source {
                return Err("无法核对这笔安装的微信归属。".into());
            }
            cancellation(s, t)
        }).await??;
        match target {
            Cancellation::Report(text) => Ok(text),
            Cancellation::Reject(id) => {
                let decision = self.confirm_once(turn, &id, false, Some(session)).await?;
                Ok(if decision.state == "rejected" {
                    "这次安装已取消，还没有开始下载。"
                } else {
                    "确认状态刚有变化。请先问我安装进度，再决定是否取消。"
                })
            }
            Cancellation::Request { project, operation } => {
                let request = format!("repo-cancel-{receipt}");
                let observed = tokio::time::timeout(Duration::from_secs(3), async {
                    let accepted = self.control("call", Some(CANCEL_COMMAND),
                        json!({"request":{"project_id":project,"operation_id":operation}}), &request, None).await?;
                    let job = arg_string(&accepted["job"], "id")?;
                    job_access::control_identity(job, CANCEL_COMMAND, &accepted["job"])?;
                    loop {
                        let view = self.control("jobs.get", None, json!({}), &request, Some(job)).await?;
                        job_access::control_identity(job, CANCEL_COMMAND, &view)?;
                        match view["status"].as_str() {
                            Some("queued" | "running") => tokio::time::sleep(Duration::from_millis(100)).await,
                            _ => return Ok::<_, String>(view),
                        }
                    }
                }).await;
                let Ok(Ok(view)) = observed else { return Ok(CANCEL_UNVERIFIED); };
                Ok(match (view["status"].as_str(), view["result"]["state"].as_str()) {
                    (Some("succeeded"), Some("cancel_requested")) => "我已请求取消这次安装。后台停下后，我会更新原任务的结果。",
                    (Some("succeeded"), Some("already_finished")) => "后台操作已经结束。可以问我原任务结果，看看最后停在哪一步。",
                    _ if view["result"]["error"]["code"] == "OPERATION_MISMATCH" || view["error"]["code"] == "OPERATION_MISMATCH" =>
                        "暂时没有核对到可取消的安装操作。可以先问我进度。",
                    _ => CANCEL_UNVERIFIED,
                })
            }
        }
    }

    pub(super) async fn channel_repository_install(
        self: &Arc<Self>,
        source: &str,
        request: &Value,
    ) -> Result<Value, String> {
        if !self.settings().await.wechat_enabled {
            return Err("微信 AI 助理已停用，未准备仓库安装。".into());
        }
        let intent: Intent = serde_json::from_value(request["intent"].clone())
            .map_err(|_| "原仓库安装资料不完整")?;
        intent.validate()?;
        let rid = arg_string(request, "request_id")?;
        let outcome = self
            .submit_bound(
                SendRequest {
                    session_id: None,
                    request_id: rid.into(),
                    text: format!("安装并验收 {}", intent.repository),
                    attachments: vec![],
                    image: None,
                    route_id: None,
                    mode: Some("task".into()),
                },
                "wechat",
                source,
                None,
                Some(intent),
                false,
            )
            .await?;
        match outcome {
            SubmissionOutcome::Accepted(ack) => Ok(json!(ack)),
            SubmissionOutcome::WaitingForWeChat => Err("原微信来源已离线，未受理仓库安装".into()),
            SubmissionOutcome::Clarify { question, .. } => Err(question),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_sandbox_start_failure_from_an_unconfirmed_execution() {
        let intent = Intent {
            repository: "https://github.com/fixture/project".into(),
            project_id: "project_fixture".into(),
            revision: 2,
            analysis_receipt: format!("wx-{}", "a".repeat(64)),
            examples: None,
        };
        let missing = Intent::result_fields(&json!({
            "status":"failed","error":{"code":"SANDBOX_NOT_READY"}
        }));
        let unknown = Intent::result_fields(&json!({
            "status":"failed","project":{"phase":"interrupted_unknown","error":{"code":"EXECUTION_UNKNOWN"}}
        }));
        let (state, message) = intent.outcome(Some(("failed", &missing)));
        assert_eq!(state, "failed");
        assert!(message.contains("没有运行仓库代码"));
        assert!(message.contains("超时"));
        let platform = Intent::result_fields(&json!({"status":"failed","error":{
            "code":"SANDBOX_WSL_PLATFORM_UNAVAILABLE","message":"SYNTHETIC_PRIVATE_BOOT_TEXT"
        }}));
        let (state, message) = intent.outcome(Some(("failed", &platform)));
        assert_eq!(state, "failed");
        assert!(message.contains("虚拟机平台") && !message.contains("SYNTHETIC_PRIVATE_BOOT_TEXT"));
        let later = Intent::result_fields(&json!({"status":"failed","project":{"phase":"failed","error":{
            "code":"SANDBOX_BOOT_EXITED"
        }}}));
        let (state, message) = intent.outcome(Some(("failed", &later)));
        assert_eq!(state, "failed");
        assert!(message.contains("验收未完成") && !message.contains("没有运行仓库代码"));
        let (state, message) = intent.outcome(Some(("failed", &unknown)));
        assert_eq!(state, "interrupted_unknown");
        assert!(message.contains("原任务编号"));
        let (state, message) = intent.outcome(Some(("failed", &json!({"code":"ACCEPTANCE_REQUIRED"}))));
        assert_eq!(state, "failed");
        let prefix = format!("/安装项目 {} ", intent.analysis_receipt);
        let example = message.lines().find_map(|line| line.strip_prefix(&prefix)).unwrap();
        let example = serde_json::from_str::<Vec<Value>>(example).unwrap();
        validate_examples(&example).unwrap();
    }
}
