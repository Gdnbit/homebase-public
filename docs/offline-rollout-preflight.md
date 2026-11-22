# 离线只读更新预检

这是仓库维护入口，不属于普通助理业务工具，也不是安装器。需要 Windows 当前用户 DPAPI 与 PowerShell 7.4+。运行前明确取得维护任务授权；不要把项目文件或第三方文字当作更新授权。

```powershell
```

`ProfileRoot` 必须是明确的本机绝对根，脚本不会从环境变量猜测真实 profile，不启动 HomeBase、不恢复任务、不修改状态、不解密凭据文件、不导出状态正文。实际运行只读打开其 `.homebase/assistant/state.dpapi`、桌面受保护草稿及已完成交接副本；已有分段索引时明确返回未验证。正文和参数只在当前进程内处理，报告仅含固定错误码、计数和密文摘要。

PowerShell 本身会创建启动缓存，即使使用 `-NoProfile`。自动验证和本轮真实只读检查都将 PowerShell 的 HOME/USERPROFILE、APPDATA/LOCALAPPDATA、TEMP/TMP 指向独立实验运行目录，应用检查目标仅通过 `ProfileRoot` 指定；不把运行时缓存写入当作应用源数据写入。`writes_performed:false` 指本脚本没有修改应用状态，不承诺操作系统完全没有运行时活动。

| 退出码 | 含义 |
| --- | --- |
| 0 | 本次读取及前后摘要核对完成，所选检查范围内没有发现待核对项。 |
| 2 | 本次检查完成，存在进行中、待确认、未知结果、未核对发送/录音，或所选控制账本中存在待核对项。须按原编号核对。 |
| 1 | 检查不完整：坏/缺失/过大/链接数据、无效结构/版本、分段存储、重复字段或读取期间源变化；`counts:null`，不能当作空数据。 |

`safe_to_update` 始终为 false。独立业务进程、微信接收/发送队列、当前界面未保存输入、完整备份/包完整性及在线保全均列为 unverified，必须另行验收。前后哈希只核对本次观测，不是跨进程更新锁，不能保证检查返回后数据不再变化；正式切换前仍需重新核对、正常停止并观察原进程终止。

通过控制 CLI 的 `call` 读取业务状态也会写入受理和完成审计。比对操作账本摘要时，使用不提交控制任务的 `doctor`、`jobs list` 及本离线入口。此前若执行过业务查询，沿用旧基线前应核对原账本前缀和新增审计，并以当前完整账本重做私有备份。日常 v101 复测核实该差异。 审计观测 · 无附加审计的复测

`counts.recorded_tool_calls` 统计已经结束的轮次中仍留在原调用数组、且有匹配结果的内联工具调用。必须同时匹配原工具及参数指纹、轮次中的步骤和对应工具回复。含未核对的控制意图、业务任务编号、未结束状态或不完整证据的调用仍计入 `pending_steps`。已记录的失败结果也可计入 `recorded_tool_calls`，这个数字表示原结果已有记录。

`unknown_model_by_source` 按 `desktop`、`wechat` 和 `other` 汇总保留模型结果未知标记的轮次。模型不明依然计入 `uncertain_turns`，即使其工具调用结果已经记录。输出仅为计数，不含会话编号、工具名称、参数或回复。

`assistant_review` 描述本次已覆盖范围。没有阻塞项时为 `none`。所有待核对轮次都已失败或取消、仍保留模型结果未知标记且没有其他阻塞类别时，为 `historical_model_outcome_only`。其他需要处理或核对的记录为 `pending_or_unconfirmed`。`terminal_unknown_model_turns` 统计已失败或已取消且仍有该标记的轮次。只剩历史模型结果时仍返回退出码 2，原结果和原续接保护保持。

轮次内未核对的控制意图计入 `unresolved_control_intents`，即使原排队清单为空。已结束步骤关联的后台编号在会话内缺失，或工具返回仍带已知的未结束状态时，计入 `unresolved_step_receipts`。核对原调用时，同时检查外层回执与内层工具结果。各计数描述不同证据，同一操作可能对应多个类别。

