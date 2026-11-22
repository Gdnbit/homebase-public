import { test, expect } from "@playwright/test";
import { navigate } from "./navigation";

test("distinguishes an unknown attachment receipt from a file that never reached sending", async ({ page }) => {
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_status = {
      bound: true, enabled: false, state: "stopped", message: "已停止接收",
      tasks: [
        { id: "unknown-clip", command: "assistant", status: "succeeded", delivery: "attachment_unknown",
          delivery_error: "文件仍在原任务。", created_at: "2026-11-21T00:00:00Z" },
        { id: "unsent-clip", command: "assistant", status: "succeeded", delivery: "attachment_failed",
          created_at: "2026-11-21T00:00:01Z" },
      ],
    };
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  const tasks = page.getByRole("region", { name: "微信最近任务" });
  await expect(tasks.getByText("回复附件发送待核对", { exact: false })).toBeVisible();
  await expect(tasks.getByText("回复附件未发送", { exact: false })).toBeVisible();
  await expect(tasks.getByText("文件仍在原任务。")).toBeVisible();
});

test("lists a captured share without exposing its text or submitting it again", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_status = {
      bound: true, enabled: false, state: "stopped", message: "已停止接收",
      tasks: [{
        id: "original-share-capture", command: "inbox_submit", status: "succeeded",
        job_id: "original-capture-job", delivery: "accepted", created_at: "2026-11-20T00:00:00Z",
      }],
    };
    fixture.calls.length = 0;
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await expect(page.getByText("帮我收录：分享内容", { exact: true })).toBeVisible();
  const tasks = page.getByRole("region", { name: "微信最近任务" });
  await expect(tasks.getByText("微信资料收录", { exact: true })).toBeVisible();
  await expect(tasks.getByText("original-share-capture", { exact: true })).toBeVisible();
  await expect(tasks.getByText("执行完成", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("inbox_submit");
});

test("shows the MP4 transcription entry and the original task identity", async ({ page }) => {
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_status = {
      bound: true, enabled: false, state: "stopped", message: "已停止接收",
      tasks: [{
        id: "original-video-receipt", command: "wechat_video_transcript",
        status: "completed", job_id: null, delivery: "accepted",
        created_at: "2026-01-01T00:00:00Z",
      }, {
        id: "unreadable-share-receipt", command: "wechat_unreadable_share",
        status: "unsupported", job_id: null, delivery: "pending",
        created_at: "2026-01-01T00:00:01Z",
      }],
    };
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await expect(page.getByText("直接发送 MP4 视频或文件", { exact: true })).toBeVisible();
  await expect(page.getByText("完成后可引用文字稿回复提问", { exact: false })).toBeVisible();
  const tasks = page.getByRole("region", { name: "微信最近任务" });
  await expect(tasks.getByText("本地视频转写", { exact: true })).toBeVisible();
  await expect(tasks.getByText("original-video-receipt", { exact: true })).toBeVisible();
  await expect(tasks.getByText("回复已被微信接口接受", { exact: false })).toBeVisible();
  await expect(tasks.getByText("未读到分享内容", { exact: true })).toBeVisible();
  await expect(tasks.getByText("unreadable-share-receipt", { exact: true })).toBeVisible();
});

test("shows how to analyze a numbered video repository and displays its original task", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_status = { bound: true, enabled: false, state: "stopped", message: "已停止接收",
      tasks: [{ id: "original-video-repo-analysis", command: "repo_project_analyze", status: "succeeded",
        job_id: "original-analysis-job", delivery: "accepted", created_at: "2026-11-20T00:00:00Z" }] };
    fixture.calls.length = 0;
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await expect(page.getByText("分析仓库 GitHub链接", { exact: true })).toBeVisible();
  await expect(page.getByText("/安装项目 原分析任务编号", { exact: true })).toBeVisible();
  await expect(page.getByText("分析第 1 个项目", { exact: true })).toBeVisible();
  await expect(page.getByText("找项目 marked", { exact: true })).toBeVisible();
  await expect(page.getByText("找第 1 个项目", { exact: true })).toBeVisible();
  const tasks = page.getByRole("region", { name: "微信最近任务" });
  await expect(tasks.getByText("仓库分析", { exact: true })).toBeVisible();
  await expect(tasks.getByText("original-video-repo-analysis", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("repo_project_analyze");
});

test("shows an original WeChat voice receipt without exposing its transcript", async ({ page }) => {
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_status = {
      bound: true, enabled: false, state: "stopped", message: "已停止接收",
      tasks: [{
        id: "original-voice-receipt", command: "wechat_voice_transcript",
        status: "completed", job_id: null, delivery: "unknown",
        created_at: "2026-01-01T00:00:00Z",
      }],
    };
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await expect(page.getByText("直接发送一条语音", { exact: true })).toBeVisible();
  const tasks = page.getByRole("region", { name: "微信最近任务" });
  await expect(tasks.getByText("微信语音转写", { exact: true })).toBeVisible();
  await expect(tasks.getByText("original-voice-receipt", { exact: true })).toBeVisible();
  await expect(tasks.getByText("回复状态待核对", { exact: false })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("assistant_send");
});

test("keeps old transcription counts separate from recent rows and clears stale counts on error", async ({ page }) => {
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_status = {
      bound: true, enabled: false, state: "stopped", message: "已停止接收",
      local_transcriptions: {
        video: { processing: 1, unverified: 0 },
        voice: { processing: 0, unverified: 2 },
      },
      tasks: [{
        id: "newer-query", command: "message", status: "completed",
        job_id: null, delivery: "accepted", created_at: "2026-01-01T00:00:00Z",
      }],
    };
  });
  const refresh = page.getByRole("button", { name: "刷新微信状态" });
  await refresh.click();
  const tasks = page.getByRole("region", { name: "微信最近任务" });
  await expect(tasks.getByRole("status")).toHaveText("本地转写 · 1 项处理中 · 2 项回执待核对");
  await expect(tasks.getByText("newer-query", { exact: true })).toBeVisible();
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_status = () => { throw new Error("合成微信状态读取失败"); };
  });
  await refresh.click();
  await expect(page.getByText("读取失败", { exact: true })).toBeVisible();
  await expect(tasks.getByRole("status")).toHaveCount(0);
});

