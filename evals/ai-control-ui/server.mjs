import { build, preview } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
const config = {
  configFile: false,
  // Browser fixtures must not load checkout .env files.
  envDir: false,
  root: fileURLToPath(new URL("../../", import.meta.url)),
  plugins: [react()],
  resolve: {
    alias: { "@": fileURLToPath(new URL("../../src", import.meta.url)) },
  },
  build: {
    outDir: process.env.HOMEBASE_UI_BUILD_DIR || ".tmp/ai-control-ui-build",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        entry: fileURLToPath(new URL("index.html", import.meta.url)),
        preflight: fileURLToPath(new URL("preflight.html", import.meta.url)),
        gpu: fileURLToPath(new URL("gpu.html", import.meta.url)),
        performance: fileURLToPath(new URL("performance.html", import.meta.url)),
      },
    },
  },
  preview: { host: "127.0.0.1", port: Number(process.env.HOMEBASE_UI_PORT || 1429), strictPort: true },
};
await build(config);
await preview(config);
