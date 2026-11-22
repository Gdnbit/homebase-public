import { test, expect, type Page } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.clock.install({ time: new Date("2026-11-08T04:00:00Z") });
  await page.goto("/evals/secret-lifecycle-ui/index.html", {
    waitUntil: "domcontentloaded",
  });
  await page.clock.runFor(3000);
  await expect(
    page.getByText("隔离测试数据 · 无真实密钥或服务连接"),
  ).toBeVisible();
});
const row = (page: Page, name = "github_token") =>
  page.locator(`[data-secret-name="${name}"]`);
async function expand(page: Page, name = "github_token") {
  await row(page, name).locator("button[aria-expanded]").click();
}
async function writes(page: Page) {
  return page.evaluate(() =>
    (window as any).__fixture.calls.filter(
      (c: any) => c.command === "secrets_save",
    ),
  );
}

async function exitCalls(page: Page) {
  return page.evaluate(() => (window as any).__fixture.calls.filter((call: { command: string }) =>
    ["plugin:dialog|message", "plugin:window|destroy"].includes(call.command)));
}

for (const editor of ["existing", "new"] as const) {
  test(`app exit protects unsaved ${editor} credential input`, async ({ page }) => {
    const secret = "synthetic-unsaved-exit-only";
    let input;
    if (editor === "existing") {
      await expand(page);
      input = row(page).getByLabel("新密钥值", { exact: true });
    } else {
      await page.getByRole("button", { name: "添加密钥", exact: true }).click();
      await page.getByText("自定义服务", { exact: true }).click();
      input = page.getByRole("textbox", { name: /^密钥值/ });
    }
    await input.fill(secret);
    await expect.poll(() => page.evaluate(() => (window as any).__secretClose.ready())).toBe(true);
    await page.evaluate(() => (window as any).__secretClose.system());
    await expect.poll(async () => (await exitCalls(page)).map((call: { command: string }) => call.command))
      .toEqual(["plugin:dialog|message"]);
    expect(JSON.stringify(await exitCalls(page))).not.toContain(secret);
    await expect(input).toHaveValue(secret);
    expect(await writes(page)).toHaveLength(0);
    await page.evaluate(() => {
      (window as any).__fixture.closeChoice = true;
      return (window as any).__secretClose.request();
    });
    expect((await exitCalls(page)).map((call: { command: string }) => call.command))
      .toEqual(["plugin:dialog|message", "plugin:dialog|message", "plugin:window|destroy"]);
    expect(await writes(page)).toHaveLength(0);
  });
}

test("app exit keeps the window open during an encrypted credential write", async ({ page }) => {
  await expand(page, "empty_key");
  const editor = page.getByRole("dialog", { name: "空值隔离测试", exact: true });
  await editor.getByLabel("密钥值", { exact: true }).fill("synthetic-inflight-exit-only");
  await page.evaluate(() => { (window as any).__fixture.delay = 1000; });
  await editor.getByRole("button", { name: "加密保存密钥", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__secretClose.ready())).toBe(true);
  await page.evaluate(() => (window as any).__secretClose.request());
  await expect(page.getByTestId("exit-notice")).toContainText("密钥保存仍在等待确认");
  expect(await exitCalls(page)).toHaveLength(0);
  await page.clock.runFor(3500);
  await expect(editor.getByRole("button", { name: "关闭密钥编辑", exact: true })).toBeEnabled();
  await page.evaluate(() => {
    (window as any).__fixture.delay = 0;
    return (window as any).__secretClose.request();
  });
  expect((await exitCalls(page)).map((call: { command: string }) => call.command))
    .toEqual(["plugin:window|destroy"]);
  expect(await writes(page)).toHaveLength(1);
});

