import { useDeferredValue, useMemo, useState, useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import ClaudeColor from "@lobehub/icons/es/Claude/components/Color";
import CodexColor from "@lobehub/icons/es/Codex/components/Color";
import GeminiColor from "@lobehub/icons/es/Gemini/components/Color";
import OpenClawColor from "@lobehub/icons/es/OpenClaw/components/Color";
import {
  AlertTriangle,
  Bot,
  Database,
  Info,
  Loader2,
  MessageSquare,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import {
  agentsApi,
  type AgentBucket,
  type HistoryEntry,
  type HistoryMessage,
  type HistorySession,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";

/**
 * Phase H · 历史对话 · 跨 agent 时间线总览
 *
 * 上：5 agent 状态桌（buckets，可作为 agent filter chip）
 * 中：搜索框 + 刷新按钮
 * 下：跨 agent 合并时间线（按 last_active_at 倒序）
 *
 * 选中条目 → 右侧详情抽屉，调 agentsApi.historyRead 拉完整对话
 *
 * 设计理念：HomeBase 是只读视图，原始 jsonl/db 不动。
 */
export function History() {
  const queryClient = useQueryClient();
  const allQuery = useQuery({
    queryKey: ["agents", "history", "all"],
    queryFn: () => agentsApi.historyAll(),
    refetchOnWindowFocus: false,
  });
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search);
  const [agentFilter, setAgentFilter] = useState<string | null>(null);
  const [selected, setSelected] = useState<{
    agentId: string;
    sessionId: string;
  } | null>(null);

  const filtered = useMemo(() => {
    const data = allQuery.data?.entries ?? [];
    let items = data;
    if (agentFilter) {
      items = items.filter((e) => e.agent_id === agentFilter);
    }
    const q = deferredSearch.trim().toLowerCase();
    if (q) {
      items = items.filter((e) => {
        const hay = [
          e.first_user_prompt ?? "",
          e.cwd ?? "",
          e.agent_id,
          e.session_id,
        ]
          .join(" ")
          .toLowerCase();
        return hay.includes(q);
      });
    }
    return items;
  }, [allQuery.data, deferredSearch, agentFilter]);

  return (
    <>
      <header className="hb-page-header">
        <h1>历史对话</h1>
      </header>

      {allQuery.isLoading ? (
        <LoadingPlaceholder />
      ) : allQuery.isError ? (
        <ErrorPanel err={allQuery.error} />
      ) : !allQuery.data ? null : (
        <>
          <BucketsStatus
            buckets={allQuery.data.buckets}
            currentFilter={agentFilter}
            onPick={setAgentFilter}
          />

          <div className="mt-6 flex items-center gap-2">
            <div className="relative flex-1">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="搜索 prompt / cwd / session_id"
                className="w-full rounded-lg border border-border bg-card py-2 pl-9 pr-3 text-sm transition-colors focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
              />
              {search && (
                <button
                  type="button"
                  onClick={() => setSearch("")}
                  className="absolute right-2 top-1/2 -translate-y-1/2 rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
                  aria-label="清除搜索"
                >
                  <X className="h-3.5 w-3.5" />
                </button>
              )}
            </div>
            <button
              type="button"
              onClick={() => {
                void queryClient.invalidateQueries({ queryKey: ["agents", "history", "read"], refetchType: "none" });
                void allQuery.refetch();
              }}
              disabled={allQuery.isFetching}
              className="inline-flex items-center gap-1.5 rounded-lg border border-border bg-card px-3 py-2 text-xs font-medium text-muted-foreground transition-colors hover:border-primary-500 hover:text-primary-600 disabled:opacity-50 dark:hover:text-primary-400"
            >
              {allQuery.isFetching ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <RefreshCw className="h-3.5 w-3.5" />
              )}
              刷新
            </button>
          </div>

          <Timeline
            key={`${deferredSearch}:${agentFilter}`}
            entries={filtered}
            totalCount={allQuery.data.entries.length}
            search={search}
            agentFilter={agentFilter}
            onSelect={(e) =>
              setSelected({ agentId: e.agent_id, sessionId: e.session_id })
            }
          />
        </>
      )}

      {selected && (
        <SessionDetailDrawer
          agentId={selected.agentId}
          sessionId={selected.sessionId}
          onClose={() => setSelected(null)}
        />
      )}
    </>
  );
}

// =====================================================================
// Buckets 状态栏（5 agent 横排）
// =====================================================================

function BucketsStatus({
  buckets,
  currentFilter,
  onPick,
}: {
  buckets: AgentBucket[];
  currentFilter: string | null;
  onPick: (id: string | null) => void;
}) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
      <BucketChip
        agentId="__all__"
        label="全部"
        count={buckets
          .filter((b) => b.supported)
          .reduce((sum, b) => sum + b.entry_count, 0)}
        supported
        active={currentFilter === null}
        onClick={() => onPick(null)}
      />
      {buckets.map((b) => (
        <BucketChip
          key={b.agent_id}
          agentId={b.agent_id}
          label={agentDisplayName(b.agent_id)}
          count={b.entry_count}
          supported={b.supported}
          unsupportedReason={b.unsupported_reason ?? undefined}
          dataRoot={b.data_root}
          active={currentFilter === b.agent_id}
          onClick={() => {
            if (!b.supported) return;
            onPick(currentFilter === b.agent_id ? null : b.agent_id);
          }}
        />
      ))}
    </div>
  );
}

