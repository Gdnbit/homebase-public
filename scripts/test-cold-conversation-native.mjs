// Synthetic DPAPI seed from the core test, real native programs and CLI.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import net from 'node:net';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createSyntheticVault } from './local-native-vault-fixture.mjs';

const parent = process.env.HOMEBASE_COLD_NATIVE_LAB;
const home = process.env.HOMEBASE_COLD_SEED_HOME;
const binary = process.env.HOMEBASE_COLD_NATIVE_BINARY;
const previous = process.env.HOMEBASE_COLD_PREVIOUS_BINARY;
assert(parent && home && binary && previous);
for (const file of [home, binary, previous]) {
  assert(path.isAbsolute(file));
  assert(path.resolve(file).startsWith(path.resolve(parent) + path.sep));
}
const expected = JSON.parse(fs.readFileSync(path.join(home, 'cold-expected.json')));
assert(expected.synthetic_only && expected.real_model_calls === 0);
assert.equal(expected.sessions.length, 3);
const vault = createSyntheticVault(home);
const env = { ...vault.env, HOMEBASE_TEST_HOME: home, HOMEBASE_AIHUB_ROOT: path.join(home, 'AIHub'), HOMEBASE_MODEL_ROOT: path.join(home, 'models'), HOMEBASE_RUNTIME_ROOT: path.join(home, 'runtime'), HOMEBASE_MODEL_CACHE: path.join(home, 'cache'), HOMEBASE_ASR_OUTPUT_ROOT: path.join(home, 'output'), APPDATA: path.join(home, 'AppData/Roaming'), LOCALAPPDATA: path.join(home, 'AppData/Local'), TEMP: path.join(home, 'tmp'), TMP: path.join(home, 'tmp'), WEBVIEW2_USER_DATA_FOLDER: path.join(home, 'webview') };
for (const key of Object.keys(env)) if (/(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|HOMEBASE_CONTROL_ENDPOINT|HOMEBASE_ASSISTANT_SEGMENTS)/i.test(key)) delete env[key];
for (const dir of [env.APPDATA, env.LOCALAPPDATA, env.TEMP]) fs.mkdirSync(dir, { recursive: true });
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const hash = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const assistant = path.join(home, '.homebase/assistant');
const legacy = path.join(assistant, 'state.dpapi');
const head = path.join(assistant, 'history-v1/head.dpapi');
const segments = path.join(assistant, 'history-v1/segments');
const originalLegacy = hash(legacy);
const segmentHashes = Object.fromEntries(fs.readdirSync(segments).map(name => [name, hash(path.join(segments, name))]));
let app, endpoint;
async function rpc(method, fields = {}) {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: '127.0.0.1', port: Number(endpoint.endpoint.split(':')[1]) });
    let buffer = '';
    socket.setEncoding('utf8');
    socket.setTimeout(15000, () => { socket.destroy(); reject(Error('Owned native timeout')); });
    socket.on('error', reject);
    socket.on('connect', () => socket.write(JSON.stringify({ protocol: 1, id: crypto.randomUUID(), token: endpoint.token, method, ...fields }) + '\n'));
    socket.on('data', text => {
      buffer += text;
      if (buffer.includes('\n')) {
        socket.destroy();
        try { const result = JSON.parse(buffer); assert(result.ok, result.error?.code); resolve(result.result); } catch (error) { reject(error); }
      }
    });
  });
}
async function call(command, args = {}, failure) {
  const grants = ['private', 'write', 'external'];
  const ack = await rpc('call', { command, args, catalog_hash: endpoint.catalog_hash, grants, request_id: 'cold-native-' + crypto.randomUUID() });
  for (let n = 0; n < 200; n++) {
    const job = await rpc('jobs.get', { job_id: ack.job.id, grants });
    if (!['running', 'queued'].includes(job.status)) {
      if (failure) { assert.equal(job.status, 'failed'); assert.equal(job.error?.code, failure); return job.error; }
      assert.equal(job.status, 'succeeded', job.error?.code); return job.result;
    }
    await pause(50);
  }
  throw Error('Owned native job timed out');
}
async function start(executable) {
  app = spawn(executable, [], { env, cwd: process.cwd(), windowsHide: true, stdio: 'ignore' });
  for (let n = 0; n < 160; n++) {
    assert.equal(app.exitCode, null);
    await pause(250);
    try { const ep = JSON.parse(fs.readFileSync(path.join(home, '.homebase/control/endpoint.json'))); if (ep.pid !== app.pid) continue; endpoint = ep; await rpc('jobs.list'); return; } catch {}
  }
  throw Error('Owned native startup timed out');
}
async function stop() {
  if (app?.exitCode === null) { const child = app; child.kill(); await new Promise(resolve => child.once('exit', resolve)); }
  app = null;
}
function cli(command, args) {
  const result = spawnSync(process.execPath, ['scripts/homebase.mjs', 'call', command, '--input', '-', '--allow-private', '--allow-write', '--allow-external', '--wait-ms', '10000'], { env: { ...env, HOMEBASE_CONTROL_ENDPOINT: path.join(home, '.homebase/control/endpoint.json') }, input: JSON.stringify(args), encoding: 'utf8', windowsHide: true, timeout: 20000 });
  assert(!result.error);
  const parsed = JSON.parse(result.stdout); assert.equal(result.status, 0, parsed.error?.code); assert(parsed.ok, parsed.error?.code); return parsed.result;
}
async function readOriginals() {
  for (const session of expected.sessions) {
    const actual = await call('assistant_session_get', { sessionId: session.id });
    assert.deepEqual(actual.messages, session.messages);
    assert.deepEqual(actual.turns.map(t => ({ id: t.id, state: t.state })), session.turns.map(t => ({ id: t.id, state: t.state })));
  }
}
try {
  await start(binary);
  assert.equal((await call('assistant_config_get')).settings.enabled, false);
  assert.equal((await call('wechat_status')).bound, false);
  const rows = await call('assistant_sessions_list'); assert.equal(rows.length, 3); assert(rows.every(r => r.message_count === 2));
  const usage = await call('assistant_usage_get'); assert.equal(usage.turns, 3); assert.equal(usage.prompt_tokens, 21); assert.equal(usage.complete, true);
  let config = await call('assistant_config_get');
  await call('assistant_config_save', { settings: { ...config.settings, max_tokens: 3072 } });
  assert.deepEqual(Object.fromEntries(fs.readdirSync(segments).map(name => [name, hash(path.join(segments, name))])), segmentHashes);
  const beforeRead = hash(head);
  await readOriginals(); assert.equal(hash(head), beforeRead);
  const desktop = expected.sessions[0];
  const request = { session_id: null, text: '仅合成的闲聊', request_id: 'cold-original-0', mode: 'chat', attachments: [] };
  assert.equal(cli('assistant_send', { request }).turn_id, desktop.turns[0].id);
  assert.equal(cli('assistant_desktop_resolve', { scope: 'new', request }).ack.turn_id, desktop.turns[0].id);
  const page = cli('assistant_session_window_get', { request: { session_id: desktop.id, limit: 1 } });
  assert.equal(page.messages.length, 1); assert.equal(page.messages[0].id, desktop.messages.at(-1).id);
  const history = cli('assistant_history_query', { request: { session_id: desktop.id, limit: 8 } }); assert.equal(history.total, 2);
  await stop();
  await start(previous); await readOriginals();
  config = await call('assistant_config_get'); assert.equal(config.settings.max_tokens, 3072);
  await call('assistant_config_save', { settings: { ...config.settings, max_tokens: 3071 } });
  await stop();
  await start(binary); await readOriginals();
  assert.equal((await call('assistant_config_get')).settings.max_tokens, 3071);
  assert.equal(cli('assistant_send', { request }).turn_id, desktop.turns[0].id);
  assert.equal((await call('assistant_sessions_list')).length, 3);
  assert.equal(hash(legacy), originalLegacy);
  config = await call('assistant_config_get');
  await call('assistant_config_save', { settings: { ...config.settings, max_tokens: 3070 } });
  await stop();
  const originals = new Map(fs.readdirSync(segments).map(name => [name, fs.readFileSync(path.join(segments, name))]));
  const committed = hash(head);
  for (const name of originals.keys()) fs.writeFileSync(path.join(segments, name), Buffer.from('synthetic-cold-corruption'));
  await start(binary);
  assert.equal((await call('assistant_sessions_list')).length, 3);
  await call('assistant_session_get', { sessionId: desktop.id }, 'ARCHIVE_CHANGED');
  assert.equal(hash(head), committed); assert.equal(hash(legacy), originalLegacy);
  await stop();
  for (const [name, bytes] of originals) fs.writeFileSync(path.join(segments, name), bytes);
  await start(binary); await readOriginals(); assert.equal(hash(head), committed);
  assert.equal((await call('assistant_config_get')).settings.enabled, false);
  assert.equal((await call('wechat_status')).bound, false);
  const proof = { passed: true, completed_at: new Date().toISOString(), binary, binary_sha256: hash(binary), previous_binary: previous, previous_sha256: hash(previous), isolated_home: home, sessions: 3, original_body_and_turn_ids_exact: true, read_only_queries_preserve_head: true, metadata_save_preserves_cold_segments: true, usage_preserved: true, actual_cli_window_history_and_original_receipts: true, new_to_compatible_foundation_to_new_roundtrip: true, corrupt_cold_body_rejected_without_blank_or_new_head: true, original_ciphers_restore_exact_records: true, legacy_cipher_unchanged: true, capacity_limits_unchanged: true, model_disabled: true, wechat_unbound: true, real_model_calls: 0, real_wechat_messages: 0, production_deployed: false };
  fs.writeFileSync(path.join(parent, 'cold-conversation-native-v1.json'), JSON.stringify(proof, null, 2), { flag: 'wx' });
  console.log(JSON.stringify(proof));
} finally { await stop(); }
