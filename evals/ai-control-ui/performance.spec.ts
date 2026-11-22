import { test, expect } from "@playwright/test";
import { navigate } from "./navigation";

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await navigate(page, "概览");
  await expect(page.getByText("磁盘健康", { exact: true })).toBeVisible();
});

test("desktop offline events allow local discovery and explicit actions without replay on reconnect", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.shared_skills_open_dir = "X:/isolated/skills";
    window.dispatchEvent(new Event("offline"));
  });
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("button", { name: /视频.*个实现/ }).click();
  await expect(page.getByRole("dialog").getByRole("heading", { name: "代码动画", exact: true })).toBeVisible();
  await page.getByLabel("关闭能力实现").click();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.capability_index_get.entries[0].name = "离线更新的动画实现";
  });
  await page.getByLabel("刷新能力索引").click();
  await page.getByRole("button", { name: /视频.*个实现/ }).click();
  await expect(page.getByRole("dialog").getByRole("heading", { name: "离线更新的动画实现", exact: true })).toBeVisible();
  await page.getByLabel("关闭能力实现").click();
  await page.getByRole("tab", { name: "工具与工作流", exact: true }).click();
  const skills = page.getByRole("region", { name: "共享 Skills 池", exact: true });
  await skills.getByRole("button", { name: /在资源管理器打开/ }).click();
  const requests = () => page.evaluate(() => {
    const p = (window as any).__performance;
    return { catalogs: p.discoveryStats.source_reads, open: p.calls.filter((c: string) => c === "shared_skills_open_dir").length };
  });
  await expect.poll(requests).toEqual({ catalogs: 2, open: 1 });
  await page.evaluate(async () => {
    window.dispatchEvent(new Event("online"));
    await (window as any).__performance.client.resumePausedMutations();
  });
  expect(await requests()).toEqual({ catalogs: 2, open: 1 });
});

test("assistant send updates its own conversation without reloading settings or the personal panel", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    const id = "performance-session";
    fixture.values.assistant_config_get.settings.enabled = true;
    fixture.values.assistant_config_get.settings.route_id = "openai::synthetic";
    fixture.values.assistant_send = ({ request }: any) => {
      const row = { id, source: "desktop", title: "隔离会话", message_count: 2,
        updated_at: new Date().toISOString(), active_turn_id: null };
      fixture.values.assistant_sessions_list = [row];
      fixture.values.assistant_session_get = {
        ...row,
        messages: [
          { id: "user", role: "user", content: request.text },
          { id: "assistant", role: "assistant", content: "已收到隔离消息" },
        ],
        turns: [],
      };
      return { session_id: id, turn_id: "performance-turn", state: "completed" };
    };
  });
  await navigate(page, "AI 助理");
  const refresh = page.getByRole("button", { name: "刷新助理" });
  await refresh.click();
  const input = page.getByLabel("发送消息");
  await input.fill("隔离性能检查");
  const calls = () => page.evaluate(() => {
    const list: string[] = (window as any).__performance.calls;
    return Object.fromEntries(["assistant_config_get", "assistant_personal_get", "assistant_send"].map(cmd => [cmd, list.filter(name => name === cmd).length]));
  });
  const send = page.getByRole("button", { name: "发送", exact: true });
  await expect(send).toBeEnabled();
  const before = await calls();
  expect(before.assistant_send).toBe(0);
  await send.click();
  await expect(page.getByText("已收到隔离消息", { exact: true })).toBeVisible();
  expect(await calls()).toEqual({ ...before, assistant_send: 1 });
  await refresh.click();
  await expect.poll(calls).toEqual({ assistant_config_get: before.assistant_config_get + 1, assistant_personal_get: before.assistant_personal_get + 1, assistant_send: 1 });
});

test("assistant manual refresh shares an in-flight read across rapid clicks", async ({ page }) => {
  await navigate(page, "AI 助理");
  await expect(page.getByRole("heading", { name: "AI 助理", exact: true })).toBeVisible();
  await expect(page.getByText("模型未启用", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    const config = fixture.values.assistant_config_get;
    const personal = fixture.values.assistant_personal_get;
    fixture.calls.length = 0;
    const barrier = new Promise<void>((resolve) => { fixture.releaseRefresh = resolve; });
    fixture.values.assistant_config_get = () => barrier.then(() => config);
    fixture.values.assistant_personal_get = () => barrier.then(() => personal);
  });

  const refresh = page.getByRole("button", { name: "刷新助理" });
  await refresh.evaluate((button: HTMLButtonElement) => {
    button.click();
    button.click();
    button.click();
  });
  const counts = () => page.evaluate(() => {
    const calls: string[] = (window as any).__performance.calls;
    return {
      config: calls.filter((name) => name === "assistant_config_get").length,
      personal: calls.filter((name) => name === "assistant_personal_get").length,
      sends: calls.filter((name) => name === "assistant_send").length,
    };
  });
  expect(await counts()).toEqual({ config: 1, personal: 1, sends: 0 });
  await expect(refresh).toBeDisabled();
  await page.evaluate(() => (window as any).__performance.releaseRefresh());
  await expect(refresh).toBeEnabled();
  await refresh.click();
  await expect.poll(counts).toEqual({ config: 2, personal: 2, sends: 0 });
});

