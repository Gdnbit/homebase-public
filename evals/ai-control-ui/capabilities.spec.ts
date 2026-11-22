import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";
const key = "homebase.capabilities.drafts.v1";
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "能力库", exact: true }),
  ).toBeVisible();
  await page.getByRole("tab", { name: "开源项目", exact: true }).click();
});
async function add(page: Page) {
  await page.getByRole("button", { name: "添加项目", exact: true }).click();
  await page
    .getByLabel("GitHub 仓库地址", { exact: true })
    .fill("https://github.com/example/synthetic-tool.git");
  await page.getByLabel("能力名称", { exact: true }).fill("隔离文本工具");
  await page
    .getByLabel("用途", { exact: true })
    .fill("仅用于隔离验收，不下载仓库");
}
test("empty library saves draft, copies exact task and persists across navigation without execution", async ({
  page,
}) => {
  await expect(page.getByText("尚未添加项目", { exact: true })).toBeVisible();
  await page.screenshot({
    path: ".tmp/capabilities-empty.png",
    animations: "disabled",
  });
  await page.evaluate(() => {
    (window as any).__performance.values[
      "plugin:clipboard-manager|write_text"
    ] = (args: any) => {
      (window as any).__copied = args.text;
      return null;
    };
  });
  await add(page);
  await page.getByLabel("固定版本或提交").fill("synthetic-commit");
  await page.getByLabel("输入约定").fill("text: string");
  await page.getByLabel("输出约定").fill("result: string");
  await page.getByRole("button", { name: "保存草稿", exact: true }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "草稿已保存" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "复制 AI 封装任务", exact: true })
    .click();
  const value = await page
    .getByLabel("封装任务说明", { exact: true })
    .inputValue();
  expect(await page.evaluate(() => (window as any).__copied)).toBe(value);
  expect(value).toContain('"status": "draft"');
  expect(value).toContain("未经验证不得标为可调用");
  await page.getByRole("button", { name: "关闭封装草稿", exact: true }).click();
  await navigate(page, "概览");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await expect(page.getByRole("list", { name: "能力项目" })).toContainText(
    "待封装与验证",
  );
  const stored = await page.evaluate(
    (k) => JSON.parse(localStorage.getItem(k)!),
    key,
  );
  expect(stored.entries[0].repository).toBe(
    "https://github.com/example/synthetic-tool",
  );
  const calls = await page.evaluate(
    () => (window as any).__performance.calls as string[],
  );
  expect(
    calls.filter((name) =>
      /install|clone|execute|secrets_save|secrets_reveal/.test(name),
    ),
  ).toEqual([]);
  await page.screenshot({
    path: ".tmp/capabilities-draft.png",
    animations: "disabled",
  });
});
test("invalid repository is rejected without persistence and clipboard failure has manual fallback", async ({
  page,
}) => {
  await add(page);
  await page
    .getByLabel("GitHub 仓库地址", { exact: true })
    .fill("https://github.com/example/tool?token=synthetic");
  await page.getByRole("button", { name: "保存草稿", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("查询参数");
  expect(await page.evaluate((k) => localStorage.getItem(k), key)).toBeNull();
  await page
    .getByLabel("GitHub 仓库地址", { exact: true })
    .fill("https://github.com/example/tool");
  await page.getByRole("button", { name: "保存草稿", exact: true }).click();
  await page
    .getByRole("button", { name: "复制 AI 封装任务", exact: true })
    .click();
  await expect(
    page.getByText("复制失败，可选中下方全文手动复制。", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "选中全文", exact: true }).click();
  await expect(page.getByLabel("封装任务说明", { exact: true })).toBeFocused();
});
test("corrupt library remains visible as error and never overwritten", async ({
  page,
}) => {
  await page.evaluate((k) => localStorage.setItem(k, "synthetic-corrupt"), key);
  await navigate(page, "概览");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("原数据已保留");
  await expect(
    page.getByRole("button", { name: "添加项目", exact: true }),
  ).toBeDisabled();
  expect(await page.evaluate((k) => localStorage.getItem(k), key)).toBe(
    "synthetic-corrupt",
  );
  await page.evaluate((k) => localStorage.removeItem(k), key);
  await page.getByRole("button", { name: "重新读取草稿", exact: true }).click();
  await expect(page.getByText("尚未添加项目", { exact: true })).toBeVisible();
});
test("write failure and concurrent change keep the form and require explicit refresh", async ({
  page,
}) => {
  await add(page);
  await page.evaluate((k) => {
    const original = Storage.prototype.setItem;
    (window as any).__restoreStorage = () => {
      Storage.prototype.setItem = original;
    };
    Storage.prototype.setItem = function (name, value) {
      if (name === k) throw Error("Synthetic quota failure");
      original.call(this, name, value);
    };
  }, key);
  await page.getByRole("button", { name: "保存草稿", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Synthetic quota failure",
  );
  await expect(page.getByLabel("能力名称", { exact: true })).toHaveValue(
    "隔离文本工具",
  );
  await page.evaluate((k) => {
    (window as any).__restoreStorage();
    localStorage.setItem(k, JSON.stringify({ version: 1, entries: [] }));
  }, key);
  await page.getByRole("button", { name: "保存草稿", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("已在其他窗口改变");
  await page
    .getByRole("button", { name: "重新读取并保留表单", exact: true })
    .click();
  await page.getByRole("button", { name: "保存草稿", exact: true }).click();
  await expect(
    page.getByText("草稿已保存，尚未封装与验证。", { exact: true }),
  ).toBeVisible();
});
test("narrow dark modal traps focus and Escape preserves unsaved draft unless confirmed", async ({
  page,
}) => {
  await page.setViewportSize({ width: 375, height: 700 });
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await add(page);
  const editor = page.getByRole("dialog", { name: "封装草稿", exact: true });
  const rect = await editor.boundingBox();
  expect(rect!.x).toBeGreaterThanOrEqual(0);
  expect(rect!.x + rect!.width).toBeLessThanOrEqual(375);
  expect(rect!.height).toBeLessThanOrEqual(700);
  page.once("dialog", (d) => d.dismiss());
  await page.keyboard.press("Escape");
  await expect(editor).toBeVisible();
  await expect(page.getByLabel("能力名称", { exact: true })).toHaveValue(
    "隔离文本工具",
  );
  await page.screenshot({
    path: ".tmp/capabilities-editor-narrow.png",
    animations: "disabled",
  });
  page.once("dialog", (d) => d.accept());
  await page.keyboard.press("Escape");
  await expect(editor).toHaveCount(0);
});
