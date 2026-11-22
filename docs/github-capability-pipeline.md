# GitHub 仓库能力接入

在能力库 → 开源项目中填公开 GitHub 仓库主页。分析只读公开元数据；“安装、封装并验收”固定 npm 发布版本、gitHead 与 sha512 integrity，禁用包安装脚本，用隔离 CLI 运行帮助、版本与行为样例，成功后注册。桌面与微信使用同一个注册表，旧草稿、HyperFrames、模型目录与密钥不迁移。

当前驱动 `npm-stdin-cli-v1` 支持根 package.json 对应官方 npm 发布包、有唯一 CLI 入口、能从标准输入读文本并从标准输出返回结果的项目。Markdown 转 HTML 自动提供两个行为用例，其他文本 CLI 需要用户提供明确样例。包声明仓库必须匹配，精确发布提交必须能在该仓库查询到。npm 的 gitHead 是发布方声明，并非可复现构建或软件安全审计证明。没有发布包、Python/容器/GUI/复杂服务、多个 CLI 入口会明确失败或 needs_adapter；不会自动运行 README 命令。

运行要求：Windows、Node ≥22.19 且安装 npm CLI，WSL Ubuntu 的 Node ≥20/Python3，以及随运行包提供并核对摘要的 bubblewrap。缺少隔离依赖时停止，不降级到宿主执行。包代码不接触 Windows 个人目录、凭据或用户配置，Linux 命名空间无网络，包只读，仅本次工作目录可写，20秒/1MiB输出上限。安装使用单独 HOME、npm配置、缓存与临时目录，不运行生命周期脚本。

