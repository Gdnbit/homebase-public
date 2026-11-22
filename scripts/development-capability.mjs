import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const hash = (value) => crypto.createHash("sha256").update(value).digest("hex");
const read = (file) =>
  JSON.parse(fs.readFileSync(file, "utf8").replace(/^\uFEFF/, ""));
const write = (file, value) => {
  const tmp = `${file}.${process.pid}.${crypto.randomUUID()}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(value, null, 2) + "\n", { flag: "wx" });
  fs.renameSync(tmp, file);
};
const workflows = [
  {
    id: "development-local",
    name: "完整开发 · 三个独立 Codex 角色",
    description: "规划 → 实现 → 独立审查 → 宿主验收",
  },
  {
    id: "execution-checked",
    name: "明确小改动 · 单执行器",
    description: "执行 → 宿主验收，需求和验收应已明确",
  },
  {
    id: "development",
    name: "Claude / Codex 混合",
    description: "Claude 规划 → Codex 实现 → Claude 审查；启动前检查现有凭据",
  },
];
function exact(value, keys) {
  if (
    !value ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.keys(value).some((k) => !keys.includes(k))
  )
    throw Error("DEVELOPMENT_INPUT_INVALID");
}
function bounded(value, max) {
  if (
    typeof value !== "string" ||
    !value.trim() ||
    value.includes("\0") ||
    Buffer.byteLength(value) > max
  )
    throw Error("DEVELOPMENT_INPUT_INVALID");
  return value;
}
const validId = (id) => /^dev-[a-f0-9]{32}$/.test(id || "");
export function createDevelopment({ home, runBridge } = {}) {
  if (!path.isAbsolute(home || "")) throw Error("DEVELOPMENT_HOME_INVALID");
  const root = path.join(home, ".homebase/capabilities/development");
  const autoloop = path.join(home, ".agent-autoloop");
  function bridge(args) {
    if (runBridge) return runBridge(args);
    const registration = read(path.join(autoloop, "workbench/entry.json"));
    for (const file of [registration.node, registration.entry]) {
      if (!path.isAbsolute(file || "") || !fs.statSync(file).isFile())
        throw Error("DEVELOPMENT_RUNTIME_UNAVAILABLE");
    }
    const child = spawnSync(registration.node, [registration.entry, ...args], {
      encoding: "utf8",
      windowsHide: true,
      timeout: 75000,
      maxBuffer: 12 * 1024 * 1024,
      env: { ...process.env, AGENT_AUTOLOOP_HOME: autoloop },
    });
    if (child.error || child.status !== 0)
      throw Error("DEVELOPMENT_BRIDGE_UNCONFIRMED");
    return JSON.parse(child.stdout);
  }
  function record(id) {
    if (!validId(id)) throw Error("DEVELOPMENT_TASK_INVALID");
    const folder = path.join(root, id);
    const item = read(path.join(folder, "request.json"));
    if (
      item.task_id !== id ||
      item.fingerprint !== hash(JSON.stringify(item.input))
    )
      throw Error("DEVELOPMENT_RECORD_CHANGED");
    return { folder, item };
  }
  function validate(request) {
    exact(request, [
      "request_id",
      "workspace",
      "brief",
      "workflow",
      "checks",
      "protected_files",
    ]);
    const requestId = bounded(request.request_id, 160);
    if (!/^[a-zA-Z0-9][a-zA-Z0-9._:-]*$/.test(requestId))
      throw Error("DEVELOPMENT_REQUEST_ID_INVALID");
    bounded(request.workspace, 2000);
    if (
      !path.isAbsolute(request.workspace) ||
      request.workspace.startsWith("\\\\")
    )
      throw Error("DEVELOPMENT_WORKSPACE_INVALID");
    const workspace = fs.realpathSync(request.workspace);
    if (
      !fs.statSync(workspace).isDirectory() ||
      workspace === path.parse(workspace).root
    )
      throw Error("DEVELOPMENT_WORKSPACE_INVALID");
    // The hub/runtime and its configuration are never a development workspace.
    const relative = path.relative(autoloop, workspace);
    if (
      !relative ||
      (!relative.startsWith(`..${path.sep}`) &&
        relative !== ".." &&
        !path.isAbsolute(relative))
    )
      throw Error("DEVELOPMENT_WORKSPACE_RESERVED");
    const brief = bounded(request.brief, 16000);
    const workflow = request.workflow || "development-local";
    if (!workflows.some((w) => w.id === workflow))
      throw Error("DEVELOPMENT_WORKFLOW_INVALID");
    const checks = request.checks ?? [];
    if (!Array.isArray(checks) || checks.length > 8)
      throw Error("DEVELOPMENT_CHECKS_INVALID");
    for (const argv of checks) {
      if (!Array.isArray(argv) || !argv.length || argv.length > 40)
        throw Error("DEVELOPMENT_CHECKS_INVALID");
      for (const arg of argv)
        if (
          typeof arg !== "string" ||
          arg.includes("\0") ||
          Buffer.byteLength(arg) > 2000
        )
          throw Error("DEVELOPMENT_CHECKS_INVALID");
      bounded(argv[0], 2000);
    }
    const protectedFiles = request.protected_files ?? [];
    if (!Array.isArray(protectedFiles) || protectedFiles.length > 20)
      throw Error("DEVELOPMENT_PROTECTED_FILES_INVALID");
    const guards = protectedFiles.map((file) => {
      bounded(file, 2000);
      const full = fs.realpathSync(path.resolve(workspace, file));
      const rel = path.relative(workspace, full);
      if (
        rel === ".." ||
        rel.startsWith(`..${path.sep}`) ||
        path.isAbsolute(rel) ||
        !fs.statSync(full).isFile()
      )
        throw Error("DEVELOPMENT_PROTECTED_FILES_INVALID");
      return full;
    });
    if (Buffer.byteLength(JSON.stringify({ checks, guards })) > 18000)
      throw Error("DEVELOPMENT_INPUT_TOO_LARGE");
    return {
      request_id: requestId,
      workspace,
      brief,
      workflow,
      checks,
      protected_files: guards,
    };
  }
  function catalog() {
    let status = "unavailable",
      version = null,
      message = "未连接本机 Agent Autoloop；请先完成部署。";
    try {
      const state = bridge(["status"]);
      status = state.agents?.some((a) => a.online) ? "ready" : "offline";
      const recordedVersion = read(path.join(autoloop, "app/package.json")).version;
      version = typeof recordedVersion === "string" ? recordedVersion : null;
      message =
        status === "ready"
          ? "已连接本机执行器；账号与模型在启动时检查，在线不代表任务验收通过。"
          : "中台可访问，但没有在线执行器。";
    } catch {
      /* Do not expose child output/configuration. */
    }
    const tasks = fs.existsSync(root)
      ? fs
          .readdirSync(root)
          .filter(validId)
          .map((id) => {
            const { item } = record(id);
            return {
              task_id: id,
              workspace: item.input.workspace,
              workflow: item.input.workflow,
              created_at: item.created_at,
            };
          })
          .sort((a, b) => b.created_at.localeCompare(a.created_at))
          .slice(0, 20)
      : [];
    return {
      capability_id: "homebase.development",
      name: "开发",
      status,
      version,
      message,
      workflows,
      tasks,
      cost: "unknown",
      default_workflow: "development-local",
    };
  }
  function get(taskId) {
    const { folder, item } = record(taskId);
    const base = {
      capability_id: "homebase.development",
      task_id: taskId,
      workspace: item.input.workspace,
      workflow: item.input.workflow,
    };
    const resultFile = path.join(
      item.input.workspace,
      ".autoloop/tasks",
      taskId,
      "result.json",
    );
    let observed;
    try {
      observed = bridge(["result", taskId]);
    } catch {
      return {
        ...base,
        state: "unknown",
        message: "原任务状态未确认，请按同一编号重新查询；不要重新创建任务。",
        runs: [],
        questions: [],
      };
    }
    if (observed.task !== taskId) throw Error("DEVELOPMENT_IDENTITY_MISMATCH");
    if (fs.existsSync(resultFile)) {
      if (fs.statSync(resultFile).size > 10 * 1024 * 1024)
        throw Error("DEVELOPMENT_RESULT_TOO_LARGE");
      const bytes = fs.readFileSync(resultFile);
      const result = JSON.parse(bytes);
      if (JSON.stringify(result) !== JSON.stringify(observed))
        throw Error("DEVELOPMENT_RESULT_CHANGED");
      if (
        result.task !== taskId ||
        !["verified", "blocked", "needs_acceptance"].includes(result.state)
      )
        throw Error("DEVELOPMENT_RESULT_INVALID");
      // A stale or edited result is never upgraded to successful verification.
      if (
        result.state === "verified" &&
        (!item.input.checks.length ||
          result.failures?.length !== 0 ||
          result.evidence?.length !== item.input.checks.length ||
          result.evidence.some(
            (e, i) =>
              e.exitCode !== 0 ||
              JSON.stringify(e.argv) !== JSON.stringify(item.input.checks[i]),
          ))
      )
        throw Error("DEVELOPMENT_RESULT_INVALID");
      const taskFolder = path.dirname(resultFile);
      for (const artifact of result.artifacts ?? []) {
        if (
          !["plan.md", "implementation.md", "review.json"].includes(
            artifact.file,
          ) ||
          hash(fs.readFileSync(path.join(taskFolder, artifact.file))) !==
            artifact.sha256
        )
          throw Error("DEVELOPMENT_ARTIFACT_CHANGED");
      }
      return {
        ...base,
        ...result,
        evidence: (result.evidence ?? []).map(e => ({ ...e, output: (e.output ?? '').slice(0, 16000), output_truncated: (e.output ?? '').length > 16000 })),
        task_id: taskId,
        result_path: resultFile,
        runs: [],
        questions: [],
        message:
          result.state === "verified"
            ? "宿主验收通过。"
            : result.state === "needs_acceptance"
              ? "执行阶段已结束，未设置验收命令，仍需验收。"
              : "验收未通过，请查看失败依据。",
      };
    }
    const runs = observed.runs ?? [],
      questions = observed.questions ?? [];
    const state = questions.length
      ? "awaiting_input"
      : runs.some((r) => ["running", "claimed"].includes(r.state))
        ? "running"
        : runs.some((r) => ["queued", "retrying"].includes(r.state))
          ? "queued"
          : runs.some((r) =>
                ["failed", "cancelled", "unknown"].includes(r.state),
              )
            ? "blocked"
            : "not_verified";
    return {
      ...base,
      state,
      runs,
      questions,
      message:
        state === "not_verified"
          ? "尚未取得宿主验收结果，请保留原编号继续查询。"
          : "",
    };
  }
  function start(request) {
    const input = validate(request);
    const taskId = `dev-${hash(input.request_id).slice(0, 32)}`;
    const folder = path.join(root, taskId);
    fs.mkdirSync(folder, { recursive: true });
    let lock;
    try {
      lock = fs.openSync(path.join(folder, "operation.lock"), "wx");
    } catch {
      throw Error("DEVELOPMENT_BUSY: 原操作仍在处理；保留原编号查询");
    }
    try {
      const file = path.join(folder, "request.json");
      const fingerprint = hash(JSON.stringify(input));
      if (fs.existsSync(file)) {
        const { item } = record(taskId);
        if (item.fingerprint !== fingerprint)
          throw Error("DEVELOPMENT_REQUEST_CONFLICT");
        return get(taskId);
      }
      write(file, {
        task_id: taskId,
        fingerprint,
        created_at: new Date().toISOString(),
        input,
      });
      fs.writeFileSync(path.join(folder, "brief.md"), input.brief, {
        flag: "wx",
      });
      write(path.join(folder, "checks.json"), input.checks);
      // Host guards are passed through the structured bridge, not prompt text.
      write(path.join(folder, "options.json"), {
        workflow: input.workflow,
        checks: input.checks,
        guards: [path.join(folder, "brief.md"), ...input.protected_files],
      });
      try {
        const receipt = bridge([
          "submit-json",
          taskId,
          input.workspace,
          path.join(folder, "brief.md"),
          path.join(folder, "options.json"),
        ]);
        if (receipt.task !== taskId)
          throw Error("DEVELOPMENT_IDENTITY_MISMATCH");
        write(path.join(folder, "receipt.json"), receipt);
        return {
          capability_id: "homebase.development",
          task_id: taskId,
          state: "submitted",
          workspace: input.workspace,
          workflow: input.workflow,
          message: "已提交；请查询原任务，提交成功不代表验收通过。",
        };
      } catch {
        return {
          capability_id: "homebase.development",
          task_id: taskId,
          state: "unknown",
          message: "提交结果未确认，原编号已保存；只查询原任务，避免重复开发。",
        };
      }
    } finally {
      fs.closeSync(lock);
      fs.unlinkSync(path.join(folder, "operation.lock"));
    }
  }
  function answer(request) {
    exact(request, ["task_id", "card_id", "option"]);
    const current = get(request.task_id);
    const card = current.questions.find((q) => q.id === request.card_id);
    if (
      !card ||
      !Number.isInteger(request.option) ||
      request.option < 1 ||
      request.option > card.options.length
    )
      throw Error("DEVELOPMENT_QUESTION_INVALID");
    bridge(["answer", String(request.card_id), String(request.option)]);
    return get(request.task_id);
  }
  return { catalog, start, get, answer, validate };
}
async function main() {
  let bytes = 0,
    chunks = [];
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > 64000) throw Error("DEVELOPMENT_INPUT_TOO_LARGE");
    chunks.push(chunk);
  }
  const payload = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  const api = createDevelopment({ home: payload.home });
  const action = process.argv[2];
  if (!["catalog", "start", "get", "answer"].includes(action))
    throw Error("DEVELOPMENT_ACTION_INVALID");
  return api[action](
    action === "get" ? payload.request.task_id : payload.request,
  );
}
if (
  process.argv[1] &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  main()
    .then((value) => console.log(JSON.stringify(value)))
    .catch((error) => {
      console.log(
        JSON.stringify({
          status: "failed",
          code: /^DEVELOPMENT_[A-Z_]+/.exec(String(error.message))?.[0] ?? "DEVELOPMENT_FAILED",
          message:
            "开发能力调用未完成，请保留原请求编号核对参数、依赖和原任务；没有自动重跑。",
        }),
      );
      process.exitCode = 1;
    });
}
