import {test} from 'node:test';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import readline from 'node:readline';
import {fileURLToPath} from 'node:url';
import {isDeepStrictEqual} from 'node:util';
const piWorker = process.env.HOMEBASE_PI_WORKER || fileURLToPath(new URL('../pi-worker.mjs',import.meta.url));
const definitions=[{type:'function',function:{name:'homebase_job',description:'fixture query',parameters:{type:'object',properties:{job_id:{type:'string'}},required:['job_id'],additionalProperties:false}}}];
function invoke(operations=[], calls=[], history=[], options={}) {
  return new Promise((resolve,reject)=>{
    const child=spawn(process.execPath,[piWorker],{env:{SystemRoot:process.env.SystemRoot,PATH:process.env.PATH},windowsHide:true,stdio:['pipe','pipe','pipe']});
    const timer=setTimeout(()=>{child.kill();reject(Error('timeout'));},60000);
    const saved=[...operations],modelCache=[...(options.modelCache??[])],toolCache={...(options.toolCache??{})};let models=0,tools=0,summaries=0,replays=0,maxReplayBytes=0,cacheHits=0,error='',queued=calls;
    child.stderr.on('data',b=>{error+=String(b).slice(0,3000);});
    child.stdin.on('error',err=>{clearTimeout(timer);child.kill();reject(err);});
    child.on('exit',code=>{clearTimeout(timer);if(code&&code!==0)reject(Error(error||'worker exit'));});
    readline.createInterface({input:child.stdout}).on('line',line=>{
      try {
        const m=JSON.parse(line);
        if(m.kind==='failed'){child.kill();clearTimeout(timer);reject(Error(m.error));return;}
        if(m.kind==='complete'){clearTimeout(timer);resolve({text:m.result.text,saved,models,tools,summaries,replays,maxReplayBytes,initBytes,cacheHits,modelCache,toolCache});return;}
        let result;
        if(m.kind==='replay'){
          assert.equal(m.payload.index,replays);
          assert(replays<operations.length);
          result=operations[replays++];
        }
        else if(m.kind==='checkpoint'){saved.push(m.payload);result={saved:true};}
        else if(m.kind==='summarize' && options.longTools){
          summaries++;
          assert(Array.isArray(m.payload.earlier_turn_context));
          assert(!JSON.stringify(m.payload).includes('fixture-large-tool-body'));
          if(options.pauseSummary){child.kill();clearTimeout(timer);resolve({saved,models,tools,summaries,modelCache,toolCache});return;}
          result={text:'已核对老阶段，不重新执行。'};
        }
        else if(m.kind==='model'){
          const cached=modelCache.find(c=>isDeepStrictEqual(c.payload,m.payload));
          if(cached){cacheHits++;child.stdin.write(JSON.stringify({id:m.id,result:cached.result})+'\n');queued=cached.result.calls;return;}
          models++;
          assert(m.payload.definitions.every(d=>d.function.name==='homebase_job'));
          if(options.longTools) {
            const messages=m.payload.messages;
            assert(Buffer.byteLength(JSON.stringify(m.payload))<128*1024);
            assert(messages.some(message=>message.role==='user' && message.content==='synthetic'));
            assert.deepEqual(messages.flatMap(x=>x.tool_calls??[]).map(c=>c.id),messages.filter(x=>x.role==='tool').map(x=>x.tool_call_id));
            const count=Object.keys(toolCache).length;
            result=count>=12?{text:'long task complete',calls:[]}:{text:'核对已完成操作',calls:Array.from({length:4},(_,i)=>({
              id:'long-'+(count+i),name:'homebase_job',arguments:{job_id:'fixture-'+(count+i)},
            }))};
            if(count>=12)assert(messages.some(x=>x.role==='system'&&x.content.includes('本轮早期上下文已收拢')));
            modelCache.push({payload:m.payload,result});
            child.stdin.write(JSON.stringify({id:m.id,result})+'\n');
            return;
          }
          const toolCount=m.payload.messages.filter(x=>x.role==='tool').length;
          const done=toolCount>=(options.sequence?2:1);
          if(history.length){const previous=m.payload.messages.find(x=>x.role==='assistant'&&x.content==='synthetic earlier answer');assert.equal(previous.reasoning_content,'synthetic history state');assert.equal(previous.response_items[0].encrypted_content,'opaque');}
          if(done){const last=m.payload.messages.findLast(x=>x.role==='assistant');assert.equal(last.reasoning_content,'synthetic-private-provider-state');assert.deepEqual(last.reasoning_details,[{type:'fixture',data:'signed-state'}]);assert.equal(last.response_items[0].encrypted_content,'opaque-next');}
          result=done?{text:'fixture complete',calls:[]}:{text:'',calls:[{id:'fixture-call',name:'homebase_job',arguments:options.arguments??{job_id:'fixture-job'}}]};
          if(options.sequence && toolCount===0)result={text:'',calls:[{id:'fixture-search',name:'homebase_job',arguments:{options:{revision:0,title:'synthetic read'},job_id:'fixture-readonly'}}]};
          queued=result.calls;
          if(!done)result.message={reasoning_content:'synthetic-private-provider-state',reasoning_details:[{type:'fixture',data:'signed-state'}],response_items:[{type:'reasoning',encrypted_content:'opaque-next'}]};
          modelCache.push({payload:m.payload,result});
        }else if(m.kind==='tool'){
          assert(options.longTools?m.payload.id.startsWith('long-'):['fixture-call',...(options.sequence?['fixture-search']:[])].includes(m.payload.id));
          if(toolCache[m.payload.id]){assert.deepEqual(m.payload.arguments,toolCache[m.payload.id].arguments);child.stdin.write(JSON.stringify({id:m.id,result:toolCache[m.payload.id].result})+'\n');return;}
          if(options.pauseTools && (!options.sequence || m.payload.id==='fixture-call')){child.kill();clearTimeout(timer);resolve({paused:true,saved,calls:queued,models,tools,modelCache,toolCache});return;}
          tools++;result={state:'succeeded',fixture:true,...(options.longTools?{body:'fixture-large-tool-body'.padEnd(9000,'x')}:{})};
          toolCache[m.payload.id]={arguments:m.payload.arguments,result};
        }
        else throw Error('unexpected callback');
        const frame=JSON.stringify({id:m.id,result});
        if(m.kind==='replay')maxReplayBytes=Math.max(maxReplayBytes,Buffer.byteLength(frame));
        child.stdin.write(frame+'\n');
      } catch(e){child.kill();clearTimeout(timer);reject(e);}
    });
    const replay={checkpoint_count:operations.length};
    const init=JSON.stringify({protocol:1,turn_id:'fixture-turn',prompt:'Use the fixture tool once.',messages:[...history,{role:'user',content:'synthetic'}],tools:options.definitions??definitions,...replay,calls});
    const initBytes=Buffer.byteLength(init);
    if(options.endAfterInit)child.stdin.end(init+'\n');else child.stdin.write(init+'\n');
  });
}
test('real Pi keeps earlier turns, uses the host tool, and reopens the final result without another call',async()=>{
  const history = [{role:'user',content:'上一轮的合成对话'}, {role:'assistant',content:'synthetic earlier answer',
    reasoning_content:'synthetic history state',response_items:[{type:'reasoning',encrypted_content:'opaque'}]}];
  const first = await invoke([],[],history);
  assert.equal(first.text,'fixture complete');
  assert.equal(first.models,2);
  assert.equal(first.tools,1);
  assert(first.saved.every(record=>record.kind==='pi_turn'));
  assert(first.modelCache[0].payload.messages.some(message=>message.content==='上一轮的合成对话'));
  const reopened = await invoke([first.saved.at(-1)]);
  assert.equal(reopened.text,first.text);
  assert.equal(reopened.models,0);
  assert.equal(reopened.tools,0);
});
test('Pi compacts complete tool batches and resumes an interrupted compaction without running a completed tool again',async()=>{
  const options={longTools:true};
  const done=await invoke([],[],[],options);
  assert.equal(done.text,'long task complete');
  assert.equal(done.tools,12);
  assert.equal(done.summaries,1);
  const first=await invoke([],[],[],{...options,pauseSummary:true});
  assert.equal(first.tools,12);
  assert.equal(first.saved.at(-1).compaction_pending,true);
  const resumed=await invoke([first.saved.at(-1)],[],[],{...options,modelCache:first.modelCache,toolCache:first.toolCache});
  assert.equal(resumed.text,done.text);
  assert.equal(resumed.summaries,0);
  assert.equal(resumed.tools,0);
  assert.equal(resumed.saved.at(-1).complete,true);
});
test('pending tool restores earlier batches by SDK call ID without repeating cached model or tool execution',async()=>{
  const defs=[{type:'function',function:{name:'homebase_job',description:'fixture query',parameters:{type:'object',properties:{job_id:{type:'string'},options:{type:'object',properties:{title:{type:'string'},revision:{type:'integer'}},required:['title','revision'],additionalProperties:false}},required:['job_id','options'],additionalProperties:false}}}];
  const first=await invoke([],[],[],{definitions:defs,arguments:{options:{revision:0,title:'合成恢复'},job_id:'fixture-job'},pauseTools:true,sequence:true});
  assert.equal(first.paused,true);assert.equal(first.models,2);assert.equal(first.tools,1);
  const restored=await invoke([first.saved.at(-1)],first.calls,[],{definitions:defs,modelCache:first.modelCache,toolCache:first.toolCache,sequence:true});
  assert.equal(restored.text,'fixture complete');assert.equal(restored.models,1);assert.equal(restored.tools,1);
  assert.equal(restored.replays,1);
  const completed=await invoke([restored.saved.at(-1)],[],[],{definitions:defs});
  assert.equal(completed.models,0);assert.equal(completed.tools,0);
});
