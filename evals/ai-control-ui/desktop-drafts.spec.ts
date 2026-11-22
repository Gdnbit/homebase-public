import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";

async function ready(page: Page, waitForIdentity = true) {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_config_get.settings = { revision: 9, enabled: true, route_id: "openrouter::fixture:free", free_only: true };
    p.values.assistant_sessions_list = ["A", "B"].map(id => ({ id, title: `隔离会话 ${id}`, source: "desktop", message_count: 0 }));
    p.values.assistant_session_get = ({ sessionId }: any) => ({ id: sessionId, source: "desktop", messages: [], turns: [] });
    p.sent = [];
    p.values.assistant_send = ({ request }: any) => { p.sent.push(request); return { session_id: request.session_id ?? "A", turn_id: "original", state: "completed" }; };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  if (!waitForIdentity) return; // Deliberately unreadable-store cases assert their blocked state below.
  // These scenarios target A, except the explicitly seeded protected first-send
  // fixture. Wait for that identity before typing into an editable composer.
  await expect.poll(() => page.evaluate(() => {
    const p = (window as any).__performance;
    return p.desktopStore.get("new")?.pending ? "ready" : p.client.getQueryData(["assistant", "session", "A"])?.pages?.[0]?.frames?.[0]?.id === "A" ? "ready" : "waiting";
  })).toBe("ready");
}
test.beforeEach(async ({ page }) => {
  await page.route("**/*", route => new URL(route.request().url()).hostname === "127.0.0.1" ? route.continue() : route.abort());
});

test("a lost conversation draft save reply preserves newer input without a background retry", async ({ page }) => {
  await page.clock.install();
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const save = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = (args: any) => {
      const result = save(args);
      if (args.request.body.text === "DURABLE FIRST DRAFT") throw Error("ISOLATED_DESKTOP_SAVE_ACK_LOST");
      return result;
    };
  });
  const input = page.getByLabel("发送消息", { exact: true });
  const status = page.getByLabel("草稿保存状态");
  await input.fill("DURABLE FIRST DRAFT");
  await page.clock.runFor(950);
  await expect(status).toContainText("草稿保存或回执需要核对");
  await input.fill("NEWER UNSENT DRAFT");
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.runFor(950);
  const count = () => page.evaluate(() => (window as any).__performance.calls.filter((c: string) => c === "assistant_desktop_save").length);
  expect(await count()).toBe(1);
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(input).toHaveValue("NEWER UNSENT DRAFT");
  expect(await count()).toBe(1);
  await page.getByRole("button", { name: "重试保存或读取", exact: true }).click();
  await expect(status).toContainText("草稿已受保护保存");
  expect(await count()).toBe(2);
  expect(await page.evaluate(() => (window as any).__performance.desktopStore.get("A").body.text)).toBe("NEWER UNSENT DRAFT");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("a failed title-bar close registration is visible and can be retried without reloading the draft", async ({ page }) => {
  await page.addInitScript(() => {
    (window as any).isTauri = true;
    (window as any).__rejectDraftCloseListener = true;
  });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  const warning = page.getByRole("alert").filter({ hasText: "标题栏关闭保护未就绪" });
  await expect(warning).toBeVisible();
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("重试关闭保护时仍在编辑的草稿");
  await page.getByRole("button", { name: "重试关闭保护", exact: true }).click();
  await expect(warning).toHaveCount(0);
  await expect(input).toHaveValue("重试关闭保护时仍在编辑的草稿");
  await expect.poll(() => page.evaluate(() => {
    const p = (window as any).__performance;
    return [p.closeRegistrationAttempts, p.registrations.filter((name: string) => name === "tauri://close-requested").length];
  })).toEqual([2, 1]);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values["plugin:window|destroy"] = () => { p.closedWith = p.desktopStore.get("A")?.body.text; };
    p.emit("tauri://close-requested", null);
  });
  await expect.poll(() => page.evaluate(() => (window as any).__performance.closedWith)).toBe("重试关闭保护时仍在编辑的草稿");
});

const closeScenarios = [
  { outcome: "saved", title: "waits for edits made during the first save" },
  { outcome: "failed", title: "keeps the editor open when the newer draft fails to save" },
  { outcome: "timeout", title: "shares one deadline across both saves" },
] as const;

