# 开发能力

能力库 → 按功能发现 → 开发。填写已有项目绝对目录、开发需求和协作流程，按需填写验收命令及受保护验收文件，再点“运行开发”。这会在选定项目产生实际改动并调用现有模型账号；不自动提交、推送或发布。不要把密钥填入需求或命令。

默认 `development-local`：三个独立 Codex 角色依次规划、实现、审查，再由宿主执行验收；不是跨厂商协作。明确的小任务可选 `execution-checked`，省略规划和独立审查。`development` 使用现有 Claude/Codex/Claude 配置，须先解决对应凭据和模型权限。模型、推理强度、单轮超时沿用 Autoloop 已登记角色，本次没有新增全任务费用硬上限；费用未知不等于免费。

验收命令为 JSON 二维参数数组，例如 `[["node","--test","acceptance.test.mjs"]]`，宿主在项目目录以 shell=false 执行。需要真实可执行文件；Windows 的 npm.cmd 等脚本不应当作可执行程序，使用 Node 加 npm CLI 的实际路径。可将已有验收文件列为受保护文件，结束时核对摘要，禁止悄悄改测试使其通过。没有验收命令只会得到 `needs_acceptance`。受保护文件必须已存在且位于项目内。

## 接口

先使用随运行包的 `hb.cmd doctor` 和 `describe COMMAND` 核对实际程序契约；源码更新不等于日常程序已更新。

| 命令 | 输入 | 含义 |
| --- | --- | --- |
| development_catalog | 无 | 依赖、可选流程、最近20个任务摘要；不启动模型 |
| development_run | request | 提交一次开发，返回稳定task_id |
| development_result | taskId | 查询原任务、问题与验收，不重做 |
| development_answer | request={task_id,card_id,option} | 回答原任务当前卡片，option从1开始 |

提交示例（存为普通UTF-8 JSON文件，再通过 `--input` 传入，不把正文拼到命令行）：

```json
{
  "request": {
    "request_id": "my-development-once",
    "workspace": "C:/HomeBase/Projects/example",
    "brief": "在现有项目中实现明确需求；说明允许范围和验收条件。",
    "workflow": "development-local",
    "checks": [["node", "--test", "acceptance.test.mjs"]],
    "protected_files": ["acceptance.test.mjs"]
  }
}
```

```text
hb.cmd validate development_run --input request.json
hb.cmd call development_run --input request.json --request-id control-development-once --allow-write --allow-private --allow-external
```

控制job受理与开发task_id是两层身份。`submitted`不代表开发完成；继续按同一task_id调用result。只有`verified`表示实际宿主检查通过，`blocked`需读失败，`awaiting_input`需回答，`unknown`只按原编号核对，不创建新请求来盲重试。助理提交由宿主绑定稳定请求身份，查询和回答限于本会话已登记任务；实际写入仍沿用具体操作确认。

## 恢复与边界

桌面在IPC前保存请求到本机 localStorage 的 `homebase.development.pending.v1`。切页/重启后点击“查询原请求”仅恢复查询；同一请求不同参数报冲突，重复调用不启动第二条链。新建任务只清除当前表单关联，不取消原任务。浏览器存储损坏时禁止覆盖；工作区文件与任务账本不在此存储中。

私有任务账本位于用户目录 `.homebase/capabilities/development/dev-…`，工作产物位于项目 `.autoloop/tasks/dev-…`；Autoloop仍维护其原账本、会话、卡片和调度。提交结果未知时保留原编号；本版没有暴露任务级取消或自动重试，原进程/操作锁状态不明时也不自动清锁。需要停止时使用既有Autoloop运行轮次入口核对，不能假设停止一轮已经取消全部下游阶段。

验收结果由宿主先保存到项目外 `.agent-autoloop/workbench/results` 的逐任务回执，再发布工作区视图；旧任务兼容原账本摘要。查询核对结果和阶段产物摘要。输出预览每条最多16000字符，完整输出保存在result_path；output_truncated显式标明。verified记录该轮结束时的检查结果，后续修改业务代码需要重新验收，不保证当前任意代码仍通过。摘要不是恶意代码的安全沙箱。任务、文件和产物不会自动清理。

当前卡片回答接口提供预设选项；自由文字补充可使用原Autoloop的`answer CARD --text 内容`入口。每条宿主验收命令沿用现有60秒限制，长构建需要另行调整受控配置，不能把超时当通过。

依赖现有本机部署注册 `.agent-autoloop/workbench/entry.json`，不会安装、升级或重配上游服务/密钥。打包携带开发适配脚本，目标机器仍须有已部署并验收的Autoloop工作流。当前本机适配器扩展submit-json结构化入口；不改变上游仓库。
