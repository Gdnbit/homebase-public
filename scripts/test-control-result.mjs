import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import net from "node:net";
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { inspectResult, validJob } from "./control-result.mjs";
const cli = fileURLToPath(new URL("homebase.mjs", import.meta.url));
const job = { id: "synthetic-job", status: "succeeded", result_available: true };

test('session catalog pages separate incomplete discovery, confirmed empty and task execution',()=>{
  const page={version:'homebase-session-catalog-v1',total:2,items:[{id:'session-a'}],next_offset:1,complete:false};
  assert.equal(inspectResult(page).result_state,'partial');assert(inspectResult(page).notices.some(n=>n.includes('snapshot_id')));
  assert.equal(inspectResult({...page,next_offset:null}).result_state,'partial');
  assert.equal(inspectResult({...page,next_offset:null,complete:true}).result_state,'returned');
  assert.equal(inspectResult({...page,total:0,items:[],next_offset:null,complete:true}).result_state,'empty');
  for(const result of [page,{...page,total:0,items:[],next_offset:null,complete:true}])assert.notEqual(inspectResult(result).result_state,'in_progress');
});

test("send receipt lookup preserves original outcome and never treats absence as permission to rerun",()=>{
  const view={version:"homebase-send-receipt-v1",lookup_state:"found",receipt:{session_id:"s",turn_id:"t",state:"completed"}};
  assert.equal(inspectResult(view).result_state,"returned");
  assert(inspectResult(view).notices.some(n=>n.includes("未重新执行")));
  for(const [state,outcome] of [["running","in_progress"],["waiting_jobs","in_progress"],["awaiting_confirmation","in_progress"],["waiting_input_queue","in_progress"],["interrupted_unknown","partial"],["future_state","partial"],["failed","reported_failure"],["disabled","reported_failure"]]) {
    // A failed original task is still a successfully found receipt, never a new execution.
    const result=inspectResult({...view,receipt:{...view.receipt,state}});
    assert.equal(result.result_state,outcome);
    assert(result.notices.some(n=>n.includes("原回执")));
  }
  for(const extra of [{lookup_state:"not_found",receipt:null},{receipt:null},{lookup_state:"future"},{receipt:{turn_id:"t"}}]){
    const result=inspectResult({...view,...extra});assert.equal(result.result_state,"partial");assert(result.notices.length);
  }
});

test("Inbox source observations and original receipts never imply a new draft was published",()=>{
  const receipt={submission_id:'87562a38-8fb4-4276-bdda-bd0d85bef544',state:'completed',file_state:'present',result:{filename:'original.md'}};
  assert.equal(inspectResult(receipt).result_state,'returned');
  for(const extra of [{state:'not_found',file_state:null},{state:'prepared',file_state:null},{state:'unknown',file_state:'missing'},{state:'canceled',file_state:null},{file_state:'missing'},{file_state:'modified'},{file_state:'unreadable'}]){
    const view=inspectResult({...receipt,...extra});assert.equal(view.result_state,'partial');assert(view.notices.length);assert.notEqual(view.result_state,'in_progress');
  }
  const source={entry:{schema_version:1,draft:{text:'LATER UNSENT B'},pending:null},submission:null,observation:'not_checked'};
  assert.equal(inspectResult(source).result_state,'returned');assert(inspectResult(source).notices.some(s=>s.includes('不代表新输入')));
  assert.equal(inspectResult({...source,observation:'failed'}).result_state,'partial');
  assert.equal(inspectResult({...source,entry:{...source.entry,pending:{submission_id:receipt.submission_id}},submission:{...receipt,state:'prepared'}}).result_state,'partial');
});

test("timeline windows distinguish missing older/detail reads from active business tasks",()=>{
  const base={messages:[{id:'m'}],turns:[],page:{version:'homebase-session-window-v1',total_messages:1,start_index:0,end_index:1,has_more:false}};
  assert.equal(inspectResult(base).result_state,'returned');
  assert.equal(inspectResult({...base,messages:[],page:{...base.page,total_messages:0,end_index:0}}).result_state,'empty');
  for(const extra of [{page:{...base.page,total_messages:4,start_index:3,end_index:4,has_more:true}},{page:{...base.page,total_messages:4}},{turns:[{details_omitted:true}]}]){
    const result=inspectResult({...base,...extra});assert.equal(result.result_state,'partial');assert(result.notices.length);assert.notEqual(result.result_state,'in_progress');
  }
});

