import React from "react";
import { createRoot } from "react-dom/client";
import { Dashboard, GpuResourceCard, SystemInfoTable } from "../../src/routes/Dashboard";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { ImageGenProgressBar } from "../../src/routes/Studio";
import type { GpuMetrics, SystemInfo } from "../../src/lib/tauri-api";
import "../../src/index.css";

// 不挂载 Dashboard，不连接 Tauri、控制服务或密钥数据。
(window as any).__TAURI_INTERNALS__ = { invoke: () => { throw Error("Real invocation forbidden"); } };
const mode = new URLSearchParams(location.search).get("mode");
if (mode === "query-errors") {
  (window as any).__TAURI_INTERNALS__.invoke = async (command: string) => {
    if (command === "agents_list") return [{ id: "synthetic" }];
    if (command === "system_info") return { os: "windows", os_version: "fixture OS", arch: "x86_64", hostname: "fixture", username: "fixture", gpu_name: null, gpu_status: "unknown", gpu_message: "GPU 信息未知：合成查询失败", issues: [{ field: "computer", status: "unknown", message: "整机信息未知：合成查询失败" }] };
    throw Error("synthetic query failure: no real data access");
  };
}
const status = mode === "detected" || mode === "counter" ? "detected" : mode === "absent" ? "not_detected" : "unknown";
const name = status === "detected" ? "Synthetic NVIDIA GPU" : null;
const message = status === "detected" ? "已识别物理 GPU" : status === "not_detected" ? "未检测到物理 GPU" : "GPU 信息未知：合成查询失败";
const info: SystemInfo = { os: "windows", os_version: "test", arch: "x86_64", hostname: "fixture", username: "fixture", gpu_name: name, gpu_status: status, gpu_message: message };
const metrics: GpuMetrics = { available: !!name, gpu_name: name, gpu_names: name ? [name] : [], gpu_status: status, gpu_message: message, gpu_percent: mode === "detected" ? 0 : null, dedicated_memory_gb: null, gpu_warning: false, error: mode === "counter" ? "已识别物理 GPU，但实时利用率计数器不可用" : name ? null : message };
const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
createRoot(document.getElementById("root")!).render(mode === "progress-unknown" ? <ImageGenProgressBar progress={null} /> : mode === "query-errors" ? <MemoryRouter><QueryClientProvider client={client}><Dashboard /></QueryClientProvider></MemoryRouter> : <main className="p-6"><section aria-label="系统信息"><SystemInfoTable data={info} /></section><section aria-label="GPU 资源"><GpuResourceCard icon={null} data={metrics} isLoading={false} /></section></main>);
