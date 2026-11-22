import { test, expect } from "@playwright/test";
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("heading", { name: "AI 助理", exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.capability_index_get = {
      version: "homebase-capability-index-v1",
      entries: Array.from({ length: 65 }, (_, i) => ({
        entry_key: i.toString(16).padStart(64, "0"),
        id: `fixture-${i}`,
        function: "video",
        kind: i % 2 ? "repository" : "model",
        name: `测试动画${i}`,
        provider: "隔离来源",
        state: "unknown",
        cost: "unknown",
        discover:
          i % 2 ? "repo_capability_describe" : "model_capability_resolve",
        target: `fixture-${i}`,
        prerequisites: ["先核对契约和费用，未执行能力。"],
        evidence: "仅合成目录元数据",
      })),
      issues: [],
      routing: "隔离元数据",
    };
  });
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await expect(
    page.getByRole("button", { name: /视频.*65 个实现/ }),
  ).toBeVisible();
});
test("groups count the complete catalog while lazy pages reuse one snapshot and never execute", async ({
  page,
}) => {
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator("article")).toHaveCount(20);
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await expect(dialog.locator("article")).toHaveCount(40);
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await expect(dialog.locator("article")).toHaveCount(60);
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await expect(dialog.locator("article")).toHaveCount(65);
  await expect(
    dialog.getByRole("button", { name: "显示更多实现" }),
  ).toHaveCount(0);
  await dialog.locator("article").first().getByText("运行条件与依据").click();
  await expect(dialog).toContainText("仅合成目录元数据");
  await expect(dialog).toContainText("调用费用需核对");
  await page.getByLabel("关闭能力实现").click();
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  await expect(dialog.locator("article")).toHaveCount(65);
  const stats = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      reads: p.discoveryStats.source_reads,
      requests: p.capabilityRequests,
      calls: p.calls,
    };
  });
  expect(stats.reads).toBe(1);
  const pages = stats.requests.filter((r: any) => r.function === "video");
  expect(pages).toHaveLength(4);
  expect(
    pages.slice(1).every((r: any) => r.snapshot_id === pages[1].snapshot_id),
  ).toBe(true);
  expect(stats.calls).not.toContain("assistant_send");
  expect(stats.calls).not.toContain("repo_capability_run");
  await page.getByLabel("关闭能力实现").click();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    for (const q of p.client.getQueryCache().getAll())
      if (
        q.queryKey[0] === "capability-index" &&
        q.queryKey[1] === "implementations"
      )
        q.setState({ dataUpdatedAt: Date.now() - 120_000 });
  });
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  await expect(dialog.locator("article")).toHaveCount(65);
  await page.waitForTimeout(500);
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.capabilityRequests.filter(
          (r: any) => r.function === "video",
        ).length,
    ),
  ).toBe(4);
});
test("search reuses the catalog, source events refresh once and pagination failure requires explicit refresh", async ({
  page,
}) => {
  await page.getByLabel("搜索能力").fill("测试动画64");
  await expect(
    page.getByRole("button", { name: /视频.*1 个实现/ }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as any).__performance.discoveryStats.source_reads,
    ),
  ).toBe(1);
  await page.getByLabel("搜索能力").fill("");
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator("article")).toHaveCount(20);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.capabilityViews.clear();
  });
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await expect(dialog.getByRole("alert")).toContainText(
    "DISCOVERY_SNAPSHOT_EXPIRED",
  );
  expect(
    await page.evaluate(
      () => (window as any).__performance.discoveryStats.source_reads,
    ),
  ).toBe(1);
  await dialog.getByRole("button", { name: "刷新目录并重新读取" }).click();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(dialog.locator("article")).toHaveCount(20);
  expect(
    await page.evaluate(
      () => (window as any).__performance.discoveryStats.source_reads,
    ),
  ).toBe(2);
  const beforeEvent = await page.evaluate(
    () => (window as any).__performance.capabilityRequests.length,
  );
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.capability_index_get.entries.push({
      ...p.values.capability_index_get.entries[0],
      id: "new",
      target: "new",
      entry_key: "new",
      name: "新增实现",
    });
    p.emit("capabilities:changed", {});
  });
  await expect(dialog.getByRole("heading", { name: "新增实现" })).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as any).__performance.discoveryStats.source_reads,
    ),
  ).toBe(3);
  const afterEvent = await page.evaluate(
    (from) => (window as any).__performance.capabilityRequests.slice(from),
    beforeEvent,
  );
  expect(afterEvent.filter((r: any) => r.function === "video")).toHaveLength(1);
});

