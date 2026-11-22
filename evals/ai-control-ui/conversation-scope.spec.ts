import { test, expect, type Page } from "@playwright/test";

async function openCenter(page: Page) {
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("heading", { name: "AI 助理", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.scopeRequests = [];
    p.scopeRows = (provider: string) => [
      {
        agent_id: provider,
        session_id: `saved-${provider}`,
        first_user_prompt: `外部历史 ${provider}`,
        last_active_at: "2026-11-18T00:00:00Z",
        message_count: 1,
      },
    ];
    p.values.agents_history_all = () => {
      p.scopeRequests.push("history:all");
      return {
        entries: ["codex", "claude-code", "gemini-cli"].flatMap(p.scopeRows),
        buckets: [],
      };
    };
    p.values.agents_history_list = ({ id }: any) => {
      p.scopeRequests.push(`history:${id}`);
      return {
        agent_id: id,
        supported: true,
        entries: p.scopeRows(id),
        unsupported_reason: null,
        data_root: "synthetic",
      };
    };
    p.values.conversation_native = ({ request }: any) => {
      p.nativeRequests.push(request);
      if (request.action === "list") {
        p.scopeRequests.push(`managed:${request.provider}`);
        if (p.scopeFailure === request.provider)
          throw Error(`合成来源读取失败 ${request.provider}`);
        if (p.scopeMalformed === request.provider) return { state: "unknown" };
        return {
          data: [
            {
              id: `managed-${request.provider}`,
              preview: `托管会话 ${request.provider}`,
              updatedAt: "2026-11-18T01:00:00Z",
            },
          ],
        };
      }
      if (request.action === "read")
        return { thread: { id: request.thread_id, turns: [] } };
      if (request.action === "status")
        return {
          id: request.thread_id,
          state: "completed",
          items: {},
          item_order: [],
        };
      throw Error("No native mutation is allowed");
    };
  });
}

const requests = (page: Page) =>
  page.evaluate(() => [...(window as any).__performance.scopeRequests]);

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
  await openCenter(page);
});

test("HomeBase and WeChat list refresh never read unrelated agent directories or provider status", async ({
  page,
}) => {
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /托管会话 gemini-cli/ }),
  ).toBeVisible();
  for (const source of ["homebase", "wechat"]) {
    await page.getByLabel("按来源筛选会话").selectOption(source);
    const before = await requests(page);
    const statusReads = await page.evaluate(
      () =>
        (window as any).__performance.calls.filter(
          (c: string) => c === "conversation_providers",
        ).length,
    );
    await page.getByLabel("刷新会话列表").click();
    await expect(
      page.getByText("HomeBase 已读取", { exact: false }),
    ).toBeVisible();
    await page.waitForTimeout(100);
    expect(await requests(page)).toEqual(before);
    expect(
      await page.evaluate(
        () =>
          (window as any).__performance.calls.filter(
            (c: string) => c === "conversation_providers",
          ).length,
      ),
    ).toBe(statusReads);
    await expect(page.getByText("正在读取会话…", { exact: true })).toHaveCount(
      0,
    );
  }
});

test("single native filter uses its scoped history and refreshes only the same provider", async ({
  page,
}) => {
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /托管会话 gemini-cli/ }),
  ).toBeVisible();
  await page.getByLabel("按来源筛选会话").selectOption("claude-code");
  await expect
    .poll(
      async () =>
        (await requests(page)).filter(
          (s: string) => s === "history:claude-code",
        ).length,
    )
    .toBe(1);
  const before = await requests(page);
  await page.getByLabel("刷新会话列表").click();
  await expect
    .poll(
      async () =>
        (await requests(page)).filter(
          (s: string) => s === "history:claude-code",
        ).length,
    )
    .toBe(2);
  expect((await requests(page)).slice(before.length).sort()).toEqual([
    "history:claude-code",
    "managed:claude-code",
  ]);
  await expect(
    page.getByRole("button", { name: /外部历史 claude-code/ }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /外部历史 gemini-cli/ }),
  ).toHaveCount(0);
});

