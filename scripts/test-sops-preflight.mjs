import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
const root = fileURLToPath(new URL("../", import.meta.url));
const executable = fileURLToPath(
  new URL(
    "../src-tauri/resources/distribution/tools/sops.exe",
    import.meta.url,
  ),
);
if (!existsSync(executable))
  throw Error(
    "Bundled SOPS test executable is missing; no real vault/identity fallback permitted.",
  );
for (const selection of [
  ["--lib", "reveal_runtime::tests::"],
  ["--lib", "adapters::sops_probe::tests::"],
  ["--test", "sops_probe"],
]) {
  const result = spawnSync(
    "cargo",
    [
      "test",
      "--locked",
      "--manifest-path",
      "evals/secret-lifecycle/Cargo.toml",
      ...selection,
      "--",
      "--test-threads=1",
    ],
    {
      cwd: root,
      stdio: "inherit",
      env: { ...process.env, HOMEBASE_TEST_SOPS_EXE: executable },
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