test("expired binding can rescan without forgetting or starting old work", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_status = {
      bound: true, enabled: false, state: "expired", message: "授权已失效，原任务保留",
      tasks: [
        { id: "old-result", command: "assistant", status: "failed", job_id: "original-turn", delivery: "unknown", created_at: "2026-01-01T00:00:00Z" },
        { id: "expired-query", command: "message", status: "canceled", delivery: "not_sent", delivery_error: "微信授权已更新，旧查询没有发送。请用原任务编号再查一次。", created_at: "2026-01-01T00:00:00Z" },
      ],
    };
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await expect(page.getByRole("button", { name: "开始接收", exact: true })).toHaveCount(0);
  await expect(page.getByText("原任务保留", { exact: false }).first()).toBeVisible();
  await page.getByRole("button", { name: "重新扫码（保留记录）", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "扫码连接微信" })).toBeVisible();
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(page.getByText("old-result", { exact: true })).toBeVisible();
  await expect(page.getByText("回复未发送", { exact: false })).toBeVisible();
  await expect(page.getByText("回复状态待核对", { exact: false })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls.filter((c: string) => c === "wechat_login_begin")).toHaveLength(1);
  expect(calls).not.toContain("wechat_forget");
  expect(calls).not.toContain("wechat_start");
  expect(calls).not.toContain("assistant_send");
});