test("compares cached implementations across pages without reading or executing again", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const [model, project] = p.values.capability_index_get.entries;
    Object.assign(model, {
      provider: "测试模型渠道",
      state: "not_verified",
      credential_binding: "测试模型的凭据引用",
      prerequisites: ["需核对测试账号额度"],
      pricing: {
        route_id: model.target,
        state: "partial",
        source_url: "https://openrouter.ai/api/v1/models",
        observed_at: "2026-11-20T05:00:00Z",
        reference_only: true,
        rates: [{ kind: "request", amount: "0", unit: "usd_per_request", per_million_tokens: null }],
        unmodeled_dimensions: 0, missing_dimensions: 2, invalid_dimensions: 0, conditional: false,
      },
    });
    Object.assign(project, {
      provider: "隔离仓库",
      state: "verified",
      cost: "local_compute",
      prerequisites: ["需要本机渲染器"],
      evidence: "验收样例包含视频导出",
    });
    p.emit("capabilities:changed", {});
  });
  await expect.poll(() => page.evaluate(() => (window as any).__performance.discoveryStats.source_reads)).toBe(2);
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  const compare = (index: number) => dialog.getByRole("button", { name: `测试动画${index} · 比较`, exact: true });
  await compare(0).click();
  await expect(compare(0)).toHaveAttribute("aria-pressed", "true");
  await expect(compare(4)).toHaveCount(0);
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await expect(dialog.locator("article")).toHaveCount(40);
  const callsBefore = await page.evaluate(() => (window as any).__performance.calls.length);
  await compare(4).click();
  await compare(1).click();
  await expect(compare(2)).toBeDisabled();
  const table = dialog.getByRole("table", { name: "比较实现" });
  const row = (name: string) => table.getByRole("rowheader", { name, exact: true }).locator("..");
  await dialog.getByRole("button", { name: "比较已选（3）" }).click();
  await expect(table.getByRole("columnheader")).toHaveText(["对照项", "测试动画0", "测试动画4", "测试动画1"]);
  await expect(row("来源与类型")).toContainText("隔离仓库");
  await expect(row("可用状态")).toContainText("账号调用权限待验证");
  await expect(row("费用")).toContainText("0 USD / 次");
  await expect(row("费用")).toContainText("零报价也不保证可免费调用");
  await expect(row("费用")).toContainText("本机算力成本");
  await expect(row("凭据与前置条件")).toContainText("测试模型的凭据引用");
  await expect(row("凭据与前置条件")).toContainText("需要本机渲染器");
  await expect(row("验收依据")).toContainText("验收样例包含视频导出");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark" });
  const scroller = dialog.getByRole("region", { name: "横向滚动比较表" });
  await scroller.focus();
  await page.keyboard.press("ArrowRight");
  await expect.poll(() => scroller.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
  expect(await dialog.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  await dialog.getByRole("button", { name: "清空比较" }).click();
  await expect(table).toHaveCount(0);
  await expect(compare(0)).toHaveAttribute("aria-pressed", "false");
  await expect(compare(2)).toBeEnabled();
  expect(await page.evaluate((from) => (window as any).__performance.calls.slice(from), callsBefore)).toEqual([]);
});

