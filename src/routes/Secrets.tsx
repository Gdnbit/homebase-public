import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";
import {
  AlertTriangle,
  ArrowLeft,
  ClipboardPaste,
  Eye,
  EyeOff,
  Plus,
  Search,
  Sparkles,
  X,
} from "lucide-react";
import {
  secretsApi,
  type SecretInfo,
} from "@/lib/tauri-api";
import { EmptyPlaceholder } from "@/components/empty-placeholder";
import { ServiceLogo } from "@/components/service-logo";
import {
  getServiceMeta,
  listKnownServices,
  type ServiceMeta,
} from "@/lib/service-registry";
import { cn } from "@/lib/utils";
import { SecretLifecycleRow as SecretRow } from "@/components/secret-lifecycle-row";
import { isConfigured, localDateToExpiry, secretStatus } from "@/lib/secret-lifecycle";
import { refreshCapabilityCatalog } from "@/lib/capability-index";
import { useClock } from "@/lib/use-clock";
import { isNativeClosePending, registerVaultCloseStatus } from "@/lib/assistant-draft-lifecycle";

/** Reveal 自动 hide 计时（秒）— 超时后清 state，不保留明文 */

/** 过期警告阈值（天）— 桌面端 WPF 也是 7 天 */

/** 直接 invoke clipboard-manager plugin（避免加 npm 依赖）
 *  等价于 @tauri-apps/plugin-clipboard-manager 的 readText()
 *  Tauri 2 后端返回 { plainText: string } 或直接 string；兼容两种
 */
async function readClipboard(): Promise<string | null> {
  const res = await invoke<{ plainText?: string } | string | null>(
    "plugin:clipboard-manager|read_text",
  );
  if (!res) return null;
  if (typeof res === "string") return res;
  return res.plainText ?? null;
}

/**
 * 密钥 — SOPS 密钥管理总控（v3 §6.3）
 *
 * Phase 13.2 重设计（状态驱动布局，桌面 GUI 全接手）：
 * - 顶部 4 张 stat 卡（总数 / 已填 / 未填 / 即将过期）
 * - 搜索 + 状态过滤（全部 / 已填 / 未填 / 即将过期）
 * - 按"即将过期 → 已填 → 未填"分组（可折叠，过期组默认展开）
 * - 待加：Add Wizard / Service logo / 完整 service map
 *
 * 安全约束（v3 §12.2）：解密结果仅内存（SecureString Drop zeroize）
 */
type StatusFilter = "all" | "filled" | "missing" | "warning";

