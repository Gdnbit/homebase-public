import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (r) =>
    new URL(r.request().url()).hostname === "127.0.0.1"
      ? r.continue()
      : r.abort(),
  );
  await page.addInitScript(() => {
    (window as any).__catalogSeed = Array.from({ length: 145 }, (_, n) => ({
      id: `catalog-${n}`,
      title: `合成会话 ${n}`,
      source: "desktop",
      updated_at: new Date(Date.UTC(2026, 10, 18, 0, 0, 145 - n)).toISOString(),
      message_count: 0,
      active_turn_id: null,
    }));
  });
});

test("assistant and sidebar share one bounded head; navigation never calls the legacy full list", async ({
  page,
}) => {
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("heading", { name: "AI 助理", exact: true }),
  ).toBeVisible();
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
  const reads = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      legacy: p.calls.filter((c: string) => c === "assistant_sessions_list")
        .length,
      requests: p.sessionCatalog.requests,
    };
  });
  expect(reads.legacy).toBe(0);
  expect(reads.requests).toHaveLength(1);
  expect(reads.requests[0].offset).toBe(0);
  expect(reads.requests[0].limit).toBeLessThanOrEqual(40);
  await page.getByRole("link", { name: "会话", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as any).__performance.sessionCatalog.requests.length,
    ),
  ).toBe(1);
});

test("search finds a session outside the first page without loading preceding pages", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.getByLabel("搜索会话").fill("合成会话 144");
  await expect(
    page.getByRole("button", { name: /合成会话 144/ }),
  ).toBeVisible();
  await page.screenshot({
    path: test.info().outputPath("catalog-search.png"),
    fullPage: true,
  });
  expect(
    await page.evaluate(() =>
      (window as any).__performance.sessionCatalog.requests.some(
        (r: any) =>
          r.query === "合成会话 144" && !r.snapshot_id && r.offset === 0,
      ),
    ),
  ).toBe(true);
});

test("older pages load once per click and browsing stays fixed during background updates", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "加载更早的 HomeBase 会话" })
    .evaluate((button: HTMLButtonElement) => {
      button.click();
      button.click();
    });
  await expect(
    page.getByText("HomeBase 已读取 40 / 145 条匹配会话，还有更早记录。", {
      exact: true,
    }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.sessionCatalog.requests.filter(
          (r: any) => r.snapshot_id,
        ).length,
    ),
  ).toBe(1);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list[0].title = "后台完成后的新标题";
    p.emit("assistant:changed", {
      session_id: "catalog-0",
      turn_id: "original-closed-task",
      config: false,
    });
  });
  await expect(
    page.getByRole("button", { name: "列表有更新，刷新查看" }),
  ).toBeVisible();
  await page.screenshot({
    path: test.info().outputPath("catalog-pinned-update.png"),
    fullPage: true,
  });
  await expect(
    page.getByRole("button", { name: /^合成会话 39 / }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /后台完成后的新标题/ }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.sessionCatalog.requests.filter(
          (r: any) => r.snapshot_id,
        ).length,
    ),
  ).toBe(1);
  await page.getByRole("button", { name: "列表有更新，刷新查看" }).click();
  await expect(
    page.getByRole("button", { name: /后台完成后的新标题/ }),
  ).toBeVisible();
  await expect(
    page.getByText("HomeBase 已读取 20 / 145 条匹配会话，还有更早记录。", {
      exact: true,
    }),
  ).toBeVisible();
});

test("manual refresh discards a late continuation even when the head snapshot is reused", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_query = async (args: any) => {
      const result = await p.sessionCatalog.query(args);
      return args.request.snapshot_id
        ? new Promise((resolve) => {
            p.releaseOlder = () => resolve(result);
          })
        : result;
    };
  });
  await page.getByRole("button", { name: "加载更早的 HomeBase 会话" }).click();
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.releaseOlder),
    )
    .toBe("function");
  await page.getByLabel("刷新会话列表").click();
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeEnabled();
  await page.evaluate(() => (window as any).__performance.releaseOlder());
  await page.waitForTimeout(100);
  await expect(
    page.getByText("HomeBase 已读取 20 / 145 条匹配会话，还有更早记录。", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: /^合成会话 39 / })).toHaveCount(
    0,
  );
});

