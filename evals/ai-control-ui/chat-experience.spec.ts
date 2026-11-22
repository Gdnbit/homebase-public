import { test, expect, type Page } from "@playwright/test";

async function ready(page: Page) {
  await page.route("**/*", route => new URL(route.request().url()).hostname === "127.0.0.1" ? route.continue() : route.abort());
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.values.assistant_config_get.settings = { revision: 7, enabled: true, route_id: "openrouter::fixture-default:free", wechat_enabled: true, max_rounds: 8, max_tokens: 2048, free_only: true, credential_source: "api_key_manager" };
    p.values.assistant_sessions_list = ["A", "B"].map(id => ({ id, title: `隔离会话 ${id}`, source: "desktop", active_turn_id: null, updated_at: "2026-11-15T00:00:00Z", message_count: 0 }));
    p.values.assistant_session_get = ({ sessionId }: any) => ({ id: sessionId, title: `隔离会话 ${sessionId}`, source: "desktop", messages: [], turns: [] });
    p.sent = [];
    p.values.assistant_send = ({ request }: any) => { p.sent.push(structuredClone(request)); return { session_id: request.session_id || "A", turn_id: "synthetic-turn", state: "completed" }; };
    p.values.model_capabilities_catalog.models.push({ ...p.values.model_capabilities_catalog.models[0], route_id: "openrouter::fixture-other:free", model_id: "fixture-other:free", name: "隔离可用免费模型", provider_id: "openrouter", account_state: "configured", protocol: "chat" });
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toBeEnabled();
}
test.beforeEach(async ({ page }) => { await ready(page); });

async function delayTextAttachmentReads(page: Page) {
  await page.evaluate(() => {
    const waiting = (blob: Blob): blob is File => blob instanceof File && blob.name.startsWith("等待-");
    const completed = new Map<string, () => Promise<void>>();
    const readers = new WeakMap<FileReader, string>();
    const started: string[] = [];
    const canceled: string[] = [];
    const arrayBuffer = Blob.prototype.arrayBuffer;
    const read = FileReader.prototype.readAsArrayBuffer;
    const abort = FileReader.prototype.abort;
    Blob.prototype.arrayBuffer = function () {
      if (!waiting(this)) return arrayBuffer.call(this);
      const file = this;
      started.push(file.name);
      return new Promise((resolve) => {
        completed.set(file.name, async () => { resolve(await arrayBuffer.call(file)); });
      });
    };
    FileReader.prototype.readAsArrayBuffer = function (file: Blob) {
      if (!waiting(file)) return read.call(this, file);
      started.push(file.name);
      readers.set(this, file.name);
      completed.set(file.name, () => new Promise<void>((resolve) => {
        this.addEventListener("loadend", () => resolve(), { once: true });
        read.call(this, file);
      }));
    };
    FileReader.prototype.abort = function () {
      const name = readers.get(this);
      if (!name) return abort.call(this);
      canceled.push(name);
      this.dispatchEvent(new ProgressEvent("abort"));
    };
    Object.assign(window, {
      __attachmentIO: {
        started,
        canceled,
        release: (name: string) => completed.get(name)?.(),
      },
    });
  });
}

