// Official protocol against a real local Codex binary and a synthetic loopback model.
// No account/auth directories, real credentials, or internet model calls are used.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import http from "node:http";
import os from "node:os";
import readline from "node:readline";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
const root = path.resolve(
  process.env.HOMEBASE_NATIVE_LAB ||
    path.join(os.tmpdir(), "homebase-codex-protocol"),
);
fs.mkdirSync(root, { recursive: true });
const profile = path.join(root, `profile-${crypto.randomUUID()}`);
fs.mkdirSync(profile);
const binary =
  process.env.HOMEBASE_CODEX_BINARY ||
  path.join(
    process.env.APPDATA,
    "npm-global/node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/codex/codex.exe",
  );
assert(fs.existsSync(binary), "Explicit native Codex binary required");
let modelCalls = 0,
  toolCalls = 0;
const requests = [];
function sse(res, type, fields) {
  res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...fields })}\n\n`);
}
const server = http.createServer(async (req, res) => {
  let body = "";
  for await (const chunk of req) body += chunk;
  if (!req.url.startsWith("/v1/responses")) {
    res.writeHead(404).end();
    return;
  }
  const input = JSON.parse(body);
  modelCalls++;
  requests.push({
    tools: input.tools?.map((t) => t.name),
    has_tool_output: JSON.stringify(input.input).includes(
      "isolated-tool-proof",
    ),
  });
  res.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
  });
  const response = {
    id: `resp_${modelCalls}`,
    object: "response",
    status: "in_progress",
    output: [],
  };
  sse(res, "response.created", { response });
  const item =
    modelCalls === 1
      ? {
          id: "fc_fixture",
          type: "function_call",
          call_id: "fixture-call",
          name: "homebase",
          arguments: JSON.stringify({ action: "context" }),
        }
      : {
          id: "msg_fixture",
          type: "message",
          role: "assistant",
          status: "completed",
          content: [{ type: "output_text", text: "隔离原生协议已验证。" }],
        };
  sse(res, "response.output_item.added", { output_index: 0, item });
  if (item.type === "message")
    sse(res, "response.output_text.delta", {
      item_id: item.id,
      output_index: 0,
      content_index: 0,
      delta: item.content[0].text,
    });
  sse(res, "response.output_item.done", { output_index: 0, item });
  sse(res, "response.completed", {
    response: {
      ...response,
      status: "completed",
      output: [item],
      usage: { input_tokens: 20, output_tokens: 8, total_tokens: 28 },
    },
  });
  res.end();
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
fs.writeFileSync(
  path.join(profile, "config.toml"),
  `model = "gpt-5.1-codex"\nmodel_provider = "fixture"\napproval_policy = "never"\nsandbox_mode = "read-only"\n[model_providers.fixture]\nname = "Isolated loopback fixture"\nbase_url = "http://127.0.0.1:${port}/v1"\nwire_api = "responses"\nrequires_openai_auth = false\n`,
  "utf8",
);
const env = {
  CODEX_HOME: profile,
  HOME: profile,
  USERPROFILE: profile,
  APPDATA: path.join(profile, "AppData/Roaming"),
  LOCALAPPDATA: path.join(profile, "AppData/Local"),
  TEMP: profile,
  TMP: profile,
};
for (const key of ["SystemRoot", "WINDIR", "COMSPEC", "PATH", "PATHEXT"])
  if (process.env[key]) env[key] = process.env[key];
const child = spawn(binary, ["app-server", "-c", "mcp_servers={}"], {
  env,
  cwd: profile,
  windowsHide: true,
  stdio: ["pipe", "pipe", "pipe"],
});
let serial = 0;
const pending = new Map();
const notices = [];
let stderr = "";
child.stderr.on("data", (chunk) => {
  stderr = (stderr + chunk.toString()).slice(-6000);
});
const lines = readline.createInterface({ input: child.stdout });
const send = (v) => child.stdin.write(`${JSON.stringify(v)}\n`);
lines.on("line", (line) => {
  let m;
  try {
    m = JSON.parse(line);
  } catch {
    return;
  }
  if (m.method && m.id !== undefined) {
    if (m.method === "item/tool/call") {
      toolCalls++;
      assert.equal(m.params.tool, "homebase");
      send({
        id: m.id,
        result: {
          success: true,
          contentItems: [
            {
              type: "inputText",
              text: JSON.stringify({
                version: "homebase-context-v1",
                proof: "isolated-tool-proof",
              }),
            },
          ],
        },
      });
    } else send({ id: m.id, result: { decision: "decline", answers: {} } });
  } else if (m.id !== undefined) {
    const task = pending.get(m.id);
    pending.delete(m.id);
    if (m.error) task?.reject(Error(JSON.stringify(m.error)));
    else task?.resolve(m.result);
  } else notices.push(m);
});
function rpc(method, params) {
  return new Promise((resolve, reject) => {
    const id = ++serial;
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(Error(`Timed out: ${method}`));
    }, 30_000);
    pending.set(id, {
      resolve: (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      reject: (e) => {
        clearTimeout(timer);
        reject(e);
      },
    });
    send({ id, method, params });
  });
}
try {
  await rpc("initialize", {
    clientInfo: { name: "homebase_protocol_test", version: "0.1.0" },
    capabilities: { experimentalApi: true },
  });
  send({ method: "initialized", params: {} });
  const created = await rpc("thread/start", {
    cwd: profile,
    sandbox: "read-only",
    approvalPolicy: "never",
    dynamicTools: [
      {
        name: "homebase",
        description: "Isolated fixture",
        inputSchema: {
          type: "object",
          properties: { action: { type: "string" } },
          required: ["action"],
          additionalProperties: false,
        },
      },
    ],
  });
  const id = created.thread.id;
  assert(id);
  await rpc("turn/start", {
    threadId: id,
    input: [
      {
        type: "text",
        text: "使用 homebase 工具读取隔离上下文，然后简短回复。",
      },
    ],
  });
  const deadline = Date.now() + 60_000;
  let terminal;
  while (Date.now() < deadline) {
    terminal = notices.find((n) => n.method === "turn/completed");
    if (terminal) break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert(
    terminal,
    `No completion; synthetic model calls=${modelCalls}; native stderr bytes=${stderr.length}`,
  );
  assert.equal(
    terminal.params.turn.status,
    "completed",
    JSON.stringify(terminal.params.turn.error),
  );
  assert.equal(toolCalls, 1);
  assert.equal(modelCalls, 2);
  assert(requests[1].has_tool_output);
  const history = await rpc("thread/read", {
    threadId: id,
    includeTurns: true,
  });
  assert(history.thread.turns.length >= 1);
  const resumed = await rpc("thread/resume", {
    threadId: id,
    sandbox: "read-only",
    approvalPolicy: "never",
  });
  assert.equal(resumed.thread.id, id);
  assert.equal(
    modelCalls,
    2,
    "Reading/resuming must not automatically call model",
  );
  const report = {
    passed: true,
    native_binary: path.basename(binary),
    isolated_profile: profile,
    model_calls: modelCalls,
    dynamic_tool_calls: toolCalls,
    original_thread_preserved: true,
    read_and_resume_do_not_infer: true,
    real_keys_used: false,
    external_model_calls: false,
  };
  fs.writeFileSync(
    path.join(root, "report.json"),
    JSON.stringify(report, null, 2),
  );
  console.log(JSON.stringify(report));
} finally {
  child.kill();
  lines.close();
  await new Promise((resolve) => server.close(resolve));
}
