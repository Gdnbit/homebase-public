import { test, expect, type Page } from "@playwright/test";

async function ready(page: Page) {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_config_get.settings = { revision: 9, enabled: true, route_id: "openrouter::fixture:free", free_only: true };
    p.values.assistant_sessions_list = ["A", "B"].map(id => ({ id, title: `隔离会话 ${id}`, source: "desktop", message_count: 0 }));
    p.values.assistant_session_get = ({ sessionId }: any) => ({ id: sessionId, source: "desktop", messages: [], turns: [] });
    p.asrStarts = 0; p.asrCancels = 0;
    p.values.assistant_voice_stage = () => { const id = crypto.randomUUID(); return { recording_id: id, source_path: `X:/isolated/${id}/input.webm`, output_root: `X:/isolated/${id}/transcripts` }; };
    p.values.assistant_voice_discard = () => null;
    p.values.asr_environment_probe = () => ({ available: true });
    p.values.asr_task_start = ({payload}: any) => { p.asrStarts++; return { jobId: crypto.randomUUID(), taskDir: `${payload.outputRoot}/task` }; };
    p.values.asr_task_get = () => ({ job: { status: "success" }, artifacts: [{ relativePath: "run/transcript.txt" }] });
    p.values.asr_task_read_artifact = () => ({ content: "蓝色圆点，简短字幕。" });
    p.values.asr_task_cancel = () => { p.asrCancels++; return null; };
    p.micStopped = 0;
    p.trackStops = [];
    p.recorders = [];
    Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { getUserMedia: async () => {
      const index = p.trackStops.push(0) - 1;
      return { getTracks: () => [{ stop: () => { p.trackStops[index]++; p.micStopped++; } }] };
    } } });
    class Recorder {
      static isTypeSupported() { return true; }
      state = "inactive"; ondataavailable: any; onstop: any; onerror: any;
      constructor() { p.recorders.push(this); }
      start() { this.state = "recording"; }
      stop() {
        this.state = "inactive";
        this.ondataavailable?.({ data: new Blob([new Uint8Array([0x1a, 0x45, 0xdf, 0xa3, 1])], { type: "audio/webm" }) });
        if (!p.deferRecorderStop) queueMicrotask(() => this.onstop?.());
      }
    }
    (window as any).MediaRecorder = Recorder;
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toBeEnabled();
}
async function record(page: Page) {
  await page.getByRole("button", { name: "语音输入", exact: true }).click();
  await expect(page.getByText("正在录音", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "停止并转写", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.asrStarts)).toBe(1);
}
async function reloadProtectedFixture(page: Page) {
  const seed = await page.evaluate(() => { const p = (window as any).__performance; return { drafts: Object.fromEntries(p.desktopStore), stages: Object.fromEntries(p.voiceStages) }; });
  await page.addInitScript(({drafts, stages}) => { (window as any).__desktopSeed = drafts; (window as any).__voiceStageSeed = stages; }, seed);
  await page.reload(); await ready(page);
}
test.beforeEach(async ({page}) => {
  await page.route("**/*", route => new URL(route.request().url()).hostname === "127.0.0.1" ? route.continue() : route.abort());
  await page.goto("/evals/ai-control-ui/performance.html"); await ready(page);
});

test("offline voice prepares a draft with assistant models disabled", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.values.assistant_config_get = {
      settings: { revision: 10, enabled: false, route_id: "", free_only: true },
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  const editor = page.getByLabel("发送消息", { exact: true });
  const send = page.getByRole("button", { name: "发送", exact: true });
  await editor.fill("原有草稿");
  await expect(send).toHaveAttribute("title", "模型尚未启用，草稿可先保存");
  await expect(send).toBeDisabled();
  await expect(page.getByRole("button", { name: "语音输入", exact: true })).toBeEnabled();
  await record(page);
  await expect(editor).toHaveValue("原有草稿\n蓝色圆点，简短字幕。");
  await expect(send).toBeDisabled();
  await editor.press("Enter");
  const calls = await page.evaluate(() => (window as any).__performance.calls as string[]);
  expect(calls).not.toContain("assistant_send");
  expect(calls).not.toContain("assistant_config_save");
});

