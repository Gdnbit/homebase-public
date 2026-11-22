import { test, expect, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { navigate } from "./navigation";
const format = JSON.parse(
  readFileSync(
    new URL("../../control/workflow-context-format.json", import.meta.url),
    "utf8",
  ),
);
const callsWithoutEffects = async (page: Page) =>
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter((c: string) =>
        [
          "assistant_send",
          "assistant_desktop_send",
          "inbox_submit",
          "studio_chat_send",
          "studio_chat_cancel",
        ].includes(c),
      ),
    ),
  ).toEqual([]);
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
});
async function material(page: Page) {
  await navigate(page, "投料口");
  await page
    .getByPlaceholder("标题（可选 · 空着会用首行前 40 字）")
    .fill("隔离资料🙂");
  await page
    .locator("textarea")
    .first()
    .fill(
      "完整材料<img src='https://example.test/no-fetch' onerror='bad()'>🙂",
    );
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(
    page.getByText("工作流资料已保留", { exact: true }),
  ).toBeVisible();
  return page.evaluate(() => {
    const p = (window as any).__performance,
      scope = p.createRequests.at(-1).requestId;
    return { scope, body: p.desktopStore.get(scope).body.text };
  });
}
async function reload(
  page: Page,
  scope: string,
  body: string,
  extra: Record<string, unknown> = {},
) {
  const seed = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      entries: Object.fromEntries(p.desktopStore),
      route: p.location.pathname + p.location.search,
    };
  });
  seed.entries[scope] = {
    ...seed.entries[scope],
    body: { ...seed.entries[scope].body, text: body },
    ...extra,
  };
  await page.addInitScript((seed) => {
    (window as any).__desktopSeed = seed.entries;
    (window as any).__initialRoute = seed.route;
  }, seed);
  await page.reload();
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
}
test("material preparation exposes a natural task and protects the original source through edits and reload", async ({
  page,
}) => {
  const { scope, body } = await material(page),
    input = page.getByLabel("发送消息", { exact: true });
  await expect(input).toHaveValue(/请帮我整理并收录这份资料/);
  expect(await input.inputValue()).not.toContain("完整材料");
  await page
    .getByLabel("原工作流资料", { exact: true })
    .locator("summary")
    .click();
  await expect(page.getByLabel("工作流资料原文")).toContainText("完整材料<img");
  expect(
    await page
      .getByLabel("原工作流资料", { exact: true })
      .locator("img")
      .count(),
  ).toBe(0);
  const task = "只检查这份资料是否已收录，不保存🙂";
  await input.fill(task);
  const frame = body.slice(body.indexOf(format.start)),
    full = task + "\n\n" + frame;
  await expect
    .poll(() =>
      page.evaluate(
        (scope) =>
          (window as any).__performance.desktopStore.get(scope).body.text,
        scope,
      ),
    )
    .toBe(full);
  await page.getByRole("button", { name: "全文编辑", exact: true }).click();
  await expect(input).toHaveValue(full);
  await page.getByRole("button", { name: "只编辑需求", exact: true }).click();
  await reload(page, scope, full);
  await expect(input).toHaveValue(task);
  await callsWithoutEffects(page);
});
test("hidden material editor cannot submit on the assistant's Ctrl+Enter and optional reads stay paused", async ({
  page,
}) => {
  await material(page);
  const before = await page.evaluate(() => {
    const p = (window as any).__performance;
    return p.calls.filter((c: string) =>
      ["inbox_list_recent", "wiki_agent_status"].includes(c),
    ).length;
  });
  await page.getByLabel("发送消息", { exact: true }).press("Control+Enter");
  await page.evaluate(async () => {
    const p = (window as any).__performance;
    await Promise.all([
      p.client.invalidateQueries({ queryKey: ["inbox"] }),
      p.client.invalidateQueries({ queryKey: ["wiki-agent", "status"] }),
    ]);
    window.dispatchEvent(new Event("focus"));
  });
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.calls.filter((c: string) =>
          ["inbox_list_recent", "wiki_agent_status"].includes(c),
        ).length,
    ),
  ).toBe(before);
  await callsWithoutEffects(page);
  await navigate(page, "投料口");
  await expect(
    page.getByPlaceholder("标题（可选 · 空着会用首行前 40 字）"),
  ).toHaveValue("隔离资料🙂");
  await expect(
    page.locator('section[aria-label="资料编辑工作区"] textarea').first(),
  ).toHaveValue(/完整材料<img/);
});
test("one-newline voice insertion preserves the entire workflow block and cannot insert again after reload", async ({
  page,
}) => {
  const { scope, body } = await material(page);
  await reload(page, scope, body, {
    voices: [
      {
        recording_id: "workflow-voice",
        phase: "ready",
        audio_sha256: "b".repeat(64),
        job: null,
        transcript: "另外只检查已有记录🙂",
        message: "",
      },
    ],
  });
  await page.getByRole("button", { name: "插入草稿", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    /另外只检查已有记录🙂$/,
  );
  await expect
    .poll(() =>
      page.evaluate(
        (scope) =>
          (window as any).__performance.desktopStore.get(scope).body.text,
        scope,
      ),
    )
    .toBe(body + "\n另外只检查已有记录🙂");
  const voices = await page.evaluate(
    (scope) => (window as any).__performance.desktopStore.get(scope).voices,
    scope,
  );
  await reload(page, scope, body + "\n另外只检查已有记录🙂", { voices });
  await expect(
    page.getByRole("button", { name: "插入草稿", exact: true }),
  ).toHaveCount(0);
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    /另外只检查已有记录🙂$/,
  );
  await callsWithoutEffects(page);
});
test("legacy and unrecognized workflow texts stay fully visible and an original unknown request is not replaced", async ({
  page,
}) => {
  const { scope, body } = await material(page),
    input = page.getByLabel("发送消息", { exact: true });
  await page.getByRole("button", { name: "全文编辑", exact: true }).click();
  const unknown = body.replace('"workflow": "inbox"', '"workflow": "unknown"');
  await input.fill(unknown);
  await expect(page.getByText("工作流资料已保留", { exact: true })).toHaveCount(
    0,
  );
  await expect(input).toHaveValue(unknown);
  const old = '旧资料原文\n{"text":"原始材料","attachments":[]}';
  const pending = {
    state: "unknown",
    request: {
      scope,
      request_id: "original-workflow-request",
      session_id: scope,
      text: old,
      mode: "auto",
    },
  };
  await reload(page, scope, old, { pending });
  await expect(input).toHaveValue(old);
  expect(
    await page.evaluate(
      (scope) => (window as any).__performance.desktopStore.get(scope).pending,
      scope,
    ),
  ).toEqual(pending);
  await callsWithoutEffects(page);
});
test("oversized manuscript requirements fail before creation and remain editable on the original page", async ({
  page,
}) => {
  await navigate(page, "文章生成");
  await page.getByTitle("新建对话", { exact: true }).click();
  const input = page.getByPlaceholder(
      "输入需求 · Enter 发送 · Shift+Enter 换行",
    ),
    long = "完整要求🙂".repeat(1500);
  await input.fill(long);
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("超过8,000字节");
  await expect(input).toHaveValue(long);
  expect(
    await page.evaluate(
      () => (window as any).__performance.createRequests.length,
    ),
  ).toBe(0);
  await input.fill("请使用简短中文标题🙂");
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(
    page.getByText("工作流资料已保留", { exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    /请使用简短中文标题🙂$/,
  );
  await navigate(page, "文章生成");
  await expect(input).toHaveValue("请使用简短中文标题🙂");
  await callsWithoutEffects(page);
});
test("historical workflow messages copy their full source and fit narrow layout without rendering source HTML", async ({
  page,
}) => {
  const { scope, body } = await material(page),
    task = body.slice(0, body.indexOf("\n\n"));
  await page.evaluate(
    ({ scope, body }) => {
      const p = (window as any).__performance;
      p.createdSessions.delete(scope);
      p.values.assistant_session_get = () => ({
        id: scope,
        source: "desktop",
        messages: [{ id: "workflow-message", role: "user", content: body }],
        turns: [],
      });
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: async (text: string) => {
            (window as any).__workflowCopied = text;
          },
        },
      });
    },
    { scope, body },
  );
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const message = page.locator("article.chat-message-user");
  await expect(message.getByText(task, { exact: true })).toBeVisible();
  await expect(
    message.getByText("附带工作流资料", { exact: true }),
  ).toBeVisible();
  await message.locator("summary").click();
  await expect(message.getByLabel("工作流资料原文")).toContainText(
    "完整材料<img",
  );
  expect(await message.locator("img").count()).toBe(0);
  await message
    .getByRole("button", { name: "复制完整消息", exact: true })
    .click();
  expect(await page.evaluate(() => (window as any).__workflowCopied)).toBe(
    body,
  );
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark" });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await callsWithoutEffects(page);
});
test("manuscript keeps composition keys local and sends once after returning to the draft", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.studio_chat_send = () => ({
      session_id: "studio-ime-session",
      task_id: "studio-ime-task",
    });
  });
  await navigate(page, "文章生成");
  const newChat = page.getByTitle("新建对话", { exact: true });
  await newChat.click();
  const input = page.getByPlaceholder("输入需求 · Enter 发送 · Shift+Enter 换行");
  const draft = "这段隔离文稿等待明确发送";
  await input.fill(draft);
  const sends = () => page.evaluate(() => (window as any).__performance.calls.filter((command: string) => command === "studio_chat_send").length);
  const enterHandled = (keyCode: number) => input.evaluate((node, code) => {
    const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    Object.defineProperty(event, "keyCode", { value: code });
    return !node.dispatchEvent(event);
  }, keyCode);

  expect(await enterHandled(229)).toBe(false);
  await input.dispatchEvent("compositionstart");
  expect(await enterHandled(13)).toBe(false);
  expect(await sends()).toBe(0);
  await expect(input).toHaveValue(draft);

  await newChat.focus();
  await expect(input).not.toBeFocused();
  await input.press("End");
  await input.press("Shift+Enter");
  await expect(input).toHaveValue(`${draft}\n`);
  await input.press("Enter");
  await expect.poll(sends).toBe(1);
});

