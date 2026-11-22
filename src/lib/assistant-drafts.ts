import { invoke } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";
import type { AssistantAck } from "./assistant";
import type { NativeReply } from "./conversations";
import type { DocumentReference } from "./document-types";
import { inputBytes, MAX_DRAFT_BYTES } from "./chat-input-limits";
import { isNativeDraftClosePending } from "./draft-close-state";

// Private input lives in memory and the OS-protected desktop store, never browser storage.
export type ChatMode = "auto" | "chat" | "task";
export interface TextAttachment {
  name: string;
  content: string;
  document?: DocumentReference;
}
export interface ImageReference {
  scope: string;
  name: string;
  sha256: string;
  bytes: number;
}
export interface ChatDraft {
  text: string;
  route: string;
  files: TextAttachment[];
  mode: ChatMode;
  image?: ImageReference;
}
export interface PendingSend {
  text: string;
  session: string | null;
  id: string;
  mode: ChatMode;
  options: { route_id?: string; attachments?: TextAttachment[]; image?: ImageReference };
}
interface WireRequest {
  session_id: string | null;
  text: string;
  request_id: string;
  mode?: ChatMode;
  route_id?: string;
  attachments?: TextAttachment[];
  image?: ImageReference;
}
export interface NativeMessage {
  provider: string;
  thread_id: string;
  text: string;
  request_id: string;
}
export interface DesktopEntry {
  scope: string;
  revision: number;
  body: ChatDraft;
  voices?: VoiceRecord[];
  native_pending?: NativeMessage | null;
  pending: {
    request: WireRequest;
    state: "prepared" | "rejected";
    error?: string | null;
  } | null;
}
export interface VoiceRecord {
  recording_id: string;
  audio_sha256: string;
  phase:
    | "staged"
    | "submitting"
    | "accepted"
    | "ready"
    | "applied"
    | "cancel_requested"
    | "cancelled"
    | "failed"
    | "unknown"
    | "dismissed";
  job: { job_id: string; task_dir: string } | null;
  transcript: string | null;
  message: string;
}
interface DesktopReply {
  status: string;
  code?: string;
  message: string;
  entry?: DesktopEntry | null;
  ack?: AssistantAck | null;
  native_result?: NativeReply | null;
  warning?: string | null;
  index?: DesktopIndex | null;
}
export interface DesktopIndex {
  entries: {
    scope: string;
    revision: number;
    has_draft: boolean;
    pending_state: string | null;
    voice_pending_count?: number;
    route: string;
    updated_at: string | null;
  }[];
  unreadable: number;
  complete: boolean;
}
export async function draftIndex(): Promise<DesktopIndex> {
  const reply = await invoke<DesktopReply>("assistant_desktop_index");
  if (!reply.index)
    throw Error(reply.message || "草稿列表读取失败，不能作为空列表。");
  return reply.index;
}
interface DraftState {
  draft: ChatDraft;
  voices: VoiceRecord[];
  revision: number;
  loaded: boolean;
  loading: boolean;
  dirty: boolean;
  status: "loading" | "saved" | "saving" | "error";
  error: string;
  warning: string;
  pending: PendingSend | null;
  nativePending: NativeMessage | null;
  pendingState: "prepared" | "rejected" | null;
  unconfirmed: boolean;
  unconfirmedSave: { revision: number; draft: ChatDraft } | null;
  epoch: number;
}
const states = new Map<string, DraftState>();
const listeners = new Map<string, Set<() => void>>();
const queues = new Map<string, Promise<unknown>>();
const pendingWrites = new Map<string, Promise<unknown>>();
const loads = new Map<string, Promise<void>>();
const timers = new Map<string, ReturnType<typeof setTimeout>>();
export const draftKey = (session: string | null) => session ?? "new";
const empty = (): ChatDraft => ({
  text: "",
  route: "",
  files: [],
  mode: "auto",
});
function state(key: string): DraftState {
  if (!states.has(key))
    states.set(key, {
      draft: empty(),
      voices: [],
      revision: 0,
      loaded: false,
      loading: false,
      dirty: false,
      status: "loading",
      error: "",
      warning: "",
      pending: null,
      nativePending: null,
      pendingState: null,
      unconfirmed: false,
      unconfirmedSave: null,
      epoch: 0,
    });
  return states.get(key)!;
}
function put(key: string, patch: Partial<DraftState>) {
  states.set(key, { ...state(key), ...patch });
  listeners.get(key)?.forEach((fn) => fn());
}
function serial<T>(
  key: string,
  work: () => Promise<T>,
  access: "write" | "read" = "write",
): Promise<T> {
  const next = (queues.get(key) ?? Promise.resolve())
    .catch(() => {})
    .then(work);
  const tail = next.catch(() => {});
  queues.set(key, tail);
  if (access === "write") {
    pendingWrites.set(key, tail);
    void tail.then(() => {
      if (pendingWrites.get(key) === tail) pendingWrites.delete(key);
    });
  }
  return next;
}
export function useDraftState(session: string | null) {
  const key = draftKey(session);
  return useSyncExternalStore(
    (fn) => {
      const set = listeners.get(key) ?? new Set();
      listeners.set(key, set);
      set.add(fn);
      return () => {
        set.delete(fn);
      };
    },
    () => state(key),
  );
}
const wire = (r: PendingSend): WireRequest => ({
  session_id: r.session,
  text: r.text,
  request_id: r.id,
  mode: r.mode,
  ...r.options,
});
const pending = (r: WireRequest): PendingSend => ({
  session: r.session_id,
  text: r.text,
  id: r.request_id,
  mode: r.mode ?? "auto",
  options: {
    ...(r.route_id ? { route_id: r.route_id } : {}),
    ...(r.attachments?.length ? { attachments: r.attachments } : {}),
    ...(r.image ? { image: r.image } : {}),
  },
});
function isOriginalDraft(draft: ChatDraft, request: PendingSend) {
  return (
    draft.text === request.text &&
    draft.mode === request.mode &&
    draft.route === (request.options.route_id ?? "") &&
    JSON.stringify(draft.files) === JSON.stringify(request.options.attachments ?? []) &&
    JSON.stringify(draft.image) === JSON.stringify(request.options.image)
  );
}
function sameDraftBody(left: ChatDraft, right: ChatDraft) {
  return left.text === right.text && left.route === right.route && left.mode === right.mode &&
    JSON.stringify(left.files) === JSON.stringify(right.files) && JSON.stringify(left.image) === JSON.stringify(right.image);
}
const nativeScope = (request: NativeMessage) =>
  `native:${request.provider}:${request.thread_id}`;

