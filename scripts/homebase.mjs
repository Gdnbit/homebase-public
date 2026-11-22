#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import net from "node:net";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { bootstrapPrompt } from "./control-bootstrap.mjs";
let inspectResult, validJob;

const root = fileURLToPath(new URL("../", import.meta.url));
let catalog;
class Failure extends Error {
  constructor(code, message, details) {
    super(message);
    this.code = code;
    this.details = details;
  }
}
const fail = (code, message, details) => {
  throw new Failure(code, message, details);
};
const output = (value) =>
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const help = `HomeBase CLI — local authenticated control, JSON output

  hb bootstrap                         Print the AI startup prompt (offline)
  hb doctor                            Check CLI, contract and app connectivity
  hb status                            App/control protocol status
  hb commands [--module NAME]           List real command availability (offline)
  hb describe COMMAND                  Full JSON schemas, examples and completion rules
  hb validate COMMAND --input FILE|-   Validate arguments offline; never executes
  hb call COMMAND [--input FILE|-]      Call an existing HomeBase operation
       [--allow-write] [--allow-private] [--allow-external]
       [--request-id ID] [--detach] [--wait-ms 30000]
  hb jobs list                         List operation metadata
  hb jobs get ID [--allow-...]          Read result; repeat with same job ID to poll
  hb rollout status --allow-private    Inspect local update activity without submitting a job
  hb documents import --file PDF --document-id UUID [--scope new|SESSION_UUID]
       --allow-write --allow-private   Import/resume local PDF bytes; never parses or sends to a model
  hb history read-turn --session-id ID --turn-id ID --allow-private
                                       Print the verified JSON snapshot of one original turn

Use --input - to read a JSON argument object from stdin. No shell fragments or
credential values in command-line arguments. Scopes reflect existing task
authorization; they do not grant permission beyond the user's request.
The desktop app must be running for live operations. Local help remains usable.
`;

function parseOptions(tokens, allowed) {
  const result = {};
  const boolean = new Set([
    "allow-write",
    "allow-private",
    "allow-external",
    "detach",
  ]);
  for (let index = 0; index < tokens.length; index++) {
    const key = tokens[index].replace(/^--/, "");
    if (
      !tokens[index].startsWith("--") ||
      !allowed.includes(key) ||
      key in result
    )
      fail("USAGE", "未知或重复选项；运行 hb help 查看用法");
    if (boolean.has(key)) result[key] = true;
    else {
      const value = tokens[++index];
      if (value == null || value.startsWith("--")) fail("USAGE", "选项缺少值");
      result[key] = value;
    }
  }
  return result;
}
function grants(options) {
  return ["write", "private", "external"].filter(
    (scope) => options[`allow-${scope}`],
  );
}
function command(name) {
  const entry = catalog.commands.find((entry) => entry.name === name);
  if (!entry) fail("UNKNOWN_COMMAND", "命令未注册，请运行 hb commands");
  return entry;
}
function endpoint() {
  const filename =
    process.env.HOMEBASE_CONTROL_ENDPOINT ||
    path.join(
      process.env.HOMEBASE_TEST_HOME || os.homedir(),
      ".homebase",
      "control",
      "endpoint.json",
    );
  let value;
  try {
    if (fs.statSync(filename).size > 16384)
      fail("ENDPOINT_INVALID", "本机端点文件过大");
    value = JSON.parse(fs.readFileSync(filename, "utf8"));
  } catch (error) {
    if (error instanceof Failure) throw error;
    fail(
      "OFFLINE",
      "HomeBase 控制端点不可用。请从当前项目启动更新后的桌面应用，再运行 hb doctor。",
    );
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) fail("ENDPOINT_INVALID", "拒绝格式无效的本机端点，认证信息未发送");
  const match = /^127\.0\.0\.1:(\d{1,5})$/.exec(value.endpoint || "");
  if (
    value.protocol !== 1 ||
    !match ||
    Number(match[1]) < 1 ||
    Number(match[1]) > 65535 ||
    !/^[a-f0-9]{64}$/.test(value.token || "") ||
    typeof value.instance_id !== "string"
  )
    fail("ENDPOINT_INVALID", "拒绝无效或非本机端点，认证信息未发送");
  return { ...value, port: Number(match[1]) };
}

