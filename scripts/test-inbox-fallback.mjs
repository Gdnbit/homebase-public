import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repository = fileURLToPath(new URL('../', import.meta.url));
const pwsh = process.env.HOMEBASE_PWSH_EXE;
const hash = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
test('new Inbox private state blocks legacy fallback and remains untouched; verify never launches', t => {
  assert(pwsh && path.isAbsolute(pwsh));
  const lab = fs.mkdtempSync(path.join(os.tmpdir(), 'hb-inbox-fallback-'));
  assert(lab.replaceAll('\\', '/').startsWith('F:/HomeBase-Labs/'));
  t.diagnostic(lab);
  const profile = path.join(lab, 'synthetic-profile'), app = path.join(lab, 'synthetic-app');
  fs.mkdirSync(profile); fs.mkdirSync(app);
  fs.writeFileSync(path.join(app, 'homebase.exe'), 'Synthetic non-executable; verify-only test.');
  const version = { root: app, binary_sha256: hash(path.join(app, 'homebase.exe')), critical_files: {} };
  const settings = { schema_version: 1, current: version, fallback: version, process_env: Object.fromEntries(['HOMEBASE_AIHUB_ROOT', 'HOMEBASE_MODEL_ROOT', 'HOMEBASE_RUNTIME_ROOT', 'HOMEBASE_ASR_OUTPUT_ROOT', 'HOMEBASE_MODEL_CACHE', 'HOMEBASE_ASR_PYTHON', 'HOMEBASE_FFMPEG', 'HOMEBASE_REVEAL_EXE', 'HOMEBASE_SECRETS_FILE'].map(k => [k, path.join(lab, k)])), fallback_requires_absent: ['.homebase/inbox'] };
  const settingsPath = path.join(lab, 'settings.json');
  const run = (rollback, expected) => {
    fs.writeFileSync(settingsPath, JSON.stringify(settings));
    const env = { ...process.env, USERPROFILE: profile, HOME: profile }; delete env.HOMEBASE_TEST_HOME;
    const result = spawnSync(pwsh, ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', path.join(repository, 'scripts/launch-local-workspace.ps1'), '-SettingsPath', settingsPath, '-VerifyOnly', ...(rollback ? ['-Rollback'] : [])], { env, encoding: 'utf8', windowsHide: true, timeout: 20000 });
    assert.ifError(result.error); assert.equal(result.status, expected ? 0 : 1, result.stdout + result.stderr);
    const report = JSON.parse(result.stdout); assert.equal(report.verified, expected);
    if (expected) assert.equal(report.launch_performed, false); else assert.equal(report.started, false);
  };
  run(true, true); // Old format only: fallback can still be verified.
  const inbox = path.join(profile, '.homebase/inbox'); fs.mkdirSync(inbox, { recursive: true });
  run(true, false); // Even an empty format directory is not evidence of absence.
  const source = path.join(inbox, 'source.workspace.dpapi'); fs.writeFileSync(source, 'SYNTHETIC_PRIVATE_BYTES');
  const before = hash(source); run(true, false); run(false, true); assert.equal(hash(source), before);
  settings.fallback_requires_absent = []; run(true, false);
  settings.fallback_requires_absent = ['../untrusted']; run(true, false);
  assert.equal(hash(source), before);
});
