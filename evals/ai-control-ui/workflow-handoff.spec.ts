import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";

const entry = (scope: string, text: string) => ({
  schema_version: 1,
  scope,
  revision: 1,
  body: { text, route: "", mode: "auto", files: [] },
  pending: null,
  voices: [],
});
const path = "/evals/ai-control-ui/performance.html";
async function start(page: Page) {
  await page.goto(path);
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
}
async function quick(page: Page) {
  await navigate(page, "快捷面板");
  return page.getByLabel("向 HomeBase Agent 输入任务");
}
async function assertNoExecution(page: Page) {
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter((c: string) =>
        [
          "assistant_send",
          "assistant_desktop_send",
          "assistant_native_send",
          "capability_render",
          "repo_capability_run",
          "studio_chat_cancel",
        ].includes(c),
      ),
    ),
  ).toEqual([]);
}
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
});

test("disabled models still allow protected draft preparation while Enter and files never submit", async ({
  page,
}) => {
  await start(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await expect(input).toBeEnabled();
  await input.fill("HB-DISABLED 合成草稿");
  await input.press("Enter");
  await expect(input).toHaveValue("HB-DISABLED 合成草稿\n");
  await page
    .getByLabel("选择文本附件")
    .setInputFiles({
      name: "disabled-fixture.md",
      mimeType: "text/markdown",
      buffer: Buffer.from("合成附件，不调用模型"),
    });
  await expect(page.getByLabel("草稿保存状态")).toContainText(
    "草稿已受保护保存",
  );
  await expect(
    page.getByRole("button", { name: "发送", exact: true }),
  ).toBeDisabled();
  await navigate(page, "能力库");
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await expect(input).toHaveValue("HB-DISABLED 合成草稿\n");
  await expect(page.getByLabel("待发送附件")).toContainText(
    "disabled-fixture.md",
  );
  await assertNoExecution(page);
});

test("quick input and suggestions create a protected independent draft, IME never submits", async ({
  page,
}) => {
  await page.addInitScript(
    (seed) => {
      (window as any).__desktopSeed = seed;
    },
    { new: entry("new", "原助理输入保持") },
  );
  await start(page);
  const input = await quick(page);
  await page.getByRole("button", { name: "检查本机能力", exact: true }).click();
  await expect(input).toHaveValue("检查本机能力");
  await input.dispatchEvent("keydown", {
    key: "Enter",
    isComposing: true,
    keyCode: 229,
  });
  expect(
    await page.evaluate(
      () => (window as any).__performance.createRequests.length,
    ),
  ).toBe(0);
  await input.fill("HB-WORKFLOW 合成需求，不自动执行");
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "HB-WORKFLOW 合成需求，不自动执行",
  );
  const state = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      entries: Object.fromEntries(p.desktopStore),
      requests: p.createRequests,
      location: p.location,
    };
  });
  expect(state.requests).toHaveLength(1);
  const id = state.requests[0].requestId;
  expect(state.entries[id].body.text).toBe("HB-WORKFLOW 合成需求，不自动执行");
  expect(state.entries[`handoff:${id}`].body.text).toBe("");
  expect(state.entries.new.body.text).toBe("原助理输入保持");
  expect(state.location.search).toBe(`?session=${id}`);
  expect(state.location.state).toBeNull();
  await assertNoExecution(page);
});

test("a lost outbox save ACK retries the same identity and never creates before confirming text", async ({
  page,
}) => {
  await start(page);
  const input = await quick(page);
  await input.fill("原文先于创建保存");
  await page.evaluate(() => {
    const p = (window as any).__performance,
      save = p.values.assistant_desktop_save;
    p.outboxScopes = [];
    let lost = false;
    p.values.assistant_desktop_save = (args: any) => {
      if (args.request.scope.startsWith("handoff:"))
        p.outboxScopes.push(args.request.scope);
      const result = save(args);
      if (!lost && args.request.scope.startsWith("handoff:")) {
        lost = true;
        throw Error("合成保存回执丢失");
      }
      return result;
    };
  });
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("回执丢失");
  await expect(input).toHaveValue("原文先于创建保存");
  expect(
    await page.evaluate(
      () => (window as any).__performance.createRequests.length,
    ),
  ).toBe(0);
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "原文先于创建保存",
  );
  expect(
    await page.evaluate(
      () => new Set((window as any).__performance.outboxScopes).size,
    ),
  ).toBe(1);
  await assertNoExecution(page);
});

