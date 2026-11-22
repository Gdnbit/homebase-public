import React from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { PreflightBanner } from "../../src/components/preflight-banner";
import "../../src/index.css";
const state = {
  calls: 0,
  failIPC: false,
  delay: 0,
  checks: [
    {
      id: "sops_exe",
      label: "SOPS 可执行文件",
      ok: false,
      severity: "critical",
      retryable: true,
      detail: null,
      hint: "合成启动超时",
    },
  ],
};
(window as any).__fixture = state;
(window as any).__TAURI_INTERNALS__ = {
  invoke: async (command: string) => {
    if (command !== "system_preflight")
      throw Error("Non-fixture command rejected");
    state.calls++;
    if (state.delay)
      await new Promise((resolve) => setTimeout(resolve, state.delay));
    if (state.failIPC) throw Error("synthetic IPC failure");
    return structuredClone(state.checks);
  },
};
const client = new QueryClient({
  defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } },
});
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={client}>
    <main className="p-6">
      <p>隔离自检 · 无真实密钥或工具调用</p>
      <PreflightBanner />
    </main>
  </QueryClientProvider>,
);
