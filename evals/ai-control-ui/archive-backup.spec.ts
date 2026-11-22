import { expect, test, type Page } from "@playwright/test";
import { archiveBackupIdKey } from "../../src/lib/assistant-archive-backup";

type BackupState = "published" | "incomplete" | "not_found";
const otherId = "413f4c6d-6949-4148-8c0e-464e9662cd0e";
const title = "会话备份";

async function fixture(page: Page) {
  await page.evaluate((key) => {
    const p = (window as any).__performance;
    p.archive = {
      calls: [],
      create: "published",
      read: "published",
      reply(id: string, state: BackupState) {
        return {
          backup_id: id, state, format: 3, segments: 6,
          directory: `F:/synthetic/backups/${id}`,
          generation: "8d644988-28b7-4c0c-8cfa-1e30be8deb44",
          encrypted_bytes: 10240,
          files: { saved: 2, unavailable: 3 },
        };
      },
    };
    const archive = p.archive;
    p.values.assistant_archive_backup = ({ backupId }: {backupId: string}) => {
      archive.calls.push(["create", backupId, localStorage.getItem(key)]);
      if (archive.create === "hang") {
        return new Promise((resolve) => {
          archive.complete = () => resolve(archive.reply(backupId, "published"));
        });
      }
      if (archive.create === "legacy") throw Error("ARCHIVE_BACKUP_NO_INDEX: synthetic");
      return archive.reply(backupId, "published");
    };
    p.values.assistant_archive_backup_get = ({ backupId }: {backupId: string}) => {
      archive.calls.push(["status", backupId]);
      if (archive.read === "mismatch") return archive.reply("unrelated-id", "published");
      return archive.reply(backupId, archive.read);
    };
    archive.items = Array.from({ length: 21 }, (_, index) => ({
      id: `7c0a4c6d-6949-4148-8c0e-${index.toString(16).padStart(12, "0")}`,
      kind: index === 0 ? "draft" : index === 20 ? "recording" : "document",
      state: index === 1 ? "changed" : index === 2 ? "unavailable" : "saved",
      name: index === 20 ? "input.wav" : `合成资料 ${index}.pdf`,
      original_id: index === 0 ? "new" : String(index),
      sha256: "a".repeat(64),
    }));
    p.values.assistant_archive_backup_files = ({ backupId, offset }: {backupId: string; offset: number}) => {
      archive.calls.push(["files", backupId, String(offset)]);
      const items = archive.fileMode === "legacy" ? [] : archive.items;
      const reply = {
        backup_id: archive.fileMode === "mismatch" ? "wrong-backup" : backupId,
        indexed: archive.fileMode !== "legacy",
        drafts_indexed: archive.fileMode !== "legacy",
        images_indexed: archive.fileMode === "with-image",
        unreadable_drafts: archive.fileMode === "legacy" ? 0 : 1,
        total: items.length,
        items: items.slice(offset, offset + 20),
        next_offset: items.length > offset + 20 ? offset + 20 : null,
      };
      if (archive.fileMode === "hold") {
        return new Promise((resolve) => { archive.completeFiles = () => resolve(reply); });
      }
      return reply;
    };
    p.values.assistant_archive_backup_draft_get = ({ backupId, fileId }: {backupId: string; fileId: string}) => {
      archive.calls.push(["draft", backupId, fileId]);
      return {
        backup_id: backupId, file_id: fileId,
        entry: {
          scope: "new", revision: 5, body: { text: "后续编辑内容", ...(archive.draftImage ? { image: archive.draftImage } : {}) },
          pending: { request: { request_id: "synthetic-original", text: "原发送正文" } },
          voices: [{}],
        },
      };
    };
    p.values.assistant_archive_backup_file_recover = ({ backupId, fileId }: {backupId: string; fileId: string}) => {
      archive.calls.push(["recover", backupId, fileId]);
      if (archive.recoverMode === "hold") return new Promise(() => {});
      if (archive.recoverMode === "changed") throw Error("ARCHIVE_CHANGED: fixture");
      const file = archive.items.find((item: {id: string}) => item.id === fileId);
      return { backup_id: backupId, file, path: `F:/synthetic/backups/${backupId}/recovered/${fileId}.${file?.kind === "image" ? "png" : "wav"}` };
    };
    p.values["plugin:clipboard-manager|write_text"] = ({ text }: {text: string}) => {
      archive.calls.push(["copy", text]);
    };
  }, archiveBackupIdKey);
}

const calls = (page: Page): Promise<string[][]> =>
  page.evaluate(() => (window as any).__performance.archive.calls);
