import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { Copy, Terminal, X } from "lucide-react";
import { aiControlApi } from "@/lib/tauri-api";

export function AiControlLauncher() {
  const [open, setOpen] = useState(false);
  const [copying, setCopying] = useState(false);
  const [feedback, setFeedback] = useState("");
  const [copyError, setCopyError] = useState("");
  const dialog = useRef<HTMLDialogElement>(null);
  const preview = useRef<HTMLTextAreaElement>(null);
  const query = useQuery({
    queryKey: ["ai-control", "bootstrap"],
    queryFn: aiControlApi.bootstrap,
    enabled: open,
    staleTime: 10_000,
    retry: false,
    refetchInterval: open ? 15_000 : false,
  });
  const data = query.data;
  const prompt = !query.isError && data?.files.ready ? data.files.prompt : null;
  useEffect(() => {
    if (open) {
      dialog.current?.showModal();
      setFeedback("");
      setCopyError("");
    } else dialog.current?.close();
  }, [open]);
  useEffect(() => {
    setFeedback("");
    setCopyError("");
  }, [prompt]);

  async function copy() {
    if (!prompt || copying) return;
    setCopying(true);
    setFeedback("");
    setCopyError("");
    try {
      await invoke("plugin:clipboard-manager|write_text", { text: prompt });
      setFeedback("启动提示词已复制，可直接粘贴给 AI。");
    } catch {
      setCopyError("复制失败。可选中下方全文，按 Ctrl+C 手动复制。");
    } finally {
      setCopying(false);
    }
  }

  return (
    <>
      <button
        type="button"
        className="mb-3 flex w-full items-center justify-center gap-2 rounded-md border border-border px-2 py-2 text-sm transition-colors hover:bg-secondary md:justify-start"
        title="AI 接管 · 复制启动提示词"
        aria-label="AI 接管"
        onClick={() => setOpen(true)}
      >
        <Terminal className="h-4 w-4 shrink-0" />
        <span className="hidden md:inline">AI 接管</span>
      </button>
      <dialog
        ref={dialog}
        aria-labelledby="ai-control-title"
        aria-describedby="ai-control-description"
        onCancel={() => setOpen(false)}
        onClose={() => setOpen(false)}
        onClick={(event) => {
          if (event.target === event.currentTarget) setOpen(false);
        }}
        className="max-h-[88vh] w-[calc(100%-2rem)] max-w-2xl overflow-auto rounded-xl border border-border bg-card p-0 text-foreground shadow-2xl backdrop:bg-black/50"
      >
        <div className="space-y-5 p-5 sm:p-6">
          <header className="flex items-start justify-between gap-4">
            <div>
              <h2
                id="ai-control-title"
                className="font-serif text-xl font-semibold"
              >
                让 AI 接管 HomeBase
              </h2>
              <p
                id="ai-control-description"
                className="mt-2 text-sm text-muted-foreground"
              >
                复制启动提示词，发给具备本机文件读取和命令执行能力的
                AI，再告诉它具体任务。
              </p>
            </div>
            <button
              type="button"
              className="rounded p-1.5 hover:bg-secondary"
              aria-label="关闭 AI 接管"
              onClick={() => setOpen(false)}
            >
              <X className="h-4 w-4" />
            </button>
          </header>
          {query.isLoading && (
            <p role="status" className="text-sm text-muted-foreground">
              正在读取启动信息…
            </p>
          )}
          {query.isError && (
            <div
              role="alert"
              className="rounded-md border border-destructive/40 p-3 text-sm"
            >
              无法读取 AI 接管信息，请重试或重新启动更新后的 HomeBase。
            </div>
          )}
          {data && !query.isError && (
            <>
              <div
                className="rounded-md border border-border bg-muted/30 p-3 text-sm"
                role="status"
              >
                <p className="font-medium">{data.service.message}</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  {data.available} 项可调用接口 · {data.blocked}{" "}
                  项人工或禁用入口 · {data.unavailable} 项尚未实现
                </p>
              </div>
              {!data.files.ready && (
                <p role="alert" className="text-sm text-destructive-strong">
                  {data.files.issue || "AI 操作文件不可用，请检查项目位置。"}
                </p>
              )}
              {prompt && (
                <>
                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      autoFocus
                      className="hb-btn-primary inline-flex items-center gap-2"
                      disabled={copying || query.isFetching}
                      onClick={() => void copy()}
                    >
                      <Copy className="h-4 w-4" />
                      {copying ? "正在复制…" : "复制启动提示词"}
                    </button>
                    <button
                      type="button"
                      className="hb-btn-outline text-sm"
                      onClick={() => {
                        preview.current?.focus();
                        preview.current?.select();
                      }}
                    >
                      选中全文
                    </button>
                  </div>
                  <label className="block text-xs text-muted-foreground">
                    启动提示词预览
                    <textarea
                      ref={preview}
                      readOnly
                      value={prompt}
                      rows={9}
                      spellCheck={false}
                  className="mt-2 block w-full resize-y rounded-md border border-border bg-background p-3 font-mono text-xs leading-relaxed text-foreground outline-none focus:ring-2 focus:ring-primary-500/30"
                    />
                  </label>
                  <p className="break-all text-xs text-muted-foreground">
                    操作入口：{data.files.startup_file}
                  </p>
                </>
              )}
            </>
          )}
          {feedback && (
            <p
              role="status"
              className="text-sm text-primary-700 dark:text-primary-300"
            >
              {feedback}
            </p>
          )}
          {copyError && (
            <p role="alert" className="text-sm text-destructive-strong">
              {copyError}
            </p>
          )}
          <footer className="flex flex-wrap items-center gap-3 border-t border-border pt-4">
            <button
              type="button"
              className="hb-btn-outline text-xs"
              disabled={query.isFetching}
              onClick={() => void query.refetch()}
            >
              重新检查
            </button>
            <p className="min-w-0 flex-1 text-xs text-muted-foreground">
              CLI
              提供命令发现、结构化调用和任务查询。实际任务仍需核对依赖、业务状态及输出结果。
            </p>
          </footer>
        </div>
      </dialog>
    </>
  );
}
