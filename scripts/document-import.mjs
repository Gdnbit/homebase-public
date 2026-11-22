import fs from 'node:fs/promises';
import path from 'node:path';
import crypto from 'node:crypto';
export class DocumentImportError extends Error {}

// Only explicit caller-selected bytes cross the public command boundary.
// The CLI path is never transmitted to HomeBase or included in errors/results.
export async function importPdf({filename,scope='new',documentId,call,onIdentity=()=>{}}) {
  if(!filename || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(documentId||'') || !(scope==='new'||/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(scope))) throw new DocumentImportError('PDF编号或作用域无效；使用稳定UUID，scope为new或已有会话UUID');
  const name=path.basename(filename);
  if(!/\.pdf$/i.test(name)||Buffer.byteLength(name)>180||/[\x00-\x1f\x7f/\\:]/.test(name)) throw new DocumentImportError('请选择名称至多180字节的PDF文件');
  const file=await fs.open(filename,'r');
  let bytes;
  try {
    const stat=await file.stat();
    if(!stat.isFile()||stat.size<1||stat.size>4*1024*1024)throw new DocumentImportError('PDF须为非空普通文件，且至多4 MiB');
    const buffer=Buffer.alloc(4*1024*1024+1);
    let size=0;
    while(size<buffer.length){const result=await file.read(buffer,size,buffer.length-size,null);if(!result.bytesRead)break;size+=result.bytesRead;}
    if(size!==stat.size||size>4*1024*1024)throw new DocumentImportError('读取期间文件大小变化，未导入；请重新选择稳定文件');
    bytes=buffer.subarray(0,size);
  } finally {await file.close();}
  const source_sha256=crypto.createHash('sha256').update(bytes).digest('hex');
  onIdentity({document_id:documentId,source_sha256,source_bytes:bytes.length});
  try {
    const chunk=256*1024;
    const part=async(index)=>call('assistant_document_import',{request:{scope,document_id:documentId,name,source_sha256,source_bytes:bytes.length,part:index,data_base64:bytes.subarray(index*chunk,Math.min((index+1)*chunk,bytes.length)).toString('base64')}});
    const status=await part(0);
    if(status.document_id!==documentId||status.source_sha256!==source_sha256||status.source_bytes!==bytes.length||!Array.isArray(status.missing))throw new DocumentImportError('导入回执身份不完整，未继续提交');
    const missing=status.missing;
    if(missing.some((v,i)=>!Number.isInteger(v)||v<1||v>=Math.ceil(bytes.length/chunk)||missing.indexOf(v)!==i))throw new DocumentImportError('导入缺失块回执无效，未继续提交');
    for(const index of missing)await part(index);
    const document=await call('assistant_document_import_commit',{scope,documentId});
    if(document.id!==documentId||document.source_sha256!==source_sha256||document.source_bytes!==bytes.length)throw new DocumentImportError('提交回执与原PDF不一致；保留原编号查询，不重新解析');
    return {document,parsed:false,message:'受保护来源已提交；尚未解析或发送给模型。用原document_id调用assistant_document_extract。'};
  } finally {bytes.fill(0);}
}
