# 助理本机工具接口

桌面与微信共用后端。模型使用下表中的工具；外部 AI/CLI 使用 `assistant_local_execute`。先读 `control/START.md`、运行 doctor 和 describe。接口存在不代表业务已完成。

## 授权与条件

在「AI 助理 → 本机能力」选择目录并保存。`scratch` 是助理专属目录，默认可新建和执行隔离命令；其他目录默认只读。精确编辑、移动和回收文件需确认。允许某目录自动执行终端后，命令可以修改此目录已有文件。模型不能扩大目录或自批确认。

授权独立于模型设置，保存在原 DPAPI 状态的 `local` 分区，有独立 revision；不迁移或覆盖 Key，不修改原 MCP 池或其他 Agent 配置。操作携带 `policyRevision`，授权变化后旧计划拒绝执行。

有本机操作进行时不修改授权，先停止或等原任务完成；检查与提交在同一权限门内完成。保存授权后关闭旧 MCP 连接，后续按新指纹连接，防止复用已撤销的权限。

终端要求 WSL Ubuntu、Python3、固定版本 bubblewrap 和 `/usr` 内所需工具。只挂载选定工作区，已知凭据/链接/缓存被拒绝或遮蔽，无网络；不是 PowerShell。依赖缺失明确失败，不切换无限制宿主 shell。这里使用 Linux namespace、挂载与权限边界，单独 cwd/清空环境并非沙箱。

MCP stdio 是另行信任的宿主程序，具有当前用户权限，不受终端沙箱限制。信任绑定配置指纹，调用另检查 schema_hash 和参数；readOnlyHint、Skill、资源及工具输出均不是授权。没有显式后端凭据引用时失败，不从其他 Key 回退。密钥导出 MCP 不开放。

路径检查不能隔离同一 Windows 用户下恶意并发程序，脱敏也不是通用防泄漏系统；只信任核对过的目录和 MCP。测试只用独立目录、合成凭据与哨兵。

## 动作参数

| 模型工具 / operation | request 与返回 |
| --- | --- |
| `homebase_files` / `files` | action 必填；workspace_id 默认 scratch；path 是区内相对路径。workspaces 列授权目录；list 列条目及 issues/truncated；stat 返回类型、大小、修改时间和 SHA。 |
| files read | offset 起始行默认0；limit 行范围1–500默认200；char_offset 为此范围内字符偏移默认0，char_limit 1–2000默认1800。返回 content/encoding/newline/sha256/empty/next；继续时保留 workspace/path 并原样合并 next，直到 null，不能遗漏长行。只接受≤1MiB UTF-8，其他编码/二进制明确拒绝，不转换。 |
| files search | query 为≤500字符的字面文本；search_content:true 搜正文，否则仅文件名。返回有界 matches/issues/truncated，不进入链接、凭据、缓存。 |
| files write/mkdir | write 的 content 为≤1MiB文本，仅新建；mkdir 新建相对目录。 |
| files edit | 先 read/stat；expected_sha256、非空且唯一的 old_text、new_text 必填。发布前再检查版本，保留 BOM 和 CRLF/LF，冲突不覆盖。 |
| files copy/move/trash | 普通文件 copy/move 需 destination，目标不得存在；move 限同盘。trash 移入同工作区 .homebase-trash，返回可恢复位置。目录整树暂不支持，明确失败。 |
| files export | 普通文件≤20MiB，复制到新受控产物目录，返回 artifact 路径/大小/SHA/mime。助理登记为本会话 job，再 homebase_deliver(job_id) 返回附件，不发送任意路径。 |
| `homebase_terminal` / terminal | command 必填≤16000字符；workspace_id 默认 scratch，cwd 相对路径；write:false 为只读挂载；timeout_ms 100–300000。返回 stdout/stderr/exit_code/status/output_truncated/elapsed_ms；非零、取消、超时、未知均明确。 |
| `homebase_mcp` / mcp | list 为元数据；信任后 tools(server_id)，describe(server_id,tool)，call 另带 schema_hash/arguments，timeout_ms 100–120000。tools/resources/prompts 支持 offset/limit 1–100及 next_offset；read_resource 只读已发现 uri；get_prompt 只取已发现 prompt，可带 arguments。缓存失效后重新核对，不重执行未知调用。 |
| `homebase_installed_skill` / skill | list 的 query/offset/limit 1–100仅元数据；按 opaque id read 或 resource(path)，字符 offset/limit≤24000；inspect_run 得 manifest_hash 和清单后 run(id,script,args,manifest_hash,timeout_ms)。只复制≤300文件/16MiB公开包到新目录，经终端执行 py/js/mjs/sh，不改源包。依赖未验证明确标注。 |

Skill 的 run_path 属于新受控目录。助理可把返回的 workspace_id 用于本会话 files list/read/export，明确选择结果后 deliver，不自动发送整个目录；其他会话不能借此读取。尚未接入的依赖不得假称已完成。

