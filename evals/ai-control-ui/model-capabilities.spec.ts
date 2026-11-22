import { test, expect } from "@playwright/test";
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("tab", { name: "模型与服务", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "能力库", exact: true }),
  ).toBeVisible();
});
test("groups models by function and canonical model, resolves channel key without generation", async ({
  page,
}) => {
  await expect(
    page.getByRole("list", { name: "功能模型" }).locator("li"),
  ).toHaveCount(1);
  await page.getByRole("button", { name: "查看调用方式" }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(
    page.getByText("sops:fixture_openai", { exact: false }),
  ).toBeVisible();
  await page
    .getByLabel("调用渠道", { exact: true })
    .selectOption("openrouter::openai/fixture");
  await expect(
    page.getByText("sops:fixture_openrouter", { exact: false }),
  ).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("model_capability_call");
  expect(calls).not.toContain("model_capabilities_refresh");
  await page.getByRole("button", { name: "关闭模型调用" }).click();
  await page.getByRole("tab", { name: "视频生成", exact: true }).click();
  await expect(page.getByText("此分组暂无匹配模型")).toBeVisible();
  await page.getByRole("tab", { name: "视频理解", exact: true }).click();
  await expect(page.getByText("隔离文本模型", { exact: true })).toBeVisible();
});
test("local PNG requires a supported route and an explicit call", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.imageRequests = [];
    fixture.values.model_capability_resolve = ({ request }: any) => {
      fixture.imageRequests.push(["resolve", request]);
      return { method: "POST", endpoint: "https://openrouter.ai/api/v1/chat/completions",
        message: "本机校验通过", credential: { credential_ref: "sops:fixture_openrouter" } };
    };
    fixture.values.model_capability_call = ({ request }: any) => {
      fixture.imageRequests.push(["call", request]);
      return { route_id: request.route_id, status: "succeeded", task_id: null, artifact: null,
        output: { choices: [{ message: { content: "合成图片结果" } }] }, message: "已取得合成结果" };
    };
  });
  await page.getByRole("button", { name: "查看调用方式" }).click();
  const channel = page.getByLabel("调用渠道", { exact: true });
  const imageMode = page.getByRole("checkbox", { name: "使用本机 PNG" });
  await expect(imageMode).toHaveCount(0);
  await channel.selectOption("openrouter::openai/fixture");
  await imageMode.check();
  const validate = page.getByRole("button", { name: "检查调用方式" });
  const call = page.getByRole("button", { name: "调用模型", exact: true });
  await expect(call).toBeDisabled();
  const pngBase64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAAGHRFWHRGaXh0dXJlAHN5bnRoZXRpYy1pZ25vcmWGeEp/AAAADElEQVR4nGMQVDIGAACuAGcVHqFfAAAAAElFTkSuQmCC";
  await page.getByLabel("选择 PNG").setInputFiles({
    name: "synthetic.png", mimeType: "image/png", buffer: Buffer.from(pngBase64, "base64"),
  });
  await page.getByLabel("图片问题").fill("只描述合成图片");
  await expect(page.getByRole("button", { name: "复制 CLI 示例" })).toBeDisabled();
  const requests = () => page.evaluate(() => (window as any).__performance.imageRequests);
  expect(await requests()).toEqual([]);
  await validate.click();
  await expect(page.getByText("本机校验通过")).toBeVisible();
  expect((await requests()).map(([operation]: [string, unknown]) => operation)).toEqual(["resolve"]);
  await call.click();
  await expect(page.getByText("已取得合成结果")).toBeVisible();
  const expected = { route_id: "openrouter::openai/fixture", output_root: null,
    input: { vision: { prompt: "只描述合成图片", png_base64: pngBase64 } } };
  expect(await requests()).toEqual([["resolve", expected], ["call", expected]]);
  await channel.selectOption("openai::fixture");
  await expect(imageMode).toHaveCount(0);
  await channel.selectOption("openrouter::openai/fixture");
  await imageMode.check();
  await expect(call).toBeDisabled();
  expect(await requests()).toHaveLength(2);
});
test("channel filters and missing credentials do not borrow another key; services stay separate", async ({
  page,
}) => {
  await page.getByRole("tab", { name: "图片生成", exact: true }).click();
  await page.getByRole("button", { name: "查看调用方式" }).click();
  await expect(
    page.getByRole("button", { name: "调用模型", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByText("api-key-manager:fixture_fal", { exact: false }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("tab", { name: "其他服务与凭据", exact: true }).click();
  await expect(page.getByText("代码仓库服务，不是生成模型")).toBeVisible();
});
test("explicit sync keeps cached models on failure; navigation and filtering reuse cache", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as any).__performance.values.model_capabilities_refresh = () => {
      throw Error("合成同步失败");
    };
  });
  await page.getByRole("button", { name: "同步模型目录", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("合成同步失败");
  await expect(page.getByText("隔离文本模型", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "工作台", exact: true }).click();
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  const count = await page.evaluate(
    () =>
      (window as any).__performance.calls.filter(
        (c: string) => c === "model_capabilities_catalog",
      ).length,
  );
  expect(count).toBe(1);
});
test("explicit synthetic call retains queued result across modal reopen and queries original task", async ({
  page,
}) => {
  await page.evaluate(() => {
    const state = (window as any).__performance;
    state.values.model_capability_call = {
      provider_id: "openai",
      model_id: "fixture",
      credential_ref: "sops:fixture_openai",
      status: "queued",
      task_id: "fixture-task",
      output: {},
      artifact: null,
      message: "合成异步任务",
    };
    state.values.model_capability_task = (args: any) => {
      if (args.taskId !== "fixture-task") throw Error("Wrong task");
      return {
        provider_id: "openai",
        model_id: "fixture",
        credential_ref: "sops:fixture_openai",
        status: "succeeded",
        task_id: "fixture-task",
        output: { text: "合成结果" },
        artifact: null,
        message: "合成完成",
      };
    };
  });
  await page.getByRole("button", { name: "查看调用方式" }).click();
  await page.getByRole("button", { name: "调用模型", exact: true }).click();
  await expect(page.getByText("已排队，尚未完成")).toBeVisible();
  await page.getByRole("button", { name: "关闭模型调用" }).click();
  await page.getByRole("button", { name: "查看调用方式" }).click();
  await expect(page.getByText("已排队，尚未完成")).toBeVisible();
  await page.getByRole("button", { name: "查询原任务状态与结果" }).click();
  await expect(page.getByText("业务已完成")).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.calls.filter(
          (c: string) => c === "model_capability_call",
        ).length,
    ),
  ).toBe(1);
});
test("narrow layout remains usable and route modal does not span a full data row", async ({
  page,
}) => {
  await page.setViewportSize({ width: 820, height: 900 });
  await page.getByRole("button", { name: "查看调用方式" }).click();
  const modal = page.getByRole("dialog");
  await expect(modal).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  await page.screenshot({
    path: ".tmp/model-capabilities-narrow.png",
    animations: "disabled",
  });
});
