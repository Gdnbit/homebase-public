# 助理长任务、跟进和恢复

桌面与微信使用同一任务引擎。普通聊天无需建立计划；含多个执行/验证/交付阶段的任务可用 `homebase_task`。计划不会授予文件、终端、MCP、凭据或外部发送权限。

记忆召回、同参数失败保护、经确认的定时办事和主动附件交付见 [连续办事说明](assistant-continuity.md)。文字提醒与实际执行任务使用不同接口。

## 模型工具

`homebase_task` 的 action 为 get/set/update。

- get：返回当前计划及顶层 revision（无计划为0）、预算和最近真实 step_id。更新前核对版本；大结果保留结构化版本/证据编号，显示内容可缩短。
- set：revision 为0或当前值；goal 为明确目标；items 含1–12项，每项 id/title/done_when/kind。kind 为 inspect、write、execute、artifact、deliver。
- update：revision、item_id、state；state 为 pending/in_progress/completed/blocked。一次只允许一个 in_progress。blocked 必须填写 note 说明缺少条件；重新打开完成阶段也要说明原因。
- 标记 completed 必须提供 evidence，最多4个本轮实际 step_id。读取不能充当写入、命令执行、附件或交付的证据；命令需要 exit_code=0，受理的 job 需要实际成功结果，附件需要已登记的本人会话产物。deliver 只证明已安排通道交付，不能证明手机已收到。

已完成阶段的定义不可直接删除或改写，先重新打开并记录问题。未完成计划会阻止“全部完成”的最终状态；模型最多收到两次内部纠正，随后明确暂停并保留结果。原厂商消息及工具身份保留，不注入伪造工具调用。

