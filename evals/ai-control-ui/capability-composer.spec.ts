import { test, expect, type Page } from "@playwright/test";
import { readFileSync } from "node:fs";
const format = JSON.parse(
  readFileSync(
    new URL("../../control/capability-selection-format.json", import.meta.url),
    "utf8",
  ),
);

async function prepare(page: Page) {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    (window as any).__performance.values.capability_index_get = {
      version: "homebase-capability-index-v1",
      issues: [],
      routing: "synthetic",
      entries: [
        {
          entry_key: "c".repeat(64),
          id: "synthetic-route",
          function: "text",
          kind: "model",
          name: "合成参考模型",
          provider: "synthetic",
          state: "not_verified",
          cost: "unknown",
          discover: "model_capability_resolve",
          target: "synthetic-route",
          prerequisites: ["仅合成元数据，未调用服务"],
        },
      ],
    };
  });
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("button", { name: /文字与推理.*1 个实现/ }).click();
  await page
    .getByRole("dialog")
    .locator("article")
    .getByRole("button", { name: "使用这个实现准备任务", exact: true })
    .click();
  const input = page.getByLabel("发送消息", { exact: true });
  await expect(input).toBeVisible();
  await expect(input).toHaveValue("");
  await expect(page.getByText("能力资料已保留", { exact: true })).toBeVisible();
  return page.evaluate(() => {
    const p = (window as any).__performance;
    const scope = p.createRequests.at(-1).requestId;
    return { scope, body: p.desktopStore.get(scope).body.text };
  });
}
async function reloadBody(
  page: Page,
  scope: string,
  text: string,
  extra: Record<string, unknown> = {},
) {
  const entries = await page.evaluate(() =>
    Object.fromEntries((window as any).__performance.desktopStore),
  );
  const route = await page.evaluate(() => {
    const l = (window as any).__performance.location;
    return l.pathname + l.search;
  });
  entries[scope] = {
    ...entries[scope],
    body: { ...entries[scope].body, text },
    ...extra,
  };
  await page.addInitScript(
    ({ entries, route }) => {
      (window as any).__desktopSeed = entries;
      (window as any).__initialRoute = route;
    },
    { entries, route },
  );
  await page.reload();
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
}
test("capability preparation leaves a clean task editor while keeping full protected context", async ({
  page,
}) => {
  const { scope, body } = await prepare(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("只核对这个实现的状态，不执行任务🙂");
  await expect(input).toHaveValue("只核对这个实现的状态，不执行任务🙂");
  await expect
    .poll(() =>
      page.evaluate(() =>
        [...(window as any).__performance.desktopStore.values()].some(
          (e: any) =>
            e.body.text.includes("只核对这个实现的状态，不执行任务🙂") &&
            e.body.text.includes("homebase-capability-selection-v1"),
        ),
      ),
    )
    .toBe(true);
  await page.getByRole("button", { name: "全文编辑", exact: true }).click();
  await expect(input).toHaveValue(
    "只核对这个实现的状态，不执行任务🙂\n\n" + body,
  );
  await page.getByRole("button", { name: "只编辑需求", exact: true }).click();
  await reloadBody(
    page,
    scope,
    "只核对这个实现的状态，不执行任务🙂\n\n" + body,
  );
  await expect(input).toHaveValue("只核对这个实现的状态，不执行任务🙂");
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter((c: string) =>
        [
          "assistant_send",
          "assistant_desktop_send",
          "model_capability_call",
        ].includes(c),
      ),
    ),
  ).toEqual([]);
});

test("legacy conversion is explicit and never changes an unknown pending request", async ({
  page,
}) => {
  const { scope, body } = await prepare(page);
  const json = body.split(format.json_marker)[1].split(format.end)[0];
  const legacy =
    "请帮我准备文字与推理任务，具体目标由我补充。\n\n我的任务：\n\n旧任务🙂" +
    format.rules +
    format.json_marker +
    json;
  await reloadBody(page, scope, legacy);
  const input = page.getByLabel("发送消息", { exact: true });
  await expect(input).toHaveValue(legacy);
  await page
    .getByRole("button", { name: "整理为任务输入（保留资料）", exact: true })
    .click();
  await expect(input).toHaveValue("旧任务🙂");
  await expect
    .poll(() =>
      page.evaluate(
        (scope) =>
          (window as any).__performance.desktopStore.get(scope).body.text,
        scope,
      ),
    )
    .toBe("旧任务🙂\n\n" + body);
  const pending = {
    state: "unknown",
    request: {
      scope,
      request_id: "frozen-request",
      session_id: scope,
      text: legacy,
      mode: "auto",
    },
  };
  await reloadBody(page, scope, legacy, { pending });
  await expect(input).toHaveValue(legacy);
  await expect(
    page.getByRole("button", {
      name: "整理为任务输入（保留资料）",
      exact: true,
    }),
  ).toBeDisabled();
  expect(
    await page.evaluate(
      (scope) => (window as any).__performance.desktopStore.get(scope).pending,
      scope,
    ),
  ).toEqual(pending);
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.includes("assistant_desktop_send"),
    ),
  ).toBe(false);
});