async function rpc(method, fields = {}) {
  const ep = endpoint();
  const id = crypto.randomUUID();
  const packet = `${JSON.stringify({ protocol: 1, id, token: ep.token, method, ...fields })}\n`;
  if (Buffer.byteLength(packet) > 1024 * 1024)
    fail("REQUEST_LIMIT", "请求超过 1MiB，请改用文件路径参数");
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: "127.0.0.1", port: ep.port });
    const parts = [];
    let size = 0,
      settled = false;
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      error ? reject(error) : resolve(value);
    };
    socket.setTimeout(10000);
    socket.on("timeout", () =>
      finish(
        new Failure(
          "TRANSPORT_TIMEOUT",
          "控制连接超时；若已提交操作，请用原 request-id 查询，禁止改编号盲目重试",
        ),
      ),
    );
    socket.on("error", () =>
      finish(
        new Failure(
          "OFFLINE",
          "无法连接本机控制服务；请检查应用是否运行及端点是否过期",
        ),
      ),
    );
    socket.on("connect", () => socket.write(packet));
    socket.on("data", (bytes) => {
      size += bytes.length;
      if (size > 8 * 1024 * 1024)
        return finish(new Failure("RESULT_LIMIT", "控制响应超过 8MiB"));
      parts.push(bytes);
      if (!bytes.includes(10)) return;
      try {
        const response = JSON.parse(
          Buffer.concat(parts).toString("utf8").split("\n")[0],
        );
        if (response.protocol !== 1)
          return finish(
            new Failure("PROTOCOL_MISMATCH", "服务端控制协议不兼容"),
          );
        if (
          !response.ok &&
          ["UNAUTHORIZED", "BAD_REQUEST", "REQUEST_LIMIT"].includes(
            response.error?.code,
          )
        )
          return finish(
            new Failure(response.error.code, response.error.message),
          );
        if (response.id !== id || typeof response.ok !== "boolean")
          return finish(
            new Failure("BAD_RESPONSE", "服务端响应编号或格式无效"),
          );
        if (!response.ok)
          return finish(
            new Failure(
              response.error?.code || "REMOTE_ERROR",
              response.error?.message || "操作失败",
            ),
          );
        if (!Object.hasOwn(response, "result"))
          return finish(new Failure("BAD_RESPONSE", "成功响应缺少 result，不能判定已取得结果"));
        finish(null, response.result);
      } catch {
        finish(new Failure("BAD_RESPONSE", "控制服务返回无效 JSON"));
      }
    });
    socket.on("end", () =>
      finish(
        new Failure(
          "DISCONNECTED",
          "连接在确认结果前关闭；请保留原 request-id 核对操作",
        ),
      ),
    );
  });
}

async function input(filename) {
  if (!filename) return {};
  let raw;
  if (filename === "-") {
    const chunks = [];
    let size = 0;
    for await (const chunk of process.stdin) {
      size += chunk.length;
      if (size > 1024 * 1024) fail("REQUEST_LIMIT", "输入超过 1MiB");
      chunks.push(chunk);
    }
    raw = Buffer.concat(chunks).toString("utf8");
  } else {
    if (fs.statSync(filename).size > 1024 * 1024)
      fail("REQUEST_LIMIT", "输入文件超过 1MiB");
    raw = fs.readFileSync(filename, "utf8");
  }
  let value;
  try {
    value = JSON.parse(raw.replace(/^\uFEFF/, ""));
  } catch {
    fail("BAD_ARGUMENT", "输入必须是有效 UTF-8 JSON；输入内容未回显");
  }
  if (!value || typeof value !== "object" || Array.isArray(value))
    fail("BAD_ARGUMENT", "输入必须是 JSON 对象");
  return value;
}
function renderJob(job) {
  if (!validJob(job, true)) fail("BAD_RESPONSE", "任务状态或结果格式无效，不能判定任务完成");
  if (job.status === "succeeded" && job.result_available) {
    const outcome = inspectResult(job.result);
    if (outcome.result_state === "reported_failure") {
      process.exitCode = 1;
      return { ok: false, job, result: job.result, ...outcome, error: { code: "BUSINESS_FAILED", message: "控制调用已结束，但业务返回失败或中断" } };
    }
    return { ok: true, job, result: job.result, ...outcome };
  }
  if (job.status === "running") {
    process.exitCode = 3;
    return { ok: true, pending: true, job, next: `hb jobs get ${job.id}` };
  }
  process.exitCode = 1;
  return {
    ok: false,
    job,
    error: job.error || {
      code:
        job.status === "interrupted_unknown"
          ? "OUTCOME_UNKNOWN"
          : "RESULT_UNAVAILABLE",
      message:
        "操作状态或结果不可完整恢复。请核对业务任务和输出，不要自动重跑。",
    },
  };
}

