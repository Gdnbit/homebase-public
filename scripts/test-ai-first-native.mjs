// Full public CLI -> native HomeBase -> real Codex -> synthetic local model -> HomeBase tool roundtrip.
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import crypto from "node:crypto";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { protectedNativeTest, protectedSend, replayPreAckClient, verifyPreAckRecovery, seedUnknownNative, verifyUnknownNative,verifyNativeLinkFault } from "./native-receipt-isolation.mjs";
import {chromium} from "@playwright/test";
const parent = process.env.HOMEBASE_AI_FIRST_LAB;
const binary = process.env.HOMEBASE_AI_FIRST_BIN;
const reportName=process.env.HOMEBASE_AI_FIRST_REPORT_NAME||(protectedNativeTest?"native-receipts-codex-validation.json":"ai-first-native-report.json");
const verifyWebview=protectedNativeTest&&process.env.HOMEBASE_NATIVE_SKIP_WEBVIEW!=="1";
const presentationTest=protectedNativeTest&&process.env.HOMEBASE_NATIVE_PRESENTATION_TEST==='1';
assert(/^[a-zA-Z0-9.-]+\.json$/.test(reportName));
assert(parent && path.isAbsolute(parent) && binary && path.isAbsolute(binary));
if(protectedNativeTest)assert(path.resolve(binary).startsWith(path.resolve(parent)+path.sep));
fs.mkdirSync(parent, { recursive: true });
const lab = fs.mkdtempSync(path.join(parent, "native-flow-"));
const profile = path.join(lab, ".codex");
fs.mkdirSync(profile);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let modelCalls = 0,
  toolOutput = false;
