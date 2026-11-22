import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {dispatch} from './repo-capabilities.mjs';
import {store,digest,write} from './repo-capabilities-store.mjs';

function fixture(){
  const root=fs.mkdtempSync(path.join(process.env.HOMEBASE_FACTS_LAB||os.tmpdir(),'repo-evidence-'));
  const context={data_root:path.join(root,'registry'),output_root:path.join(root,'output')},s=store(context,true);
  const base=path.join(s.root,'capabilities','cap_fixture'),pkg=path.join(base,'package');fs.mkdirSync(pkg,{recursive:true});
  const code='process.stdout.write("SYNTHETIC FIXTURE ONLY");\n';fs.writeFileSync(path.join(pkg,'cli.mjs'),code);
  const manifest={version:1,driver:'npm-stdin-cli-v1',name:'合成文档能力',description:'Synthetic only',source:{repository:'https://github.com/test/fixture',commit:'a'.repeat(40),package:'fixture-cli',package_version:'1.2.3',license:'MIT'},package_root:pkg,entry:'/package/cli.mjs',bin:'fixture',input_schema:{type:'object',properties:{stdin:{type:'string'}},required:['stdin'],additionalProperties:false},output_schema:{type:'string'},limits:{timeout_ms:1000,stdin_bytes:4096,stdout_bytes:4096,network:'isolated',environment:'empty'},examples:[{input:{stdin:'fixture'},contains:['FIXTURE']}],files:{'cli.mjs':digest(Buffer.from(code))}};
  const proof={version:1,passed:true,checked_at:'2026-11-16T02:30:00+08:00',manifest_sha256:digest(manifest),help_sha256:'b'.repeat(64),cli_version:'fixture 1.2.3',examples:[{index:0,status:'succeeded',exit_code:0,stdout_sha256:'c'.repeat(64),elapsed_ms:1}],invalid_input_rejected_before_execution:true,sandbox:'wsl-bubblewrap',network:'isolated'};
  const cap={id:'cap_fixture',name:manifest.name,description:manifest.description,repository:manifest.source.repository,version:'1.2.3',commit:manifest.source.commit,license:'MIT',manifest_path:path.join(base,'capability.json'),manifest_sha256:digest(manifest),verification_path:path.join(base,'verification.json'),verification_sha256:digest(proof),registered_at:'2026-11-16T02:30:01+08:00',project_id:'project_fixture',state:'verified'};
  write(cap.manifest_path,manifest);write(cap.verification_path,proof);s.insertCapability(cap);s.close();
  return {root,context,cap,manifest,proof,pkg};
}
const query=f=>dispatch('describe',f.context,{capability_id:f.cap.id});
const update=(f,cap)=>{const s=store(f.context,true);s.insertCapability({...cap,id:cap.id+'_copy'});s.close();};

test('repository describe binds the exact checked verification bytes and stays passive',async()=>{
  const f=fixture(),before=new Map([f.cap.manifest_path,f.cap.verification_path,path.join(f.context.data_root,'registry.sqlite'),path.join(f.pkg,'cli.mjs')].map(file=>[file,digest(fs.readFileSync(file))]));
  const result=await query(f);assert.equal(result.verification.passed,true);assert.equal(result.provenance.state,'records_checked');assert.equal(result.provenance.manifest_sha256,f.cap.manifest_sha256);assert.equal(result.provenance.verification_sha256,f.cap.verification_sha256);assert.equal(result.provenance.checked_at,f.proof.checked_at);assert.equal(result.provenance.code_integrity_checked,false);
  const listed=await dispatch('capabilities',f.context,{});assert.equal(listed.entries[0].provenance.state,'registry_recorded');assert.equal(listed.entries[0].provenance.checked_at,null);
  for(const[file,hash]of before)assert.equal(digest(fs.readFileSync(file)),hash);
});

test('changed verification cannot be shown as accepted evidence or reach execution',async()=>{
  const f=fixture();write(f.cap.verification_path,{...f.proof,checked_at:'2099-01-01T00:00:00Z'});
  await assert.rejects(query(f),{code:'CAPABILITY_CHANGED'});
  await assert.rejects(dispatch('run',f.context,{capability_id:f.cap.id,input:{stdin:'fixture'},request_id:'never-execute'}),{code:'CAPABILITY_CHANGED'});
});

test('registered source aliases cannot borrow a different manifest or proof',async()=>{
  const f=fixture();update(f,{...f.cap,repository:'https://github.com/test/other'});
  await assert.rejects(dispatch('describe',f.context,{capability_id:f.cap.id+'_copy'}),{code:'CAPABILITY_CHANGED'});
});

test('a changed installed file is not scanned by describe and is rejected before run',async()=>{
  const f=fixture();fs.writeFileSync(path.join(f.pkg,'cli.mjs'),'SYNTHETIC CHANGE');
  assert.equal((await query(f)).provenance.code_integrity_checked,false);
  await assert.rejects(dispatch('run',f.context,{capability_id:f.cap.id,input:{stdin:'fixture'},request_id:'code-change'}),{code:'CAPABILITY_CHANGED'});
});

