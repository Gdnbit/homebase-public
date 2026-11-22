# 快捷面板任务恢复

页面恢复的含义是重新查看任务、恢复当前实例仍持有的监控关系。打开、切换页面和手动重查均为只读操作。

## 原始状态与运行观察

`asr_task_reconcile(output_root)` 和 `model_download_reconcile(jobs_root)` 保留旧接口名称，已移除全部任务状态写入。返回原始任务字段，并增加仅存在于响应中的两个字段：

| runtime_state | 含义 | 页面处理 |
|---|---|---|
| managed | 活动记录匹配当前实例持有的任务，且状态文件路径一致 | 恢复监控和完成事件处理；取消仍须用户明确点击 |
| unmanaged | 磁盘记录是活动状态，但当前实例没有其受控子进程记录 | 显示“任务状态未知”，不监控、不取消、不重启，保留原始记录 |
| terminal | 记录已为 completed/succeeded/failed/cancelled/interrupted/verification_failed | 只读展示终态，不恢复执行 |
| unknown | 无法识别的状态 | 明确提示未知，保留原记录，阻止误发起新任务 |

`runtime_message` 给出说明。不会持久化这两个观察字段。原始 `status`、phase、PID、revision、error、updated_at/updatedAt、completed_at/completedAt 和任务包元数据均不会被恢复查询修改。

“当前实例未持有”不代表进程已死：任务可能属于其他 HomeBase 进程、外部启动器，也可能经历重启、崩溃或 PID 重用。恢复检查不通过磁盘 PID 猜测所有权，不据此发送终止信号，也不把 running 自动改成 interrupted。

旧版本已经写成 interrupted 的记录不自动回改，避免用另一个猜测覆盖历史状态。

## 前端与 CLI

- 新挂载、任务根目录变化、手动重查时读取当前只读观察；同时发生的请求共用 Promise。
- 迟到的旧页面或旧目录响应被丢弃；旧目录的任务不会被接到新目录的监控关系中。
- 只有 managed 活动任务启用进度轮询和取消按钮。未确认任务、读取失败或检查尚未完成时，暂不允许从该面板启动新任务，避免重复执行。
- 通过“重新检查下载任务状态”/“重新检查转写任务状态”重试只读检查。它们不修复或重写状态文件。确认终态由实际 worker 写入后，重查可清除未知提示。
- 缺失目录不创建目录；损坏、截断、不可读取或编号与文件名不匹配的记录返回错误，不能假装没有任务。
- CLI 两个接口只需要 private 范围，不要求 write；未托管或未知记录的 `result_state` 为 partial，即使原始 status 是 running。
- 显式开始、取消和正在执行的 worker 仍可以维护自己的任务文件；这些行为不是页面恢复的副作用。

## 隔离验收

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --lib task_recovery -- --test-threads=1
npm exec playwright test -- --config playwright.ai-control.config.ts recovery.spec.ts updates.spec.ts
npm run test:queries
npm run test:control
npm run build
node scripts/test-task-recovery-live.mjs --isolated-only
```

原生测试使用临时目录和合成任务，比较文件清单、字节及修改时间；用于模拟受控子进程的程序仅为测试可执行文件的 `--list`，清空环境且丢弃输出，不运行模型、ASR 或密钥服务。界面夹具不连接真实后端，阻止外网，验证未触发 start/cancel/organize。

实际后端/CLI 的 isolated-only 脚本需要最新版桌面程序启动，只创建自己的合成目录，不接受真实任务根目录。完整结果见 本轮验证记录。