test("one managed source failure preserves other source results and disappears when hidden", async ({
  page,
}) => {
  await page.evaluate(
    () => ((window as any).__performance.scopeFailure = "claude-code"),
  );
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /托管会话 gemini-cli/ }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toContainText(
    "合成来源读取失败 claude-code",
  );
  await expect(
    page.getByText("还没有符合条件的会话。", { exact: true }),
  ).toHaveCount(0);
  await page.getByLabel("按来源筛选会话").selectOption("homebase");
  await expect(page.getByRole("alert")).toHaveCount(0);
  const before = await requests(page);
  await page.getByLabel("刷新会话列表").click();
  expect(await requests(page)).toEqual(before);
});

test("malformed managed source is explicit instead of becoming an empty directory", async ({
  page,
}) => {
  await page.evaluate(
    () => ((window as any).__performance.scopeMalformed = "claude-code"),
  );
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByLabel("按来源筛选会话").selectOption("claude-code");
  await expect(page.getByRole("alert")).toContainText("原生会话目录返回无效");
  await expect(
    page.getByText("还没有符合条件的会话。", { exact: true }),
  ).toHaveCount(0);
});

test("repeated refresh shares both in-flight source reads without cancelling them", async ({
  page,
}) => {
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByLabel("按来源筛选会话").selectOption("claude-code");
  await expect(
    page.getByRole("button", { name: /外部历史 claude-code/ }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.scopeReleases = [];
    for (const command of ["agents_history_list", "conversation_native"]) {
      const previous = p.values[command];
      p.values[command] = (args: any) => {
        const result = previous(args);
        if (command === "conversation_native" && args.request.action !== "list")
          return result;
        return new Promise((resolve) =>
          p.scopeReleases.push(() => resolve(result)),
        );
      };
    }
  });
  const before = await requests(page);
  await page
    .getByLabel("刷新会话列表")
    .evaluate((button: HTMLButtonElement) => {
      button.click();
      button.click();
    });
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).__performance.scopeReleases.length),
    )
    .toBe(2);
  expect((await requests(page)).slice(before.length).sort()).toEqual([
    "history:claude-code",
    "managed:claude-code",
  ]);
  await page.evaluate(() =>
    (window as any).__performance.scopeReleases.forEach((release: any) =>
      release(),
    ),
  );
  await expect(
    page.getByRole("button", { name: /外部历史 claude-code/ }),
  ).toBeVisible();
});

test("late scoped history stays in its own cache and never replaces the newly selected source", async ({
  page,
}) => {
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.evaluate(() => {
    const p = (window as any).__performance,
      previous = p.values.agents_history_list;
    p.values.agents_history_list = (args: any) => {
      const result = previous(args);
      return args.id === "claude-code"
        ? new Promise((resolve) => (p.releaseScope = () => resolve(result)))
        : result;
    };
  });
  await page.getByLabel("按来源筛选会话").selectOption("claude-code");
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.releaseScope),
    )
    .toBe("function");
  await page.getByLabel("按来源筛选会话").selectOption("gemini-cli");
  await expect(
    page.getByRole("button", { name: /外部历史 gemini-cli/ }),
  ).toBeVisible();
  await page.evaluate(() => (window as any).__performance.releaseScope());
  await page.waitForTimeout(100);
  await expect(
    page.getByRole("button", { name: /外部历史 claude-code/ }),
  ).toHaveCount(0);
  await expect(page.getByLabel("按来源筛选会话")).toHaveValue("gemini-cli");
});

test("unsupported and partially unavailable sources are visible and never claim complete emptiness", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.agents_history_all = {
      entries: [],
      buckets: [
        {
          agent_id: "codex",
          supported: false,
          unsupported_reason: "合成目录读取失败",
        },
      ],
    };
    p.values.agents_history_list = ({ id }: any) => ({
      agent_id: id,
      supported: false,
      unsupported_reason: "合成来源尚未接入",
      entries: [],
      data_root: "synthetic",
    });
    p.values.conversation_native = ({ request }: any) => {
      p.nativeRequests.push(request);
      return { data: [] };
    };
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(
    page.getByText(/部分来源未完整读取：.*合成目录读取失败/),
  ).toBeVisible();
  await expect(
    page.getByText("还没有符合条件的会话。", { exact: true }),
  ).toHaveCount(0);
  await page.getByLabel("按来源筛选会话").selectOption("codex");
  await expect(
    page.getByText(/部分来源未完整读取：.*合成来源尚未接入/),
  ).toBeVisible();
  await expect(
    page.getByText("还没有符合条件的会话。", { exact: true }),
  ).toHaveCount(0);
});

