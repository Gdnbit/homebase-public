// Explicit fresh desktop profile only. No real model, account or WeChat message.
import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import net from 'node:net';
import assert from 'node:assert/strict';import {spawn,spawnSync} from 'node:child_process';import {chromium} from '@playwright/test';
const parent=process.env.HOMEBASE_DOCUMENT_NATIVE_LAB,binary=process.env.HOMEBASE_DOCUMENT_BINARY,fixture=process.env.HOMEBASE_DOCUMENT_FIXTURE;
const reportName=process.env.HOMEBASE_DOCUMENT_NATIVE_REPORT||'documents-native-validation-v1.json';assert(/^[a-zA-Z0-9.-]+\.json$/.test(reportName));
assert(parent&&path.isAbsolute(parent)&&binary&&path.isAbsolute(binary)&&fixture&&path.isAbsolute(fixture));
assert(path.resolve(binary).startsWith(path.resolve(parent)+path.sep),'Staged lab binary required');
const home=fs.mkdtempSync(path.join(parent,'document-app-'));
const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
const env={...process.env,HOMEBASE_TEST_HOME:home,HOMEBASE_SECRETS_FILE:path.join(home,'secrets.yaml'),HOMEBASE_SECRETS_META:path.join(home,'secrets-meta.json'),HOMEBASE_AIHUB_ROOT:path.join(home,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(home,'models'),HOMEBASE_RUNTIME_ROOT:path.join(home,'runtime'),HOMEBASE_MODEL_CACHE:path.join(home,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(home,'output'),WEBVIEW2_USER_DATA_FOLDER:path.join(home,'wv'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`};
delete env.HOMEBASE_CONTROL_ENDPOINT;
const pause=ms=>new Promise(r=>setTimeout(r,ms));const hash=b=>crypto.createHash('sha256').update(b).digest('hex');
let app,browser,page;
function cli(args,input,allowFailure=false){
  const run=spawnSync(process.execPath,['scripts/homebase.mjs',...args],{env,input:input===undefined?undefined:JSON.stringify(input),encoding:'utf8',windowsHide:true,timeout:60000,maxBuffer:8*1024*1024});
  assert(!run.error,run.error?.message);let value;try{value=JSON.parse(run.stdout);}catch{throw Error('Native CLI did not return structured JSON');}
  if(allowFailure)assert.equal(value.ok,false);else assert.equal(value.ok,true,JSON.stringify(value.error));return value;
}
function call(name,args={},allowFailure=false){const v=cli(['call',name,'--input','-','--allow-write','--allow-private','--allow-external','--request-id',crypto.randomUUID()],args,allowFailure);return allowFailure?v:v.result;}
async function launch(){app=spawn(binary,[],{env,cwd:process.cwd(),stdio:'ignore',windowsHide:true});for(let i=0;i<100;i++){await pause(300);assert.equal(app.exitCode,null);try{cli(['doctor']);return;}catch{}}throw Error('Isolated application unavailable');}
async function stop(){if(browser){await browser.close();browser=null;}if(app?.exitCode===null){app.kill();await new Promise(r=>app.once('exit',r));}}
try{
  await launch();const config=call('assistant_config_get').settings;assert.equal(config.enabled,false);assert.equal(call('wechat_status').bound,false);
  const id=crypto.randomUUID();const imported=cli(['documents','import','--file',fixture,'--document-id',id,'--scope','new','--allow-write','--allow-private']);assert.equal(imported.parsed,false);
  const sourceFile=path.join(home,'.homebase/assistant/documents',id,'source.dpapi');const sourceHash=hash(fs.readFileSync(sourceFile));assert(!fs.readFileSync(sourceFile).includes(Buffer.from('中文')));
  cli(['documents','import','--file',fixture,'--document-id',id,'--scope','new','--allow-write','--allow-private']);assert.equal(hash(fs.readFileSync(sourceFile)),sourceHash);
  assert.equal(call('assistant_document_get',{scope:'new',documentId:id}).extractions.length,0);
  const started=call('assistant_document_extract',{scope:'new',documentId:id});assert.equal(started.status,'started');const key=started.extraction.id;
  let receipt;for(let i=0;i<100;i++){receipt=call('assistant_document_status',{scope:'new',documentId:id,extractionId:key});if(!['processing','cancel_requested'].includes(receipt.phase))break;await pause(150);}
  assert.equal(receipt.phase,'ready');const attempt=receipt.attempt;
  const parsed=call('assistant_document_result',{scope:'new',documentId:id,extractionId:key});assert.equal(parsed.result.pages[0].text,'中文🙂');
  const excerpt=call('assistant_document_excerpt',{scope:'new',documentId:id,extractionId:key,pages:{from:1,to:1}});assert.equal(excerpt.document.source_sha256,hash(fs.readFileSync(fixture)));
  browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);page=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().startsWith('devtools:'));assert(page);
  console.log(JSON.stringify({native_ui_url:page.url()}));
  assert(/^(https?:\/\/tauri\.localhost|tauri:\/\/localhost)/.test(page.url()),'Native frontend must use embedded application assets, not an external directory URL');
  await page.getByRole('heading',{name:'AI 助理',exact:true}).waitFor();await page.getByRole('button',{name:'管理 PDF 文档'}).click();
  const dialog=page.getByRole('dialog',{name:'PDF 文字预览'});await dialog.getByRole('button',{name:new RegExp(path.basename(fixture).replace(/[.*+?^${}()|[\]\\]/g,'\\$&'))}).click();
  await page.getByRole('button',{name:'加入消息',exact:true}).waitFor();await page.waitForFunction(()=>document.querySelector('[aria-label="PDF 提取文字"]')?.textContent?.includes('中文🙂'));
  await page.screenshot({path:path.join(home,'native-pdf-preview.png')});await dialog.getByRole('button',{name:'加入消息',exact:true}).click();
  let draft;for(let i=0;i<50;i++){draft=call('assistant_desktop_get',{scope:'new'}).entry;if(draft.body.files[0]?.document?.document_id===id)break;await pause(150);}
  assert.deepEqual(draft.body.files[0].document,excerpt.document);assert.equal(call('assistant_document_get',{scope:'new',documentId:id}).extractions.length,1);
  const desktopFile=path.join(home,'.homebase/assistant/desktop',hash('new')+'.draft.dpapi');const draftHash=hash(fs.readFileSync(desktopFile));
  await stop();await launch();draft=call('assistant_desktop_get',{scope:'new'}).entry;assert.equal(hash(fs.readFileSync(desktopFile)),draftHash);assert.deepEqual(draft.body.files[0].document,excerpt.document);assert.equal(hash(fs.readFileSync(sourceFile)),sourceHash);
  assert.equal(call('assistant_document_status',{scope:'new',documentId:id,extractionId:key}).attempt,attempt);
  const saved=call('assistant_desktop_save',{request:{scope:'new',revision:draft.revision,body:{...draft.body,text:'仅合成 PDF 原生链路验证'}}});
  const request={session_id:null,text:'仅合成 PDF 原生链路验证',request_id:'document-native-original',mode:saved.entry.body.mode,attachments:saved.entry.body.files};
  const ack=call('assistant_desktop_send',{request,revision:saved.entry.revision});assert.equal(ack.status,'accepted');assert.equal(ack.ack.state,'disabled');const scope=ack.ack.session_id;
  call('assistant_document_get',{scope:'new',documentId:id},true);assert.equal(call('assistant_document_get',{scope,documentId:id}).document.id,id);
  const other=call('assistant_session_create').id;call('assistant_document_get',{scope:other,documentId:id},true);
  await stop();await launch();const again=call('assistant_desktop_send',{request,revision:0});assert.equal(again.ack.turn_id,ack.ack.turn_id);
  const resolved=call('assistant_desktop_resolve',{scope:'new',request});assert.equal(resolved.ack.turn_id,ack.ack.turn_id);
  const before=call('assistant_sessions_list').length;
  call('assistant_send',{request:{...request,request_id:'document-forged',session_id:scope,attachments:[{...excerpt,content:excerpt.content+'伪造'}]}},true);assert.equal(call('assistant_sessions_list').length,before);
  const large=path.join(home,'synthetic-four-mib.pdf');const raw=Buffer.alloc(4*1024*1024,32);fs.readFileSync(fixture).copy(raw);fs.writeFileSync(large,raw);const largeId=crypto.randomUUID();
  const full=cli(['documents','import','--file',large,'--document-id',largeId,'--scope','new','--allow-write','--allow-private']);assert.equal(full.document.source_bytes,4*1024*1024);assert.equal(full.document.source_sha256,hash(raw));assert.equal(call('assistant_document_get',{scope:'new',documentId:largeId}).extractions.length,0);
  assert.deepEqual(call('assistant_config_get').settings,config);assert.equal(call('wechat_status').bound,false);
  const report={passed:true,binary,binary_sha256:hash(fs.readFileSync(binary)),isolated_home:home,real_cli_chunk_import:true,full_four_mib_control_frames:true,actual_local_pdfjs:true,unicode_text:'中文🙂',native_preview_and_explicit_add:true,protected_draft_restart:true,source_cipher_unchanged:true,formal_session_ownership:true,foreign_session_rejected:true,forged_text_rejected:true,restart_ack_reused:true,model_disabled_unchanged:true,real_model_calls:0,real_wechat_messages:0};
  fs.writeFileSync(path.join(parent,reportName),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
}catch(error){if(page){try{await page.screenshot({path:path.join(home,'native-pdf-failure.png')});console.log(JSON.stringify({failed_url:page.url(),visible_body:(await page.locator('body').innerText()).slice(0,2000)}));}catch{}}throw error;}finally{await stop();}