export function Secrets() {
  const now = useClock();
  const dirtyRows = useRef(new Set<string>());
  const updateDirty = useCallback((name: string, dirty: boolean) => {
    if (dirty) dirtyRows.current.add(name); else dirtyRows.current.delete(name);
  }, []);
  const allowFilter = () => dirtyRows.current.size === 0 || window.confirm("更改搜索或筛选可能关闭编辑并丢弃未保存草稿，是否继续？");
  useEffect(() => {
    const guard = (event: MouseEvent) => {
      const link = (event.target as Element | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
      if (!link || link.origin !== location.origin || link.pathname === location.pathname || dirtyRows.current.size === 0) return;
      if (!window.confirm("离开密钥页面会丢弃未保存的草稿，是否继续？")) { event.preventDefault(); event.stopPropagation(); }
    };
    document.addEventListener("click", guard, true);
    return () => document.removeEventListener("click", guard, true);
  }, []);
  const {
    data: secrets = [],
    isLoading,
    isError,
    error,
    refetch,
  } = useQuery({
    queryKey: ["secrets", "list"],
    queryFn: () => secretsApi.list(),
  });

  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
  const [addOpen, setAddOpen] = useState(false);

  // 按状态分类 + 应用搜索/过滤（结果用于列表渲染）
  const { warnings, filled, missing } = useMemo(() => {
    const w: SecretInfo[] = [];
    const f: SecretInfo[] = [];
    const m: SecretInfo[] = [];
    const q = search.trim().toLowerCase();
    for (const s of secrets) {
      if (q) {
        const haystack = [
          s.short_name,
          s.mcp_long_name ?? "",
          s.display_name ?? "",
          s.category,
          getServiceMeta(s.short_name).displayName,
          ...s.used_by,
        ]
          .join(" ")
          .toLowerCase();
        if (!haystack.includes(q)) continue;
      }
      const status = categorizeByStatus(s, now);
      if (statusFilter !== "all" && status !== statusFilter) continue;
      if (status === "warning") w.push(s);
      else if (status === "filled") f.push(s);
      else m.push(s);
    }
    return { warnings: w, filled: f, missing: m };
  }, [secrets, search, statusFilter, now]);

  // 全局统计（不受过滤影响，反映真实情况）
  const totalCount = secrets.length;
  const filledCount = secrets.filter(isConfigured).length;
  const missingCount = secrets.filter((s) => !isConfigured(s)).length;
  const warningCount = secrets.filter(
    (s) => categorizeByStatus(s, now) === "warning",
  ).length;

  const hasActiveFilter = search.trim() !== "" || statusFilter !== "all";
  const noResults =
    !isLoading &&
    !isError &&
    secrets.length > 0 &&
    warnings.length === 0 &&
    filled.length === 0 &&
    missing.length === 0;

  return (
    <div className="animate-fade-in space-y-6">
      <header className="hb-page-header">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0 flex-1">
            <span className="hb-section-label">Secrets</span>
            <h1>密钥</h1>
            <p>
              本机加密管理的 {totalCount} 把密钥
              <span className="ml-2 text-xs text-muted-foreground">
                · 密钥正文不写入明文草稿、日志或元数据
              </span>
            </p>
          </div>
          <button
            type="button"
            className="hb-btn-primary shrink-0"
            onClick={() => setAddOpen(true)}
            title="添加新密钥（短名 + value + 可选过期日）"
          >
            <Plus className="h-4 w-4" />
            添加密钥
          </button>
        </div>
      </header>

      {!isLoading && !isError && secrets.length > 0 && (
        <>
          <SecretsStats
            total={totalCount}
            filled={filledCount}
            missing={missingCount}
            warning={warningCount}
          />
          <SecretsToolbar
            search={search}
            onSearchChange={(value) => { if (allowFilter()) setSearch(value); }}
            statusFilter={statusFilter}
            onStatusFilterChange={(value) => { if (allowFilter()) setStatusFilter(value); }}
          />
        </>
      )}

      {isLoading ? (
        <SecretsSkeleton />
      ) : isError ? (
        <EmptyPlaceholder
          title="读取密钥列表失败"
          description={String(
            error ?? "请检查 sops.exe 路径和 age key（SOPS_AGE_KEY_FILE）是否就位",
          )}
          icon="🔐"
          action={
            <button className="hb-btn-outline" onClick={() => refetch()}>
              重试
            </button>
          }
        />
      ) : secrets.length === 0 ? (
        <EmptyPlaceholder
          title="未读取到任何密钥"
          description="secrets.yaml 解析为空。请检查 C:\\HomeBase\\Secrets\\secrets.yaml 是否存在 + age key 是否能解密"
          icon="🔐"
        />
      ) : noResults ? (
        <EmptyPlaceholder
          title="无匹配密钥"
          description={
            search.trim()
              ? `搜索 "${search}" 没有结果`
              : "当前过滤条件下没有结果"
          }
          icon="🔍"
          action={
            hasActiveFilter ? (
              <button
                className="hb-btn-outline"
                onClick={() => {
                  setSearch("");
                  setStatusFilter("all");
                }}
              >
                清除过滤
              </button>
            ) : undefined
          }
        />
      ) : (
        <ul className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,260px),1fr))] items-start gap-4" aria-label="密钥列表">
          {[...warnings, ...filled, ...missing].map((secret) => (
            <SecretRow key={secret.short_name} secret={secret} onDirtyChange={updateDirty} />
          ))}
        </ul>
      )}

      <AddSecretModal
        open={addOpen}
        onClose={() => setAddOpen(false)}
        existingNames={secrets.map((s) => s.short_name)}
        vaultVersion={secrets[0]?.vault_version ?? ""}
      />
    </div>
  );
}

