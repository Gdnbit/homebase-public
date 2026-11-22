# HomeBase

**Windows 上的本地 AI 助理和能力工作台。**

在桌面或微信里用自然语言交办任务，HomeBase 会按用途查找可用的模型、MCP、Skill 或本机能力。任务受理后可以查原进度、恢复中断的流程并取回产物。

## 界面预览

![HomeBase AI 助理首页](docs/screenshots/assistant.png)

以上为合成数据界面。 [查看能力库截图](docs/screenshots/capabilities.png)

## 主要能力

| 模块 | 可以做什么 |
| --- | --- |
| Pi 助理 | 连续对话、上下文摘要、个人记忆、提醒和任务恢复 |
| 微信接入 | 本人扫码绑定，聊天、交办、查询进度与接收结果 |
| 能力库 | 按功能查看本机工具、模型服务、MCP、Skill 和已注册的仓库能力 |
| 会话与工作台 | 查看来自桌面、微信和已接入原生 Agent 的会话与运行状态 |
| 本机 CLI | 让其他 AI 通过命令契约发现能力、校验输入、查询原任务及结果 |

## 从源码运行

准备 Windows x64、Node.js 22.19 或更新版本、Rust stable，以及 MSVC C++ Build Tools 和 Windows SDK。在仓库根目录执行以下命令。

```powershell
npm ci
npm ci --prefix agent-runtime --ignore-scripts
npm run tauri:dev
```

前端开发可用 `npm run dev`。浏览器中的界面预览无法调用 Tauri 原生工具。首次启动后，按需在设置里配置模型、绑定微信或开放本机工作区。完整安装包需要单独准备固定版本的运行时。 [Windows 打包](docs/windows-packaging.md)

## AI 和 CLI

在桌面 **设置 → 高级集成 → AI 接管** 复制启动提示词，或运行以下命令。

```powershell
.\hb.cmd bootstrap
.\hb.cmd commands
.\hb.cmd describe capability_index_query
```

在线任务由正在运行的桌面程序处理。启动提示词和接口文档包含命令、参数、授权及任务回执说明。 [AI 操作入口](control/START.md) · [CLI 手册](docs/ai-control.md)

## 验证与文档

```powershell
npm run build
npm run lint
npm run test:control:contract
npm test --prefix agent-runtime
cargo check --locked --manifest-path src-tauri/Cargo.toml --all-targets
```

- [助理与微信](docs/assistant.md) · [微信绑定与使用](docs/wechat-integration.md)
- [能力库](docs/capability-library.md) · [能力发现接口](docs/capability-discovery.md)
- [架构](docs/architecture.md) · [维护迭代](docs/maintenance-goal.md)

公开仓库包含源码、合成测试和接口文档。模型权重、凭据、个人配置、会话、真实产物和可执行分发包由使用者另行准备。第三方组件按其各自的许可提供。 [第三方说明](packaging/windows/THIRD-PARTY-NOTICES.md)