test("WeChat stops QR checks after confirmation and keeps manual status refresh", async ({ page }) => {
  await page.evaluate(() => {
    const { values } = (window as any).__performance;
    values.wechat_status = { bound: false, enabled: false, state: "unbound", message: "隔离未绑定", tasks: [] };
    values.wechat_login_begin = null;
    values.wechat_pairing = { state: "wait", message: "等待扫码", qr_data_url: null };
  });
  await page.getByRole("link", { name: "微信连接", exact: true }).click();
  await page.getByRole("button", { name: "扫码绑定", exact: true }).click();
  await expect(page.getByText("等待扫码", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_pairing = { state: "confirmed", message: "绑定成功", qr_data_url: null };
  });
  await expect(page.getByRole("button", { name: "完成", exact: true })).toBeVisible();
  const countPair = () => page.evaluate(() => (window as any).__performance.calls.filter((name: string) => name === "wechat_pairing").length);
  const confirmed = await countPair();
  await page.waitForTimeout(1750);
  expect(await countPair()).toBe(confirmed);
  await page.getByRole("button", { name: "完成", exact: true }).click();
  await page.evaluate(() => {
    Object.assign((window as any).__performance.values.wechat_status, { bound: true, enabled: true, state: "connected", message: "隔离已重连" });
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await expect(page.getByText("隔离已重连", { exact: true })).toBeVisible();
});

test("all routes render; fresh reads survive navigation and unopened graph/list views stay idle", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_status = {
      bound: false, enabled: false, state: "unbound", message: "隔离未绑定", tasks: [],
    };
  });
  const counts = () =>
    page.evaluate(() =>
      (window as any).__performance.calls.reduce(
        (a: Record<string, number>, k: string) => ({
          ...a,
          [k]: (a[k] || 0) + 1,
        }),
        {},
      ),
    );
  for (const name of [
    "AI 助理",
    "会话",
    "微信连接",
    "设置",
    "AI 代理",
    "历史对话",
    "文章生成",
    "快捷面板",
    "能力库",
    "投料口",
    "知识库",
    "密钥",
    "治理追溯",
    "系统",
  ]) {
    await navigate(page, name);
    await expect(page.getByText("正在加载页面…", { exact: true })).toHaveCount(
      0,
    );
    await page.waitForTimeout(100);
  }
  await navigate(page, "知识库");
  await expect(page.getByRole("button", { name: /AI 知识库/ })).toBeVisible();
  expect((await counts()).topology_load).toBeUndefined();
  await page.getByRole("button", { name: /AI 知识库/ }).click();
  await expect.poll(async () => (await counts()).topology_load).toBe(1);
  expect((await counts()).docs_list).toBeUndefined();
  await page.getByRole("button", { name: "列表", exact: true }).click();
  await expect.poll(async () => (await counts()).docs_list).toBe(1);
  await page.getByRole("button", { name: "逻辑图", exact: true }).click();
  await page.getByRole("button", { name: "列表", exact: true }).click();
  expect((await counts()).docs_list).toBe(1);
  expect((await counts()).wiki_list).toBe(1);
  expect((await counts()).wiki_dirs).toBe(1);
  expect((await counts()).system_info).toBe(1);
  expect((await counts()).system_disks).toBe(1);
  expect((await counts()).secrets_list).toBe(1);
  expect((await counts()).system_preflight).toBe(1);
  expect(errors).toEqual([]);
  expect(
    await page.evaluate(() => (window as any).__performance.rejected),
  ).toEqual([]);
});