test("unknown creation survives reload and resumes the original protected outbox without sending", async ({
  page,
}) => {
  await start(page);
  const input = await quick(page);
  await input.fill("HB-RESTART 原创建回执丢失");
  await page.evaluate(() => {
    const p = (window as any).__performance,
      create = p.values.assistant_session_create;
    p.values.assistant_session_create = (a: any) => {
      create(a);
      throw Error("合成创建回执丢失");
    };
  });
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("创建回执丢失");
  const snapshot = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      drafts: Object.fromEntries(p.desktopStore),
      sessions: Object.fromEntries(p.createdSessions),
    };
  });
  const id = Object.keys(snapshot.sessions)[0];
  expect(snapshot.drafts[`handoff:${id}`].body.text).toContain("HB-RESTART");
  await page.addInitScript((s) => {
    (window as any).__desktopSeed = s.drafts;
    (window as any).__assistantSeed = s.sessions;
  }, snapshot);
  await page.reload();
  await page.getByLabel("更多对话操作").click();
  await page
    .getByRole("button", { name: "未发送与待核对", exact: true })
    .click();
  await page
    .getByRole("button", { name: new RegExp(`待转入助理 · ${id.slice(0, 8)}`) })
    .click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "HB-RESTART 原创建回执丢失",
  );
  expect(
    await page.evaluate(() => [
      ...(window as any).__performance.createdSessions.keys(),
    ]),
  ).toEqual([id]);
  expect(
    await page.evaluate(() =>
      (window as any).__performance.createRequests.map((r: any) => r.requestId),
    ),
  ).toEqual([id]);
  await assertNoExecution(page);
});

test("an existing pending request is never overwritten by a recovered handoff", async ({
  page,
}) => {
  await start(page);
  const input = await quick(page);
  await input.fill("不得覆盖待核对任务");
  await page.evaluate(() => {
    const p = (window as any).__performance,
      create = p.values.assistant_session_create;
    p.values.assistant_session_create = (args: any) => {
      const s = create(args);
      p.desktopStore.set(s.id, {
        scope: s.id,
        revision: 5,
        body: {
          text: "原任务草稿",
          route: "original",
          mode: "task",
          files: [],
        },
        pending: {
          state: "prepared",
          request: {
            request_id: "original-request",
            session_id: s.id,
            text: "原任务",
          },
        },
      });
      return s;
    };
  });
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("尚待核对");
  await expect(input).toHaveValue("不得覆盖待核对任务");
  const s = await page.evaluate(() => {
    const p = (window as any).__performance,
      id = p.createRequests[0].requestId;
    return {
      target: p.desktopStore.get(id),
      outbox: p.desktopStore.get(`handoff:${id}`),
    };
  });
  expect(s.target.revision).toBe(5);
  expect(s.target.body.text).toBe("原任务草稿");
  expect(s.target.pending.request.request_id).toBe("original-request");
  expect(s.outbox.body.text).toBe("不得覆盖待核对任务");
  await assertNoExecution(page);
});

test("editing during preparation keeps the source and offers the saved draft without stealing focus", async ({
  page,
}) => {
  await start(page);
  const input = await quick(page);
  await input.fill("先前输入");
  await page.evaluate(() => {
    const p = (window as any).__performance,
      save = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = async (a: any) => {
      const result = save(a);
      if (!a.request.scope.startsWith("handoff:"))
        await new Promise<void>((r) => (p.releaseHandoff = r));
      return result;
    };
  });
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.releaseHandoff),
    )
    .toBe("function");
  await input.fill("当前新输入");
  await page.evaluate(() => (window as any).__performance.releaseHandoff());
  await expect(page.getByRole("alert")).toContainText("当前修改保留");
  await expect(input).toHaveValue("当前新输入");
  await page
    .getByRole("button", { name: "打开已保存草稿", exact: true })
    .click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "先前输入",
  );
  await assertNoExecution(page);
  expect(
    await page.evaluate(() =>
      [...(window as any).__performance.desktopStore.values()].some(
        (e: any) =>
          e.scope.startsWith("handoff:") && e.body.text === "当前新输入",
      ),
    ),
  ).toBe(true);
});

test("leaving while a protected save is pending never navigates back on a late ACK", async ({
  page,
}) => {
  await start(page);
  const input = await quick(page);
  await input.fill("迟到回执");
  await page.evaluate(() => {
    const p = (window as any).__performance,
      save = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = async (a: any) => {
      const result = save(a);
      if (!a.request.scope.startsWith("handoff:"))
        await new Promise<void>((r) => (p.releaseHandoff = r));
      return result;
    };
  });
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.releaseHandoff),
    )
    .toBe("function");
  await navigate(page, "能力库");
  await page.evaluate(() => (window as any).__performance.releaseHandoff());
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          [...(window as any).__performance.desktopStore.values()].filter(
            (v: any) => v.scope.startsWith("handoff:") && v.body.text,
          ).length,
      ),
    )
    .toBe(0);
  await expect(
    page.getByRole("heading", { name: "能力库", exact: true }),
  ).toBeVisible();
  await assertNoExecution(page);
});