for (const { outcome, title } of closeScenarios) {
  test(`window close ${title}`, async ({ page }) => {
    if (outcome === "timeout") await page.clock.install();
    await page.addInitScript(() => { (window as any).isTauri = true; });
    await page.goto("/evals/ai-control-ui/performance.html");
    await ready(page);
    await expect.poll(() => page.evaluate(() => (window as any).__performance.registrations.includes("tauri://close-requested"))).toBe(true);
    await page.evaluate((saveOutcome) => {
      const p = (window as any).__performance;
      const save = p.values.assistant_desktop_save;
      p.saveStarted = [];
      p.closedWith = [];
      p.releaseSave = {};
      p.values.assistant_desktop_save = async (args: any) => {
        const text = args.request.body.text;
        if (text === "关闭时原稿" || text === "关闭时最新输入") {
          await new Promise<void>((resolve) => {
            p.releaseSave[text] = resolve;
            p.saveStarted.push(text);
          });
        }
        if (saveOutcome === "failed" && text === "关闭时最新输入") throw Error("隔离保存失败");
        return save(args);
      };
      p.values["plugin:window|destroy"] = () => {
        p.closedWith.push(p.desktopStore.get("A")?.body.text);
        return null;
      };
      p.values["plugin:dialog|message"] = () => "返回核对";
    }, outcome);
    const input = page.getByLabel("发送消息", { exact: true });
    await input.fill("关闭时原稿");
    await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
    await expect.poll(() => page.evaluate(() => (window as any).__performance.saveStarted)).toEqual(["关闭时原稿"]);
    await input.fill("关闭时最新输入");
    if (outcome === "timeout") await page.clock.runFor(6000);
    await page.evaluate(() => (window as any).__performance.releaseSave["关闭时原稿"]());
    await expect.poll(() => page.evaluate(() => (window as any).__performance.saveStarted)).toEqual(["关闭时原稿", "关闭时最新输入"]);
    expect(await page.evaluate(() => (window as any).__performance.closedWith)).toEqual([]);
    if (outcome === "timeout") {
      expect(await page.evaluate(() => (window as any).__performance.calls.includes("plugin:dialog|message"))).toBe(false);
      await page.clock.runFor(4300);
    } else {
      await page.evaluate(() => (window as any).__performance.releaseSave["关闭时最新输入"]());
    }
    if (outcome !== "saved") {
      await expect.poll(() => page.evaluate(() => (window as any).__performance.calls.includes("plugin:dialog|message"))).toBe(true);
      await expect(input).toHaveValue("关闭时最新输入");
      expect(await page.evaluate(() => (window as any).__performance.closedWith)).toEqual([]);
      if (outcome === "timeout") await page.evaluate(() => (window as any).__performance.releaseSave["关闭时最新输入"]());
    } else {
      await expect.poll(() => page.evaluate(() => (window as any).__performance.closedWith)).toEqual(["关闭时最新输入"]);
    }
  });
}

test("window close does not wait for a read of an already saved draft", async ({ page }) => {
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("关闭前已保护保存");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const get = p.values.assistant_desktop_get;
    p.values.assistant_desktop_get = async (args: any) => {
      if (args.scope === "A") await new Promise<void>((resolve) => { p.releaseRead = resolve; });
      return get(args);
    };
    p.values["plugin:window|destroy"] = () => {
      p.closedWith = p.desktopStore.get("A")?.body.text;
      return null;
    };
    p.values["plugin:dialog|message"] = () => "返回核对";
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseRead)).toBe("function");
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  try {
    await expect.poll(() => page.evaluate(() => (window as any).__performance.closedWith), { timeout: 2500 }).toBe("关闭前已保护保存");
    expect(await page.evaluate(() => (window as any).__performance.calls.includes("plugin:dialog|message"))).toBe(false);
    expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
  } finally {
    await page.evaluate(() => (window as any).__performance.releaseRead());
  }
});