test("canceling a slow text attachment keeps existing files and allows a new selection", async ({ page }) => {
  const input = page.getByLabel("发送消息", { exact: true });
  const fileInput = page.getByLabel("选择文本附件");
  const send = page.getByRole("button", { name: "发送", exact: true });
  await input.fill("继续编辑原草稿");
  await fileInput.setInputFiles({ name: "保留.txt", mimeType: "text/plain", buffer: Buffer.from("已有附件") });
  await expect(page.getByLabel("待发送附件").getByText("保留.txt")).toBeVisible();
  await delayTextAttachmentReads(page);
  await fileInput.setInputFiles([
    { name: "半批.txt", mimeType: "text/plain", buffer: Buffer.from("整批取消时一并舍弃") },
    { name: "等待-旧文件.txt", mimeType: "text/plain", buffer: Buffer.from("不应加入") },
  ]);
  await expect.poll(() => page.evaluate(() => (window as any).__attachmentIO.started)).toEqual(["等待-旧文件.txt"]);
  await expect(send).toBeDisabled();
  await page.getByRole("button", { name: "取消读取", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__attachmentIO.canceled)).toEqual(["等待-旧文件.txt"]);
  await expect(input).toBeFocused();
  await expect(send).toBeEnabled();
  await expect(input).toHaveValue("继续编辑原草稿");

  await fileInput.setInputFiles({ name: "新文件.txt", mimeType: "text/plain", buffer: Buffer.from("本次选择") });
  await expect(page.getByLabel("待发送附件").getByText("新文件.txt")).toBeVisible();
  await page.evaluate(() => (window as any).__attachmentIO.release("等待-旧文件.txt"));
  await send.click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
  const files = await page.evaluate(() => (window as any).__performance.sent[0].attachments);
  expect(files.map((file: {name: string}) => file.name)).toEqual(["保留.txt", "新文件.txt"]);
});

test("switching conversations aborts the old file read without changing either draft", async ({ page }) => {
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("A 的原始草稿");
  await delayTextAttachmentReads(page);
  await page.getByLabel("选择文本附件").setInputFiles({
    name: "等待-A.txt", mimeType: "text/plain", buffer: Buffer.from("不能写进另一个会话"),
  });
  await expect.poll(() => page.evaluate(() => (window as any).__attachmentIO.started)).toEqual(["等待-A.txt"]);
  await page.getByRole("link", { name: "隔离会话 B", exact: true }).click();
  await expect(input).toHaveValue("");
  await input.fill("B 的独立草稿");
  expect(await page.evaluate(() => (window as any).__attachmentIO.canceled)).toEqual(["等待-A.txt"]);
  await page.evaluate(() => (window as any).__attachmentIO.release("等待-A.txt"));
  await expect(input).toHaveValue("B 的独立草稿");
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await expect(input).toHaveValue("A 的原始草稿");
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(0);
});

test("model picker and text files reach the request; unknown sends reuse the same identity", async ({ page }) => {
  await page.getByRole("button", { name: "选择本次对话模型" }).click();
  const picker = page.getByRole("dialog", { name: "本次对话模型" });
  await picker.getByRole("option", { name: /隔离可用免费模型/ }).click();
  await page.getByLabel("选择文本附件").setInputFiles({ name: "隔离说明.md", mimeType: "text/markdown", buffer: Buffer.from("蓝色圆点，简短中文字幕。") });
  await expect(page.getByLabel("待发送附件").getByText("隔离说明.md")).toBeVisible();
  await page.getByLabel("发送消息", { exact: true }).fill("总结附件");
  await page.evaluate(() => { const p = (window as any).__performance; p.values.assistant_send = ({ request }: any) => { p.sent.push(structuredClone(request)); if (p.sent.length === 1) throw Error("合成连接中断，受理未知"); return { session_id: request.session_id, turn_id: "same-original", state: "completed" }; }; });
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("受理未知");
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("总结附件");
  await expect(page.getByLabel("待发送附件")).toBeVisible();
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("");
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  const result = await page.evaluate(() => { const p = (window as any).__performance; return { sent: p.sent, settings: p.values.assistant_config_get.settings, calls: p.calls }; });
  expect(result.sent).toHaveLength(2); expect(result.sent[0]).toEqual(result.sent[1]);
  expect(result.sent[0].route_id).toBe("openrouter::fixture-other:free");
  expect(result.sent[0].attachments).toEqual([{ name: "隔离说明.md", content: "蓝色圆点，简短中文字幕。" }]);
  expect(result.settings.route_id).toBe("openrouter::fixture-default:free"); expect(result.settings.revision).toBe(7);
  expect(result.calls).not.toContain("assistant_config_save"); expect(result.calls).not.toContain("model_capability_call");
});

