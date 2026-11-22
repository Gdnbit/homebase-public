// Only synthetic data made by the previous binary; no real profile or key.
import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';
import assert from 'node:assert/strict';import {spawn,spawnSync} from 'node:child_process';
import net from 'node:net';
import {chromium} from '@playwright/test';
const parent=process.env.HOMEBASE_HISTORY_NATIVE_LAB,binary=process.env.HOMEBASE_HISTORY_NATIVE_BINARY;
const reportName=process.env.HOMEBASE_HISTORY_NATIVE_REPORT||'history-native-v1.json';
const verifyWindow=process.env.HOMEBASE_HISTORY_VERIFY_WINDOW==='1';
assert(/^[a-zA-Z0-9.-]+\.json$/.test(reportName));
assert(parent&&binary&&path.isAbsolute(parent)&&path.isAbsolute(binary));assert(path.resolve(binary).startsWith(path.resolve(parent)+path.sep));
const baseline=JSON.parse(fs.readFileSync(path.join(parent,'history-before-native-v1.json'),'utf8'));
assert(baseline.complete_response&&baseline.seeded_messages===180);assert(path.resolve(baseline.isolated_home).startsWith(path.resolve(parent)+path.sep));
const source=path.join(baseline.isolated_home,'.homebase/assistant/state.dpapi');
const hash=data=>crypto.createHash('sha256').update(data).digest('hex');const sourceHash=hash(fs.readFileSync(source));
const home=fs.mkdtempSync(path.join(parent,'history-app-'));fs.mkdirSync(path.join(home,'.homebase/assistant'),{recursive:true});
const stateFile=path.join(home,'.homebase/assistant/state.dpapi');fs.copyFileSync(source,stateFile,fs.constants.COPYFILE_EXCL);
const env={...process.env,HOMEBASE_TEST_HOME:home,HOMEBASE_SECRETS_FILE:path.join(home,'secrets.yaml'),HOMEBASE_SECRETS_META:path.join(home,'secrets-meta.json'),HOMEBASE_AIHUB_ROOT:path.join(home,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(home,'models'),HOMEBASE_RUNTIME_ROOT:path.join(home,'runtime'),HOMEBASE_MODEL_CACHE:path.join(home,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(home,'output'),HOME:home,APPDATA:path.join(home,'AppData/Roaming'),LOCALAPPDATA:path.join(home,'AppData/Local'),TEMP:path.join(home,'tmp'),TMP:path.join(home,'tmp'),WEBVIEW2_USER_DATA_FOLDER:path.join(home,'wv')};
delete env.HOMEBASE_CONTROL_ENDPOINT;for(const dir of [env.APPDATA,env.LOCALAPPDATA,env.TEMP])fs.mkdirSync(dir,{recursive:true});
let windowPort;
if(verifyWindow){const listener=net.createServer();await new Promise(resolve=>listener.listen(0,'127.0.0.1',resolve));windowPort=listener.address().port;await new Promise(resolve=>listener.close(resolve));env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=`--remote-debugging-port=${windowPort}`;}
const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));let app;
function cli(args,input){const p=spawnSync(process.execPath,['scripts/homebase.mjs',...args],{env,input:input===undefined?undefined:JSON.stringify(input),encoding:'utf8',windowsHide:true,timeout:60000,maxBuffer:6*1024*1024});assert(!p.error,p.error?.message);return JSON.parse(p.stdout);}
function call(name,args={},failure){const value=cli(['call',name,'--input','-','--allow-private','--allow-write','--allow-external','--request-id',crypto.randomUUID(),'--wait-ms','10000'],args);if(failure){assert.equal(value.ok,false);assert.equal(value.error.code,failure);return value;}assert(value.ok,JSON.stringify(value.error));return value;}
async function launch(){app=spawn(binary,[],{env,cwd:process.cwd(),stdio:'ignore',windowsHide:true});for(let n=0;n<100;n++){await pause(250);assert.equal(app.exitCode,null);try{assert(cli(['doctor']).ok);return;}catch{}}throw Error('Owned isolated app unavailable');}
async function stop(){if(app&&app.exitCode===null){const current=app;spawnSync('taskkill',['/PID',String(current.pid),'/T','/F'],{stdio:'ignore',windowsHide:true});for(let n=0;n<80&&current.exitCode===null;n++)await pause(50);assert.notEqual(current.exitCode,null);}app=undefined;}
try{
 await launch();assert.equal(call('assistant_config_get').result.settings.enabled,false);assert.equal(call('wechat_status').result.bound,false);
 const legacy=call('assistant_session_get',{sessionId:baseline.session_id}).result;assert.equal(legacy.messages.length,180);
 let windowProof;
 if(verifyWindow){
  const readonlyBefore=hash(fs.readFileSync(stateFile));let before=null,pages=0,maxBytes=0,firstBytes=0;const blocks=[];
  do{const result=call('assistant_session_window_get',{request:{session_id:baseline.session_id,before_message_id:before,limit:40}}).result;const size=Buffer.byteLength(JSON.stringify(result));assert(size<=256*1024);if(pages===0)firstBytes=size;maxBytes=Math.max(maxBytes,size);assert.equal(result.page.returned,result.messages.length);assert.equal(result.page.end_index-result.page.start_index,result.messages.length);assert.equal(result.page.has_more,result.page.before_message_id!==null);assert(result.turns.every(item=>item.details_omitted&&item.turn.steps.length===0&&item.turn.pending===null));blocks.push(result.messages);before=result.page.before_message_id;pages++;}while(before);
  const firstCount=blocks[0].length,secondCount=blocks[1].length;assert(firstCount>0&&firstCount<=40);
  assert.deepEqual([...blocks].reverse().flat(),legacy.messages);assert.equal(hash(fs.readFileSync(stateFile)),readonlyBefore);
  const invalid=cli(['call','assistant_session_window_get','--input','-','--allow-private','--request-id',crypto.randomUUID(),'--wait-ms','10000'],{request:{session_id:baseline.session_id,before_message_id:'not-a-message',limit:40}});assert.equal(invalid.ok,false);assert.equal(invalid.error.code,'WINDOW_CURSOR_NOT_FOUND');
  let browser;try{
   for(let n=0;n<100;n++){try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${windowPort}`,{timeout:1000});break;}catch{await pause(200);}}
   assert(browser,'Owned WebView debug endpoint unavailable');let tab;
   for(let n=0;n<100;n++){tab=browser.contexts().flatMap(context=>context.pages()).find(page=>!page.url().startsWith('devtools:')&&!page.url().startsWith('about:'));if(tab)break;await pause(200);}assert(tab);
   await tab.waitForFunction(expected=>document.querySelectorAll('.chat-message').length===expected,firstCount,{timeout:30000});
   const count=await tab.locator('.chat-message').count();assert.equal(count,firstCount);
   const transcript=tab.getByLabel('对话内容',{exact:true});await transcript.evaluate(node=>{node.scrollTop=0;node.dispatchEvent(new Event('scroll',{bubbles:true}));});
   await tab.getByRole('button',{name:'显示更早消息',exact:true}).click();await tab.waitForFunction(expected=>document.querySelectorAll('.chat-message').length===expected,firstCount+secondCount,{timeout:10000});
   windowProof={actual_cli:true,complete_messages:180,pages,max_result_bytes:maxBytes,first_result_bytes:firstBytes,legacy_full_result_bytes:Buffer.byteLength(JSON.stringify(legacy)),wrong_cursor_explicit:true,actual_webview:true,requested_limit:40,initial_dom_messages:count,after_earlier_dom_messages:firstCount+secondCount,state_cipher_unchanged:true};
  }finally{await browser?.close();}
  assert.equal(hash(fs.readFileSync(stateFile)),readonlyBefore);
 }
 const stateBefore=hash(fs.readFileSync(stateFile)),args={session_id:baseline.session_id,kind:'messages',query:'',offset:0,limit:20};
 const first=call('assistant_history_query',{request:args}).result;assert.equal(first.version,'homebase-history-v1');assert.equal(first.total,180);
 const ids=[];let page=first,pageCalls=0;
 while(true){assert(Buffer.byteLength(JSON.stringify(page))<=9000);ids.push(...page.items.map(item=>item.id));pageCalls++;if(page.next_offset===null){assert(page.complete);break;}page=call('assistant_history_query',{request:{...args,snapshot_id:first.snapshot_id,offset:page.next_offset}}).result;}
 assert.deepEqual(ids,legacy.messages.map(message=>message.id));
 assert.equal(call('assistant_history_query',{request:{...args,query:'隔离'}}).result.total,90);
 const empty=call('assistant_history_query',{request:{...args,query:'不存在的合成关键词-xyz'}});assert.equal(empty.result.total,0);assert.equal(empty.result.complete,true);
 let joined='',offset=0,chunks=0;
 const read={session_id:baseline.session_id,snapshot_id:first.snapshot_id,item_id:first.items[0].id,limit:8192};
 while(true){const chunk=call('assistant_history_read',{request:{...read,offset}}).result;assert(Buffer.byteLength(JSON.stringify(chunk))<=9000);joined+=chunk.content;chunks++;if(chunk.next_offset===null){assert(chunk.complete);assert.equal(Buffer.byteLength(joined),chunk.total_bytes);assert.equal(hash(Buffer.from(joined)),chunk.json_sha256);break;}assert(chunk.next_offset>offset);offset=chunk.next_offset;}
 assert(chunks>1);assert.deepEqual(JSON.parse(joined),legacy.messages[0]);
 const inside=Buffer.byteLength(joined.slice(0,joined.indexOf('隔')))+1;
 call('assistant_history_read',{request:{...read,offset:inside}},'HISTORY_BAD_OFFSET');
 call('assistant_history_query',{request:{...args,snapshot_id:first.snapshot_id,query:'changed'}},'HISTORY_SCOPE_MISMATCH');
 call('assistant_history_read',{request:{...read,item_id:'not-in-snapshot',offset:0}},'HISTORY_ITEM_NOT_FOUND');
 assert.equal(hash(fs.readFileSync(stateFile)),stateBefore);await stop();await launch();
 call('assistant_history_read',{request:{...read,offset:0}},'HISTORY_SNAPSHOT_EXPIRED');
 const duplicate=call('assistant_send',{request:{text:'隔离"\\\\'.repeat(850),request_id:'history-before-0',mode:'task'}}).result;
 assert.equal(duplicate.turn_id,legacy.turns[0].id);assert.equal(duplicate.state,'disabled');
 assert.equal(hash(fs.readFileSync(stateFile)),stateBefore);assert.equal(hash(fs.readFileSync(source)),sourceHash);
 const report={passed:true,binary,binary_sha256:hash(fs.readFileSync(binary)),isolated_home:home,previous_binary_sha256:baseline.binary_sha256,old_state_copy_only:true,messages:ids.length,page_calls:pageCalls,chunks,legacy_full_response_preserved:true,bounded_actual_cli_pages_and_chunks:true,filter_and_empty_explicit:true,utf8_and_scope_errors_explicit:true,restart_expiry_explicit:true,original_receipt_not_replayed:true,state_cipher_unchanged_on_reads:true,source_cipher_unchanged:true,real_model_calls:0,real_wechat_messages:0};
 if(windowProof)report.timeline_window=windowProof;
 fs.writeFileSync(path.join(parent,reportName),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
}finally{await stop();}
