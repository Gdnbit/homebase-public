// Minimal MCP stdio surface for this native turn's private HomeBase gateway.
import readline from 'node:readline';
const schema={type:'object',properties:{action:{type:'string',enum:['context','discover','describe','task','status']},text:{type:'string'},query:{type:'string'},module:{type:'string'},command:{type:'string'},turn_id:{type:'string'}},required:['action'],additionalProperties:false};
const send=v=>process.stdout.write(JSON.stringify(v)+'\n');
const lines=readline.createInterface({input:process.stdin,crlfDelay:Infinity});
lines.on('line',async line=>{
  if(line.length>1024*1024)return;
  let m;try{m=JSON.parse(line);}catch{return;}
  if(m.id===undefined)return;
  try{
    let result;
    if(m.method==='initialize')result={protocolVersion:m.params?.protocolVersion??'2024-11-05',capabilities:{tools:{}},serverInfo:{name:'homebase-native',version:'1.0.0'}};
    else if(m.method==='ping')result={};
    else if(m.method==='tools/list')result={tools:[{name:'homebase',description:'HomeBase统一入口：context读取当前会话背景；discover/describe发现契约；task委托共享执行器；status查询原任务和产物。等待本人确认时不能自批；未知结果不重做。',inputSchema:schema}]};
    else if(m.method==='tools/call'&&m.params?.name==='homebase'){
      const url=process.env.HB_NATIVE_BRIDGE_URL;
      if(!/^http:\/\/127\.0\.0\.1:\d+\/tool$/.test(url??''))throw Error('Private gateway unavailable');
      const r=await fetch(url,{method:'POST',headers:{'content-type':'application/json','authorization':`Bearer ${process.env.HB_NATIVE_BRIDGE_TOKEN}`},body:JSON.stringify({id:m.id,arguments:m.params.arguments??{}}),signal:AbortSignal.timeout(180000)});
      if(!r.ok)throw Error('Private gateway unavailable');const v=await r.json();
      result={content:[{type:'text',text:JSON.stringify(v.result??{error:v.error})}],isError:!!v.error};
    }else {send({jsonrpc:'2.0',id:m.id,error:{code:-32601,message:'Method not supported'}});return;}
    send({jsonrpc:'2.0',id:m.id,result});
  }catch{send({jsonrpc:'2.0',id:m.id,result:{content:[{type:'text',text:'HomeBase工具连接中断，结果未知；先查询原任务，不要重做。'}],isError:true}});}
});