async function waitForJob(jobId, scopes, wait) {
  const deadline = Date.now() + wait;
  do {
    const job = await rpc("jobs.get", { job_id: jobId, grants: scopes });
    if (!validJob(job, true)) fail("BAD_RESPONSE", "任务状态或结果格式无效，请保留原编号核对");
    if (job.status !== "running" || Date.now() >= deadline) return job;
    await sleep(200);
  } while (true);
}

async function readHistoryTurn(sessionId, turnId, scopes) {
  if (!scopes.includes("private")) fail("SCOPE_REQUIRED", "读取原任务详情需要 --allow-private");
  if ([sessionId, turnId].some(id => !id || Buffer.byteLength(id) > 256 || /\p{Cc}/u.test(id)))
    fail("USAGE", "须提供实际的 --session-id 和 --turn-id");
  async function read(name, args) {
    const entry = command(name);
    if (entry.status !== "available" || entry.requires.some(scope => !scopes.includes(scope)))
      fail("SCOPE_REQUIRED", "原轮次快照接口不可用或缺少授权");
    const requestId = crypto.randomUUID();
    const accepted = await rpc("call", {
      command: name, args, grants: scopes, request_id: requestId, catalog_hash: catalog.catalog_hash,
    });
    if (!validJob(accepted?.job)) fail("BAD_RESPONSE", "历史读取响应缺少有效任务状态");
    const job = await waitForJob(accepted.job.id, scopes, 30000);
    if (job.status !== "succeeded" || !job.result_available)
      fail(job.error?.code || "HISTORY_READ_INCOMPLETE", "原快照读取未完成，未输出部分正文", { job_id: job.id, request_id: requestId });
    return job.result;
  }
  const snapshot = await read("assistant_turn_snapshot", { sessionId, turnId });
  const [item] = Array.isArray(snapshot?.items) ? snapshot.items : [];
  if (snapshot?.version !== "homebase-history-v1" || snapshot.session_id !== sessionId ||
      snapshot.kind !== "turns" || typeof snapshot.snapshot_id !== "string" ||
      !/^[a-f0-9]{8}(-[a-f0-9]{4}){3}-[a-f0-9]{12}$/i.test(snapshot.snapshot_id) ||
      snapshot.total !== 1 || !Array.isArray(snapshot.items) || snapshot.items.length !== 1 || snapshot.complete !== true ||
      snapshot.next_offset !== null || item?.id !== turnId || item.turn_id !== turnId ||
      !Number.isSafeInteger(item.json_bytes) || item.json_bytes < 1 || item.json_bytes > 64 * 1024 * 1024 ||
      typeof item.json_sha256 !== "string" || !/^[a-f0-9]{64}$/.test(item.json_sha256))
    fail("BAD_RESPONSE", "历史快照身份或容量不一致，未读取其他任务");
  const chunks = [];
  const hash = crypto.createHash("sha256");
  let offset = 0;
  while (offset < item.json_bytes) {
    const chunk = await read("assistant_history_read", {
      request: { session_id: sessionId, snapshot_id: snapshot.snapshot_id, item_id: turnId, offset, limit: 512 * 1024 },
    });
    if (chunk?.version !== snapshot.version || chunk.session_id !== sessionId || chunk.kind !== "turns" ||
        chunk.snapshot_id !== snapshot.snapshot_id || chunk.item_id !== turnId || chunk.format !== "turn_view_json" ||
        chunk.json_sha256 !== item.json_sha256 || chunk.total_bytes !== item.json_bytes ||
        chunk.offset !== offset || typeof chunk.content !== "string")
      fail("BAD_RESPONSE", "历史分块身份或校验信息不一致，未输出部分正文");
    const bytes = Buffer.from(chunk.content, "utf8");
    const next = offset + bytes.length;
    const complete = next === item.json_bytes;
    if (!bytes.length || bytes.length > 512 * 1024 || next > item.json_bytes || chunks.length >= 1024 ||
        chunk.complete !== complete || chunk.next_offset !== (complete ? null : next))
      fail("BAD_RESPONSE", "历史分块的偏移或结束标记不一致，未输出部分正文");
    hash.update(bytes);
    chunks.push(bytes);
    offset = next;
  }
  if (hash.digest("hex") !== item.json_sha256) fail("BAD_RESPONSE", "历史正文 SHA-256 校验失败，未输出部分正文");
  const body = Buffer.concat(chunks, offset);
  let view;
  try { view = JSON.parse(body.toString("utf8")); }
  catch { fail("BAD_RESPONSE", "历史正文无法解析，未输出部分正文"); }
  if (view?.id !== turnId || view.session_id !== sessionId)
    fail("BAD_RESPONSE", "完整正文不属于原轮次，未输出其他任务");
  process.stdout.write(body);
}

