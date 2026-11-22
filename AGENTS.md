---
doc_id: homebase-agents
project: HomeBase
title: HomeBase Agent 规则
updated: 2026-07-19
authority: authoritative
status: current
volatility: periodic
sensitivity: public
review_after: 2026-10-18
tags: [homebase, agent-rules, tauri, react, rust]
---

# HomeBase Agent 规则

本文件适用于仓库根目录及其子目录。它规定工作方式和安全边界，不维护
路由数、command 数、代码行数、工具版本、构建通过数、阶段完成度或安装位置。

## 权威顺序

1. 当前用户指令和本文件。
2. `package.json`、lockfile、`src-tauri/Cargo.toml`、Tauri/Vite 配置和实际代码。
3. 当前任务相关且能与代码相互验证的项目文档和测试。
4. 当前 `README.md` 只作项目入口；旧 ROADMAP、Phase 表和本机指南中的 HomeBase 设计文档均是候选/历史资料。

当前实现与旧设计冲突时，以代码、配置和本轮验证为准，并更新直接受影响的当前文档。

## 开始任务

1. 运行 `git status --short --branch`，保护用户现有未提交改动和未跟踪文件。
2. 读取当前任务涉及的 manifest、配置、代码和最近文档；不要预读整个本机指南或所有历史设计。
3. 非平凡任务先写目标、范围、预计文件、风险和验证步骤。
4. 本机路径、工具和运行状态通过 `local-machine-context` Skill/探针现场获取。
5. 不从 README 徽章、Phase 名称、文件 mtime 或目录存在推断功能已完成。

## 实现规则

- 复用现有命令、adapter、route、component 和 IPC 模式，保持 diff 小且可审查。
- 前后端命令、事件、schema 或持久化契约变化时同步两端类型、测试和直接文档。
- 不把历史设计中的模块、路径、provider 或共享层自动补回当前代码。
- 不为文档整理顺手升级依赖、重构架构、改变 UI 或生成分发包。
- 生成物、依赖、缓存和 distribution 只视为清理候选；删除需单独授权。

## 秘密与隐私

- 不读取、输出、记录或复制 `.env*` 中的真实值、SOPS 解密结果、令牌、私钥和数据库连接串。
- 秘密只通过本机受控包装器注入目标子进程；不写入 SQLite、日志、剪贴板、前端状态或错误消息。
- `C:\HomeBase\Private`、`C:\HomeBase\Wiki\raw` 与遗留 `C:\HomeBase\Wiki\_raw` 不得被 HomeBase 通用索引批量读取。
- 涉及模型/provider/账号/配额/价格时从当前配置与官方来源核验，不在项目规则固化快照。

## 外部状态

以下操作需要任务明确授权和回滚计划：部署/发布、推送、数据库写入、账户操作、凭据轮换、
系统服务/防火墙/环境变量修改，以及向云端上传私人素材。UI 按钮或现有 command 存在不构成授权。

## 环境与验证

### AI 操作入口

操作 HomeBase 运行中功能时从 `control/START.md` 进入，按需读取 `control/OPERATIONS.md`。
优先用 `hb.cmd` 或 `node scripts/homebase.mjs` 的命令发现、调用和 job 查询；先运行 doctor，
不能把 CLI 可调用、任务编号或占位返回当成业务成功。不要读取控制端点文件内容给模型。
新增/修改 Tauri 命令时审查 `control/policy.json`，运行 `npm run control:generate`，同步生成
契约与原生调度器，再执行 `npm run test:control` 和相应原生检查。测试使用合成夹具，不以
真实密钥、Agent 配置或生产写入做冒烟。

运行环境必须现场查询：

```powershell
Get-Command node,npm,rustc,cargo -All -ErrorAction SilentlyContinue
node --version
rustc --version
npm run
```

需要 MSVC/Windows SDK 时通过 Visual Studio Installer、`vswhere` 或当前 Developer Shell 发现，
不得固化某次版本或安装目录。

按改动选择最小验证：

- TypeScript/React：当前 manifest 中的 lint、typecheck、test 或 build。
- Rust/Tauri：受影响 crate/module 的 fmt、check、test；需要打包时再执行完整构建。
- UI：启动当前开发入口并人工检查受影响流程、空/加载/错误态与可访问性。
- 文档：链接、frontmatter、命令和实现锚点对账。

验证失败先修复或明确阻断，不用历史通过记录替代本轮结果。

## 文档状态

- `docs/quick-panel.md` 等混合设计/实现记录若标为 `needs-review`，只用于发现候选锚点。
- 本机指南的 `homebase-design*`、带日期进度/冒烟报告和竞品研究均为 historical/do_not_execute。
- 结构或决策发生变化时更新最近的当前文档；瞬时版本、计数和运行结果进入带时间戳报告。