test("navigation keeps accepted ASR and restores a preview without inserting into another conversation", async ({page}) => {
  await page.getByLabel("发送消息", {exact:true}).fill("A 的原稿");
  await page.evaluate(() => { (window as any).__performance.values.asr_task_get = () => ({job:{status:"running"},artifacts:[]}); });
  await record(page);
  await page.getByRole("link", {name:"隔离会话 B",exact:true}).click();
  await page.getByLabel("发送消息", {exact:true}).fill("B 的独立内容");
  await page.evaluate(() => { (window as any).__performance.values.asr_task_get = () => ({job:{status:"success"},artifacts:[{relativePath:"run/transcript.txt"}]}); });
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("B 的独立内容");
  await page.getByRole("link", {name:"隔离会话 A",exact:true}).click();
  await expect(page.getByRole("button", {name:"插入草稿",exact:true})).toBeVisible();
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("A 的原稿");
  await page.getByRole("button", {name:"插入草稿",exact:true}).click();
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("A 的原稿\n蓝色圆点，简短字幕。");
  const result = await page.evaluate(() => { const p = (window as any).__performance; return { starts:p.asrStarts,cancels:p.asrCancels,sends:p.calls.filter((c:string)=>c==="assistant_send").length }; });
  expect(result).toEqual({starts:1,cancels:0,sends:0});
});

test("restart queries the same accepted task and inserts a persisted transcript at most once", async ({page}) => {
  await page.evaluate(() => { (window as any).__performance.values.asr_task_get = () => ({job:{status:"running"},artifacts:[]}); });
  await record(page); await reloadProtectedFixture(page);
  await expect(page.getByRole("button", {name:"插入草稿",exact:true})).toBeVisible();
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("");
  await page.getByRole("button", {name:"插入草稿",exact:true}).click();
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("蓝色圆点，简短字幕。");
  await reloadProtectedFixture(page);
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("蓝色圆点，简短字幕。");
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(0);
  expect(await page.evaluate(() => (window as any).__performance.calls.includes("assistant_voice_apply"))).toBe(false);
});

async function restoreUnconfirmedJob(page: Page, phase: "accepted" | "unknown") {
  await page.addInitScript(phase => {
    (window as any).__desktopSeed = {new:{
      scope:"new", revision:4, body:{text:"重启前的原稿",mode:"auto",route:"",files:[]}, pending:null,
      voices:[{recording_id:"original-recording",audio_sha256:"a".repeat(64),phase,
        job:{job_id:"original-job",task_dir:"X:/isolated/voice/task"},transcript:null,message:"原任务执行状态未确认"}],
    }};
  }, phase);
  await page.reload();
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    fixture.originalQueries = [];
    fixture.originalResultReady = false;
    fixture.values.assistant_voice_get = ({scope,recordingId}: any) => {
      fixture.originalQueries.push({scope,recordingId});
      const entry = structuredClone(fixture.desktopStore.get(scope));
      const voice = entry.voices.find((voice: any) => voice.recording_id === recordingId);
      const recovered = fixture.originalResultReady;
      const expected = recovered ? "ready" : "unknown";
      if (voice.phase !== expected) entry.revision++;
      voice.phase = expected;
      voice.transcript = recovered ? "原任务完成的转写。" : null;
      voice.message = recovered ? "" : "原任务执行状态未确认";
      fixture.desktopStore.set(scope,entry);
      return {status:"loaded",entry};
    };
  });
  await ready(page);
}

test("known original voice recovery checks for completion without blocking or modifying the draft", async ({page}) => {
  await restoreUnconfirmedJob(page, "unknown");
  const input = page.getByLabel("发送消息", {exact:true});
  await expect(page.getByText("正在查询原转写…", {exact:true})).toBeVisible();
  await expect(page.getByText("原任务执行状态未确认", {exact:true})).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.originalQueries.length)).toBeGreaterThan(0);
  await input.fill("重启后补充的草稿");
  await page.evaluate(() => { (window as any).__performance.originalResultReady = true; });
  const insert = page.getByRole("button", {name:"插入草稿",exact:true});
  await expect(insert).toBeVisible();
  await expect(input).toHaveValue("重启后补充的草稿");
  await insert.click();
  await expect(input).toHaveValue("重启后补充的草稿\n原任务完成的转写。");
  const observed = await page.evaluate(() => {
    const fixture = (window as any).__performance;
    return {queries:fixture.originalQueries, starts:fixture.asrStarts, calls:fixture.calls};
  });
  expect(observed.queries.length).toBeGreaterThan(1);
  expect(observed.queries.every((query: any) => query.scope === "new" && query.recordingId === "original-recording")).toBe(true);
  expect(observed.starts).toBe(0);
  expect(observed.calls).not.toContain("assistant_send");
  expect(observed.calls.filter((call: string) => call === "assistant_voice_apply")).toHaveLength(1);
});

