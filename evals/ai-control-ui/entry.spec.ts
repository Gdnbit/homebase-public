import { test, expect } from "@playwright/test";
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/index.html");
});
test("explicit integration launcher does not automatically read bootstrap or copy text", async ({
  page,
}) => {
  expect(
    await page.evaluate(() =>
      (window as any).__fixture.calls.filter(
        (command: string) => command === "ai_control_bootstrap",
      ),
    ),
  ).toEqual([]);
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page.getByText("103 项可调用接口 · 6 项人工或禁用入口 · 4 项尚未实现"),
  ).toBeVisible();
  expect(await page.evaluate(() => (window as any).__fixture.copied)).toEqual(
    [],
  );
  await expect(
    page.getByRole("textbox", { name: "启动提示词预览" }),
  ).toContainText("START.md");
});
test("copy writes the exact shared template prompt with spaces and Unicode", async ({
  page,
}) => {
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await page
    .getByRole("button", { name: "复制启动提示词", exact: true })
    .click();
  await expect(
    page.getByText("启动提示词已复制，可直接粘贴给 AI。"),
  ).toBeVisible();
  const value = await page
    .getByRole("textbox", { name: "启动提示词预览" })
    .inputValue();
  const { copied, data: { files: { prompt } } } = await page.evaluate(() => (window as any).__fixture);
  expect(copied).toEqual([prompt]);
  // The browser normalizes textarea line endings on Windows.
  expect(value).toBe(prompt.replace(/\r\n/g, "\n"));
  expect(value).toContain("E:\\隔离项目\\Home Base");
  expect(value).not.toContain("{{");
  expect(value).not.toContain("endpoint.json");
});
test("clipboard failure is visible and full-text selection works", async ({
  page,
}) => {
  await page.evaluate(() => ((window as any).__fixture.failCopy = true));
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await page
    .getByRole("button", { name: "复制启动提示词", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText("复制失败");
  await page.getByRole("button", { name: "选中全文", exact: true }).click();
  expect(
    await page
      .getByRole("textbox")
      .evaluate(
        (element: HTMLTextAreaElement) =>
          element.selectionEnd - element.selectionStart ===
          element.value.length,
      ),
  ).toBe(true);
});
test("missing project resources never produce a fake copyable prompt", async ({
  page,
}) => {
  await page.evaluate(() => {
    const state = (window as any).__fixture;
    state.data.files.ready = false;
    state.data.files.prompt = null;
    state.data.files.issue = "隔离操作文件缺失";
  });
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("隔离操作文件缺失");
  await expect(
    page.getByRole("button", { name: "复制启动提示词", exact: true }),
  ).toHaveCount(0);
});
test("failed reads can retry and refreshed failures hide stale ready information", async ({
  page,
}) => {
  await page.evaluate(() => ((window as any).__fixture.failRead = true));
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("无法读取");
  await page.evaluate(() => ((window as any).__fixture.failRead = false));
  await page.getByRole("button", { name: "重新检查", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "复制启动提示词", exact: true }),
  ).toBeEnabled();
  await page.evaluate(() => ((window as any).__fixture.failRead = true));
  await page.getByRole("button", { name: "重新检查", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("无法读取");
  await expect(
    page.getByRole("button", { name: "复制启动提示词", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByText("本机控制服务已就绪", { exact: true }),
  ).toHaveCount(0);
});
test("disabled service is disclosed while valid instructions can still be copied", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as any).__fixture.data.service = {
      state: "disabled",
      message: "控制服务已关闭",
    };
  });
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await expect(page.getByText("控制服务已关闭", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "复制启动提示词", exact: true }),
  ).toBeEnabled();
  await expect(page.getByText(/实际任务仍需核对依赖/)).toBeVisible();
});
test("Escape closes the dialog and returns focus to the launcher", async ({
  page,
}) => {
  const launcher = page.getByRole("button", { name: "AI 接管", exact: true });
  await launcher.click();
  await expect(
    page.getByRole("button", { name: "复制启动提示词", exact: true }),
  ).toBeEnabled();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await expect(launcher).toBeFocused();
});
test("narrow sidebar and dialog remain accessible without horizontal overflow", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "复制启动提示词", exact: true }),
  ).toBeEnabled();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({ path: ".tmp/ai-control-mobile.png" });
});
test("desktop dark appearance uses the same global entry", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await page.getByRole("button", { name: "AI 接管", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "复制启动提示词", exact: true }),
  ).toBeEnabled();
  await page.screenshot({ path: ".tmp/ai-control-desktop.png" });
  const calls = await page.evaluate(() => (window as any).__fixture.calls);
  expect(
    calls.every((name: string) =>
      ["ai_control_bootstrap", "secrets_lifecycle_status"].includes(name),
    ),
  ).toBe(true);
});