test("comparison uses one snapshot and hands off only the selected implementation", async ({ page }) => {
  const group = page.getByRole("button", { name: /视频.*65 个实现/ });
  await group.click();
  const dialog = page.getByRole("dialog");
  const compare = (index: number) => dialog.getByRole("button", { name: `测试动画${index} · 比较`, exact: true });
  const table = dialog.getByRole("table", { name: "比较实现" });
  await compare(0).click();
  await compare(1).click();
  await expect(table).toBeVisible();
  await page.keyboard.press("Escape");
  await group.click();
  await expect(table).toHaveCount(0);
  await compare(0).click();
  await compare(1).click();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.capability_index_get.entries[1].name = "更正后的实现";
    p.emit("capabilities:changed", {});
  });
  await expect(dialog.getByRole("heading", { name: "更正后的实现" })).toBeVisible();
  await expect(table).toHaveCount(0);
  await expect(compare(0)).toHaveAttribute("aria-pressed", "false");
  expect(await page.evaluate(() => (window as any).__performance.createRequests.length)).toBe(0);
  await compare(0).click();
  await dialog.getByRole("button", { name: "更正后的实现 · 比较" }).click();
  await table.getByRole("button", { name: "准备任务 · 更正后的实现" }).click();
  await expect(page.getByRole("heading", { name: "AI 助理", exact: true })).toBeVisible();
  const receipt = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      calls: p.calls,
      drafts: p.createRequests.length,
      text: p.desktopStore.get(p.createRequests[0]?.requestId)?.body.text,
    };
  });
  expect(receipt.drafts).toBe(1);
  expect(receipt.text).toContain(`"entry_key": "${"1".padStart(64, "0")}"`);
  expect(receipt.text).not.toContain(`"entry_key": "${"0".repeat(64)}"`);
  expect(receipt.calls).not.toContain("assistant_send");
  expect(receipt.calls).not.toContain("assistant_desktop_send");
  expect(receipt.calls).not.toContain("repo_capability_run");
});

test("prepares an assistant comparison with only the selected keys and their exact pages", async ({ page }) => {
  await page.getByLabel("搜索能力").fill("测试动画");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.capabilityRequests.at(-1)?.query)).toBe("测试动画");
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  const pick = (i: number) => dialog.getByRole("button", { name: `测试动画${i} · 比较`, exact: true });
  await pick(0).click();
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await pick(4).click();
  await pick(1).click();
  await dialog.getByRole("button", { name: "交给助理比较", exact: true }).click();
  await expect(page.getByRole("heading", { name: "AI 助理", exact: true })).toBeVisible();
  const observed = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      creates: p.createRequests.length,
      draft: p.desktopStore.get(p.createRequests[0]?.requestId)?.body.text as string,
      pages: p.capabilityRequests.filter((r: any) => r.function === "video"),
      calls: p.calls,
    };
  });
  expect(observed.creates).toBe(1);
  expect(observed.pages.map((r: any) => r.offset)).toEqual([0, 20]);
  expect(observed.draft).toContain("仅查询能力目录");
  expect(observed.draft).toContain("找不到所选实现时请指出缺项");
  expect(observed.draft).toContain("以相同 function 和 entry_keys 再查一次");
  const data = JSON.parse(observed.draft.split("```json\n")[1].split("\n```")[0]);
  expect(data).toEqual({
    catalog_id: observed.pages[0].catalog_id,
    snapshot_id: observed.pages[1].snapshot_id,
    function: "video",
    query: "测试动画",
    selections: [
      { entry_key: "0".repeat(64), offset: 0 },
      { entry_key: "4".padStart(64, "0"), offset: 20 },
      { entry_key: "1".padStart(64, "0"), offset: 0 },
    ],
  });
  expect(observed.calls).not.toContain("assistant_send");
  expect(observed.calls).not.toContain("assistant_desktop_send");
  expect(observed.calls).not.toContain("repo_capability_run");
});

test("an open expired snapshot cannot prepare drafts or fetch another page", async ({ page }) => {
  await page.clock.install();
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  for (const i of [0, 1])
    await dialog.getByRole("button", { name: `测试动画${i} · 比较`, exact: true }).click();
  const group = dialog.getByRole("button", { name: /让助理按任务选择/ });
  const compare = dialog.getByRole("button", { name: "交给助理比较", exact: true });
  await expect(group).toBeEnabled();
  const before = await page.evaluate(() => (window as any).__performance.capabilityRequests.length);
  await page.clock.fastForward(301_000);
  await expect(dialog.getByRole("alert")).toContainText("能力目录已过期");
  await expect(group).toBeDisabled();
  await expect(compare).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "显示更多实现" })).toBeDisabled();
  await expect(dialog.getByRole("button", { name: "使用这个实现准备任务" }).first()).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__performance.capabilityRequests.length)).toBe(before);
  await dialog.getByRole("button", { name: "刷新目录并重新读取" }).click();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(group).toBeEnabled();
  await expect(compare).toHaveCount(0);
  const result = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { reads: p.capabilityRequests.length, creates: p.createRequests.length };
  });
  expect(result).toEqual({ reads: before + 2, creates: 0 });
});

