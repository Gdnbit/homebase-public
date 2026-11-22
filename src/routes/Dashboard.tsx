import { useQueries, useQuery } from "@tanstack/react-query";
import { ArrowRight, Cpu, MemoryStick, MonitorCog, Stethoscope, Upload } from "lucide-react";
import { Link } from "react-router-dom";
import {
  agentsApi,
  secretsApi,
  systemApi,
  type GpuMetrics,
  type SystemInfo,
  type SystemMetrics,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";
import { isConfigured, secretStatus } from "@/lib/secret-lifecycle";
import { useClock } from "@/lib/use-clock";
import { AssistantUsage } from "@/components/assistant-usage";

/**
 * 控制台首页 — 一屏看完本机系统状态（v3 §6.1）
 *
 * 实时聚合（轮询频率见 refetchInterval）：
 * - 4 张关键指标卡：磁盘 / AI 代理 / 密钥 / 告警（5s）
 * - 3 张系统资源卡：CPU / RAM（5s 共用 system_metrics）+ GPU（10s 独立）
 * - Agents status：每 5s
 */
export function Dashboard() {
  const now = useClock();
  const info = useQuery({
    queryKey: ["system", "info"],
    queryFn: () => systemApi.info(),
  });
  const disks = useQuery({
    queryKey: ["system", "disks"],
    queryFn: () => systemApi.disks(),
    refetchInterval: 30_000, // 磁盘 30s 轮询（容量变化慢）
  });
  const metrics = useQuery({
    queryKey: ["system", "metrics"],
    queryFn: () => systemApi.metrics(),
    refetchInterval: 5_000, // CPU + RAM 5s 实时
  });
  const gpu = useQuery({
    queryKey: ["system", "gpu"],
    queryFn: () => systemApi.gpu(),
    refetchInterval: 10_000, // GPU perf counter 慢，10s 一次
  });
  const secrets = useQuery({
    queryKey: ["secrets", "list"],
    queryFn: () => secretsApi.list(),
  });
  const agents = useQuery({
    queryKey: ["agents", "list"],
    queryFn: () => agentsApi.list(),
  });
  const agentStatuses = useQueries({
    queries: (agents.data ?? []).map((a) => ({
      queryKey: ["agents", "status", a.id],
      queryFn: () => agentsApi.status(a.id),
      refetchInterval: 5_000,
    })),
  });

  // 聚合
  const diskCount = disks.data?.length ?? 0;
  const diskWarn = disks.data?.filter((d) => d.warning).length ?? 0;
  const diskHealthy = diskCount - diskWarn;

  const secretCount = secrets.data?.length ?? 0;
  const secretConfigured =
    secrets.data?.filter(isConfigured).length ?? 0;
  const secretMissing = secretCount - secretConfigured;
  const secretExpiryWarnings = secrets.data?.filter((s) => secretStatus(s, now) === "warning").length ?? 0;

  const agentCount = agents.data?.length ?? 0;
  const agentRunning = agentStatuses.filter((s) => s.data?.state === "running")
    .length;
  const agentMisconfigured = agentStatuses.filter(
    (s) => s.data?.state === "misconfigured",
  ).length;

  const totalWarnings = diskWarn + secretMissing + secretExpiryWarnings + agentMisconfigured;
  const incomplete = disks.isError || secrets.isError || agents.isError || agentStatuses.some(s => s.isError);

  const isLoading =
    disks.isLoading || secrets.isLoading || agents.isLoading || agentStatuses.some(s => s.isLoading);

  return (
    <div className="animate-fade-in space-y-10">
      <header className="hb-page-header"><span className="hb-section-label">Workbench</span><h1>工作台</h1><p>查看任务、已上报用量与本机运行状况。</p></header>
      <AssistantUsage />
      {/* 4 数据卡片 — 实时聚合 */}
      <section
        aria-label="关键指标"
        className="grid grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-4"
      >
        <StatCard
          label="磁盘健康"
          value={
            isLoading ? "…" : disks.isError ? "未知" : diskCount === 0 ? "—" : `${diskHealthy} / ${diskCount}`
          }
          hint={
            disks.isError
              ? "读取失败"
              : diskCount === 0
              ? "未读取到磁盘"
              : diskWarn > 0
              ? `${diskWarn} 块磁盘空间告急`
              : "全部健康"
          }
          tone={disks.isError || diskWarn > 0 ? "warning" : "success"}
        />
        <StatCard
          label="AI 代理"
          value={
            agents.isLoading || agentStatuses.some(s => s.isLoading)
              ? "…"
              : agents.isError || agentStatuses.some(s => s.isError) ? "未知" : `${agentRunning} / ${agentCount}`
          }
          hint={
            agents.isError || agentStatuses.some(s => s.isError)
              ? "部分代理状态读取失败，数量未能完整核实"
              : agentRunning === 0
              ? "无代理运行"
              : `${agentRunning} 个代理运行中`
          }
          tone={agents.isError || agentStatuses.some(s => s.isError) ? "warning" : agentRunning > 0 ? "success" : "neutral"}
        />
        <StatCard
          label="密钥"
          value={
            secrets.isLoading
              ? "…"
              : secrets.isError ? "—" : `${secretConfigured} / ${secretCount}`
          }
          hint={
            secrets.isError
              ? "读取失败"
              : `${secretMissing} 把未填 · ${secretExpiryWarnings} 把到期／日期告警`
          }
          tone={secrets.isError || secretMissing > 0 || secretExpiryWarnings > 0 ? "warning" : "success"}
        />
        <StatCard
          label="告警"
          value={isLoading ? "…" : incomplete ? "未知" : String(totalWarnings)}
          hint={
            incomplete ? "部分查询失败，无法确认告警总数" : totalWarnings === 0
              ? "无未处理告警"
              : `磁盘 ${diskWarn} · 密钥未填 ${secretMissing} · 到期 ${secretExpiryWarnings} · 代理 ${agentMisconfigured}`
          }
          tone={incomplete ? "warning" : totalWarnings > 0 ? "destructive" : "success"}
        />
      </section>

      {/* 3 张系统资源卡 — CPU / RAM / GPU 实时利用率 + 进度条 */}
      <section aria-label="系统资源">
        <h2 className="mb-3 font-serif text-lg font-semibold">系统资源</h2>
        <div className="grid grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-3">
          <ResourceCard
            icon={<Cpu className="h-4 w-4" aria-hidden />}
            label="CPU"
            metrics={metrics.data}
            isLoading={metrics.isLoading}
            isError={metrics.isError}
            kind="cpu"
          />
          <ResourceCard
            icon={<MemoryStick className="h-4 w-4" aria-hidden />}
            label="内存"
            metrics={metrics.data}
            isLoading={metrics.isLoading}
            isError={metrics.isError}
            kind="memory"
          />
          <GpuResourceCard
            icon={<MonitorCog className="h-4 w-4" aria-hidden />}
            data={gpu.data}
            isLoading={gpu.isLoading}
          />
        </div>
      </section>

      {/* 快速操作· 仅放侧栏不能直达的动作类入口（避免与导航重复）*/}
      <section aria-label="快速操作">
        <h2 className="mb-3 font-serif text-lg font-semibold">快速操作</h2>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <QuickActionCard
            to="/knowledge"
            icon={<Upload className="h-5 w-5" aria-hidden />}
            label="发布现有 wiki"
            desc="发布已选文章至配置的站点"
            tone="default"
          />
          <QuickActionCard
            to="/system"
            icon={<Stethoscope className="h-5 w-5" aria-hidden />}
            label="GDNbit 诊断"
            desc="本地 6 项 doctor + 联网诊断 + 批量同步 vault"
            tone="default"
          />
        </div>
      </section>

      {/* 系统信息（来自 system_info）*/}
      <section aria-label="系统信息">
        <h2 className="mb-3 font-serif text-lg font-semibold">系统信息</h2>
        <div className="hb-card">
          {info.isLoading ? (
            <SkeletonRows rows={6} />
          ) : info.data ? (
            <SystemInfoTable data={info.data} />
          ) : (
            <p className="text-sm text-muted-foreground">未能获取系统信息</p>
          )}
        </div>
      </section>
    </div>
  );
}

type StatTone = "success" | "warning" | "destructive" | "neutral";

interface StatCardProps {
  label: string;
  value: string;
  hint?: string;
  tone?: StatTone;
}

function StatCard({ label, value, hint, tone = "neutral" }: StatCardProps) {
  return (
    <article className="hb-stat-card">
      <div className="flex items-center gap-2">
        <span className="hb-stat-card__label">{label}</span>
        <span className={cn("h-1.5 w-1.5 rounded-full", toneDotColor(tone))} aria-hidden />
      </div>
      <span className="hb-stat-card__value">{value}</span>
      {hint && <span className="hb-stat-card__hint">{hint}</span>}
    </article>
  );
}

function toneDotColor(tone: StatTone): string {
  switch (tone) {
    case "success":
      return "bg-success";
    case "warning":
      return "bg-warning";
    case "destructive":
      return "bg-destructive";
    case "neutral":
      return "bg-muted-foreground/40";
  }
}

// =====================================================================
// ResourceCard — CPU / RAM 通用卡（共用 system_metrics 数据）
// =====================================================================

interface ResourceCardProps {
  icon: React.ReactNode;
  label: string;
  metrics: SystemMetrics | undefined;
  isLoading: boolean;
  isError: boolean;
  kind: "cpu" | "memory";
}

function ResourceCard({
  icon,
  label,
  metrics,
  isLoading,
  isError,
  kind,
}: ResourceCardProps) {
  const value = metrics
    ? kind === "cpu"
      ? metrics.cpu_percent
      : metrics.memory_percent
    : null;
  const warning = metrics
    ? kind === "cpu"
      ? metrics.cpu_warning
      : metrics.memory_warning
    : false;
  const tone: StatTone = isError || warning ? "warning" : value === null || value > 60 ? "neutral" : "success";

  return (
    <article className="hb-card flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <span className="text-muted-foreground">{icon}</span>
        <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
          {label}
        </span>
        <span
          className={cn("ml-auto h-1.5 w-1.5 rounded-full", toneDotColor(tone))}
          aria-hidden
        />
      </div>
      {isError ? (
        <span className="font-serif text-xl text-destructive">读取失败</span>
      ) : isLoading ? (
        <span className="font-serif text-3xl text-muted-foreground">…</span>
      ) : value === null ? (
        <span className="text-muted-foreground">未知：未取得有效采样</span>
      ) : (
        <>
          <span className="font-serif text-3xl font-semibold tracking-tight tabular-nums">
            {value.toFixed(1)}
            <span className="ml-1 text-base font-normal text-muted-foreground">%</span>
          </span>
          <ProgressBar percent={value} warning={warning} />
        </>
      )}
    </article>
  );
}

// =====================================================================
// GpuResourceCard — GPU 单独卡（system_gpu，可能不可用）
// =====================================================================

interface GpuResourceCardProps {
  icon: React.ReactNode;
  data: GpuMetrics | undefined;
  isLoading: boolean;
}

export function GpuResourceCard({ icon, data, isLoading }: GpuResourceCardProps) {
  return (
    <article className="hb-card flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <span className="text-muted-foreground">{icon}</span>
        <span className="text-xs font-medium uppercase tracking-widest text-muted-foreground">
          GPU
        </span>
        <span
          className={cn(
            "ml-auto h-1.5 w-1.5 rounded-full",
            data?.gpu_warning
              ? "bg-warning"
              : data?.available
              ? "bg-success"
              : "bg-muted-foreground/40",
          )}
          aria-hidden
        />
      </div>
      {isLoading ? (
        <span className="font-serif text-3xl text-muted-foreground">…</span>
      ) : !data?.available || data.gpu_percent == null ? (
        <>
          <span className="font-serif text-2xl text-muted-foreground">
            {data?.gpu_status === "not_detected" ? "未检测到" : "未知"}
          </span>
          <span className="text-xs text-muted-foreground">
            {data?.error ?? data?.gpu_message ?? "GPU 信息未知：查询未返回有效结果"}
          </span>
        </>
      ) : (
        <>
          <span className="font-serif text-3xl font-semibold tracking-tight tabular-nums">
            {(data.gpu_percent ?? 0).toFixed(1)}
            <span className="ml-1 text-base font-normal text-muted-foreground">%</span>
          </span>
          <ProgressBar percent={data.gpu_percent ?? 0} warning={data.gpu_warning} />
        </>
      )}
    </article>
  );
}

// =====================================================================
// ProgressBar — 通用进度条（warning 时变橙）
// =====================================================================

function ProgressBar({ percent, warning }: { percent: number; warning: boolean }) {
  const clamped = Math.max(0, Math.min(100, percent));
  return (
    <div
      className="h-1.5 w-full overflow-hidden rounded-full bg-muted"
      role="progressbar"
      aria-valuenow={Math.round(clamped)}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className={cn(
          "h-full rounded-full transition-all duration-500",
          warning ? "bg-warning" : "bg-success",
        )}
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}

// =====================================================================
// SystemInfoTable — 概览底部"系统信息"· 一闪看全机型资料
// =====================================================================

export function SystemInfoTable({ data }: { data: SystemInfo }) {
  // 显卡始终显示识别状态，其他可选硬件字段按数据展示。
  type Row = { label: string; value: React.ReactNode; mono?: boolean };
  const rows: Row[] = [];

  // —— 硬件 ——
  if (data.cpu_brand) {
    const cores: string[] = [];
    if (data.cpu_physical_cores) cores.push(`${data.cpu_physical_cores} 核`);
    if (data.cpu_logical_cores) cores.push(`${data.cpu_logical_cores} 线程`);
    rows.push({
      label: "处理器",
      value: (
        <>
          {data.cpu_brand}
          {cores.length > 0 && (
            <span className="ml-2 text-muted-foreground">· {cores.join(" / ")}</span>
          )}
        </>
      ),
    });
  } else if (data.cpu_logical_cores) {
    rows.push({
      label: "处理器",
      value: `${data.cpu_logical_cores} 线程`,
    });
  } else {
    rows.push({ label: "处理器", value: "未知：系统未提供处理器信息" });
  }

  if (data.memory_total_gb) {
    rows.push({
      label: "内存",
      value: `${data.memory_total_gb.toFixed(1)} GB`,
    });
  } else {
    rows.push({ label: "内存", value: "未知：系统未提供有效内存总量" });
  }

  rows.push({
    label: "显卡",
    value: data.gpu_name ?? data.gpu_message ?? (
      data.gpu_status === "not_detected" ? "未检测到物理 GPU" : "GPU 信息未知：查询未返回有效结果"
    ),
  });

  // —— 主机 ——
  const machine: string[] = [];
  if (data.computer_manufacturer) machine.push(data.computer_manufacturer);
  if (data.computer_model) machine.push(data.computer_model);
  if (machine.length > 0) {
    rows.push({ label: "整机", value: machine.join(" · ") });
  } else {
    rows.push({ label: "整机", value: "未知：未取得厂商及型号" });
  }

  rows.push({
    label: "操作系统",
    value: (
      <>
        {data.os_version || data.os}
        <span className="ml-2 text-muted-foreground">({data.arch})</span>
      </>
    ),
  });

  // —— 账号（次要 · 放最后）——
  rows.push({ label: "主机名", value: data.hostname, mono: true });
  rows.push({ label: "用户名", value: data.username, mono: true });

  return (
    <><dl className="grid grid-cols-1 gap-x-8 gap-y-2 text-sm sm:grid-cols-[6rem_1fr]">
      {rows.map((r) => (
        <div key={r.label} className="contents">
          <dt className="text-muted-foreground">{r.label}</dt>
          <dd className={cn("min-w-0 break-words", r.mono && "font-mono")}>
            {r.value}
          </dd>
        </div>
      ))}
    </dl>
    {!!data.issues?.length && <ul aria-label="系统信息缺失说明" className="mt-3 text-sm text-warning">{data.issues.map((issue, index) => <li key={`${issue.field}-${index}`}>{issue.message}</li>)}</ul>}</>
  );
}

function SkeletonRows({ rows }: { rows: number }) {
  return (
    <div className="space-y-3" aria-busy aria-live="polite">
      {Array.from({ length: rows }).map((_, i) => (
        <div key={i} className="flex gap-4">
          <div className="h-4 w-24 animate-pulse rounded bg-muted" />
          <div className="h-4 flex-1 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}

// =====================================================================
// QuickActionCard — 快捷入口卡（Phase C 入口优化 · 2026-05-06）
// =====================================================================

interface QuickActionCardProps {
  to: string;
  icon: React.ReactNode;
  label: string;
  desc: string;
  tone?: "primary" | "default";
  /** 可选右上角小徽章（如 "D1 骨架" / "Beta" / "New"） */
  badge?: string;
}

function QuickActionCard({ to, icon, label, desc, tone = "default", badge }: QuickActionCardProps) {
  return (
    <Link
      to={to}
      className={cn(
        "hb-card group relative flex items-start gap-3 transition-all",
        "hover:border-primary-400 hover:shadow-sm",
        tone === "primary" && "border-primary-300/60 bg-primary-50/30 dark:bg-primary-900/10",
      )}
    >
      {badge && (
        <span
          className="absolute right-3 top-3 rounded-full border border-primary-300/60 bg-primary-50 px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-widest text-primary-700 dark:border-primary-700/60 dark:bg-primary-900/30 dark:text-primary-300"
          aria-hidden
        >
          {badge}
        </span>
      )}
      <span
        className={cn(
          "mt-0.5 flex h-10 w-10 flex-none items-center justify-center rounded-md",
          tone === "primary"
            ? "bg-primary-600 text-white"
            : "bg-muted text-muted-foreground group-hover:bg-primary-100 group-hover:text-primary-700",
        )}
      >
        {icon}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="font-serif text-base font-semibold leading-tight">{label}</span>
          <ArrowRight
            className="h-3.5 w-3.5 text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:text-primary-600"
            aria-hidden
          />
        </div>
        <p className="mt-1 text-xs text-muted-foreground">{desc}</p>
      </div>
    </Link>
  );
}
