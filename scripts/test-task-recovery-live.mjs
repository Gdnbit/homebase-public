// End-to-end native/CLI verification against newly created synthetic roots only.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

if (!process.argv.includes("--isolated-only")) throw Error("Use --isolated-only: never pass real task roots.");
const project = fileURLToPath(new URL("../", import.meta.url));
const cli = path.join(project, "scripts", "homebase.mjs");
function invoke(args, input) {
  const run = spawnSync(process.execPath, [cli, ...args], { cwd: project, input: input === undefined ? undefined : JSON.stringify(input), encoding: "utf8", timeout: 45000, maxBuffer: 1024 * 1024, windowsHide: true });
  if (run.status !== 0) throw Error("Isolated CLI check failed; no task retry or repair was attempted.");
  return JSON.parse(run.stdout);
}
assert.equal(invoke(["doctor"]).ok, true);
const scratch = path.join(project, ".tmp");
fs.mkdirSync(scratch, { recursive: true });
assert.ok(fs.realpathSync(scratch).toLowerCase().startsWith(fs.realpathSync(project).toLowerCase() + path.sep));
const root = fs.mkdtempSync(path.join(scratch, "task-recovery-live-"));
const modelRoot = path.join(root, "model-jobs");
const asrRoot = path.join(root, "asr-output");
fs.mkdirSync(modelRoot);
fs.mkdirSync(path.join(asrRoot, "_jobs"), { recursive: true });
fs.mkdirSync(path.join(asrRoot, "synthetic-task"));
const id = "synthetic-live-job";
const write = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, 2), "utf8");
write(path.join(modelRoot, `${id}.json`), { job_id: id, status: "running", phase: "downloading", pid: process.pid, revision: 7, updated_at: "synthetic-original", completed_at: null });
write(path.join(asrRoot, "_jobs", `${id}.json`), { schemaVersion: 1, jobId: id, status: "running", sourcePath: path.join(root, "nonexistent-synthetic.wav"), outputRoot: asrRoot, taskDir: path.join(asrRoot, "synthetic-task"), presetId: "synthetic", engineIds: [], pid: process.pid, createdAt: "synthetic-original", updatedAt: "synthetic-original", completedAt: null, exitCode: null, error: null });
write(path.join(asrRoot, "synthetic-task", "task.meta.json"), { status: "running", marker: "synthetic-preserve" });
function snapshot(folder) {
  const entries = [];
  for (const entry of fs.readdirSync(folder, { withFileTypes: true })) {
    const file = path.join(folder, entry.name);
    if (entry.isDirectory()) entries.push(...snapshot(file));
    else entries.push([file, crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex"), fs.statSync(file).mtimeMs]);
  }
  return entries.sort((a, b) => a[0].localeCompare(b[0]));
}
const before = [...snapshot(modelRoot), ...snapshot(asrRoot)];
const jobs = [];
const request = crypto.randomUUID();
for (let iteration = 0; iteration < 3; iteration++) {
  for (const [command, input] of [["model_download_reconcile", { jobsRoot: modelRoot }], ["asr_task_reconcile", { outputRoot: asrRoot }]]) {
    const response = invoke(["call", command, "--input", "-", "--allow-private", "--request-id", `recovery-isolated-${request}-${iteration}-${command}`], input);
    assert.equal(response.ok, true);
    assert.equal(response.job.status, "succeeded");
    assert.equal(response.result_state, "partial");
    assert.equal(response.result.length, 1);
    assert.equal(response.result[0].status, "running");
    assert.equal(response.result[0].runtime_state, "unmanaged");
    jobs.push(response.job.id);
    assert.deepEqual([...snapshot(modelRoot), ...snapshot(asrRoot)], before);
  }
}
const report = { passed: true, synthetic_root: root, observations: 6, source_files: before.length, bytes_and_mtimes_unchanged: true, active_pid_did_not_imply_ownership: true, runtime_state: "unmanaged", result_state: "partial", write_grant_used: false, real_task_roots_used: false, tasks_started_or_cancelled: 0, control_jobs: jobs };
write(path.join(root, "validation.json"), report);
console.log(JSON.stringify(report, null, 2));