test("turn progress reports original activity and keeps omitted details explicit", () => {
  const turn = { id: "original-turn", session_id: "original-session", details_omitted: true, step_count: 4 };
  for (const [state, expected] of [
    ["running", "in_progress"],
    ["awaiting_confirmation", "in_progress"],
    ["interrupted_unknown", "partial"],
    ["completed", "partial"],
    ["failed", "reported_failure"],
  ]) {
    const outcome = inspectResult({ ...turn, state });
    assert.equal(outcome.result_state, expected);
    assert(outcome.notices.some(notice => notice.includes("同一编号")));
  }
});

test("history pages and JSON chunks are partial reads, never executing jobs",()=>{
  const base={version:'homebase-history-v1',next_offset:null,complete:true,omitted_unassigned_messages:0};
  assert.equal(inspectResult(base).result_state,'returned');
  assert.equal(inspectResult({...base,total:0,items:[]}).result_state,'empty');
  for(const extra of [{next_offset:8,complete:false},{format:'message_json',next_offset:4096,complete:false},{omitted_unassigned_messages:1}]) {
    const read=inspectResult({...base,...extra});assert.equal(read.result_state,'partial');assert(read.notices.length>0);assert.notEqual(read.result_state,'in_progress');
  }
});

test("discovery pagination and incomplete sources are explicit, not an executing task",()=>{
  const base={version:"homebase-capability-discovery-v1",source_complete:true,next_offset:null,groups_has_more:false,issues_has_more:false,entries:[]};
  assert.equal(inspectResult(base).result_state,"returned");
  const page=inspectResult({...base,next_offset:12});assert.equal(page.result_state,"partial");assert(page.notices.some(n=>n.includes("snapshot_id")));
  const source=inspectResult({...base,source_complete:false,issues:[],issues_has_more:true});assert.equal(source.result_state,"partial");assert(source.notices.some(n=>n.includes("来源")));
  assert.equal(inspectResult({...base,unmatched_entry_keys:[]}).result_state,"returned");
  const missing=inspectResult({...base,unmatched_entry_keys:['0'.repeat(64)]});assert.equal(missing.result_state,"partial");assert(missing.notices.some(n=>n.includes("未匹配")));
  const unresolved=inspectResult({...base,source_complete:false,unmatched_entry_keys:['0'.repeat(64)]});assert(unresolved.notices.some(n=>n.includes("尚未核实")));
});

test("document receipts distinguish processing, no text, unknown and stopped outcomes",()=>{
  const receipt={document_id:"synthetic-document",id:"synthetic-extraction"};
  for(const [phase,expected]of [["processing","in_progress"],["cancel_requested","in_progress"],["no_text","partial"],["awaiting_pages","partial"],["unknown","partial"],["failed","reported_failure"],["cancelled","reported_failure"],["ready","returned"]]){
    assert.equal(inspectResult({...receipt,phase}).result_state,expected);
    assert.equal(inspectResult({status:"cached",extraction:{...receipt,phase}}).result_state,expected);
  }
  assert.equal(inspectResult({...receipt,phase:"ready",partial:true}).result_state,"partial");
  for(const status of ["capacity","runtime_unavailable","not_running_here"])assert.equal(inspectResult({status,extraction:null}).result_state,"partial");
});

test("empty, partial, failures and ongoing business outcomes are explicit", () => {
  for (const value of [null, []]) assert.equal(inspectResult(value).result_state, "empty");
  for (const value of [{ success: false }, { ok: false }, { status: "failed" }]) assert.equal(inspectResult(value).result_state, "reported_failure");
  for (const value of [{ issues: [{ field: "cpu" }] }, { gpu_status: "unknown" }, { available: false }, { agents: [{ status: "error" }] }, [{ ok: false }], { current: null, latest: "1.0" }]) assert.equal(inspectResult(value).result_state, "partial");
  assert.equal(inspectResult({ state: "running" }).result_state, "in_progress");
  assert.equal(inspectResult({ job: { status: "running" } }).result_state, "in_progress");
  assert.equal(inspectResult({ task_meta: { status: "failed" } }).result_state, "reported_failure");
  assert.equal(inspectResult({ status: "unknown" }).result_state, "partial");
  assert.equal(inspectResult({ state: "not_configured" }).result_state, "partial");
  assert.equal(inspectResult([{ state: "linked" }, { state: "blocked" }]).result_state, "partial");
  assert.equal(inspectResult({ cpu_percent: 0, error: null }).result_state, "returned");
});
test("malformed job metadata cannot prove completion", () => {
  for (const value of [null, {}, { ...job, status: "new_state" }, { ...job, status: "running" }, job]) assert.equal(validJob(value, true), false);
  assert.equal(validJob({ ...job, result: null }, true), true);
  assert.equal(validJob({ ...job, status: "failed", error: { code: "QUERY_INVALID" } }, true), true);
  assert.equal(validJob({ ...job, status: "running", result_available: false }, true), true);
});

