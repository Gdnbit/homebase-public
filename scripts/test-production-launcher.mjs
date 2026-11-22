import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';

// Run the exact launcher with synthetic settings and a Start-Process stub.
// No HomeBase process, real profile or UI is opened by this test.
const [launcher, lab] = process.argv.slice(2);
assert(launcher && lab && path.isAbsolute(launcher) && path.isAbsolute(lab));
assert(fs.statSync(lab).isDirectory());
const root = fs.mkdtempSync(path.join(lab, 'launcher-environment-'));
const target = path.join(root, 'Launch-HomeBase.ps1');
const probe = path.join(root, 'probe.mjs');
const wrapper = path.join(root, 'wrapper.ps1');
const program = path.join(root, 'synthetic-program.bin');
const rollback = path.join(root, 'rollback.ps1');
const flags = ['HOMEBASE_ASSISTANT_SEGMENTS', 'HOMEBASE_ASSISTANT_CONTEXT_SEGMENTS', 'HOMEBASE_ASSISTANT_TASK_ARCHIVE'];
const digest = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
fs.copyFileSync(launcher, target, fs.constants.COPYFILE_EXCL);
fs.writeFileSync(program, 'Synthetic launcher test. Never executable.');
fs.writeFileSync(probe, `import fs from 'node:fs';
const flags=${JSON.stringify(flags)};
fs.writeFileSync(process.env.HB_LAUNCH_TEST_REPORT,JSON.stringify({flags:Object.fromEntries(flags.map(k=>[k,{present:Object.hasOwn(process.env,k),value:process.env[k]??null}])),path:process.env.HOMEBASE_MODEL_ROOT??null}),{flag:'wx'});
`);
fs.writeFileSync(rollback, '& $env:HB_LAUNCH_TEST_NODE $env:HB_LAUNCH_TEST_PROBE\nif($LASTEXITCODE){throw \'Probe failed\'}\n');
const settings = {
  binary: path.basename(program),
  critical_files: { [path.basename(program)]: digest(program) },
  process_env: { HOMEBASE_MODEL_ROOT: path.join(root, 'synthetic-models') },
  rollback_launcher: rollback,
  rollback_guard: { [rollback]: digest(rollback) },
};
fs.writeFileSync(path.join(root, 'launch-settings.json'), JSON.stringify(settings));
fs.writeFileSync(wrapper, `param([switch]$Rollback)
$ErrorActionPreference='Stop'
function Get-Process { param($Name,$ErrorAction) return @() }
function Start-Process {
  param($FilePath,$WorkingDirectory,$WindowStyle,[switch]$PassThru)
  if($FilePath -ne $env:HB_LAUNCH_TEST_PROGRAM -or $WorkingDirectory -ne $env:HB_LAUNCH_TEST_ROOT -or $WindowStyle -ne 'Hidden'){throw 'Unexpected synthetic launch'}
  & $env:HB_LAUNCH_TEST_NODE $env:HB_LAUNCH_TEST_PROBE
  if($LASTEXITCODE){throw 'Probe failed'}
  return [pscustomobject]@{Id=101}
}
& (Join-Path $PSScriptRoot 'Launch-HomeBase.ps1') -Rollback:$Rollback
`);
const base = { ...process.env, USERPROFILE: root, HB_LAUNCH_TEST_NODE: process.execPath, HB_LAUNCH_TEST_ROOT: root, HB_LAUNCH_TEST_PROGRAM: program, HB_LAUNCH_TEST_PROBE: probe };
delete base.HOMEBASE_TEST_HOME;
const results = [];
function run(name, value, rollbackMode = false, rejection = null, extra = {}) {
  const report = path.join(root, `${name}.json`);
  const env = { ...base, ...Object.fromEntries(flags.map(key => [key, value])), HB_LAUNCH_TEST_REPORT: report, ...extra };
  const result = spawnSync('pwsh', ['-NoProfile', '-File', wrapper, ...(rollbackMode ? ['-Rollback'] : [])], { env, windowsHide: true, encoding: 'utf8', timeout: 30000 });
  assert(!result.error, result.error?.message);
  if (rejection) {
    assert.notEqual(result.status, 0, name);
    assert((result.stderr + result.stdout).includes(rejection), `${name}: missing expected rejection`);
    assert(!fs.existsSync(report), `${name}: must not launch a child`);
  } else {
    assert.equal(result.status, 0, result.stderr);
    const observed = JSON.parse(fs.readFileSync(report));
    for (const flag of flags) assert.equal(observed.flags[flag].present, false, `${name}: ${flag} leaked to child`);
    if (!rollbackMode) assert.equal(observed.path, settings.process_env.HOMEBASE_MODEL_ROOT);
  }
  results.push({ name, passed: true, child_started: !rejection });
}
try {
  for (const [name, value] of [['enabled', '1'], ['disabled', '0'], ['empty', ''], ['invalid', 'unknown']]) {
    run(name, value);
    run(`${name}-rollback`, value, true);
  }
  run('test-home-refused', '1', false, 'Refusing a production launch', { HOMEBASE_TEST_HOME: root });
  const history = path.join(root, '.homebase/assistant/history-v1');
  fs.mkdirSync(history, { recursive: true });
  run('migrated-home-refused', '1', false, 'preserves legacy storage only');
  fs.renameSync(history, `${history}-retained-fixture`);
  fs.appendFileSync(program, '\nChanged synthetic content.');
  run('program-integrity-refused', '1', false, 'Program integrity mismatch');
  fs.appendFileSync(rollback, '\n# Changed synthetic content.\n');
  run('rollback-integrity-refused', '1', true, 'Rollback package integrity mismatch');
  console.log(JSON.stringify({ passed: true, launcher_sha256: digest(launcher), exact_script: true, root, cases: results, real_homebase_processes_started: 0, real_model_calls: 0, real_wechat_messages: 0 }));
} catch (error) {
  console.error(JSON.stringify({ passed: false, launcher_sha256: digest(launcher), root, completed_cases: results, reason: error.message, real_homebase_processes_started: 0 }));
  process.exitCode = 1;
}