test("window close still waits for a send from an already saved draft", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("关窗时等待原受理");
  await page.clock.runFor(1000);
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const send = p.values.assistant_desktop_send;
    p.values.assistant_desktop_send = async (args: any) => {
      await new Promise<void>((resolve) => { p.releaseSend = resolve; });
      return send(args);
    };
    p.values["plugin:window|destroy"] = () => {
      p.closedWith = p.sent.length;
      return null;
    };
    p.values["plugin:dialog|message"] = () => "返回核对";
  });
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseSend)).toBe("function");
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  await page.clock.runFor(500);
  expect(await page.evaluate(() => (window as any).__performance.closedWith)).toBeUndefined();
  await page.evaluate(() => (window as any).__performance.releaseSend());
  await expect.poll(() => page.evaluate(() => (window as any).__performance.closedWith)).toBe(1);
  const sent = await page.evaluate(() => (window as any).__performance.sent);
  expect(sent).toEqual([expect.objectContaining({ session_id: "A", text: "关窗时等待原受理" })]);
});

test("window close waits for a separate draft save before offering to discard a failed draft", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.registrations.includes("tauri://close-requested"))).toBe(true);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const saveDraft = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = async (args: any) => {
      await new Promise<void>((resolve) => { p.releaseOtherDraft = resolve; });
      return saveDraft(args);
    };
    p.values.inbox_workspace_save = () => { throw Error("隔离资料草稿保存失败"); };
    p.values["plugin:dialog|message"] = () => new Promise<string>((resolve) => { p.returnToDraft = resolve; });
  });
  await page.getByLabel("发送消息", { exact: true }).fill("仍在保存的助理草稿");
  await page.clock.runFor(600);
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseOtherDraft)).toBe("function");
  await navigate(page, "投料口");
  const inbox = page.locator("textarea").first();
  await inbox.fill("未能保存的资料草稿");
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  await expect.poll(() => page.evaluate(() => (window as any).__performance.calls.includes("inbox_workspace_save"))).toBe(true);
  await expect(page.getByRole("status").filter({ hasText: "正在核对草稿保存" })).toBeVisible({ timeout: 2000 });
  expect(await page.evaluate(() => (window as any).__performance.calls.includes("plugin:dialog|message"))).toBe(false);

  await page.evaluate(() => (window as any).__performance.releaseOtherDraft());
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.returnToDraft)).toBe("function");
  const calls = await page.evaluate(() => {
    const p = (window as any).__performance;
    p.returnToDraft("返回核对");
    return {
      saved: p.desktopStore.get("A")?.body.text,
      effects: p.calls.filter((name: string) => ["assistant_desktop_save", "plugin:dialog|message", "plugin:window|destroy", "inbox_workspace_submit"].includes(name)),
      sends: p.sent.length,
    };
  });
  expect(calls).toEqual({ saved: "仍在保存的助理草稿", effects: ["assistant_desktop_save", "plugin:dialog|message"], sends: 0 });
  await expect(inbox).toHaveValue("未能保存的资料草稿");
});

