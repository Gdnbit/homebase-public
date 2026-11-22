import { defineConfig, type UserConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

const host = process.env.TAURI_DEV_HOST;

// https://vitejs.dev/config/
export default defineConfig(async (): Promise<UserConfig> => ({
  plugins: [react()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  // Tauri 期望固定端口；详见 https://tauri.app/start/frontend/vite/
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // src-tauri 改动由 cargo watch 处理，不要 vite 重建
      ignored: ["**/src-tauri/**"],
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: "esnext",
    minify: "esbuild",
    sourcemap: false,
  },
  // 提前声明需要 pre-bundle 的依赖
  // - 17 个 lobehub Mono 子模块：避免 dev 启动时 lazy 触发 esbuild 多次 cold-start 而崩溃
  //   （白屏修复：esbuild "service was stopped"）
  // - @xyflow/react：React Flow 体积大，提前 pre-bundle 提升首次渲染速度
  optimizeDeps: {
    include: [
      "@xyflow/react",
      "@lobehub/icons/es/Anthropic/components/Mono",
      "@lobehub/icons/es/Aws/components/Mono",
      "@lobehub/icons/es/Azure/components/Mono",
      "@lobehub/icons/es/Bing/components/Mono",
      "@lobehub/icons/es/Claude/components/Mono",
      "@lobehub/icons/es/Cohere/components/Mono",
      "@lobehub/icons/es/DeepSeek/components/Mono",
      "@lobehub/icons/es/Gemini/components/Mono",
      "@lobehub/icons/es/Grok/components/Mono",
      "@lobehub/icons/es/Groq/components/Mono",
      "@lobehub/icons/es/HuggingFace/components/Mono",
      "@lobehub/icons/es/Mistral/components/Mono",
      "@lobehub/icons/es/OpenAI/components/Mono",
      "@lobehub/icons/es/OpenRouter/components/Mono",
      "@lobehub/icons/es/Perplexity/components/Mono",
      "@lobehub/icons/es/Together/components/Mono",
      "@lobehub/icons/es/XAI/components/Mono",
    ],
  },
}));
