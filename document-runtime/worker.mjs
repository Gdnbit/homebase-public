// One credential-free process, one bounded request and response. The host owns
// the hard deadline/cancel (a parser may block JS); there are no child workers.
import {extractDocument,MAX_SOURCE_BYTES} from './extract.mjs';
const LIMIT=Math.ceil(MAX_SOURCE_BYTES/3)*4+4096;
globalThis.fetch=async()=>{throw Error('External document resources are disabled');};
async function main(){
  const chunks=[];let size=0;
  for await(const chunk of process.stdin){size+=chunk.length;if(size>LIMIT)return fail('FRAME_LIMIT','文档输入超过传输上限，未解析。');chunks.push(chunk);}
  let request;try{request=JSON.parse(Buffer.concat(chunks).toString('utf8'));}catch{return fail('INVALID_REQUEST','文档请求格式无效，未解析。');}
  return extractDocument(request);
}
function fail(code,message){return {version:'homebase-document-v1',status:'failed',code,message};}
// Only the final JSON may reach stdout; library diagnostics have no document
// payload or stack in the protocol. The supervising host discards stderr.
console.log=()=>{};console.warn=()=>{};console.info=()=>{};console.error=()=>{};
try{process.stdout.write(JSON.stringify(await main())+'\n');}
catch{process.stdout.write(JSON.stringify(fail('EXTRACTION_FAILED','本地文档解析失败，未按空内容处理。'))+'\n');process.exitCode=1;}
