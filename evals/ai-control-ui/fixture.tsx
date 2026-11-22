import React from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { Sidebar } from "../../src/components/layout/Sidebar";
import { AiControlLauncher } from "../../src/components/ai-control-launcher";
import template from "../../control/bootstrap.template.txt?raw";
import "../../src/index.css";

const root = "E:\\隔离项目\\Home Base";
const prompt = template
  .replaceAll("{{PROJECT_ROOT}}", root)
  .replaceAll("{{START_FILE}}", `${root}\\control\\START.md`)
  .replaceAll("{{CLI_FILE}}", `${root}\\scripts\\homebase.mjs`);
const state = {
  calls: [] as string[],
  copied: [] as string[],
  failRead: false,
  failCopy: false,
  data: {
    files: {
      ready: true,
      project_root: root,
      startup_file: `${root}\\control\\START.md`,
      cli_file: `${root}\\scripts\\homebase.mjs`,
      prompt,
      issue: null as string | null,
    },
    service: { state: "ready", message: "本机控制服务已就绪" },
    total: 113,
    available: 103,
    blocked: 6,
    unavailable: 4,
  },
};
(window as any).__fixture = state;
(window as any).__TAURI_INTERNALS__ = {
  invoke: async (command: string, args: any = {}) => {
    state.calls.push(command);
    if (command === "secrets_lifecycle_status") return [];
    if (command === "assistant_sessions_list") return [];
    if (command === "ai_control_bootstrap") {
      if (state.failRead) throw new Error("synthetic read failure");
      return structuredClone(state.data);
    }
    if (command === "plugin:clipboard-manager|write_text") {
      if (state.failCopy) throw new Error("synthetic clipboard failure");
      state.copied.push(args.text);
      return;
    }
    throw new Error(`Non-fixture command rejected: ${command}`);
  },
};
const client = new QueryClient({
  defaultOptions: { queries: { retry: false } },
});
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <BrowserRouter>
      <div className="flex h-screen w-screen bg-background">
        <Sidebar />
        <main className="min-w-0 flex-1 p-6">
          <p>隔离界面 · 未连接真实 HomeBase</p>
          <AiControlLauncher />
        </main>
      </div>
    </BrowserRouter>
  </QueryClientProvider>,
);