function sameNativeRequest(left: NativeMessage, right: NativeMessage) {
  return left.provider === right.provider && left.thread_id === right.thread_id &&
    left.text === right.text && left.request_id === right.request_id;
}

function requireEntryScope(key: string, entry: DesktopEntry) {
  if (entry.scope !== key)
    throw Error("草稿会话不一致，未覆盖当前输入。");
  if (entry.native_pending && nativeScope(entry.native_pending) !== key)
    throw Error("原生待核对请求与草稿归属不一致，未覆盖当前输入。");
}

function apply(
  key: string,
  entry: DesktopEntry,
  epoch: number,
  keepPending = false,
) {
  requireEntryScope(key, entry);
  const current = state(key),
    same = current.epoch === epoch;
  const unconfirmedSave = same ? null : current.unconfirmedSave;
  put(key, {
    revision: entry.revision,
    voices: entry.voices ?? [],
    loaded: true,
    loading: false,
    ...(same ? { draft: entry.body, dirty: false } : {}),
    pending: entry.pending
      ? pending(entry.pending.request)
      : keepPending
        ? current.pending
        : null,
    pendingState:
      entry.pending?.state ?? (keepPending ? current.pendingState : null),
    nativePending:
      entry.native_pending ?? (keepPending ? current.nativePending : null),
    unconfirmed:
      keepPending &&
      !entry.pending &&
      !entry.native_pending &&
      !!(current.pending || current.nativePending),
    unconfirmedSave,
    status: unconfirmedSave ? "error" : same || !current.dirty ? "saved" : "saving",
    error: unconfirmedSave ? current.error : "",
  });
}
function observeSavedDraft(key: string, entry: DesktopEntry) {
  requireEntryScope(key, entry);
  const current = state(key);
  const attempt = current.unconfirmedSave;
  const matchesCurrent = sameDraftBody(current.draft, entry.body);
  const matchesAttempt = attempt && entry.revision === attempt.revision + 1 && sameDraftBody(attempt.draft, entry.body);
  if (current.dirty && current.revision !== entry.revision && !matchesCurrent && !matchesAttempt)
    throw Error("另一个窗口已保存新版本。当前输入保留，请先查看已保存版本。");
  apply(key, entry, !current.dirty || matchesCurrent ? current.epoch : -1, current.unconfirmed);
}
export function ensureDraft(
  session: string | null,
  refresh = false,
): Promise<void> {
  const key = draftKey(session);
  if (loads.has(key)) return loads.get(key)!;
  if (state(key).loaded && !refresh) return Promise.resolve();
  const task = serial(key, async () => {
    put(key, { loading: true });
    try {
      const reply = await invoke<DesktopReply>("assistant_desktop_get", {
        scope: key,
      });
      if (!reply.entry)
        throw Error(reply.message || "草稿读取未确认，原内容保留。");
      observeSavedDraft(key, reply.entry);
    } catch (e) {
      put(key, { loading: false, status: "error", error: String(e) });
    }
  }, "read").finally(() => loads.delete(key));
  loads.set(key, task);
  return task;
}
function cancelTimer(key: string) {
  clearTimeout(timers.get(key));
  timers.delete(key);
}
export function updateDraft(session: string | null, patch: Partial<ChatDraft>, expectedText?: string) {
  const key = draftKey(session),
    current = state(key);
  if (expectedText !== undefined && (!current.loaded || current.draft.text !== expectedText))
    throw Error("草稿已变化，未按旧资料整理；请核对当前内容后重试。");
  if (!current.loaded) return;
  const draft = { ...current.draft, ...patch };
  if (JSON.stringify(draft) === JSON.stringify(current.draft)) return;
  put(key, {
    draft,
    dirty: true,
    epoch: current.epoch + 1,
    status: current.unconfirmed || current.unconfirmedSave ? "error" : "saving",
  });
  cancelTimer(key);
  if (current.unconfirmedSave) return;
  timers.set(
    key,
    setTimeout(() => {
      void flushDraft(session).catch(() => {});
    }, 800),
  );
}
export function flushDraft(session: string | null): Promise<void> {
  const key = draftKey(session);
  cancelTimer(key);
  return serial(key, () => saveDraft(key));
}
async function saveDraft(key: string, reconciled = false) {
  if (isNativeDraftClosePending()) return;
  const current = state(key);
  if (!current.loaded || !current.dirty) return;
  if (current.unconfirmed) {
    const message = "请先查询原请求，受理情况未知时不会覆盖发送回执。";
    put(key, { status: "error", error: message });
    throw Error(message);
  }
  if (current.unconfirmedSave && !reconciled)
    throw Error("草稿保存尚未确认，请先核对原版本或重试保存。");
  if (inputBytes(current.draft.text) > MAX_DRAFT_BYTES) {
    const message = "草稿正文超过 64 KiB，尚未保存。请缩短后保存，已保存的原稿保留。";
    put(key, { status: "error", error: message });
    throw Error(message);
  }
  put(key, { status: "saving" });
  try {
    const reply = await invoke<DesktopReply>("assistant_desktop_save", {
      request: { scope: key, revision: current.revision, body: current.draft },
    });
    if (!reply.entry) throw Error(reply.message || "草稿保存未确认。");
    requireEntryScope(key, reply.entry);
    if (!sameDraftBody(reply.entry.body, current.draft)) throw Error("保存回执内容与本次草稿不一致，当前输入保留。");
    put(key, { unconfirmedSave: null });
    apply(key, reply.entry, current.epoch);
  } catch (error) {
    put(key, { status: "error", error: String(error), unconfirmedSave: { revision: current.revision, draft: current.draft } });
    throw error;
  }
}
export function retryDraftSave(session: string | null): Promise<void> {
  const key = draftKey(session);
  cancelTimer(key);
  return serial(key, async () => {
    if (isNativeDraftClosePending()) return;
    if (state(key).unconfirmedSave) observeSavedDraft(key, await savedDraft(session));
    await saveDraft(key, true);
    // Edits made while the reconciled write was in flight had no autosave timer.
    if (state(key).dirty && !state(key).unconfirmedSave) await saveDraft(key);
  });
}
export const readDraft = (session: string | null) =>
  state(draftKey(session)).draft;