/** 状态分类：warning（已过期或 < 7 天） / filled（已填且不接近过期） / missing（未填） */
function categorizeByStatus(s: SecretInfo, now = Date.now()): "warning" | "filled" | "missing" {
  return secretStatus(s, now);
}

interface SecretsStatsProps {
  total: number;
  filled: number;
  missing: number;
  warning: number;
}

function SecretsStats({ total, filled, missing, warning }: SecretsStatsProps) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
      <StatCard label="总数" value={total} tone="default" />
      <StatCard label="已填" value={filled} tone="success" />
      <StatCard label="未填" value={missing} tone="muted" />
      <StatCard
        label="到期告警"
        value={warning}
        tone={warning > 0 ? "warning" : "default"}
      />
    </div>
  );
}

function StatCard({
  label,
  value,
  tone,
}: {
  label: string;
  value: number;
  tone: "default" | "success" | "muted" | "warning";
}) {
  const palette = {
    default: "border-border bg-card text-foreground",
    success: "border-success/40 bg-success/10 text-success-strong",
    muted: "border-border bg-muted/30 text-muted-foreground",
    warning: "border-warning/50 bg-warning/10 text-warning-strong",
  }[tone];
  return (
    <div className={cn("rounded-xl border px-4 py-3 transition-colors", palette)}>
      <p className="text-xs font-medium uppercase tracking-widest opacity-80">
        {label}
      </p>
      <p className="mt-1 font-mono text-2xl font-semibold tabular-nums">{value}</p>
    </div>
  );
}

interface SecretsToolbarProps {
  search: string;
  onSearchChange: (v: string) => void;
  statusFilter: StatusFilter;
  onStatusFilterChange: (v: StatusFilter) => void;
}

function SecretsToolbar({
  search,
  onSearchChange,
  statusFilter,
  onStatusFilterChange,
}: SecretsToolbarProps) {
  const filters: Array<{ value: StatusFilter; label: string }> = [
    { value: "all", label: "全部" },
    { value: "filled", label: "已填" },
    { value: "missing", label: "未填" },
    { value: "warning", label: "过期" },
  ];
  return (
    <div className="flex flex-wrap items-center gap-3">
      <div className="relative min-w-[220px] flex-1">
        <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
        <input
          type="text"
          value={search}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder="搜索短名 / 长名 / 消费者…"
          className="w-full rounded-lg border border-border bg-background py-2 pl-9 pr-3 text-sm transition-colors placeholder:text-muted-foreground focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
          aria-label="搜索密钥"
        />
      </div>
      <div
        className="flex gap-1 rounded-lg border border-border bg-muted/30 p-1"
        role="group"
        aria-label="状态过滤"
      >
        {filters.map((f) => (
          <button
            key={f.value}
            type="button"
            className={cn(
              "rounded-md px-3 py-1.5 text-xs font-medium transition-all",
              statusFilter === f.value
                ? "bg-background text-foreground shadow-sm"
                : "text-muted-foreground hover:text-foreground",
            )}
            onClick={() => onStatusFilterChange(f.value)}
            aria-pressed={statusFilter === f.value}
          >
            {f.label}
          </button>
        ))}
      </div>
    </div>
  );
}


// =====================================================================
// AddSecretModal — 添加新密钥（短名 + value + 可选过期日）
// =====================================================================

interface AddSecretModalProps {
  vaultVersion: string;
  open: boolean;
  onClose: () => void;
  existingNames: string[];
}

/** 客户端校验（与后端 validate_short_name 对齐，保险层） */
function validateShortNameClient(
  name: string,
  existing: string[],
): string | null {
  if (name.length < 3 || name.length > 64) {
    return "短名长度须在 3-64 字符之间";
  }
  if (!/^[a-z]/.test(name)) return "短名必须以小写字母开头";
  if (!/^[a-z][a-z0-9_]*$/.test(name)) {
    return "短名只能含小写字母 / 数字 / 下划线";
  }
  if (existing.includes(name)) return `短名「${name}」已存在`;
  return null;
}