test("known original voice recovery ends bounded observation with explicit retry", async ({page}) => {
  await page.clock.install();
  await restoreUnconfirmedJob(page, "accepted");
  const queryCount = () => page.evaluate(() => (window as any).__performance.originalQueries.length);
  await page.clock.runFor(2000);
  expect(await queryCount()).toBe(1);
  await expect(page.getByText("原任务执行状态未确认", {exact:true})).toBeVisible();
  await expect(page.getByText("正在查询原转写…", {exact:true})).toBeVisible();
  await page.clock.fastForward(180_000);
  const retry = page.getByRole("button", {name:"重试转写",exact:true});
  await expect(retry).toBeVisible();
  const boundedCount = await queryCount();
  await page.clock.fastForward(10_000);
  expect(await queryCount()).toBe(boundedCount);
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("重启前的原稿");
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(0);
});

test("an unknown submission survives restart and never restarts ASR on query", async ({page}) => {
  await page.evaluate(() => { const p = (window as any).__performance; p.values.asr_task_start = () => { p.asrStarts++; throw Error("合成受理未知"); }; });
  await record(page); await expect(page.getByText(/合成受理未知/)).toBeVisible();
  await reloadProtectedFixture(page);
  await page.getByRole("button", {name:"重试转写",exact:true}).click();
  await expect(page.getByText(/提交结果未知/)).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(0);
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("");
});

test("empty recognition is explicit and preserves existing input", async ({page}) => {
  await page.getByLabel("发送消息", {exact:true}).fill("保留原稿");
  await page.evaluate(() => { (window as any).__performance.values.asr_task_read_artifact = () => ({content:"   "}); });
  await record(page); await expect(page.getByText(/未识别到语音内容/)).toBeVisible();
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("保留原稿");
  expect(await page.evaluate(() => (window as any).__performance.calls.includes("assistant_voice_apply"))).toBe(false);
});

test("a lost insertion response resolves its persisted marker without a second append", async ({page}) => {
  await page.evaluate(() => { const p = (window as any).__performance, original = p.values.assistant_voice_apply; p.values.assistant_voice_apply = async (args:any) => { await original(args); throw Error("合成插入响应丢失"); }; });
  await record(page); await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("蓝色圆点，简短字幕。");
  await expect(page.getByRole("button", {name:"语音输入",exact:true})).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.calls.filter((c:string)=>c==="assistant_voice_apply").length)).toBe(1);
});

test("an unconfirmed cancellation retains its original receipt across navigation", async ({page}) => {
  await page.evaluate(() => { const p = (window as any).__performance; p.values.asr_task_get = () => ({job:{status:"running"},artifacts:[]}); p.values.asr_task_cancel = () => { p.asrCancels++; throw Error("合成取消未确认"); }; });
  await record(page); await page.getByRole("button", {name:"取消语音输入",exact:true}).click();
  await expect(page.getByText(/取消尚未确认/)).toBeVisible();
  await page.getByRole("button", {name:"重试转写",exact:true}).click();
  await expect(page.getByText(/取消尚未确认/)).toBeVisible();
  await page.getByRole("link", {name:"隔离会话 B",exact:true}).click();
  await page.getByRole("link", {name:"隔离会话 A",exact:true}).click();
  await expect(page.getByText(/取消尚未确认/)).toBeVisible();
  const result = await page.evaluate(() => { const p = (window as any).__performance; return { cancels:p.asrCancels, starts:p.asrStarts, phase:p.desktopStore.get("A").voices.at(-1).phase }; });
  expect(result).toEqual({cancels:1,starts:1,phase:"cancel_requested"});
});

test("cancel before a late staging reply does not launch ASR or reopen the input", async ({page}) => {
  await page.evaluate(() => { const p = (window as any).__performance, original = p.values.assistant_voice_begin; p.values.assistant_voice_begin = async (args:any) => { const reply = await original(args); return new Promise(resolve => { p.finishStage = () => resolve(reply); }); }; });
  await page.getByRole("button", {name:"语音输入",exact:true}).click();
  await page.getByRole("button", {name:"停止并转写",exact:true}).click();
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishStage)).toBe("function");
  await page.getByRole("button", {name:"取消语音输入",exact:true}).click();
  await page.evaluate(() => (window as any).__performance.finishStage());
  await expect(page.getByRole("button", {name:"语音输入",exact:true})).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(0);
  expect(await page.evaluate(() => (window as any).__performance.desktopStore.get("A").voices.at(-1).phase)).toBe("dismissed");
});

