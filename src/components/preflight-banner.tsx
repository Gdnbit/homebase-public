import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  AlertTriangle,
  ShieldAlert,
  CheckCircle2,
  RefreshCw,
} from "lucide-react";
import { systemApi, type PreflightCheck } from "@/lib/tauri-api";
import { cn } from "@/lib/utils";

/**
 * Phase G-8 · 启动自检横幅 — SOPS 五件套状态
 *
 * 渲染逻辑：
 * - 五件套全通过 → **不渲染**（零打扰）
 * - 有 critical 失败 → 红色横幅（destructive）列出失败项
 * - 仅 warning 失败 → 黄色横幅（warning）
 *
 * 临时超时有限次自动复检；永久错误与重试耗尽时保留人工复检。
 */
export function PreflightBanner() {
  const [automaticRetries, setAutomaticRetries] = useState(0);
  const {
    data,
    isLoading,
    isError,
    isFetching,
    refetch,
    dataUpdatedAt,
    errorUpdatedAt,
  } = useQuery({
    queryKey: ["system", "preflight"],
    queryFn: () => systemApi.preflight(),
    staleTime: Infinity, // 会话级缓存
    retry: false,
  });

  const retryable =
    isError || !!data?.some((check) => !check.ok && check.retryable);
  useEffect(() => {
    if (!retryable || isFetching || automaticRetries >= 2) return;
    const timer = setTimeout(() => {
      setAutomaticRetries((count) => count + 1);
      void refetch();
    }, 30_000);
    return () => clearTimeout(timer);
  }, [
    retryable,
    isFetching,
    automaticRetries,
    dataUpdatedAt,
    errorUpdatedAt,
    refetch,
  ]);

  if (isLoading)
    return (
      <p role="status" className="mb-4 text-xs text-muted-foreground">
        正在检查本机密钥工具，首次启动可能稍慢…
      </p>
    );
  const recoveryNotice =
    retryable && !isFetching ? (
      <p role="status" className="basis-full px-5 pb-3 text-xs">
        {automaticRetries < 2
          ? `临时检查失败，30 秒后自动复检（${automaticRetries + 1}/2）；也可立即重新检查。`
          : "自动复检次数已用完，错误仍保留；请查看原因，修复后重新检查。"}
      </p>
    ) : null;

  const retryButton = (
    <button
      type="button"
      onClick={() => {
        setAutomaticRetries(0);
        void refetch();
      }}
      disabled={isFetching}
      className="inline-flex shrink-0 items-center gap-2 rounded-md border px-3 py-1.5 text-xs disabled:opacity-50 focus-visible:outline focus-visible:outline-2"
    >
      <RefreshCw
        aria-hidden
        className={cn(
          "h-3.5 w-3.5",
          isFetching && "animate-spin motion-reduce:animate-none",
        )}
      />
      {isFetching ? "正在检查…" : "重新检查"}
    </button>
  );

  if (isError || !data) {
    return (
      <div
        role="alert"
        aria-busy={isFetching}
        className="mb-6 flex flex-wrap items-center justify-between gap-4 rounded-xl border border-warning/30 bg-warning/5 px-5 py-3 text-warning-strong"
      >
        <p className="text-sm">
          启动自检未完成，暂时无法确认密钥环境状态。请重新检查。
        </p>
        {retryButton}
        {recoveryNotice}
      </div>
    );
  }

  const failed = data.filter((c) => !c.ok);
  if (failed.length === 0) return null;

  const hasCritical = failed.some((c) => c.severity === "critical");
  const tone: "critical" | "warning" = hasCritical ? "critical" : "warning";

  return (
    <div
      role="alert"
      aria-busy={isFetching}
      className={cn(
        "mb-6 overflow-hidden rounded-xl border",
        tone === "critical"
          ? "border-destructive/30 bg-destructive/5"
          : "border-warning/30 bg-warning/5",
      )}
    >
      <div className="flex items-center justify-between gap-4 pr-5">
        <BannerHeader tone={tone} failedCount={failed.length} />
        {retryButton}
      </div>
      <ul className="divide-y divide-border/50">
        {failed.map((c) => (
          <PreflightItem key={c.id} check={c} />
        ))}
      </ul>
      {recoveryNotice}
    </div>
  );
}

function BannerHeader({
  tone,
  failedCount,
}: {
  tone: "critical" | "warning";
  failedCount: number;
}) {
  const Icon = tone === "critical" ? ShieldAlert : AlertTriangle;
  const title =
    tone === "critical" ? "密钥环境检查未通过" : "部分可选组件检查未通过";
  const subtitle =
    tone === "critical"
      ? `${failedCount} 项检查未通过，相关密钥功能可能受影响，请查看下方原因并重新检查。`
      : `${failedCount} 项可选检查未通过，相关功能可能受影响，请查看下方原因。`;

  return (
    <header
      className={cn(
        "flex items-center gap-3 px-5 py-3",
        tone === "critical" ? "text-destructive-strong" : "text-warning-strong",
      )}
    >
      <Icon className="h-5 w-5 shrink-0" aria-hidden />
      <div className="min-w-0 flex-1">
        <p className="font-serif text-sm font-semibold">{title}</p>
        <p className="mt-0.5 text-xs opacity-80">{subtitle}</p>
      </div>
    </header>
  );
}

function PreflightItem({ check }: { check: PreflightCheck }) {
  const isCritical = check.severity === "critical";
  return (
    <li className="flex items-start gap-3 px-5 py-3">
      <span
        className={cn(
          "mt-0.5 inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-full",
          isCritical
            ? "bg-destructive/15 text-destructive-strong"
            : "bg-warning/15 text-warning-strong",
        )}
        aria-hidden
      >
        <span className="text-[10px] font-bold">!</span>
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="font-medium text-sm text-foreground">
            {check.label}
          </span>
          <span
            className={cn(
              "rounded px-1.5 py-px font-mono text-[10px] uppercase tracking-wider",
              isCritical
                ? "bg-destructive/10 text-destructive-strong"
                : "bg-warning/10 text-warning-strong",
            )}
          >
            {check.severity}
          </span>
        </div>
        {check.hint && (
          <p className="mt-1 break-words text-xs text-muted-foreground">
            {check.hint}
          </p>
        )}
      </div>
    </li>
  );
}

/**
 * 辅助导出：让调用方能渲染一个"全通过"成功提示（可选，不在主 banner 逻辑内）
 * 目前未在 AppLayout 使用；预留给 Secrets 页或 System 页做状态展示。
 */
export function PreflightAllOkBadge() {
  const { data } = useQuery({
    queryKey: ["system", "preflight"],
    queryFn: () => systemApi.preflight(),
    staleTime: Infinity,
    retry: false,
  });
  if (!data) return null;
  const allOk = data.every((c) => c.ok);
  if (!allOk) return null;
  return (
    <span
      className="inline-flex items-center gap-1 rounded-md bg-success/10 px-2 py-0.5 text-xs text-success-strong"
      title="SOPS 五件套全部就绪"
    >
      <CheckCircle2 className="h-3 w-3" aria-hidden />
      密钥环境就绪
    </span>
  );
}