// Read the latest files after asynchronous preparation, never a render's snapshot.
export function appendDraftFiles(
  session: string | null,
  incoming: TextAttachment[],
): void {
  const current = state(draftKey(session));
  if (
    !current.loaded ||
    current.loading ||
    current.unconfirmed ||
    current.pending ||
    current.nativePending
  )
    throw Error("草稿或原请求尚待核对，附件未加入；当前内容保留。");
  if (current.draft.image)
    throw Error("图片消息只包含一张 PNG，请先移除图片再加入文字附件。");
  const documentIds = new Set(
    incoming.map((f) => f.document?.document_id).filter(Boolean),
  );
  const next = [
    ...current.draft.files.filter(
      (f) => !f.document || !documentIds.has(f.document.document_id),
    ),
    ...incoming,
  ];
  if (
    next.length > 4 ||
    next.reduce((n, f) => n + inputBytes(f.content), 0) > 32 * 1024
  )
    throw Error("附件至多 4 个，文字总内容不能超过 32 KiB；当前附件保留，请先移除附件或缩小选择范围。");
  updateDraft(session, { files: next });
}

// Prepare a separate text handoff without replacing any existing input or ACK.
// A retry confirms an identical stored body after a lost save response.
export function prepareHandoffDraft(session: string, text: string): Promise<void> {
  if (!text.trim() || inputBytes(text) > MAX_DRAFT_BYTES)
    return Promise.reject(Error("交接内容为空或超过64 KiB，原内容保留，请先缩短。"));
  const key=draftKey(session), desired:ChatDraft={text,route:"",files:[],mode:"auto"};
  const same=(body:ChatDraft)=>body.text===text&&body.route===""&&body.mode==="auto"&&body.files.length===0&&!body.image;
  const blank=(body:ChatDraft)=>body.text===""&&body.route===""&&body.mode==="auto"&&body.files.length===0&&!body.image;
  const protectedVoice=(voices:VoiceRecord[])=>voices.some(v=>!["applied","dismissed","cancelled"].includes(v.phase));
  return serial(key,async()=>{
    cancelTimer(key);
    const epoch=state(key).epoch;
    const reply=await invoke<DesktopReply>("assistant_desktop_get",{scope:key});
    const entry=reply.entry,current=state(key);
    if(!entry||entry.scope!==key)throw Error(reply.message||"交接草稿读取未确认，原内容保留。");
    if(current.epoch!==epoch)throw Error("草稿读取期间已有新输入，交接未覆盖它。");
    if(current.unconfirmed||current.pending||current.nativePending||entry.pending||entry.native_pending||protectedVoice(current.voices)||protectedVoice(entry.voices??[]))
      throw Error("原请求或录音尚待核对，交接未覆盖它；请先核对原草稿。");
    if((!blank(current.draft)&&!same(current.draft))||(!blank(entry.body)&&!same(entry.body)))
      throw Error("这份草稿已有不同内容，交接未覆盖它；请先打开核对。");
    if(same(entry.body)){apply(key,entry,epoch);return;}
    const changed=epoch+1;
    put(key,{draft:desired,revision:entry.revision,loaded:true,loading:false,dirty:true,epoch:changed,status:"saving",error:""});
    try{
      const saved=await invoke<DesktopReply>("assistant_desktop_save",{request:{scope:key,revision:entry.revision,body:desired}});
      if(!saved.entry||saved.entry.scope!==key||!same(saved.entry.body)||saved.entry.revision<entry.revision)
        throw Error(saved.message||"交接保存尚未确认，原页面内容与待保存草稿保留。");
      apply(key,saved.entry,changed);
      if(state(key).dirty||!same(state(key).draft))throw Error("保存期间已有新输入，已保留当前修改，请打开草稿核对。");
    }catch(error){put(key,{status:"error",error:String(error)});throw error;}
  });
}
export function clearHandoffCopy(scope:string,expectedText:string):Promise<void>{
  if(!scope.startsWith("handoff:"))return Promise.reject(Error("只能清理已转入的交接副本。"));
  return serial(scope,async()=>{
    const current=state(scope),epoch=current.epoch;
    const reply=await invoke<DesktopReply>("assistant_desktop_get",{scope});
    const entry=reply.entry;
    if(!entry||entry.scope!==scope||entry.pending||entry.native_pending||entry.body.text!==expectedText||entry.body.files.length||entry.body.route||entry.body.mode!=="auto"||current.epoch!==epoch||current.pending||current.nativePending||current.unconfirmed||current.voices.some(v=>!["applied","dismissed","cancelled"].includes(v.phase))||(entry.voices??[]).some(v=>!["applied","dismissed","cancelled"].includes(v.phase)))
      throw Error("原交接副本需要核对，已保留。");
    const saved=await invoke<DesktopReply>("assistant_desktop_save",{request:{scope,revision:entry.revision,body:{text:"",route:"",files:[],mode:"auto"}}});
    if(!saved.entry||saved.entry.scope!==scope||saved.entry.body.text)throw Error("交接副本清理未确认，目标草稿已保留。");
    apply(scope,saved.entry,epoch);
  });
}
export const pendingSend = (session: string | null) =>
  state(draftKey(session)).pending;
