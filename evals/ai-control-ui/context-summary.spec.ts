import { test, expect } from "@playwright/test";

test("context shows bounded counts, follows its own task and never executes", async ({
  page,
}) => {
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
    const p = (window as any).__performance;
    p.values.assistant_sessions_list = [
      { id: "A", title: "合成上下文", source: "desktop", message_count: 0 },
    ];
    p.values.assistant_session_get = {
      id: "A",
      source: "desktop",
      messages: [],
      turns: [],
    };
    p.values.assistant_context_get = {
      version: "homebase-context-v1",
      projection_schema_revision: 2,
      summary_only: true,
      fingerprint: "synthetic",
      state_revision: 1,
      policy_revision: 1,
      tasks: [{ id: "primary", state: "awaiting_confirmation" }],
      tasks_total: 9,
      tasks_has_more: true,
      queued_tasks: 8,
      memories: [],
      context_has_more: true,
      authorized_workspaces: [{ id: "one", access: "read" }],
      authorized_workspaces_total: 14,
      authorized_workspaces_has_more: true,
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByLabel("草稿保存状态")).toContainText("输入就绪");
  await page.getByRole("button", { name: "上下文自动组织" }).click();
  const panel = page.getByRole("region", { name: "本次上下文" });
  await expect(panel).toContainText("9 条 · 摘要展示 1 条");
  await expect(panel).toContainText("14 个 · 摘要展示 1 个");
  await expect(panel).toContainText("部分内容已省略");
  const reads = () =>
    page.evaluate(
      () =>
        (window as any).__performance.calls.filter(
          (c: string) => c === "assistant_context_get",
        ).length,
    );
  expect(await reads()).toBe(1);
  await page.evaluate(() =>
    (window as any).__performance.emit("assistant:changed", {
      session_id: "unrelated",
      turn_id: null,
      config: false,
    }),
  );
  // Let the event coalescing interval pass: unrelated work must not reread.
  await page.waitForTimeout(650);
  expect(await reads()).toBe(1);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_context_get.tasks_total = 10;
    for (let i = 0; i < 5; i++)
      p.emit("assistant:changed", {
        session_id: "A",
        turn_id: null,
        config: false,
      });
  });
  await expect(panel).toContainText("10 条 · 摘要展示 1 条");
  expect(await reads()).toBe(2);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_context_get.tasks_total = 11;
    for (let i = 0; i < 5; i++)
      p.emit("assistant:changed", {
        session_id: null,
        turn_id: null,
        config: true,
      });
  });
  await expect(panel).toContainText("11 条 · 摘要展示 1 条");
  expect(await reads()).toBe(3);
  await page.getByRole("button", { name: "关闭上下文" }).click();
  await page.evaluate(() =>
    (window as any).__performance.emit("assistant:changed", {
      session_id: null,
      turn_id: null,
      config: true,
    }),
  );
  await page.waitForTimeout(650);
  expect(await reads()).toBe(3);
  await page.getByRole("button", { name: "上下文自动组织" }).click();
  await expect.poll(reads).toBe(4);
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_send");
  expect(calls).not.toContain("assistant_resume");
});
