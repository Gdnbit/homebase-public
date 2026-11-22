import { test, expect, type Page } from "@playwright/test";

async function ready(page: Page, { enabled = true, resumable = false, chat = false, stoppedModelUnknown }: {
  enabled?: boolean;
  resumable?: boolean;
  chat?: boolean;
  stoppedModelUnknown?: boolean;
} = {}) {
  await page.route("**/*", route => new URL(route.request().url()).hostname === "127.0.0.1" ? route.continue() : route.abort());
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.evaluate(({ enabled, resumable, chat, stoppedModelUnknown }) => {
    const p = (window as any).__performance;
    p.values.assistant_config_get.settings.enabled = enabled;
    p.recoveryResumes = []; p.recoveryReads = [];
    const stopped = stoppedModelUnknown !== undefined;
    const turn = { id: "original-recovery-turn", session_id: "recovery-session", state: stopped ? "canceled" : "interrupted_unknown", text: "原任务暂时中断。", runtime: "deepagents", resumable, resume_generation: 0,
      model_outcome_unknown: stoppedModelUnknown,
      interaction: chat ? "chat" : "task", steps: [], pending: null, jobs: [], artifacts: [],
      progress: { goal: "合成恢复任务", completed: ["核对合成资料"], current: null,
        blocked: stopped ? stoppedModelUnknown ? "后续步骤已停下，模型请求的结果仍未确认。" : null : "这次模型响应没有完整确认，原任务和已有结果保留。",
        next: stopped && !stoppedModelUnknown ? null : "先核对原任务和已有结果。", completed_stages: 1, total_stages: 2 } };
    p.recoveryTurn = turn;
    p.values.assistant_sessions_list = [{ id: turn.session_id, title: "隔离任务恢复", source: "desktop", active_turn_id: null }];
    p.values.assistant_session_get = { id: turn.session_id, title: "隔离任务恢复", source: "desktop", messages: [
      { id: "original-recovery-user", role: "user", content: "整理合成资料", turn_id: turn.id },
      { id: "original-recovery-assistant", role: "assistant", content: turn.text, turn_id: turn.id }], turns: [turn] };
    p.values.assistant_turn_get = ({ turnId }: any) => { p.recoveryReads.push(turnId); return turn; };
    p.values.assistant_resume = ({ turnId }: any) => {
      p.recoveryResumes.push(turnId); turn.state = "running"; turn.resumable = false;
      turn.progress = { ...turn.progress, current: "继续核对已有结果", blocked: null as any, next: null as any };
      return { session_id: turn.session_id, turn_id: turnId, state: turn.state };
    };
  }, { enabled, resumable, chat, stoppedModelUnknown });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByText("原任务暂时中断。", { exact: true })).toBeVisible();
  return page.getByRole("region", { name: "任务恢复说明", exact: true });
}

test("interrupted chat shows a concrete read-only next step and querying never resumes or sends", async ({ page }) => {
  const note = await ready(page, { chat: true });
  await expect(note).toContainText("这次模型响应没有完整确认");
  await expect(note).toContainText("先核对原任务");
  await expect(note.getByRole("button", { name: "继续原任务", exact: true })).toHaveCount(0);
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryReads.length)).toBe(1);
  await expect(note.getByRole("status")).toContainText("原记录没有保存步骤");
  const result = await page.evaluate(() => { const p = (window as any).__performance; return { reads: p.recoveryReads, resumes: p.recoveryResumes, calls: p.calls }; });
  expect(result.reads).toEqual(["original-recovery-turn"]); expect(result.resumes).toEqual([]);
  expect(result.calls).not.toContain("assistant_send"); expect(result.calls).not.toContain("assistant_resume");
});

test("a stopped reply with an unknown model outcome offers only the original lookup", async ({ page }) => {
  const note = await ready(page, { chat: true, stoppedModelUnknown: true });
  await expect(note).toContainText("模型请求的结果仍未确认");
  await expect(note.getByRole("button", { name: "继续原任务", exact: true })).toHaveCount(0);
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryReads)).toEqual(["original-recovery-turn"]);
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_send");
  expect(calls).not.toContain("assistant_resume");
});