test("drafts, attachments and model choices are scoped to their conversations", async ({ page }) => {
  await page.getByLabel("发送消息", { exact: true }).fill("A 的未发送草稿");
  await page.getByLabel("选择文本附件").setInputFiles({ name: "A.txt", mimeType: "text/plain", buffer: Buffer.from("只属于 A") });
  await expect(page.getByLabel("待发送附件")).toBeVisible();
  await page.getByRole("link", { name: "隔离会话 B", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("");
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  await page.getByLabel("发送消息", { exact: true }).fill("B 的独立草稿");
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("A 的未发送草稿");
  await expect(page.getByLabel("待发送附件").getByText("A.txt")).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(0);
});

test("acknowledged model choice survives switching conversations", async ({ page }) => {
  await page.getByRole("button", { name: "选择本次对话模型" }).click();
  await page.getByRole("dialog", { name: "本次对话模型" }).getByRole("option", { name: /隔离可用免费模型/ }).click();
  await page.getByLabel("发送消息", { exact: true }).fill("第一条隔离消息");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("");
  await page.getByRole("link", { name: "隔离会话 B", exact: true }).click();
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await expect(page.getByRole("button", { name: "选择本次对话模型" })).toContainText("隔离可用免费模型");
  await page.getByLabel("发送消息", { exact: true }).fill("第二条隔离消息");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.length)).toBe(2);
  const sent = await page.evaluate(() => (window as any).__performance.sent);
  expect(sent[1].route_id).toBe(sent[0].route_id);
});

test("unknown send retains its original request across leaving and returning to the assistant", async ({ page }) => {
  await page.evaluate(() => { const p = (window as any).__performance; p.values.assistant_send = ({ request }: any) => { p.sent.push(structuredClone(request)); if (p.sent.length === 1) throw Error("合成受理未知"); return { session_id: request.session_id, turn_id: "original-after-navigation", state: "completed" }; }; });
  await page.getByLabel("发送消息", { exact: true }).fill("不能重复执行的隔离要求");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("受理未知");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("不能重复执行的隔离要求");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.length)).toBe(2);
  const sent = await page.evaluate(() => (window as any).__performance.sent);
  expect(sent[1]).toEqual(sent[0]);
});

test("unsupported or oversized attachments leave the draft intact and never send", async ({ page }) => {
  await page.getByLabel("发送消息", { exact: true }).fill("保留这段内容");
  await page.getByLabel("选择文本附件").setInputFiles({ name: "不支持.jpg", mimeType: "image/jpeg", buffer: Buffer.from("synthetic") });
  await expect(page.getByRole("alert")).toContainText("暂不支持");
  await page.getByLabel("选择文本附件").setInputFiles({ name: "过大.txt", mimeType: "text/plain", buffer: Buffer.alloc(16385, 65) });
  await expect(page.getByRole("alert")).toContainText("16 KiB");
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("保留这段内容");
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(0);
});

