import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {sandbox} from './repo-capabilities-process.mjs';
import {dispatch} from './repo-capabilities.mjs';
import {digest} from './repo-capabilities-store.mjs';
const receipt=process.env.HOMEBASE_REPO_RECEIPT;assert(receipt&&path.isAbsolute(receipt));
const proof=JSON.parse(fs.readFileSync(receipt));const root=path.join(path.dirname(receipt),`sandbox-${crypto.randomUUID()}`);const packageRoot=path.join(root,'package');fs.mkdirSync(packageRoot,{recursive:true});
const code=`import fs from 'node:fs';import net from 'node:net';
const outside=['/mnt/c','/root','/home/agent/.npmrc'].map(p=>fs.existsSync(p));
let writeDenied=false;try{fs.writeFileSync('/package/changed.txt','denied');}catch{writeDenied=true;}
fs.writeFileSync('/workspace/own.txt','owned');
const network=await new Promise(resolve=>{const s=net.connect({host:'1.1.1.1',port:443});s.setTimeout(500);s.on('connect',()=>{s.destroy();resolve(true)});s.on('error',()=>resolve(false));s.on('timeout',()=>{s.destroy();resolve(false)});});
console.log(JSON.stringify({outside,writeDenied,network,secret:process.env.HB_SYNTHETIC_KEY||null}));`;
fs.writeFileSync(path.join(packageRoot,'probe.mjs'),code);fs.writeFileSync(path.join(packageRoot,'wait.mjs'),'setInterval(()=>{},1000);');
process.env.HB_SYNTHETIC_KEY='SYNTHETIC_SENTINEL_ONLY';
let isolated;try{isolated=await sandbox(proof.context,packageRoot,path.join(root,'work'),'/package/probe.mjs','');}finally{delete process.env.HB_SYNTHETIC_KEY;}
assert.equal(isolated.status,'succeeded',JSON.stringify(isolated));const observed=JSON.parse(isolated.stdout);assert.deepEqual(observed,{outside:[false,false,false],writeDenied:true,network:false,secret:null});
const timeout=await sandbox(proof.context,packageRoot,path.join(root,'timeout'),'/package/wait.mjs','x'.repeat(65000),[],{timeout:1200});assert.equal(timeout.status,'timed_out');
const controller=new AbortController();const timer=setTimeout(()=>controller.abort(),1800);try{const canceled=await sandbox(proof.context,packageRoot,path.join(root,'cancel'),'/package/wait.mjs','',[],{signal:controller.signal});assert.equal(canceled.status,'canceled');}finally{clearTimeout(timer);}
// Result/code tampering is tested only against this already isolated fixture.
const description=await dispatch('describe',proof.context,{capability_id:proof.capability_id});assert.equal(description.verification.passed,true);
const first=await dispatch('result',proof.context,{run_id:proof.run_id});const original=fs.readFileSync(first.artifact.path);
try{fs.appendFileSync(first.artifact.path,'tampered');await assert.rejects(dispatch('result',proof.context,{run_id:proof.run_id}),{code:'RESULT_CHANGED'});}finally{fs.writeFileSync(first.artifact.path,original);}
const project=await dispatch('get',proof.context,{project_id:proof.project_id});const manifest=JSON.parse(fs.readFileSync(project.project.manifest_path));const extra=path.join(manifest.package_root,'extra.mjs');
try{fs.writeFileSync(extra,'throw Error("must never run")');await assert.rejects(dispatch('run',proof.context,{capability_id:proof.capability_id,input:{stdin:'# denied'},request_id:'tamper-denied'}),{code:'CAPABILITY_CHANGED'});}finally{fs.unlinkSync(extra);}
fs.writeFileSync(path.join(root,'validation.json'),JSON.stringify({passed:true,observed,timeout:timeout.status,cancellation:'canceled',tamper_rejected:true,live_code_or_credentials_read:false},null,2));console.log(JSON.stringify({passed:true,root}));