function BucketChip({
  agentId,
  label,
  count,
  supported,
  unsupportedReason,
  dataRoot,
  active,
  onClick,
}: {
  agentId: string;
  label: string;
  count: number;
  supported: boolean;
  unsupportedReason?: string;
  dataRoot?: string;
  active: boolean;
  onClick: () => void;
}) {
  const disabled = !supported && agentId !== "__all__";
  const title = unsupportedReason ?? dataRoot ?? "";
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={cn(
        "group flex items-start gap-2.5 rounded-xl border p-3 text-left transition-all",
        active
          ? "border-primary-400 bg-primary-50 dark:border-primary-600 dark:bg-primary-950/30"
          : supported
            ? "border-border bg-card hover:border-primary-300/60 hover:shadow-sm dark:hover:border-primary-700/60"
            : "border-dashed border-border/60 bg-muted/30",
        disabled && "cursor-not-allowed opacity-70",
      )}
    >
      <div className="shrink-0 rounded-md bg-card p-1.5 dark:bg-background/40">
        <AgentIcon agentId={agentId} size={18} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="truncate text-sm font-medium">{label}</span>
          {!supported && agentId !== "__all__" && (
            <Info className="h-3 w-3 shrink-0 text-warning-strong" />
          )}
        </div>
        <p className="mt-0.5 font-mono text-[10px] text-muted-foreground">
          {supported ? `${count} 个会话` : "不可用"}
        </p>
      </div>
    </button>
  );
}

// =====================================================================
// 时间线（master 列表）
// =====================================================================

function Timeline({
  entries,
  totalCount,
  search,
  agentFilter,
  onSelect,
}: {
  entries: HistoryEntry[];
  totalCount: number;
  search: string;
  agentFilter: string | null;
  onSelect: (e: HistoryEntry) => void;
}) {
  // 按日期分组（hook 必须无条件调用，放在 early return 前）
  const [visibleCount, setVisibleCount] = useState(100);
  const groups = useMemo(() => groupByDay(entries.slice(0, visibleCount)), [entries, visibleCount]);

  if (totalCount === 0) {
    return (
      <div className="mt-6 rounded-xl border border-dashed border-border bg-card/50 px-6 py-12 text-center">
        <MessageSquare className="mx-auto h-8 w-8 text-muted-foreground/50" />
        <p className="mt-3 text-sm text-foreground">尚无历史对话</p>
        <p className="mt-1 text-xs text-muted-foreground">
          在任意 AI 代理里跑过会话后，这里会自动出现
        </p>
      </div>
    );
  }

  if (entries.length === 0) {
    return (
      <div className="mt-6 rounded-xl border border-dashed border-border bg-card/50 px-6 py-10 text-center">
        <p className="text-sm text-muted-foreground">
          {agentFilter
            ? `无匹配 ${agentDisplayName(agentFilter)} ${search ? `+ "${search}"` : ""} 的会话`
            : `无匹配 "${search}" 的会话`}
        </p>
      </div>
    );
  }

  return (
    <div className="mt-6 space-y-4">
      <p className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
        显示 {Math.min(visibleCount, entries.length)} / {totalCount} 个会话
      </p>
      {groups.map((g) => (
        <div key={g.day} className="space-y-1.5">
          <h2 className="sticky top-0 z-10 -mx-1 bg-background/80 px-1 py-1 font-serif text-sm font-semibold backdrop-blur">
            {g.day}
            <span className="ml-2 font-mono text-[10px] font-normal text-muted-foreground">
              {g.items.length}
            </span>
          </h2>
          <ul className="space-y-1">
            {g.items.map((e) => (
              <li key={`${e.agent_id}:${e.session_id}`}>
                <TimelineRow entry={e} onClick={() => onSelect(e)} />
              </li>
            ))}
          </ul>
        </div>
      ))}
      {visibleCount < entries.length && (
        <button type="button" className="hb-btn-outline" onClick={() => setVisibleCount(count => count + 100)}>
          加载更多会话（剩余 {entries.length - visibleCount}）
        </button>
      )}
    </div>
  );
}

