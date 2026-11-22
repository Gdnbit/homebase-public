import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import {importPdf} from './document-import.mjs';
test('PDF CLI sends bounded bytes, resumes missing parts and never transmits local paths',async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'hb-doc-cli-'));
  const filename=path.join(root,'仅合成.pdf');
  const bytes=Buffer.alloc(4*1024*1024,42);await fs.writeFile(filename,bytes);
  const documentId=crypto.randomUUID();const calls=[];
  const result=await importPdf({filename,documentId,call:async(name,args)=>{
    calls.push({name,args});assert(Buffer.byteLength(JSON.stringify(args))<1024*1024-4096);
    assert(!JSON.stringify(args).includes(root));
    if(name==='assistant_document_import')return {document_id:documentId,source_sha256:crypto.createHash('sha256').update(bytes).digest('hex'),source_bytes:bytes.length,missing:[2,15]};
    assert.equal(name,'assistant_document_import_commit');
    return {id:documentId,source_sha256:crypto.createHash('sha256').update(bytes).digest('hex'),source_bytes:bytes.length};
  }});
  assert.deepEqual(calls.filter(c=>c.name==='assistant_document_import').map(c=>c.args.request.part),[0,2,15]);
  assert.equal(result.parsed,false);
});
test('PDF CLI stops on unconfirmed chunk and never commits or extracts',async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'hb-doc-cli-'));
  const filename=path.join(root,'synthetic.pdf');await fs.writeFile(filename,Buffer.alloc(300000,42));
  const calls=[];
  await assert.rejects(importPdf({filename,documentId:crypto.randomUUID(),call:async(name)=>{calls.push(name);throw Error('synthetic disconnect');}}));
  assert.deepEqual(calls,['assistant_document_import']);
});
test('PDF CLI rejects oversized files or invalid identity before any call',async()=>{
  const root=await fs.mkdtemp(path.join(os.tmpdir(),'hb-doc-cli-'));
  const filename=path.join(root,'synthetic.pdf');await fs.writeFile(filename,Buffer.alloc(4*1024*1024+1,42));
  let calls=0;const call=async()=>{calls++;};
  await assert.rejects(importPdf({filename,documentId:crypto.randomUUID(),call}));
  await assert.rejects(importPdf({filename,documentId:'../foreign',call}));
  assert.equal(calls,0);
});