test("window close distinguishes a saved draft from a rejected native close", async ({ page }) => {
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.registrations.includes("tauri://close-requested"))).toBe(true);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("已保存的关闭对照稿");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values["plugin:window|destroy"] = () => { throw Error("isolated native close failure"); };
    p.values["plugin:dialog|message"] = () => "返回核对";
    p.emit("tauri://close-requested", null);
  });
  await expect(page.getByRole("alert").filter({ hasText: "草稿已保存，但窗口未能关闭" })).toBeVisible({ timeout: 4000 });
  expect(await page.evaluate(() => (window as any).__performance.calls.includes("plugin:dialog|message"))).toBe(false);
  await expect(input).toHaveValue("已保存的关闭对照稿");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values["plugin:window|destroy"] = () => null;
    p.emit("tauri://close-requested", null);
  });
  await expect.poll(() => page.evaluate(() => (window as any).__performance.calls.filter((command: string) => command === "plugin:window|destroy").length)).toBe(2);
  await expect(page.getByRole("alert").filter({ hasText: "窗口未能关闭" })).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("window close explains an unavailable discard confirmation and keeps the draft", async ({ page }) => {
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.registrations.includes("tauri://close-requested"))).toBe(true);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("旧稿先保存");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.priorCloseSave = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = () => { throw Error("isolated save failure"); };
    p.values["plugin:dialog|message"] = () => { throw Error("isolated native dialog failure"); };
  });
  await input.fill("尚未保存的最新输入");
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  await expect(page.getByRole("alert").filter({ hasText: "无法显示关闭确认" })).toBeVisible({ timeout: 4000 });
  await expect(input).toHaveValue("尚未保存的最新输入");
  const failure = await page.evaluate(() => {
    const p = (window as any).__performance;
    return { persisted: p.desktopStore.get("A").body.text, closed: p.calls.includes("plugin:window|destroy") };
  });
  expect(failure).toEqual({ persisted: "旧稿先保存", closed: false });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_desktop_save = p.priorCloseSave;
    p.values["plugin:window|destroy"] = () => null;
  });
  await page.getByRole("button", { name: "重试保存或读取", exact: true }).click();
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  await expect.poll(() => page.evaluate(() => {
    const p = (window as any).__performance;
    return { saved: p.desktopStore.get("A").body.text, closed: p.calls.includes("plugin:window|destroy"), sent: p.sent.length };
  })).toEqual({ saved: "尚未保存的最新输入", closed: true, sent: 0 });
  await expect(page.getByRole("alert").filter({ hasText: "无法显示关闭确认" })).toHaveCount(0);
});

for (const reply of ["pending", "accepted"] as const) {
  test(`a window still open after a ${reply} native close explains the delay without a duplicate`, async ({ page }) => {
    await page.clock.install();
    await page.addInitScript(() => { (window as any).isTauri = true; });
    await page.goto("/evals/ai-control-ui/performance.html");
    await ready(page);
    const input = page.getByLabel("发送消息", { exact: true });
    await input.fill("窗口退出前已保存的草稿");
    await page.clock.runFor(1000);
    await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");

    await page.evaluate((outcome) => {
      const fixture = (window as any).__performance;
      fixture.values["plugin:window|destroy"] = () => outcome === "accepted" ? null :
        new Promise((_, reject) => { fixture.rejectClose = reject; });
    }, reply);
    const exit = page.getByRole("button", { name: "退出 HomeBase", exact: true });
    await exit.click();
    const closeEffects = () => page.evaluate(() => {
      const fixture = (window as any).__performance;
      return {
        destroys: fixture.calls.filter((name: string) => name === "plugin:window|destroy").length,
        confirmations: fixture.calls.filter((name: string) => name === "plugin:dialog|message").length,
        sends: fixture.sent.length,
        draft: fixture.desktopStore.get("A")?.body.text,
      };
    });
    const original = { destroys: 1, confirmations: 0, sends: 0, draft: "窗口退出前已保存的草稿" };
    await expect.poll(closeEffects).toEqual(original);
    const closing = page.getByRole("dialog", { name: "正在退出 HomeBase", exact: true });
    await expect(closing).toBeVisible();
    expect(await closing.evaluate((dialog) => dialog.matches(":modal"))).toBe(true);
    expect(await exit.evaluate((button) => {
      button.focus();
      return document.activeElement === button;
    })).toBe(false);
    await page.keyboard.press("Escape");
    await page.keyboard.type("退出后输入不能进入草稿");
    await expect(closing).toBeVisible();
    await expect(input).toHaveValue(original.draft);
    await page.clock.fastForward(11_000);
    const delayed = page.getByRole("alert").filter({ hasText: reply === "accepted" ? "退出请求已受理，窗口仍然打开" : "退出请求仍未得到回应" });
    await expect(delayed).toBeVisible({ timeout: 2500 });
    await expect(input).toHaveValue(original.draft);
    await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
    await page.clock.runFor(1000);
    expect(await closeEffects()).toEqual(original);

    const retry = page.getByRole("button", { name: "重试退出", exact: true });
    if (reply === "accepted") {
      await expect(retry).toBeVisible();
      await page.evaluate(() => {
        const fixture = (window as any).__performance;
        fixture.values["plugin:window|destroy"] = () => new Promise((_, reject) => { fixture.rejectRetry = reject; });
      });
      await retry.click();
      await expect(retry).toHaveCount(0);
      await expect(closing).toBeVisible();
      await expect.poll(closeEffects).toEqual({ ...original, destroys: 2 });
      await page.evaluate(() => {
        const fixture = (window as any).__performance;
        fixture.rejectRetry(Error("隔离的退出重试拒绝"));
        fixture.values["plugin:window|destroy"] = () => null;
      });
      await expect(closing.getByRole("alert")).toContainText("仍在等待先前的退出请求");
      expect(await closing.evaluate((dialog) => dialog.matches(":modal"))).toBe(true);
      await retry.click();
      await expect.poll(closeEffects).toEqual({ ...original, destroys: 3 });
    } else {
      await expect(retry).toHaveCount(0);
      await page.evaluate(() => {
        const fixture = (window as any).__performance;
        fixture.rejectClose(Error("隔离的本机关闭拒绝"));
        fixture.values["plugin:window|destroy"] = () => null;
      });
      await expect(page.getByRole("alert").filter({ hasText: "草稿已保存，但窗口未能关闭" })).toBeVisible();
      await expect(closing).toHaveCount(0);
      await expect(delayed).toHaveCount(0);
      expect(await closeEffects()).toEqual(original);
      await exit.click();
      await expect.poll(closeEffects).toEqual({ ...original, destroys: 2 });
    }
  });
}

