import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface EmptyPlaceholderProps {
  /** 主标题（中文，简短） */
  title: string;
  /** 副标题/原因说明 */
  description?: string;
  /** Phase 注记，例如 "13.1 Day 5-7" — 显示为 mono small badge */
  phase?: string;
  /** 自定义图标（emoji 或 lucide）*/
  icon?: ReactNode;
  /** 行动按钮区 */
  action?: ReactNode;
  className?: string;
}

/**
 * 空状态占位卡片 — GDNbit 风格（虚线边框 + 居中 + 文艺感）
 *
 * 使用场景：列表数据为空 / 模块尚未实现 / 加载失败的兜底
 */
export function EmptyPlaceholder({
  title,
  description,
  phase,
  icon,
  action,
  className,
}: EmptyPlaceholderProps) {
  return (
    <div
      className={cn(
        "flex flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-border bg-card/50 px-6 py-10 text-center",
        className,
      )}
    >
      {icon && <div className="text-3xl opacity-60">{icon}</div>}
      <div className="space-y-1">
        <p className="font-serif text-base font-semibold tracking-tight text-foreground">
          {title}
        </p>
        {description && (
          <p className="max-w-md text-sm text-muted-foreground">{description}</p>
        )}
      </div>
      {phase && (
        <span className="rounded bg-muted px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
          Phase {phase}
        </span>
      )}
      {action && <div className="mt-1">{action}</div>}
    </div>
  );
}