test("failed reauthorization keeps stopped binding and existing task visible", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_status = {
      bound: true, enabled: false, state: "stopped", message: "已停止接收",
      tasks: [{ id: "kept-task", command: "assistant", status: "completed", job_id: "kept-turn", delivery: "accepted", created_at: "2026-01-01T00:00:00Z" }],
    };
    fixture.values.wechat_login_begin = () => { throw new Error("合成网络失败，原绑定保留"); };
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await page.getByRole("button", { name: "重新扫码（保留记录）", exact: true }).click();
  await expect(page.getByRole("alert").first()).toContainText("原绑定保留");
  await expect(page.getByRole("dialog")).not.toBeVisible();
  await expect(page.getByText("kept-task", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("wechat_forget");
  expect(calls).not.toContain("wechat_start");
});

test("late cancel after durable confirmation shows completion instead of pretending to undo login", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_login_cancel = () => {
      fixture.values.wechat_pairing = { state: "confirmed", message: "新授权已经保存，原任务保留", qr_data_url: null };
      fixture.values.wechat_status = { bound: true, enabled: true, state: "connected", message: "已连接", tasks: [] };
      throw new Error("新授权已经保存，未撤销或恢复旧凭据");
    };
  });
  await page.getByRole("button", { name: "扫码绑定", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "扫码连接微信" })).toBeVisible();
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await expect(page.getByText("新授权已经保存，原任务保留", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "完成", exact: true }).click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls.filter((c: string) => c === "wechat_login_cancel")).toHaveLength(1);
  expect(calls).not.toContain("wechat_forget");
  expect(calls).not.toContain("wechat_stop");
});

test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_status = {
      bound: false,
      enabled: false,
      state: "unbound",
      message: "尚未绑定微信",
      tasks: [],
      protocol_version: "synthetic",
    };
    fixture.values.wechat_pairing = {
      state: "wait",
      message: "请用本人微信扫码",
      qr_data_url:
        "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='240' height='240'%3E%3Crect width='240' height='240' fill='white'/%3E%3Ctext x='40' y='120'%3ESYNTHETIC QR%3C/text%3E%3C/svg%3E",
    };
    fixture.values.wechat_login_begin = async () => {
      await new Promise((r) => setTimeout(r, 100));
      return null;
    };
    fixture.values.wechat_login_cancel = null;
    fixture.values.wechat_login_pin = () => {
      fixture.values.wechat_pairing = {
        state: "confirmed",
        message: "绑定成功",
        qr_data_url: null,
      };
      fixture.values.wechat_status = {
        ...fixture.values.wechat_status,
        bound: true,
        enabled: true,
        state: "connected",
        message: "已连接",
      };
      return null;
    };
  });
  await page.getByRole("link", { name: "微信连接", exact: true }).click();
});
test("opening and reentering never creates a QR or changes existing tasks", async ({
  page,
}) => {
  await expect(
    page.getByRole("button", { name: "扫码绑定", exact: true }),
  ).toBeVisible();
  await navigate(page, "概览");
  await page.getByRole("link", { name: "微信连接", exact: true }).click();
  const calls = await page.evaluate(() =>
    (window as any).__performance.calls.filter((c: string) =>
      c.startsWith("wechat_"),
    ),
  );
  expect(calls).toEqual(["wechat_status"]);
});

