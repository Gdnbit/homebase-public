# 按功能组织的模型与服务能力

能力库包含「模型与服务」和原有「开源项目」。前者读取已有 Key 的配置状态、模型目录和固定适配器；后者保留 HyperFrames 及已有 GitHub 封装草稿。此功能不迁移或复制任何真实 Key，不改现有配置和轮换策略。

## 选择顺序与数据含义

功能分组 → 模型 → 调用渠道 → 自动匹配凭据引用。模型发布商不等于调用渠道：OpenRouter 上的 OpenAI 模型用 OpenRouter Key；OpenAI 直连使用 OpenAI Key。只有发布商和模型 ID 明确一致时合并渠道，不做模糊名称合并。

请求可显式选择 `credential_source=default/sops/api_key_manager`，分别为原优先级、只用 SOPS、只用独立凭据；失败不回退或覆盖其他来源。同步和异步查询的顶层参数为 `credentialSource`，异步查询必须沿用生成时的来源。内置助理单独保存其选择，不改变其他消费者默认优先级。

按输出模态区分图片/视频生成；输入视频、输出文字归为视频理解，不能据此认定支持视频生成。没有足够模态信息的模型放入其他模型；未实现的协议明确不可调用，不猜测厂商地址、地域或账号权限。

目录有两个来源：官方参考模型与显式同步的厂商目录。官方参考不是完整账号权限清单；目录接口可读也不证明生成权限、余额、模型仍可调用或所有地域均支持。未确认来源保留说明。

OpenRouter显式同步结果可附带按`route_id`绑定的`pricing`，详情随调用渠道切换，采集时间在读取时保持不变。输入/输出精确换算为USD/百万token，请求为USD/次；缺失、无效、条件和额外维度明确提示。只作参考，不保证免费、账号权限或最终账单；旧pins无此字段仍可读，不自动重写。详细规则见[能力费用与验收依据](capability-facts.md)。

当前支持同步 OpenRouter 用户目录、OpenAI、fal.ai、ElevenLabs 和百炼兼容目录。fal 按游标读取，超过保护上限或分页格式异常时保留旧目录并报错，不把部分页说成完整目录。部分服务不提供已接入的完整自动枚举接口，保持官方参考。TryAllAI 缺少已确认 Base URL，豆包语音缺少独立凭据/资源配置，均不借用其他 Key。

## 凭据绑定

SOPS 中已有对应条目时优先，并保留空值、类型、撤销、事务及到期策略校验；任何不可用状态都不回退 DPAPI 或其他渠道。只有对应 SOPS 条目确实不存在时，受支持的服务才使用 API-Key-Manager 的既有 DPAPI 引用。每次真正使用重新解析凭据，不缓存、传给前端或输出正文。

ElevenLabs 可能没有独立 DPAPI 文件但已有 SOPS Key，本页按有效优先来源展示，不能仅根据独立文件状态判为未配置。功能页只返回 credential_ref/source/state，不返回密钥掩码或正文。

GitHub、Figma、Tavily、Firecrawl、Hugging Face、蚁小二和内部数据库/签名 Key 单独列为服务与凭据，不混进生成模型列表。配置状态、到期提醒和远端有效性不同；未知服务保持待确认。

服务配置state按原value_state区分：empty/missing为not_configured；invalid值或已填条目的无效日期为invalid，未知/矛盾元数据为unknown。只有已填且日期有效的条目才区分configured/expired，恰好到期也算expired。invalid/unknown都不能当成未填写、已验证或自动改用其他凭据；到期标记本身不修改warn/block策略。统一能力页、模型与服务子页和交接资料保持相同事实。

## 调用接口

1. `model_capabilities_catalog(offset=0,limit=200)`：读取本机凭据状态、服务、分组和第一页模型。根据 next_offset 继续，直到 null；total_models 是总数。后续页不重复读取凭据。
2. `model_capabilities_refresh(providerIds)`：显式只读目录同步，可能读取对应后端凭据并联网，不生成。失败保留上次成功模型且 sources 标记 error。缓存为本机进程内存，重启后需重新同步。
3. `model_capability_resolve(request)`：从 route_id 解析 endpoint、实际请求与 credential_ref，不发送生成请求。input 按该模型 input_schema / input_example 和 source_url 官方文档填写；不允许客户端传入密钥、认证头、任意端点或不匹配的 model。
4. `model_capability_call(request)`：根据已有授权执行，可能产生服务商费用。CLI 需 write/private/external，使用固定 request-id；没有自动重试或跨渠道切换。当前为非流式调用，不自动执行模型返回的工具调用。
5. `model_capability_task(routeId,taskId,outputRoot)`：查询相同渠道的异步任务；按固定协议构造地址，不接受服务商返回的任意查询 URL。不会取消、恢复或重提旧任务。

