// Trusted host adapter. Only the native parent supplies context; never expose this RPC to the model.
import crypto from 'node:crypto';
import {once} from 'node:events';
import {fileOperation} from './files.mjs';
import {runTerminal} from './terminal.mjs';
import {McpBroker} from './mcp.mjs';
import {skillIndex,skillOperation} from './skills.mjs';
import {executionFailure} from './policy.mjs';

const broker=new McpBroker(), jobs=new Map(), canceled=new Set();
const MAX_FRAME=4*1024*1024;
const completed=[];let resultBytes=0;
let closing=false, output=Promise.resolve();
function send(message){
  output=output.then(async()=>{
    let line=JSON.stringify(message);
    if(Buffer.byteLength(line)>MAX_FRAME)line=JSON.stringify({id:message.id,kind:'result',result:{status:'unknown',code:'OUTPUT_TOO_LARGE',message:'操作已返回但结果超过上限，请查询原任务或产物，不重新执行'}});
    if(!process.stdout.write(`${line}\n`))await once(process.stdout,'drain');
  }).catch(()=>{closing=true;for(const job of jobs.values())job.abort.abort();});
  return output;
}
function fingerprint(message){return crypto.createHash('sha256').update(JSON.stringify([message.operation,message.request,message.context])).digest('hex');}
async function dispatch(message){
  const id=message?.id;
  if(typeof id!=='string'||!id||id.length>160)return send({kind:'protocol_error',message:'请求编号无效'});
  if(message.kind==='reset_connections'){
    await broker.close();return send({id,kind:'result',result:{status:'succeeded',state:'connections_closed'}});
  }
  if(message.kind==='disconnect_server'){
    if(typeof message.server_id!=='string'||message.server_id.length>128)return send({id,kind:'protocol_error',message:'服务器编号无效'});
    await broker.disconnect(message.server_id);return send({id,kind:'result',result:{status:'succeeded',state:'connection_closed'}});
  }
  if(message.kind==='cancel'){
    const job=jobs.get(id);if(job&&!job.done)job.abort.abort();
    if(!job&&canceled.size<10000)canceled.add(id);
    return send({id,kind:'cancel_ack',state:job?job.done?'finished':'cancel_requested':'not_found'});
  }
  if(message.kind==='status'){
    const job=jobs.get(id);return send({id,kind:'status',state:job?job.done?'finished':'running':'not_found',result:job?.result||null,result_expired:!!job?.expired});
  }
  if(message.kind!=='execute'||closing||!message.context||!message.request)return send({id,kind:'protocol_error',message:'请求格式无效或运行时正在关闭'});
  if(canceled.has(id))return send({id,kind:'result',result:{status:'canceled',message:'此操作在启动前已取消，未执行'}});
  const hash=fingerprint(message),previous=jobs.get(id);
  if(previous){
    if(previous.hash!==hash)return send({id,kind:'protocol_error',message:'请求编号已用于不同操作，未执行'});
    if(previous.expired)return send({id,kind:'result',result:{status:'unknown',code:'HOST_RESULT_EXPIRED',message:'此操作已执行，宿主短期结果缓存已过期；请查询原持久任务，未重新执行'}});
    return previous.result?send({id,kind:'result',result:previous.result,reused:true}):send({id,kind:'accepted',reused:true});
  }
  if(jobs.size>=10000||[...jobs.values()].filter(j=>!j.done).length>=8)return send({id,kind:'result',result:{status:'failed',code:'HOST_BUSY',message:'本机工具队列已满，请查询既有任务'}});
  const job={hash,abort:new AbortController(),result:null,done:false};jobs.set(id,job);
  await send({id,kind:'accepted'});
  job.promise=(async()=>{
    const buffers=new Map();let timer;
    function flush(){
      if(timer)clearTimeout(timer);timer=null;
      for(const event of buffers.values())send({id,kind:'progress',event});buffers.clear();
    }
    const options={signal:job.abort.signal,onEvent:event=>{
      if(!['stdout','stderr'].includes(event.event)||typeof event.text!=='string')return;
      const previous=buffers.get(event.event),text=(previous?.text||'')+event.text;
      const chars=Array.from(text.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g,''));
      buffers.set(event.event,{event:event.event,text:chars.slice(-8000).join(''),truncated:!!previous?.truncated||chars.length>8000});
      if(!timer)timer=setTimeout(flush,100);
    }};
    try{
      switch(message.operation){
        case 'files':job.result=await fileOperation(message.context,message.request);break;
        case 'terminal':job.result=await runTerminal(message.context,message.request,options);break;
        case 'mcp':job.result=await broker.operation(message.context,message.request,options);break;
        case 'skill':job.result=await skillOperation(message.context,message.request,options);break;
        case 'skill_index':job.result=await skillIndex(message.context);break;
        default:job.result={status:'failed',code:'OPERATION_UNSUPPORTED',message:'本机操作未登记'};
      }
    }catch(error){job.result=executionFailure(error);}
    flush();
    job.done=true;
    await send({id,kind:'result',result:job.result});
    job.bytes=Buffer.byteLength(JSON.stringify(job.result));resultBytes+=job.bytes;completed.push(job);
    while(completed.length>256||resultBytes>16*1024*1024){const old=completed.shift();resultBytes-=old.bytes;old.result=null;old.expired=true;}
  })();
}
let buffered=Buffer.alloc(0);
try{
  for await(const chunk of process.stdin){
    buffered=Buffer.concat([buffered,chunk]);
    let end;
    while((end=buffered.indexOf(10))!==-1){
      if(end>MAX_FRAME)throw new Error('frame');
      const line=buffered.subarray(0,end);buffered=buffered.subarray(end+1);
      let message;try{message=JSON.parse(line.toString('utf8'));}catch{throw new Error('frame');}
      await dispatch(message);
    }
    if(buffered.length>MAX_FRAME)throw new Error('frame');
  }
}catch{await send({kind:'protocol_error',message:'本机工具协议无效，已停止接收请求'});}
finally{
  closing=true;for(const job of jobs.values())if(!job.done)job.abort.abort();
  await Promise.allSettled([...jobs.values()].map(j=>j.promise));
  await broker.close();await output;
}