function TimelineRow({
  entry,
  onClick,
}: {
  entry: HistoryEntry;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="group flex w-full items-start gap-3 rounded-lg border border-border/60 bg-card px-3 py-2.5 text-left transition-all hover:border-primary-300/60 hover:shadow-sm dark:hover:border-primary-700/60"
    >
      <div className="mt-0.5 shrink-0 rounded-md bg-muted p-1.5 transition-colors group-hover:bg-primary-50 dark:group-hover:bg-primary-950/40">
        <AgentIcon agentId={entry.agent_id} size={16} />
      </div>
      <div className="min-w-0 flex-1">
        <p className="line-clamp-2 text-[13px] leading-snug">
          {entry.first_user_prompt ?? (
            <span className="italic text-muted-foreground">（无 user 消息）</span>
          )}
        </p>
        <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-[10px] text-muted-foreground">
          <span className="rounded bg-muted px-1.5 py-px font-sans font-medium">
            {agentDisplayName(entry.agent_id)}
          </span>
          {entry.cwd && (
            <span className="truncate" title={entry.cwd}>
              {compactCwd(entry.cwd)}
            </span>
          )}
          <span className="ml-auto whitespace-nowrap">
            {formatRelativeIso(entry.last_active_at)}
          </span>
          <span className="whitespace-nowrap">{entry.message_count} 条</span>
        </div>
      </div>
    </button>
  );
}

// =====================================================================
// 详情抽屉（detail）
// =====================================================================

function SessionDetailDrawer({
  agentId,
  sessionId,
  onClose,
}: {
  agentId: string;
  sessionId: string;
  onClose: () => void;
}) {
  const detailQuery = useQuery({
    queryKey: ["agents", "history", "read", agentId, sessionId],
    queryFn: () => agentsApi.historyRead(agentId, sessionId),
    refetchOnWindowFocus: false,
  });

  // ESC 关闭
  useEffect(() => {
    const h = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", h);
    return () => window.removeEventListener("keydown", h);
  }, [onClose]);

  return (
    <>
      <div
        className="fixed inset-0 z-40 bg-black/30 backdrop-blur-[2px]"
        onClick={onClose}
        aria-hidden
      />
      <aside
        role="dialog"
        aria-label="会话详情"
        className="fixed inset-y-0 right-0 z-50 flex w-full max-w-2xl flex-col border-l border-border bg-card shadow-2xl"
      >
        <header className="flex items-start justify-between gap-3 border-b border-border/60 px-5 py-4">
          <div className="flex min-w-0 items-start gap-3">
            <div className="shrink-0 rounded-md bg-muted p-2">
              <AgentIcon agentId={agentId} size={20} />
            </div>
            <div className="min-w-0">
              <h2 className="font-serif text-base font-semibold tracking-tight">
                {agentDisplayName(agentId)} · 会话详情
              </h2>
              <code className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground">
                {sessionId}
              </code>
            </div>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="关闭 (ESC)"
            aria-label="关闭"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="flex-1 overflow-y-auto">
          {detailQuery.isLoading && (
            <div className="flex items-center gap-2 p-6 text-xs text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              解析会话…
            </div>
          )}
          {detailQuery.isError && (
            <div className="m-5 flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>读取失败：{String(detailQuery.error ?? "未知错误")}</span>
            </div>
          )}
          {detailQuery.data && <DetailContent session={detailQuery.data} />}
        </div>
      </aside>
    </>
  );
}

