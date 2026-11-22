import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";

const file = (name: string) => ({
  name,
  mimeType: "text/plain",
  buffer: Buffer.from("synthetic only"),
});
const body = (page: Page) => page.locator("textarea").first();
const title = (page: Page) =>
  page.getByPlaceholder("标题（可选 · 空着会用首行前 40 字）");
const tags = (page: Page) =>
  page.getByPlaceholder("标签（空格 / 逗号分隔，如 idea tech reading）");
const submit = (page: Page) => page.getByRole("button", { name: /^投递/ });

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
  await navigate(page, "投料口");
  await body(page).fill("隔离资料正文🙂");
});

test("a whole upload batch blocks submission, Ctrl+Enter and handoff until every reference is saved", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.uploads = [];
    p.releaseUploads = [];
    p.values.inbox_attach = ({ payload }: any) =>
      new Promise((resolve) => {
        const i = p.uploads.push(payload) - 1;
        p.releaseUploads[i] = () =>
          resolve({
            relative_path: `raw/inbox/2026-11-${15 + i}/_attachments/${payload.filename}`,
            final_filename: payload.filename,
            size: payload.bytes.length,
          });
      });
  });
  await page
    .locator('input[type="file"]')
    .first()
    .setInputFiles([file("same.txt"), file("same.txt")]);
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).__performance.uploads.length),
    )
    .toBe(1);
  await expect(submit(page)).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "交给助理", exact: true }),
  ).toBeDisabled();
  await body(page).press("Control+Enter");
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter(
        (c: string) => c === "inbox_submit",
      ),
    ),
  ).toEqual([]);
  await page.evaluate(() => (window as any).__performance.releaseUploads[0]());
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).__performance.uploads.length),
    )
    .toBe(2);
  await expect(submit(page)).toBeDisabled();
  await body(page).fill("等待上传期间的新要求🙂");
  await page.evaluate(() => (window as any).__performance.releaseUploads[1]());
  await expect(submit(page)).toBeEnabled();
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(
    page.getByText("工作流资料已保留", { exact: true }),
  ).toBeVisible();
  const saved = await page.evaluate(() => {
    const p = (window as any).__performance;
    return p.desktopStore.get(p.createRequests.at(-1).requestId).body.text;
  });
  expect(saved).toContain("等待上传期间的新要求🙂");
  expect(saved).toContain("raw/inbox/2026-11-15/_attachments/same.txt");
  expect(saved).toContain("raw/inbox/2026-11-16/_attachments/same.txt");
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter((c: string) =>
        ["inbox_submit", "assistant_send", "assistant_desktop_send"].includes(
          c,
        ),
      ),
    ),
  ).toEqual([]);
});

test("a late submission result clears only its submitted snapshot and preserves subsequent input", async ({
  page,
}) => {
  await title(page).fill("原标题");
  await tags(page).fill("original");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.submissions = [];
    p.values.inbox_submit = ({ payload }: any) =>
      new Promise((resolve) => {
        p.submissions.push(structuredClone(payload));
        p.finishSubmit = () =>
          resolve({
            path: "F:/synthetic/Wiki/raw/inbox/2026-11-17/once.md",
            filename: "once.md",
            day: "2026-11-17",
            char_count: payload.text.length,
          });
      });
  });
  await submit(page).click();
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).__performance.submissions.length),
    )
    .toBe(1);
  await body(page).fill("投递等待期间写下的下一条🙂");
  await title(page).fill("下一条标题");
  await tags(page).fill("next");
  await body(page).press("Control+Enter");
  expect(
    await page.evaluate(() => (window as any).__performance.submissions.length),
  ).toBe(1);
  await page.evaluate(() => (window as any).__performance.finishSubmit());
  await expect(body(page)).toHaveValue("投递等待期间写下的下一条🙂");
  await expect(title(page)).toHaveValue("下一条标题");
  await expect(tags(page)).toHaveValue("next");
  await expect(submit(page)).toBeEnabled();
  const sent = await page.evaluate(
    () => (window as any).__performance.submissions,
  );
  expect(sent).toHaveLength(1);
  expect(sent[0].text).toBe("隔离资料正文🙂");
  expect(sent[0].title).toBe("原标题");
});

test("failed upload stays visible after another batch succeeds and handoff carries the uncertainty without a retry", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.uploadCalls = 0;
    p.values.inbox_attach = ({ payload }: any) => {
      p.uploadCalls += 1;
      if (payload.filename === "failed.txt")
        throw Error("SYNTHETIC_WRITE_UNCONFIRMED");
      return {
        relative_path: "raw/inbox/2026-11-15/_attachments/saved.txt",
        final_filename: "saved.txt",
        size: payload.bytes.length,
      };
    };
  });
  const picker = page.locator('input[type="file"]').first();
  await picker.setInputFiles(file("failed.txt"));
  await expect(
    page.getByRole("status", { name: "附件保存状态" }),
  ).toContainText("有附件尚未确认保存");
  await picker.setInputFiles(file("saved.txt"));
  await expect(submit(page)).toBeEnabled();
  await expect(
    page.getByRole("status", { name: "附件保存状态" }),
  ).toContainText("有附件尚未确认保存");
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(
    page.getByText("工作流资料已保留", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("待核对", { exact: true })).toBeVisible();
  const state = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      calls: p.uploadCalls,
      draft: p.desktopStore.get(p.createRequests.at(-1).requestId).body.text,
      submitted: p.calls.filter((c: string) => c === "inbox_submit"),
    };
  });
  expect(state.calls).toBe(2);
  expect(state.submitted).toEqual([]);
  expect(state.draft).toContain('"uncertain": true');
  expect(state.draft).toContain("raw/inbox/2026-11-15/_attachments/saved.txt");
  expect(state.draft).not.toContain('"rel": "failed.txt"');
});
