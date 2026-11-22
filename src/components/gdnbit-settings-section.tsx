/**
 * GDNbit Settings Section · Phase C-3a
 *
 * 在 System 页底部展示 GDNbit 集成状态：
 * - 本地 6 项 doctor 诊断（node / publish.mjs / sync.mjs / vault / SOPS key / API URL）
 * - 🔬 kitDoctor 按钮（跑 publish.mjs doctor 看 API/鉴权）
 * - 📜 publishing-kit README 链接
 * - ⚙️ 编辑 gdnbit.json
 * - 📦 批量同步 vault（TODO: C-3b 增量，当前给出命令提示）
 *
 * 密钥从 SOPS 注入，UI 里永远不展示明文。
 */

import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import {
  AlertTriangle,
  CheckCircle2,
  ExternalLink,
  FileText,
  Globe,
  Loader2,
  Package,
  RefreshCw,
  Send,
  Stethoscope,
  Terminal,
  X,
  XCircle,
} from "lucide-react";
import {
  gdnbitApi,
  type GdnbitDoctorResult,
  type GdnbitEvent,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";

export function GdnbitSettingsSection() {
  const doctorQuery = useQuery({
    queryKey: ["gdnbit", "doctor"],
    queryFn: () => gdnbitApi.doctor(),
    refetchOnWindowFocus: false,
  });
  const configQuery = useQuery({
    queryKey: ["gdnbit", "config"],
    queryFn: () => gdnbitApi.getConfig(),
    refetchOnWindowFocus: false,
  });

  return (
    <section aria-label="GDNbit 网站集成">
      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="font-serif text-lg font-semibold">GDNbit 网站集成</h2>
        <span className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
          Phase C · Wiki↔Site
        </span>
      </div>
      <p className="mb-4 text-xs text-muted-foreground">
        `C:\HomeBase\Wiki` ↔ 已配置的网站 单向发布 ·
        密钥通过 SOPS (<code className="font-mono">gdnbit_content_api_key</code>)
        注入到 node 子进程 env，不落盘。 配置文件位于{" "}
        <code className="font-mono">~/.homebase/gdnbit.json</code>（可手工编辑）。
      </p>

      {/* 本地 doctor 6 项诊断 */}
      <DoctorPanel
        data={doctorQuery.data}
        isLoading={doctorQuery.isLoading}
        isError={doctorQuery.isError}
        error={doctorQuery.error}
        isRefreshing={doctorQuery.isFetching}
        onRefresh={() => doctorQuery.refetch()}
      />

      {/* 操作按钮区 */}
      <div className="mt-4 grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <KitDoctorButton />
        <OpenFileButton
          label="publishing-kit"
          icon={FileText}
          title="打开 publishing-kit/README.md"
          path={
            configQuery.data
              ? configQuery.data.publish_kit_path.replace(
                  /scripts[\\/]publish\.mjs$/,
                  "README.md",
                )
              : null
          }
        />
        <OpenFileButton
          label="编辑 config"
          icon={Terminal}
          title="用系统默认编辑器打开 ~/.homebase/gdnbit.json"
          path={(() => {
            // Windows %USERPROFILE%\.homebase\gdnbit.json
            const home =
              typeof window !== "undefined"
                ? (window as { __HOMEBASE_USER_HOME__?: string }).__HOMEBASE_USER_HOME__
                : undefined;
            return home ?? null;
          })()}
          // 占位 · 用户手工改，按钮只作为提示（因为前端拿不到 USERPROFILE）
          fallbackHint="在终端手工编辑 ~/.homebase/gdnbit.json"
        />
        <OpenUrlButton
          label="打开网站"
          icon={Globe}
          title={configQuery.data?.api_url ?? "https://example.invalid"}
          url={configQuery.data?.api_url ?? "https://example.invalid"}
        />
      </div>

      {/* 批量同步按钮 · C-3b */}
      <SyncVaultLauncher vaultPath={configQuery.data?.vault_path ?? ""} />
    </section>
  );
}

// =====================================================================
// Doctor 面板 · 本地 6 项诊断
// =====================================================================

function DoctorPanel({
  data,
  isLoading,
  isError,
  error,
  isRefreshing,
  onRefresh,
}: {
  data: GdnbitDoctorResult | undefined;
  isLoading: boolean;
  isError: boolean;
  error: unknown;
  isRefreshing: boolean;
  onRefresh: () => void;
}) {
  if (isLoading) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-border/60 bg-card p-3 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        跑 gdnbit_doctor 本地诊断…
      </div>
    );
  }
  if (isError || !data) {
    return (
      <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive-strong">
        <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        <div className="flex-1">
          <p className="font-medium">doctor 命令失败</p>
          <p className="mt-0.5 font-mono text-[11px]">{String(error ?? "未知错误")}</p>
          <button
            type="button"
            onClick={onRefresh}
            className="mt-2 inline-flex items-center gap-1 rounded border border-destructive/40 bg-destructive/5 px-2 py-0.5 text-[11px] hover:bg-destructive/10"
          >
            <RefreshCw className="h-3 w-3" /> 重试
          </button>
        </div>
      </div>
    );
  }

  return (
    <div
      className={cn(
        "rounded-lg border p-3",
        data.ok
          ? "border-emerald-200 bg-emerald-50/40 dark:border-emerald-800/60 dark:bg-emerald-950/20"
          : "border-amber-200 bg-amber-50/40 dark:border-amber-800/60 dark:bg-amber-950/20",
      )}
    >
      <div className="mb-2 flex items-center justify-between">
        <span className="inline-flex items-center gap-1.5 text-xs font-medium">
          <Stethoscope className="h-3.5 w-3.5" />
          本地 6 项诊断 ·{" "}
          <span
            className={cn(
              data.ok ? "text-emerald-700 dark:text-emerald-300" : "text-amber-700 dark:text-amber-300",
            )}
          >
            {data.ok ? "全部通过" : `${data.checks.filter((c) => c.ok).length}/${data.checks.length} 通过`}
          </span>
        </span>
        <button
          type="button"
          onClick={onRefresh}
          disabled={isRefreshing}
          className="inline-flex items-center gap-1 rounded border border-border/60 bg-background/60 px-2 py-0.5 text-[11px] text-muted-foreground hover:text-foreground disabled:opacity-50"
        >
          {isRefreshing ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <RefreshCw className="h-3 w-3" />
          )}
          重跑
        </button>
      </div>
      <ul className="space-y-1">
        {data.checks.map((c) => (
          <li
            key={c.id}
            className="flex items-start gap-2 text-[11.5px] leading-tight"
          >
            {c.ok ? (
              <CheckCircle2 className="mt-0.5 h-3 w-3 shrink-0 text-emerald-600 dark:text-emerald-400" />
            ) : (
              <XCircle className="mt-0.5 h-3 w-3 shrink-0 text-rose-600 dark:text-rose-400" />
            )}
            <div className="flex-1 min-w-0">
              <span className="font-medium">{c.label}</span>
              <span className="ml-2 font-mono text-[10.5px] text-muted-foreground break-all">
                {c.detail}
              </span>
            </div>
          </li>
        ))}
      </ul>
      {data.suggestions.length > 0 && (
        <div className="mt-2 space-y-0.5 rounded border border-amber-300/40 bg-amber-50/60 p-2 text-[10.5px] text-amber-900 dark:border-amber-800/60 dark:bg-amber-950/40 dark:text-amber-200">
          <p className="font-medium">修复建议：</p>
          {data.suggestions.map((s, i) => (
            <p key={i}>· {s}</p>
          ))}
        </div>
      )}
    </div>
  );
}