test("refreshing the selected native conversation preserves its draft and never refreshes a different visible provider", async ({
  page,
}) => {
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByRole("button", { name: /托管会话 claude-code/ }).click();
  await page.getByLabel("给 Claude Code 发消息").fill("未发送的原生需求🙂");
  await expect(page.getByLabel("原生草稿保存状态")).toContainText(
    "已受保护保存",
  );
  await page.getByLabel("按来源筛选会话").selectOption("gemini-cli");
  await expect(
    page.getByRole("button", { name: /外部历史 gemini-cli/ }),
  ).toBeVisible();
  const before = await requests(page);
  await page.getByLabel("刷新原生会话").click();
  await expect(page.getByLabel("刷新原生会话")).toBeEnabled();
  expect(await requests(page)).toEqual(before);
  await expect(page.getByLabel("给 Claude Code 发消息")).toHaveValue(
    "未发送的原生需求🙂",
  );
  expect(
    await page.evaluate(() =>
      (window as any).__performance.nativeRequests.every((r: any) =>
        ["list", "read", "status"].includes(r.action),
      ),
    ),
  ).toBe(true);
});

async function installEditableHistoryFixture(page: Page) {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const originalRows = p.scopeRows;
    p.scopeRows = (provider: string) =>
      originalRows(provider).map((row: object) => ({ ...row, session_id: "shared-id" }));
    p.bodyReads = [];
    p.historyText = { codex: "原 Codex 消息", "claude-code": "原 Claude 消息" };
    p.values.agents_history_read = ({ id, sessionId }: { id: string; sessionId: string }) => {
      p.bodyReads.push([id, sessionId]);
      if (p.historyFailure === id) throw Error("合成读取失败");
      const text = p.historyText[id];
      const result = {
        agent_id: p.historyWrongIdentity === id ? "gemini-cli" : id,
        session_id: sessionId,
        messages: p.historyMalformed === id
          ? [{}]
          : text == null ? [] : [{ uuid: "synthetic-message", role: "user", content: text }],
      };
      if (p.holdHistoryRead === id) {
        p.holdHistoryRead = null;
        return new Promise((resolve) => { p.releaseHistoryRead = () => resolve(result); });
      }
      return result;
    };
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
}

const bodyReads = (page: Page) =>
  page.evaluate(() => [...(window as any).__performance.bodyReads]);

test("read-only refresh reuses the in-flight read and late results stay with the original provider", async ({ page }) => {
  await installEditableHistoryFixture(page);
  const history = page.getByRole("region", { name: "已保存的原生对话" });
  const refresh = page.getByRole("button", { name: "刷新原对话" });
  const codex = page.getByRole("button", { name: /外部历史 codex/ });
  const claude = page.getByRole("button", { name: /外部历史 claude-code/ });
  await codex.click();
  await expect(history.getByText("原 Codex 消息")).toBeVisible();
  const before = await requests(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.historyText.codex = "刷新后的 Codex 消息";
    p.holdHistoryRead = "codex";
  });
  await refresh.click();
  await expect(refresh).toBeDisabled();
  await expect(history.getByRole("status")).toHaveText("正在刷新原对话…");
  await expect(history.getByText("原 Codex 消息")).toBeVisible();
  await refresh.evaluate((element: HTMLButtonElement) => element.click());
  expect(await bodyReads(page)).toEqual([["codex", "shared-id"], ["codex", "shared-id"]]);

  await claude.click();
  await expect(history.getByText("原 Claude 消息")).toBeVisible();
  await page.evaluate(() => (window as any).__performance.releaseHistoryRead());
  await expect.poll(() => page.evaluate(() =>
    (window as any).__performance.client.getQueryData([
      "agents", "history", "read", "codex", "shared-id",
    ])?.messages[0].content,
  )).toBe("刷新后的 Codex 消息");
  await expect(history.getByText("刷新后的 Codex 消息")).toHaveCount(0);
  await codex.click();
  await expect(history.getByText("刷新后的 Codex 消息")).toBeVisible();
  expect(await bodyReads(page)).toEqual([
    ["codex", "shared-id"], ["codex", "shared-id"], ["claude-code", "shared-id"],
  ]);
  expect(await requests(page)).toEqual(before);
});