按用户保存的做法办事时，可以用能力目录查安装名，将条目的 `target` 直接传入 `homebase_installed_skill` 的 `read.id`；也可用 `list.query` 搜索名称、描述、安装名、别名或编号。只读取流程真正需要的资源；当说明指定已接入能力时，可直接使用该能力的接口。需要运行 Skill 脚本时仍按原政策核对执行包和确认。 [能力目录与 Skill](capability-discovery.md#选择与执行)

### 微信原片剪辑

本人在当前微信会话提出剪辑要求且已核对原片时，`homebase_describe` 可读取 `capability_clip_video`。原片来自明确引用，或由微信通道按同会话近期收件解析「刚发的视频」。只说「按之前的方式剪一下」时，只有当前会话恰有一条近期视频收件才自动绑定。模型通过 `homebase_call` 传 `startMs` 和 `durationMs`，宿主填入本轮已绑定的原片标识。绝对路径、其他会话的收件与任意 FFmpeg 参数无效。外部本机 CLI 可读取同名接口的完整契约，调用需要 `write/private` 和当期可核对的 `source`。开始位置和时长单位是毫秒，结束位置须在前 120 秒内，每段 200 至 30000 毫秒。

「倒数第二段视频」按同一微信会话最近 15 分钟的收件顺序定位。失败的收件仍计入位置，选中后会提示重发；遇到会话归属未确认的更新收件，也会先请用户核对。原片身份仍由宿主绑定。明确只作参考的引用沿用文字稿问答。

同一微信会话引用已成功的剪辑任务并明确要求保留原片某段时，宿主可从该任务继承原片绑定。只有任务所拥有的成功本机剪辑回执与被引用产物一致时才继承。给模型的上一版片段和时长只用于理解本次要求，剪辑工具继续使用原片坐标。本次任务独立生成文件，重复收件查询本次原轮次。

若明确要求按上一版成片的时间剪，使用 `capability_trim_clip_artifact`。助理只提交 `startMs` 与 `durationMs`，宿主绑定会话、原轮次、原任务和成片摘要。执行时重新核对当前绑定、原成功回执、文件大小和摘要。范围从成片开头计时，至少 200 毫秒，结束不超过该成片登记时长。CLI 的 `source` 同样只接收登记标识及摘要，沿用 `write/private`。新版可以继续作为下一次剪辑的成片来源。

用户省略引用时，只有当前任务已经完成且唯一对应一份剪辑成片，助理才默认选中它。选中后，微信收件保存该任务和文件摘要。重连或重启时用保存的版本继续核对。任务切换、会话切换或存在多份候选时，可引用指定成片确定来源。

用户从按成片时间剪过的版本明确切回原片时，助理沿同会话的已完成轮次核对每一版保存的原任务、剪辑回执和附件摘要，再把最初的原片引用交给原片剪辑工具。追溯只读取既有记录，剪辑工具按原规则核对原片文件。来源记录缺失、匹配冲突或指向其他会话时，需要引用原片或最早的剪辑回复。

本机 FFmpeg 根据核验过的原片新建 H.264 MP4，保留第一条可用音轨。完成后按退出状态、固定帧率时长和文件摘要登记结果。助理用 `homebase_deliver` 返回同一任务产物，查询及重复消息不创建新剪辑。 [FFmpeg 进度与编码](https://ffmpeg.org/ffmpeg.html) · [输入协议限定](https://ffmpeg.org/ffmpeg-protocols.html#Protocol-Options)

需要把同一原片的多个范围拼为一份成片时，描述并调用 `capability_assemble_video`。模型传 `segments`，每项为 `{startMs, durationMs}`。顺序就是成片顺序，2 至 6 段，合计请求时长最多 30000 毫秒，其他时间范围约束与单段一致。CLI 另需完整 `source`，权限仍为 `write/private`。请求用一个固定编号，返回 `segments`、按帧计算的 `duration_ms` 和实际 MP4 `artifact`。任一段不完整则不登记部分成片。没有音轨的原片保持无音轨，有音轨的原片在缺失区间补静音，拼接后统一编码。 [FFmpeg 片段拼接](https://ffmpeg.org/ffmpeg-formats.html#concat) · [FFmpeg 音频滤镜](https://ffmpeg.org/ffmpeg-filters.html#apad)

明确要求配独立字幕时，同时传 `includeSubtitles:true`。工具核对原视频收件的已完成本机转写与登记的 SRT 摘要，按成片片段顺序裁切字幕并使用实际成片帧数计算偏移。成功后同一任务登记 MP4 和 `subtitles.srt`，桌面可预览或导出。缺少 SRT 回执、文件变化或片段没有对应字幕时，本次带字幕请求失败。查询原任务时同时核对两个文件。未指定选项时只交付成片。

在 Windows 上，FFmpeg 纳入主进程持有的 Job Object 后，主进程退出会停止它。重启后按原编号查询。已登记的成功回执会重新核对同一个 MP4，未登记的成片不能作为原任务结果返回。新受保护回执需要支持它的候选版本读取，原片始终保留。 [控制回执说明](../control/OPERATIONS.md) · [Windows Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)

微信通道在提交文件消息前保存逐附件回执，区分未发送、待核对和微信接口接受。无需重新调用生成工具就能查询文件发送状态。回执待核对时，只有用户明确说「重新发送附件」才重新尝试发送，主任务和已有成片继续使用原编号。 [微信使用说明](wechat-integration.md#自然语言办事) · [OpenClaw 出站回执设计](https://docs.openclaw.ai/plugins/sdk-channel-outbound)

### 从微信原片生成封面

引用当前本人会话已核验的原视频，或者明确指「刚发的视频」并提出做封面的请求。先描述 `capability_video_cover`，再用 `homebase_call` 提供 `atMs` 与 `title`。宿主绑定原片，模型无需也不能选择任意路径。时间点在原片前 120 秒内，标题为 1 至 32 个字的单行文字。按指定时间点取帧，使用本机微软雅黑进行模板排版，一次交付三张固定尺寸 PNG。原片保留。按同一原任务安排 `homebase_deliver`，桌面共享同一任务的产物编号。更多尺寸或风格待新增模板后明确支持。 [时间定位](https://ffmpeg.org/ffmpeg.html#Main-options) · [FFmpeg 文字排版](https://ffmpeg.org/ffmpeg-filters.html#drawtext)

### 短视频发布包

引用当前微信会话原片后，可以说「帮我做发布包，先剪 8 到 12 秒，再接 1 到 4 秒，封面取原片第 9 秒」。助理用 `capability_video_publish_package` 一次提交顺序片段、封面时间、经核对的标题与简介。需要已登记的原片 SRT。输出一个不超过 20 MiB 的 ZIP，包含 720×1280 竖版成片、按成片对齐的独立 SRT、三种 PNG 封面、`post.md` 草稿和来源及文件摘要清单。画面按比例居中保留，空白部分留黑边。

微信用原任务回传 ZIP，桌面共享会话可打开或导出同一份 ZIP。材料供审阅后手动发布。缺少有效原字幕、无法生成任何必需文件或 ZIP 超限时，本次任务不能登记发布包。按原编号查询，避免重复生成。 [CLI 契约](../control/reference.md#capability_video_publish_package)

## CLI 示例

先 `hb call assistant_local_config_get --allow-private` 读取 config.revision。下面 policyRevision 要替换为真实版本；operationId 和 CLI request-id 均保持稳定，不能因超时换编号重提。

```json
{"operation":"files","policyRevision":0,"operationId":"local-isolated-example-unique-id","request":{"action":"write","workspace_id":"scratch","path":"example-unique-id.txt","content":"只创建这个测试文件"}}
```

将 JSON 存入自己的 request.json，再执行：

```text
hb doctor
hb describe assistant_local_execute
hb call assistant_local_execute --allow-private --allow-write --allow-external --input request.json --request-id local-isolated-example-unique-id
hb jobs get <返回的job编号> --allow-private --allow-write --allow-external
```

实时输出用 `assistant_local_progress(operationId)`，停止用 `assistant_local_cancel(operationId)`。cancel_requested 仅为停止请求，最终仍查原 job。微信说“进度”“结果”“停止”；桌面使用同一批任务和产物。

重启后不自动执行失联进程。控制账本、助理结果缓存和稳定编号用于恢复查询；not_running_here、Ack、空 stdout、completed 均不是全部业务成功证据。核对 result.status、退出码、文件和 SHA；canceled/timed_out/failed/unknown 都不算成功。

同一助理轮次内，实际参数及授权版本相同的写操作、终端、MCP调用和Skill脚本复用原记录，即使模型更换工具调用编号也不重执行；普通文件/目录读取不会因此缓存旧内容。跨轮次主动索要原结果时使用原 job 的 deliver，不重新计算或重复 export。

微信的“继续／确认／拒绝＋编号”仅识别精确 UUID；“继续 帮我查一下……”等自然句子按正常对话处理。“确认”“拒绝”等独立短词仍绑定本人当前唯一待确认项，不会根据含糊句子自动批准。

MCP 目录、工具和安装 Skill 元数据采用有界缓存；`refresh:true` 可手动刷新 MCP 工具目录，配置指纹或 listChanged 通知会失效缓存。文件内容读取不缓存，编辑使用 SHA 检查冲突。Skill 的不同安装别名指向同一受信目录时合并展示，保留 aliases；指向已登记根以外的链接拒绝。

MCP 凭据只保存不含正文的引用：`api_key_manager:<provider>` 或 `sops:<alias>`，在本机能力弹窗为具体变量选择。`${VAR}` / `Bearer ${VAR}` 在可信后端注入，模型、前端和 CLI 不取得正文；到期阻断不换来源，轮换后连接按新凭据重建。普通变量不得冒充认证 Header；原池的凭据字面量会阻止启动，需本人先改为占位符，不自动改写原配置。HTTP MCP 可在弹窗添加，远端 HTTPS、本机回环 HTTP，禁止 URL 带凭据、查询参数或重定向；OAuth 交互登录尚未接入，401/403 明确提示需要认证。

旧检查点继续使用原10项工具契约，新轮次使用14项；恢复不会自动添加新工具或重新运行未知结果。每会话最多索引100项任务，达到上限先拒绝新提交，请新建对话；普通进度查询不创建业务任务。部署与验收证据以 Goal 进度 为准。