test("an ordinary stopped reply does not show an unknown model outcome", async ({ page }) => {
  const note = await ready(page, { chat: true, stoppedModelUnknown: false });
  await expect(note).toHaveCount(0);
  await expect(page.getByText("模型请求的结果仍未确认")).toHaveCount(0);
});

test("a closed turn with an unverified plan offers the original lookup and shows the remaining phase", async ({ page }) => {
  const note = await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const turn = p.recoveryTurn;
    turn.state = "completed";
    turn.progress.blocked = "「保存合成结果」的完成结果待核对。";
    turn.progress.next = "先核对原任务的状态和已有结果，再决定后续步骤。";
    turn.task = { stop: null, limits: { model_calls_used: 2, max_model_calls: 8,
      steps_used: 0, max_steps: 192, active_seconds: 1, max_active_seconds: 1800 },
      plan: { revision: 2, goal: "整理合成资料", items: [
      { id: "inspect", title: "核对合成资料", state: "completed", done_when: "已取得原记录", evidence: ["known"] },
      { id: "write", title: "保存合成结果", state: "blocked", done_when: "已核对写入回执",
        note: "原完成凭据还未核对", evidence: ["unconfirmed"] },
    ] } };
    p.values.assistant_turn_inspect = ({ turnId }: { turnId: string }) => {
      p.recoveryReads.push(turnId);
      return { ...turn, details_omitted: true, step_count: 0, step_summaries: [] };
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByText("结果待核对", { exact: true })).toBeVisible();
  await expect(note).toContainText("「保存合成结果」的完成结果待核对");
  await expect(note.getByRole("button", { name: "继续原任务", exact: true })).toHaveCount(0);
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(note.getByRole("status")).toContainText("已核对原记录");
  expect(await page.evaluate(() => (window as any).__performance.recoveryReads)).toEqual(["original-recovery-turn"]);
  await page.getByText("执行详情 · 0 步", { exact: true }).click();
  const plan = page.getByLabel("任务计划", { exact: true });
  await expect(plan).toContainText("核对合成资料");
  await expect(plan).toContainText("原完成凭据还未核对");
  const result = await page.evaluate(() => {
    const p = (window as any).__performance;
    return p.calls;
  });
  expect(result).not.toContain("assistant_resume");
  expect(result).not.toContain("assistant_send");
});

test("original evidence after a stopped unknown turn opens only the chosen saved step", async ({ page }) => {
  const note = await ready(page, { chat: true, stoppedModelUnknown: true });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const turn = p.recoveryTurn;
    p.evidenceInspections = [];
    p.evidenceReads = [];
    const summaries = [
      { id: "retained-step", name: "homebase_discover", state: "completed" },
      { id: "unread-step", name: "homebase_mcp", state: "failed" },
    ];
    p.values.assistant_turn_inspect = ({ sessionId, turnId }: { sessionId: string; turnId: string }) => {
      p.evidenceInspections.push([sessionId, turnId]);
      return { ...turn, details_omitted: true, step_count: summaries.length, step_summaries: summaries, steps: [] };
    };
    p.values.assistant_turn_step_get = ({ sessionId, turnId, stepId }: { sessionId: string; turnId: string; stepId: string }) => {
      p.evidenceReads.push([sessionId, turnId, stepId]);
      const result = p.evidenceReads.length === 1 ? "其他会话的合成结果" : "已保存的合成能力发现结果";
      return {
        session_id: p.evidenceReads.length === 1 ? "different-session" : sessionId,
        turn_id: turnId,
        step: { ...summaries.find(step => step.id === stepId), arguments: {}, result, job_id: null },
      };
    };
  });
  await expect(note.getByText("原步骤记录 · 2 步", { exact: true })).toHaveCount(0);
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(note.getByRole("status")).toContainText("已核对原记录");
  await expect(note).toContainText("模型请求的结果仍未确认");
  await note.getByText("原步骤记录 · 2 步", { exact: true }).click();
  const step = note.getByText("查找可用能力 · 已完成", { exact: true });
  await expect(step).toBeVisible();
  const reads = () => page.evaluate(() => (window as any).__performance.evidenceReads);
  expect(await reads()).toEqual([]);
  await step.click();
  await expect(note.getByRole("alert")).toContainText("步骤来源不匹配");
  await expect(note.getByText("其他会话的合成结果", { exact: true })).toHaveCount(0);
  await note.getByRole("button", { name: "重试读取步骤" }).click();
  const result = note.locator("pre").filter({ hasText: "已保存的合成能力发现结果" });
  await expect(result).toBeVisible();
  await step.click();
  await expect(result).toHaveCount(0);
  await step.click();
  await expect(result).toBeVisible();
  expect(await reads()).toEqual(Array(2).fill(["recovery-session", "original-recovery-turn", "retained-step"]));
  const observed = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { inspections: p.evidenceInspections, writes: p.calls.filter((call: string) => ["assistant_send", "assistant_resume", "assistant_confirm"].includes(call)) };
  });
  expect(observed).toEqual({ inspections: [["recovery-session", "original-recovery-turn"]], writes: [] });
});

