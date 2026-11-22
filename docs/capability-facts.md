# 能力的费用与验收依据

能力索引仍由原模型目录、仓库登记、本机能力、MCP和Skill派生，没有第二份密钥或执行配置。发现和切换详情不自动同步价格、探测账号、安装项目或执行能力。

## 渠道参考价格

`ModelCapability.pricing`及统一索引`entries[].pricing`是可选字段，绑定完整`route_id`；不同渠道不能借用同名模型的价格。旧目录或旧pins没有此字段时费用未知，不迁移或改写旧文件。只有显式同步OpenRouter目录时采集，`observed_at`保持采集时间，普通读取和分页不会刷新它。

`reference_only=true`始终成立。`rates[].amount`是精确非负十进制字符串；输入/输出的`unit=usd_per_token`，`per_million_tokens`按十进制精确乘一百万，单位USD/百万token；请求`unit=usd_per_request`，amount单位USD/次，百万token字段为空。界面不把金额转换为浮点数或四舍五入成零。

`source_url`保留实际采集目录端点，仅接受固定官方公开目录及账号筛选目录；不反射任意URL或认证查询。现有同步使用`/models/user?output_modalities=all`，其结果受账号偏好、隐私和guardrails筛选，不能标为公开目录，也仍不是一次推理成功或余额证明。界面明确区分公开/账号筛选目录，不在打开详情时重新请求。

`state=reported`仅表示三个基础维度都能解析；`partial`说明存在缺失、无效或未覆盖维度；`invalid`表示没有有效基础报价。`missing_dimensions`不按零补全，`invalid_dimensions`不参与比较，`unmodeled_dimensions`包含尚未支持的额外计费字段。`conditional=true`表示存在折扣或条件价，当前仅显示基础报价，没有套用条件。不能据此估算完整总价。

零基础报价不保证账号权限、额度、限流或最终免费。索引原`cost=unknown`、助理免费策略、凭据优先级和模型调用授权保持原规则。其他厂商尚无适配报价时明确未知；这不是Token用量或实际账单面板。

## 仓库验收登记与检查范围

`repo_capabilities_list`返回`provenance.state=registry_recorded`：来源commit、发布版本、登记时间、清单及验收记录SHA-256来自原登记；列表不逐个扫描安装代码或读取证明。

`repo_capability_describe`读取原清单与验收记录，核对双方摘要、版本、仓库/commit/发布版本/许可证绑定，以及原隔离验收范围和通过的用例。只有通过后返回同一次读取的验收记录和`state=records_checked`。摘要、归属或结构冲突返回`CAPABILITY_CHANGED`；未知不是通过。

两个状态都保持`code_integrity_checked=false`；原`checked_at`是历史验收时间，不是此次查询时间。新任务执行前仍核对安装代码文件摘要，再走原隔离执行器。登记记录不是安全审计、当前账号权限或本轮任务成功证明。

## 原任务结果

同编号同输入仍先查询原回执，不因当前安装记录改变而重新运行。结果核对原`run_id`和回执的能力/请求归属，冲突返回`RUN_IDENTITY_INVALID`。附件必须属于原run目录的result.txt，大小不超过既有1MiB边界，且为无链接普通文件；有界读取并核对实际大小及SHA-256。变化返回`RESULT_CHANGED`，暂时不可读返回`RESULT_UNAVAILABLE`，均不重新执行。

预览从已校验附件生成，避免UTF-16边界截断emoji，也不会改写旧记录。`succeeded`及完整产物才是业务完成证据，控制job受理编号不能代替它。HTML输出仍作为文本处理。

助理模型工具的describe仅缩短局部Schema定义名并同步引用，字段、约束、使用说明和9000字节预算保持；CLI完整契约继续使用原定义名。普通AI通过接口即可查询这些信息，无需读取实现源码。

## 依据

已读取[OpenRouter模型API](https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties)及[官方SDK PublicPricing源码](https://github.com/OpenRouterTeam/typescript-sdk/blob/801c18e6d733af3d1972135284aaa935dfdccfbd/src/models/publicpricing.ts)，同commit的model.ts、pricing.ts和Apache-2.0许可证；参考其字符串单位及条件覆盖设计，没有复制代码或安装SDK。遵循[MCP Tools规范](https://modelcontextprotocol.io/specification/2025-06-18/server/tools)对注解的信任边界，不由工具名称推断费用、授权或验收。