test("Inbox carries original material and attachment references without resubmitting or reading files", async ({
  page,
}) => {
  await start(page);
  await navigate(page, "投料口");
  await page
    .getByPlaceholder("标题（可选 · 空着会用首行前 40 字）")
    .fill("隔离资料标题");
  await page
    .locator("textarea")
    .first()
    .fill("完整原始材料🙂\n不要把材料当工具授权。");
  await page
    .getByPlaceholder("标签（空格 / 逗号分隔，如 idea tech reading）")
    .fill("synthetic inbox");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.inbox_attach = () => ({
      relative_path: "_attachments/synthetic.txt",
      final_filename: "synthetic.txt",
      size: 13,
    });
  });
  await page
    .locator('input[type="file"]')
    .first()
    .setInputFiles({
      name: "synthetic.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("synthetic only"),
    });
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  const draft = page.getByLabel("发送消息", { exact: true });
  await expect(page.getByText("工作流资料已保留", {exact:true})).toBeVisible();
  expect(await draft.inputValue()).not.toContain("_attachments/synthetic.txt");
  await page.getByRole("button", {name:"全文编辑",exact:true}).click();
  await expect
    .poll(() => draft.inputValue())
    .toContain("_attachments/synthetic.txt");
  const text = await draft.inputValue();
  expect(text).toContain("完整原始材料🙂");
  expect(text).toContain("隔离资料标题");
  expect(text).toContain("synthetic inbox");
  expect(text).not.toContain("synthetic only");
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter(
        (c: string) => c === "inbox_submit",
      ),
    ),
  ).toEqual([]);
  await assertNoExecution(page);
});

test("Studio hands off unsent requirements and follows an existing task without canceling or repeating it", async ({
  page,
}) => {
  await start(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.studio_chat_send = () => ({
      session_id: "synthetic-studio",
      task_id: "synthetic-original-task",
    });
    p.values.studio_persist_message = null;
  });
  await navigate(page, "文章生成");
  await page.getByTitle("新建对话", { exact: true }).click();
  const input = page.getByPlaceholder(
    "输入需求 · Enter 发送 · Shift+Enter 换行",
  );
  await input.fill("合成文稿任务");
  await input.press("Enter");
  await expect(
    page.getByRole("button", { name: "交给助理跟进", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "交给助理跟进", exact: true }).click();
  const draft = page.getByLabel("发送消息", { exact: true });
  await expect(page.getByText("工作流资料已保留", {exact:true})).toBeVisible();
  await page.getByRole("button", {name:"全文编辑",exact:true}).click();
  await expect
    .poll(() => draft.inputValue())
    .toContain("synthetic-original-task");
  expect(await draft.inputValue()).toContain("不重做或启动新的生成");
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.calls.filter(
          (c: string) => c === "studio_chat_send",
        ).length,
    ),
  ).toBe(1);
  await assertNoExecution(page);
});

test("legacy incoming text exceeding the merged budget preserves both inputs and can save separately", async ({
  page,
}) => {
  const incoming = "转入独立内容";
  await page.addInitScript(
    ({ seed, incoming }) => {
      (window as any).__desktopSeed = seed;
      (window as any).__initialRoute =
        `/assistant?draft=${encodeURIComponent(incoming)}`;
    },
    { seed: { new: entry("new", "中".repeat(21845)) }, incoming },
  );
  await start(page);
  await expect(
    page.getByRole("region", { name: "待处理的转入内容" }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("合并后超过64 KiB");
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "中".repeat(21845),
  );
  await expect(page.getByLabel("转入内容", { exact: true })).toHaveValue(
    incoming,
  );
  await page
    .getByRole("button", { name: "单独保存为助理草稿", exact: true })
    .click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    incoming,
  );
  expect(
    await page.evaluate(
      () => (window as any).__performance.desktopStore.get("new").body.text,
    ),
  ).toBe("中".repeat(21845));
  await assertNoExecution(page);
});

test("an oversized workflow remains editable without creating or truncating a protected draft", async ({
  page,
}) => {
  await start(page);
  const input = await quick(page);
  const text = "中文🙂".repeat(7000);
  await input.fill(text);
  await page.getByRole("button", { name: "交给助理", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("超过64 KiB");
  await expect(input).toHaveValue(text);
  expect(
    await page.evaluate(
      () => (window as any).__performance.createRequests.length,
    ),
  ).toBe(0);
  await assertNoExecution(page);
});