test("cache deduplicates reads, expires, invalidates after mutation, and never caches failed reads as success", async ({
  page,
}) => {
  const result = await page.evaluate(async () => {
    const { client } = (window as any).__performance;
    let reads = 0;
    const options = {
      queryKey: ["wiki", "synthetic"],
      queryFn: async () => {
        reads++;
        await new Promise((r) => setTimeout(r, 30));
        return reads;
      },
    };
    await Promise.all([
      client.fetchQuery(options),
      client.fetchQuery(options),
      client.fetchQuery(options),
    ]);
    const concurrent = reads;
    await client.fetchQuery(options);
    const cached = reads;
    await client.invalidateQueries({ queryKey: ["wiki", "synthetic"] });
    await client.fetchQuery(options);
    const invalidated = reads;
    await client.fetchQuery({ ...options, staleTime: 0 });
    let failures = 0;
    try {
      await client.fetchQuery({
        queryKey: ["synthetic-fail"],
        queryFn: () => {
          failures++;
          throw Error("synthetic");
        },
      });
    } catch {}
    return {
      concurrent,
      cached,
      invalidated,
      expired: reads,
      failures,
      status: client.getQueryState(["synthetic-fail"]).status,
    };
  });
  expect(result).toEqual({
    concurrent: 1,
    cached: 1,
    invalidated: 2,
    expired: 3,
    failures: 1,
    status: "error",
  });
});

test("knowledge count distinguishes unopened, loading and empty without eager list reads", async ({ page }) => {
  await navigate(page, "知识库");
  await page.getByRole("button", { name: /AI 知识库/ }).click();
  const status = page.getByRole("status", { name: "文档索引状态" });
  await expect(status).toHaveText("· 切换到列表读取文档");
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "docs_list").length)).toBe(0);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.docs_list = () => new Promise((resolve) => { p.resolveIndex = resolve; });
  });
  await page.getByRole("button", { name: "列表", exact: true }).click();
  await expect(status).toHaveText("· 正在读取索引…");
  await page.evaluate(() => (window as any).__performance.resolveIndex([]));
  await expect(status).toHaveText("· 0 篇");
  await expect(page.getByText("索引中暂无登记文档", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "逻辑图", exact: true }).click();
  await page.getByRole("button", { name: "列表", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "docs_list").length)).toBe(1);
});

test("knowledge failures stay explicit and refresh retains stale documents before recovery", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.docs_list = () => { throw Error("合成索引不可读取"); };
    p.values.obsidian_health = () => { throw Error("合成健康查询失败"); };
  });
  await navigate(page, "知识库");
  await page.getByRole("button", { name: /AI 知识库/ }).click();
  await page.getByRole("button", { name: "列表", exact: true }).click();
  const status = page.getByRole("status", { name: "文档索引状态" });
  await expect(status).toHaveText("· 索引读取失败");
  await expect(page.getByRole("alert").filter({ hasText: "文档索引读取失败" })).toContainText("合成索引不可读取");
  await expect(page.getByText("健康检查失败，状态未知", { exact: true })).toBeVisible();
  await expect(page.getByText("索引中暂无登记文档", { exact: true })).toHaveCount(0);
  await page.evaluate(() => {
    (window as any).__performance.values.docs_list = [{ doc_id: "synthetic-doc", path: "synthetic.md", role: "合成索引文档", updated: null, starred: false, is_new: false }];
  });
  await page.getByRole("button", { name: "刷新知识库", exact: true }).click();
  await expect(status).toHaveText("· 1 篇");
  await page.evaluate(() => {
    (window as any).__performance.values.docs_list = () => { throw Error("合成刷新失败"); };
  });
  await page.getByRole("button", { name: "刷新知识库", exact: true }).click();
  await expect(status).toHaveText("· 索引读取失败（保留上次 1 篇）");
  await expect(page.getByRole("alert").filter({ hasText: "文档索引读取失败" })).toContainText("已保留上次读取的列表");
  await expect(page.getByRole("button", { name: /合成索引文档/ }).first()).toBeVisible();
  await page.evaluate(() => { (window as any).__performance.values.docs_list = []; });
  await page.getByRole("button", { name: "刷新知识库", exact: true }).click();
  await expect(status).toHaveText("· 0 篇");
  await expect(page.getByRole("alert").filter({ hasText: "文档索引读取失败" })).toHaveCount(0);
});

test("slow polling never overlaps, pauses when hidden, resumes and cleans up", async ({
  page,
}) => {
  const result = await page.evaluate(async () => {
    const { startPolling } = (window as any).__performance;
    let hidden = false;
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => hidden,
    });
    let active = 0,
      max = 0,
      calls = 0;
    const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));
    const stop = startPolling(async () => {
      calls++;
      active++;
      max = Math.max(max, active);
      await delay(80);
      active--;
    }, 10);
    await delay(225);
    hidden = true;
    document.dispatchEvent(new Event("visibilitychange"));
    await delay(100);
    const paused = calls;
    await delay(100);
    const stillPaused = calls === paused;
    hidden = false;
    document.dispatchEvent(new Event("visibilitychange"));
    await delay(100);
    const resumed = calls > paused;
    stop();
    const stopped = calls;
    await delay(110);
    return { max, stillPaused, resumed, cleaned: calls === stopped };
  });
  expect(result).toEqual({
    max: 1,
    stillPaused: true,
    resumed: true,
    cleaned: true,
  });
});