export const savedVoice = (session: string | null, id: string) =>
  state(draftKey(session)).voices.find((voice) => voice.recording_id === id);
export function sendDraft(request: PendingSend): Promise<AssistantAck> {
  const key = draftKey(request.session);
  cancelTimer(key);
  return serial(key, async () => {
    const current = state(key);
    if (!current.loaded) throw Error("草稿尚未读取，不会发送消息。");
    if (current.unconfirmedSave && current.pendingState !== "prepared")
      throw Error("草稿保存尚未确认，请先核对或重试保存，再发送消息。");
    if (
      current.pending &&
      current.pendingState === "prepared" &&
      JSON.stringify(wire(current.pending)) !== JSON.stringify(wire(request))
    )
      throw Error(
        "原请求受理情况尚未确认，请先查询原请求；也可以新建对话，原稿会保留。",
      );
    put(key, {
      pending: request,
      pendingState: "prepared",
      unconfirmed: true,
      error: "",
      warning: "",
    });
    let reply: DesktopReply;
    try {
      reply = await invoke<DesktopReply>("assistant_desktop_send", {
        request: wire(request),
        revision: current.revision,
      });
    } catch (e) {
      put(key, {
        status: "error",
        error: `${String(e)} · 受理情况未知，原请求已保留，请查询原请求。`,
      });
      throw e;
    }
    if (reply.entry) apply(key, reply.entry, current.epoch);
    if (reply.ack) {
      put(key, {
        warning: reply.warning ? `${reply.message}：${reply.warning}` : "",
        error: "",
      });
      return reply.ack;
    }
    if (
      !reply.entry &&
      ["DRAFT_PREPARE_FAILED", "MODEL_ROUTE_INVALID", "SCOPE_INVALID"].includes(
        reply.code ?? "",
      )
    )
      put(key, {
        pending: current.pending,
        pendingState: current.pendingState,
        unconfirmed: current.unconfirmed,
      });
    const message = reply.message || "发送未确认，原请求保留。";
    put(key, { status: "error", error: message, warning: reply.warning ?? "" });
    throw Error(message);
  });
}
export function resolveDraft(
  session: string | null,
): Promise<AssistantAck | null> {
  const key = draftKey(session);
  return serial(key, async () => {
    const current = state(key);
    try {
      const reply = await invoke<DesktopReply>("assistant_desktop_resolve", {
        scope: key,
        request: current.pending ? wire(current.pending) : null,
      });
      if (!reply.entry && !reply.ack)
        throw Error(reply.message || "原请求查询未确认。");
      if (reply.entry)
        apply(
          key,
          reply.entry,
          current.dirty &&
            !(
              reply.ack &&
              current.pending &&
              isOriginalDraft(current.draft, current.pending)
            )
            ? -1
            : current.epoch,
          !reply.ack && current.unconfirmed,
        );
      put(key, { warning: reply.warning || reply.message, error: "" });
      return reply.ack ?? null;
    } catch (e) {
      put(key, { status: "error", error: String(e) });
      throw e;
    }
  });
}
export async function savedDraft(
  session: string | null,
): Promise<DesktopEntry> {
  const scope = draftKey(session);
  const reply = await invoke<DesktopReply>("assistant_desktop_get", {
    scope,
  });
  if (!reply.entry) throw Error(reply.message || "已保存版本读取失败。");
  requireEntryScope(scope, reply.entry);
  return reply.entry;
}
export function adoptSavedDraft(session: string | null, entry: DesktopEntry) {
  const key = draftKey(session);
  requireEntryScope(key, entry);
  const current = state(key);
  if (
    current.unconfirmed ||
    current.pendingState === "prepared" ||
    current.nativePending
  )
    throw Error("原请求尚未核对，不能通过替换草稿清除其回执；请先查询原请求。");
  cancelTimer(key);
  put(key, { epoch: current.epoch + 1 });
  apply(key, entry, state(key).epoch);
}

