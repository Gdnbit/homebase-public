/**
 * Shared MCP Pool Section · S2 · 2026-05-06
 *
 * 让 USER 一处配置 · 4 个官方 agent（claude-code / codex / gemini-cli / openclaw）
 * 共享同一池 MCP server。HomeBase 把 pool 写入各 agent 默认配置文件 ·
 * 严格只动 `homebase-` 前缀的 entry · USER 自加的 server 一字不动。
 *
 * UI 结构：
 * - 标题 + 解释
 * - Server 卡片列表（id / 名称 / 描述 / command）
 * - 分配矩阵（横轴 server · 纵轴 4 agent · checkbox）
 * - [Apply] 按钮 · 写盘 + 触发 reconcile · 显示 report
 */

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertCircle,
  CheckCircle2,
  Loader2,
  RefreshCw,
  Server,
  XCircle,
} from "lucide-react";
import {
  sharedMcpApi,
  type AgentReconcileResult,
  type SharedMcpPool,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";
import { refreshCapabilityCatalog } from "@/lib/capability-index";
import { InfoTooltip } from "@/components/info-tooltip";

const AGENT_DISPLAY: Record<string, { label: string; short: string }> = {
  "claude-code": { label: "Claude Code", short: "claude" },
  codex: { label: "Codex CLI", short: "codex" },
  "gemini-cli": { label: "Gemini CLI", short: "gemini" },
  openclaw: { label: "OpenClaw", short: "openclaw" },
};

export function SharedMcpSection() {
  const queryClient = useQueryClient();
  const poolQuery = useQuery({
    queryKey: ["shared-mcp", "pool"],
    queryFn: () => sharedMcpApi.get(),
    refetchOnWindowFocus: false,
  });

  // 本地 draft state · 编辑时不直接动 query data
  const [draft, setDraft] = useState<SharedMcpPool | null>(null);
  const pool = draft ?? poolQuery.data ?? null;
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(poolQuery.data);

  const [report, setReport] = useState<AgentReconcileResult[] | null>(null);

  const applyMut = useMutation({
    mutationFn: (p: SharedMcpPool) => sharedMcpApi.saveAndApply(p),
    onSuccess: (r) => {
      setReport(r.agents);
      setDraft(null);
      queryClient.invalidateQueries({ queryKey: ["shared-mcp", "pool"] });
      refreshCapabilityCatalog(queryClient);
    },
  });

  const reapplyMut = useMutation({
    mutationFn: () => sharedMcpApi.apply(),
    onSuccess: (r) => setReport(r.agents),
  });

  const toggleAssignment = (agentId: string, serverId: string) => {
    if (!pool) return;
    const current = pool.assignments[agentId] ?? [];
    const has = current.includes(serverId);
    const next = has
      ? current.filter((id) => id !== serverId)
      : [...current, serverId];
    setDraft({
      ...pool,
      assignments: { ...pool.assignments, [agentId]: next },
    });
  };

  return (
    <section aria-label="共享 MCP 池">
      <div className="mb-3 flex items-center gap-2">
        <h2 className="font-serif text-lg font-semibold">共享 MCP 池</h2>
        <InfoTooltip>
          一处配置 · 4 个 agent 共用。HomeBase 只会动带{" "}
          <code className="font-mono">homebase-</code> 前缀的 entry · 你自加的
          server 不会被碰。写入路径：<code className="font-mono">~/.claude/settings.json</code>、
          <code className="font-mono">~/.codex/config.toml</code>、
          <code className="font-mono">~/.gemini/settings.json</code>、
          <code className="font-mono">~/.openclaw/settings.json</code>。
        </InfoTooltip>
      </div>

      {poolQuery.isLoading && (
        <div className="flex items-center gap-2 rounded-lg border border-border bg-card p-4 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          加载共享池...
        </div>
      )}

      {poolQuery.isError && (
        <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
          <AlertCircle className="h-4 w-4 shrink-0" />
          <div>
            <p className="font-medium">加载失败</p>
            <p className="mt-0.5 text-xs">{String(poolQuery.error)}</p>
          </div>
        </div>
      )}

      {pool && pool.servers.length === 0 && (
        <div className="rounded-lg border border-dashed border-border bg-card p-6 text-center text-sm text-muted-foreground">
          <Server className="mx-auto mb-2 h-6 w-6 opacity-50" />
          <p>共享池为空</p>
        </div>
      )}

      {pool && pool.servers.length > 0 && (
        <>
          {/* 分配矩阵 */}
          <div className="overflow-x-auto rounded-lg border border-border bg-card">
            <table className="min-w-full text-sm">
              <thead className="border-b border-border bg-muted/30 text-xs uppercase tracking-wider text-muted-foreground">
                <tr>
                  <th className="px-3 py-2 text-left font-medium">MCP Server</th>
                  {Object.keys(AGENT_DISPLAY).map((aid) => (
                    <th key={aid} className="px-3 py-2 text-center font-medium">
                      {AGENT_DISPLAY[aid].label}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {pool.servers.map((server) => (
                  <tr key={server.id} className="border-t border-border">
                    <td className="px-3 py-3">
                      <div className="flex items-center gap-1.5 text-sm font-medium">
                        {server.name}
                        <InfoTooltip width={300}>
                          <div className="space-y-1">
                            <div className="font-mono text-[10px] text-muted-foreground">
                              id: {server.id}
                            </div>
                            {server.description && (
                              <div>{server.description}</div>
                            )}
                            <div className="truncate font-mono text-[10px] text-muted-foreground/80">
                              {server.command}
                            </div>
                          </div>
                        </InfoTooltip>
                      </div>
                    </td>
                    {Object.keys(AGENT_DISPLAY).map((aid) => {
                      const checked = (pool.assignments[aid] ?? []).includes(server.id);
                      return (
                        <td key={aid} className="px-3 py-3 text-center">
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => toggleAssignment(aid, server.id)}
                            className="h-4 w-4 cursor-pointer rounded border-border accent-primary-600"
                            aria-label={`${AGENT_DISPLAY[aid].label} 启用 ${server.id}`}
                          />
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {/* 操作按钮 */}
          <div className="mt-3 flex items-center gap-2">
            <button
              type="button"
              disabled={!dirty || applyMut.isPending}
              onClick={() => pool && applyMut.mutate(pool)}
              className={cn(
                "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
                dirty
                  ? "bg-primary-600 text-white hover:bg-primary-700 dark:bg-primary-500 dark:hover:bg-primary-600"
                  : "cursor-not-allowed bg-secondary text-muted-foreground",
              )}
            >
              {applyMut.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <CheckCircle2 className="h-3.5 w-3.5" />
              )}
              {dirty ? "保存并应用" : "无改动"}
            </button>
            <button
              type="button"
              disabled={reapplyMut.isPending}
              onClick={() => reapplyMut.mutate()}
              title="重新把当前配置推到 4 agent · 不改动分配矩阵"
              className="flex items-center gap-1.5 rounded-md border border-border px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
            >
              {reapplyMut.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <RefreshCw className="h-3.5 w-3.5" />
              )}
              重新推送
            </button>
            {dirty && (
              <button
                type="button"
                onClick={() => setDraft(null)}
                className="ml-auto text-xs text-muted-foreground hover:text-foreground"
              >
                放弃改动
              </button>
            )}
          </div>

          {/* 错误显示 */}
          {applyMut.isError && (
            <div className="mt-3 flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive">
              <AlertCircle className="h-3.5 w-3.5 shrink-0" />
              <div>
                <p className="font-medium">应用失败</p>
                <p className="mt-0.5">{String(applyMut.error)}</p>
              </div>
            </div>
          )}

          {/* Reconcile report */}
          {report && <ReconcileReportPanel report={report} onClose={() => setReport(null)} />}
        </>
      )}
    </section>
  );
}

function ReconcileReportPanel({
  report,
  onClose,
}: {
  report: AgentReconcileResult[];
  onClose: () => void;
}) {
  return (
    <div className="mt-3 rounded-lg border border-border bg-card p-3">
      <div className="mb-2 flex items-center justify-between">
        <h3 className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
          推送报告
        </h3>
        <button
          type="button"
          onClick={onClose}
          className="text-[10px] text-muted-foreground hover:text-foreground"
        >
          关闭
        </button>
      </div>
      <ul className="space-y-1.5">
        {report.map((r) => {
          const display = AGENT_DISPLAY[r.agent_id];
          const Icon =
            r.status === "applied"
              ? CheckCircle2
              : r.status === "skipped"
                ? AlertCircle
                : XCircle;
          const color =
            r.status === "applied"
              ? "text-green-600 dark:text-green-500"
              : r.status === "skipped"
                ? "text-amber-600 dark:text-amber-500"
                : "text-destructive";
          return (
            <li key={r.agent_id} className="flex items-start gap-2 text-xs">
              <Icon className={cn("h-3.5 w-3.5 shrink-0 mt-0.5", color)} />
              <div className="min-w-0 flex-1">
                <div className="flex items-baseline gap-2">
                  <span className="font-medium">{display?.label ?? r.agent_id}</span>
                  <span className={cn("font-mono uppercase text-[10px]", color)}>
                    {r.status}
                  </span>
                  {r.status === "applied" && (
                    <span className="text-muted-foreground">
                      {r.applied_count} server
                    </span>
                  )}
                </div>
                <div className="truncate font-mono text-[10px] text-muted-foreground">
                  {r.path}
                </div>
                {r.message && (
                  <div className="mt-0.5 text-[11px] text-muted-foreground">
                    {r.message}
                  </div>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