test("a stopped wait offers the original lookup even when its pending job is beyond the visible summary", async ({ page }) => {
  const note = await ready(page, { stoppedModelUnknown: false });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.recoveryTurn.pending_accepted_jobs = 1;
    p.recoveryTurn.jobs = Array.from({ length: 8 }, (_, index) => ({ id: `view-only-${index}`, command: "capability_render", state: "succeeded", owned: false }));
    const original = p.values.assistant_turn_get;
    p.values.assistant_turn_get = (args: { turnId: string }) => {
      const turn = original(args);
      turn.pending_accepted_jobs = 0;
      turn.artifacts = [{ id: "original-completed-artifact", job_id: "original-accepted-job", name: "stopped-original.txt", bytes: 9, sha256: "b".repeat(64), mime_type: "text/plain" }];
      return turn;
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(note).toContainText("还有 1 项原任务尚未结束", { timeout: 3500 });
  await expect(note.getByRole("button", { name: "继续原任务", exact: true })).toHaveCount(0);
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(page.getByText("stopped-original.txt", { exact: true })).toBeVisible();
  await expect(note).toHaveCount(0);
  const observed = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { reads: p.recoveryReads, dispatched: p.calls.filter((call: string) => ["assistant_resume", "assistant_send"].includes(call)) };
  });
  expect(observed).toEqual({ reads: ["original-recovery-turn"], dispatched: [] });
});

test("disabled models keep recovery readable and block continuation without enabling configuration", async ({ page }) => {
  const note = await ready(page, { enabled: false, resumable: true });
  await expect(note.getByRole("button", { name: "继续原任务", exact: true })).toBeDisabled();
  await expect(note).toContainText("模型尚未启用");
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryReads.length)).toBe(1);
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_config_save"); expect(calls).not.toContain("assistant_resume"); expect(calls).not.toContain("assistant_send");
});

test("explicit continuation uses the original turn exactly once and never creates a new request", async ({ page }) => {
  const note = await ready(page, { resumable: true });
  await note.getByRole("button", { name: "继续原任务", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual(["original-recovery-turn"]);
  await expect(note).not.toBeVisible();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_send"); expect(calls).not.toContain("assistant_session_create");
});

test("a failed original query preserves the explanation and draft and allows an explicit read retry", async ({ page }) => {
  const note = await ready(page);
  await page.getByLabel("发送消息", { exact: true }).fill("稍后再处理的合成草稿🙂");
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_turn_get;
    p.values.assistant_turn_get = (args: any) => { if (!p.failedRecoveryRead) { p.failedRecoveryRead = true; throw Error("合成原任务读取失败"); } return original(args); };
  });
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("合成原任务读取失败");
  await expect(note).toContainText("原任务和已有结果保留");
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("稍后再处理的合成草稿🙂");
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual([]);
});