export async function flushAllDrafts(): Promise<void> {
  // A clean scope with only a read in flight has no input to save. A queued
  // mutation still needs protection, even if its draft was already saved.
  const results = await Promise.allSettled(
    [...states]
      .filter(([key, draft]) => draft.dirty || pendingWrites.has(key))
      .map(([key]) => flushDraft(key === "new" ? null : key)),
  );
  if (results.some((result) => result.status === "rejected"))
    throw Error("部分输入尚未安全保存，请返回助理核对草稿或原请求。");
}
export const hasUnsavedDrafts = () =>
  [...states.values()].some((value) => value.loaded && value.dirty);

export const pendingNativeSend = (scope: string) => state(scope).nativePending;

function applyNativeReply(
  key: string,
  reply: DesktopReply,
  epoch: number,
  request: NativeMessage | null,
) {
  if (reply.entry) {
    requireEntryScope(key, reply.entry);
    if (reply.entry.native_pending && request &&
      !sameNativeRequest(reply.entry.native_pending, request))
      throw Error("原生回执的请求内容或编号不一致，保留原请求待核对。");
  }
  const result = reply.native_result;
  if (result && (result.state === "accepted" || result.thread_id !== undefined) &&
    (!request || result.thread_id !== request.thread_id))
    throw Error("原生回执的会话编号不一致，保留原请求待核对。");
  const current = state(key),
    accepted = result?.state === "accepted";
  if (
    reply.entry &&
    current.dirty &&
    reply.entry.revision !== current.revision &&
    reply.entry.body.text !== current.draft.text &&
    reply.entry.body.text !== request?.text &&
    !(accepted && reply.entry.body.text === "")
  ) {
    put(key, {
      nativePending: reply.entry.native_pending ?? null,
      unconfirmed: !accepted && current.unconfirmed,
      status: "error",
      error:
        "另一窗口已保存不同的原生草稿。当前输入保留，请先查看已保存版本；未覆盖新稿。",
      warning: accepted
        ? "原消息已受理；草稿版本冲突需单独核对。"
        : reply.message,
    });
    return;
  }
  if (reply.entry)
    apply(
      key,
      reply.entry,
      current.dirty && !(accepted && current.draft.text === request?.text)
        ? -1
        : epoch,
      !accepted && current.unconfirmed,
    );
  put(key, {
    warning: reply.warning
      ? `${reply.message}：${reply.warning}`
      : reply.message,
    error: "",
  });
}

