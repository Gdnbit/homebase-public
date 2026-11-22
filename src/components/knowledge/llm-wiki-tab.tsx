/**
 * LLMWikiTab — 读 C:\HomeBase\Wiki 的递归扫描结果，master-detail 布局
 *
 * Phase 14.5（Karpathy LLM Wiki 模式）：
 * - sidebar：**真实目录树**（顶层 = C:\HomeBase\Wiki 第一层；文件夹 details 可折叠；文件叶子点击选中）
 * - 搜索：激活时切扁平结果列表（避免树折叠遮挡）
 * - 预览：选中 → wiki_read 取全文 → markdown 渲染
 * - 「在 Typora 中打开」：调用后端 docs_open_in_typora（C:\HomeBase\Tools\Typora\Typora.exe）
 */

import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  CheckCircle2,
  ChevronRight,
  Copy,
  Eye,
  ExternalLink,
  FileText,
  Folder,
  FolderOpen,
  Info,
  Loader2,
  RefreshCw,
  Search,
  Sparkles,
  Upload,
  X,
} from "lucide-react";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { listen } from "@tauri-apps/api/event";
import {
  gdnbitApi,
  wikiApi,
  type GdnbitDoctorResult,
  type GdnbitEvent,
  type WikiDoc,
} from "@/lib/tauri-api";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { EmptyPlaceholder } from "@/components/empty-placeholder";
import { cn } from "@/lib/utils";

// =====================================================================
// 主入口
// =====================================================================

export function LLMWikiTab() {
  const [selectedRelPath, setSelectedRelPath] = useState<string | null>(null);
  const [search, setSearch] = useState("");

  const docs = useQuery({
    queryKey: ["wiki", "list"],
    queryFn: () => wikiApi.list(),
  });

  // 空骨架目录（Karpathy wiki 层由 LLM 维护，初始是空目录，需另外拿）
  const dirsQuery = useQuery({
    queryKey: ["wiki", "dirs"],
    queryFn: () => wikiApi.dirs(),
  });

  // 过滤
  const filtered = useMemo(() => {
    const all = docs.data ?? [];
    const q = search.trim().toLowerCase();
    if (!q) return all;
    return all.filter((d) => {
      const hay = [d.title, d.rel_path, d.doc_id ?? ""].join(" ").toLowerCase();
      return hay.includes(q);
    });
  }, [docs.data, search]);

  // 选中文档
  const selectedDoc = useMemo(
    () => docs.data?.find((d) => d.rel_path === selectedRelPath) ?? null,
    [docs.data, selectedRelPath],
  );

  return (
    <div className="flex flex-col gap-6 lg:flex-row">
      <aside
        className="w-full shrink-0 lg:sticky lg:top-6 lg:w-[300px] lg:self-start"
        style={{ maxHeight: "calc(100vh - 8rem)" }}
      >
        <WikiSidebar
          docs={filtered}
          allDirs={dirsQuery.data ?? []}
          totalCount={docs.data?.length ?? 0}
          isLoading={docs.isLoading || dirsQuery.isLoading}
          selectedRelPath={selectedRelPath}
          onSelect={setSelectedRelPath}
          search={search}
          onSearchChange={setSearch}
        />
      </aside>

      <main className="min-w-0 flex-1">
        {selectedDoc ? (
          <WikiPreview
            doc={selectedDoc}
            onBack={() => setSelectedRelPath(null)}
          />
        ) : (
          <WikiOverview
            docs={docs.data ?? []}
            isLoading={docs.isLoading}
            onSelectDoc={setSelectedRelPath}
          />
        )}
      </main>
    </div>
  );
}

// =====================================================================
// Sidebar
// =====================================================================

interface WikiSidebarProps {
  docs: WikiDoc[];
  /** 所有目录的相对路径（含空骨架），来自 wiki_dirs */
  allDirs: string[];
  totalCount: number;
  isLoading: boolean;
  selectedRelPath: string | null;
  onSelect: (rel: string | null) => void;
  search: string;
  onSearchChange: (s: string) => void;
}