request 包含 route_id、input、可选 output_root。二进制音频（当前 MP3）或较大 JSON 结果需要专用绝对输出目录，只创建 UUID 子目录，不覆盖现有文件。返回 artifact 时包含路径、字节数、SHA-256 和 MIME。返回远端 URL 只代表有结果位置，下载/预览仍按用户范围执行。

queued/running 必须继续查询；failed/unknown 不代表成功。HTTP 成功但缺少有效业务字段、空列表或空音频不会被一概当作生成成功。先用外层 control jobs 获取业务返回，再查询业务 task ID；网络中断时可能已被服务商接受，应保留原编号核对，不更换 Key 重跑。

### OpenAI 直连单张 PNG 产物

已有目录中的 `gpt-image-*` 直连路由可以将单张非流式 PNG 登记为任务产物。调用沿用原渠道与权限。内置助理会指定会话专用输出目录，显式调用也可传 `output_root`。输入可用 `{"prompt":"蓝色封面","n":1,"output_format":"png"}`，省略张数和输出格式时使用服务商的单张 PNG 默认。服务商的 `data[0].b64_json` 必须可完整解码为静态 PNG，输入及保存后均至多 8 MiB，每边不超过 4096 像素，总像素不超过 8294400。只有像素校验通过后才保存文件和 SHA-256，响应中保留格式与已知用量。异构、空或损坏图片不给完成回执，保留原任务供核对。未经这条产物适配的其他格式或多张输出仍沿用它们原有的接口返回。 [OpenAI 生图返回契约](https://developers.openai.com/api/reference/resources/images/methods/generate)

### 本机图片理解

OpenRouter 的具体模型只有在当前目录声明支持图片输入和文字输出，且已接通 Chat Completions 协议时，能力库的调用弹窗才显示「使用本机 PNG」。选择图片和检查调用方式都在本机完成，点击「调用模型」后才发送给当前渠道。检查结果省略问题和图片字节。切换渠道或关闭弹窗清除所选文件；图片模式下的调用参数不复制到剪贴板。

CLI 和 API 使用 `request.input={"vision":{"prompt":"图片问题","png_base64":"<静态 PNG 的标准 Base64>"}}`。单张源文件及发送前重新编码的 PNG 均限 384 KiB，每边不超过 2048 像素且总像素不超过 2097152，问题最多 4000 UTF-8 字节。后端完整解码，发送重新生成的像素图片。输入不接受 URL、额外消息、工具参数或模型覆盖。配额或账号权限仍以实际调用为准。请求绑定模型，关闭回退和自动重试，仅接受完整文字结果。此入口目前位于能力库，助理聊天草稿仍沿用原有附件范围。 [OpenRouter 图片输入协议](https://openrouter.ai/blog/tutorials/send-image-to-llm/)

签名 CLI 使用 `VISION_UNSUPPORTED`、`VISION_INVALID_PNG`、`VISION_IMAGE_LIMIT` 和 `VISION_PROMPT_LIMIT` 区分渠道、格式、容量和问题长度。错误只带具体可公开原因。

## 界面与缓存

前端复用 React Query，配置与模型目录五分钟内保持缓存；分组/搜索/渠道筛选只在内存执行，卡片逐批展示，不把整个模型目录一次渲染进 DOM。同步和刷新必须显式触发；失败保留当前目录。保存或轮换密钥后使目录的凭据状态缓存失效。

弹窗选择渠道后清除旧计划，更新 Key 引用与参数模板；未配置、协议未知时禁用生成。同一路由最近结果保留在内存，重开弹窗可查询原任务；queued/running 不允许重复提交。该缓存不替代持久化的通用任务中心，CLI 调用仍应保留控制 job 与业务 task ID。

## 验证边界

生成验证仅使用临时目录、合成凭据与 loopback HTTP 夹具：校验渠道/模型/认证头绑定、密钥来源优先级、空结果、失败、异步状态、输出文件与哈希。不会用真实 Key 做收费生成、上传私人素材或发布测试。实机只读模型目录与 UI 核对另记在带日期验收报告。

设计基准位于 `outputs/page-design/model-capabilities/2026-11-10/main-and-states.png`；图片中的模型名称是视觉示意，实际页面由后端数据驱动，原有品牌与导航保持不变。

实机目录、隔离测试、原生界面与未验证项见 2026-11-10 验收记录。微信后续接入见 官方 iLink 协议调研。