test("expired continuation keeps already read rows and requires explicit refresh", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.getByRole("button", { name: "加载更早的 HomeBase 会话" }).click();
  await expect(
    page.getByText("HomeBase 已读取 40 / 145 条匹配会话，还有更早记录。", {
      exact: true,
    }),
  ).toBeVisible();
  await page.evaluate(() =>
    (window as any).__performance.sessionCatalog.snapshots.clear(),
  );
  await page.getByRole("button", { name: "加载更早的 HomeBase 会话" }).click();
  await expect(page.getByRole("alert")).toContainText(
    "SESSION_CATALOG_SNAPSHOT_EXPIRED",
  );
  await expect(
    page.getByRole("button", { name: /^合成会话 39 / }),
  ).toBeVisible();
  await expect(
    page.getByText("还没有符合条件的会话。", { exact: true }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.sessionCatalog.requests.filter(
          (r: any) => !r.snapshot_id,
        ).length,
    ),
  ).toBe(1);
  await page.getByLabel("刷新会话列表").click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(
    page.getByText("HomeBase 已读取 20 / 145 条匹配会话，还有更早记录。", {
      exact: true,
    }),
  ).toBeVisible();
});

test("search and refresh retain a selected old session and its protected unsent input", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.getByLabel("搜索会话").fill("合成会话 144");
  await page.getByRole("button", { name: /合成会话 144/ }).click();
  await page.evaluate(() => {
    (window as any).__performance.values.assistant_config_get.settings.enabled =
      true;
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await page
    .getByLabel("发送消息", { exact: true })
    .fill("保留旧会话未发送内容🙂");
  await expect(page.getByLabel("草稿保存状态")).toContainText(
    "草稿已受保护保存",
  );
  await page.getByLabel("搜索会话").fill("");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  await page.getByLabel("刷新会话列表").click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue(
    "保留旧会话未发送内容🙂",
  );
  const state = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      selection: p.client.getQueryData(["conversations", "selection"]),
      draft: p.desktopStore.get("catalog-144"),
      calls: p.calls,
    };
  });
  expect(state.selection.selected.id).toBe("catalog-144");
  expect(state.draft.body.text).toBe("保留旧会话未发送内容🙂");
  expect(state.calls).not.toContain("assistant_send");
  expect(state.calls).not.toContain("assistant_resume");
});

test("completion during a slow head read coalesces into one fresh followup without rereading old pages", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    let delay = true;
    p.values.assistant_sessions_query = async (args: any) => {
      const result = await p.sessionCatalog.query(args);
      if (delay && !args.request.snapshot_id) {
        delay = false;
        return new Promise((resolve) => {
          p.releaseHead = () => resolve(result);
        });
      }
      return result;
    };
  });
  await page.getByLabel("刷新会话列表").click();
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.releaseHead),
    )
    .toBe("function");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list[0].title = "慢读取期间已完成";
    p.emit("assistant:changed", {
      session_id: "catalog-0",
      turn_id: "completed-original",
      config: false,
    });
  });
  await page.waitForTimeout(450);
  await page.evaluate(() => (window as any).__performance.releaseHead());
  await expect(
    page.getByRole("button", { name: /慢读取期间已完成/ }),
  ).toBeVisible();
  const requests = await page.evaluate(
    () => (window as any).__performance.sessionCatalog.requests,
  );
  expect(requests.filter((r: any) => !r.snapshot_id)).toHaveLength(3);
  expect(requests.filter((r: any) => r.snapshot_id)).toHaveLength(0);
});

test("Chinese search composition waits for commitment and sends only the final keyword", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  const input = page.getByLabel("搜索会话");
  await input.dispatchEvent("compositionstart");
  await input.fill("中");
  await page.waitForTimeout(350);
  expect(
    await page.evaluate(() =>
      (window as any).__performance.sessionCatalog.requests.some(
        (r: any) => r.query === "中",
      ),
    ),
  ).toBe(false);
  await input.fill("合成会话 144");
  await input.dispatchEvent("compositionend");
  await expect(
    page.getByRole("button", { name: /合成会话 144/ }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.sessionCatalog.requests.filter(
          (r: any) => r.query === "合成会话 144",
        ).length,
    ),
  ).toBe(1);
});