test("a partial desktop acknowledgement retains its accepted task and explicit warning", () => {
  const result = { status: "partial", code: "LOCAL_ACK_SAVE_FAILED", ack: { session_id: "synthetic-session", turn_id: "original-task", state: "running" }, warning: "Synthetic protected cleanup failed" };
  const outcome = inspectResult(result);
  assert.equal(outcome.result_state, "partial");
  assert(outcome.notices.some(notice => notice.includes("不要盲目重做")));
  assert.equal(result.ack.turn_id, "original-task");
  assert.equal(inspectResult({ status: "partial", index: { entries: [], complete: false, unreadable: 1 } }).result_state, "partial");
});
test("read-only recovery observations do not claim an orphan record is running", () => {
  for (const runtime_state of ["unmanaged", "unknown"]) {
    assert.equal(inspectResult({ status: "running", runtime_state }).result_state, "partial");
    assert.equal(inspectResult([{ status: "queued", runtime_state }]).result_state, "partial");
  }
  assert.equal(inspectResult({ status: "running", runtime_state: "managed" }).result_state, "in_progress");
});

async function withServer(args, respond, { withoutCatalog = false } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "hb-query-result-"));
  const server = net.createServer(socket => {
    let raw = "";
    socket.on("data", bytes => {
      raw += bytes.toString();
      if (!raw.includes("\n")) return;
      const request = JSON.parse(raw.split("\n")[0]);
      const result = respond(request);
      if (result === "disconnect") socket.end();
      else socket.end(JSON.stringify({ protocol: 1, id: request.id, ok: true, ...result }) + "\n");
    });
  });
  try {
    await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
    const endpoint = path.join(dir, "endpoint.json");
    fs.writeFileSync(endpoint, JSON.stringify({ protocol: 1, endpoint: `127.0.0.1:${server.address().port}`, token: "a".repeat(64), instance_id: "synthetic" }));
    const env = Object.fromEntries(["PATH", "SystemRoot", "WINDIR"].filter(k => process.env[k]).map(k => [k, process.env[k]]));
    for (const key of ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP"]) env[key] = dir;
    env.HOMEBASE_CONTROL_ENDPOINT = endpoint;
    let script = cli;
    if (withoutCatalog) {
      const scripts = path.join(dir, "scripts");
      fs.mkdirSync(scripts);
      for (const name of ["homebase.mjs", "control-bootstrap.mjs", "control-result.mjs"]) {
        fs.copyFileSync(path.join(path.dirname(cli), name), path.join(scripts, name));
      }
      script = path.join(scripts, "homebase.mjs");
    }
    const proc = spawn(process.execPath, [script, ...args], { env, windowsHide: true });
    const chunks = [];
    proc.stdout.on("data", bytes => chunks.push(bytes));
    let errors = "";
    proc.stderr.on("data", bytes => errors += bytes);
    const code = await new Promise((resolve, reject) => { proc.once("error", reject); proc.once("close", resolve); });
    const output = Buffer.concat(chunks).toString("utf8");
    assert.ok(!output.includes("a".repeat(64)) && !errors.includes("a".repeat(64)));
    return { code, value: JSON.parse(output) };
  } finally {
    await new Promise(resolve => server.close(resolve));
    const cleanupRoot = fs.realpathSync(os.tmpdir());
    const cleanupTarget = fs.realpathSync(dir);
    assert.equal(path.dirname(cleanupTarget), cleanupRoot);
    assert.ok(path.basename(cleanupTarget).startsWith("hb-query-result-"));
    fs.rmSync(dir, { recursive: true, force: true });
  }
}
test("existing jobs can be observed without loading the command catalog", async () => {
  const options = { withoutCatalog: true };
  const list = await withServer(["jobs", "list"], ({ method }) => {
    assert.equal(method, "jobs.list");
    return { result: [job] };
  }, options);
  assert.equal(list.code, 0);
  assert.deepEqual(list.value.jobs, [job]);

  const details = await withServer(["jobs", "get", job.id, "--allow-private"], (request) => {
    assert.equal(request.method, "jobs.get");
    assert.equal(request.job_id, job.id);
    assert.deepEqual(request.grants, ["private"]);
    return { result: { ...job, result: { title: "original result" } } };
  }, options);
  assert.equal(details.code, 0);
  assert.deepEqual(details.value.result, { title: "original result" });

  const discovery = await withServer(["commands"], () => {
    assert.fail("Offline command discovery must not contact the service");
  }, options);
  assert.equal(discovery.code, 1);
  assert.equal(discovery.value.error.code, "CATALOG_INVALID");
});

