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
const testRoot = fs.mkdtempSync(path.join(os.tmpdir(), "homebase-control-"));
const inherited = new Set([
  "path", "pathext", "systemroot", "windir", "comspec", "systemdrive",
  "programfiles", "programfiles(x86)", "programw6432", "programdata",
  "number_of_processors", "processor_architecture", "os",
  "include", "lib", "libpath", "vsinstalldir", "vcinstalldir", "vctoolsinstalldir",
  "windowssdkdir", "windowssdkversion", "universalcrtsdkdir", "ucrtversion",
  "cargo_net_offline", "cargo_build_jobs", "rustup_toolchain", "rustflags",
  "rust_backtrace", "rustc", "rustc_wrapper", "rustc_workspace_wrapper", "cc", "cxx", "ar",
]);
const toolchain = Object.fromEntries(Object.entries(process.env).filter(([name]) =>
  inherited.has(name.toLowerCase()) || /^(VSCMD_|CARGO_PROFILE_|CARGO_TERM_)/i.test(name)
));
Object.assign(toolchain, {
  CARGO_HOME: process.env.CARGO_HOME || path.join(os.homedir(), ".cargo"),
  RUSTUP_HOME: process.env.RUSTUP_HOME || path.join(os.homedir(), ".rustup"),
  CARGO_TARGET_DIR: target,
});

function isolatedProfile(name) {
  const home = path.join(testRoot, name);
  const roaming = path.join(home, "AppData", "Roaming");
  const local = path.join(home, "AppData", "Local");
  for (const directory of [roaming, local]) fs.mkdirSync(directory, { recursive: true });
  return {
    ...toolchain,
    HOMEBASE_TEST_HOME: home,
    HOME: home,
    USERPROFILE: home,
    APPDATA: roaming,
    LOCALAPPDATA: local,
    XDG_CONFIG_HOME: path.join(home, ".config"),
    TMPDIR: home,
    TEMP: home,
    TMP: home,
    ...(process.platform === "win32" ? { SystemTemp: home } : {}),
  };
}

function run(phase, executable, args, env) {
  const result = spawnSync(executable, args, { cwd: root, stdio: "inherit", env });
  if (result.status === 0) return true;
  const reason = result.error?.code || result.signal || result.status || "unknown";
  console.error(`控制层测试阶段「${phase}」失败（${reason}）`);
  return false;
}
const cargoTests = ["test", "--locked", "--manifest-path", manifest];
const testThreads = ["--", "--test-threads=1"];
const unitPassed = run(
  "Rust 单测", "cargo", [...cargoTests, "--lib", ...testThreads], isolatedProfile("unit"),
);
const integrationPassed = run(
  "Rust 集成",
  "cargo",
  [...cargoTests, "--test", "*", "--no-fail-fast", ...testThreads],
  isolatedProfile("integration"),
);
const binary = path.join(target, "debug", `fixture${process.platform === "win32" ? ".exe" : ""}`);
const cliEnv = isolatedProfile("cli");
const fixtureReady = run(
  "CLI 夹具构建", "cargo", ["build", "--locked", "--manifest-path", manifest, "--bin", "fixture"], cliEnv,
);
const cliPassed = fixtureReady && run(
  "CLI 端到端",
  process.execPath,
  ["--test", "scripts/test-homebase-control.mjs"],
  { ...cliEnv, HOMEBASE_CONTROL_FIXTURE_BIN: binary },
);
if (!fixtureReady) console.error("CLI 夹具未构建成功，CLI 端到端测试未运行。");
process.exitCode = unitPassed && integrationPassed && cliPassed ? 0 : 1;