test("long Unicode queries explain the limit without rereading; a narrow drawer stays usable", async ({
  page,
}) => {
  await page.getByLabel("搜索能力").fill("中".repeat(81));
  await expect(page.getByRole("alert")).toContainText("关键词过长");
  expect(
    await page.evaluate(
      () => (window as any).__performance.capabilityRequests.length,
    ),
  ).toBe(1);
  await page.getByLabel("搜索能力").fill("测试动画64");
  await expect(
    page.getByRole("button", { name: /视频.*1 个实现/ }),
  ).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark" });
  await page.getByRole("button", { name: /视频.*1 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "测试动画64", exact: true }),
  ).toBeVisible();
  expect(
    await dialog.evaluate((el) => el.scrollWidth <= el.clientWidth + 1),
  ).toBe(true);
  await page.keyboard.press("Escape");
  await expect(dialog).not.toBeVisible();
  expect(
    await page.evaluate(
      () => (window as any).__performance.discoveryStats.source_reads,
    ),
  ).toBe(1);
});

test("a changed snapshot on append is rejected, and an explicit refresh restores handoff", async ({ page }) => {
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator("article")).toHaveCount(20);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const original = p.values.capability_index_query;
    p.values.capability_index_query = async (args: any) => {
      const result = await original(args);
      if (args.request.offset > 0) {
        result.snapshot_id = crypto.randomUUID();
        result.entries[0] = { ...result.entries[0], name: "错误快照的实现" };
      }
      return result;
    };
    p.restoreCapabilityRead = () => { p.values.capability_index_query = original; };
  });
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await expect(dialog.getByRole("alert")).toContainText("目录、快照或筛选不一致");
  await expect(dialog.locator("article")).toHaveCount(20);
  await expect(dialog.getByText("错误快照的实现")).toHaveCount(0);
  const choose = dialog.getByRole("button", { name: "使用这个实现准备任务" }).first();
  await expect(choose).toBeDisabled();
  await expect(dialog.getByRole("button", { name: /让助理按任务选择/ })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__performance.createRequests.length)).toBe(0);
  await page.evaluate(() => (window as any).__performance.restoreCapabilityRead());
  await dialog.getByRole("button", { name: "刷新目录并重新读取" }).click();
  await expect(dialog.getByRole("alert")).toHaveCount(0);
  await expect(choose).toBeEnabled();
  await choose.click();
  await expect(page.getByRole("heading", { name: "AI 助理", exact: true })).toBeVisible();
  const calls = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { creates: p.createRequests.length, calls: p.calls };
  });
  expect(calls.creates).toBe(1);
  expect(calls.calls).not.toContain("assistant_send");
  expect(calls.calls).not.toContain("assistant_desktop_send");
});

test("a price for another route cannot become the chosen implementation", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const original = p.values.capability_index_query;
    p.values.capability_index_query = async (args: any) => {
      const result = await original(args);
      if (args.request.function === "video") {
        const model = result.entries.find((e: any) => e.kind === "model");
        model.pricing = {
          route_id: "other-provider::unrelated",
          state: "reported",
          source_url: "https://openrouter.ai/api/v1/models",
          observed_at: result.assembled_at,
          reference_only: true,
          rates: [{ kind: "request", amount: "0", unit: "usd_per_request", per_million_tokens: null }],
          unmodeled_dimensions: 0, missing_dimensions: 0, invalid_dimensions: 0, conditional: false,
        };
      }
      return result;
    };
  });
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText("参考报价与实现不一致");
  await expect(dialog.locator("article")).toHaveCount(0);
  await expect(dialog.getByRole("button", { name: /让助理按任务选择/ })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__performance.createRequests.length)).toBe(0);
});