test("rollout status projects counts only, and separates legacy, unknown, and invalid observations", async () => {
  const args = ["rollout", "status", "--allow-private"];
  const control = { healthy: true, jobs: 105, active: 1, to_review: 2 };
  const wechat = { records: 58, pending_work: 1, work_to_review: 1, pending_delivery: 1,
    delivery_to_review: 2, receiver_enabled: true, pairing_active: false, submit_busy: false, attachment_busy: true };
  const base = { schema_version: 1, pid: 21, safe_to_update: false, control, wechat };
  for (const [response, expected] of [
    [{ result: { ...base, control: { ...control, payload: "synthetic-private" },
      wechat: { ...wechat, token: "synthetic-private" }, body: "synthetic-private" } }, null],
    [{ result: { ...base, wechat: null } }, null],
    [{ ok: false, error: { code: "UNKNOWN_METHOD", message: "synthetic old server" } }, "ROLLOUT_STATUS_UNAVAILABLE"],
    [{ result: { ...base, safe_to_update: true } }, "BAD_RESPONSE"],
    [{ result: { ...base, wechat: { ...wechat, pending_work: 59 } } }, "BAD_RESPONSE"],
    [{ result: { ...base, wechat: { ...wechat, submit_busy: null } } }, "BAD_RESPONSE"],
  ]) {
    const result = await withServer(args, packet => {
      assert.equal(packet.method, "rollout.status");
      assert.deepEqual(packet.grants, ["private"]);
      assert(!packet.command && !packet.request_id && !packet.job_id);
      return response;
    });
    if (expected) {
      assert.equal(result.code, 1);
      assert.equal(result.value.error.code, expected);
    } else {
      assert.equal(result.code, 0);
      assert.deepEqual(result.value.control, control);
      assert.deepEqual(result.value.wechat, response.result.wechat === null ? null : wechat);
      assert.equal(result.value.safe_to_update, false);
      assert(!JSON.stringify(result.value).includes("synthetic-private"));
      if (result.value.wechat === null) assert.match(result.value.notice, /无法核对/);
    }
  }
  const refused = await withServer(args.slice(0, -1), () => assert.fail("missing scope must not contact the server"));
  assert.equal(refused.value.error.code, "SCOPE_REQUIRED");
});