function DetailContent({ session }: { session: HistorySession }) {
  const tokenTotal =
    session.stats.total_input_tokens + session.stats.total_output_tokens;

  return (
    <>
      <div className="border-b border-border/60 px-5 py-3">
        <div className="space-y-0.5 text-[11px]">
          {session.cwd && (
            <div
              className="flex items-baseline gap-2 font-mono text-muted-foreground"
              title={session.cwd}
            >
              <span className="text-[9px] uppercase tracking-wider">cwd</span>
              <code className="truncate">{session.cwd}</code>
            </div>
          )}
          <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-[10px] text-muted-foreground">
            {session.git_branch && <span>branch: {session.git_branch}</span>}
            {session.version && <span>· v{session.version}</span>}
            <span>· {session.messages.length} 条消息</span>
            {tokenTotal > 0 && (
              <span>
                · {session.stats.total_input_tokens}↓/
                {session.stats.total_output_tokens}↑ tok
              </span>
            )}
            {session.stats.duration_seconds > 0 && (
              <span>· {formatDuration(session.stats.duration_seconds)}</span>
            )}
          </div>
        </div>
      </div>

      <div className="px-5 py-3">
        {session.messages.length === 0 ? (
          <div className="flex h-32 items-center justify-center text-xs text-muted-foreground">
            无消息
          </div>
        ) : (
          <ul className="space-y-2">
            {session.messages.map((m) => (
              <MessageBubble key={m.uuid} msg={m} />
            ))}
          </ul>
        )}
      </div>
    </>
  );
}

function MessageBubble({ msg }: { msg: HistoryMessage }) {
  const isUser = msg.role === "user";
  const isAssistant = msg.role === "assistant";
  const isTool = msg.role === "tool" || msg.role === "function";
  const isDeveloperOrSystem =
    msg.role === "developer" || msg.role === "system";

  return (
    <li
      className={cn(
        "rounded-md border px-3 py-2 text-[12px] leading-relaxed",
        isUser &&
          "border-primary-200 bg-primary-50/40 dark:border-primary-800/60 dark:bg-primary-950/20",
        isAssistant && "border-border bg-card",
        isTool && "border-border/60 bg-muted/40",
        isDeveloperOrSystem &&
          "border-dashed border-border/60 bg-muted/10 text-muted-foreground",
      )}
    >
      <div className="mb-1 flex items-center gap-1.5 text-[10px]">
        <span
          className={cn(
            "rounded px-1.5 py-0.5 font-mono uppercase",
            isUser &&
              "bg-primary-100 text-primary-800 dark:bg-primary-900/40 dark:text-primary-200",
            isAssistant && "bg-muted text-foreground",
            isTool && "bg-muted/70 text-muted-foreground",
            isDeveloperOrSystem && "bg-transparent text-muted-foreground",
          )}
        >
          {msg.role}
        </span>
        <span className="font-mono text-muted-foreground">
          {formatShortTime(msg.timestamp)}
        </span>
        {msg.model && (
          <code className="ml-1 truncate font-mono text-[9.5px] text-muted-foreground/70">
            {msg.model}
          </code>
        )}
        {(msg.input_tokens || msg.output_tokens) && (
          <span className="ml-auto font-mono text-[9.5px] text-muted-foreground/70">
            {msg.input_tokens ?? 0}↓/{msg.output_tokens ?? 0}↑
          </span>
        )}
      </div>
      <pre className="whitespace-pre-wrap break-words font-sans text-[12px]">
        {msg.content}
      </pre>
    </li>
  );
}

// =====================================================================
// 占位 / 错误视图
// =====================================================================

function LoadingPlaceholder() {
  return (
    <div className="flex items-center gap-2 p-6 text-sm text-muted-foreground">
      <Loader2 className="h-4 w-4 animate-spin" />
      正在汇总跨 agent 历史…
    </div>
  );
}