test("a partially verified query shows the confirmed result and can retry the unavailable task", async ({ page }) => {
  const note = await ready(page);
  await page.getByLabel("发送消息", { exact: true }).fill("继续保留的草稿");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const read = p.values.assistant_turn_get;
    p.values.assistant_turn_get = (args: { turnId: string }) => {
      const turn = read(args);
      const unverified = p.recoveryReads.length === 1 ? 1 : 0;
      return { ...turn, unverified_accepted_jobs: unverified, pending_accepted_jobs: unverified, artifacts: [{
        id: "verified-original-output", job_id: "finished-original", name: "verified-original.txt",
        bytes: 17, sha256: "e".repeat(64), mime_type: "text/plain",
      }] };
    };
  });
  const query = note.getByRole("button", { name: "核对原任务", exact: true });
  await query.click();
  const warning = note.getByRole("status");
  await expect(warning).toContainText("1 项原任务暂未查到最新状态");
  await expect(page.getByText("verified-original.txt", { exact: true })).toBeVisible();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("继续保留的草稿");
  await query.click();
  await expect(warning).toContainText("已核对原记录");
  await expect(warning).not.toContainText("暂未查到最新状态");
  await expect(page.getByText("verified-original.txt", { exact: true })).toBeVisible();
  const effects = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { reads: p.recoveryReads, sent: p.calls.includes("assistant_send"), resumed: p.recoveryResumes };
  });
  expect(effects).toEqual({ reads: ["original-recovery-turn", "original-recovery-turn"], sent: false, resumed: [] });
});

test("a lost continuation ACK queries the original state and never offers a second execution", async ({ page }) => {
  const note = await ready(page, { resumable: true });
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_resume;
    p.values.assistant_resume = (args: any) => { original(args); throw Error("合成续接回执丢失"); };
  });
  await note.getByRole("button", { name: "继续原任务", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("合成续接回执丢失");
  await expect(note).not.toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual(["original-recovery-turn"]);
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("assistant_send");
});

test("a lost confirmation ACK reloads only the original task and removes the processed prompt", async ({ page }) => {
  await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const turn = p.recoveryTurn;
    turn.state = "awaiting_confirmation";
    turn.text = "";
    turn.progress = null;
    turn.pending = { id: "original-confirmation", description: "核对合成操作", arguments: {}, approved: false };
    p.confirmationCalls = [];
    p.values.assistant_confirm = (args: unknown) => {
      p.confirmationCalls.push(args);
      turn.state = "completed";
      turn.pending = null;
      turn.text = "原操作已完成。";
      throw Error("合成确认回执丢失");
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const confirm = page.getByRole("button", { name: "确认执行", exact: true });
  await expect(confirm).toBeVisible();
  await page.evaluate(() => { (window as any).__performance.calls.length = 0; });
  await confirm.click();
  await expect(page.getByRole("alert")).toContainText("合成确认回执丢失");
  const result = await page.evaluate(() => {
    const { calls, confirmationCalls } = (window as any).__performance;
    return { calls: calls as string[], confirmationCalls };
  });
  expect(result.calls).toContain("assistant_session_window_get");
  expect(result.calls).not.toContain("assistant_desktop_get");
  expect(result.calls).not.toContain("assistant_send");
  expect(result.confirmationCalls).toEqual([{ turnId: "original-recovery-turn", confirmationId: "original-confirmation", approve: true }]);
  await expect(page.getByText("原操作已完成。", { exact: true })).toBeVisible();
  await expect(confirm).not.toBeVisible();
});

test("continuation binds the observed generation when the original task has already paused again", async ({ page }) => {
  const note = await ready(page, { resumable: true });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.resumeRequests = [];
    p.values.assistant_resume = (args: { turnId: string; expectedGeneration?: number }) => {
      p.resumeRequests.push(args);
      const turn = p.recoveryTurn;
      turn.resume_generation = 1;
      if (args.expectedGeneration === undefined || args.expectedGeneration === turn.resume_generation) {
        p.recoveryResumes.push(args.turnId);
        turn.state = "running";
        turn.resumable = false;
      }
      return { session_id: turn.session_id, turn_id: turn.id, state: turn.state };
    };
  });
  const button = note.getByRole("button", { name: "继续原任务", exact: true });
  await button.click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.resumeRequests)).toEqual([
    { turnId: "original-recovery-turn", expectedGeneration: 0 },
  ]);
  await expect(button).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual([]);
  await button.click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.resumeRequests)).toEqual([
    { turnId: "original-recovery-turn", expectedGeneration: 0 },
    { turnId: "original-recovery-turn", expectedGeneration: 1 },
  ]);
  await expect(note).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual(["original-recovery-turn"]);
});