下载前，宿主用固定的发布摘要核对随包 bubblewrap。WSL 监督进程再核对一次摘要，使用与仓库执行相同的基础隔离参数运行 `node --version`。隔离命名空间成功启动且其中的 Node.js 达到 20 后，才允许安装。预检不挂载仓库资料，也不运行仓库代码。摘要不符返回 `SANDBOX_CHANGED`，Node 不可用或版本过低返回 `SANDBOX_RUNTIME_UNAVAILABLE`，隔离启动失败返回 `SANDBOX_ISOLATION_UNAVAILABLE`。 [Bubblewrap 隔离参数](https://github.com/containers/bubblewrap#sandboxing)

WSL 监督进程启动超时返回 `SANDBOX_NOT_READY`。提前退出、失配的就绪信息和已知 Windows 启动错误各有明确诊断。仅报告已识别的原因，原始进程输出留在本机进程内。 [WSL 命令说明](https://learn.microsoft.com/en-us/windows/wsl/basic-commands) · [Microsoft WSL 错误代码实现](https://github.com/microsoft/WSL/blob/master/src/windows/common/wslutil.cpp) · [WSL 故障排查](https://learn.microsoft.com/en-us/windows/wsl/troubleshooting-guide)

预检失败时保留原分析版本，未创建安装尝试、业务安装回执或能力登记。下载后的验收阶段若启动下一轮隔离进程失败，显示原项目验收未完成。若隔离请求已经发出但没有可信终态，则保留 `EXECUTION_UNKNOWN`，按原任务核对。桌面和微信显示对应原因。

等待 WSL 时，项目详情和列表显示 `preflighting`，原分析版本保持。桌面可点「取消此操作」，CLI 使用同一 `project_id` 和本次 `operation.request_id`。`cancel_requested` 只表示已收到取消请求。若取消在下载获准前生效，原操作返回 `canceled`，沿用同一请求编号会读回取消结果。并行安装由原项目锁阻止。若进程在预检中断，先核对原操作，`preflight_interrupted` 表示预检已中断、原分析已保留。核对过程不启动下载。 [长任务进度及并发约定](https://google.aip.dev/151)

## CLI 完整流程

在项目根目录使用 `node scripts/homebase.mjs`（或 hb.cmd）。每个 request.json 是普通 UTF-8 JSON 文件；先 describe/validate，再 call。以下编号和示例须替换为实际响应，不把模板原样执行。

只知道项目名时，先使用 `hb describe repo_search_public`，输入 `{"query":"marked"}`，授权 `--allow-private --allow-external`。这只查询公开 GitHub 候选。已知归属可给 `markedjs/marked`。候选最多五个，`partial=true` 表示结果可能不全。先确认公开仓库地址，再进入项目分析。 [GitHub 搜索接口](https://docs.github.com/en/rest/search#search-repositories)

1. `hb describe repo_project_analyze`。输入 `{"request":{"repository_url":"https://github.com/markedjs/marked","request_id":"import-marked-once"}}`。调用时声明 --allow-private --allow-write --allow-external，另给稳定外层 --request-id。
2. 外层 job 未结束时 `hb jobs get JOB_ID --allow-private --allow-write --allow-external`；保存 project.id/revision。`repo_project_get` 读当前进度，查询不修改状态。
3. `repo_project_install` 输入 `{"request":{"project_id":"实际id","revision":实际版本号,"request_id":"install-marked-once"}}`。其他通用 CLI 可加 examples：`[{"input":{"stdin":"明确输入"},"contains":["预期输出片段"]}]`。只有 project.phase=registered 且 verification.passed=true 才通过。
4. `repo_capabilities_list` 分页发现；`repo_capability_describe` 输入 `{"capabilityId":"实际id"}`，读取 input_schema、examples、limits、notice。
5. `repo_capability_run` 输入 `{"request":{"capability_id":"实际id","input":{"stdin":"# Hello\n\n**HomeBase**"},"request_id":"convert-once"}}`。成功返回业务 run_id、stdout、artifact（路径/大小/SHA256/MIME）。正文超过32768字符时 stdout_truncated=true，stdout仅预览，完整内容保存在已校验附件，避免大正文阻断控制协议。
6. `repo_capability_result` 输入 `{"runId":"实际run_id"}` 查询原状态和结果并核验文件摘要。不需要读源码，不再生成。

权限、完整顶层参数及返回字段用 `hb describe COMMAND` 查询。schema内嵌字段保持 snake_case，顶层参数使用 camelCase。外层 CLI request-id 与内部 request.request_id 是两层去重身份，都保持不变。相同身份不同输入返回冲突，同一任务不重复执行；不清理旧去重记录来“腾位置”。

## 助理与微信

自然语言请求由已有共享 Deep Agents 助理处理。通过 homebase_discover/homebase_call 发现 repo_capabilities_list → describe → run；宿主把 request_id 绑定原轮次、命令及有效参数，同一轮重复工具调用复用原结果。homebase_job 查询同一控制 job，homebase_deliver 返回已登记附件。HTML只作为文本返回、使用 result.txt，不嵌入 DOM。助理不能猜测其他会话 run_id；本机用户 CLI 可以按已有授权核对任务。

安装新仓库属于引入可执行软件，助理会准备具体待确认操作，由本人在桌面或绑定微信确认；不能由模型自批。已注册能力的普通调用不反复申请安装授权。

微信可直接说「分析仓库 https://github.com/markedjs/marked」或「把 https://github.com/markedjs/marked 加入能力库」。指定的公开主页会进入与桌面能力库相同的分析器。支持的仓库视图地址解析成 GitHub 主页，分析通过后沿原结果准备安装确认。 [微信直接分析仓库](wechat-integration.md#直接分析仓库候选)

微信视频结果如含编号的仓库主页，引用原视频回复「分析第 1 个项目」即可进入同一个分析器，也可用 `/分析项目 原收件编号 1`。同一条微信请求绑定原地址及分析任务，微信结果给出项目编号与当前驱动状态。 [微信用法与验收](wechat-integration.md#从文字稿识别仓库链接候选)

只有本机转写提出的项目名时，引用视频说「找第 1 个项目」可以使用原结果的名字查询公开候选；查到相同名称的多个仓库时，先确认归属和编号再分析。 [视频项目名识别](wechat-integration.md#视频里只有项目名时查找候选候选)

微信候选版可直接引用分析回复说「安装这个项目」，或发送 `/安装项目 原分析任务编号`。通用文本 CLI 可在同一命令后附上 `examples` 数组内容。确认绑定原项目编号、分析版本和验收示例，复用同一个 `repo_project_install` 和能力注册表。安装器在执行前重新检查版本；等待确认、失败或结果未知时可用原助理任务查询。 [微信安装确认与示例](wechat-integration.md#引用仓库分析后确认安装候选)

## 恢复与边界

2026-11-20 验收。独立 Windows 能力库真实点击 Marked 安装，约 16 秒显示 WSL 未就绪，七次签名 CLI 与两次正常退出通过。重开时原分析版本为 2，项目摘要一致，零安装尝试及能力登记。真实本机 Node 探针约 13 秒返回同一错误。合成协议覆盖未就绪时不向子进程写入项目请求、取消、收到就绪后执行，以及已经发送请求但未取得终态。45 项仓库与接口、3 项 Rust 和 4 项浏览器用例通过。此前已下载软件的原安装记录仍为结果未知。 运行证据 · 接口回归 · 浏览器回归

- 下载只对无副作用的公开获取做有限重试。网络、限流、403、坏JSON、安装失败、验收失败均明确显示，不伪装为空目录或成功。
- 原进程退出时观察为 interrupted_unknown；读列表或切页面不会恢复、重装、改写任务。repo_operation_reconcile 仅对唯一匹配身份且原工作/子进程均已退出的操作核对并释放锁，不执行项目。原结果未知时不自动重跑。
- repo_operation_cancel 指定一个 project_id 或 run_id，加匹配 operation_id；cancel_requested仅表示请求写入，继续查询直到终态。取消不删除依赖、旧产物或其他任务。
- 每项目操作锁和每请求锁隔离并发；SQLite WAL保存新注册表，损坏报错、不重建空库。源码、清单、验收凭据和产物摘要变化时停止，不以旧验收放行。
- 自动安装上限128MiB/4096文件，最多512个项目和512个能力。超限明确失败，不自动驱逐旧数据。当前没有自动升级/自动卸载，以保留原调用和证据。

## 仓库视图参考

参考官方开源项目 [Gitingest](https://github.com/coderamp-labs/gitingest) 的仓库文本摘要、[GitMCP](https://github.com/idosal/git-mcp) 的文档检索入口，以及 [DeepWiki](https://docs.devin.ai/work-with-devin/deepwiki) 的文档问答视图。支持把这些已知域名的 owner/repo 地址归一为 GitHub 地址，并提供对应链接。它们帮助阅读资料，不能代替固定提交、安装和实际验收；默认分支视图可能与发布提交不同。不上传个人文件或私有仓库，也不改已有 MCP 配置。

首例项目为 [Marked](https://github.com/markedjs/marked)，MIT，Markdown→HTML，不需要模型或服务商 Key。具体版本以分析响应为准，设计图片中的示意版本不作为运行事实。