test('malformed or incomplete verification is explicit even with a matching registry digest',async()=>{
  for(const mutate of [()=>null,p=>({...p,version:2}),p=>({...p,checked_at:'unknown'}),p=>({...p,examples:[null]}),p=>({...p,examples:[]}),p=>({...p,network:'allowed'})]){
    const f=fixture(),proof=mutate(f.proof);write(f.cap.verification_path,proof);update(f,{...f.cap,verification_sha256:digest(proof)});
    await assert.rejects(dispatch('describe',f.context,{capability_id:f.cap.id+'_copy'}),{code:'CAPABILITY_CHANGED'});
  }
});

test('foreign metadata paths are rejected without exposing or editing their content',async()=>{
  const f=fixture(),foreign=path.join(f.root,'foreign.json');fs.writeFileSync(foreign,JSON.stringify(f.manifest));update(f,{...f.cap,manifest_path:foreign});
  await assert.rejects(dispatch('describe',f.context,{capability_id:f.cap.id+'_copy'}),{code:'CAPABILITY_CHANGED'});assert.equal(digest(JSON.parse(fs.readFileSync(foreign))),f.cap.manifest_sha256);
});

test('a result and a saved receipt cannot return another original run identity',async()=>{
  const f=fixture(),s=store(f.context,true),runId='run_fixture',body={capability_id:f.cap.id,input:{stdin:'fixture'}};
  write(s.file('runs',runId),{id:'run_other',capability_id:f.cap.id,request_id:'original-request',status:'succeeded'});
  write(s.file('requests',digest('run:original-request')),{kind:'run',request_id:'original-request',fingerprint:digest(body),run_id:runId});s.close();
  await assert.rejects(dispatch('result',f.context,{run_id:runId}),{code:'RUN_IDENTITY_INVALID'});
  await assert.rejects(dispatch('run',f.context,{...body,request_id:'original-request'}),{code:'RUN_IDENTITY_INVALID'});
  write(path.join(f.context.data_root,'runs',runId+'.json'),{id:runId,capability_id:f.cap.id,request_id:'foreign-request',status:'succeeded'});
  await assert.rejects(dispatch('run',f.context,{...body,request_id:'original-request'}),{code:'RUN_IDENTITY_INVALID'});
});

test('valid original receipts still resolve when current installation evidence changes',async()=>{
  const f=fixture(),s=store(f.context,true),runId='run_prior',body={capability_id:f.cap.id,input:{stdin:'fixture'}};
  write(s.file('runs',runId),{id:runId,capability_id:f.cap.id,request_id:'prior-request',status:'unknown'});write(s.file('requests',digest('run:prior-request')),{kind:'run',request_id:'prior-request',fingerprint:digest(body),run_id:runId});s.close();
  write(f.cap.verification_path,{...f.proof,passed:false});const before=digest(fs.readFileSync(path.join(f.context.data_root,'runs',runId+'.json')));
  const result=await dispatch('run',f.context,{...body,request_id:'prior-request'});assert.equal(result.run_id,runId);assert.equal(result.status,'unknown');assert.equal(digest(fs.readFileSync(path.join(f.context.data_root,'runs',runId+'.json'))),before);
});

function resultFixture(f,runId,text){
  const root=path.join(f.context.output_root,runId);fs.mkdirSync(root,{recursive:true});const file=path.join(root,'result.txt');fs.writeFileSync(file,text);
  const record={id:runId,capability_id:f.cap.id,request_id:'artifact-request',status:'succeeded',stdout:text.slice(0,32768),stdout_truncated:text.length>32768,empty_output:!text.length,artifact:{path:file,bytes:Buffer.byteLength(text),sha256:digest(text),mime_type:'text/plain'}};
  write(path.join(f.context.data_root,'runs',runId+'.json'),record);return {file,record};
}

test('result attachments stay inside the original run, not a sibling run',async()=>{
  const f=fixture(),a=resultFixture(f,'run_a','original'),b=resultFixture(f,'run_b','SIBLING FIXTURE');write(path.join(f.context.data_root,'runs/run_a.json'),{...a.record,artifact:b.record.artifact});
  await assert.rejects(dispatch('result',f.context,{run_id:'run_a'}),{code:'RESULT_CHANGED'});
});

test('oversized artifacts are rejected before a whole-file read',async()=>{
  const f=fixture(),a=resultFixture(f,'run_large','small');fs.truncateSync(a.file,2*1024*1024);
  const original=fs.readFileSync;fs.readFileSync=function(file,...args){if(file===a.file)throw Object.assign(Error('Unexpected unbounded artifact read'),{code:'UNBOUNDED_READ'});return original.call(this,file,...args);};
  try{await assert.rejects(dispatch('result',f.context,{run_id:'run_large'}),{code:'RESULT_CHANGED'});}finally{fs.readFileSync=original;}
});

test('the checked result file supplies a valid Unicode preview without rewriting legacy records',async()=>{
  const f=fixture(),text='a'.repeat(32767)+'🙂结果',a=resultFixture(f,'run_unicode',text),file=path.join(f.context.data_root,'runs/run_unicode.json'),before=digest(fs.readFileSync(file));
  const result=await dispatch('result',f.context,{run_id:'run_unicode'});assert.equal(result.stdout,'a'.repeat(32767));assert.equal(result.stdout_truncated,true);assert.equal(result.empty_output,false);assert.equal(digest(fs.readFileSync(file)),before);
  write(file,{...a.record,stdout:'OUTDATED SYNTHETIC PREVIEW'});assert.equal((await dispatch('result',f.context,{run_id:'run_unicode'})).stdout,result.stdout);
});
