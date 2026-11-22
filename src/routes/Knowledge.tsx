import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { useIsFetching, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertCircle,
  BookOpen,
  Bot,
  Check,
  ChevronRight,
  Copy,
  ExternalLink,
  FileText,
  Loader2,
  Network,
  Search,
  Sparkles,
  Star,
  X,
} from "lucide-react";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import {
  docsApi,
  type DocInfo,
  type ObsidianHealth,
  type ObsidianVariant,
} from "@/lib/tauri-api";
import { EmptyPlaceholder } from "@/components/empty-placeholder";
import { MarkdownRenderer } from "@/components/markdown-renderer";
const LLMWikiTab = lazy(() => import("@/components/knowledge/llm-wiki-tab").then(m => ({ default: m.LLMWikiTab })));
const AIKnowledgeGraph = lazy(() => import("@/components/knowledge/ai-knowledge-graph").then(m => ({ default: m.AIKnowledgeGraph })));
import { cn } from "@/lib/utils";

type KnowledgeTab = "wiki" | "ai";
type AIView = "graph" | "list";

/**
 * 知识库（Phase 14.5 双 Tab 重构）
 *
 * - **LLM Wiki Tab（默认）**：扫 C:\HomeBase\Wiki，给人看的个人笔记（Karpathy 模式）
 * - **AI 知识库 Tab**：解析 INDEX.md，给 AI 看的整机操作索引；默认显示**逻辑框图**（React Flow）
 */
export function Knowledge() {
  const client = useQueryClient();
  const loading = useIsFetching({ predicate: query => ["wiki", "docs", "topology", "obsidian"].includes(String(query.queryKey[0])) });
  const [tab, setTab] = useState<KnowledgeTab>("wiki");

  return (
    <div className="animate-fade-in">
      <header className="hb-page-header mb-4">
        <span className="hb-section-label">Knowledge</span>
        <h1>知识库</h1>
        <p>
          双重知识库：左 Tab 给人看（LLM Wiki / C:\HomeBase\Wiki），右 Tab 给 AI 看（INDEX.md 整机索引 + 逻辑框图）。
        </p>
        <button type="button" className="hb-btn-outline mt-3" disabled={loading > 0}
          onClick={() => void Promise.all(["wiki", "docs", "topology", "obsidian"].map(key => client.invalidateQueries({ queryKey: [key] })))}>
          {loading > 0 ? "正在读取知识库…" : "刷新知识库"}
        </button>
      </header>

      {/* Tab 切换 */}
      <div className="mb-4 flex items-center gap-1 border-b border-border">
        <TabButton
          active={tab === "wiki"}
          onClick={() => setTab("wiki")}
          icon={<BookOpen className="h-4 w-4" />}
          label="LLM Wiki"
          hint="个人笔记（C:\\HomeBase\\Wiki）"
        />
        <TabButton
          active={tab === "ai"}
          onClick={() => setTab("ai")}
          icon={<Bot className="h-4 w-4" />}
          label="AI 知识库"
          hint="整机索引（INDEX.md）"
        />
      </div>

      <Suspense fallback={<p role="status">正在加载知识视图…</p>}>
        {tab === "wiki" ? <LLMWikiTab /> : <AIKnowledgeView />}
      </Suspense>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  icon,
  label,
  hint,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
  hint?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "group relative -mb-px flex items-center gap-2 px-4 py-2.5 text-sm font-medium transition-colors",
        active
          ? "border-b-2 border-primary-500 text-foreground"
          : "border-b-2 border-transparent text-muted-foreground hover:text-foreground",
      )}
      aria-pressed={active}
    >
      {icon}
      <span>{label}</span>
      {hint && (
        <span className="hidden font-mono text-[10px] text-muted-foreground/70 sm:inline">
          {hint}
        </span>
      )}
    </button>
  );
}

// =====================================================================
// AI 知识库 Tab（INDEX.md，默认显示节点图）
// =====================================================================

