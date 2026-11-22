// Native-client fault fixtures. Only fresh homes below an explicitly named lab.
// Never import a real profile, key, or global Agent configuration.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {spawnSync} from 'node:child_process';
export const protectedNativeTest=process.env.HOMEBASE_NATIVE_RECEIPTS_TEST==='1';
const hash=data=>crypto.createHash('sha256').update(data).digest('hex');
export const nativeScope=r=>`native:${r.provider}:${r.thread_id}`;
const message=(provider,thread_id,text,request_id)=>({provider,thread_id,text,request_id});
function assertHome(home){
  const root=process.env.HOMEBASE_NATIVE_RECEIPT_TEST_ROOT;
  assert(root&&path.isAbsolute(root)&&path.isAbsolute(home),'Explicit absolute native receipt lab required');
  assert(path.resolve(home).startsWith(path.resolve(root)+path.sep),'No real profile may be used');
  assert(/^(native-flow|profile)-[a-zA-Z0-9]+$/.test(path.basename(home)),'A fresh test home is required');
}
function codec(home,file,action,value){
  assertHome(home);
  assert(path.resolve(file).startsWith(path.resolve(home,'.homebase')+path.sep));
  const script=`$ErrorActionPreference='Stop'
[Console]::InputEncoding=[Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
$inputData=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String([Console]::In.ReadToEnd().Trim()))|ConvertFrom-Json
$file=[IO.Path]::GetFullPath($inputData.file)
$root=[IO.Path]::GetFullPath($inputData.home).TrimEnd([IO.Path]::DirectorySeparatorChar)+[IO.Path]::DirectorySeparatorChar
if(-not $file.StartsWith($root,[StringComparison]::OrdinalIgnoreCase)){throw 'Outside isolated home'}
Add-Type -AssemblyName System.Security
if($inputData.action -eq 'read'){
  $plain=[Security.Cryptography.ProtectedData]::Unprotect([IO.File]::ReadAllBytes($file),$null,[Security.Cryptography.DataProtectionScope]::CurrentUser)
  [Console]::Write([Convert]::ToBase64String($plain))
}elseif($inputData.action -eq 'write'){
  $plain=[Text.Encoding]::UTF8.GetBytes($inputData.json)
  $cipher=[Security.Cryptography.ProtectedData]::Protect($plain,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser)
  [IO.File]::WriteAllBytes($file,$cipher)
}else{throw 'Unsupported fixture action'}`;
  const payload=Buffer.from(JSON.stringify({home,file,action,json:JSON.stringify(value)}),'utf8').toString('base64');
  const r=spawnSync('pwsh',['-NoProfile','-Command',script],{input:payload,encoding:'utf8',windowsHide:true,timeout:15000,maxBuffer:8*1024*1024});
  assert(!r.error&&r.status===0,`Isolated DPAPI fixture failed: ${r.error?.code??r.status}; ${r.stderr.slice(0,1800)}`);
  return action==='read'?JSON.parse(Buffer.from(r.stdout.trim(),'base64').toString('utf8')):null;
}
function stopped(pid){
  assert(Number.isInteger(pid)&&pid>0);
  try{process.kill(pid,0);}catch(e){assert.equal(e.code,'ESRCH');return;}
  throw Error('Stop the owned test host before injecting a client checkpoint');
}
function files(home,request){
  assertHome(home);
  return {client:path.join(home,'.homebase/assistant/desktop',hash(nativeScope(request))+'.draft.dpapi'),
    server:path.join(home,'.homebase/conversations',request.provider==='codex'?'receipts.dpapi':'providers.dpapi')};
}
export async function protectedSend(call,provider,thread,text,requestId){
  const request=message(provider,thread,text,requestId),scope=nativeScope(request);
  const current=(await call('assistant_desktop_get',{scope})).entry;
  const saved=await call('assistant_desktop_save',{request:{scope,revision:current.revision,body:{text,route:'',mode:'auto',files:[]}}});
  const result=await call('assistant_native_send',{request,revision:saved.entry.revision});
  assert.equal(result.native_result?.state,'accepted');assert.equal(result.entry.native_pending,null);assert.equal(result.entry.body.text,'');
  const duplicate=await call('assistant_native_send',{request,revision:0});
  assert.equal(duplicate.native_result.turn.id,result.native_result.turn.id);
  return {request,ack:result.native_result};
}
export function replayPreAckClient(home,request,pid,newBody=''){
  stopped(pid);const f=files(home,request),entry=codec(home,f.client,'read');
  entry.native_pending=request;entry.body.text=newBody||request.text;entry.revision++;
  codec(home,f.client,'write',entry);
  return {request,body:newBody,server_sha256:hash(fs.readFileSync(f.server))};
}
export async function verifyPreAckRecovery(call,home,fixture,ack){
  const {request}=fixture,f=files(home,request),scope=nativeScope(request);
  const loaded=(await call('assistant_desktop_get',{scope})).entry;
  assert.deepEqual(loaded.native_pending,request);
  const result=await call('assistant_native_resolve',{scope});
  assert.equal(result.native_result?.state,'accepted');assert.equal(result.native_result.turn.id,ack.turn.id);
  assert.equal(result.entry.native_pending,null);assert.equal(result.entry.body.text,fixture.body);
  const duplicate=await call('assistant_native_send',{request,revision:0});assert.equal(duplicate.native_result.turn.id,ack.turn.id);
  assert.equal(hash(fs.readFileSync(f.server)),fixture.server_sha256,'Client recovery must not rewrite native execution receipts');
}
export function seedUnknownNative(home,provider,thread,pid){
  stopped(pid);const request=message(provider,thread,'仅合成的未知原生请求',`unknown-fixture-${provider}`),f=files(home,request);
  const entry=codec(home,f.client,'read');entry.native_pending=request;entry.body.text=request.text;entry.revision++;codec(home,f.client,'write',entry);
  const server=codec(home,f.server,'read');
  const wire={action:'send',thread_id:request.thread_id,text:request.text,request_id:request.request_id,cursor:null};
  if(provider!=='codex')Object.assign(wire,{provider,credential_ref:null});
  const key=provider==='codex'?request.request_id:`${provider}:${request.request_id}`;
  server.receipts[key]={fingerprint:hash(JSON.stringify(wire)),result:{state:'unknown',thread_id:thread,message:'Synthetic interrupted acknowledgement'}};
  codec(home,f.server,'write',server);
  return {request,server_sha256:hash(fs.readFileSync(f.server)),client_sha256:hash(fs.readFileSync(f.client))};
}
export async function verifyUnknownNative(call,home,fixture){
  const {request}=fixture,f=files(home,request),scope=nativeScope(request);
  const resolved=await call('assistant_native_resolve',{scope});assert.equal(resolved.native_result?.state,'unknown');
  const repeated=await call('assistant_native_send',{request,revision:0});assert.equal(repeated.native_result?.state,'unknown');
  assert.deepEqual(repeated.entry.native_pending,request);
  assert.equal(hash(fs.readFileSync(f.server)),fixture.server_sha256);
  assert.equal(hash(fs.readFileSync(f.client)),fixture.client_sha256);
}
export function sealNativeFixtures(home,fixtures){
  for(const fixture of fixtures){
    const f=files(home,fixture.request);
    fixture.server_sha256=hash(fs.readFileSync(f.server));
    if(fixture.client_sha256)fixture.client_sha256=hash(fs.readFileSync(f.client));
  }
  return fixtures;
}
export async function verifyNativeLinkFault(call,home,request,mode){
  assert.equal(request.provider,'codex');
  const f=files(home,request),before=fs.readFileSync(f.server);
  try{
    if(mode==='unavailable')fs.writeFileSync(f.server,Buffer.from('SYNTHETIC-CORRUPT-LINK-METADATA'));
    else{
      assert.equal(mode,'not_linked');const ledger=codec(home,f.server,'read');
      delete ledger.links[request.thread_id];codec(home,f.server,'write',ledger);
    }
    const fault=hash(fs.readFileSync(f.server));
    const result=await call('conversation_native',{request:{action:'read',provider:'codex',thread_id:request.thread_id}});
    assert.equal(result.context_link_state,mode);assert.equal(result.assistant_session_id,null);
    assert(result.thread.turns.length>0,'Local link failure must not hide readable SDK history');
    if(mode==='unavailable'){assert.equal(result.status,'partial');assert(result.context_link_message);}
    assert.equal(hash(fs.readFileSync(f.server)),fault,'Read must not rewrite or repair binding metadata');
  }finally{fs.writeFileSync(f.server,before);}
  assert.equal(hash(fs.readFileSync(f.server)),hash(before));
}