test("PNG preserves its original scope and selected route through an unknown send", async ({ page }) => {
  const route = "openrouter::fixture-vision:free";
  await page.evaluate((route) => {
    const p = (window as any).__performance;
    const models = p.values.model_capabilities_catalog.models;
    models.push({ ...models[0], route_id: route, model_id: "fixture-vision:free", provider_id: "openrouter", protocol: "chat", account_state: "configured", name: "隔离视觉模型", input_schema: { anyOf: [{ required: ["vision"] }] } });
    p.values.assistant_desktop_image_stage = ({ scope, name, pngBase64 }: any) => {
      if (!pngBase64.startsWith("iVBOR")) throw Error("隔离校验失败");
      return { scope, name, sha256: "a".repeat(64), bytes: 69 };
    };
    p.values.assistant_send = ({ request }: any) => {
      p.sent.push(structuredClone(request));
      if (p.sent.length === 1) throw Error("合成图片发送待核对");
      return { session_id: request.session_id, turn_id: "original-image-turn", state: "completed" };
    };
  }, route);
  const png = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGMQVDIGAACuAGcVHqFfAAAAAElFTkSuQmCC", "base64");
  const message = page.getByLabel("发送消息", { exact: true });
  const send = page.getByRole("button", { name: "发送", exact: true });
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await expect(message).toBeEnabled();
  await message.fill("帮我看看这张图。");
  await page.getByLabel("选择文本附件").setInputFiles({ name: "synthetic.png", mimeType: "image/png", buffer: png });
  await expect(page.getByLabel("待发送附件")).toContainText("synthetic.png");
  await expect(send).toBeDisabled();
  await expect(page.getByText("请为这张图片选择支持视觉的模型。", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "选择本次对话模型" }).click();
  await page.getByRole("option", { name: /隔离视觉模型/ }).click();
  await expect(page.getByLabel("消息方式")).toHaveValue("chat");
  await expect(send).toBeEnabled();

  await page.getByRole("link", { name: "隔离会话 B", exact: true }).click();
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await expect(page.getByLabel("待发送附件")).toContainText("synthetic.png");
  await send.click();
  await expect(page.getByRole("alert")).toContainText("合成图片发送待核对");
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("link", { name: "AI 助理", exact: true }).click();
  await page.getByRole("link", { name: "隔离会话 A", exact: true }).click();
  await expect(page.getByLabel("待发送附件")).toContainText("synthetic.png");
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
  await send.click();
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.length)).toBe(2);
  const [first, second] = await page.evaluate(() => (window as any).__performance.sent);
  expect(first).toMatchObject({ session_id: "A", route_id: route, mode: "chat", image: { scope: "A", name: "synthetic.png", bytes: 69 } });
  expect(second).toEqual(first);
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
});

test("IME Enter never sends; Shift Enter makes a newline and ordinary Enter submits once", async ({ page }) => {
  const input = page.getByLabel("发送消息", { exact: true });
  await input.fill("中文输入");
  await input.dispatchEvent("compositionstart"); await input.press("Enter");
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(0);
  await input.dispatchEvent("compositionend"); await input.press("Shift+Enter");
  await expect(input).toHaveValue(/中文输入\n+$/);
  await input.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
});

test("assistant resumes Enter after an interrupted composition loses focus", async ({ page }) => {
  const input = page.getByLabel("发送消息", { exact: true });
  const draft = "中断组词后保留的助理草稿";
  await input.fill(draft);
  await input.dispatchEvent("compositionstart");
  await page.getByRole("button", { name: "选择本次对话模型" }).focus();
  await expect(input).not.toBeFocused();
  await expect(input).toHaveValue(draft);
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);

  await input.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.map((request: any) => request.text))).toEqual([draft]);
});

test("model search Enter preserves the draft until explicit send", async ({ page }) => {
  const draft = page.getByLabel("发送消息", { exact: true });
  const trigger = page.getByRole("button", { name: "选择本次对话模型" });
  await draft.fill("等我选好模型再发送");
  await trigger.click();
  const picker = page.getByRole("dialog", { name: "本次对话模型" });
  const search = picker.getByRole("textbox", { name: "搜索模型" });
  await search.fill("隔离可用");
  await expect(picker.getByRole("option", { name: /隔离可用免费模型/ })).toBeVisible();
  await search.press("Enter");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
  await expect(draft).toHaveValue("等我选好模型再发送");
  await expect(search).toBeFocused();
  await search.press("Escape");
  await expect(trigger).toBeFocused();
  await page.getByRole("button", { name: "发送", exact: true }).press("Space");
  await expect.poll(() => page.evaluate(() => (window as any).__performance.sent.length)).toBe(1);
});

