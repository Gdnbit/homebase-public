// Reuse only the public source-analysis fixture; do not read or copy personal configuration.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {dispatch} from './repo-capabilities.mjs';
import {store,read,write} from './repo-capabilities-store.mjs';
const receipt=process.env.HOMEBASE_REPO_RECEIPT;assert(receipt&&path.isAbsolute(receipt));const proof=JSON.parse(fs.readFileSync(receipt));
const original=read(path.join(proof.context.data_root,'projects',proof.project_id+'.json'));
const root=path.join(path.dirname(receipt),`negative-install-${crypto.randomUUID()}`);const context={...proof.context,data_root:path.join(root,'registry'),output_root:path.join(root,'output')};
function fixture(name){const s=store(context,true);const p={id:name,repository:original.repository,name:original.name,source:original.source,phase:'analyzed',revision:1};write(s.file('projects',p.id),p);s.putProject(p);s.close();return p;}
let p=fixture('failed-acceptance');let r=await dispatch('install',context,{project_id:p.id,revision:p.revision,request_id:'expected-behavior-failure',examples:[{input:{stdin:'# HomeBase'},contains:['THIS_CANNOT_BE_THE_OUTPUT_412c767a']}]});
if(r.project.error?.code==='INSTALL_FAILED'){p=(await dispatch('get',context,{project_id:p.id})).project;r=await dispatch('install',context,{project_id:p.id,revision:p.revision,request_id:'expected-behavior-network-retry',examples:[{input:{stdin:'# HomeBase'},contains:['THIS_CANNOT_BE_THE_OUTPUT_412c767a']}]});}
assert.equal(r.project.phase,'failed');assert.equal(r.project.error.code,'ACCEPTANCE_FAILED',JSON.stringify(r.project.error));assert.equal((await dispatch('capabilities',context,{})).total,0);
p=fixture('cancel-install');const pending=dispatch('install',context,{project_id:p.id,revision:p.revision,request_id:'cancel-install'});
await new Promise(resolve=>setTimeout(resolve,100));assert.equal((await dispatch('cancel',context,{project_id:p.id,operation_id:'cancel-install'})).state,'cancel_requested');
const canceled=await pending;assert.equal(canceled.project.phase,'canceled');assert.equal((await dispatch('capabilities',context,{})).total,0);
const before=JSON.stringify((await dispatch('get',context,{project_id:p.id})).project);await dispatch('projects',context,{});assert.equal(JSON.stringify((await dispatch('get',context,{project_id:p.id})).project),before);
fs.writeFileSync(path.join(root,'validation.json'),JSON.stringify({passed:true,fixture:'previously-pinned-public-source-only',actual_behavior_failure_blocked_registration:true,actual_install_cancellation:true,query_did_not_mutate_task:true,registered_capabilities:0,real_keys_read:false},null,2));console.log(JSON.stringify({passed:true,root}));
