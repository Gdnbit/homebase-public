# HomeBase AI 启动与 CLI 控制入口

当前实现提供一个可发给 AI 的启动提示词、一套可按需读取的操作文件，以及调用桌面业务命令的本机 CLI。AI 需要具备本机文件读取与命令执行工具；提示词本身不会赋予模型这些工具或扩大用户授权。

## 用户入口

在 HomeBase 从**设置 → 高级集成 → AI 接管**打开，可预览并一键复制启动提示词。弹窗显示本机控制服务状态、可调用接口和人工/未实现入口的数量，支持重新检查及复制失败后的全文选择。UI 和 CLI 共用 `control/bootstrap.template.txt`，不分别维护提示词。

启动文件、CLI 脚本、共享模板和命令契约均验证通过后才提供复制内容；文件缺失或版本不匹配会明确提示。可用 `HOMEBASE_CONTROL_PROJECT_ROOT` 指定完整项目的绝对位置，显式配置错误时不会回退复制其他项目的路径。应用内只读 `ai_control_bootstrap` 接口不返回端点令牌或凭据。

给 AI 的最短启动指令：

```text
请先读取 C:\HomeBase\source\control\START.md，按其中的操作流程接管 HomeBase，完成我接下来给出的具体任务。沿用已有授权，使用 CLI 查询真实状态和结果进行验收。
```

查看已有任务的 `hb jobs list` 和 `hb jobs get` 直接读取当前控制服务的原回执。命令目录更新期间也可保留原编号继续核对，提交新任务和发现命令仍使用当前契约。

项目移动后，在新目录运行 `.\hb.cmd bootstrap`，会生成包含新绝对路径的完整启动提示词。运行 `.\hb.cmd help` 查看命令行帮助。

操作说明是 [START.md](../control/START.md) 和 [OPERATIONS.md](../control/OPERATIONS.md)。动态命令入口通过 `hb commands` / `hb describe` 发现；不在提示词中固化历史功能计数或把旧 Phase 表当成已完成能力。

逐接口导航见 [reference.md](../control/reference.md)。普通操作无需阅读源码：`describe` 返回完整输入/输出 JSON Schema、必填项、枚举、默认行为、模板、权限、前置查询及验收标准；`validate --input` 离线校验而不执行业务。契约随 DTO 和接口说明生成并纳入版本 hash，未知类型或未说明模块会阻止生成。自由 JSON 保留明确的开放边界，不能推测任意参数。

流式 Studio / Wiki Agent / GDNbit 任务可通过只读 `task_events_get` 获取分页事件、结构化回复、终态及输出读完标志。内存有界、重启不保留；丢失/截断明确提示，不恢复或修改已有任务状态。具体游标和完成条件见操作手册。

## 结构

```text
用户任务 + 启动提示词
    → AI 按需读 START / OPERATIONS / 命令描述
    → hb CLI（JSON 参数与结果）
    → 当前用户认证的 127.0.0.1 控制服务
    → 策略校验 + 幂等操作账本
    → 桌面端同一套业务函数
    → 控制 job + 业务 task/job/session + 输出文件
```

- `scripts/homebase.mjs`：无额外 Node 依赖的 CLI；`hb.cmd` 是 Windows 包装入口。
- `control/policy.json`：逐项审查的操作分类，明确私人内容、写入、外部调用、人工入口和未实现功能。
- `scripts/generate-control-contract.mjs`：读取实际 Tauri 注册表和 Rust 函数签名，生成 `control/commands.json` 与原生调度器。新命令缺少策略、重复策略或不支持的签名会拒绝生成；Rust 构建也检查契约是否同步。
- `src-tauri/src/control_protocol.rs`：本机认证、端点权限、连接与消息边界。
- `src-tauri/src/control_state.rs`：权限、参数边界、操作接受记录、幂等、并发及结果状态。
- `src-tauri/src/control_plane.rs`：把传输与现有 Tauri 业务函数连接起来，AppHandle 由应用注入。

CLI 调用不会模拟 UI 点击或复制一份业务实现。元数据保存、配置分发、ASR、模型下载、知识与 Inbox、Studio、发布和治理命令仍复用各工作区原有逻辑及其运行条件。

