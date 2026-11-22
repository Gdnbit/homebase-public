/**
 * lobehub 单色图标 barrel — 集中所有 Mono deep import
 *
 * 为什么需要这层 barrel：
 * - `@lobehub/icons` 顶层导出会触发 `Avatar.js` → `@lobehub/ui` + `antd-style`，
 *   而我们项目用 Tailwind 没有 antd ConfigProvider → runtime crash → logo fallback 失败
 * - Mono.js 子模块是纯 React + SVG（仅 import react / jsx-runtime / style.js），无 antd 依赖
 * - 把 17 个 deep import 收口到这里，让 Vite 只看到一个稳定入口；
 *   `vite.config.ts` 的 `optimizeDeps.include` 也只需声明这 17 条
 *
 * 命名约定：与 lobehub 顶层导出一致（Anthropic / OpenAI / ...），保持 service-logo.tsx 调用零变化
 */

export { default as Anthropic } from "@lobehub/icons/es/Anthropic/components/Mono";
export { default as Aws } from "@lobehub/icons/es/Aws/components/Mono";
export { default as Azure } from "@lobehub/icons/es/Azure/components/Mono";
export { default as Bing } from "@lobehub/icons/es/Bing/components/Mono";
export { default as Claude } from "@lobehub/icons/es/Claude/components/Mono";
export { default as Cohere } from "@lobehub/icons/es/Cohere/components/Mono";
export { default as DeepSeek } from "@lobehub/icons/es/DeepSeek/components/Mono";
export { default as Gemini } from "@lobehub/icons/es/Gemini/components/Mono";
export { default as Grok } from "@lobehub/icons/es/Grok/components/Mono";
export { default as Groq } from "@lobehub/icons/es/Groq/components/Mono";
export { default as HuggingFace } from "@lobehub/icons/es/HuggingFace/components/Mono";
export { default as Mistral } from "@lobehub/icons/es/Mistral/components/Mono";
export { default as OpenAI } from "@lobehub/icons/es/OpenAI/components/Mono";
export { default as OpenRouter } from "@lobehub/icons/es/OpenRouter/components/Mono";
export { default as Perplexity } from "@lobehub/icons/es/Perplexity/components/Mono";
export { default as Together } from "@lobehub/icons/es/Together/components/Mono";
export { default as XAI } from "@lobehub/icons/es/XAI/components/Mono";
