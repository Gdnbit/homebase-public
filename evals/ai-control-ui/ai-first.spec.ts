import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("heading", { name: "AI 助理", exact: true }),
  ).toBeVisible();
});

test("assistant is home; context inspection and page changes never resume or send", async ({
  page,
}) => {
  await expect(
    page.getByRole("link", { name: "AI 接管", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("link", { name: "文章生成", exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "上下文自动组织" }).click();
  await expect(
    page.getByRole("heading", { name: "这次对话可以使用什么" }),
  ).toBeVisible();
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(page.getByText("选一件聊过的事，或开始新对话。")).toBeVisible();
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(
    calls.filter((c: string) => c === "assistant_context_get"),
  ).toHaveLength(1);
  expect(
    calls.filter((c: string) => c === "assistant_config_get"),
  ).toHaveLength(1);
  expect(calls).not.toContain("assistant_send");
  expect(calls).not.toContain("assistant_resume");
  expect(await page.evaluate(()=>(window as any).__performance.nativeRequests.every((r:any)=>['list','read','status'].includes(r.action)))).toBe(true);
});

test("functional capability drawer prepares a draft without executing or exposing a key", async ({
  page,
}) => {
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("button", { name: /视频.*个实现/ }).click();
  await expect(
    page.getByRole("dialog").getByText("本机算力成本"),
  ).toBeVisible();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "使用这个实现准备任务", exact: true })
    .click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("");
  await expect(page.getByText("能力资料已保留",{exact:true})).toBeVisible();
  const full=await page.evaluate(()=>{const p=(window as any).__performance;return p.desktopStore.get(p.createRequests.at(-1).requestId).body.text;});
  expect(full).toContain("代码动画");
  expect(full).toContain("homebase-capability-selection-v1");
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_send");
  expect(calls).not.toContain("capability_render");
  expect(calls).not.toContain("model_capability_call");
});

test("cross-agent handoff rejects a mismatched acknowledgement and retries the original identity without changing source", async ({
  page,
}) => {
  await page.getByLabel("发送消息", { exact: true }).fill("原本准备中的合成输入");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.agents_history_all = {
      entries: [
        {
          agent_id: "claude-code",
          session_id: "original",
          first_user_prompt: "整理动画脚本",
          last_active_at: "2026-11-14T00:00:00Z",
        },
      ],
      buckets: [],
    };
    p.values.agents_history_read = {
      agent_id: "claude-code",
      session_id: "original",
      cwd: "X:/isolated/project",
      messages: [
        { uuid: "one", role: "user", content: "蓝色圆点" },
        { uuid: "two", role: "assistant", content: "脚本已准备，尚未渲染" },
      ],
    };
    const create = p.values.assistant_session_create;
    p.unrelatedHandoffId = crypto.randomUUID();
    p.desktopStore.set(p.unrelatedHandoffId, {
      scope: p.unrelatedHandoffId, revision: 7,
      body: {text: "其他会话不可覆盖", route: "", mode: "auto", files: []},
      pending: null, voices: [],
    });
    let wrongAcknowledgement = true;
    p.values.assistant_session_create = (request: any) => {
      const session = create(request);
      if (wrongAcknowledgement) {
        wrongAcknowledgement = false;
        return {...session, id: p.unrelatedHandoffId};
      }
      return session;
    };
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByRole("button", { name: /整理动画脚本/ }).click();
  await expect(
    page.getByText("脚本已准备，尚未渲染", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "交接给 HomeBase" }).click();
  await page
    .getByLabel("交接目标", { exact: true })
    .fill("接着渲染，但不要重写脚本");
  await page.getByRole("button", { name: "准备交接草稿" }).click();
  await expect(page.getByRole("alert")).toContainText("标识或来源未确认");
  const guarded = await page.evaluate(() => {
    const p = (window as any).__performance, id = p.createRequests[0].requestId;
    return {
      target: p.desktopStore.get(id),
      outbox: p.desktopStore.get(`handoff:${id}`).body.text,
      unrelated: p.desktopStore.get(p.unrelatedHandoffId),
    };
  });
  expect(guarded.target).toBeUndefined();
  expect(guarded.outbox).toContain("尚未渲染");
  expect(guarded.unrelated.revision).toBe(7);
  expect(guarded.unrelated.body.text).toBe("其他会话不可覆盖");
  await expect(page.getByLabel("交接目标", { exact: true })).toHaveValue("接着渲染，但不要重写脚本");
  await page.getByRole("button", { name: "准备交接草稿" }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    /尚未渲染/,
  );
  const result = await page.evaluate(() => ({
    calls: (window as any).__performance.calls,
    original: (window as any).__performance.values.agents_history_read.messages
      .length,
    requests: (window as any).__performance.createRequests.map((r: any) => r.requestId),
    sessions: [...(window as any).__performance.createdSessions.keys()],
    originalDraft: (window as any).__performance.desktopStore.get("new").body.text,
    unrelated: (window as any).__performance.desktopStore.get((window as any).__performance.unrelatedHandoffId),
  }));
  expect(result.original).toBe(2);
  expect(result.requests).toHaveLength(2);
  expect(new Set(result.requests).size).toBe(1);
  expect(result.sessions).toEqual([result.requests[0]]);
  expect(result.originalDraft).toBe("原本准备中的合成输入");
  expect(result.unrelated.revision).toBe(7);
  expect(result.unrelated.body.text).toBe("其他会话不可覆盖");
  expect(result.calls).not.toContain("assistant_send");
  expect(await page.evaluate(()=>(window as any).__performance.nativeRequests.every((r:any)=>['list','read','status'].includes(r.action)))).toBe(true);
});

test("workbench distinguishes unknown usage and advanced integration stays in settings", async ({
  page,
}) => {
  if (!(await page.getByRole("link", { name: "工作台", exact: true }).isVisible())) await page.getByText("更多功能", {exact:true}).click();
  await page.getByRole("link", { name: "工作台", exact: true }).click();
  await expect(
    page
      .getByRole("region", { name: "助理任务与用量" })
      .getByText("未知", { exact: true }),
  ).toHaveCount(3);
  await page.getByRole("link", { name: "设置", exact: true }).click();
  await page.getByText("高级集成", { exact: true }).click();
  await expect(page.getByRole("button", { name: /AI 接管/ })).toBeVisible();
});

test("workbench refreshes usage once per event batch and coalesces an in-flight read", async ({ page }) => {
  const openWorkbench = async () => {
    const link = page.getByRole("link", { name: "工作台", exact: true });
    if (!(await link.isVisible())) await page.getByText("更多功能", { exact: true }).click();
    await link.click();
  };
  const usageReads = () => page.evaluate(() => (window as any).__performance.calls.filter((name: string) => name === "assistant_usage_get").length);
  const reportUsage = (tokens: number) => page.evaluate((promptTokens) => {
    const p = (window as any).__performance;
    p.currentUsage = { ...(p.currentUsage ?? p.values.assistant_usage_get), turns: 1, active: 1, samples: 1, prompt_tokens: promptTokens, reported_cost_usd: 0, complete: true };
    p.values.assistant_usage_get = () => p.currentUsage;
  }, tokens);
  const change = () => page.evaluate(() => {
    const p = (window as any).__performance;
    for (let index = 0; index < 3; index++) p.emit("assistant:changed", { session_id:"A", turn_id:"original", config:false });
  });
  await reportUsage(12);
  await openWorkbench();
  const usage = page.getByRole("region", { name: "助理任务与用量" });
  await expect(usage.getByText("12", { exact: true })).toBeVisible();
  const initial = await usageReads();
  await reportUsage(34);
  await change();
  await expect(usage.getByText("34", { exact: true })).toBeVisible({ timeout: 3500 });
  expect(await usageReads()).toBe(initial + 1);

  await page.getByRole("link", { name: "设置", exact: true }).click();
  await expect(usage).toHaveCount(0);
  await reportUsage(56);
  await change();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.client.getQueryState(["assistant", "usage"])?.isInvalidated)).toBe(true);
  expect(await usageReads()).toBe(initial + 1);
  await openWorkbench();
  await expect(usage.getByText("56", { exact: true })).toBeVisible();
  expect(await usageReads()).toBe(initial + 2);

  await page.evaluate(() => {
    const p = (window as any).__performance;
    const oldUsage = p.currentUsage;
    p.values.assistant_usage_get = () => new Promise((resolve) => { p.finishUsage = () => resolve(oldUsage); });
  });
  const refresh = usage.getByRole("button", { name: "刷新用量" });
  await refresh.click();
  await expect(refresh).toBeDisabled();
  expect(await usageReads()).toBe(initial + 3);
  await reportUsage(78);
  await change();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.client.getQueryState(["assistant", "usage"])?.isInvalidated)).toBe(true);
  expect(await usageReads()).toBe(initial + 3);
  await page.evaluate(() => (window as any).__performance.finishUsage());
  await expect(usage.getByText("78", { exact: true })).toBeVisible();
  expect(await usageReads()).toBe(initial + 4);

  await page.evaluate(() => { (window as any).__performance.values.assistant_usage_get = () => { throw Error("隔离用量读取失败"); }; });
  await refresh.click();
  await expect(usage.getByRole("alert")).toContainText("用量读取失败");
  await expect(usage.getByText("78", { exact: true })).toHaveCount(0);
  expect(await usageReads()).toBe(initial + 5);
  await reportUsage(90);
  await refresh.click();
  await expect(usage.getByText("90", { exact: true })).toBeVisible();
  await expect(usage.getByRole("alert")).toHaveCount(0);
  expect(await usageReads()).toBe(initial + 6);
});