test("app exit rechecks a metadata save that starts while the discard prompt is open", async ({ page }) => {
  await expand(page);
  await page.clock.pauseAt(new Date(await page.evaluate(() => Date.now()) + 100));
  await row(page).getByLabel("显示名称", { exact: true }).fill("合成密钥元数据编辑");
  await page.evaluate(() => {
    const fixture = (window as any).__fixture;
    fixture.delay = 1000;
    fixture.closeChoice = true;
    fixture.closeDecision = new Promise<void>((resolve) => { fixture.resolveCloseDecision = resolve; });
    void (window as any).__secretClose.request();
  });
  await expect.poll(async () => (await exitCalls(page)).length).toBe(1);
  await page.clock.runFor(900);
  expect(await writes(page)).toHaveLength(1);
  await page.evaluate(() => (window as any).__fixture.resolveCloseDecision());
  await expect(page.getByTestId("exit-notice")).toContainText("密钥保存仍在等待确认");
  expect((await exitCalls(page)).map((call: { command: string }) => call.command)).toEqual(["plugin:dialog|message"]);
  await page.clock.runFor(3400);
  await expect(row(page).getByRole("status").last()).toContainText("元数据已保存");
  expect(await writes(page)).toHaveLength(1);
});

test("a pending native exit defers vault autosave until explicit rejection", async ({ page }) => {
  await expand(page);
  await page.clock.pauseAt(new Date(await page.evaluate(() => Date.now()) + 100));
  const name = row(page).getByLabel("显示名称", { exact: true });
  await name.fill("隔离退出后不应保存的元数据");
  await page.evaluate(() => {
    const fixture = (window as any).__fixture;
    fixture.closeChoice = true;
    fixture.closeResult = new Promise<void>((_, reject) => { fixture.rejectNativeClose = reject; });
    void (window as any).__secretClose.request();
  });
  await expect.poll(async () => (await exitCalls(page)).map((call: { command: string }) => call.command))
    .toEqual(["plugin:dialog|message", "plugin:window|destroy"]);
  await page.clock.runFor(1200);
  expect(await writes(page)).toHaveLength(0);
  await page.evaluate(() => (window as any).__fixture.rejectNativeClose(new Error("模拟关闭拒绝")));
  await expect(page.getByTestId("exit-notice")).toContainText("当前输入仍在窗口");
  await expect(name).toHaveValue("隔离退出后不应保存的元数据");
  await page.clock.runFor(1200);
  await expect(row(page).getByRole("status").last()).toContainText("元数据已保存");
  expect(await writes(page)).toHaveLength(1);
});

test("cards stay in a responsive grid while modal editing preserves drafts", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(async () => {
    const f = (window as any).__fixture;
    const template = f.secrets[0];
    for (let i = 0; i < 6; i++) f.secrets.push({ ...structuredClone(template), short_name: `synthetic_${i}`, display_name: i === 0 ? "SyntheticVeryLongServiceNameWithoutSpacesForLayoutVerification" : `合成服务 ${i + 1}` });
    await (window as any).__refreshFixture();
  });
  const cards = page.getByRole("list", { name: "密钥列表", exact: true }).locator(":scope > li");
  await expect(cards).toHaveCount(8);
  const first = await cards.nth(0).boundingBox();
  const second = await cards.nth(1).boundingBox();
  expect(first!.y).toBe(second!.y);
  expect(second!.x).toBeGreaterThan(first!.x + first!.width);
  expect(first!.width).toBeLessThan(400);
  await page.screenshot({ path: ".tmp/secrets-cards-wide.png", fullPage: true, animations: "disabled" });
  await expand(page);
  await expect(page.getByRole("button", { name: "关闭密钥编辑", exact: true })).toBeFocused();
  const candidate = row(page).getByLabel("新密钥值", { exact: true });
  await candidate.fill("synthetic-unsaved-card-draft");
  const editor = page.getByRole("dialog", { name: "GitHub 隔离测试", exact: true });
  await expect(editor).toBeVisible();
  await expect(editor).toHaveAttribute("aria-modal", "true");
  expect((await row(page).boundingBox())!.width).toBe(first!.width);
  await page.setViewportSize({ width: 375, height: 800 });
  await expect(candidate).toHaveValue("synthetic-unsaved-card-draft");
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(375);
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await page.screenshot({ path: ".tmp/secrets-cards-narrow.png", fullPage: true, animations: "disabled" });
  expect(await writes(page)).toHaveLength(0);
  page.once("dialog", (dialog) => dialog.dismiss());
  await page.keyboard.press("Escape");
  await expect(editor).toBeVisible();
  await expect(candidate).toHaveValue("synthetic-unsaved-card-draft");
  page.once("dialog", (dialog) => dialog.accept());
  await page.getByRole("button", { name: "关闭密钥编辑", exact: true }).click();
  await expect(editor).toHaveCount(0);
  await expect(row(page).locator("button[aria-expanded]")).toBeFocused();
  await expand(page);
  await expect(candidate).toHaveValue("");
  await page.keyboard.press("Escape");
  await expect(editor).toHaveCount(0);
});

