import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";
import fs from "node:fs";
import os from "node:os";
const root = fileURLToPath(new URL("../", import.meta.url));
const manifest = "evals/control-plane/Cargo.toml";
const target = process.env.CARGO_TARGET_DIR
  ? path.resolve(root, process.env.CARGO_TARGET_DIR)
  : path.join(root, "evals/control-plane/target");
// A minimum guard, not a full cold-build space estimate. Do not start Cargo on
// an already full volume or silently move output to another user's directory.
let diskPath = target;
while (!fs.existsSync(diskPath) && path.dirname(diskPath) !== diskPath)
  diskPath = path.dirname(diskPath);
try {
  const disk = fs.statfsSync(diskPath);
  if (disk.bavail * disk.bsize < 1024 ** 3) {
    console.error("构建目录所在盘可用空间不足 1 GiB，未启动编译。请通过 CARGO_TARGET_DIR 指定空间充足的构建目录；此门槛不代表完整构建只需 1 GiB。");
    process.exit(1);
  }
} catch {
  console.error("无法核对构建目录可用空间，未启动编译。请检查 CARGO_TARGET_DIR 指向的目录或所在盘。");
  process.exit(1);
}
// Unit fixtures allocate their data below the temporary directory. Keep
// their scope aligned without replacing the explicit integration profile.
const unitEnv = { ...process.env };
if (process.env.HOMEBASE_TEST_HOME) {
  const unitRoot = fs.mkdtempSync(path.join(os.tmpdir(), "homebase-control-unit-"));
  Object.assign(unitEnv, { HOMEBASE_TEST_HOME: unitRoot, TMPDIR: unitRoot, TEMP: unitRoot, TMP: unitRoot });
}
const cargoTests = ["test", "--locked", "--manifest-path", manifest];
const testThreads = ["--", "--test-threads=1"];
for (const [args, env] of [
  [[...cargoTests, "--lib", ...testThreads], unitEnv],
  [[...cargoTests, "--test", "*", ...testThreads], process.env],
  [["build", "--locked", "--manifest-path", manifest, "--bin", "fixture"], process.env],
]) {
  const result = spawnSync("cargo", args, { cwd: root, stdio: "inherit", env });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
const binary = path.join(target, "debug", `fixture${process.platform === "win32" ? ".exe" : ""}`);
const result = spawnSync(
  process.execPath,
  ["--test", "scripts/test-homebase-control.mjs"],
  {
    cwd: root,
    stdio: "inherit",
    env: { ...process.env, HOMEBASE_CONTROL_FIXTURE_BIN: binary },
  },
);
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
