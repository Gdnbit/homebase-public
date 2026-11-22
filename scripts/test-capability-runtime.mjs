import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  recipe,
  rootPath,
  videoInput,
  composition,
  cleanEnvironment,
  catalog,
  install,
  resultFor,
  dispatch,
  child,
} from "./capability-runtime.mjs";
const fixture = () =>
  fs.mkdtempSync(path.join(os.tmpdir(), "homebase-capability-isolated-"));
const marker = (root) =>
  fs.writeFileSync(
    path.join(root, ".homebase-capability.json"),
    JSON.stringify({ owner: "homebase-capability-runtime-v1" }),
  );
test("request validation accepts bounded values and rejects arbitrary code, paths and extra keys", () => {
  assert.equal(videoInput({ title: "Synthetic" }).seconds, 6);
  for (const input of [
    { title: "" },
    { title: "ok", seconds: 1 },
    { title: "ok", seconds: 13 },
    { title: "ok", command: "synthetic" },
    null,
  ])
    assert.throws(() => videoInput(input));
  assert.throws(() => rootPath("relative"));
  assert.throws(() => rootPath(path.parse(process.cwd()).root));
});
test("composition escapes text, uses local timeline and has no remote assets", () => {
  const html = composition({
    title: "<script>synthetic</script>",
    subtitle: '"<&',
  });
  assert.ok(html.includes("&lt;script&gt;synthetic&lt;/script&gt;"));
  assert.ok(html.includes("window.__timelines"));
  assert.ok(!html.includes("https://"));
  assert.ok(html.includes("connect-src 'none'"));
});
test("renderer environment does not inherit synthetic credentials or user profile", () => {
  process.env.HOMEBASE_SYNTHETIC_SECRET = "synthetic-only";
  const root = fixture();
  const env = cleanEnvironment(root);
  assert.equal(env.HOMEBASE_SYNTHETIC_SECRET, undefined);
  assert.equal(env.USERPROFILE, path.join(root, "home"));
  assert.equal(env.HYPERFRAMES_NO_TELEMETRY, "1");
  delete process.env.HOMEBASE_SYNTHETIC_SECRET;
});
test("installation refuses nonempty existing directories without modifying their contents", async () => {
  const root = fixture();
  const sentinel = path.join(root, "synthetic-existing-data.txt");
  fs.writeFileSync(sentinel, "unchanged");
  await assert.rejects(install(root), /不是空目录/);
  assert.equal(fs.readFileSync(sentinel, "utf8"), "unchanged");
  assert.deepEqual(fs.readdirSync(root), ["synthetic-existing-data.txt"]);
});
test("catalog reads are passive and malformed installation remains unknown", () => {
  const root = path.join(fixture(), "not-created");
  assert.equal(catalog(root).state, "not_installed");
  assert.ok(!fs.existsSync(root));
  fs.mkdirSync(root);
  marker(root);
  fs.writeFileSync(path.join(root, "installation.json"), "{synthetic broken");
  const before = fs.readFileSync(path.join(root, "installation.json"));
  assert.equal(catalog(root).state, "unknown");
  assert.deepEqual(
    fs.readFileSync(path.join(root, "installation.json")),
    before,
  );
});
test("concurrent operations refuse existing lock without deleting it", async () => {
  const root = fixture();
  marker(root);
  fs.writeFileSync(path.join(root, ".operation.lock"), "synthetic-lock");
  await assert.rejects(install(root), /已有操作/);
  assert.equal(
    fs.readFileSync(path.join(root, ".operation.lock"), "utf8"),
    "synthetic-lock",
  );
});
test("symlinked roots are refused without touching target data", () => {
  const root = fixture();
  const target = fixture();
  const link = path.join(root, "junction");
  fs.symlinkSync(
    target,
    link,
    process.platform === "win32" ? "junction" : "dir",
  );
  assert.throws(() => rootPath(link), /符号链接/);
});
test("persistent results reject traversal, missing files and changed artifacts", () => {
  const root = fixture();
  marker(root);
  assert.throws(() => resultFor(root, "../synthetic"));
  const id = crypto.randomUUID();
  const folder = path.join(root, "runs", id);
  fs.mkdirSync(folder, { recursive: true });
  const video = Buffer.from("synthetic-artifact-not-a-real-video");
  fs.writeFileSync(path.join(folder, "output.mp4"), video);
  fs.writeFileSync(
    path.join(folder, "result.json"),
    JSON.stringify({
      run_id: id,
      status: "succeeded",
      capability_id: recipe.id,
      version: recipe.version,
      artifact: {
        sha256: crypto.createHash("sha256").update(video).digest("hex"),
      },
      verification: { decoded: true },
    }),
  );
  assert.equal(resultFor(root, id).run_id, id);
  fs.appendFileSync(path.join(folder, "output.mp4"), "changed");
  assert.throws(() => resultFor(root, id), /外部修改/);
});
test("unsupported actions fail explicitly", async () => {
  await assert.rejects(dispatch("arbitrary-shell", {}), /不支持/);
});
test("timeout terminates the isolated subprocess instead of claiming success", async () => {
  const root = fixture();
  await assert.rejects(child(process.execPath, ["-e", "setInterval(()=>{},1000)"], root, root, path.join(root, "timeout.log"), 150), /执行超时/);
});
test("JSON stdin preserves Chinese UTF-8 characters split between pipe chunks", async () => {
  const root = fixture(); const requested = path.join(root, "中文目录");
  const bytes = Buffer.from(JSON.stringify({ root_path: requested }));
  const split = bytes.indexOf(Buffer.from("中")) + 1;
  const output = await new Promise((resolve, reject) => {
    const process = spawn(globalThis.process.execPath, [fileURLToPath(new URL("./capability-runtime.mjs", import.meta.url)), "catalog"], { env: cleanEnvironment(root), stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
    const chunks = []; process.stdout.on("data", data => chunks.push(data));
    process.once("error", reject); process.once("close", code => code === 0 ? resolve(Buffer.concat(chunks).toString("utf8")) : reject(new Error("Fixture CLI failed")));
    process.stdin.write(bytes.subarray(0, split));
    setTimeout(() => process.stdin.end(bytes.subarray(split)), 100);
  });
  assert.equal(JSON.parse(output).root_path, requested);
});