// =====================================================================
// KitDoctor 按钮 · 流式跑 publish.mjs doctor
// =====================================================================

function KitDoctorButton() {
  const [lines, setLines] = useState<string[]>([]);
  const [taskId, setTaskId] = useState<string | null>(null);
  const [status, setStatus] = useState<"idle" | "running" | "done" | "failed">(
    "idle",
  );

  const startMutation = useMutation({
    mutationFn: () => gdnbitApi.kitDoctor(),
    onMutate: () => {
      setLines([]);
      setStatus("running");
    },
    onSuccess: (r) => {
      setTaskId(r.task_id);
    },
    onError: (e) => {
      setLines([String(e)]);
      setStatus("failed");
    },
  });

  useEffect(() => {
    if (!taskId || status !== "running") return;
    const un = listen<GdnbitEvent>("gdnbit:event", (e) => {
      const ev = e.payload;
      if (ev.task_id !== taskId) return;
      if (ev.kind === "stream" || ev.kind === "stderr") {
        // eslint-disable-next-line no-control-regex
        setLines((l) => [...l, ev.line.replace(/\x1b\[[0-9;]*m/g, "")]);
      } else if (ev.kind === "completed") {
        setStatus(ev.exit_code === 0 ? "done" : "failed");
      } else if (ev.kind === "failed") {
        setLines((l) => [...l, `[FAIL] ${ev.message}`]);
        setStatus("failed");
      }
    });
    return () => {
      un.then((u) => u());
    };
  }, [taskId, status]);

  return (
    <div className="space-y-1.5">
      <button
        type="button"
        disabled={status === "running"}
        onClick={() => startMutation.mutate()}
        className={cn(
          "group inline-flex w-full items-center justify-center gap-1.5 rounded-md border px-3 py-2 text-[11px] font-medium transition-colors",
          status === "done"
            ? "border-emerald-300 bg-emerald-50 text-emerald-800 hover:bg-emerald-100 dark:border-emerald-800/60 dark:bg-emerald-950/30 dark:text-emerald-200"
            : status === "failed"
              ? "border-rose-300 bg-rose-50 text-rose-800 hover:bg-rose-100 dark:border-rose-800/60 dark:bg-rose-950/30 dark:text-rose-200"
              : "border-border bg-card text-foreground hover:bg-muted",
        )}
        title="跑 publish.mjs doctor 看 API 连通 / Key 鉴权"
      >
        {status === "running" ? (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        ) : (
          <Stethoscope className="h-3.5 w-3.5" />
        )}
        🔬 联网诊断
      </button>
      {lines.length > 0 && (
        <div className="max-h-32 overflow-auto rounded border border-border/60 bg-background/60 p-1.5 font-mono text-[9.5px] leading-snug">
          {lines.map((l, i) => (
            <div key={i}>{l}</div>
          ))}
        </div>
      )}
    </div>
  );
}

// =====================================================================
// OpenFileButton · 用 shell.open 打开文件
// =====================================================================

function OpenFileButton({
  label,
  icon: Icon,
  title,
  path,
  fallbackHint,
}: {
  label: string;
  icon: typeof FileText;
  title: string;
  path: string | null;
  fallbackHint?: string;
}) {
  const [err, setErr] = useState<string | null>(null);
  const disabled = path === null;
  return (
    <button
      type="button"
      title={disabled && fallbackHint ? fallbackHint : title}
      disabled={disabled}
      onClick={async () => {
        if (!path) return;
        try {
          await openExternal(path);
          setErr(null);
        } catch (e) {
          setErr(String(e));
        }
      }}
      className={cn(
        "inline-flex w-full items-center justify-center gap-1.5 rounded-md border border-border bg-card px-3 py-2 text-[11px] font-medium transition-colors",
        disabled
          ? "cursor-not-allowed opacity-60"
          : "text-foreground hover:bg-muted",
      )}
    >
      <Icon className="h-3.5 w-3.5" />
      {err ? "打开失败" : label}
    </button>
  );
}

// =====================================================================
// OpenUrlButton · 用 shell.open 打开 http URL
// =====================================================================

function OpenUrlButton({
  label,
  icon: Icon,
  title,
  url,
}: {
  label: string;
  icon: typeof Globe;
  title: string;
  url: string;
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={() => openExternal(url)}
      className="inline-flex w-full items-center justify-center gap-1.5 rounded-md border border-border bg-card px-3 py-2 text-[11px] font-medium text-foreground transition-colors hover:bg-muted"
    >
      <Icon className="h-3.5 w-3.5" />
      {label}
      <ExternalLink className="h-2.5 w-2.5 opacity-60" />
    </button>
  );
}

// =====================================================================
// SyncVaultLauncher · Phase C-3b · 批量同步 vault 入口
// =====================================================================
// 点击「📦 批量同步」展开 dialog，内含：
// - folder 过滤（可选 · vault 相对路径，只同步某子目录）
// - dry-run 开关（默认 on · 不写入网站）
// - force 开关（忽略 frontmatter `publish: true`）
// - status 选择（draft / published）
// - 命令预览（基于参数实时拼）
// - 启动按钮 → gdnbitApi.syncVault · 流式日志 + 取消按钮

type SyncState =
  | { phase: "idle" }
  | { phase: "running"; task_id: string; dry_run: boolean }
  | {
      phase: "done";
      task_id: string | null;
      dry_run: boolean;
      exit_code: number;
    }
  | { phase: "failed"; task_id: string | null; error: string }
  | { phase: "cancelled"; task_id: string };

function SyncVaultLauncher({ vaultPath }: { vaultPath: string }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="mt-4">
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className={cn(
          "inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-3 py-2 text-[11px] font-medium text-foreground transition-colors hover:bg-muted",
        )}
      >
        <Package className="h-3.5 w-3.5" />
        {expanded ? "收起批量同步" : "📦 批量同步 vault"}
        {!expanded && (
          <span className="ml-1 font-mono text-[10px] text-muted-foreground">
            (扫整个 {vaultPath || "vault"} 发 publish:true 文章)
          </span>
        )}
      </button>
      {expanded && <SyncVaultForm />}
    </div>
  );
}

