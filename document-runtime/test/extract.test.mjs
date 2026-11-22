import {test} from 'node:test';import assert from 'node:assert/strict';
import fs from 'node:fs';import path from 'node:path';import {fileURLToPath} from 'node:url';import {spawnSync} from 'node:child_process';import {createHash} from 'node:crypto';
import {pdfFixture,request} from './fixtures.mjs';
const root=fileURLToPath(new URL('../',import.meta.url));
const runtime=process.env.HOMEBASE_DOCUMENT_RUNTIME||root;
const lab=process.env.HOMEBASE_DOCUMENT_LAB;
assert(lab&&path.isAbsolute(lab),'Use an explicit isolated document test root');
fs.mkdirSync(lab,{recursive:true});const home=fs.mkdtempSync(path.join(lab,'parser-'));
const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>['systemroot','windir','comspec'].includes(k.toLowerCase())));
Object.assign(env,{HOME:home,USERPROFILE:home,TEMP:home,TMP:home});
const node=process.env.HOMEBASE_DOCUMENT_NODE||process.execPath;
function run(value){
  const child=spawnSync(node,['--permission',`--allow-fs-read=${runtime}`,'--allow-addons','--max-old-space-size=192',path.join(runtime,'worker.mjs')],{cwd:home,env,input:typeof value==='string'?value:JSON.stringify(value),encoding:'utf8',windowsHide:true,timeout:20000,maxBuffer:2*1024*1024});
  assert(!child.error,child.error?.code);assert.equal(child.status,0,child.stderr);
  const lines=child.stdout.trim().split('\n');assert.equal(lines.length,1,'Only one JSON response is allowed');
  const result=JSON.parse(lines[0]);assert.equal(result.version,'homebase-document-v1');return result;
}
test('real pinned parser extracts Chinese, ASCII and surrogate-pair emoji and hashes original source bytes',()=>{
  const bytes=pdfFixture(['0001 0002 0003','0048 0065 006C 006C 006F']);
  const before=Buffer.from(bytes),result=run(request(bytes));
  assert.equal(result.status,'ready');assert.equal(result.pages[0].text,'中文🙂');assert.equal(result.pages[1].text,'Hello');
  assert.equal(result.pages[0].bytes,10);assert.equal(result.text_bytes,15);assert.equal(result.partial,false);
  assert.equal(result.source_sha256,createHash('sha256').update(bytes).digest('hex'));assert.deepEqual(bytes,before);
});
test('empty and mixed pages are explicit and never called OCR or complete visual understanding',()=>{
  const empty=run(request(pdfFixture([null])));assert.equal(empty.status,'no_text');assert.equal(empty.code,'NO_TEXT');assert.deepEqual(empty.missing_text_pages,[1]);assert(empty.message.includes('未进行OCR'));
  const mixed=run(request(pdfFixture(['0001 0002',null])));assert.equal(mixed.status,'ready');assert.equal(mixed.partial,true);assert.deepEqual(mixed.missing_text_pages,[2]);assert.equal(mixed.pages[1].text,'');
});
test('large page count asks for an explicit page range and a selected subset remains marked partial',()=>{
  const bytes=pdfFixture(Array(41).fill('0041'));
  const all=run(request(bytes));assert.equal(all.status,'page_selection_required');assert.equal(all.total_pages,41);assert.equal(all.pages,undefined);
  const selected=run(request(bytes,{from:40,to:41}));assert.equal(selected.status,'ready');assert.deepEqual(selected.pages.map(p=>p.number),[40,41]);assert.equal(selected.partial,true);
});
test('invalid ranges, request URLs and path-shaped fields cannot become document resource access',()=>{
  const bytes=pdfFixture(['0041']);
  for(const pages of [{from:0,to:1},{from:1,to:2},{from:1.5,to:1.5},{from:1,to:41},{from:1,to:1,url:'https://invalid.example/'}]){
    const result=run(request(bytes,pages));assert.equal(result.status,'failed');assert.equal(result.code,'INVALID_RANGE');
  }
  for(const extra of [{url:'https://invalid.example/private.pdf'},{path:'C:/private.txt'},{password:'synthetic-only'}]){
    assert.equal(run({...request(bytes),...extra}).code,'INVALID_REQUEST');
  }
});
test('encrypted and malformed PDFs produce fixed diagnostic codes without private stack or payload',()=>{
  assert.equal(run(request(pdfFixture(['0041'],{encrypted:true}))).code,'PASSWORD_REQUIRED');
  const bad=run(request(Buffer.from('%PDF-1.7\nSYNTHETIC_PRIVATE_PARSE_ERROR_BODY')));assert.equal(bad.code,'INVALID_PDF');assert(!JSON.stringify(bad).includes('SYNTHETIC_PRIVATE_PARSE_ERROR_BODY'));assert.equal(bad.pages,undefined);
  assert.equal(run(request(Buffer.from('wrong file header'))).code,'INVALID_PDF');
});
test('document actions are not executed while ordinary text remains available',()=>{
  const result=run(request(pdfFixture(['0041'],{action:true})));assert.equal(result.status,'ready');assert.equal(result.pages[0].text,'A');
});
test('text and source limits reject without silently truncating the document',()=>{
  const text=run(request(pdfFixture(['0001'.repeat(90000)],{wide:true})));assert.equal(text.code,'TEXT_LIMIT');assert.equal(text.pages,undefined);
  assert.equal(run(request(Buffer.alloc(4*1024*1024+1))).code,'SOURCE_LIMIT');
  assert.equal(run({...request(pdfFixture()),bytes_base64:'@@@'}).code,'INVALID_REQUEST');
  assert.equal(run(request(Buffer.alloc(0))).code,'SOURCE_LIMIT');
});
test('transport bounds and malformed JSON are explicit failures',()=>{
  assert.equal(run('{invalid JSON').code,'INVALID_REQUEST');
  const large=' '.repeat(Math.ceil(4*1024*1024/3)*4+8192);assert.equal(run(large).code,'FRAME_LIMIT');
});
test('Node file and process restrictions are exercised independently of PDF parser behavior',()=>{
  const outside=path.join(home,'private-synthetic.txt');fs.writeFileSync(outside,'synthetic-only');
  const code=`import fs from 'node:fs';import {spawnSync} from 'node:child_process';let denied=0;for(const action of [()=>fs.readFileSync(${JSON.stringify(outside)}),()=>fs.writeFileSync(${JSON.stringify(outside)},'changed'),()=>spawnSync(process.execPath,['--version'])]){try{const r=action();if(r?.error?.code==='ERR_ACCESS_DENIED')denied++;}catch(e){if(e.code==='ERR_ACCESS_DENIED')denied++;}}console.log(denied);`;
  const r=spawnSync(node,['--permission',`--allow-fs-read=${runtime}`,'--allow-addons','--input-type=module','-e',code],{env,cwd:home,encoding:'utf8',windowsHide:true,timeout:5000});
  assert.equal(r.status,0);assert.equal(r.stdout.trim(),'3');assert.equal(fs.readFileSync(outside,'utf8'),'synthetic-only');
});