test("a desktop session behind a full WeChat head is discovered before automatic selection", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__catalogOverride = [
      ...Array.from({ length: 30 }, (_, n) => ({
        id: `wechat-${n}`,
        title: `微信会话 ${n}`,
        source: "wechat",
        updated_at: new Date(
          Date.UTC(2026, 10, 18, 0, 0, 40 - n),
        ).toISOString(),
      })),
      {
        id: "desktop-old",
        title: "更早的桌面会话",
        source: "desktop",
        updated_at: "2026-11-01T00:00:00Z",
      },
    ];
  });
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect
    .poll(() =>
      page.evaluate(() =>
        (window as any).__performance.windowRequests.some(
          (r: any) => r.session_id === "desktop-old",
        ),
      ),
    )
    .toBe(true);
  const state = await page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      requests: p.sessionCatalog.requests,
      windows: p.windowRequests,
      calls: p.calls,
    };
  });
  expect(
    state.requests.filter((r: any) => r.source === "desktop"),
  ).toHaveLength(1);
  expect(
    state.windows.some((r: any) => r.session_id.startsWith("wechat-")),
  ).toBe(false);
  expect(state.calls).not.toContain("assistant_send");
});

test("malformed pages are errors with original rows retained rather than a confirmed empty list", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: /^合成会话 0 / }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_query = async (args: any) => ({
      ...(await p.sessionCatalog.query(args)),
      complete: "unverified",
    });
  });
  await page.getByLabel("刷新会话列表").click();
  await expect(page.getByRole("alert")).toContainText("分页结果无法核对");
  await expect(
    page.getByRole("button", { name: /^合成会话 0 / }),
  ).toBeVisible();
  await expect(page.getByText("还没有符合条件的会话。")).toHaveCount(0);
});

test("all explicitly loaded rows remain reachable with at most a hundred rendered at once", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  for (let n = 1; n <= 7; n++) {
    await page
      .getByRole("button", { name: "加载更早的 HomeBase 会话" })
      .click();
    await expect(
      page.getByText(
        new RegExp(`HomeBase 已读取 ${Math.min((n + 1) * 20, 145)} / 145`),
      ),
    ).toBeVisible();
  }
  expect(
    await page.getByRole("button", { name: /^合成会话 \d+ / }).count(),
  ).toBe(100);
  await page.getByRole("button", { name: "下一组会话" }).click();
  await expect(
    page.getByRole("button", { name: /^合成会话 144 / }),
  ).toBeVisible();
  expect(
    await page.getByRole("button", { name: /^合成会话 \d+ / }).count(),
  ).toBe(45);
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toHaveCount(0);
  expect(
    await page.evaluate(
      () => (window as any).__performance.sessionCatalog.requests.length,
    ),
  ).toBe(8);
});

test("native Agent filtering does not request a HomeBase catalog or discard native history", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.agents_history_list = ({ id }: { id: string }) => {
      if (id !== "codex") throw Error("unexpected history source");
      return {
        agent_id: "codex",
        supported: true,
        unsupported_reason: null,
        data_root: "synthetic",
        entries: [
          {
            agent_id: "codex",
            session_id: "native-old",
            first_user_prompt: "原生SDK会话",
            last_active_at: "2026-11-18T00:00:00Z",
          },
        ],
      };
    };
  });
  await page.getByLabel("按来源筛选会话").selectOption("codex");
  const before = await page.evaluate(
    () => (window as any).__performance.sessionCatalog.requests.length,
  );
  await page.getByLabel("刷新会话列表").click();
  await expect(page.getByRole("button", { name: /原生SDK会话/ })).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as any).__performance.sessionCatalog.requests.length,
    ),
  ).toBe(before);
});

test("localized source search finds older WeChat metadata and does not hide an explicit query failure", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list[144].source = "wechat";
  });
  await page.getByLabel("搜索会话").fill("微信");
  await expect(
    page.getByRole("button", { name: /合成会话 144/ }),
  ).toBeVisible();
  expect(
    await page.evaluate(() =>
      (window as any).__performance.sessionCatalog.requests.some(
        (r: any) => r.query === "wechat",
      ),
    ),
  ).toBe(true);
  await page.evaluate(() => {
    (window as any).__performance.values.assistant_sessions_query = () => {
      throw Error("synthetic directory unavailable");
    };
  });
  await page.getByLabel("搜索会话").fill("不存在的条件");
  await expect(page.getByRole("alert")).toContainText(
    "synthetic directory unavailable",
  );
  await expect(page.getByText("还没有符合条件的会话。")).toHaveCount(0);
});

