import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import net from "node:net";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { chromium } from "@playwright/test";

const parent = process.env.HOMEBASE_DESKTOP_LAB;
const binary = process.env.HOMEBASE_DESKTOP_BINARY;
const reportName = process.env.HOMEBASE_DESKTOP_REPORT_NAME || "desktop-drafts-native-validation.json";
assert(/^[a-zA-Z0-9.-]+\.json$/.test(reportName), "A simple lab report filename is required");
assert(parent && path.isAbsolute(parent), "Explicit absolute lab required");
assert(binary && path.isAbsolute(binary), "Explicit staged binary required");
assert(path.resolve(binary).startsWith(path.resolve(parent) + path.sep));
assert(fs.existsSync(binary));
const home = fs.mkdtempSync(path.join(parent, "desktop-native-"));
const server = net.createServer();
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
await new Promise(resolve => server.close(resolve));
const env = { ...process.env, HOMEBASE_TEST_HOME: home,
  HOMEBASE_SECRETS_FILE: path.join(home, "secrets.yaml"), HOMEBASE_SECRETS_META: path.join(home, "secrets-meta.json"),
  HOMEBASE_AIHUB_ROOT: path.join(home, "AIHub"), HOMEBASE_MODEL_ROOT: path.join(home, "models"),
  HOMEBASE_RUNTIME_ROOT: path.join(home, "runtime"), HOMEBASE_MODEL_CACHE: path.join(home, "cache"),
  HOMEBASE_ASR_OUTPUT_ROOT: path.join(home, "output"),
  WEBVIEW2_USER_DATA_FOLDER: path.join(home, "webview-profile"),
  WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port}` };
delete env.HOMEBASE_CONTROL_ENDPOINT;
let app, browser;
let normalCloseVerified = false;
let bufferedCloseVerified = false;
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const hash = data => crypto.createHash("sha256").update(data).digest("hex");
function cli(args, input, expectedFailure = false) {
  const result = spawnSync(process.execPath, ["scripts/homebase.mjs", ...args], { env, input: input === undefined ? undefined : JSON.stringify(input), encoding: "utf8", timeout: 45000, maxBuffer: 4 * 1024 * 1024, windowsHide: true });
  assert(!result.error, result.error?.message);
  const value = JSON.parse(result.stdout);
  if (expectedFailure) { assert.equal(value.ok, false); assert.equal(value.error?.code, "BUSINESS_FAILED"); }
  else assert(value.ok, JSON.stringify(value.error));
  return value;
}
function call(command, args = {}, expectedFailure = false) {
  const value = cli(["call", command, "--input", "-", "--allow-private", "--allow-write", "--allow-external", "--request-id", crypto.randomUUID(), "--wait-ms", "30000"], args, expectedFailure);
  assert.equal(value.job.status, "succeeded", JSON.stringify(value.job.error)); return value.job.result;
}
async function launch() {
  app = spawn(binary, [], { env, cwd: process.cwd(), windowsHide: true, stdio: "ignore" });
  for (let i = 0; i < 120; i++) {
    await pause(250);
    try { if (cli(["doctor"]).ok) return; } catch {}
    assert(app.exitCode === null, "Isolated native application exited");
  }
  throw Error("Isolated app unavailable");
}
async function stop() {
  if (browser) { await browser.close(); browser = null; }
  if (app?.exitCode === null) { app.kill(); await new Promise(resolve => app.once("exit", resolve)); }
}
try {
  await launch();
  const originalConfig = call("assistant_config_get").settings;
  assert.equal(originalConfig.enabled, false);
  const empty = call("assistant_desktop_get", { scope: "new" }); assert.equal(empty.entry.revision, 0);
  const body = { text: "真实桌面跨重启的隔离草稿", route: "", mode: "task", files: [{ name: "fixture.md", content: "仅合成数据，蓝色圆点。" }] };
  const saved = call("assistant_desktop_save", { request: { scope: "new", revision: 0, body } });
  assert.equal(saved.status, "saved");
  const same = call("assistant_desktop_save", { request: { scope: "new", revision: 0, body } }); assert.equal(same.entry.revision, saved.entry.revision);
  const conflict = call("assistant_desktop_save", { request: { scope: "new", revision: 0, body: { ...body, text: "禁止过时覆盖" } } }, true); assert.equal(conflict.status, "failed");
  const desktopRoot = path.join(home, ".homebase/assistant/desktop");
  const file = path.join(desktopRoot, hash("new") + ".draft.dpapi");
  assert(!fs.readFileSync(file).includes(Buffer.from(body.text)));
  const savedHash = hash(fs.readFileSync(file));
  await stop(); await launch();
  assert.deepEqual(call("assistant_desktop_get", { scope: "new" }).entry.body, body);
  assert.equal(hash(fs.readFileSync(file)), savedHash, "Reading after restart must not rewrite ciphertext");
  assert.deepEqual(call("assistant_config_get").settings, originalConfig);
  assert.deepEqual(call("assistant_sessions_list"), []);
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  const page = browser.contexts().flatMap(context => context.pages()).find(page => !page.url().startsWith("devtools:"));
  assert(page, "Native WebView unavailable");
  await page.getByRole("heading", { name: "AI 助理", exact: true }).waitFor();
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await page.getByLabel("发送消息", { exact: true }).waitFor();
  await page.waitForFunction(text => document.getElementById("assistant-message")?.value === text, body.text);
  assert.equal(await page.getByLabel("消息方式").inputValue(), "task");
  assert(await page.getByLabel("待发送附件").getByText("fixture.md").isVisible());
  assert(await page.getByLabel("发送消息", { exact: true }).isEnabled());
  assert(await page.getByRole("button", { name: "发送", exact: true }).isDisabled());
  await page.screenshot({ path: path.join(home, "native-draft-restored.png") });
  if (process.env.HOMEBASE_DESKTOP_SKIP_CLOSE !== "1") {
    // Model stays disabled. Real draft editing and close/save do not need a provider.
    const lastEdit = body.text + "\n关闭前尚未等待缓冲的修改";
    await page.getByLabel("发送消息", { exact: true }).fill(lastEdit);
    assert((await page.getByLabel("草稿保存状态").textContent()).includes("正在保存草稿"));
    await page.evaluate(() => window.__TAURI_INTERNALS__.invoke("plugin:window|close", { label: "main" }));
    for (let i = 0; i < 40 && app.exitCode === null; i++) await pause(250);
    assert(app.exitCode !== null, "Normal close handler did not finish");
    normalCloseVerified = true;
  }
  await stop(); await launch();
  const beforeSend = call("assistant_desktop_get", { scope: "new" }).entry;
  if (normalCloseVerified) {
    assert.equal(beforeSend.body.text, body.text + "\n关闭前尚未等待缓冲的修改");
    bufferedCloseVerified = true;
  }
  // Draft capacity is intentionally larger than the execution message budget.
  const longText = "长中文草稿🙂".repeat(1600);
  const longBody = { ...body, text: longText };
  assert(Buffer.byteLength(longText) > 8000 && Buffer.byteLength(longText) < 65536);
  const longSaved = call("assistant_desktop_save", { request: { scope:"new", revision:beforeSend.revision, body:longBody } });
  assert.equal(longSaved.entry.body.text, longText);
  const longHash = hash(fs.readFileSync(file));
  await stop(); await launch();
  assert.equal(call("assistant_desktop_get", {scope:"new"}).entry.body.text, longText);
  assert.equal(hash(fs.readFileSync(file)), longHash);
  const overMessage = call("assistant_desktop_send", {request:{session_id:null,text:longText,request_id:"long-draft-not-a-send",mode:"chat"},revision:longSaved.entry.revision}, true);
  assert.equal(overMessage.status,"failed"); assert(overMessage.message.includes("8000"));
  assert.equal(call("assistant_desktop_get", {scope:"new"}).entry.pending, null);
  assert.deepEqual(call("assistant_sessions_list"), []);
  assert.equal(hash(fs.readFileSync(file)), longHash);
  const oversize = call("assistant_desktop_save", {request:{scope:"new",revision:longSaved.entry.revision,body:{...body,text:"中".repeat(22000)}}},true);
  assert.equal(oversize.status,"failed"); assert(oversize.message.includes("64 KiB"));
  assert.equal(hash(fs.readFileSync(file)),longHash);
  const nativeLongScope = "native:gemini-cli:opaque_long_draft_01";
  call("assistant_desktop_save", {request:{scope:nativeLongScope,revision:0,body:{text:longText,route:"",mode:"auto",files:[]}}});
  await stop(); await launch();
  assert.equal(call("assistant_desktop_get", {scope:nativeLongScope}).entry.body.text,longText);
  const request = { session_id: null, text: "原编号隔离发送", request_id: "native-original-receipt", mode: "chat", attachments: body.files };
  const first = call("assistant_desktop_send", { request, revision: longSaved.entry.revision });
  assert.equal(first.status, "accepted"); assert.equal(first.ack.state, "disabled");
  assert.equal(first.entry.pending, null); assert.equal(first.entry.body.text, "");
  await stop(); await launch();
  const recovered = call("assistant_desktop_resolve", { scope: "new", request });
  assert.equal(recovered.ack.turn_id, first.ack.turn_id);
  const duplicate = call("assistant_desktop_send", { request, revision: 0 }); assert.equal(duplicate.ack.turn_id, first.ack.turn_id);
  const changed = call("assistant_desktop_send", { request: { ...request, text: "同编号不可修改" }, revision: 0 }, true); assert.equal(changed.code, "RECEIPT_CONFLICT");
  assert.equal(call("assistant_sessions_list").length, 1);
  assert.deepEqual(call("assistant_config_get").settings, originalConfig);
  const scope = first.ack.session_id, corruptPath = path.join(desktopRoot, hash(scope) + ".draft.dpapi");
  fs.writeFileSync(corruptPath, Buffer.from("SYNTHETIC-CORRUPT-ENVELOPE"));
  const corruptHash = hash(fs.readFileSync(corruptPath));
  assert.equal(call("assistant_desktop_get", { scope }, true).status, "failed");
  assert.equal(call("assistant_desktop_save", { request: { scope, revision: 0, body } }, true).status, "failed");
  assert.equal(hash(fs.readFileSync(corruptPath)), corruptHash);
  const index = call("assistant_desktop_index").index; assert.equal(index.complete, false); assert.equal(index.unreadable, 1);
  const result = { passed: true, binary, binary_sha256: hash(fs.readFileSync(binary)), isolated_home: home,
    restart_restores_protected_input: true, native_webview_restores_files_and_mode: true,
    long_chinese_draft_restart: true, native_long_draft_restart: true,
    oversized_message_never_prepared: true, over_capacity_preserves_cipher: true,
    normal_close_verified: normalCloseVerified, buffered_close_save_verified: bufferedCloseVerified,
    restart_original_ack_reused: true, stale_revision_blocked: true,
    corrupt_cipher_preserved: true, index_incomplete_explicit: true, settings_unchanged: true,
    real_model_calls: 0, real_wechat_test_messages: 0 };
  fs.writeFileSync(path.join(parent, reportName), JSON.stringify(result, null, 2), { flag: "wx" });
  console.log(JSON.stringify(result));
} finally { await stop(); }