test("a late acknowledgement gets a full window-close grace period before explicit retry", async ({ page }) => {
  await page.clock.install();
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values["plugin:window|destroy"] = () => new Promise((resolve) => { fixture.ackLateClose = resolve; });
  });
  await page.getByRole("button", { name: "退出 HomeBase", exact: true }).click();
  const destroys = () => page.evaluate(() => (window as any).__performance.calls.filter((name: string) => name === "plugin:window|destroy").length);
  await expect.poll(destroys).toBe(1);
  await page.clock.fastForward(11_000);
  await expect(page.getByRole("alert").filter({ hasText: "退出请求仍未得到回应" })).toBeVisible();
  const retry = page.getByRole("button", { name: "重试退出", exact: true });
  await expect(retry).toHaveCount(0);
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.ackLateClose(null);
    fixture.values["plugin:window|destroy"] = () => null;
  });
  await expect(page.getByRole("alert").filter({ hasText: "退出请求已受理，窗口仍然打开" })).toBeVisible();
  await page.clock.runFor(9000);
  await expect(retry).toHaveCount(0);
  expect(await destroys()).toBe(1);
  await page.clock.runFor(1100);
  await expect(retry).toBeVisible();
  await retry.click();
  await expect.poll(destroys).toBe(2);
});

test("in-app exit shares the draft guard with Windows close and can be retried after canceling", async ({ page }) => {
  await page.addInitScript(() => { (window as any).isTauri = true; });
  await page.goto("/evals/ai-control-ui/performance.html");
  await ready(page);
  await expect.poll(() => page.evaluate(() => (window as any).__performance.registrations.includes("tauri://close-requested"))).toBe(true);
  const exit = page.getByRole("button", { name: "退出 HomeBase", exact: true });
  await expect(exit).toBeVisible({ timeout: 3000 });
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("已保存的旧稿");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.savedExitHandler = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = () => { throw Error("synthetic save failure"); };
    p.values["plugin:dialog|message"] = () => new Promise<string>((resolve) => { p.resolveExit = resolve; });
    p.values["plugin:window|destroy"] = () => { p.closedWith = p.desktopStore.get("A")?.body.text; };
  });
  await input.fill("退出前的最新草稿");
  await exit.click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.resolveExit)).toBe("function");
  await exit.click();
  await page.evaluate(() => (window as any).__performance.emit("tauri://close-requested", null));
  await expect(page.getByRole("alert").filter({ hasText: "部分输入尚未确认保存" })).toBeVisible();
  const closeCalls = () => page.evaluate(() => {
    const p = (window as any).__performance;
    return {
      confirms: p.calls.filter((name: string) => name === "plugin:dialog|message").length,
      exits: p.calls.filter((name: string) => name === "plugin:window|destroy").length,
      closedWith: p.closedWith ?? null,
      sends: p.sent.length,
    };
  });
  expect(await closeCalls()).toEqual({ confirms: 1, exits: 0, closedWith: null, sends: 0 });
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_desktop_save = p.savedExitHandler;
    p.resolveExit("返回核对");
  });
  await expect(page.getByRole("alert").filter({ hasText: "部分输入尚未确认保存" })).toHaveCount(0);
  await expect(input).toHaveValue("退出前的最新草稿");
  await page.getByRole("button", { name: "重试保存或读取", exact: true }).click();
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await exit.click();
  await expect.poll(closeCalls).toEqual({ confirms: 1, exits: 1, closedWith: "退出前的最新草稿", sends: 0 });
});

