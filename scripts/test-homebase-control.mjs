import test, { before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import { contract, generated } from "./generate-control-contract.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const cli = path.join(root, "scripts/homebase.mjs");
const binary = process.env.HOMEBASE_CONTROL_FIXTURE_BIN;
assert.ok(
  binary && path.isAbsolute(binary) && fs.existsSync(binary),
  "Use npm run test:control: fixture binary must be explicitly supplied; no real app fallback.",
);
const directory = fs.mkdtempSync(
  path.join(os.tmpdir(), "homebase-control-test-"),
);
const stateDirectory = path.join(directory, "state");
fs.mkdirSync(stateDirectory);
const endpointFile = path.join(stateDirectory, "endpoint.json");
const env = {};
for (const name of ["SystemRoot", "WINDIR", "PATH"])
  if (process.env[name]) env[name] = process.env[name];
for (const name of [
  "HOME",
  "USERPROFILE",
  "APPDATA",
  "LOCALAPPDATA",
  "XDG_CONFIG_HOME",
  "TEMP",
  "TMP",
])
  env[name] = directory;
env.HOMEBASE_CONTROL_ENDPOINT = endpointFile;
let server;
async function start() {
  server = spawn(binary, [stateDirectory], {
    env,
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "",
    errors = "";
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(
      () => reject(new Error("Synthetic control fixture did not become ready")),
      15000,
    );
    server.stdout.on("data", (bytes) => {
      output += bytes;
      if (output.includes("READY")) {
        clearTimeout(timeout);
        resolve();
      }
    });
    server.stderr.on("data", (bytes) => {
      errors += bytes;
    });
    server.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    server.on("exit", (code) => {
      clearTimeout(timeout);
      reject(new Error(`Synthetic fixture exited ${code}: ${errors}`));
    });
  });
}
async function stop() {
  if (server && server.exitCode == null) {
    const done = once(server, "exit");
    server.kill();
    await done;
  }
}
before(start);
after(async () => {
  await stop();
  const resolved = path.resolve(directory);
  assert.ok(
    path.dirname(resolved) === path.resolve(os.tmpdir()) &&
      path.basename(resolved).startsWith("homebase-control-test-"),
  );
  fs.rmSync(resolved, { recursive: true, force: true });
});

async function run(args, input, overrides = {}, cliFile = cli) {
  const child = spawn(process.execPath, [cliFile, ...args], {
    cwd: directory,
    env: { ...env, ...overrides },
    windowsHide: true,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "",
    stderr = "";
  child.stdout.on("data", (value) => (stdout += value));
  child.stderr.on("data", (value) => (stderr += value));
  child.stdin.end(
    input == null
      ? undefined
      : typeof input === "string"
        ? input
        : JSON.stringify(input),
  );
  const [code] = await once(child, "exit");
  let json;
  try {
    json = JSON.parse(stdout);
  } catch {
    json = null;
  }
  return { code, stdout, stderr, json };
}
const call = (name, options = [], args) =>
  run(
    ["call", name, ...(args === undefined ? [] : ["--input", "-"]), ...options],
    args,
  );

test("generated catalog matches every registered command and policy", () => {
  const result = contract();
  for (const name of ["capabilities_catalog", "capability_result"]) {
    const command = result.commands.find(command => command.name === name);
    assert.equal(command?.status, "available");
    assert.deepEqual(command?.requires, ["private"]);
  }
  for (const name of ["capability_install", "capability_render"]) {
    const command = result.commands.find(command => command.name === name);
    assert.equal(command?.status, "available");
    assert.deepEqual(command?.requires, ["write", "external"]);
  }
  for (const name of ["asr_task_reconcile", "model_download_reconcile"]) {
    const command = result.commands.find(command => command.name === name);
    assert.deepEqual(command.requires, ["private"]);
  }
  assert.equal(
    result.commands.filter((command) => command.status === "unavailable")
      .length,
    4,
  );
  assert.deepEqual(
    result.commands.filter(command => command.status === "blocked").map(command => command.name).sort(),
    [
      "assistant_artifact_export", "assistant_artifact_image_preview", "assistant_confirm", "assistant_local_config_save",
      "secrets_reveal", "secrets_set", "secrets_set_meta", "shared_secrets_consent_resolve", "shared_secrets_ipc_endpoint",
      "shared_secrets_open_yaml", "wechat_forget", "wechat_login_cancel", "wechat_login_pin", "wechat_pairing",
    ],
  );
  assert.deepEqual(result.commands.find(command => command.name === "assistant_turn_snapshot")?.requires, ["private"]);
  assert.deepEqual(result.commands.find(command => command.name === "wechat_login_begin")?.requires, ["write", "private", "external"]);
  assert.deepEqual(result.commands.find(command => command.name === "assistant_send")?.requires, ["write", "private", "external"]);
  assert.deepEqual(result.commands.find(command => command.name === "assistant_followup_save")?.requires, ["write", "private", "external"]);
  assert.deepEqual(result.commands.find(command => command.name === "assistant_followup_cancel")?.requires, ["write", "private"]);
  assert.deepEqual(result.commands.find(command => command.name === "assistant_matters_get")?.requires, ["private"]);
  for(const name of ["assistant_matter_save","assistant_matter_review_ack"])
    assert.deepEqual(result.commands.find(command => command.name === name)?.requires,["write","private"]);
  for (const [filename, text] of generated())
    assert.equal(fs.readFileSync(path.join(root, filename), "utf8"), text);
});
test("new commands, duplicate policies and unsupported signatures require explicit review", () => {
  const project = path.join(directory, "generator");
  fs.mkdirSync(path.join(project, "src-tauri/src/commands"), {
    recursive: true,
  });
  fs.mkdirSync(path.join(project, "control"));
  fs.writeFileSync(path.join(project,"control/interface-guide.json"), JSON.stringify({modules:{example:{purpose:"Synthetic interface",before:[],completion:"Synthetic completion"}}}));
  const lib = path.join(project, "src-tauri/src/lib.rs"),
    policy = path.join(project, "control/policy.json"),
    source = path.join(project, "src-tauri/src/commands/example.rs");
  fs.writeFileSync(lib, "tauri::generate_handler![commands::example::one,])");
  fs.writeFileSync(
    source,
    "pub async fn one() -> Result<(), String> { Ok(()) }",
  );
  fs.writeFileSync(
    policy,
    JSON.stringify({ schema_version: 1, read: ["one"] }),
  );
  assert.equal(contract(project).commands.length, 1);
  fs.writeFileSync(
    lib,
    "tauri::generate_handler![commands::example::one,commands::example::unreviewed,])",
  );
  assert.throws(() => contract(project), /Review/);
  fs.writeFileSync(lib, "tauri::generate_handler![commands::example::one,])");
  fs.writeFileSync(
    policy,
    JSON.stringify({ schema_version: 1, read: ["one"], write: ["one"] }),
  );
  assert.throws(() => contract(project), /Duplicate policy/);
  fs.writeFileSync(
    policy,
    JSON.stringify({ schema_version: 1, read: ["one"] }),
  );
  fs.writeFileSync(
    source,
    "pub async fn one(app: &AppHandle) -> Result<(), String> { Ok(()) }",
  );
  assert.throws(() => contract(project), /Unsupported command argument/);
});
test("bootstrap and command discovery work outside the repository without exposing authentication", async () => {
  const boot = await run(["bootstrap"]);
  assert.equal(boot.code, 0);
  assert.match(boot.stdout, /control[\\/]START.md/);
  assert.ok(boot.stdout.includes(path.resolve(root)));
  const list = await run(["commands", "--module", "asr"]);
  assert.equal(list.code, 0);
  assert.ok(list.json.commands.every((command) => command.module === "asr"));
  const description = await run(["describe", "asr_task_start"]);
  assert.equal(description.json.command.parameters[0].name, "payload");
  const token = JSON.parse(fs.readFileSync(endpointFile, "utf8")).token;
  assert.ok(!boot.stdout.includes(token));
  assert.ok(!list.stdout.includes(token));
});
test("doctor verifies the real isolated native service", async () => {
  const result = await run(["doctor"]);
  assert.equal(result.code, 0);
  assert.equal(result.json.service, "homebase-control");
  assert.equal(result.json.state_healthy, true);
});
test("failed native jobs expose their original error through call, replay and list", async () => {
  const first = await call("system_disks", ["--request-id", "synthetic-query-failure"]);
  assert.equal(first.code, 1);
  assert.equal(first.json.error.code, "QUERY_INVALID");
  assert.equal(first.json.job.result_available, true);
  const replay = await call("system_disks", ["--request-id", "synthetic-query-failure", "--detach"]);
  assert.equal(replay.code, 0);
  assert.equal(replay.json.reused, true);
  assert.equal(replay.json.job.id, first.json.job.id);
  assert.equal(replay.json.job.status, "failed");
  const fetched = await run(["jobs", "get", first.json.job.id]);
  assert.equal(fetched.code, 1);
  assert.equal(fetched.json.error.code, "QUERY_INVALID");
  const list = await run(["jobs", "list"]);
  assert.equal(list.code, 0);
  assert.ok(list.json.jobs.some(job => job.id === first.json.job.id && job.status === "failed"));
});
test("missing and foreign endpoints never fall back to real user state", async () => {
  const missing = await run(["status"], null, {
    HOMEBASE_CONTROL_ENDPOINT: path.join(directory, "absent.json"),
  });
  assert.equal(missing.json.error.code, "OFFLINE");
  const foreign = path.join(directory, "foreign.json");
  fs.writeFileSync(
    foreign,
    JSON.stringify({
      protocol: 1,
      endpoint: "192.0.2.1:1234",
      token: "a".repeat(64),
      instance_id: "synthetic",
    }),
  );
  const rejected = await run(["status"], null, {
    HOMEBASE_CONTROL_ENDPOINT: foreign,
  });
  assert.equal(rejected.json.error.code, "ENDPOINT_INVALID");
});
test("read calls use production transport and complete job tracking", async () => {
  const result = await call("system_info");
  assert.equal(result.code, 0);
  assert.equal(result.json.result.os, "synthetic-fixture");
  assert.equal(result.json.job.status, "succeeded");
});
test("credential references require private scope and never expose a key-export entry", async () => {
  const denied = await call("service_credentials_status");
  assert.equal(denied.json.error.code, "SCOPE_REQUIRED");
  const metadata = await call("service_credentials_status", ["--allow-private"]);
  assert.equal(metadata.code, 0);
  assert.equal(metadata.json.result_state, "partial");
  assert.equal(metadata.json.result[0].credential_ref, "api-key-manager:fal");
  assert.equal(metadata.json.result[1].state, "not_configured");
  assert.ok(metadata.json.result.every(row => !Object.hasOwn(row, "value") && !Object.hasOwn(row, "masked_value")));
  const exportAttempt = await call("resolve_api_key");
  assert.equal(exportAttempt.json.error.code, "UNKNOWN_COMMAND");
});
test("write, private and external scopes are explicit", async () => {
  assert.equal(
    (await call("agents_set_model", [], { id: "codex", model: "synthetic" }))
      .json.error.code,
    "SCOPE_REQUIRED",
  );
  assert.equal(
    (await call("shared_mcp_get")).json.error.code,
    "SCOPE_REQUIRED",
  );
  assert.equal(
    (await call("openrouter_models_catalog")).json.error.code,
    "SCOPE_REQUIRED",
  );
  const result = await call("openrouter_models_catalog", ["--allow-external"]);
  assert.equal(result.code, 0);
  assert.equal(result.json.result.source, "synthetic-no-network");
});
test("credential fields are redacted and references remain usable", async () => {
  const result = await call("shared_mcp_get", ["--allow-private"]);
  assert.equal(result.code, 0);
  assert.ok(!result.stdout.includes("synthetic-hidden"));
  assert.equal(result.json.result.api_key, "[redacted]");
  assert.equal(result.json.result.env.REFERENCE, "${secrets.sample}");
});
test("unavailable stubs and human credential operations are never claimed successful", async () => {
  assert.equal((await call("tasks_list")).json.error.code, "NOT_IMPLEMENTED");
  assert.equal(
    (await call("secrets_reveal", [], { shortName: "sample_key" })).json.error
      .code,
    "HUMAN_REQUIRED",
  );
  assert.equal(
    (
      await call("secrets_save", ["--allow-write"], {
        request: { value: "synthetic-never-written" },
      })
    ).json.error.code,
    "HUMAN_REQUIRED",
  );
});
test("unknown, missing and conflicting aliases fail before the business runner", async () => {
  assert.equal(
    (await call("agents_status", [], {})).json.error.code,
    "BAD_ARGUMENT",
  );
  assert.equal(
    (await call("agents_status", [], { id: "codex", unknown: 1 })).json.error
      .code,
    "BAD_ARGUMENT",
  );
  assert.equal(
    (
      await call("secrets_rotate_url", [], {
        shortName: "sample_key",
        short_name: "sample_key",
      })
    ).json.error.code,
    "BAD_ARGUMENT",
  );
});
test("write recovery uses the same request id without executing twice", async () => {
  const options = ["--allow-write", "--request-id", "operation-once"];
  const args = { id: "codex", model: "synthetic-model" };
  const first = await call("agents_set_model", options, args);
  const second = await call("agents_set_model", options, args);
  assert.equal(first.code, 0);
  assert.equal(second.code, 0);
  assert.equal(first.json.job.id, second.json.job.id);
  assert.equal(first.json.result.calls, second.json.result.calls);
  const conflict = await call("agents_set_model", options, {
    ...args,
    model: "different",
  });
  assert.equal(conflict.json.error.code, "REQUEST_CONFLICT");
});
test("detach and waiting timeout do not imply completion or cancel the operation", async () => {
  const result = await call(
    "studio_chat_send",
    [
      "--allow-write",
      "--allow-external",
      "--request-id",
      "synthetic-pending",
      "--wait-ms",
      "0",
    ],
    { payload: {} },
  );
  assert.equal(result.code, 3);
  assert.equal(result.json.pending, true);
  const id = result.json.job.id;
  await new Promise((resolve) => setTimeout(resolve, 1000));
  const finished = await run([
    "jobs",
    "get",
    id,
    "--allow-write",
    "--allow-external",
  ]);
  assert.equal(finished.code, 0);
  assert.ok(finished.json.result.task_id);
  assert.equal(finished.json.result_state, "in_progress");
  const detached = await call("system_info", ["--detach"]);
  assert.equal(detached.code, 0);
  assert.equal(detached.json.accepted, true);
});
test("documentation-only capsule completes takeover, task polling and paginated streaming result", async () => {
  const capsule=path.join(directory,'documentation-only');
  for(const file of ['AGENTS.md','hb.cmd','control/START.md','control/OPERATIONS.md','control/reference.md','control/commands.json','control/bootstrap.template.txt','scripts/homebase.mjs','scripts/control-bootstrap.mjs','scripts/control-result.mjs','scripts/control-validation.mjs']){
    const target=path.join(capsule,file);fs.mkdirSync(path.dirname(target),{recursive:true});fs.copyFileSync(path.join(root,file),target);
  }
  assert.equal(fs.existsSync(path.join(capsule,'src-tauri')),false);
  const client=(args,input)=>run(args,input,{},path.join(capsule,'scripts/homebase.mjs'));
  const boot=await client(['bootstrap']);assert.equal(boot.code,0);assert.ok(boot.stdout.includes(capsule));
  assert.equal((await client(['doctor'])).json.ok,true);
  const description=await client(['describe','studio_chat_send']);
  assert.ok(description.json.command.input_schema);assert.ok(description.json.command.output_schema);
  const payload={payload:{user_message:'Synthetic documentation-only task; no keys, files or external services.'}};
  assert.equal((await client(['validate','studio_chat_send','--input','-'],payload)).json.executed,false);
  const options=['--allow-write','--allow-external'];
  const submitted=await client(['call','studio_chat_send','--input','-',...options,'--request-id','docs-only-capsule','--detach'],payload);
  let task;
  for(let i=0;i<30;i++){
    const value=await client(['jobs','get',submitted.json.job.id,...options]);
    if(value.json.job.result_available){task=value.json.job.result.task_id;break;}
    await new Promise(resolve=>setTimeout(resolve,100));
  }
  assert.ok(task);
  let cursor=0;const events=[];let complete=false;
  const contract=(await client(['describe','task_events_get'])).json.command;
  for(let i=0;i<30;i++){
    const input={topic:'studio:event',taskId:task,afterSeq:cursor,limit:1};
    assert.equal((await client(['validate','task_events_get','--input','-'],input)).code,0);
    const page=(await client(['call','task_events_get','--allow-private','--input','-'],input)).json.result;
    const {validateSchema}=await import('./control-validation.mjs');
    assert.deepEqual(validateSchema(contract.output_schema,page,contract.output_schema.$defs),[]);
    assert.equal(page.history_lost,false);assert.equal(page.truncated,false);
    events.push(...page.events);cursor=page.next_seq;
    if(page.settled && !page.has_more){assert.equal(page.status,'succeeded');complete=true;break;}
    await new Promise(resolve=>setTimeout(resolve,100));
  }
  assert.ok(complete);assert.equal(events.find(e=>e.payload.kind==='assistant_text').payload.text,'Synthetic documentation-only result');
});

test("model routes bind channel-specific references and reject mismatches without real credentials", async () => {
  const request={route_id:"openrouter::openai/fixture",input:{messages:[{role:"user",content:"synthetic"}]} ,output_root:null};
  const plan=await call("model_capability_resolve",["--allow-private"],{request});
  assert.equal(plan.json.result.credential.credential_ref,"sops:fixture_openrouter");
  assert.ok(plan.json.result.endpoint.startsWith("https://openrouter.ai/"));
  assert.equal(plan.json.result.route.publisher,"openai");
  const mismatch=await call("model_capability_resolve",["--allow-private"],{request:{...request,input:{...request.input,model:"wrong-model"}}});
  assert.equal(mismatch.json.ok,false);
  const unscoped=await call("model_capability_call",[],{request});assert.equal(unscoped.json.error.code,"SCOPE_REQUIRED");
  const result=await call("model_capability_call",["--allow-private","--allow-write","--allow-external"],{request});assert.equal(result.json.result.status,"succeeded");
  const failed=await call("model_capability_call",["--allow-private","--allow-write","--allow-external"],{request:{...request,input:{...request.input,forceFailure:true}}});assert.equal(failed.code,1);assert.equal(failed.json.error.code,"BUSINESS_FAILED");
});

test("invalid input and request ids are not echoed", async () => {
  const invalid = await run(
    ["call", "agents_status", "--input", "-"],
    "synthetic-private-invalid-json",
  );
  assert.equal(invalid.json.error.code, "BAD_ARGUMENT");
  assert.ok(!invalid.stdout.includes("synthetic-private-invalid-json"));
  const id = await call("system_info", [
    "--request-id",
    "invalid\nsynthetic-private",
  ]);
  assert.equal(id.json.error.code, "USAGE");
  assert.ok(!id.stderr.includes("synthetic-private"));
});
test("invalid authentication is reported without printing token values", async () => {
  const original = JSON.parse(fs.readFileSync(endpointFile, "utf8"));
  const filename = path.join(directory, "wrong-token.json");
  fs.writeFileSync(
    filename,
    JSON.stringify({ ...original, token: "b".repeat(64) }),
  );
  const result = await run(["status"], null, {
    HOMEBASE_CONTROL_ENDPOINT: filename,
  });
  assert.equal(result.json.error.code, "UNAUTHORIZED");
  assert.ok(!result.stdout.includes(original.token));
});
test("operation journal excludes all synthetic parameter and result bodies", () => {
  const journal = fs.readFileSync(
    path.join(stateDirectory, "operations.jsonl"),
    "utf8",
  );
  for (const text of [
    "synthetic-hidden",
    "synthetic-model",
    "synthetic-never-written",
    "synthetic-private-invalid-json",
  ])
    assert.ok(!journal.includes(text));
});
test("restarting preserves deduplication and explicitly reports lost memory results", async () => {
  await stop();
  await start();
  const replay = await call(
    "agents_set_model",
    ["--allow-write", "--request-id", "operation-once"],
    { id: "codex", model: "synthetic-model" },
  );
  assert.equal(replay.code, 1);
  assert.equal(replay.json.error.code, "RESULT_UNAVAILABLE");
  const fresh = await call("system_info");
  assert.equal(fresh.json.result.calls, 1);
});
