import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useQuery } from "@tanstack/react-query";
import { Play, RefreshCw } from "lucide-react";
import { startPolling } from "../lib/poll";
import {
  chooseDevelopmentVersion,
  flushDevelopmentWorkspace,
  inspectDevelopmentWorkspace,
  newDevelopmentRequest,
  prepareDevelopmentRequest,
  updateDevelopmentDraft,
  useDevelopmentWorkspace,
  type DevelopmentRequest,
} from "../lib/development-workspace";
type Result = {
  task_id: string;
  state: string;
  message?: string;
  result_path?: string;
  runs?: { id: number; rule: string; state: string; summary: string }[];
  questions?: { id: number; title: string; options: string[] }[];
  failures?: string[];
  evidence?: { argv: string[]; exitCode: number | null; output: string }[];
};
type Catalog = {
  status: string;
  version: string | null;
  message: string;
  workflows: { id: string; name: string; description: string }[];
  tasks: {
    task_id: string;
    workspace: string;
    workflow: string;
    created_at: string;
  }[];
};
const names: Record<string, string> = {
  submitted: "已受理 · 待执行",
  queued: "排队中",
  running: "执行中",
  awaiting_input: "需要你回答",
  verified: "宿主验收通过",
  needs_acceptance: "执行结束 · 待验收",
  blocked: "未通过或已受阻",
  unknown: "原状态未确认",
  not_verified: "尚未取得验收结果",
};
export function DevelopmentCapability() {
  const editor = useDevelopmentWorkspace();
  const {
    workspace,
    brief,
    workflow,
    checks_json: checks,
    protected_files_input: protectedFiles,
  } = editor.draft;
  const pending = editor.entry?.pending ?? null;
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [taskId, setTaskId] = useState("");
  const lock = useRef(false);
  const generation = useRef(0);
  const reads = useRef(new Map<string, Promise<Result>>());
  const observing = useRef<string | null>(null);
  const alive = useRef(true);
  function invalidateReads() {
    generation.current++;
    reads.current.clear();
  }
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      invalidateReads();
      observing.current = null;
    };
  }, []);
  const catalog = useQuery({
    queryKey: ["development-catalog"],
    queryFn: () => invoke<Catalog>("development_catalog"),
    staleTime: 30000,
    retry: false,
  });
  async function query(id: string, automatic = false) {
    if (
      !alive.current ||
      lock.current ||
      (automatic && observing.current !== id)
    )
      return;
    if (!automatic) observing.current = id;
    const current = ++generation.current;
    try {
      let pendingRead = reads.current.get(id);
      if (!pendingRead) {
        const started = invoke<Result>("development_result", {
          taskId: id,
        }).finally(() => {
          if (reads.current.get(id) === started) reads.current.delete(id);
        });
        reads.current.set(id, started);
        pendingRead = started;
      }
      const value = await pendingRead;
      if (alive.current && generation.current === current) {
        setResult(value);
        setTaskId(id);
        setError("");
      }
    } catch (reason) {
      if (alive.current && generation.current === current)
        setError(String(reason));
    }
  }
  useEffect(() => {
    if (
      !result ||
      ![
        "submitted",
        "queued",
        "running",
        "awaiting_input",
        "not_verified",
      ].includes(result.state) ||
      busy
    )
      return;
    return startPolling(() => query(result.task_id, true), 4000, false);
  }, [result, busy]);
  async function start() {
    if (lock.current || !editor.loaded || editor.blocked || editor.conflict)
      return;
    lock.current = true;
    invalidateReads();
    setBusy(true);
    setError("");
    try {
      if (pending)
        throw Error(
          "原请求已保存。请先查询原任务；要开发新需求，请明确点击新建开发任务。",
        );
      const parsed: unknown = JSON.parse(checks);
      if (
        !Array.isArray(parsed) ||
        parsed.length > 8 ||
        parsed.some(
          (a) =>
            !Array.isArray(a) ||
            !a.length ||
            a.some((v) => typeof v !== "string"),
        )
      )
        throw Error(
          '验收命令应是 JSON 二维字符串数组，例如 [["node","--test"]]。',
        );
      if (
        !workspace.trim() ||
        !brief.trim() ||
        new TextEncoder().encode(brief).length > 16000
      )
        throw Error("请填写项目目录和需求；需求最多 16000 UTF-8 字节。");
      const request: DevelopmentRequest = {
        request_id: crypto.randomUUID(),
        workspace: workspace.trim(),
        brief,
        workflow,
        checks: parsed,
        protected_files: protectedFiles
          .split(/\r?\n/)
          .map((f) => f.trim())
          .filter(Boolean),
      };
      // A lost preparation response must not reach the executor.
      const original = await prepareDevelopmentRequest(request);
      const value = await invoke<Result>("development_run", {
        request: original,
      });
      if (alive.current) {
        observing.current = value.task_id;
        setResult(value);
        setTaskId(value.task_id);
        void catalog.refetch();
      }
    } catch (reason) {
      if (alive.current) setError(String(reason));
    } finally {
      lock.current = false;
      if (alive.current) setBusy(false);
    }
  }
  async function recover(original = pending) {
    if (!original || lock.current) return;
    // Compute the same stable identity without resubmitting the business action.
    const digest = await crypto.subtle.digest(
      "SHA-256",
      new TextEncoder().encode(original.request_id),
    );
    const id =
      "dev-" +
      Array.from(new Uint8Array(digest))
        .map((v) => v.toString(16).padStart(2, "0"))
        .join("")
        .slice(0, 32);
    await query(id);
  }
  async function answer(cardId: number, option: number) {
    if (!result || lock.current) return;
    lock.current = true;
    invalidateReads();
    setBusy(true);
    try {
      const value = await invoke<Result>("development_answer", {
        request: { task_id: result.task_id, card_id: cardId, option },
      });
      if (alive.current) {
        observing.current = value.task_id;
        setResult(value);
      }
    } catch (reason) {
      if (alive.current) setError(String(reason));
    } finally {
      lock.current = false;
      if (alive.current) setBusy(false);
    }
  }
  async function fresh() {
    if (lock.current || busy || editor.busy || editor.blocked || !editor.loaded)
      return;
    if (
      pending &&
      !window.confirm("新建请求不会取消原开发任务。确认要准备另一个任务？")
    )
      return;
    lock.current = true;
    invalidateReads();
    setBusy(true);
    try {
      await newDevelopmentRequest();
      observing.current = null;
      setResult(null);
      setTaskId("");
      setError("");
    } catch {
      setError("原请求记录无法更新，未创建新请求。");
    } finally {
      lock.current = false;
      if (alive.current) setBusy(false);
    }
  }
  return (
    <section className="hb-card space-y-4" aria-label="开发能力">
      <header className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold">开发</h2>
          <p className="mt-1 text-sm text-muted-foreground">
            设置项目和需求，运行规划、实现、审查与验收。
          </p>
        </div>
        <button
          className="hb-btn-ghost"
          aria-label="刷新开发能力"
          onClick={() => void catalog.refetch()}
        >
          <RefreshCw size={16} />
        </button>
      </header>
      {catalog.isPending ? (
        <p role="status">正在检查本机开发能力…</p>
      ) : catalog.isError ? (
        <p role="alert">开发能力读取失败：{String(catalog.error)}</p>
      ) : (
        <p className="text-sm text-muted-foreground">
          Agent Autoloop{" "}
          {catalog.data.version ? `v${catalog.data.version}` : ""} ·{" "}
          {catalog.data.message} 模型调用可能产生费用。
        </p>
      )}
      <div
        className="space-y-2 text-xs text-muted-foreground"
        aria-label="开发草稿保存状态"
      >
        <p role="status">
          {!editor.loaded
            ? editor.blocked
              ? "开发草稿尚待核对，原记录保留。"
              : "正在恢复开发输入…"
            : editor.error
              ? "输入尚待保存或核对。"
              : editor.dirty
                ? "正在保存开发输入…"
                : editor.entry?.revision
                  ? "开发草稿已受保护保存。"
                  : "编辑后会自动在本机受保护保存。"}
        </p>
        <div className="flex flex-wrap gap-3">
          <button
            className="underline"
            disabled={busy || editor.busy}
            onClick={() => {
              setError("");
              void inspectDevelopmentWorkspace().catch((reason) =>
                setError(String(reason)),
              );
            }}
          >
            刷新开发草稿
          </button>
          {editor.dirty && !editor.conflict && (
            <button
              className="underline"
              disabled={busy || editor.busy}
              onClick={() => {
                setError("");
                void flushDevelopmentWorkspace({ explicitRetry: true }).catch((reason) =>
                  setError(String(reason)),
                );
              }}
            >
              保存开发草稿
            </button>
          )}
        </div>
        {editor.conflict && (
          <div className="flex flex-wrap gap-3">
            <button
              className="underline"
              onClick={() =>
                void chooseDevelopmentVersion(true).catch((reason) =>
                  setError(String(reason)),
                )
              }
            >
              保留当前输入并保存
            </button>
            <button
              className="underline"
              onClick={() => void chooseDevelopmentVersion(false)}
            >
              采用已保存的开发草稿
            </button>
          </div>
        )}
      </div>
      <fieldset
        disabled={
          busy ||
          editor.busy ||
          !!editor.attempt ||
          !!pending ||
          !editor.loaded ||
          editor.blocked
        }
        className="grid gap-4 sm:grid-cols-2"
      >
        <label className="text-sm sm:col-span-2">
          项目目录
          <input
            className="hb-input mt-1 w-full"
            value={workspace}
            onChange={(e) =>
              updateDevelopmentDraft({ workspace: e.target.value })
            }
            placeholder="已有项目的本机绝对路径"
          />
        </label>
        <label className="text-sm sm:col-span-2">
          开发需求
          <textarea
            aria-label="开发需求"
            className="hb-input mt-1 min-h-28 w-full"
            value={brief}
            onChange={(e) => updateDevelopmentDraft({ brief: e.target.value })}
            placeholder="说明要实现什么、允许改动的范围和验收标准"
          />
        </label>
        <label className="text-sm sm:col-span-2">
          协作流程
          <select
            className="hb-input mt-1 w-full"
            value={workflow}
            onChange={(e) =>
              updateDevelopmentDraft({ workflow: e.target.value })
            }
          >
            {(
              catalog.data?.workflows ?? [
                {
                  id: "development-local",
                  name: "完整开发 · 三个独立 Codex 角色",
                },
              ]
            ).map((w) => (
              <option key={w.id} value={w.id}>
                {w.name}
              </option>
            ))}
          </select>
        </label>
        <details className="sm:col-span-2">
          <summary className="cursor-pointer text-sm">验收设置</summary>
          <div className="mt-3 space-y-3">
            <label className="block text-sm">
              验收命令（JSON 参数数组）
              <textarea
                aria-label="验收命令（JSON 参数数组）"
                className="hb-input mt-1 min-h-24 w-full font-mono"
                value={checks}
                onChange={(e) =>
                  updateDevelopmentDraft({ checks_json: e.target.value })
                }
              />
            </label>
            <p className="text-xs text-muted-foreground">
              例如
              [["node","--test"]]；在项目目录执行。未设置命令时只标记“待验收”。
            </p>
            <label className="block text-sm">
              受保护验收文件（每行一个项目内路径）
              <textarea
                aria-label="受保护验收文件（每行一个项目内路径）"
                className="hb-input mt-1 w-full"
                value={protectedFiles}
                onChange={(e) =>
                  updateDevelopmentDraft({
                    protected_files_input: e.target.value,
                  })
                }
              />
            </label>
          </div>
        </details>
      </fieldset>
      <div className="flex flex-wrap gap-3">
        <button
          className="hb-btn-primary"
          disabled={
            busy ||
            editor.busy ||
            !!pending ||
            !editor.loaded ||
            editor.blocked ||
            editor.conflict ||
            catalog.data?.status !== "ready"
          }
          onClick={() => void start()}
        >
          <Play size={15} />
          {busy ? "处理中…" : editor.attempt ? "重试原准备并运行" : "运行开发"}
        </button>
        {pending && (
          <button
            className="hb-btn-outline"
            disabled={busy}
            onClick={() => void recover()}
          >
            查询原请求
          </button>
        )}
        <button
          className="hb-btn-ghost"
          disabled={
            busy ||
            editor.busy ||
            !editor.loaded ||
            editor.blocked ||
            !!editor.attempt ||
            editor.conflict
          }
          onClick={() => void fresh()}
        >
          新建开发任务
        </button>
      </div>
      {pending && (
        <p className="break-all text-xs text-muted-foreground">
          请求编号：{pending.request_id}
          。原请求已受保护保存在本机；这不代表已受理或完成。切换页面不会取消后台任务。
        </p>
      )}
      {(error || editor.error) && (
        <p
          role="alert"
          className="whitespace-pre-wrap break-words text-sm text-destructive-strong"
        >
          {error || editor.error}
        </p>
      )}
      {!!editor.entry?.previous.length && (
        <details>
          <summary className="cursor-pointer text-sm">
            历史原请求（{editor.entry.previous.length}）
          </summary>
          <div className="mt-3 space-y-2">
            {[...editor.entry.previous].reverse().map((original) => (
              <button
                key={original.request_id}
                className="block break-all text-left text-xs underline"
                disabled={busy}
                onClick={() => void recover(original)}
              >
                查询历史原请求 {original.request_id}
              </button>
            ))}
          </div>
        </details>
      )}
      {result && (
        <div
          className="space-y-3 border-t border-border pt-4"
          aria-live="polite"
        >
          <p className="font-medium">{names[result.state] ?? "状态待核对"}</p>
          <p className="break-all text-xs">任务编号：{result.task_id}</p>
          <p className="text-sm">{result.message}</p>
          {result.result_path && (
            <p className="break-all text-xs">验收记录：{result.result_path}</p>
          )}
          {result.runs?.map((run) => (
            <p key={run.id} className="break-words text-sm">
              {run.rule.split("-").at(-1)} · {run.state}：{run.summary}
            </p>
          ))}
          {result.failures?.map((f, i) => (
            <p key={i} className="text-sm">
              {f}
            </p>
          ))}
          {result.questions?.map((q) => (
            <div
              key={q.id}
              className="space-y-2 rounded-lg border border-border p-3"
            >
              <p>{q.title}</p>
              <div className="flex flex-wrap gap-2">
                {q.options.map((option, i) => (
                  <button
                    key={i}
                    className="hb-btn-outline"
                    disabled={busy}
                    onClick={() => void answer(q.id, i + 1)}
                  >
                    {option}
                  </button>
                ))}
              </div>
            </div>
          ))}
          {result.evidence?.map((e, i) => (
            <details key={i}>
              <summary className="cursor-pointer text-sm">
                验收 {i + 1} · 退出码 {String(e.exitCode)}
              </summary>
              <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-all text-xs">
                {e.argv.join(" ")} {"\n"}
                {e.output}
              </pre>
            </details>
          ))}
          <button
            className="hb-btn-outline"
            disabled={busy}
            onClick={() => void query(result.task_id)}
          >
            刷新原任务
          </button>
        </div>
      )}
      <details>
        <summary className="cursor-pointer text-sm">查询已有任务</summary>
        <div className="mt-3 flex gap-2">
          <input
            aria-label="已有开发任务编号"
            className="hb-input min-w-0 flex-1"
            value={taskId}
            onChange={(e) => setTaskId(e.target.value)}
            placeholder="dev-…"
          />
          <button
            className="hb-btn-outline"
            disabled={busy || !taskId}
            onClick={() => void query(taskId)}
          >
            查询任务
          </button>
        </div>
        <ul className="mt-3 space-y-2">
          {catalog.data?.tasks.map((t) => (
            <li key={t.task_id}>
              <button
                className="break-all text-left text-xs underline"
                onClick={() => void query(t.task_id)}
              >
                {t.workspace} · {t.created_at}
              </button>
            </li>
          ))}
        </ul>
      </details>
    </section>
  );
}
