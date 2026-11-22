import { test, expect } from "@playwright/test";
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.clock.install({ time: new Date("2026-11-08T12:00:00Z") });
  await page.goto("/evals/ai-control-ui/preflight.html");
  await page.clock.runFor(500);
  await expect(page.getByText("合成启动超时", { exact: true })).toBeVisible();
});
test("transient startup failures automatically recover and clear the warning", async ({
  page,
}) => {
  await page.evaluate(() => ((window as any).__fixture.checks[0].ok = true));
  await page.clock.runFor(31000);
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__fixture.calls)).toBe(2);
  await page.clock.runFor(120000);
  expect(await page.evaluate(() => (window as any).__fixture.calls)).toBe(2);
});
test("persistent transient failures stop after two automatic rechecks", async ({
  page,
}) => {
  await page.clock.runFor(31000);
  await expect
    .poll(() => page.evaluate(() => (window as any).__fixture.calls))
    .toBe(2);
  await page.clock.runFor(31000);
  await expect
    .poll(() => page.evaluate(() => (window as any).__fixture.calls))
    .toBe(3);
  await expect(page.getByText(/自动复检次数已用完/)).toBeVisible();
  await page.clock.runFor(180000);
  expect(await page.evaluate(() => (window as any).__fixture.calls)).toBe(3);
  await expect(page.getByRole("alert")).toBeVisible();
});
test("missing or malformed tools are not repeatedly retried", async ({
  page,
}) => {
  await page.evaluate(() => {
    const check = (window as any).__fixture.checks[0];
    check.retryable = false;
    check.hint = "合成文件缺失";
  });
  await page.getByRole("button", { name: "重新检查", exact: true }).click();
  await expect(page.getByText("合成文件缺失", { exact: true })).toBeVisible();
  await page.clock.runFor(180000);
  expect(await page.evaluate(() => (window as any).__fixture.calls)).toBe(2);
});
test("manual retry resets the budget and correctly clears recovered errors", async ({
  page,
}) => {
  await page.clock.runFor(31000);
  await page.clock.runFor(31000);
  await expect(page.getByText(/自动复检次数已用完/)).toBeVisible();
  await page.evaluate(() => ((window as any).__fixture.checks[0].ok = true));
  await page.getByRole("button", { name: "重新检查", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__fixture.calls)).toBe(4);
});
test("IPC failures also recover without showing cached success", async ({
  page,
}) => {
  await page.evaluate(() => ((window as any).__fixture.failIPC = true));
  await page.getByRole("button", { name: "重新检查", exact: true }).click();
  await expect(page.getByText(/启动自检未完成/)).toBeVisible();
  await page.evaluate(() => {
    (window as any).__fixture.failIPC = false;
    (window as any).__fixture.checks[0].ok = true;
  });
  await page.clock.runFor(31000);
  await expect(page.getByRole("alert")).toHaveCount(0);
});
test("in-flight retry disables repeated clicks and exposes the checking state", async ({
  page,
}) => {
  await page.evaluate(() => ((window as any).__fixture.delay = 2000));
  await page.getByRole("button", { name: "重新检查", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "正在检查…", exact: true }),
  ).toBeDisabled();
  await page.clock.runFor(2500);
  await expect(
    page.getByRole("button", { name: "重新检查", exact: true }),
  ).toBeEnabled();
});