export function sendNativeDraft(request: NativeMessage): Promise<NativeReply> {
  const key = nativeScope(request);
  cancelTimer(key);
  return serial(key, async () => {
    const current = state(key);
    if (!current.loaded) throw Error("原生草稿尚未读取，不会发送。");
    if (current.unconfirmedSave && !current.nativePending)
      throw Error("原生草稿保存尚未确认，请先核对或重试保存，再发送消息。");
    if (
      current.nativePending && !sameNativeRequest(current.nativePending, request)
    )
      throw Error("原生请求尚待核对，不能更换编号或内容；请先查询原请求。");
    put(key, {
      nativePending: request,
      unconfirmed: true,
      error: "",
      warning: "",
    });
    try {
      const reply = await invoke<DesktopReply>("assistant_native_send", {
        request,
        revision: current.revision,
      });
      if (reply.entry || reply.native_result)
        applyNativeReply(key, reply, current.epoch, request);
      if (reply.native_result?.state === "accepted") return reply.native_result;
      if (
        !reply.entry &&
        !reply.native_result &&
        [
          "NATIVE_PREPARE_FAILED",
          "NATIVE_REQUEST_INVALID",
          "NATIVE_RECEIPT_FAILED",
        ].includes(reply.code ?? "")
      )
        put(key, {
          nativePending: current.nativePending,
          unconfirmed: current.unconfirmed,
        });
      const message =
        reply.message || "原生受理情况未知，原编号保留，请查询原请求。";
      put(key, {
        status: "error",
        error: message,
        warning: reply.warning ?? "",
      });
      throw Error(message);
    } catch (e) {
      put(key, { status: "error", error: String(e) });
      throw e;
    }
  });
}

