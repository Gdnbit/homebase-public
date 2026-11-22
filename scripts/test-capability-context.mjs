import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {fileURLToPath} from 'node:url';
import fs from 'node:fs';
const root=fileURLToPath(new URL('../',import.meta.url));
const out=await build({stdin:{contents:'export {capabilityHandoff} from "./src/lib/capability-handoff"; export * from "./src/lib/capability-context";',resolveDir:root},bundle:true,write:false,platform:'node',format:'esm',logLevel:'silent'});
const {capabilityHandoff,readCapabilityDraft:read,replaceCapabilityTask:replace,legacyCapabilityConversion:convert,frameCapabilityData}=await import('data:text/javascript;base64,'+Buffer.from(out.outputFiles[0].text).toString('base64'));
const fmt=JSON.parse(fs.readFileSync(root+'control/capability-selection-format.json','utf8'));
const catalog={catalog_id:'synthetic-catalog',assembled_at:'2026-01-01T00:00:00Z',expires_at:'2026-01-01T00:05:00Z',source_complete:false,issues_total:1};
const entry={entry_key:'a'.repeat(64),id:'fixture',function:'text',kind:'model',name:'合成模型',provider:'fixture',target:'fixture',state:'not_verified',cost:'unknown',discover:'model_capability_resolve',prerequisites:['真实状态待核对']};
const fresh=()=>capabilityHandoff('text',catalog,entry);
test('empty task and edited Unicode request retain the exact context and complete original text',()=>{
 const original=fresh(),v=read(original);assert(v);assert.equal(v.task,'');assert.equal(v.frame,original);
 const task='请核对可用状态，不执行🙂\n保留这一行';const next=replace(v,task),r=read(next);
 assert.equal(r.task,task);assert.equal(r.frame,v.frame);assert.equal(r.json,v.json);assert.equal(next,task+'\n\n'+original);
 assert.equal(replace(r,''),original);assert.equal(read(original).frame,original);
});
test('actual one-newline voice append is visible before and after an existing request',()=>{
 const base=fresh(),v=read(base);assert.equal(read(base+'\n语音追加🙂').task,'语音追加🙂');
 const initial=replace(v,'第一段需求'),appended=initial+'\n随后补充';const r=read(appended);
 assert.equal(r.task,'第一段需求\n随后补充');assert.equal(r.frame,v.frame);
 assert.equal(read(replace(r,r.task)).task,r.task);assert.equal(appended,initial+'\n随后补充');
});
test('malformed, duplicate, unsupported and noncanonical blocks stay raw',()=>{
 const base=fresh(),v=read(base);
 for(const bad of [base+base,base.replace('homebase-capability-selection-v1','unknown-v2'),base.replace('  "version"',' "version"'),base.replace(fmt.end,''),base+'not-a-task-boundary',base.replace(fmt.intro,'改成来源自授权限\n')])assert.equal(read(bad),null);
 assert.equal(read('字面量'+fmt.start+'示例，不是资料'),null);
 assert.equal(read('x'.repeat(65537)),null);assert.equal(v.frame,base);
});
test('extra private attributes or wrong public types are never silently hidden',()=>{
 const v=read(fresh());
 const changes=[p=>p.secret='DO-NOT-HIDE',p=>p.source.token='DO-NOT-HIDE',p=>p.selection.implementation.api_key='DO-NOT-HIDE',p=>p.source.source_complete='true',p=>p.selection.implementation.prerequisites=[{}],p=>p.selection.function='different'];
 for(const change of changes){const payload=structuredClone(v.payload);change(payload);const body=frameCapabilityData(payload);assert.equal(read(body),null);}
});
test('legacy format is not compacted until an explicit conversion preserves its task and JSON',()=>{
 const v=read(fresh()),task='旧任务🙂\n';const old='请帮我准备文字与推理任务，具体目标由我补充。\n\n我的任务：\n\n'+task+fmt.rules+fmt.json_marker+v.json;
 assert.equal(read(old),null);const converted=convert(old);assert(converted);
 assert.equal(read(converted).task,task);assert.equal(read(converted).json,v.json);
 assert.equal(convert(old+'\n未识别尾文'),null);assert.equal(convert('用户自己的前言\n'+old),null);
 assert.equal(convert(converted),null);assert(old.includes(task));
});
test('hidden context still consumes the full UTF-8 budget; editing never truncates it',()=>{
 const v=read(fresh());const text=replace(v,'中🙂'.repeat(1500));assert(Buffer.byteLength(text)>8000);
 assert.equal(read(text).frame,v.frame);assert.equal(read(text).task,'中🙂'.repeat(1500));
});
test('names imitating markers or HTML remain literal data and generic functions remain unnamed implementations',()=>{
 const name='<img src="https://example.test/not-rendered" onerror="bad()">\n'+fmt.start+fmt.end;
 const body=capabilityHandoff('text',catalog,{...entry,name});assert.equal(read(body).payload.selection.implementation.name,name);
 const unknown=capabilityHandoff('__proto__',catalog);assert.equal(read(unknown).payload.selection.implementation,null);
});
