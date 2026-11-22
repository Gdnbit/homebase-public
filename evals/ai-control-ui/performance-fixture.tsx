import React from "react";
import { createRoot } from "react-dom/client";
import { MemoryRouter, useLocation } from "react-router-dom";
import { QueryClientProvider } from "@tanstack/react-query";
import { App } from "../../src/App";
import { createQueryClient } from "../../src/lib/query-client";
import { startPolling } from "../../src/lib/poll";
import {
  createDefaultAsrPortableConfig,
  exportAsrPortableBundle,
} from "../../src/lib/asr-portability";
import "../../src/index.css";
import { modelCapabilityFixture } from "./model-capabilities-data";
import { installDraftLifecycle } from "../../src/lib/assistant-draft-lifecycle";
import { inboxWorkspaceFixture } from "./inbox-workspace-fixture";
import { sessionCatalogFixture } from "./session-catalog-fixture";

const calls: string[] = [];
const windowRequests: any[] = [];
let closeRegistrationAttempts = 0;
void installDraftLifecycle().catch(() => {});
const nativeRequests: any[]=[];
const capabilityRequests: any[]=[];
const capabilityCatalogs=new Map<string,any>();
const capabilityViews=new Map<string,any>();
const discoveryStats={source_reads:0};
// Synthetic-only process store. Reload tests seed it explicitly from Playwright;
// this fixture never uses real IPC or private browser persistence.
const desktopStore = new Map<string, any>(Object.entries((window as any).__desktopSeed ?? {}));
const createdSessions = new Map<string, any>(Object.entries((window as any).__assistantSeed ?? {}));
const createRequests: any[] = [];
const desktopReceipts = new Map<string, any>();
const nativeReceipts = new Map<string, any>(Object.entries((window as any).__nativeReceiptSeed ?? {}));
const sameNative = (a:any,b:any) => ["provider","thread_id","text","request_id"].every(k=>a[k]===b[k]);
const desktopEntry = (scope: string) => structuredClone(desktopStore.get(scope) ?? {
  schema_version: 1, scope, revision: 0,
  body: { text: "", route: "", mode: "auto", files: [] }, pending: null, voices: [], updated_at: null,
});
const desktopReply = (status: string, entry: any, ack: any = null) => ({ status, entry, ack, message: "", warning: null });
const desktopBodyFromRequest = (request: any) => ({ text: request.text, route: request.route_id ?? "", mode: request.mode ?? "auto", files: request.attachments ?? [], ...(request.image ? { image: request.image } : {}) });
function desktopAccept(scope: string, request: any, ack: any) {
  const entry = desktopEntry(scope);
  const body = desktopBodyFromRequest(request);
  const cleared = { text: "", route: body.route, mode: body.mode, files: [] };
  if (entry.pending?.request.request_id === request.request_id) {
    if (JSON.stringify(entry.body) === JSON.stringify(body)) entry.body = cleared;
    entry.pending = null; entry.revision++;
    desktopStore.set(scope, entry);
  }
  if (scope === "new" && !desktopStore.has(ack.session_id))
    desktopStore.set(ack.session_id, { ...desktopEntry(ack.session_id), revision: 1, body: cleared });
  return desktopReply("accepted", entry, ack);
}
function nativeAccept(scope:string, request:any, result:any) {
  const entry=desktopEntry(scope);
  if(entry.native_pending && entry.native_pending.request_id===request.request_id){
    if(entry.body.text===request.text)entry.body.text="";
    entry.native_pending=null;entry.revision++;desktopStore.set(scope,structuredClone(entry));
  }
  return {...desktopReply(result.state==="accepted"?"accepted":"unknown",entry),native_result:result};
}
const voiceStages = new Map<string, any>(Object.entries((window as any).__voiceStageSeed ?? {}));
const voiceJournal = new Map<string, any>();
function saveVoice(entry: any) { entry.revision++; desktopStore.set(entry.scope, structuredClone(entry)); return desktopReply("loaded", entry); }
function findVoice(entry: any, id: string) {
  const record = entry.voices?.find((v: any) => v.recording_id === id);
  if (!record) throw Error("Synthetic voice receipt missing"); return record;
}
async function getVoice({scope, recordingId}: any) {
  const entry = desktopEntry(scope), record = findVoice(entry, recordingId);
  if (["staged", "ready", "applied", "dismissed", "cancelled", "failed"].includes(record.phase)) return desktopReply("loaded", entry);
  if (!record.job && voiceJournal.has(recordingId)) record.job = voiceJournal.get(recordingId);
  if (!record.job) { record.phase = "unknown"; record.message ||= "转写提交结果未知，未重复提交；请查询原任务。"; return saveVoice(entry); }
  const stage = voiceStages.get(recordingId);
  const view = await (window as any).__TAURI_INTERNALS__.invoke("asr_task_get", { taskDir: record.job.task_dir, outputRoot: stage?.output_root, jobId: record.job.job_id });
  if (["success", "completed", "succeeded"].includes(view.job?.status)) {
    const file = view.artifacts?.find((a: any) => /transcript\.txt$/i.test(a.relativePath));
    if (!file) throw Error("转写完成但没有正文文件");
    const result = await (window as any).__TAURI_INTERNALS__.invoke("asr_task_read_artifact", { taskDir: record.job.task_dir, relativePath: file.relativePath, outputRoot: stage?.output_root });
    record.transcript = result.content.trim() || null; record.phase = record.transcript ? "ready" : "failed";
    record.message = record.transcript ? "" : "未识别到语音内容；原结果保留";
  } else if (["failed", "partial", "canceled", "cancelled", "interrupted", "unknown"].includes(view.job?.status)) {
    record.phase = ["canceled", "cancelled"].includes(view.job.status) ? "cancelled" : "failed";
    record.message = view.job.error || "原转写未完整完成，未插入草稿。";
  } else if (record.phase !== "cancel_requested") { record.phase = "accepted"; record.message = "正在本地转写"; }
  return saveVoice(entry);
}
const values: Record<string, unknown> = {
  assistant_session_create: ({requestId}:any) => {
    createRequests.push({requestId});
    const id=requestId??crypto.randomUUID();
    if(!createdSessions.has(id))createdSessions.set(id,{id,title:"新对话",source:"desktop",updated_at:new Date().toISOString(),message_count:0,active_turn_id:null});
    if(Array.isArray(values.assistant_sessions_list)) {
      const existing=values.assistant_sessions_list as any[];
      if(!existing.some(s=>s.id===id))existing.unshift(createdSessions.get(id));
    }
    return createdSessions.get(id);
  },
  capability_index_query: async ({request}:any) => {
    const r={offset:0,limit:20,query:"",...request};capabilityRequests.push(structuredClone(r));
    const filters={query:r.query.trim().toLowerCase(),function:r.function??null,kind:r.kind??null};
    let catalogId=r.catalog_id,snapshotId=r.snapshot_id;
    if(!catalogId&&!snapshotId){
      discoveryStats.source_reads++;
      const raw=typeof values.capability_index_get==="function"?await (values.capability_index_get as any)():values.capability_index_get;
      const copy=structuredClone(raw);copy.entries.sort((a:any,b:any)=>{const left=[a.function,a.name,a.provider,a.entry_key??a.id].join("\0"),right=[b.function,b.name,b.provider,b.entry_key??b.id].join("\0");return left<right?-1:left>right?1:0;});
      catalogId=crypto.randomUUID();capabilityCatalogs.set(catalogId,{...copy,expires_at:new Date(Date.now()+300_000).toISOString()});
    }
    if(snapshotId){
      const view=capabilityViews.get(snapshotId);
      if(!view)throw Error("DISCOVERY_SNAPSHOT_EXPIRED: 请显式刷新");
      if(JSON.stringify(view.filters)!==JSON.stringify(filters))throw Error("DISCOVERY_FILTER_MISMATCH");
      catalogId=view.catalogId;
    }
    const catalog=capabilityCatalogs.get(catalogId);
    if(!catalog||Date.parse(catalog.expires_at)<=Date.now())throw Error("DISCOVERY_CATALOG_EXPIRED: 请显式刷新");
    if(!snapshotId){
      const labels:Record<string,string>={video:"视频 动画",text:"文字 推理",document:"文档 PDF",tools:"扩展工具",workflow:"工作流"};
      const entries=catalog.entries.filter((e:any)=>(!filters.function||e.function===filters.function)&&(!filters.kind||e.kind===filters.kind)&&`${e.name} ${e.provider} ${e.function} ${labels[e.function]??""}`.toLowerCase().includes(filters.query));
      snapshotId=crypto.randomUUID();capabilityViews.set(snapshotId,{catalogId,filters,entries});
    }
    const entries=capabilityViews.get(snapshotId).entries;
    const page={version:"homebase-capability-discovery-v1",catalog_id:catalogId,snapshot_id:snapshotId,assembled_at:new Date().toISOString(),expires_at:catalog.expires_at,filters,total:entries.length,offset:r.offset,next_offset:null as number|null,entries:entries.slice(r.offset,r.offset+r.limit),groups:[...new Set(entries.map((e:any)=>e.function))].map(f=>({function:f,total:entries.filter((e:any)=>e.function===f).length,kinds:[...new Set(entries.filter((e:any)=>e.function===f).map((e:any)=>e.kind))]})),groups_has_more:false,issues:catalog.issues,issues_total:catalog.issues.length,issues_has_more:false,source_complete:!catalog.issues.length,routing:catalog.routing};
    while(new TextEncoder().encode(JSON.stringify(page)).length>9000&&page.entries.length>1)page.entries.pop();
    const next=r.offset+page.entries.length;page.next_offset=next<entries.length?next:null;return page;
  },
  assistant_voice_begin: async ({scope, audioBase64, mimeType}: any) => {
    const stage = await (window as any).__TAURI_INTERNALS__.invoke("assistant_voice_stage", { audioBase64, mimeType });
    voiceStages.set(stage.recording_id, stage);
    const entry = desktopEntry(scope); entry.voices ??= [];
    entry.voices.push({ recording_id: stage.recording_id, audio_sha256: "a".repeat(64), phase: "staged", job: null, transcript: null, message: "" });
    return saveVoice(entry);
  },
  assistant_voice_submit: async ({scope, recordingId}: any) => {
    const entry = desktopEntry(scope), record = findVoice(entry, recordingId), stage = voiceStages.get(recordingId);
    if (record.phase !== "staged") return getVoice({scope, recordingId});
    const payload = { sourcePath: stage.source_path, outputRoot: stage.output_root, presetId: "asr_zh_balanced", engineIds: ["faster_whisper_large_v3_cpu"], allowSystemDrive: true };
    const probe = await (window as any).__TAURI_INTERNALS__.invoke("asr_environment_probe", { payload });
    if (!probe.available) { record.message = probe.error || "本地转写环境尚未就绪，录音已保存。"; return { ...saveVoice(entry), status: "failed", message: record.message }; }
    record.phase = "submitting"; saveVoice(entry);
    try {
      const job = await (window as any).__TAURI_INTERNALS__.invoke("asr_task_start", { payload: { ...payload, taskName: "助理语音输入", language: "zh", fusionMode: "manual" } });
      record.job = { job_id: job.jobId, task_dir: job.taskDir }; voiceJournal.set(recordingId, record.job); record.phase = "accepted";
    } catch (error) { record.phase = "unknown"; record.message = `转写提交结果未知，未重复提交：${String(error)}`; }
    return saveVoice(entry);
  },
  assistant_voice_get: getVoice,
  assistant_voice_apply: async ({scope, recordingId, revision}: any) => {
    const entry = desktopEntry(scope), record = findVoice(entry, recordingId);
    if (record.phase === "applied") return desktopReply("loaded", entry);
    if (entry.revision !== revision || record.phase !== "ready") return { status: "failed", message: "输入已变化或转写未就绪，原文保留。" };
    entry.body.text += `${entry.body.text ? "\n" : ""}${record.transcript}`;
    record.phase = "applied"; record.transcript = null; saveVoice(entry);
    try { await (window as any).__TAURI_INTERNALS__.invoke("assistant_voice_discard", {recordingId}); }
    catch { return { ...desktopReply("partial", entry), warning:"转写已插入，但原录音清理尚未确认；不要重复生成或追加" }; }
    return desktopReply("loaded", entry);
  },
  assistant_voice_cancel: async ({scope, recordingId}: any) => {
    const entry = desktopEntry(scope), record = findVoice(entry, recordingId);
    if (record.phase === "staged" || record.phase === "ready") { record.phase = "cancelled"; return saveVoice(entry); }
    record.phase = "cancel_requested"; saveVoice(entry);
    try { if (!record.job) throw Error("回执尚未确认"); await (window as any).__TAURI_INTERNALS__.invoke("asr_task_cancel", {jobId:record.job.job_id}); record.phase = "cancelled"; }
    catch { record.message = "取消尚未确认，原回执保留。"; }
    return saveVoice(entry);
  },
  assistant_voice_dismiss: ({scope, recordingId}: any) => { const entry = desktopEntry(scope), record = findVoice(entry, recordingId); record.phase = "dismissed"; record.transcript = null; return saveVoice(entry); },
  assistant_desktop_index: () => ({ status: "loaded", index: { entries: [...desktopStore.values()].map(e => ({ scope: e.scope, revision: e.revision, has_draft: !!e.body.text || !!e.body.files.length, pending_state: e.pending?.state ?? (e.native_pending?"prepared":null), voice_pending_count: e.voices?.filter((v:any)=>!["applied","dismissed"].includes(v.phase)).length ?? 0 })), unreadable: 0, complete: true } }),
  assistant_desktop_get: ({scope}: any) => desktopReply("loaded", desktopEntry(scope)),
  assistant_desktop_save: ({request}: any) => {
    const entry = desktopEntry(request.scope);
    if (entry.revision !== request.revision && JSON.stringify(entry.body) !== JSON.stringify(request.body))
      return { status: "failed", code: "DRAFT_SAVE_FAILED", message: "另一个窗口已保存新版本，未覆盖原内容。" };
    if (JSON.stringify(entry.body) !== JSON.stringify(request.body)) { entry.body = request.body; entry.revision++; desktopStore.set(request.scope, entry); }
    return desktopReply("saved", entry);
  },
  assistant_desktop_send: async ({request, revision}: any) => {
    const scope = request.session_id ?? "new", entry = desktopEntry(scope);
    const known = desktopReceipts.get(request.request_id);
    if (known) return desktopAccept(scope, request, known);
    if (entry.pending?.state === "prepared" && JSON.stringify(entry.pending.request) !== JSON.stringify(request))
      return { status: "failed", code: "DRAFT_PREPARE_FAILED", message: "原请求受理未知，未替换。" };
    if (!entry.pending && entry.revision !== revision)
      return { status: "failed", code: "DRAFT_PREPARE_FAILED", message: "草稿版本已变化，未执行。" };
    if (!entry.pending) {
      entry.body = desktopBodyFromRequest(request);
      entry.pending = { request, state: "prepared" }; entry.revision++; desktopStore.set(scope, entry);
    }
    const ack = await (window as any).__TAURI_INTERNALS__.invoke("assistant_send", { request });
    desktopReceipts.set(request.request_id, ack);
    return desktopAccept(scope, request, ack);
  },
  assistant_desktop_resolve: ({scope, request}: any) => {
    const entry = desktopEntry(scope), original = request ?? entry.pending?.request;
    const ack = original && desktopReceipts.get(original.request_id);
    return ack ? desktopAccept(scope, original, ack) : { ...desktopReply(entry.pending ? "not_found" : "loaded", entry), message: entry.pending ? "尚未查到原请求，未重新执行。" : "" };
  },
  assistant_native_send: async ({request,revision}:any) => {
    const scope=`native:${request.provider}:${request.thread_id}`,entry=desktopEntry(scope),key=`${request.provider}:${request.request_id}`;
    const known=nativeReceipts.get(key);
    if(known && !sameNative(known.request,request))return {status:"failed",code:"NATIVE_RECEIPT_FAILED",message:"原编号内容不匹配"};
    if(known?.result.state==="accepted")return nativeAccept(scope,request,known.result);
    if(entry.native_pending && !sameNative(entry.native_pending,request))return {status:"failed",code:"NATIVE_PREPARE_FAILED",message:"原请求待核对"};
    if(!entry.native_pending){
      if(entry.revision!==revision)return {status:"failed",code:"NATIVE_PREPARE_FAILED",message:"另一窗口已更新原生草稿，未发送"};
      entry.body.text=request.text;entry.native_pending=request;entry.revision++;desktopStore.set(scope,structuredClone(entry));
    }
    if(known)return {...desktopReply("unknown",entry),native_result:known.result,message:"原生受理未知，原编号保留"};
    try {
      const result=await (window as any).__TAURI_INTERNALS__.invoke("conversation_native",{request:{action:"send",...request}});
      const normalized={...result,thread_id:request.thread_id};
      nativeReceipts.set(key,{request,result:normalized});
      return result.state==="accepted"?nativeAccept(scope,request,normalized):{...desktopReply("unknown",entry),native_result:normalized,message:"原生受理未知，原编号保留"};
    }catch(e){return {...desktopReply("unknown",entry),message:`原生提交未确认：${String(e)}`};}
  },
  assistant_native_resolve: ({scope,request}:any) => {
    const entry=desktopEntry(scope),original=request??entry.native_pending;
    if(!original)return desktopReply("loaded",entry);
    const known=nativeReceipts.get(`${original.provider}:${original.request_id}`);
    if(known && !sameNative(known.request,original))return {status:"failed",message:"原生回执冲突，未清理"};
    return known?.result.state==="accepted"?nativeAccept(scope,original,known.result):{
      ...desktopReply(known?"unknown":"not_found",entry),native_result:known?.result??null,message:known?"原生受理未知，原编号保留":"尚未查到原生请求，不代表取消或未执行；原编号保留。"
    };
  },
  conversation_native: ({request}:any)=>{nativeRequests.push(request);if(request.action==='list')return {data:[]};throw Error('Native mutations require an explicit fixture');},
  assistant_config_get: {settings:{revision:0,enabled:false,route_id:"",wechat_enabled:false,max_rounds:8,max_tokens:2048,runtime:"pi"},workspace_root:"X:/isolated/assistant",model_state:"disabled",message:"模型暂未启用"},
  assistant_sessions_list: (window as any).__catalogOverride ?? (window as any).__catalogSeed ?? [],
  assistant_personal_get: {revision:0,skills:[],memories:[],reminders:[],followups:{revision:0,entries:[]},matters:{revision:0,entries:[]}},
  assistant_context_get: {version:"homebase-context-v1",fingerprint:"synthetic",state_revision:0,policy_revision:0,memories:[],tasks:[],authorized_workspaces:[]},
  assistant_usage_get: {scope:"隔离夹具",source:"provider_reported",turns:0,active:0,samples:0,reported_cost_usd:null,prompt_tokens:null,completion_tokens:null,cost_unknown_samples:0,prompt_unknown_samples:0,completion_unknown_samples:0,unreported_turns:0,complete:false},
  conversation_providers: {providers:[{id:"homebase",name:"HomeBase 助理",interactive:true,context:"automatic",message:"隔离夹具"},{id:"codex",name:"Codex",interactive:true,context:"automatic",message:"隔离原生夹具"},{id:"claude-code",name:"Claude Code",interactive:false,context:"read_only",message:"只读"}]},
  capability_index_get: {version:"homebase-capability-index-v1",entries:[{id:"hyperframes",function:"video",kind:"local",name:"代码动画",provider:"HyperFrames",state:"check_before_call",cost:"local_compute",discover:"capabilities_catalog",target:"hyperframes"},{id:"fixture-free",function:"text",kind:"model",name:"隔离免费模型",provider:"fixture",state:"unknown",cost:"unknown",discover:"model_capability_resolve",target:"fixture-free"}],issues:[],routing:"隔离测试"},
  repo_projects_list: {status:'succeeded',entries:[],total:0,next_offset:null},
  repo_capabilities_list: {status:'succeeded',entries:[],total:0,next_offset:null},
  model_capabilities_catalog: modelCapabilityFixture,
  capabilities_catalog: { id: "hyperframes.animation", name: "HyperFrames", version: "0.9.26", state: "not_installed", message: "尚未安装；请选择独立目录。" },
  system_preflight: [],
  secrets_lifecycle_status: [],
  secrets_list: [],
  agents_list: [],
  system_disks: [],
  system_path_list: [],
  system_processes: [],
  system_info: {
    os: "windows",
    os_version: "fixture",
    hostname: "fixture",
    username: "fixture",
    arch: "x86_64",
    gpu_status: "not_detected",
    gpu_message: "未检测到物理 GPU",
    gpu_name: null,
    memory_total_gb: 16,
    cpu_cores: 8,
  },
  system_metrics: {
    cpu_percent: 0,
    memory_percent: 20,
    memory_total_gb: 16,
    memory_used_gb: 3,
    cpu_cores: 8,
    cpu_warning: false,
    memory_warning: false,
  },
  system_gpu: {
    available: false,
    gpu_names: [],
    gpu_status: "not_detected",
    gpu_message: "未检测到物理 GPU",
    gpu_name: null,
    gpu_percent: null,
  },
  agents_history_all: { entries: [], buckets: [] },
  studio_list_sessions: [],
  agent_versions_check: [],
  shared_mcp_get: { servers: [], assignments: {} },
  shared_skills_list: [],
  shared_secrets_ipc_endpoint: null,
  shared_secrets_list_names: [],
  shared_secrets_acl_list: {
    entries: {},
    matrix: { agents: [], secrets: [], cells: [] },
  },
  wiki_list: [],
  wiki_dirs: [],
  docs_list: [],
  topology_load: { roots: [], edges: [], drive_groups: {} },
  topology_parse_refs: [],
  inbox_list_recent: [],
  wiki_agent_status: {
    pending_count: 0,
    digested_count: 0,
    total_count: 0,
    vault_root: "synthetic",
    has_agents_md: false,
    backend: "claude-code",
  },
  governance_audit_recent: [],
  governance_snapshots_list: [],
  governance_capability_matrix: { rows: [] },
  governance_config_ownership: { entries: [] },
  governance_interface_registry: [],
  model_center_manifest: {
    source_path: "synthetic",
    manifest_state: "missing",
    models: [],
    error: null,
  },
  model_download_reconcile: [],
  asr_task_reconcile: [],
  asr_task_list: [],
  openrouter_models_catalog: {
    fetched_at: "synthetic",
    source_url: "synthetic",
    groups: [],
  },
};
const allowedFailures = new Set([
  "gdnbit_doctor",
  "gdnbit_get_config",
  "obsidian_health",
  "distribution_info",
]);
const rejected: string[] = [];
const callbacks = new Map<number, (event: unknown) => void>();
const listeners = new Map<number, { event: string; handler: number }>();
const registrations: string[] = [];
let nextId = 1;
function emit(event: string, payload: unknown) {
  for (const [id, listener] of [...listeners]) {
    if (listener.event === event)
      callbacks.get(listener.handler)?.({ id, event, payload });
  }
}
(window as any).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
  unregisterListener: (_event: string, id: number) => {
    listeners.delete(id);
  },
};
(window as any).__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "main" } },
  transformCallback: (callback: (event: unknown) => void) => {
    const id = nextId++;
    callbacks.set(id, callback);
    return id;
  },
  unregisterCallback: (id: number) => {
    callbacks.delete(id);
  },
  invoke: async (command: string, args: any) => {
    if (command === "plugin:event|listen") {
      if (args.event === "tauri://close-requested") {
        closeRegistrationAttempts++;
        if (closeRegistrationAttempts === 1 && (window as any).__rejectDraftCloseListener) {
          throw Error("synthetic first registration failed");
        }
      }
      const id = nextId++;
      listeners.set(id, args);
      registrations.push(args.event);
      return id;
    }
    if (command === "plugin:event|unlisten") {
      listeners.delete(args.eventId);
      return;
    }
    calls.push(command);
    if(command === "assistant_sessions_query" && !(command in values))return sessionCatalog.query(args);
    if (command === "assistant_session_window_get" && !(command in values)) {
      const request = args.request;
      windowRequests.push(structuredClone(request));
      const raw = (values as any).assistant_session_get;
      const original = structuredClone(createdSessions.has(request.session_id) ? {...createdSessions.get(request.session_id),messages:[],turns:[]} : typeof raw === "function" ? await raw({sessionId:request.session_id}) : raw ?? {id:request.session_id,source:"desktop",messages:[],turns:[]});
      const end = request.before_message_id ? original.messages.findIndex((m:any)=>m.id===request.before_message_id) : original.messages.length;
      if (end < 0) throw Error("WINDOW_CURSOR_NOT_FOUND: synthetic cursor missing");
      const start = Math.max(0,end-(request.limit??40));
      const messages = original.messages.slice(start,end);
      const ids = new Set(messages.map((m:any)=>m.turn_id));
      const active = original.turns.find((t:any)=>["queued","running","waiting_jobs","awaiting_confirmation"].includes(t.state));
      const queued = original.turns.find((t:any)=>t.state==="waiting_input_queue");
      if(active)ids.add(active.id);if(queued)ids.add(queued.id);
      return {id:original.id??request.session_id,title:original.title??"",source:original.source,queue_resume_generation:original.queue_resume_generation,messages,turns:original.turns.filter((t:any)=>ids.has(t.id)).map((t:any)=>({turn:{...t,session_id:t.session_id??request.session_id,steps:[],pending:null,jobs:(t.jobs??[]).slice(0,8).map((j:any)=>({...j,result:null,artifacts:[],remote:null})),artifacts:(t.artifacts??[]).slice(0,16)},details_omitted:true,step_count:(t.steps??[]).length,job_count:(t.jobs??[]).length,artifact_count:(t.artifacts??[]).length})),page:{version:"homebase-session-window-v1",captured_revision:Date.now(),total_messages:original.messages.length,start_index:start,end_index:end,returned:messages.length,requested_limit:request.limit??40,before_message_id:start>0?messages[0].id:null,has_more:start>0}};
    }
    if (["assistant_turn_inspect", "assistant_turn_step_get"].includes(command) && !(command in values)) {
      const original = values.assistant_turn_get;
      const turn = typeof original === "function" ? await original({ turnId: args.turnId }) : original;
      if (!turn || turn.id !== args.turnId) throw Error("隔离轮次不存在");
      const sessionId = turn.session_id ?? args.sessionId;
      const steps = turn.steps ?? [];
      if (command === "assistant_turn_step_get") {
        const step = steps.find((item: { id: string }) => item.id === args.stepId);
        if (!step) throw Error("隔离步骤不存在");
        return structuredClone({ session_id: sessionId, turn_id: turn.id, step });
      }
      const { jobs, model_diagnostics: _diagnostics, ...inspection } = turn;
      return structuredClone({
        ...inspection, session_id: sessionId, details_omitted: true, steps: [],
        step_count: steps.length,
        step_summaries: steps.map(({ id, name, state }: { id: string; name: string; state: string }) => ({ id, name, state })),
        jobs: (jobs ?? []).slice(0, 8).map((job: any) => ({ ...job, result: null, artifacts: [], remote: null })),
      });
    }
    if (command in values) {
      const value =
        typeof values[command] === "function"
          ? await (values[command] as any)(args)
          : values[command];
      if (command === "assistant_turn_get" && args.includeDetails === false && value?.details_omitted !== true && value) {
        const { steps, jobs, model_diagnostics: _diagnostics, ...progress } = value;
        return structuredClone({
          ...progress,
          details_omitted: true,
          step_count: steps?.length ?? 0,
          steps: [],
          jobs: (jobs ?? []).slice(0, 8).map((job: any) => ({ ...job, result: null, artifacts: [], remote: null })),
        });
      }
      return structuredClone(value);
    }
    if (!allowedFailures.has(command)) rejected.push(command);
    throw Error("Synthetic unavailable; real IPC forbidden");
  },
};
const inboxFixture = inboxWorkspaceFixture(values);
const sessionCatalog = sessionCatalogFixture(async()=>typeof values.assistant_sessions_list === 'function' ? await (values.assistant_sessions_list as any)({}) : values.assistant_sessions_list);
const client = createQueryClient();
(window as any).__performance = {
  calls,
  inboxFixture,
  sessionCatalog,
  windowRequests,
  nativeRequests,
  capabilityRequests,
  capabilityCatalogs,
  capabilityViews,
  discoveryStats,
  desktopStore,
  createdSessions,
  createRequests,
  desktopReceipts,
  nativeReceipts,
  voiceStages,
  voiceJournal,
  client,
  startPolling,
  rejected,
  values,
  emit,
  listeners,
  registrations,
  get closeRegistrationAttempts() { return closeRegistrationAttempts; },
  changeRecoveryRoots: () => {
    const config = createDefaultAsrPortableConfig({
      preferredModelRoot: "X:\\RootB\\models",
      preferredModelDrive: "X:",
    } as any);
    config.roots.aihub_root = "X:\\RootB";
    config.roots.output_root = "X:\\RootB\\output";
    localStorage.setItem(
      "homebase.asr.portable-profile.v1",
      JSON.stringify(exportAsrPortableBundle(config)),
    );
    window.dispatchEvent(
      new CustomEvent("homebase:asr-portable-config", { detail: config }),
    );
  },
};
function FixtureLocation() {
  const location=useLocation();
  (window as any).__performance.location=location;
  return null;
}
createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[(window as any).__initialRoute??"/"]}>
        <FixtureLocation />
        <App />
      </MemoryRouter>
    </QueryClientProvider>
  </React.StrictMode>,
);
