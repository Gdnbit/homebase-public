/**
 * Agent Versions Section · S5 · 2026-05-06
 *
 * 一次检查 5 agent 版本（带 1h 缓存）· 显示当前 vs 最新 + 更新提示。
 * 不阻断 Agents 页面其他功能 · 单独 query · 失败也不显示红框。
 *
 * 给 USER 的更新方式：
 * - 复制 update_command 到剪贴板（避免我们直接 spawn npm i -g · 防破坏）
 * - hermes (USER fork) 显示"手动 git pull"提示
 */

import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  AlertCircle,
  Check,
  CheckCircle2,
  Copy,
  Loader2,
  Package,
  RefreshCw,
  type LucideIcon,
} from "lucide-react";
import { Claude } from "@/lib/lobehub-icons";
import Codex from "@lobehub/icons/es/Codex/components/Mono";
import OpenClaw from "@lobehub/icons/es/OpenClaw/components/Mono";
import GeminiColor from "@lobehub/icons/es/Gemini/components/Color";
import { agentVersionsApi, type AgentVersionInfo } from "@/lib/tauri-api";
import { cn } from "@/lib/utils";

const AGENT_DISPLAY: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex CLI",
  "gemini-cli": "Gemini CLI",
  openclaw: "OpenClaw",
  hermes: "Hermes",
};

/** 渲染 agent 品牌图标 · hermes 回退 lucide Package */
function AgentIcon({ agentId, className }: { agentId: string; className?: string }) {
  const cls = className ?? "h-4 w-4 shrink-0";
  switch (agentId) {
    case "claude-code":
      return <Claude size={16} className={cls} />;
    case "codex":
      return <Codex size={16} className={cls} />;
    case "gemini-cli":
      return <GeminiColor size={16} className={cls} />;
    case "openclaw":
      return <OpenClaw size={16} className={cls} />;
    default: {
      const Fallback: LucideIcon = Package;
      return <Fallback className={cn(cls, "text-muted-foreground")} />;
    }
  }
}

export function AgentVersionsSection() {
  const versionsQuery = useQuery({
    queryKey: ["agent-versions"],
    queryFn: () => agentVersionsApi.check(),
    refetchOnWindowFocus: false,
    // 1h 缓存（跟后端缓存一致 · 减少 npm 调用）
    staleTime: 60 * 60 * 1000,
  });

  return (
    <section aria-label="Agent 版本">
      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="font-serif text-lg font-semibold">Agent 版本</h2>
        <button
          type="button"
          onClick={() => versionsQuery.refetch()}
          disabled={versionsQuery.isFetching}
          title="重新检查"
          className="text-muted-foreground hover:text-foreground disabled:opacity-50"
        >
          <RefreshCw className={cn("h-3.5 w-3.5", versionsQuery.isFetching && "animate-spin")} />
        </button>
      </div>

      {versionsQuery.isLoading && (
        <div className="flex items-center gap-2 rounded-lg border border-border bg-card p-4 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          检查中...
        </div>
      )}

      {versionsQuery.isError && (
        <div className="flex items-start gap-2 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 text-xs text-amber-700 dark:text-amber-400">
          <AlertCircle className="h-3.5 w-3.5 shrink-0" />
          <div>版本检查失败：{String(versionsQuery.error)}</div>
        </div>
      )}

      {versionsQuery.data && (
        <ul className="space-y-2">
          {versionsQuery.data.map((v) => (
            <VersionRow key={v.agent_id} info={v} />
          ))}
        </ul>
      )}
    </section>
  );
}

function VersionRow({ info }: { info: AgentVersionInfo }) {
  const [copied, setCopied] = useState(false);

  const copyMut = useMutation({
    mutationFn: async (cmd: string) => {
      await navigator.clipboard.writeText(cmd);
    },
    onSuccess: () => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    },
  });

  const display = AGENT_DISPLAY[info.agent_id] ?? info.agent_id;

  // 状态徽章 · 中文化 · 只说用户关心的
  let badge: { label: string; tone: "success" | "warn" | "muted" | "info" } = {
    label: "未装",
    tone: "muted",
  };
  if (info.kind === "fork") {
    badge = { label: "本地版", tone: "info" };
  } else if (info.current && info.latest) {
    if (info.update_available) {
      badge = { label: "有新版本", tone: "warn" };
    } else {
      badge = { label: "最新版", tone: "success" };
    }
  } else if (info.current) {
    badge = { label: "已装", tone: "muted" };
  } else {
    badge = { label: "未装", tone: "muted" };
  }

  // standalone 的 update_command 是个注释提示 · 不当“可复制命令”处理
  const hasRunnableCommand =
    !!info.update_command && !info.update_command.trimStart().startsWith("#");

  const toneClass = {
    success: "text-green-600 dark:text-green-500 border-green-500/30 bg-green-500/5",
    warn: "text-amber-700 dark:text-amber-400 border-amber-500/30 bg-amber-500/5",
    muted: "text-muted-foreground border-border bg-card",
    info: "text-primary-700 dark:text-primary-300 border-primary-500/30 bg-primary-500/5",
  }[badge.tone];

  return (
    <li className="flex items-center gap-3 rounded-lg border border-border bg-card px-3 py-2">
      <AgentIcon agentId={info.agent_id} />

      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="text-sm font-medium">{display}</span>
          <span
            className={cn(
              "rounded-md border px-1.5 py-0.5 text-[10px]",
              toneClass,
            )}
          >
            {badge.label}
          </span>
        </div>
        {(info.current || info.latest || info.message) && (
          <div className="font-mono text-[10px] text-muted-foreground">
            {info.current && <span>当前 {info.current}</span>}
            {info.current && info.latest && <span> · </span>}
            {info.latest && <span>最新 {info.latest}</span>}
            {!info.latest && info.message && (
              <span title={info.message}>{info.message}</span>
            )}
          </div>
        )}
      </div>

      {info.update_available && hasRunnableCommand && (
        <button
          type="button"
          onClick={() => copyMut.mutate(info.update_command!)}
          title={`复制：${info.update_command}`}
          className="flex shrink-0 items-center gap-1 rounded-md border border-amber-500/30 bg-amber-500/5 px-2 py-1 text-[11px] font-medium text-amber-700 transition-colors hover:bg-amber-500/10 dark:text-amber-400"
        >
          {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
          {copied ? "已复制" : "复制更新命令"}
        </button>
      )}
      {info.update_available && !hasRunnableCommand && (
        <span
          className="shrink-0 rounded-md border border-amber-500/30 bg-amber-500/5 px-2 py-1 text-[11px] font-medium text-amber-700 dark:text-amber-400"
          title="请按官方方式手动重装"
        >
          手动更新
        </span>
      )}
      {!info.update_available && info.current && info.latest && (
        <CheckCircle2 className="h-4 w-4 shrink-0 text-green-600 dark:text-green-500" />
      )}
    </li>
  );
}
