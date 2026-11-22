/**
 * Inbox 投料口 — Phase 14.5c
 *
 * 用户场景：
 * - 习惯手机备忘录 / 微博式低门槛输入
 * - 不愿打开 Obsidian 写文档
 * - "一股脑扔给我，AI 帮我归档" — 减少记忆成本
 *
 * 本页只做 I 输入层：
 * - 大 textarea + 可选标题 + 可选标签 + 来源
 * - Ctrl+Enter 投递
 * - 落到当前资料库的 raw/inbox/YYYY-MM-DD/，文件位置以原收录结果为准
 * - 「打开 Obsidian」快捷按钮（用 obsidian://open?vault=Wiki URI scheme）
 * - 「最近投料」列表方便确认写进去了
 *
 * II 消化层（raw/inbox/ → wiki/）由 Obsidian Wiki Agent 异步做，不在本页范围。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useClock } from "@/lib/use-clock";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  AlertCircle,
  BookOpen,
  CheckCircle2,
  ChevronDown,
  ChevronUp,
  Clock,
  Copy,
  Database,
  ExternalLink,
  FileText,
  FileUp,
  Image as ImageIcon,
  Loader2,
  Paperclip,
  PlayCircle,
  RefreshCw,
  Send,
  Settings as SettingsIcon,
  Sparkles,
  Square,
  Terminal,
  X,
} from "lucide-react";
import {
  inboxApi,
  topologyApi,
  wikiAgentApi,
  WIKI_AGENT_EVENT,
  type InboxAttachResult,
  type InboxRecentEntry,
  type WikiAgentConfig,
  type WikiAgentEvent,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";
import { WorkflowAssistantEntry } from "@/components/workflow-assistant-entry";
import { inboxHandoff } from "@/lib/workflow-context";
import { useWorkflowVisibility } from "@/lib/workflow-visibility";
import { useInboxSource, editInboxSource, submitInboxSource, inspectInboxSource, retryInboxSourceSave, continueInboxSource, cancelInboxSource, chooseInboxVersion, trackInboxUpload } from "@/lib/inbox-workspace";

// =====================================================================
// 附件内部表示
// =====================================================================

interface AttachmentItem {
  /** 上传返回的完整库内路径，含原日期 */
  rel: string;
  /** 后端返回的最终文件名 */
  name: string;
  size: number;
  isImage: boolean;
}

const IMAGE_EXTS = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];
const SOURCE_HINTS = [
  { value: "", label: "来源 · auto" },
  { value: "mobile-memo", label: "📱 手机备忘录" },
  { value: "weibo", label: "🔥 微博" },
  { value: "wechat", label: "💬 微信对话" },
  { value: "chat", label: "🤖 AI 对话导出" },
  { value: "web", label: "🌐 网页剪藏" },
  { value: "idea", label: "💡 灵感" },
  { value: "reading", label: "📖 读书" },
];

function isImageFilename(name: string): boolean {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  return IMAGE_EXTS.includes(ext);
}

// =====================================================================
// 主组件
// =====================================================================

