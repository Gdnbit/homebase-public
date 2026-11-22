// Real official CLIs, synthetic loopback models, fresh profiles only.
import assert from 'node:assert/strict';
import fs from 'node:fs';import path from 'node:path';import os from 'node:os';import crypto from 'node:crypto';
import http from 'node:http';import readline from 'node:readline';import {spawn,spawnSync} from 'node:child_process';
import {protectedNativeTest,protectedSend,replayPreAckClient,verifyPreAckRecovery,seedUnknownNative,verifyUnknownNative,sealNativeFixtures} from './native-receipt-isolation.mjs';
const lab=process.env.HOMEBASE_NATIVE_LAB||path.join(os.tmpdir(),'homebase-native-providers');
const emptyResult=process.argv.includes('--empty-result');
const missingAuth=process.argv.includes('--missing-auth');
const persistenceFault=process.argv.includes('--persistence-fault');
fs.mkdirSync(lab,{recursive:true});const profile=fs.mkdtempSync(path.join(lab,'profile-'));
const calls=[];let toolCalls=0;
const server=http.createServer(async(req,res)=>{
  fs.appendFileSync(path.join(profile,'fixture-http.jsonl'),JSON.stringify({url:req.url})+'\n');
  let raw='';for await(const c of req)raw+=c;
  const body=raw?JSON.parse(raw):{};
  if(req.url.includes('countTokens')){res.writeHead(200,{'content-type':'application/json'}).end(JSON.stringify({totalTokens:100}));return;}
  const claude=req.url.includes('/messages');
  if(!claude&&!req.url.toLowerCase().includes('generatecontent')){res.writeHead(200,{'content-type':'application/json'}).end('{}');return;}
  if(!claude&&!req.url.includes('streamGenerateContent')){res.writeHead(200,{'content-type':'application/json'}).end(JSON.stringify({candidates:[{content:{role:'model',parts:[{text:'{"reasoning":"fixture","model":"gemini-2.5-pro","complexity":1}'}]},finishReason:'STOP'}]}));return;}
  const tools=claude?body.tools??[]:(body.tools??[]).flatMap(t=>t.functionDeclarations??[]);
  fs.writeFileSync(path.join(profile,'fixture-tools.json'),JSON.stringify(body.tools??[],null,2));
  const tool=tools.find(t=>t.name.includes('homebase'));
  calls.push({provider:claude?'claude-code':'gemini-cli',url:req.url,tools:tools.map(t=>t.name),history:JSON.stringify((claude?body.messages:body.contents)??[])});
  const history=calls.at(-1).history;
  const outputs=claude?(body.messages??[]).flatMap(m=>(Array.isArray(m.content)?m.content:[]).filter(c=>c.type==='tool_result')):(body.contents??[]).flatMap(m=>(m.parts??[]).filter(p=>p.functionResponse));
  calls.at(-1).has_context_tool_output=JSON.stringify(outputs).includes('homebase-context-v1');
  fs.writeFileSync(path.join(profile,'fixture-requests.json'),JSON.stringify(calls,null,2));
  if(history.includes('CANCEL_NATIVE_TURN'))return;
  const first=!emptyResult&&outputs.length===0&&!history.includes('SECOND_NATIVE_TURN');
  if(persistenceFault&&outputs.length&&calls.at(-1).has_context_tool_output)fs.chmodSync(path.join(profile,'.homebase/conversations/providers.dpapi'),0o444);
  res.writeHead(200,{'content-type':'text/event-stream','cache-control':'no-cache'});
  if(claude){
    const id='msg_'+crypto.randomUUID().replaceAll('-','');
    const event=(type,fields)=>res.write(`event: ${type}\ndata: ${JSON.stringify({type,...fields})}\n\n`);
    event('message_start',{message:{id,type:'message',role:'assistant',model:'homebase-fixture-model',content:[],stop_reason:null,usage:{input_tokens:100,output_tokens:0}}});
    if(first&&tool){event('content_block_start',{index:0,content_block:{type:'tool_use',id:'tool_fixture',name:tool.name,input:{}}});event('content_block_delta',{index:0,delta:{type:'input_json_delta',partial_json:'{"action":"context"}'}});}
    else{event('content_block_start',{index:0,content_block:{type:'text',text:''}});event('content_block_delta',{index:0,delta:{type:'text_delta',text:emptyResult?'\n ':'原生隔离验证完成。'}});}
    event('content_block_stop',{index:0});event('message_delta',{delta:{stop_reason:first&&tool?'tool_use':'end_turn',stop_sequence:null},usage:{output_tokens:12}});event('message_stop',{});
  }else{
    const parts=first&&tool?[{functionCall:{name:tool.name,args:{action:'context'}}}]:[{text:emptyResult?'\n ':'原生隔离验证完成。'}];
    res.write(`data: ${JSON.stringify({candidates:[{index:0,content:{role:'model',parts},finishReason:'STOP'}],usageMetadata:{promptTokenCount:100,candidatesTokenCount:12,totalTokenCount:112},modelVersion:'homebase-fixture-model'})}\n\n`);
  }
  res.end();
});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const base=`http://127.0.0.1:${server.address().port}`;
const env=Object.fromEntries(['SystemRoot','WINDIR','COMSPEC','PATH','PATHEXT'].filter(k=>process.env[k]).map(k=>[k,process.env[k]]));
Object.assign(env,{HOME:profile,USERPROFILE:profile,APPDATA:path.join(profile,'AppData/Roaming'),LOCALAPPDATA:path.join(profile,'AppData/Local'),TEMP:profile,TMP:profile,CLAUDE_CONFIG_DIR:path.join(profile,'.claude'),GEMINI_CLI_HOME:path.join(profile,'gemini'),ANTHROPIC_API_KEY:'homebase-fixture-key',ANTHROPIC_BASE_URL:base,ANTHROPIC_MODEL:'homebase-fixture-model',GEMINI_API_KEY:'homebase-fixture-key',GOOGLE_GEMINI_BASE_URL:base,NO_COLOR:'1'});
const binary=process.env.HOMEBASE_CLAUDE_BINARY||'C:/Users/homebase/.local/bin/claude.exe';
env.HOMEBASE_NATIVE_FIXTURE_DEBUG='1';
if(missingAuth){delete env.GEMINI_API_KEY;delete env.GOOGLE_GEMINI_BASE_URL;}
let app,serial=0;
const appBinary=process.env.HOMEBASE_NATIVE_APP;
if(protectedNativeTest)assert(appBinary&&!emptyResult&&!missingAuth&&!persistenceFault,'Protected native receipt test requires the staged native application and normal transport');
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const appEnv={...env,HOMEBASE_TEST_HOME:profile,HOMEBASE_SECRETS_FILE:path.join(profile,'secrets.yaml'),HOMEBASE_SECRETS_META:path.join(profile,'secrets-meta.json'),HOMEBASE_AIHUB_ROOT:path.join(profile,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(profile,'models'),HOMEBASE_RUNTIME_ROOT:path.join(profile,'runtime'),HOMEBASE_MODEL_CACHE:path.join(profile,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(profile,'output')};
appEnv.WEBVIEW2_USER_DATA_FOLDER=path.join(profile,'webview-profile');
// The GUI host needs Windows' registered profile location; SDK children still
// receive only their isolated profile and synthetic loopback credentials.
appEnv.USERPROFILE=process.env.USERPROFILE??profile;
function cli(args){const r=spawnSync(process.execPath,['scripts/homebase.mjs',...args],{env:appEnv,windowsHide:true,encoding:'utf8',timeout:20000});const v=JSON.parse(r.stdout);assert(v.ok,JSON.stringify(v.error));return v;}
async function call(command,args={}){const file=path.join(profile,`request-${++serial}.json`);fs.writeFileSync(file,JSON.stringify(args));let v=cli(['call',command,'--input',file,'--allow-private','--allow-write','--allow-external','--request-id',`native-fixture-${serial}`,'--wait-ms','500']);for(let i=0;i<150&&['running','queued'].includes(v.job.status);i++){await sleep(100);v=cli(['jobs','get',v.job.id,'--allow-private','--allow-write','--allow-external']);}assert.equal(v.job.status,'succeeded',JSON.stringify(v.job.error));return v.job.result;}
async function startApp(){app=spawn(appBinary,[],{cwd:process.cwd(),env:appEnv,windowsHide:true,stdio:'ignore'});for(let i=0;i<150;i++){const ep=path.join(profile,'.homebase/control/endpoint.json');if(fs.existsSync(ep)&&JSON.parse(fs.readFileSync(ep,'utf8')).pid===app.pid)return;assert.equal(app.exitCode,null);await sleep(200);}throw Error('Native app startup timed out');}
function stopApp(){if(app&&app.exitCode===null)spawnSync('taskkill',['/PID',String(app.pid),'/T','/F'],{windowsHide:true,stdio:'ignore'});}
const logicalIds=new Map();
const protectedMessages=new Map();
async function nativeRun(provider,nativeId,second){
  let logical=logicalIds.get(provider);if(!logical){const r=await call('conversation_native',{request:{action:'create',provider,request_id:`create-${provider}`}});logical=r.thread.id;logicalIds.set(provider,logical);assert.equal((await call('conversation_native',{request:{action:'create',provider,request_id:`create-${provider}`}})).thread.id,logical);}
  const input={request:{action:'send',provider,thread_id:logical,text:second?'SECOND_NATIVE_TURN':'FIRST_NATIVE_SENTINEL',request_id:`send-${provider}-${second}`}};
  const guarded=protectedNativeTest?await protectedSend(call,provider,logical,input.request.text,input.request.request_id):null;
  if(guarded)protectedMessages.set(provider,guarded);
  const ack=guarded?.ack??await call('conversation_native',input);assert.equal(ack.state,'accepted');assert.equal((await call('conversation_native',input)).turn.id,ack.turn.id);
  let status;for(let i=0;i<150;i++){status=await call('conversation_native',{request:{action:'status',provider,thread_id:logical}});if(['completed','failed','unknown','interrupted'].includes(status.state))break;await sleep(100);}
  assert.equal(status.state,persistenceFault?'unknown':'completed',JSON.stringify(status));if(persistenceFault)assert(status.error.message.includes('保存失败'));assert(status.native_id);const own=await call('conversation_native',{request:{action:'read',provider,thread_id:logical}});assert.equal(own.thread.turns.length,second?2:1);assert(!('turns'in status));
  return status.native_id;
}
async function run(provider,nativeId,second=false){
  if(appBinary)return nativeRun(provider,nativeId,second);
  const cwd=path.join(profile,provider);fs.mkdirSync(cwd,{recursive:true});
  const runRoot=path.join(profile,'runs',crypto.randomUUID());
  const p=spawn(process.execPath,['native-agent-runtime/worker.mjs'],{cwd:process.cwd(),env,windowsHide:true,stdio:['pipe','pipe','pipe']});
  const events=[];let stderr='';p.stderr.on('data',c=>{stderr+=c;fs.writeFileSync(path.join(profile,'fixture-stderr.txt'),stderr);});
  const lines=readline.createInterface({input:p.stdout});
  lines.on('line',line=>{const v=JSON.parse(line);events.push(v);if(v.event==='tool_request'){toolCalls++;p.stdin.write(JSON.stringify({type:'tool_result',id:v.id,result:{proof:'isolated-tool-proof',version:1}})+'\n');}});
  p.stdin.write(JSON.stringify({type:'start',protocol:1,provider,session_id:nativeId??crypto.randomUUID(),native_id:nativeId,resume:second,cwd,run_root:runRoot,turn_id:crypto.randomUUID(),prompt:second?'SECOND_NATIVE_TURN':'FIRST_NATIVE_SENTINEL',context:{source:'isolated'},test:true,claude_binary:binary})+'\n');
  await new Promise((resolve,reject)=>{const t=setTimeout(()=>{p.kill();reject(Error(`${provider} timed out`));},120000);p.once('exit',()=>{clearTimeout(t);resolve();});p.once('error',reject);});
  const terminal=events.find(e=>e.event==='terminal');
  // Only synthetic model diagnostics may be written in this fresh fixture directory.
  fs.writeFileSync(path.join(runRoot,'fixture-events.json'),JSON.stringify({events,stderr},null,2));
  assert.equal(terminal?.state,emptyResult||missingAuth?'failed':'completed',`${provider}: ${JSON.stringify(terminal)}; ${runRoot}`);
  if(missingAuth){assert.equal(terminal.error.code,'NATIVE_INITIALIZATION');assert.equal(calls.length,0);return null;}
  const id=events.find(e=>e.event==='native_session')?.id;assert(id);
  assert.equal(events.filter(e=>e.event==='item'&&e.item.type==='userMessage').length,1,'History replay must not duplicate user items');
  return id;
}
try{
  if(appBinary)await startApp();
  for(const provider of (process.argv.includes('--gemini-only')?['gemini-cli']:process.argv.includes('--claude-only')?['claude-code']:['claude-code','gemini-cli'])){
    if(missingAuth){assert.equal(provider,'gemini-cli');assert(!appBinary);await run(provider);console.log(JSON.stringify({provider,missing_auth_explicit_failure:true,model_calls:0}));continue;}
    if(emptyResult){assert(!appBinary,'Empty result test is an isolated transport test');await run(provider);console.log(JSON.stringify({provider,empty_result_explicit_failure:true}));continue;}
    if(persistenceFault){assert(appBinary);await run(provider);console.log(JSON.stringify({provider,persistence_failure_explicit_unknown:true}));fs.chmodSync(path.join(profile,'.homebase/conversations/providers.dpapi'),0o666);continue;}
    const before=toolCalls;const id=await run(provider);if(!appBinary)assert.equal(toolCalls,before+1,`${provider} must actually call the MCP bridge`);
    else assert(calls.some(c=>c.provider===provider&&c.has_context_tool_output),'Real native gateway must return shared context');
    const offset=calls.length;assert.equal(await run(provider,id,true),id,'Native resume must preserve SDK session ID');
    const resumed=calls.slice(offset).filter(c=>c.provider===provider);assert(resumed.some(c=>c.history.includes('FIRST_NATIVE_SENTINEL')&&c.history.includes('SECOND_NATIVE_TURN')&&c.history.includes('原生隔离验证完成')),'Native checkpoint must carry earlier user turn and assistant reply');
    if(provider==='gemini-cli')assert(calls.filter(c=>c.provider===provider).every(c=>c.tools.every(t=>t.includes('homebase'))),'Only HomeBase tools may be exposed');
    console.log(JSON.stringify({provider,native_resume:true,tool_roundtrip:true,isolated_profile:profile}));
  }
  if(appBinary&&!persistenceFault){
    const providers=await call('conversation_providers');for(const provider of ['claude-code','gemini-cli'])assert(providers.providers.find(p=>p.id===provider).interactive);
    const inbox=await call('inbox_submit',{payload:{text:'仅用于隔离收录验证',title:'HB-WORKFLOW-FIXTURE',extra_tags:['test'],source_hint:'assistant-test',attachments:[]}});assert(path.resolve(inbox.path).startsWith(path.resolve(profile,'Wiki')+path.sep));assert(fs.readFileSync(inbox.path,'utf8').includes('仅用于隔离收录验证'));
    const before=calls.length;stopApp();await sleep(700);
    const clientFixtures=protectedNativeTest?[...protectedMessages.values()].map(v=>({fixture:replayPreAckClient(profile,v.request,app.pid,`后来输入的${v.request.provider}合成新稿`),ack:v.ack})):[];
    await startApp();
    for(const v of clientFixtures)await verifyPreAckRecovery(call,profile,v.fixture,v.ack);
    for(const [provider,thread_id]of logicalIds){const state=await call('conversation_native',{request:{action:'status',provider,thread_id}});assert.equal(state.state,'completed');const read=await call('conversation_native',{request:{action:'read',provider,thread_id}});assert.equal(read.thread.turns.length,2);}
    assert.equal(calls.length,before,'Read/restart must not invoke model or replay tools');
    for(const [provider,thread_id]of logicalIds){
      const sent=await call('conversation_native',{request:{action:'send',provider,thread_id,text:'CANCEL_NATIVE_TURN',request_id:`cancel-send-${provider}`}});assert.equal(sent.state,'accepted');
      for(let i=0;i<100&&!calls.some(c=>c.provider===provider&&c.history.includes('CANCEL_NATIVE_TURN'));i++)await sleep(100);
      assert(calls.some(c=>c.provider===provider&&c.history.includes('CANCEL_NATIVE_TURN')));
      const input={request:{action:'interrupt',provider,thread_id,request_id:`cancel-${provider}`}};
      const interrupted=await call('conversation_native',input);assert.equal(interrupted.state,'interrupt_requested');assert.deepEqual(await call('conversation_native',input),interrupted);
      let status;for(let i=0;i<80;i++){status=await call('conversation_native',{request:{action:'status',provider,thread_id}});if(status.state!=='inProgress')break;await sleep(100);}
      assert.equal(status.state,'interrupted',JSON.stringify(status));
    }
    if(protectedNativeTest){
      const beforeUnknown=calls.length;stopApp();await sleep(700);
      const fixtures=sealNativeFixtures(profile,[...logicalIds].map(([provider,thread])=>seedUnknownNative(profile,provider,thread,app.pid)));
      await startApp();for(const fixture of fixtures)await verifyUnknownNative(call,profile,fixture);
      assert.equal(calls.length,beforeUnknown,'Unknown original receipts must not repeat native model calls');
    }
  }
  const report={passed:true,mode:persistenceFault?'persistence-fault':emptyResult?'empty':missingAuth?'missing-auth':'normal',protected_native_send:protectedNativeTest,client_checkpoint_fault:protectedNativeTest?'replayed-client-pre-ack-state':null,client_restart_and_unknown_no_replay:protectedNativeTest,native_app:!!appBinary,binary:appBinary??null,binary_sha256:appBinary?crypto.createHash('sha256').update(fs.readFileSync(appBinary)).digest('hex'):null,native_runtime_manifest_sha256:appBinary?crypto.createHash('sha256').update(fs.readFileSync(path.join(path.dirname(appBinary),'distribution/native-agent-runtime/validation.json'))).digest('hex'):null,tool_calls:toolCalls,model_calls:calls.length,calls,profile,real_model_calls:0,real_wechat_messages:0,completed_at:new Date().toISOString()};
  fs.writeFileSync(path.join(profile,'validation.json'),JSON.stringify(report,null,2));
}finally{stopApp();server.closeAllConnections();server.close();}
