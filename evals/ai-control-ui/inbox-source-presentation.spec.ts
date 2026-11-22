import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";

const body = (page: Page) => page.locator("textarea").first();
async function start(page: Page) {
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
  await navigate(page, "投料口");
  await expect(body(page)).toBeEnabled();
}
test.beforeEach(async ({ page }) => {
  await page.route("**/*", r => new URL(r.request().url()).hostname === "127.0.0.1" ? r.continue() : r.abort());
});

test("a restored custom source remains visible and unchanged through editing and AI handoff", async ({ page }) => {
  const hint = "synthetic-custom-source-中文🙂";
  await page.addInitScript(hint => { (window as any).__inboxSeed = { entry: { schema_version: 1, revision: 1, draft: { text: "合成资料A", title: "", tags_input: "", source_hint: hint, attachments: [], attachment_unconfirmed: false }, pending: null, last_submission_id: null } }; }, hint);
  await start(page);
  const select = page.locator("select").first();
  await expect(select).toHaveValue(hint);
  await expect(select.locator("option:checked")).toContainText(hint);
  await body(page).fill("后来修改的合成资料B🙂");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("后来修改的合成资料B🙂");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.source_hint)).toBe(hint);
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByText("工作流资料已保留", { exact: true })).toBeVisible();
  const saved = await page.evaluate(() => { const p = (window as any).__performance; return p.desktopStore.get(p.createRequests.at(-1).requestId).body.text; });
  expect(saved).toContain(hint); expect(saved).toContain("后来修改的合成资料B🙂");
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => /inbox_workspace_(prepare|submit|cancel|resolve)/.test(c)))).toEqual([]);
});

test("a pristine empty source promises autosave after editing without claiming an existing file or writing", async ({ page }) => {
  await start(page);
  await expect(page.getByLabel("资料草稿状态")).toContainText("编辑后会自动在本机保存");
  await expect(page.getByLabel("资料草稿状态")).not.toContainText("已在本机受保护保存");
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => /inbox_workspace_(save|prepare|submit|cancel|resolve)/.test(c)))).toEqual([]);
  await body(page).fill("开始编辑🙂");
  await expect(page.getByLabel("资料草稿状态")).toContainText("已在本机受保护保存");
});

test("a long literal custom source fits a narrow page and the destination is relative to the configured vault", async ({ page }) => {
  const hint = "<img src=x onerror=alert(1)>" + "合成来源🙂".repeat(70);
  await page.addInitScript(hint => { (window as any).__inboxSeed = { entry: { schema_version: 1, revision: 1, draft: { text: "窄屏资料", title: "", tags_input: "", source_hint: hint, attachments: [], attachment_unconfirmed: false }, pending: null, last_submission_id: null } }; }, hint);
  await page.setViewportSize({ width: 390, height: 844 });
  await start(page);
  await expect(page.locator("select").first()).toHaveValue(hint);
  await expect(page.getByText("raw/inbox/YYYY-MM-DD/", { exact: true })).toBeVisible();
  await expect(page.getByText("C:\\HomeBase\\Wiki\\raw\\inbox\\YYYY-MM-DD\\", { exact: true })).toHaveCount(0);
  expect(await page.locator('[aria-label="资料编辑工作区"]').evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  expect(await page.locator('img[src="x"]').count()).toBe(0);
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "inbox_workspace_save"))).toEqual([]);
});

test("an external saved draft can be manually refreshed without publishing or overwriting a dirty local edit", async ({ page }) => {
  await start(page);
  const refresh = page.getByRole("button", { name: "刷新资料草稿", exact: true });
  await expect(refresh).toBeVisible();
  await page.evaluate(() => { const p = (window as any).__performance; p.inboxFixture.entry.draft.text = "外部接口保存C🙂"; p.inboxFixture.entry.draft.source_hint = "external-source"; p.inboxFixture.entry.revision++; });
  await refresh.click();
  await expect(body(page)).toHaveValue("外部接口保存C🙂");
  await expect(page.locator("select").first()).toHaveValue("external-source");
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "inbox_workspace_get").length)).toBe(2);
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => /inbox_workspace_(save|prepare|submit|cancel|resolve)/.test(c)))).toEqual([]);
  await body(page).fill("本地未确认的新输入B");
  await page.evaluate(() => { const p = (window as any).__performance; p.inboxFixture.entry.draft.text = "外部更新D"; p.inboxFixture.entry.revision++; });
  await page.getByRole("button", { name: /^(刷新资料草稿|核对原收录与草稿)$/ }).click();
  await expect(body(page)).toHaveValue("本地未确认的新输入B");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("外部更新D");
  await expect(page.getByRole("button", { name: "以当前输入保存", exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes)).toEqual([]);
});