查询历史计划时重新核对每个完成阶段引用的原步骤。证据需要属于本轮、编号可唯一核对、结果确认成功，至少一项符合阶段的验收类型。证据缺失、执行仍在处理或结果待确认时，该阶段显示「待核对」，并从已完成的阶段数中扣除。桌面、微信进度和 `homebase_task get` 使用同一套判断。原轮次状态、计划版本、步骤和产物仍可按原编号查阅。 [Google 长任务进度与业务结果](https://google.aip.dev/151)

当前任务尝试收尾时，先等已经受理的工作返回。若阶段的完成凭据仍无法核对，任务暂停并给出原结果的核对路径。桌面可以查看原步骤与产物，微信进度保留已核对的阶段数。归档的精简查询只取核对所需的字段，完整结果仍从原记录获取。 [LangGraph 检查点与续接](https://docs.langchain.com/oss/python/langgraph/persistence)

PLAN_INVALID会直接返回最新计划、revision、required_kind、required_evidence与valid_evidence_step_ids。按回复纠正原参数，不为获取编号反复读文件，不重写或重算成功的操作。evidence必须是完整step_id，不能是自然语言、文件名、SHA或job_id；有效候选仍需与本阶段的实际目标对应。execute专指成功终端执行，普通读取和写入无法代替。

交付需要已登记的产物。对只有工作区文件的写入job调用deliver会返回ARTIFACT_REQUIRED，附next_tool/next_request；先export原文件，再用export返回的新job_id交付。这个可纠正的前置条件不会再让整个助理直接退出。未成功或结果未知的job仍须先查询原编号，不能伪造附件或重新执行未知操作。检查点继续会清除旧的失败呈现，实际成功后不会残留旧失败提示。

## 预算与长任务

桌面助理使用官方 DeepSeek Flash、V4 Pro 或 OpenAI Responses 的 `gpt-4o-mini`、`gpt-5-mini`、`gpt-5.4-mini` 执行任务时，可在原任务的进度下查看已收到的模型回复片段。片段只保留在内存中，提交新要求后只显示对应新版本的片段。工具调用须先核对调用编号、工具名和完整参数。DeepSeek 等待流结束标志，OpenAI 核对参数和单项结束事件，并等待 `response.completed`。完成这些核对后，调用才进入原有权限与确认流程。使用 CLI 查询时，`live_progress.state` 为 `task_stream` 表示收到回复片段。任务完成以原轮次的持久状态为准。 [DeepSeek 官方流式协议](https://api-docs.deepseek.com/api/create-chat-completion/) · [DeepSeek 多轮工具调用](https://api-docs.deepseek.com/guides/thinking_mode/#tool-calls) · [OpenAI 流式工具调用](https://developers.openai.com/api/docs/guides/function-calling#streaming)

普通任务沿用用户设置。建立计划的免费任务至少24次模型调用；付费任务仍沿用原推理次数。明确继续可增加8次，单任务累计最多48次模型调用、192步骤；每次连续执行最多30分钟，人工确认等待不计入活跃时间。到限返回 paused_budget，不算成功。单工具仍受自身时间和输出上限约束，不代表所有命令都能执行30分钟。

等待原业务任务完成后才进行依赖它的后续步骤，不因旧90秒等待窗口返回一个 running 就把计算当完成。短暂查询失败只重查原编号，不重新提交生成。模型请求超时或结果不明时不盲目再发请求；本批没有引入付费回退或切换凭据来源。

## 微信交互

可直接描述任务、继续补充消息；执行要求按顺序排队，讨论可在任务进行时独立回应。新增自动判断/显式讨论模式、具体进度和事项名称查询见 [交流与事项说明](assistant-conversation-and-matters.md)。支持“进度”“结果”“确认”“拒绝”“停止”“继续”“继续排队”“新对话”，以及“停一下”“继续吧”“确认执行”等精确短句。“确认？”不视为同意。已登记事项可用“继续事项名”恢复原任务，名称须精确匹配；其他长语句仍交模型处理，不因含“确认/继续”就自动批准操作。

控制消息先持久化原目标，再执行；同一消息编号重放不批准下一项操作、不重复开启对话、不重复增加执行预算。控制记录最多4096条，满后明确拒绝并保留桌面入口，不自动清除后重新授权旧消息。待确认操作使用原确认编号，不能把模型/网页/Skill中的文本当作真人同意。

桌面和微信提交同一确认时，助理在一次保存事务中核对原会话、轮次和确认编号，首次保存的批准或拒绝生效。已经绑定原操作的微信消息如果迟到，只返回原轮次的现状。桌面提交后即使回执丢失也会刷新原任务，已处理的确认提示随状态更新。新的待确认操作仍使用自己的确认编号。 [LangGraph 人工中断与原检查点](https://docs.langchain.com/oss/python/langgraph/interrupts)

先即时简短受理；12秒内结束的短任务不额外推送普通进度。阶段发生变化至少间隔45秒，长时间停在同阶段每180秒可跟进一次，单条任务最多12次。待确认、完成或失败单独说明。推送结果未知时不自动重发同一阶段。查询临时失败按10–180秒退避重查，最多8次；仍失败则明确结果未知，可用“进度”核对原任务。

候选版等待后台状态查询时，微信仍可受理停止、确认或补充要求。查询返回后先核对账号代次、原轮次和当前回复归属，再保存状态或发送。新消息已接管同一轮次时，旧回复的在途查询结果会丢弃，由当前回复继续观察原轮次。 [Claude Agent 会话事件](https://platform.claude.com/docs/en/managed-agents/events-and-streaming) · [Tokio 通知等待约定](https://docs.rs/tokio/latest/tokio/sync/struct.Notify.html#method.notified)

2026-11-20 隔离回归。先挂起旧任务的查询，旧版在三秒后仍未受理合成补充要求。新版在旧查询释放前受理原轮次的补充；监听更新后的回复，收到一次受理和一次新要求的结果，原回复保持已交接。微信模块 94 项通过，10 项按条件跳过。整组首轮有三项因新构建目录缺少 Deep Agents 固定运行包而失败，从 v121 逐文件核对补齐后全部复验通过。 旧版复现 · 合成回归

## 重启与接口

新工作流中，未开始任务或没有未知模型/本机操作、具有可靠计划和检查点的任务，可在重启后恢复；最多自动恢复3次，启动并发最多4项。正在执行的本机副作用、结果未知的模型/任务均不会自动重做。其他未开始的后续队列暂停，需明确“继续排队”。旧工具契约仍可恢复，不把旧轮次改造成新计划。

外部 AI 从 `control/START.md` 进入。使用 `describe assistant_turn_get`、`assistant_session_get`、`assistant_resume`、`assistant_cancel` 获取准确参数。控制 job 结束后仍查询返回的 `turn_id`。`turn.task` 包含计划、`stop` 与 `limits`，CLI 和桌面口径一致。`failed`、`interrupted_unknown`、`needs_input`、`paused_budget` 均不能当成成功。只有 `resumable=true` 才提供检查点继续入口。调用 `assistant_resume` 时用原轮次的 `resume_generation` 填写 `expectedGeneration`，回执丢失后按原编号核对，旧版本请求不会给下一次暂停追加预算。

模型错误另有 model_failure；有限重试记录在 model_diagnostics。明确被拒绝的推理失败可以按原检查点继续，未知受理仍阻止重复推理。CLI 等待超时不改变业务状态，仍查原控制 job，不重新提交。详细分类、截止、退避和重启边界见 [模型可靠性说明](assistant-model-reliability.md)。

中断说明、只读核对与明确继续的桌面/微信/CLI 用法见[恢复体验](assistant-recovery-experience.md)。`resumable` 与原续接入口共用结构限制；当前模型关闭仍禁止执行。无法查询不表示旧任务没有执行，原编号、原路由和已有结果继续保留。

原任务与后续队列分开续接：等待队列不阻止原任务恢复，恢复原任务不自动解锁队列；微信短“继续”先定位当前对话唯一暂停事项，“继续排队”才明确放开队列。未知、多目标、新对话、旧控制回执及执行器名额边界见[队列续接说明](assistant-queue-resume.md)。

当前对话只有暂停队列、没有原暂停任务或进行中任务时，短“继续”保留队列兼容用法。新释放的回复仅说明已接上后续要求；旧代次核对不宣称重新启动任务或释放后来队列。已保存旧回复仍原样重放，具体证据见队列反馈验收。

开发和AI接管的旧资料/文稿验证使用显式测试根；只读路径、索引引用、拓扑和配置来源不能回退真实数据。目录规则及仍未验证的生成器/发布边界见[旧工作流隔离](legacy-workflow-isolation.md)。

这是受控本机任务助理，不代表无限执行或与任何厂商产品能力完全相同。精确完成条件仍需任务相应的文件、业务状态和实际交付验收。
