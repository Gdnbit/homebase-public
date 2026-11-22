import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {spawn} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import readline from 'node:readline';
import crypto from 'node:crypto';
import {executionFailure,LocalError} from '../local/policy.mjs';

test('uncertain execution failures remain unknown and never echo private errors',()=>{
  for(const code of ['EXECUTION_UNKNOWN','MCP_CALL_UNKNOWN']) {
    const result=executionFailure(Object.assign(new Error('SYNTHETIC_PRIVATE_BODY'),{code}));
    assert.equal(result.status,'unknown');assert.equal(result.code,code);assert(!JSON.stringify(result).includes('SYNTHETIC_PRIVATE_BODY'));
  }
  assert.equal(executionFailure(new LocalError('PATH_INVALID','synthetic validation')).status,'failed');
});

test('native-host RPC deduplicates effects, rejects changed IDs, reports failures and closes cleanly',async()=>{
  const root=await fs.mkdtemp(path.join(process.env.HOMEBASE_LOCAL_TEST_LAB||os.tmpdir(),'host-'));
  const child=spawn(process.execPath,[process.env.HOMEBASE_AGENT_LOCAL_HOST||fileURLToPath(new URL('../local/host.mjs',import.meta.url))],{stdio:['pipe','pipe','pipe'],windowsHide:true});
  let stderr='';child.stderr.on('data',chunk=>stderr+=chunk);
  const messages=[],waiters=[];
  const lines=readline.createInterface({input:child.stdout});
  lines.on('line',line=>{const value=JSON.parse(line);messages.push(value);for(const wake of waiters.splice(0))wake();});
  const wait=async predicate=>{
    const deadline=Date.now()+15000;
    while(!messages.some(predicate)){assert(Date.now()<deadline,'host response timed out');await Promise.race([new Promise(resolve=>waiters.push(resolve)),new Promise(resolve=>setTimeout(resolve,100))]);}
    return messages.find(predicate);
  };
  const context={workspaces:[{id:'test',path:root,access:'write'}],default_workspace:'test',output_root:path.join(root,'output')};
  const request={id:'write-once',kind:'execute',operation:'files',context,request:{action:'write',path:'hello.txt',content:'只创建一次'}};
  const send=message=>child.stdin.write(`${JSON.stringify(message)}\n`);
  try{
    send(request);assert.equal((await wait(m=>m.id===request.id&&m.kind==='result')).result.status,'succeeded');
    send(request);assert.equal((await wait(m=>m.id===request.id&&m.reused&&m.kind==='result')).result.status,'succeeded');
    send({...request,request:{...request.request,content:'不得覆盖'}});
    await wait(m=>m.id===request.id&&m.kind==='protocol_error');
    assert.equal(await fs.readFile(path.join(root,'hello.txt'),'utf8'),'只创建一次');
    send({...request,id:'denied',request:{action:'read',path:'../outside'}});
    const denied=await wait(m=>m.id==='denied'&&m.kind==='result');assert.equal(denied.result.status,'failed');assert.equal(denied.result.code,'PATH_INVALID');
    send({id:request.id,kind:'status'});assert.equal((await wait(m=>m.id===request.id&&m.kind==='status')).state,'finished');
    send({id:'before-start',kind:'cancel'});await wait(m=>m.id==='before-start'&&m.kind==='cancel_ack');
    send({...request,id:'before-start',request:{action:'write',path:'never.txt',content:'must not run'}});
    assert.equal((await wait(m=>m.id==='before-start'&&m.kind==='result')).result.status,'canceled');
    await assert.rejects(fs.stat(path.join(root,'never.txt')),error=>error.code==='ENOENT');
    send({id:'connections-reset',kind:'reset_connections'});assert.equal((await wait(m=>m.id==='connections-reset'&&m.kind==='result')).result.status,'succeeded');
    if(process.platform==='win32') {
      const executable=path.join(root,'fake-bwrap.bin');await fs.writeFile(executable,'SYNTHETIC EXECUTOR HASH ONLY');
      const hash=crypto.createHash('sha256').update(await fs.readFile(executable)).digest('hex');
      const uncertain={id:'unknown-once',kind:'execute',operation:'terminal',context:{...context,sandbox:{kind:'wsl-bubblewrap',executable,sha256:hash,wsl_command:process.execPath}},request:{command:'synthetic-not-executed',timeout_ms:1000}};
      send(uncertain);const result=await wait(m=>m.id===uncertain.id&&m.kind==='result');assert.equal(result.result.status,'unknown');assert.equal(result.result.code,'EXECUTION_UNKNOWN');
      send(uncertain);await wait(m=>m.id===uncertain.id&&m.kind==='result'&&m.reused===true);
      assert.equal(messages.filter(m=>m.id===uncertain.id&&m.kind==='accepted').length,1);
    }
    for(let i=0;i<257;i++){const id=`bounded-read-${i}`;send({...request,id,request:{action:'stat',path:'hello.txt'}});await wait(m=>m.id===id&&m.kind==='result');}
    send(request);await wait(m=>m.id===request.id&&m.result?.code==='HOST_RESULT_EXPIRED');
    assert.equal(await fs.readFile(path.join(root,'hello.txt'),'utf8'),'只创建一次');
    child.stdin.end();const exit=await new Promise(resolve=>child.on('exit',resolve));assert.equal(exit,0,stderr);assert.equal(stderr,'');
  }finally{child.kill();}
});