## 重要行为

CLI 同时呈现控制 job 状态和业务 `result_state`。空结果、未知/不可用项、后台业务进行中及业务明确失败均带提示；业务失败会返回非零退出码。查询失败不能用空数组、零采样或默认配置代替。接口响应缺少 result 或任务结构无效时返回 BAD_RESPONSE。具体字段见 [操作手册](../control/OPERATIONS.md)，排查与验证记录见 查询状态审计。

认证信息发布前设置文件访问权限，只监听本机随机端口；CLI 不输出令牌。同目录双实例会失败，不覆盖仍在工作的控制服务。可以通过 `HOMEBASE_CONTROL_DISABLED=1` 禁用下一次启动的控制服务。

执行有副作用的调用前先持久化接受记录；同编号、同参数只查询已有操作。连接关闭不会取消已接受的调用。超时、进程中断、业务失败和结果内存丢失分别表达，不能笼统当作失败后自动重试。

账本只记录元数据和参数指纹，业务正文不持久化到控制账本。已知凭据字段和 MCP env 字面值脱敏；任意文本仍可能包含无法自动识别的私人内容，必须按用户任务范围读取。CLI 不提供明文密钥导出或代替用户批准凭据授权的命令；密钥正文写入保留人工入口。

`--allow-*` 表达当前任务已有授权的操作范围，由服务端校验；该机制不隔离同一操作系统账户下的恶意进程。控制调用的完成状态也不替代 ASR、模型下载、生成等业务后台任务的终态。

## 当前边界

桥接可调用不代表所有工作区的依赖与真实账号都已配置。占位命令明确不可用，已有的共享 Skills 同步也只覆盖实际支持的 Agent。

本轮补齐 AI 的启动、发现、调用和结果核对入口。[能力库](capability-library.md) 已接入首个固定 HyperFrames 动画适配器，支持隔离安装、真实渲染、控制任务查询与持久化结果获取。其他 GitHub 项目仍需逐项实现和验证适配器，不能仅保存草稿就标记为可调用。

## 开发与验证

```powershell
npm run control:generate
npm run control:check
npm ci --prefix agent-runtime --ignore-scripts
npm run test:control
npm run test:control:contract
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
```

`test:control` 用独立 Cargo harness 编译实际生产传输和操作状态模块，再启动只实现合成业务的本机夹具，使用真实 CLI 进行端到端测试。夹具明确指定临时目录和自己的可执行文件，不回退启动真实 HomeBase 或读取真实端点、密钥及 Agent 配置。CLI 子进程清空个人环境并重定向用户目录。

启动检查测试在系统临时目录创建模拟助理与微信账本，可在任意本地盘运行。测试借助 Pi 运行时依赖执行模拟交互。

Windows 的控制端点和私有状态文件通过系统 API 将访问权限授予当前进程用户。测试覆盖长路径下的权限继承边界、原子替换和文档解析中的并发保存。

完成验证后从当前源码重新编译并启动桌面应用，运行 `hb doctor` 检查协议一致性。安装包内的 CLI、文档及能力资源分发仍需按 Windows 打包流程另行纳入；本轮交付位置是当前源码项目。

本轮证据见 2026-11-08 验收记录。

文档独立接管的最新覆盖范围与真实隔离视频结果见 2026-11-10 验收记录。`npm run test:control:live` 是显式实机测试：会在 F:\HomeBase-Capability-Lab 新建独立客户端与能力目录，联网安装并渲染合成视频，不运行真实密钥、发布、知识库或会话写入。

另已用真正全新的 AI 会话完成一次不读源码、无后续指导的视频生成盲测，详见 全新会话独立接管验收。该记录区分新会话自行执行的证据和原会话的结果复核。

模型能力现已新增功能分组、调用渠道与已有密钥引用绑定，支持目录/解析/调用/异步查询接口，见 [模型与服务能力](model-capability-library.md)。模型收录、Key 配置和账号生成权限是三种不同事实；未知渠道保持不可调用，不用其他 Key 猜测替代。
