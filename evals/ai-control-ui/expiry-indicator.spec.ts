import { test, expect, type Page } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "密钥提醒：暂无待处理", exact: true }),
  ).toBeVisible();
});
async function expired(page: Page) {
  await page.evaluate(async () => {
    const f = (window as any).__performance;
    f.values.secrets_lifecycle_status = [
      {
        short_name: "synthetic",
        display_name: "Synthetic service",
        expires_at: "2020-01-01T00:00:00Z",
        pending_revocation: false,
        expiry_policy: "warn",
      },
    ];
    await f.client.invalidateQueries({ queryKey: ["secrets", "lifecycle"] });
  });
}
test("expired keys use a quiet dot; details appear only on demand without extra queries", async ({
  page,
}) => {
  await expired(page);
  const trigger = page.getByRole("button", {
    name: "密钥提醒：1 项待处理",
    exact: true,
  });
  await expect(page.getByTestId("secret-reminder-dot")).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByText(/1 把密钥已到期/)).toHaveCount(0);
  const before = await page.evaluate(
    () =>
      (window as any).__performance.calls.filter(
        (name: string) => name === "secrets_lifecycle_status",
      ).length,
  );
  await trigger.click();
  const panel = page.getByRole("dialog", { name: "密钥提醒", exact: true });
  await expect(panel).toBeVisible();
  await expect(panel).toHaveAttribute("aria-modal", "false");
  await expect(panel).toContainText("Synthetic service：已到期");
  await expect(
    page.getByRole("button", { name: "关闭密钥提醒", exact: true }),
  ).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(panel).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(page.getByTestId("secret-reminder-dot")).toBeVisible();
  await trigger.click();
  await page.getByRole("heading", { name: "HomeBase", exact: true }).click();
  await expect(panel).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.calls.filter(
          (name: string) => name === "secrets_lifecycle_status",
        ).length,
    ),
  ).toBe(before);
  await page.screenshot({ path: ".tmp/expiry-indicator-wide.png" });
});
test("failed checks remain unknown and recover through the popover", async ({
  page,
}) => {
  await page.evaluate(async () => {
    const f = (window as any).__performance;
    f.values.secrets_lifecycle_status = () => {
      throw Error("Synthetic unavailable");
    };
    await f.client.invalidateQueries({ queryKey: ["secrets", "lifecycle"] });
  });
  await page
    .getByRole("button", { name: "密钥提醒：状态未知", exact: true })
    .click();
  await expect(
    page.getByRole("dialog", { name: "密钥提醒", exact: true }),
  ).toContainText("暂时无法确认是否有到期告警");
  await page.evaluate(() => {
    (window as any).__performance.values.secrets_lifecycle_status = [];
  });
  await page.getByRole("button", { name: "重试到期检查", exact: true }).click();
  await expect(
    page.getByText("暂无到期或轮换待办。", { exact: true }),
  ).toBeVisible();
  await expect(page.getByTestId("secret-reminder-dot")).toHaveCount(0);
});
test("narrow sidebar, dark theme and long reminder lists stay within the viewport", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 560 });
  await page.evaluate(async () => {
    document.documentElement.classList.add("dark");
    const f = (window as any).__performance;
    f.values.secrets_lifecycle_status = Array.from({ length: 20 }, (_, i) => ({
      short_name: `synthetic-${i}`,
      display_name: `Synthetic ${i}`,
      expires_at: "2020-01-01T00:00:00Z",
      pending_revocation: false,
    }));
    await f.client.invalidateQueries({ queryKey: ["secrets", "lifecycle"] });
  });
  await page
    .getByRole("button", { name: "密钥提醒：20 项待处理", exact: true })
    .click();
  const rect = await page
    .getByRole("dialog", { name: "密钥提醒", exact: true })
    .boundingBox();
  expect(rect!.x).toBeGreaterThanOrEqual(0);
  expect(rect!.x + rect!.width).toBeLessThanOrEqual(375);
  expect(rect!.y + rect!.height).toBeLessThanOrEqual(560);
  await page.screenshot({ path: ".tmp/expiry-indicator-narrow.png" });
});
test("rotation reminders keep the dot until resolved and navigation closes the panel", async ({
  page,
}) => {
  await page.evaluate(async () => {
    const f = (window as any).__performance;
    f.values.secrets_lifecycle_status = [
      {
        short_name: "synthetic",
        display_name: "Synthetic service",
        expires_at: null,
        pending_revocation: true,
      },
    ];
    await f.client.invalidateQueries({ queryKey: ["secrets", "lifecycle"] });
  });
  await page
    .getByRole("button", { name: "密钥提醒：1 项待处理", exact: true })
    .click();
  await expect(
    page.getByRole("dialog", { name: "密钥提醒", exact: true }),
  ).toContainText("1 次轮换待确认旧密钥停用");
  await page.getByRole("link", { name: "前往密钥管理", exact: true }).click();
  await expect(
    page.getByRole("dialog", { name: "密钥提醒", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByTestId("secret-reminder-dot")).toBeVisible();
  await page.evaluate(async () => {
    const f = (window as any).__performance;
    f.values.secrets_lifecycle_status = [];
    await f.client.invalidateQueries({ queryKey: ["secrets", "lifecycle"] });
  });
  await expect(page.getByTestId("secret-reminder-dot")).toHaveCount(0);
});
