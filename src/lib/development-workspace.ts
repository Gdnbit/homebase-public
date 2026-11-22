import { invoke } from "@tauri-apps/api/core";
import { useEffect, useSyncExternalStore } from "react";
import { isNativeDraftClosePending } from "./draft-close-state";

export type DevelopmentDraft = {
  workspace: string;
  brief: string;
  workflow: string;
  checks_json: string;
  protected_files_input: string;
};
export type DevelopmentRequest = {
  request_id: string;
  workspace: string;
  brief: string;
  workflow: string;
  checks: string[][];
  protected_files: string[];
};
type Entry = {
  schema_version: number;
  revision: number;
  draft: DevelopmentDraft;
  pending: DevelopmentRequest | null;
  previous: DevelopmentRequest[];
};
const empty = (): DevelopmentDraft => ({
  workspace: "",
  brief: "",
  workflow: "development-local",
  checks_json: "[]",
  protected_files_input: "",
});
const legacyKey = "homebase.development.pending.v1";
const copy = <T>(value: T): T => structuredClone(value);
const same = (a: unknown, b: unknown) =>
  JSON.stringify(a) === JSON.stringify(b);
type State = {
  entry: Entry | null;
  draft: DevelopmentDraft;
  loaded: boolean;
  dirty: boolean;
  busy: boolean;
  conflict: boolean;
  blocked: boolean;
  error: string;
  unconfirmedSave: { revision: number; draft: DevelopmentDraft } | null;
  attempt: { draft: DevelopmentDraft; original: DevelopmentRequest } | null;
};
let state: State = {
  entry: null,
  draft: empty(),
  loaded: false,
  dirty: false,
  busy: false,
  conflict: false,
  blocked: false,
  error: "",
  unconfirmedSave: null,
  attempt: null,
};
const listeners = new Set<() => void>();
let queue: Promise<unknown> = Promise.resolve();
let timer: ReturnType<typeof setTimeout> | undefined;
let loading: Promise<void> | undefined;
let pendingWrites = 0;
const publish = (change: Partial<State>) => {
  state = { ...state, ...change };
  listeners.forEach((fn) => fn());
};
function trackWrite<T>(work: Promise<T>): Promise<T> {
  pendingWrites += 1;
  return work.finally(() => {
    pendingWrites -= 1;
  });
}
const serial = <T>(
  fn: () => Promise<T>,
  access: "read" | "write" = "write",
): Promise<T> => {
  const work = queue.then(fn, fn);
  const next = access === "read" ? work : trackWrite(work);
  queue = next.catch(() => {});
  return next;
};
function checked(value: Entry): Entry {
  if (
    !value ||
    value.schema_version !== 1 ||
    !Number.isSafeInteger(value.revision) ||
    !value.draft ||
    !Array.isArray(value.previous) ||
    !(value.pending === null || typeof value.pending?.request_id === "string")
  )
    throw Error("开发草稿响应无效，原记录保持。");
  return value;
}
function accept(entry: Entry, snapshot?: DevelopmentDraft) {
  // Reverting to the last saved version while another save is in flight can
  // leave dirty=false until that save replaces the persisted baseline.
  const keepLocal = snapshot ? !same(state.draft, snapshot) : state.dirty;
  const draft = keepLocal ? state.draft : copy(entry.draft);
  publish({ entry, draft, loaded: true, dirty: !same(draft, entry.draft) });
}
function legacy(raw: string): DevelopmentRequest {
  const r = JSON.parse(raw);
  if (
    !r ||
    typeof r.request_id !== "string" ||
    typeof r.workspace !== "string" ||
    typeof r.brief !== "string" ||
    (r.workflow !== undefined && typeof r.workflow !== "string") ||
    !Array.isArray(r.checks) ||
    r.checks.some(
      (a: unknown) => !Array.isArray(a) || a.some((v) => typeof v !== "string"),
    ) ||
    !Array.isArray(r.protected_files) ||
    r.protected_files.some((v: unknown) => typeof v !== "string") ||
    Object.keys(r).some(
      (k) =>
        ![
          "request_id",
          "workspace",
          "brief",
          "workflow",
          "checks",
          "protected_files",
        ].includes(k),
    )
  )
    throw Error("无法读取开发请求记录，原记录未覆盖。请核对旧记录后重试。");
  return {
    request_id: r.request_id,
    workspace: r.workspace,
    brief: r.brief,
    workflow: r.workflow ?? "development-local",
    checks: r.checks,
    protected_files: r.protected_files,
  };
}
async function importLegacy(entry: Entry): Promise<Entry> {
  let raw: string | null;
  try {
    raw = localStorage.getItem(legacyKey);
  } catch {
    throw Error("旧开发请求存储无法读取，未启动或覆盖原记录。");
  }
  if (!raw) return entry;
  let original: DevelopmentRequest;
  try {
    original = legacy(raw);
  } catch {
    throw Error("无法读取开发请求记录，原记录未覆盖。请核对旧记录后重试。");
  }
  const alreadyProtected = [entry.pending, ...entry.previous].some(
    (p) => p && same(p, original),
  );
  if (!alreadyProtected) {
    if (
      entry.revision !== 0 ||
      entry.pending ||
      entry.previous.length ||
      !same(entry.draft, empty())
    )
      throw Error(
        "受保护开发草稿与旧请求同时存在，双方均保留；请先核对，未自动合并或执行。",
      );
    const draft: DevelopmentDraft = {
      workspace: original.workspace,
      brief: original.brief,
      workflow: original.workflow,
      checks_json: JSON.stringify(original.checks, null, 2),
      protected_files_input: original.protected_files.join("\n"),
    };
    entry = await trackWrite(
      invoke<Entry>("development_workspace_prepare", {
        request: { revision: entry.revision, draft, original },
      }).then(checked).catch(async (reason) => {
        const observed = checked(await invoke<Entry>("development_workspace_get"));
        if (!same(observed.pending, original)) throw reason;
        return observed;
      }),
    );
  }
  // Remove only the exact original browser bytes after protected persistence is confirmed.
  if (localStorage.getItem(legacyKey) !== raw)
    throw Error("旧开发请求已被其他窗口更新，原记录保留，请核对。");
  localStorage.removeItem(legacyKey);
  return entry;
}
export function loadDevelopmentWorkspace(): Promise<void> {
  if (state.loaded) return Promise.resolve();
  if (loading) return loading;
  loading = serial(async () => {
    try {
      accept(
        await importLegacy(
          checked(await invoke<Entry>("development_workspace_get")),
        ),
      );
      publish({ error: "", blocked: false });
    } catch (reason) {
      publish({ error: String(reason), blocked: true });
    }
  }, "read").finally(() => {
    loading = undefined;
  });
  return loading;
}
export function updateDevelopmentDraft(change: Partial<DevelopmentDraft>) {
  if (!state.loaded || state.blocked || state.busy) return;
  const draft = { ...state.draft, ...change };
  publish({ draft, dirty: !!state.unconfirmedSave || !same(draft, state.entry?.draft) });
  clearTimeout(timer);
  if (state.unconfirmedSave) return;
  timer = setTimeout(() => {
    void flushDevelopmentWorkspace().catch(() => {});
  }, 450);
}
export function hasUnsavedDevelopmentWorkspace() {
  return state.dirty || !!state.attempt || pendingWrites > 0;
}
function observeSavedDevelopment(entry: Entry) {
  const attempt = state.unconfirmedSave;
  const matchesAttempt = attempt && entry.revision === attempt.revision + 1 && same(entry.draft, attempt.draft);
  const conflict = state.dirty && !matchesAttempt && !same(state.entry?.draft, entry.draft) && !same(state.draft, entry.draft);
  accept(entry);
  publish({
    conflict,
    blocked: false,
    error: conflict
      ? "开发草稿存在两个版本，双方均保留，请选择后保存。"
      : state.dirty && attempt ? state.error : "",
    unconfirmedSave: state.dirty ? attempt : null,
    attempt: entry.pending ? null : state.attempt,
  });
}
export function flushDevelopmentWorkspace({ explicitRetry = false } = {}): Promise<void> {
  clearTimeout(timer);
  if (!hasUnsavedDevelopmentWorkspace()) return Promise.resolve();
  return serial(async () => {
    if (!state.dirty && !state.attempt) return;
    if (!state.entry || state.blocked || state.conflict || state.attempt)
      throw Error(state.error || "开发草稿尚待核对，输入保留。");
    if (state.unconfirmedSave) {
      if (!explicitRetry) throw Error("开发草稿保存尚未确认，请先核对或重试保存。");
      observeSavedDevelopment(checked(await invoke<Entry>("development_workspace_get")));
      if (state.conflict || state.attempt) return;
    }
    while (state.dirty && !isNativeDraftClosePending()) {
      const request = { revision: state.entry!.revision, draft: copy(state.draft) };
      try {
        const entry = checked(await invoke<Entry>("development_workspace_save", { request }));
        if (!same(entry.draft, request.draft)) throw Error("保存回执内容与本次开发草稿不一致，当前输入保留。");
        accept(entry, request.draft);
        publish({ error: "", unconfirmedSave: null });
      } catch (reason) {
        publish({ unconfirmedSave: request, dirty: true });
        const conflict = String(reason).includes(
          "DEVELOPMENT_WORKSPACE_CONFLICT",
        );
        if (conflict) {
          try {
            accept(checked(await invoke<Entry>("development_workspace_get")));
          } catch {
            /* Keep the original local input and original failure. */
          }
        }
        publish({ error: String(reason), conflict });
        throw reason;
      }
    }
  });
}
export async function inspectDevelopmentWorkspace() {
  if (!state.loaded) {
    await loadDevelopmentWorkspace();
    return;
  }
  return serial(async () => {
    publish({ busy: true });
    try {
      observeSavedDevelopment(checked(await invoke<Entry>("development_workspace_get")));
    } catch (reason) {
      publish({ error: String(reason) });
      throw reason;
    } finally {
      publish({ busy: false });
    }
  }, "read");
}
export async function chooseDevelopmentVersion(local: boolean) {
  if (!state.entry || state.busy || state.attempt) return;
  publish({
    draft: local ? state.draft : copy(state.entry.draft),
    dirty: local && !same(state.draft, state.entry.draft),
    conflict: false,
    error: "",
    unconfirmedSave: null,
  });
  if (local) await flushDevelopmentWorkspace();
}
export async function prepareDevelopmentRequest(
  original: DevelopmentRequest,
): Promise<DevelopmentRequest> {
  publish({ busy: true });
  try {
    if (!state.attempt) await flushDevelopmentWorkspace();
    return await serial(async () => {
      if (
        !state.entry ||
        state.blocked ||
        state.conflict ||
        state.entry.pending
      )
        throw Error("已有原开发请求或草稿需要核对，未另建任务。");
      const attempt = state.attempt ?? {
        draft: copy(state.draft),
        original: copy(original),
      };
      publish({ attempt });
      try {
        const entry = checked(
          await invoke<Entry>("development_workspace_prepare", {
            request: { revision: state.entry.revision, ...attempt },
          }),
        );
        if (!same(entry.pending, attempt.original))
          throw Error("开发原请求保护回执不一致，未执行。");
        accept(entry, attempt.draft);
        publish({ attempt: null, error: "" });
        return copy(attempt.original);
      } catch (reason) {
        try {
          const entry = checked(
            await invoke<Entry>("development_workspace_get"),
          );
          accept(entry);
          if (same(entry.pending, attempt.original)) publish({ attempt: null });
        } catch {
          /* Unknown means preserve the exact attempted identity. */
        }
        publish({ error: String(reason) });
        throw reason;
      }
    });
  } finally {
    publish({ busy: false });
  }
}
export async function newDevelopmentRequest() {
  publish({ busy: true });
  try {
    await flushDevelopmentWorkspace();
    await serial(async () => {
      if (!state.entry || state.blocked || state.conflict)
        throw Error("原开发输入尚待核对，未新建请求。");
      const entry = checked(
        await invoke<Entry>("development_workspace_new", {
          revision: state.entry.revision,
          requestId: state.entry.pending?.request_id ?? null,
        }),
      );
      accept(entry);
      publish({ error: "" });
    });
  } catch (reason) {
    publish({ error: String(reason) });
    throw reason;
  } finally {
    publish({ busy: false });
  }
}
export function useDevelopmentWorkspace() {
  const value = useSyncExternalStore(
    (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    () => state,
  );
  useEffect(() => {
    void loadDevelopmentWorkspace();
  }, []);
  return value;
}