test("read-only refresh retains verified text and waits for valid current excerpts before handoff", async ({ page }) => {
  await installEditableHistoryFixture(page);
  const history = page.getByRole("region", { name: "已保存的原生对话" });
  const refresh = page.getByRole("button", { name: "刷新原对话" });
  const prepare = page.getByRole("button", { name: "准备交接草稿" });
  await page.getByRole("button", { name: /外部历史 codex/ }).click();
  await expect(history.getByText("原 Codex 消息")).toBeVisible();
  await page.getByRole("button", { name: "交接给 HomeBase" }).click();
  await page.getByRole("textbox", { name: "交接目标" }).fill("合成交接要求");
  await expect(prepare).toBeEnabled();

  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.historyText.codex = "错误来源的消息";
    p.holdHistoryRead = "codex";
    p.historyWrongIdentity = "codex";
  });
  await refresh.click();
  await expect(prepare).toBeDisabled();
  await page.evaluate(() => (window as any).__performance.releaseHistoryRead());
  await expect(history.getByRole("alert")).toContainText("标识不匹配");
  await expect(history.getByRole("alert")).toContainText("上次读取的内容仍可查看");
  await expect(history.getByText("原 Codex 消息")).toBeVisible();
  await expect(history.getByText("错误来源的消息")).toHaveCount(0);
  await expect(prepare).toBeDisabled();

  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.historyWrongIdentity = null;
    p.historyMalformed = "codex";
  });
  await refresh.click();
  await expect(history.getByRole("alert")).toContainText("消息格式无效");
  await expect(prepare).toBeDisabled();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.historyMalformed = null;
    p.historyText.codex = "核对后的 Codex 消息";
  });
  await refresh.click();
  await expect(history.getByText("核对后的 Codex 消息")).toBeVisible();
  await expect(history.getByRole("alert")).toHaveCount(0);
  await expect(prepare).toBeEnabled();
  await expect(page.getByRole("textbox", { name: "交接目标" })).toHaveValue("合成交接要求");
  expect(await page.evaluate(() => (window as any).__performance.createRequests.length)).toBe(0);
});