test("a first-conversation voice draft is restored before selecting existing sessions", async ({page}) => {
  await page.addInitScript(() => {
    (window as any).__desktopSeed = { new: { scope:"new",revision:3,body:{text:"",mode:"auto",route:"",files:[]},pending:null,
      voices:[{recording_id:"first-voice",phase:"ready",audio_sha256:"a".repeat(64),job:null,transcript:"首条语音待插入",message:""}] } };
  });
  await page.reload(); await ready(page);
  await expect(page.getByRole("button", {name:"插入草稿",exact:true})).toBeVisible();
  for (const width of [1024,390]) {
    await page.setViewportSize({width,height:920});
    await expect(page.getByRole("button",{name:"插入草稿",exact:true})).toBeInViewport();
    expect(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
  }
  await page.getByRole("button", {name:"插入草稿",exact:true}).click();
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("首条语音待插入");
  const entries = await page.evaluate(() => Object.fromEntries((window as any).__performance.desktopStore));
  expect(entries.new.body.text).toBe("首条语音待插入"); expect(entries.A?.body.text || "").toBe("");
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(0);
});

test("the recovery drawer opens a protected native draft without creating or sending a native turn", async ({page}) => {
  await page.evaluate(() => {
    const p = (window as any).__performance, id = "d15d80d6-f7da-4f2e-a05a-0c9a652d363b";
    p.desktopStore.set(`native:codex:${id}`, { scope:`native:codex:${id}`,revision:4,body:{text:"原生会话已保存输入",mode:"auto",route:"",files:[]},pending:null,voices:[] });
    p.nativeActions = [];
    p.values.conversation_native = ({request}:any) => {
      p.nativeActions.push(request.action);
      if(request.action==="read") return {thread:{id,turns:[]},next_cursor:null};
      if(request.action==="status") return {id,state:"idle",output:[]};
      if(request.action==="list") return {data:[]};
      throw Error("Synthetic native writes forbidden");
    };
  });
  await page.getByLabel("更多对话操作").click();
  await page.getByRole("button", {name:"未发送与待核对",exact:true}).click();
  await page.getByRole("button", {name:/codex · d15d80d6/}).click();
  await expect(page.getByLabel("给 Codex 发消息", {exact:true})).toHaveValue("原生会话已保存输入");
  const actions = await page.evaluate(() => (window as any).__performance.nativeActions);
  expect(actions).not.toContain("send"); expect(actions).not.toContain("create");
});

test("native long drafts are saved but never submitted beyond the native message budget", async ({page}) => {
  const id = "opaque_native_long_01", scope = `native:codex:${id}`;
  await page.evaluate(({id,scope}) => {
    const p = (window as any).__performance;
    p.desktopStore.set(scope, {scope,revision:1,body:{text:"",mode:"auto",route:"",files:[]},pending:null,voices:[]});
    p.nativeActions = [];
    p.values.conversation_native = ({request}:any) => {
      p.nativeActions.push(request.action);
      if(request.action==="read") return {thread:{id,turns:[]},next_cursor:null};
      if(request.action==="status") return {id,state:"idle",output:[]};
      if(request.action==="list") return {data:[]};
      throw Error("Synthetic native writes forbidden");
    };
  }, {id,scope});
  // Nonempty scope is listed in the recovery drawer.
  await page.evaluate(scope => { (window as any).__performance.desktopStore.get(scope).body.text="原生长稿"; }, scope);
  await page.getByLabel("更多对话操作").click();
  await page.getByRole("button", {name:"未发送与待核对",exact:true}).click();
  await page.getByRole("button", {name:/codex · opaque_n/}).click();
  const input = page.getByLabel("给 Codex 发消息", {exact:true}), long = "中文原生草稿".repeat(1800);
  await input.fill(long);
  await expect(page.getByLabel("原生草稿保存状态")).toContainText("草稿已受保护保存");
  await expect(page.locator("#native-input-budget")).toContainText("发送上限为 8,000 字节");
  await page.getByRole("button", {name:"发送",exact:true}).click();
  await expect(page.getByRole("alert")).toContainText("8,000 字节");
  expect(await page.evaluate(scope => (window as any).__performance.desktopStore.get(scope).body.text, scope)).toBe(long);
  expect(await page.evaluate(() => (window as any).__performance.nativeActions)).not.toContain("send");
  await expect(input).toHaveValue(long);
});

test("leaving live capture releases the microphone and does not start uncommitted ASR", async ({page}) => {
  await page.getByRole("button", {name:"语音输入",exact:true}).click();
  await expect(page.getByText("正在录音", {exact:true})).toBeVisible();
  await page.getByRole("link", {name:"隔离会话 B",exact:true}).click();
  expect(await page.evaluate(() => (window as any).__performance.micStopped)).toBeGreaterThan(0);
  const calls = await page.evaluate(() => (window as any).__performance.calls);
  expect(calls).not.toContain("assistant_voice_begin"); expect(calls).not.toContain("asr_task_start"); expect(calls).not.toContain("asr_task_cancel");
});

for (const lateEvent of ["onstop", "onerror", "permission rejection"] as const) {
  test(`a cancelled capture cannot release a replacement microphone after ${lateEvent}`, async ({page}) => {
    const input = page.getByLabel("发送消息", {exact:true});
    const start = page.getByRole("button", {name:"语音输入",exact:true});
    await input.fill("保留旧稿");
    await page.evaluate(event => {
      const fixture = (window as any).__performance;
      if (event === "permission rejection") {
        const media = navigator.mediaDevices;
        const getUserMedia = media.getUserMedia;
        media.getUserMedia = () => new Promise((_resolve, reject) => {
          fixture.rejectFirstPermission = () => reject(new DOMException("synthetic", "NotAllowedError"));
          media.getUserMedia = getUserMedia;
        });
      } else fixture.deferRecorderStop = true;
    }, lateEvent);
    await start.click();
    await expect(page.getByText(lateEvent === "permission rejection" ? "等待麦克风权限…" : "正在录音", {exact:true})).toBeVisible();
    await page.getByRole("button", {name:"取消语音输入",exact:true}).click();
    await start.click();
    await expect(page.getByText("正在录音", {exact:true})).toBeVisible();
    const stoppedBeforeSubmission = await page.evaluate(async event => {
      const fixture = (window as any).__performance;
      fixture.deferRecorderStop = false;
      if (event === "permission rejection") fixture.rejectFirstPermission();
      else fixture.recorders[0][event]();
      await Promise.resolve();
      return [...fixture.trackStops];
    }, lateEvent);
    expect(stoppedBeforeSubmission).toEqual(lateEvent === "permission rejection" ? [0] : [1, 0]);
    await page.getByRole("button", {name:"停止并转写",exact:true}).click();
    await expect(input).toHaveValue("保留旧稿\n蓝色圆点，简短字幕。");
    const result = await page.evaluate(() => {
      const fixture = (window as any).__performance;
      return { trackStops:fixture.trackStops, starts:fixture.asrStarts, cancels:fixture.asrCancels, sends:fixture.calls.filter((name: string) => name === "assistant_send").length };
    });
    expect(result).toEqual({trackStops:lateEvent === "permission rejection" ? [1] : [1, 1], starts:1, cancels:0, sends:0});
  });
}

test("a failed preflight persists its staged reason and permits one explicit retry after repair", async ({page}) => {
  await page.evaluate(() => { (window as any).__performance.values.asr_environment_probe = () => ({available:false,error:"合成本地引擎未安装"}); });
  await page.getByRole("button", {name:"语音输入",exact:true}).click();
  await page.getByRole("button", {name:"停止并转写",exact:true}).click();
  await expect(page.getByText(/合成本地引擎未安装/)).toBeVisible();
  await page.getByRole("link", {name:"隔离会话 B",exact:true}).click();
  await page.getByRole("link", {name:"隔离会话 A",exact:true}).click();
  await expect(page.getByText(/合成本地引擎未安装/)).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(0);
  await page.evaluate(() => { (window as any).__performance.values.asr_environment_probe = () => ({available:true}); });
  await page.getByRole("button", {name:"重试转写",exact:true}).click();
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("蓝色圆点，简短字幕。");
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(1);
});

test("raw cleanup failure is a partial warning after a successful atomic insertion", async ({page}) => {
  await page.evaluate(() => { (window as any).__performance.values.assistant_voice_discard = () => { throw Error("合成文件占用"); }; });
  await record(page);
  await expect(page.getByLabel("发送消息", {exact:true})).toHaveValue("蓝色圆点，简短字幕。");
  await expect(page.getByLabel("草稿保存状态")).toContainText("原录音清理尚未确认");
  expect(await page.evaluate(() => (window as any).__performance.desktopStore.get("A").voices.at(-1).phase)).toBe("applied");
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(1);
});
