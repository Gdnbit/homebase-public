import { useEffect, useRef, useState } from "react";
import { DevelopmentCapability } from "@/components/development-capability";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import {
  HandoffStatus,
  useAssistantHandoff,
} from "@/components/workflow-assistant-entry";
import { Box, ArrowRight, RefreshCw, Search, X } from "lucide-react";
import {
  capabilityCatalogKey,
  queryCapabilities,
  type Implementation,
} from "@/lib/capability-index";
import {
  CapabilityEvidence,
  CapabilityPricing,
} from "@/components/capability-facts";
import {
  capabilityComparisonHandoff,
  capabilityHandoff,
  capabilityFunctionNames as names,
  MAX_CAPABILITY_COMPARISON,
} from "@/lib/capability-handoff";
const kinds: Record<string, string> = {
  model: "模型与服务",
  service: "服务",
  repository: "开源项目",
  local: "本地能力",
  mcp: "MCP 工具",
  skill: "Skill 工作流",
};
const states: Record<string, string> = {
  verified: "有验收登记",
  check_before_call: "调用前检查",
  authorized_not_probed: "已授权 · 待检查连接",
  authorization_required: "待授权",
  secure_reference_required: "需要更换为凭据引用",
  blocked_secret_export: "助理不可使用",
  disabled: "本机工具已停用",
  instructions_available: "说明可用 · 运行前检查",
  incomplete: "资料不完整",
  unknown: "可用性待核对",
  available: "目录已接入",
  unverified: "待核验",
  not_verified: "账号调用权限待验证",
  public_catalog_not_entitlement: "公共目录 · 账号权限待验证",
  listed_not_inference_verified: "目录已收录 · 尚未实测调用",
  configured: "已配置 · 调用前核对",
  not_configured: "尚未配置",
  expired: "已到期 · 核对原到期策略",
  blocked: "已阻断 · 检查原凭据策略",
  linked: "已关联 · 调用前核对",
  invalid: "配置或到期日期无效",
};
const costs: Record<Implementation["cost"], string> = {
  local_compute: "本机算力成本",
  depends_on_tools: "成本取决于使用的工具",
  unknown: "调用费用需核对",
};
function implementationKey(entry: Implementation) {
  return JSON.stringify([
    entry.entry_key ?? null,
    entry.kind,
    entry.id,
    entry.provider,
    entry.target,
  ]);
}

function catalogHasExpired(expiresAt: string) {
  const deadline = Date.parse(expiresAt);
  return !Number.isFinite(deadline) || deadline <= Date.now();
}

