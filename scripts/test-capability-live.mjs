// Opt-in, real local integration. Uses only a fresh marked capability root and synthetic video text.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
const execute = promisify(execFile);
const project = fileURLToPath(new URL("../", import.meta.url));
if (!process.argv.includes("--isolated-only"))
  throw new Error("Requires --isolated-only");
const lab = fs
  .readFileSync(path.join(project, ".tmp/video-capability-lab.txt"), "utf8")
  .trim();
if (!/^F:\\HomeBase-Capability-Lab\\hyperframes-\d{8}-\d{6}$/.test(lab))
  throw new Error("Unexpected lab root");
const root = path.join(lab, "runtime");
const evidence = path.join(lab, `validation-${crypto.randomUUID()}`);
fs.mkdirSync(evidence);
async function cli(args, allowFailure = false) {
  try {
    const { stdout } = await execute(
      process.execPath,
      [path.join(project, "scripts/homebase.mjs"), ...args],
      {
        cwd: project,
        timeout: 25000,
        maxBuffer: 2 * 1024 * 1024,
        windowsHide: true,
      },
    );
    return JSON.parse(stdout);
  } catch (error) {
    if (allowFailure && error.stdout) return JSON.parse(error.stdout);
    throw error;
  }
}
const flags = ["--allow-write", "--allow-external", "--allow-private"];
const file = (name, value) => {
  const target = path.join(evidence, name);
  fs.writeFileSync(target, JSON.stringify(value, null, 2));
  return target;
};
async function submit(command, payload, name) {
  const input = file(`${name}-input.json`, payload);
  const request = `capability-${name}-${crypto.randomUUID()}`;
  const response = await cli([
    "call",
    command,
    ...flags,
    "--input",
    input,
    "--request-id",
    request,
    "--detach",
  ]);
  assert.ok(response.ok, JSON.stringify(response));
  assert.ok(response.job?.id, "Missing control job id");
  file(`${name}-submitted.json`, response);
  console.log(
    JSON.stringify({
      step: name,
      job_id: response.job.id,
      status: response.job.status,
    }),
  );
  return { response, input, request, command, name };
}
async function wait(task, failure = false) {
  const snapshots = [];
  for (let i = 0; i < 360; i++) {
    const value = await cli(
      ["jobs", "get", task.response.job.id, ...flags],
      true,
    );
    const job = value.job;
    assert.ok(job, JSON.stringify(value));
    snapshots.push({
      at: new Date().toISOString(),
      status: job.status,
      result_state: value.result_state,
    });
    if (!["queued", "running"].includes(job.status)) {
      file(`${task.name}-status.json`, snapshots);
      file(`${task.name}-completed.json`, value);
      if (!failure) {
        assert.equal(job.status, "succeeded");
        assert.equal(
          job.result?.status,
          "succeeded",
          JSON.stringify(job.result),
        );
      }
      console.log(
        JSON.stringify({
          step: task.name,
          status: job.status,
          business_status: job.result?.status,
          run_id: job.result?.run_id,
        }),
      );
      return value;
    }
    if (i % 10 === 0)
      console.log(JSON.stringify({ step: task.name, status: job.status }));
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(
    "Integration wait expired; use saved job id, do not resubmit",
  );
}
const doctor = await cli(["doctor"]);
assert.ok(doctor.ok);
file("doctor.json", doctor);
const installTask = await submit(
  "capability_install",
  { rootPath: root },
  "install",
);
await wait(installTask);
const renderTask = await submit(
  "capability_render",
  {
    rootPath: root,
    input: {
      title: "HELLO HOMEBASE",
      subtitle: "Open source. Local render. One callable capability.",
      seconds: 6,
    },
  },
  "render",
);
const duplicate = await cli([
  "call",
  renderTask.command,
  ...flags,
  "--input",
  renderTask.input,
  "--request-id",
  renderTask.request,
  "--detach",
]);
assert.equal(
  duplicate.job.id,
  renderTask.response.job.id,
  "Duplicate request created another job",
);
file("render-deduplicated.json", duplicate);
const rendered = await wait(renderTask);
const runId = rendered.job.result.run_id;
const resultInput = file("result-input.json", { rootPath: root, runId });
const result = await cli([
  "call",
  "capability_result",
  "--allow-private",
  "--input",
  resultInput,
]);
assert.ok(result.ok);
assert.equal(result.result.status, "succeeded");
assert.equal(result.result.verification.frames, 144);
assert.ok(fs.existsSync(result.result.artifact.path));
file("persistent-result.json", result);
const bad = await submit(
  "capability_render",
  { rootPath: root, input: { title: "Synthetic invalid input", seconds: 99 } },
  "invalid-input",
);
const rejected = await wait(bad, true);
assert.equal(rejected.job.result?.status, "failed");
const state = await cli([
  "call",
  "capabilities_catalog",
  "--allow-private",
  "--input",
  file("catalog-input.json", { rootPath: root }),
]);
assert.equal(state.result.state, "verified");
file("catalog-verified.json", state);
file("validation.json", {
  root,
  evidence,
  install_job: installTask.response.job.id,
  render_job: renderTask.response.job.id,
  run_id: runId,
  artifact: result.result.artifact,
  verification: result.result.verification,
  duplicate_request_reused_job: true,
  invalid_input_rejected: true,
  secrets_accessed: false,
  existing_business_data_changed: false,
});
console.log(
  JSON.stringify(
    {
      ok: true,
      evidence,
      artifact: result.result.artifact.path,
      run_id: runId,
    },
    null,
    2,
  ),
);
