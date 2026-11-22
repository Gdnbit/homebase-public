// Explicit isolated native smoke; never enables a model or uses a real key.
import fs from "node:fs";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
const repo = fileURLToPath(new URL("../", import.meta.url));
const parent = process.env.HOMEBASE_ASSISTANT_NATIVE_LAB;
assert(parent && path.isAbsolute(parent), "Set explicit HOMEBASE_ASSISTANT_NATIVE_LAB");
fs.mkdirSync(parent, { recursive: true });
const lab = fs.mkdtempSync(path.join(parent, "native-"));
const digest=file=>fs.existsSync(file)?crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex'):null;
const binary = path.resolve(process.env.HOMEBASE_ASSISTANT_NATIVE_BIN || path.join(repo, "src-tauri/target/debug/homebase.exe"));
const distributionRoot = path.join(path.dirname(binary), 'distribution');
const toolsRoot = path.join(distributionRoot, 'tools');
const local = relative => path.join(lab, relative);
const env = Object.fromEntries(Object.entries(process.env).filter(([name]) =>
  /^(path|pathext|systemroot|windir|comspec|systemdrive|programfiles(\(x86\))?|programw6432|programdata|number_of_processors|processor_architecture|os)$/i.test(name)
));
Object.assign(env, {
  HOMEBASE_TEST_HOME: lab, HOME: lab, USERPROFILE: lab,
  APPDATA: local('AppData/Roaming'), LOCALAPPDATA: local('AppData/Local'),
  TEMP: local('tmp'), TMP: local('tmp'), TMPDIR: local('tmp'), SystemTemp: local('tmp'),
  HOMEBASE_SECRETS_FILE: local('secrets.yaml'), HOMEBASE_SECRETS_META: local('secrets-meta.json'),
  HOMEBASE_AIHUB_ROOT: local('AIHub'), HOMEBASE_MODEL_ROOT: local('models'),
  HOMEBASE_RUNTIME_ROOT: local('runtime'), HOMEBASE_MODEL_CACHE: local('cache'),
  HOMEBASE_ASR_OUTPUT_ROOT: local('output'), SOPS_DISABLE_VERSION_CHECK: 'true',
  HOMEBASE_SOPS_EXE: path.join(toolsRoot, 'sops.exe'),
  HOMEBASE_AGE_KEYGEN: path.join(toolsRoot, 'age-keygen.exe'),
  HOMEBASE_REVEAL_EXE: path.join(distributionRoot, 'bin', 'homebase-reveal.exe'),
});
for (const directory of [env.APPDATA, env.LOCALAPPDATA, env.TEMP, local('Documents'), local('AIHub/models')]) {
  fs.mkdirSync(directory, {recursive: true});
}
// Packaged bootstrap can seed a missing manifest from a legacy machine path.
fs.writeFileSync(local('AIHub/models/installed-models.json'), JSON.stringify({schema_version: 1, models: {}}), {flag: 'wx'});
const testedRuntime=path.join(path.dirname(binary),'distribution/agent-runtime');
function start() {
  const log = fs.openSync(local('application.log'), 'a');
  try {
    return spawn(binary, [], {cwd: repo, env, windowsHide: true, stdio: ['ignore', log, log]});
  } finally {
    fs.closeSync(log);
  }
}
let app = start();
const sleep = ms => new Promise(r => setTimeout(r, ms));
function cli(args, allowFailure=false) {
  const request = ['scripts/homebase.mjs', ...args];
  const options = {cwd: repo, env, windowsHide: true, encoding: 'utf8', timeout: 20000};
  // A timed-out submit can already be durable. Retry only the same request ID.
  const replayable = (args[0] === 'call' && args.includes('--request-id')) ||
    (args[0] === 'jobs' && args[1] === 'get');
  for (let attempt = 0; ; attempt++) {
    const child = spawnSync(process.execPath, request, options);
    const value = child.stdout ? JSON.parse(child.stdout) : null;
    const interrupted = child.error?.code === 'ETIMEDOUT' ||
      ['TRANSPORT_TIMEOUT', 'OFFLINE'].includes(value?.error?.code);
    if (interrupted && replayable && attempt === 0) {
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);
      continue;
    }
    if (child.error) throw child.error;
    assert(value && (value.ok || allowFailure), JSON.stringify(value?.error));
    return value;
  }
}
let next = 0;
function call(command, input = {}, expected = 'succeeded') {
  const file = path.join(lab, `request-${++next}.json`);
  const grants = ['--allow-private', '--allow-write', '--allow-external'];
  fs.writeFileSync(file, JSON.stringify(input));
  const submitted = cli(['call', command, '--input', file, ...grants, '--request-id', `native-assistant-${next}`, '--detach']);
  let job = submitted.job;
  assert(job?.id, `Missing accepted job for ${command}`);
  const deadline = Date.now() + 75_000;
  while (['queued', 'running'].includes(job.status) && Date.now() < deadline) {
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 200);
    const queried = cli(['jobs', 'get', job.id, ...grants], true);
    assert(queried.job, JSON.stringify(queried.error));
    job = queried.job;
  }
  assert.equal(job.status, expected, `${command}: ${JSON.stringify(job.error || job.result)}`);
  return job.result;
}
try {
  const ep = path.join(lab, ".homebase/control/endpoint.json");
  for (let i = 0; i < 100 && !fs.existsSync(ep); i++) { if (app.exitCode !== null) throw Error("Native app exited before ready"); await sleep(200); }
  const doctor = cli(["doctor"]);
  const profileId=`io.github.gdnbit.homebase.test.${crypto.createHash('sha256').update(lab).digest('hex').slice(0,16)}`;
  assert(fs.existsSync(path.join(lab,'webview','main')), 'Native WebView profile must use the isolated root');
  const distribution=call('distribution_info');
  assert.equal(path.resolve(distribution.dataRoot), path.join(env.LOCALAPPDATA, profileId, 'distribution'),
    'Native distribution data must use the isolated profile');
  const config = call("assistant_config_get"); assert.equal(config.settings.enabled, false); assert.equal(config.settings.wechat_enabled, true); assert.equal(config.model_state, "disabled");
  const session = call("assistant_session_create");
  const request = { session_id: session.id, text: "隔离验证：模型未启用，请勿执行任务", request_id: "synthetic-disabled-message" };
  const ack = call("assistant_send", { request }); const repeated = call("assistant_send", { request }); assert.equal(ack.turn_id, repeated.turn_id);
  const turn = call("assistant_turn_get", { turnId: ack.turn_id }); assert.equal(turn.state, "disabled"); assert.match(turn.text, /尚未启用/); assert.equal(turn.jobs.length, 0);
  const settings = call("assistant_config_save", { settings: { ...config.settings, runtime:"pi", enabled: false, credential_source:"api_key_manager", free_only:true } }); assert.equal(settings.settings.enabled, false);assert.equal(settings.settings.runtime,"pi");assert.equal(settings.settings.credential_source,"api_key_manager");assert.equal(settings.settings.free_only,true);
  const personal = call("assistant_personal_get");
  assert.equal(personal.memories.length,0);
  assert.deepEqual(personal.followups,{revision:0,entries:[]});
  assert.deepEqual(call('assistant_matters_get'),personal.matters);
  assert.deepEqual(personal.matters.entries,[]);
  assert(cli(['describe','assistant_matter_save']).command.input_schema);
  const chat=call('assistant_send',{request:{session_id:session.id,text:'只聊聊当前隔离测试，不执行',request_id:'native-disabled-chat',mode:'chat'}});
  const disabledChat=call('assistant_turn_get',{turnId:chat.turn_id});assert.equal(disabledChat.state,'disabled');assert.equal(disabledChat.interaction,'chat');assert.deepEqual(disabledChat.jobs,[]);
  const matter={revision:0,id:null,title:'隔离持续事项',goal:'只记录合成任务，不修改真实数据',summary:'合成资料已准备',next_action:'等待后续核对',state:'waiting',review_at:null};
  const firstMatter=call('assistant_matter_save',{request:matter});assert.equal(firstMatter.state,'waiting');
  call('assistant_matter_save',{request:{...matter,revision:1,summary:'不得覆盖的同名事项'}},'failed');
  assert(cli(["describe","assistant_followup_save"]).command.input_schema);
  call("assistant_followup_save",{request:{revision:0,title:"隔离定时检查",instruction:"不能在未启用模型时执行",due_at:new Date(Date.now()+60_000).toISOString(),interval_minutes:null,max_runs:1}},'failed');
  assert.deepEqual(call("assistant_personal_get").followups,{revision:0,entries:[]});
  const local=call('assistant_local_config_get');
  call('assistant_local_execute',{operation:'files',request:{action:'write',path:'native-unknown-sentinel.txt',content:'ISOLATED NATIVE SENTINEL'},operationId:'native-write-sentinel',policyRevision:local.config.revision});
  const terminalInput=path.join(lab,'terminal-request.json');
  fs.writeFileSync(terminalInput,JSON.stringify({operation:'terminal',request:{command:"printf HB_NATIVE_UNKNOWN_TEST",write:false,timeout_ms:1000},operationId:'native-terminal-unknown',policyRevision:local.config.revision}));
  const terminalArgs=['call','assistant_local_execute','--input',terminalInput,'--allow-write','--allow-private','--allow-external','--request-id','native-terminal-unknown-once','--detach'];
  let terminal=cli(terminalArgs,true);assert(terminal.job?.id);const terminalId=terminal.job.id;
  for(let i=0;i<100;i++){terminal=cli(['jobs','get',terminalId,'--allow-private','--allow-write','--allow-external'],true);assert(terminal.job,JSON.stringify(terminal.error));if(!['queued','running'].includes(terminal.job.status))break;await sleep(500);}
  assert(!['queued','running'].includes(terminal.job.status),'Isolated terminal did not reach an explicit outcome');
  const terminalOutcome=terminal.job.result;
  assert(terminalOutcome&&typeof terminalOutcome.status==='string','Missing terminal business outcome');
  assert(terminalOutcome.status==='unknown'&&['EXECUTION_UNKNOWN','LOCAL_RESULT_UNKNOWN'].includes(terminalOutcome.code)||terminalOutcome.status==='succeeded'&&terminalOutcome.exit_code===0&&terminalOutcome.stdout.trim()==='HB_NATIVE_UNKNOWN_TEST',JSON.stringify(terminalOutcome));
  assert.equal(cli(terminalArgs,true).job.id,terminalId,'Unknown terminal replay must reuse original task');
  const unchanged=call('assistant_local_execute',{operation:'files',request:{action:'read',path:'native-unknown-sentinel.txt'},operationId:'native-read-sentinel',policyRevision:local.config.revision});
  assert.equal(unchanged.content,'ISOLATED NATIVE SENTINEL');
  const memory = { revision:personal.revision,id:null,title:"隔离偏好",content:"合成测试内容 native-personal-fixture",expires_at:null };
  call("assistant_memory_save", {request:memory});
  let stored=call("assistant_personal_get");
  assert.equal(stored.memories[0].content,memory.content);
  for(const id of ['homebase-operations','personal-memory','reminders','personal-workflows'])assert(stored.skills.some(s=>s.id===id));
  call("assistant_memory_save",{request:{...memory,revision:stored.revision,content:"不应覆盖的冲突偏好"}},'failed');
  const mid=stored.memories[0].id;
  call("assistant_memory_save",{request:{...memory,id:mid,revision:stored.revision,content:"临时更新的隔离偏好"}});
  let revised=call("assistant_personal_get");assert.equal(revised.memories[0].previous.length,1);
  call("assistant_memory_save",{request:{...memory,id:mid,revision:revised.revision}});
  stored=call("assistant_personal_get");assert.equal(stored.memories[0].content,memory.content);assert.equal(stored.memories[0].previous.length,2);
  const found=call('assistant_memory_search',{query:'隔离偏好'});assert.equal(found.memories[0].id,mid);assert.match(found.memories[0].expected_sha256,/^[a-f0-9]{64}$/);
  assert.equal(found.memories[0].previous,undefined);assert.equal(found.memories[0].corrections,undefined);assert(Buffer.byteLength(JSON.stringify(found))<9500);
  assert.deepEqual(call('assistant_memory_search',{query:'隔离偏好',offset:12}).memories,[]);
  const correction={revision:found.revision,id:mid,expected_sha256:found.memories[0].expected_sha256,content:memory.content+'（核对）',reason:'隔离原生纠错验证'};
  const corrected=call('assistant_memory_correct',{request:correction});assert.equal(corrected.id,mid);assert.equal(corrected.corrections[0].previous_content,memory.content);
  call('assistant_memory_correct',{request:correction},'failed');
  const fresh=call('assistant_memory_search',{query:'隔离偏好'});
  call('assistant_memory_correct',{request:{...correction,revision:fresh.revision,expected_sha256:fresh.memories[0].expected_sha256,content:memory.content,reason:'恢复合成夹具内容'}});
  stored=call('assistant_personal_get');assert.equal(stored.memories[0].corrections.length,2);assert.equal(stored.memories[0].content,memory.content);
  assert(cli(['describe','assistant_turn_steer']).command.input_schema);
  call('assistant_turn_steer',{request:{turn_id:turn.id,revision:0,request_id:'native-steer-disabled',text:'仅测试拒绝已结束任务'}},'failed');
  call("assistant_reminder_save", {request:{revision:stored.revision,id:null,text:"隔离原生提醒",due_at:"2026-01-01T00:00:00Z",interval_minutes:null,enabled:true}});
  for(let i=0;i<30;i++) { stored=call("assistant_personal_get");if(stored.reminders[0].delivery==="desktop_due")break;await sleep(100); }
  assert.equal(stored.reminders[0].delivery,"desktop_due");
  call("assistant_reminder_ack",{reminderId:stored.reminders[0].id,cancel:false});
  stored=call("assistant_personal_get");
  const due=Date.now()+20_000;
  call('assistant_matter_save',{request:{...matter,id:firstMatter.id,revision:1,review_at:new Date(due).toISOString()}});
  call("assistant_reminder_save",{request:{revision:stored.revision,id:null,text:"启动后无需打开助理页面的隔离提醒",due_at:new Date(due).toISOString(),interval_minutes:null,enabled:true}});
  app.kill(); await new Promise(resolve=>app.once("exit",resolve));
  app=start();
  for(let i=0;i<100;i++){await sleep(200);try{if(JSON.parse(fs.readFileSync(ep,'utf8')).pid===app.pid)break;}catch{}}
  await sleep(1000);
  assert(Date.now()<due,"Native startup exceeded reminder fixture deadline");
  const baseline=fs.statSync(path.join(lab,".homebase/assistant/state.dpapi")).mtimeMs;
  await sleep(Math.max(1,due-Date.now()+1500));
  assert(fs.statSync(path.join(lab,".homebase/assistant/state.dpapi")).mtimeMs>baseline,"Reminder timer did not run before any CLI/page assistant query");
  for(let i=0;i<100;i++){await sleep(200);try{if(call("assistant_config_get").settings.runtime==="pi")break;}catch{}}
  stored=call("assistant_personal_get");
  assert.equal(stored.memories[0].content,memory.content);
  assert.equal(stored.reminders[0].delivery,"dismissed");
  assert.equal(stored.reminders[0].enabled,false);
  assert.equal(stored.reminders[1].delivery,"desktop_due");
  const afterMatters=call('assistant_matters_get');assert.equal(afterMatters.entries[0].id,firstMatter.id);assert.equal(afterMatters.entries[0].delivery,'desktop_due');assert.equal(afterMatters.entries[0].state,'waiting');
  call('assistant_matter_review_ack',{matterId:firstMatter.id,revision:afterMatters.revision});
  assert.equal(call('assistant_matters_get').entries[0].delivery,'dismissed');
  const encrypted = fs.readFileSync(path.join(lab, ".homebase/assistant/state.dpapi")); assert(!encrypted.includes(Buffer.from(request.text))); assert(!encrypted.includes(Buffer.from(memory.content))); assert(encrypted.length > 0);
  const commands = cli(["jobs", "list"]).jobs.map(j => j.command); assert(!commands.includes("model_capability_call"));
  const runtime_hashes=Object.fromEntries(['pi-worker.mjs','package.json','package-lock.json','local/host.mjs','local/policy.mjs'].map(name=>[name,digest(path.join(testedRuntime,name))]));
  const report = { lab, binary, binary_sha256:digest(binary), runtime_hashes,doctor_ok: doctor.ok, isolated_webview:true,isolated_distribution:true, session_id: session.id, turn_id: turn.id, state: turn.state, duplicate_turn_preserved: true, personal_restart_preserved:true, reminder_due_and_ack:true, startup_timer_without_assistant_query:true, followup_contract:true,disabled_followup_rejected:true,memory_conflict_and_history:true,terminal_outcome:terminalOutcome.status,unknown_terminal_replay_dedup:true,native_file_sentinel_preserved:true,runtime:"pi",runtime_version:turn.runtime_version,encrypted_state_bytes: encrypted.length, plaintext_not_in_state: true, model_enabled: false, real_model_calls: 0, jobs: turn.jobs.length };
  Object.assign(report,{matter_contract:true,matter_restart_review_and_ack:true,disabled_discussion_no_model:true,original_task_progress_contract:!!turn.progress});
  Object.assign(report,{memory_correction_contract:true,memory_correction_stale_guard:true,memory_search_budget_and_offset:true,steering_contract:true,disabled_turn_steer_rejected:true});
  fs.writeFileSync(path.join(lab, "validation.json"), JSON.stringify(report, null, 2)); console.log(JSON.stringify(report, null, 2));
} finally { app.kill(); }
