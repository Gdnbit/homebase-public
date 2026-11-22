import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import { createDevelopment } from './development-capability.mjs';

function fixture() {
  const home=fs.mkdtempSync(path.join(os.tmpdir(),'homebase-development-'));
  const workspace=path.join(home,'project 中文');fs.mkdirSync(workspace);
  let observed, submitted=0; const calls=[];
  const api=createDevelopment({home,runBridge(args){
    calls.push(args);
    if(args[0]==='submit-json') {submitted++; observed={task:args[1],runs:[{id:1,state:'queued'}],questions:[]};return {task:args[1],receipt:{routed:[1]}};}
    if(args[0]==='result'){if(observed instanceof Error)throw observed;return structuredClone(observed);}
    if(args[0]==='answer'){observed.questions=[];return {answered:true};}
    return {agents:[{online:true}]};
  }});
  const request={request_id:'synthetic-one',workspace,brief:'只在合成项目创建一个函数。',checks:[['node','--test']],protected_files:[]};
  return {api,home,workspace,request,calls,get submitted(){return submitted;},set observed(value){observed=value;},get observed(){return observed;}};
}
test('reject invalid scope, workflow and argv before dispatch',()=>{
  const f=fixture();for(const patch of [{workspace:'relative'},{workspace:path.parse(f.workspace).root},{workflow:'invented'},{checks:['node --test']},{checks:[['node','\0']]},{request_id:'a/b'},{protected_files:['../outside']},{extra:'not supported'}])assert.throws(()=>f.api.start({...f.request,...patch}));
  assert.equal(f.submitted,0);
  assert.throws(()=>f.api.start({...f.request,workspace:path.join(f.home,'.agent-autoloop')}));
});
test('stable request identity survives restart; changed input conflicts and never dispatches twice',()=>{
  const f=fixture(), first=f.api.start(f.request);assert.equal(first.state,'submitted');
  assert.equal(f.api.start(f.request).task_id,first.task_id);assert.equal(f.submitted,1);
  assert.throws(()=>f.api.start({...f.request,brief:'另一项开发'}),/CONFLICT/);assert.equal(f.submitted,1);
  assert.equal(fs.readFileSync(f.calls[0][3],'utf8'),f.request.brief);
  assert(!f.calls[0].includes(f.request.brief),'private requirement is not an argv value');
});
test('acceptance guard is in private options and includes only existing project files',()=>{
  const f=fixture();fs.writeFileSync(path.join(f.workspace,'acceptance.test.mjs'),'synthetic');
  f.api.start({...f.request,protected_files:['acceptance.test.mjs']});
  const options=JSON.parse(fs.readFileSync(f.calls[0][4],'utf8'));assert.equal(options.guards.length,2);assert.equal(options.guards[1],fs.realpathSync(path.join(f.workspace,'acceptance.test.mjs')));
});
test('lost result is unknown and repeated submit does not restart work',()=>{
  const f=fixture(), ack=f.api.start(f.request);f.observed=Error('offline');
  assert.equal(f.api.get(ack.task_id).state,'unknown');assert.equal(f.api.start(f.request).state,'unknown');assert.equal(f.submitted,1);
});
test('running and retrying states are projected without claiming completion',()=>{
  const f=fixture(), ack=f.api.start(f.request);
  for(const [native,state] of [['running','running'],['claimed','running'],['queued','queued'],['retrying','queued'],['failed','blocked'],['done','not_verified']]){f.observed={task:ack.task_id,runs:[{state:native}],questions:[]};assert.equal(f.api.get(ack.task_id).state,state);}
});
test('questions are task scoped and answers use 1-based options',()=>{
  const f=fixture(), ack=f.api.start(f.request);f.observed={task:ack.task_id,runs:[],questions:[{id:4,title:'选一个',options:['一','二']}]};
  assert.equal(f.api.get(ack.task_id).state,'awaiting_input');
  for(const patch of [{card_id:99,option:1},{card_id:4,option:0},{card_id:4,option:3}])assert.throws(()=>f.api.answer({task_id:ack.task_id,...patch}));
  f.api.answer({task_id:ack.task_id,card_id:4,option:2});assert.deepEqual(f.calls.find(a=>a[0]==='answer'),['answer','4','2']);
});
test('host verification is checked against configured commands and artifact bytes; queries do not write',()=>{
  const f=fixture(),ack=f.api.start(f.request),folder=path.join(f.workspace,'.autoloop/tasks',ack.task_id);fs.mkdirSync(folder,{recursive:true});
  const body='evidence',sha=crypto.createHash('sha256').update(body).digest('hex');fs.writeFileSync(path.join(folder,'implementation.md'),body);
  f.observed={task:ack.task_id,state:'verified',failures:[],evidence:[{argv:f.request.checks[0],exitCode:0,output:'passed'}],artifacts:[{file:'implementation.md',sha256:sha}]};
  fs.writeFileSync(path.join(folder,'result.json'),JSON.stringify(f.observed));
  const before=fs.readdirSync(path.join(f.home,'.homebase/capabilities/development',ack.task_id));
  assert.equal(f.api.get(ack.task_id).state,'verified');assert.deepEqual(fs.readdirSync(path.join(f.home,'.homebase/capabilities/development',ack.task_id)),before);
  fs.writeFileSync(path.join(folder,'implementation.md'),'changed');assert.throws(()=>f.api.get(ack.task_id),/ARTIFACT_CHANGED/);
});
test('without host checks cannot forge verified',()=>{
  const f=fixture(),ack=f.api.start({...f.request,checks:[]}),folder=path.join(f.workspace,'.autoloop/tasks',ack.task_id);fs.mkdirSync(folder,{recursive:true});
  f.observed={task:ack.task_id,state:'verified',failures:[],evidence:[],artifacts:[]};fs.writeFileSync(path.join(folder,'result.json'),JSON.stringify(f.observed));assert.throws(()=>f.api.get(ack.task_id),/RESULT_INVALID/);
  f.observed.state='needs_acceptance';fs.writeFileSync(path.join(folder,'result.json'),JSON.stringify(f.observed));assert.equal(f.api.get(ack.task_id).state,'needs_acceptance');
});
test('reject other task results and invalid identities',()=>{
  const f=fixture(),ack=f.api.start(f.request);f.observed={task:'different',runs:[],questions:[]};assert.throws(()=>f.api.get(ack.task_id),/IDENTITY_MISMATCH/);assert.throws(()=>f.api.get('../other'),/TASK_INVALID/);
});
