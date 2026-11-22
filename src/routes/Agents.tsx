import { useEffect, useMemo, useState } from "react";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import Codex from "@lobehub/icons/es/Codex/components/Mono";
import OpenClaw from "@lobehub/icons/es/OpenClaw/components/Mono";
import ClaudeColor from "@lobehub/icons/es/Claude/components/Color";
import GeminiColor from "@lobehub/icons/es/Gemini/components/Color";
import {
  AlertTriangle,
  Bot,
  Check,
  CheckCircle2,
  ChevronRight,
  Copy,
  Cpu,
  FileText,
  History,
  Info,
  Loader2,
  MessageSquare,
  Play,
  RefreshCw,
  Save,
  Sparkles,
  X,
} from "lucide-react";
import {
  agentsApi,
  type AgentEndpointInfo,
  type AgentInfo,
  type AgentModelInfo,
  type AgentState,
  type AgentStatus,
  type EndpointConfig,
  type EndpointTestResult,
  type HistoryEntry,
  type HistoryMessage,
  type HistorySession,
  type ModelIdEntry,
} from "@/lib/tauri-api";
import { EmptyPlaceholder } from "@/components/empty-placeholder";
import { AgentVersionsSection } from "@/components/agent-versions-section";
import { SharedSecretsSection } from "@/components/shared-secrets-section";
import { cn } from "@/lib/utils";

/**
 * AI 代理 — 5 个内置 agent 的状态检测 + 启停（v3 §6.2）
 *
 * Phase 13.1 Day 11-12（已实现）：
 * - sysinfo 真实进程检测（每 3 秒轮询）
 * - 配置文件存在性检查（~/.claude/settings.json 等）
 * - 4 态语义色徽章：running 绿 / stopped 灰 / misconfigured 黄 / not_installed 红
 * - 官方品牌 logo（来自 @lobehub/icons）
 *
 * Phase 13.2 待加：启动按钮（agent-env adapter）+ 配置一键编辑 + MCP server 健康检查
 */
export function Agents() {
  const {
    data: agents = [],
    isLoading,
    isError,
    error,
    refetch,
  } = useQuery({
    queryKey: ["agents", "list"],
    queryFn: () => agentsApi.list(),
  });

  // 5 个 agent 的状态各自独立 poll，每 3 秒（v3 §6.2 要求）
  const statuses = useQueries({
    queries: agents.map((a) => ({
      queryKey: ["agents", "status", a.id],
      queryFn: () => agentsApi.status(a.id),
      refetchInterval: 3_000,
      enabled: agents.length > 0,
    })),
  });

  const runningCount = statuses.filter((s) => s.data?.state === "running").length;

  // 详情抽屉选中的 agent
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const selectedAgent = agents.find((a) => a.id === selectedAgentId) ?? null;
  const selectedStatus =
    selectedAgentId !== null
      ? statuses[agents.findIndex((a) => a.id === selectedAgentId)]?.data
      : undefined;

  return (
    <div className="animate-fade-in space-y-8">
      <header className="hb-page-header">
        <span className="hb-section-label">Agents</span>
        <h1>AI 代理</h1>
        <p>
          检测、启停 5 个内置 AI 代理 · 每 3 秒轮询进程状态
          {!isLoading && agents.length > 0 && (
            <span className="ml-2 font-mono text-xs text-muted-foreground">
              · 当前运行 {runningCount}/{agents.length}
            </span>
          )}
        </p>
      </header>

      {isLoading ? (
        <AgentsSkeleton />
      ) : isError ? (
        <EmptyPlaceholder
          title="读取代理列表失败"
          description={String(error ?? "未知错误，请稍后重试")}
          icon="⚠️"
          action={
            <button className="hb-btn-outline" onClick={() => refetch()}>
              重试
            </button>
          }
        />
      ) : agents.length === 0 ? (
        <EmptyPlaceholder
          title="未读取到代理"
          description="agents_list 返回空"
          icon="🤖"
        />
      ) : (
        <section
          aria-label="代理列表"
          className="grid grid-cols-1 gap-5 lg:grid-cols-2 xl:grid-cols-3"
        >
          {agents.map((a, i) => (
            <AgentCard
              key={a.id}
              agent={a}
              status={statuses[i]?.data}
              statusLoading={statuses[i]?.isLoading ?? false}
              onSelect={() => setSelectedAgentId(a.id)}
            />
          ))}
        </section>
      )}

      <AgentDetailDrawer
        agent={selectedAgent}
        status={selectedStatus}
        open={selectedAgentId !== null}
        onClose={() => setSelectedAgentId(null)}
      />

      {/* S5 · Agent 版本检测（npm registry 拉最新 · 1h 缓存） */}
      <AgentVersionsSection />

      {/* S3 · 共享 Secrets · ACL 矩阵 + IPC 状态 + 能打开 SOPS 编辑器 */}
      <SharedSecretsSection />
    </div>
  );
}

interface AgentCardProps {
  agent: AgentInfo;
  status: AgentStatus | undefined;
  statusLoading: boolean;
  onSelect: () => void;
}

