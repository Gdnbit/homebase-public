# Claude / Gemini 原生对话与工作流

会话页的新 Agent 下拉框支持 Codex、Claude Code 和 Gemini CLI。每个 Agent 使用自己的原生会话，不通过换模型冒充原生续接。HomeBase 自动组织有界上下文，挂载统一工具桥；共享执行器保留文件授权、本人确认、任务状态、产物与去重机制。原生 Agent 的订阅/API 费用独立于内置助理的免费模型限制。

Claude 使用本机官方 CLI 的流式 Agent SDK 模式，沿用原生账号或明确选择已有 Anthropic 凭据引用。新建使用明确会话 UUID，继续使用 `--resume` 原 ID，不使用 latest/continue。仅开放 HomeBase MCP，禁用原生内置工具和项目 hooks；已有旧会话按真实目录与原生 ID 续接，历史资料仍不能替代授权。

Gemini 使用独立固定的 `@google/gemini-cli@0.68.0` 和 ACP v1。全局旧版 0.1.7 不升级、不改配置、不导入/重写 OAuth。每个会话有 HomeBase 自管 profile，凭据来自进程环境或新建时选择的已有 Google Key 引用；未配置或权限不足时明确失败，不切换凭据。其旧版提示日志没有现代检查点，只支持查看与显式摘录交接。新建之后产生的 ACP 会话支持真正的 session/load；加载的历史事件不再次写入新轮次。

`native-agent-runtime/` 通过锁文件固定依赖，安装使用 `npm ci --ignore-scripts --no-audit --no-fund`（工作目录必须为该目录）。不自动安装、升级用户的全局 CLI。发布打包调用 `scripts/package-native-agents.mjs`，拒绝目录链接和额外依赖。Node 复用已固定的 HomeBase 运行包。只对 HomeBase 创建的独立 Gemini 工作区跳过启动信任询问；原生 shell/文件工具并未因此开放。

供 AI 使用的接口：

- `conversation_providers`：读取真实程序/运行包支持情况；可启动不等于账号已验证。
- `conversation_native({request:{provider,action,...}})`：provider 为 codex（默认）、claude-code、gemini-cli。list/read/status 只读；create/send/interrupt 必须固定 request_id。create 可选 credential_ref，格式 sops:name 或 api_key_manager:service，仅保存引用。
- create 仅准备逻辑会话/关联助理会话，不请求模型；send 受理后查询同一 thread_id 的 status。native_id 由官方 SDK 返回，不能与逻辑 thread_id 混用。
- accepted 不表示业务完成。completed 需真实模型内容及 SDK 成功终态；failed/unknown/interrupted 不重做。相同 request_id 的重试返回原回执，参数变化拒绝执行。status 是有界增量投影，历史 read 每页最多20轮。
- 桌面刷新最近一页时保留当前会话已经翻读的窗口及阅读位置。窗口按原轮次编号合并，旧页沿用自己的游标。最新窗口与已读内容之间暂有缺口时，可用「补读中间的原生对话」请求新窗口的原游标。该缓存保存在应用内存，刷新或补读不会发送消息。 长历史验收
- HomeBase 桥支持 context/discover/describe/task/status。task 委托既有助理，status 仅能查询该关联会话；本人确认仍在桌面关联助理或原绑定微信进行，原生模型不能自批。原生停止不宣称撤销已受理共享任务，须查询原任务。

task 依赖已启用的 HomeBase 内置助理；原生订阅账号不能替代共享执行器的模型配置。当前真实实例仍使用原先的免费模型与免费限制。原生复合命令的 CLI 授权以 describe.requires 为准，list/read/status 的只读含义是不修改或重做任务，不代表可以省略命令要求的权限。

旧工作流先收进助理，旧页面和旧记录暂时保留：读取内置 `personal-workflows` Skill 可执行资料收录、Markdown 草稿保存与交付、已有快捷能力任务。资料保存复用 inbox_submit；文字稿、转换和视频仍根据实际能力决定，不把保存链接称为解析成功。文稿用已授权文件工具保存/export，不自动写真实站点或发布。桌面文本预览仅接受已登记产物、验证路径与 SHA-256，UTF-8 文本最多256KiB；微信可返回 md/markdown/csv 附件。未知结果先查询原 job，不能重复生成。

隔离验证使用 HOMEBASE_TEST_HOME，旧 inbox 路径也转入隔离 Wiki；测试 Claude 为 bare 空账号配置，Gemini 为独立 profile，所有模型响应来自127.0.0.1合成服务，微信为自动模拟。不使用真实 Key，也不以此声称真实厂商账号/手机收发已经复测。