type WizardStep = "select" | "fill";
interface SelectedService {
  key: string;
  meta: ServiceMeta;
  /** 1 = HomeBase 已接入预填；2 = 常见服务仅 logo（用户填短名） */
  tier: 1 | 2;
}

function AddSecretModal({ open, onClose: onDismiss, existingNames, vaultVersion }: AddSecretModalProps) {
  const queryClient = useQueryClient();
  const [step, setStep] = useState<WizardStep>("select");
  const [selectedService, setSelectedService] = useState<SelectedService | null>(null);
  const [shortName, setShortName] = useState("");
  // 用户自定义显示名：预填为 Tier 1/2 的 displayName，用户可改
  const [displayName, setDisplayName] = useState("");
  const [value, setValue] = useState("");
  const [expiresAt, setExpiresAt] = useState("");
  const [showValue, setShowValue] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // open 关闭时全部 reset
  useEffect(() => {
    if (!open) {
      setStep("select");
      setSelectedService(null);
      setShortName("");
      setDisplayName("");
      setValue("");
      setExpiresAt("");
      setShowValue(false);
      setError(null);
    }
  }, [open]);

  const nameError =
    step === "fill" && shortName
      ? validateShortNameClient(shortName, existingNames)
      : null;

  const [isSubmitting, setSubmitting] = useState(false);
  const submitting = useRef(false);
  const hasDraft = !!(value || shortName || displayName || expiresAt);
  useLayoutEffect(() => {
    if (!open) return;
    return registerVaultCloseStatus(() => {
      if (submitting.current) return "saving";
      return hasDraft ? "draft" : "clean";
    });
  }, [open, hasDraft]);
  const [currentVersion, setCurrentVersion] = useState(vaultVersion);
  useEffect(() => { if (open && !error && !submitting.current) setCurrentVersion(vaultVersion); }, [open, vaultVersion]);
  const submitMutation = {
    isPending: isSubmitting,
    mutate: async () => {
      if (submitting.current || isNativeClosePending()) return;
      submitting.current = true; setSubmitting(true); setError(null);
      try {
        if (!value.trim()) throw new Error("密钥值不能为空");
        const result = await secretsApi.save({
          short_name: shortName, expected_revision: 0, expected_vault_version: currentVersion,
          value, display_name: displayName.trim(), expires_at: localDateToExpiry(expiresAt),
          service_id: selectedService?.tier === 1 ? selectedService.meta.suggestedShortName || selectedService.key : "",
          rotate_url: selectedService?.meta.suggestedRotateUrl || "", expiry_policy: "warn", operation: "save",
        });
        if (!result.verified) throw new Error("服务未确认写后校验，请保留草稿并刷新核对");
        setValue("");
        refreshCapabilityCatalog(queryClient);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["secrets", "list"] }),
          queryClient.invalidateQueries({ queryKey: ["model-capabilities", "catalog"] }),
          queryClient.invalidateQueries({ queryKey: ["secrets", "lifecycle"] }),
        ]);
        onDismiss();
      } catch (error) { setError(error instanceof Error ? error.message : String(error)); }
      finally { submitting.current = false; setSubmitting(false); }
    },
  };
  const onClose = () => {
    if (submitting.current) return;
    if (hasDraft && !window.confirm("关闭会丢弃尚未保存的密钥草稿，是否继续？")) return;
    onDismiss();
  };

  const handleSelectService = (svc: SelectedService) => {
    setSelectedService(svc);
    // Tier 1：预填官方短名 + 预填 displayName。Tier 2：预填 displayName，短名留空让用户自己起
    setShortName(svc.tier === 1 ? svc.meta.suggestedShortName ?? svc.key : "");
    setDisplayName(svc.meta.displayName);
    setStep("fill");
  };

  const handleCustom = () => {
    setSelectedService(null);
    setShortName("");
    setDisplayName("");
    setStep("fill");
  };

  const handleBack = () => {
    if (submitting.current) return;
    if ((value || expiresAt) && !window.confirm("返回选择服务会丢弃当前密钥草稿，是否继续？")) return;
    setStep("select");
    setSelectedService(null);
    setShortName("");
    setDisplayName("");
    setValue("");
    setExpiresAt("");
    setShowValue(false);
    setError(null);
  };

  const handlePaste = async () => {
    try {
      const v = await readClipboard();
      if (v) {
        setValue(v);
        setError(null);
      }
    } catch (e) {
      setError(`读剪贴板失败：${e}`);
    }
  };

  const canSubmit =
    !!shortName && !!value && !nameError && !submitMutation.isPending;

  if (!open) return null;
  const titleId = "add-secret-title";
  const isPending = submitMutation.isPending;

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 backdrop-blur-sm"
      onClick={onClose}
      role="dialog"
      aria-modal="true"
      aria-labelledby={titleId}
    >
      <div
        className={cn(
          "flex w-full flex-col rounded-xl border border-border bg-card shadow-2xl",
          step === "select" ? "max-w-3xl max-h-[85vh]" : "max-w-md",
        )}
        onClick={(e) => e.stopPropagation()}
      >
        {/* Modal header */}
        <div className="flex items-center justify-between border-b border-border/60 px-6 py-4">
          <div className="flex items-center gap-2">
            {step === "fill" && (
              <button
                type="button"
                className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                onClick={handleBack}
                aria-label="返回选择服务"
                disabled={isPending}
              >
                <ArrowLeft className="h-4 w-4" />
              </button>
            )}
            <h2 id={titleId} className="font-serif text-lg font-semibold">
              {step === "select" ? "选择服务" : "添加密钥"}
            </h2>
            {step === "fill" && selectedService && (
              <span className="ml-1 text-xs text-muted-foreground">
                · {selectedService.meta.displayName}
              </span>
            )}
          </div>
          <button
            type="button"
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            onClick={onClose}
            aria-label="关闭"
            disabled={isPending}
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        {/* Step 内容 */}
        {error && <div className="px-6 py-2"><button type="button" className="hb-btn-outline text-xs" disabled={isPending} onClick={async () => {
          try { const latest = await secretsApi.list(); if (!latest[0]) throw new Error("无法取得最新库版本"); setCurrentVersion(latest[0].vault_version); setError("已刷新服务器版本，草稿保留；请核对后再次点击添加"); } catch (error) { setError(String(error)); }
        }}>刷新服务器版本，保留新增草稿</button></div>}
        {step === "select" ? (
          <ServiceSelectorStep
            existingNames={existingNames}
            onSelect={handleSelectService}
            onCustom={handleCustom}
          />
        ) : (
          <FieldsFormStep
            selectedService={selectedService}
            shortName={shortName}
            onShortNameChange={setShortName}
            displayName={displayName}
            onDisplayNameChange={setDisplayName}
            value={value}
            onValueChange={setValue}
            expiresAt={expiresAt}
            onExpiresAtChange={setExpiresAt}
            showValue={showValue}
            onToggleShowValue={() => setShowValue((v) => !v)}
            onPaste={handlePaste}
            nameError={nameError}
            error={error}
            isPending={isPending}
            canSubmit={canSubmit}
            onCancel={onClose}
            onSubmit={() => submitMutation.mutate()}
          />
        )}
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────
// Step 1 — 服务选择器
// ─────────────────────────────────────────────────────────────────────