const server = http.createServer(async (req, res) => {
  let body = "";
  for await (const chunk of req) body += chunk;
  if (!req.url.startsWith("/v1/responses")) {
    res.writeHead(404).end();
    return;
  }
  const input = JSON.parse(body);
  modelCalls++;
  const returned = (input.input ?? []).some(
    (item) =>
      item.type === "function_call_output" &&
      JSON.stringify(item.output).includes("homebase-context-v1"),
  );
  toolOutput ||= returned;
  const toolText=(input.input ?? []).filter(i=>i.type==="function_call_output").map(i=>typeof i.output==="string" ? i.output : (i.output ?? []).map(c=>c.text ?? "").join("\n")).join("\n");
  const delegatedId=/"turn_id"\s*:\s*"([a-zA-Z0-9-]+)"/.exec(toolText)?.[1];
  res.writeHead(200, { "content-type": "text/event-stream" });
  const event = (type, data) =>
    res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
  const response = {
    id: `fixture_${modelCalls}`,
    status: "in_progress",
    output: [],
  };
  event("response.created", { response });
  const item = modelCalls<=3
    ? {
        id: `call-${modelCalls}`,
        type: "function_call",
        call_id: `owned-call-${modelCalls}`,
        name: "homebase",
        arguments: JSON.stringify(modelCalls===1 ? {action:"context",query:"隔离测试"} : modelCalls===2 ? {action:"task",text:"只查询当前系统状态，不修改数据"} : {action:"status",turn_id:delegatedId}),
      }
    : {
        id: "fixture-message",
        type: "message",
        role: "assistant",
        status: "completed",
        content: [
          {
            type: "output_text",
            text: "共享执行器尚未启用模型，任务没有执行；我已核对原任务状态。",
          },
        ],
      };
  event("response.output_item.added", { output_index: 0, item });
  if (item.type==="message")
    event("response.output_text.delta", {
      item_id: item.id,
      output_index: 0,
      content_index: 0,
      delta: item.content[0].text,
    });
  event("response.output_item.done", { output_index: 0, item });
  event("response.completed", {
    response: {
      ...response,
      status: "completed",
      output: [item],
      usage: { input_tokens: 20, output_tokens: 8, total_tokens: 28 },
    },
  });
  res.end();
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
let debugPort;
if(protectedNativeTest){
  const lease=http.createServer();await new Promise(r=>lease.listen(0,"127.0.0.1",r));debugPort=lease.address().port;await new Promise(r=>lease.close(r));
}
fs.writeFileSync(
  path.join(profile, "config.toml"),
  `model="gpt-5.1-codex"\nmodel_provider="fixture"\napproval_policy="never"\nsandbox_mode="read-only"\n[model_providers.fixture]\nname="HomeBase isolated fixture"\nbase_url="http://127.0.0.1:${server.address().port}/v1"\nwire_api="responses"\nrequires_openai_auth=false\n`,
  "utf8",
);
const env = {
  ...Object.fromEntries(["SystemRoot","WINDIR","COMSPEC","PATH","PATHEXT"].filter(k=>process.env[k]).map(k=>[k,process.env[k]])),
  // WebView requires the registered Windows profile location. Native SDK
  // children replace it with their own test profile in Client::spawn.
  USERPROFILE:process.env.USERPROFILE??lab, HOME:lab, APPDATA:path.join(lab,"AppData/Roaming"), LOCALAPPDATA:path.join(lab,"AppData/Local"), TEMP:lab, TMP:lab,
  HOMEBASE_TEST_HOME: lab,
  HOMEBASE_SECRETS_FILE: path.join(lab, "secrets.yaml"),
  HOMEBASE_SECRETS_META: path.join(lab, "secrets-meta.json"),
  HOMEBASE_AIHUB_ROOT: path.join(lab, "AIHub"),
  HOMEBASE_MODEL_ROOT: path.join(lab, "models"),
  HOMEBASE_RUNTIME_ROOT: path.join(lab, "runtime"),
  HOMEBASE_MODEL_CACHE: path.join(lab, "cache"),
  HOMEBASE_ASR_OUTPUT_ROOT: path.join(lab, "output"),
  WEBVIEW2_USER_DATA_FOLDER: path.join(lab, "webview-profile"),
};
delete env.HOMEBASE_CONTROL_ENDPOINT;
if(protectedNativeTest)env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=`--remote-debugging-port=${debugPort}`;
if(process.env.HOMEBASE_NATIVE_TEST_WEBVIEW_DIR){
  const directory=process.env.HOMEBASE_NATIVE_TEST_WEBVIEW_DIR;
  assert(path.isAbsolute(directory)&&fs.existsSync(path.join(directory,"msedgewebview2.exe")),"Explicit installed WebView runtime directory required");
  env.WEBVIEW2_BROWSER_EXECUTABLE_FOLDER=directory;
}
let app;
let serial = 0;
function cli(args) {
  const r = spawnSync(process.execPath, ["scripts/homebase.mjs", ...args], {
    env,
    cwd: process.cwd(),
    windowsHide: true,
    encoding: "utf8",
    timeout: 15000,
  });
  const v = JSON.parse(r.stdout);
  assert(v.ok, JSON.stringify(v.error));
  return v;
}
async function call(command, args = {}) {
  const file = path.join(lab, `input-${++serial}.json`);
  fs.writeFileSync(file, JSON.stringify(args));
  let v = cli([
    "call",
    command,
    "--input",
    file,
    "--allow-private",
    "--allow-write",
    "--allow-external",
    "--request-id",
    `ai-first-${serial}`,
    "--wait-ms",
    "1000",
  ]);
  for (
    let i = 0;
    i < 120 && ["queued", "running"].includes(v.job.status);
    i++
  ) {
    await sleep(100);
    v = cli([
      "jobs",
      "get",
      v.job.id,
      "--allow-private",
      "--allow-write",
      "--allow-external",
    ]);
  }
  assert.equal(v.job.status, "succeeded", JSON.stringify(v.job.error));
  return v.job.result;
}
async function start() {
  app = spawn(binary, [], {
    env,
    cwd: process.cwd(),
    windowsHide: true,
    stdio: "ignore",
  });
  for (let i = 0; i < 150; i++) {
    const endpoint = path.join(lab, ".homebase/control/endpoint.json");
    if (fs.existsSync(endpoint)) {
      try {
        if (JSON.parse(fs.readFileSync(endpoint, "utf8")).pid === app.pid)
          return;
      } catch {}
    }
    assert.equal(app.exitCode, null);
    await sleep(200);
  }
  throw Error("Isolated HomeBase did not start");
}
function stop() {
  if (app?.pid && app.exitCode === null) {
    if (process.platform === "win32")
      spawnSync("taskkill", ["/PID", String(app.pid), "/T", "/F"], {
        windowsHide: true,
        stdio: "ignore",
      });
    else app.kill();
  }
}
try {
  await start();
  cli(["doctor"]);
  const empty = await call("conversation_native", {
    request: { action: "status", thread_id: "not-loaded" },
  });
  assert.equal(empty.state, "not_loaded");
  const providers = await call("conversation_providers");
  assert(providers.providers.find((p) => p.id === "codex").interactive);
  const input = {
    request: { action: "create", request_id: "isolated-native-create" },
  };
  const created = await call("conversation_native", input);
  assert(created.thread.id);
  assert.equal(
    (await call("conversation_native", input)).thread.id,
    created.thread.id,
  );
  assert.equal(
    (await call("assistant_sessions_list")).length,
    1,
    "Repeated creation must not create a second linked assistant session",
  );
  const request = {
    request: {
      action: "send",
      thread_id: created.thread.id,
      text: "读取隔离 HomeBase 上下文",
      request_id: "isolated-native-send",
    },
  };
  const protectedResult=protectedNativeTest?await protectedSend(call,"codex",created.thread.id,request.request.text,request.request.request_id):null;
  const sent = protectedResult?.ack ?? await call("conversation_native", request);
  assert.equal(sent.state, "accepted");
  const repeated = await call("conversation_native", request);
  assert.equal(repeated.turn.id, sent.turn.id);
  let status;
  for (let i = 0; i < 100; i++) {
    status = await call("conversation_native", {
      request: { action: "status", thread_id: created.thread.id },
    });
    if (["completed", "failed", "unknown"].includes(status.state)) break;
    await sleep(150);
  }
  assert.equal(status.state, "completed", JSON.stringify(status));
  assert.equal(modelCalls, 4);
  assert(toolOutput);
  const delegated=await call("assistant_session_get",{sessionId:created.assistant_session_id});assert.equal(delegated.turns.length,1);assert.equal(delegated.turns[0].state,"disabled");
  assert(
    !("turns" in status),
    "Live status must not repeatedly return full history",
  );
  const read = await call("conversation_native", {
    request: { action: "read", thread_id: created.thread.id },
  });
  assert.equal(read.thread.id, created.thread.id);
  assert(read.thread.turns.length > 0);
  if(presentationTest){
    assert.equal(read.assistant_session_id,created.assistant_session_id);assert.equal(read.context_link_state,'linked');
    const first=read.thread.turns.flatMap(t=>t.items).find(i=>i.type==='userMessage');
    assert.equal(first.homebase_input_blocks[0],request.request.text);assert(first.homebase_input_blocks.length>=2,'Original SDK supplemental input must remain available');
    assert.equal(first.content.map(b=>b.text??'').join(''),first.homebase_input_blocks.join(''),'SDK raw input must remain byte-identical');
    const history=await call('agents_history_all');const summary=history.entries.find(s=>s.agent_id==='codex'&&s.session_id===created.thread.id);
    assert.equal(summary?.first_user_prompt,request.request.text,'SDK environment records must not become the conversation title');
    assert(path.resolve(read.thread.path).startsWith(path.resolve(profile,'sessions')+path.sep));
    const sourceHash=crypto.createHash('sha256').update(fs.readFileSync(read.thread.path)).digest('hex');
    await verifyNativeLinkFault(call,lab,protectedResult.request,'unavailable');
    await verifyNativeLinkFault(call,lab,protectedResult.request,'not_linked');
    assert.equal((await call('conversation_native',{request:{action:'read',thread_id:created.thread.id}})).assistant_session_id,created.assistant_session_id);
    assert.equal(crypto.createHash('sha256').update(fs.readFileSync(read.thread.path)).digest('hex'),sourceHash,'Presentation and binding lookup must not rewrite SDK history');
  }
  const context = await call("assistant_context_get", {
    sessionId: created.assistant_session_id,
    query: "隔离测试",
  });
  assert.equal(context.version, "homebase-context-v1");
  const usage = await call("assistant_usage_get");
  assert.equal(usage.reported_cost_usd, null);
  const before = await call("assistant_config_get");
  assert.equal(before.settings.enabled, false);
  const index = await call("capability_index_get");
  assert.equal(index.version, "homebase-capability-index-v1");
  assert(!index.issues.some(issue=>issue.source==="repositories"),"Repository aggregation must respect the registry page contract, including an empty isolated registry");
  assert.equal(modelCalls, 4, "Read/context/index/usage must never infer");
  stop();
  await sleep(600);
  const clientFixture=protectedNativeTest?replayPreAckClient(lab,protectedResult.request,app.pid,"后来输入的合成草稿"):null;
  await start();
  if(clientFixture){
    await verifyPreAckRecovery(call,lab,clientFixture,sent);
    assert.equal((await call("conversation_native",{request:{action:"status",thread_id:created.thread.id}})).state,"not_loaded","Receipt recovery must not initialize the native SDK");
  }
  const restored = await call("conversation_native", request);
  assert.equal(restored.turn.id, sent.turn.id);
  assert.equal(modelCalls, 4, "Restart must not replay original native send");
  if(protectedNativeTest){
    if(verifyWebview){
    stop();await sleep(600);
    replayPreAckClient(lab,protectedResult.request,app.pid,"UI恢复后仍保留的合成新稿");
    await start();
    let browser;
    for(let i=0;i<80&&!browser;i++){
      try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${debugPort}`,{timeout:2000});}
      catch{assert.equal(app.exitCode,null,"Owned test host exited before its WebView was ready");await sleep(250);}
    }
    assert(browser,"Owned test WebView CDP endpoint unavailable");
    try{
      const page=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().startsWith("devtools:"));assert(page);
      await page.getByLabel("更多对话操作").click();
      await page.getByRole("button",{name:"未发送与待核对",exact:true}).click();
      await page.getByRole("button",{name:new RegExp(`codex · ${created.thread.id.slice(0,8)}`)}).click();
      await page.waitForFunction(()=>document.getElementById("native-message")?.value==="UI恢复后仍保留的合成新稿"&&!document.querySelector('[aria-label="原生发送回执"]'));
      const entry=(await call("assistant_desktop_get",{scope:`native:codex:${created.thread.id}`})).entry;
      assert.equal(entry.native_pending,null);assert.equal(entry.body.text,"UI恢复后仍保留的合成新稿");
      if(presentationTest){
        const pane=page.getByLabel('原生对话消息');
        await pane.getByText(request.request.text,{exact:true}).waitFor();
        const context=pane.locator('pre').filter({hasText:'HomeBase 自动背景快照'});
        assert(await context.count()>0);assert.equal(await context.first().isVisible(),false);
        const details=context.first().locator('..');
        assert.equal(await details.evaluate(e=>e.tagName),'DETAILS');
        await details.locator('summary').click();assert(await context.first().isVisible());
        await details.locator('summary').click();
        if(await page.getByLabel('查看原生最新消息').count())await page.getByLabel('查看原生最新消息').click();
        await page.getByRole('link',{name:'查看共享执行任务、结果与待确认操作',exact:true}).waitFor();
        assert.equal(await page.getByRole('link',{name:'查看共享执行任务、结果与待确认操作',exact:true}).getAttribute('href'),`/assistant?session=${created.assistant_session_id}`);
      }
      await page.screenshot({path:path.join(lab,"native-client-recovery.png")});
      assert.equal(modelCalls,4,"Opening the recovered native UI must not run another model turn");
    }finally{await browser.close();}
    }
    stop();await sleep(600);
    const unknown=seedUnknownNative(lab,"codex",created.thread.id,app.pid);
    await start();await verifyUnknownNative(call,lab,unknown);
    assert.equal((await call("conversation_native",{request:{action:"status",thread_id:created.thread.id}})).state,"not_loaded");
    assert.equal(modelCalls,4,"Unknown native receipts must never reissue a model turn");
  }
  const after = await call("assistant_config_get");
  assert.deepEqual(after.settings, before.settings);
  const report = {
    passed: true,
    protected_native_send:protectedNativeTest,
    client_checkpoint_fault:protectedNativeTest?"replayed-client-pre-ack-state":null,
    client_restart_and_unknown_no_replay:protectedNativeTest,
    native_webview_ack_recovery:verifyWebview,
    native_presentation_verified:presentationTest,
    binary_sha256:crypto.createHash("sha256").update(fs.readFileSync(binary)).digest("hex"),
    lab,
    native_thread: created.thread.id,
    real_codex: true,
    dynamic_homebase_context: true,
    delegated_task_and_status_use_shared_engine:true,
    disabled_model_boundary_preserved:true,
    model_calls: modelCalls,
    duplicate_create_and_send_reused: true,
    restart_does_not_replay: true,
    live_status_excludes_history: true,
    default_model_still_disabled: true,
    real_keys_used: false,
    real_wechat_messages_sent: 0,
  };
  fs.writeFileSync(
    path.join(parent, reportName),
    JSON.stringify(report, null, 2), {flag:"wx"},
  );
  console.log(JSON.stringify(report));
} finally {
  stop();
  server.closeAllConnections();
  await new Promise((r) => server.close(r));
}