test("workbench shows reported model totals and leaves missing figures unknown", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const known = { samples: 2, prompt_tokens: 21, completion_tokens: 5, reported_cost_usd: 0, cost_unknown_samples: 0, prompt_unknown_samples: 0, completion_unknown_samples: 1 };
    const unknown = { ...known, samples: 1, reported_cost_usd: null, cost_unknown_samples: 1 };
    p.values.assistant_usage_get = {
      ...p.values.assistant_usage_get, samples: 4, groups: [
        { kind: "model", model: "synthetic/flash", model_count: 1, totals: known },
        { kind: "unidentified", model: null, model_count: 0, totals: unknown },
        { kind: "other", model: null, model_count: 4, totals: { ...unknown, reported_cost_usd: 0.0000004, cost_unknown_samples: 0 } },
      ],
    };
  });
  const link = page.getByRole("link", { name: "工作台", exact: true });
  if (!(await link.isVisible())) await page.getByText("更多功能", { exact: true }).click();
  await link.click();
  const table = page.getByRole("table", { name: "按模型查看用量" });
  await expect(table.getByRole("row")).toHaveCount(4);
  const known = table.getByRole("row", { name: /synthetic\/flash/ });
  await expect(known.getByRole("cell").last()).toHaveText("0.000000");
  await expect(known.getByRole("cell").nth(2)).toContainText("1 条缺失");
  await expect(table.getByRole("row", { name: /模型未上报/ }).getByRole("cell").last()).toContainText("未知");
  await expect(table.getByRole("rowheader", { name: "其余 4 个模型" })).toBeVisible();
  await expect(table.getByRole("row", { name: /其余 4 个模型/ }).getByRole("cell").last()).toHaveText("4.00e-7");
  await page.setViewportSize({ width: 600, height: 740 });
  expect(await table.evaluate((element) => element.parentElement!.clientWidth < element.scrollWidth)).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const calls = await page.evaluate(() => (window as any).__performance.calls.filter((name: string) => name === "assistant_usage_get"));
  expect(calls).toHaveLength(1);
});

