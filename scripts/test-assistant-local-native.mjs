// Real native CLI and SDK, isolated identity/home/database. No provider keys or messages.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import {spawn,spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import readline from 'node:readline';
import net from 'node:net';
import {serverFingerprint} from '../agent-runtime/local/mcp.mjs';
import {createSyntheticVault} from './local-native-vault-fixture.mjs';
const repo=fileURLToPath(new URL('../',import.meta.url));
const parent=process.env.HOMEBASE_ASSISTANT_NATIVE_LAB;
assert(parent&&path.resolve(parent).replaceAll('\\','/').startsWith('F:/HomeBase-Labs/'),'Explicit F: lab required');
const binary=process.env.HOMEBASE_ASSISTANT_NATIVE_BIN;
assert(binary&&path.isAbsolute(binary),'Explicit new native binary required');
const distribution=path.join(path.dirname(binary),'distribution');
fs.mkdirSync(parent,{recursive:true});
const lab=fs.mkdtempSync(path.join(parent,'local-native-'));
const inheritedEnv=Object.fromEntries(Object.entries(process.env).filter(([name]) =>
  /^(path|pathext|systemroot|windir|comspec|systemdrive|programfiles(\(x86\))?|programw6432|programdata|number_of_processors|processor_architecture|os)$/i.test(name)
));
const local=relative=>path.join(lab,relative);
const temp=local('tmp');
Object.assign(inheritedEnv,{APPDATA:local('AppData/Roaming'),LOCALAPPDATA:local('AppData/Local'),TEMP:temp,TMP:temp,TMPDIR:temp,SystemTemp:temp});
for(const dir of [inheritedEnv.APPDATA,inheritedEnv.LOCALAPPDATA,temp,local('Documents'),local('AIHub/models')])fs.mkdirSync(dir,{recursive:true});
fs.writeFileSync(local('AIHub/models/installed-models.json'),JSON.stringify({schema_version:1,models:{}}),{flag:'wx'});
const fixture=createSyntheticVault(lab,{toolDirectory:path.join(distribution,'tools'),inheritedEnv});
const readonly=path.join(lab,'readonly');fs.mkdirSync(readonly);
fs.writeFileSync(path.join(readonly,'sentinel.txt'),'SYNTHETIC SENTINEL');
const skill=path.join(lab,'.codex/skills/synthetic');fs.mkdirSync(skill,{recursive:true});
fs.writeFileSync(path.join(skill,'SKILL.md'),'---\nname: synthetic-native\ndescription: 合成技能，不访问真实数据\n---\n请只在新目录执行脚本。');
fs.writeFileSync(path.join(skill,'proof.py'),'from pathlib import Path\nprint("SYNTHETIC-SKILL")\nPath("result.txt").write_text("隔离技能输出",encoding="utf-8")\n');
const audit=path.join(lab,'credential-digests.jsonl');
const server={id:'homebase-fixture',name:'合成 MCP',description:'隔离协议验证',transport:'stdio',command:process.execPath,args:[path.join(repo,'agent-runtime/test/fixtures/mcp-local.mjs')],env:{FIXTURE_TOKEN:'${FIXTURE_TOKEN}',FIXTURE_AUDIT_FILE:audit,FIXTURE_AUDIT_SOURCE:'sops'}};
const referenceServer={...server,id:'homebase-reference-fixture',env:{...server.env,FIXTURE_AUDIT_SOURCE:'api_key_manager'}};
const httpChild=spawn(process.execPath,[path.join(repo,'agent-runtime/test/fixtures/mcp-local.mjs'),'--http'],{windowsHide:true,stdio:['ignore','pipe','pipe'],env:{SystemRoot:process.env.SystemRoot,PATH:process.env.PATH,TEMP:lab,TMP:lab,FIXTURE_TOKEN:'SYNTHETIC-NATIVE-HTTP-ONLY'}});
httpChild.stderr.on('data',()=>{});const httpTimer=setTimeout(()=>httpChild.kill(),480000);
const httpPort=await new Promise((resolve,reject)=>{readline.createInterface({input:httpChild.stdout}).once('line',line=>resolve(JSON.parse(line).port));httpChild.once('error',reject);httpChild.once('exit',()=>reject(Error('Synthetic HTTP fixture exited before ready')));});
const httpServer={id:'assistant-http-fixture',name:'合成HTTP MCP',description:'原生HTTP隔离验证',url:`http://127.0.0.1:${httpPort}/mcp`,headers:{}};
const httpDefinition={...httpServer,transport:'streamable-http'};
const authServer={...httpServer,id:'assistant-http-auth',url:`http://127.0.0.1:${httpPort}/auth`};
const shared=path.join(lab,'.homebase/shared');fs.mkdirSync(shared,{recursive:true});fs.writeFileSync(path.join(shared,'mcp-pool.json'),JSON.stringify({servers:[server,referenceServer],assignments:{}}));
const state={settings:{revision:0,enabled:false,route_id:'',wechat_enabled:true,max_rounds:8,max_tokens:2048,runtime:'pi',credential_source:'api_key_manager',free_only:true},sessions:[],revision:0,changed:{session_id:null,turn_id:null,config:false,personal:false},local:{revision:0,enabled:true,workspaces:[{id:'readonly',path:readonly,access:'read',auto_write:false}],http_servers:[httpServer,authServer],mcp_grants:[{id:server.id,fingerprint:serverFingerprint(server),tools:['sum','stats'],resources:true,prompts:true},{id:httpServer.id,fingerprint:serverFingerprint(httpDefinition),tools:['sum'],resources:false,prompts:false},{id:authServer.id,fingerprint:serverFingerprint({...authServer,transport:'streamable-http'}),tools:[],resources:false,prompts:false}]}};
state.local.mcp_grants[0].credential_bindings=[{variable:'FIXTURE_TOKEN',reference:'sops:fixture_token'}];
state.local.mcp_grants.push({id:referenceServer.id,fingerprint:serverFingerprint(referenceServer),tools:['sum'],resources:false,prompts:false,credential_bindings:[{variable:'FIXTURE_TOKEN',reference:'api_key_manager:openrouter'}]});
fs.writeFileSync(path.join(lab,'seed-state.json'),JSON.stringify(state));
const seed=spawnSync('pwsh',['-NoProfile','-File','scripts/test-local-native-seed.ps1','-Lab',lab],{cwd:repo,env:fixture.env,encoding:'utf8',windowsHide:true});assert.equal(seed.status,0,seed.stderr);
const env={...fixture.env,HOMEBASE_TEST_HOME:lab,HOMEBASE_AIHUB_ROOT:local('AIHub'),HOMEBASE_MODEL_ROOT:local('models'),HOMEBASE_RUNTIME_ROOT:local('runtime'),HOMEBASE_MODEL_CACHE:local('cache'),HOMEBASE_ASR_OUTPUT_ROOT:local('asr-output'),HOMEBASE_REVEAL_EXE:path.join(distribution,'bin','homebase-reveal.exe')};
const digest=file=>fs.existsSync(file)?crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex'):null;
const start=()=>spawn(binary,[],{cwd:repo,env,stdio:'ignore',windowsHide:true});let app;
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));let seq=0;
function cli(args,allowFailure=false){const child=spawnSync(process.execPath,['scripts/homebase.mjs',...args],{cwd:repo,env,encoding:'utf8',windowsHide:true,timeout:50000});if(child.error)throw child.error;const value=JSON.parse(child.stdout);assert(value.ok||allowFailure,JSON.stringify(value.error));return value;}
function call(command,input={},wait=15000,allowFailure=false){const file=path.join(lab,`request-${++seq}.json`);fs.writeFileSync(file,JSON.stringify(input));const value=cli(['call',command,'--input',file,'--allow-private','--allow-write','--allow-external','--request-id',`isolated-local-${seq}`,'--wait-ms',String(wait)],allowFailure);return value.job;}
function result(command,input={}){
  let job=call(command,input,0);
  assert(job?.id,`Missing accepted job for ${command}`);
  const deadline=Date.now()+75_000;
  const pause=new Int32Array(new SharedArrayBuffer(4));
  while(['queued','running'].includes(job.status)&&Date.now()<deadline){
    Atomics.wait(pause,0,0,200);
    const queried=cli(['jobs','get',job.id,'--allow-private','--allow-write','--allow-external'],true);
    assert(queried.job,JSON.stringify(queried.error));
    job=queried.job;
  }
  assert.equal(job.status,'succeeded',JSON.stringify(job));
  return job.result;
}
function execute(operation,request,operationId=`local-fixture-${seq+1}`){return result('assistant_local_execute',{operation,request,operationId,policyRevision:0});}
async function ready(){const endpoint=path.join(lab,'.homebase/control/endpoint.json');for(let i=0;i<150;i++){if(app.exitCode!==null)throw Error('Native test app exited');try{if(JSON.parse(fs.readFileSync(endpoint,'utf8')).pid===app.pid){cli(['doctor']);return;}}catch{}await sleep(200);}throw Error('Native app did not become ready');}
async function stop(){if(app&&app.exitCode===null){app.kill();await new Promise(resolve=>app.once('exit',resolve));}}
async function privateReadCannotBeCalledExternally(){
  const endpoint=JSON.parse(fs.readFileSync(path.join(lab,'.homebase/control/endpoint.json'),'utf8'));
  const response=await new Promise((resolve,reject)=>{
    const socket=net.createConnection({host:'127.0.0.1',port:Number(endpoint.endpoint.split(':')[1])});let buffer='';
    socket.setTimeout(5000,()=>{socket.destroy();reject(Error('Isolated protocol check timed out'));});socket.on('error',reject);
    socket.on('connect',()=>socket.write(JSON.stringify({protocol:1,id:crypto.randomUUID(),token:endpoint.token,catalog_hash:endpoint.catalog_hash,method:'assistant.local_read',grants:['private','write','external'],args:{operation:'files',request:{action:'write',path:'must-not-exist.txt',content:'SYNTHETIC'},operationId:'private-read-bypass',policyRevision:0}})+'\n'));
    socket.on('data',chunk=>{buffer+=chunk;if(buffer.includes('\n')){socket.destroy();resolve(JSON.parse(buffer));}});
  });
  assert.equal(response.ok,false);assert.equal(response.error.code,'UNKNOWN_METHOD');
}
try{
  app=start();await ready();
  await privateReadCannotBeCalledExternally();
  const config=result('assistant_local_config_get');assert.equal(config.config.revision,0);assert.equal(config.config.workspaces[0].access,'read');assert.equal(config.mcp_servers.find(s=>s.id===server.id).state,'granted');assert.equal(config.terminal.binary_present,true);
  const roots=execute('files',{action:'workspaces'});const scratch=roots.workspaces.find(w=>w.id==='scratch').path;
  assert.equal(fs.existsSync(path.join(scratch,'must-not-exist.txt')),false);
  const write=execute('files',{action:'write',path:'proof.txt',content:'原生隔离文件\r\n第二行'});assert.equal(write.status,'succeeded');
  assert.equal(execute('files',{action:'read',path:'proof.txt'}).newline,'crlf');
  const edit=execute('files',{action:'edit',path:'proof.txt',expected_sha256:write.sha256,old_text:'第二行',new_text:'已精确编辑'});assert.equal(edit.status,'succeeded');
  const denied=call('assistant_local_execute',{operation:'files',request:{action:'read',path:'../outside.txt'},operationId:'traversal-denied',policyRevision:0},5000,true).result;assert.equal(denied.status,'failed');assert.equal(denied.code,'PATH_INVALID');
  const ro=call('assistant_local_execute',{operation:'files',request:{action:'write',workspace_id:'readonly',path:'sentinel.txt',content:'never'},operationId:'ro-denied',policyRevision:0},5000,true);assert.equal(ro.result.status,'failed');assert.match(ro.result.message,/只读|写入/);assert.equal(fs.readFileSync(path.join(readonly,'sentinel.txt'),'utf8'),'SYNTHETIC SENTINEL');
  fs.writeFileSync(path.join(scratch,'.env'),'SYNTHETIC SECRET');
  const terminal=execute('terminal',{command:'python3 -c "from pathlib import Path; print(\'原生终端通过\'); assert not Path(\'/mnt/c\').exists(); Path(\'render.txt\').write_text(\'SYNTHETIC RESULT\')"',timeout_ms:10000});assert.equal(terminal.status,'succeeded');assert.match(terminal.stdout,/原生终端通过/);
  const exported=execute('files',{action:'export',path:'render.txt'});assert.equal(digest(exported.artifact.path),exported.artifact.sha256);
  const description=execute('mcp',{action:'describe',server_id:server.id,tool:'sum'});
  const sum=execute('mcp',{action:'call',server_id:server.id,tool:'sum',schema_hash:description.tool.schema_hash,arguments:{a:8,b:9}});assert.equal(sum.result.structuredContent.sum,17);
  const httpDescription=execute('mcp',{action:'describe',server_id:httpServer.id,tool:'sum'});
  const httpSum=execute('mcp',{action:'call',server_id:httpServer.id,tool:'sum',schema_hash:httpDescription.tool.schema_hash,arguments:{a:5,b:6}});assert.equal(httpSum.result.structuredContent.sum,11);
  const auth=call('assistant_local_execute',{operation:'mcp',request:{action:'tools',server_id:authServer.id},operationId:'auth-required',policyRevision:0},5000,true).result;assert.equal(auth.status,'failed');assert.equal(auth.code,'MCP_AUTH_REQUIRED');
  assert(!JSON.stringify(sum).includes(fixture.values.fixture_token));
  const refDescription=execute('mcp',{action:'describe',server_id:referenceServer.id,tool:'sum'});
  execute('mcp',{action:'call',server_id:referenceServer.id,tool:'sum',schema_hash:refDescription.tool.schema_hash,arguments:{a:1,b:2}});
  const sha=value=>crypto.createHash('sha256').update(value).digest('hex');
  const entries=()=>fs.readFileSync(audit,'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(entries().find(v=>v.source==='sops').sha256,sha(fixture.values.fixture_token));
  assert.equal(entries().find(v=>v.source==='api_key_manager').sha256,sha('SYNTHETIC-API-MANAGER-ONLY'));
  const secretRow=()=>result('secrets_list').find(v=>v.short_name==='fixture_token');
  let row=secretRow();assert(row);
  fixture.rotateToken('SYNTHETIC-SOPS-SECOND-ONLY');
  execute('mcp',{action:'call',server_id:server.id,tool:'sum',schema_hash:description.tool.schema_hash,arguments:{a:2,b:3}});
  assert.equal(entries().filter(v=>v.source==='sops').at(-1).sha256,sha('SYNTHETIC-SOPS-SECOND-ONLY'));
  row=secretRow();result('secrets_save',{request:{short_name:'fixture_token',expected_revision:row.revision,expected_vault_version:row.vault_version,value:null,expires_at:'2020-01-01T00:00:00Z',operation:'save',candidate_verified:false,expiry_policy:'block'}});
  const count=entries().length;
  const expired=call('assistant_local_execute',{operation:'mcp',request:{action:'call',server_id:server.id,tool:'sum',schema_hash:description.tool.schema_hash,arguments:{a:3,b:4}},operationId:'expired-reference-denied',policyRevision:0},15000,true);
  assert.equal(expired.result.status,'failed');assert.match(expired.result.message,/到期/);
  assert.equal(entries().length,count,'Expired credential must not call the MCP or fall back to another source');
  assert(!fs.readFileSync(path.join(lab,'secrets.yaml'),'utf8').includes('SYNTHETIC-'));
  const skills=execute('skill',{action:'list'});const selected=skills.skills.find(s=>s.name==='synthetic-native');assert(selected);
  const plan=execute('skill',{action:'inspect_run',id:selected.id});
  const skillRun=execute('skill',{action:'run',id:selected.id,script:'proof.py',manifest_hash:plan.manifest_hash,args:[]});assert.equal(skillRun.status,'succeeded');assert.match(skillRun.stdout,/SYNTHETIC-SKILL/);assert.equal(fs.existsSync(path.join(skill,'result.txt')),false);
  const skillOutput=execute('files',{action:'read',workspace_id:skillRun.workspace_id,path:'result.txt'});assert.equal(skillOutput.content,'隔离技能输出');
  const skillExport=execute('files',{action:'export',workspace_id:skillRun.workspace_id,path:'result.txt'});assert.equal(digest(skillExport.artifact.path),skillOutput.sha256);
  const cancelId='native-cancel-terminal';const running=call('assistant_local_execute',{operation:'terminal',request:{command:'echo READY; sleep 20; echo BAD > late.txt',timeout_ms:30000},operationId:cancelId,policyRevision:0},0);assert.equal(running.status,'running');
  for(let i=0;i<30;i++){const progress=result('assistant_local_progress',{operationId:cancelId});if(progress.output?.includes('READY'))break;await sleep(100);}
  result('assistant_local_cancel',{operationId:cancelId});
  let canceled;for(let i=0;i<30;i++){const response=cli(['jobs','get',running.id,'--allow-private','--allow-write','--allow-external'],true);assert(response.job,JSON.stringify(response.error));canceled=response.job;if(canceled.status!=='running')break;await sleep(100);}assert.equal(canceled.result.status,'canceled');assert.equal(fs.existsSync(path.join(scratch,'late.txt')),false);
  const recoveryId='native-recovery-terminal';const interrupted=call('assistant_local_execute',{operation:'terminal',request:{command:'echo RESTART_READY; sleep 8; echo BAD > restarted.txt',timeout_ms:20000},operationId:recoveryId,policyRevision:0},0);
  let started=false;for(let i=0;i<30;i++){if(result('assistant_local_progress',{operationId:recoveryId}).output?.includes('RESTART_READY')){started=true;break;}await sleep(100);}assert(started);
  await stop();await sleep(1500);app=start();await ready();
  const recovered=cli(['jobs','get',interrupted.id,'--allow-private','--allow-write','--allow-external'],true).job;assert.equal(recovered.status,'interrupted_unknown');await sleep(8500);assert.equal(fs.existsSync(path.join(scratch,'restarted.txt')),false);
  assert.equal(result('assistant_config_get').settings.enabled,false);
  const report={lab,binary,binary_sha256:digest(binary),real_model_calls:0,real_wechat_messages:0,files:true,terminal:true,mcp_sdk:true,mcp_http:true,http_auth_required_explicit:true,skill:true,skill_output_read_and_export:true,cancel:true,restart_no_reexecution:true,artifact_hash:exported.artifact.sha256};
  Object.assign(report,{synthetic_sops_rotation:true,synthetic_expiry_block:true,explicit_reference_no_fallback:true,credential_values_redacted:true,internal_read_not_publicly_callable:true});
  fs.writeFileSync(path.join(lab,'validation.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));
}finally{await stop();httpChild.kill();clearTimeout(httpTimer);}
