import {test} from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {fileOperation} from '../local/files.mjs';

async function fixture() {
  const parent=process.env.HOMEBASE_LOCAL_TEST_LAB || path.join(os.tmpdir(),'homebase-local-synthetic');
  await fs.mkdir(parent,{recursive:true});
  const root=await fs.mkdtemp(path.join(parent,'files-'));
  const workspace=path.join(root,'workspace');await fs.mkdir(workspace);
  return {root,workspace,context:{default_workspace:'fixture',workspaces:[{id:'fixture',path:workspace,access:'write'}],protected_paths:[],output_root:path.join(root,'outputs')}};
}
test('new text, BOM/CRLF exact edits and conflict detection preserve original bytes',async()=>{
  const f=await fixture();const p=path.join(f.workspace,'notes.md');
  await fs.writeFile(p,Buffer.concat([Buffer.from([239,187,191]),Buffer.from('第一行\r\n旧内容\r\n')]));
  const read=await fileOperation(f.context,{action:'read',path:'notes.md'});
  assert.equal(read.encoding,'utf-8-bom');assert.equal(read.newline,'crlf');
  const edited=await fileOperation(f.context,{action:'edit',path:'notes.md',expected_sha256:read.sha256,old_text:'旧内容\n',new_text:'新内容\n'});
  assert.equal(edited.status,'succeeded');
  assert.deepEqual(await fs.readFile(p),Buffer.concat([Buffer.from([239,187,191]),Buffer.from('第一行\r\n新内容\r\n')]));
  await assert.rejects(fileOperation(f.context,{action:'edit',path:'notes.md',expected_sha256:read.sha256,old_text:'新内容',new_text:'不能覆盖'}),e=>e.code==='FILE_CONFLICT');
  await assert.rejects(fileOperation(f.context,{action:'write',path:'notes.md',content:'不能覆盖'}),e=>e.code==='EEXIST');
  await fs.writeFile(path.join(f.workspace,'legacy.txt'),Buffer.from([255,254,65,0]));
  await assert.rejects(fileOperation(f.context,{action:'read',path:'legacy.txt'}),e=>['BINARY_FILE','ENCODING_UNSUPPORTED'].includes(e.code));
});
test('traversal, junctions, hardlinks, secret paths and read-only writes are rejected',async()=>{
  const f=await fixture();const outside=path.join(f.root,'outside');await fs.mkdir(outside);await fs.writeFile(path.join(outside,'sentinel.txt'),'SYNTHETIC ONLY');
  await fs.symlink(outside,path.join(f.workspace,'junction'),process.platform==='win32'?'junction':'dir');
  await fs.link(path.join(outside,'sentinel.txt'),path.join(f.workspace,'hardlink.txt'));
  for(const p of ['../outside/sentinel.txt','junction/sentinel.txt','hardlink.txt','.env','secrets.yaml','CON.txt'])await assert.rejects(fileOperation(f.context,{action:'read',path:p}));
  const readOnly={...f.context,workspaces:f.context.workspaces.map(w=>({...w,access:'read'}))};
  await assert.rejects(fileOperation(readOnly,{action:'write',path:'blocked.txt',content:'x'}),e=>e.code==='WORKSPACE_READ_ONLY');
  assert.equal(await fs.readFile(path.join(outside,'sentinel.txt'),'utf8'),'SYNTHETIC ONLY');
});
test('copy/move are no-clobber, trash is recoverable and export stays in owned output',async()=>{
  const f=await fixture();const write=await fileOperation(f.context,{action:'write',path:'fixture.txt',content:'独立测试'});
  await fileOperation(f.context,{action:'copy',path:'fixture.txt',destination:'copy.txt'});
  await assert.rejects(fileOperation(f.context,{action:'move',path:'fixture.txt',destination:'copy.txt'}),e=>e.code==='DESTINATION_INVALID');
  await fileOperation(f.context,{action:'move',path:'copy.txt',destination:'moved.txt'});
  const trash=await fileOperation(f.context,{action:'trash',path:'moved.txt'});assert.equal(trash.recoverable,true);
  assert.equal(await fs.readFile(path.join(f.workspace,trash.destination),'utf8'),'独立测试');
  const exported=await fileOperation(f.context,{action:'export',path:'fixture.txt'});assert.equal(exported.artifact.sha256,write.sha256);
  assert.equal(await fs.readFile(exported.artifact.path,'utf8'),'独立测试');
});
test('search reports bounded matches without traversing protected or linked directories',async()=>{
  const f=await fixture();await fs.writeFile(path.join(f.workspace,'normal.txt'),'synthetic needle\nsecond line');
  await fs.writeFile(path.join(f.workspace,'.env'),'synthetic needle hidden');
  const result=await fileOperation(f.context,{action:'search',query:'needle',content:true});
  assert.equal(result.matches.length,1);assert.equal(result.matches[0].path,'normal.txt');assert.equal(result.matches[0].line,1);
  const read=await fileOperation(f.context,{action:'read',path:'normal.txt',offset:1,limit:1});assert.equal(read.content,'second line');
});
test('one long Chinese line is completely readable through explicit character cursors',async()=>{
  const f=await fixture();const text='隔离分页'.repeat(1400);await fs.writeFile(path.join(f.workspace,'long.txt'),text);
  let request={action:'read',path:'long.txt',limit:1},output='';let pages=0;
  do{const result=await fileOperation(f.context,request);assert(result.content.length<=2000);output+=result.content;pages++;request=result.next?{...request,...result.next}:null;}while(request);
  assert.equal(output,text);assert(pages>1);
});