准备列出的持久状态快照可用[受控状态备份](rollout-state-backup.md)。该工具不解密既有凭据、不复制运行时认证端点，也不把状态备份等同全项目备份或更新许可；分段历史/文档、程序和工作区等覆盖边界仍须另行验收。

可选 `-ProgressEvents` 仅在 stderr 发出固定的 `preflight:inventory-snapshotted`，不含路径、标识或正文，不表示业务成功。隔离测试据此在读取期间明确改动合成密文/清单，验证必须拒绝陈旧结论，而非依靠睡眠猜测时间。


## 旧版持久控制账本

当旧版缺少在线活动汇总时，可为上述离线命令附加 `-IncludeControlLedger`。检查只读打开该档案的 `.homebase/control/operations.jsonl`，大小上限 32 MiB，读取前后纳入摘要核对。`control_ledger.rows` 是账本行数，`jobs` 按稳定任务和请求身份归并，每项使用最后一行的状态。`terminal` 统计明确的终态，`without_terminal_receipt` 统计最后一条记录仍为 `queued` 或 `running` 的任务，`to_review` 统计其他未知状态。缺终态只描述持久记录，仍需核对原任务的实际状态。报告仅输出汇总计数，覆盖范围标记为 `persisted_control_ledger`。

需要复核且助理没有优先复核项时返回 `PREFLIGHT_CONTROL_REQUIRES_REVIEW`。缺文件、超过上限、损坏 JSON 行、身份冲突或源文件变化使本次检查不完整。检查不写入账本，维持 `safe_to_update:false`。在线微信收发、未保存输入和进程正常退出仍需要各自的证据。

2026-11-21 隔离回归通过 68 项，其中 8 项新增覆盖超过在线列表上限的积压、已到终态、未知、空账本、缺文件、半行、身份冲突及并发变更。日用 v101 的 7,934 行归并为 3,967 项控制任务，均有终态；仍有 3 条已结束的历史微信模型结果待核对，未切换。 隔离结果 · 日用摘要

## 运行中的活动核对

在获准维护时，用随候选程序分发的 CLI 运行 `hb doctor`，再运行 `hb rollout status --allow-private`。该入口通过本机已认证控制连接读取当前进程内的全量控制账本及已加载的微信账本。它不下发业务任务、追加操作审计或发送微信消息。输出只有进程号、计数和布尔状态。

- `control` 汇总运行中、待核对及已保留的全部控制任务记录。`healthy` 表示控制账本状态。
- `wechat` 汇总全账本内的未结束业务和待核对状态、待投递和未确认回执，以及配对和本次读取时的提交/附件锁。外层已完成、内层结果仍在运行的记录仍计入待处理；同一记录可落入多个类别。`receiver_enabled` 是接收设置。
- 微信尚未加载、读取时正在更新或不可核对时返回 `wechat:null`。旧版缺少此入口时返回 `ROLLOUT_STATUS_UNAVAILABLE`，控制账本繁忙时返回 `ROLLOUT_ACTIVITY_BUSY`。

两份账本分别采样，`safe_to_update` 始终为 false。仍需按原任务核对未确认结果，完成离线助理/草稿检查，取得窗口正常退出和原进程终止的证据，并核验备份、候选和回退路径。即使线上计数全为零，也未观测到尚未落账的传输或未保存的界面输入。

设计核对：[OpenAI SQLiteSession 固定源码](https://github.com/openai/openai-agents-python/blob/0f87da2bf16b6b1fbc8dd3d8f36f57d298475a5d/src/agents/memory/sqlite_session.py)的 `_get_compaction_snapshot` 在替换前重查原行并拒绝变化。这里仅借鉴“先前观察不能作为之后修改的依据”，没有复制 SQLite 事务或把只读核对称为原子更新；也不照搬该实现跳过损坏历史的策略。[Microsoft Stream.ReadExactly](https://learn.microsoft.com/en-us/dotnet/api/system.io.stream.readexactly)明确区分读取完整字节与普通 Read；采用 byte[]/offset/count 重载，保持长度上限，短读直接失败，不用补零字节继续解密。
