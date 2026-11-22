import { test, expect, type Page } from "@playwright/test";
const root = "X:\\Synthetic-HyperFrames";
const base = {
  id: "hyperframes.animation",
  name: "HyperFrames",
  version: "0.9.26",
  state: "not_installed",
  message: "尚未安装；请选择独立目录。",
};
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("tab", { name: "开源项目", exact: true }).click();
});
async function connect(page: Page) {
  await page.getByLabel("独立安装目录", { exact: true }).fill(root);
  await page.getByRole("button", { name: "连接目录", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "安装到独立目录", exact: true }),
  ).toBeEnabled();
}
test("installation is explicit, duplicate clicks are disabled and completion refreshes runtime state", async ({
  page,
}) => {
  await connect(page);
  await page.evaluate((value) => {
    const f = (window as any).__performance;
    f.values.capability_install = () =>
      new Promise((resolve) => {
        (window as any).__finishInstall = () => {
          f.values.capabilities_catalog = {
            ...value,
            state: "installed",
            message: "已安装，尚未通过本机视频验收。",
          };
          resolve({ status: "succeeded" });
        };
      });
  }, base);
  await page
    .getByRole("button", { name: "安装到独立目录", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "安装到独立目录", exact: true }),
  ).toBeDisabled();
  await expect(
    page.getByText("正在安装；依赖和缓存写入独立目录。", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => (window as any).__finishInstall());
  await expect(
    page.getByText("已安装 · 待验收", { exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.calls.filter(
          (name: string) => name === "capability_install",
        ).length,
    ),
  ).toBe(1);
  await expect(
    page.getByRole("button", { name: "生成视频", exact: true }),
  ).toBeVisible();
});
test("render failure stays explicit; successful output refreshes and a failed result read clears stale success", async ({
  page,
}) => {
  await connect(page);
  await page.evaluate((value) => {
    const f = (window as any).__performance;
    f.values.capabilities_catalog = {
      ...value,
      state: "installed",
      message: "已安装，尚未通过本机视频验收。",
    };
    f.values.capability_render = {
      status: "failed",
      error: "Synthetic render error",
    };
  }, base);
  await page.getByRole("button", { name: "刷新能力状态", exact: true }).click();
  await page.getByRole("button", { name: "生成视频", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Synthetic render error");
  await expect(
    page.getByText("视频已生成并通过完整解码验证。", { exact: true }),
  ).toHaveCount(0);
  await page.evaluate((value) => {
    const f = (window as any).__performance;
    f.values.capability_render = () => {
      f.values.capabilities_catalog = {
        ...value,
        state: "verified",
        message: "已完成本机视频渲染及结果验证。",
        installation: { last_verified_run: "synthetic-run" },
      };
      return {
        status: "succeeded",
        run_id: "synthetic-run",
        artifact: {
          path: "X:\\Synthetic-HyperFrames\\output.mp4",
          bytes: 120,
          sha256: "synthetic",
        },
        verification: { width: 960, height: 540, frames: 144, seconds: 6 },
      };
    };
    f.values.capability_result = () => new Promise(resolve => {
      (window as any).__finishResultRead = () => resolve({ status: "failed", error: "Synthetic changed artifact" });
    });
  }, base);
  await page.getByRole("button", { name: "生成视频", exact: true }).click();
  await expect(page.getByText("已通过本机验收", { exact: true })).toBeVisible();
  await expect(
    page.getByText("视频已生成并通过完整解码验证。", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "获取最近验证结果", exact: true })
    .click();
  await expect(page.getByRole("button", { name: "连接目录", exact: true })).toBeDisabled();
  await expect(page.getByRole("button", { name: "生成视频", exact: true })).toBeDisabled();
  await page.evaluate(() => (window as any).__finishResultRead());
  await expect(page.getByRole("alert")).toContainText(
    "Synthetic changed artifact",
  );
  await expect(
    page.getByText("视频已生成并通过完整解码验证。", { exact: true }),
  ).toHaveCount(0);
});
test("catalog failures and unknown installation metadata never appear healthy", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as any).__performance.values.capabilities_catalog = () => {
      throw Error("Synthetic unavailable");
    };
  });
  await page.getByRole("button", { name: "刷新能力状态", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("能力状态读取失败");
  await expect(page.getByText("状态未知", { exact: true })).toBeVisible();
  await page.evaluate((value) => {
    (window as any).__performance.values.capabilities_catalog = {
      ...value,
      state: "unknown",
      message: "Synthetic invalid manifest",
    };
  }, base);
  await page.getByRole("button", { name: "刷新能力状态", exact: true }).click();
  await expect(
    page.getByText("Synthetic invalid manifest", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("状态未知", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "生成视频", exact: true }),
  ).toHaveCount(0);
});
