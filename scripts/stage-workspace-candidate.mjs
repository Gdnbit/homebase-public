// Offline staging only. This does not install, launch, migrate or replace HomeBase.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';

export const digest = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const sha = value => assert.match(value, /^[a-f0-9]{64}$/, 'Expected SHA-256');
// Public design provenance referenced by the WeChat acceptance document.
const publicDocument = relative => /^(?:control|docs)\/.+\.(?:md|txt)$/.test(relative) || relative === 'outputs/page-design/wechat/2026-11-10/README.md';
export function relativeFile(value) {
  assert(typeof value === 'string' && value.length > 0 && !/[\\:\x00-\x1f]/.test(value), 'Unsafe relative file');
  assert(!value.startsWith('/') && value.split('/').every(p => p && p !== '.' && p !== '..'), 'Unsafe relative file');
  assert(value.split('/').every(p => !/[. ]$/.test(p) && !/^(?:con|prn|aux|nul|com[0-9]|lpt[0-9])(?:\.|$)/i.test(p)), 'Unsafe Windows file');
  return value;
}

function regularFile(root, relative) {
  relativeFile(relative);
  let current = path.resolve(root);
  assert(fs.lstatSync(current).isDirectory() && !fs.lstatSync(current).isSymbolicLink(), 'Linked source root');
  for (const part of relative.split('/')) {
    current = path.join(current, part);
    assert(!fs.lstatSync(current).isSymbolicLink(), 'Linked source component');
  }
  assert(fs.lstatSync(current).isFile(), 'Expected regular file');
  return current;
}

// Only local public usage documents, never source links, private files or URLs.
export function documentClosure(root, seeds) {
  const found = new Set(), pending = [...seeds];
  while (pending.length) {
    const relative = relativeFile(pending.pop());
    assert(publicDocument(relative), 'Not an interface document');
    if (found.has(relative)) continue;
    const filename = regularFile(root, relative);
    const text = fs.readFileSync(filename, 'utf8');
    found.add(relative);
    for (const match of text.matchAll(/\[[^\]]*\]\(([^\s)]+)(?:\s+"[^"]*")?\)/g)) {
      let target = match[1].replace(/^<|>$/g, '').split('#')[0];
      if (!target || /^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('/')) continue;
      target = decodeURIComponent(target);
      if (!/\.(?:md|txt)$/.test(target)) continue;
      const resolved = path.posix.normalize(path.posix.join(path.posix.dirname(relative), target));
      assert(publicDocument(resolved), `Document link leaves the interface: ${relative} -> ${resolved}`);
      pending.push(resolved);
    }
  }
  return [...found].sort();
}

function sourceFiles(root, directory) {
  const found = [];
  for (const entry of fs.readdirSync(path.join(root, directory), {withFileTypes:true})) {
    const relative = `${directory}/${entry.name}`;
    assert(!entry.isSymbolicLink(), 'Linked runtime source');
    if (entry.isDirectory()) found.push(...sourceFiles(root, relative));
    else { regularFile(root, relative); found.push(relative); }
  }
  return found;
}

