/**
 * InfoTooltip · 2026-05-07
 *
 * 通用 (i) 圆圈图标 · hover 显示详情。
 * 设计原则：界面默认只展示用户关心的信息 · 实现细节 / 技术说明 / 路径 / 频率
 * 这些专业内容统一收纳到这里 · 想看的人 hover 即可看见。
 *
 * 用法：
 * ```tsx
 * <h2>共享 MCP 池 <InfoTooltip>HomeBase 把池里的 server 写入各 agent 默认配置...</InfoTooltip></h2>
 * ```
 */

import { Info } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface InfoTooltipProps {
  children: ReactNode;
  /** 弹出框宽度 · 默认 240px */
  width?: number;
  /** 弹出位置 · 默认 right · 在窄屏自动改 */
  side?: "right" | "top" | "bottom";
  className?: string;
}

export function InfoTooltip({
  children,
  width = 280,
  side = "right",
  className,
}: InfoTooltipProps) {
  return (
    <span className={cn("group/info relative inline-flex align-middle", className)}>
      <button
        type="button"
        tabIndex={0}
        aria-label="详细说明"
        className="inline-flex h-3.5 w-3.5 items-center justify-center rounded-full text-muted-foreground/50 outline-none transition-colors hover:text-muted-foreground focus-visible:text-foreground focus-visible:ring-2 focus-visible:ring-primary-500/40"
      >
        <Info className="h-3.5 w-3.5" aria-hidden />
      </button>
      <span
        role="tooltip"
        style={{ width }}
        className={cn(
          // 默认隐藏 · group hover/focus-within 显示
          "pointer-events-none invisible absolute z-50 rounded-md border border-border bg-card p-2.5 text-[11px] leading-relaxed text-card-foreground shadow-lg opacity-0 transition-opacity",
          "group-hover/info:visible group-hover/info:opacity-100",
          "group-focus-within/info:visible group-focus-within/info:opacity-100",
          // 位置
          side === "right" &&
            "left-[calc(100%+8px)] top-1/2 -translate-y-1/2",
          side === "top" && "bottom-[calc(100%+8px)] left-1/2 -translate-x-1/2",
          side === "bottom" && "top-[calc(100%+8px)] left-1/2 -translate-x-1/2",
        )}
      >
        {children}
      </span>
    </span>
  );
}
