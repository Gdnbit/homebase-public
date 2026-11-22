// Actual app + CLI, with an explicit synthetic corpus and no provider calls.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import net from 'node:net';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createSyntheticVault } from './local-native-vault-fixture.mjs';

const lab = process.env.HOMEBASE_WINDOW_NATIVE_LAB;
const home = process.env.HOMEBASE_WINDOW_SEED_HOME;
const binary = process.env.HOMEBASE_WINDOW_NATIVE_BINARY;
const previous = process.env.HOMEBASE_WINDOW_PREVIOUS_BINARY;
const run = process.env.HOMEBASE_WINDOW_NATIVE_RUN || '1';
assert(/^\d+$/.test(run));
for (const file of [lab, home, binary, previous]) assert(file && path.isAbsolute(file));
assert(path.resolve(lab).replaceAll('\\', '/').startsWith('F:/HomeBase-Labs/'));
assert(path.resolve(home).startsWith(path.resolve(lab) + path.sep));
const expected = JSON.parse(fs.readFileSync(path.join(home, 'window-expected.json')));
assert(expected.synthetic_only && expected.model_disabled && expected.messages.length === 180);
const hash = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const protection = JSON.parse(fs.readFileSync(path.join(lab, 'task-rollout-production-verify-v2.json')));
function productionAlive() {
  const result = spawnSync('pwsh', ['-NoProfile', '-Command', '$p=Get-Process -Id '+protection.pid+' -ErrorAction Stop; if($p.Path -ne $env:HB_WINDOW_ORIGINAL_EXE){throw \'Original process identity changed\'}'], { env: { ...process.env, HB_WINDOW_ORIGINAL_EXE: protection.binary }, windowsHide: true, encoding: 'utf8', timeout: 10000 });
  assert.equal(result.status, 0, 'Production process must remain untouched');
  assert.equal(hash(protection.binary), protection.binary_sha256);
}
productionAlive();
const vault = createSyntheticVault(home);
const env = { ...vault.env, HOMEBASE_TEST_HOME: home, HOME: home, USERPROFILE: home, HOMEBASE_AIHUB_ROOT: path.join(home,'AIHub'), HOMEBASE_RUNTIME_ROOT: path.join(home,'runtime'), HOMEBASE_MODEL_ROOT: path.join(home,'models'), HOMEBASE_MODEL_CACHE: path.join(home,'cache'), HOMEBASE_ASR_OUTPUT_ROOT: path.join(home,'output'), APPDATA: path.join(home,'AppData/Roaming'), LOCALAPPDATA: path.join(home,'AppData/Local'), TEMP: path.join(home,'tmp'), TMP: path.join(home,'tmp'), WEBVIEW2_USER_DATA_FOLDER: path.join(home,'webview') };
for (const key of Object.keys(env)) if (/(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|HOMEBASE_CONTROL_ENDPOINT|HOMEBASE_ASSISTANT_(?:SEGMENTS|CONTEXT_SEGMENTS|TASK_ARCHIVE))/.test(key)) delete env[key];
for (const key of ['APPDATA','LOCALAPPDATA','TEMP']) fs.mkdirSync(env[key], {recursive:true});
const assistant = path.join(home,'.homebase/assistant'), head = path.join(assistant,'history-v1/head.dpapi'), legacy = path.join(assistant,'state.dpapi');
const context = path.resolve(expected.context_file), retained = context+'.retained-fixture';
assert(context.startsWith(path.join(assistant,'history-v1/segments')+path.sep));
assert(!fs.existsSync(retained));
const original = { head: hash(head), legacy: hash(legacy), context: hash(context) };
const segments = Object.fromEntries(fs.readdirSync(path.dirname(context)).map(n=>[n,hash(path.join(path.dirname(context),n))]));
let app, endpoint;
const wait = ms => new Promise(resolve=>setTimeout(resolve,ms));
async function rpc(method, fields={}) {
  return new Promise((resolve,reject)=>{
    const socket=net.createConnection({host:'127.0.0.1',port:Number(endpoint.endpoint.split(':')[1])});
    let text='';socket.setEncoding('utf8');socket.setTimeout(10000,()=>{socket.destroy();reject(Error('Synthetic read timeout'));});socket.on('error',reject);
    socket.on('connect',()=>socket.write(JSON.stringify({protocol:1,id:crypto.randomUUID(),token:endpoint.token,method,...fields})+'\n'));
    socket.on('data',chunk=>{text+=chunk;if(text.includes('\n')){socket.destroy();try{const value=JSON.parse(text);assert(value.ok,`${method} ${fields.command||''}: ${value.error?.code}`);resolve(value.result);}catch(error){reject(error);}}});
  });
}
async function call(command,args={},failure=null) {
  const grants=command==='assistant_send'?['private','write','external']:['private','write'];
  const ack=await rpc('call',{catalog_hash:endpoint.catalog_hash,command,args,grants,request_id:'cold-window-'+crypto.randomUUID()});
  for(let n=0;n<300;n++){
    const job=await rpc('jobs.get',{job_id:ack.job.id,grants});
    if(!['queued','running'].includes(job.status)){
      if(failure){assert.equal(job.status,'failed');assert.equal(job.error.code,failure);return job;}
      assert.equal(job.status,'succeeded',job.error?.code);return job.result;
    }
    await wait(50);
  }
  throw Error('Synthetic job wait limit');
}
function cliWindow(cursor) {
  const result=spawnSync(process.execPath,['scripts/homebase.mjs','call','assistant_session_window_get','--input','-','--allow-private','--wait-ms','30000'],{env:{...env,HOMEBASE_CONTROL_ENDPOINT:path.join(home,'.homebase/control/endpoint.json')},cwd:process.cwd(),windowsHide:true,encoding:'utf8',timeout:35000,maxBuffer:1024*1024,input:JSON.stringify({request:{session_id:expected.session_id,before_message_id:cursor,limit:40}})});
  assert(!result.error,result.error?.message);const value=JSON.parse(result.stdout);assert.equal(result.status,0,value.error?.code||result.stderr);assert(value.ok,value.error?.code);assert.equal(value.job.status,'succeeded');return value.result;
}
async function start(file) {
  productionAlive();app=spawn(file,[],{env,cwd:process.cwd(),windowsHide:true,stdio:'ignore'});assert.notEqual(app.pid,protection.pid);
  for(let n=0;n<160;n++){
    await wait(250);assert.equal(app.exitCode,null);
    try{const ep=JSON.parse(fs.readFileSync(path.join(home,'.homebase/control/endpoint.json')));if(ep.pid!==app.pid)continue;assert(/^127\.0\.0\.1:\d+$/.test(ep.endpoint));endpoint=ep;await rpc('jobs.list');assert.equal((await call('assistant_config_get')).settings.enabled,false);assert.equal((await call('wechat_status')).bound,false);return;}catch{}
  }
  throw Error('Owned synthetic app startup timeout');
}
async function stop() {
  if(app?.exitCode===null){const owned=app;assert.notEqual(owned.pid,protection.pid);spawnSync('taskkill',['/PID',String(owned.pid),'/T','/F'],{windowsHide:true,stdio:'ignore'});for(let n=0;n<100&&owned.exitCode===null;n++)await wait(50);assert.notEqual(owned.exitCode,null);}
  app=undefined;productionAlive();
}
function verify(page) {
  assert.equal(page.id,expected.session_id);assert.equal(page.page.total_messages,180);assert(page.messages.length<=40);
  assert.deepEqual(page.messages,expected.messages.slice(page.page.start_index,page.page.end_index));
  for(const row of page.turns)assert.equal(row.turn.progress.goal,expected.goal);
}
try {
  fs.renameSync(context,retained);
  await start(previous);
  await call('assistant_session_window_get',{request:{session_id:expected.session_id,limit:40}},'ARCHIVE_MISSING');
  console.log(JSON.stringify({phase:'old-whole-context-load',passed:true}));
  assert.equal(hash(head),original.head);await stop();
  await start(binary);
  const first=cliWindow(null);verify(first);assert.equal(first.page.start_index,140);
  verify(cliWindow(null));
  await call('assistant_session_window_get',{request:{session_id:expected.session_id,before_message_id:'unknown-original',limit:40}},'WINDOW_CURSOR_NOT_FOUND');
  let cursor=null,pages=[];
  do {const page=cliWindow(cursor);verify(page);pages.push(page.messages);cursor=page.page.before_message_id;}while(cursor);
  assert.deepEqual(pages.reverse().flat(),expected.messages);
  await call('assistant_session_get',{sessionId:expected.session_id},'ARCHIVE_MISSING');
  verify(cliWindow(null));assert.equal(hash(head),original.head);assert.equal(hash(legacy),original.legacy);
  console.log(JSON.stringify({phase:'cold-cli-pages-without-context',passed:true,pages:pages.length}));
  await stop();fs.renameSync(retained,context);assert.equal(hash(context),original.context);
  await start(binary);verify(cliWindow(null));
  const config=await call('assistant_config_get');config.settings.max_tokens=3000;
  await call('assistant_config_save',{settings:config.settings});await stop();
  await start(binary);verify(cliWindow(null));assert.equal((await call('assistant_config_get')).settings.max_tokens,3000);
  assert.deepEqual((await call('assistant_session_get',{sessionId:expected.session_id})).messages,expected.messages);
  console.log(JSON.stringify({phase:'metadata-save-and-restart',passed:true}));
  assert.equal((await call('assistant_send',{request:expected.request})).turn_id,expected.turn_id);
  assert.equal(hash(legacy),original.legacy);
  for(const[name,digest]of Object.entries(segments))assert.equal(hash(path.join(path.dirname(context),name)),digest);
  const report={passed:true,completed_at:new Date().toISOString(),home,binary,binary_sha256:hash(binary),previous,previous_sha256:hash(previous),old_whole_load_reproduced:true,missing_unrequested_context:true,cli_pages:pages.length,total_messages:180,original_progress_goal:true,unknown_cursor_specific:true,full_read_missing_context_explicit:true,read_only_head_unchanged:true,restored_context_and_restart:true,metadata_save_keeps_all_original_segments:true,original_request_no_replay:true,legacy_cipher_unchanged:true,real_model_calls:0,real_wechat_messages:0,production_pid:protection.pid,production_deployed:false};
  fs.writeFileSync(path.join(lab,`cold-window-native-v${run}.json`),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
} finally {
  await stop();if(fs.existsSync(retained)&&!fs.existsSync(context))fs.renameSync(retained,context);
}
