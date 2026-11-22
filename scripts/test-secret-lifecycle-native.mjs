import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
const root = fileURLToPath(new URL('../', import.meta.url));
const tools = fileURLToPath(new URL('../src-tauri/resources/distribution/tools/', import.meta.url));
for (const exe of ['sops.exe', 'age-keygen.exe']) {
  if (!existsSync(`${tools}/${exe}`)) throw new Error(`Missing isolated test prerequisite: ${exe}. Run bundle:prepare first.`);
}
for (const selection of [
  ['--lib', '--test', 'isolated_sops'],
  ['--bin', 'homebase-reveal-isolated', 'process_runner::tests::'],
]) {
  const result = spawnSync('cargo', ['test', '--locked', '--manifest-path', 'evals/secret-lifecycle/Cargo.toml', ...selection, '--', '--test-threads=1'], {
    cwd: root, stdio: 'inherit', env: { ...process.env, HOMEBASE_TEST_SOPS_BIN_DIR: tools },
  });
  if (result.error) throw result.error;
  if (result.status !== 0) { process.exitCode = result.status ?? 1; break; }
}