function WikiSidebar({
  docs,
  allDirs,
  totalCount,
  isLoading,
  selectedRelPath,
  onSelect,
  search,
  onSearchChange,
}: WikiSidebarProps) {
  // 树结构（搜索时不构建，直接走扁平列表）
  const tree = useMemo(
    () => (search.trim() ? null : buildTree(docs, allDirs)),
    [docs, allDirs, search],
  );

  return (
    <div className="flex max-h-[60vh] flex-col overflow-hidden rounded-xl border border-border bg-card lg:max-h-full lg:h-full">
      {/* 顶部状态条 */}
      <div className="flex items-center justify-between border-b border-border/60 px-3 py-2 text-xs">
        <span className="font-medium">
          C:\HomeBase\Wiki
          {!isLoading && (
            <span className="ml-1.5 font-mono text-muted-foreground">
              · {totalCount}
            </span>
          )}
        </span>
        {search && (
          <span className="font-mono text-muted-foreground">
            匹配 {docs.length}
          </span>
        )}
      </div>

      {/* 搜索 */}
      <div className="border-b border-border/60 px-2 py-2">
        <div className="relative">
          <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            type="text"
            value={search}
            onChange={(e) => onSearchChange(e.target.value)}
            placeholder="搜索 title / 路径 / doc_id"
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

      {/* 列表 */}
      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
        {isLoading ? (
          <div className="space-y-2 p-2" aria-busy>
            {Array.from({ length: 6 }).map((_, i) => (
              <div key={i} className="h-7 animate-pulse rounded bg-muted/40" />
            ))}
          </div>
        ) : docs.length === 0 && (search || allDirs.length === 0) ? (
          <div className="px-3 py-8 text-center text-xs text-muted-foreground">
            {search
              ? `无匹配 "${search}"`
              : "C:\\HomeBase\\Wiki 为空或不可读。请确认目录存在。"}
          </div>
        ) : tree ? (
          // 真实目录树
          <ul className="space-y-0.5">
            {tree.map((node) => (
              <TreeNodeView
                key={nodeKey(node)}
                node={node}
                depth={0}
                selectedRelPath={selectedRelPath}
                onSelect={onSelect}
              />
            ))}
          </ul>
        ) : (
          // 搜索激活：扁平列表
          <ul className="space-y-0.5">
            {docs.map((d) => (
              <SidebarItem
                key={d.rel_path}
                doc={d}
                selected={d.rel_path === selectedRelPath}
                onSelect={() => onSelect(d.rel_path)}
                showFullPath
              />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

// =====================================================================
// 树视图
// =====================================================================

type TreeNode =
  | { kind: "dir"; name: string; path: string; children: TreeNode[] }
  | { kind: "file"; doc: WikiDoc };

function nodeKey(n: TreeNode): string {
  return n.kind === "dir" ? `dir:${n.path}` : `file:${n.doc.rel_path}`;
}

/**
 * 把 wiki 扁平数据还原成目录树
 *
 * 融合两个数据源：
 * - `allDirs`: 来自 `wiki_dirs`，所有目录（含空骨架），让 LLM 待填充的目录可见
 * - `docs`: 来自 `wiki_list`，所有 markdown 文件
 *
 * 顺序：先建所有目录节点 → 再把文件塞进对应父目录。这样空目录（如 `wiki/notes/llm/`）
 * 即使没有 markdown 也会作为 DirNode 出现在树里。
 *
 * 排序：同级 dir 在前、file 在后；同类按 name 字母序
 */
function buildTree(docs: WikiDoc[], allDirs: string[] = []): TreeNode[] {
  type DirRef = Extract<TreeNode, { kind: "dir" }>;
  const root: DirRef = { kind: "dir", name: "", path: "", children: [] };

  // 辅助：确保从 root 到 parts[] 这条路径上的所有 dir 节点都存在，返回最深那层
  const ensureDir = (parts: string[]): DirRef => {
    let cur: DirRef = root;
    for (let i = 0; i < parts.length; i++) {
      const segName = parts[i];
      const segPath = parts.slice(0, i + 1).join("/");
      let next = cur.children.find(
        (c): c is DirRef => c.kind === "dir" && c.name === segName,
      );
      if (!next) {
        next = { kind: "dir", name: segName, path: segPath, children: [] };
        cur.children.push(next);
      }
      cur = next;
    }
    return cur;
  };

  // 1. 先建所有目录骨架（含空目录）
  for (const dirPath of allDirs) {
    const parts = dirPath.split("/").filter(Boolean);
    if (parts.length > 0) ensureDir(parts);
  }

  // 2. 再塞文件
  for (const doc of docs) {
    const parts = doc.rel_path.split("/").filter(Boolean);
    const parentParts = parts.slice(0, -1);
    const parent = parentParts.length > 0 ? ensureDir(parentParts) : root;
    parent.children.push({ kind: "file", doc });
  }

  // 递归排序
  sortTree(root);
  return root.children;
}

function sortTree(dir: Extract<TreeNode, { kind: "dir" }>) {
  dir.children.sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === "dir" ? -1 : 1;
    const an = a.kind === "dir" ? a.name : a.doc.title;
    const bn = b.kind === "dir" ? b.name : b.doc.title;
    return an.localeCompare(bn, "zh");
  });
  for (const c of dir.children) {
    if (c.kind === "dir") sortTree(c);
  }
}

/**
 * 递归渲染 — depth 控制缩进
 *
 * 顶层（depth=0）目录默认展开，更深目录默认折叠
 */
function TreeNodeView({
  node,
  depth,
  selectedRelPath,
  onSelect,
}: {
  node: TreeNode;
  depth: number;
  selectedRelPath: string | null;
  onSelect: (rel: string) => void;
}) {
  if (node.kind === "file") {
    return (
      <li>
        <SidebarItem
          doc={node.doc}
          selected={node.doc.rel_path === selectedRelPath}
          onSelect={() => onSelect(node.doc.rel_path)}
          depth={depth}
        />
      </li>
    );
  }

  // 目录：默认展开顶层（depth 0）和 wiki/raw 这种用户高频入口
  const defaultOpen = depth === 0;
  const fileCount = countFiles(node);

  return (
    <li>
      <details className="group/dir" open={defaultOpen}>
        <summary
          className="flex cursor-pointer list-none items-center gap-1 rounded px-1.5 py-1 text-xs transition-colors hover:bg-muted/40"
          style={{ paddingLeft: `${depth * 12 + 6}px` }}
        >
          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground transition-transform group-open/dir:rotate-90" />
          <Folder className="h-3.5 w-3.5 shrink-0 text-muted-foreground group-open/dir:hidden" />
          <FolderOpen className="hidden h-3.5 w-3.5 shrink-0 text-primary-500 group-open/dir:block" />
          <span className="flex-1 truncate font-medium">{node.name}</span>
          <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
            {fileCount}
          </span>
        </summary>
        <ul className="mt-0.5 space-y-0.5">
          {node.children.map((c) => (
            <TreeNodeView
              key={nodeKey(c)}
              node={c}
              depth={depth + 1}
              selectedRelPath={selectedRelPath}
              onSelect={onSelect}
            />
          ))}
        </ul>
      </details>
    </li>
  );
}

function countFiles(dir: Extract<TreeNode, { kind: "dir" }>): number {
  let total = 0;
  for (const c of dir.children) {
    total += c.kind === "file" ? 1 : countFiles(c);
  }
  return total;
}

function SidebarItem({
  doc,
  selected,
  onSelect,
  depth = 0,
  showFullPath = false,
}: {
  doc: WikiDoc;
  selected: boolean;
  onSelect: () => void;
  depth?: number;
  showFullPath?: boolean;
}) {
  const btnRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (selected && btnRef.current) {
      btnRef.current.scrollIntoView({ block: "nearest", behavior: "smooth" });
    }
  }, [selected]);

  return (
    <button
      ref={btnRef}
      type="button"
      onClick={onSelect}
      className={cn(
        "flex w-full items-start gap-1.5 rounded px-1.5 py-1 text-left transition-colors",
        selected
          ? "bg-primary-100 text-primary-900 dark:bg-primary-900/30 dark:text-primary-100"
          : "hover:bg-muted/50",
      )}
      style={{ paddingLeft: `${depth * 12 + 6}px` }}
      aria-pressed={selected}
    >
      <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-xs font-medium leading-tight">
          {doc.title}
        </span>
        {showFullPath && (
          <span className="mt-0.5 block truncate font-mono text-[10px] text-muted-foreground">
            {doc.rel_path}
          </span>
        )}
      </span>
    </button>
  );
}

// =====================================================================
// 预览（选中后渲染）
// =====================================================================

function WikiPreview({
  doc,
  onBack,
}: {
  doc: WikiDoc;
  onBack: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const [publishOpen, setPublishOpen] = useState(false);

  const content = useQuery({
    queryKey: ["wiki", "read", doc.rel_path],
    queryFn: () => wikiApi.read(doc.rel_path),
  });

  // Phase C 入口优化（2026-05-06）：文章 frontmatter 含 `publish: true` 时自动展开发布面板
  // 避免 USER "选了文章但没点'发布到网站'按钮" 看不到 Phase C 所有功能的情况
  // 依赖 doc.rel_path + content · 切新文章时重新判断；USER 手动收起后不会被覆盖（deps 未变）
  useEffect(() => {
    if (!content.data?.content) return;
    const fmMatch = content.data.content.match(/^---\r?\n([\s\S]*?)^---/m);
    const hasPublishTrue = fmMatch ? /^publish:\s*true\b/m.test(fmMatch[1]) : false;
    setPublishOpen(hasPublishTrue);
  }, [doc.rel_path, content.data?.content]);

  const onOpenInTypora = async () => {
    try {
      const res = await wikiApi.openInTypora(doc.absolute_path);
      if (res.opened_with === "fallback_shell") {
        // Typora 不在，回退 OS 默认应用
        await openExternal(doc.absolute_path);
      }
    } catch (e) {
      console.error("Typora 启动失败：", e);
      // 尝试 fallback
      try {
        await openExternal(doc.absolute_path);
      } catch (e2) {
        console.error("OS 默认打开也失败：", e2);
      }
    }
  };

  const onCopyPath = async () => {
    try {
      await navigator.clipboard.writeText(doc.absolute_path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.error("复制路径失败：", e);
    }
  };

  return (
    <div className="space-y-5">
      {/* 头部 */}
      <header className="border-b border-border/60 pb-4">
        <button
          type="button"
          onClick={onBack}
          className="mb-3 inline-flex items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          返回总览
        </button>
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0 flex-1">
            <h2 className="font-serif text-xl font-semibold leading-tight">
              {doc.title}
            </h2>
            <div className="mt-2 flex flex-wrap items-center gap-2 text-xs">
              {doc.doc_id && (
                <code className="rounded bg-muted px-1.5 py-0.5 font-mono">
                  {doc.doc_id}
                </code>
              )}
              <span
                className="min-w-0 truncate font-mono text-muted-foreground"
                title={doc.absolute_path}
              >
                {doc.rel_path}
              </span>
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              onClick={onCopyPath}
              className="hb-btn-outline inline-flex items-center gap-1.5 px-3 py-1.5 text-xs"
              title="复制绝对路径"
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
              className="hb-btn-outline inline-flex items-center gap-1.5 px-3 py-1.5 text-xs"
              title="用 Typora 打开（C:\HomeBase\Tools\Typora\Typora.exe；找不到时回退 OS 默认）"
            >
              <ExternalLink className="h-3.5 w-3.5" />
              在 Typora 中打开
            </button>
            <button
              type="button"
              onClick={() => setPublishOpen((v) => !v)}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
                publishOpen
                  ? "bg-primary-600 text-white hover:bg-primary-700"
                  : "hb-btn-outline",
              )}
              title="发布到 GDNbit 网站（跑 obsidian-sync.mjs）"
            >
              <Upload className="h-3.5 w-3.5" />
              {publishOpen ? "收起发布" : "发布到网站"}
            </button>
          </div>
        </div>
      </header>

      {publishOpen && (
        <WikiPublishPanel
          absolutePath={doc.absolute_path}
          relPath={doc.rel_path}
          onClose={() => setPublishOpen(false)}
        />
      )}

      {/* 内容 */}
      {content.isLoading ? (
        <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          加载文档…
        </div>
      ) : content.isError ? (
        <EmptyPlaceholder
          title="读取失败"
          description={String(content.error ?? "")}
        />
      ) : content.data?.error ? (
        <EmptyPlaceholder
          title="无法预览"
          description={content.data.error}
        />
      ) : content.data?.content ? (
        <MarkdownRenderer source={content.data.content} />
      ) : (
        <EmptyPlaceholder title="空文件" description="该文档没有内容。" />
      )}
    </div>
  );
}

// =====================================================================
// 发布到 GDNbit 网站（Phase C · 2026-05-04）
// =====================================================================
//
// 流程：单文件模式 → 调 gdnbit_publish(single_file=absPath, dry_run) → 订阅 gdnbit:event
// 展示 command_preview + stdout/stderr 实时流 + 完成/失败徽章。
// v1 用默认 config（到 Settings · GDNbit 段改）；若配置缺失后端会 doctor-fail early。

// publish.mjs 输出含 ANSI 彩色码 \x1b[XXm；前端 <pre> 显示前先剥离让其干净
// 不引第三方依赖（strip-ansi 之类的），自己写一行正则
function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

// 5 种动作 · render/ai-optimize 一次性，preview/publish-draft/publish-direct 流式
type PublishMode =
  | "render"
  | "preview"
  | "publish-draft"
  | "publish-direct"
  | "ai-optimize";

const MODE_META: Record<
  PublishMode,
  { label: string; verb: string; doneLabel: string }
> = {
  render: { label: "render", verb: "本地预览", doneLabel: "render 完成" },
  preview: { label: "preview", verb: "API dry-run", doneLabel: "preview 完成" },
  "publish-draft": { label: "publish (draft)", verb: "发布草稿", doneLabel: "已发布草稿" },
  "publish-direct": { label: "publish (published)", verb: "直接发布", doneLabel: "已直发上线" },
  "ai-optimize": { label: "ai_optimize", verb: "AI 优化", doneLabel: "AI 优化完成" },
};

type PublishState =
  | { phase: "idle" }
  | {
      phase: "running";
      task_id: string;
      mode: PublishMode;
    }
  | {
      phase: "done";
      task_id: string | null;
      mode: PublishMode;
      exit_code: number;
      /** render 模式特有 · 生成的 HTML 文件 file:// URL */
      render_url?: string | null;
    }
  | {
      phase: "failed";
      task_id: string | null;
      mode: PublishMode | null;
      error: string;
    }
  | {
      phase: "cancelled";
      task_id: string;
      mode: PublishMode;
    };

function WikiPublishPanel({
  absolutePath,
  relPath,
  onClose,
}: {
  absolutePath: string;
  relPath: string;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [state, setState] = useState<PublishState>({ phase: "idle" });
  const [commandPreview, setCommandPreview] = useState<string | null>(null);
  const [logs, setLogs] = useState<
    Array<{ stream: "stdout" | "stderr"; line: string; ts: number }>
  >([]);
  const logsRef = useRef<HTMLDivElement | null>(null);

  const doctorQuery = useQuery({
    queryKey: ["gdnbit", "doctor"],
    queryFn: () => gdnbitApi.doctor(),
    refetchOnWindowFocus: false,
  });
  const configQuery = useQuery({
    queryKey: ["gdnbit", "config"],
    queryFn: () => gdnbitApi.getConfig(),
    refetchOnWindowFocus: false,
  });

  // 自动滚到底
  useEffect(() => {
    if (logsRef.current) {
      logsRef.current.scrollTop = logsRef.current.scrollHeight;
    }
  }, [logs.length]);

  // 订阅 gdnbit:event — 只接收当前 task_id 的事件
  useEffect(() => {
    if (state.phase !== "running") return;
    const taskId = state.task_id;
    const mode = state.mode;
    const unlistenPromise = listen<GdnbitEvent>("gdnbit:event", (e) => {
      const ev = e.payload;
      if (ev.task_id !== taskId) return;
      switch (ev.kind) {
        case "stream":
          setLogs((l) => [
            ...l,
            { stream: "stdout", line: ev.line, ts: Date.now() },
          ]);
          break;
        case "stderr":
          setLogs((l) => [
            ...l,
            { stream: "stderr", line: ev.line, ts: Date.now() },
          ]);
          break;
        case "completed":
          setState({
            phase: "done",
            task_id: taskId,
            mode,
            exit_code: ev.exit_code,
          });
          if (mode === "ai-optimize") {
            void queryClient.invalidateQueries({ queryKey: ["wiki"] });
            void queryClient.invalidateQueries({ queryKey: ["docs"] });
          }
          break;
        case "failed":
          setState({
            phase: "failed",
            task_id: taskId,
            mode,
            error: ev.message,
          });
          break;
        case "cancelled":
          setState({ phase: "cancelled", task_id: taskId, mode });
          break;
        default:
          break;
      }
    });
    return () => {
      unlistenPromise.then((un) => un());
    };
  }, [state, queryClient]);

  // 一次性 · render（不流式，调 publish.mjs render --open）
  const renderMutation = useMutation({
    mutationFn: () =>
      gdnbitApi.renderFile({ file: absolutePath, open: true }),
    onMutate: () => {
      setLogs([]);
      setCommandPreview(`node publish.mjs render ${absolutePath} --open`);
    },
    onSuccess: (r) => {
      // stdout 一次性塞进 logs（含 ✅/file:// URL 等）
      const lines = r.stdout
        .split(/\r?\n/)
        .filter((l) => l.length > 0)
        .map((line) => ({ stream: "stdout" as const, line, ts: Date.now() }));
      const errLines = r.stderr
        .split(/\r?\n/)
        .filter((l) => l.length > 0)
        .map((line) => ({ stream: "stderr" as const, line, ts: Date.now() }));
      setLogs([...lines, ...errLines]);
      // 抽 file:// URL（"🌐 在浏览器打开：file:///xxx" 那行）
      const m = r.stdout.match(/file:\/\/\/\S+\.html/);
      setState({
        phase: "done",
        task_id: null,
        mode: "render",
        exit_code: r.exit_code,
        render_url: m ? m[0] : null,
      });
    },
    onError: (e) =>
      setState({
        phase: "failed",
        task_id: null,
        mode: "render",
        error: String(e),
      }),
  });

  // 流式 · preview / publish-draft / publish-direct（调 publish.mjs preview/publish）
  const streamMutation = useMutation({
    mutationFn: (mode: PublishMode) => {
      if (mode === "preview") {
        return gdnbitApi.publishFile({ mode: "preview", file: absolutePath });
      }
      if (mode === "publish-draft") {
        return gdnbitApi.publishFile({
          mode: "publish",
          file: absolutePath,
          status: "draft",
        });
      }
      // publish-direct
      return gdnbitApi.publishFile({
        mode: "publish",
        file: absolutePath,
        status: "published",
      });
    },
    onSuccess: (r, mode) => {
      setCommandPreview(r.command_preview);
      setLogs([]);
      setState({ phase: "running", task_id: r.task_id, mode });
    },
    onError: (e, mode) =>
      setState({ phase: "failed", task_id: null, mode, error: String(e) }),
  });

  // 流式 · ai-optimize（调 claude CLI）· focus 字段从 window.prompt 取
  const aiOptimizeMutation = useMutation({
    mutationFn: (focus: string | null) =>
      gdnbitApi.aiOptimize({ file: absolutePath, focus }),
    onSuccess: (r) => {
      setCommandPreview(r.command_preview);
      setLogs([]);
      setState({ phase: "running", task_id: r.task_id, mode: "ai-optimize" });
    },
    onError: (e) =>
      setState({
        phase: "failed",
        task_id: null,
        mode: "ai-optimize",
        error: String(e),
      }),
  });

  const cancelMutation = useMutation({
    mutationFn: (taskId: string) => gdnbitApi.cancel(taskId),
  });

  const running = state.phase === "running";
  const busy =
    running ||
    streamMutation.isPending ||
    renderMutation.isPending ||
    aiOptimizeMutation.isPending;

  // 通用按钮工厂
  const ActionButton = ({
    mode,
    icon: Icon,
    label,
    variant,
  }: {
    mode: PublishMode;
    icon: typeof Upload;
    label: string;
    variant: "neutral" | "primary" | "warning" | "danger";
  }) => {
    const variants: Record<typeof variant, string> = {
      neutral: "border border-border bg-card text-foreground hover:bg-muted",
      primary:
        "bg-primary-600 text-white hover:bg-primary-700 dark:bg-primary-500 dark:hover:bg-primary-600",
      warning:
        "bg-amber-500 text-white hover:bg-amber-600 dark:bg-amber-600 dark:hover:bg-amber-700",
      danger:
        "bg-rose-600 text-white hover:bg-rose-700 dark:bg-rose-700 dark:hover:bg-rose-800",
    };
    const isCurrent =
      (running && state.mode === mode) ||
      (renderMutation.isPending && mode === "render") ||
      (aiOptimizeMutation.isPending && mode === "ai-optimize") ||
      (streamMutation.isPending && streamMutation.variables === mode);
    return (
      <button
        type="button"
        disabled={busy}
        onClick={() => {
          if (mode === "render") {
            renderMutation.mutate();
          } else if (mode === "ai-optimize") {
            // 弹 prompt 让用户填 focus（留空 = 全面优化 · 取消 = 不跑）
            const input = window.prompt(
              "本次 AI 优化的 focus？（留空 = 全面优化）\n例：简化冗语 / 补齐代码示例 / 改英文标题",
              "",
            );
            if (input === null) return; // 用户按取消
            const focus = input.trim() === "" ? null : input.trim();
            aiOptimizeMutation.mutate(focus);
          } else {
            streamMutation.mutate(mode);
          }
        }}
        className={cn(
          "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[11px] font-medium transition-colors",
          busy
            ? "cursor-not-allowed bg-muted text-muted-foreground"
            : variants[variant],
        )}
        title={MODE_META[mode].verb}
      >
        {isCurrent ? (
          <Loader2 className="h-3 w-3 animate-spin" />
        ) : (
          <Icon className="h-3 w-3" />
        )}
        {label}
      </button>
    );
  };

  return (
    <section className="rounded-xl border border-primary-200 bg-primary-50/30 p-4 dark:border-primary-800/60 dark:bg-primary-950/20">
      <div className="flex items-center justify-between gap-3 border-b border-primary-200/60 pb-3 dark:border-primary-800/40">
        <div className="min-w-0">
          <h3 className="flex items-center gap-1.5 text-sm font-semibold">
            <Upload className="h-3.5 w-3.5" />
            发布到 GDNbit 网站
          </h3>
          <p className="mt-0.5 truncate font-mono text-[10.5px] text-muted-foreground">
            {relPath}
          </p>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="rounded p-1 text-muted-foreground hover:bg-muted hover:text-foreground"
          title="关闭"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>

      {/* Doctor 诊断（打开 panel 时自动跑一次） */}
      {doctorQuery.isLoading ? (
        <div className="mt-3 flex items-center gap-2 rounded-md border border-border/60 bg-muted/10 p-2 text-[11px] text-muted-foreground">
          <Loader2 className="h-3 w-3 animate-spin" />
          跑 gdnbit_doctor 诊断…
        </div>
      ) : doctorQuery.data ? (
        <GdnbitDoctorBanner
          data={doctorQuery.data}
          configPath={"~/.homebase/gdnbit.json"}
          apiUrl={configQuery.data?.api_url ?? null}
          onRefresh={() => doctorQuery.refetch()}
          isRefreshing={doctorQuery.isFetching}
        />
      ) : null}

      {/* 5 个动作按钮（取代 v1 的 dryRun/force/status 三选项 + 单按钮）*/}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <ActionButton
          mode="ai-optimize"
          icon={Sparkles}
          label="✨ AI 优化"
          variant="neutral"
        />
        <span className="mx-1 h-4 w-px bg-border" aria-hidden />
        <ActionButton
          mode="render"
          icon={Eye}
          label="本地预览"
          variant="neutral"
        />
        <ActionButton
          mode="preview"
          icon={Search}
          label="API 预览"
          variant="neutral"
        />
        <span className="mx-1 h-4 w-px bg-border" aria-hidden />
        <ActionButton
          mode="publish-draft"
          icon={Upload}
          label="发布草稿"
          variant="primary"
        />
        <ActionButton
          mode="publish-direct"
          icon={Upload}
          label="直接发布"
          variant="warning"
        />
        {running && (
          <button
            type="button"
            disabled={cancelMutation.isPending}
            onClick={() =>
              state.phase === "running" && cancelMutation.mutate(state.task_id)
            }
            className="ml-auto inline-flex items-center gap-1 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-1.5 text-[11px] font-medium text-destructive-strong hover:bg-destructive/20"
          >
            {cancelMutation.isPending ? (
              <Loader2 className="h-3 w-3 animate-spin" />
            ) : (
              <X className="h-3 w-3" />
            )}
            取消
          </button>
        )}
      </div>
      <p className="mt-2 text-[10px] leading-relaxed text-muted-foreground">
        ✨ AI 优化 = Claude 按 STYLE_GUIDE 直改文件 · 本地预览 = 离线 HTML · API 预览 = 看后端自动分类 · 草稿不显前台 · 直发立即上线
      </p>

      {/* 命令预览（脱敏） */}
      {commandPreview && (
        <div className="mt-3">
          <p className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            执行命令（密钥从 SOPS 注入到 env，不入命令行）
          </p>
          <pre className="overflow-x-auto rounded border border-border/60 bg-background/60 p-2 font-mono text-[10.5px] leading-relaxed">
            {commandPreview}
          </pre>
        </div>
      )}

      {/* 实时日志 */}
      {logs.length > 0 && (
        <div className="mt-3">
          <p className="mb-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            输出流（{logs.length}）
          </p>
          <div
            ref={logsRef}
            className="max-h-64 overflow-auto rounded border border-border/60 bg-background/60 p-2 font-mono text-[10.5px] leading-relaxed"
          >
            {logs.map((l, i) => (
              <div
                key={i}
                className={cn(
                  l.stream === "stderr" && "text-destructive-strong",
                )}
              >
                {stripAnsi(l.line)}
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 最终状态徽章 */}
      {state.phase === "done" && (
        <div
          className={cn(
            "mt-3 flex items-center gap-2 rounded-md border px-3 py-2 text-[11px]",
            state.exit_code === 0
              ? "border-emerald-300 bg-emerald-50/50 text-emerald-900 dark:border-emerald-800 dark:bg-emerald-950/30 dark:text-emerald-200"
              : "border-amber-300 bg-amber-50/50 text-amber-900 dark:border-amber-800 dark:bg-amber-950/30 dark:text-amber-200",
          )}
        >
          <CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
          <span className="font-medium">
            {MODE_META[state.mode].doneLabel} · exit={state.exit_code}
          </span>
          {state.render_url && (
            <a
              href={state.render_url}
              target="_blank"
              rel="noreferrer"
              className="ml-auto inline-flex items-center gap-1 underline-offset-2 hover:underline"
            >
              在浏览器查看
            </a>
          )}
        </div>
      )}
      {state.phase === "failed" && (
        <div className="mt-3 flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-[11px] text-destructive-strong">
          <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
          <span>
            {state.mode ? `${MODE_META[state.mode].verb} ` : ""}失败：
            {state.error}
          </span>
        </div>
      )}
      {state.phase === "cancelled" && (
        <div className="mt-3 flex items-center gap-2 rounded-md border border-border/60 bg-muted/20 px-3 py-2 text-[11px] text-muted-foreground">
          <X className="h-3.5 w-3.5 shrink-0" />
          <span>
            已取消 {MODE_META[state.mode].verb}（task {state.task_id.slice(0, 8)}…）
          </span>
        </div>
      )}
    </section>
  );
}

function GdnbitDoctorBanner({
  data,
  configPath,
  apiUrl,
  onRefresh,
  isRefreshing,
}: {
  data: GdnbitDoctorResult;
  configPath: string;
  apiUrl: string | null;
  onRefresh: () => void;
  isRefreshing: boolean;
}) {
  const [expanded, setExpanded] = useState(!data.ok);
  const failCount = data.checks.filter((c) => !c.ok).length;

  return (
    <div
      className={cn(
        "mt-3 rounded-md border text-[11px]",
        data.ok
          ? "border-emerald-300 bg-emerald-50/40 dark:border-emerald-800 dark:bg-emerald-950/20"
          : "border-amber-300 bg-amber-50/40 dark:border-amber-800 dark:bg-amber-950/20",
      )}
    >
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left"
      >
        {data.ok ? (
          <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-emerald-600 dark:text-emerald-400" />
        ) : (
          <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-amber-600 dark:text-amber-400" />
        )}
        <span className="font-medium">
          {data.ok
            ? "诊断全部通过"
            : `诊断有问题（${failCount} 项未通过）`}
        </span>
        {apiUrl && (
          <code className="ml-1 truncate font-mono text-[10px] text-muted-foreground">
            → {apiUrl}
          </code>
        )}
        <ChevronRight
          className={cn(
            "ml-auto h-3 w-3 shrink-0 transition-transform",
            expanded && "rotate-90",
          )}
        />
      </button>

      {expanded && (
        <div className="space-y-2 border-t border-current/20 px-3 py-2">
          <ul className="space-y-1">
            {data.checks.map((c) => (
              <li key={c.id} className="flex items-start gap-2">
                {c.ok ? (
                  <CheckCircle2 className="mt-0.5 h-3 w-3 shrink-0 text-emerald-600 dark:text-emerald-400" />
                ) : (
                  <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0 text-destructive-strong" />
                )}
                <div className="min-w-0 flex-1">
                  <div className="font-medium">{c.label}</div>
                  <div className="truncate font-mono text-[10px] text-muted-foreground">
                    {c.detail}
                  </div>
                </div>
              </li>
            ))}
          </ul>

          {data.suggestions.length > 0 && (
            <div className="rounded border border-dashed border-current/30 bg-background/40 p-2">
              <div className="mb-1 flex items-center gap-1 font-medium">
                <Info className="h-3 w-3" />
                下一步建议
              </div>
              <ul className="ml-4 list-disc space-y-0.5 text-[10.5px] text-muted-foreground">
                {data.suggestions.map((s, i) => (
                  <li key={i}>{s}</li>
                ))}
              </ul>
            </div>
          )}

          <div className="flex items-center justify-between border-t border-current/20 pt-2">
            <span className="font-mono text-[10px] text-muted-foreground">
              配置：{configPath}
            </span>
            <button
              type="button"
              disabled={isRefreshing}
              onClick={onRefresh}
              className="inline-flex items-center gap-1 rounded border border-current/30 bg-background/40 px-2 py-0.5 text-[10px] hover:bg-background"
            >
              {isRefreshing ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <RefreshCw className="h-3 w-3" />
              )}
              重测
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// =====================================================================
// 总览（未选中文档时）— 「最近修改」时间线
// =====================================================================

function WikiOverview({
  docs,
  isLoading,
  onSelectDoc,
}: {
  docs: WikiDoc[];
  isLoading: boolean;
  onSelectDoc: (rel: string) => void;
}) {
  const recent = useMemo(
    () =>
      [...docs]
        .filter((d) => d.modified_ms > 0)
        .sort((a, b) => b.modified_ms - a.modified_ms)
        .slice(0, 12),
    [docs],
  );

  return (
    <div className="space-y-5">
      <section className="rounded-xl border border-border bg-card p-5">
        <h3 className="font-serif text-lg font-semibold">📚 LLM Wiki</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Karpathy 模式：raw 素材层 + wiki LLM 维护层 + AGENTS.md schema 层。
          左侧选文档即可只读预览；用「在 Typora 中打开」编辑。
        </p>
        <p className="mt-2 text-xs text-muted-foreground">
          扫描根：
          <code className="ml-1 rounded bg-muted px-1.5 py-0.5 font-mono text-[11px]">
            C:\HomeBase\Wiki
          </code>
        </p>
      </section>

      <section className="rounded-xl border border-border bg-card p-5">
        <h3 className="mb-3 text-sm font-semibold">最近修改</h3>
        {isLoading ? (
          <div className="space-y-2" aria-busy>
            {Array.from({ length: 5 }).map((_, i) => (
              <div key={i} className="h-9 animate-pulse rounded bg-muted/40" />
            ))}
          </div>
        ) : recent.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            还没扫描到 markdown。新建一个 .md 文件试试。
          </p>
        ) : (
          <ul className="divide-y divide-border/60">
            {recent.map((d) => (
              <li key={d.rel_path}>
                <button
                  type="button"
                  onClick={() => onSelectDoc(d.rel_path)}
                  className="flex w-full items-start gap-3 py-2.5 text-left transition-colors hover:bg-muted/30"
                >
                  <FileText className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{d.title}</p>
                    <p className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground">
                      {d.rel_path}
                    </p>
                  </div>
                  <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
                    {formatRelative(d.modified_ms)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}

// =====================================================================
// 工具
// =====================================================================

function formatRelative(ms: number): string {
  const diff = Date.now() - ms;
  const s = Math.floor(diff / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d}d`;
  // 超过 30 天显示日期
  const date = new Date(ms);
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

function pad(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}