test("a mismatched task query never displays another session or changes the original recovery", async ({ page }) => {
  const note = await ready(page);
  await page.evaluate(() => { const p = (window as any).__performance; p.values.assistant_turn_get = () => ({ ...p.recoveryTurn, session_id: "foreign-session", text: "其他会话不可显示" }); });
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("来源不匹配");
  await expect(page.getByText("其他会话不可显示", { exact: true })).toHaveCount(0);
  await expect(note).toContainText("先核对原任务");
  expect(await page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual([]);
});

test("reading recovery never locks typing and a late response cannot switch back from a new draft", async ({ page }) => {
  const note = await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_turn_get;
    p.values.assistant_turn_get = (args: any) => new Promise(resolve => { p.releaseRecoveryRead = () => resolve(original(args)); });
  });
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseRecoveryRead)).toBe("function");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeEnabled();
  await page.getByLabel("发送消息", { exact: true }).fill("查询期间也能继续写");
  await page.getByRole("link", { name: "新对话", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toBeEnabled();
  await page.getByLabel("发送消息", { exact: true }).fill("新对话的独立草稿🙂");
  await page.evaluate(() => (window as any).__performance.releaseRecoveryRead());
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryReads.length)).toBe(1);
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("新对话的独立草稿🙂");
  await expect(note).toHaveCount(0);
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_send"); expect(calls).not.toContain("assistant_resume");
});

test("a late stopped snapshot cannot replace a newer running event or stop status polling", async ({ page }) => {
  const note = await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_turn_get = ({ turnId }: any) => {
      p.recoveryReads.push(turnId);
      if (p.recoveryReads.length === 1) {
        const stopped = structuredClone(p.recoveryTurn);
        return new Promise(resolve => { p.releaseStoppedSnapshot = () => resolve(stopped); });
      }
      return p.recoveryTurn;
    };
  });
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseStoppedSnapshot)).toBe("function");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.recoveryTurn.state = "running";
    p.recoveryTurn.progress = { ...p.recoveryTurn.progress, current: "按新进展核对结果", blocked: null, next: null };
    p.values.assistant_sessions_list[0].active_turn_id = p.recoveryTurn.id;
    p.emit("assistant:changed", { session_id: p.recoveryTurn.session_id, turn_id: p.recoveryTurn.id, config: false });
  });
  await expect(note).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryReads.length)).toBeGreaterThanOrEqual(2);
  await page.evaluate(() => (window as any).__performance.releaseStoppedSnapshot());
  // Allow the delayed continuation to commit, then require the actual live
  // state to remain visible and its bounded polling to keep observing progress.
  await page.waitForTimeout(300);
  await expect(note).toHaveCount(0);
  await expect(page.getByText("正在做：按新进展核对结果", { exact: true })).toBeVisible();
  const before = await page.evaluate(() => (window as any).__performance.recoveryReads.length);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryReads.length), { timeout: 6000 }).toBeGreaterThan(before);
  expect(await page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual([]);
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("assistant_send");
});

test("checking an expanded stopped task reads its details once without a second identical reload", async ({ page }) => {
  const note = await ready(page);
  await page.getByText("执行详情 · 0 步", { exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.recoveryReads.length)).toBe(1);
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(note.getByRole("button", { name: "核对原任务", exact: true })).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__performance.recoveryReads.length)).toBe(2);
  expect(await page.evaluate(() => (window as any).__performance.recoveryResumes)).toEqual([]);
});

