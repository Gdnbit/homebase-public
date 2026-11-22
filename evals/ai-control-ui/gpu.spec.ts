import { test, expect } from "@playwright/test";

test("未取得生图进度时明确显示未知，不显示零进度", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/gpu.html?mode=progress-unknown");
  await expect(page.getByText("进度未知：暂未检测到进度记录")).toBeVisible();
  await expect(page.getByRole("progressbar")).toHaveCount(0);
});

test("已识别 GPU 展示名称，真实零利用率显示 0.0%", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/gpu.html?mode=detected");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("Synthetic NVIDIA GPU");
  await expect(page.getByRole("region", { name: "GPU 资源" })).toContainText("0.0%");
});
test("未检测到 GPU 时仍显示显卡行和明确提示", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/gpu.html?mode=absent");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("显卡未检测到物理 GPU");
  await expect(page.getByRole("region", { name: "GPU 资源" })).toContainText("未检测到");
});
test("查询失败显示未知与原因，不伪造零利用率", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/gpu.html?mode=unknown");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("GPU 信息未知：合成查询失败");
  await expect(page.getByRole("region", { name: "GPU 资源" })).toContainText("未知");
  await expect(page.getByRole("progressbar")).toHaveCount(0);
});
test("名称已识别但计数器不可用时保留名称并提示利用率未知", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/gpu.html?mode=counter");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("Synthetic NVIDIA GPU");
  await expect(page.getByRole("region", { name: "GPU 资源" })).toContainText("实时利用率计数器不可用");
  await expect(page.getByRole("progressbar")).toHaveCount(0);
});
test("系统查询失败不会显示零告警、无代理运行或隐藏未知硬件", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/gpu.html?mode=query-errors");
  const stats = page.getByRole("region", { name: "关键指标" });
  await expect(stats).toContainText("部分查询失败，无法确认告警总数");
  await expect(stats).not.toContainText("无未处理告警");
  await expect(stats).not.toContainText("无代理运行");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("未知：系统未提供处理器信息");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("未知：系统未提供有效内存总量");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("整机信息未知：合成查询失败");
  await expect(page.getByRole("region", { name: "系统信息" })).toContainText("fixture OS");
  await expect(page.getByRole("region", { name: "系统资源" })).toContainText("读取失败");
  await expect(page.getByRole("progressbar")).toHaveCount(0);
});