export function Inbox() {
  const active = useWorkflowVisibility();
  const source = useInboxSource();
  const { text, title, tags_input: tagsInput, source_hint: sourceHint, attachment_unconfirmed: uploadHasError } = source.draft;
  const attachments = source.draft.attachments.map(a => ({ ...a, isImage: isImageFilename(a.name) }));
  const setText = (text: string) => editInboxSource({ text });
  const setTitle = (title: string) => editInboxSource({ title });
  const setTagsInput = (tags_input: string) => editInboxSource({ tags_input });
  const setSourceHint = (source_hint: string) => editInboxSource({ source_hint });
  const setUploadHasError = (attachment_unconfirmed: boolean) => editInboxSource({ attachment_unconfirmed });
  const setAttachments = (update: (old: AttachmentItem[]) => AttachmentItem[]) => editInboxSource(d => ({ attachments: update(d.attachments.map(a => ({ ...a, isImage: isImageFilename(a.name) }))).map(({ rel, name, size }) => ({ rel, name, size })) }));
  const [uploading, setUploading] = useState(0);
  const activity = useRef({ uploading: 0, submitting: false });
  const [isDragging, setIsDragging] = useState(false);
  const [lastResult, setLastResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);
  const dragCounter = useRef(0);

  const qc = useQueryClient();

  const recentQuery = useQuery({
    queryKey: ["inbox", "recent"],
    queryFn: () => inboxApi.listRecent(10),
    enabled: active,
    refetchOnWindowFocus: false,
  });

  // Reserve the whole batch before awaiting any file; no gap may permit a
  // submission or handoff with only part of the selected material.
  const uploadFiles = useCallback((files: File[]) => trackInboxUpload((async () => {
    if (files.length === 0) return;
    if (!source.loaded || source.conflict) return;
    if (activity.current.submitting) {
      setLastResult({
        ok: false,
        message: "当前内容正在收录，请完成后再添加附件。",
      });
      return;
    }
    activity.current.uploading += files.length;
    setUploading(activity.current.uploading);
    let saved = 0;
    const failures: string[] = [];
    for (const file of files) {
      try {
        const buf = await file.arrayBuffer();
        const bytes = Array.from(new Uint8Array(buf));
        const filename =
          file.name && file.name.trim().length > 0
            ? file.name
            : `${file.type.startsWith("image/") ? "screenshot" : "file"}-${Date.now()}.bin`;

        const result: InboxAttachResult = await inboxApi.attach({
          filename,
          bytes,
        });

        setAttachments((prev) => [
          ...prev,
          {
            rel: result.relative_path,
            name: result.final_filename,
            size: result.size,
            isImage: isImageFilename(result.final_filename),
          },
        ]);
        saved += 1;
      } catch (e) {
        failures.push(`${file.name || "未命名附件"}：${String(e)}`);
        setUploadHasError(true);
      } finally {
        activity.current.uploading -= 1;
        setUploading(activity.current.uploading);
      }
    }
    setLastResult(
      failures.length
        ? {
            ok: false,
            message: `${saved} 个附件已保存，${failures.length} 个未完成。${failures.join("；")}。未自动重传，请先核对原附件。`,
          }
        : { ok: true, message: `${saved} 个附件已保存，原上传日期已保留。` },
    );
  })()), [source.loaded, source.conflict]);

  const submitMutation = useMutation({
    mutationFn: submitInboxSource,
    onSuccess: () => {
      setLastResult(null);
      qc.invalidateQueries({ queryKey: ["inbox", "recent"] });
      qc.invalidateQueries({ queryKey: ["wiki-agent", "status"] });
    },
    onError: (e) =>
      setLastResult({ ok: false, message: `投递失败：${String(e)}` }),
    onSettled: () => {
      activity.current.submitting = false;
    },
  });

  const requestSubmit = useCallback(() => {
    if (activity.current.submitting || activity.current.uploading > 0) return;
    if (!source.loaded || source.busy || source.conflict || source.unconfirmedSave || source.entry?.pending || source.attempt) return;
    if (!text.trim() && attachments.length === 0) return;
    activity.current.submitting = true;
    submitMutation.mutate();
  }, [text, attachments, submitMutation, source]);

  const openObsidianMutation = useMutation({
    mutationFn: () => inboxApi.openObsidian("Wiki"),
    onSuccess: () =>
      setLastResult({ ok: true, message: "已通过 obsidian:// 协议唤起" }),
    onError: (e) =>
      setLastResult({
        ok: false,
        message: `打开 Obsidian 失败：${String(e)}`,
      }),
  });

  // Ctrl+Enter / Cmd+Enter 投递
  useEffect(() => {
    if (!active) return;
    const handler = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        if (
          (text.trim().length > 0 || attachments.length > 0) &&
          !submitMutation.isPending
        ) {
          requestSubmit();
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [
    active,
    text,
    attachments.length,
    submitMutation.isPending,
    requestSubmit,
  ]);

  // 粘贴处理（textarea / 全局）——从 clipboardData.items 取图片
  const handlePaste = useCallback(
    async (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
      const items = e.clipboardData?.items;
      if (!items) return;
      const files: File[] = [];
      for (let i = 0; i < items.length; i++) {
        const it = items[i];
        if (it.kind === "file") {
          const f = it.getAsFile();
          if (f) files.push(f);
        }
      }
      if (files.length === 0) return;
      e.preventDefault(); // 阻止默认粘贴二进制到 textarea
      await uploadFiles(files);
    },
    [uploadFiles],
  );

  // 拖拽处理 (dropzone)
  const handleDragEnter = (e: React.DragEvent) => {
    e.preventDefault();
    dragCounter.current += 1;
    if (e.dataTransfer.types.includes("Files")) {
      setIsDragging(true);
    }
  };
  const handleDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    dragCounter.current -= 1;
    if (dragCounter.current <= 0) {
      setIsDragging(false);
      dragCounter.current = 0;
    }
  };
  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
  };
  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      e.preventDefault();
      setIsDragging(false);
      dragCounter.current = 0;
      const files = Array.from(e.dataTransfer.files);
      await uploadFiles(files);
    },
    [uploadFiles],
  );

  const removeAttachment = (rel: string) => {
    // 仅从 UI state 移除——后端文件已在 _attachments/ 里，按不可变原则不删
    setAttachments((prev) => prev.filter((a) => a.rel !== rel));
  };

  const charCount = text.length;
  const canSubmit =
    (text.trim().length > 0 || attachments.length > 0) &&
    source.loaded && !source.busy && !source.conflict && !source.unconfirmedSave && !source.entry?.pending && !source.attempt &&
    !submitMutation.isPending &&
    uploading === 0;

  return (
    <div className="mx-auto max-w-4xl space-y-6">
      <header>
        <h1 className="flex items-center gap-2 font-serif text-2xl font-semibold">
          <Sparkles className="h-6 w-6 text-primary-600" />
          投料口
        </h1>
        <p className="mt-1.5 text-sm text-muted-foreground">
          整理资料，收录到当前资料库，按日期保存至{" "}
          <code className="rounded bg-muted/60 px-1 py-0.5 font-mono text-[11px]">
            raw/inbox/YYYY-MM-DD/
          </code>
        </p>
      </header>

      <WorkflowAssistantEntry
        title="在助理中整理资料"
        description="先将当前内容和已有附件引用保存为独立草稿，确认后再发送；原投料流程保持可用。"
        disabled={
          !source.loaded || source.conflict || source.busy ||
          submitMutation.isPending ||
          uploading > 0 ||
          (!text.trim() && !title.trim() && attachments.length === 0)
        }
        getText={() => {
          if (activity.current.uploading > 0 || activity.current.submitting)
            throw Error("资料仍在保存，请完成后再交给助理。");
          return inboxHandoff({
            text,
            title,
            tags: tagsInput,
            source: sourceHint,
            attachments,
            uncertain: !!source.entry?.pending || !!source.attempt || submitMutation.isError || uploadHasError,
            ...((source.entry?.pending ?? source.attempt) ? { receipt: {
              submission_id: (source.entry?.pending ?? source.attempt)!.submission_id,
              state: source.submission?.submission_id === (source.entry?.pending ?? source.attempt)!.submission_id ? source.submission.state : "unknown" as const,
              matches_current: JSON.stringify(source.draft) === JSON.stringify((source.entry?.pending ?? source.attempt)!.snapshot),
            } } : {}),
          });
        }}
      />

      <div role="status" aria-label="资料草稿状态" className="space-y-2 text-xs text-muted-foreground">
        <p>{source.loading ? "正在恢复资料草稿…" : !source.loaded ? "资料草稿尚未读取，原内容未覆盖。" : source.dirty ? source.unconfirmedSave ? "当前输入保留，自动保存已暂停。" : source.error ? "当前输入保留，保存尚未确认。" : "当前输入正在本机受保护保存…" : source.entry?.revision === 0 ? "直接输入资料，编辑后会自动在本机保存。" : "资料草稿已在本机受保护保存。"}</p>
        {source.error && <p className="text-amber-600 dark:text-amber-400">{source.error}</p>}
        {source.submission && <p>{source.submission.message}</p>}
        <div className="flex flex-wrap gap-3">
          <button type="button" disabled={source.busy} title="重新核对已保存内容，不会提交或继续任务" onClick={() => { void inspectInboxSource().then(() => { setLastResult(null); qc.invalidateQueries({ queryKey: ["inbox", "recent"] }); qc.invalidateQueries({ queryKey: ["wiki-agent", "status"] }); }).catch(() => {}); }} className="underline underline-offset-4">{source.entry?.pending || source.attempt || source.unconfirmedSave || source.error || !source.loaded ? "核对原收录与草稿" : "刷新资料草稿"}</button>
          {source.unconfirmedSave && !source.conflict && <button type="button" disabled={source.busy} onClick={() => { void retryInboxSourceSave().catch(() => {}); }} className="underline underline-offset-4">重试保存资料草稿</button>}
          {!source.conflict && (source.entry?.pending?.stage === "reserved" || source.submission?.state === "prepared" || (!source.entry?.pending && source.attempt)) && <button type="button" disabled={source.busy} onClick={() => { void continueInboxSource().then(() => { setLastResult(null); qc.invalidateQueries({ queryKey: ["inbox", "recent"] }); qc.invalidateQueries({ queryKey: ["wiki-agent", "status"] }); }).catch(() => {}); }} className="underline underline-offset-4">{source.entry?.pending?.stage === "ready" ? "继续原收录" : "完成原准备"}</button>}
          {!source.conflict && source.entry?.pending && (source.entry.pending.stage === "reserved" || source.submission?.state === "prepared") && <button type="button" disabled={source.busy} onClick={() => { void cancelInboxSource().catch(() => {}); }} className="underline underline-offset-4">取消本次准备，保留输入</button>}
          {source.conflict && <><button type="button" onClick={() => { void chooseInboxVersion(true).catch(() => {}); }} className="underline underline-offset-4">以当前输入保存</button><button type="button" onClick={() => { void chooseInboxVersion(false); }} className="underline underline-offset-4">采用已保存版本</button></>}
        </div>
      </div>

      {/* ============ 投料表单 ============ */}
      <section
        className={cn(
          "relative rounded-xl border bg-card p-5 shadow-sm transition-colors",
          isDragging
            ? "border-primary-500 bg-primary-50/40 dark:bg-primary-950/40"
            : "border-border",
        )}
        onDragEnter={handleDragEnter}
        onDragLeave={handleDragLeave}
        onDragOver={handleDragOver}
        onDrop={handleDrop}
      >
        {/* 拖拽遮罩层 */}
        {isDragging && (
          <div className="pointer-events-none absolute inset-0 z-10 flex flex-col items-center justify-center rounded-xl border-2 border-dashed border-primary-500 bg-primary-50/80 backdrop-blur-sm dark:bg-primary-950/80">
            <FileUp className="mb-2 h-8 w-8 text-primary-600 dark:text-primary-400" />
            <p className="font-medium text-primary-800 dark:text-primary-200">
              松开鼠标上传
            </p>
            <p className="mt-1 text-xs text-primary-700/80 dark:text-primary-300/80">
              文件会落到 raw/inbox/&lt;今天&gt;/_attachments/
            </p>
          </div>
        )}

        <div className="space-y-3">
          <input
            type="text"
            disabled={!source.loaded}
            placeholder="标题（可选 · 空着会用首行前 40 字）"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            className="w-full rounded-md border border-border bg-background px-3 py-2 text-sm placeholder:text-muted-foreground/60 focus:border-primary-400 focus:outline-none"
          />

          <textarea
            disabled={!source.loaded}
            placeholder={`想到什么就写…\n直接粘贴微博 / 备忘录 / 公众号 / 对话 / 任何东西\n可拖拽 · 可粘贴截图 / 文件\n按 Ctrl+Enter 投递`}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onPaste={handlePaste}
            rows={12}
            className="w-full resize-y rounded-md border border-border bg-background px-3 py-2 text-sm leading-relaxed placeholder:text-muted-foreground/60 focus:border-primary-400 focus:outline-none"
            autoFocus
          />

          {/* 附件列表 */}
          {attachments.length > 0 && (
            <div className="rounded-md border border-border/60 bg-muted/30 p-2.5">
              <div className="mb-2 flex items-center justify-between">
                <span className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
                  <Paperclip className="h-3 w-3" />
                  附件（{attachments.length}）
                </span>
                <span className="font-mono text-[10px] text-muted-foreground/70">
                  保留原上传日期
                </span>
              </div>
              <div className="flex flex-wrap gap-2">
                {attachments.map((a) => (
                  <AttachmentChip
                    key={a.rel}
                    item={a}
                    disabled={submitMutation.isPending}
                    onRemove={() => removeAttachment(a.rel)}
                  />
                ))}
              </div>
            </div>
          )}

          {/* 附件上传提示 + 手动选文件按钮 */}
          <div className="flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
            <span className="flex items-center gap-1.5">
              <FileUp className="h-3 w-3" />
              拖文件进来 · 或在文本区粘贴截图 / 文件
            </span>
            <label className="inline-flex cursor-pointer items-center gap-1 rounded-md border border-border bg-card px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:border-primary-400 hover:text-foreground">
              <Paperclip className="h-3 w-3" />
              选文件
              <input
                type="file"
                multiple
                disabled={submitMutation.isPending}
                className="hidden"
                onChange={async (e) => {
                  const files = Array.from(e.target.files ?? []);
                  const input = e.currentTarget;
                  await uploadFiles(files);
                  input.value = ""; // 重置，便于同名重选
                }}
              />
            </label>
          </div>

          {(uploading > 0 || submitMutation.isPending) && (
            <p
              role="status"
              aria-label="资料保存进度"
              aria-live="polite"
              className="text-xs text-muted-foreground"
            >
              {uploading > 0
                ? `正在保存 ${uploading} 个附件，完成后可收录或交给助理。`
                : "正在收录已提交的内容，你现在补充的文字会保留。"}
            </p>
          )}
          {uploadHasError && uploading === 0 && (
            <p
              role="status"
              aria-label="附件保存状态"
              className="text-xs text-amber-600 dark:text-amber-400"
            >
              有附件尚未确认保存，请先核对原附件；交给助理时会保留待核对状态，不会自动重传。
            </p>
          )}

          <div className="flex flex-wrap items-center gap-3">
            <input
              type="text"
              disabled={!source.loaded}
              placeholder="标签（空格 / 逗号分隔，如 idea tech reading）"
              value={tagsInput}
              onChange={(e) => setTagsInput(e.target.value)}
              className="min-w-[200px] flex-1 rounded-md border border-border bg-background px-3 py-1.5 text-xs placeholder:text-muted-foreground/60 focus:border-primary-400 focus:outline-none"
            />
            <select
              aria-label="资料来源"
              title={sourceHint || "自动判断来源"}
              disabled={!source.loaded}
              value={sourceHint}
              onChange={(e) => setSourceHint(e.target.value)}
              className="min-w-0 max-w-full rounded-md border border-border bg-background px-2 py-1.5 text-xs focus:border-primary-400 focus:outline-none sm:max-w-64"
            >
              {!SOURCE_HINTS.some(s => s.value === sourceHint) && <option value={sourceHint}>来源 · {sourceHint}（自定义）</option>}
              {SOURCE_HINTS.map(s => <option key={s.value} value={s.value}>{s.label}</option>)}
            </select>
            <span className="font-mono text-[11px] text-muted-foreground">
              {charCount} 字
            </span>
          </div>
        </div>

        {/* 操作 + 状态 */}
        <div className="mt-5 flex items-center justify-between gap-4 border-t border-border/60 pt-4">
          <div className="min-w-0 flex-1 text-xs">
            {lastResult && (
              <span
                className={cn(
                  "truncate",
                  lastResult.ok
                    ? "text-emerald-600 dark:text-emerald-400"
                    : "text-amber-600 dark:text-amber-400",
                )}
                title={lastResult.message}
              >
                {lastResult.ok ? "✅" : "⚠️"} {lastResult.message}
              </span>
            )}
          </div>
          <div className="flex shrink-0 gap-2">
            <button
              type="button"
              onClick={() => openObsidianMutation.mutate()}
              disabled={openObsidianMutation.isPending}
              className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:border-primary-400 hover:text-foreground disabled:cursor-wait disabled:opacity-60"
              title="用 obsidian:// URI scheme 唤起应用 / 查看原生知识图谱"
            >
              {openObsidianMutation.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <BookOpen className="h-3.5 w-3.5" />
              )}
              打开 Obsidian
            </button>
            <button
              type="button"
              onClick={requestSubmit}
              disabled={!canSubmit}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md px-4 py-1.5 text-xs font-medium transition-colors",
                canSubmit
                  ? "bg-primary-600 text-white shadow-sm hover:bg-primary-700"
                  : "cursor-not-allowed bg-muted text-muted-foreground",
              )}
              title="Ctrl+Enter 也可投递"
            >
              {submitMutation.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <Send className="h-3.5 w-3.5" />
              )}
              投递
              <kbd className="ml-1 rounded bg-white/20 px-1 font-mono text-[9px]">
                Ctrl+↵
              </kbd>
            </button>
          </div>
        </div>
      </section>

      {/* ============ Wiki Agent 消化区块 ============ */}
      <WikiAgentPanel />

      {/* ============ 最近投料 ============ */}
      <section className="rounded-xl border border-border bg-card">
        <div className="flex items-center justify-between border-b border-border/60 px-4 py-3">
          <h2 className="font-serif text-base font-semibold">📜 最近投料</h2>
          <span className="font-mono text-[10px] text-muted-foreground">
            {recentQuery.data?.length ?? 0} 条 · 等 AI 消化
          </span>
        </div>
        <div className="divide-y divide-border/60">
          {recentQuery.isLoading && (
            <div className="px-4 py-8 text-center text-sm text-muted-foreground">
              <Loader2 className="mx-auto mb-2 h-4 w-4 animate-spin" />
              加载…
            </div>
          )}
          {!recentQuery.isLoading && (recentQuery.data?.length ?? 0) === 0 && (
            <div className="px-4 py-8 text-center text-sm text-muted-foreground">
              还没有投料 — 在上面写点什么试试 ☝
            </div>
          )}
          {recentQuery.data?.map((entry) => (
            <RecentEntryRow key={entry.path} entry={entry} />
          ))}
        </div>
      </section>
    </div>
  );
}