test("confirmed drafts, attachments and mode survive a process reload without sending", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("跨重启合成草稿");
  await page.getByLabel("选择文本附件").setInputFiles({ name: "synthetic.md", mimeType: "text/markdown", buffer: Buffer.from("隔离附件正文") });
  await page.getByLabel("消息方式").selectOption("task");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  const snapshot = await page.evaluate(() => Object.fromEntries((window as any).__performance.desktopStore));
  expect(snapshot.A.body.mode).toBe("task"); expect(snapshot.A.body.text).toBe("跨重启合成草稿");
  await page.addInitScript(seed => { (window as any).__desktopSeed = seed; }, snapshot);
  await page.reload(); await ready(page);
  await expect(input).toHaveValue("跨重启合成草稿");
  await expect(page.getByLabel("待发送附件")).toContainText("synthetic.md");
  await expect(page.getByLabel("消息方式")).toHaveValue("task");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("typing coalesces saves and navigation reuses the loaded scope", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("一"); await input.fill("一二"); await input.fill("一二三");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  const before = await page.evaluate(() => { const p = (window as any).__performance; return { saves: p.calls.filter((c: string) => c === "assistant_desktop_save").length, reads: p.calls.filter((c: string) => c === "assistant_desktop_get").length }; });
  expect(before.saves).toBe(1);
  await input.fill("一二三");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await expect(page.getByRole("heading", { name: "能力库", exact: true })).toBeVisible();
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await expect(input).toHaveValue("一二三");
  const after = await page.evaluate(() => { const p = (window as any).__performance; return { saves: p.calls.filter((c: string) => c === "assistant_desktop_save").length, reads: p.calls.filter((c: string) => c === "assistant_desktop_get").length }; });
  expect(after).toEqual(before);
});

test("a long Chinese draft survives reload while sending stays within the UTF-8 budget", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input = page.getByLabel("发送消息", { exact: true });
  const long = "中文草稿🙂".repeat(900);
  await input.fill(long);
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await expect(page.locator("#assistant-input-budget")).toContainText("发送上限为 8,000 字节");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("8,000 字节");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
  const snapshot = await page.evaluate(() => Object.fromEntries((window as any).__performance.desktopStore));
  expect(snapshot.A.body.text).toBe(long); expect(snapshot.A.pending).toBeNull();
  await page.addInitScript(seed => { (window as any).__desktopSeed = seed; }, snapshot);
  await page.reload(); await ready(page);
  await expect(input).toHaveValue(long);
  await input.fill("缩短后的可发内容");
  await expect(page.locator("#assistant-input-budget")).toHaveCount(0);
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
});

test("text exceeding protected capacity is explicitly unsaved without replacing the original draft", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("已保存原稿");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_desktop_save;
    p.values.assistant_desktop_save = (args:any) => new TextEncoder().encode(args.request.body.text).length > 65536
      ? { status:"failed",message:"草稿正文超过64 KiB，尚未保存；已保存的原稿保留" } : original(args);
  });
  const long = "中文".repeat(12000);
  await input.fill(long);
  await expect(page.locator("#assistant-input-budget")).toContainText("超过 64 KiB，尚未保存");
  await expect(page.getByLabel("草稿保存状态")).toContainText("需要核对");
  await expect(page.getByRole("alert")).toContainText("尚未保存");
  await expect(input).toHaveValue(long);
  expect(await page.evaluate(() => (window as any).__performance.desktopStore.get("A").body.text)).toBe("已保存原稿");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
  await input.fill("缩短并重新保存");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
});