function AIKnowledgeView() {
  const [view, setView] = useState<AIView>("graph");
  const [selectedDocId, setSelectedDocId] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  const docs = useQuery({
    queryKey: ["docs", "list"],
    queryFn: () => docsApi.list(),
    enabled: view === "list",
  });
  const health = useQuery({
    queryKey: ["obsidian", "health"],
    queryFn: () => docsApi.obsidianHealth(),
    enabled: view === "list",
  });

  const filteredDocs = useMemo(() => {
    const all = docs.data ?? [];
    const q = search.trim().toLowerCase();
    if (!q) return all;
    return all.filter((d) => {
      const hay = [d.role ?? "", d.doc_id, d.path].join(" ").toLowerCase();
      return hay.includes(q);
    });
  }, [docs.data, search]);

  const groupedAll = useMemo(
    () => groupDocsByPrefix(docs.data ?? []),
    [docs.data],
  );
  const grouped = useMemo(
    () => search.trim() ? groupDocsByPrefix(filteredDocs) : groupedAll,
    [filteredDocs, groupedAll, search],
  );

  const selectedDoc = useMemo(
    () => docs.data?.find((d) => d.doc_id === selectedDocId) ?? null,
    [docs.data, selectedDocId],
  );

  return (
    <div className="space-y-4">
      {/* 视图切换 + INDEX.md 描述 */}
      <div className="flex items-center justify-between gap-3 rounded-xl border border-border bg-card px-4 py-3">
        <div>
          <p className="text-sm">
            解析{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
              本机操作指南/INDEX.md
            </code>
            <span role="status" aria-label="文档索引状态" className="ml-2 font-mono text-xs text-muted-foreground">
              {docs.isError
                ? `· 索引读取失败${docs.data ? `（保留上次 ${docs.data.length} 篇）` : ""}`
                : docs.data
                  ? `· ${docs.data.length} 篇${docs.isFetching ? " · 刷新中…" : ""}`
                  : docs.isFetching
                    ? "· 正在读取索引…"
                    : "· 切换到列表读取文档"}
            </span>
          </p>
          <p className="mt-0.5 text-xs text-muted-foreground">
            <strong>不扫描任何盘符</strong>，仅显示 INDEX.md 登记的文档。
          </p>
        </div>
        <div className="flex shrink-0 items-center rounded-lg border border-border bg-background p-0.5">
          <ViewToggle
            active={view === "graph"}
            onClick={() => setView("graph")}
            icon={<Network className="h-3.5 w-3.5" />}
            label="逻辑图"
          />
          <ViewToggle
            active={view === "list"}
            onClick={() => setView("list")}
            icon={<FileText className="h-3.5 w-3.5" />}
            label="列表"
          />
        </div>
      </div>

      {view === "list" && docs.isError && (
        <p role="alert" className="rounded-lg border border-danger/30 bg-danger/5 px-4 py-3 text-sm text-danger">
          文档索引读取失败，不能判定为没有文档。{String(docs.error ?? "")}
          {docs.data && " 已保留上次读取的列表。"}请点击“刷新知识库”重试。
        </p>
      )}
      {view === "graph" ? (
        <AIKnowledgeGraph />
      ) : (
        <div className="flex flex-col gap-6 lg:flex-row">
          <aside
            className="w-full shrink-0 lg:sticky lg:top-6 lg:w-[300px] lg:self-start"
            style={{ maxHeight: "calc(100vh - 8rem)" }}
          >
            <KnowledgeSidebar
              docs={filteredDocs}
              grouped={grouped}
              health={health.data ?? null}
              healthError={health.isError}
              isLoading={docs.isLoading}
              readFailed={docs.isError}
              selectedDocId={selectedDocId}
              onSelect={setSelectedDocId}
              search={search}
              onSearchChange={setSearch}
            />
          </aside>

          <main className="min-w-0 flex-1">
            <KnowledgePreview
              selectedDoc={selectedDoc}
              health={health.data ?? null}
              healthLoading={health.isLoading}
              healthError={health.isError ? String(health.error ?? "") : null}
              grouped={groupedAll}
              docsLoading={docs.isLoading}
              onSelectDoc={setSelectedDocId}
            />
          </main>
        </div>
      )}
    </div>
  );
}

function ViewToggle({
  active,
  onClick,
  icon,
  label,
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "inline-flex items-center gap-1.5 rounded px-2.5 py-1 text-xs font-medium transition-colors",
        active
          ? "bg-primary-500 text-white shadow-sm"
          : "text-muted-foreground hover:text-foreground",
      )}
      aria-pressed={active}
    >
      {icon}
      {label}
    </button>
  );
}

