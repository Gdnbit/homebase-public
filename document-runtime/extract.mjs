// Local bytes only. Text extraction does not execute document actions, render
// pages, perform OCR, or infer that an empty page has been visually understood.
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
export const MAX_SOURCE_BYTES=4*1024*1024;
export const MAX_TEXT_BYTES=256*1024;
export const MAX_PAGES=40;
const messages={
  INVALID_REQUEST:'文档请求格式无效，未解析。',
  SOURCE_LIMIT:'文档为空或超过4 MiB，请选择较小文件。',
  INVALID_PDF:'PDF损坏或格式不支持，未按空文档处理。',
  PASSWORD_REQUIRED:'PDF已加密；请在本机自行解除加密后重新选择，不要在聊天中提供密码。',
  INVALID_RANGE:'页范围无效，请使用文档中实际存在的连续页。',
  PAGE_SELECTION_REQUIRED:'文档超过一次40页的提取范围，请选择需要阅读的页。',
  TEXT_LIMIT:'所选页文字超过256 KiB，未截断；请缩小页范围。',
  NO_TEXT:'所选页没有可提取文字，可能是扫描件或空白页；当前未进行OCR或图像理解。',
  EXTRACTION_FAILED:'本地文档解析失败，未按成功或空内容处理。',
};
function failure(code,extra={}){return {version:'homebase-document-v1',status:'failed',code,message:messages[code],...extra};}
export async function extractDocument(request){
  if(!request||typeof request!=='object'||Array.isArray(request)||request.version!==1||request.mime_type!=='application/pdf'||
      Object.keys(request).some(k=>!['version','mime_type','bytes_base64','pages'].includes(k))||typeof request.bytes_base64!=='string')return failure('INVALID_REQUEST');
  const encoded=request.bytes_base64;
  if(encoded.length>Math.ceil(MAX_SOURCE_BYTES/3)*4)return failure('SOURCE_LIMIT');
  if(encoded.length%4!==0||/[^A-Za-z0-9+/=]/.test(encoded))return failure('INVALID_REQUEST');
  const bytes=Buffer.from(encoded,'base64');
  if(bytes.length>MAX_SOURCE_BYTES)return failure('SOURCE_LIMIT');
  if(bytes.toString('base64')!==encoded)return failure('INVALID_REQUEST');
  if(!bytes.length)return failure('SOURCE_LIMIT');
  const info={method:'pdf_text',source_sha256:createHash('sha256').update(bytes).digest('hex'),source_bytes:bytes.length};
  if(!bytes.subarray(0,1024).includes(Buffer.from('%PDF-')))return failure('INVALID_PDF',info);
  const range=request.pages;
  if(range!==undefined&&(!range||typeof range!=='object'||Array.isArray(range)||Object.keys(range).some(k=>!['from','to'].includes(k))||
      !Number.isInteger(range.from)||!Number.isInteger(range.to)||range.from<1||range.to<range.from||range.to-range.from+1>MAX_PAGES))return failure('INVALID_RANGE',info);
  let task;
  try{
    const {getDocument,VerbosityLevel}=await import('pdfjs-dist/legacy/build/pdf.mjs');
    const assets=new URL('./node_modules/pdfjs-dist/',import.meta.url);
    const localAsset=name=>fileURLToPath(new URL(name+'/',assets)).replaceAll('\\','/');
    // PDF.js uses its Node file readers for these fixed bundled assets. No URL,
    // password, external font base, or user-supplied path enters this call.
    task=getDocument({data:new Uint8Array(bytes),stopAtErrors:true,verbosity:VerbosityLevel.ERRORS,
      useWorkerFetch:false,cMapUrl:localAsset('cmaps'),cMapPacked:true,
      standardFontDataUrl:localAsset('standard_fonts'),wasmUrl:localAsset('wasm'),
      useSystemFonts:false,disableFontFace:true,enableXfa:false,useWasm:false,
      isOffscreenCanvasSupported:false,isImageDecoderSupported:false});
    const pdf=await task.promise;
    info.total_pages=pdf.numPages;
    if(!range&&pdf.numPages>MAX_PAGES)return {...failure('PAGE_SELECTION_REQUIRED',info),status:'page_selection_required'};
    const from=range?.from??1,to=range?.to??pdf.numPages;
    if(to>pdf.numPages)return failure('INVALID_RANGE',info);
    const pages=[];let total=0;
    for(let number=from;number<=to;number++){
      const page=await pdf.getPage(number);const reader=page.streamTextContent().getReader();
      const parts=[];let exceeded=false;
      try{
        for(;;){
          const {done,value}=await reader.read();if(done)break;
          for(const item of value.items){
            if(typeof item.str!=='string')continue;
            const text=item.str+(item.hasEOL?'\n':'');total=Math.min(MAX_TEXT_BYTES+1,total+Buffer.byteLength(text));
            // PDF.js' fake-worker stream can race a close with reader.cancel.
            // Drain this page without retaining more text, then return a fixed
            // limit failure. The supervisor still enforces heap/time limits.
            if(total>MAX_TEXT_BYTES){exceeded=true;continue;}
            if(!exceeded)parts.push(text);
          }
        }
      }finally{reader.releaseLock();page.cleanup();}
      if(exceeded)return failure('TEXT_LIMIT',{...info,page_from:from,page_to:to});
      const text=parts.join('').trim();pages.push({number,text,bytes:Buffer.byteLength(text),empty:!text});
    }
    const missing=pages.filter(p=>p.empty).map(p=>p.number),textBytes=pages.reduce((n,p)=>n+p.bytes,0);
    return {version:'homebase-document-v1',status:textBytes?'ready':'no_text',code:textBytes?null:'NO_TEXT',
      message:textBytes?(missing.length?'部分页没有可提取文字；这些页未进行OCR或图像理解。':'已在本机提取所选页文字，尚未发送给模型。'):messages.NO_TEXT,
      ...info,page_from:from,page_to:to,pages,text_bytes:textBytes,missing_text_pages:missing,
      partial:from!==1||to!==pdf.numPages||missing.length>0};
  }catch(error){
    return failure(error?.name==='PasswordException'?'PASSWORD_REQUIRED':['InvalidPDFException','FormatError'].includes(error?.name)?'INVALID_PDF':'EXTRACTION_FAILED',info);
  }finally{if(task)try{await task.destroy();}catch{/* Parsing outcome stays authoritative. */}}
}