interface ServiceSelectorStepProps {
  existingNames: string[];
  onSelect: (svc: SelectedService) => void;
  onCustom: () => void;
}

function ServiceSelectorStep({
  existingNames,
  onSelect,
  onCustom,
}: ServiceSelectorStepProps) {
  const [search, setSearch] = useState("");
  const allServices = useMemo(() => listKnownServices(), []);
  const q = search.trim().toLowerCase();
  const filtered = q
    ? allServices.filter((s) =>
        [s.key, s.meta.displayName, s.meta.description ?? ""]
          .join(" ")
          .toLowerCase()
          .includes(q),
      )
    : allServices;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* 搜索框 */}
      <div className="border-b border-border/60 px-6 py-3">
        <div className="relative">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜索服务名 / 短名 / 用途…"
            className="w-full rounded-lg border border-border bg-background py-2 pl-9 pr-3 text-sm transition-colors placeholder:text-muted-foreground focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
            autoFocus
            spellCheck={false}
            autoComplete="off"
          />
        </div>
      </div>

      {/* Grid */}
      <div className="grid min-h-0 flex-1 grid-cols-2 gap-3 overflow-y-auto px-6 py-4 sm:grid-cols-3">
        {filtered.map(({ key, meta, tier }) => {
          // 只有 Tier 1 才检查 "已加"；Tier 2 的 key 是建议占位，用户实际短名任意
          const added = tier === 1 && existingNames.includes(key);
          return (
            <button
              key={`${tier}-${key}`}
              type="button"
              disabled={added}
              onClick={() => onSelect({ key, meta, tier })}
              className={cn(
                "group flex flex-col items-start gap-2 rounded-lg border border-border bg-background p-3 text-left transition-all",
                added
                  ? "cursor-not-allowed opacity-50"
                  : "hover:-translate-y-0.5 hover:border-primary-500/50 hover:bg-muted/40 hover:shadow-md",
              )}
              title={added ? "已添加" : meta.description ?? meta.displayName}
            >
              <div className="flex w-full items-center gap-2">
                <ServiceLogo shortName={key} size={28} />
                <span className="truncate font-serif text-sm font-semibold">
                  {meta.displayName}
                </span>
                {added ? (
                  <span className="ml-auto shrink-0 rounded bg-success/10 px-1.5 py-0.5 font-mono text-[10px] font-semibold text-success-strong">
                    已加
                  </span>
                ) : tier === 2 ? (
                  <span
                    className="ml-auto shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] font-medium text-muted-foreground"
                    title="点击后需自定义短名"
                  >
                    自定义
                  </span>
                ) : null}
              </div>
              {meta.description && (
                <p className="line-clamp-2 text-[11px] text-muted-foreground">
                  {meta.description}
                </p>
              )}
            </button>
          );
        })}
        {filtered.length === 0 && (
          <p className="col-span-full py-8 text-center text-sm text-muted-foreground">
            没有匹配的服务，可以点下方「自定义」直接添加
          </p>
        )}
      </div>

      {/* 自定义入口 */}
      <div className="flex items-center justify-between border-t border-border/60 px-6 py-3">
        <p className="text-xs text-muted-foreground">
          列表里没有？支持自定义短名 + 任意 token
        </p>
        <button
          type="button"
          className="hb-btn-outline inline-flex items-center gap-1.5 px-3 py-1.5 text-xs"
          onClick={onCustom}
        >
          <Sparkles className="h-3.5 w-3.5" />
          自定义服务
        </button>
      </div>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────────
