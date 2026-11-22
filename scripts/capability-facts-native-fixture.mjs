// Explicitly opted-in synthetic native checks. Never import a real user profile.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {dispatch} from './repo-capabilities.mjs';
import {digest,write} from './repo-capabilities-store.mjs';

export async function prepareFacts({home,binary}) {
  assert.equal(process.env.HOMEBASE_CAPABILITY_FACTS_TEST,'1');
  const parent = process.env.HOMEBASE_CAPABILITY_NATIVE_LAB;
  assert(parent && path.isAbsolute(parent), 'Set HOMEBASE_CAPABILITY_NATIVE_LAB');
  assert.equal(path.dirname(path.resolve(home)), path.resolve(parent));
  assert(path.basename(home).startsWith('capability-app-'));
  const executable=path.join(path.dirname(binary),'distribution/agent-runtime/tools/bwrap');
  const context={data_root:path.join(home,'.homebase/capabilities/projects'),output_root:path.join(home,'.homebase/wechat/workspace/repo-capabilities'),sandbox:{kind:'wsl-bubblewrap',executable,sha256:digest(fs.readFileSync(executable)),distro:'Ubuntu'}};
  const analyzed=await dispatch('analyze',context,{repository_url:'https://github.com/markedjs/marked',request_id:'facts-analyze'});
  assert.equal(analyzed.project.phase,'analyzed',JSON.stringify(analyzed.project.error));
  const installed=await dispatch('install',context,{project_id:analyzed.project.id,revision:analyzed.project.revision,request_id:'facts-install'});
  assert.equal(installed.project.phase,'registered',JSON.stringify(installed.project.error));
  const pricing={route_id:'openrouter::fixture-hb-facts',state:'partial',source_url:'https://openrouter.ai/api/v1/models',observed_at:'2026-11-16T03:00:00+08:00',reference_only:true,rates:[{kind:'prompt',amount:'0.000000000000000000000001',unit:'usd_per_token',per_million_tokens:'0.000000000000000001'},{kind:'completion',amount:'0',unit:'usd_per_token',per_million_tokens:'0'},{kind:'request',amount:'0.01',unit:'usd_per_request',per_million_tokens:null}],unmodeled_dimensions:2,missing_dimensions:0,invalid_dimensions:0,conditional:true};
  pricing.source_url='https://openrouter.ai/api/v1/models/user?output_modalities=all';
  const route={route_id:pricing.route_id,model_id:'fixture-hb-facts',name:'隔离报价模型',publisher:'fixture',canonical_id:'fixture/hb-facts',provider_id:'openrouter',functions:['text'],protocol:'chat',source:'api_catalog',source_url:'https://openrouter.ai/fixture-hb-facts',account_state:'listed_not_inference_verified',input_schema:{type:'object'},input_example:{messages:[{role:'user',content:'Synthetic only'}]},pricing};
  const pins=path.join(home,'.homebase/assistant/model-routes.json');write(pins,{[route.route_id]:route});
  return {context,capability_id:installed.project.capability_id,pins,pins_sha256:digest(fs.readFileSync(pins)),pricing};
}