test("checking a stopped task exposes the original artifact without clearing the unknown model outcome", async ({ page }) => {
  const note = await ready(page, { stoppedModelUnknown: true });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const original = p.values.assistant_turn_get;
    p.values.assistant_turn_get = (args: { turnId: string }) => {
      const turn = original(args);
      turn.artifacts = [{ id: "original-artifact", job_id: "accepted-job", name: "original-result.txt", bytes: 9, sha256: "a".repeat(64), mime_type: "text/plain" }];
      return turn;
    };
  });
  await expect(page.getByText("original-result.txt", { exact: true })).toHaveCount(0);
  await note.getByRole("button", { name: "核对原任务", exact: true }).click();
  await expect(page.getByText("original-result.txt", { exact: true })).toBeVisible();
  await expect(note).toContainText("模型请求的结果仍未确认");
  const observed = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { reads: p.recoveryReads, dispatched: p.calls.filter((call: string) => ["assistant_resume", "assistant_send"].includes(call)) };
  });
  expect(observed).toEqual({ reads: ["original-recovery-turn"], dispatched: [] });
});

test("active inspection reads one requested step and preserves the live confirmation", async ({ page }) => {
  await page.clock.install();
  await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const turn = p.recoveryTurn;
    turn.state = "running";
    turn.progress = { ...turn.progress, current: "核对长任务结果", blocked: null, next: null };
    turn.steps = [
      { id: "long-step", name: "homebase_call", state: "completed", arguments: {},
        result: { text: "隔离大结果".repeat(45000) }, job_id: null },
      { id: "other-step", name: "homebase_video", state: "completed", arguments: {},
        result: { text: "隔离另一条结果" }, job_id: null },
    ];
    p.turnReadModes = [];
    p.stepRequests = [];
    p.values.assistant_turn_get = ({ includeDetails }: { includeDetails?: boolean }) => {
      p.turnReadModes.push(includeDetails === false ? "progress" : "full");
      if (includeDetails !== false) return turn;
      return { ...turn, details_omitted: true, step_count: turn.steps.length, steps: [], jobs: [] };
    };
    p.values.assistant_turn_inspect = ({ sessionId, turnId }: any) => {
      p.turnReadModes.push("inspection");
      return { ...turn, id: turnId, session_id: sessionId, steps: [], jobs: [],
        details_omitted: true, step_count: turn.steps.length,
        step_summaries: turn.steps.map(({ id, name, state }: any) => ({ id, name, state })) };
    };
    p.values.assistant_turn_step_get = ({ sessionId, turnId, stepId }: any) => {
      p.stepRequests.push(stepId);
      if (p.stepRequests.length === 1)
        return { session_id: "other-session", turn_id: turnId,
          step: { ...turn.steps[0], result: { text: "其他会话正文不可显示" } } };
      return { session_id: sessionId, turn_id: turnId, step: turn.steps.find((s: any) => s.id === stepId) };
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByText("正在做：核对长任务结果", { exact: true })).toBeVisible();
  const modes = () => page.evaluate(() => (window as any).__performance.turnReadModes as string[]);
  await expect.poll(modes).toContain("progress");
  expect(await modes()).not.toContain("full");
  await page.getByText("执行详情 · 2 步", { exact: true }).click();
  const step = page.getByText("调用能力 · 已完成", { exact: true });
  await expect(step).toBeVisible();
  const stepRequests = () => page.evaluate(() => (window as any).__performance.stepRequests as string[]);
  expect(await stepRequests()).toEqual([]);
  expect((await modes()).filter(x => x === "inspection")).toHaveLength(1);
  expect(await modes()).not.toContain("full");
  await step.click();
  await expect(page.getByText(/步骤来源不匹配/)).toBeVisible();
  await expect(page.getByText("其他会话正文不可显示", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "重试读取步骤" }).click();
  const stepBody = page.locator("details pre").filter({ hasText: "隔离大结果" });
  await expect(stepBody).toBeVisible();
  await step.click();
  await expect(stepBody).toHaveCount(0);
  await step.click();
  await expect(stepBody).toBeVisible();
  expect(await stepRequests()).toEqual(["long-step", "long-step"]);
  const previous = (await modes()).length;
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.recoveryTurn.state = "awaiting_confirmation";
    p.recoveryTurn.pending = {
      id: "original-confirmation", description: "检查原文件", arguments: { name: "合成文件" }, approved: false,
    };
  });
  await page.clock.fastForward(2600);
  await expect.poll(async () => (await modes()).length).toBeGreaterThan(previous);
  await expect(page.getByText("需要你的确认：检查原文件", { exact: true })).toBeVisible();
  await expect(page.getByText(/合成文件/)).toBeVisible();
  expect((await modes()).filter(x => x === "inspection")).toHaveLength(1);
  expect(await modes()).not.toContain("full");
  expect(await stepRequests()).toEqual(["long-step", "long-step"]);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const turn = p.recoveryTurn;
    turn.steps[0] = { ...turn.steps[0], state: "failed", result: { text: "隔离步骤更新后的结果" } };
    p.emit("assistant:changed", { session_id: turn.session_id, turn_id: turn.id, config: false });
  });
  await page.clock.fastForward(450);
  await expect(page.locator("details pre").filter({ hasText: "隔离步骤更新后的结果" })).toBeVisible();
  await expect(page.getByText("调用能力 · 执行失败", { exact: true })).toBeVisible();
  expect(await stepRequests()).toEqual(["long-step", "long-step", "long-step"]);
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("assistant_confirm");
});