test("model search IME owns composition keys and normal navigation can choose an option", async ({ page }) => {
  const draft = page.getByLabel("发送消息", { exact: true });
  const trigger = page.getByRole("button", { name: "选择本次对话模型" });
  await draft.fill("保留这份独立草稿");
  await trigger.click();
  const picker = page.getByRole("dialog", { name: "本次对话模型" });
  const search = picker.getByRole("textbox", { name: "搜索模型" });
  const option = picker.getByRole("option", { name: /隔离可用免费模型/ });
  await expect(option).toBeEnabled();
  await expect(search).toBeFocused();
  const keyWasHandled = (key: string, composing = false) => search.evaluate((node, input) => {
    return !node.dispatchEvent(new KeyboardEvent("keydown", {
      key: input.key, isComposing: input.composing, bubbles: true, cancelable: true,
    }));
  }, { key, composing });
  expect(await keyWasHandled("ArrowDown", true)).toBe(false);
  await search.dispatchEvent("compositionstart");
  for (const key of ["ArrowUp", "ArrowDown", "Escape"]) {
    expect(await keyWasHandled(key)).toBe(false);
    await expect(search).toBeFocused();
  }
  await search.press("Enter");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
  await search.dispatchEvent("compositionend");
  await search.fill("隔离可用");
  await search.press("ArrowUp");
  await expect(option).toBeFocused();
  await option.press("Enter");
  await expect(picker).toHaveCount(0);
  await expect(trigger).toBeFocused();
  await expect(trigger).toContainText("隔离可用免费模型");
  await expect(draft).toHaveValue("保留这份独立草稿");
  expect(await page.evaluate(() => (window as any).__performance.sent)).toEqual([]);
});

async function syntheticMicrophone(page: Page) {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { getUserMedia: async () => ({ getTracks: () => [{ stop: () => { p.micStopped = (p.micStopped || 0) + 1; } }] }) } });
    (window as any).MediaRecorder = class {
      static isTypeSupported() { return true; }
      state = "inactive"; ondataavailable: any; onstop: any; onerror: any;
      start() { this.state = "recording"; }
      stop() { this.state = "inactive"; this.ondataavailable?.({ data: new Blob([new Uint8Array([26,69,223,163,0,0,0,0])], { type: "audio/webm" }) }); this.onstop?.(); }
    };
    p.values.assistant_voice_stage = (args: any) => { p.stage = args; return { recording_id: "synthetic-recording", source_path: "X:/isolated/assistant/recordings/input.webm", output_root: "X:/isolated/assistant/recordings/transcripts" }; };
    p.values.assistant_voice_discard = () => null;
    p.values.asr_environment_probe = ({ payload }: any) => { p.probe = payload; return { available: true }; };
    p.values.asr_task_start = ({ payload }: any) => { p.asr = payload; return { jobId: "original-asr", taskDir: "X:/isolated/assistant/recordings/transcripts/task" }; };
    p.values.asr_task_get = () => ({ job: { status: "success" }, artifacts: [{ relativePath: "runs/fixture/transcript.txt" }] });
    p.values.asr_task_read_artifact = () => ({ content: "这是一条隔离转写草稿。" });
    p.values.asr_task_cancel = () => null;
  });
}

