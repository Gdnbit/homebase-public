// Real native IPC in a newly created synthetic home; never a production restart.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import net from 'node:net';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';

const lab=process.env.HOMEBASE_HANDOFF_LAB,binary=process.env.HOMEBASE_HANDOFF_BINARY;
assert(lab&&binary&&path.isAbsolute(lab)&&path.isAbsolute(binary));
assert(path.resolve(lab).replaceAll('\\','/').startsWith('F:/HomeBase-Labs/'));
assert(path.resolve(binary).startsWith(path.resolve(lab)+path.sep));
const home=fs.mkdtempSync(path.join(lab,'workflow-native-'));
const cliRoot=path.resolve(process.env.HOMEBASE_HANDOFF_CLI_ROOT||process.cwd());
const cliNode=path.resolve(process.env.HOMEBASE_HANDOFF_CLI_NODE||process.execPath);
if(process.env.HOMEBASE_HANDOFF_CLI_ROOT){
  assert.equal(cliRoot,path.dirname(path.resolve(binary)),'Packaged CLI must belong to the tested binary');
  assert(cliNode.startsWith(cliRoot+path.sep),'Packaged Node must belong to the same candidate');
}
const inheritedEnv={...process.env};
for(const name of Object.keys(inheritedEnv))if(/^(?:HOMEBASE_|SOPS_)|(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN)/.test(name))delete inheritedEnv[name];
const env={...inheritedEnv,HOMEBASE_TEST_HOME:home,HOME:home,USERPROFILE:home,
  APPDATA:path.join(home,'AppData/Roaming'),LOCALAPPDATA:path.join(home,'AppData/Local'),
  TEMP:path.join(home,'tmp'),TMP:path.join(home,'tmp'),
  HOMEBASE_SECRETS_FILE:path.join(home,'synthetic-secrets.yaml'),HOMEBASE_SECRETS_META:path.join(home,'synthetic-secrets-meta.json'),
  HOMEBASE_AIHUB_ROOT:path.join(home,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(home,'models'),
  HOMEBASE_RUNTIME_ROOT:path.join(home,'runtime'),HOMEBASE_MODEL_CACHE:path.join(home,'cache'),
  HOMEBASE_ASR_OUTPUT_ROOT:path.join(home,'output'),WEBVIEW2_USER_DATA_FOLDER:path.join(home,'webview')};
for(const name of Object.keys(env))if(/(?:API_KEY|ACCESS_TOKEN|AUTH_TOKEN|HOMEBASE_CONTROL_ENDPOINT|HOMEBASE_ASSISTANT_(?:SEGMENTS|CONTEXT_SEGMENTS|TASK_ARCHIVE))/.test(name))delete env[name];
for(const name of ['APPDATA','LOCALAPPDATA','TEMP'])fs.mkdirSync(env[name],{recursive:true});
// Packaged bootstrap must never import the user's real installed-models manifest.
fs.mkdirSync(path.join(home,'AIHub/models'),{recursive:true});
fs.writeFileSync(path.join(home,'AIHub/models/installed-models.json'),JSON.stringify({schema_version:1,models:{}}),{flag:'wx'});
if(process.env.HOMEBASE_HANDOFF_CLI_ROOT)env.HOMEBASE_CONTROL_PROJECT_ROOT=cliRoot;
const hash=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
const normalizedPath=p=>path.toNamespacedPath(path.resolve(p)).toLowerCase();
const wait=ms=>new Promise(r=>setTimeout(r,ms));
let app,lifecycle,endpoint,calls=0,cliDoctors=0,cliChecks=0,stopUnconfirmed=false;
async function cli(args,{json=true,expectedCode=0,input}={}){
  return new Promise((resolve,reject)=>{
    const child=spawn(cliNode,[path.join(cliRoot,'scripts/homebase.mjs'),...args],{cwd:cliRoot,env:{...env,HOMEBASE_CONTROL_ENDPOINT:path.join(home,'.homebase/control/endpoint.json')},windowsHide:true,timeout:10000,stdio:['pipe','pipe','pipe']});
    let output='',errors='';
    child.stdout.setEncoding('utf8');child.stdout.on('data',chunk=>{output+=chunk;});
    child.stderr.setEncoding('utf8');child.stderr.on('data',chunk=>{errors+=chunk;});
    child.on('error',reject);child.on('close',code=>{
      try{assert.equal(code,expectedCode,`CLI ${args[0]} failed: ${errors}`);if(endpoint)assert(!output.includes(endpoint.token)&&!errors.includes(endpoint.token),'CLI must not print authentication');cliChecks++;resolve(json?JSON.parse(output):output);}catch(error){reject(error);}
    });
    child.stdin.end(input===undefined?'':JSON.stringify(input));
  });
}
async function doctor(){
  const result=await cli(['doctor']);assert.equal(result.service,'homebase-control');assert.equal(result.state_healthy,true);
  assert.equal(path.resolve(result.project),cliRoot);cliDoctors++;
}
async function rpc(method,fields={}){
  return new Promise((resolve,reject)=>{
    const socket=net.createConnection({host:'127.0.0.1',port:Number(endpoint.endpoint.split(':')[1])});let text='';
    socket.setEncoding('utf8');socket.setTimeout(10000,()=>{socket.destroy();reject(Error('Synthetic handoff RPC timeout'));});socket.on('error',reject);
    socket.on('connect',()=>socket.write(JSON.stringify({protocol:1,id:crypto.randomUUID(),token:endpoint.token,method,...fields})+'\n'));
    socket.on('data',chunk=>{text+=chunk;if(text.includes('\n')){socket.destroy();try{const value=JSON.parse(text);assert(value.ok,value.error?.code);resolve(value.result);}catch(e){reject(e);}}});
  });
}
async function call(command,args={},failure){
  calls++;const grants=command==='assistant_desktop_send'?['private','write','external']:['private','write'];
  const accepted=await rpc('call',{catalog_hash:endpoint.catalog_hash,command,args,grants,request_id:'synthetic-workflow-'+crypto.randomUUID()});
  for(let n=0;n<200;n++){
    const job=await rpc('jobs.get',{job_id:accepted.job.id,grants});
    if(!['queued','running'].includes(job.status)){
      if(failure){assert.equal(job.status,'failed');assert.equal(job.error.code,failure);return job;}
      assert.equal(job.status,'succeeded',job.error?.code);return job.result;
    }
    await wait(50);
  }
  throw Error('Synthetic handoff job exceeded the observation budget; do not resubmit');
}
async function start(){
  assert(!stopUnconfirmed,'Previous owned stop was not confirmed; do not relaunch');
  app=spawn(binary,[],{env,cwd:cliRoot,windowsHide:true,stdio:'ignore'});
  lifecycle={closed:false,code:null,signal:null};
  const current=lifecycle;
  app.once('close',(code,signal)=>Object.assign(current,{closed:true,code,signal}));
  await new Promise((resolve,reject)=>{app.once('spawn',resolve);app.once('error',reject);});
  console.log(JSON.stringify({phase:'owned-launch',pid:app.pid,home,binary}));
  for(let n=0;n<120;n++){
    await wait(250);assert(!lifecycle.closed,'Owned synthetic app exited');
    try{const value=JSON.parse(fs.readFileSync(path.join(home,'.homebase/control/endpoint.json')));if(value.pid!==app.pid)continue;endpoint=value;await rpc('jobs.list');}catch{continue;}
    await doctor();return;
  }
  throw Error('Owned synthetic app startup unconfirmed; no production process was touched');
}
async function stop(){
  if(app&&!lifecycle.closed){
    stopUnconfirmed=true;
    app.kill();
    for(let n=0;n<100&&!lifecycle.closed;n++)await wait(50);
    assert(lifecycle.closed,'Owned synthetic app stop unconfirmed; do not relaunch');
    stopUnconfirmed=false;
  }
  if(app)console.log(JSON.stringify({phase:'owned-stop-confirmed',code:lifecycle.code,signal:lifecycle.signal}));
  app=undefined;lifecycle=undefined;
}
try{
  const prompt=await cli(['bootstrap'],{json:false});assert(prompt.includes(cliRoot));assert(!prompt.includes('{{PROJECT_ROOT}}'));
  const described=await cli(['describe','assistant_desktop_get']);assert.equal(described.command.name,'assistant_desktop_get');
  const validated=await cli(['validate','assistant_desktop_get','--input','-'],{input:{scope:'new'}});assert.equal(validated.ok,true);
  await start();const settings=(await call('assistant_config_get')).settings;
  assert.equal(settings.enabled,false);assert.equal((await call('wechat_status')).bound,false);
  const control=await cli(['call','assistant_config_get','--allow-private','--request-id','synthetic-packaged-config']);
  assert.equal(control.job.status,'succeeded');assert.equal(control.result.settings.enabled,false);
  const queried=await cli(['jobs','get',control.job.id,'--allow-private']);assert.deepEqual(queried.result,control.result);
  if(process.env.HOMEBASE_HANDOFF_CLI_ROOT){
    const bootstrap=await call('ai_control_bootstrap');assert.equal(bootstrap.files.ready,true);assert.equal(path.resolve(bootstrap.files.project_root),cliRoot);
    const distribution=await call('distribution_info');assert.equal(distribution.mode,'packaged');assert.equal(distribution.resourcesReady,true);
    assert.equal(normalizedPath(distribution.resourceRoot),normalizedPath(path.join(cliRoot,'distribution')));
    assert.equal(normalizedPath(distribution.secretsPath),normalizedPath(env.HOMEBASE_SECRETS_FILE));
    for(const root of Object.values(distribution.roots))assert(normalizedPath(root).startsWith(normalizedPath(home)+path.sep),'Runtime data root must stay in the synthetic home');
  }
  const id=crypto.randomUUID(),scope=`handoff:${id}`,text='HB-HANDOFF-NATIVE 仅合成资料🙂'.repeat(700),body={text,route:'',mode:'auto',files:[]};
  assert(Buffer.byteLength(text)>8000&&Buffer.byteLength(text)<65536);
  assert.equal((await call('assistant_desktop_get',{scope})).entry.revision,0);
  const saved=await call('assistant_desktop_save',{request:{scope,revision:0,body}});assert.equal(saved.entry.body.text,text);
  assert.deepEqual(await call('assistant_sessions_list'),[]);
  const draftPath=path.join(home,'.homebase/assistant/desktop',crypto.createHash('sha256').update(scope).digest('hex')+'.draft.dpapi');
  const cipherHash=hash(draftPath);assert(!fs.readFileSync(draftPath).includes(Buffer.from('HB-HANDOFF-NATIVE')));
  assert((await call('assistant_desktop_index')).index.entries.some(e=>e.scope===scope&&e.has_draft));
  const invalid=await call('assistant_desktop_save',{request:{scope,revision:saved.entry.revision,body:{...body,mode:'task'}}});assert.equal(invalid.status,'failed');assert.equal(hash(draftPath),cipherHash);
  const blocked=await call('assistant_desktop_resolve',{scope});assert.equal(blocked.code,'SCOPE_INVALID');assert.equal(hash(draftPath),cipherHash);
  await call('assistant_session_create',{requestId:'bad-id'},'SESSION_CREATE_BAD_ARGUMENT');
  await stop();await start();assert.equal((await call('assistant_desktop_get',{scope})).entry.body.text,text);assert.equal(hash(draftPath),cipherHash);
  const creations=await Promise.all([call('assistant_session_create',{requestId:id}),call('assistant_session_create',{requestId:id}),call('assistant_session_create',{requestId:id})]);assert(creations.every(s=>s.id===id&&s.source==='desktop'));
  const statePath=path.join(home,'.homebase/assistant/state.dpapi'),stateHash=hash(statePath);
  assert.equal((await call('assistant_session_create',{requestId:id})).id,id);assert.equal(hash(statePath),stateHash,'Duplicate creation must not republish state');
  assert.equal((await call('assistant_sessions_list')).length,1);
  await call('assistant_desktop_save',{request:{scope:id,revision:0,body}});assert.equal((await call('assistant_desktop_get',{scope:id})).entry.body.text,text);
  const clear=await call('assistant_desktop_save',{request:{scope,revision:saved.entry.revision,body:{...body,text:''}}});assert.equal(clear.entry.body.text,'');
  const completedPath=path.join(path.dirname(draftPath),'completed-handoffs',path.basename(draftPath));
  assert(!fs.existsSync(draftPath)&&fs.existsSync(completedPath),'Completed handoff must leave the active quota without deleting its revision');
  assert.equal((await call('assistant_desktop_get',{scope})).entry.revision,clear.entry.revision);
  assert(!(await call('assistant_desktop_index')).index.entries.some(e=>e.scope===scope));
  const stale=await call('assistant_desktop_save',{request:{scope,revision:saved.entry.revision,body}});assert.equal(stale.status,'failed');assert.equal((await call('assistant_desktop_get',{scope})).entry.body.text,'');
  await stop();await start();assert.equal((await call('assistant_desktop_get',{scope:id})).entry.body.text,text);
  assert.equal((await call('assistant_session_create',{requestId:id})).id,id);assert.equal((await call('assistant_sessions_list')).length,1);
  const target=(await call('assistant_desktop_get',{scope:id})).entry,request={session_id:id,text:'合成关闭模型回执验证',request_id:'synthetic-original-disabled-request',mode:'chat'};
  const first=await call('assistant_desktop_send',{request,revision:target.revision});assert.equal(first.status,'accepted');assert.equal(first.ack.state,'disabled');
  await stop();await start();const resolved=await call('assistant_desktop_resolve',{scope:id,request});assert.equal(resolved.ack.turn_id,first.ack.turn_id);
  const original=await call('assistant_desktop_send',{request,revision:0});assert.equal(original.ack.turn_id,first.ack.turn_id);assert.equal((await call('assistant_sessions_list')).length,1);
  assert.deepEqual((await call('assistant_config_get')).settings,settings);assert.equal((await call('wechat_status')).bound,false);
  await stop();
  const result={passed:true,binary,binary_sha256:hash(binary),home,cli_root:cliRoot,cli_node:cliNode,native_calls:calls,cli_doctors:cliDoctors,cli_checks:cliChecks,protected_text_before_creation:true,original_uuid_restart:true,concurrent_create_once:true,duplicate_create_no_state_write:true,original_disabled_ack_reused:true,settings_preserved:true,real_model_calls:0,real_wechat_test_messages:0};
  fs.writeFileSync(path.join(home,'workflow-native-validation.json'),JSON.stringify(result,null,2),{flag:'wx'});console.log(JSON.stringify(result));
}catch(error){
  console.error('Primary synthetic verification failure:',String(error));
  throw error;
}finally{
  if(stopUnconfirmed){console.error('Owned stop remains unconfirmed; no repeated stop or launch.',app?.pid);process.exitCode=1;}
  else try{await stop();}catch(error){console.error('Owned cleanup unconfirmed:',String(error));process.exitCode=1;}
}
