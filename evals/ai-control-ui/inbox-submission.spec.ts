import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";

const body = (page: Page) => page.locator("textarea").first();
const submit = (page: Page) => page.getByRole("button", { name: /^投递/ });
const inspect = (page: Page) => page.getByRole("button", { name: "核对原收录与草稿", exact: true });
const id = "87562a38-8fb4-4276-bdda-bd0d85bef544";
const draft = (text: string) => ({ text, title: "", tags_input: "", source_hint: "", attachments: [], attachment_unconfirmed: false });
async function start(page: Page) {
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
  await navigate(page, "投料口");
  await expect(body(page)).toBeEnabled();
}
test.beforeEach(async ({ page }) => {
  await page.route("**/*", r => new URL(r.request().url()).hostname === "127.0.0.1" ? r.continue() : r.abort());
});

test("window close does not wait for the first read of saved inbox input", async ({ page }) => {
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.registrations.includes("tauri://close-requested"))).toBe(true);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const get = p.values.inbox_workspace_get;
    p.values.inbox_workspace_get = async () => {
      await new Promise<void>((resolve) => { p.releaseInboxRead = resolve; });
      return get();
    };
    p.values["plugin:window|destroy"] = () => { p.closedDuringInboxRead = true; };
    p.values["plugin:dialog|message"] = () => "返回核对";
  });
  await navigate(page, "投料口");
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseInboxRead)).toBe("function");
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  try {
    await expect.poll(() => page.evaluate(() => (window as any).__performance.closedDuringInboxRead), { timeout: 2500 }).toBe(true);
    expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => /^inbox_workspace_(save|prepare|submit)/.test(c)))).toEqual([]);
  } finally {
    await page.evaluate(() => (window as any).__performance.releaseInboxRead());
  }
});

test("a lost acknowledgement never turns a retry of one intent into a second note", async ({ page }) => {
  await start(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const original = p.values.inbox_workspace_submit;
    p.values.inbox_workspace_submit = async (args: any) => {
      await original(args);
      throw Error("SYNTHETIC_ACK_LOST_AFTER_COMMIT");
    };
  });
  await body(page).fill("一次合成收录意图🙂");
  await submit(page).click();
  await expect(page.getByText(/投递失败：.*SYNTHETIC_ACK_LOST_AFTER_COMMIT/)).toBeVisible();
  await expect(submit(page)).toBeDisabled();
  await body(page).fill("后来输入 B，不能当作已收录");
  await body(page).press("Control+Enter");
  await inspect(page).click();
  await expect(body(page)).toHaveValue("后来输入 B，不能当作已收录");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.inboxFixture.notes.length)).toBe(1);
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "inbox_workspace_submit").length)).toBe(1);
});

test("an unconfirmed draft save waits for explicit reconciliation and keeps later input", async ({ page }) => {
  await page.clock.install();
  await start(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const save = p.values.inbox_workspace_save;
    p.values.inbox_workspace_save = (args: any) => {
      const result = save(args);
      if (args.request.draft.text === "FIRST INPUT") throw Error("ISOLATED_DRAFT_ACK_LOST");
      return result;
    };
  });
  await body(page).fill("FIRST INPUT");
  await page.clock.runFor(650);
  await expect(page.getByLabel("资料草稿状态")).toContainText("ISOLATED_DRAFT_ACK_LOST");
  await body(page).fill("LATER INPUT");
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.runFor(650);
  const writes = () => page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "inbox_workspace_save").length);
  expect(await writes()).toBe(1);
  await expect(submit(page)).toBeDisabled();
  await inspect(page).click();
  await expect(body(page)).toHaveValue("LATER INPUT");
  await expect(page.getByRole("button", { name: "以当前输入保存", exact: true })).toHaveCount(0);
  expect(await writes()).toBe(1);
  await page.getByRole("button", { name: "重试保存资料草稿", exact: true }).click();
  await expect(page.getByLabel("资料草稿状态")).toContainText("资料草稿已在本机受保护保存");
  expect(await writes()).toBe(2);
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("LATER INPUT");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes)).toEqual([]);
});

test("retrying an accepted exit does not restart the discarded draft save", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await start(page);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.registrations.includes("tauri://close-requested"))).toBe(true);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.inbox_workspace_save = () => { throw Error("ISOLATED_SAVE_REJECTED"); };
    p.values["plugin:dialog|message"] = () => "仍然关闭";
    p.values["plugin:window|destroy"] = () => null;
  });
  await body(page).fill("DISCARD ONLY ON EXIT");
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  await expect(page.getByRole("dialog", { name: "正在退出 HomeBase", exact: true })).toBeVisible();
  await page.evaluate(() => {
    window.dispatchEvent(new Event("blur"));
    Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await page.clock.runFor(10_100);
  await page.getByRole("button", { name: "重试退出", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "plugin:window|destroy").length)).toBe(2);
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => ["inbox_workspace_save", "plugin:dialog|message", "plugin:window|destroy"].includes(c)))).toEqual([
    "inbox_workspace_save", "plugin:dialog|message", "plugin:window|destroy", "plugin:window|destroy",
  ]);
});

