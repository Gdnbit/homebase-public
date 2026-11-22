import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Cpu,
  Route,
  Server,
  Wrench,
} from "lucide-react";
import { systemApi, type PathEntry, type ProcessInfo } from "@/lib/tauri-api";
import { EmptyPlaceholder } from "@/components/empty-placeholder";
import { GdnbitSettingsSection } from "@/components/gdnbit-settings-section";
import { cn, formatRelativeTime } from "@/lib/utils";

/**
 * 系统 — 磁盘 / PATH / 进程（v3 §6.6）
 *
 * Phase 13.1 Day 8 起：sysinfo crate + Get-PSDrive 接入
 */
export function System() {
  const {
    data: disks = [],
    isLoading,
    isError,
    error,
    refetch,
    dataUpdatedAt,
  } = useQuery({
    queryKey: ["system", "disks"],
    queryFn: () => systemApi.disks(),
    refetchInterval: 30_000, // 30s 轮询（磁盘容量变化慢，不需要更高频率）
  });

  return (
    <div className="animate-fade-in space-y-10">
      <header className="hb-page-header">
        <span className="hb-section-label">System</span>
        <h1>系统</h1>
        <p>
          磁盘容量 · 环境变量 PATH · 关键进程一览
          <span className="ml-2 font-mono text-xs text-muted-foreground">
            · 磁盘 30s 轮询
          </span>
        </p>
      </header>

      <section aria-label="磁盘容量">
        <div className="mb-3 flex items-baseline justify-between">
          <h2 className="font-serif text-lg font-semibold">磁盘容量</h2>
          <span className="font-mono text-xs text-muted-foreground">
            Get-PSDrive ·{" "}
            {dataUpdatedAt ? formatRelativeTime(dataUpdatedAt) : "未拉取"}
          </span>
        </div>
        {isLoading ? (
          <DisksSkeleton />
        ) : isError ? (
          <EmptyPlaceholder
            title="读取磁盘信息失败"
            description={String(error ?? "请检查 PowerShell 是否可用")}
            icon="💾"
            action={
              <button className="hb-btn-outline" onClick={() => refetch()}>
                重试
              </button>
            }
          />
        ) : disks.length === 0 ? (
          <EmptyPlaceholder
            title="未读取到磁盘"
            description="Get-PSDrive 返回为空。请确认 PowerShell 5.1+ 可用"
            icon="💾"
          />
        ) : (
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
            {disks.map((d) => (
              <DiskCard key={d.drive} disk={d} />
            ))}
          </div>
        )}
      </section>

      <PathSection />

      <ProcessesSection />

      {/* Phase C-3a · GDNbit 网站集成 */}
      <GdnbitSettingsSection />

      {/* 关于 · 版本与技术栈（从侧栏挪过来） */}
      <section aria-label="关于" className="border-t border-border pt-6">
        <p className="font-mono text-xs text-muted-foreground">
          HomeBase v0.1.0 · GDNbit 长颈鹿
        </p>
        <p className="mt-1 font-mono text-[11px] text-muted-foreground/70">
          Tauri 2 + React 19 · 阶段 13.1 · homebase-design.md v3
        </p>
      </section>
    </div>
  );
}

interface DiskCardProps {
  disk: {
    drive: string;
    free_gb: number;
    total_gb: number;
    warning?: boolean;
  };
}

function DiskCard({ disk }: DiskCardProps) {
  const used = disk.total_gb - disk.free_gb;
  const pct = disk.total_gb > 0 ? Math.round((used / disk.total_gb) * 100) : 0;
  return (
    <article className="hb-card">
      <div className="flex items-baseline justify-between">
        <h3 className="font-mono text-base font-semibold">{disk.drive}</h3>
        <span
          className={cn(
            "font-mono text-xs",
            disk.warning
              ? "text-destructive"
              : pct > 80
              ? "text-warning"
              : "text-muted-foreground",
          )}
        >
          {pct}%
        </span>
      </div>
      <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          className={cn(
            "h-full rounded-full transition-all",
            disk.warning
              ? "bg-destructive"
              : pct > 80
              ? "bg-warning"
              : "bg-primary-500",
          )}
          style={{ width: `${pct}%` }}
        />
      </div>
      <p className="mt-3 font-mono text-xs text-muted-foreground">
        剩余 {disk.free_gb.toFixed(1)} / {disk.total_gb.toFixed(1)} GB
      </p>
      {disk.warning && (
        <p className="mt-2 text-xs text-destructive">⚠ 容量告急</p>
      )}
    </article>
  );
}