// =====================================================================
// Sidebar — 健康徽章 mini + 分组文档列表
// =====================================================================

interface KnowledgeSidebarProps {
  docs: DocInfo[];
  grouped: Record<string, DocInfo[]>;
  health: ObsidianHealth | null;
  healthError: boolean;
  isLoading: boolean;
  readFailed: boolean;
  selectedDocId: string | null;
  onSelect: (id: string | null) => void;
  search: string;
  onSearchChange: (s: string) => void;
}

function KnowledgeSidebar({
  docs,
  grouped,
  health,
  healthError,
  isLoading,
  readFailed,
  selectedDocId,
  onSelect,
  search,
  onSearchChange,
}: KnowledgeSidebarProps) {
  return (
    <div
      className="flex max-h-[60vh] flex-col overflow-hidden rounded-xl border border-border bg-card lg:max-h-full lg:h-full"
    >
      {/* Obsidian 健康徽章（紧凑） */}
      <div className="border-b border-border/60 px-3 py-2">
        {health ? (
          <SidebarHealthBadge health={health} onClick={() => onSelect(null)} />
        ) : (
          <span className="text-xs text-muted-foreground">{healthError ? "健康检查失败，状态未知" : "健康检查加载中…"}</span>
        )}
      </div>

      {/* 搜索框 */}
      <div className="border-b border-border/60 px-2 py-2">
        <div className="relative">
          <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            type="text"
            value={search}
            onChange={(e) => onSearchChange(e.target.value)}
            placeholder="搜索 role / id / 路径"
            className="w-full rounded-md border border-border bg-background py-1.5 pl-7 pr-7 text-xs placeholder:text-muted-foreground/70 focus:border-primary-400 focus:outline-none focus:ring-1 focus:ring-primary-400"
          />
          {search && (
            <button
              type="button"
              onClick={() => onSearchChange("")}
              className="absolute right-1.5 top-1/2 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:bg-muted/40 hover:text-foreground"
              aria-label="清除搜索"
            >
              <X className="h-3 w-3" />
            </button>
          )}
        </div>
      </div>

      {/* 文档分组列表（可滚动） */}
      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
        {isLoading ? (
          <SidebarSkeleton />
        ) : docs.length === 0 ? (
          <div className="px-3 py-8 text-center text-xs text-muted-foreground">
            {readFailed ? "索引读取失败，请刷新重试" : search ? `无匹配 "${search}"` : "索引中暂无登记文档"}
          </div>
        ) : (
          <div className="space-y-1">
            {Object.entries(grouped).map(([prefix, groupDocs]) => (
              <SidebarGroup
                key={prefix}
                prefix={prefix}
                docs={groupDocs}
                selectedDocId={selectedDocId}
                onSelect={onSelect}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function SidebarHealthBadge({
  health,
  onClick,
}: {
  health: ObsidianHealth;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex w-full items-center gap-2 rounded px-1 py-1 text-left transition-colors hover:bg-muted/40"
      title={`Obsidian: ${variantLabel(health.variant)} · 点击查看详情`}
    >
      <span
        className={cn(
          "h-2 w-2 shrink-0 rounded-full",
          variantDotColor(health.variant),
        )}
      />
      <span className="min-w-0 flex-1 truncate text-xs font-medium">
        Obsidian · {variantLabel(health.variant)}
      </span>
      <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
    </button>
  );
}

function SidebarGroup({
  prefix,
  docs,
  selectedDocId,
  onSelect,
}: {
  prefix: string;
  docs: DocInfo[];
  selectedDocId: string | null;
  onSelect: (id: string) => void;
}) {
  return (
    <details open className="group">
      <summary className="flex cursor-pointer list-none items-center gap-1.5 rounded px-2 py-1.5 text-xs font-semibold uppercase tracking-wider text-muted-foreground transition-colors hover:bg-muted/40">
        <ChevronRight className="h-3 w-3 shrink-0 transition-transform group-open:rotate-90" />
        <span className="flex-1 truncate normal-case">{prefix}</span>
        <span className="shrink-0 font-mono text-[10px] font-normal">
          {docs.length}
        </span>
      </summary>
      <ul className="mt-0.5 space-y-0.5">
        {docs.map((d) => (
          <SidebarDocItem
            key={d.doc_id}
            doc={d}
            selected={selectedDocId === d.doc_id}
            onSelect={() => onSelect(d.doc_id)}
          />
        ))}
      </ul>
    </details>
  );
}

function SidebarDocItem({
  doc,
  selected,
  onSelect,
}: {
  doc: DocInfo;
  selected: boolean;
  onSelect: () => void;
}) {
  const btnRef = useRef<HTMLButtonElement>(null);
  // 选中时自动滚入可视区（在 sidebar 内部滚动容器里）
  useEffect(() => {
    if (selected && btnRef.current) {
      btnRef.current.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
  }, [selected]);

  return (
    <li>
      <button
        ref={btnRef}
        type="button"
        onClick={onSelect}
        className={cn(
          "flex w-full items-start gap-1.5 rounded px-2 py-1.5 text-left transition-colors",
          selected
            ? "bg-primary-100 text-primary-900 dark:bg-primary-900/30 dark:text-primary-100"
            : "hover:bg-muted/50",
        )}
        aria-pressed={selected}
      >
        <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-xs font-medium leading-tight">
            {doc.role ?? doc.doc_id}
          </span>
          <span className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground">
            {doc.doc_id}
          </span>
        </span>
        <span className="mt-0.5 flex shrink-0 items-center gap-0.5">
          {doc.starred && (
            <Star
              className="h-3 w-3 fill-warning text-warning"
              aria-label="核心文档"
            />
          )}
          {doc.is_new && (
            <Sparkles
              className="h-3 w-3 text-primary-600 dark:text-primary-400"
              aria-label="新增"
            />
          )}
        </span>
      </button>
    </li>
  );
}

function SidebarSkeleton() {
  return (
    <div className="space-y-3 px-2 py-2" aria-busy>
      {Array.from({ length: 3 }).map((_, gi) => (
        <div key={gi}>
          <div className="mb-1 h-3 w-20 animate-pulse rounded bg-muted" />
          {Array.from({ length: 3 }).map((_, i) => (
            <div
              key={i}
              className="my-1 h-7 animate-pulse rounded bg-muted/60"
            />
          ))}
        </div>
      ))}
    </div>
  );
}

// =====================================================================
// Preview — 默认显示健康卡 + 文档总览；选中后显示 markdown 渲染
// =====================================================================

interface KnowledgePreviewProps {
  selectedDoc: DocInfo | null;
  health: ObsidianHealth | null;
  healthLoading: boolean;
  healthError: string | null;
  grouped: Record<string, DocInfo[]>;
  docsLoading: boolean;
  onSelectDoc: (id: string) => void;
}

function KnowledgePreview({
  selectedDoc,
  health,
  healthLoading,
  healthError,
  grouped,
  docsLoading,
  onSelectDoc,
}: KnowledgePreviewProps) {
  // 选中：渲染 markdown
  if (selectedDoc) {
    return <DocPreview doc={selectedDoc} onBack={() => onSelectDoc("")} />;
  }

  // 默认：健康卡 + 文档总览
  return (
    <div className="space-y-8">
      <section aria-label="Obsidian 健康检查">
        <h2 className="mb-3 font-serif text-lg font-semibold">
          Obsidian 健康检查
        </h2>
        {healthLoading ? (
          <ObsidianSkeleton />
        ) : healthError ? (
          <EmptyPlaceholder
            title="读取 Obsidian 配置失败"
            description={healthError}
            icon="🔍"
          />
        ) : health ? (
          <ObsidianHealthCard health={health} />
        ) : null}
      </section>

      <section aria-label="文档总览">
        <h2 className="mb-3 font-serif text-lg font-semibold">
          文档总览
          <span className="ml-2 font-mono text-xs font-normal text-muted-foreground">
            （点击进入只读预览）
          </span>
        </h2>
        {docsLoading ? (
          <DocsSkeleton />
        ) : Object.keys(grouped).length === 0 ? (
          <EmptyPlaceholder
            title="未读取到 INDEX.md"
            description="预期路径：C:\\Users\\homebase\\INDEX.md"
            icon="📚"
          />
        ) : (
          <div className="space-y-6">
            {Object.entries(grouped).map(([prefix, groupDocs]) => (
              <DocGroupOverview
                key={prefix}
                prefix={prefix}
                docs={groupDocs}
                onSelect={onSelectDoc}
              />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

function DocPreview({
  doc,
  onBack,
}: {
  doc: DocInfo;
  onBack?: () => void;
}) {
  const [copied, setCopied] = useState(false);

  const content = useQuery({
    queryKey: ["docs", "read", doc.doc_id],
    queryFn: () => docsApi.read(doc.doc_id),
  });

  const onOpenInTypora = async () => {
    const path = content.data?.absolute_path;
    if (!path) return;
    try {
      // 优先走后端 Typora（C:\HomeBase\Tools\Typora\Typora.exe，用户已装）
      const { wikiApi: api } = await import("@/lib/tauri-api");
      const res = await api.openInTypora(path);
      if (res.opened_with === "fallback_shell") {
        await openExternal(path);
      }
    } catch (e) {
      console.error("Typora 启动失败，回退 OS 默认：", e);
      try {
        await openExternal(path);
      } catch (e2) {
        console.error("OS 默认也失败：", e2);
      }
    }
  };

  const onCopyPath = async () => {
    const path = content.data?.absolute_path;
    if (!path) return;
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("复制路径失败：", e);
    }
  };

  return (
    <div className="space-y-5">
      {/* 头部：role 标题 + 元信息 + 操作 */}
      <header className="border-b border-border/60 pb-4">
        {onBack && (
          <button
            type="button"
            onClick={onBack}
            className="mb-3 inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
          >
            ← 返回总览
          </button>
        )}
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0 flex-1">
            <h2 className="font-serif text-xl font-semibold leading-tight">
              {doc.role ?? doc.doc_id}
              {doc.starred && (
                <Star
                  className="ml-2 inline-block h-4 w-4 fill-warning text-warning"
                  aria-label="核心文档"
                />
              )}
            </h2>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
              <code className="rounded bg-muted px-1.5 py-0.5 font-mono">
                {doc.doc_id}
              </code>
              <span
                className="min-w-0 truncate font-mono text-muted-foreground"
                title={doc.path}
              >
                {doc.path}
              </span>
              {doc.updated && (
                <span className="font-mono text-muted-foreground">
                  · {doc.updated}
                </span>
              )}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={onCopyPath}
              disabled={!content.data?.absolute_path}
              className="hb-btn-outline inline-flex items-center gap-1.5 px-3 py-1.5 text-xs disabled:opacity-50"
              title="复制绝对路径到剪贴板"
            >
              {copied ? (
                <>
                  <Check className="h-3.5 w-3.5 text-success-strong" />
                  已复制
                </>
              ) : (
                <>
                  <Copy className="h-3.5 w-3.5" />
                  复制路径
                </>
              )}
            </button>
            <button
              type="button"
              onClick={onOpenInTypora}
              disabled={!content.data?.absolute_path}
              className="hb-btn-outline inline-flex items-center gap-1.5 px-3 py-1.5 text-xs disabled:opacity-50"
              title="优先用 Typora 打开（C:\HomeBase\Tools\Typora\Typora.exe）；找不到时回退 OS 默认"
            >
              <ExternalLink className="h-3.5 w-3.5" />
              在 Typora 中打开
            </button>
          </div>
        </div>
      </header>

      {/* 内容 */}
      {content.isLoading ? (
        <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          加载文档…
        </div>
      ) : content.isError ? (
        <EmptyPlaceholder
          title="读取文件失败"
          description={String(content.error ?? "未知错误")}
          icon="⚠️"
        />
      ) : content.data?.error ? (
        <div className="flex items-start gap-3 rounded-lg border border-warning/40 bg-warning/5 p-4 text-sm">
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0 text-warning-strong" />
          <div className="min-w-0 flex-1">
            <p className="font-medium text-warning-strong">无法预览</p>
            <p className="mt-1 text-muted-foreground">{content.data.error}</p>
            <p className="mt-2 font-mono text-xs text-muted-foreground">
              {content.data.absolute_path}
            </p>
          </div>
        </div>
      ) : content.data?.content !== null && content.data?.content !== undefined ? (
        <MarkdownRenderer source={content.data.content} />
      ) : null}
    </div>
  );
}

function DocGroupOverview({
  prefix,
  docs,
  onSelect,
}: {
  prefix: string;
  docs: DocInfo[];
  onSelect: (id: string) => void;
}) {
  return (
    <div>
      <h3 className="mb-2 text-sm font-semibold text-foreground/80">
        {prefix}{" "}
        <span className="font-mono text-xs font-normal text-muted-foreground">
          ({docs.length})
        </span>
      </h3>
      <ul className="divide-y divide-border/60 rounded-xl border border-border bg-card">
        {docs.map((d) => (
          <li key={`${prefix}-${d.doc_id}`}>
            <button
              type="button"
              onClick={() => onSelect(d.doc_id)}
              className="block w-full px-5 py-3 text-left transition-colors hover:bg-muted/30"
            >
              <div className="flex items-baseline gap-2">
                <p className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
                  {d.role ?? (
                    <span className="italic text-muted-foreground">
                      （未填 role）
                    </span>
                  )}
                </p>
                {d.starred && (
                  <Star
                    className="h-3.5 w-3.5 shrink-0 fill-warning text-warning"
                    aria-label="核心文档"
                  />
                )}
                {d.is_new && (
                  <Sparkles
                    className="h-3.5 w-3.5 shrink-0 text-primary-600 dark:text-primary-400"
                    aria-label="新增"
                  />
                )}
                {d.updated && (
                  <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                    {d.updated}
                  </span>
                )}
              </div>
              <div className="mt-1 flex items-center gap-2 text-xs">
                <code className="shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-[11px]">
                  {d.doc_id}
                </code>
                <span
                  className="truncate font-mono text-muted-foreground"
                  title={d.path}
                >
                  {d.path}
                </span>
              </div>
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

// =====================================================================
// Obsidian 健康卡
// =====================================================================

function ObsidianHealthCard({ health }: { health: ObsidianHealth }) {
  const palette = variantPalette(health.variant);
  const label = variantLabel(health.variant);

  return (
    <div className="hb-card space-y-4">
      {/* 状态标题行 */}
      <div className="flex items-center gap-3">
        <span
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs font-medium",
            palette,
          )}
        >
          <span className={cn("h-1.5 w-1.5 rounded-full", variantDotColor(health.variant))} />
          {label}
        </span>
        <span className="font-mono text-xs text-muted-foreground">
          v3 §22.3 检测
        </span>
      </div>

      {/* 字段行 */}
      <dl className="grid grid-cols-1 gap-x-8 gap-y-2 text-sm sm:grid-cols-[10rem_1fr]">
        <dt className="text-muted-foreground">配置文件</dt>
        <dd>
          <span className="font-mono text-xs">{health.config_path}</span>
          <span
            className={cn(
              "ml-2 rounded px-2 py-0.5 font-mono text-[11px] font-semibold",
              health.config_exists
                ? "bg-success/10 text-success-strong"
                : "bg-muted text-muted-foreground",
            )}
          >
            {health.config_exists ? "存在" : "不存在"}
          </span>
        </dd>

        <dt className="text-muted-foreground">期望 Vault</dt>
        <dd>
          <span className="font-mono text-xs">{health.expected_vault_path}</span>
          <span
            className={cn(
              "ml-2 rounded px-2 py-0.5 font-mono text-[11px] font-semibold",
              health.expected_vault_is_obsidian
                ? "bg-success/10 text-success-strong"
                : health.expected_vault_exists
                ? "bg-warning/10 text-warning-strong"
                : "bg-destructive/10 text-destructive-strong",
            )}
          >
            {health.expected_vault_is_obsidian
              ? "已初始化为 vault"
              : health.expected_vault_exists
              ? "目录存在但非 vault"
              : "目录不存在"}
          </span>
        </dd>

        {health.vaults.length > 0 && (
          <>
            <dt className="text-muted-foreground">注册 Vaults</dt>
            <dd className="space-y-1.5">
              {health.vaults.map((v) => (
                <div key={v.id} className="flex items-baseline gap-2">
                  <span className="truncate font-mono text-xs" title={v.path}>
                    {v.path}
                  </span>
                  {v.matches_expected && (
                    <span className="rounded bg-success/10 px-1.5 py-0.5 font-mono text-[11px] font-semibold text-success-strong">
                      期望
                    </span>
                  )}
                  {v.open && (
                    <span className="rounded bg-primary-100 px-1 font-mono text-[10px] text-primary-700 dark:bg-primary-900/40 dark:text-primary-300">
                      open
                    </span>
                  )}
                  {!v.exists && (
                    <span className="rounded bg-destructive/10 px-1.5 py-0.5 font-mono text-[11px] font-semibold text-destructive-strong">
                      不存在
                    </span>
                  )}
                </div>
              ))}
            </dd>
          </>
        )}
      </dl>

      {/* 建议 */}
      <div className="border-t border-border/60 pt-3 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">建议：</span>{" "}
        {health.recommendation}
      </div>
    </div>
  );
}

function variantLabel(v: ObsidianVariant): string {
  switch (v) {
    case "healthy":
      return "✓ 健康";
    case "not_initialized":
      return "— 未初始化";
    case "dangling":
      return "⚠ Dangling (B')";
    case "not_a_vault":
      return "⚠ 非 vault";
  }
}

function variantPalette(v: ObsidianVariant): string {
  switch (v) {
    case "healthy":
      return "border-success/50 bg-success/10 text-success-strong";
    case "not_initialized":
      return "border-border bg-muted text-foreground";
    case "dangling":
    case "not_a_vault":
      return "border-warning/50 bg-warning/10 text-warning-strong";
  }
}

function variantDotColor(v: ObsidianVariant): string {
  switch (v) {
    case "healthy":
      return "bg-success";
    case "not_initialized":
      return "bg-muted-foreground/60";
    case "dangling":
    case "not_a_vault":
      return "bg-warning";
  }
}

// =====================================================================
// 文档树（按前缀分组）
// =====================================================================

function groupDocsByPrefix(docs: DocInfo[]): Record<string, DocInfo[]> {
  const groups: Record<string, DocInfo[]> = {};
  for (const d of docs) {
    const prefix = inferPrefix(d.path);
    if (!groups[prefix]) groups[prefix] = [];
    groups[prefix].push(d);
  }
  // starred 优先 + 按 doc_id 字母序
  for (const k of Object.keys(groups)) {
    groups[k].sort((a, b) => {
      if (a.starred !== b.starred) return a.starred ? -1 : 1;
      return a.doc_id.localeCompare(b.doc_id);
    });
  }
  return groups;
}

/**
 * 分组标签（用户友好的中文命名 + 原始前缀）
 * 设计原则：优先让用户理解"这是什么类型的文档"，而不是暴露文件路径结构。
 */
function inferPrefix(path: string): string {
  if (/^[A-Z]:\\/i.test(path)) return "外部项目文档";
  const first = path.split("/")[0];
  switch (first) {
    case "system":
      return "系统规划";
    case "tools":
      return "工具配置";
    case "agents":
      return "Agent 相关";
    case "projects":
      return "项目笔记";
    case "sessions":
      return "会话记录";
    case "rules":
      return "规则约束";
    default:
      return first || "其他";
  }
}

function DocsSkeleton() {
  return (
    <div className="space-y-6" aria-busy>
      {Array.from({ length: 2 }).map((_, gi) => (
        <div key={gi}>
          <div className="mb-2 h-3 w-24 animate-pulse rounded bg-muted" />
          <ul className="divide-y divide-border/60 rounded-xl border border-border bg-card">
            {Array.from({ length: 3 }).map((_, i) => (
              <li key={i} className="px-5 py-3">
                <div className="h-4 w-40 animate-pulse rounded bg-muted" />
                <div className="mt-2 h-3 w-3/4 animate-pulse rounded bg-muted" />
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

function ObsidianSkeleton() {
  return (
    <div className="hb-card space-y-3" aria-busy>
      <div className="h-6 w-32 animate-pulse rounded bg-muted" />
      <div className="h-3 w-3/4 animate-pulse rounded bg-muted" />
      <div className="h-3 w-1/2 animate-pulse rounded bg-muted" />
    </div>
  );
}
