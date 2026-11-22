---
doc_id: homebase-openrouter-model-catalog
title: HomeBase OpenRouter Model Catalog
role: HomeBase 快捷面板与 Agent API 模型目录说明
canonical_path: C:\HomeBase\source\docs\openrouter-model-catalog.md
updated: 2026-06-07
related:
  - doc_id: homebase-quick-panel
    path: C:\HomeBase\source\docs\quick-panel.md
    rel: parent-ui
tags: [homebase, openrouter, model-catalog, quick-panel, api]
---

# OpenRouter Model Catalog

HomeBase 的快捷面板当前先接入 OpenRouter API 方案，用它作为统一的云端模型入口。

## Human UI

位置：

```text
C:\HomeBase\source\src\routes\QuickPanel.tsx
```

快捷面板会展示一个“API 模型”折叠区：

- 模型来源：OpenRouter `/api/v1/models?output_modalities=all`
- 展示方式：按 provider/品牌分组，例如 OpenAI、Anthropic、Google、xAI、Qwen、DeepSeek
- 组内排序：使用能力评分启发式排序，高能力模型在前
- 价格标注：在下拉选项中显示 OpenRouter 返回的 prompt/completion/image/request 价格
- 手动刷新：点击“刷新列表”重新拉取 OpenRouter 当前模型目录
- 前端缓存：React Query `staleTime` 为 1 小时，避免每次切换页面都重复请求

## AI/API Entry

Tauri command:

```text
openrouter_models_catalog
```

Frontend wrapper:

```text
openrouterApi.modelsCatalog()
```

返回结构：

```ts
interface OpenRouterModelCatalog {
  fetched_at: string;
  source_url: string;
  groups: OpenRouterModelGroup[];
}
```

AI Agent 可以调用这个命令读取最新模型目录，再根据任务类型选择模型。

## Auto Update Rule

模型列表不是写死在 HomeBase 里的。

每次刷新都会直接请求 OpenRouter 当前模型目录，所以 OpenRouter 上线、下架、改价后，HomeBase 会在下一次刷新时看到变化。当前没有做本地长期缓存，也没有后台定时同步。

建议后续补两个能力：

- 后台每日同步：写入 `~/.homebase/cache/openrouter-models.json`，离线时使用上次成功结果
- 任务型推荐：把模型按文本、视觉、音频、图像生成、视频、低价、长上下文等能力生成二级索引，供 HomeBase Agent 自动路由

## Sorting Notes

OpenRouter 不直接提供“综合能力排名”。HomeBase 当前用启发式能力评分：

- 强模型关键词加权，例如 `opus`、`gpt-5`、`gpt-4.5`、`pro`、`sonnet`、`grok`
- 多模态输入输出加权，例如 image/audio/video
- tool calling、structured output、reasoning 参数加权
- 长上下文加权
- free 模型轻微降权，避免免费变体压过主力模型

这只是 UI 排序，不等同于正式评测结果。后续可以接入一份人工维护的模型偏好表覆盖默认排序。