test("failed outbound delivery shows its safe reason separately from business success",async({page})=>{
  await page.evaluate(()=>{
    (window as any).__performance.values.wechat_status={bound:true,enabled:true,state:"connected",message:"已连接",tasks:[{id:"fixture-output",command:"assistant",status:"succeeded",job_id:"fixture-turn",delivery:"unknown",delivery_error:"微信暂不接受这次发送（ret=-2），请发送消息后查询原结果",created_at:"2026-11-13T00:00:00Z"}],protocol_version:"synthetic"};
  });
  await page.getByRole("button",{name:"刷新微信状态",exact:true}).click();
  await expect(page.getByText("执行完成",{exact:true})).toBeVisible();
  await expect(page.getByText(/请发送消息后查询原结果/)).toBeVisible();
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((s:string)=>s==="wechat_start"))).toHaveLength(0);
});
test("binding is explicit, duplicate clicks are suppressed and cancellation removes QR", async ({
  page,
}) => {
  const begin = page.getByRole("button", { name: "扫码绑定", exact: true });
  await begin.click();
  await expect(
    page.getByRole("dialog", { name: "扫码连接微信" }),
  ).toBeVisible();
  await expect(
    page.getByAltText("微信绑定二维码，请用本人微信扫码"),
  ).toBeVisible();
  await page.getByRole("button", { name: "取消", exact: true }).click();
  await expect(page.getByRole("dialog")).not.toBeVisible();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls.filter((c: string) => c === "wechat_login_begin")).toHaveLength(
    1,
  );
  expect(calls).toContain("wechat_login_cancel");
});
test("phone PIN stays in binding form and confirmed binding clears QR", async ({
  page,
}) => {
  await page.evaluate(() => {
    (window as any).__performance.values.wechat_pairing = {
      state: "need_pin",
      message: "请输入手机配对码",
      qr_data_url: null,
    };
  });
  await page.getByRole("button", { name: "扫码绑定", exact: true }).click();
  await page.getByLabel("手机配对码", { exact: true }).fill("123456");
  await page.getByRole("button", { name: "提交配对码", exact: true }).click();
  await expect(page.getByText("绑定成功", { exact: true })).toBeVisible();
  await expect(
    page.getByAltText("微信绑定二维码，请用本人微信扫码"),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "完成", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "停止接收", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(() =>
      JSON.stringify({ ...localStorage, ...sessionStorage }),
    ),
  ).not.toContain("123456");
});
test("network failures stay explicit and stop does not touch capability tasks", async ({
  page,
}) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_status = {
      bound: true,
      enabled: true,
      state: "reconnecting",
      message: "合成网络中断，正在重连",
      tasks: [
        {
          id: "wx-synthetic",
          command: "capability_render",
          status: "running",
          job_id: "synthetic-job",
          delivery: "accepted",
          created_at: "2026-11-10T00:00:00Z",
        },
      ],
    };
    fixture.values.wechat_stop = () => {
      throw Error("合成保存失败，原状态保留");
    };
  });
  await page.getByRole("button", { name: "刷新微信状态" }).click();
  await expect(page.getByText("合成网络中断，正在重连")).toBeVisible();
  await page.getByRole("button", { name: "停止接收", exact: true }).click();
  await expect(page.getByRole("alert").first()).toContainText("合成保存失败");
  await expect(page.getByText("执行中", { exact: true })).toBeVisible();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("capability_render");
  expect(calls).not.toContain("task_events_get");
  expect(calls).not.toContain("wechat_forget");
  await page.screenshot({
    path: ".tmp/wechat-ui-connected.png",
    fullPage: true,
  });
});

test("a failed binding read suspends connection changes until the original status is verified", async ({ page }) => {
  const status = {
    bound: true,
    enabled: true,
    state: "connected",
    message: "隔离连接已就绪",
    tasks: [],
  };
  await page.evaluate((value) => {
    (window as any).__performance.values.wechat_status = value;
  }, status);
  const refresh = page.getByRole("button", { name: "刷新微信状态" });
  await refresh.click();
  await expect(page.getByText(status.message)).toBeVisible();

  // Opening a dialog must not let an old binding through after a failed poll.
  await page.getByRole("button", { name: "解除本机绑定", exact: true }).click();
  await page.evaluate(async () => {
    const fixture = (window as any).__performance;
    fixture.values.wechat_status = () => { throw Error("隔离状态读取失败"); };
    await fixture.client.refetchQueries({ queryKey: ["wechat", "status"], exact: true });
  });
  await expect(page.getByText("读取失败", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "确认解除本机绑定" })).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "停止接收" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "解除本机绑定", exact: true })).toBeDisabled();

  await page.evaluate((value) => {
    (window as any).__performance.values.wechat_status = value;
  }, status);
  await refresh.click();
  await expect(page.getByRole("button", { name: "停止接收" })).toBeEnabled();
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("wechat_stop");
  expect(calls).not.toContain("wechat_forget");
});