async function slowProgress(page: Page) {
  await page.clock.install();
  await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.recoveryTurn.state = "running";
    p.recoveryTurn.progress = { ...p.recoveryTurn.progress, current: "准备合成资料", blocked: null, next: null };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByText("正在做：准备合成资料", { exact: true })).toBeVisible();
  const initial = await page.evaluate(() => {
    const p = (window as any).__performance;
    p.progressReadReleases = [];
    p.values.assistant_turn_get = ({ turnId }: { turnId: string }) => {
      p.recoveryReads.push(turnId);
      const snapshot = structuredClone(p.recoveryTurn);
      return new Promise(resolve => p.progressReadReleases.push(() => resolve(snapshot)));
    };
    return p.calls.filter((name: string) => name === "assistant_session_window_get").length;
  });
  async function progress(stage: string) {
    await page.evaluate(stage => {
      const p = (window as any).__performance;
      p.recoveryTurn.progress.current = stage;
      for (let count = 0; count < 8; count++) p.emit("assistant:changed", {
        session_id: p.recoveryTurn.session_id, turn_id: p.recoveryTurn.id, config: false,
      });
    }, stage);
    await page.clock.runFor(450);
  }
  const reads = () => page.evaluate(() => (window as any).__performance.progressReadReleases.length);
  const release = (index: number) => page.evaluate(index => (window as any).__performance.progressReadReleases[index](), index);
  return { progress, reads, release, initial };
}

test("slow progress reads publish in order and coalesce newer events into one follow-up", async ({ page }) => {
  const { progress, reads, release, initial } = await slowProgress(page);
  await progress("核对第一批合成资料");
  await expect.poll(reads).toBe(1);
  await progress("核对第二批合成资料");
  await progress("生成最后的合成摘要");
  expect(await reads()).toBe(1);
  await release(0);
  await expect(page.getByText("正在做：核对第一批合成资料", { exact: true })).toBeVisible();
  await expect.poll(reads).toBe(2);
  await release(1);
  await expect(page.getByText("正在做：生成最后的合成摘要", { exact: true })).toBeVisible();
  expect(await reads()).toBe(2);
  const calls = await page.evaluate(() => (window as any).__performance.calls as string[]);
  expect(calls.filter(name => name === "assistant_session_window_get")).toHaveLength(initial);
  expect(calls).not.toContain("assistant_resume");
  expect(calls).not.toContain("assistant_send");
});

test("slow progress stays stale off-page and refreshes only on return", async ({ page }) => {
  const { progress, reads, release } = await slowProgress(page);
  await progress("正在整理第一批资料");
  await expect.poll(reads).toBe(1);
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveCount(0);
  await progress("后台整理完合成摘要");
  await release(0);
  await expect.poll(() => page.evaluate(() => {
    const p = (window as any).__performance;
    const state = p.client.getQueryState(["assistant", "turn", p.recoveryTurn.id, p.recoveryTurn.session_id, "progress"]);
    return { fetch: state?.fetchStatus, stale: state?.isInvalidated };
  })).toEqual({ fetch: "idle", stale: true });
  expect(await reads()).toBe(1);
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await expect.poll(reads).toBe(2);
  await release(1);
  await expect(page.getByText("正在做：后台整理完合成摘要", { exact: true })).toBeVisible();
});