test("interactive handoff rejects a mismatched native thread and can reread the original", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance, read = p.values.conversation_native;
    p.wrongHandoffSource = true;
    const scope = "native:gemini-cli:managed-gemini-cli";
    p.desktopStore.set(scope, { scope, revision: 3, pending: null, body: {
      text: "Gemini 未发草稿", route: "", mode: "auto", files: [],
    } });
    p.values.conversation_native = (args: any) => {
      const { request } = args;
      if (request.action !== "read" || request.provider !== "gemini-cli") return read(args);
      const mismatch = p.wrongHandoffSource;
      return { thread: { id: request.thread_id, provider: mismatch ? "claude-code" : "gemini-cli", turns: [{
        id: "source-turn", items: [{ id: "source-body", type: "userMessage", text: mismatch ? "错误提供方的消息" : "已核对的原始摘录" }],
      }] } };
    };
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByRole("button", { name: /托管会话 gemini-cli/ }).click();
  await page.getByRole("button", { name: "交接给 HomeBase" }).click();
  const transfer = page.getByRole("region", { name: "交接会话" });
  await transfer.getByRole("textbox", { name: "交接目标" }).fill("核对原始请求");
  await expect(transfer.getByRole("alert")).toContainText("标识不匹配");
  await expect(transfer.getByRole("button", { name: "准备交接草稿" })).toBeDisabled();
  await expect(page.getByText("错误提供方的消息", { exact: true })).toHaveCount(0);
  await page.evaluate(() => { (window as any).__performance.wrongHandoffSource = false; });
  await page.getByRole("button", { name: "刷新原生会话" }).click();
  await expect(transfer.getByRole("alert")).toHaveCount(0);
  await expect(transfer.getByRole("button", { name: "准备交接草稿" })).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__performance.createRequests.length)).toBe(0);
  await transfer.getByRole("checkbox", { name: "携带已保存的未发草稿和普通文本附件" }).check();
  await expect(transfer.getByText("已读取第 3 版草稿。", { exact: true })).toBeVisible();
  await transfer.getByRole("button", { name: "准备交接草稿" }).click();
  const composer = page.getByRole("textbox", { name: "发送消息" });
  await expect(composer).toHaveValue(/已核对的原始摘录/);
  await expect(composer).toHaveValue(/Gemini 未发草稿/);
  const source = await page.evaluate(() => (window as any).__performance.desktopStore.get("native:gemini-cli:managed-gemini-cli"));
  expect(source.revision).toBe(3);
  expect(source.body.text).toBe("Gemini 未发草稿");
});

test("explicit handoff copies the saved text and plain attachment while leaving the PDF and source intact", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list = [{ id: "draft-source", title: "原桌面测试会话", source: "desktop", message_count: 1 }];
    p.values.assistant_session_get = ({ sessionId }: any) => ({ id: sessionId, source: "desktop", messages: [], turns: [] });
    p.desktopStore.set("draft-source", { scope: "draft-source", revision: 5, pending: null, body: {
      text: "已保存但尚未发送的原始草稿🙂", route: "synthetic-original-route", mode: "task", files: [
        { name: "记录.txt", content: "可以携带的普通文件内容" },
        { name: "原会话.pdf", content: "不从原范围复制的 PDF 摘录", document: { document_id: "owned-pdf" } },
      ],
    } });
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByLabel("刷新会话列表").click();
  await page.getByRole("button", { name: /原桌面测试会话/ }).click();
  await page.getByRole("button", { name: "交接给 HomeBase" }).click();
  const transfer = page.getByRole("region", { name: "交接会话" });
  await transfer.getByRole("checkbox", { name: /携带最近对话摘录/ }).uncheck();
  await transfer.getByRole("textbox", { name: "交接目标" }).fill("整理已保存的内容");
  const include = transfer.getByRole("checkbox", { name: "携带已保存的未发草稿和普通文本附件" });
  await expect(include).not.toBeChecked();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const get = p.values.assistant_desktop_get;
    p.handoffCase = "pending";
    p.values.assistant_desktop_get = (args: any) => {
      const reply = get(args);
      if (args.scope !== "draft-source") return reply;
      if (p.handoffCase === "pending") reply.entry.pending = { state: "prepared" };
      if (p.handoffCase === "oversized") reply.entry.body.files[0].content = "复".repeat(4000);
      return reply;
    };
  });
  await include.check();
  const prepare = transfer.getByRole("button", { name: "准备交接草稿" });
  const reread = transfer.getByRole("button", { name: "重读已保存草稿" });
  await expect(transfer.getByRole("alert")).toContainText("发送回执尚未核对");
  await expect(prepare).toBeDisabled();
  await page.evaluate(() => { (window as any).__performance.handoffCase = "oversized"; });
  await reread.click();
  await expect(prepare).toBeEnabled();
  await prepare.click();
  await expect(transfer.getByRole("alert")).toContainText("超过8000字节");
  expect(await page.evaluate(() => (window as any).__performance.createRequests.length)).toBe(0);
  await page.evaluate(() => { (window as any).__performance.handoffCase = "saved"; });
  await include.uncheck();
  await include.check();
  await expect(transfer.getByText("第 5 版", { exact: false })).toBeVisible();
  await expect(transfer.getByText("1 个 PDF 引用留在原会话", { exact: false })).toBeVisible();
  await prepare.click();
  await expect(page.getByRole("textbox", { name: "发送消息" })).toHaveValue(/可以携带的普通文件内容/);
  await expect.poll(() => page.evaluate(() => {
    const p = (window as any).__performance;
    const id = p.createRequests.at(-1)?.requestId;
    return id ? p.desktopStore.get(id)?.body.text : undefined;
  })).toContain("可以携带的普通文件内容");
  const result = await page.evaluate(() => {
    const p = (window as any).__performance;
    const id = p.createRequests.at(-1)?.requestId;
    return { source: p.desktopStore.get("draft-source"), target: p.desktopStore.get(id), calls: p.calls, creations: p.createRequests.length };
  });
  expect(result.creations).toBe(1);
  expect(result.source.body.text).toBe("已保存但尚未发送的原始草稿🙂");
  expect(result.source.body.files).toHaveLength(2);
  expect(result.source.revision).toBe(5);
  expect(result.target.body.route).toBe("");
  expect(result.target.body.text).toContain("已保存但尚未发送的原始草稿🙂");
  expect(result.target.body.text).not.toContain("不从原范围复制的 PDF 摘录");
  expect(result.target.body.text).not.toContain("synthetic-original-route");
  expect(result.calls).not.toContain("assistant_send");
  expect(result.calls).not.toContain("assistant_desktop_send");
});