// Step 2 — 字段表单（预填或空白）
// ─────────────────────────────────────────────────────────────────────

interface FieldsFormStepProps {
  selectedService: SelectedService | null;
  shortName: string;
  onShortNameChange: (v: string) => void;
  value: string;
  onValueChange: (v: string) => void;
  expiresAt: string;
  onExpiresAtChange: (v: string) => void;
  /** 用户可调的显示名（写入 secrets-meta.json 的 display_name） */
  displayName: string;
  onDisplayNameChange: (v: string) => void;
  showValue: boolean;
  onToggleShowValue: () => void;
  onPaste: () => void;
  nameError: string | null;
  error: string | null;
  isPending: boolean;
  canSubmit: boolean;
  onCancel: () => void;
  onSubmit: () => void;
}

function FieldsFormStep({
  selectedService,
  shortName,
  onShortNameChange,
  displayName,
  onDisplayNameChange,
  value,
  onValueChange,
  expiresAt,
  onExpiresAtChange,
  showValue,
  onToggleShowValue,
  onPaste,
  nameError,
  error,
  isPending,
  canSubmit,
  onCancel,
  onSubmit,
}: FieldsFormStepProps) {
  // 根据 tier 判断短名 placeholder：
  // - Tier 1 已选：预填了官方短名，placeholder 可略
  // - Tier 2 已选：提示以 service.key 为参考
  // - 自定义（无 selectedService）：默认 placeholder
  const shortNamePlaceholder =
    selectedService?.tier === 2
      ? `如：${selectedService.key}、my_${selectedService.meta.displayName.toLowerCase().replace(/[^a-z0-9]+/g, "_")}_token`
      : selectedService?.tier === 1
        ? selectedService.meta.suggestedShortName ?? selectedService.key
        : "例如：openrouter_key、my_new_token";
  const onRotateClick = async () => {
    const url = selectedService?.meta.suggestedRotateUrl;
    if (!url) return;
    try {
      await openExternal(url);
    } catch (e) {
      console.error("打开浏览器失败：", e);
    }
  };

  return (
    <div className="flex flex-col px-6 py-5">
      {/* 服务卡片 — 已选服务展示 */}
      {selectedService && (
        <div className="mb-4 flex items-start gap-3 rounded-lg border border-primary-500/30 bg-primary-500/5 p-3">
          <ServiceLogo shortName={selectedService.key} size={36} />
          <div className="min-w-0 flex-1">
            <p className="font-serif text-sm font-semibold">
              {selectedService.meta.displayName}
            </p>
            {selectedService.meta.description && (
              <p className="mt-0.5 text-xs text-muted-foreground">
                {selectedService.meta.description}
              </p>
            )}
            {selectedService.meta.suggestedLongName && (
              <p className="mt-1 font-mono text-[11px] text-muted-foreground">
                env: {selectedService.meta.suggestedLongName}
              </p>
            )}
          </div>
          {selectedService.meta.suggestedRotateUrl && (
            <button
              type="button"
              className="shrink-0 self-start rounded border border-border bg-background px-2 py-1 text-[11px] font-medium transition-colors hover:bg-muted"
              onClick={onRotateClick}
              title={`打开 ${selectedService.meta.suggestedRotateUrl}`}
            >
              获取 token ↗
            </button>
          )}
        </div>
      )}

      <div className="space-y-4">
        {/* 显示名（可选）— 用于 SecretRow 卡片主标题 */}
        <div>
          <label
            htmlFor="add-secret-display"
            className="mb-1 block text-xs font-medium uppercase tracking-wider text-muted-foreground"
          >
            显示名{" "}
            <span className="font-normal lowercase tracking-normal text-muted-foreground">
              （列表卡片的主标题）
            </span>
          </label>
          <input
            id="add-secret-display"
            type="text"
            value={displayName}
            onChange={(e) => onDisplayNameChange(e.target.value)}
            placeholder="如：GitHub 备用号、生产环境 Stripe…"
            className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm transition-colors placeholder:text-muted-foreground focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
            spellCheck={false}
            autoComplete="off"
          />
          <p className="mt-1 text-xs text-muted-foreground">
            {selectedService
              ? `预填为「${selectedService.meta.displayName}」，可改为自定义名称`
              : "留空则后续按短名启发式生成"}
          </p>
        </div>

        {/* 短名 — 存储 / 引用 的 unique key */}
        <div>
          <label
            htmlFor="add-secret-name"
            className="mb-1 block text-xs font-medium uppercase tracking-wider text-muted-foreground"
          >
            短名 <span className="text-destructive">*</span>
          </label>
          <input
            id="add-secret-name"
            type="text"
            value={shortName}
            onChange={(e) => onShortNameChange(e.target.value)}
            placeholder={shortNamePlaceholder}
            className="w-full rounded-lg border border-border bg-background px-3 py-2 font-mono text-sm transition-colors placeholder:text-muted-foreground focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
            autoFocus={!selectedService || selectedService.tier === 2}
            spellCheck={false}
            autoComplete="off"
          />
          {nameError ? (
            <p className="mt-1 text-xs text-destructive-strong">{nameError}</p>
          ) : (
            <p className="mt-1 text-xs text-muted-foreground">
              {selectedService?.tier === 1
                ? "已预填官方短名，可改为自定义"
                : selectedService?.tier === 2
                  ? "请为该服务起个唯一短名（小写 a-z 0-9 _ · 3-64 字符）"
                  : "小写字母开头 · 仅 a-z 0-9 _ · 3-64 字符"}
            </p>
          )}
        </div>

        {/* Value */}
        <div>
          <label
            htmlFor="add-secret-value"
            className="mb-1 block text-xs font-medium uppercase tracking-wider text-muted-foreground"
          >
            密钥值 <span className="text-destructive">*</span>
          </label>
          <div className="relative">
            <input
              id="add-secret-value"
              type={showValue ? "text" : "password"}
              value={value}
              onChange={(e) => onValueChange(e.target.value)}
              placeholder="粘贴 token 或在此输入..."
              className="w-full rounded-lg border border-border bg-background px-3 py-2 pr-20 font-mono text-sm transition-colors placeholder:text-muted-foreground focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
              spellCheck={false}
              autoComplete="off"
              autoFocus={!!selectedService}
            />
            <div className="absolute inset-y-0 right-1 flex items-center gap-0.5">
              <button
                type="button"
                className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                onClick={onToggleShowValue}
                aria-label={showValue ? "隐藏密钥值" : "显示密钥值"}
                title={showValue ? "隐藏" : "显示"}
              >
                {showValue ? (
                  <EyeOff className="h-4 w-4" />
                ) : (
                  <Eye className="h-4 w-4" />
                )}
              </button>
              <button
                type="button"
                className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                onClick={onPaste}
                aria-label="从剪贴板粘贴"
                title="从剪贴板粘贴"
              >
                <ClipboardPaste className="h-4 w-4" />
              </button>
            </div>
          </div>
          {value && (
            <p className="mt-1 text-xs text-muted-foreground">
              {value.length} 字符 · 写入前会先 backup secrets.yaml
            </p>
          )}
        </div>

        {/* expires_at */}
        <div>
          <label
            htmlFor="add-secret-expires"
            className="mb-1 block text-xs font-medium uppercase tracking-wider text-muted-foreground"
          >
            过期日期{" "}
            <span className="font-normal lowercase tracking-normal text-muted-foreground">
              （可选）
            </span>
          </label>
          <input
            id="add-secret-expires"
            type="date"
            value={expiresAt}
            onChange={(e) => onExpiresAtChange(e.target.value)}
            className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm transition-colors focus:border-primary-500 focus:outline-none focus:ring-2 focus:ring-primary-500/30"
          />
          <p className="mt-1 text-xs text-muted-foreground">
            用于过期警告（写入 secrets-meta.json）
          </p>
        </div>
      </div>

      {error && (
        <div className="mt-4 flex items-start gap-2 rounded-md border border-destructive/50 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
          <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
          <span className="break-words">{error}</span>
        </div>
      )}

      <div className="mt-6 flex justify-end gap-2">
        <button
          type="button"
          className="hb-btn-outline"
          onClick={onCancel}
          disabled={isPending}
        >
          取消
        </button>
        <button
          type="button"
          className="hb-btn-primary"
          disabled={!canSubmit}
          onClick={onSubmit}
        >
          {isPending ? "写入中…" : "添加"}
        </button>
      </div>
    </div>
  );
}


function SecretsSkeleton() {
  return (
    <ul className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,260px),1fr))] gap-4" aria-label="正在加载密钥" aria-busy="true">
      {Array.from({ length: 6 }).map((_, i) => (
        <li key={i} className="hb-card min-h-[176px] space-y-4">
          <div className="flex items-start gap-3">
            <div className="h-8 w-8 shrink-0 animate-pulse rounded-lg bg-muted" />
            <div className="min-w-0 flex-1 space-y-2">
              <div className="h-4 w-3/4 animate-pulse rounded bg-muted" />
              <div className="h-3 w-1/2 animate-pulse rounded bg-muted" />
              <div className="h-3 w-2/3 animate-pulse rounded bg-muted" />
            </div>
          </div>
          <div className="h-3 w-3/4 animate-pulse rounded bg-muted" />
        </li>
      ))}
    </ul>
  );
}