test("a failed protected prepare blocks execution and retains the editable draft", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  await page.evaluate(() => { (window as any).__performance.values.assistant_desktop_send = () => ({ status: "failed", code: "DRAFT_PREPARE_FAILED", message: "隔离模拟磁盘不可写，未执行。" }); });
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("绝不能丢失"); await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("未执行");
  await expect(input).toHaveValue("绝不能丢失"); await expect(input).toBeEnabled();
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("an unreadable store fails closed instead of loading an empty writable draft", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.evaluate(() => { (window as any).__performance.values.assistant_desktop_get = () => ({ status: "failed", message: "隔离坏密文，原文件保留。" }); });
  await ready(page, false);
  // New scope was already read; the selected desktop scope is the failed read.
  await expect(page.getByRole("alert")).toContainText("坏密文");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeDisabled();
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c: string) => ["assistant_send", "assistant_desktop_save"].includes(c)))).toEqual([]);
});

test("a persisted first send recovers by query and explicit retry keeps its original identity", async ({ page }) => {
  const request = { session_id: null, text: "未知首发", request_id: "original-before-restart", mode: "task" };
  await page.addInitScript(({ request }) => { (window as any).__desktopSeed = { new: { scope: "new", revision: 2, body: { text: request.text, mode: "task", route: "", files: [] }, pending: { request, state: "prepared" } } }; }, { request });
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("未知首发");
  await expect(page.getByLabel("草稿保存状态")).toContainText("尚未查到原请求");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
  await page.getByRole("button", { name: "查询原请求", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([request]);
});

test("a lost response after durable acknowledgement resolves the original and clears only that draft", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_desktop_send;
    p.values.assistant_desktop_send = async (args: any) => { const reply = await original(args); p.accepted = reply.ack; throw Error("合成回执响应丢失"); };
  });
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("受理过的原请求"); await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("响应丢失");
  await page.getByRole("button", { name: "查询原请求", exact: true }).click();
  await expect(input).toHaveValue("");
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
});

test("a remote revision conflict retains local input until explicit adoption", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input = page.getByLabel("发送消息", { exact: true }); await expect(input).toBeEnabled();
  await page.evaluate(() => { (window as any).__performance.desktopStore.set("A", { scope: "A", revision: 8, body: { text: "另一窗口的稿件", route: "", mode: "chat", files: [] }, pending: null }); });
  await input.fill("本窗口还没保存的稿件");
  await expect(page.getByRole("alert")).toContainText("另一个窗口");
  await expect(input).toHaveValue("本窗口还没保存的稿件");
  await page.getByRole("button", { name: "查看已保存版本", exact: true }).click();
  await expect(page.getByLabel("已保存草稿版本")).toContainText("另一窗口的稿件");
  await expect(input).toHaveValue("本窗口还没保存的稿件");
  await page.getByRole("button", { name: "使用已保存版本", exact: true }).click();
  await expect(input).toHaveValue("另一窗口的稿件");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("checking an accepted request preserves a newer unsent edit", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_desktop_send;
    p.values.assistant_desktop_send = async (args: any) => { await original(args); throw Error("合成响应丢失"); };
  });
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("原任务"); await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("响应丢失");
  await input.fill("下一条尚未发送的补充");
  await page.getByRole("button", { name: "查询原请求", exact: true }).click();
  await expect(input).toHaveValue("下一条尚未发送的补充");
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
});

