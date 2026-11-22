import { test, expect } from "@playwright/test";
test("a late session list never redirects an already focused new input to an old conversation", async ({
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
    p.values.assistant_config_get.settings.enabled = true;
    p.values.assistant_config_get.settings.route_id =
      "openrouter::fixture:free";
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const input = page.getByLabel("发送消息", { exact: true });
  await expect(input).toBeEnabled();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list = () =>
      new Promise((resolve) => {
        p.releaseSessions = resolve;
      });
    p.values.assistant_session_get = ({ sessionId }: any) => ({
      id: sessionId,
      source: "desktop",
      messages: [],
      turns: [],
    });
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.releaseSessions),
    )
    .toBe("function");
  await input.focus();
  await page.evaluate(() =>
    (window as any).__performance.releaseSessions([
      { id: "A", title: "旧会话", source: "desktop", message_count: 0 },
    ]),
  );
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__performance.client.getQueryData([
            "assistant",
            "sessions",
          ])?.items?.length,
      ),
    )
    .toBe(1);
  await page.evaluate(
    () =>
      new Promise((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(resolve)),
      ),
  );
  await input.fill("这是我在新入口准备的草稿🙂");
  await expect(page.getByLabel("草稿保存状态")).toContainText(
    "草稿已受保护保存",
  );
  const state = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      fresh: p.desktopStore.get("new"),
      old: p.desktopStore.get("A"),
      calls: p.calls,
    };
  });
  expect(state.fresh?.body.text).toBe("这是我在新入口准备的草稿🙂");
  expect(state.old?.body.text || "").toBe("");
  expect(state.calls).not.toContain("assistant_send");
});