function AgentCard({ agent, status, statusLoading, onSelect }: AgentCardProps) {
  const queryClient = useQueryClient();
  const state: AgentState | "loading" = statusLoading
    ? "loading"
    : status?.state ?? "stopped";

  const [feedback, setFeedback] = useState<{
    tone: "success" | "error";
    msg: string;
  } | null>(null);

  // feedback 自动淡出（成功 4s / 失败 10s）
  useEffect(() => {
    if (!feedback) return;
    const t = setTimeout(
      () => setFeedback(null),
      feedback.tone === "success" ? 4000 : 10000,
    );
    return () => clearTimeout(t);
  }, [feedback]);

  const startMutation = useMutation({
    mutationFn: () => agentsApi.start(agent.id),
    onSuccess: () => {
      setFeedback({
        tone: "success",
        msg: "启动命令已发送 · 新终端窗口已打开",
      });
      // 4s 后刷新状态（给 agent 进程启动时间）
      setTimeout(() => {
        queryClient.invalidateQueries({
          queryKey: ["agents", "status", agent.id],
        });
      }, 4000);
    },
    onError: (e) =>
      setFeedback({
        tone: "error",
        msg: e instanceof Error ? e.message : String(e),
      }),
  });

  const openConfigMutation = useMutation({
    mutationFn: () => agentsApi.openConfig(agent.id),
    onSuccess: () =>
      setFeedback({ tone: "success", msg: "已用默认编辑器打开配置文件" }),
    onError: (e) =>
      setFeedback({
        tone: "error",
        msg: e instanceof Error ? e.message : String(e),
      }),
  });

  // 启动按钮禁用场景
  const startDisabled = !agent.launch_subcommand || state === "running" || startMutation.isPending;
  const startDisabledReason = !agent.launch_subcommand
    ? "此 agent 不支持一键启动（请手动启动）"
    : state === "running"
      ? "agent 已在运行中"
      : startMutation.isPending
        ? "启动中…"
        : undefined;

  // 打开配置禁用场景
  const openConfigDisabled =
    !agent.config_path || !agent.config_exists || openConfigMutation.isPending;
  const openConfigDisabledReason = !agent.config_path
    ? "此 agent 无独立配置文件"
    : !agent.config_exists
      ? "配置文件不存在（请先初始化 agent）"
      : openConfigMutation.isPending
        ? "打开中…"
        : undefined;

  return (
    <article
      className="hb-card group cursor-pointer transition-colors hover:border-primary-400/60"
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      role="button"
      tabIndex={0}
      aria-label={`查看 ${agent.name} 详情`}
    >
      {/* 右上角「详情」提示 */}
      <span className="absolute right-3 top-3 inline-flex items-center gap-0.5 rounded bg-muted/50 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100">
        详情
        <ChevronRight className="h-2.5 w-2.5" />
      </span>
      <header className="flex items-start gap-4">
        <AgentLogo id={agent.id} />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-3">
            <h3 className="font-serif text-base font-semibold tracking-tight">
              {agent.name}
            </h3>
            <StateBadge state={state} />
          </div>
          {/* PID + 进程匹配关键字 */}
          <p className="mt-1 font-mono text-xs text-muted-foreground">
            {status?.pid ? (
              <>
                PID {status.pid} · 进程匹配{" "}
                <span className="opacity-70">
                  {agent.process_match.join(" / ")}
                </span>
              </>
            ) : (
              <>
                未运行 · 进程关键字{" "}
                <span className="opacity-70">
                  {agent.process_match.join(" / ")}
                </span>
              </>
            )}
          </p>
        </div>
      </header>

      {/* 配置路径 + 配置存在性指示 */}
      <div className="mt-4 space-y-2 border-t border-border/60 pt-3 text-xs">
        {agent.config_path ? (
          <div className="flex items-center gap-2">
            <span className="text-muted-foreground">配置：</span>
            <span
              className={cn(
                "truncate font-mono",
                agent.config_exists ? "text-foreground" : "text-warning",
              )}
              title={agent.config_path}
            >
              {agent.config_path}
            </span>
            <span
              className={cn(
                "ml-auto shrink-0 rounded px-2 py-0.5 font-mono text-[11px] font-semibold",
                agent.config_exists
                  ? "bg-success/10 text-success-strong"
                  : "bg-warning/10 text-warning-strong",
              )}
            >
              {agent.config_exists ? "已配置" : "未找到"}
            </span>
          </div>
        ) : (
          <div className="flex items-center gap-2 text-muted-foreground">
            <span>配置：</span>
            <span className="opacity-60">无独立配置文件（直接调主命令）</span>
          </div>
        )}
        {agent.launch_subcommand && (
          <div className="flex items-center gap-2">
            <span className="text-muted-foreground">启动：</span>
            <span className="font-mono">
              agent-env {agent.launch_subcommand}
            </span>
          </div>
        )}
      </div>

      {/* Action row — 一键启动 + 打开配置（点击 stopPropagation 避免冒泡到卡片）*/}
      <div className="mt-4 flex items-center gap-2" onClick={(e) => e.stopPropagation()}>
        <button
          type="button"
          className={cn(
            "inline-flex flex-1 items-center justify-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium shadow-sm transition-colors",
            "bg-primary-500 text-white hover:bg-primary-600",
            "disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-foreground disabled:shadow-none",
          )}
          onClick={(e) => {
            e.stopPropagation();
            setFeedback(null);
            startMutation.mutate();
          }}
          disabled={startDisabled}
          title={startDisabledReason}
        >
          {startMutation.isPending ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Play className="h-3.5 w-3.5" />
          )}
          {state === "running" ? "运行中" : startMutation.isPending ? "启动中…" : "启动"}
        </button>
        <button
          type="button"
          className={cn(
            "inline-flex flex-1 items-center justify-center gap-1.5 rounded-md border border-border bg-background px-3 py-1.5 text-xs font-medium transition-colors",
            "hover:bg-muted hover:text-foreground",
            "disabled:cursor-not-allowed disabled:opacity-50",
          )}
          onClick={(e) => {
            e.stopPropagation();
            setFeedback(null);
            openConfigMutation.mutate();
          }}
          disabled={openConfigDisabled}
          title={openConfigDisabledReason}
        >
          {openConfigMutation.isPending ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <FileText className="h-3.5 w-3.5" />
          )}
          打开配置
        </button>
      </div>

      {feedback && (
        <div
          className={cn(
            "mt-2 flex items-start gap-1.5 rounded-md border px-2.5 py-1.5 text-xs",
            feedback.tone === "success"
              ? "border-success/50 bg-success/10 text-success-strong"
              : "border-destructive/50 bg-destructive/10 text-destructive-strong",
          )}
        >
          {feedback.tone === "success" ? (
            <Check className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          ) : (
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          )}
          <span className="break-words">{feedback.msg}</span>
        </div>
      )}
    </article>
  );
}

/** LobeHub 官方品牌 logo；找不到时回退到 lucide Bot 图标 */
function AgentLogo({ id }: { id: string }) {
  const wrapper = "flex h-10 w-10 shrink-0 items-center justify-center rounded-lg";
  switch (id) {
    case "claude-code":
      return (
        <div className={cn(wrapper, "bg-[#D97757]/10")}>
          <ClaudeColor size={28} />
        </div>
      );
    case "codex":
      return (
        <div className={cn(wrapper, "bg-foreground/5")}>
          <Codex size={28} />
        </div>
      );
    case "gemini-cli":
      return (
        <div className={cn(wrapper, "bg-[#4285F4]/10")}>
          <GeminiColor size={28} />
        </div>
      );
    case "openclaw":
      return (
        <div className={cn(wrapper, "bg-[#E63946]/10")}>
          <OpenClaw size={28} />
        </div>
      );
    case "hermes":
    default:
      // LobeHub 的 Hermes Agent 图标 export 名不确定 — 用 lucide Bot 占位
      return (
        <div className={cn(wrapper, "bg-primary-100 dark:bg-primary-900/30")}>
          <Bot className="h-7 w-7 text-primary-700 dark:text-primary-300" />
        </div>
      );
  }
}

function StateBadge({ state }: { state: AgentState | "loading" }) {
  const palette = stateBadgePalette(state);
  const label = stateBadgeLabel(state);
  return (
    <span
      className={cn(
        // 2026-05-03 修：字号从 text-xs 升至 text-sm，加 font-semibold 增强可读性；
        // 配色从 text-*-foreground（给实色背景用）改为 text-*-strong（给软背景用）。
        "inline-flex items-center gap-1.5 rounded-full border px-3 py-1 text-sm font-semibold",
        palette,
      )}
    >
      <Dot state={state} />
      {label}
    </span>
  );
}

function Dot({ state }: { state: AgentState | "loading" }) {
  const color = stateDotColor(state);
  return (
    <span
      className={cn(
        "h-1.5 w-1.5 rounded-full",
        color,
        state === "running" && "animate-pulse",
      )}
      aria-hidden
    />
  );
}

function stateBadgeLabel(state: AgentState | "loading"): string {
  switch (state) {
    case "running":
      return "运行中";
    case "stopped":
      return "已停止";
    case "misconfigured":
      return "未配置";
    case "not_installed":
      return "未安装";
    case "loading":
      return "检测中";
  }
}

function stateBadgePalette(state: AgentState | "loading"): string {
  // 所有软背景（bg-X/10）配对应 text-X-strong（WCAG AA 4.5:1 对比度）
  switch (state) {
    case "running":
      return "border-success/50 bg-success/10 text-success-strong";
    case "stopped":
      return "border-border bg-muted text-foreground";
    case "misconfigured":
      return "border-warning/50 bg-warning/10 text-warning-strong";
    case "not_installed":
      return "border-destructive/50 bg-destructive/10 text-destructive-strong";
    case "loading":
      return "border-border bg-muted/50 text-muted-foreground";
  }
}

function stateDotColor(state: AgentState | "loading"): string {
  switch (state) {
    case "running":
      return "bg-success";
    case "stopped":
      return "bg-muted-foreground";
    case "misconfigured":
      return "bg-warning";
    case "not_installed":
      return "bg-destructive";
    case "loading":
      return "bg-muted-foreground/50";
  }
}