function SyncVaultForm() {
  const [dryRun, setDryRun] = useState(true);
  const [force, setForce] = useState(false);
  const [status, setStatus] = useState<"draft" | "published">("draft");
  const [folder, setFolder] = useState("");
  const [state, setState] = useState<SyncState>({ phase: "idle" });
  const [commandPreview, setCommandPreview] = useState<string | null>(null);
  const [logs, setLogs] = useState<
    Array<{ stream: "stdout" | "stderr"; line: string }>
  >([]);
  const logsRef = useRef<HTMLDivElement | null>(null);

  // 自动滚到底
  useEffect(() => {
    if (logsRef.current) {
      logsRef.current.scrollTop = logsRef.current.scrollHeight;
    }
  }, [logs.length]);

  // 订阅 gdnbit:event
  useEffect(() => {
    if (state.phase !== "running") return;
    const taskId = state.task_id;
    const dryRunAtStart = state.dry_run;
    const un = listen<GdnbitEvent>("gdnbit:event", (e) => {
      const ev = e.payload;
      if (ev.task_id !== taskId) return;
      if (ev.kind === "stream") {
        // eslint-disable-next-line no-control-regex
        const line = ev.line.replace(/\x1b\[[0-9;]*m/g, "");
        setLogs((l) => [...l, { stream: "stdout", line }]);
      } else if (ev.kind === "stderr") {
        // eslint-disable-next-line no-control-regex
        const line = ev.line.replace(/\x1b\[[0-9;]*m/g, "");
        setLogs((l) => [...l, { stream: "stderr", line }]);
      } else if (ev.kind === "completed") {
        setState({
          phase: "done",
          task_id: taskId,
          dry_run: dryRunAtStart,
          exit_code: ev.exit_code,
        });
      } else if (ev.kind === "failed") {
        setState({ phase: "failed", task_id: taskId, error: ev.message });
      } else if (ev.kind === "cancelled") {
        setState({ phase: "cancelled", task_id: taskId });
      }
    });
    return () => {
      un.then((u) => u());
    };
  }, [state]);

  const startMutation = useMutation({
    mutationFn: () =>
      gdnbitApi.syncVault({
        dry_run: dryRun,
        force,
        status,
        folder: folder.trim() === "" ? null : folder.trim(),
      }),
    onSuccess: (r) => {
      setCommandPreview(r.command_preview);
      setLogs([]);
      setState({ phase: "running", task_id: r.task_id, dry_run: r.dry_run });
    },
    onError: (e) =>
      setState({ phase: "failed", task_id: null, error: String(e) }),
  });

  const cancelMutation = useMutation({
    mutationFn: (taskId: string) => gdnbitApi.cancel(taskId),
  });

  const running = state.phase === "running";
  const busy = running || startMutation.isPending;

  return (
    <div className="mt-3 space-y-3 rounded-lg border border-border/60 bg-background/60 p-3">
      {/* 参数表单 */}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <label className="flex flex-col gap-1 text-[11px]">
          <span className="text-muted-foreground">子目录过滤（相对 vault）</span>
          <input
            type="text"
            value={folder}
            disabled={busy}
            onChange={(e) => setFolder(e.target.value)}
            placeholder="留空 = 扫全部；例：Projects 或 Blog/2026"
            className="rounded border border-border bg-card px-2 py-1 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
          />
        </label>
        <label className="flex flex-col gap-1 text-[11px]">
          <span className="text-muted-foreground">status（frontmatter 缺省时用）</span>
          <select
            value={status}
            disabled={busy}
            onChange={(e) =>
              setStatus(e.target.value as "draft" | "published")
            }
            className="rounded border border-border bg-card px-2 py-1 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
          >
            <option value="draft">draft</option>
            <option value="published">published</option>
          </select>
        </label>
      </div>
      <div className="flex flex-wrap items-center gap-4 text-[11px]">
        <label className="inline-flex cursor-pointer items-center gap-1.5">
          <input
            type="checkbox"
            checked={dryRun}
            disabled={busy}
            onChange={(e) => setDryRun(e.target.checked)}
            className="h-3.5 w-3.5 rounded border-border text-primary-600 focus:ring-primary-500"
          />
          <span>
            预览模式（<code className="font-mono">--dry-run</code> · 不真写入）
          </span>
        </label>
        <label className="inline-flex cursor-pointer items-center gap-1.5">
          <input
            type="checkbox"
            checked={force}
            disabled={busy}
            onChange={(e) => setForce(e.target.checked)}
            className="h-3.5 w-3.5 rounded border-border text-primary-600 focus:ring-primary-500"
          />
          <span>
            强制（忽略 <code className="font-mono">publish: true</code> · 全发）
          </span>
        </label>
      </div>

      {/* 启动/取消按钮 */}
      <div className="flex items-center gap-2">
        <button
          type="button"
          disabled={busy}
          onClick={() => startMutation.mutate()}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[11px] font-medium transition-colors",
            busy
              ? "cursor-not-allowed bg-muted text-muted-foreground"
              : dryRun
                ? "border border-border bg-card text-foreground hover:bg-muted"
                : "bg-amber-500 text-white hover:bg-amber-600 dark:bg-amber-600 dark:hover:bg-amber-700",
          )}
        >
          {startMutation.isPending || running ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Send className="h-3 w-3" />
          )}
          {startMutation.isPending
            ? "启动中…"
            : running
              ? "运行中…"
              : dryRun
                ? "跑 dry-run 看看"
                : "🚀 正式批量发"}
        </button>
        {running && (
          <button
            type="button"
            disabled={cancelMutation.isPending}
            onClick={() =>
              state.phase === "running" && cancelMutation.mutate(state.task_id)
            }
            className="inline-flex items-center gap-1 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-1.5 text-[11px] font-medium text-destructive-strong hover:bg-destructive/20"
          >
            {cancelMutation.isPending ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <X className="h-3 w-3" />
            )}
            取消
          </button>
        )}
      </div>

      {/* 命令预览 */}
      {commandPreview && (
        <div>
          <p className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            执行命令（密钥从 SOPS 注入到 env）
          </p>
          <pre className="overflow-x-auto rounded border border-border/60 bg-background/60 p-2 font-mono text-[10.5px] leading-relaxed">
            {commandPreview}
          </pre>
        </div>
      )}

      {/* 流式日志 */}
      {logs.length > 0 && (
        <div>
          <p className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            输出流（{logs.length}）
          </p>
          <div
            ref={logsRef}
            className="max-h-64 overflow-auto rounded border border-border/60 bg-background/60 p-2 font-mono text-[10.5px] leading-relaxed"
          >
            {logs.map((l, i) => (
              <div
                key={i}
                className={cn(
                  l.stream === "stderr" && "text-destructive-strong",
                )}
              >
                {l.line}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 完成/失败/取消徽章 */}
      {state.phase === "done" && (
        <div
          className={cn(
            "flex items-center gap-2 rounded-md border px-3 py-2 text-[11px]",
            state.exit_code === 0
              ? "border-emerald-300 bg-emerald-50/50 text-emerald-900 dark:border-emerald-800 dark:bg-emerald-950/30 dark:text-emerald-200"
              : "border-amber-300 bg-amber-50/50 text-amber-900 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-200",
          )}
        >
          <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
          <span className="font-medium">
            {state.dry_run ? "预览完成" : "批量发布完成"} · exit={state.exit_code}
          </span>
        </div>
      )}
      {state.phase === "failed" && (
        <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-[11px] text-destructive-strong">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>失败：{state.error}</span>
        </div>
      )}
      {state.phase === "cancelled" && (
        <div className="flex items-center gap-2 rounded-md border border-border/60 bg-muted/20 px-3 py-2 text-[11px] text-muted-foreground">
          <X className="h-3.5 w-3.5 shrink-0" />
          <span>已取消（task {state.task_id.slice(0, 8)}…）</span>
        </div>
      )}
    </div>
  );
}