test("modal cannot close during an isolated secret write and saves only once", async ({ page }) => {
  await expand(page, "empty_key");
  const editor = page.getByRole("dialog", { name: "空值隔离测试", exact: true });
  await row(page, "empty_key").getByLabel("密钥值", { exact: true }).fill("synthetic-modal-save-only");
  await page.evaluate(() => { (window as any).__fixture.delay = 1000; });
  await editor.getByRole("button", { name: "加密保存密钥", exact: true }).click();
  await expect(editor.getByRole("button", { name: "关闭密钥编辑", exact: true })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(editor).toBeVisible();
  await page.clock.runFor(3500);
  expect(await writes(page)).toHaveLength(1);
  await expect(editor.getByRole("button", { name: "关闭密钥编辑", exact: true })).toBeEnabled();
  await editor.getByRole("button", { name: "关闭密钥编辑", exact: true }).click();
  await expect(editor).toHaveCount(0);
});

test("empty is missing and never claimed valid", async ({ page }) => {
  await expect(row(page, "empty_key")).toContainText("未填");
  await expect(row(page)).toContainText("已填 · 未证明有效");
});

test("metadata autosaves without sending the draft credential", async ({
  page,
}) => {
  await expand(page);
  const card = row(page);
  await card
    .getByLabel("新密钥值", { exact: true })
    .fill("synthetic-draft-not-committed");
  await card.getByLabel("显示名称", { exact: true }).fill("自动保存的新名称");
  await page.clock.runFor(1100);
  await expect(card.getByRole("status").last()).toContainText("元数据已保存");
  const saved = await writes(page);
  expect(saved.length).toBeGreaterThan(0);
  expect(saved.every((c: any) => c.args.request.value == null)).toBe(true);
  await expect(card.getByLabel("新密钥值", { exact: true })).toHaveValue(
    "synthetic-draft-not-committed",
  );
});

test("failed autosave preserves draft and allows explicit retry", async ({
  page,
}) => {
  await expand(page);
  await page.evaluate(() => ((window as any).__fixture.failNextSave = true));
  await row(page)
    .getByLabel("显示名称", { exact: true })
    .fill("失败后保留的草稿");
  await page.clock.runFor(1100);
  await expect(row(page).getByRole("alert")).toContainText("模拟写入失败");
  await expect(row(page).getByLabel("显示名称", { exact: true })).toHaveValue(
    "失败后保留的草稿",
  );
  await row(page)
    .getByRole("button", { name: "立即保存元数据／重试", exact: true })
    .click();
  await expect(row(page).getByRole("alert")).toHaveCount(0);
});

test("conflicts require refresh and preserve both metadata and value drafts", async ({
  page,
}) => {
  await expand(page);
  await page.evaluate(
    () => ((window as any).__fixture.conflictNextSave = true),
  );
  await row(page)
    .getByLabel("新密钥值", { exact: true })
    .fill("synthetic-unsaved");
  await row(page).getByLabel("显示名称", { exact: true }).fill("冲突草稿");
  await page.clock.runFor(1100);
  await expect(row(page).getByRole("alert")).toContainText("密钥库已变化");
  await row(page)
    .getByRole("button", { name: "刷新服务器状态，保留草稿", exact: true })
    .click();
  await expect(row(page).getByLabel("新密钥值", { exact: true })).toHaveValue(
    "synthetic-unsaved",
  );
  await row(page)
    .getByRole("button", { name: "已核对，按当前元数据草稿重试", exact: true })
    .click();
  await expect(row(page).getByRole("alert")).toHaveCount(0);
});

test("rotation requires acknowledgment and does not alter old expiry while drafting", async ({
  page,
}) => {
  await expand(page);
  const card = row(page);
  await card.getByLabel("新密钥值", { exact: true }).fill("synthetic-rotated");
  await card
    .getByLabel("新密钥到期日期（留空为未记录）", { exact: true })
    .fill("2026-12-01");
  await page.clock.runFor(1100);
  expect((await writes(page)).length).toBe(0);
  await expect(
    card.getByRole("button", { name: "验证并切换到新密钥", exact: true }),
  ).toBeDisabled();
  await card.getByRole("checkbox").check();
  await card
    .getByRole("button", { name: "验证并切换到新密钥", exact: true })
    .click();
  await expect(card.getByText("旧密钥待停用", { exact: true })).toBeVisible();
  const request = (await writes(page)).at(-1).args.request;
  expect(request.operation).toBe("rotate");
  expect(request.expires_at).toBe("2026-12-01T15:59:59.999Z");
  await expect(card.getByLabel("新密钥值", { exact: true })).toHaveValue("");
});

test("revocation is a distinct explicit manual acknowledgment", async ({
  page,
}) => {
  await expand(page);
  const card = row(page);
  await card.getByLabel("新密钥值", { exact: true }).fill("synthetic-rotated");
  await card.getByRole("checkbox").check();
  await card
    .getByRole("button", { name: "验证并切换到新密钥", exact: true })
    .click();
  await expect(card.getByText("旧密钥待停用", { exact: true })).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await card
    .getByRole("button", { name: "确认旧密钥已在服务商停用", exact: true })
    .click();
  await expect(card.getByText("旧密钥待停用", { exact: true })).toHaveCount(0);
  expect((await writes(page)).at(-1).args.request.operation).toBe(
    "confirm_revocation",
  );
});

test("rollback restores only the selected key before revocation", async ({
  page,
}) => {
  await expand(page);
  const card = row(page);
  await card.getByLabel("新密钥值", { exact: true }).fill("synthetic-rotated");
  await card.getByRole("checkbox").check();
  await card
    .getByRole("button", { name: "验证并切换到新密钥", exact: true })
    .click();
  await expect(card.getByText("旧密钥待停用", { exact: true })).toBeVisible();
  page.once("dialog", (dialog) => dialog.accept());
  await card
    .getByRole("button", { name: "撤回切换，恢复旧密钥", exact: true })
    .click();
  await expect(card.getByText("旧密钥待停用", { exact: true })).toHaveCount(0);
  expect((await writes(page)).at(-1).args.request.operation).toBe(
    "rollback_rotation",
  );
});

test("new secrets commit metadata and encrypted value in one call", async ({
  page,
}) => {
  await page.getByRole("button", { name: "添加密钥", exact: true }).click();
  await page.getByText("自定义服务", { exact: true }).click();
  // Actual wizard field labels are checked by the browser, not source matching.
  await page.getByLabel("短名", { exact: false }).fill("new_fixture_key");
  await page
    .getByRole("textbox", { name: /^密钥值/ })
    .fill("synthetic-new-key");
  await page.getByRole("button", { name: "添加", exact: true }).click();
  await expect(row(page, "new_fixture_key")).toBeVisible();
  const saved = await writes(page);
  expect(saved).toHaveLength(1);
  expect(saved[0].args.request.value).toBe("synthetic-new-key");
});

test("expiry refresh preserves a draft when the row changes status", async ({
  page,
}) => {
  await page.evaluate(async () => {
    (window as any).__fixture.secrets[0].expires_at = "2026-11-08T04:00:10Z";
    await (window as any).__refreshFixture();
  });
  await expand(page);
  await row(page)
    .getByLabel("新密钥值", { exact: true })
    .fill("synthetic-preserved-on-expiry");
  await page.clock.runFor(65000);
  await expect(row(page)).toContainText("已到期");
  await expect(row(page).getByLabel("新密钥值", { exact: true })).toHaveValue(
    "synthetic-preserved-on-expiry",
  );
  expect(await writes(page)).toHaveLength(0);
});

test("revealed values disappear after thirty seconds and on blur", async ({
  page,
}) => {
  await expand(page);
  await row(page)
    .getByRole("button", { name: "显示密钥（30 秒后隐藏）", exact: true })
    .click();
  await expect(
    row(page).getByText("synthetic-current-value", { exact: true }),
  ).toBeVisible();
  await page.clock.runFor(30010);
  await expect(
    row(page).getByText("synthetic-current-value", { exact: true }),
  ).toHaveCount(0);
  await row(page)
    .getByRole("button", { name: "显示密钥（30 秒后隐藏）", exact: true })
    .click();
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await expect(
    row(page).getByText("synthetic-current-value", { exact: true }),
  ).toHaveCount(0);
});

test("read failure is visible and notification consent is explicit", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as any).__fixture.secrets[0].expires_at = "2020-01-01T00:00:00Z";
  });
  await page.clock.runFor(65000);
  await page.getByRole("button", { name: /密钥提醒：/ }).click();
  await expect(
    page.getByRole("button", { name: "启用系统到期提醒", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() => (window as any).__fixture.notifications.length),
  ).toBe(0);
  await page
    .getByRole("button", { name: "启用系统到期提醒", exact: true })
    .click();
  await expect
    .poll(() =>
      page.evaluate(() => (window as any).__fixture.notifications.length),
    )
    .toBe(1);
  await page.clock.runFor(65000);
  expect(
    await page.evaluate(() => (window as any).__fixture.notifications.length),
  ).toBe(1);
  await page.evaluate(() => {
    (window as any).__fixture.failReads = true;
  });
  await page.getByRole("button", { name: "刷新到期状态", exact: true }).click();
  await expect(
    page.getByText("密钥到期状态读取失败，暂时无法确认是否有到期告警。", {
      exact: true,
    }),
  ).toBeVisible();
});

