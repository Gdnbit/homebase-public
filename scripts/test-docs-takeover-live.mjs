// Opt-in live test. Fresh isolated runtime, synthetic content, no credentials or existing business files.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {fileURLToPath} from 'node:url';
const execute=promisify(execFile);
if(!process.argv.includes('--isolated-only'))throw Error('Requires --isolated-only');
const project=fileURLToPath(new URL('../',import.meta.url));
const lab=path.join('F:\\HomeBase-Capability-Lab',`docs-takeover-${new Date().toISOString().replace(/[:.]/g,'-')}-${crypto.randomUUID().slice(0,8)}`);
fs.mkdirSync(lab,{recursive:true});
const capsule=path.join(lab,'client');
for(const file of ['AGENTS.md','hb.cmd','control/START.md','control/OPERATIONS.md','control/reference.md','control/commands.json','control/bootstrap.template.txt','scripts/homebase.mjs','scripts/control-bootstrap.mjs','scripts/control-result.mjs','scripts/control-validation.mjs']){
  const target=path.join(capsule,file);fs.mkdirSync(path.dirname(target),{recursive:true});fs.copyFileSync(path.join(project,file),target);
}
assert.equal(fs.existsSync(path.join(capsule,'src-tauri')),false);
// Validation helper belongs to the published CLI, not business implementation.
const {validateSchema}=await import(new URL(`file:///${path.join(capsule,'scripts/control-validation.mjs').replaceAll('\\','/')}`));
const cliFile=path.join(capsule,'scripts/homebase.mjs');
const save=(name,value)=>{const file=path.join(lab,name);fs.writeFileSync(file,typeof value==='string'?value:JSON.stringify(value,null,2),'utf8');return file;};
save('boundary.json',{capsule,sourcePresent:false,scope:'fresh marked capability runtime; synthetic two-second video; no credential or existing business operations'});
async function raw(args){
  try{return (await execute(process.execPath,[cliFile,...args],{cwd:capsule,windowsHide:true,timeout:35000,maxBuffer:4*1024*1024})).stdout;}
  catch(error){if(error.stdout)return error.stdout;throw Error('CLI transport failed; inspect saved job IDs before retry');}
}
const cli=async args=>JSON.parse(await raw(args));
save('startup-prompt.txt',await raw(['bootstrap']));
const doctor=await cli(['doctor']);assert.equal(doctor.ok,true);save('doctor.json',doctor);
save('commands.json',await cli(['commands']));
const contracts={};
for(const name of ['capabilities_catalog','capability_install','capability_render','capability_result']){
  const value=await cli(['describe',name]);assert.equal(value.ok,true);contracts[name]=value.command;save(`${name}-describe.json`,value);
}
const flags=['--allow-write','--allow-external','--allow-private'];
async function call(name,args,detached=false){
  const input=save(`${name}-input.json`,args);
  const valid=await cli(['validate',name,'--input',input]);assert.equal(valid.executed,false);assert.equal(valid.valid,true);save(`${name}-validated.json`,valid);
  const request=`docs-${name}-${crypto.randomUUID()}`;
  let value=await cli(['call',name,...flags,'--request-id',request,'--input',input,...(detached?['--detach']:[])]);
  save(`${name}-submitted.json`,value);assert.equal(value.ok,true);
  const id=value.job.id;console.log(JSON.stringify({step:name,job_id:id}));
  for(let i=0;i<600 && (detached || ['running','queued'].includes(value.job.status));i++){
    value=await cli(['jobs','get',id,...flags]);
    if(!['running','queued'].includes(value.job.status))break;
    if(i%30===0)console.log(JSON.stringify({step:name,status:value.job.status}));
    await new Promise(resolve=>setTimeout(resolve,1000));
  }
  save(`${name}-completed.json`,value);assert.equal(value.ok,true);assert.equal(value.job.status,'succeeded');
  const result=value.job.result;
  const schema=contracts[name].output_schema;
  assert.deepEqual(validateSchema(schema,result,schema.$defs),[],`${name} output contract mismatch`);
  return {job_id:id,result};
}
const runtime=path.join(lab,'runtime');
const before=await call('capabilities_catalog',{rootPath:runtime});
assert.equal(before.result.state,'not_installed');
const installed=await call('capability_install',{rootPath:runtime},true);
const rendered=await call('capability_render',{rootPath:runtime,input:{title:'接口接管验证',subtitle:'仅使用启动提示词和接口文档',seconds:2}},true);
const result=await call('capability_result',{rootPath:runtime,runId:rendered.result.run_id});
assert.equal(result.result.status,'succeeded');
const artifact=result.result.artifact;
assert.ok(path.resolve(artifact.path).startsWith(path.resolve(runtime)+path.sep));
const bytes=fs.readFileSync(artifact.path);assert.equal(bytes.length,artifact.bytes);
assert.equal(crypto.createHash('sha256').update(bytes).digest('hex'),artifact.sha256);
const report={at:new Date().toISOString(),lab,capsule,sourcePresent:false,catalog_hash:doctor.catalog_hash,installed,rendered,result,isolated:true};
save('verified.json',report);fs.writeFileSync(path.join(project,'.tmp/docs-takeover-lab.txt'),lab,'utf8');
console.log(JSON.stringify({verified:true,lab,artifact:artifact.path,sha256:artifact.sha256}));