function AgentsSkeleton() {
  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-2 xl:grid-cols-3" aria-busy>
      {Array.from({ length: 4 }).map((_, i) => (
        <div key={i} className="hb-card">
          <div className="flex items-start gap-4">
            <div className="h-10 w-10 shrink-0 animate-pulse rounded-lg bg-muted" />
            <div className="flex-1 space-y-2">
              <div className="h-5 w-32 animate-pulse rounded bg-muted" />
              <div className="h-3 w-48 animate-pulse rounded bg-muted" />
            </div>
          </div>
          <div className="mt-4 h-3 w-3/4 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}

// =====================================================================
// AI 代理 · 详情抽屉（Phase 1）
// =====================================================================
//
// Phase 1 范围：
// - 4 tab 骨架：概览 / 模型 / 历史 / 资源
// - 概览 tab：完整可用（status 信息聚合 · 配置路径 · 启动命令）
// - 模型 tab：完整可用（dropdown 选预设 + 保存写入 settings.json/config.toml）
// - 历史 tab + 资源 tab：placeholder，标 "Phase 2/3 接入"

type DetailTab = "overview" | "model" | "history" | "resources";

interface AgentDetailDrawerProps {
  agent: AgentInfo | null;
  status: AgentStatus | undefined;
  open: boolean;
  onClose: () => void;
}

function AgentDetailDrawer({
  agent,
  status,
  open,
  onClose,
}: AgentDetailDrawerProps) {
  const [tab, setTab] = useState<DetailTab>("overview");

  // 切 agent 时回到概览 tab
  useEffect(() => {
    if (open) setTab("overview");
  }, [agent?.id, open]);

  // ESC 关闭
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, onClose]);

  return (
    <>
      {/* 遮罩 */}
      <div
        className={cn(
          "fixed inset-0 z-40 bg-black/30 backdrop-blur-[2px] transition-opacity duration-200",
          open ? "opacity-100" : "pointer-events-none opacity-0",
        )}
        onClick={onClose}
        aria-hidden
      />
      {/* 抽屉 */}
      <aside
        className={cn(
          "fixed inset-y-0 right-0 z-50 flex w-full max-w-lg flex-col border-l border-border bg-card shadow-2xl transition-transform duration-200 ease-out",
          open ? "translate-x-0" : "translate-x-full",
        )}
        role="dialog"
        aria-label={agent ? `${agent.name} 详情` : "代理详情"}
      >
        {agent && (
          <>
            {/* 顶栏 */}
            <header className="flex items-start justify-between gap-4 border-b border-border/60 px-5 py-4">
              <div className="flex min-w-0 items-start gap-3">
                <AgentLogo id={agent.id} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <h2 className="font-serif text-lg font-semibold tracking-tight">
                      {agent.name}
                    </h2>
                    <StateBadge state={status?.state ?? "stopped"} />
                  </div>
                  <p className="mt-0.5 font-mono text-[11px] text-muted-foreground">
                    {agent.id}
                  </p>
                </div>
              </div>
              <button
                type="button"
                onClick={onClose}
                className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                title="关闭 (ESC)"
              >
                <X className="h-4 w-4" />
              </button>
            </header>

            {/* Tab 栏 */}
            <nav className="flex items-center gap-1 border-b border-border/60 px-3 py-2">
              <DetailTabButton
                active={tab === "overview"}
                onClick={() => setTab("overview")}
                icon={Info}
                label="概览"
              />
              <DetailTabButton
                active={tab === "model"}
                onClick={() => setTab("model")}
                icon={Sparkles}
                label="模型"
              />
              <DetailTabButton
                active={tab === "history"}
                onClick={() => setTab("history")}
                icon={MessageSquare}
                label="历史"
              />
              <DetailTabButton
                active={tab === "resources"}
                onClick={() => setTab("resources")}
                icon={Cpu}
                label="资源"
                badge="规划中"
              />
            </nav>

            {/* Tab 内容 */}
            <div className="flex-1 overflow-y-auto">
              {tab === "overview" && <OverviewTab agent={agent} status={status} />}
              {tab === "model" && <ModelTab agent={agent} />}
              {tab === "history" && <HistoryTab agent={agent} />}
              {tab === "resources" && <ComingSoonTab phase="规划中" feature="进程资源 · token 统计 · 运行日志" hint="sysinfo PID CPU/RAM · cost 统计 · log tail" />}
            </div>
          </>
        )}
      </aside>
    </>
  );
}

function DetailTabButton({
  active,
  onClick,
  icon: Icon,
  label,
  badge,
}: {
  active: boolean;
  onClick: () => void;
  icon: typeof Info;
  label: string;
  badge?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
        active
          ? "bg-primary-100 text-primary-800 dark:bg-primary-900/40 dark:text-primary-200"
          : "text-muted-foreground hover:bg-muted hover:text-foreground",
      )}
    >
      <Icon className="h-3.5 w-3.5" />
      {label}
      {badge && (
        <span className="rounded-full bg-muted px-1.5 py-0.5 font-mono text-[9px] text-muted-foreground">
          {badge}
        </span>
      )}
    </button>
  );
}

// ---------------------------------------------------------------------
// 概览 Tab
// ---------------------------------------------------------------------