test("late reveal after blur is discarded and drafts never enter persistent storage", async ({
  page,
}) => {
  await expand(page);
  await row(page)
    .getByLabel("新密钥值", { exact: true })
    .fill("synthetic-draft-memory-only");
  await page.evaluate(() => {
    (window as any).__fixture.delay = 1000;
  });
  await row(page)
    .getByRole("button", { name: "显示密钥（30 秒后隐藏）", exact: true })
    .click();
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.runFor(1100);
  await expect(
    row(page).getByText("synthetic-current-value", { exact: true }),
  ).toHaveCount(0);
  const storage = await page.evaluate(() =>
    JSON.stringify({
      local: { ...localStorage },
      session: { ...sessionStorage },
    }),
  );
  expect(storage).not.toContain("synthetic-draft-memory-only");
  expect(storage).not.toContain("synthetic-current-value");
  expect(await writes(page)).toHaveLength(0);
});

test("wizard back navigation keeps the draft when discard is declined", async ({ page }) => {
  await page.getByRole("button",{name:"添加密钥",exact:true}).click();
  await page.getByText("自定义服务",{exact:true}).click();
  await page.getByRole("textbox",{name:/^密钥值/}).fill("synthetic-wizard-draft");
  page.once("dialog", dialog => dialog.dismiss());
  await page.getByRole("button",{name:"返回选择服务",exact:true}).click();
  await expect(page.getByRole("textbox",{name:/^密钥值/})).toHaveValue("synthetic-wizard-draft");
  expect(await writes(page)).toHaveLength(0);
});