test("query polling stops off-route and in hidden windows", async ({
  page,
}) => {
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
    (window as any).__performance.calls.length = 0;
  });
  await page.waitForTimeout(5500);
  expect(
    await page.evaluate(() => (window as any).__performance.calls),
  ).toEqual([]);
  await page.evaluate(() => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => "visible",
    });
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await navigate(page, "历史对话");
  await expect(page.getByText("尚无历史对话", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    (window as any).__performance.calls.length = 0;
  });
  await page.waitForTimeout(5500);
  expect(
    await page.evaluate(() => (window as any).__performance.calls),
  ).toEqual([]);
});

test("agent drawer subpages load on demand and reuse fresh results", async ({
  page,
}) => {
  await page.evaluate(async () => {
    const { values, client } = (window as any).__performance;
    values.agents_list = [
      {
        id: "codex",
        name: "Synthetic Codex",
        kind: "cli",
        config_path: null,
        config_exists: true,
        launch_subcommand: null,
        process_match: [],
      },
    ];
    values.agents_status = {
      id: "codex",
      state: "stopped",
      running: false,
      pid: null,
      config_exists: true,
      config_path: null,
      mcp_servers: [],
      last_seen: null,
      error: null,
    };
    values.agents_get_model = {
      agent_id: "codex",
      current: null,
      presets: [],
      supported: false,
      unsupported_reason: "Synthetic model",
    };
    values.agents_history_list = {
      agent_id: "codex",
      entries: [],
      supported: true,
      data_root: "synthetic",
    };
    await client.invalidateQueries({ queryKey: ["agents", "list"] });
  });
  await navigate(page, "AI 代理");
  await page
    .getByRole("button", { name: "查看 Synthetic Codex 详情", exact: true })
    .click();
  const drawer = page.getByRole("dialog", { name: "Synthetic Codex 详情" });
  const count = (command: string) =>
    page.evaluate(
      (c) =>
        (window as any).__performance.calls.filter((s: string) => s === c)
          .length,
      command,
    );
  expect(await count("agents_get_model")).toBe(0);
  expect(await count("agents_history_list")).toBe(0);
  await drawer.getByRole("button", { name: "模型", exact: true }).click();
  await expect(
    page.getByText("Synthetic model", { exact: true }),
  ).toBeVisible();
  await drawer.getByRole("button", { name: "历史", exact: true }).click();
  await expect.poll(() => count("agents_history_list")).toBe(1);
  await drawer.getByRole("button", { name: "模型", exact: true }).click();
  await drawer.getByRole("button", { name: "历史", exact: true }).click();
  expect(await count("agents_get_model")).toBe(1);
  expect(await count("agents_history_list")).toBe(1);
});

test("large history renders incrementally while search includes unloaded sessions", async ({
  page,
}) => {
  await page.evaluate(() => {
    const { values } = (window as any).__performance;
    values.agents_history_all = {
      buckets: [],
      entries: Array.from({ length: 205 }, (_, i) => ({
        agent_id: "codex",
        session_id: `session-${i}`,
        cwd: "synthetic",
        first_user_prompt: `Synthetic prompt ${i}`,
        started_at: "2026-11-09T00:00:00Z",
        last_active_at: "2026-11-09T00:00:00Z",
        message_count: 1,
        file_path: "synthetic",
      })),
    };
  });
  await navigate(page, "历史对话");
  await expect(
    page.getByText("显示 100 / 205 个会话", { exact: true }),
  ).toBeVisible();
  expect(
    await page
      .getByRole("button")
      .filter({ hasText: /Synthetic prompt/ })
      .count(),
  ).toBe(100);
  await page.getByRole("button", { name: /加载更多会话/ }).click();
  expect(
    await page
      .getByRole("button")
      .filter({ hasText: /Synthetic prompt/ })
      .count(),
  ).toBe(200);
  await page.getByRole("textbox").fill("Synthetic prompt 204");
  await expect(
    page.getByText("Synthetic prompt 204", { exact: true }),
  ).toBeVisible();
  expect(
    await page
      .getByRole("button")
      .filter({ hasText: /Synthetic prompt/ })
      .count(),
  ).toBe(1);
});
