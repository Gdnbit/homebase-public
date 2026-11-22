import { build, preview } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../../", import.meta.url));
const config = {
  configFile: false,
  envDir: false,
  root,
  plugins: [react()],
  resolve: { alias: { "@": fileURLToPath(new URL("../../src", import.meta.url)) } },
  build: { outDir: process.env.HOMEBASE_SECRET_UI_BUILD_DIR || ".tmp/secret-lifecycle-ui-build", emptyOutDir: true, rollupOptions: { input: fileURLToPath(new URL("index.html", import.meta.url)) } },
  preview: { host: "127.0.0.1", port: 1428, strictPort: true },
};
await build(config);
await preview(config);
console.log("Isolated UI fixture: http://127.0.0.1:1428/evals/secret-lifecycle-ui/index.html");