function DisksSkeleton() {
  return (
    <div
      className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4"
      aria-busy
    >
      {Array.from({ length: 4 }).map((_, i) => (
        <div key={i} className="hb-card">
          <div className="flex items-baseline justify-between">
            <div className="h-5 w-12 animate-pulse rounded bg-muted" />
            <div className="h-3 w-8 animate-pulse rounded bg-muted" />
          </div>
          <div className="mt-3 h-1.5 animate-pulse rounded-full bg-muted" />
          <div className="mt-3 h-3 w-3/4 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}

// =====================================================================
// PATH · 进程（2026-05-23）
// =====================================================================

/** PATH 项目：User + Machine 两级，含死链检测（system_path_list 命令） */
function PathSection() {
  const {
    data: entries = [],
    isLoading,
    isError,
    error,
    refetch,
    dataUpdatedAt,
  } = useQuery({
    queryKey: ["system", "path-list"],
    queryFn: () => systemApi.pathList(),
    staleTime: 60_000, // 1 分钟内不重拉
  });

  // 默认显示死链 + 用户级；机器级（System32 等系统目录）默认折叠
  const [showAll, setShowAll] = useState(false);

  const userEntries = entries.filter((e) => e.source === "user");
  const machineEntries = entries.filter((e) => e.source === "machine");
  const broken = entries.filter((e) => !e.exists);

  return (
    <section aria-label="PATH 项目">
      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="flex items-center gap-2 font-serif text-lg font-semibold">
          <Route className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
          PATH 项目
          {!isLoading && !isError && entries.length > 0 && (
            <span className="font-mono text-xs font-normal text-muted-foreground">
              · {entries.length} 项 · {broken.length > 0 ? (
                <span className="text-destructive">{broken.length} 死链</span>
              ) : (
                <span className="text-success">全部可达</span>
              )}
            </span>
          )}
        </h2>
        <span className="font-mono text-xs text-muted-foreground">
          {dataUpdatedAt ? formatRelativeTime(dataUpdatedAt) : "未拉取"}
        </span>
      </div>

      {isLoading ? (
        <PathSkeleton />
      ) : isError ? (
        <EmptyPlaceholder
          title="读取 PATH 失败"
          description={String(error ?? "PowerShell 调用失败 · 检查执行策略")}
          icon="🛣️"
          action={
            <button className="hb-btn-outline" onClick={() => refetch()}>
              重试
            </button>
          }
        />
      ) : entries.length === 0 ? (
        <EmptyPlaceholder
          title="未读取到 PATH 项"
          description="HKCU + HKLM 都为空，少见情况"
          icon="🛣️"
        />
      ) : (
        <div className="space-y-3">
          {/* 死链高亮 · 始终展开 */}
          {broken.length > 0 && (
            <PathGroup
              label="死链"
              hint="路径不存在 · 建议从 PATH 注册表移除"
              entries={broken}
              tone="danger"
              defaultOpen
            />
          )}
          {/* 用户级 PATH（HKCU\Environment\Path）· 默认展开 */}
          {userEntries.length > 0 && (
            <PathGroup
              label="用户级（HKCU）"
              hint={`${userEntries.length} 项 · 当前用户私有`}
              entries={userEntries}
              tone="user"
              defaultOpen
            />
          )}
          {/* 机器级 PATH（HKLM）· 默认折叠 */}
          {machineEntries.length > 0 && (
            <PathGroup
              label="机器级（HKLM）"
              hint={`${machineEntries.length} 项 · 全用户共享 · 含 System32 / 系统工具`}
              entries={machineEntries}
              tone="machine"
              defaultOpen={showAll}
              onToggle={() => setShowAll(true)}
            />
          )}
        </div>
      )}
    </section>
  );
}

/** PATH 分组卡片（折叠展开 · 死链高亮 · raw vs expanded 对比） */
function PathGroup({
  label,
  hint,
  entries,
  tone,
  defaultOpen,
  onToggle,
}: {
  label: string;
  hint: string;
  entries: PathEntry[];
  tone: "danger" | "user" | "machine";
  defaultOpen?: boolean;
  onToggle?: () => void;
}) {
  const [open, setOpen] = useState(!!defaultOpen);

  const toneStyle = {
    danger: "border-destructive/40 bg-destructive/5",
    user: "border-border bg-card",
    machine: "border-border bg-card",
  }[tone];

  return (
    <div className={cn("rounded-xl border", toneStyle)}>
      <button
        type="button"
        onClick={() => {
          setOpen((v) => !v);
          if (!open) onToggle?.();
        }}
        className="flex w-full items-center justify-between px-4 py-3 text-left"
        aria-expanded={open}
      >
        <div className="flex items-center gap-2">
          {open ? (
            <ChevronDown className="h-4 w-4 text-muted-foreground" aria-hidden />
          ) : (
            <ChevronRight className="h-4 w-4 text-muted-foreground" aria-hidden />
          )}
          <span
            className={cn(
              "font-medium",
              tone === "danger" && "text-destructive",
            )}
          >
            {label}
          </span>
          <span className="font-mono text-[11px] text-muted-foreground">
            {hint}
          </span>
        </div>
        <span className="font-mono text-xs text-muted-foreground">
          {entries.length}
        </span>
      </button>
      {open && (
        <ul className="divide-y divide-border/60 border-t border-border/60">
          {entries.map((e, i) => (
            <PathRow key={`${e.source}-${i}-${e.path}`} entry={e} />
          ))}
        </ul>
      )}
    </div>
  );
}

/** 单个 PATH 行：状态徽章 + 展开后的路径 + 原始（如不同） */
function PathRow({ entry }: { entry: PathEntry }) {
  const showRaw = entry.raw !== entry.path;
  return (
    <li className="flex items-start gap-3 px-4 py-2.5">
      <span
        className={cn(
          "mt-0.5 flex-none",
          entry.exists ? "text-success" : "text-destructive",
        )}
        title={entry.exists ? "路径存在" : "死链 · 路径不存在"}
      >
        {entry.exists ? (
          <CheckCircle2 className="h-4 w-4" aria-hidden />
        ) : (
          <AlertTriangle className="h-4 w-4" aria-hidden />
        )}
      </span>
      <div className="min-w-0 flex-1">
        <p
          className={cn(
            "break-all font-mono text-[12px]",
            entry.exists ? "text-foreground" : "text-destructive line-through",
          )}
          title={entry.path}
        >
          {entry.path}
        </p>
        {showRaw && (
          <p
            className="break-all font-mono text-[10px] text-muted-foreground/70"
            title="原始 PATH 值（含 %VAR% 占位符）"
          >
            原始：{entry.raw}
          </p>
        )}
      </div>
    </li>
  );
}

function PathSkeleton() {
  return (
    <div className="space-y-3" aria-busy>
      {[12, 8].map((rows, i) => (
        <div key={i} className="rounded-xl border border-border bg-card">
          <div className="border-b border-border/60 px-4 py-3">
            <div className="h-4 w-40 animate-pulse rounded bg-muted" />
          </div>
          <ul className="divide-y divide-border/60">
            {Array.from({ length: rows }).map((_, j) => (
              <li key={j} className="flex items-center gap-3 px-4 py-2.5">
                <div className="h-4 w-4 animate-pulse rounded bg-muted" />
                <div className="h-3 w-3/4 animate-pulse rounded bg-muted" />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

/** 关键进程一览：HomeBase 自身 / 5 agent / sops（system_processes 命令） */
function ProcessesSection() {
  const {
    data: procs = [],
    isLoading,
    isError,
    error,
    refetch,
    dataUpdatedAt,
  } = useQuery({
    queryKey: ["system", "processes"],
    queryFn: () => systemApi.processes(),
    refetchInterval: 5_000, // 5s 轮询 · 让 CPU% / 内存动态变化
  });

  return (
    <section aria-label="关键进程">
      <div className="mb-3 flex items-baseline justify-between">
        <h2 className="flex items-center gap-2 font-serif text-lg font-semibold">
          <Cpu className="h-4 w-4 text-primary-600 dark:text-primary-400" aria-hidden />
          关键进程
          {!isLoading && !isError && (
            <span className="font-mono text-xs font-normal text-muted-foreground">
              · {procs.length} 个在跑
            </span>
          )}
        </h2>
        <span className="font-mono text-xs text-muted-foreground">
          sysinfo · {dataUpdatedAt ? formatRelativeTime(dataUpdatedAt) : "未拉取"}
        </span>
      </div>

      {isLoading ? (
        <ProcessesSkeleton />
      ) : isError ? (
        <EmptyPlaceholder
          title="读取进程列表失败"
          description={String(error ?? "未知错误")}
          icon="⚙️"
          action={
            <button className="hb-btn-outline" onClick={() => refetch()}>
              重试
            </button>
          }
        />
      ) : procs.length === 0 ? (
        <EmptyPlaceholder
          title="目前没有关键进程在跑"
          description="HomeBase 自身、5 个 AI 代理、sops 都未检出。当前应用从开发模式启动时进程名可能不匹配。"
          icon="💤"
        />
      ) : (
        <div className="overflow-hidden rounded-xl border border-border bg-card">
          <table className="w-full text-sm">
            <thead className="border-b border-border/60 bg-muted/20 text-left text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
              <tr>
                <th className="px-4 py-2">进程</th>
                <th className="px-4 py-2 text-right">PID</th>
                <th className="px-4 py-2 text-right">CPU</th>
                <th className="px-4 py-2 text-right">RAM</th>
                <th className="px-4 py-2 text-right">运行</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border/60">
              {procs.map((p) => (
                <ProcessRow key={`${p.category}-${p.pid}`} proc={p} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function ProcessRow({ proc }: { proc: ProcessInfo }) {
  return (
    <tr className="transition-colors hover:bg-muted/20">
      <td className="px-4 py-2.5">
        <div className="flex items-center gap-2">
          <CategoryIcon category={proc.category} />
          <div className="min-w-0">
            <p className="truncate font-medium">{proc.display_name}</p>
            <p className="truncate font-mono text-[10px] text-muted-foreground">
              {proc.name}
            </p>
          </div>
        </div>
      </td>
      <td className="px-4 py-2.5 text-right font-mono text-xs text-muted-foreground">
        {proc.pid}
      </td>
      <td
        className={cn(
          "px-4 py-2.5 text-right font-mono text-xs",
          proc.cpu_percent > 50 ? "text-warning" : "text-foreground",
        )}
      >
        {proc.cpu_percent.toFixed(1)}%
      </td>
      <td className="px-4 py-2.5 text-right font-mono text-xs">
        {formatMemory(proc.memory_mb)}
      </td>
      <td className="px-4 py-2.5 text-right font-mono text-xs text-muted-foreground">
        {formatRunTime(proc.run_secs)}
      </td>
    </tr>
  );
}

function CategoryIcon({ category }: { category: string }) {
  const map: Record<string, { icon: typeof Server; color: string; title: string }> = {
    homebase: { icon: Server, color: "text-primary-600 dark:text-primary-400", title: "HomeBase 自身" },
    agent: { icon: Cpu, color: "text-blue-600 dark:text-blue-400", title: "AI 代理" },
    tool: { icon: Wrench, color: "text-amber-600 dark:text-amber-400", title: "工具进程" },
  };
  const meta = map[category] ?? { icon: Cpu, color: "text-muted-foreground", title: category };
  const Icon = meta.icon;
  return (
    <span title={meta.title} className={cn("flex-none", meta.color)}>
      <Icon className="h-4 w-4" aria-hidden />
    </span>
  );
}

function ProcessesSkeleton() {
  return (
    <div className="overflow-hidden rounded-xl border border-border bg-card" aria-busy>
      <div className="border-b border-border/60 bg-muted/20 px-4 py-2">
        <div className="h-3 w-20 animate-pulse rounded bg-muted" />
      </div>
      {Array.from({ length: 4 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3 border-b border-border/60 px-4 py-2.5 last:border-0">
          <div className="h-4 w-4 animate-pulse rounded bg-muted" />
          <div className="h-3 w-32 animate-pulse rounded bg-muted" />
          <div className="ml-auto h-3 w-12 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}

function formatMemory(mb: number): string {
  if (mb >= 1024) return `${(mb / 1024).toFixed(2)} GB`;
  return `${mb.toFixed(1)} MB`;
}

function formatRunTime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  const mins = Math.floor(secs / 60);
  if (mins < 60) return `${mins}m ${secs % 60}s`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h ${mins % 60}m`;
  const days = Math.floor(hrs / 24);
  return `${days}d ${hrs % 24}h`;
}