test("an original manuscript completes while away and navigation neither restarts nor persists it twice", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.studio_chat_send = () => ({
      session_id: "original-studio",
      task_id: "original-studio-task",
    });
    p.values.studio_persist_message = null;
  });
  await navigate(page, "文章生成");
  await page.getByTitle("新建对话", { exact: true }).click();
  await page
    .getByPlaceholder("输入需求 · Enter 发送 · Shift+Enter 换行")
    .fill("合成原文稿任务");
  await page
    .getByPlaceholder("输入需求 · Enter 发送 · Shift+Enter 换行")
    .press("Enter");
  await expect(
    page.getByRole("button", { name: "交给助理跟进", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "交给助理跟进", exact: true }).click();
  await expect(
    page.getByText("工作流资料已保留", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.emit("studio:event", {
      kind: "assistant_text",
      task_id: "original-studio-task",
      text: "离页后完成的合成文稿🙂",
    });
    p.emit("studio:event", {
      kind: "completed",
      task_id: "original-studio-task",
      session_id: "original-studio",
      exit_code: 0,
    });
  });
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (window as any).__performance.calls.filter(
            (c: string) => c === "studio_persist_message",
          ).length,
      ),
    )
    .toBe(2);
  await navigate(page, "文章生成");
  await expect(
    page.getByText("离页后完成的合成文稿🙂", { exact: true }),
  ).toBeVisible();
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await navigate(page, "文章生成");
  expect(
    await page.evaluate(() => {
      const c = (window as any).__performance.calls;
      return {
        send: c.filter((s: string) => s === "studio_chat_send").length,
        persist: c.filter((s: string) => s === "studio_persist_message").length,
        cancel: c.filter((s: string) => s === "studio_chat_cancel").length,
      };
    }),
  ).toEqual({ send: 1, persist: 2, cancel: 0 });
});