test("read-only history distinguishes a first read failure from an empty result", async ({ page }) => {
  await installEditableHistoryFixture(page);
  const history = page.getByRole("region", { name: "已保存的原生对话" });
  const refresh = page.getByRole("button", { name: "刷新原对话" });
  await page.evaluate(() => { (window as any).__performance.historyFailure = "codex"; });
  await page.getByRole("button", { name: /外部历史 codex/ }).click();
  await expect(history.getByRole("alert")).toContainText("合成读取失败");
  await expect(history.getByText("这段对话没有可显示的消息。")).toHaveCount(0);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.historyFailure = null;
    p.historyText.codex = null;
  });
  await refresh.click();
  await expect(history.getByText("这段对话没有可显示的消息。")).toBeVisible();
  await expect(history.getByRole("alert")).toHaveCount(0);
  expect(await bodyReads(page)).toEqual([["codex", "shared-id"], ["codex", "shared-id"]]);
});

for (const freshHistory of [true, false]) {
  test(`native completion refreshes only its directory even when persisted history is ${freshHistory ? "already fresh" : "still stale"}`, async ({
    page,
  }) => {
    await page.evaluate((fresh) => {
      const p = (window as any).__performance,
        previous = p.values.conversation_native;
      p.scopeNativeFinished = false;
      p.values.conversation_native = (args: any) => {
        const r = args.request;
        if (
          r.provider !== "claude-code" ||
          !["status", "read"].includes(r.action)
        )
          return previous(args);
        p.nativeRequests.push(r);
        const item = {
          id: "scope-native-final",
          type: "agentMessage",
          text: "合成任务已完成",
        };
        if (r.action === "read")
          return {
            thread: {
              id: r.thread_id,
              turns: fresh ? [{ id: "original", items: [item] }] : [],
            },
          };
        return p.scopeNativeFinished
          ? {
              id: r.thread_id,
              provider: r.provider,
              state: "completed",
              items: { [item.id]: item },
              item_order: [item.id],
            }
          : { id: r.thread_id, provider: r.provider, state: "inProgress", items: {}, item_order: [] };
      };
    }, freshHistory);
    await page.getByRole("link", { name: "会话", exact: true }).click();
    await page.getByRole("button", { name: /托管会话 claude-code/ }).click();
    await expect
      .poll(() =>
        page.evaluate(() =>
          (window as any).__performance.nativeRequests.some(
            (r: any) => r.provider === "claude-code" && r.action === "status",
          ),
        ),
      )
      .toBe(true);
    const before = await requests(page);
    await page.evaluate(
      () => ((window as any).__performance.scopeNativeFinished = true),
    );
    await expect(
      page.getByText("合成任务已完成", { exact: true }),
    ).toBeVisible();
    await expect
      .poll(
        async () =>
          (await requests(page)).filter(
            (r: string) => r === "managed:claude-code",
          ).length,
      )
      .toBe(
        before.filter((r: string) => r === "managed:claude-code").length + 1,
      );
    expect((await requests(page)).slice(before.length).sort()).toEqual([
      "history:all",
      "managed:claude-code",
    ]);
    await page.waitForTimeout(350);
    expect((await requests(page)).slice(before.length).sort()).toEqual([
      "history:all",
      "managed:claude-code",
    ]);
    expect(
      await page.evaluate(() =>
        (window as any).__performance.nativeRequests.every((r: any) =>
          ["list", "read", "status"].includes(r.action),
        ),
      ),
    ).toBe(true);
  });
}