test("voice insertion appends once to full context and remains visible after reload", async ({
  page,
}) => {
  const { scope, body } = await prepare(page);
  await reloadBody(page, scope, body, {
    voices: [
      {
        recording_id: "synthetic-voice",
        phase: "ready",
        audio_sha256: "a".repeat(64),
        job: null,
        transcript: "蓝色圆点，简短中文🙂",
        message: "",
      },
    ],
  });
  await page.getByRole("button", { name: "插入草稿", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "蓝色圆点，简短中文🙂",
  );
  await expect
    .poll(() =>
      page.evaluate(
        (scope) =>
          (window as any).__performance.desktopStore.get(scope).body.text,
        scope,
      ),
    )
    .toBe(body + "\n蓝色圆点，简短中文🙂");
  const voices = await page.evaluate(
    (scope) => (window as any).__performance.desktopStore.get(scope).voices,
    scope,
  );
  await reloadBody(page, scope, body + "\n蓝色圆点，简短中文🙂", { voices });
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "蓝色圆点，简短中文🙂",
  );
  await expect(
    page.getByRole("button", { name: "插入草稿", exact: true }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter((c: string) =>
        [
          "assistant_voice_apply",
          "assistant_desktop_send",
          "asr_task_start",
        ].includes(c),
      ),
    ),
  ).toEqual([]);
});

test("hidden context counts toward the wire limit and IME Enter cannot submit", async ({
  page,
}) => {
  const { scope, body } = await prepare(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_config_get.settings.enabled = true;
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("中".repeat(2300));
  await expect(page.getByText(/发送上限为 8,000 字节/)).toBeVisible();
  await input.press("Enter");
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.includes("assistant_desktop_send"),
    ),
  ).toBe(false);
  await expect
    .poll(() =>
      page.evaluate(
        (scope) =>
          (window as any).__performance.desktopStore.get(scope).body.text,
        scope,
      ),
    )
    .toBe("中".repeat(2300) + "\n\n" + body);
  await input.fill("中文输入测试");
  await input.dispatchEvent("compositionstart");
  await input.press("Enter");
  await input.dispatchEvent("compositionend");
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.includes("assistant_desktop_send"),
    ),
  ).toBe(false);
  await input.fill("");
  await expect(
    page.getByRole("button", { name: "发送", exact: true }),
  ).toBeDisabled();
});

test("sent messages show the human request and literal metadata with safe narrow layout", async ({
  page,
}) => {
  const { scope, body } = await prepare(page),
    task = "请核对这个实现，不执行任务。";
  const full =
    task +
    "\n\n" +
    body.replace(
      "合成参考模型",
      "<img src='https://example.test/private' onerror='bad()'>",
    );
  await page.evaluate(
    ({ scope, full }) => {
      const p = (window as any).__performance;
      p.createdSessions.delete(scope);
      p.values.assistant_session_get = () => ({
        id: scope,
        source: "desktop",
        messages: [{ id: "message", role: "user", content: full }],
        turns: [],
      });
    },
    { scope, full },
  );
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const message = page.locator("article.chat-message-user");
  await expect(message.getByText(task, { exact: true })).toBeVisible();
  await expect(
    message.getByText("附带能力资料", { exact: true }),
  ).toBeVisible();
  await message.locator("summary").click();
  await expect(message.getByLabel("能力资料原文")).toContainText("<img src=");
  expect(await message.locator("img").count()).toBe(0);
  await page.evaluate(() => Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text: string) => { (window as any).__copiedFullMessage=text; } } }));
  await message.getByRole("button", { name: "复制完整消息", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__copiedFullMessage)).toBe(full);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark" });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.getByRole("button", { name: "全文编辑", exact: true }).click();
  await page
    .getByLabel("发送消息", { exact: true })
    .fill(body + "\n无法识别的额外" + format.end);
  await expect(page.getByText("能力资料已保留", { exact: true })).toHaveCount(
    0,
  );
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    body + "\n无法识别的额外" + format.end,
  );
});
