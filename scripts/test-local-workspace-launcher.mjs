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
function verifyProtectedFallbacks(t, labParent) {
  assert(pwsh && path.isAbsolute(pwsh));
  assert(/^[CDF]:\/HomeBase-Labs(?:\/|$)/i.test(labParent.replaceAll('\\', '/')));
  const lab = fs.mkdtempSync(path.join(labParent, 'hb-workspace-fallback-'));
  t.diagnostic(lab);
  const profile = path.join(lab, 'synthetic-profile'), app = path.join(lab, 'synthetic-app');
  fs.mkdirSync(profile); fs.mkdirSync(app);
  fs.writeFileSync(path.join(app, 'homebase.exe'), 'Synthetic non-executable; verify-only test.');
  const version = { root: app, binary_sha256: hash(path.join(app, 'homebase.exe')), critical_files: {} };
  const settings = { schema_version: 1, current: version, fallback: version, process_env: Object.fromEntries(['HOMEBASE_AIHUB_ROOT', 'HOMEBASE_MODEL_ROOT', 'HOMEBASE_RUNTIME_ROOT', 'HOMEBASE_ASR_OUTPUT_ROOT', 'HOMEBASE_MODEL_CACHE', 'HOMEBASE_ASR_PYTHON', 'HOMEBASE_FFMPEG', 'HOMEBASE_REVEAL_EXE', 'HOMEBASE_SECRETS_FILE'].map(k => [k, path.join(lab, k)])), fallback_requires_absent: ['.homebase/inbox'] };
  const settingsPath = path.join(lab, 'settings.json');
  let scenarios = 0;
  const run = (rollback, expected) => {
    fs.writeFileSync(settingsPath, JSON.stringify(settings));
    const env = { ...process.env, USERPROFILE: profile, HOME: profile }; delete env.HOMEBASE_TEST_HOME;
    const result = spawnSync(pwsh, ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', path.join(repository, 'scripts/launch-local-workspace.ps1'), '-SettingsPath', settingsPath, '-VerifyOnly', ...(rollback ? ['-Rollback'] : [])], { env, encoding: 'utf8', windowsHide: true, timeout: 20000 });
    assert.ifError(result.error); assert.equal(result.status, expected ? 0 : 1, result.stdout + result.stderr);
    const report = JSON.parse(result.stdout); assert.equal(report.verified, expected);
    if (expected) assert.equal(report.launch_performed, false); else assert.equal(report.started, false);
    scenarios++;
  };
  run(true, true);
  const inbox = path.join(profile, '.homebase/inbox'); fs.mkdirSync(inbox, { recursive: true });
  run(true, false);
  const source = path.join(inbox, 'source.workspace.dpapi'); fs.writeFileSync(source, 'SYNTHETIC_PRIVATE_BYTES');
  const before = hash(source); run(true, false); run(false, true); assert.equal(hash(source), before);
  settings.fallback_requires_absent = []; run(true, false);
  settings.fallback_requires_absent = ['../untrusted']; run(true, false);
  delete settings.fallback_requires_absent;
  settings.fallback_state_guard = { [source]: before };
  run(true, true); // Only a data-compatible fallback may omit format guards.
  fs.writeFileSync(source, 'SYNTHETIC CHANGED STATE'); run(true, false);
  fs.writeFileSync(source, 'SYNTHETIC_PRIVATE_BYTES');
  settings.fallback_requires_absent = ['.homebase/capabilities/development-workspace']; run(true, true);
  const development = path.join(profile, '.homebase/capabilities/development-workspace'); fs.mkdirSync(development, { recursive: true });
  run(true, false);
  const editor = path.join(development, 'editor.workspace.dpapi'); fs.writeFileSync(editor, 'SYNTHETIC DEVELOPER DRAFT');
  const editorBefore = hash(editor); run(true, false); run(false, true);
  assert.equal(hash(source), before); assert.equal(hash(editor), editorBefore);
  delete settings.fallback_requires_absent;
  run(true, true);
  if (labParent.startsWith('D:')) {
    const outside = fs.mkdtempSync('D:/HomeBase-Labs-outside-');
    fs.copyFileSync(path.join(app, 'homebase.exe'), path.join(outside, 'homebase.exe'));
    settings.current = { ...version, root: outside };
    // The binary hash matches. The root guard should reject the fixture.
    run(false, false);
    settings.current = version;
  }
  const archive = path.join(profile, '.homebase/assistant/history-v1');
  fs.mkdirSync(archive, { recursive: true });
  const head = path.join(archive, 'head.dpapi');
  fs.writeFileSync(head, 'SYNTHETIC ARCHIVE HEAD');
  const headBefore = hash(head);
  run(false, false); run(true, false);
  assert.equal(hash(head), headBefore);
  t.diagnostic(JSON.stringify({ scenarios, actual_app_launches: 0, source_unchanged: true }));
}

test('legacy and compatible fallbacks protect editors and archives on F:', t =>
  verifyProtectedFallbacks(t, os.tmpdir()));

test('legacy and compatible fallbacks protect editors and archives on C:', t =>
  verifyProtectedFallbacks(t, 'C:/HomeBase-Labs'));

test('D: accepts packages within HomeBase-Labs and rejects a similarly named root', t =>
  verifyProtectedFallbacks(t, 'D:/HomeBase-Labs'));

test('local launcher clears inherited archive capacity flags before the hidden child starts', t => {
  assert(pwsh && path.isAbsolute(pwsh));
  const lab = fs.mkdtempSync(path.join(os.tmpdir(), 'hb-workspace-environment-'));
  assert(lab.replaceAll('\\', '/').startsWith('F:/HomeBase-Labs/'));
  const profile = path.join(lab, 'profile'), app = path.join(lab, 'app');
  fs.mkdirSync(profile); fs.mkdirSync(app);
  const binary = path.join(app, 'homebase.exe');
  fs.writeFileSync(binary, 'Synthetic non-executable: Start-Process is replaced by the test.');
  const version = {root: app, binary_sha256: hash(binary), critical_files: {}};
  const paths = Object.fromEntries(['HOMEBASE_AIHUB_ROOT', 'HOMEBASE_MODEL_ROOT', 'HOMEBASE_RUNTIME_ROOT', 'HOMEBASE_ASR_OUTPUT_ROOT', 'HOMEBASE_MODEL_CACHE', 'HOMEBASE_ASR_PYTHON', 'HOMEBASE_FFMPEG', 'HOMEBASE_REVEAL_EXE', 'HOMEBASE_SECRETS_FILE'].map(k => [k, path.join(lab, k)]));
  const settings = path.join(lab, 'settings.json');
  fs.writeFileSync(settings, JSON.stringify({schema_version: 1, current: version, fallback: version, process_env: paths}));
  const flags = ['HOMEBASE_ASSISTANT_SEGMENTS', 'HOMEBASE_ASSISTANT_CONTEXT_SEGMENTS', 'HOMEBASE_ASSISTANT_TASK_ARCHIVE', 'HOMEBASE_ASSISTANT_CAPACITY'];
  const probe = path.join(lab, 'probe.mjs');
  fs.writeFileSync(probe, `import fs from 'node:fs';fs.writeFileSync(process.env.HB_LOCAL_TEST_REPORT,JSON.stringify({flags:Object.fromEntries(${JSON.stringify(flags)}.map(k=>[k,Object.hasOwn(process.env,k)])),model:process.env.HOMEBASE_MODEL_ROOT}),{flag:'wx'});`);
  const wrapper = path.join(lab, 'wrapper.ps1');
  fs.writeFileSync(wrapper, `
function Get-Process { param($Name,$ErrorAction) return @() }
function Start-Process {
  param($FilePath,$WorkingDirectory,$WindowStyle,[switch]$PassThru)
  if($FilePath -ne $env:HB_LOCAL_TEST_BINARY -or $WorkingDirectory -ne $env:HB_LOCAL_TEST_APP -or $WindowStyle -ne 'Hidden'){throw 'Unexpected test child'}
  & $env:HB_LOCAL_TEST_NODE $env:HB_LOCAL_TEST_PROBE
  if($LASTEXITCODE){throw 'Probe failed'}
  return [pscustomobject]@{Id=101}
}
& $env:HB_LOCAL_TEST_LAUNCHER -SettingsPath $env:HB_LOCAL_TEST_SETTINGS
`);
  for (const value of ['1', 'unknown']) {
    const report = path.join(lab, `report-${value}.json`);
    const env = {...process.env, USERPROFILE: profile, HOME: profile,
      ...Object.fromEntries(flags.map(k => [k, value])), HB_LOCAL_TEST_REPORT: report,
      HB_LOCAL_TEST_BINARY: binary, HB_LOCAL_TEST_APP: app, HB_LOCAL_TEST_NODE: process.execPath,
      HB_LOCAL_TEST_PROBE: probe, HB_LOCAL_TEST_LAUNCHER: path.join(repository, 'scripts/launch-local-workspace.ps1'), HB_LOCAL_TEST_SETTINGS: settings};
    delete env.HOMEBASE_TEST_HOME;
    const result = spawnSync(pwsh, ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', wrapper], {env, encoding: 'utf8', windowsHide: true, timeout: 20000});
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.equal(JSON.parse(result.stdout).started, true);
    const child = JSON.parse(fs.readFileSync(report, 'utf8'));
    assert(flags.every(k => child.flags[k] === false), 'archive flag leaked to child');
    assert.equal(child.model, paths.HOMEBASE_MODEL_ROOT);
  }
  t.diagnostic(JSON.stringify({root: lab, environment_cases: 2, real_homebase_processes: 0}));
});