function ErrorPanel({ err }: { err: unknown }) {
  return (
    <div className="rounded-xl border border-destructive/40 bg-destructive/5 p-5">
      <div className="flex items-start gap-2 text-destructive-strong">
        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
        <div>
          <p className="font-medium">读取历史失败</p>
          <p className="mt-1 font-mono text-xs">{String(err ?? "未知错误")}</p>
        </div>
      </div>
    </div>
  );
}

// =====================================================================
// helpers
// =====================================================================

function agentDisplayName(id: string): string {
  switch (id) {
    case "claude-code":
      return "Claude Code";
    case "codex":
      return "Codex";
    case "gemini-cli":
      return "Gemini CLI";
    case "openclaw":
      return "OpenClaw";
    case "hermes":
      return "Hermes";
    default:
      return id;
  }
}

function AgentIcon({ agentId, size = 18 }: { agentId: string; size?: number }) {
  // lobehub icons 的 .Color 子组件给彩色版本；不在 lobehub 的用 lucide 占位
  switch (agentId) {
    case "__all__":
      return <Database size={size} className="text-primary-600 dark:text-primary-400" />;
    case "claude-code":
      return <ClaudeColor size={size} />;
    case "codex":
      return <CodexColor size={size} />;
    case "gemini-cli":
      return <GeminiColor size={size} />;
    case "openclaw":
      return <OpenClawColor size={size} />;
    case "hermes":
      return <Bot size={size} className="text-muted-foreground" />;
    default:
      return <Bot size={size} className="text-muted-foreground" />;
  }
}

/** 把长 cwd 简化成尾部 2-3 段（保留可读性）*/
function compactCwd(cwd: string): string {
  const parts = cwd.replace(/[\\/]+/g, "/").split("/").filter(Boolean);
  if (parts.length <= 3) return cwd;
  return ".../" + parts.slice(-3).join("/");
}

interface DayGroup {
  day: string;
  items: HistoryEntry[];
}

function groupByDay(entries: HistoryEntry[]): DayGroup[] {
  const map = new Map<string, HistoryEntry[]>();
  for (const e of entries) {
    const day = formatDayLabel(e.last_active_at);
    const arr = map.get(day) ?? [];
    arr.push(e);
    map.set(day, arr);
  }
  return Array.from(map.entries()).map(([day, items]) => ({ day, items }));
}

function formatDayLabel(iso: string): string {
  if (!iso) return "未知日期";
  try {
    const d = new Date(iso);
    const today = new Date();
    const sameDay =
      d.getFullYear() === today.getFullYear() &&
      d.getMonth() === today.getMonth() &&
      d.getDate() === today.getDate();
    if (sameDay) return "今天";
    const yesterday = new Date(today);
    yesterday.setDate(yesterday.getDate() - 1);
    const sameYesterday =
      d.getFullYear() === yesterday.getFullYear() &&
      d.getMonth() === yesterday.getMonth() &&
      d.getDate() === yesterday.getDate();
    if (sameYesterday) return "昨天";
    const yyyy = d.getFullYear();
    const mm = String(d.getMonth() + 1).padStart(2, "0");
    const dd = String(d.getDate()).padStart(2, "0");
    return `${yyyy}-${mm}-${dd}`;
  } catch {
    return iso.slice(0, 10);
  }
}

function formatShortTime(iso: string): string {
  try {
    const d = new Date(iso);
    const mm = String(d.getMonth() + 1).padStart(2, "0");
    const dd = String(d.getDate()).padStart(2, "0");
    const hh = String(d.getHours()).padStart(2, "0");
    const mi = String(d.getMinutes()).padStart(2, "0");
    return `${mm}-${dd} ${hh}:${mi}`;
  } catch {
    return iso.slice(5, 16).replace("T", " ");
  }
}

function formatRelativeIso(iso: string): string {
  if (!iso) return "";
  try {
    const d = new Date(iso);
    const diff = Date.now() - d.getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return "刚刚";
    if (mins < 60) return `${mins} 分钟前`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `${hours} 小时前`;
    const days = Math.floor(hours / 24);
    if (days < 30) return `${days} 天前`;
    return iso.slice(0, 10);
  } catch {
    return iso.slice(0, 16);
  }
}

function formatDuration(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  if (m < 60) return `${m}m${seconds % 60}s`;
  const h = Math.floor(m / 60);
  return `${h}h${m % 60}m`;
}