test("source changes during a slow catalog read queue one refresh without rereading old pages", async ({ page }) => {
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator("article")).toHaveCount(20);
  await dialog.getByRole("button", { name: "显示更多实现" }).click();
  await expect(dialog.locator("article")).toHaveCount(40);
  const catalogId = await page.evaluate(() => {
    const p = (window as any).__performance;
    const original = p.values.capability_index_query;
    let holdNextCatalog = true;
    p.values.capability_index_query = async (args: any) => {
      const page = await original(args);
      if (holdNextCatalog && !args.request.catalog_id && !args.request.snapshot_id) {
        holdNextCatalog = false;
        p.intermediateCapabilityId = page.catalog_id;
        return new Promise(resolve => { p.releaseCapabilityCatalog = () => resolve(page); });
      }
      return page;
    };
    p.emit("capabilities:changed", {});
    return p.capabilityRequests.find((r: any) => r.function === "video").catalog_id;
  });
  await expect.poll(() => page.evaluate(() => !!(window as any).__performance.releaseCapabilityCatalog)).toBe(true);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const row = p.values.capability_index_get.entries[0];
    p.values.capability_index_get.entries.unshift({ ...row, entry_key: "latest", id: "latest", target: "latest", name: "00 最新实现" });
    for (let i = 0; i < 7; i++) p.emit("capabilities:changed", {});
    p.emit("assistant:changed", { session_id: null, turn_id: null, config: true });
  });
  await page.waitForTimeout(550);
  expect(await page.evaluate(() => (window as any).__performance.discoveryStats.source_reads)).toBe(2);
  await expect(dialog.getByRole("button", { name: /让助理按任务选择/ })).toBeDisabled();
  await page.evaluate(() => (window as any).__performance.releaseCapabilityCatalog());
  await expect(dialog.getByRole("heading", { name: "00 最新实现", exact: true })).toBeVisible();
  const stats = await page.evaluate((originalId) => {
    const p = (window as any).__performance;
    return {
      reads: p.discoveryStats.source_reads,
      oldPages: p.capabilityRequests.filter((r: any) => r.catalog_id === originalId).length,
      intermediatePages: p.capabilityRequests.filter((r: any) => r.catalog_id === p.intermediateCapabilityId).length,
    };
  }, catalogId);
  expect(stats).toEqual({ reads: 3, oldPages: 2, intermediatePages: 0 });
});

test("saving a credential setting marks the dormant capability catalog stale", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const secret = {
      short_name: "synthetic_refresh", display_name: "隔离凭据", category: "core", configured: true,
      value_state: "filled", masked_value: "isolation***", last_health: null, expires_at: null,
      revision: 1, vault_version: "fixture-1", service_id: null, expiry_policy: "warn",
      rotations: [], used_by: [], has_health_check: false, mcp_long_name: null, rotate_url: "",
    };
    p.values.secrets_list = [secret];
    p.values.secrets_save = ({ request }: any) => {
      if (request.short_name !== secret.short_name || request.value != null)
        throw Error("Only the isolated metadata is writable");
      secret.display_name = request.display_name;
      secret.revision++;
      p.values.capability_index_get.entries[0].name = "00 凭据更新后";
      return { revision: secret.revision, vault_version: secret.vault_version, verified: false };
    };
  });
  await page.getByRole("link", { name: "密钥与连接", exact: true }).click();
  const row = page.locator('[data-secret-name="synthetic_refresh"]');
  await row.locator("button[aria-expanded]").click();
  await row.getByLabel("显示名称", { exact: true }).fill("隔离元数据已更新");
  await expect(row.getByRole("status").last()).toContainText("元数据已保存");
  await page.getByRole("button", { name: "关闭密钥编辑", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__performance.discoveryStats.source_reads)).toBe(1);
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("button", { name: /视频.*65 个实现/ }).click();
  await expect(page.getByRole("dialog").getByRole("heading", { name: "00 凭据更新后", exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.discoveryStats.source_reads)).toBe(2);
});