test("recording uses owned local ASR paths and returns editable text without auto sending", async ({ page }) => {
  await syntheticMicrophone(page);
  await page.getByLabel("发送消息", { exact: true }).fill("原草稿");
  await page.getByRole("button", { name: "语音输入", exact: true }).click();
  await expect(page.getByText("正在录音", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "停止并转写", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("原草稿\n这是一条隔离转写草稿。");
  const result = await page.evaluate(() => { const p = (window as any).__performance; return { sent: p.sent, asr: p.asr, probe: p.probe, micStopped: p.micStopped }; });
  expect(result.sent).toHaveLength(0); expect(result.micStopped).toBeGreaterThan(0);
  expect(result.asr.outputRoot).toBe("X:/isolated/assistant/recordings/transcripts");
  expect(result.probe.outputRoot).toBe(result.asr.outputRoot); expect(result.asr.engineIds).toEqual(["faster_whisper_large_v3_cpu"]);
});

test("unknown ASR acceptance never submits a second transcription", async ({ page }) => {
  await syntheticMicrophone(page);
  await page.evaluate(() => { const p = (window as any).__performance; p.values.asr_task_start = () => { p.asrStarts = (p.asrStarts || 0) + 1; throw Error("合成受理未知"); }; });
  await page.getByRole("button", { name: "语音输入", exact: true }).click();
  await page.getByRole("button", { name: "停止并转写", exact: true }).click();
  await expect(page.getByText(/合成受理未知/)).toBeVisible();
  await page.getByRole("button", { name: "重试转写" }).click();
  await expect(page.getByText(/转写提交结果未知，未重复提交/)).toBeVisible();
  expect(await page.evaluate(() => (window as any).__performance.asrStarts)).toBe(1);
  expect(await page.evaluate(() => (window as any).__performance.sent.length)).toBe(0);
});

test("partial transcription is explicit and cannot silently replace the draft", async ({ page }) => {
  await syntheticMicrophone(page);
  await page.evaluate(() => { (window as any).__performance.values.asr_task_get = () => ({ job: { status: "partial", error: "合成部分转写，需核对原任务" }, artifacts: [{ relativePath: "runs/fixture/transcript.txt" }] }); });
  await page.getByLabel("发送消息", { exact: true }).fill("保留原草稿");
  await page.getByRole("button", { name: "语音输入", exact: true }).click();
  await page.getByRole("button", { name: "停止并转写", exact: true }).click();
  await expect(page.getByText(/合成部分转写/)).toBeVisible();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("保留原草稿");
  expect(await page.evaluate(() => (window as any).__performance.calls)).not.toContain("asr_task_read_artifact");
});

test("startup metadata races retry the original ASR query without resubmission", async ({ page }) => {
  await syntheticMicrophone(page);
  await page.evaluate(() => { const p = (window as any).__performance; p.values.asr_task_get = () => { p.asrReads = (p.asrReads || 0) + 1; if (p.asrReads <= 2) throw Error("合成元数据尚未发布"); return { job: { status: "success" }, artifacts: [{ relativePath: "runs/fixture/transcript.txt" }] }; }; });
  await page.getByRole("button", { name: "语音输入", exact: true }).click();
  await page.getByRole("button", { name: "停止并转写", exact: true }).click();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("这是一条隔离转写草稿。", { timeout: 10000 });
  const data = await page.evaluate(() => { const p = (window as any).__performance; return { reads: p.asrReads, starts: p.calls.filter((x: string) => x === "asr_task_start").length, sent: p.sent.length }; });
  expect(data.reads).toBe(3); expect(data.starts).toBe(1); expect(data.sent).toBe(0);
});

test("microphone denial gives a clear explanation and preserves input", async ({ page }) => {
  await page.evaluate(() => { Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { getUserMedia: () => Promise.reject(new DOMException("synthetic", "NotAllowedError")) } }); });
  await page.getByLabel("发送消息", { exact: true }).fill("不能丢失的草稿");
  await page.getByRole("button", { name: "语音输入", exact: true }).click();
  await expect(page.getByText(/未获得麦克风权限/)).toBeVisible();
  await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("不能丢失的草稿");
});

test("workspace fits narrow and dark layouts and produces current implementation screenshots", async ({ page }) => {
  for (const width of [1536, 1024, 390]) {
    await page.setViewportSize({ width, height: 920 });
    await expect(page.getByRole("button", { name: "发送", exact: true })).toBeInViewport();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  }
  await page.setViewportSize({ width: 1536, height: 1024 });
  await page.screenshot({ path: "outputs/page-design/assistant-chat/2026-11-15/implemented-empty.png" });
  await page.getByRole("button", { name: "选择本次对话模型" }).click();
  await page.screenshot({ path: "outputs/page-design/assistant-chat/2026-11-15/implemented-models.png" });
  await page.keyboard.press("Escape");
  await page.evaluate(() => document.documentElement.classList.add("dark"));
  await page.screenshot({ path: "outputs/page-design/assistant-chat/2026-11-15/implemented-dark.png" });
});
