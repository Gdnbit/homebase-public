---
name: reminders
description: 用户要求到时提醒、重复提醒或取消提醒时使用。
---

先用 homebase_reminder(action=list) 取得 revision。时间含糊时询问具体日期、时间和时区；不能猜测用户所在地区。

create 使用 text、due_at（RFC3339，包含时区）、revision，可选 interval_minutes。须等待真人确认。这里只创建文字提醒，不执行生成、发邮件、转账或其他业务。微信创建的提醒绑定原本人渠道，桌面创建的提醒在桌面显示。

提醒要求 HomeBase 运行。关闭后不执行，恢复后检查漏跑；重复提醒跳过积压，不连续轰炸。取消使用实际 id 和最新 revision 并等待确认。发送结果 unknown 不自动重发；accepted 仅表示发送接口接受，不等于手机已读。

如果用户要求“到时帮我处理/检查并把结果发来”，使用 homebase_followup，而不是把业务指令塞进文字提醒。先 list 获取该工具独立 revision，create 提供 title、instruction、due_at、max_runs；一次 max_runs=1，重复时额外提供 interval_minutes（至少5分钟，最多30次）。时间不明确先询问，不猜测。create/cancel 等待本人确认，记忆或Skill不能代替确认。

主动任务只能使用 Pi 当前已启用的明确免费渠道。首次在未来30天内；应用退出不运行，原微信必须在线，离线超过24小时则暂停，重复任务不补跑积压。模型或本机授权版本变化时暂停，不自动换模型或凭据。每次执行仍受现有工具确认限制，任务不能递归创建任务。list 可查原 turn_id，失败时查原任务，不重新创建相同任务。

当前工具提供 trigger 时，可以安排事件跟进：create 加 trigger={turn_id:本人原任务编号,on:"completed"或"needs_attention"}，max_runs=1，不设重复间隔。due_at 此时是等待事件的最后有效期；只监听尚未发生的目标状态。任务状态变化唤醒执行，不反复请求模型检查。本人确认后才登记，不能监听其他来源、普通讨论或主动跟进任务，也不能递归创建监听。completed 复用原会话结果；needs_attention 使用已核验进度快照，不擅自批准原任务。

notification_policy 默认 meaningful，成功的只读检查结果与上次相同时安静保留，查询原任务仍能得到结果；明确要求每次汇报才使用 all。只有结构化真实证据参与筛选，不凭模型声称“没变化”静默。确认、异常、写入、新附件以及本人主动问进度/结果不会被这项筛选隐藏。旧检查点没有这些参数时不要传入。
