import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_sessions_list = [
      {
        id: "window-session",
        title: "隔离真实分页",
        source: "desktop",
        message_count: 120,
        active_turn_id: null,
      },
    ];
    p.values.assistant_session_get = {
      id: "window-session",
      title: "隔离真实分页",
      source: "desktop",
      messages: Array.from({ length: 120 }, (_, i) => ({
        id: `row-${i}`,
        role: i % 2 ? "assistant" : "user",
        content: `完整窗口消息${String(i).padStart(3, "0")}：${"中文正文，保持完整。".repeat(6)}`,
        turn_id: null,
      })),
      turns: [],
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.locator(".chat-message")).toHaveCount(40);
});

test("initial timeline reads one bounded window and earlier loading preserves reading anchor", async ({
  page,
}) => {
  const original = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { calls: p.calls, requests: p.windowRequests };
  });
  expect(original.calls).not.toContain("assistant_session_get");
  expect(original.requests).toHaveLength(1);
  expect(original.requests[0].limit).toBe(40);
  const transcript = page.getByLabel("对话内容", { exact: true });
  await transcript.evaluate((e) => {
    e.scrollTop = 0;
    e.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  const anchor = page.getByText(/^完整窗口消息080：/);
  const top = await anchor.evaluate((e) => e.getBoundingClientRect().top);
  await page.getByRole("button", { name: "显示更早消息", exact: true }).click();
  await expect(page.locator(".chat-message")).toHaveCount(80);
  expect(
    Math.abs(
      (await anchor.evaluate((e) => e.getBoundingClientRect().top)) - top,
    ),
  ).toBeLessThan(4);
  await expect(
    page.getByRole("button", { name: "回到最新回复", exact: true }),
  ).toHaveCount(0);
  const calls = await page.evaluate(
    () => (window as any).__performance.windowRequests,
  );
  expect(calls.at(-1).before_message_id).toBe("row-80");
});

test("refresh while reading keeps the loaded oldest message and updates without another send", async ({
  page,
}) => {
  const transcript = page.getByLabel("对话内容", { exact: true });
  await transcript.evaluate((e) => {
    e.scrollTop = 0;
    e.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  await page.getByRole("button", { name: "显示更早消息", exact: true }).click();
  await expect(page.locator(".chat-message")).toHaveCount(80);
  await transcript.evaluate((e) => {
    e.scrollTop = 0;
    e.dispatchEvent(new Event("scroll", { bubbles: true }));
  });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    for (let i = 120; i < 122; i++)
      p.values.assistant_session_get.messages.push({
        id: `row-${i}`,
        role: "assistant",
        content: `新的窗口消息${i}`,
        turn_id: null,
      });
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByText(/^完整窗口消息040：/)).toBeAttached();
  await expect(
    page.getByText("新的窗口消息121", { exact: true }),
  ).toBeAttached();
  expect(
    (await page.evaluate(() => (window as any).__performance.calls)).filter(
      (c: string) => ["assistant_send", "assistant_resume"].includes(c),
    ),
  ).toHaveLength(0);
});

async function completedTask(
  page: import("@playwright/test").Page,
  wrongScope = false,
) {
  await page.evaluate(
    ({ wrongScope }) => {
      const p = (window as any).__performance;
      const task = {
        id: "window-task",
        session_id: "window-session",
        state: "completed",
        text: "已完成合成步骤",
        interaction: "task",
        steps: [
          {
            id: "private-step",
            name: "homebase_call",
            state: "completed",
            arguments: { proof: "private-step-sentinel" },
            result: { status: "succeeded" },
            job_id: null,
          },
          {
            id: "unopened-step",
            name: "homebase_mcp",
            state: "completed",
            arguments: { proof: "unopened-step-sentinel" },
            result: { status: "succeeded", content: "合成结果".repeat(4096) },
            job_id: null,
          },
        ],
        pending: null,
        usage: [],
        jobs: [],
        artifacts: [],
        runtime: "deepagents",
        runtime_version: "homebase-da-20261113.8",
        resumable: false,
        queue_paused: false,
      };
      p.values.assistant_session_get.messages[119] = {
        id: "row-119",
        role: "assistant",
        content: task.text,
        turn_id: task.id,
      };
      p.values.assistant_session_get.turns = [task];
      p.values.assistant_turn_get = wrongScope
        ? {
            ...task,
            session_id: "other-session",
            state: "awaiting_confirmation",
            pending: {
              id: "do-not-confirm",
              description: "错误来源审批",
              arguments: { proof: "wrong-private-parameters" },
              approved: false,
            },
          }
        : task;
    },
    { wrongScope },
  );
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByText("已完成合成步骤", { exact: true })).toBeVisible();
}

test("execution details only format the step the user has opened", async ({
  page,
}) => {
  await completedTask(page);
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    const stringify = JSON.stringify;
    fixture.formattedStepBodies = 0;
    JSON.stringify = (...args: Parameters<typeof JSON.stringify>) => {
      const input = args[0];
      if (["private-step-sentinel", "unopened-step-sentinel"].includes(input?.parameters?.proof))
        fixture.formattedStepBodies++;
      return stringify(...args);
    };
  });
  expect(
    (await page.evaluate(() => (window as any).__performance.calls)).filter(
      (c: string) => c === "assistant_turn_get",
    ),
  ).toHaveLength(0);
  await expect(page.getByText(/private-step-sentinel/)).toHaveCount(0);
  const details = page.getByText("执行详情 · 2 步", { exact: true });
  const step = page.getByText("调用能力 · 已完成", { exact: true });
  const body = page.getByText(/private-step-sentinel/);
  const unopened = page.getByText(/unopened-step-sentinel/);
  const formats = () => page.evaluate(() => (window as any).__performance.formattedStepBodies);
  const detailReads = () => page.evaluate(() =>
    (window as any).__performance.calls.filter((name: string) =>
      ["assistant_turn_get", "assistant_turn_inspect", "assistant_turn_step_get"].includes(name),
    ),
  );
  await details.click();
  await expect(step).toBeVisible();
  expect(await detailReads()).toEqual(["assistant_turn_inspect"]);
  expect(await formats()).toBe(0);
  await expect(body).toHaveCount(0);
  await step.click();
  await expect(body).toBeVisible();
  await expect(unopened).toHaveCount(0);
  expect(await formats()).toBe(1);
  expect(await detailReads()).toEqual(["assistant_turn_inspect", "assistant_turn_step_get"]);
  await details.click();
  await expect(body).toHaveCount(0);
  await details.click();
  await expect(body).toBeVisible();
  expect(await detailReads()).toEqual(["assistant_turn_inspect", "assistant_turn_step_get"]);
  await step.click();
  await expect(body).toHaveCount(0);
});

test("wrong task-detail scope cannot expose parameters or request confirmation", async ({
  page,
}) => {
  await completedTask(page, true);
  await page.getByText("执行详情 · 2 步", { exact: true }).click();
  await expect(page.getByText(/详情读取失败/)).toBeVisible();
  await expect(page.getByText(/wrong-private-parameters/)).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "确认执行", exact: true }),
  ).toHaveCount(0);
  expect(
    (await page.evaluate(() => (window as any).__performance.calls)).filter(
      (c: string) =>
        ["assistant_confirm", "assistant_send", "assistant_resume"].includes(c),
    ),
  ).toHaveLength(0);
});