test("CLI rejects missing results, incomplete doctor and malformed list/job responses", async () => {
  for (const [args, response] of [
    [["doctor"], {}],
    [["doctor"], { result: {} }],
    [["jobs", "list"], { result: {} }],
    [["jobs", "get", "synthetic-job"], { result: job }],
    [["call", "system_info", "--detach"], { result: { job: {} } }],
  ]) {
    const result = await withServer(args, () => response);
    assert.equal(result.code, 1);
    assert.equal(result.value.error.code, "BAD_RESPONSE");
  }
});
test("CLI reads one original turn and emits JSON only after all offsets and its digest agree", async () => {
  const sessionId = "fixture-session";
  const turnId = "fixture-turn";
  const snapshotId = "00000000-0000-4000-8000-000000000001";
  const args = ["history", "read-turn", "--session-id", sessionId, "--turn-id", turnId, "--allow-private"];
  const view = { id: turnId, session_id: sessionId, text: "隔离🙂原正文", steps: ["原步骤"] };
  const json = JSON.stringify(view);
  const fragments = [json.slice(0, json.indexOf("原正文")), json.slice(json.indexOf("原正文"))];
  const total = Buffer.byteLength(json);
  for (const fault of [null, "scope", "offset", "digest"]) {
    const sha = fault === "digest" ? "b".repeat(64) : createHash("sha256").update(json).digest("hex");
    const offsets = [0, Buffer.byteLength(fragments[0])];
    let current;
    let reads = 0;
    const result = await withServer(args, packet => {
      assert.deepEqual(packet.grants, ["private"]);
      if (packet.method === "jobs.get") return { result: { ...job, result: current } };
      assert.equal(packet.method, "call");
      if (packet.command === "assistant_turn_snapshot") {
        assert.deepEqual(packet.args, { sessionId, turnId });
        current = { version: "homebase-history-v1", session_id: sessionId, kind: "turns", snapshot_id: snapshotId, total: 1,
          complete: true, next_offset: null, items: [{ id: turnId, turn_id: turnId, json_bytes: total, json_sha256: sha }] };
      } else {
        assert.equal(packet.command, "assistant_history_read");
        const index = reads++;
        assert.deepEqual(packet.args.request, { session_id: sessionId, snapshot_id: snapshotId, item_id: turnId, offset: offsets[index], limit: 524288 });
        current = { version: "homebase-history-v1", session_id: sessionId, kind: "turns", snapshot_id: snapshotId,
          item_id: fault === "scope" && index === 1 ? "wrong-turn" : turnId, format: "turn_view_json", json_sha256: sha,
          total_bytes: total, offset: fault === "offset" && index === 1 ? offsets[1] + 1 : offsets[index],
          next_offset: index === 0 ? offsets[1] : null, complete: index === 1, content: fragments[index] };
      }
      return { result: { job } };
    });
    assert.equal(reads, 2);
    if (!fault) {
      assert.equal(result.code, 0);
      assert.deepEqual(result.value, view);
    } else {
      assert.equal(result.code, 1);
      assert.equal(result.value.error.code, "BAD_RESPONSE");
      assert(!JSON.stringify(result.value).includes(view.text));
    }
  }
  const refused = await withServer(args.slice(0, -1), () => assert.fail("missing scope must not contact the server"));
  assert.equal(refused.code, 1);
  assert.equal(refused.value.error.code, "SCOPE_REQUIRED");
});
test("CLI reports actual business failure, optional unknown and empty results", async () => {
  for (const [result, state, code] of [[{ success: false }, "reported_failure", 1], [null, "empty", 0], [[], "empty", 0], [{ issues: [{ field: "memory", status: "unknown" }] }, "partial", 0], [{ state: "running" }, "in_progress", 0]]) {
    const response = await withServer(["jobs", "get", "synthetic-job"], () => ({ result: { ...job, result } }));
    assert.equal(response.code, code);
    assert.equal(response.value.result_state, state);
    assert.equal(response.value.job.status, "succeeded");
    assert.ok(response.value.notices.length);
  }
});
test("CLI disconnect is a failure rather than an empty success", async () => {
  const result = await withServer(["doctor"], () => "disconnect");
  assert.equal(result.code, 1);
  assert.equal(result.value.error.code, "DISCONNECTED");
});
test("CLI preserves a failed native job whose available result is an error", async () => {
  const result = await withServer(["jobs", "get", "synthetic-job"], () => ({ result: { ...job, status: "failed", error: { code: "QUERY_INVALID", message: "Synthetic query failed" }, result: null } }));
  assert.equal(result.code, 1);
  assert.equal(result.value.error.code, "QUERY_INVALID");
  assert.equal(result.value.job.status, "failed");
});

test("task identifiers and incomplete event pages never imply business completion", () => {
  assert.equal(inspectResult({task_id:'synthetic'}).result_state,'in_progress');
  assert.equal(inspectResult({status:'succeeded',settled:false,has_more:false}).result_state,'in_progress');
  assert.equal(inspectResult({status:'succeeded',settled:true,has_more:true}).result_state,'in_progress');
  assert.equal(inspectResult({status:'succeeded',settled:true,has_more:false,history_lost:true}).result_state,'partial');
  assert.equal(inspectResult({status:'unknown',settled:false}).result_state,'partial');
  assert.equal(inspectResult({status:'succeeded',settled:true,has_more:false}).result_state,'returned');
});