function OverviewTab({
  agent,
  status,
}: {
  agent: AgentInfo;
  status: AgentStatus | undefined;
}) {
  return (
    <div className="space-y-5 p-5 text-sm">
      <DetailField label="状态">
        <div className="flex items-center gap-2">
          <StateBadge state={status?.state ?? "stopped"} />
          {status?.pid && (
            <span className="font-mono text-[11px] text-muted-foreground">
              PID {status.pid}
            </span>
          )}
        </div>
      </DetailField>

      <DetailField label="进程匹配关键字">
        <div className="flex flex-wrap gap-1.5">
          {agent.process_match.map((kw) => (
            <code
              key={kw}
              className="rounded border border-border bg-muted/40 px-1.5 py-0.5 font-mono text-[11px]"
            >
              {kw}
            </code>
          ))}
        </div>
      </DetailField>

      <DetailField label="配置文件">
        {agent.config_path ? (
          <div className="space-y-1">
            <code className="block break-all rounded-md border border-border/60 bg-muted/30 px-2.5 py-1.5 font-mono text-[11px]">
              {agent.config_path}
            </code>
            <span
              className={cn(
                "inline-block rounded px-1.5 py-0.5 font-mono text-[10px]",
                agent.config_exists
                  ? "bg-success/10 text-success-strong"
                  : "bg-warning/10 text-warning-strong",
              )}
            >
              {agent.config_exists ? "已配置" : "未找到"}
            </span>
          </div>
        ) : (
          <span className="text-muted-foreground">无独立配置文件</span>
        )}
      </DetailField>

      <DetailField label="启动命令">
        {agent.launch_subcommand ? (
          <code className="block rounded-md border border-border/60 bg-muted/30 px-2.5 py-1.5 font-mono text-[11px]">
            agent-env {agent.launch_subcommand}
          </code>
        ) : (
          <span className="text-muted-foreground">无 launch_subcommand</span>
        )}
      </DetailField>

      <DetailField label="MCP servers">
        {status?.mcp_servers && status.mcp_servers.length > 0 ? (
          <ul className="space-y-1">
            {status.mcp_servers.map((s) => (
              <li
                key={s.name}
                className="flex items-center justify-between rounded border border-border/60 bg-muted/20 px-2 py-1 font-mono text-[11px]"
              >
                <span>{s.name}</span>
                <span
                  className={cn(
                    "rounded px-1.5 py-0.5 text-[10px]",
                    s.healthy
                      ? "bg-success/10 text-success-strong"
                      : "bg-destructive/10 text-destructive-strong",
                  )}
                >
                  {s.healthy ? "healthy" : "down"}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <span className="text-[11px] text-muted-foreground">
            无（或未实现 MCP 检测）
          </span>
        )}
      </DetailField>
    </div>
  );
}

function DetailField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      {children}
    </div>
  );
}

// ---------------------------------------------------------------------
// 模型 Tab
// ---------------------------------------------------------------------

function ModelTab({ agent }: { agent: AgentInfo }) {
  const qc = useQueryClient();
  const [savedFlash, setSavedFlash] = useState(false);

  const modelQuery = useQuery({
    queryKey: ["agents", "model", agent.id],
    queryFn: () => agentsApi.getModel(agent.id),
    refetchOnWindowFocus: false,
  });

  const setModelMutation = useMutation({
    mutationFn: (model: string) => agentsApi.setModel(agent.id, model),
    onSuccess: (info: AgentModelInfo) => {
      qc.setQueryData(["agents", "model", agent.id], info);
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 2000);
    },
  });

  if (modelQuery.isLoading) {
    return (
      <div className="flex items-center gap-2 p-6 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        加载模型配置…
      </div>
    );
  }

  if (modelQuery.isError) {
    return (
      <div className="m-5 flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
        <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        <span>读取失败：{String(modelQuery.error)}</span>
      </div>
    );
  }

  const info = modelQuery.data;
  if (!info) return null;

  // 不支持的 backend：直接显示原因
  if (!info.supported) {
    return (
      <div className="space-y-3 p-5">
        <div className="rounded-md border border-border/60 bg-muted/20 p-4">
          <div className="flex items-start gap-2">
            <Info className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
            <div>
              <p className="text-sm font-medium">此 agent 不支持模型修改</p>
              <p className="mt-1 text-xs text-muted-foreground">
                {info.unsupported_reason ?? "未知原因"}
              </p>
            </div>
          </div>
        </div>
        <p className="text-[11px] text-muted-foreground">
          如需修改模型，请直接打开配置文件或在 agent 内部命令里指定。
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-5 p-5">
      <div>
        <div className="mb-2">
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            当前模型
          </span>
        </div>
        {info.current ? (
          <code className="inline-block rounded-md border border-primary-300/60 bg-primary-50/40 px-2.5 py-1 font-mono text-xs text-primary-800 dark:border-primary-800/50 dark:bg-primary-950/40 dark:text-primary-200">
            {info.current}
          </code>
        ) : (
          <span className="text-xs italic text-muted-foreground">
            未设置（使用 backend 默认）
          </span>
        )}
      </div>

      <div>
        <div className="mb-2 flex items-center justify-between">
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            选择模型
          </span>
          {savedFlash && (
            <span className="inline-flex items-center gap-1 text-[11px] text-emerald-600 dark:text-emerald-400">
              <CheckCircle2 className="h-3 w-3" />
              已写入 {agent.config_path?.split("\\").pop()}
            </span>
          )}
        </div>

        <div className="space-y-1.5">
          {info.presets.map((preset) => {
            const isCurrent = info.current === preset.value;
            const isSaving = setModelMutation.isPending;
            return (
              <button
                key={preset.value}
                type="button"
                disabled={isSaving || isCurrent}
                onClick={() => setModelMutation.mutate(preset.value)}
                className={cn(
                  "flex w-full items-start gap-2.5 rounded-md border px-3 py-2 text-left transition-colors",
                  isCurrent
                    ? "border-primary-500 bg-primary-50/40 dark:bg-primary-950/40"
                    : "border-border bg-card hover:border-primary-400 hover:bg-muted/40",
                  (isSaving || isCurrent) && "cursor-default",
                )}
              >
                <div
                  className={cn(
                    "mt-0.5 h-3.5 w-3.5 shrink-0 rounded-full border-2",
                    isCurrent
                      ? "border-primary-600 bg-primary-600 ring-2 ring-primary-200 dark:ring-primary-900"
                      : "border-border",
                  )}
                />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="text-sm font-medium">{preset.label}</span>
                    <code className="font-mono text-[10px] text-muted-foreground/70">
                      {preset.value}
                    </code>
                    {isCurrent && (
                      <span className="ml-auto rounded bg-primary-100 px-1.5 py-0.5 font-mono text-[9px] uppercase text-primary-800 dark:bg-primary-900/40 dark:text-primary-200">
                        当前
                      </span>
                    )}
                  </div>
                  <div className="mt-0.5 text-[11px] text-muted-foreground">
                    {preset.hint}
                  </div>
                </div>
              </button>
            );
          })}
        </div>
      </div>

      {setModelMutation.isError && (
        <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>{String(setModelMutation.error)}</span>
        </div>
      )}

      {setModelMutation.isPending && (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          写入 {agent.config_path}…
        </div>
      )}

      <div className="rounded-md border border-dashed border-border/60 bg-muted/10 p-3 text-[11px] text-muted-foreground">
        <p className="mb-1 font-medium text-foreground/80">提示</p>
        <ul className="ml-4 list-disc space-y-0.5">
          <li>会写入 <code className="font-mono">{agent.config_path?.split("\\").pop()}</code> 顶层 <code className="font-mono">model</code> 字段</li>
          <li>其他字段（API key / settings 等）保持不变</li>
          <li>对正在运行的 agent 进程不会立即生效，下次启动时读取</li>
        </ul>
      </div>

      <EndpointSection agent={agent} />
    </div>
  );
}

// ---------------------------------------------------------------------
// Endpoint 配置 — 真实写入 ~/.claude/settings.json 的 env 块（Phase B · 2026-05-04）
// ---------------------------------------------------------------------
//
// Claude Code 的 endpoint 由环境变量控制：
//   - ANTHROPIC_BASE_URL          API 端点（OpenRouter / Anthropic / 自定义 gateway）
//   - ANTHROPIC_AUTH_TOKEN        Bearer token
//   - ANTHROPIC_DEFAULT_OPUS_MODEL / _SONNET_MODEL / _HAIKU_MODEL   alias → 具体 model ID
//
// 写到 settings.json 的 env 块（Claude Code 启动时自动注入子进程），不污染 system env。

const PROVIDER_OPTIONS: Array<{
  k: "anthropic-direct" | "openrouter" | "custom";
  label: string;
  base_url: string;
  auth_token_var: "ANTHROPIC_API_KEY" | "ANTHROPIC_AUTH_TOKEN";
  hint: string;
}> = [
  {
    k: "anthropic-direct",
    label: "Anthropic 直连",
    base_url: "https://api.anthropic.com",
    auth_token_var: "ANTHROPIC_API_KEY",
    hint: "官方 API · sk-ant- 开头的 key · 走 keychain 登录可不填",
  },
  {
    k: "openrouter",
    label: "OpenRouter",
    base_url: "https://openrouter.ai/api/v1",
    auth_token_var: "ANTHROPIC_AUTH_TOKEN",
    hint: "★ 推荐 · 单 key 访问 400+ 模型 · 自动 fallback · 当前推荐配置",
  },
  {
    k: "custom",
    label: "自定义 Gateway",
    base_url: "",
    auth_token_var: "ANTHROPIC_AUTH_TOKEN",
    hint: "Cloudflare AI Gateway / LiteLLM / 企业 proxy · 自填 base_url",
  },
];

function EndpointSection({ agent }: { agent: AgentInfo }) {
  const qc = useQueryClient();
  const [draft, setDraft] = useState<EndpointConfig | null>(null);
  const [savedFlash, setSavedFlash] = useState(false);
  const [testResult, setTestResult] = useState<EndpointTestResult | null>(null);

  const endpointQuery = useQuery({
    queryKey: ["agents", "endpoint", agent.id],
    queryFn: () => agentsApi.getEndpoint(agent.id),
    refetchOnWindowFocus: false,
  });

  // 第一次加载或外部刷新 → 把 server state 同步到 draft（不覆盖 user 正在编辑的）
  useEffect(() => {
    if (endpointQuery.data && draft === null) {
      setDraft(endpointQuery.data.current);
    }
  }, [endpointQuery.data, draft]);

  const setMutation = useMutation({
    mutationFn: (config: EndpointConfig) =>
      agentsApi.setEndpoint(agent.id, config),
    onSuccess: (info: AgentEndpointInfo) => {
      qc.setQueryData(["agents", "endpoint", agent.id], info);
      setDraft(info.current);
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 2500);
    },
  });

  const testMutation = useMutation({
    mutationFn: (config: EndpointConfig) =>
      agentsApi.testEndpoint(agent.id, config),
    onSuccess: (r) => setTestResult(r),
    onError: (e) =>
      setTestResult({
        ok: false,
        status_code: null,
        latency_ms: 0,
        error: String(e),
        body_preview: null,
      }),
  });

  if (endpointQuery.isLoading || !draft) {
    return (
      <div className="flex items-center gap-2 rounded-md border border-border/60 bg-muted/10 p-3 text-[11px] text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" />
        加载 endpoint 配置…
      </div>
    );
  }

  if (endpointQuery.isError) {
    return (
      <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
        <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        <span>读取 endpoint 失败：{String(endpointQuery.error)}</span>
      </div>
    );
  }

  const info = endpointQuery.data;
  if (!info) return null;

  if (!info.supported) {
    return (
      <div className="rounded-md border border-border/60 bg-muted/10 p-3 text-[11px] text-muted-foreground">
        {info.unsupported_reason ?? "此 agent 不支持 endpoint 配置"}
      </div>
    );
  }

  // OpenRouter Anthropic 模型按 tier 分组
  const presetsByTier = {
    opus: info.presets.openrouter_anthropic.filter((m) => m.tier === "opus"),
    sonnet: info.presets.openrouter_anthropic.filter((m) => m.tier === "sonnet"),
    haiku: info.presets.openrouter_anthropic.filter((m) => m.tier === "haiku"),
  };

  const isOpenRouter = draft.provider === "openrouter";
  const isAnthropic = draft.provider === "anthropic-direct";
  const secretChanged =
    draft.secret_short_name !== info.current.secret_short_name;
  const baseUrlChanged = draft.base_url !== info.current.base_url;
  const modelsChanged =
    draft.default_opus !== info.current.default_opus ||
    draft.default_sonnet !== info.current.default_sonnet ||
    draft.default_haiku !== info.current.default_haiku;
  const dirty =
    secretChanged ||
    baseUrlChanged ||
    modelsChanged ||
    draft.provider !== info.current.provider;

  const switchProvider = (k: typeof PROVIDER_OPTIONS[number]["k"]) => {
    const opt = PROVIDER_OPTIONS.find((p) => p.k === k);
    if (!opt) return;
    setDraft((d) =>
      d
        ? {
            ...d,
            provider: k,
            base_url: opt.base_url || d.base_url,
            auth_token_var: opt.auth_token_var,
            // 切到 OpenRouter 时自动建议默认 alias 映射
            default_opus:
              k === "openrouter"
                ? d.default_opus ?? "anthropic/claude-opus-4.7"
                : k === "anthropic-direct"
                  ? null
                  : d.default_opus,
            default_sonnet:
              k === "openrouter"
                ? d.default_sonnet ?? "anthropic/claude-sonnet-4.6"
                : k === "anthropic-direct"
                  ? null
                  : d.default_sonnet,
            default_haiku:
              k === "openrouter"
                ? d.default_haiku ?? "anthropic/claude-haiku-4.5"
                : k === "anthropic-direct"
                  ? null
                  : d.default_haiku,
          }
        : d,
    );
    setTestResult(null);
  };

  // 选 SOPS secret → 自动填 provider / base_url / auth_token_var
  const switchSecret = (shortName: string) => {
    if (!shortName) {
      setDraft((d) => (d ? { ...d, secret_short_name: null } : d));
      setTestResult(null);
      return;
    }
    const meta = info.available_secrets.find((s) => s.short_name === shortName);
    if (!meta) {
      // 未知 short_name（不在白名单）— 保守只赋值，不自动填其他字段
      setDraft((d) => (d ? { ...d, secret_short_name: shortName } : d));
      setTestResult(null);
      return;
    }
    setDraft((d) =>
      d
        ? {
            ...d,
            secret_short_name: shortName,
            // 自动应用建议（如果用户之前没自定义）
            provider: meta.suggested_provider || d.provider,
            auth_token_var: meta.suggested_token_var || d.auth_token_var,
            base_url:
              meta.suggested_base_url ?? d.base_url ?? null,
          }
        : d,
    );
    setTestResult(null);
  };

  return (
    <div className="space-y-3 rounded-md border border-border/60 bg-card p-4">
      <div className="flex items-center justify-between">
        <div>
          <h4 className="text-sm font-semibold">Endpoint 配置</h4>
          <p className="mt-0.5 text-[10.5px] text-muted-foreground">
            {agent.id === "claude-code" && (
              <>
                写入 <code className="font-mono">~/.claude/settings.json</code> 顶层 <code className="font-mono">apiKeyHelper</code> 字段 ·{" "}
                <strong className="text-emerald-700 dark:text-emerald-400">磁盘上完全无 token 明文</strong>
              </>
            )}
            {agent.id === "codex" && (
              <>
                写入 <code className="font-mono">~/.codex/config.toml</code> 的 <code className="font-mono">[model_providers.homebase-sops.auth]</code> 段 ·{" "}
                <strong className="text-emerald-700 dark:text-emerald-400">Codex 启动时自己 spawn helper 解密</strong>
              </>
            )}
            {(agent.id === "gemini-cli" ||
              agent.id === "openclaw" ||
              agent.id === "hermes") && (
              <>
                通过 <code className="font-mono">agent-env.ps1</code> wrapper 注入 env ·{" "}
                <strong className="text-emerald-700 dark:text-emerald-400">进程级 env，子进程退出销毁</strong>
              </>
            )}
          </p>
        </div>
        {savedFlash && (
          <span className="inline-flex items-center gap-1 text-[11px] text-emerald-600 dark:text-emerald-400">
            <CheckCircle2 className="h-3 w-3" />
            已保存
          </span>
        )}
      </div>

      {/* Provider 选择 */}
      <div className="space-y-1.5">
        <label className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          Provider
        </label>
        <div className="grid grid-cols-3 gap-1.5">
          {PROVIDER_OPTIONS.map((opt) => {
            const active = draft.provider === opt.k;
            return (
              <button
                key={opt.k}
                type="button"
                onClick={() => switchProvider(opt.k)}
                className={cn(
                  "rounded border px-2.5 py-1.5 text-left text-[11px] transition-colors",
                  active
                    ? "border-primary-500 bg-primary-50/40 text-foreground dark:bg-primary-950/40"
                    : "border-border bg-card hover:border-primary-400 hover:bg-muted/40",
                )}
              >
                <div className="font-medium">{opt.label}</div>
                <div className="mt-0.5 line-clamp-2 text-[10px] text-muted-foreground">
                  {opt.hint}
                </div>
              </button>
            );
          })}
        </div>
      </div>

      {/* Base URL */}
      <div className="space-y-1.5">
        <label className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          Base URL
          <span className="ml-1 font-normal normal-case text-muted-foreground/70">
            （写入 <code className="font-mono">ANTHROPIC_BASE_URL</code>）
          </span>
        </label>
        <input
          type="text"
          value={draft.base_url ?? ""}
          onChange={(e) =>
            setDraft((d) =>
              d ? { ...d, base_url: e.target.value || null } : d,
            )
          }
          placeholder={
            isAnthropic
              ? "默认 https://api.anthropic.com（可留空）"
              : "https://openrouter.ai/api/v1"
          }
          className="w-full rounded border border-border bg-background px-2.5 py-1.5 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
        />
      </div>

      {/* Auth Token via SOPS — Phase F：dropdown 引用 SOPS secret，永不展示明文 */}
      <div className="space-y-1.5">
        <label className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          Auth Token Secret
          <span className="ml-1 font-normal normal-case text-muted-foreground/70">
            （SOPS 引用 → 写入 <code className="font-mono">{draft.auth_token_var || "ANTHROPIC_AUTH_TOKEN"}</code>）
          </span>
        </label>
        <select
          value={draft.secret_short_name ?? ""}
          onChange={(e) => switchSecret(e.target.value)}
          className="w-full rounded border border-border bg-background px-2.5 py-1.5 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
        >
          <option value="">— 未选择 —</option>
          {info.available_secrets.map((s) => (
            <option key={s.short_name} value={s.short_name} disabled={!s.present}>
              {s.display_label} · {s.short_name}
              {!s.present ? "（未导入 SOPS）" : ""}
            </option>
          ))}
        </select>
        <p className="text-[10px] text-muted-foreground">
          只能引用 <code className="font-mono">C:\HomeBase\Secrets\secrets.yaml</code> 中已加密的 key；
          UI 永不接收 / 展示明文。
          {info.available_secrets.some((s) => !s.present) && (
            <>
              {" "}缺值的 key 请到 Secrets 面板用 <code className="font-mono">secrets_set</code> 导入。
            </>
          )}
        </p>
      </div>

      {/* External token 警告：settings.json 里的 token 不在 SOPS 里 */}
      {info.external_token_detected && (
        <div className="flex items-start gap-2 rounded-md border border-amber-300 bg-amber-50/50 px-3 py-2 text-[11px] text-amber-900 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-200">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <div className="min-w-0 flex-1">
            <p className="font-medium">检测到外部 token（未通过 SOPS）</p>
            <p className="mt-0.5 leading-relaxed">
              <code className="font-mono">~/.claude/settings.json</code> 的 env 块里有
              <code className="ml-1 font-mono">{info.current_token_masked}</code>，
              但 SOPS 中找不到匹配。建议：把该 key 用 <code className="font-mono">secrets_set</code> 导入 SOPS，
              再回这里下拉选择并保存，让本机所有 agent 走单源管理。
            </p>
          </div>
        </div>
      )}

      {/* SOPS 命中提示 */}
      {!info.external_token_detected &&
        info.current.secret_short_name &&
        info.current_token_masked && (
          <div className="flex items-center gap-2 rounded-md border border-emerald-300 bg-emerald-50/40 px-3 py-1.5 text-[11px] text-emerald-900 dark:border-emerald-800 dark:bg-emerald-950/30 dark:text-emerald-200">
            <CheckCircle2 className="h-3 w-3 shrink-0" />
            <span>
              当前 endpoint 由 SOPS secret <code className="font-mono">{info.current.secret_short_name}</code> 提供 ·{" "}
              <span className="opacity-70">{info.current_token_masked}</span>
            </span>
          </div>
        )}

      {/* Model alias 映射（仅 Claude Code + OpenRouter/custom 显示 — Codex/Gemini 无此概念） */}
      {agent.id === "claude-code" && (isOpenRouter || draft.provider === "custom") && (
        <div className="space-y-2 border-t border-border/60 pt-3">
          <div>
            <label className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
              Alias → Model ID 映射
            </label>
            <p className="mt-0.5 text-[10px] text-muted-foreground">
              Claude Code 选 <code className="font-mono">/model opus</code> 等 alias 时，实际请求哪个具体模型。
            </p>
          </div>
          {(["opus", "sonnet", "haiku"] as const).map((tier) => (
            <ModelTierPicker
              key={tier}
              tier={tier}
              value={draft[`default_${tier}` as const]}
              presets={presetsByTier[tier]}
              isOpenRouter={isOpenRouter}
              onChange={(v) =>
                setDraft((d) =>
                  d ? { ...d, [`default_${tier}`]: v } : d,
                )
              }
            />
          ))}
        </div>
      )}

      {/* 操作按钮 */}
      <div className="flex items-center gap-2 border-t border-border/60 pt-3">
        <button
          type="button"
          disabled={setMutation.isPending || !dirty}
          onClick={() => draft && setMutation.mutate(draft)}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[11px] font-medium transition-colors",
            dirty
              ? "bg-primary-600 text-white hover:bg-primary-700"
              : "cursor-not-allowed bg-muted text-muted-foreground",
          )}
        >
          {setMutation.isPending ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Save className="h-3 w-3" />
          )}
          {setMutation.isPending ? "保存中…" : dirty ? "保存" : "未修改"}
        </button>
        <button
          type="button"
          disabled={
            testMutation.isPending ||
            !draft.base_url ||
            !draft.secret_short_name
          }
          onClick={() => draft && testMutation.mutate(draft)}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-[11px] font-medium transition-colors",
            !draft.base_url || !draft.secret_short_name
              ? "cursor-not-allowed border-border bg-muted text-muted-foreground"
              : "border-primary-300 bg-primary-50/40 text-primary-800 hover:border-primary-500 dark:border-primary-800 dark:bg-primary-950/40 dark:text-primary-200",
          )}
          title={
            !draft.base_url
              ? "需要先填 base_url"
              : !draft.secret_short_name
                ? "需要先选一个 SOPS secret"
                : "从SOPS 解密 token 后发一条 max_tokens=1 的最小请求探测连通"
          }
        >
          {testMutation.isPending ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <RefreshCw className="h-3 w-3" />
          )}
          {testMutation.isPending ? "测试中…" : "测试连通"}
        </button>
        {dirty && (
          <button
            type="button"
            onClick={() => {
              setDraft(info.current);
              setTestResult(null);
            }}
            className="inline-flex items-center gap-1 px-2 text-[10.5px] text-muted-foreground hover:text-foreground"
          >
            重置
          </button>
        )}
      </div>

      {/* 测试结果 */}
      {testResult && (
        <div
          className={cn(
            "rounded border px-3 py-2 text-[11px]",
            testResult.ok
              ? "border-emerald-300 bg-emerald-50/50 text-emerald-900 dark:border-emerald-800 dark:bg-emerald-950/30 dark:text-emerald-200"
              : "border-destructive/40 bg-destructive/10 text-destructive-strong",
          )}
        >
          <div className="flex items-center gap-2">
            {testResult.ok ? (
              <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
            ) : (
              <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
            )}
            <span className="font-medium">
              {testResult.ok ? "连通正常" : "连通失败"}
            </span>
            {testResult.status_code !== null && (
              <code className="font-mono text-[10px]">
                HTTP {testResult.status_code}
              </code>
            )}
            <span className="ml-auto font-mono text-[10px] opacity-70">
              {testResult.latency_ms} ms
            </span>
          </div>
          {testResult.error && (
            <p className="mt-1 font-mono text-[10px]">{testResult.error}</p>
          )}
          {testResult.body_preview && (
            <details className="mt-1.5">
              <summary className="cursor-pointer text-[10px] opacity-70 hover:opacity-100">
                响应预览（前 400 字）
              </summary>
              <pre className="mt-1 max-h-32 overflow-auto rounded border border-current/20 bg-background/50 p-1.5 font-mono text-[10px] leading-relaxed">
                {testResult.body_preview}
              </pre>
            </details>
          )}
        </div>
      )}

      {setMutation.isError && (
        <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>保存失败：{String(setMutation.error)}</span>
        </div>
      )}

      {/* 高级 fallback：保留 PowerShell 复制式向导 */}
      <EndpointConfigHint />
    </div>
  );
}