export function stageCandidate(config) {
  const {repo, lab, runtimeStage, runtimeManifest, sourceSnapshot, binary, binarySha256, destination} = config;
  for (const item of [repo, lab, runtimeStage, runtimeManifest, sourceSnapshot, binary, destination]) assert(path.isAbsolute(item), 'Absolute paths required');
  sha(binarySha256);
  const laboratory = path.resolve(lab), target = path.resolve(destination);
  assert(/^[DF]:\/HomeBase-Labs\//i.test(laboratory.replaceAll('\\', '/')), 'Explicit D or F drive laboratory required');
  assert(path.dirname(target) === laboratory && !fs.existsSync(target), 'A new direct laboratory child is required');
  const info = fs.lstatSync(laboratory);
  assert(info.isDirectory() && !info.isSymbolicLink() &&
    fs.realpathSync.native(laboratory).toLowerCase() === laboratory.toLowerCase(), 'Linked laboratory');
  assert(digest(binary) === binarySha256, 'Candidate binary changed');
  const snapshot = JSON.parse(fs.readFileSync(sourceSnapshot, 'utf8'));
  assert(Object.keys(snapshot.files).length > 0, 'Empty build snapshot');
  for (const [relative, expected] of Object.entries(snapshot.files)) {
    sha(expected);
    const file = path.isAbsolute(relative) ? relative : regularFile(repo, relative);
    if (path.isAbsolute(relative)) assert(path.resolve(file).startsWith(laboratory + path.sep), 'Build artifact outside laboratory');
    assert(digest(file) === expected, 'Build inputs changed since native acceptance');
  }
  const previous = JSON.parse(fs.readFileSync(runtimeManifest, 'utf8'));
  assert(previous.passed === true && path.resolve(previous.stage) === path.resolve(runtimeStage), 'Runtime manifest does not identify this stage');
  const entries = new Map();
  function add(relative, source, expected) {
    relativeFile(relative); sha(expected);
    assert(!entries.has(relative), 'Duplicate staging target');
    entries.set(relative, {source, hash:expected});
  }
  for (const [relative, expected] of Object.entries(previous.files)) {
    if (relative === 'homebase.exe') continue;
    assert(!/^distribution\/agent-runtime\/(?:worker\.mjs$|node_modules\/(?:@langchain\/|deepagents\/|langsmith\/))/.test(relative), 'Retired assistant runtime cannot be packaged');
    assert(/^distribution\/(?:agent-runtime|native-agent-runtime|document-runtime|capability-pipeline|aihub|bin|runtime|tools|licenses)\//.test(relative) || /^distribution\/(?:python-packages\.txt|README-安装与迁移\.md|THIRD-PARTY-NOTICES\.md)$/.test(relative), 'Unrecognized runtime file');
    const source = regularFile(runtimeStage, relative);
    sha(expected);
    assert(digest(source) === expected, 'Recorded runtime file changed');
    add(relative, source, expected);
  }
  // Reuse installed dependencies only with the identical current lock and package.
  for (const directory of ['agent-runtime', 'native-agent-runtime', 'document-runtime']) {
    for (const name of ['package.json', 'package-lock.json']) {
      const relative = `distribution/${directory}/${name}`;
      assert(entries.get(relative)?.hash === digest(regularFile(repo, `${directory}/${name}`)), 'Runtime dependency lock mismatch');
    }
  }
  const runtimeSources = [
    'agent-runtime/pi-worker.mjs', ...sourceFiles(repo, 'agent-runtime/local'),
    ...sourceFiles(repo, 'agent-runtime/tools'), ...sourceFiles(repo, 'agent-runtime/skills'),
    'native-agent-runtime/worker.mjs', 'native-agent-runtime/mcp-bridge.mjs',
    'document-runtime/worker.mjs', 'document-runtime/extract.mjs',
  ];
  for (const relative of runtimeSources) {
    const source = regularFile(repo, relative), target = `distribution/${relative}`;
    entries.set(target, {source, hash:digest(source)});
  }
  for (const name of ['development-capability.mjs','repo-capabilities.mjs','repo-capabilities-store.mjs','repo-capabilities-analysis.mjs','repo-capabilities-process.mjs','repo-capabilities-sandbox.py','control-validation.mjs']) {
    const source = regularFile(repo, `scripts/${name}`);
    entries.set(`distribution/capability-pipeline/${name}`, {source, hash:digest(source)});
  }
  const documents = documentClosure(repo, ['control/START.md','control/OPERATIONS.md','control/reference.md']);
  const interfaceFiles = [...documents, 'AGENTS.md', 'control/commands.json', 'control/policy.json', 'control/interface-guide.json', 'control/bootstrap.template.txt', 'control/capability-selection-format.json', 'control/workflow-context-format.json'];
  const scriptQueue = ['scripts/homebase.mjs'], scripts = new Set();
  while (scriptQueue.length) {
    const relative = relativeFile(scriptQueue.pop());
    if (scripts.has(relative)) continue;
    assert(/^scripts\/[^/]+\.mjs$/.test(relative), 'Unexpected CLI dependency');
    const file = regularFile(repo, relative);
    scripts.add(relative);
    const text = fs.readFileSync(file, 'utf8');
    for (const match of text.matchAll(/(?:from\s*|import\s*(?:\(\s*)?)\s*["'](\.\.?\/[^"']+)["']/g)) {
      scriptQueue.push(path.posix.normalize(path.posix.join(path.posix.dirname(relative), match[1])));
    }
  }
  for (const relative of [...interfaceFiles, ...scripts]) {
    const file = regularFile(repo, relative); add(relative, file, digest(file));
  }
  add('homebase.exe', binary, binarySha256);
  const totalBytes = [...entries.values()].reduce((sum, entry) => sum + fs.statSync(entry.source).size, 0);
  const disk = fs.statfsSync(laboratory);
  assert(disk.bavail * disk.bsize > totalBytes + 256 * 1024 * 1024, 'Insufficient staging capacity');
  const catalog = JSON.parse(fs.readFileSync(regularFile(repo, 'control/commands.json'), 'utf8'));
  sha(catalog.catalog_hash);
  // All validation above is read only. From here onward, only the new destination.
  fs.mkdirSync(target);
  const hashes = {};
  for (const [relative, entry] of entries) {
    assert(digest(entry.source) === entry.hash, 'Source changed during staging');
    const output = path.join(target, relative);
    fs.mkdirSync(path.dirname(output), {recursive:true});
    fs.copyFileSync(entry.source, output, fs.constants.COPYFILE_EXCL);
    assert(digest(output) === entry.hash, 'Staged file integrity mismatch');
    hashes[relative] = entry.hash;
  }
  function generated(relative, text) {
    const output = path.join(target, relative);
    fs.writeFileSync(output, text, {flag:'wx'}); hashes[relative] = digest(output);
  }
  generated('package.json', JSON.stringify({private:true, type:'module'}, null, 2)+'\n');
  generated('hb.cmd', '@echo off\r\n"%~dp0distribution\\agent-runtime\\node.exe" "%~dp0scripts\\homebase.mjs" %*\r\n');
  generated('README.md', '# HomeBase isolated candidate\n\nNot installed or approved for daily use. No production launcher or rollback is included.\nUse the bundled hb.cmd or distribution/agent-runtime/node.exe scripts/homebase.mjs.\nRun only with an explicit fresh HOMEBASE_TEST_HOME and synthetic runtime roots.\nDo not point this candidate at an existing profile. UI and compatible rollback acceptance remain required.\n');
  // The candidate exercises real packaged resource resolution; no model weights/data.
  generated('distribution/bundle-manifest.json', JSON.stringify({schema_version:1, package:'HomeBase isolated Windows candidate', generated_at:new Date().toISOString(), signed:false, model_weights_included:false, secrets_included:false, runtime_manifest_sha256:digest(runtimeManifest)}, null, 2)+'\n');
  const manifest = {schema_version:1, prepared_at:new Date().toISOString(), destination:target, binary_sha256:binarySha256, catalog_hash:catalog.catalog_hash, source_snapshot_sha256:digest(sourceSnapshot), runtime_manifest_sha256:digest(runtimeManifest), files:hashes, bytes:totalBytes, interface_documents:documents.length, cli_scripts:scripts.size, staged:true, acceptance_passed:false, production_modified:false, rollback_ready:false};
  fs.writeFileSync(path.join(target, 'stage-manifest.json'), JSON.stringify(manifest, null, 2)+'\n', {flag:'wx'});
  return {destination:target, files:Object.keys(hashes).length, interface_documents:documents.length, cli_scripts:scripts.size, binary_sha256:binarySha256, acceptance_passed:false, production_modified:false};
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  assert.equal(process.argv.length, 3, 'Usage: node scripts/stage-workspace-candidate.mjs ABSOLUTE_CONFIG_JSON');
  assert(path.isAbsolute(process.argv[2]), 'Absolute config required');
  console.log(JSON.stringify(stageCandidate(JSON.parse(fs.readFileSync(process.argv[2], 'utf8')))));
}