test("a visible dialogue streams alongside a task and can stop only that reply", async ({ page }) => {
  await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const task = p.recoveryTurn;
    task.state = "running";
    task.progress = { ...task.progress, current: "并行运行原任务", blocked: null, next: null };
    const chat = {
      ...task,
      id: "original-streaming-chat",
      interaction: "chat",
      progress: null,
      text: "等待回复",
      live_progress: { state: "dialogue_stream", output: "合成增量第一段" } as { state: string; output: string } | undefined,
    };
    p.streamingChat = chat;
    p.values.assistant_session_get.turns.push(chat);
    p.values.assistant_session_get.messages.push({
      id: "chat-placeholder", role: "assistant", turn_id: chat.id, content: chat.text,
    });
    p.values.assistant_turn_get = ({ turnId }: { turnId: string }) => (turnId === chat.id ? chat : task);
    p.values.assistant_cancel = ({ turnId }: { turnId: string }) => {
      p.canceledReply = turnId;
      chat.live_progress = undefined;
      chat.state = "canceled";
      chat.text = "合成回复已停止";
      return null;
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByText("合成增量第一段", { exact: true })).toBeVisible();
  await expect(page.getByText("正在做：并行运行原任务", { exact: true })).toBeVisible();
  const reads = await page.evaluate(() => (window as any).__performance.calls.filter((name: string) => name === "assistant_session_window_get").length);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const chat = p.streamingChat;
    chat.live_progress.output = "合成增量第一段和第二段";
    p.emit("assistant:preview", { session_id: chat.session_id, turn_id: chat.id });
  });
  await expect(page.getByText("合成增量第一段和第二段", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((name: string) => name === "assistant_session_window_get").length)).toBe(reads);
  await page.getByRole("button", { name: "停止回复", exact: true }).click();
  await expect(page.getByText("合成回复已停止", { exact: true })).toBeVisible();
  await expect(page.getByText("合成增量第一段和第二段", { exact: true })).toHaveCount(0);
  await expect(page.getByText("正在做：并行运行原任务", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.canceledReply)).toBe("original-streaming-chat");
});

test("task fragments preserve real progress and disappear after the original stream fails", async ({ page }) => {
  await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const task = p.recoveryTurn;
    task.state = "running";
    task.text = "";
    task.progress = { ...task.progress, current: "核对原任务资料", blocked: null, next: null };
    const chat = { ...task, id: "older-concurrent-dialogue", interaction: "chat", progress: null, text: "并行对话" };
    p.values.assistant_session_get.turns.unshift(chat);
    p.values.assistant_session_get.messages.unshift({ id: "earlier-chat", role: "assistant", turn_id: chat.id, content: chat.text });
    p.values.assistant_turn_get = ({ turnId }: { turnId: string }) => (turnId === chat.id ? chat : task);
    task.live_progress = { state: "task_stream", output: "这段内容仍在接收。" };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const fragment = page.getByLabel("模型回复片段", { exact: true });
  await expect(fragment).toContainText("这段内容仍在接收。");
  await expect(page.getByText("正在做：核对原任务资料", { exact: true })).toBeVisible();
  await expect(page.getByText("并行对话", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.calls.length = 0;
    p.recoveryTurn.live_progress.output += "第二段也已收到。";
    p.emit("assistant:preview", { session_id: p.recoveryTurn.session_id, turn_id: p.recoveryTurn.id });
  });
  await expect(fragment).toContainText("第二段也已收到。");
  const calls: string[] = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_session_window_get");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await expect(fragment).toHaveCount(0);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const task = p.recoveryTurn;
    task.state = "failed";
    task.live_progress = undefined;
    task.text = "这轮模型回复中断，原任务已保留。";
    p.emit("assistant:changed", { session_id: task.session_id, turn_id: task.id, config: false });
  });
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await expect(fragment).toHaveCount(0);
  await expect(page.getByText("这轮模型回复中断，原任务已保留。", { exact: true }).first()).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("assistant_send");
});
