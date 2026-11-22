import { invoke } from "@tauri-apps/api/core";
import { useEffect, useSyncExternalStore } from "react";
import type { InboxSubmissionView } from "./tauri-api";
import { isNativeDraftClosePending } from "./draft-close-state";

export interface InboxSourceDraft {
  text: string;
  title: string;
  tags_input: string;
  source_hint: string;
  attachments: { rel: string; name: string; size: number }[];
  attachment_unconfirmed: boolean;
}
interface Pending {
  submission_id: string;
  snapshot: InboxSourceDraft;
  stage: "reserved" | "ready";
}
interface Entry {
  schema_version: number;
  revision: number;
  draft: InboxSourceDraft;
  pending: Pending | null;
  last_submission_id: string | null;
}
interface Reply {
  entry: Entry;
  submission: InboxSubmissionView | null;
  observation: "not_needed" | "not_checked" | "loaded" | "failed";
  observation_error: string | null;
}
interface State {
  draft: InboxSourceDraft;
  loaded: boolean;
  loading: boolean;
  dirty: boolean;
  busy: boolean;
  conflict: boolean;
  error: string;
  entry: Entry | null;
  submission: InboxSubmissionView | null;
  unconfirmedSave: { revision: number; draft: InboxSourceDraft } | null;
  attempt: { submission_id: string; snapshot: InboxSourceDraft } | null;
}
const empty = (): InboxSourceDraft => ({ text: "", title: "", tags_input: "", source_hint: "", attachments: [], attachment_unconfirmed: false });
let state: State = { draft: empty(), loaded: false, loading: false, dirty: false, busy: false, conflict: false, error: "", entry: null, submission: null, unconfirmedSave: null, attempt: null };
const listeners = new Set<() => void>();
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const clone = <T,>(v: T): T => structuredClone(v);
const publish = (patch: Partial<State>) => { state = { ...state, ...patch }; listeners.forEach(fn => fn()); };
let tail: Promise<unknown> = Promise.resolve();
let timer: ReturnType<typeof setTimeout> | undefined;
let hydration: Promise<void> | undefined;
let saving: Promise<void> | undefined;
const uploads = new Set<Promise<unknown>>();
function queued<T>(action: () => Promise<T>): Promise<T> {
  const result = tail.then(action);
  tail = result.catch(() => {});
  return result;
}
function fail(e: unknown) { publish({ error: String(e), conflict: state.conflict || String(e).includes("CONFLICT") }); }
function checked(reply: Reply): Reply {
  if (!reply?.entry || reply.entry.schema_version !== 1 || !Number.isSafeInteger(reply.entry.revision) || !reply.entry.draft)
    throw Error("资料草稿回复不完整，不能当作空草稿或保存成功。");
  return reply;
}
// Merge only fields edited after the RPC snapshot. A late ACK cannot erase B.
function accept(raw: Reply, before: InboxSourceDraft) {
  const reply = checked(raw), draft = clone(reply.entry.draft);
  for (const key of Object.keys(draft) as (keyof InboxSourceDraft)[]) {
    if (!same(state.draft[key], before[key])) Object.assign(draft, { [key]: clone(state.draft[key]) });
  }
  const id = reply.entry.pending?.submission_id ?? reply.entry.last_submission_id;
  const observation = reply.observation === "not_checked" && state.submission?.submission_id === id ? state.submission : reply.submission;
  publish({ entry: reply.entry, draft, dirty: !same(draft, reply.entry.draft), submission: observation, error: reply.observation_error ?? "", loaded: true });
}
async function read() {
  const reply = checked(await invoke<Reply>("inbox_workspace_get"));
  const failedSave = state.unconfirmedSave;
  const acknowledged = failedSave && reply.entry.revision === failedSave.revision + 1 && same(reply.entry.draft, failedSave.draft);
  // An exact committed snapshot is the merge base for edits made after that save.
  const base = acknowledged ? failedSave.draft : state.entry?.draft ?? empty();
  // Unknown prepare/submit may have changed only the original snapshot. Other
  // windows changing the same source field require an explicit choice.
  const knownOperation = state.attempt && (reply.entry.pending?.submission_id === state.attempt.submission_id || reply.entry.last_submission_id === state.attempt.submission_id);
  const collision = state.loaded && state.dirty && !knownOperation && (Object.keys(base) as (keyof InboxSourceDraft)[]).some(key => !same(base[key], reply.entry.draft[key]) && !same(base[key], state.draft[key]) && !same(state.draft[key], reply.entry.draft[key]));
  if (collision) {
    publish({ entry: reply.entry, conflict: true, error: "另一窗口也修改了资料，当前输入和已保存版本都保留。请选择核对后的版本。", submission: reply.submission });
    return;
  }
  accept(reply, base);
  publish({ conflict: false, unconfirmedSave: state.dirty ? state.unconfirmedSave : null });
  if (state.attempt && (reply.entry.pending?.submission_id === state.attempt.submission_id || reply.entry.last_submission_id === state.attempt.submission_id)) publish({ attempt: null });
}
export function loadInboxSource() {
  if (!hydration) {
    publish({ loading: true });
    hydration = queued(read).catch(fail).finally(() => publish({ loading: false }));
  }
  return hydration;
}
export function editInboxSource(patch: Partial<InboxSourceDraft> | ((draft: InboxSourceDraft) => Partial<InboxSourceDraft>)) {
  if (!state.loaded || isNativeDraftClosePending()) return;
  publish({ draft: { ...state.draft, ...(typeof patch === "function" ? patch(state.draft) : patch) }, dirty: true });
  clearTimeout(timer);
  if (!state.unconfirmedSave) timer = setTimeout(() => { void flushInboxSource().catch(() => {}); }, 450);
}
async function save() {
  if (!state.loaded || state.conflict) { if (state.dirty) throw Error("请先核对资料草稿，尚未覆盖保存。"); return; }
  if (state.unconfirmedSave) throw Error("资料草稿保存尚未确认，请核对原版本后重试。");
  while (state.dirty && !isNativeDraftClosePending()) {
    const request = { revision: state.entry!.revision, draft: clone(state.draft) };
    try {
      accept(await invoke<Reply>("inbox_workspace_save", { request }), request.draft);
    } catch (e) {
      publish({ unconfirmedSave: request });
      fail(e);
      throw e;
    }
  }
}
export function flushInboxSource(): Promise<void> {
  clearTimeout(timer);
  if (saving) return saving;
  if (!hasUnsavedInboxSource() || isNativeDraftClosePending()) return Promise.resolve();
  saving = Promise.allSettled([...uploads]).then(async () => {
    await queued(save);
    if (state.attempt) throw Error("原收录准备或发送回执尚未确认，请先核对；原输入保留。");
  }).finally(() => { saving = undefined; });
  return saving;
}
export function trackInboxUpload<T>(promise: Promise<T>) {
  uploads.add(promise);
  void promise.finally(() => uploads.delete(promise)).catch(() => {});
  return promise;
}
export function hasUnsavedInboxSource() { return state.dirty || state.busy || !!saving || uploads.size > 0 || !!state.attempt; }
async function action(run: () => Promise<void>) {
  if (state.busy) return;
  clearTimeout(timer);
  publish({ busy: true });
  let completed = false;
  try { await queued(run); completed = true; }
  catch (e) { fail(e); throw e; }
  finally {
    publish({ busy: false });
    if (completed && state.dirty && !state.conflict && !state.attempt && !state.unconfirmedSave)
      void flushInboxSource().catch(() => {});
  }
}
async function finish(id: string, command: "inbox_workspace_submit" | "inbox_workspace_resolve" | "inbox_workspace_cancel") {
  const before = clone(state.entry!.draft);
  accept(await invoke<Reply>(command, { submissionId: id }), before);
  publish({ attempt: null });
}
export async function submitInboxSource() {
  if (!state.loaded || state.busy || state.conflict || state.unconfirmedSave || state.entry?.pending || state.attempt || uploads.size) return;
  const snapshot = clone(state.draft), id = crypto.randomUUID();
  publish({ attempt: { submission_id: id, snapshot } });
  return action(async () => {
    if (state.unconfirmedSave) {
      publish({ attempt: null });
      throw Error("资料草稿保存尚未确认，未投递。请先核对原版本。");
    }
    accept(await invoke<Reply>("inbox_workspace_prepare", { request: { revision: state.entry!.revision, submission_id: id, snapshot } }), snapshot);
    if (state.entry?.pending?.stage !== "ready") throw Error("原收录准备尚未确认，未执行发布。");
    // The immutable source is durable before the one explicit publication call.
    await finish(id, "inbox_workspace_submit");
  });
}
export async function inspectInboxSource() {
  return action(async () => {
    await read();
    if (!state.conflict && state.entry?.pending && ["completed", "canceled"].includes(state.submission?.state ?? "")) await finish(state.entry.pending.submission_id, "inbox_workspace_resolve");
  });
}
export async function retryInboxSourceSave() {
  if (!state.unconfirmedSave || state.conflict) return;
  return action(async () => {
    await read();
    if (state.conflict || isNativeDraftClosePending()) return;
    publish({ unconfirmedSave: null });
    await save();
  });
}
export async function continueInboxSource() {
  if (state.conflict) return;
  const pending = state.entry?.pending ?? state.attempt;
  if (!pending) return;
  return action(async () => {
    await read();
    if (state.conflict) return;
    const current = state.entry?.pending;
    if (!current) {
      // An unconfirmed prepare is retried only through this explicit action,
      // with the original id/snapshot, never by mount, refresh or an autosave.
      if (!state.attempt) return;
      const original = state.attempt;
      accept(await invoke<Reply>("inbox_workspace_prepare", { request: { revision: state.entry!.revision, ...original } }), original.snapshot);
      return;
    }
    if (current.stage === "reserved") {
      accept(await invoke<Reply>("inbox_workspace_prepare", { request: { revision: state.entry!.revision, submission_id: current.submission_id, snapshot: current.snapshot } }), current.snapshot);
      publish({ attempt: null });
      return; // Preparation recovery itself never publishes.
    }
    if (state.submission?.state !== "prepared") throw Error("原结果尚未确认，不能重复收录；请核对原收录。");
    await finish(current.submission_id, "inbox_workspace_submit");
  });
}
export async function cancelInboxSource() {
  const id = state.entry?.pending?.submission_id;
  if (!id || state.conflict) return;
  return action(() => finish(id, "inbox_workspace_cancel"));
}
export async function chooseInboxVersion(local: boolean) {
  if (!state.entry || state.busy) return;
  publish({ conflict: false, error: "", unconfirmedSave: null, draft: local ? state.draft : clone(state.entry.draft), dirty: local && !same(state.draft, state.entry.draft) });
  if (local) await flushInboxSource();
}
export function useInboxSource() {
  const value = useSyncExternalStore(fn => { listeners.add(fn); return () => listeners.delete(fn); }, () => state);
  useEffect(() => { void loadInboxSource(); }, []);
  return value;
}