const saved = (page: Page) =>
  page.evaluate((key) => localStorage.getItem(key), archiveBackupIdKey);
const getDialog = (page: Page) => page.getByRole("dialog", { name: title });
async function open(page: Page) {
  await page.getByRole("button", { name: title, exact: true }).click();
  await expect(getDialog(page)).toBeVisible();
}

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1" ? route.continue() : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByRole("heading", { name: "AI 助理", exact: true })).toBeVisible();
  await fixture(page);
  await page.getByRole("link", { name: "会话", exact: true }).click();
});

test("backs up explicitly, stores the original UUID before IPC, and copies the verified local directory", async ({ page }) => {
  expect(await calls(page)).toEqual([]);
  await open(page);
  expect(await calls(page)).toEqual([]);
  const dialog = getDialog(page);
  await expect(dialog.getByText(/PDF 原件、任务产物、桌面草稿与录音/)).toBeVisible();
  const button = dialog.getByRole("button", { name: "创建加密备份" });
  await button.evaluate((element: HTMLButtonElement) => {
    element.click();
    element.click();
  });
  await expect(dialog.getByText("备份已发布 · 6 个分段 · 10 KiB")).toBeVisible();
  await expect(dialog.getByText("配套文件已保存 2 项，3 项不可用。")).toBeVisible();
  const id = await saved(page);
  expect(id).toMatch(/^[0-9a-f-]{36}$/);
  expect(await calls(page)).toEqual([["create", id, id]]);
  await dialog.getByRole("button", { name: "复制目录" }).click();
  await expect(dialog.getByText("目录已复制。")).toBeVisible();
  expect((await calls(page)).at(-1)).toEqual(["copy", `F:/synthetic/backups/${id}`]);
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("button", { name: title, exact: true })).toBeFocused();
  await open(page);
  await expect(dialog.getByText(/备份已发布/)).toBeVisible();
  expect((await calls(page)).filter(([method]) => method === "status")).toEqual([["status", id]]);
});

test("route change and full reload query the original backup without resubmission", async ({ page }) => {
  await page.evaluate(() => {
    const archive = (window as any).__performance.archive;
    archive.create = "hang";
    archive.read = "incomplete";
  });
  await open(page);
  await getDialog(page).getByRole("button", { name: "创建加密备份" }).click();
  await expect(getDialog(page).getByText(/正在创建/)).toBeVisible();
  const id = await saved(page);
  await page.keyboard.press("Escape");
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await open(page);
  await expect(getDialog(page).getByText(/密文头尚未发布/)).toBeVisible();
  expect(await calls(page)).toEqual([["create", id, id], ["status", id]]);
  await page.evaluate(() => (window as any).__performance.archive.complete());
  await expect(getDialog(page).getByText(/密文头尚未发布/)).toBeVisible();
  await page.reload();
  await expect(page.getByRole("heading", { name: "AI 助理", exact: true })).toBeVisible();
  await fixture(page);
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await open(page);
  await expect(getDialog(page).getByText(/备份已发布/)).toBeVisible();
  expect(await calls(page)).toEqual([["status", id]]);
  expect(await saved(page)).toBe(id);
});

test("a timed-out response keeps the original ID while the user checks the result", async ({ page }) => {
  await page.clock.install();
  await page.evaluate(() => {
    const archive = (window as any).__performance.archive;
    archive.create = "hang";
    archive.read = "incomplete";
  });
  await open(page);
  const dialog = getDialog(page);
  await dialog.getByRole("button", { name: "创建加密备份" }).click();
  const id = await saved(page);
  await page.clock.fastForward(16_000);
  await expect(dialog.getByRole("alert")).toHaveText("备份结果待核对。请查询原编号。");
  await dialog.getByRole("button", { name: "查询状态" }).click();
  await expect(dialog.getByText(/密文头尚未发布/)).toBeVisible();
  await page.evaluate(() => (window as any).__performance.archive.complete());
  expect(await calls(page)).toEqual([["create", id, id], ["status", id]]);
  await expect(dialog.getByText(/密文头尚未发布/)).toBeVisible();
});