test("approved screens fit desktop widths and produce implementation screenshots", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 950 });
  for (const [label, file] of [
    ["AI 助理", "assistant"],
    ["会话", "conversations"],
    ["能力库", "capabilities"],
    ["工作台", "workbench"],
  ]) {
    await page.getByRole("link", { name: label, exact: true }).click();
    await expect(
      page.getByRole("heading", { name: label, exact: true }),
    ).toBeVisible();
    await page.evaluate(async () => {
      await document.fonts.ready;
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      );
    });
    await expect
      .poll(() =>
        page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
      )
      .toBe(true);
    if (process.env.HOMEBASE_AI_FIRST_SCREENSHOTS)
      await page.screenshot({
        path: `${process.env.HOMEBASE_AI_FIRST_SCREENSHOTS}/${file}.png`,
        fullPage: true,
        animations: "disabled",
      });
  }
  await page.setViewportSize({ width: 1024, height: 768 });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
    )
    .toBe(true);
});

test("native conversation keeps an unknown send and draft across navigation without duplicating work", async ({
  page,
}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.nativeRequests = [];
    let sends = 0;
    p.values.conversation_native = ({ request }: any) => {
      p.nativeRequests.push(request);
      if(request.action==="list") return {data:[]};
      if(request.action==="read") return {thread:{id:"fixture-native",turns:[]},assistant_session_id:"linked-assistant"};
      if (request.action === "create")
        return {
          state: "created",
          thread: {
            id: "fixture-native",
            cwd: "X:/isolated/native",
            turns: [],
          },
          assistant_session_id: "linked-assistant",
        };
      if (request.action === "send") {
        sends++;
        if (sends === 1) throw Error("合成连接中断，受理结果未知");
        return {
          state: "accepted",
          turn: { id: "native-turn", status: "inProgress" },
          assistant_session_id: "linked-assistant",
        };
      }
      if (request.action === "status")
        return {
          id: "fixture-native",
          state: "completed",
          item_order: ["z-user", "a-agent"],
          items: {
            "a-agent": {
              id: "a-agent",
              type: "agentMessage",
              text: "已核对能力目录，没有修改数据",
            },
            "z-user": {
              id: "z-user",
              type: "userMessage",
              content: [{ type: "inputText", text: "核对能力目录" }],
            },
          },
        };
      throw Error("Unexpected native action");
    };
  });
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await page.getByLabel("新会话使用的 Agent").selectOption("codex");
  await page.getByRole("button", { name: "新建会话", exact: true }).click();
  await page.getByLabel("给 Codex 发消息").fill("核对能力目录");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("合成连接中断");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(page.getByLabel("给 Codex 发消息")).toHaveValue("核对能力目录");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(
    page.getByText("已核对能力目录，没有修改数据", { exact: true }),
  ).toBeVisible();
  const data = await page.evaluate(
    () => (window as any).__performance.nativeRequests,
  );
  const sends = data.filter((r: any) => r.action === "send");
  expect(sends).toHaveLength(2);
  expect(sends[0].request_id).toBe(sends[1].request_id);
  expect(data.filter((r: any) => r.action === "create")).toHaveLength(1);
  const texts = await page
    .locator('[aria-label="Codex 原生对话"] article')
    .allTextContents();
  expect(texts[0]).toContain("核对能力目录");
  expect(texts[1]).toContain("已核对能力目录，没有修改数据");
});
