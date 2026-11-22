import test from 'node:test';
import assert from 'node:assert/strict';
import {build} from 'esbuild';
import {fileURLToPath} from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const output=await build({stdin:{contents:'export * from "./src/lib/workflow-context";export {readAssistantTaskContext} from "./src/lib/assistant-task-context";export {replaceCapabilityTask,frameCapabilityData} from "./src/lib/capability-context";',resolveDir:root},bundle:true,write:false,platform:'node',format:'esm',logLevel:'silent'});
const {studioHandoff,inboxHandoff,readWorkflowDraft:read,workflowFrame,readAssistantTaskContext:readAny,replaceCapabilityTask:replace,frameCapabilityData}=await import('data:text/javascript;base64,'+Buffer.from(output.outputFiles[0].text).toString('base64'));
const material=()=>({text:'完整原始资料🙂\n<img src="https://example.test/private" onerror="bad()">',title:'资料标题',tags:'synthetic',source:'原来源',attachments:[{rel:'_attachments/synthetic.txt',name:'synthetic.txt',size:15}],uncertain:true});

test('Studio reserves the complete wire budget for original references and all unsent requirements before historical excerpts',()=>{
 const input='新的要求🙂'.repeat(180);
 const text=studioHandoff({session:'synthetic-session',task:'synthetic-original-task',input,messages:Array.from({length:8},(_,i)=>({role:i%2?'assistant':'user',content:'历史'.repeat(1000)}))});
 assert(text.includes(input),'Current unsent requirements must never be truncated');
 assert(text.includes('synthetic-session')&&text.includes('synthetic-original-task'));
 assert(Buffer.byteLength(text)<=8000,`Whole handoff must fit, got ${Buffer.byteLength(text)} bytes`);
});

test('material stays complete and literal while the human goal remains independently editable',()=>{
 const data=material(),original=structuredClone(data),text=inboxHandoff(data),v=read(text);
 assert(v);assert(!v.task.includes(data.text));assert(v.task.includes('先查已有记录'));
 assert.deepEqual(v.payload.source,data);assert.equal(readAny(text).kind,'workflow');
 const next=read(replace(v,'改为只检查已有记录🙂'));assert.equal(next.frame,v.frame);assert.equal(next.task,'改为只检查已有记录🙂');
 assert.deepEqual(data,original);assert.equal(read(replace(next,'')).task,'');
 const extra={...data,private_extra:'NOT-COPIED',attachments:[{...data.attachments[0],private_extra:'NOT-COPIED'}]};assert(!inboxHandoff(extra).includes('NOT-COPIED'));
});

test('recent manuscript excerpts are bounded and marked partial without truncating task identity or new instructions',()=>{
 const input='完整补充🙂\n"\\'.repeat(120),messages=Array.from({length:26},(_,i)=>({role:i%2?'assistant':'user',content:`第${i}条`+'历史🙂'.repeat(1000)}));
 const data={session:'original-session',task:'original-task',error:'原受理未知',input,messages},original=structuredClone(data);
 const text=studioHandoff(data),v=read(text),s=v.payload.source;
 assert(v.task.endsWith(input));assert(v.task.includes('不重新生成'));assert(Buffer.byteLength(text)<=8000);
 assert.equal(s.session_id,data.session);assert.equal(s.task_id,data.task);assert.equal(s.previous_error,data.error);
 assert.equal(s.total_messages,26);assert.equal(s.selected_messages,8);assert.equal(s.partial,true);assert(s.excerpts.length>0&&s.excerpts.length<=8);
 assert(s.excerpts.at(-1).content.startsWith('第25条'));assert(s.excerpts.some(e=>e.truncated));
 assert(s.excerpts.every(e=>Buffer.byteLength(e.content)<=1200&&!e.content.includes('\ufffd')));assert.deepEqual(data,original);
});

test('an oversized current goal or source identity fails before creating anything and preserves the original caller data',()=>{
 const data={session:'session',task:'original-task',input:'完整要求🙂'.repeat(1500),messages:[{role:'user',content:'原历史'}]},original=structuredClone(data);
 assert.throws(()=>studioHandoff(data),/超过8,000字节.*未创建新会话/);assert.deepEqual(data,original);
 assert.throws(()=>studioHandoff({...data,input:'short',task:'id'.repeat(5000)}),/工作流引用太长/);
});

test('unknown, noncanonical, extra, duplicate and mixed frames remain full raw text',()=>{
 const text=inboxHandoff(material()),v=read(text);
 for(const bad of [text+text,text.replace('homebase-workflow-context-v1','unknown-v2'),text.replace('  "version"',' "version"'),text+'invalid-boundary',text.replace('【HomeBase 工作流资料结束】','')])assert.equal(read(bad),null);
 const p=structuredClone(v.payload);p.source.private_extra='DO-NOT-HIDE';assert.equal(read(workflowFrame(p)),null);
 const p2=structuredClone(v.payload);p2.source.attachments[0].size=-1;assert.equal(read(workflowFrame(p2)),null);
 const p3=structuredClone(v.payload);p3.workflow='unknown';assert.equal(read(workflowFrame(p3)),null);
 const capability=frameCapabilityData({version:'homebase-capability-selection-v1',source:{catalog_id:'synthetic',assembled_at:'unknown',expires_at:'unknown',source_complete:false,issues_total:0},selection:{function:'text',implementation:null}});
 assert.equal(readAny(text+'\n'+capability),null);assert.equal(readAny(capability+'\n'+text),null);
 assert.equal(read('普通旧稿\n'+v.json),null);
});

test('one-newline voice tails remain visible and editing retains every source byte',()=>{
 const text=inboxHandoff(material()),v=read(text+'\n语音补充🙂');assert(v.task.endsWith('\n语音补充🙂'));
 assert.equal(v.frame,read(text).frame);assert.equal(read(replace(v,v.task)).json,v.json);
});

test('empty or unaccepted manuscript sources remain explicit, and large raw material is never silently truncated',()=>{
 const fresh=read(studioHandoff({session:null,task:null,input:'',messages:[]}));assert(fresh.task.includes('给我预览和文件'));assert.equal(fresh.payload.source.task_id,null);
 const unknown=read(studioHandoff({session:'s',task:null,error:'未知',input:'保留新要求',messages:[]}));assert(unknown.task.includes('先核对'));assert.equal(unknown.payload.source.previous_error,'未知');
 const data={...material(),text:'完整资料🙂'.repeat(5000)},text=inboxHandoff(data);assert(text.includes(data.text));assert(Buffer.byteLength(text)>65536);assert.equal(read(text),null);
});
