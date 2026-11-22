// A previous binary creates synthetic DPAPI data; the new one reads and appends
// in the same fresh home. This never opens the real HomeBase profile or vault.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";

const parent = process.env.HOMEBASE_RECORDS_LAB;
const previous = process.env.HOMEBASE_RECORDS_PREVIOUS_BINARY;
const binary = process.env.HOMEBASE_RECORDS_BINARY;
assert(parent && previous && binary);
for (const file of [previous, binary]) {
  assert(path.isAbsolute(file));
  assert(path.resolve(file).startsWith(path.resolve(parent) + path.sep));
  assert(fs.statSync(file).isFile());
}
const reportPath = path.join(parent, "records-native-compatibility-v1.json");
assert(!fs.existsSync(reportPath), "Never overwrite compatibility evidence");
const home = fs.mkdtempSync(path.join(parent, "records-compat-"));
const env = {
  ...process.env,
  HOMEBASE_TEST_HOME: home,
  HOMEBASE_SECRETS_FILE: path.join(home, "secrets.yaml"),
  HOMEBASE_SECRETS_META: path.join(home, "secrets-meta.json"),
  HOMEBASE_AIHUB_ROOT: path.join(home, "AIHub"),
  HOMEBASE_MODEL_ROOT: path.join(home, "models"),
  HOMEBASE_RUNTIME_ROOT: path.join(home, "runtime"),
  HOMEBASE_MODEL_CACHE: path.join(home, "cache"),
  HOMEBASE_ASR_OUTPUT_ROOT: path.join(home, "output"),
  HOME: home,
  APPDATA: path.join(home, "AppData/Roaming"),
  LOCALAPPDATA: path.join(home, "AppData/Local"),
  TEMP: path.join(home, "tmp"),
  TMP: path.join(home, "tmp"),
  WEBVIEW2_USER_DATA_FOLDER: path.join(home, "wv"),
};
delete env.HOMEBASE_CONTROL_ENDPOINT;
for (const dir of [env.APPDATA, env.LOCALAPPDATA, env.TEMP]) fs.mkdirSync(dir, { recursive: true });
const hash = file => crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
let app;
function cli(args, input) {
  const run = spawnSync(process.execPath, ["scripts/homebase.mjs", ...args], {
    env, input: input === undefined ? undefined : JSON.stringify(input),
    encoding: "utf8", windowsHide: true, timeout: 30000, maxBuffer: 4 * 1024 * 1024,
  });
  assert(!run.error, run.error?.message);
  const value = JSON.parse(run.stdout);
  assert(value.ok, JSON.stringify(value.error));
  return value;
}
function call(command, args = {}) {
  const value = cli(["call", command, "--input", "-", "--allow-private", "--allow-write", "--allow-external", "--request-id", crypto.randomUUID(), "--wait-ms", "10000"], args);
  assert.equal(value.job.status, "succeeded");
  return value.result;
}
async function launch(file) {
  app = spawn(file, [], { env, cwd: process.cwd(), stdio: "ignore", windowsHide: true });
  for (let n = 0; n < 100; n++) {
    await pause(250);
    assert.equal(app.exitCode, null, "The owned isolated app stopped unexpectedly");
    try { cli(["doctor"]); return; } catch { /* Observe this process; never start a second one. */ }
  }
  throw Error("Owned isolated app did not become ready");
}
async function stop() {
  if (!app || app.exitCode !== null) return;
  const owned = app;
  spawnSync("taskkill", ["/PID", String(owned.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" });
  for (let n = 0; n < 80 && owned.exitCode === null; n++) await pause(50);
  assert.notEqual(owned.exitCode, null, "The owned app must stop before opening its test home again");
  app = undefined;
}
const inputA = { text: "旧数组格式隔离记录 A🙂", request_id: "records-compat-a", mode: "task" };
const send = request => call("assistant_send", { request });
const session = id => call("assistant_session_get", { sessionId: id });
const turn = id => call("assistant_turn_get", { turnId: id });
const stateFile = path.join(home, ".homebase/assistant/state.dpapi");
try {
  await launch(previous);
  const settings = call("assistant_config_get").settings;
  assert.equal(settings.enabled, false);
  assert.equal(call("wechat_status").bound, false);
  const a = send(inputA);
  assert.equal(a.state, "disabled");
  const b = send({ text: "旧数组格式隔离记录 B", request_id: "records-compat-b", mode: "task", session_id: a.session_id });
  assert.equal(b.state, "disabled");
  const oldView = session(a.session_id), oldTurn = turn(a.turn_id);
  assert.equal(oldView.messages.length, 4);
  assert.equal(oldView.turns.length, 2);
  await stop();
  const originalCipher = hash(stateFile);

  await launch(binary);
  assert.deepEqual(call("assistant_config_get").settings, settings);
  assert.deepEqual(session(a.session_id), oldView);
  assert.deepEqual(turn(a.turn_id), oldTurn);
  assert.deepEqual(send(inputA), a, "Original receipt must resolve without replay or migration");
  assert.equal(hash(stateFile), originalCipher, "Reading old arrays must not rewrite their DPAPI state");
  const c = send({ text: "新集合追加的隔离记录 C", request_id: "records-compat-c", mode: "task", session_id: a.session_id });
  assert.equal(c.state, "disabled");
  const appended = session(a.session_id);
  assert.equal(appended.messages.length, 6);
  assert.equal(appended.turns.length, 3);
  assert.deepEqual(appended.messages.slice(0, 4), oldView.messages);
  assert.deepEqual(turn(a.turn_id), oldTurn);
  const appendedCipher = hash(stateFile);
  await stop();

  await launch(binary);
  assert.deepEqual(session(a.session_id), appended);
  assert.deepEqual(send(inputA), a);
  assert.equal(hash(stateFile), appendedCipher);
  assert.deepEqual(call("assistant_config_get").settings, settings);
  assert.equal(call("wechat_status").bound, false);
  const report = {
    passed: true, isolated_home: home, previous_binary: previous, previous_binary_sha256: hash(previous),
    binary, binary_sha256: hash(binary), old_dpapi_arrays_read_without_rewrite: true,
    old_messages_turns_and_original_receipt_preserved: true, append_and_second_process_restart: true,
    messages: 6, turns: 3, model_disabled_unchanged: true, real_model_calls: 0, real_wechat_messages: 0,
  };
  fs.writeFileSync(reportPath, JSON.stringify(report, null, 2), { flag: "wx" });
  console.log(JSON.stringify(report));
} finally { await stop(); }
