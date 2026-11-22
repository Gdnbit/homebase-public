import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import readline from 'node:readline';
import {fileURLToPath} from 'node:url';
import {store,read,write,inside,noLinks,digest,fail,id,text,fields,locked,page,alive} from './repo-capabilities-store.mjs';
import {analyzeSource,publicJson,repository,searchRepositories} from './repo-capabilities-analysis.mjs';
import {processRun,npmCli,probeSandbox,sandbox} from './repo-capabilities-process.mjs';
import {validateSchema} from './control-validation.mjs';

const active=new Set(['analyzing','preflighting','installing','wrapping','verifying','registering','running']);
const time=()=>new Date().toISOString();
const unique=prefix=>`${prefix}_${crypto.randomUUID()}`;
function summary(p){return {id:p.id,repository:p.source?.repository?.url||p.repository,name:p.source?.package?.name||p.name||p.repository,version:p.source?.package?.version||null,phase:p.phase,revision:p.revision,capability_id:p.capability_id||null,error:p.error||null,updated_at:p.updated_at,operation:p.operation};}
function observed(v){return active.has(v.phase||v.status)&&v.operation&&!alive(v.operation.pid)?{...v,observed_state:'interrupted_unknown',notice:'原工作进程已退出，结果未确认；先核对原操作，不自动重跑。'}:v;}
function projectView(p){const source=p.source?structuredClone(p.source):null;if(source){source.readme_preview=source.readme_excerpt?.slice(0,1500)||'';delete source.readme_excerpt;}return observed({...p,source});}
function installCheckView(s, p) {
  if (active.has(p.phase)) return p;
  const check = s.projectInstallCheck(p.id);
  if (!check || check.revision !== p.revision) return p;
  const lock = read(inside(s.root,'locks',digest(`resource:install:${p.id}`)+'.lock'),null);
  if (lock?.operation!==check.operation_id || lock.pid!==check.pid) return p;
  const view = observed({ ...p, phase:'preflighting', error:null, operation:{request_id:check.operation_id,pid:check.pid,child_pid:null},
    notice:check.stage==='canceled'?'已请求取消，正在结束隔离检查。':'正在检查隔离环境，准备下载。' });
  if (view.observed_state==='interrupted_unknown' && check.stage!=='admitted') {
    view.notice='隔离检查进程已退出，下载尚未开始。请先核对这次检查。';
  }
  return view;
}
function project(s,key){return read(s.file('projects',key));}
function saveProject(s,p){p.updated_at=time();p.revision++;write(s.file('projects',p.id),p);s.putProject(summary(p));}
function resultProject(p){return {status:['registered','analyzed','needs_adapter'].includes(p.phase)?'succeeded':active.has(p.phase)?'running':p.phase==='canceled'?'canceled':'failed',project:projectView(p),registered:p.phase==='registered'};}
function request(s,kind,key,body){try{id(key);const old=read(s.file('requests',digest(`${kind}:${key}`)),null);if(old&&old.fingerprint!==digest(body))fail('REQUEST_CONFLICT','原请求编号对应不同参数，未执行');return old;}catch(e){s.close();throw e;}}
function accept(s,kind,key,body,extra){write(s.file('requests',digest(`${kind}:${id(key)}`)),{kind,request_id:key,fingerprint:digest(body),...extra});}
function canceled(s,key){return fs.existsSync(s.file('cancellations',key));}
function controller(s,key,shouldCancel=()=>canceled(s,key)){const abort=new AbortController();const timer=setInterval(()=>{try{if(shouldCancel())abort.abort();}catch{abort.abort();}},200);return {signal:abort.signal,close:()=>clearInterval(timer)};}
function canceledInstallCheck(p) {
  return {status:'canceled',project:projectView(p),registered:false,message:'已取消隔离检查，下载尚未开始。'};
}
function failure(e){const safe=e.homebase||['CANCELED','PROCESS_START_FAILED','PROCESS_LIMIT','INSTALL_FAILED','SANDBOX_START_FAILED','EXECUTION_UNKNOWN'].includes(e.code);return {code:safe?e.code:'CAPABILITY_FAILED',message:safe?e.message:'能力操作未完成，原状态与结果保留；未回显环境或原始异常'};}
function examples(value,schema){if(!Array.isArray(value)||value.length<1||value.length>8)fail('ACCEPTANCE_REQUIRED','需要1至8条明确的输入与预期输出验收用例');for(const v of value){fields(v,['input','contains']);if(validateSchema(schema,v.input).length)fail('ACCEPTANCE_INVALID','验收输入不符合接口结构');if(!Array.isArray(v.contains)||!v.contains.length||v.contains.length>8||v.contains.some(c=>typeof c!=='string'||!c.length||Buffer.byteLength(c)>2000))fail('ACCEPTANCE_INVALID','每条验收须有明确的预期包含文本');}return value;}
function files(root){const output={};let size=0,count=0;const visit=dir=>{for(const e of fs.readdirSync(dir,{withFileTypes:true})){const file=inside(root,path.relative(root,path.join(dir,e.name)));const stat=fs.lstatSync(file);if(stat.isSymbolicLink()||stat.nlink>1&&!stat.isDirectory())fail('PACKAGE_LINK_DENIED','安装内容含链接，未执行');if(++count>4096)fail('PACKAGE_LIMIT','安装文件数超过上限');if(stat.isDirectory())visit(file);else if(stat.isFile()){size+=stat.size;if(size>128*1024*1024)fail('PACKAGE_LIMIT','安装内容超过128MiB上限');output[path.relative(root,file).replaceAll('\\','/')]=digest(fs.readFileSync(file));}else fail('PACKAGE_FILE_INVALID','安装含非普通文件');}};visit(root);return output;}
function provenance(cap,proof=null){
  return {state:proof?'records_checked':'registry_recorded',source_commit:cap.commit,package_version:cap.version,manifest_sha256:cap.manifest_sha256,verification_sha256:cap.verification_sha256,registered_at:cap.registered_at,checked_at:proof?.checked_at||null,code_integrity_checked:false};
}
function capabilityEntries(s) {
  return s.index().capabilities.map(({manifest_path,manifest_sha256,verification_path,...entry})=>({
    ...entry, provenance:provenance({...entry,manifest_sha256}),
  }));
}
function managedPath(s,file){
  if(typeof file!=='string'||!path.isAbsolute(file))fail('CAPABILITY_CHANGED','受管理的接口或验收路径无效，未读取其他位置');
  const relative=path.relative(s.root,file);
  if(!relative||relative==='..'||relative.startsWith('..'+path.sep)||path.isAbsolute(relative))fail('CAPABILITY_CHANGED','接口或验收依据超出原注册目录，未读取其他位置');
  return inside(s.root,relative);
}
function readEvidence(s,cap){
  const manifest=read(managedPath(s,cap.manifest_path)),proof=read(managedPath(s,cap.verification_path));
  if(!manifest||typeof manifest!=='object'||Array.isArray(manifest)||!proof||typeof proof!=='object'||Array.isArray(proof))fail('CAPABILITY_CHANGED','接口或验收记录结构无效，未视为通过');
  const source=manifest.source;
  if(cap.state!=='verified'||manifest.version!==1||manifest.driver!=='npm-stdin-cli-v1'||proof.version!==1||proof.passed!==true||digest(manifest)!==cap.manifest_sha256||proof.manifest_sha256!==cap.manifest_sha256||cap.verification_sha256!==digest(proof))fail('CAPABILITY_CHANGED','接口或验收记录摘要变化，未展示为有效依据或运行');
  if(!source||source.repository!==cap.repository||source.commit!==cap.commit||source.package_version!==cap.version||manifest.name!==cap.name||digest(source.license??null)!==digest(cap.license??null))fail('CAPABILITY_CHANGED','验收记录与仓库、commit或发布版本绑定不符，未借用其他来源的依据');
  if(typeof proof.checked_at!=='string'||proof.checked_at.length>40||!/^\d{4}-\d{2}-\d{2}T/.test(proof.checked_at)||!Number.isFinite(Date.parse(proof.checked_at))||proof.invalid_input_rejected_before_execution!==true||proof.network!=='isolated'||proof.sandbox!=='wsl-bubblewrap'||manifest.limits?.network!=='isolated'||!Array.isArray(proof.examples)||!Array.isArray(manifest.examples)||proof.examples.length!==manifest.examples.length||!proof.examples.length||proof.examples.some((v,i)=>!v||v.index!==i||v.status!=='succeeded'||v.exit_code!==0||! /^[a-f0-9]{64}$/.test(v.stdout_sha256)))fail('CAPABILITY_CHANGED','验收记录缺少有效时间、隔离范围或通过的原用例，未视为通过');
  managedPath(s,manifest.package_root);
  return {manifest,proof};
}
function verifyManifest(s,cap){const {manifest}=readEvidence(s,cap);if(digest(files(manifest.package_root))!==digest(manifest.files))fail('CAPABILITY_CHANGED','已安装代码文件或摘要变化，未运行');return manifest;}
function preview(value,limit){let end=Math.min(value.length,limit);const last=value.charCodeAt(end-1);if(last>=0xd800&&last<=0xdbff&&(end===value.length||value.charCodeAt(end)>=0xdc00&&value.charCodeAt(end)<=0xdfff))end--;return value.slice(0,end);}
function checkedArtifact(s,key,artifact){
  if(!artifact||typeof artifact!=='object'||typeof artifact.path!=='string'||!path.isAbsolute(artifact.path))fail('RESULT_CHANGED','结果附件结构无效，未读取其他文件或重提');
  const expected=inside(s.output,key,'result.txt'),file=noLinks(artifact.path),same=process.platform==='win32'?file.toLowerCase()===expected.toLowerCase():file===expected;
  if(!same||artifact.mime_type!=='text/plain'||!Number.isSafeInteger(artifact.bytes)||artifact.bytes<0||artifact.bytes>1024*1024||! /^[a-f0-9]{64}$/.test(artifact.sha256))fail('RESULT_CHANGED','结果附件与原任务、类型或字节边界不符，未返回其他文件或重提');
  let fd;
  try{
    fd=fs.openSync(file,'r');const stat=fs.fstatSync(fd);
    if(!stat.isFile()||stat.nlink!==1||stat.size!==artifact.bytes)fail('RESULT_CHANGED','原结果文件类型、大小或链接发生变化，未读取正文');
    const buffer=Buffer.alloc(artifact.bytes+1);let length=0;
    while(length<buffer.length){const count=fs.readSync(fd,buffer,length,buffer.length-length,null);if(!count)break;length+=count;}
    const data=buffer.subarray(0,length);
    if(length!==artifact.bytes||digest(data)!==artifact.sha256)fail('RESULT_CHANGED','结果文件已变化，未返回过期内容或重新执行');
    return data.toString('utf8');
  }catch(error){if(error.homebase)throw error;fail('RESULT_UNAVAILABLE','原结果附件暂时无法读取；原任务保留，不能据此重新执行');}
  finally{if(fd!==undefined)fs.closeSync(fd);}
}
function capById(s,key){const cap=s.index().capabilities.find(c=>c.id===id(key));if(!cap)fail('CAPABILITY_NOT_FOUND','未找到已注册能力；请先查询能力列表');return cap;}
function childState(s,p){return pid=>{p.operation.child_pid=pid||null;saveProject(s,p);};}

