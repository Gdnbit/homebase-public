import { clsx, type ClassValue } from "clsx";
import { twMerge } from "tailwind-merge";

/** shadcn/ui 标准 cn 助手 */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

/**
 * 相对时间格式化（用于 "上次更新" 提示）
 * - <60s: "刚刚"
 * - <60min: "N 分钟前"
 * - <24h: "N 小时前"
 * - 否则: yyyy-mm-dd HH:mm
 */
export function formatRelativeTime(timestamp: number): string {
  if (
    !Number.isFinite(timestamp) ||
    !Number.isFinite(new Date(timestamp).getTime())
  )
    return "时间未知";
  const diffMs = Date.now() - timestamp;
  const diffSec = Math.floor(diffMs / 1000);
  if (diffSec < 5) return "刚刚";
  if (diffSec < 60) return `${diffSec} 秒前`;
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin} 分钟前`;
  const diffHour = Math.floor(diffMin / 60);
  if (diffHour < 24) return `${diffHour} 小时前`;
  const d = new Date(timestamp);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Missing or invalid timestamps are unknown, never the current time. */
export function formatLocalDateTime(value: unknown): string {
  const timestamp =
    typeof value === "number"
      ? value
      : typeof value === "string" && value.trim()
        ? Date.parse(value)
        : NaN;
  const date = new Date(timestamp);
  return Number.isFinite(date.getTime()) ? date.toLocaleString() : "时间未知";
}
