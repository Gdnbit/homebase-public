import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("heading", { name: "AI 助理", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    const state = (window as any).__performance;
    const pricing = {
      route_id: "openrouter::openai/fixture",
      state: "partial",
      source_url: "https://openrouter.ai/api/v1/models",
      observed_at: "2026-11-16T03:00:00+08:00",
      reference_only: true,
      rates: [
        {
          kind: "prompt",
          unit: "usd_per_token",
          amount: "0.000000000000000000000001",
          per_million_tokens: "0.000000000000000001",
        },
        {
          kind: "completion",
          unit: "usd_per_token",
          amount: "0",
          per_million_tokens: "0",
        },
        {
          kind: "request",
          unit: "usd_per_request",
          amount: "0.01",
          per_million_tokens: null,
        },
      ],
      unmodeled_dimensions: 2,
      missing_dimensions: 0,
      invalid_dimensions: 0,
      conditional: true,
    };
    state.values.model_capabilities_catalog.models.find(
      (m: any) => m.provider_id === "openrouter",
    ).pricing = pricing;
    state.values.capability_index_get = {
      version: "homebase-capability-index-v1",
      issues: [],
      routing: "合成来源",
      entries: [
        {
          id: pricing.route_id,
          target: pricing.route_id,
          function: "text",
          kind: "model",
          name: "参考模型",
          provider: "openrouter",
          state: "public_catalog_not_entitlement",
          cost: "unknown",
          discover: "model_capability_resolve",
          pricing,
        },
        {
          id: "synthetic-repo",
          target: "synthetic-repo",
          function: "text",
          kind: "repository",
          name: "登记项目",
          provider: "https://github.com/example/fixture",
          state: "verified",
          cost: "local_compute",
          discover: "repo_capability_describe",
          provenance: {
            state: "registry_recorded",
            source_commit: "a".repeat(40),
            package_version: "1.0.0",
            manifest_sha256: "b".repeat(64),
            verification_sha256: "c".repeat(64),
            registered_at: "2026-11-16T03:00:00+08:00",
            checked_at: null,
            code_integrity_checked: false,
          },
        },
      ],
    };
  });
  await page.getByRole("link", { name: "能力库", exact: true }).click();
});

test("functional details expose exact reference prices and the limited scope of registry evidence without executing", async ({
  page,
}) => {
  await page.getByRole("button", { name: /文字与推理.*2 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByLabel("参考价格")).toContainText(
    "0.000000000000000001 USD / 百万 token",
  );
  await expect(dialog).toContainText("请求：0.01 USD / 次");
  await expect(dialog).toContainText("未套用条件");
  await expect(dialog).toContainText("2 个未覆盖计费维度");
  await expect(dialog).toContainText("零报价也不保证可免费调用");
  await expect(dialog).toContainText("有验收登记");
  await expect(dialog).toContainText("公共目录 · 账号权限待验证");
  await expect(dialog).toContainText("公开模型目录");
  await expect(dialog).not.toContainText("public_catalog_not_entitlement");
  await dialog.getByText("验收登记 · 运行前核对", { exact: true }).click();
  await expect(dialog).toContainText("a".repeat(40));
  await expect(dialog).toContainText("此处未检查当前安装代码完整性");
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("repo_capability_describe");
  expect(calls).not.toContain("repo_capability_run");
  expect(calls).not.toContain("model_capabilities_refresh");
  expect(calls).not.toContain("model_capability_call");
});

test("channel selection changes the reference facts and keeps unknown providers unknown", async ({
  page,
}) => {
  await page.getByRole("tab", { name: "模型与服务", exact: true }).click();
  await page.getByRole("button", { name: "查看调用方式" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("参考价格未知");
  await dialog
    .getByLabel("调用渠道", { exact: true })
    .selectOption("openrouter::openai/fixture");
  await expect(dialog.getByLabel("参考价格")).toContainText(
    "0.000000000000000001",
  );
  await dialog
    .getByLabel("调用渠道", { exact: true })
    .selectOption("openai::fixture");
  await expect(dialog.getByLabel("参考价格")).toHaveCount(0);
  await expect(dialog).toContainText("参考价格未知");
  expect(
    (await page.evaluate(() => (window as any).__performance.calls)).filter(
      (c: string) => c === "model_capability_call",
    ),
  ).toHaveLength(0);
});

test("invalid pricing and a missing base dimension are explicit rather than zero", async ({
  page,
}) => {
  await page.evaluate(() => {
    const state = (window as any).__performance;
    const p = state.values.model_capabilities_catalog.models.find(
      (m: any) => m.provider_id === "openrouter",
    ).pricing;
    p.state = "invalid";
    p.source_url = "https://openrouter.ai/api/v1/models/user?output_modalities=all";
    p.rates = [];
    p.missing_dimensions = 1;
    p.invalid_dimensions = 2;
  });
  await page.getByRole("tab", { name: "模型与服务", exact: true }).click();
  await page.getByRole("button", { name: "查看调用方式" }).click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel("调用渠道", { exact: true })
    .selectOption("openrouter::openai/fixture");
  await expect(dialog.getByLabel("参考价格")).toContainText("无有效报价");
  await expect(dialog.getByLabel("参考价格")).toContainText("账号筛选目录");
  await expect(dialog).toContainText("1 个基础维度未提供");
  await expect(dialog).toContainText("2 个报价无效");
  await expect(dialog.getByLabel("参考价格")).not.toContainText(
    "USD / 百万 token",
  );
});
