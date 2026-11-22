import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import assert from 'node:assert/strict';
import test from 'node:test';
import {relativeFile, documentClosure, stageCandidate, digest} from './stage-workspace-candidate.mjs';

test('staging rejects Windows traversal, alternate streams and ambiguous names', () => {
  for (const name of ['../a','a/../b','/a','C:/a','a\\b','a//b','a/./b','a:stream','a/CON.txt','a/nul','a/trailing.','a/trailing ','a/line\n']) {
    assert.throws(() => relativeFile(name), name);
  }
  assert.equal(relativeFile('distribution/licenses/许可证.txt'), 'distribution/licenses/许可证.txt');
  assert.equal(relativeFile('distribution/agent-runtime/node_modules/.bin/yaml.cmd'), 'distribution/agent-runtime/node_modules/.bin/yaml.cmd');
});

function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'hb-public-docs-'));
  // Deliberately retain fixtures as acceptance evidence; no recursive deletion.
  fs.mkdirSync(path.join(root, 'control')); fs.mkdirSync(path.join(root, 'docs'));
  t.diagnostic(root);
  return root;
}
test('interface closure follows nested usage documents once, ignores source and external links', t => {
  const root = fixture(t);
  fs.writeFileSync(path.join(root, 'control/START.md'), '[usage](../docs/usage.md#intro) [external](https://example.org/x.md) [code](../src/private.rs)');
  fs.writeFileSync(path.join(root, 'docs/usage.md'), '[more](more%20help.md) [start](../control/START.md)');
  fs.writeFileSync(path.join(root, 'docs/more help.md'), 'Public help');
  assert.deepEqual(documentClosure(root, ['control/START.md']), ['control/START.md','docs/more help.md','docs/usage.md']);
});
test('a missing usage document fails staging instead of silently shipping broken guidance', t => {
  const root = fixture(t);
  fs.writeFileSync(path.join(root, 'control/START.md'), '[missing](../docs/missing.md)');
  assert.throws(() => documentClosure(root, ['control/START.md']), /ENOENT/);
});
test('interface traversal cannot read a private Markdown document', t => {
  const root = fixture(t);
  fs.writeFileSync(path.join(root, 'private.md'), 'Never include this');
  fs.writeFileSync(path.join(root, 'control/START.md'), '[private](../private.md)');
  assert.throws(() => documentClosure(root, ['control/START.md']), /leaves the interface/);
});
test('a directory junction cannot bring private documents into the interface', t => {
  const root=fixture(t), outside=path.join(root,'synthetic-private');
  fs.mkdirSync(outside);fs.writeFileSync(path.join(outside,'private.md'),'Synthetic private body');
  fs.symlinkSync(outside,path.join(root,'docs/linked'),process.platform==='win32'?'junction':'dir');
  fs.writeFileSync(path.join(root,'control/START.md'),'[private](../docs/linked/private.md)');
  assert.throws(() => documentClosure(root,['control/START.md']), /Linked source component/);
});