export async function checkFacts({facts,call,query,page,home}) {
  if(await page.getByRole('dialog').count())await page.getByLabel('关闭能力实现').click();
  const catalog=call('model_capabilities_catalog',{offset:0,limit:200});
  assert.deepEqual(catalog.models.find(m=>m.route_id===facts.pricing.route_id).pricing,facts.pricing);
  const projection=query({query:'隔离报价模型',limit:10});assert.equal(projection.total,1);
  assert.deepEqual(projection.entries[0].pricing,facts.pricing);assert.equal(projection.entries[0].cost,'unknown');
  const listed=call('repo_capabilities_list',{query:facts.capability_id,offset:0,limit:6});
  assert.equal(listed.entries[0].provenance.state,'registry_recorded');
  const details=call('repo_capability_describe',{capabilityId:facts.capability_id});
  assert.equal(details.provenance.state,'records_checked');assert.equal(details.provenance.code_integrity_checked,false);
  assert.equal(details.provenance.source_commit,details.capability.commit);
  const request={capability_id:facts.capability_id,input:{stdin:'# Native facts\n\n**Keyless** 🙂'},request_id:'facts-native-run'};
  const first=call('repo_capability_run',{request});assert.equal(first.status,'succeeded',JSON.stringify(first));
  assert.match(first.stdout,/<strong>Keyless<\/strong>/);assert(first.stdout.includes('🙂'));
  const replay=call('repo_capability_run',{request});assert.equal(replay.run_id,first.run_id);
  const result=call('repo_capability_result',{runId:first.run_id});assert.equal(result.artifact.sha256,first.artifact.sha256);
  const registry=path.join(facts.context.data_root,'registry.sqlite'),before=digest(fs.readFileSync(registry));
  const index=query({query:facts.capability_id,limit:10});assert.equal(index.entries[0].provenance.state,'registry_recorded');
  assert.equal(digest(fs.readFileSync(registry)),before);
  await page.getByRole('link',{name:'能力库',exact:true}).click();
  await page.getByRole('button',{name:/文字与推理.*个实现/}).click();
  const card=page.getByRole('dialog').locator('article').filter({has:page.getByRole('heading',{name:'隔离报价模型',exact:true})});
  await card.getByLabel('参考价格').waitFor();assert((await card.innerText()).includes('0.000000000000000001 USD / 百万 token'));
  assert((await card.innerText()).includes('目录已收录 · 尚未实测调用'));assert((await card.innerText()).includes('账号筛选目录'));assert(!(await card.innerText()).includes('listed_not_inference_verified'));
  await card.scrollIntoViewIfNeeded();await page.screenshot({path:path.join(home,'native-price-facts.png')});await card.screenshot({path:path.join(home,'native-price-card.png')});await page.getByLabel('关闭能力实现').click();
  await page.getByRole('button',{name:/文档处理.*个实现/}).click();
  const repo=page.getByRole('dialog').locator('article').filter({has:page.getByRole('heading',{name:details.capability.name,exact:true})});
  await repo.getByText('验收登记 · 运行前核对',{exact:true}).click();assert((await repo.innerText()).includes(details.capability.commit));
  await page.screenshot({path:path.join(home,'native-repository-evidence.png')});await page.getByLabel('关闭能力实现').click();
  const runFile=path.join(facts.context.data_root,'runs',first.run_id+'.json'),original=fs.readFileSync(runFile),record=JSON.parse(original);
  write(runFile,{...record,id:'run_foreign'});
  const alias=call('repo_capability_result',{runId:first.run_id},true);assert.equal(alias.error.code,'BUSINESS_FAILED');assert.equal(alias.result.error.code,'RUN_IDENTITY_INVALID');
  fs.writeFileSync(runFile,original);
  assert.equal(call('repo_capability_result',{runId:first.run_id}).artifact.sha256,first.artifact.sha256);
  const cap=listed.entries[0];
  // Locate from the owned registry rather than inventing production paths.
  const {store}=await import('./repo-capabilities-store.mjs');const s=store(facts.context);const registered=s.index().capabilities.find(c=>c.id===cap.id);s.close();
  const oldProof=fs.readFileSync(registered.verification_path);write(registered.verification_path,{version:1,passed:false});
  const changed=call('repo_capability_describe',{capabilityId:facts.capability_id},true);assert.equal(changed.error.code,'BUSINESS_FAILED');assert.equal(changed.result.error.code,'CAPABILITY_CHANGED');
  assert.equal(call('repo_capability_run',{request}).run_id,first.run_id,'An original receipt must survive changed current installation evidence');
  fs.writeFileSync(registered.verification_path,oldProof);assert.equal(call('repo_capability_describe',{capabilityId:facts.capability_id}).verification.passed,true);
  assert.equal(digest(fs.readFileSync(facts.pins)),facts.pins_sha256);
  const report={passed:true,actual_native_cli:true,actual_os_sandbox:true,original_run_id:first.run_id,one_business_run:true,exact_pinned_prices:true,registry_and_record_scope_distinct:true,returned_identity_checked:true,changed_proof_rejected:true,original_receipt_resolves_after_proof_change:true,real_model_calls:0,real_wechat_messages:0};
  write(path.join(home,'native-facts.json'),report);return report;
}
