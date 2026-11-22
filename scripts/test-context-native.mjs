// Fresh local profile and disabled model only. Never targets the production endpoint.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import net from 'node:net';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { chromium } from '@playwright/test';
import { reachableDefinitions, validateSchema } from './control-validation.mjs';

const parent=process.env.HOMEBASE_CONTEXT_NATIVE_LAB;
const binary=process.env.HOMEBASE_CONTEXT_NATIVE_BINARY;
const reportName=process.env.HOMEBASE_CONTEXT_NATIVE_REPORT||'context-native-validation-v1.json';
assert(parent&&path.isAbsolute(parent)&&binary&&path.isAbsolute(binary));
assert(path.resolve(binary).startsWith(path.resolve(parent)+path.sep),'Only a staged lab binary is allowed');
assert(/^[a-zA-Z0-9.-]+\.json$/.test(reportName));
assert(!fs.existsSync(path.join(parent,reportName)),'Never replace validation evidence');
const home=fs.mkdtempSync(path.join(parent,'context-app-'));
const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));
const port=server.address().port;await new Promise(r=>server.close(r));
const env={...process.env,HOMEBASE_TEST_HOME:home,HOMEBASE_SECRETS_FILE:path.join(home,'secrets.yaml'),HOMEBASE_SECRETS_META:path.join(home,'secrets-meta.json'),HOMEBASE_AIHUB_ROOT:path.join(home,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(home,'models'),HOMEBASE_RUNTIME_ROOT:path.join(home,'runtime'),HOMEBASE_MODEL_CACHE:path.join(home,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(home,'output'),HOME:home,APPDATA:path.join(home,'AppData/Roaming'),LOCALAPPDATA:path.join(home,'AppData/Local'),TEMP:path.join(home,'tmp'),TMP:path.join(home,'tmp'),WEBVIEW2_USER_DATA_FOLDER:path.join(home,'wv'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`};
for(const dir of [env.APPDATA,env.LOCALAPPDATA,env.TEMP])fs.mkdirSync(dir,{recursive:true});
delete env.HOMEBASE_CONTROL_ENDPOINT;
const pause=ms=>new Promise(r=>setTimeout(r,ms));
const hash=data=>crypto.createHash('sha256').update(data).digest('hex');
let app,browser,page;
function cli(args,input){
  const r=spawnSync(process.execPath,['scripts/homebase.mjs',...args],{env,input:input===undefined?undefined:JSON.stringify(input),encoding:'utf8',windowsHide:true,timeout:60000,maxBuffer:4*1024*1024});
  assert(!r.error,r.error?.message);
  let v;try{v=JSON.parse(r.stdout);}catch{throw Error('Expected structured isolated CLI output');}
  assert.equal(v.ok,true,JSON.stringify(v.error));return v;
}
function call(name,args={}){return cli(['call',name,'--input','-','--allow-write','--allow-private','--allow-external','--request-id',crypto.randomUUID()],args).result;}
async function launch(){
  app=spawn(binary,[],{env,cwd:process.cwd(),stdio:'ignore',windowsHide:true});
  for(let i=0;i<100;i++){await pause(300);assert.equal(app.exitCode,null);try{cli(['doctor']);return;}catch{}}
  throw Error('Fresh isolated app did not become ready');
}
async function stop(){if(browser){await browser.close();browser=null;}if(app?.exitCode===null){app.kill();await new Promise(r=>app.once('exit',r));}}
function context(sessionId){
  const v=call('assistant_context_get',{sessionId});
  const catalog=JSON.parse(fs.readFileSync('control/commands.json','utf8'));
  const c=catalog.commands.find(c=>c.name==='assistant_context_get');
  assert.deepEqual(validateSchema(c.output_schema,v,reachableDefinitions(c.output_schema,catalog.$defs)),[]);
  assert(Buffer.byteLength(JSON.stringify(v))<=9000);assert.equal(v.summary_only,true);return v;
}
try{
  await launch();const settings=call('assistant_config_get').settings;
  assert.equal(settings.enabled,false);assert.equal(call('wechat_status').bound,false);
  const request={text:'合成上下文原任务',request_id:'context-native-original',mode:'task'};
  const first=call('assistant_send',{request});assert.equal(first.state,'disabled');
  const sessionId=first.session_id;
  for(let i=0;i<10;i++)call('assistant_send',{request:{session_id:sessionId,text:`仅讨论${i}`,request_id:`context-native-chat-${i}`,mode:'chat'}});
  for(let i=0;i<2;i++)call('assistant_send',{request:{session_id:sessionId,text:`合成后续工作${i}`,request_id:`context-native-work-${i}`,mode:'task'}});
  const before=context(sessionId);assert.equal(before.tasks_total,3);assert.equal(before.discussion_turns,10);
  assert(before.tasks.some(t=>t.id===first.turn_id));
  call('assistant_send',{request:{text:'FOREIGN-SESSION-SENTINEL',request_id:'context-native-foreign',mode:'task'}});
  const after=context(sessionId);assert.equal(after.fingerprint,before.fingerprint);
  assert(!JSON.stringify(after).includes('FOREIGN-SESSION-SENTINEL'));
  browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);
  page=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().startsWith('devtools:'));assert(page);
  const url=new URL(page.url());assert(/^(https?:\/\/tauri\.localhost|tauri:\/\/localhost)/.test(url.href));
  await page.route('**/*',r=>['tauri.localhost','localhost','127.0.0.1','ipc.localhost'].includes(new URL(r.request().url()).hostname)?r.continue():r.abort());
  url.searchParams.set('session',sessionId);await page.goto(url.href);
  await page.getByRole('heading',{name:'AI 助理',exact:true}).waitFor();
  await page.getByRole('button',{name:'上下文自动组织'}).click();
  const panel=page.getByRole('region',{name:'本次上下文'});
  await panel.getByText('3 条',{exact:true}).waitFor();
  call('assistant_send',{request:{session_id:sessionId,text:'外部接口新增的合成工作',request_id:'context-native-update',mode:'task'}});
  await panel.getByText('4 条',{exact:true}).waitFor();
  await page.screenshot({path:path.join(home,'native-context-summary.png')});
  const final=context(sessionId);assert.equal(final.tasks_total,4);assert.equal(final.discussion_turns,10);
  await stop();await launch();
  const restored=context(sessionId);assert.equal(restored.fingerprint,final.fingerprint);
  assert.equal(call('assistant_send',{request}).turn_id,first.turn_id);
  assert.deepEqual(call('assistant_config_get').settings,settings);assert.equal(call('wechat_status').bound,false);
  assert.equal(call('assistant_sessions_list').length,2);
  const report={passed:true,binary,binary_sha256:hash(fs.readFileSync(binary)),isolated_home:home,actual_cli_and_output_schema:true,task_and_discussion_counts:true,foreign_scope_and_fingerprint:true,native_embedded_panel:true,live_task_event_update:true,restart_fingerprint_and_original_ack:true,model_disabled_unchanged:true,real_model_calls:0,real_wechat_messages:0};
  fs.writeFileSync(path.join(parent,reportName),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
}catch(error){if(page)try{await page.screenshot({path:path.join(home,'native-context-failure.png')});console.log(JSON.stringify({failed_url:page.url(),visible_body:(await page.locator('body').innerText()).slice(0,1500)}));}catch{}throw error;}finally{await stop();}
