# 隔离 UI 回归

`npm run test:control:ui` 启动本地合成 IPC 夹具并使用独立浏览器上下文。用例阻止非 localhost 网络访问，不连接真实 HomeBase、微信或模型。实际原生窗口/CLI测试是另一层，不能用此夹具代替它们。

当前首页为 AI 助理。主导航使用会话、能力库、工作台、Agent 管理、资料库、密钥与连接、设置和微信连接。旧历史时间线、文章、投料与快捷面板仍从“设置 → 已有工作入口”进入；系统和操作记录也从设置进入。测试旧功能时用 `navigation.ts` 的真实 SPA 导航，不使用 goto/reload 代替页面切换，以免重建 QueryClient 而使缓存、订阅与恢复检查失真。

共享 MCP/Skill 在能力库的“工具与工作流”，打开前不应加载。启动提示词组件位于设置的“高级集成”；`fixture.tsx` 显式挂载同一集成组件以独立验证复制、缺失资源、失败重试和焦点，不假定 Sidebar 仍提供旧按钮。`ai-first.spec.ts` 检查实际设置入口。

`performance.html` 使用真实应用路由和合成 IPC。桌面保护状态的跨 reload 用例由 Playwright 显式保存、注入合成快照，不将生产草稿写入 localStorage/sessionStorage。能力库的既有草稿测试与会话加密草稿是不同功能，不混用其存储契约。

只选择部分文件运行时，报告必须写明范围。历史夹具与旧布局不一致产生的失败应修正到当前真实入口；不得恢复过时布局、删掉业务断言或把非空错误视为空数据来让测试通过。完整验收还需相关原生、CLI、协议及保全检查。

`native-presentation.spec.ts` 验证原生分块完整可达、非文本/无效格式提示、阅读旧消息与分页锚点、共享关联失败、UTF-8有界交接及加载缓存复用。它使用合成SDK结构；实际SDK格式、标题来源、关联元数据不可读和真实WebView需另用 `scripts/test-ai-first-native.mjs` 的受保护隔离模式验收。只在显式实验根中设置 `HOMEBASE_NATIVE_RECEIPTS_TEST=1`、`HOMEBASE_NATIVE_PRESENTATION_TEST=1`，不可对真实profile运行故障注入。
