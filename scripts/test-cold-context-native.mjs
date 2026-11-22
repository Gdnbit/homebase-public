// Real app/CLI with a synthetic DPAPI corpus. No live model or WeChat binding.
import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import net from 'node:net';
import assert from 'node:assert/strict';import {spawn,spawnSync} from 'node:child_process';
import {createSyntheticVault} from './local-native-vault-fixture.mjs';
const parent=process.env.HOMEBASE_CONTEXT_NATIVE_LAB,home=process.env.HOMEBASE_COLD_SEED_HOME,binary=process.env.HOMEBASE_CONTEXT_NATIVE_BINARY,previous=process.env.HOMEBASE_CONTEXT_PREVIOUS_BINARY,output=process.env.HOMEBASE_CONTEXT_EXPORT_HOME;
const run=process.env.HOMEBASE_CONTEXT_NATIVE_RUN||'1';assert(/^[0-9]+$/.test(run));
assert(parent&&home&&binary&&previous&&output);
for(const value of [home,binary,previous,output]){assert(path.isAbsolute(value));assert(path.resolve(value).startsWith(path.resolve(parent)+path.sep));}
const expected=JSON.parse(fs.readFileSync(path.join(home,'cold-expected.json')));
assert(expected.synthetic_only&&expected.sessions.length===3&&expected.sessions.every(s=>JSON.stringify(s.history).includes('CTX_NATIVE_ONLY')));
assert(!fs.existsSync(output));
const hash=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex'),pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const directory=selected=>path.join(selected,'.homebase/assistant');
const head=selected=>path.join(directory(selected),'history-v1/head.dpapi');
const originalLegacy=hash(path.join(directory(home),'state.dpapi'));
const envs=new Map();let app,endpoint,env,selected,fixtureSeq=0;
function environment(target){
 if(envs.has(target))return envs.get(target);
 const vault=createSyntheticVault(target);
 const value={...vault.env,HOMEBASE_TEST_HOME:target,HOMEBASE_AIHUB_ROOT:path.join(target,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(target,'models'),HOMEBASE_RUNTIME_ROOT:path.join(target,'runtime'),HOMEBASE_MODEL_CACHE:path.join(target,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(target,'output'),APPDATA:path.join(target,'AppData/Roaming'),LOCALAPPDATA:path.join(target,'AppData/Local'),TEMP:path.join(target,'tmp'),TMP:path.join(target,'tmp'),WEBVIEW2_USER_DATA_FOLDER:path.join(target,'webview')};
 for(const key of Object.keys(value))if(/(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|HOMEBASE_CONTROL_ENDPOINT|HOMEBASE_ASSISTANT_(?:CONTEXT_)?SEGMENTS)/i.test(key))delete value[key];
 for(const dir of [value.APPDATA,value.LOCALAPPDATA,value.TEMP])fs.mkdirSync(dir,{recursive:true});envs.set(target,value);return value;
}
async function rpc(method,fields={}){return new Promise((resolve,reject)=>{
 const socket=net.createConnection({host:'127.0.0.1',port:Number(endpoint.endpoint.split(':')[1])});let text='';socket.setEncoding('utf8');
 socket.setTimeout(15000,()=>{socket.destroy();reject(Error('Owned context test timeout'));});socket.on('error',reject);
 socket.on('connect',()=>socket.write(JSON.stringify({protocol:1,id:crypto.randomUUID(),token:endpoint.token,method,...fields})+'\n'));
 socket.on('data',chunk=>{text+=chunk;if(text.includes('\n')){socket.destroy();try{const value=JSON.parse(text);assert(value.ok,value.error?.code);resolve(value.result);}catch(error){reject(error);}}});
 });}
async function call(command,args={},failure){
 const grants=['private','write','external'];const ack=await rpc('call',{command,args,catalog_hash:endpoint.catalog_hash,grants,request_id:'ctx-native-'+crypto.randomUUID()});
 for(let n=0;n<200;n++){const job=await rpc('jobs.get',{job_id:ack.job.id,grants});if(!['queued','running'].includes(job.status)){
   if(failure){assert.equal(job.status,'failed');assert.equal(job.error?.code,failure);return job.error;}
   assert.equal(job.status,'succeeded',job.error?.code);return job.result;}await pause(50);
 }throw Error('Owned context job timeout');
}
async function start(executable,target,initialize=false){
 selected=target;env=environment(target);const childEnv={...env};if(initialize)childEnv.HOMEBASE_ASSISTANT_CONTEXT_SEGMENTS='1';
 app=spawn(executable,[],{env:childEnv,cwd:process.cwd(),windowsHide:true,stdio:'ignore'});
 for(let n=0;n<160;n++){assert.equal(app.exitCode,null);await pause(250);try{const ep=JSON.parse(fs.readFileSync(path.join(target,'.homebase/control/endpoint.json')));if(ep.pid!==app.pid)continue;endpoint=ep;await rpc('jobs.list');return;}catch{}}
 throw Error('Owned context app startup timeout');
}
async function stop(){if(app?.exitCode===null){const child=app;child.kill();await new Promise(resolve=>child.once('exit',resolve));}app=null;}
function cli(command,args){
 const result=spawnSync(process.execPath,['scripts/homebase.mjs','call',command,'--input','-','--allow-private','--allow-write','--allow-external','--wait-ms','10000'],{env:{...env,HOMEBASE_CONTROL_ENDPOINT:path.join(selected,'.homebase/control/endpoint.json')},input:JSON.stringify(args),encoding:'utf8',windowsHide:true,timeout:20000});
 assert(!result.error);const value=JSON.parse(result.stdout);assert.equal(result.status,0,value.error?.code);assert(value.ok,value.error?.code);return value.result;
}
function fixture(mode,target,destination,format){
 const result=spawnSync('cargo',['test','--locked','--manifest-path','evals/control-plane/Cargo.toml','--lib','export_or_verify_native_context_snapshot','--','--ignored','--test-threads=1'],{env:{...process.env,CARGO_TARGET_DIR:path.join(parent,'document-control-target'),TEMP:path.join(parent,'state-store-temp'),TMP:path.join(parent,'state-store-temp'),HOMEBASE_CONTEXT_FIXTURE_MODE:mode,HOMEBASE_CONTEXT_SOURCE_HOME:target,HOMEBASE_CONTEXT_EXPORT_HOME:destination||'',HOMEBASE_CONTEXT_FIXTURE_FORMAT:String(format)},encoding:'utf8',windowsHide:true,timeout:180000});
 fs.writeFileSync(path.join(parent,`cold-context-fixture-${++fixtureSeq}-${mode}-v${run}.log`),result.stdout+'\n'+result.stderr,{flag:'wx'});assert(!result.error);assert.equal(result.status,0,'Synthetic snapshot fixture failed');
}
async function originals(){for(const session of expected.sessions){const actual=await call('assistant_session_get',{sessionId:session.id});assert.deepEqual(actual.messages,session.messages);assert.deepEqual(actual.turns.map(t=>({id:t.id,state:t.state})),session.turns.map(t=>({id:t.id,state:t.state})));}}
const request={session_id:null,text:'仅合成的闲聊',request_id:'cold-original-0',mode:'chat',attachments:[]};
try{
 await start(binary,home,true);
 assert.equal((await call('assistant_config_get')).settings.enabled,false);assert.equal((await call('wechat_status')).bound,false);
 let config=await call('assistant_config_get');await call('assistant_config_save',{settings:{...config.settings,max_tokens:3072}});await stop();
 await start(binary,home); // No context flag: the codec must auto-detect format 2.
 const committed=hash(head(home));const rows=await call('assistant_sessions_list');assert(rows.length===3&&rows.every(r=>r.message_count===2));
 const usage=await call('assistant_usage_get');assert.equal(usage.prompt_tokens,21);assert.equal(usage.turns,3);
 await originals();assert.equal(cli('assistant_send',{request}).turn_id,expected.sessions[0].turns[0].id);
 const page=cli('assistant_session_window_get',{request:{session_id:expected.sessions[0].id,limit:1}});assert.equal(page.messages.length,1);
 const history=cli('assistant_history_query',{request:{session_id:expected.sessions[0].id,limit:8}});assert.equal(history.total,2);
 assert.equal(hash(head(home)),committed);
 config=await call('assistant_config_get');await call('assistant_config_save',{settings:{...config.settings,max_tokens:3071}});await stop();
 const beforeOld=hash(head(home));
 await start(previous,home);await call('assistant_sessions_list',{},'ARCHIVE_INVALID');await call('assistant_config_get',{},'ARCHIVE_INVALID');await stop();
 assert.equal(hash(head(home)),beforeOld);assert.equal(hash(path.join(directory(home),'state.dpapi')),originalLegacy);
 fixture('export',home,output,2); // Verifies every original context before exporting.
 await start(previous,output);await originals();config=await call('assistant_config_get');assert.equal(config.settings.max_tokens,3071);
 await call('assistant_config_save',{settings:{...config.settings,max_tokens:3070}});await stop();
 fixture('verify',output,null,1);
 await start(binary,output,true);await originals();assert.equal(cli('assistant_send',{request}).turn_id,expected.sessions[0].turns[0].id);
 config=await call('assistant_config_get');assert.equal(config.settings.max_tokens,3070);await call('assistant_config_save',{settings:{...config.settings,max_tokens:3069}});
 assert.equal((await call('assistant_config_get')).settings.enabled,false);assert.equal((await call('wechat_status')).bound,false);await stop();
 fixture('verify',output,null,2);
 assert.equal(hash(head(home)),beforeOld);assert.equal(hash(path.join(directory(home),'state.dpapi')),originalLegacy);
 const report={passed:true,completed_at:new Date().toISOString(),binary,binary_sha256:hash(binary),previous,previous_sha256:hash(previous),isolated_home:home,exported_home:output,explicit_format1_to_format2_upgrade:true,no_flag_format2_restart:true,original_cli_receipt_window_and_history:true,read_only_queries_preserve_head:true,old_reader_rejects_without_overwrite:true,complete_context_and_business_export_to_fresh_v1:true,compatible_old_reader_writes_export:true,new_reader_reupgrades_export_and_preserves_originals:true,source_head_and_legacy_cipher_unchanged_after_export:true,real_model_calls:0,real_wechat_messages:0,model_disabled:true,wechat_unbound:true,production_deployed:false,capacity_limits_unchanged:true};
 fs.writeFileSync(path.join(parent,`cold-context-native-v${run}.json`),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
}finally{await stop();}
