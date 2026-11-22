import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";

const count = (page: Page, command: string) =>
  page.evaluate(
    (c) =>
      (window as any).__performance.calls.filter((s: string) => s === c).length,
    command,
  );
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
async function open(page: Page) {
  await navigate(page, "快捷面板");
  await expect(
    page.getByRole("button", { name: "重新检查下载任务状态", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "重新检查转写任务状态", exact: true }),
  ).toBeEnabled();
}
async function noWrites(page: Page) {
  expect(
    await page.evaluate(() =>
      (window as any).__performance.calls.filter((s: string) =>
        /^(model_download_(start|cancel)|asr_task_(start|cancel|organize))$/.test(
          s,
        ),
      ),
    ),
  ).toEqual([]);
  expect(
    await page.evaluate(() => (window as any).__performance.rejected),
  ).toEqual([]);
}

test("unmanaged active records remain unknown across mount, tab switches and manual rechecks", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.model_download_reconcile = [
      {
        job_id: "old-download",
        model_name: "Synthetic old download",
        status: "running",
        phase: "downloading",
        progress_percent: 40,
        revision: 9,
        runtime_state: "unmanaged",
        runtime_message:
          "Synthetic unmanaged download: original record unchanged",
      },
    ];
    f.values.asr_task_reconcile = [
      {
        jobId: "old-asr",
        taskDir: "synthetic",
        status: "preprocessing",
        pid: 42,
        updatedAt: "original",
        runtime_state: "unmanaged",
        runtime_message: "Synthetic unmanaged ASR: original record unchanged",
      },
    ];
    f.beforeRecovery = JSON.stringify([
      f.values.model_download_reconcile,
      f.values.asr_task_reconcile,
    ]);
  });
  await open(page);
  await expect(
    page.getByText(
      "1 个历史下载任务状态未知，原始记录未修改；请先核对任务和结果。",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(
    page.getByText(
      "1 个历史转写任务状态未知，原始记录未修改；请先核对任务和结果。",
      { exact: true },
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "先核对已有任务状态", exact: true }),
  ).toHaveCount(2);
  for (const button of await page
    .getByRole("button", { name: "先核对已有任务状态", exact: true })
    .all())
    await expect(button).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "取消当前下载", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "取消任务", exact: true }),
  ).toHaveCount(0);
  expect(await count(page, "model_download_reconcile")).toBe(1);
  expect(await count(page, "asr_task_reconcile")).toBe(1);
  await page
    .getByRole("button", { name: "重新检查下载任务状态", exact: true })
    .click();
  await page
    .getByRole("button", { name: "重新检查转写任务状态", exact: true })
    .click();
  await expect.poll(() => count(page, "model_download_reconcile")).toBe(2);
  await expect.poll(() => count(page, "asr_task_reconcile")).toBe(2);
  await navigate(page, "历史对话");
  await open(page);
  await expect.poll(() => count(page, "model_download_reconcile")).toBe(3);
  await expect.poll(() => count(page, "asr_task_reconcile")).toBe(3);
  await page.waitForTimeout(2100);
  expect(await count(page, "model_download_get")).toBe(0);
  expect(await count(page, "asr_task_get")).toBe(0);
  expect(
    await page.evaluate(() => {
      const f = (window as any).__performance;
      return (
        JSON.stringify([
          f.values.model_download_reconcile,
          f.values.asr_task_reconcile,
        ]) === f.beforeRecovery
      );
    }),
  ).toBe(true);
  await noWrites(page);
});

