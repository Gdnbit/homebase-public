// Full native/CLI pipeline under an explicit isolated home. Never enables a model or WeChat.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {spawn,spawnSync} from 'node:child_process';
const parent=process.env.HOMEBASE_REPO_LAB,binary=process.env.HOMEBASE_REPO_NATIVE_BIN;
assert(parent&&binary&&path.isAbsolute(parent)&&path.isAbsolute(binary));
const lab=fs.mkdtempSync(path.join(parent,'native-pipeline-'));const hash=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
const originalSql=path.join(process.env.APPDATA,'io.github.gdnbit.homebase/homebase.db');const sqlHash=fs.existsSync(originalSql)?hash(originalSql):null;
const env={...process.env,HOMEBASE_TEST_HOME:lab,HOMEBASE_SECRETS_FILE:path.join(lab,'secrets.yaml'),HOMEBASE_SECRETS_META:path.join(lab,'secrets-meta.json'),HOMEBASE_AIHUB_ROOT:path.join(lab,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(lab,'models'),HOMEBASE_RUNTIME_ROOT:path.join(lab,'runtime'),HOMEBASE_MODEL_CACHE:path.join(lab,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(lab,'output')};delete env.HOMEBASE_CONTROL_ENDPOINT;
const sleep=ms=>new Promise(r=>setTimeout(r,ms));let next=0;const cli=(args,input)=>{const p=spawnSync(process.execPath,['scripts/homebase.mjs',...args],{env,windowsHide:true,encoding:'utf8',timeout:20000,input:input===undefined?undefined:JSON.stringify(input)});let v;try{v=JSON.parse(p.stdout);}catch{throw Error(`Invalid CLI response exit=${p.status}`);}return v;};
async function call(command,input={},requestId=`native-repo-${++next}`){let v=cli(['call',command,'--input','-','--allow-private','--allow-write','--allow-external','--request-id',requestId,'--wait-ms','1000'],input);assert(v.job?.id,JSON.stringify(v.error));for(let i=0;i<240&&['running','queued'].includes(v.job.status);i++){await sleep(500);v=cli(['jobs','get',v.job.id,'--allow-private','--allow-write','--allow-external']);}assert.equal(v.job.status,'succeeded',JSON.stringify(v.job.error));return v.job.result;}
const start=()=>spawn(binary,[],{env,windowsHide:true,stdio:'ignore'});let app=start();const endpoint=path.join(lab,'.homebase/control/endpoint.json');
async function ready(){for(let i=0;i<150;i++){if(app.exitCode!==null)throw Error('Isolated native app exited');try{if(JSON.parse(fs.readFileSync(endpoint)).pid===app.pid&&cli(['doctor']).ok)return;}catch{}await sleep(200);}throw Error('Native readiness timeout');}
try{
 await ready();assert.equal((await call('assistant_config_get')).settings.enabled,false);assert.equal((await call('wechat_status')).bound,false);
 assert.equal((await call('repo_capabilities_list')).total,0);
 const a=await call('repo_project_analyze',{request:{repository_url:'https://github.com/markedjs/marked',request_id:'analyze-once'}});assert.equal(a.project.phase,'analyzed',JSON.stringify(a));console.log('native analyzed',a.project.source.package.version);
 let p=await call('repo_project_get',{projectId:a.project.id});assert.equal(p.project.revision,a.project.revision);
 let installed=await call('repo_project_install',{request:{project_id:a.project.id,revision:p.project.revision,request_id:'install-once'}});
 // A failed public download is explicit; one manually requested retry uses a fresh revision.
 if(installed.project.phase==='failed'&&installed.project.error?.code==='INSTALL_FAILED'){p=await call('repo_project_get',{projectId:a.project.id});installed=await call('repo_project_install',{request:{project_id:a.project.id,revision:p.project.revision,request_id:'install-network-retry'}});}
 assert.equal(installed.project.phase,'registered',JSON.stringify(installed.project.error));
 const capability_id=installed.project.capability_id;const described=await call('repo_capability_describe',{capabilityId:capability_id});assert.equal(described.verification.passed,true);
 const input={request:{capability_id,input:{stdin:'# Native CLI\n\n**Isolated**'},request_id:'convert-once'}};const result=await call('repo_capability_run',input,'native-convert-once');assert.equal(result.status,'succeeded',JSON.stringify(result));assert.match(result.stdout,/<h1>Native CLI<\/h1>/);assert.match(result.stdout,/<strong>Isolated<\/strong>/);
 const replay=await call('repo_capability_run',input,'native-convert-once');assert.equal(replay.run_id,result.run_id);
 const businessReplay=await call('repo_capability_run',input);assert.equal(businessReplay.run_id,result.run_id);
 const invalid=await call('repo_capability_run',{request:{capability_id,input:{stdin:42},request_id:'invalid-input'}});assert.equal(invalid.status,'failed');assert.equal(invalid.error.code,'CAPABILITY_INPUT_INVALID');
 const artifactHash=hash(result.artifact.path);assert.equal(artifactHash,result.artifact.sha256);
 app.kill();await new Promise(r=>app.once('exit',r));app=start();await ready();
 assert.equal((await call('repo_capabilities_list')).total,1);assert.equal((await call('repo_project_get',{projectId:a.project.id})).project.phase,'registered');
 const restored=await call('repo_capability_result',{runId:result.run_id});assert.equal(restored.artifact.sha256,artifactHash);assert.equal(restored.stdout,result.stdout);
 assert.equal((await call('repo_capability_run',input)).run_id,result.run_id);
 assert.equal(fs.existsSync(originalSql)?hash(originalSql):null,sqlHash);
 const pipeline=path.join(path.dirname(binary),'distribution/capability-pipeline');
 const report={passed:true,lab,binary,binary_sha256:hash(binary),pipeline_hashes:Object.fromEntries(fs.readdirSync(pipeline).map(n=>[n,hash(path.join(pipeline,n))])),capability_id,project_id:a.project.id,run_id:result.run_id,artifact:result.artifact,doctor_ok:true,isolated_home:true,production_sql_unchanged:true,real_model_calls:0,real_wechat_messages:0,restart_preserved:true,replay_execution_count:1,invalid_input_rejected:true};
 fs.writeFileSync(path.join(lab,'validation.json'),JSON.stringify(report,null,2));console.log(JSON.stringify({passed:true,report:path.join(lab,'validation.json'),capability_id}));
}finally{app.kill();}