test("the recovery list is lazy, reports unreadable records and never executes on open", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input = page.getByLabel("发送消息", { exact: true }); await input.fill("A 的已保存稿件");
  await expect(page.getByLabel("草稿保存状态")).toContainText("草稿已受保护保存");
  expect(await page.evaluate(() => (window as any).__performance.calls.includes("assistant_desktop_index"))).toBe(false);
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_desktop_index;
    p.values.assistant_desktop_index = () => { const r = original(); r.index.complete = false; r.index.unreadable = 1; return r; };
  });
  await page.getByLabel("更多对话操作").click();
  await page.getByRole("button", { name: "未发送与待核对", exact: true }).click();
  const dialog = page.getByRole("dialog").filter({ has: page.getByRole("heading", { name: "未发送与待核对" }) });
  await expect(dialog).toContainText("有 1 份草稿不可读");
  await dialog.getByRole("button", { name: /隔离会话 A/ }).click();
  await expect(input).toHaveValue("A 的已保存稿件");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("losing focus flushes buffered input without waiting for another navigation", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  await page.getByLabel("发送消息", { exact: true }).fill("切走前的修改");
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await expect.poll(() => page.evaluate(() => (window as any).__performance.desktopStore.get("A")?.body.text)).toBe("切走前的修改");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("a late draft load cannot overwrite the conversation the user returned to", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("A 的输入");
  await page.evaluate(() => {
    const p = (window as any).__performance, original = p.values.assistant_desktop_get;
    p.desktopStore.set("B", { scope: "B", revision: 2, body: { text: "B 的独立草稿", mode: "chat", route: "", files: [] }, pending: null });
    p.values.assistant_desktop_get = (args: any) => args.scope === "B" ? new Promise(resolve => { p.finishB = () => resolve(original(args)); }) : original(args);
  });
  await page.getByRole("link", { name: "隔离会话 B", exact: true }).click();
  await expect(input).toBeDisabled();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishB)).toBe("function");
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await input.fill("A 后来输入的内容");
  await page.evaluate(() => (window as any).__performance.finishB());
  await expect(input).toHaveValue("A 后来输入的内容");
  await page.getByRole("link", { name: "隔离会话 B", exact: true }).click();
  await expect(input).toHaveValue("B 的独立草稿");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

test("a late acknowledgement clears its own draft without stealing the user's new conversation", async ({ page }) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_send = ({ request }: any) => { p.sent.push(request); return new Promise(resolve => { p.finishSend = () => resolve({ session_id: request.session_id, turn_id: "original", state: "completed" }); }); };
  });
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("A 的原消息"); await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishSend)).toBe("function");
  await page.getByRole("link", { name: "隔离会话 B", exact: true }).click();
  await input.fill("B 的新稿件");
  await page.evaluate(() => (window as any).__performance.finishSend());
  await expect(input).toHaveValue("B 的新稿件");
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await expect(input).toHaveValue("");
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
});

test("manual refresh adopts an external saved update but never overwrites a dirty local draft", async ({page}) => {
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
  const input=page.getByLabel("发送消息",{exact:true}); await expect(input).toBeEnabled();
  await page.evaluate(()=>{const p=(window as any).__performance;p.desktopStore.set("A",{scope:"A",revision:3,body:{text:"CLI保存的新稿",route:"",mode:"chat",files:[]},pending:null,voices:[]});});
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await expect(input).toHaveValue("CLI保存的新稿");
  await input.fill("当前窗口尚未保存的内容");
  await page.evaluate(()=>{const p=(window as any).__performance;const entry=p.desktopStore.get("A");entry.revision++;entry.body.text="外部窗口第二版";p.desktopStore.set("A",entry);});
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("另一个窗口");
  await expect(input).toHaveValue("当前窗口尚未保存的内容");
  expect(await page.evaluate(()=>(window as any).__performance.sent)).toEqual([]);
});

test("adopting a saved version cannot discard an unresolved original send", async ({page}) => {
  await page.goto("/evals/ai-control-ui/performance.html");await ready(page);
  await page.evaluate(()=>{const p=(window as any).__performance;p.values.assistant_send=({request}:any)=>{p.sent.push(request);throw Error("合成受理未知");};});
  const input=page.getByLabel("发送消息",{exact:true});await input.fill("必须保留编号的原请求");
  await page.getByRole("button",{name:"发送",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("受理未知");
  await page.getByRole("button",{name:"查看已保存版本",exact:true}).click();
  await page.getByRole("button",{name:"使用已保存版本",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("原请求尚未核对");
  await expect(input).toHaveValue("必须保留编号的原请求");
  expect(await page.evaluate(()=>(window as any).__performance.sent.length)).toBe(1);
  await expect(page.getByRole("button",{name:"查询原请求",exact:true})).toBeVisible();
});