test("a legacy profile can query absence and explicitly continue using the same UUID", async ({ page }) => {
  await page.evaluate(() => {
    const archive = (window as any).__performance.archive;
    archive.create = "legacy";
    archive.read = "not_found";
  });
  await open(page);
  const dialog = getDialog(page);
  await dialog.getByRole("button", { name: "创建加密备份" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("当前还没有可备份的分段档案。");
  const id = await saved(page);
  await dialog.getByRole("button", { name: "查询状态" }).click();
  await expect(dialog.getByText("尚未找到这个备份。")).toBeVisible();
  await page.evaluate(() => { (window as any).__performance.archive.create = "published"; });
  await dialog.getByRole("button", { name: "用原编号继续创建" }).click();
  await expect(dialog.getByText(/备份已发布/)).toBeVisible();
  expect(await calls(page)).toEqual([["create", id, id], ["status", id], ["create", id, id]]);
});

test("manual lookup handles a different ID and rejects a mismatched receipt", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 680 });
  await page.evaluate(() => {
    document.documentElement.classList.add("dark");
    (window as any).__performance.archive.read = "mismatch";
  });
  await open(page);
  const dialog = getDialog(page);
  expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
  await dialog.getByText("按编号查询", { exact: true }).click();
  const input = dialog.getByRole("textbox", { name: "要查询的备份编号" });
  await input.fill("../other");
  await expect(dialog.getByRole("button", { name: "查询备份" })).toBeDisabled();
  await input.fill(otherId);
  await input.press("Enter");
  await expect(dialog.getByRole("alert")).toHaveText("暂时无法确认备份状态，请再次查询。");
  await expect(dialog.getByRole("button", { name: "复制目录" })).toHaveCount(0);
  await page.evaluate(() => { (window as any).__performance.archive.read = "published"; });
  await dialog.getByRole("button", { name: "查询状态" }).click();
  await expect(dialog.getByText(/备份已发布/)).toBeVisible();
  expect(await calls(page)).toEqual([["status", otherId], ["status", otherId]]);
  expect(await saved(page)).toBeNull();
});

test("a failed local pointer write leaves the native backup command untouched", async ({ page }) => {
  await page.evaluate((key) => {
    const original = Storage.prototype.setItem;
    Storage.prototype.setItem = function (name, value) {
      if (name === key) throw Error("synthetic local quota");
      original.call(this, name, value);
    };
  }, archiveBackupIdKey);
  await open(page);
  const dialog = getDialog(page);
  await dialog.getByRole("button", { name: "创建加密备份" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("无法读写本次备份编号。可按编号查询已有备份。");
  expect(await calls(page)).toEqual([]);
  expect(await saved(page)).toBeNull();
});

async function createPublished(page: Page) {
  await open(page);
  const dialog = getDialog(page);
  await dialog.getByRole("button", { name: "创建加密备份" }).click();
  await expect(dialog.getByText(/备份已发布/)).toBeVisible();
  return dialog;
}

test("loads file pages on demand, caches them, and previews a draft without extraction", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 680 });
  const dialog = await createPublished(page);
  const id = await saved(page);
  expect(await calls(page)).toEqual([["create", id, id]]);
  await dialog.getByRole("button", { name: "查看文件清单" }).click();
  await expect(dialog.getByText("1 份草稿无法核验。")).toBeVisible();
  await expect(dialog.getByText("PDF 原件 · 原件已变化")).toBeVisible();
  await expect(dialog.getByText("PDF 原件 · 不可用")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "取回原件 合成资料 1.pdf" })).toHaveCount(0);
  await dialog.getByRole("button", { name: "只读查看草稿" }).click();
  await expect(dialog.getByText("后续编辑内容")).toBeVisible();
  await expect(dialog.getByText("原发送正文")).toBeVisible();
  await expect(dialog.getByText("synthetic-original")).toBeVisible();
  await expect(dialog.getByText("包含 1 条录音回执。")).toBeVisible();
  await dialog.getByRole("button", { name: "收起文件清单" }).click();
  await dialog.getByRole("button", { name: "查看文件清单" }).click();
  await expect(dialog.getByText("原发送正文")).toHaveCount(0);
  await dialog.getByRole("button", { name: "下一页" }).click();
  await expect(dialog.getByText("input.wav", { exact: true })).toBeVisible();
  await expect(dialog.getByText("21–21 / 21")).toBeVisible();
  await dialog.getByRole("button", { name: "上一页" }).click();
  await expect(dialog.getByText("1–20 / 21")).toBeVisible();
  expect((await calls(page)).slice(1).map(([method, , offset]) =>
    method === "files" ? [method, offset] : [method])).toEqual([["files", "0"], ["draft"], ["files", "20"]]);
  expect(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
});