test("a late older page cannot replace a newly searched list", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: "加载更早的 HomeBase 会话" }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_query = async (args: any) => {
      const result = await p.sessionCatalog.query(args);
      return args.request.snapshot_id
        ? new Promise((resolve) => {
            p.releaseOldFilter = () => resolve(result);
          })
        : result;
    };
  });
  await page.getByRole("button", { name: "加载更早的 HomeBase 会话" }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () => typeof (window as any).__performance.releaseOldFilter,
      ),
    )
    .toBe("function");
  await page.getByLabel("搜索会话").fill("合成会话 144");
  await expect(
    page.getByRole("button", { name: /合成会话 144/ }),
  ).toBeVisible();
  await page.evaluate(() => (window as any).__performance.releaseOldFilter());
  await page.waitForTimeout(100);
  await expect(page.getByRole("button", { name: /^合成会话 39 / })).toHaveCount(
    0,
  );
  await expect(
    page.getByRole("button", { name: /合成会话 144/ }),
  ).toBeVisible();
});

test("an unobserved cached running task still refreshes the active conversation index on completion", async ({
  page,
}) => {
  await page.addInitScript(() => {
    (window as any).__initialRoute = "/conversations";
    (window as any).__catalogOverride = [
      {
        id: "hidden-task-session",
        title: "后台任务正在处理",
        source: "desktop",
        updated_at: "2026-11-18T00:00:00Z",
        message_count: 2,
        active_turn_id: "unobserved-original",
      },
    ];
  });
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: /后台任务正在处理/ }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.client.setQueryData(["assistant", "session", "hidden-task-session"], {
      id: "hidden-task-session",
      source: "desktop",
      messages: [],
      turns: [{ id: "unobserved-original", state: "running" }],
    });
    p.values.assistant_sessions_list[0].title = "未打开详情的任务已完成";
    p.values.assistant_sessions_list[0].active_turn_id = null;
    p.emit("assistant:changed", {
      session_id: "hidden-task-session",
      turn_id: "unobserved-original",
      config: false,
    });
  });
  await expect(
    page.getByRole("button", { name: /未打开详情的任务已完成/ }),
  ).toBeVisible();
  const state = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { requests: p.sessionCatalog.requests, calls: p.calls };
  });
  expect(state.requests).toHaveLength(2);
  expect(state.calls).not.toContain("assistant_turn_get");
  expect(state.calls).not.toContain("assistant_send");
});

test("a completed head read is shown while a later event followup is still pending", async ({
  page,
}) => {
  await page.addInitScript(
    () => ((window as any).__initialRoute = "/conversations"),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(
    page.getByRole("button", { name: /^合成会话 0 / }),
  ).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    let count = 0;
    p.headReleases = [];
    p.values.assistant_sessions_list[0].title = "第一阶段已读快照";
    p.values.assistant_sessions_query = async (args: any) => {
      const result = await p.sessionCatalog.query(args);
      const index = count++;
      return index < 2
        ? new Promise((resolve) => {
            p.headReleases[index] = () => resolve(result);
          })
        : result;
    };
  });
  await page.getByLabel("刷新会话列表").click();
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.headReleases[0]),
    )
    .toBe("function");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list[0].title = "后续阶段新快照";
    p.emit("assistant:changed", {
      session_id: "catalog-0",
      turn_id: "later-change",
      config: false,
    });
  });
  await page.waitForTimeout(450);
  await page.evaluate(() => (window as any).__performance.headReleases[0]());
  await expect
    .poll(() =>
      page.evaluate(() => typeof (window as any).__performance.headReleases[1]),
    )
    .toBe("function");
  await expect(
    page.getByRole("button", { name: /第一阶段已读快照/ }),
  ).toBeVisible();
  await page.evaluate(() => (window as any).__performance.headReleases[1]());
  await expect(
    page.getByRole("button", { name: /后续阶段新快照/ }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => (window as any).__performance.sessionCatalog.requests.length,
    ),
  ).toBe(3);
});