function ModelTierPicker({
  tier,
  value,
  presets,
  isOpenRouter,
  onChange,
}: {
  tier: "opus" | "sonnet" | "haiku";
  value: string | null;
  presets: ModelIdEntry[];
  isOpenRouter: boolean;
  onChange: (v: string | null) => void;
}) {
  const tierLabel = { opus: "Opus", sonnet: "Sonnet", haiku: "Haiku" }[tier];
  return (
    <div className="space-y-1">
      <div className="flex items-baseline gap-2">
        <span className="font-mono text-[10.5px] font-semibold uppercase text-muted-foreground">
          {tierLabel}
        </span>
        <span className="text-[10px] text-muted-foreground/70">
          → ANTHROPIC_DEFAULT_{tier.toUpperCase()}_MODEL
        </span>
      </div>
      {isOpenRouter && presets.length > 0 ? (
        <select
          value={value ?? ""}
          onChange={(e) => onChange(e.target.value || null)}
          className="w-full rounded border border-border bg-background px-2 py-1.5 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
        >
          <option value="">— 不映射（用 Claude Code 默认）—</option>
          {presets.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label} · {m.context} · {m.pricing} · {m.id}
            </option>
          ))}
        </select>
      ) : (
        <input
          type="text"
          value={value ?? ""}
          onChange={(e) => onChange(e.target.value || null)}
          placeholder={`provider/${tier}-model-id`}
          className="w-full rounded border border-border bg-background px-2 py-1.5 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------------
// 端点 / 第三方 provider 配置向导（PowerShell 复制式 fallback · 保留）
// ---------------------------------------------------------------------

type EndpointPreset = "anthropic" | "openrouter" | "custom";

function EndpointConfigHint() {
  const [open, setOpen] = useState(false);
  const [preset, setPreset] = useState<EndpointPreset>("anthropic");
  const [copied, setCopied] = useState(false);

  const snippet = endpointSnippet(preset);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(snippet);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (e) {
      console.error("复制失败：", e);
    }
  };

  return (
    <div className="rounded-md border border-border/60 bg-card">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center justify-between px-3 py-2 text-[11px] text-muted-foreground hover:bg-muted/40"
      >
        <span className="inline-flex items-center gap-1.5">
          <Sparkles className="h-3 w-3" />
          端点 / 第三方 provider（OpenRouter 等） · 高级
        </span>
        <ChevronRight
          className={cn("h-3 w-3 transition-transform", open && "rotate-90")}
        />
      </button>

      {open && (
        <div className="space-y-3 border-t border-border/60 p-3">
          <p className="text-[11px] text-muted-foreground">
            Claude Code 支持通过 <code className="font-mono">ANTHROPIC_BASE_URL</code> 把请求路由到 OpenRouter 或自建 gateway。
            这通过 <strong>环境变量</strong>控制，不在 settings.json 的 model 字段里。
            下面是配置片段，复制到 shell profile（PowerShell <code className="font-mono">$PROFILE</code> / bash <code className="font-mono">~/.bashrc</code>）。
          </p>

          <div className="flex items-center gap-1">
            {(
              [
                { k: "anthropic", label: "Anthropic 官方" },
                { k: "openrouter", label: "OpenRouter" },
                { k: "custom", label: "自定义 Gateway" },
              ] as const
            ).map((opt) => (
              <button
                key={opt.k}
                type="button"
                onClick={() => setPreset(opt.k)}
                className={cn(
                  "rounded px-2 py-1 text-[10.5px] font-medium transition-colors",
                  preset === opt.k
                    ? "bg-primary-100 text-primary-800 dark:bg-primary-900/40 dark:text-primary-200"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
              >
                {opt.label}
              </button>
            ))}
          </div>

          <pre className="max-h-48 overflow-auto rounded border border-border/60 bg-muted/40 p-2 font-mono text-[10.5px] leading-relaxed text-foreground">
            {snippet}
          </pre>

          <div className="flex items-center justify-between gap-2">
            <button
              type="button"
              onClick={copy}
              className={cn(
                "inline-flex items-center gap-1 rounded-md px-2.5 py-1 text-[11px] font-medium transition-colors",
                copied
                  ? "bg-emerald-600 text-white"
                  : "bg-primary-600 text-white hover:bg-primary-700",
              )}
            >
              {copied ? (
                <CheckCircle2 className="h-3 w-3" />
              ) : (
                <Copy className="h-3 w-3" />
              )}
              {copied ? "已复制" : "复制片段"}
            </button>
            <span className="text-[10px] text-muted-foreground">
              设完重启终端 · 在 Claude Code 内跑 <code className="font-mono">/status</code> 校验
            </span>
          </div>

          {preset === "openrouter" && (
            <div className="rounded border border-dashed border-primary-400/40 bg-primary-50/30 p-2 text-[10.5px] leading-relaxed text-muted-foreground dark:bg-primary-950/20">
              <strong className="text-foreground/80">OpenRouter 优势</strong>：单一 key 访问 Claude + GPT + Gemini + Kimi
              等 400+ 模型，自动故障转移、统一计费、额度可控。适合日常主力使用。
              获取 API key：<code className="font-mono">openrouter.ai/settings/keys</code>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function endpointSnippet(preset: EndpointPreset): string {
  if (preset === "anthropic") {
    return `# Anthropic 官方（默认，通常无需配置）
# Claude Code 会从 keychain 读登录凭据。
# 如要覆盖 ENV：
# $env:ANTHROPIC_API_KEY = "sk-ant-..."`;
  }
  if (preset === "openrouter") {
    return `# OpenRouter · 代理所有 Anthropic / OpenAI / Gemini / Kimi 等
# PowerShell（加到 $PROFILE）：
$env:ANTHROPIC_BASE_URL  = "https://openrouter.ai/api"
$env:ANTHROPIC_AUTH_TOKEN = "<你的 OpenRouter key>"
$env:ANTHROPIC_API_KEY   = ""   # 必须显式置空，防止冲突

# 可选：把 alias 映射到具体 model（OpenRouter 格式 provider/model）
$env:ANTHROPIC_DEFAULT_OPUS_MODEL   = "anthropic/claude-opus-4.7"
$env:ANTHROPIC_DEFAULT_SONNET_MODEL = "anthropic/claude-sonnet-4.6"
$env:ANTHROPIC_DEFAULT_HAIKU_MODEL  = "google/gemini-2.5-flash"

# 或完全跨 provider（用 Claude Code 驱动任何模型）：
# $env:ANTHROPIC_DEFAULT_SONNET_MODEL = "moonshotai/kimi-k2"
# $env:ANTHROPIC_DEFAULT_OPUS_MODEL   = "openai/gpt-5"`;
  }
  // custom
  return `# 自定义 LLM Gateway（Cloudflare AI Gateway / LiteLLM / 企业 proxy）
$env:ANTHROPIC_BASE_URL   = "https://<your-gateway>/v1"
$env:ANTHROPIC_AUTH_TOKEN = "<gateway key>"
$env:ANTHROPIC_API_KEY    = ""

# 可选：自定义 model 选项（在 /model picker 里显示）
$env:ANTHROPIC_CUSTOM_MODEL_OPTION             = "my-gateway/opus-4.7"
$env:ANTHROPIC_CUSTOM_MODEL_OPTION_NAME        = "Opus via Gateway"
$env:ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION = "Custom deployment"`;
}

// ---------------------------------------------------------------------
// HistoryTab — Phase D · 2026-05-04
// ---------------------------------------------------------------------
//
// Claude Code / Codex 历史对话抽屉：
//   - master-detail 结构：列表 → 点击 session → 详情
//   - 列表按 cwd 分组（Claude）或扁平（Codex）
//   - 搜索按 first_user_prompt / cwd 过滤
//   - 详情渲染消息流（user/assistant/system 气泡）+ 顶部 stats

function HistoryTab({ agent }: { agent: AgentInfo }) {
  const queryClient = useQueryClient();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const listQuery = useQuery({
    queryKey: ["agents", "history", "list", agent.id],
    queryFn: () => agentsApi.historyList(agent.id),
    refetchOnWindowFocus: false,
  });

  if (listQuery.isLoading) {
    return (
      <div className="flex items-center gap-2 p-6 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        扫描历史对话…
      </div>
    );
  }
  if (listQuery.isError) {
    return (
      <div className="m-5 flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
        <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
        <span>读取失败：{String(listQuery.error)}</span>
      </div>
    );
  }

  const data = listQuery.data;
  if (!data) return null;

  if (!data.supported) {
    return (
      <div className="space-y-3 p-5">
        <div className="flex items-start gap-2 rounded-md border border-border/60 bg-muted/20 p-4">
          <Info className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
          <div>
            <p className="text-sm font-medium">此 agent 暂不支持历史对话读取</p>
            <p className="mt-1 text-xs text-muted-foreground">
              {data.unsupported_reason ?? "未知原因"}
            </p>
          </div>
        </div>
      </div>
    );
  }

  if (selectedId) {
    return (
      <HistoryDetailView
        agentId={agent.id}
        sessionId={selectedId}
        onBack={() => setSelectedId(null)}
      />
    );
  }

  return (
    <HistoryListView
      entries={data.entries}
      dataRoot={data.data_root}
      onSelect={setSelectedId}
      onRefresh={() => {
        void queryClient.invalidateQueries({ queryKey: ["agents", "history", "read", agent.id], refetchType: "none" });
        void listQuery.refetch();
      }}
      isRefreshing={listQuery.isFetching}
    />
  );
}

function HistoryListView({
  entries,
  dataRoot,
  onSelect,
  onRefresh,
  isRefreshing,
}: {
  entries: HistoryEntry[];
  dataRoot: string;
  onSelect: (sessionId: string) => void;
  onRefresh: () => void;
  isRefreshing: boolean;
}) {
  const [search, setSearch] = useState("");

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter((e) => {
      const hay = [e.first_user_prompt ?? "", e.cwd ?? "", e.session_id]
        .join(" ")
        .toLowerCase();
      return hay.includes(q);
    });
  }, [entries, search]);

  // 按 cwd 分组（Codex 都会落入 "(无 cwd)"）
  const grouped = useMemo(() => {
    const map = new Map<string, HistoryEntry[]>();
    for (const e of filtered) {
      const key = e.cwd ?? "(无 cwd)";
      const arr = map.get(key) ?? [];
      arr.push(e);
      map.set(key, arr);
    }
    // 组内按 last_active_at 倒序；组间按组内最新时间倒序
    return Array.from(map.entries())
      .map(([cwd, items]) => ({
        cwd,
        items: [...items].sort((a, b) =>
          b.last_active_at.localeCompare(a.last_active_at),
        ),
      }))
      .sort((a, b) =>
        (b.items[0]?.last_active_at ?? "").localeCompare(
          a.items[0]?.last_active_at ?? "",
        ),
      );
  }, [filtered]);

  return (
    <div className="flex h-full flex-col">
      {/* 顶栏 */}
      <div className="border-b border-border/60 p-3">
        <div className="mb-2 flex items-center justify-between gap-2">
          <div className="min-w-0">
            <div className="flex items-center gap-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
              <MessageSquare className="h-3 w-3" />
              历史对话 · {entries.length} 个会话
            </div>
            <div
              className="mt-0.5 truncate font-mono text-[10px] text-muted-foreground"
              title={dataRoot}
            >
              {dataRoot}
            </div>
          </div>
          <button
            type="button"
            onClick={onRefresh}
            disabled={isRefreshing}
            className="inline-flex items-center gap-1 rounded border border-border bg-card px-2 py-1 text-[10px] text-muted-foreground hover:text-foreground"
          >
            {isRefreshing ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <RefreshCw className="h-3 w-3" />
            )}
            刷新
          </button>
        </div>
        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="搜索 prompt / cwd / session_id"
          className="w-full rounded border border-border bg-background px-2.5 py-1.5 text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
        />
      </div>

      {/* 列表 */}
      {entries.length === 0 ? (
        <div className="flex flex-1 items-center justify-center p-8 text-center">
          <div>
            <p className="text-sm text-muted-foreground">无历史记录</p>
            <p className="mt-1 font-mono text-[10px] text-muted-foreground/70">
              在该 agent 里跑过会话后，这里会自动出现
            </p>
          </div>
        </div>
      ) : filtered.length === 0 ? (
        <div className="flex flex-1 items-center justify-center p-8 text-center text-xs text-muted-foreground">
          无匹配 &quot;{search}&quot;
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto">
          {grouped.map((g) => (
            <div key={g.cwd} className="border-b border-border/60 last:border-0">
              <div className="sticky top-0 z-10 border-b border-border/60 bg-muted/60 px-3 py-1.5 backdrop-blur">
                <div className="flex items-center gap-1.5 text-[10.5px] font-medium">
                  <FileText className="h-3 w-3 text-muted-foreground" />
                  <code className="truncate font-mono">{g.cwd}</code>
                  <span className="ml-auto font-mono text-[10px] text-muted-foreground">
                    {g.items.length}
                  </span>
                </div>
              </div>
              <ul className="divide-y divide-border/60">
                {g.items.map((e) => (
                  <li key={e.session_id}>
                    <button
                      type="button"
                      onClick={() => onSelect(e.session_id)}
                      className="flex w-full items-start gap-2 px-3 py-2 text-left transition-colors hover:bg-muted/40"
                    >
                      <MessageSquare className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                      <div className="min-w-0 flex-1">
                        <p className="line-clamp-2 text-[12px] leading-snug">
                          {e.first_user_prompt ?? (
                            <span className="italic text-muted-foreground">
                              （无 user 消息）
                            </span>
                          )}
                        </p>
                        <div className="mt-1 flex items-center gap-2 font-mono text-[10px] text-muted-foreground">
                          <span>{formatRelativeIso(e.last_active_at)}</span>
                          <span>·</span>
                          <span>{e.message_count} 条</span>
                          <code className="ml-auto truncate text-[9.5px] opacity-70">
                            {e.session_id.slice(0, 8)}
                          </code>
                        </div>
                      </div>
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function HistoryDetailView({
  agentId,
  sessionId,
  onBack,
}: {
  agentId: string;
  sessionId: string;
  onBack: () => void;
}) {
  const detailQuery = useQuery({
    queryKey: ["agents", "history", "read", agentId, sessionId],
    queryFn: () => agentsApi.historyRead(agentId, sessionId),
    refetchOnWindowFocus: false,
  });

  if (detailQuery.isLoading) {
    return (
      <div className="flex items-center gap-2 p-6 text-xs text-muted-foreground">
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
        解析会话…
      </div>
    );
  }

  if (detailQuery.isError || !detailQuery.data) {
    return (
      <div className="space-y-3 p-5">
        <button
          type="button"
          onClick={onBack}
          className="inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
        >
          ← 返回列表
        </button>
        <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>读取失败：{String(detailQuery.error ?? "未知错误")}</span>
        </div>
      </div>
    );
  }

  const s: HistorySession = detailQuery.data;
  const tokenTotal =
    s.stats.total_input_tokens + s.stats.total_output_tokens;

  return (
    <div className="flex h-full flex-col">
      {/* 顶栏 */}
      <div className="border-b border-border/60 p-3">
        <button
          type="button"
          onClick={onBack}
          className="mb-2 inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
        >
          ← 返回列表
        </button>
        <div className="space-y-0.5 text-[11px]">
          {s.cwd && (
            <div className="truncate font-mono text-muted-foreground">
              cwd: {s.cwd}
            </div>
          )}
          <div className="flex flex-wrap items-center gap-2 font-mono text-[10px] text-muted-foreground">
            <code>{sessionId.slice(0, 8)}</code>
            {s.git_branch && <span>· branch: {s.git_branch}</span>}
            {s.version && <span>· v{s.version}</span>}
            <span>· {s.messages.length} 条消息</span>
            {tokenTotal > 0 && (
              <span>
                · {s.stats.total_input_tokens}↓/{s.stats.total_output_tokens}↑ tok
              </span>
            )}
            {s.stats.duration_seconds > 0 && (
              <span>· {formatDuration(s.stats.duration_seconds)}</span>
            )}
          </div>
        </div>
      </div>

      {/* 消息流 */}
      <div className="flex-1 overflow-y-auto px-3 py-2">
        {s.messages.length === 0 ? (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            无消息
          </div>
        ) : (
          <ul className="space-y-2">
            {s.messages.map((m) => (
              <MessageBubble key={m.uuid} msg={m} />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function MessageBubble({ msg }: { msg: HistoryMessage }) {
  const isUser = msg.role === "user";
  const isAssistant = msg.role === "assistant";
  const isSystem = msg.role === "system";

  return (
    <li
      className={cn(
        "rounded-md border px-3 py-2 text-[12px] leading-relaxed",
        isUser &&
          "border-primary-200 bg-primary-50/40 dark:border-primary-800/60 dark:bg-primary-950/20",
        isAssistant && "border-border bg-card",
        isSystem && "border-dashed border-border/60 bg-muted/10 text-muted-foreground",
      )}
    >
      <div className="mb-1 flex items-center gap-1.5 text-[10px]">
        <span
          className={cn(
            "rounded px-1.5 py-0.5 font-mono uppercase",
            isUser && "bg-primary-100 text-primary-800 dark:bg-primary-900/40 dark:text-primary-200",
            isAssistant && "bg-muted text-foreground",
            isSystem && "bg-transparent text-muted-foreground",
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
    const now = Date.now();
    const diff = now - d.getTime();
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

// ---------------------------------------------------------------------
// 占位 Tab — 进程资源（Phase 3）
// ---------------------------------------------------------------------

function ComingSoonTab({
  phase,
  feature,
  hint,
}: {
  phase: string;
  feature: string;
  hint: string;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-10 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full bg-muted">
        <History className="h-5 w-5 text-muted-foreground" />
      </div>
      <h3 className="font-serif text-base font-semibold">
        {feature} <span className="ml-1 rounded bg-muted px-2 py-0.5 font-mono text-[10px] text-muted-foreground">{phase}</span>
      </h3>
      <p className="max-w-sm text-xs leading-relaxed text-muted-foreground">
        {hint}
      </p>
      <p className="font-mono text-[10px] text-muted-foreground/60">
        下一轮接入 · 已规划路径
      </p>
    </div>
  );
}
