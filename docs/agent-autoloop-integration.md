# Agent Autoloop 开发流程接入

本次接入使用独立的用户级中台与外部 CLI，暂不修改当前桌面运行程序、原生 Agent SDK、助理任务引擎、微信通道或现有控制契约。

## 已有入口

```powershell
node scripts/autoloop.mjs status
node scripts/autoloop.mjs smoke my-smoke development-local
node scripts/autoloop.mjs result my-smoke
node scripts/autoloop.mjs answer <卡片编号> <选项序号>
node scripts/autoloop.mjs cancel <运行编号>
```

本机注册位于用户 `.agent-autoloop/workbench/entry.json`，只包含运行时和适配代码路径，不保存厂商凭据。其入口代码位于 `C:\Users\homebase\Documents\ChatGPT\个人工作台\ops\autoloop`，模板可以替换各阶段 Agent。

## 当前实现与限制

- 上游中台处理事件、派工、通知、续接、去重与运行用量；本适配增加稳定任务包、开发阶段与宿主验收。
- 实际任务需显式传入独立目录、需求文件、工作流和可信检查命令文件；源码已有未提交改动时不自动创建或切换 checkout，不提交或发布。
- 只有宿主检查通过才是 verified；没有检查命令是 needs_acceptance，复核不通过或输入被改动是 blocked。
- `development-local` 的三个角色是独立 Codex 会话。`development` 的跨 Claude/Codex 路线依赖真实认证，不把程序可启动或 auth status 的静态标记当账号可用。
- 能力库新增“开发”表单与 development_catalog/run/result/answer 窄接口；复用现有控制面、本人确认及助理任务归属。CLI桥接仍可独立使用。原生Agent历史与全局事项卡片同步尚未接入。参数和恢复见[开发能力](development-capability.md)。

## 接入与后续范围

1. HomeBase 持有用户意图与授权；Autoloop 负责一个明确的开发执行子任务。固定保存 HomeBase turn/job 与 Autoloop task/event/run 的映射。恢复只查询原编号，未知结果不重提。
2. 已新增提交、目录、结果和回答接口，沿用command/schema/policy生成；未开放任务级取消、自动重试、任意shell或owner token。验收argv由具体授权请求提供，不由外部事件正文直接执行。
3. 外部任务必须有真实测试、差异、产物与模型来源。它们尚未注册为 HomeBase step_id，不能直接冒充现有 homebase_task 的完成证据。
4. 保留 HomeBase 的凭据引用与本人确认；同一动作只能由一个系统批准和执行。不要把一个系统的“同意”解释为另一个系统全部授权。
5. 路由策略应根据任务类型、实际验收表现、耗时、账号用量和已知价格选择；静态角色映射是初始策略，后续再考虑自动升级、失败后换 Agent 和预算上限。

使用随运行包的接口契约；源码更新不等于运行程序已更新。切换前确认无在途任务并按现有备份/回退约定处理，不强制重启正在执行工作的程序。

## 验证入口

适配入口语法检查：`node --check scripts/autoloop.mjs`。宿主验收的隔离测试见外部入口目录的 `workbench.test.mjs`。部署版本、实际验收结果及认证问题以当日部署记录为准，不在此固化为永久状态。