test("a late preparation ACK preserves later input and publishes only the click snapshot", async ({ page }) => {
  await start(page);
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.inbox_workspace_prepare;
    p.values.inbox_workspace_prepare = (args: any) => new Promise(resolve => { p.releasePrepare = () => resolve(original(args)); });
  });
  await body(page).fill("CLICK A"); await submit(page).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releasePrepare)).toBe("function");
  await body(page).fill("LATER B🙂");
  await page.evaluate(() => (window as any).__performance.releasePrepare());
  await expect(submit(page)).toBeEnabled(); await expect(body(page)).toHaveValue("LATER B🙂");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes.map((n: any) => n.payload.text))).toEqual(["CLICK A"]);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("LATER B🙂");
});

test("saved source restores across reload without automatic publication or repeated reads on navigation", async ({ page }) => {
  await start(page); await body(page).fill("源草稿🙂");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("源草稿🙂");
  const seed = await page.evaluate(() => ({ entry: (window as any).__performance.inboxFixture.entry }));
  await navigate(page, "概览"); await navigate(page, "投料口");
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "inbox_workspace_get").length)).toBe(1);
  await page.addInitScript(seed => { (window as any).__inboxSeed = seed; }, seed);
  await start(page); await expect(body(page)).toHaveValue("源草稿🙂");
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => /inbox_workspace_(prepare|submit|cancel|resolve)/.test(c)))).toEqual([]);
});

for (const stage of ["reserved", "ready"] as const) test(`${stage} restoration and inspection never publish; explicit recovery uses the original id`, async ({ page }) => {
  const a = draft("ORIGINAL A"), b = draft("LATER B");
  await page.addInitScript(seed => { (window as any).__inboxSeed = seed; }, { entry: { schema_version: 1, revision: 5, draft: b, pending: { submission_id: id, snapshot: a, stage }, last_submission_id: null }, receipts: { [id]: { state: "prepared", snapshot: a } } });
  await start(page); await expect(body(page)).toHaveValue("LATER B");
  await inspect(page).click();
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes)).toEqual([]);
  if (stage === "reserved") {
    await page.getByRole("button", { name: "完成原准备", exact: true }).click();
    await expect(page.getByRole("button", { name: "继续原收录", exact: true })).toBeVisible();
    expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes)).toEqual([]);
  }
  await page.getByRole("button", { name: "继续原收录", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.inboxFixture.notes.length)).toBe(1);
  await expect(body(page)).toHaveValue("LATER B");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes[0].id)).toBe(id);
});

test("unknown publication offers observation rather than retry or cancellation and carries its id into AI context", async ({ page }) => {
  const a = draft("ORIGINAL"), b = draft("NEW UNSENT INPUT");
  await page.addInitScript(seed => { (window as any).__inboxSeed = seed; }, { entry: { schema_version: 1, revision: 5, draft: b, pending: { submission_id: id, snapshot: a, stage: "ready" }, last_submission_id: null }, receipts: { [id]: { state: "unknown", snapshot: a } } });
  await start(page); await expect(submit(page)).toBeDisabled();
  await expect(page.getByRole("button", { name: "继续原收录", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "取消本次准备，保留输入", exact: true })).toHaveCount(0);
  await inspect(page).click();
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByText("工作流资料已保留", { exact: true })).toBeVisible();
  const saved = await page.evaluate(() => { const p = (window as any).__performance; return p.desktopStore.get(p.createRequests.at(-1).requestId).body.text; });
  expect(saved).toContain(id); expect(saved).toContain('"matches_current": false'); expect(saved).toContain("NEW UNSENT INPUT");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes)).toEqual([]);
});

test("a corrupt source never becomes an editable empty draft and can be explicitly read again", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
  await page.evaluate(() => { const p = (window as any).__performance; p.restoreGet = p.values.inbox_workspace_get; p.values.inbox_workspace_get = () => { throw Error("INBOX_WORKSPACE_READ"); }; });
  await navigate(page, "投料口"); await expect(body(page)).toBeDisabled();
  await expect(page.getByLabel("资料草稿状态")).toContainText("INBOX_WORKSPACE_READ");
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => /inbox_workspace_(save|prepare|submit)/.test(c)))).toEqual([]);
  await page.evaluate(() => { const p = (window as any).__performance; p.values.inbox_workspace_get = p.restoreGet; });
  await inspect(page).click(); await expect(body(page)).toBeEnabled();
});

test("a conflicting window never overwrites the saved version until an explicit choice", async ({ page }) => {
  await start(page); await body(page).fill("BASE");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("BASE");
  await page.evaluate(() => { const p = (window as any).__performance; p.inboxFixture.entry.draft.text = "OTHER WINDOW"; p.inboxFixture.entry.revision++; });
  await body(page).fill("LOCAL EDIT");
  await expect(page.getByLabel("资料草稿状态")).toContainText("CONFLICT");
  await inspect(page).click();
  await expect(body(page)).toHaveValue("LOCAL EDIT");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("OTHER WINDOW");
  await page.getByRole("button", { name: "以当前输入保存", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.inboxFixture.entry.draft.text)).toBe("LOCAL EDIT");
  expect(await page.evaluate(() => (window as any).__performance.inboxFixture.notes)).toEqual([]);
});