export function resolveNativeDraft(scope: string): Promise<NativeReply | null> {
  return serial(scope, async () => {
    const current = state(scope);
    if (!current.loaded) throw Error("原生输入尚未读取，未核对请求。");
    try {
      const reply = await invoke<DesktopReply>("assistant_native_resolve", {
        scope,
        request: current.nativePending,
      });
      if (!reply.entry && !reply.native_result)
        throw Error(reply.message || "原生回执读取未确认。");
      applyNativeReply(scope, reply, current.epoch, current.nativePending);
      return reply.native_result ?? null;
    } catch (e) {
      put(scope, { status: "error", error: String(e) });
      throw e;
    }
  });
}

// Voice state and typing use one per-scope queue, so status observations cannot
// cause stale revisions or overwrite newer, unsaved text.
export async function voiceRequest(
  session: string | null,
  action: "begin" | "get" | "submit" | "apply" | "dismiss" | "cancel",
  args: Record<string, unknown> = {},
) {
  if (action === "apply") await flushDraft(session);
  const key = draftKey(session);
  return serial(key, async () => {
    const current = state(key);
    if (!current.loaded) throw Error("输入尚未读取，不会更改录音状态。");
    const reply = await invoke<DesktopReply>(`assistant_voice_${action}`, {
      ...args,
      scope: key,
      ...(action === "apply" ? { revision: current.revision } : {}),
    });
    if (!reply.entry)
      throw Error(reply.message || "录音状态未确认，原回执保留。");
    apply(
      key,
      reply.entry,
      current.dirty ? -1 : current.epoch,
      current.unconfirmed,
    );
    if (reply.warning) put(key, { warning: reply.warning });
    if (reply.status === "failed")
      throw Error(reply.message || "录音操作未完成，原回执保留。");
    return reply;
  });
}