test("failed source retries only the current source and restores its real rows", async ({
  page,
}) => {
  await page.evaluate(
    () => ((window as any).__performance.scopeFailure = "claude-code"),
  );
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByLabel("按来源筛选会话").selectOption("claude-code");
  await expect(page.getByRole("alert")).toContainText(
    "合成来源读取失败 claude-code",
  );
  const before = await requests(page);
  await page.evaluate(
    () => ((window as any).__performance.scopeFailure = null),
  );
  await page.getByLabel("刷新会话列表").click();
  await expect(
    page.getByRole("button", { name: /托管会话 claude-code/ }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect((await requests(page)).slice(before.length).sort()).toEqual([
    "history:claude-code",
    "managed:claude-code",
  ]);
});

test("same ID across agents stays separate while duplicate same-agent rows keep original priority", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.scopeRows = (provider: string) => [
      {
        agent_id: provider,
        session_id: "shared",
        first_user_prompt: `优先历史 ${provider}`,
        last_active_at: "2026-11-18T00:00:00Z",
      },
    ];
    const previous = p.values.conversation_native;
    p.values.conversation_native = (args: any) =>
      args.request.action === "list"
        ? {
            data: [
              {
                id: "shared",
                preview: "重复托管行",
                updatedAt: "2026-11-18T01:00:00Z",
              },
            ],
          }
        : previous(args);
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(page.getByRole("button", { name: /优先历史/ })).toHaveCount(3);
  await expect(page.getByRole("button", { name: /重复托管行/ })).toHaveCount(0);
  await page.getByRole("button", { name: /优先历史 claude-code/ }).click();
  await expect(page.getByLabel("给 Claude Code 发消息")).toBeEnabled();
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.client.getQueryData([
          "conversations",
          "selection",
        ]).selected,
    ),
  ).toMatchObject({ provider: "claude-code", id: "shared" });
});

test("opening an already completed conversation never refreshes directory or sends a message", async ({
  page,
}) => {
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /托管会话 claude-code/ }),
  ).toBeVisible();
  const before = await requests(page);
  await page.getByRole("button", { name: /托管会话 claude-code/ }).click();
  await expect(page.getByLabel("给 Claude Code 发消息")).toBeEnabled();
  await page.waitForTimeout(200);
  expect(await requests(page)).toEqual(before);
  expect(
    await page.evaluate(() =>
      (window as any).__performance.nativeRequests.every((r: any) =>
        ["list", "read", "status"].includes(r.action),
      ),
    ),
  ).toBe(true);
});

test("Agent status has an explicit retry independent of list refresh", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.scopeProviderReply = p.values.conversation_providers;
    p.values.conversation_providers = () => {
      throw Error("synthetic provider metadata unavailable");
    };
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "重试 Agent 状态" }),
  ).toBeVisible();
  await page.getByLabel("按来源筛选会话").selectOption("homebase");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.conversation_providers = p.scopeProviderReply;
  });
  const before = await requests(page);
  await page.getByRole("button", { name: "重试 Agent 状态" }).click();
  await expect(
    page.getByRole("button", { name: "重试 Agent 状态" }),
  ).toHaveCount(0);
  expect(await requests(page)).toEqual(before);
});