export function FunctionalCapabilities() {
  const [search, setSearch] = useState("");
  const [query, setQuery] = useState("");
  const [selected, select] = useState<string>();
  const [expiredCatalog, setExpiredCatalog] = useState<string>();
  const [comparison, setComparison] = useState<{
    scope: string;
    keys: string[];
  }>();
  const dialog = useRef<HTMLDialogElement>(null);
  const comparisonTable = useRef<HTMLDivElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const index = useQuery({
    queryKey: capabilityCatalogKey,
    queryFn: () => queryCapabilities({ limit: 1 }),
    staleTime: 60_000,
  });
  useEffect(() => {
    const timer = setTimeout(() => setQuery(search.trim()), 300);
    return () => clearTimeout(timer);
  }, [search]);
  const catalogId = index.data?.catalog_id;
  const expiresAt = index.data?.expires_at;
  const selectionExpired =
    !!catalogId &&
    (expiredCatalog === catalogId || catalogHasExpired(expiresAt ?? ""));
  useEffect(() => {
    if (!selected || !catalogId || !expiresAt || selectionExpired) return;
    const deadline = Date.parse(expiresAt);
    let timer: ReturnType<typeof setTimeout>;
    function checkExpiry() {
      clearTimeout(timer);
      const delay = deadline - Date.now();
      if (delay > 0) timer = setTimeout(checkExpiry, delay);
      else setExpiredCatalog(catalogId);
    }
    checkExpiry();
    window.addEventListener("focus", checkExpiry);
    document.addEventListener("visibilitychange", checkExpiry);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("focus", checkExpiry);
      document.removeEventListener("visibilitychange", checkExpiry);
    };
  }, [catalogId, expiresAt, selected, selectionExpired]);
  const catalogReady = !!catalogId && !index.isFetching && !index.isError;
  const queryTooLong = new TextEncoder().encode(query).length > 240;
  const filtered = useQuery({
    queryKey: [...capabilityCatalogKey, "search", catalogId, query],
    queryFn: () =>
      queryCapabilities({ catalog_id: catalogId, query, limit: 1 }),
    enabled: catalogReady && !!query && !queryTooLong,
    // A catalog is immutable. Only a new catalog ID may change these results.
    staleTime: Infinity,
  });
  const browse = query ? filtered : index;
  const implementations = useInfiniteQuery({
    queryKey: [
      ...capabilityCatalogKey,
      "implementations",
      catalogId,
      query,
      selected,
    ],
    initialPageParam: {
      offset: 0,
      snapshot_id: undefined as string | undefined,
      total: undefined as number | undefined,
    },
    queryFn: async ({ pageParam: { offset, snapshot_id, total } }) => {
      const page = await queryCapabilities({
        catalog_id: catalogId,
        query,
        function: selected,
        offset,
        snapshot_id,
      });
      if (total !== undefined && page.total !== total)
        throw Error("同一快照的实现数量发生变化，请刷新能力目录。");
      return page;
    },
    getNextPageParam: (page) =>
      page.next_offset === null
        ? undefined
        : {
            offset: page.next_offset,
            snapshot_id: page.snapshot_id,
            total: page.total,
          },
    enabled: catalogReady && !!selected,
    staleTime: Infinity,
  });
  useEffect(() => {
    if (selected) dialog.current?.showModal();
    else dialog.current?.close();
  }, [selected]);
  const entries =
    implementations.data?.pages.flatMap((page) =>
      page.entries.map((entry) => ({
        entry,
        key: implementationKey(entry),
        offset: page.offset,
      })),
    ) ?? [];
  const selectionPage = implementations.data?.pages[0];
  const selectionError = index.error || browse.error || implementations.error;
  const selectionPending =
    index.isFetching || browse.isFetching || implementations.isFetching;
  const canPrepare =
    !!selectionPage?.total &&
    !selectionPending &&
    !selectionError &&
    !selectionExpired;
  const comparisonScope = JSON.stringify([
    catalogId,
    selectionPage?.snapshot_id,
    query,
    selected,
  ]);
  const comparedKeys =
    comparison?.scope === comparisonScope ? comparison.keys : [];
  const compared = comparedKeys.flatMap((key) => {
    const match = entries.find((item) => item.key === key);
    return match ? [match] : [];
  });
  const groups = queryTooLong ? [] : (browse.data?.groups ?? []);
  function close() {
    setComparison(undefined);
    select(undefined);
  }
  function toggleComparison(key: string) {
    if (!canPrepare || catalogHasExpired(expiresAt ?? "")) return;
    setComparison((current) => {
      const keys = current?.scope === comparisonScope ? current.keys : [];
      if (keys.includes(key))
        return {
          scope: comparisonScope,
          keys: keys.filter((value) => value !== key),
        };
      if (keys.length >= MAX_CAPABILITY_COMPARISON) return current;
      return { scope: comparisonScope, keys: [...keys, key] };
    });
  }
  async function choose(id: string) {
    if (selectionExpired) {
      const fresh = await index.refetch({ cancelRefetch: false });
      if (fresh.isError) return;
    }
    select(id);
  }
  function draft(e?: Implementation) {
    requireCurrentSelection();
    return capabilityHandoff(selected, selectionPage, e);
  }
  function requireCurrentSelection() {
    if (selectionExpired || catalogHasExpired(expiresAt ?? ""))
      throw Error("能力目录已过期，请刷新后重新选择。");
    if (!canPrepare) throw Error("能力资料尚未核对，请刷新后再选择。");
  }
  return (
    <section className="space-y-5" aria-label="按功能发现能力">
      <div className="flex items-center gap-3">
        <label className="flex min-w-0 flex-1 items-center gap-2 rounded-xl border border-border bg-card px-3">
          <Search size={16} />
          <input
            aria-label="搜索能力"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="w-full bg-transparent py-3 text-sm outline-none"
            placeholder="想完成什么？例如视频、文档、语音…"
          />
        </label>
        <button
          className="hb-btn-ghost"
          aria-label="刷新能力索引"
          onClick={() => void index.refetch({ cancelRefetch: false })}
          disabled={index.isFetching}
        >
          <RefreshCw size={16} />
        </button>
      </div>
      <p className="text-sm text-muted-foreground">
        先选要完成的事，再选择实现；也可以直接告诉助理。已有凭据、项目与工具保持各自的配置。
      </p>
      {!queryTooLong && (index.isPending || browse.isPending) && (
        <p role="status">正在整理已接入能力…</p>
      )}
      {queryTooLong && <p role="alert">关键词过长，请缩短后再搜索。</p>}
      {(index.isError || browse.isError) && (
        <p role="alert">
          能力索引读取失败：{String(index.error || browse.error)}
          。可以刷新能力索引重新读取。
        </p>
      )}
      {browse.data?.groups_has_more && (
        <p className="text-xs text-muted-foreground">
          这里只展示部分功能分组；可输入具体功能继续查找。
        </p>
      )}
      {!!index.data?.issues_total && (
        <details className="rounded-lg border border-border p-3 text-sm">
          <summary>部分来源需要核对（{index.data.issues_total}）</summary>
          <ul className="mt-2 space-y-1 text-muted-foreground">
            {index.data.issues.map((i, n) => (
              <li key={`${i.source}:${n}`}>{i.message}</li>
            ))}
          </ul>
          {index.data.issues_has_more && (
            <p className="mt-2 text-xs text-muted-foreground">
              部分来源说明已省略，请按原来源接口核对完整状态。
            </p>
          )}
        </details>
      )}
      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        {groups.map(({ function: id, total, kinds: variants }) => {
          return (
            <button
              key={id}
              className="group rounded-xl border border-border bg-card p-5 text-left transition-colors hover:border-primary/50"
              onClick={() => void choose(id)}
            >
              <div className="flex items-center justify-between">
                <Box className="text-primary" size={21} />
                <ArrowRight size={16} className="text-muted-foreground" />
              </div>
              <h2 className="mt-5 font-serif text-lg font-semibold">
                {names[id] ?? id}
              </h2>
              <p className="mt-2 text-sm text-muted-foreground">
                查看实现，或交给助理按任务选择
              </p>
              <p className="mt-5 text-xs text-muted-foreground">
                {total} 个实现 ·{" "}
                {variants.map((kind) => kinds[kind] ?? kind).join(" / ")}
              </p>
            </button>
          );
        })}
      </div>
      {!index.isPending &&
        !queryTooLong &&
        !browse.isPending &&
        !index.isError &&
        !browse.isError &&
        !groups.length && (
          <p className="rounded-xl border border-dashed border-border p-10 text-center text-muted-foreground">
            没有符合条件的能力。可以调整搜索，或在实现管理中接入项目。
          </p>
        )}
      <dialog
        ref={dialog}
        onCancel={close}
        className="ml-auto mr-0 h-full max-h-full w-full max-w-2xl border-l border-border bg-card p-0 text-foreground backdrop:bg-black/25"
      >
        <div className="sticky top-0 z-10 flex items-center justify-between gap-3 border-b border-border bg-card p-5">
          <h2 className="min-w-0 break-words font-serif text-xl">
            {names[selected ?? ""] ?? selected}
          </h2>
          <div className="flex shrink-0 items-center gap-3">
            {compared.length >= 2 ? (
              <button
                type="button"
                className="hb-btn-ghost text-sm"
                onClick={() => {
                  comparisonTable.current?.scrollIntoView({ block: "start" });
                  comparisonTable.current?.focus({ preventScroll: true });
                }}
              >
                比较已选（{compared.length}）
              </button>
            ) : (
              <span
                aria-live="polite"
                className="text-xs text-muted-foreground"
              >
                {compared.length === 1 && "已选 1 项，至少选 2 项比较"}
              </span>
            )}
            <button
              type="button"
              ref={closeButton}
              aria-label="关闭能力实现"
              onClick={close}
            >
              <X size={18} />
            </button>
          </div>
        </div>
        <div className="space-y-4 p-5">
          {selected === "development" && <DevelopmentCapability />}
          <CapabilityDraftButton
            contextKey={JSON.stringify([catalogId, query, selected])}
            disabled={!canPrepare}
            className="flex items-center justify-between rounded-xl border border-primary/30 bg-primary/5 p-4 text-sm"
            getText={() => draft()}
          >
            <span>
              <strong className="block">让助理按任务选择</strong>
              <span className="mt-1 block text-muted-foreground">
                先核对能力、成本和授权，再执行
              </span>
            </span>
            <ArrowRight size={16} />
          </CapabilityDraftButton>
          {selectionPending && <p role="status">正在读取实现…</p>}
          {(selectionError || selectionExpired) && (
            <div role="alert" className="text-sm">
              {selectionExpired && <p>能力目录已过期，请刷新后重新选择。</p>}
              {selectionError && (
                <p>
                  实现读取失败：{String(selectionError)}
                  。原目录和配置未修改。
                </p>
              )}
              <button
                className="hb-btn-ghost mt-2"
                disabled={index.isFetching}
                onClick={() => void index.refetch({ cancelRefetch: false })}
              >
                刷新目录并重新读取
              </button>
            </div>
          )}
          {!selectionPending &&
            !selectionError &&
            !selectionExpired &&
            selectionPage?.total === 0 && (
              <p className="text-sm text-muted-foreground">
                当前目录没有符合筛选的实现。
              </p>
            )}
          {compared.length >= 2 && (
            <section className="rounded-xl border border-border">
              <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3">
                <h3 className="text-sm font-semibold">实现对照</h3>
                <div className="flex flex-wrap items-start gap-2">
                  <CapabilityDraftButton
                    disabled={!canPrepare}
                    contextKey={JSON.stringify([comparisonScope, comparedKeys])}
                    getText={() => {
                      requireCurrentSelection();
                      return capabilityComparisonHandoff(
                        selectionPage,
                        compared,
                      );
                    }}
                    className="hb-btn-ghost text-xs text-primary"
                  >
                    交给助理比较
                  </CapabilityDraftButton>
                  <button
                    type="button"
                    className="hb-btn-ghost text-xs"
                    onClick={() => {
                      setComparison(undefined);
                      closeButton.current?.focus({ preventScroll: true });
                    }}
                  >
                    清空比较
                  </button>
                </div>
              </div>
              <div
                className="scroll-mt-20 overflow-x-auto rounded-b-xl focus-visible:outline focus-visible:outline-primary"
                ref={comparisonTable}
                role="region"
                aria-label="横向滚动比较表"
                tabIndex={0}
              >
                <CapabilityComparison
                  entries={compared.map(({ entry }) => entry)}
                  renderAction={(e) => (
                    <CapabilityDraftButton
                      disabled={!canPrepare}
                      contextKey={JSON.stringify([
                        comparisonScope,
                        implementationKey(e),
                      ])}
                      getText={() => draft(e)}
                      className="text-left text-sm text-primary disabled:opacity-50"
                      label={`准备任务 · ${e.name}`}
                    >
                      准备任务
                    </CapabilityDraftButton>
                  )}
                />
              </div>
            </section>
          )}
          {entries.map(({ entry: e, key }) => (
            <article key={key} className="rounded-xl border border-border p-4">
              <div className="flex items-start justify-between gap-3">
                <p className="min-w-0 break-all text-xs text-muted-foreground">
                  {kinds[e.kind]} · {e.provider}
                </p>
                {(selectionPage?.total ?? 0) > 1 && (
                  <button
                    type="button"
                    className="shrink-0 rounded-md border border-border px-2 py-1 text-xs text-muted-foreground hover:border-primary/50 aria-pressed:border-primary/50 aria-pressed:bg-primary/10 aria-pressed:text-primary disabled:opacity-50"
                    aria-label={`${e.name} · 比较`}
                    aria-pressed={comparedKeys.includes(key)}
                    disabled={
                      !canPrepare ||
                      (comparedKeys.length >= MAX_CAPABILITY_COMPARISON &&
                        !comparedKeys.includes(key))
                    }
                    onClick={() => toggleComparison(key)}
                  >
                    比较
                  </button>
                )}
              </div>
              <h3 className="mt-2 break-words font-medium">{e.name}</h3>
              <p className="mt-2 text-xs text-muted-foreground">
                {states[e.state] ?? "可用性待核对"} ·{" "}
                {costs[e.cost] ?? costs.unknown}
              </p>
              {e.kind === "model" && e.pricing && (
                <CapabilityPricing pricing={e.pricing} routeId={e.target} />
              )}
              <CapabilityEvidence provenance={e.provenance} />
              <details className="mt-3 break-words text-xs text-muted-foreground">
                <summary>运行条件与依据</summary>
                <p className="mt-2 break-all">接口说明：{e.discover}</p>
                {e.credential_binding && (
                  <p className="mt-2 break-all">
                    凭据引用：{e.credential_binding}
                  </p>
                )}
                <ul className="mt-2 space-y-2">
                  {e.prerequisites?.map((text, n) => (
                    <li key={n}>{text}</li>
                  ))}
                </ul>
                {e.evidence && <p className="mt-2">{e.evidence}</p>}
                {e.summary_truncated && (
                  <p className="mt-2">
                    说明已缩略，执行前按原接口读取完整契约。
                  </p>
                )}
              </details>
              <CapabilityDraftButton
                disabled={!canPrepare}
                contextKey={JSON.stringify([
                  catalogId,
                  e.entry_key,
                  e.kind,
                  e.function,
                  e.provider,
                  e.id,
                  e.target,
                ])}
                getText={() => draft(e)}
                className="mt-4 inline-flex items-center gap-2 text-sm text-primary"
              >
                使用这个实现准备任务
                <ArrowRight size={14} />
              </CapabilityDraftButton>
            </article>
          ))}
          {implementations.hasNextPage && (
            <button
              className="hb-btn-ghost"
              disabled={
                selectionPending || !!selectionError || selectionExpired
              }
              onClick={() => {
                if (!catalogHasExpired(expiresAt ?? ""))
                  void implementations.fetchNextPage({ cancelRefetch: false });
              }}
            >
              {implementations.isFetchingNextPage
                ? "正在读取…"
                : "显示更多实现"}
            </button>
          )}
          <p className="text-xs leading-6 text-muted-foreground">
            选择只准备对话草稿。模型目录收录不代表账号一定可调用；MCP 与 Skill
            先检查授权和实际契约，不能仅凭名称认为能力相同。
          </p>
        </div>
      </dialog>
    </section>
  );
}