for (const hasImageIndex of [true, false]) {
  test(`draft with an image ${hasImageIndex ? "recovers from the indexed original" : "explains a missing image index"}`, async ({ page }) => {
    await page.evaluate((hasImageIndex) => {
      const archive = (window as any).__performance.archive;
      archive.draftImage = { scope: "new", name: "测试图片.png", sha256: "a".repeat(64), bytes: 75 };
      if (hasImageIndex) {
        archive.fileMode = "with-image";
        Object.assign(archive.items[3], { kind: "image", name: archive.draftImage.name, original_id: `new:${archive.draftImage.sha256}` });
      }
    }, hasImageIndex);
    const dialog = await createPublished(page);
    await dialog.getByRole("button", { name: "查看文件清单" }).click();
    await dialog.getByRole("button", { name: "只读查看草稿" }).click();
    await expect(dialog.getByText("PNG 附件 · 测试图片.png", { exact: true })).toBeVisible();
    const recover = dialog.getByRole("button", { name: "取回原件 测试图片.png" });
    expect((await calls(page)).filter(([method]) => method === "recover")).toHaveLength(0);
    if (hasImageIndex) {
      await expect(dialog.getByText("PNG 图片 · 已保存")).toBeVisible();
      await recover.click();
      await expect(dialog.getByText(/\/recovered\/.*\.png$/)).toBeVisible();
      expect((await calls(page)).filter(([method]) => method === "recover")).toHaveLength(1);
    } else {
      await expect(dialog.getByText("这份备份没有图片清单，无法从中取回 PNG。")).toBeVisible();
      await expect(recover).toHaveCount(0);
    }
  });
}

test("an extraction timeout stays uncertain and explicit retry uses the original item", async ({ page }) => {
  await page.clock.install();
  const dialog = await createPublished(page);
  await dialog.getByRole("button", { name: "查看文件清单" }).click();
  await dialog.getByRole("button", { name: "下一页" }).click();
  const button = dialog.getByRole("button", { name: "取回原件 input.wav" });
  await page.evaluate(() => { (window as any).__performance.archive.recoverMode = "hold"; });
  await button.evaluate((element: HTMLButtonElement) => { element.click(); element.click(); });
  await expect(dialog.getByText("正在核验并取回原件…")).toBeVisible();
  await page.clock.fastForward(16_000);
  await expect(dialog.getByRole("alert")).toHaveText(/取回结果待核对/);
  expect((await calls(page)).filter(([method]) => method === "recover")).toHaveLength(1);
  await page.evaluate(() => { (window as any).__performance.archive.recoverMode = "changed"; });
  await button.click();
  await expect(dialog.getByRole("alert")).toHaveText(/现有文件已保留/);
  await page.evaluate(() => { (window as any).__performance.archive.recoverMode = "saved"; });
  await button.click();
  await expect(dialog.getByText(/已取回到/)).toBeVisible();
  await expect(button).toBeDisabled();
  const extractions = (await calls(page)).filter(([method]) => method === "recover");
  expect(extractions).toHaveLength(3);
  expect(new Set(extractions.map(call => JSON.stringify(call))).size).toBe(1);
});

test("a mismatched file page is rejected and a legacy backup shows no file index", async ({ page }) => {
  const dialog = await createPublished(page);
  await page.evaluate(() => { (window as any).__performance.archive.fileMode = "mismatch"; });
  await dialog.getByRole("button", { name: "查看文件清单" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("暂时无法读取文件清单。");
  await expect(dialog.getByRole("button", { name: "只读查看草稿" })).toHaveCount(0);
  await page.evaluate(() => { (window as any).__performance.archive.fileMode = "legacy"; });
  await dialog.getByRole("button", { name: "重试读取" }).click();
  await expect(dialog.getByText("这份旧备份没有配套文件清单。")).toBeVisible();
  expect((await calls(page)).map(([method]) => method)).toEqual(["create", "files", "files"]);
});

test("closing a backup drops the file view and ignores a late page", async ({ page }) => {
  const dialog = await createPublished(page);
  await page.evaluate(() => { (window as any).__performance.archive.fileMode = "hold"; });
  await dialog.getByRole("button", { name: "查看文件清单" }).click();
  await expect(dialog.getByText("正在读取文件清单…")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await open(page);
  await expect(dialog.getByText(/备份已发布/)).toBeVisible();
  await page.evaluate(() => (window as any).__performance.archive.completeFiles());
  await expect(dialog.getByRole("button", { name: "查看文件清单" })).toHaveAttribute("aria-expanded", "false");
  await expect(dialog.getByRole("button", { name: "只读查看草稿" })).toHaveCount(0);
  expect((await calls(page)).map(([method]) => method)).toEqual(["create", "files", "status"]);
});