async function main() {
  const [action = "help", ...rest] = process.argv.slice(2);
  if (["help", "--help", "-h"].includes(action)) {
    process.stdout.write(help);
    return;
  }
  if (action === "bootstrap") {
    if (rest.length) fail("USAGE", "bootstrap 不接受选项");
    process.stdout.write(bootstrapPrompt(root));
    return;
  }
  // Job observation uses the protocol receipt and does not resolve commands.
  if (action !== "jobs") {
    try {
      catalog = JSON.parse(fs.readFileSync(path.join(root, "control/commands.json"), "utf8"));
      if (!Array.isArray(catalog.commands) || typeof catalog.catalog_hash !== "string") throw Error();
    } catch { fail("CATALOG_INVALID", "命令目录缺失或格式无效，请恢复项目契约后重试"); }
  }
  try { ({ inspectResult, validJob } = await import("./control-result.mjs")); }
  catch { fail("CLI_DEPENDENCY_INVALID", "CLI 结果处理模块缺失或无效，请恢复 scripts/control-result.mjs"); }
  if (action === "history") {
    if (rest[0] !== "read-turn") fail("USAGE", "用法：hb history read-turn --session-id ID --turn-id ID --allow-private");
    const options = parseOptions(rest.slice(1), ["session-id", "turn-id", "allow-private"]);
    await readHistoryTurn(options["session-id"], options["turn-id"], grants(options));
    return;
  }
  if(action==='documents') {
    if(rest[0]!=='import')fail('USAGE','用法：hb documents import --file PDF --document-id UUID --scope new --allow-write --allow-private');
    const options=parseOptions(rest.slice(1),['file','document-id','scope','allow-write','allow-private']);
    const scopes=grants(options);
    if(!options.file||!options['document-id'])fail('USAGE','导入需要--file和稳定的--document-id；中断后保留同一文件及编号');
    if(!scopes.includes('write')||!scopes.includes('private'))fail('SCOPE_REQUIRED','导入私人文档需要--allow-write和--allow-private；不启用模型或云端上传');
    const {importPdf,DocumentImportError}=await import('./document-import.mjs');
    const result=await importPdf({filename:options.file,scope:options.scope||'new',documentId:options['document-id'],onIdentity:value=>process.stderr.write(`HomeBase document-id: ${value.document_id}\n`),call:async(name,args)=>{
      const entry=command(name);
      if(entry.status!=='available'||entry.requires.some(scope=>!scopes.includes(scope)))fail('SCOPE_REQUIRED','文档命令不可用或缺少授权');
      const requestId=crypto.randomUUID();
      process.stderr.write(`HomeBase request-id: ${requestId}\n`);
      const accepted=await rpc('call',{command:name,args,grants:scopes,request_id:requestId,catalog_hash:catalog.catalog_hash});
      if(!validJob(accepted?.job))fail('BAD_RESPONSE','导入响应缺少任务编号；保留document-id核对');
      const deadline=Date.now()+30000;
      while(true){
        const job=await rpc('jobs.get',{job_id:accepted.job.id,grants:scopes});
        if(!validJob(job,true))fail('BAD_RESPONSE','导入任务回执无效；未继续提交',{document_id:options['document-id'],job_id:accepted.job.id});
        if(job.status==='succeeded'&&job.result_available)return job.result;
        if(job.status!=='running'||Date.now()>=deadline)fail('DOCUMENT_IMPORT_UNCONFIRMED','文档尚未确认提交；保留原编号查询，不启动解析',{document_id:options['document-id'],job_id:job.id,state:job.status});
        await sleep(200);
      }
    }}).catch(error=>{if(error instanceof DocumentImportError)fail('DOCUMENT_IMPORT_INVALID',error.message,{document_id:options['document-id']});throw error;});
    output({ok:true,...result});return;
  }
  if (action === "commands") {
    const options = parseOptions(rest, ["module"]);
    if (options.module && !catalog.commands.some(entry => entry.module === options.module)) fail("UNKNOWN_MODULE", "模块不存在，请运行 hb commands 查看真实模块");
    output({
      ok: true,
      catalog_hash: catalog.catalog_hash,
      commands: catalog.commands
        .filter((entry) => !options.module || entry.module === options.module)
        .map(({ name, module, status, requires, reason }) => ({
          name,
          module,
          status,
          requires,
          reason,
        })),
    });
    return;
  }
  if (action === "describe") {
    if (rest.length !== 1) fail("USAGE", "用法：hb describe COMMAND");
    const entry = command(rest[0]);
    let definitions;
    try { definitions=(await import("./control-validation.mjs")).reachableDefinitions; } catch {fail("CLI_DEPENDENCY_INVALID","接口说明模块缺失，请恢复 CLI 文件");}
    output({
      ok: true,
      command: { ...entry, source: path.join(root, entry.source), input_schema: entry.input_schema ? { ...entry.input_schema, $defs: definitions(entry.input_schema,catalog.$defs||{}) } : null, output_schema: entry.output_schema ? { ...entry.output_schema, $defs: definitions(entry.output_schema,catalog.$defs||{}) } : null },
      argument_naming:
        "顶层优先 camelCase，也接受 Rust snake_case；嵌套 payload 按 input_schema 字段名。",
      input:
        "先 hb validate COMMAND --input FILE|-，再按 usage.submit 调用；input_schema / output_schema 与 usage 已包含正常使用所需信息，无需读取源码。source 仅供开发排错。",
    });
    return;
  }
  if (action === "validate") {
    if(!rest[0]) fail("USAGE", "用法：hb validate COMMAND --input FILE|-");
    const entry=command(rest[0]); const options=parseOptions(rest.slice(1),["input"]);
    if(entry.status!=="available"||!entry.input_schema) fail(entry.status==="blocked"?"HUMAN_REQUIRED":"NOT_IMPLEMENTED",entry.reason||"此命令不提供自动执行契约");
    let validator;
    try { validator=await import("./control-validation.mjs"); } catch { fail("CLI_DEPENDENCY_INVALID","接口校验模块缺失，请恢复 CLI 文件"); }
    let args; try {args=validator.canonicalArguments(entry,await input(options.input));} catch(error) {if(error instanceof Failure)throw error; fail("INPUT_INVALID","顶层参数别名重复");}
    const errors=validator.validateSchema(entry.input_schema,args,catalog.$defs||{});
    if(errors.length) {process.exitCode=1;output({ok:false,error:{code:"INPUT_INVALID",message:"参数不符合接口契约，未执行任何业务操作。"},issues:errors});return;}
    output({ok:true,command:entry.name,valid:true,executed:false});return;
  }
  if (action === "doctor" || action === "status") {
    if (rest.length) fail("USAGE", `${action} 不接受选项`);
    const status = await rpc("hello");
    if (!status || typeof status !== "object" || status.protocol !== 1 || typeof status.state_healthy !== "boolean" || status.service !== "homebase-control") fail("BAD_RESPONSE", "控制服务状态不完整，不能判定连接正常");
    if (status.catalog_hash !== catalog.catalog_hash)
      fail(
        "CATALOG_MISMATCH",
        "CLI 与当前应用的命令契约不一致，请重启更新后的应用",
      );
    if (action === "doctor" && status.state_healthy !== true)
      fail(
        "STATE_IO",
        "控制服务账本状态异常，暂不接受新操作，请核对已有任务后恢复",
      );
    output({
      ok: true,
      ...status,
      node: process.version,
      project: root,
      startup: path.join(root, "control", "START.md"),
    });
    return;
  }
  if (action === "jobs") {
    if (rest[0] === "list" && rest.length === 1) {
      const jobs = await rpc("jobs.list");
      if (!Array.isArray(jobs) || jobs.some(job => !validJob(job))) fail("BAD_RESPONSE", "任务列表格式无效，不能判定为空列表");
      output({ ok: true, jobs, ...(jobs.length === 0 ? { notice: "当前没有控制任务记录" } : {}) });
      return;
    }
    if (rest[0] !== "get" || !rest[1])
      fail("USAGE", "用法：hb jobs list 或 hb jobs get ID [--allow-...]");
    const options = parseOptions(rest.slice(2), [
      "allow-write",
      "allow-private",
      "allow-external",
    ]);
    output(
      renderJob(
        await rpc("jobs.get", { job_id: rest[1], grants: grants(options) }),
      ),
    );
    return;
  }
  if (action === "rollout") {
    if (rest[0] !== "status") fail("USAGE", "用法：hb rollout status --allow-private");
    const options = parseOptions(rest.slice(1), ["allow-private"]);
    if (!options["allow-private"]) fail("SCOPE_REQUIRED", "更新预检需要已获授权的 private 范围");
    let report;
    try {
      report = await rpc("rollout.status", { grants: ["private"] });
    } catch (error) {
      if (error.code === "UNKNOWN_METHOD")
        fail("ROLLOUT_STATUS_UNAVAILABLE", "当前程序未提供更新活动汇总，微信与控制队列的全量状态待核对");
      throw error;
    }
    const count = number => Number.isSafeInteger(number) && number >= 0;
    const { schema_version, pid, safe_to_update, control, wechat } = report ?? {};
    if (schema_version !== 1 || !count(pid) || pid === 0 || safe_to_update !== false ||
        !control || typeof control.healthy !== "boolean" ||
        ![control.jobs, control.active, control.to_review].every(count) ||
        control.active + control.to_review > control.jobs) {
      fail("BAD_RESPONSE", "更新活动汇总不完整，不能判定为空闲");
    }
    let channel = null;
    if (wechat !== null) {
      if (!wechat || ![wechat.records, wechat.pending_work, wechat.work_to_review,
        wechat.pending_delivery, wechat.delivery_to_review].every(value => count(value) && value <= wechat.records) ||
        ![wechat.receiver_enabled, wechat.pairing_active, wechat.submit_busy, wechat.attachment_busy]
          .every(value => typeof value === "boolean")) {
        fail("BAD_RESPONSE", "微信活动汇总不完整，不能判定为空闲");
      }
      const { records, pending_work, work_to_review, pending_delivery, delivery_to_review,
        receiver_enabled, pairing_active, submit_busy, attachment_busy } = wechat;
      channel = { records, pending_work, work_to_review, pending_delivery, delivery_to_review,
        receiver_enabled, pairing_active, submit_busy, attachment_busy };
    }
    const { healthy, jobs, active, to_review } = control;
    output({ ok: true, schema_version, pid, safe_to_update, control: { healthy, jobs, active, to_review },
      wechat: channel, ...(channel === null ? { notice: "微信通道当前无法核对，保持待确认" } : {}) });
    return;
  }
  if (action !== "call" || !rest[0]) fail("USAGE", "未知入口；运行 hb help");
  const entry = command(rest[0]);
  const options = parseOptions(rest.slice(1), [
    "input",
    "allow-write",
    "allow-private",
    "allow-external",
    "request-id",
    "detach",
    "wait-ms",
  ]);
  if (entry.status !== "available")
    fail(
      entry.status === "blocked" ? "HUMAN_REQUIRED" : "NOT_IMPLEMENTED",
      entry.reason,
    );
  const scopes = grants(options);
  if (entry.requires.some((scope) => !scopes.includes(scope)))
    fail(
      "SCOPE_REQUIRED",
      `此命令要求明确的任务授权范围：${entry.requires.join(", ")}。已有授权时附带相应 --allow-* 选项。`,
    );
  const wait = options["wait-ms"] == null ? 30000 : Number(options["wait-ms"]);
  if (!Number.isSafeInteger(wait) || wait < 0 || wait > 300000)
    fail("USAGE", "--wait-ms 必须为 0–300000");
  const requestId = options["request-id"] || crypto.randomUUID();
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(requestId))
    fail("USAGE", "request-id 须为 1–128 位字母、数字、下划线或短横线");
  // Print the recovery ID to stderr BEFORE connecting; never print arguments.
  process.stderr.write(`HomeBase request-id: ${requestId}\n`);
  const accepted = await rpc("call", {
    command: entry.name,
    args: await input(options.input),
    grants: scopes,
    request_id: requestId,
    catalog_hash: catalog.catalog_hash,
  });
  if (!validJob(accepted?.job)) fail("BAD_RESPONSE", "提交响应缺少有效任务状态；请保留原 request-id 核对，禁止盲目重跑");
  if (options.detach) {
    output({ ok: true, accepted: true, ...accepted });
    return;
  }
  output(renderJob(await waitForJob(accepted.job.id, scopes, wait)));
}

main().catch((error) => {
  process.exitCode = 1;
  output({
    ok: false,
    error: {
      code: error instanceof Failure ? error.code : "CLI_ERROR",
      message:
        error instanceof Failure
          ? error.message
          : "命令行处理失败，请检查文件路径、权限和输入格式",
      ...(error instanceof Failure && error.details
        ? { details: error.details }
        : {}),
    },
  });
});
