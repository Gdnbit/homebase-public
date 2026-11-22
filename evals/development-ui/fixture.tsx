import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { DevelopmentCapability } from "../../src/components/development-capability";
import { installDraftLifecycle } from "../../src/lib/assistant-draft-lifecycle";
import "../../src/index.css";
const state={calls:[] as {command:string;args:any}[],offline:false,lost:false,phase:"running",questions:[] as any[],failRead:false,resultDelay:0};
const privateState={entry:structuredClone((window as any).__developmentSeed??{schema_version:1,revision:0,draft:{workspace:'',brief:'',workflow:'development-local',checks_json:'[]',protected_files_input:''},pending:null,previous:[]}),failSave:false,failGet:false,loseSave:false,losePrepare:false,saveDelay:0,holdGet:false,holdPrepare:false,releaseGet:undefined as (()=>void)|undefined,releasePrepare:undefined as (()=>void)|undefined};
Object.assign(privateState,(window as any).__developmentFlags??{});
(window as any).__developmentWorkspace=privateState;
(window as any).__development=state;
const callbacks=new Map<number,(event:unknown)=>void>();
const close={handler:undefined as number|undefined,attempts:0,
  request(){if(close.handler!==undefined) callbacks.get(close.handler)?.({event:'tauri://close-requested',id:1,payload:null});}};
(window as any).__developmentClose=close;
(window as any).__TAURI_INTERNALS__={metadata:{currentWindow:{label:'main'}},transformCallback:(cb:(event:unknown)=>void)=>{
  const id=callbacks.size+1;callbacks.set(id,cb);return id;
},invoke:async(command:string,args:any={})=>{
  state.calls.push({command,args});
  if(command==='plugin:event|listen'&&args.event==='tauri://close-requested'){close.handler=args.handler;return 1;}
  if(command==='plugin:window|destroy'){close.attempts++;return null;}
  if(command==='plugin:dialog|message')return '返回核对';
  if(command==='development_workspace_get'){
    if(privateState.holdGet)await new Promise<void>(resolve=>{privateState.releaseGet=resolve;});
    if(privateState.failGet)throw Error('DEVELOPMENT_WORKSPACE_READ');return structuredClone(privateState.entry);
  }
  if(command==='development_workspace_save'||command==='development_workspace_prepare'){
    if(command==='development_workspace_prepare'&&privateState.holdPrepare)
      await new Promise<void>(resolve=>{privateState.releasePrepare=resolve;});
    if(privateState.failSave)throw Error('DEVELOPMENT_WORKSPACE_SAVE');
    const r=args.request,e=privateState.entry;
    if(command==='development_workspace_prepare'&&e.pending){if(JSON.stringify(e.pending)===JSON.stringify(r.original))return structuredClone(e);throw Error('DEVELOPMENT_WORKSPACE_PENDING');}
    if(r.revision!==e.revision)throw Error('DEVELOPMENT_WORKSPACE_CONFLICT');
    e.draft=structuredClone(r.draft);e.revision++;
    if(command==='development_workspace_save'&&privateState.loseSave){privateState.loseSave=false;throw Error('ISOLATED_DEVELOPMENT_SAVE_ACK_LOST');}
    if(command==='development_workspace_prepare'){e.pending=structuredClone(r.original);if(privateState.losePrepare)throw Error('synthetic prepare response lost');}
    const result=structuredClone(e);if(privateState.saveDelay)await new Promise(r=>setTimeout(r,privateState.saveDelay));return result;
  }
  if(command==='development_workspace_new'){const e=privateState.entry;if(args.revision!==e.revision||args.requestId!==e.pending?.request_id)throw Error('DEVELOPMENT_WORKSPACE_CONFLICT');if(e.pending)e.previous.push(e.pending);e.pending=null;e.revision++;return structuredClone(e);}
  if(command==='development_catalog')return {status:state.offline?'offline':'ready',version:'fixture',message:'仅合成执行器',workflows:[{id:'development-local',name:'完整开发 · 三个独立 Codex 角色'},{id:'execution-checked',name:'明确小改动 · 单执行器'},{id:'development',name:'Claude / Codex 混合'}],tasks:[]};
  if(command==='development_run'){if(state.lost)throw Error('synthetic response lost');return {task_id:'dev-'+'a'.repeat(32),state:'submitted',message:'已受理'};}
  if(command==='development_result'){if(state.failRead)throw Error('synthetic query failure');const result=structuredClone({task_id:args.taskId,state:state.phase,questions:state.questions,runs:[],message:'合成任务状态'});if(state.resultDelay)await new Promise(r=>setTimeout(r,state.resultDelay));return result;}
  if(command==='development_answer'){state.questions=[];return {task_id:args.request.task_id,state:'running',questions:[],runs:[]};}
  throw Error(`Unregistered fixture operation ${command}`);
}};
createRoot(document.getElementById('root')!).render(<QueryClientProvider client={new QueryClient({defaultOptions:{queries:{retry:false}}})}><main className="mx-auto max-w-3xl p-5"><DevelopmentCapability /></main></QueryClientProvider>);
void installDraftLifecycle();