function packageFixture(t) {
  const lab = fixture(t), repo = path.join(lab, 'repo'), runtimeStage = path.join(lab, 'runtime-stage');
  assert(/^[DF]:\/HomeBase-Labs\//i.test(lab.replaceAll('\\','/')), 'Package tests require an isolated D or F drive TEMP');
  const write = (root, relative, text) => {const file=path.join(root,relative);fs.mkdirSync(path.dirname(file),{recursive:true});fs.writeFileSync(file,text);return file;};
  const previous = {passed:true, stage:runtimeStage, files:{}};
  for (const directory of ['agent-runtime','native-agent-runtime','document-runtime']) {
    for (const file of ['package.json','package-lock.json']) {
      write(repo, `${directory}/${file}`, '{}');
      const staged = write(runtimeStage, `distribution/${directory}/${file}`, '{}');
      previous.files[`distribution/${directory}/${file}`] = digest(staged);
    }
    write(repo, `${directory}/worker.mjs`, 'export {};');
  }
  for (const directory of ['agent-runtime/local','agent-runtime/tools','agent-runtime/skills']) write(repo, `${directory}/public.txt`, 'Synthetic public runtime resource');
  for (const file of ['agent-runtime/pi-worker.mjs','native-agent-runtime/mcp-bridge.mjs','document-runtime/extract.mjs']) write(repo, file, 'export {};');
  for (const file of ['development-capability.mjs','repo-capabilities.mjs','repo-capabilities-store.mjs','repo-capabilities-analysis.mjs','repo-capabilities-process.mjs','repo-capabilities-sandbox.py','control-validation.mjs']) write(repo, `scripts/${file}`, 'Synthetic source');
  for (const file of ['control/START.md','control/OPERATIONS.md','control/reference.md','AGENTS.md','control/policy.json','control/interface-guide.json','control/bootstrap.template.txt','control/capability-selection-format.json','control/workflow-context-format.json']) write(repo, file, 'Synthetic public guidance');
  write(repo, 'control/commands.json', JSON.stringify({catalog_hash:'a'.repeat(64),commands:[]}));
  write(repo, 'scripts/homebase.mjs', 'import "./helper.mjs";');
  write(repo, 'scripts/helper.mjs', 'export {};');
  write(repo, 'private.yaml', 'Must never be staged');
  const binary = write(lab, 'synthetic.exe', 'Synthetic non-executable binary');
  const runtimeManifest = write(lab, 'runtime.json', JSON.stringify(previous));
  const sourceSnapshot = write(lab, 'snapshot.json', JSON.stringify({files:{'AGENTS.md':digest(path.join(repo,'AGENTS.md'))}}));
  return {repo,lab,runtimeStage,runtimeManifest,sourceSnapshot,binary,binarySha256:digest(binary),destination:path.join(lab,'candidate')};
}
test('a new isolated package has complete local CLI dependencies and preserves unrelated data', t => {
  const config = packageFixture(t), secretHash = digest(path.join(config.repo, 'private.yaml'));
  const staged = stageCandidate(config), manifest = JSON.parse(fs.readFileSync(path.join(config.destination,'stage-manifest.json')));
  assert.equal(staged.acceptance_passed,false);assert.equal(staged.production_modified,false);
  assert(fs.existsSync(path.join(config.destination,'scripts/helper.mjs')));
  assert(fs.existsSync(path.join(config.destination,'distribution/agent-runtime/pi-worker.mjs')));
  assert(fs.existsSync(path.join(config.destination,'control/capability-selection-format.json')));
  assert(fs.existsSync(path.join(config.destination,'control/workflow-context-format.json')));
  assert.equal(fs.existsSync(path.join(config.destination,'private.yaml')),false);
  assert.equal(fs.existsSync(path.join(config.destination,'launch-settings.json')),false);
  assert.equal(digest(path.join(config.repo, 'private.yaml')),secretHash);
  for (const [file,hash] of Object.entries(manifest.files)) assert.equal(digest(path.join(config.destination,file)),hash);
  const before = digest(path.join(config.destination,'stage-manifest.json'));
  assert.throws(() => stageCandidate(config), /new direct laboratory child/);
  assert.equal(digest(path.join(config.destination,'stage-manifest.json')),before);
});
test('recorded runtime corruption fails before creating the candidate directory', t => {
  const config = packageFixture(t);
  fs.appendFileSync(path.join(config.runtimeStage,'distribution/agent-runtime/package.json'), 'changed');
  assert.throws(() => stageCandidate(config), /Recorded runtime file changed/);
  assert.equal(fs.existsSync(config.destination),false);
});
test('staging rejects a laboratory reached through a directory link', t => {
  const config = packageFixture(t);
  const actual = path.join(config.lab, 'actual');
  fs.mkdirSync(path.join(actual, 'nested'), {recursive:true});
  const redirected = path.join(config.lab, 'redirected');
  fs.symlinkSync(actual, redirected, 'junction');
  const lab = path.join(redirected, 'nested');
  assert.throws(() => stageCandidate({...config, lab, destination:path.join(lab, 'candidate')}), /Linked laboratory/);
  assert.equal(fs.existsSync(path.join(actual, 'nested', 'candidate')), false);
});
test('source drift fails before copying a native candidate', t => {
  const config = packageFixture(t);
  fs.appendFileSync(path.join(config.repo,'AGENTS.md'), 'changed');
  assert.throws(() => stageCandidate(config), /Build inputs changed/);
  assert.equal(fs.existsSync(config.destination),false);
});
test('changed dependency lock cannot reuse an earlier dependency installation', t => {
  const config = packageFixture(t);
  fs.appendFileSync(path.join(config.repo,'document-runtime/package-lock.json'), 'changed');
  assert.throws(() => stageCandidate(config), /Runtime dependency lock mismatch/);
  assert.equal(fs.existsSync(config.destination),false);
});