function CapabilityComparison({
  entries,
  renderAction,
}: {
  entries: Implementation[];
  renderAction: (entry: Implementation) => React.ReactNode;
}) {
  const rows: {
    label: string;
    fact: (entry: Implementation) => React.ReactNode;
  }[] = [
    { label: "来源与类型", fact: (e) => `${kinds[e.kind]} · ${e.provider}` },
    { label: "可用状态", fact: (e) => states[e.state] ?? "可用性待核对" },
    {
      label: "费用",
      fact: (e) => (
        <>
          <p>{costs[e.cost] ?? costs.unknown}</p>
          {e.kind === "model" && (
            <CapabilityPricing pricing={e.pricing} routeId={e.target} />
          )}
        </>
      ),
    },
    {
      label: "凭据与前置条件",
      fact: (e) => (
        <>
          <p className="break-all">
            {e.credential_binding
              ? `凭据引用：${e.credential_binding}`
              : "目录未提供凭据引用"}
          </p>
          {e.prerequisites?.length ? (
            <ul className="mt-2 list-disc space-y-1 pl-4">
              {e.prerequisites.map((text, index) => (
                <li key={index}>{text}</li>
              ))}
            </ul>
          ) : (
            <p className="mt-2">目录未提供前置条件</p>
          )}
        </>
      ),
    },
    {
      label: "验收依据",
      fact: (e) => (
        <>
          <p>{e.evidence || "目录未提供验收说明"}</p>
          <CapabilityEvidence provenance={e.provenance} />
          <p className="mt-2 break-all">接口说明：{e.discover}</p>
          {e.summary_truncated && (
            <p className="mt-2">说明已缩略，执行前核对完整契约。</p>
          )}
        </>
      ),
    },
    { label: "使用实现", fact: renderAction },
  ];
  return (
    <table
      className="w-full table-fixed border-collapse text-left text-xs leading-relaxed"
      style={{ minWidth: 100 + 176 * entries.length }}
    >
      <caption className="sr-only">比较实现</caption>
      <thead>
        <tr className="border-y border-border">
          <th scope="col" className="w-24 bg-secondary/40 px-3 py-2">
            对照项
          </th>
          {entries.map((entry) => (
            <th
              key={implementationKey(entry)}
              scope="col"
              className="break-words border-l border-border px-3 py-2"
            >
              {entry.name}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map(({ label, fact }) => (
          <tr key={label} className="border-b border-border last:border-0">
            <th
              scope="row"
              className="bg-secondary/40 px-3 py-2 align-top font-medium"
            >
              {label}
            </th>
            {entries.map((entry) => (
              <td
                key={implementationKey(entry)}
                className="break-words border-l border-border px-3 py-2 align-top text-muted-foreground"
              >
                {fact(entry)}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function CapabilityDraftButton({
  getText,
  contextKey,
  className,
  children,
  disabled,
  label,
}: {
  getText: () => string;
  contextKey: string;
  className: string;
  children: React.ReactNode;
  disabled: boolean;
  label?: string;
}) {
  const handoff = useAssistantHandoff(getText, contextKey);
  return (
    <div>
      <button
        type="button"
        className={className}
        aria-label={label}
        disabled={disabled || handoff.busy}
        onClick={() => void handoff.prepare()}
      >
        {handoff.busy ? "正在保存草稿…" : children}
      </button>
      <HandoffStatus handoff={handoff} />
    </div>
  );
}
