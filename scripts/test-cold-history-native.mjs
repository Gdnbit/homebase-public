// Actual HomeBase/CLI history reads. Only an explicit, synthetic F: corpus.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import net from 'node:net';
import assert from 'node:assert/strict';
import {spawn,spawnSync} from 'node:child_process';
import {createSyntheticVault} from './local-native-vault-fixture.mjs';

const lab=process.env.HOMEBASE_HISTORY_SNAPSHOT_LAB,home=process.env.HOMEBASE_WINDOW_SEED_HOME,
  binary=process.env.HOMEBASE_HISTORY_SNAPSHOT_BINARY,previous=process.env.HOMEBASE_HISTORY_SNAPSHOT_PREVIOUS,
  run=process.env.HOMEBASE_HISTORY_SNAPSHOT_RUN||'1';
for(const p of [lab,home,binary,previous])assert(p&&path.isAbsolute(p));
assert(/^\d+$/.test(run));assert(path.resolve(lab).replaceAll('\\','/').startsWith('F:/HomeBase-Labs/'));
assert(path.resolve(home).startsWith(path.resolve(lab)+path.sep));
const expected=JSON.parse(fs.readFileSync(path.join(home,'window-expected.json')));
assert(expected.synthetic_only&&expected.model_disabled&&expected.messages.length===180);
const hash=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
const original=JSON.parse(fs.readFileSync(path.join(lab,'task-rollout-production-verify-v2.json')));
function productionAlive(){
  const r=spawnSync('pwsh',['-NoProfile','-Command','$p=Get-Process -Id '+original.pid+' -ErrorAction Stop; if($p.Path -ne $env:HB_HISTORY_ORIGINAL_EXE){throw \'Original identity changed\'}'],{env:{...process.env,HB_HISTORY_ORIGINAL_EXE:original.binary},encoding:'utf8',windowsHide:true,timeout:10000});
  assert.equal(r.status,0,'Production process must remain untouched');assert.equal(hash(original.binary),original.binary_sha256);
}
productionAlive();
const vault=createSyntheticVault(home),env={...vault.env,HOMEBASE_TEST_HOME:home,HOME:home,USERPROFILE:home,HOMEBASE_AIHUB_ROOT:path.join(home,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(home,'models'),HOMEBASE_RUNTIME_ROOT:path.join(home,'runtime'),HOMEBASE_MODEL_CACHE:path.join(home,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(home,'output'),APPDATA:path.join(home,'AppData/Roaming'),LOCALAPPDATA:path.join(home,'AppData/Local'),TEMP:path.join(home,'tmp'),TMP:path.join(home,'tmp'),WEBVIEW2_USER_DATA_FOLDER:path.join(home,'webview')};
for(const name of Object.keys(env))if(/(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|HOMEBASE_CONTROL_ENDPOINT|HOMEBASE_ASSISTANT_(?:SEGMENTS|CONTEXT_SEGMENTS|TASK_ARCHIVE))/.test(name))delete env[name];
for(const name of ['APPDATA','LOCALAPPDATA','TEMP'])fs.mkdirSync(env[name],{recursive:true});
const root=path.join(home,'.homebase/assistant'),head=path.join(root,'history-v1/head.dpapi'),headHeld=head+'.retained-fixture',legacy=path.join(root,'state.dpapi'),context=path.resolve(expected.context_file),contextHeld=context+'.retained-fixture';
assert(context.startsWith(path.join(root,'history-v1/segments')+path.sep));assert(!fs.existsSync(headHeld)&&!fs.existsSync(contextHeld));
const initial={head:hash(head),legacy:hash(legacy),context:hash(context)};
const segments=Object.fromEntries(fs.readdirSync(path.dirname(context)).map(name=>[name,hash(path.join(path.dirname(context),name))]));
let app,endpoint,cliCalls=0;
const wait=ms=>new Promise(resolve=>setTimeout(resolve,ms));
async function rpc(method,fields={}){
  return new Promise((resolve,reject)=>{const socket=net.createConnection({host:'127.0.0.1',port:Number(endpoint.endpoint.split(':')[1])});let text='';socket.setEncoding('utf8');socket.setTimeout(10000,()=>{socket.destroy();reject(Error('Synthetic history timeout'));});socket.on('error',reject);socket.on('connect',()=>socket.write(JSON.stringify({protocol:1,id:crypto.randomUUID(),token:endpoint.token,method,...fields})+'\n'));socket.on('data',chunk=>{text+=chunk;if(text.includes('\n')){socket.destroy();try{const v=JSON.parse(text);assert(v.ok,`${method} ${fields.command||''}: ${v.error?.code}`);resolve(v.result);}catch(error){reject(error);}}});});
}
async function call(command,args={},failure=null){
  const grants=command==='assistant_send'?['private','write','external']:['private','write'];
  const ack=await rpc('call',{catalog_hash:endpoint.catalog_hash,command,args,grants,request_id:'cold-history-'+crypto.randomUUID()});
  for(let n=0;n<300;n++){const job=await rpc('jobs.get',{job_id:ack.job.id,grants});if(!['queued','running'].includes(job.status)){if(failure){assert.equal(job.status,'failed');assert.equal(job.error.code,failure);return job;}assert.equal(job.status,'succeeded',job.error?.code);return job.result;}await wait(50);}throw Error('Synthetic history job wait limit');
}
function cli(command,request){
  const r=spawnSync(process.execPath,['scripts/homebase.mjs','call',command,'--input','-','--allow-private','--wait-ms','30000'],{env:{...env,HOMEBASE_CONTROL_ENDPOINT:path.join(home,'.homebase/control/endpoint.json')},windowsHide:true,encoding:'utf8',timeout:35000,maxBuffer:1024*1024,input:JSON.stringify({request})});assert(!r.error,r.error?.message);const value=JSON.parse(r.stdout);assert.equal(r.status,0,value.error?.code);assert(value.ok,value.error?.code);cliCalls++;return value.result;
}
const query=(kind='messages')=>({session_id:expected.session_id,kind,query:'',offset:0,limit:20});
async function read(snapshot,item,useCli=true){
  let offset=0,text='';let first;
  for(let n=0;n<5000;n++){
    const request={session_id:expected.session_id,snapshot_id:snapshot,item_id:item,offset,limit:8192};
    const chunk=useCli?cli('assistant_history_read',request):await call('assistant_history_read',{request});
    first??=chunk;assert.equal(chunk.offset,offset);assert.equal(chunk.json_sha256,first.json_sha256);text+=chunk.content;
    if(chunk.next_offset===null||chunk.next_offset===undefined){assert(chunk.complete);assert.equal(Buffer.byteLength(text),chunk.total_bytes);assert.equal(crypto.createHash('sha256').update(text).digest('hex'),chunk.json_sha256);return {value:JSON.parse(text),sha256:chunk.json_sha256};}
    assert(chunk.next_offset>offset);offset=chunk.next_offset;
  }throw Error('History body did not complete');
}
async function start(file){
  productionAlive();app=spawn(file,[],{env,cwd:process.cwd(),windowsHide:true,stdio:'ignore'});assert.notEqual(app.pid,original.pid);
  for(let n=0;n<160;n++){await wait(250);assert.equal(app.exitCode,null);try{const ep=JSON.parse(fs.readFileSync(path.join(home,'.homebase/control/endpoint.json')));if(ep.pid!==app.pid)continue;assert(/^127\.0\.0\.1:\d+$/.test(ep.endpoint));endpoint=ep;await rpc('jobs.list');assert.equal((await call('assistant_config_get')).settings.enabled,false);assert.equal((await call('wechat_status')).bound,false);return;}catch{}}
  throw Error('Owned history app startup timeout');
}
async function stop(){if(app?.exitCode===null){const owned=app;assert.notEqual(owned.pid,original.pid);spawnSync('taskkill',['/PID',String(owned.pid),'/T','/F'],{windowsHide:true,stdio:'ignore'});for(let n=0;n<100&&owned.exitCode===null;n++)await wait(50);assert.notEqual(owned.exitCode,null);}app=undefined;productionAlive();}
try{
  await start(previous);const old=await call('assistant_history_query',{request:query('turns')});
  assert.equal((await read(old.snapshot_id,old.items[0].id,false)).value.progress.goal,'');await stop();
  fs.renameSync(context,contextHeld);await start(previous);
  await call('assistant_history_query',{request:query()},'ARCHIVE_MISSING');assert.equal(hash(head),initial.head);await stop();
  console.log(JSON.stringify({phase:'old-context-dependency-and-empty-goal',passed:true}));
  await start(binary);let page=cli('assistant_history_query',query());const snapshot=page.snapshot_id;
  fs.renameSync(head,headHeld);
  await call('assistant_history_query',{request:query()},'ARCHIVE_INCOMPLETE');
  await call('assistant_history_query',{request:{...query(),snapshot_id:crypto.randomUUID()}},'HISTORY_SNAPSHOT_EXPIRED');
  await call('assistant_history_query',{request:{...query(),snapshot_id:snapshot,query:'changed'}},'HISTORY_SCOPE_MISMATCH');
  await call('assistant_history_query',{request:{...query(),through_turn_id:'foreign-cutoff'}},'HISTORY_TURN_NOT_FOUND');
  let messages=[],pages=0;
  while(true){
    assert.equal(page.total,180);assert(page.items.length>0);assert.equal(page.offset,messages.length);pages++;
    for(const item of page.items){const row=await read(snapshot,item.id);assert.equal(row.sha256,item.json_sha256);assert.deepEqual(row.value,expected.messages[messages.length]);messages.push(row.value);if(messages.length%60===0)console.log(JSON.stringify({phase:'actual-cli-frozen-bodies',messages:messages.length}));}
    if(page.next_offset===null||page.next_offset===undefined){assert(page.complete);break;}
    assert.equal(page.next_offset,messages.length);page=cli('assistant_history_query',{...query(),snapshot_id:snapshot,offset:page.next_offset});assert(pages<200);
  }
  assert.deepEqual(messages,expected.messages);fs.renameSync(headHeld,head);assert.equal(hash(head),initial.head);
  const turns=cli('assistant_history_query',query('turns'));
  const turn=await read(turns.snapshot_id,turns.items[0].id);assert.equal(turn.value.progress.goal,expected.goal);
  assert.equal(turn.value.id,expected.turn_id);await call('assistant_session_get',{sessionId:expected.session_id},'ARCHIVE_MISSING');
  const none=cli('assistant_history_query',{...query(),query:'not-present-in-this-synthetic-corpus'});assert(none.complete&&none.total===0&&none.items.length===0);
  assert.equal(hash(head),initial.head);await stop();fs.renameSync(contextHeld,context);assert.equal(hash(context),initial.context);
  await start(binary);await call('assistant_history_query',{request:{...query(),snapshot_id:snapshot}},'HISTORY_SNAPSHOT_EXPIRED');
  const settings=(await call('assistant_config_get')).settings;settings.max_tokens=3000;await call('assistant_config_save',{settings});await stop();
  await start(binary);assert.equal((await call('assistant_config_get')).settings.max_tokens,3000);
  const restored=cli('assistant_history_query',query());assert.equal(restored.total,180);
  assert.equal((await read(restored.snapshot_id,restored.items[0].id)).value.id,expected.messages[0].id);
  assert.deepEqual((await call('assistant_session_get',{sessionId:expected.session_id})).messages,expected.messages);
  assert.equal((await call('assistant_send',{request:expected.request})).turn_id,expected.turn_id);
  assert.equal(hash(legacy),initial.legacy);for(const[name,digest]of Object.entries(segments))assert.equal(hash(path.join(path.dirname(context),name)),digest);
  const report={passed:true,completed_at:new Date().toISOString(),home,binary,binary_sha256:hash(binary),previous,previous_sha256:hash(previous),old_defects_reproduced:true,actual_cli_calls:cliCalls,message_bodies:messages.length,message_pages:pages,all_json_hashes_verified:true,snapshot_survives_missing_head_and_context:true,new_query_missing_head_explicit:true,unknown_cutoff_before_source_read:true,conditions_and_expiry_specific:true,turn_original_goal:true,read_only_head_unchanged:true,settings_save_restart_and_original_receipt:true,legacy_and_all_segments_unchanged:true,real_model_calls:0,real_wechat_messages:0,production_pid:original.pid,production_deployed:false};
  fs.writeFileSync(path.join(lab,`cold-history-native-v${run}.json`),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
}finally{await stop();if(fs.existsSync(headHeld)&&!fs.existsSync(head))fs.renameSync(headHeld,head);if(fs.existsSync(contextHeld)&&!fs.existsSync(context))fs.renameSync(contextHeld,context);}
