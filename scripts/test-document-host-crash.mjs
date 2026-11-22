// Real host termination, owned synthetic source/worker only. No real app PID.
import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import assert from 'node:assert/strict';import {spawn,spawnSync} from 'node:child_process';
const parent=process.env.HOMEBASE_DOCUMENT_CRASH_LAB,binary=process.env.HOMEBASE_DOCUMENT_CRASH_BIN;
assert(parent&&path.isAbsolute(parent)&&binary&&path.isAbsolute(binary));
fs.mkdirSync(parent,{recursive:true});const home=fs.mkdtempSync(path.join(parent,'document-crash-'));fs.mkdirSync(path.join(home,'runtime'));
fs.writeFileSync(path.join(home,'runtime/worker.mjs'),'while(true){};');
const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>['systemroot','windir','comspec','path','pathext'].includes(k.toLowerCase())));
Object.assign(env,{HOMEBASE_DOCUMENT_CRASH_HOME:home,HOMEBASE_DOCUMENT_NODE:process.execPath,HOME:home,USERPROFILE:home,TEMP:home,TMP:home});
const host=spawn(binary,[],{env,cwd:home,windowsHide:true,stdio:'ignore'});let childPid;
const sleep=ms=>new Promise(r=>setTimeout(r,ms));const alive=pid=>{try{process.kill(pid,0);return true;}catch(e){assert.equal(e.code,'ESRCH');return false;}};
const digest=file=>crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
try{
  for(let i=0;i<30&&!childPid;i++){
    assert.equal(host.exitCode,null,'Test host exited before crash injection');
    const p=spawnSync('pwsh',['-NoProfile','-Command',`Get-CimInstance Win32_Process -Filter "ParentProcessId = ${host.pid}" | Select-Object ProcessId,ExecutablePath | ConvertTo-Json -Compress`],{env,windowsHide:true,encoding:'utf8',timeout:10000});
    assert.equal(p.status,0);const values=p.stdout.trim()?JSON.parse(p.stdout):[];
    const found=(Array.isArray(values)?values:[values]).filter(v=>v.ExecutablePath&&path.resolve(v.ExecutablePath).toLowerCase()===path.resolve(process.execPath).toLowerCase());
    assert(found.length<=1);if(found.length)childPid=found[0].ProcessId;else await sleep(100);
  }
  assert(childPid&&alive(childPid),'Owned child must be confirmed live before stopping its host');
  await sleep(250);assert(alive(childPid));
  const source=path.join(home,'private-documents/75c8874c-109f-4c59-a445-ad1c7077eb0b/source.dpapi'),before=digest(source);
  host.kill();for(let i=0;i<100&&alive(childPid);i++)await sleep(50);
  assert(!alive(childPid),'Parser survived its host; duplicate recovery must not proceed');
  const inspect=spawnSync(binary,['--inspect'],{env,cwd:home,windowsHide:true,encoding:'utf8',timeout:10000});assert.equal(inspect.status,0);
  const receipt=JSON.parse(inspect.stdout);assert.equal(receipt.phase,'unknown');assert.equal(receipt.code,'PROCESS_UNCONFIRMED');assert.equal(digest(source),before);
  const report={passed:true,home,host_pid:host.pid,child_pid:childPid,confirmed_live_before_host_kill:true,child_stopped_on_host_kill:true,restarted_receipt_phase:receipt.phase,source_ciphertext_preserved:true,real_model_calls:0,real_wechat_messages:0};
  fs.writeFileSync(path.join(home,'validation.json'),JSON.stringify(report,null,2),{flag:'wx'});console.log(JSON.stringify(report));
}finally{if(host.exitCode===null)host.kill();if(childPid&&alive(childPid))process.kill(childPid);}