test("recovery failure is explicit, blocks new work, and manual recheck can recover", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    for (const key of ["model_download_reconcile", "asr_task_reconcile"])
      f.values[key] = () => {
        throw Error("Synthetic partial JSON / access denied");
      };
  });
  await open(page);
  await expect(
    page.getByText("恢复检查失败：任务状态未知，原始记录未修改，请重试。", {
      exact: true,
    }),
  ).toHaveCount(2);
  await expect(
    page.getByRole("button", { name: "开始转写", exact: true }),
  ).toBeDisabled();
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.model_download_reconcile = [];
    f.values.asr_task_reconcile = [];
  });
  await page
    .getByRole("button", { name: "重新检查下载任务状态", exact: true })
    .click();
  await page
    .getByRole("button", { name: "重新检查转写任务状态", exact: true })
    .click();
  await expect(
    page.getByText("恢复检查失败：任务状态未知，原始记录未修改，请重试。", {
      exact: true,
    }),
  ).toHaveCount(0);
  await noWrites(page);
});

test("terminal records remain terminal and never auto-start or poll", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.model_download_reconcile = [
      {
        job_id: "finished",
        status: "completed",
        phase: "completed",
        progress_percent: 100,
        runtime_state: "terminal",
      },
    ];
    f.values.asr_task_reconcile = [
      {
        jobId: "finished",
        taskDir: "synthetic",
        status: "cancelled",
        runtime_state: "terminal",
      },
    ];
  });
  await open(page);
  await page.waitForTimeout(2000);
  expect(await count(page, "model_download_get")).toBe(0);
  expect(await count(page, "asr_task_get")).toBe(0);
  await noWrites(page);
});

test("late recovery response after navigating away does not adopt or poll a task", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.model_download_reconcile = () =>
      new Promise((resolve) => {
        f.releaseModelRecovery = () =>
          resolve([
            { job_id: "late", status: "running", runtime_state: "managed" },
          ]);
      });
    f.values.asr_task_reconcile = () =>
      new Promise((resolve) => {
        f.releaseAsrRecovery = () =>
          resolve([
            {
              jobId: "late",
              taskDir: "synthetic",
              status: "running",
              runtime_state: "managed",
            },
          ]);
      });
  });
  await navigate(page, "快捷面板");
  await expect.poll(() => count(page, "model_download_reconcile")).toBe(1);
  await expect.poll(() => count(page, "asr_task_reconcile")).toBe(1);
  await navigate(page, "历史对话");
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.releaseModelRecovery();
    f.releaseAsrRecovery();
  });
  await page.waitForTimeout(2200);
  expect(await count(page, "model_download_get")).toBe(0);
  expect(await count(page, "asr_task_get")).toBe(0);
  await noWrites(page);
});

test("changing task roots drops late observations from the old directory", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.model_download_reconcile = (args: any) =>
      args.jobsRoot.includes("RootB")
        ? []
        : new Promise((resolve) => {
            f.releaseOldModel = () =>
              resolve([
                {
                  job_id: "old-root",
                  status: "running",
                  runtime_state: "managed",
                },
              ]);
          });
    f.values.asr_task_reconcile = (args: any) =>
      args.outputRoot.includes("RootB")
        ? []
        : new Promise((resolve) => {
            f.releaseOldAsr = () =>
              resolve([
                {
                  jobId: "old-root",
                  taskDir: "synthetic",
                  status: "running",
                  runtime_state: "managed",
                },
              ]);
          });
  });
  await navigate(page, "快捷面板");
  await expect.poll(() => count(page, "model_download_reconcile")).toBe(1);
  await expect.poll(() => count(page, "asr_task_reconcile")).toBe(1);
  await page.evaluate(() =>
    (window as any).__performance.changeRecoveryRoots(),
  );
  await expect.poll(() => count(page, "model_download_reconcile")).toBe(2);
  await expect.poll(() => count(page, "asr_task_reconcile")).toBe(2);
  await expect(
    page.getByRole("button", { name: "重新检查下载任务状态", exact: true }),
  ).toBeEnabled();
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.releaseOldModel();
    f.releaseOldAsr();
  });
  await page.waitForTimeout(2200);
  expect(await count(page, "model_download_get")).toBe(0);
  expect(await count(page, "asr_task_get")).toBe(0);
  await noWrites(page);
});