export async function analyze(context,input,options={}){
  fields(input,['repository_url','revision','request_id','refresh']);const normalized=repository(input.repository_url),body={repository_url:normalized.url,revision:input.revision||null};const s=store(context,true);
  const old=request(s,'analyze',input.request_id,body);if(old){try{return resultProject(project(s,old.project_id));}finally{s.close();}}
  return locked(s,input.request_id,async()=>{
    const cached=!input.refresh?s.index().projects.filter(v=>v.repository===normalized.url).reverse().map(v=>project(s,v.id)).find(p=>p.source&&(p.requested_revision||null)===body.revision&&Date.now()-Date.parse(p.updated_at)<300000):null;
    if(cached){accept(s,'analyze',input.request_id,body,{project_id:cached.id});return resultProject(cached);}
    if(s.index().projects.length>=512)fail('PROJECT_LIMIT','项目数达到512，请先明确归档；未清除去重记录或覆盖原项目');
    const p={id:unique('project'),repository:normalized.url,name:`${normalized.owner}/${normalized.repo}`,requested_revision:body.revision,phase:'analyzing',revision:0,operation:{request_id:input.request_id,pid:process.pid,child_pid:null},created_at:time()};
    accept(s,'analyze',input.request_id,body,{project_id:p.id});saveProject(s,p);
    try{p.source=await analyzeSource(input,{...options,get:async url=>{if(canceled(s,input.request_id))fail('CANCELED','分析已取消，未安装或注册');const value=await (options.get||publicJson)(url);if(canceled(s,input.request_id))fail('CANCELED','分析已取消，未安装或注册');return value;}});p.phase=p.source.supported?'analyzed':'needs_adapter';p.error=null;}catch(e){p.phase=e.code==='CANCELED'?'canceled':'failed';p.error=failure(e);}saveProject(s,p);return resultProject(p);
  },`analyze:${normalized.url}`);
}
export async function install(context,input,options={}){
  fields(input,['project_id','revision','request_id','examples']);const s=store(context,true);const body={project_id:id(input.project_id),revision:input.revision,examples:input.examples||null};const old=request(s,'install',input.request_id,body);if(old){try{return resultProject(project(s,old.project_id));}finally{s.close();}}
  return locked(s,input.request_id,async()=>{
    const p=project(s,input.project_id);
    const receipt=request(s,'install',input.request_id,body);
    if (receipt) return resultProject(project(s,receipt.project_id));
    const check = s.installCheck(input.request_id);
    const fingerprint = digest(body);
    if (check) {
      if (check.fingerprint !== fingerprint) fail('REQUEST_CONFLICT','原请求编号对应不同参数，未执行');
      if (check.stage === 'canceled') return canceledInstallCheck(p);
      fail('OPERATION_BUSY','原隔离检查结果待核对，未重新执行');
    }
    if(p.revision!==input.revision)fail('PROJECT_STALE','项目状态已更新，请读取当前版本后重试');if(!p.source?.supported||active.has(p.phase))fail('PROJECT_NOT_READY','项目尚未具备可安装入口，或原操作仍在进行');
    const source=p.source;if(source.analysis_sha256!==digest(Object.fromEntries(Object.entries(source).filter(([k])=>k!=='analysis_sha256'))))fail('ANALYSIS_CHANGED','固定来源分析摘要变化，未安装');
    const tests=examples(input.examples||source.profile.examples,source.profile.input_schema);
    if(source.package.unpacked_size>128*1024*1024)fail('PACKAGE_LIMIT','发布包大小超过自动接入上限');
    s.startInstallCheck(input.request_id,p,fingerprint);
    let admitted = false;
    const abort=controller(s,input.request_id,()=>canceled(s,input.request_id)||(!admitted&&s.installCheck(input.request_id)?.stage==='canceled'));
    try{
      await (options.probeSandbox ?? probeSandbox)(context,inside(s.root,'sandbox-preflight'),{signal:abort.signal});
      if (abort.signal.aborted && s.installCheck(input.request_id)?.stage!=='canceled') fail('STATE_IO','隔离检查状态无法确认，未开始下载');
      if (!s.admitInstallCheck(input.request_id)) {
        if (s.installCheck(input.request_id)?.stage==='canceled') return canceledInstallCheck(p);
        fail('OPERATION_MISMATCH','原隔离检查状态无法核对，未开始下载');
      }
      accept(s,'install',input.request_id,body,{project_id:p.id});p.operation={request_id:input.request_id,pid:process.pid,child_pid:null};p.phase='installing';p.error=null;saveProject(s,p);
      admitted = true;
      if (canceled(s,input.request_id)) fail('CANCELED','已取消安装，下载尚未开始');
      const base=inside(s.root,'attempts',unique('install')),prefix=path.join(base,'runtime');fs.mkdirSync(prefix,{recursive:true});write(path.join(prefix,'package.json'),{name:'homebase-isolated-capability',version:'1.0.0',private:true});
      await processRun(process.execPath,[npmCli(),'install',`${source.package.name}@${source.package.version}`,'--prefix',prefix,'--ignore-scripts','--no-audit','--no-fund','--fetch-retries=2','--fetch-retry-mintimeout=1000','--fetch-retry-maxtimeout=3000','--fetch-timeout=20000'],{cwd:prefix,root:base,signal:abort.signal,onSpawn:childState(s,p)});
      const lock=read(path.join(prefix,'package-lock.json'));const installed=lock.packages?.[`node_modules/${source.package.name}`];if(installed?.version!==source.package.version||installed.integrity!==source.package.integrity)fail('PACKAGE_INTEGRITY_MISMATCH','精确版本或包完整性与分析不一致');
      for(const [name,pkg]of Object.entries(lock.packages||{})){if(!name)continue;let u;try{u=new URL(pkg.resolved);}catch{fail('DEPENDENCY_SOURCE_DENIED','依赖缺少固定来源');}if(pkg.link||u.protocol!=='https:'||u.hostname!=='registry.npmjs.org'||u.username||u.password||u.port||!/^sha512-/.test(pkg.integrity||''))fail('DEPENDENCY_SOURCE_DENIED','依赖包含未固定或非公开npm来源，未执行');}
      p.phase='wrapping';saveProject(s,p);
      const [bin,target]=source.package.bin[0];const entry=`node_modules/${source.package.name}/${target.replace(/^\.\//,'')}`;
      const packageJson=read(inside(prefix,'node_modules',...source.package.name.split('/'),'package.json'));if(packageJson.name!==source.package.name||packageJson.version!==source.package.version)fail('PACKAGE_IDENTITY_MISMATCH','已安装包身份与分析不一致');
      const manifest={version:1,driver:'npm-stdin-cli-v1',name:source.profile.name,description:source.description,source:{repository:source.repository.url,commit:source.source_commit,package:source.package.name,package_version:source.package.version,integrity:source.package.integrity,license:source.license},package_root:prefix,entry:`/package/${entry}`,bin,input_schema:source.profile.input_schema,output_schema:source.profile.output_schema,limits:{timeout_ms:20000,stdin_bytes:65536,stdout_bytes:1024*1024,network:'isolated',environment:'empty'},examples:tests,notice:source.profile.notice,files:files(prefix)};
      if(!Object.hasOwn(manifest.files,entry))fail('CLI_ENTRY_MISSING','发布包没有声明的CLI入口，未运行构建脚本');
      const manifestPath=path.join(base,'capability.json');write(manifestPath,manifest);p.manifest_path=manifestPath;p.phase='verifying';saveProject(s,p);
      const proofs=[];const execute=(stdin,args,label)=>sandbox(context,prefix,path.join(base,'checks',label),manifest.entry,stdin,args,{signal:abort.signal,onSpawn:childState(s,p)});
      const help=await execute('',['--help'],'help');if(help.status!=='succeeded'||!help.stdout.trim())fail('CLI_HELP_FAILED','CLI帮助入口未通过隔离检查');
      const version=await execute('',['--version'],'version');if(version.status!=='succeeded'||!version.stdout.includes(source.package.version))fail('CLI_VERSION_FAILED','CLI版本与固定发布版本不一致');
      for(let n=0;n<tests.length;n++){if(abort.signal.aborted)fail('CANCELED','接入已取消，未注册');const r=await execute(text(tests[n].input.stdin),[],`example-${n}`);if(r.status!=='succeeded'||!tests[n].contains.every(v=>r.stdout.includes(v)))fail('ACCEPTANCE_FAILED','实际输出未满足声明的验收用例，未注册');proofs.push({index:n,status:r.status,exit_code:r.exit_code,stdout_sha256:digest(r.stdout),elapsed_ms:r.elapsed_ms});}
      if(validateSchema(manifest.input_schema,{stdin:2}).length===0)fail('SCHEMA_INVALID','非法输入未被结构校验拒绝');
      write(path.join(base,'verification.json'),{version:1,passed:true,checked_at:time(),manifest_sha256:digest(manifest),help_sha256:digest(help.stdout),cli_version:version.stdout.trim(),examples:proofs,invalid_input_rejected_before_execution:true,sandbox:'wsl-bubblewrap',network:'isolated'});
      if(abort.signal.aborted)fail('CANCELED','接入已取消，未注册');p.phase='registering';saveProject(s,p);
      const capId=`repo.${source.repository.owner.slice(0,30)}.${source.repository.repo.slice(0,50)}.${bin.slice(0,30)}.${digest({source:source.analysis_sha256,tests,driver:manifest.driver}).slice(0,12)}`.toLowerCase();
      const idx=s.index(),existing=idx.capabilities.find(c=>c.id===capId);const cap={id:capId,name:manifest.name,description:manifest.description,repository:manifest.source.repository,version:manifest.source.package_version,commit:manifest.source.commit,license:manifest.source.license,manifest_path:manifestPath,manifest_sha256:digest(manifest),verification_path:path.join(base,'verification.json'),verification_sha256:digest(read(path.join(base,'verification.json'))),registered_at:time(),project_id:p.id,state:'verified'};
      if(!existing&&idx.capabilities.length>=512)fail('CAPABILITY_LIMIT','能力数达到512，未覆盖或驱逐原登记');
      if(!existing){if(!s.insertCapability(cap))verifyManifest(s,capById(s,capId));}else verifyManifest(s,existing);
      p.capability_id=capId;p.phase='registered';p.verification={passed:true,tests:proofs.length,at:time(),network:'isolated'};p.error=null;saveProject(s,p);return resultProject(p);
    }catch(e){
      if (!admitted) {
        if (s.installCheck(input.request_id)?.stage==='canceled') return canceledInstallCheck(p);
        throw e;
      }
      p.phase=e.code==='CANCELED'?'canceled':e.code==='EXECUTION_UNKNOWN'?'interrupted_unknown':'failed';p.error=failure(e);saveProject(s,p);return resultProject(p);
    }finally{abort.close();s.finishInstallCheck(input.request_id,process.pid);}
  },`install:${input.project_id}`);
}
export async function run(context,input){
  fields(input,['capability_id','input','request_id']);const s=store(context,true);const key=id(input.request_id),body={capability_id:id(input.capability_id),input:input.input};const prior=request(s,'run',key,body);if(prior){try{return runResult(s,prior.run_id,{capability_id:body.capability_id,request_id:key});}finally{s.close();}}
  return locked(s,key,async()=>{
    const cap=capById(s,input.capability_id),manifest=verifyManifest(s,cap);if(validateSchema(manifest.input_schema,input.input).length)fail('CAPABILITY_INPUT_INVALID','输入不符合已注册接口，未执行');text(input.input.stdin,manifest.limits.stdin_bytes);
    const runId=unique('run');const root=inside(s.output,runId);fs.mkdirSync(path.join(root,'work'),{recursive:true});const r={id:runId,capability_id:cap.id,request_id:key,status:'running',operation:{request_id:key,pid:process.pid,child_pid:null},created_at:time()};
    accept(s,'run',key,body,{run_id:runId});write(s.file('runs',runId),r);const abort=controller(s,key);
    try{
      const output=await sandbox(context,manifest.package_root,path.join(root,'work'),manifest.entry,input.input.stdin,[],{signal:abort.signal,timeout:manifest.limits.timeout_ms,onSpawn:pid=>{r.operation.child_pid=pid;write(s.file('runs',runId),r);}});
      r.execution={status:output.status,exit_code:output.exit_code,elapsed_ms:output.elapsed_ms,network:output.network};
      if(output.status==='succeeded'){
        const artifact=path.join(root,'result.txt');fs.writeFileSync(artifact,output.stdout,{flag:'wx',mode:0o600});r.status='succeeded';r.stdout=preview(output.stdout,32768);r.stdout_truncated=r.stdout.length<output.stdout.length;r.empty_output=!output.stdout.length;r.artifact={path:artifact,bytes:Buffer.byteLength(output.stdout),sha256:digest(output.stdout),mime_type:'text/plain'};r.message=r.empty_output?'执行成功，输出为空。':r.stdout_truncated?'执行成功，正文较长；stdout为预览，完整输出在已校验的结果附件中。':'执行成功，输出按文本返回，未执行其中内容。';
      }else {r.status=output.status==='canceled'?'canceled':output.status==='unknown'?'unknown':'failed';r.error={code:output.status==='timed_out'?'CAPABILITY_TIMEOUT':'CAPABILITY_EXECUTION_FAILED',message:'隔离CLI未成功完成；原任务与输出目录保留，不自动重跑'};r.stderr=preview(output.stderr,2000);}
    }catch(e){r.status=e.code==='EXECUTION_UNKNOWN'?'unknown':e.code==='CANCELED'?'canceled':'failed';r.error=failure(e);}finally{abort.close();r.finished_at=time();write(s.file('runs',runId),r);}
    return runResult(s,runId);
  });
}
function runResult(s,key,expected=null){
  const r=observed(read(s.file('runs',key)));
  if(!r||r.id!==key||expected&&(r.capability_id!==expected.capability_id||r.request_id!==expected.request_id))fail('RUN_IDENTITY_INVALID','结果与原运行编号或请求归属不符，未返回其他任务或重提');
  if(r.artifact){const text=checkedArtifact(s,key,r.artifact);r.stdout=preview(text,32768);r.stdout_truncated=r.stdout.length<text.length;r.empty_output=!text.length;}
  if(typeof r.stderr==='string')r.stderr=preview(r.stderr,2000);
  return {...r,run_id:r.id,status:r.observed_state||r.status};
}
export async function dispatch(action,context,input,options={}){
  if(action==='search'){fields(input,['query']);return searchRepositories(input.query,options);}
  if(action==='analyze')return analyze(context,input,options);if(action==='install')return install(context,input,options);if(action==='run')return run(context,input);
  const s=store(context,false);
  try{
  if(action==='projects'){
    fields(input,['query','offset','limit']);
    const result=page(s.index().projects.map(observed),input.query||'',input.offset||0,input.limit||6);
    return {status:'succeeded',...result,entries:result.entries.map(p=>installCheckView(s,p))};
  }
  if(action==='get'){fields(input,['project_id']);return {status:'succeeded',project:projectView(installCheckView(s,project(s,input.project_id)))};}
  if(action==='capabilities'){fields(input,['query','offset','limit']);return {status:'succeeded',...page(capabilityEntries(s),input.query||'',input.offset||0,input.limit||6)};}
  if(action==='capability-index'){
    fields(input,[]);
    const entries=capabilityEntries(s);
    if(entries.length>512)fail('CAPABILITY_LIMIT','仓库目录超过512项，未视为完整目录');
    return {status:'succeeded',entries};
  }
  if(action==='describe'){fields(input,['capability_id']);const cap=capById(s,input.capability_id),{manifest:m,proof}=readEvidence(s,cap);return {status:'succeeded',capability:{id:cap.id,name:cap.name,description:cap.description,version:cap.version,repository:cap.repository,commit:cap.commit,license:cap.license,input_schema:m.input_schema,output_schema:m.output_schema,examples:m.examples,limits:m.limits,notice:m.notice},verification:proof,provenance:provenance(cap,proof)};}
  if(action==='result'){fields(input,['run_id']);return runResult(s,input.run_id);}
  if(action==='cancel'){
    fields(input,['project_id','run_id','operation_id']);if(Boolean(input.project_id)===Boolean(input.run_id))fail('INPUT_INVALID','指定一个项目或运行任务');const target=input.project_id?project(s,input.project_id):read(s.file('runs',input.run_id));
    const operation=id(input.operation_id);
    const check=input.project_id ? s.installCheck(operation) : null;
    const matchingCheck=check?.project_id===target.id && (check.revision===target.revision || target.operation?.request_id===operation || check.stage==='canceled');
    if (matchingCheck && check.stage==='canceled') return {status:'succeeded',state:check.pid===null?'already_finished':'cancel_requested'};
    if (matchingCheck && alive(check.pid)) {
      const writable=store(context,true);
      try {
        if (writable.cancelInstallCheck(operation,target.id,check.pid)) return {status:'succeeded',state:'cancel_requested'};
        const latest=writable.installCheck(operation);
        if (latest?.stage==='canceled') return {status:'succeeded',state:latest.pid===null?'already_finished':'cancel_requested'};
        if (latest?.stage!=='admitted') return {status:'succeeded',state:'already_finished'};
      } finally {writable.close();}
    } else {
      if(target.operation?.request_id!==operation)fail('OPERATION_MISMATCH','操作身份已变化，未取消其他任务');
      if(!active.has(target.phase||target.status))return {status:'succeeded',state:'already_finished'};
    }
    write(s.file('cancellations',operation),{at:time()});return {status:'succeeded',state:'cancel_requested'};
  }
  if(action==='reconcile'){
    fields(input,['operation_id']);const wanted=id(input.operation_id);const locks=inside(s.root,'locks');const matches=fs.existsSync(locks)?fs.readdirSync(locks).filter(n=>/^[a-f0-9]{64}\.lock$/.test(n)).map(n=>({file:inside(locks,n),lock:read(inside(locks,n))})).filter(v=>v.lock.operation===wanted):[];if(!matches.length||matches.some(v=>v.lock.pid!==matches[0].lock.pid))fail('OPERATION_MISMATCH','没有一致匹配的中断锁，未修改其他操作');const {lock}=matches[0];
    const records=['install','analyze','run'].map(kind=>read(s.file('requests',digest(`${kind}:${lock.operation}`)),null)).filter(Boolean);const targets=records.map(r=>r.run_id?read(s.file('runs',r.run_id)):project(s,r.project_id));
    if(alive(lock.pid)||targets.some(p=>alive(p.operation.child_pid)))fail('OPERATION_BUSY','原工作进程仍存在，不能解除锁或重复执行');
    const originalCheck = s.installCheck(wanted);
    const preflightInterrupted = originalCheck?.pid===lock.pid && targets.length===0;
    if (originalCheck?.pid===lock.pid) {
      const writable=store(context,true);
      try { writable.finishInstallCheck(wanted,lock.pid); } finally { writable.close(); }
    }
    for(const {file}of matches)fs.unlinkSync(file);for(const p of targets){if(p.phase){p.phase='interrupted_unknown';p.error={code:'INTERRUPTED_UNKNOWN',message:'已确认旧进程退出，原执行结果未知；未自动重跑。'};s.close();const writable=store(context,true);try{saveProject(writable,p);}finally{writable.close();}}else{p.status='unknown';write(s.file('runs',p.id),p);}}
    return {status:'succeeded',state:preflightInterrupted?'preflight_interrupted':'interrupted_unknown',automatic_retry:false};
  }
  fail('ACTION_INVALID','能力操作未注册');
  }finally{s.close();}
}

if(process.argv[1]&&path.resolve(process.argv[1])===fileURLToPath(import.meta.url)){
  const lines=readline.createInterface({input:process.stdin});let used=false;
  lines.on('line',async line=>{if(used)return;used=true;try{if(Buffer.byteLength(line)>256*1024)fail('INPUT_LIMIT','请求过大');const {context,request}=JSON.parse(line);const value=await dispatch(process.argv[2],context,request||{});process.stdout.write(JSON.stringify(value)+'\n');process.exit(0);}catch(e){process.stdout.write(JSON.stringify({status:'failed',error:failure(e)})+'\n');process.exitCode=1;process.exit();}});
}
