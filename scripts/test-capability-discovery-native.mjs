// Fresh native profile only; no real credential, provider, MCP connection or WeChat message.
import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import net from 'node:net';
import assert from 'node:assert/strict';import {spawn,spawnSync} from 'node:child_process';import {chromium} from '@playwright/test';
import {reachableDefinitions,validateSchema} from './control-validation.mjs';
const parent=process.env.HOMEBASE_CAPABILITY_NATIVE_LAB,binary=process.env.HOMEBASE_CAPABILITY_NATIVE_BINARY,fixture=process.env.HOMEBASE_CAPABILITY_PDF_FIXTURE;
const reportName=process.env.HOMEBASE_CAPABILITY_NATIVE_REPORT||'capability-discovery-native-v1.json';
assert(parent&&path.isAbsolute(parent)&&binary&&path.isAbsolute(binary)&&fixture&&path.isAbsolute(fixture));
assert(path.resolve(binary).startsWith(path.resolve(parent)+path.sep));assert(/^[a-zA-Z0-9.-]+\.json$/.test(reportName));assert(!fs.existsSync(path.join(parent,reportName)));
const home=fs.mkdtempSync(path.join(parent,'capability-app-'));
const server=net.createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const port=server.address().port;await new Promise(r=>server.close(r));
const env={...process.env,HOMEBASE_TEST_HOME:home,HOMEBASE_SECRETS_FILE:path.join(home,'secrets.yaml'),HOMEBASE_SECRETS_META:path.join(home,'secrets-meta.json'),HOMEBASE_AIHUB_ROOT:path.join(home,'AIHub'),HOMEBASE_MODEL_ROOT:path.join(home,'models'),HOMEBASE_RUNTIME_ROOT:path.join(home,'runtime'),HOMEBASE_MODEL_CACHE:path.join(home,'cache'),HOMEBASE_ASR_OUTPUT_ROOT:path.join(home,'output'),HOME:home,APPDATA:path.join(home,'AppData/Roaming'),LOCALAPPDATA:path.join(home,'AppData/Local'),TEMP:path.join(home,'tmp'),TMP:path.join(home,'tmp'),WEBVIEW2_USER_DATA_FOLDER:path.join(home,'wv'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`};
for(const dir of [env.APPDATA,env.LOCALAPPDATA,env.TEMP])fs.mkdirSync(dir,{recursive:true});delete env.HOMEBASE_CONTROL_ENDPOINT;
const pause=ms=>new Promise(r=>setTimeout(r,ms)),hash=b=>crypto.createHash('sha256').update(b).digest('hex');
let app,browser,page,facts,factsResult;
function cli(args,input,allowFailure=false){const r=spawnSync(process.execPath,['scripts/homebase.mjs',...args],{env,input:input===undefined?undefined:JSON.stringify(input),encoding:'utf8',windowsHide:true,timeout:60000,maxBuffer:4*1024*1024});assert(!r.error,r.error?.message);let value;try{value=JSON.parse(r.stdout);}catch{throw Error('Expected isolated structured CLI output');}assert.equal(value.ok,!allowFailure,JSON.stringify(value.error));return value;}
function call(name,args={},allowFailure=false){const v=cli(['call',name,'--input','-','--allow-write','--allow-private','--allow-external','--request-id',crypto.randomUUID()],args,allowFailure);return allowFailure?v:v.result;}
async function launch(){app=spawn(binary,[],{env,cwd:process.cwd(),stdio:'ignore',windowsHide:true});for(let i=0;i<100;i++){await pause(300);assert.equal(app.exitCode,null);try{cli(['doctor']);return;}catch{}}throw Error('Isolated app unavailable');}
async function stop(){if(browser){await browser.close();browser=null;}if(app?.exitCode===null){app.kill();await new Promise(r=>app.once('exit',r));}}
const catalog=JSON.parse(fs.readFileSync('control/commands.json','utf8'));const command=catalog.commands.find(c=>c.name==='capability_index_query');
function query(request={}){const response=cli(['call','capability_index_query','--input','-','--allow-private'],{request:{offset:0,limit:1,...request}});const p=response.result;assert.deepEqual(validateSchema(command.output_schema,p,reachableDefinitions(command.output_schema,catalog.$defs)),[]);assert(Buffer.byteLength(JSON.stringify(p))<=9000);if(p.next_offset!==null)assert.equal(response.result_state,'partial');return p;}
try{
  if(process.env.HOMEBASE_CAPABILITY_FACTS_TEST==='1')facts=await (await import('./capability-facts-native-fixture.mjs')).prepareFacts({home,binary});
  await launch();const settings=call('assistant_config_get').settings;assert.equal(settings.enabled,false);assert.equal(call('wechat_status').bound,false);
  const original=query();assert(original.total>=2);const entries=[...original.entries];let current=original;
  while(current.next_offset!==null){current=query({snapshot_id:original.snapshot_id,offset:current.next_offset,limit:40});entries.push(...current.entries);}
  assert.equal(entries.length,original.total);assert.equal(new Set(entries.map(e=>e.entry_key)).size,entries.length);
  const functions=[...new Set(entries.map(e=>e.function))];
  for(const name of functions){const filtered=query({catalog_id:original.catalog_id,function:name,limit:10});assert.equal(filtered.total,entries.filter(e=>e.function===name).length);assert(filtered.entries.every(e=>e.function===name));}
  const mismatch=call('capability_index_query',{request:{snapshot_id:original.snapshot_id,query:'different-filter',offset:0,limit:1}},true);assert.equal(mismatch.error.code,'DISCOVERY_FILTER_MISMATCH');
  const pdf=entries.find(e=>e.target==='homebase.pdf_text');assert(pdf);assert(pdf.prerequisites.some(t=>t.includes('微信')));
  const marker=path.join(home,'mcp-must-not-start.txt');
  call('shared_mcp_save',{pool:{servers:[{id:'homebase-discovery-fixture',name:'仅隔离索引新增来源',description:'Metadata only',transport:'stdio',command:process.execPath,args:['-e',`require('node:fs').writeFileSync(${JSON.stringify(marker)},'unexpected probe')`],env:{}}],assignments:{}}});
  assert.equal(query({catalog_id:original.catalog_id,query:'仅隔离索引新增来源'}).total,0,'A frozen catalog must not reread the changed source');
  const fresh=query({query:'仅隔离索引新增来源'});assert.equal(fresh.total,1);assert.equal(fresh.entries[0].state,'authorization_required');assert.equal(fresh.entries[0].cost,'unknown');assert(!fs.existsSync(marker));
  const docId=crypto.randomUUID();const imported=cli(['documents','import','--file',fixture,'--document-id',docId,'--scope','new','--allow-write','--allow-private']);assert.equal(imported.parsed,false);
  const extraction=call('assistant_document_extract',{scope:'new',documentId:docId});assert.equal(extraction.status,'started');let status;
  for(let i=0;i<100;i++){status=call('assistant_document_status',{scope:'new',documentId:docId,extractionId:extraction.extraction.id});if(status.phase!=='processing')break;await pause(150);}
  assert.equal(status.phase,'ready');const result=call('assistant_document_result',{scope:'new',documentId:docId,extractionId:status.id});assert.equal(result.result.pages[0].text,'中文🙂');
  assert.equal(call('assistant_document_get',{scope:'new',documentId:docId}).extractions.length,1);
  browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);page=browser.contexts().flatMap(c=>c.pages()).find(p=>!p.url().startsWith('devtools:'));assert(page);assert(/^(https?:\/\/tauri\.localhost|tauri:\/\/localhost)/.test(page.url()));
  await page.getByRole('heading',{name:'AI 助理',exact:true}).waitFor();const modelButton=page.getByRole('button',{name:'选择本次对话模型',exact:true});await modelButton.focus();assert(await modelButton.evaluate(el=>document.activeElement===el));
  const history=call('assistant_send',{request:{text:'这是旧会话的合成历史，不应跳转',request_id:'native-focused-new-input',mode:'task'}});assert.equal(history.state,'disabled');
  await page.locator(`a[href*="session=${history.session_id}"]`).first().waitFor();await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));assert(await page.getByRole('heading',{name:'今天想一起完成什么？',exact:true}).isVisible(),'A late old conversation must not replace the focused new composer');
  const sessionsBeforeSelection=call('assistant_sessions_list').length;assert.equal(sessionsBeforeSelection,1);
  await page.getByRole('link',{name:'能力库',exact:true}).click();await page.getByRole('button',{name:/文档处理.*个实现/}).click();const dialog=page.getByRole('dialog');await dialog.getByRole('heading',{name:'PDF文字提取',exact:true}).waitFor();
  const card=dialog.locator('article').filter({has:page.getByRole('heading',{name:'PDF文字提取',exact:true})});await card.getByText('运行条件与依据').click();assert((await card.innerText()).includes('微信与原生Agent附件、OCR'));
  await page.screenshot({path:path.join(home,'native-capability-discovery.png')});await card.getByRole('link',{name:'使用这个实现准备任务'}).click();await page.waitForFunction(()=>document.querySelector('#assistant-message')?.value.includes('PDF文字提取'));assert.equal(call('assistant_sessions_list').length,sessionsBeforeSelection,'Selecting an implementation must not execute or create a conversation');
  let prepared;for(let i=0;i<30;i++){prepared=call('assistant_desktop_get',{scope:'new'});if(prepared.entry.body.text.includes('PDF文字提取'))break;await pause(100);}assert(prepared.entry.body.text.includes('PDF文字提取'),'The prepared capability draft must belong to the new input, not the old conversation');
  const service=entries.find(e=>e.function==='内容分发');assert(service,'The real default service group must be included in this verification');
  await page.getByRole('link',{name:'能力库',exact:true}).click();await page.getByRole('button',{name:/内容分发.*个实现/}).click();await page.getByRole('dialog').getByRole('heading',{name:service.name,exact:true}).waitFor();await page.screenshot({path:path.join(home,'native-unicode-service-group.png')});
  if(facts)factsResult=await (await import('./capability-facts-native-fixture.mjs')).checkFacts({facts,call,query,page,home});
  const pool=path.join(home,'.homebase/shared/mcp-pool.json');assert(fs.existsSync(pool));const savedHash=hash(fs.readFileSync(pool));await stop();await launch();
  const expired=call('capability_index_query',{request:{snapshot_id:original.snapshot_id,offset:0,limit:1}},true);assert.equal(expired.error.code,'DISCOVERY_SNAPSHOT_EXPIRED');
  const oldCatalog=call('capability_index_query',{request:{catalog_id:original.catalog_id,offset:0,limit:1}},true);assert.equal(oldCatalog.error.code,'DISCOVERY_CATALOG_EXPIRED');assert.equal(hash(fs.readFileSync(pool)),savedHash);assert(!fs.existsSync(marker));
  assert.equal(query({query:'仅隔离索引新增来源'}).total,1);assert.deepEqual(call('assistant_config_get').settings,settings);assert.equal(call('wechat_status').bound,false);assert.equal(call('assistant_sessions_list').length,sessionsBeforeSelection);
  const report={passed:true,binary,binary_sha256:hash(fs.readFileSync(binary)),isolated_home:home,actual_cli_and_typed_page:true,enumerated:original.total,all_actual_functions_queried:functions,unicode_service_group_native:true,focused_new_input_keeps_its_scope:true,stable_snapshot_and_explicit_refresh:true,no_mcp_probe:true,metadata_only_cost_and_authorization:true,discovered_pdf_actual_extract_status_result:true,one_extraction:true,native_group_drawer_and_prerequisites:true,selection_only_prepares_draft:true,restart_expiry_explicit:true,isolated_pool_unchanged_on_restart:true,model_disabled_unchanged:true,real_model_calls:0,real_wechat_messages:0};
  if(facts){assert.equal(hash(fs.readFileSync(facts.pins)),facts.pins_sha256);assert.deepEqual(call('model_capabilities_catalog',{offset:0,limit:200}).models.find(m=>m.route_id===facts.pricing.route_id).pricing,facts.pricing);report.facts=factsResult;}
  fs.writeFileSync(path.join(parent,reportName),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
}catch(error){if(page)try{await page.screenshot({path:path.join(home,'native-capability-failure.png')});console.log(JSON.stringify({failed_url:page.url(),visible_body:(await page.locator('body').innerText()).slice(0,1800)}));}catch{}throw error;}finally{await stop();}