// =====================================================================
// 最近投料的一行
// =====================================================================

function RecentEntryRow({ entry }: { entry: InboxRecentEntry }) {
  const handleOpen = async () => {
    try {
      // 复用 topology_open_dir 在 explorer 高亮该文件 / 默认 app 打开
      await topologyApi.openDir(entry.path);
    } catch (e) {
      console.error("打开文件失败：", e);
    }
  };

  return (
    <div className="group flex items-start gap-3 px-4 py-3 transition-colors hover:bg-muted/40">
      <FileText className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <code className="font-mono text-[11px] text-foreground">
            {entry.day}/{entry.filename}
          </code>
          <span className="font-mono text-[10px] text-muted-foreground">
            {formatBytes(entry.size)}
          </span>
        </div>
        <p className="mt-1 line-clamp-2 text-sm text-foreground/80">
          {entry.preview || "(空文件)"}
        </p>
        <div className="mt-1 flex items-center gap-1 text-[10px] text-muted-foreground">
          <Clock className="h-3 w-3" />
          {entry.modified_at}
        </div>
      </div>
      <button
        type="button"
        onClick={handleOpen}
        className="shrink-0 rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-muted hover:text-foreground group-hover:opacity-100"
        title="在系统默认应用打开"
      >
        <ExternalLink className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

function formatBytes(b: number): string {
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / 1024 / 1024).toFixed(2)} MB`;
}

// =====================================================================
// 附件 chip — 图片显示缩略名 + size，文件显示文件名 + size
// =====================================================================

function AttachmentChip({
  item,
  onRemove,
  disabled,
}: {
  item: AttachmentItem;
  onRemove: () => void;
  disabled: boolean;
}) {
  const Icon = item.isImage ? ImageIcon : Paperclip;
  return (
    <div className="group inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-2 py-1 text-[11px]">
      <Icon className="h-3 w-3 shrink-0 text-muted-foreground" />
      <span className="max-w-[180px] truncate" title={item.rel}>
        {item.name}
      </span>
      <span className="font-mono text-[10px] text-muted-foreground/70">
        {item.rel.match(
          /^raw\/inbox\/(\d{4}-\d{2}-\d{2})\/_attachments\//,
        )?.[1] ?? "日期待核对"}
      </span>
      <span className="font-mono text-[10px] text-muted-foreground/70">
        {formatBytes(item.size)}
      </span>
      <button
        type="button"
        onClick={onRemove}
        disabled={disabled}
        className="ml-0.5 rounded p-0.5 text-muted-foreground opacity-60 transition-colors hover:bg-muted hover:text-foreground hover:opacity-100"
        title="从此次投料里移除（文件已在 _attachments/ 不会删除）"
      >
        <X className="h-3 w-3" />
      </button>
    </div>
  );
}

// =====================================================================
// Wiki Agent 消化面板（方案 A · v1 stub）
// =====================================================================
//
// 当前 v1：
// - status 显示未消化文件数 / vault 路径 / AGENTS.md 是否存在
// - 「立即消化」按钮 = 生成 ingest prompt → 复制到剪贴板 + 打开终端 + 弹示意
//   （用户在终端跑 `claude` / `codex` 粘贴 prompt 即可消化）
// - v2（settings 子页选完 backend）：spawn 子进程直接跑 headless

/** 解析一行 Claude Code stream-json，提取一句人能看懂的摘要（容错） */
function summarizeStreamLine(rawLine: string): {
  label: string;
  detail?: string;
  tone: "info" | "muted" | "success" | "warning" | "danger";
} {
  try {
    const obj = JSON.parse(rawLine);
    const type = obj.type as string | undefined;
    const subtype = obj.subtype as string | undefined;

    if (type === "system" && subtype === "init") {
      return { label: "系统初始化", detail: obj.session_id, tone: "muted" };
    }
    if (type === "system" && subtype === "api_retry") {
      return {
        label: `API 重试 ${obj.attempt}/${obj.max_retries}`,
        detail: obj.error,
        tone: "warning",
      };
    }
    if (type === "assistant" && obj.message) {
      const blocks = obj.message?.content as Array<{ type: string; name?: string; text?: string }> | undefined;
      const toolCall = blocks?.find((b) => b.type === "tool_use");
      if (toolCall) {
        return {
          label: `调用工具 · ${toolCall.name ?? "?"}`,
          tone: "info",
        };
      }
      const text = blocks?.find((b) => b.type === "text")?.text;
      if (text) {
        return {
          label: "AI 思考",
          detail: text.slice(0, 200),
          tone: "muted",
        };
      }
      return { label: "AI 消息", tone: "muted" };
    }
    if (type === "user" && obj.message) {
      // tool_result
      return { label: "工具返回", tone: "muted" };
    }
    if (type === "result" && subtype === "success") {
      return { label: "✓ 任务完成", detail: `cost ${obj.total_cost_usd ?? "?"} USD`, tone: "success" };
    }
    if (type === "result") {
      return { label: `结果 · ${subtype ?? "?"}`, tone: "warning" };
    }
    if (type === "stream_event") {
      // partial text delta — 噪音多，跳过
      return { label: "", tone: "muted" };
    }
    return { label: type ?? "未知事件", detail: subtype, tone: "muted" };
  } catch {
    return { label: "(无法解析的行)", detail: rawLine.slice(0, 100), tone: "muted" };
  }
}

interface IngestRunningState {
  taskId: string;
  events: WikiAgentEvent[];
  startedAt: number;
}

function WikiAgentPanel() {
  const active=useWorkflowVisibility();
  const qc = useQueryClient();
  const [stage, setStage] = useState<
    | { type: "idle" }
    | { type: "preparing" }
    | { type: "ready"; prompt: string; targetCount: number }
    | { type: "running"; running: IngestRunningState }
    | { type: "completed"; running: IngestRunningState; exitCode: number }
    | { type: "failed"; running: IngestRunningState; message: string }
    | { type: "cancelled"; running: IngestRunningState }
    | { type: "error"; message: string }
  >({ type: "idle" });
  const [copied, setCopied] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [logExpanded, setLogExpanded] = useState(true);
  const logScrollRef = useRef<HTMLDivElement>(null);

  const statusQuery = useQuery({
    queryKey: ["wiki-agent", "status"],
    queryFn: () => wikiAgentApi.status(),
    refetchOnWindowFocus: false,
    refetchInterval: active ? 30_000 : false,
    enabled: active,
  });

  // 全局订阅 wiki-agent:event；按 currentTaskId 过滤后 append 到 events
  const currentTaskId =
    stage.type === "running" || stage.type === "completed" || stage.type === "failed" || stage.type === "cancelled"
      ? stage.running.taskId
      : null;
  useEffect(() => {
    if (!currentTaskId) return;
    let disposed = false;
    let terminal = false;
    let unlisten: (() => void) | undefined;
    listen<WikiAgentEvent>(WIKI_AGENT_EVENT, (e) => {
      const ev = e.payload;
      if (disposed || terminal || ev.task_id !== currentTaskId) return;
      if (ev.kind === "completed" || ev.kind === "failed" || ev.kind === "cancelled") terminal = true;
      setStage((prev) => {
        // 只在 running 阶段累加事件
        if (prev.type !== "running") return prev;
        const next: IngestRunningState = {
          ...prev.running,
          events: [...prev.running.events, ev],
        };
        if (ev.kind === "completed") {
          return { type: "completed", running: next, exitCode: ev.exit_code };
        }
        if (ev.kind === "failed") {
          return { type: "failed", running: next, message: ev.message };
        }
        if (ev.kind === "cancelled") {
          return { type: "cancelled", running: next };
        }
        return { type: "running", running: next };
      });
      if (ev.kind === "completed") {
        void qc.invalidateQueries({ queryKey: ["wiki-agent", "status"] });
        void qc.invalidateQueries({ queryKey: ["inbox", "recent"] });
        void qc.invalidateQueries({ queryKey: ["wiki"] });
        void qc.invalidateQueries({ queryKey: ["docs"] });
      }
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [currentTaskId, qc]);

  // 日志区域自动滚到底
  useEffect(() => {
    if (logScrollRef.current) {
      logScrollRef.current.scrollTop = logScrollRef.current.scrollHeight;
    }
  }, [stage]);

  const prepareMutation = useMutation({
    mutationFn: () => wikiAgentApi.preparePrompt(),
    onMutate: () => {
      setStage({ type: "preparing" });
      setCopied(false);
    },
    onSuccess: (result) => {
      setStage({
        type: "ready",
        prompt: result.prompt,
        targetCount: result.target_files.length,
      });
    },
    onError: (e) =>
      setStage({ type: "error", message: `生成 prompt 失败：${String(e)}` }),
  });

  const runIngestMutation = useMutation({
    mutationFn: () => wikiAgentApi.runIngest(),
    onSuccess: (result) => {
      setStage({
        type: "running",
        running: {
          taskId: result.task_id,
          events: [],
          startedAt: Date.now(),
        },
      });
    },
    onError: (e) =>
      setStage({ type: "error", message: `启动消化失败：${String(e)}` }),
  });

  const cancelIngestMutation = useMutation({
    mutationFn: (taskId: string) => wikiAgentApi.cancelIngest(taskId),
  });

  const openTerminalMutation = useMutation({
    mutationFn: () => wikiAgentApi.openInTerminal(),
  });

  const handleCopy = async () => {
    if (stage.type !== "ready") return;
    try {
      await navigator.clipboard.writeText(stage.prompt);
      setCopied(true);
      setTimeout(() => setCopied(false), 2500);
    } catch (e) {
      console.error("复制失败：", e);
    }
  };

  const status = statusQuery.data;
  const pending = status?.pending_count ?? 0;
  const isLoading = statusQuery.isLoading;
  const hasAgentsMd = status?.has_agents_md ?? false;

  return (
    <>
    <section className="rounded-xl border border-border bg-card shadow-sm">
      {/* 标题栏 */}
      <header className="flex items-center justify-between border-b border-border/60 px-5 py-3">
        <div className="flex items-center gap-2">
          <Database className="h-4 w-4 text-primary-600 dark:text-primary-400" />
          <h2 className="font-serif text-base font-semibold">Wiki Agent · 消化</h2>
          <span className="rounded-full border border-border bg-muted/40 px-2 py-0.5 font-mono text-[10px] text-muted-foreground">
            {status?.backend ?? "claude-code"}
          </span>
        </div>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => setSettingsOpen(true)}
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="配置 Backend AI · 自动消化"
          >
            <SettingsIcon className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            onClick={() => qc.invalidateQueries({ queryKey: ["wiki-agent", "status"] })}
            className="rounded p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="刷新状态"
          >
            <RefreshCw
              className={cn("h-3.5 w-3.5", statusQuery.isFetching && "animate-spin")}
            />
          </button>
        </div>
      </header>

      {/* 状态卡 */}
      <div className="grid grid-cols-3 gap-3 border-b border-border/60 px-5 py-4">
        <StatBlock
          label="未消化"
          value={isLoading ? "…" : `${pending}`}
          tone={pending > 0 ? "warning" : "neutral"}
          hint={pending > 0 ? "等待 AI 处理" : "已全部消化"}
        />
        <StatBlock
          label="已消化"
          value={isLoading ? "…" : `${status?.digested_count ?? 0}`}
          tone="success"
          hint={`共 ${status?.total_count ?? 0} 条投料`}
        />
        <StatBlock
          label="最后投料"
          value={status?.last_inbox_at?.split(" ")[1]?.slice(0, 5) ?? "—"}
          tone="neutral"
          hint={status?.last_inbox_at?.split(" ")[0] ?? "暂无"}
        />
      </div>

      {/* AGENTS.md 缺失警告 */}
      {!isLoading && !hasAgentsMd && (
        <div className="flex items-start gap-2 border-b border-border/60 bg-amber-50/60 px-5 py-3 text-xs text-amber-900 dark:bg-amber-950/40 dark:text-amber-200">
          <AlertCircle className="mt-0.5 h-4 w-4 shrink-0" />
          <div>
            <p className="font-medium">未找到 AGENTS.md</p>
            <p className="mt-0.5 text-amber-900/80 dark:text-amber-200/80">
              vault 根目录 <code className="font-mono">{status?.vault_root}</code>{" "}
              下没有 AGENTS.md，AI 不知道如何消化。请先建一份（参考
              <code className="mx-1 font-mono">_schema/NAMING_CONVENTION.md</code>
              ）。
            </p>
          </div>
        </div>
      )}

      {/* 操作区 */}
      <div className="px-5 py-4">
        {stage.type === "idle" && (
          <div className="space-y-3">
            <div className="flex items-center justify-between gap-3">
              <p className="text-xs text-muted-foreground">
                点【立即消化】→ HomeBase 自动跑 <code className="font-mono text-[10.5px]">claude -p --output-format stream-json</code>
                ，按 AGENTS.md 4 阶段把 raw/inbox/ 消化进 wiki/，实时显示进度。
              </p>
              <button
                type="button"
                onClick={() => runIngestMutation.mutate()}
                disabled={pending === 0 || runIngestMutation.isPending}
                className={cn(
                  "inline-flex shrink-0 items-center gap-1.5 rounded-md px-3.5 py-1.5 text-xs font-medium transition-colors",
                  pending > 0
                    ? "bg-primary-600 text-white shadow-sm hover:bg-primary-700"
                    : "cursor-not-allowed bg-muted text-muted-foreground",
                )}
                title={
                  pending === 0
                    ? "没有未消化的投料，先去上面写点东西"
                    : "spawn claude 子进程自动消化"
                }
              >
                {runIngestMutation.isPending ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <PlayCircle className="h-3.5 w-3.5" />
                )}
                {runIngestMutation.isPending ? "启动中…" : "立即消化"}
              </button>
            </div>
            <div className="flex items-center gap-2 text-[10.5px] text-muted-foreground">
              <span>或者：</span>
              <button
                type="button"
                onClick={() => prepareMutation.mutate()}
                disabled={pending === 0 || prepareMutation.isPending}
                className="inline-flex items-center gap-1 rounded border border-border/60 px-2 py-0.5 hover:border-primary-400 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
                title="生成 prompt 复制到外部 Claude Code / Codex 终端跑"
              >
                <Sparkles className="h-3 w-3" />
                复制 prompt 手动跑（fallback）
              </button>
            </div>
          </div>
        )}

        {stage.type === "preparing" && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            扫描 raw/inbox/ 未消化文件 + 渲染 prompt…
          </div>
        )}

        {stage.type === "error" && (
          <div className="space-y-2">
            <div className="flex items-start gap-2 rounded-md border border-amber-300/60 bg-amber-50/60 px-3 py-2 text-xs text-amber-900 dark:bg-amber-950/40 dark:text-amber-200">
              <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>{stage.message}</span>
            </div>
            <button
              type="button"
              onClick={() => setStage({ type: "idle" })}
              className="text-[11px] text-muted-foreground underline-offset-2 hover:underline"
            >
              返回
            </button>
          </div>
        )}

        {stage.type === "ready" && (
          <div className="space-y-3">
            <div className="flex items-center justify-between gap-2 text-xs">
              <span className="flex items-center gap-1.5 text-emerald-600 dark:text-emerald-400">
                <CheckCircle2 className="h-3.5 w-3.5" />
                已生成 prompt — 涉及 {stage.targetCount} 个 raw 文件
              </span>
              <button
                type="button"
                onClick={() => setStage({ type: "idle" })}
                className="text-muted-foreground hover:text-foreground"
              >
                重新生成
              </button>
            </div>

            <pre className="max-h-64 overflow-auto rounded-md border border-border bg-muted/30 p-3 font-mono text-[11px] leading-relaxed text-foreground">
              {stage.prompt}
            </pre>

            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={handleCopy}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
                  copied
                    ? "bg-emerald-600 text-white"
                    : "bg-primary-600 text-white hover:bg-primary-700",
                )}
              >
                {copied ? (
                  <CheckCircle2 className="h-3.5 w-3.5" />
                ) : (
                  <Copy className="h-3.5 w-3.5" />
                )}
                {copied ? "已复制" : "复制 prompt"}
              </button>
              <button
                type="button"
                onClick={() => openTerminalMutation.mutate()}
                disabled={openTerminalMutation.isPending}
                className="inline-flex items-center gap-1.5 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:border-primary-400 hover:text-foreground disabled:cursor-wait disabled:opacity-60"
                title="在 vault 目录下打开终端（让你跑 claude / codex）"
              >
                <Terminal className="h-3.5 w-3.5" />
                在 vault 打开终端
              </button>
              <span className="ml-auto text-[10px] text-muted-foreground">
                复制后，在终端跑 <code className="font-mono">claude</code> /{" "}
                <code className="font-mono">codex</code> 粘贴执行
              </span>
            </div>
          </div>
        )}

        {(stage.type === "running" ||
          stage.type === "completed" ||
          stage.type === "failed" ||
          stage.type === "cancelled") && (
          <IngestLogPanel
            stage={stage}
            logExpanded={logExpanded}
            toggleLog={() => setLogExpanded((v) => !v)}
            logScrollRef={logScrollRef}
            onCancel={() =>
              cancelIngestMutation.mutate(stage.running.taskId)
            }
            onRestart={() => setStage({ type: "idle" })}
            cancelling={cancelIngestMutation.isPending}
          />
        )}
      </div>

      {/* footer */}
      <footer className="flex items-center justify-between border-t border-border/60 bg-muted/20 px-5 py-2 text-[10px] text-muted-foreground">
        <span>
          配置在右上角齿轮 · backend / 定时消化 · v2 调度由 tokio scheduler 接
        </span>
        <span className="font-mono opacity-60">~/.homebase/wiki_agent.json</span>
      </footer>
    </section>

    <WikiAgentSettingsDrawer
      open={settingsOpen}
      onClose={() => setSettingsOpen(false)}
    />
    </>
  );
}

// =====================================================================
// Ingest 流式日志面板 — running / completed / failed / cancelled 共用
// =====================================================================

type IngestStageWithRunning =
  | { type: "running"; running: IngestRunningState }
  | { type: "completed"; running: IngestRunningState; exitCode: number }
  | { type: "failed"; running: IngestRunningState; message: string }
  | { type: "cancelled"; running: IngestRunningState };

function IngestLogPanel({
  stage,
  logExpanded,
  toggleLog,
  logScrollRef,
  onCancel,
  onRestart,
  cancelling,
}: {
  stage: IngestStageWithRunning;
  logExpanded: boolean;
  toggleLog: () => void;
  logScrollRef: React.RefObject<HTMLDivElement | null>;
  onCancel: () => void;
  onRestart: () => void;
  cancelling: boolean;
}) {
  const running = stage.running;
  const now = useClock(1_000, stage.type === "running");
  const elapsedSec = Math.floor((now - running.startedAt) / 1000);

  // 渲染用的事件流（过滤掉 stream_event partial 这类空 label 的）
  const renderableEvents = useMemo(() => running.events
    .map((ev) => {
      if (ev.kind === "started") {
        return {
          kind: "meta" as const,
          label: `任务启动 · ${ev.target_count} 个文件 · backend=${ev.backend}`,
          tone: "info" as const,
        };
      }
      if (ev.kind === "stream") {
        const s = summarizeStreamLine(ev.line);
        if (!s.label) return null;
        return { kind: "stream" as const, label: s.label, detail: s.detail, tone: s.tone };
      }
      if (ev.kind === "stderr") {
        return {
          kind: "stderr" as const,
          label: "stderr",
          detail: ev.line,
          tone: "warning" as const,
        };
      }
      if (ev.kind === "completed") {
        return {
          kind: "meta" as const,
          label: `✓ 任务正常退出 (code ${ev.exit_code})`,
          tone: "success" as const,
        };
      }
      if (ev.kind === "failed") {
        return {
          kind: "meta" as const,
          label: `✗ 任务失败 · ${ev.message}`,
          tone: "danger" as const,
        };
      }
      if (ev.kind === "cancelled") {
        return {
          kind: "meta" as const,
          label: "✗ 已被用户中止",
          tone: "muted" as const,
        };
      }
      return null;
    })
    .filter((x): x is NonNullable<typeof x> => x !== null), [running.events]);

  // 统计：调用工具次数 + 重试次数
  const toolCallCount = renderableEvents.filter((e) => e.label.startsWith("调用工具")).length;
  const stderrCount = renderableEvents.filter((e) => e.kind === "stderr").length;

  // 状态条颜色 + 图标
  const { statusIcon, statusText, statusTone } = (() => {
    if (stage.type === "running") {
      return {
        statusIcon: <Loader2 className="h-3.5 w-3.5 animate-spin" />,
        statusText: "消化中",
        statusTone: "text-primary-700 dark:text-primary-200",
      };
    }
    if (stage.type === "completed") {
      return {
        statusIcon: <CheckCircle2 className="h-3.5 w-3.5" />,
        statusText: `已完成 (exit ${stage.exitCode})`,
        statusTone: "text-emerald-700 dark:text-emerald-300",
      };
    }
    if (stage.type === "failed") {
      return {
        statusIcon: <AlertCircle className="h-3.5 w-3.5" />,
        statusText: "失败",
        statusTone: "text-destructive-strong",
      };
    }
    return {
      statusIcon: <X className="h-3.5 w-3.5" />,
      statusText: "已中止",
      statusTone: "text-muted-foreground",
    };
  })();

  return (
    <div className="space-y-3">
      {/* 状态条 */}
      <div className="flex items-center justify-between gap-3 rounded-md border border-border/60 bg-muted/20 px-3 py-2 text-xs">
        <span className={cn("inline-flex items-center gap-1.5 font-medium", statusTone)}>
          {statusIcon}
          {statusText}
        </span>
        <div className="flex items-center gap-3 text-[10.5px] text-muted-foreground">
          <span>⏱ {elapsedSec}s</span>
          <span>🔧 {toolCallCount} 次工具</span>
          {stderrCount > 0 && (
            <span className="text-amber-600 dark:text-amber-400">⚠ {stderrCount} stderr</span>
          )}
          <code
            className="font-mono opacity-70"
            title={running.taskId}
          >
            #{running.taskId.slice(0, 6)}
          </code>
        </div>
      </div>

      {/* 失败消息 */}
      {stage.type === "failed" && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive-strong">
          <div className="font-medium">失败原因：</div>
          <div className="mt-0.5 font-mono text-[11px]">{stage.message}</div>
          <div className="mt-1 text-[10.5px] opacity-80">
            检查：claude CLI 是否在 PATH · 是否已登录（<code className="font-mono">claude /login</code>）· 网络 / API 配额
          </div>
        </div>
      )}

      {/* 日志区（可折叠） */}
      <div className="rounded-md border border-border/60 bg-card">
        <button
          type="button"
          onClick={toggleLog}
          className="flex w-full items-center justify-between px-3 py-1.5 text-[11px] text-muted-foreground hover:bg-muted/40"
        >
          <span className="inline-flex items-center gap-1.5">
            {logExpanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronUp className="h-3 w-3" />
            )}
            事件流（{renderableEvents.length} 条）
          </span>
          <span className="font-mono opacity-60">stream-json</span>
        </button>
        {logExpanded && (
          <div
            ref={logScrollRef}
            className="max-h-80 overflow-auto border-t border-border/60 bg-muted/10 px-3 py-2"
          >
            {renderableEvents.length === 0 ? (
              <div className="py-4 text-center text-[11px] italic text-muted-foreground">
                等待首条事件…
              </div>
            ) : (
              <ul className="space-y-1 font-mono text-[10.5px] leading-relaxed">
                {renderableEvents.map((ev, i) => (
                  <li key={i} className="flex items-start gap-2">
                    <span
                      className={cn(
                        "mt-[3px] h-1.5 w-1.5 shrink-0 rounded-full",
                        ev.tone === "success" && "bg-emerald-500",
                        ev.tone === "info" && "bg-primary-500",
                        ev.tone === "warning" && "bg-amber-500",
                        ev.tone === "danger" && "bg-destructive",
                        ev.tone === "muted" && "bg-muted-foreground/40",
                      )}
                    />
                    <div className="min-w-0 flex-1">
                      <span
                        className={cn(
                          "font-medium",
                          ev.tone === "success" && "text-emerald-700 dark:text-emerald-300",
                          ev.tone === "info" && "text-primary-700 dark:text-primary-200",
                          ev.tone === "warning" && "text-amber-700 dark:text-amber-300",
                          ev.tone === "danger" && "text-destructive-strong",
                          ev.tone === "muted" && "text-muted-foreground",
                        )}
                      >
                        {ev.label}
                      </span>
                      {ev.detail && (
                        <span className="ml-2 text-muted-foreground/80 break-all">
                          {ev.detail}
                        </span>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </div>

      {/* 底部按钮 */}
      <div className="flex items-center justify-end gap-2">
        {stage.type === "running" ? (
          <button
            type="button"
            onClick={onCancel}
            disabled={cancelling}
            className="inline-flex items-center gap-1.5 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-1.5 text-xs font-medium text-destructive-strong transition-colors hover:bg-destructive/20 disabled:cursor-wait disabled:opacity-60"
          >
            {cancelling ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Square className="h-3.5 w-3.5" />
            )}
            {cancelling ? "中止中…" : "中止任务"}
          </button>
        ) : (
          <button
            type="button"
            onClick={onRestart}
            className="inline-flex items-center gap-1.5 rounded-md bg-primary-600 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:bg-primary-700"
          >
            <RefreshCw className="h-3.5 w-3.5" />
            返回
          </button>
        )}
      </div>
    </div>
  );
}

function StatBlock({
  label,
  value,
  tone,
  hint,
}: {
  label: string;
  value: string;
  tone: "neutral" | "success" | "warning";
  hint: string;
}) {
  const toneClass =
    tone === "success"
      ? "text-emerald-600 dark:text-emerald-400"
      : tone === "warning"
      ? "text-amber-600 dark:text-amber-400"
      : "text-foreground";
  return (
    <div className="rounded-md border border-border/60 bg-muted/20 px-3 py-2.5">
      <div className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
        {label}
      </div>
      <div className={cn("mt-0.5 font-serif text-2xl font-semibold", toneClass)}>
        {value}
      </div>
      <div className="text-[10px] text-muted-foreground/80">{hint}</div>
    </div>
  );
}

// =====================================================================
// Wiki Agent · 配置抽屉（方案 3 · 右侧滑出）
// =====================================================================
//
// v1：
// - backend dropdown：claude-code / codex / gemini-cli / hermes
// - 自动消化开关 + 间隔（1h / 6h / 24h）→ v1 只持久化，调度 v2 接 tokio scheduler
// - vault 根：只读（v1 默认 C:\HomeBase\Wiki，未来可改）

const BACKEND_OPTIONS = [
  { value: "claude-code", label: "Claude Code", hint: "Anthropic · 推荐 · 强 reasoning" },
  { value: "codex", label: "Codex CLI", hint: "OpenAI · 速度优先" },
  { value: "gemini-cli", label: "Gemini CLI", hint: "Google · 长上下文" },
  { value: "hermes", label: "Hermes", hint: "本地 · 隐私优先（WSL 必需）" },
];

const INTERVAL_OPTIONS = [
  { value: 1, label: "每 1 小时", hint: "高频 · 写得多时用" },
  { value: 6, label: "每 6 小时", hint: "推荐 · 一天 4 次" },
  { value: 24, label: "每 24 小时", hint: "低频 · 早上集中消化" },
];

function WikiAgentSettingsDrawer({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const active=useWorkflowVisibility();
  const qc = useQueryClient();
  const [draft, setDraft] = useState<WikiAgentConfig | null>(null);
  const [saved, setSaved] = useState(false);

  const configQuery = useQuery({
    queryKey: ["wiki-agent", "config"],
    queryFn: () => wikiAgentApi.getConfig(),
    enabled: open && active, // 仅当前页面打开抽屉时读取
    refetchOnWindowFocus: false,
  });

  // 同步 draft
  useEffect(() => {
    if (configQuery.data && draft === null) {
      setDraft(configQuery.data);
    }
  }, [configQuery.data, draft]);

  // 关掉时重置 draft，下次打开重新拉
  useEffect(() => {
    if (!open) {
      setDraft(null);
      setSaved(false);
    }
  }, [open]);

  // ESC 关闭
  useEffect(() => {
    if (!open || !active) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open, active, onClose]);

  const saveMutation = useMutation({
    mutationFn: (cfg: WikiAgentConfig) => wikiAgentApi.setConfig(cfg),
    onSuccess: (cfg) => {
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
      qc.setQueryData(["wiki-agent", "config"], cfg);
      qc.invalidateQueries({ queryKey: ["wiki-agent", "status"] });
    },
  });

  const isDirty =
    draft !== null &&
    configQuery.data !== undefined &&
    JSON.stringify(draft) !== JSON.stringify(configQuery.data);

  return (
    <>
      {/* 遮罩 */}
      <div
        className={cn(
          "fixed inset-0 z-40 bg-black/30 backdrop-blur-[2px] transition-opacity duration-200",
          open ? "opacity-100" : "pointer-events-none opacity-0",
        )}
        onClick={onClose}
        aria-hidden
      />
      {/* 抽屉 */}
      <aside
        className={cn(
          "fixed inset-y-0 right-0 z-50 flex w-full max-w-md flex-col border-l border-border bg-card shadow-2xl transition-transform duration-200 ease-out",
          open ? "translate-x-0" : "translate-x-full",
        )}
        role="dialog"
        aria-label="Wiki Agent 配置"
      >
        {/* 抽屉头 */}
        <header className="flex items-center justify-between border-b border-border/60 px-5 py-4">
          <div>
            <h3 className="flex items-center gap-2 font-serif text-base font-semibold">
              <SettingsIcon className="h-4 w-4 text-primary-600 dark:text-primary-400" />
              Wiki Agent · 配置
            </h3>
            <p className="mt-0.5 text-[11px] text-muted-foreground">
              <code className="font-mono">~/.homebase/wiki_agent.json</code>
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded p-1.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
            title="关闭 (ESC)"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        {/* 内容 */}
        <div className="flex-1 overflow-y-auto">
          {configQuery.isLoading || draft === null ? (
            <div className="flex items-center gap-2 p-6 text-xs text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              加载配置…
            </div>
          ) : (
            <div className="space-y-6 p-5">
              {/* Backend AI */}
              <Field
                label="Backend AI"
                hint="实际跑消化任务的 CLI · 需要本地装好对应工具"
              >
                <div className="space-y-1.5">
                  {BACKEND_OPTIONS.map((opt) => (
                    <RadioCard
                      key={opt.value}
                      checked={draft.backend === opt.value}
                      onChange={() => setDraft({ ...draft, backend: opt.value })}
                      label={opt.label}
                      hint={opt.hint}
                      monoLabel={opt.value}
                    />
                  ))}
                </div>
              </Field>

              {/* 自动消化 */}
              <Field
                label="自动消化"
                hint="v1 · 仅持久化配置 · 调度由 v2 tokio scheduler 实际触发"
              >
                <label className="flex cursor-pointer items-center gap-3 rounded-md border border-border bg-muted/20 px-3 py-2.5">
                  <input
                    type="checkbox"
                    checked={draft.auto_digest}
                    onChange={(e) =>
                      setDraft({ ...draft, auto_digest: e.target.checked })
                    }
                    className="h-4 w-4 rounded border-border text-primary-600 focus:ring-primary-500"
                  />
                  <div className="flex-1 text-sm">
                    <div className="font-medium">启用自动消化</div>
                    <div className="text-[11px] text-muted-foreground">
                      关掉就只能手动点【生成 ingest prompt】
                    </div>
                  </div>
                </label>

                {draft.auto_digest && (
                  <div className="mt-3 space-y-1.5">
                    {INTERVAL_OPTIONS.map((opt) => (
                      <RadioCard
                        key={opt.value}
                        checked={draft.interval_hours === opt.value}
                        onChange={() =>
                          setDraft({ ...draft, interval_hours: opt.value })
                        }
                        label={opt.label}
                        hint={opt.hint}
                      />
                    ))}
                  </div>
                )}
              </Field>

              {/* Vault 根 */}
              <Field label="Vault 根目录" hint="v1 只读 · v2 支持 picker">
                <code className="block rounded-md border border-border/60 bg-muted/30 px-3 py-2 font-mono text-xs text-muted-foreground">
                  {draft.vault_root}
                </code>
              </Field>

              {/* v2 路线 */}
              <div className="rounded-md border border-dashed border-border/60 bg-muted/10 p-3 text-[11px] text-muted-foreground">
                <p className="mb-1 font-medium text-foreground/80">下一轮 v2</p>
                <ul className="ml-4 list-disc space-y-0.5">
                  <li>tokio scheduler 真正按 interval 跑</li>
                  <li>spawn 子进程直接执行 backend，不再走"复制粘贴"</li>
                  <li>Vault 多 vault 支持 + picker</li>
                </ul>
              </div>
            </div>
          )}
        </div>

        {/* 底部保存栏 */}
        <footer className="flex items-center justify-between gap-3 border-t border-border/60 bg-muted/10 px-5 py-3">
          <span className="text-[11px] text-muted-foreground">
            {saved ? (
              <span className="flex items-center gap-1 text-emerald-600 dark:text-emerald-400">
                <CheckCircle2 className="h-3 w-3" />
                已保存
              </span>
            ) : isDirty ? (
              <span className="text-amber-600 dark:text-amber-400">
                · 有未保存修改
              </span>
            ) : (
              <span className="opacity-60">无修改</span>
            )}
          </span>
          <div className="flex gap-2">
            <button
              type="button"
              onClick={onClose}
              className="rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:border-primary-400 hover:text-foreground"
            >
              取消
            </button>
            <button
              type="button"
              onClick={() => draft && saveMutation.mutate(draft)}
              disabled={!isDirty || saveMutation.isPending}
              className={cn(
                "inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
                isDirty && !saveMutation.isPending
                  ? "bg-primary-600 text-white shadow-sm hover:bg-primary-700"
                  : "cursor-not-allowed bg-muted text-muted-foreground",
              )}
            >
              {saveMutation.isPending ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
              ) : (
                <CheckCircle2 className="h-3.5 w-3.5" />
              )}
              保存
            </button>
          </div>
        </footer>
      </aside>
    </>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <div className="mb-2">
        <label className="block text-xs font-medium text-foreground">
          {label}
        </label>
        {hint && (
          <p className="mt-0.5 text-[10px] text-muted-foreground">{hint}</p>
        )}
      </div>
      {children}
    </div>
  );
}

function RadioCard({
  checked,
  onChange,
  label,
  hint,
  monoLabel,
}: {
  checked: boolean;
  onChange: () => void;
  label: string;
  hint?: string;
  monoLabel?: string;
}) {
  return (
    <button
      type="button"
      onClick={onChange}
      className={cn(
        "flex w-full items-start gap-2.5 rounded-md border px-3 py-2 text-left transition-colors",
        checked
          ? "border-primary-500 bg-primary-50/40 dark:bg-primary-950/40"
          : "border-border bg-card hover:border-primary-400 hover:bg-muted/40",
      )}
    >
      <div
        className={cn(
          "mt-0.5 h-3.5 w-3.5 shrink-0 rounded-full border-2",
          checked
            ? "border-primary-600 bg-primary-600 ring-2 ring-primary-200 dark:ring-primary-900"
            : "border-border",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium">{label}</span>
          {monoLabel && (
            <code className="font-mono text-[10px] text-muted-foreground/70">
              {monoLabel}
            </code>
          )}
        </div>
        {hint && (
          <div className="mt-0.5 text-[11px] text-muted-foreground">{hint}</div>
        )}
      </div>
    </button>
  );
}
