import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { FileText, LoaderCircle, RefreshCw, X } from "lucide-react";
import {
  documentActive,
  documentKey,
  documentPhase,
  documentsApi,
  importDocument,
  type Extraction,
  type PageRange,
} from "@/lib/documents";
import type { TextAttachment } from "@/lib/assistant-drafts";
const field =
  "min-w-0 rounded-lg border border-border bg-background px-2 py-1.5 text-sm";
const button =
  "rounded-lg border border-border px-3 py-2 text-sm disabled:opacity-40";

export function AssistantDocuments({
  scope,
  open,
  incoming = null,
  onConsumed,
  onClose,
  onAdd,
  readOnly = false,
}: {
  scope: string;
  open: boolean;
  incoming?: File | null;
  onConsumed?: () => void;
  onClose: () => void;
  onAdd?: (file: TextAttachment) => void;
  readOnly?: boolean;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const picker = useRef<HTMLInputElement>(null);
  const generation = useRef(0);
  const importing = useRef(false);
  const [selected, setSelected] = useState("");
  const [working, setWorking] = useState(false);
  const [progress, setProgress] = useState("");
  const [error, setError] = useState("");
  const client = useQueryClient();
  const index = useQuery({
    queryKey: documentKey(scope),
    queryFn: () => documentsApi.index(scope),
    enabled: open,
    staleTime: readOnly ? 0 : Infinity,
    retry: false,
  });
  useEffect(() => {
    if (open) dialog.current?.showModal();
    else dialog.current?.close();
    return () => {
      generation.current++;
    };
  }, [open]);
  async function add(file: File) {
    if (importing.current) {
      setError("上一份 PDF 仍在保存，请稍后重新选择。");
      return;
    }
    importing.current = true;
    const token = ++generation.current;
    let refreshIndex = false;
    const check = () => {
      if (token !== generation.current)
        throw Error("已离开本次导入，保存的原分块保留。");
    };
    setWorking(true);
    setError("");
    setProgress("正在核对本地文件…");
    try {
      const meta = await importDocument(file, scope, {
        check,
        progress: (saved, total) => setProgress(`已保存 ${saved}/${total} 块`),
        beforeWrite: () => {
          refreshIndex = true;
        },
      });
      check();
      setSelected(meta.id);
      refreshIndex = true;
    } catch (e) {
      if (token === generation.current) setError(String(e));
    } finally {
      if (refreshIndex) {
        // The write can land after close or remount. Discard an index read that
        // started earlier, even if that read has not populated the cache yet.
        const indexFilter = {
          queryKey: documentKey(scope),
          exact: true,
        };
        await client.cancelQueries(indexFilter);
        await client.invalidateQueries(indexFilter);
      }
      importing.current = false;
      if (dialog.current) {
        setWorking(false);
        setProgress("");
      }
    }
  }
  // Consume each explicit file-picker/drop intent once. Opening alone never imports.
  const latestAdd = useRef(add);
  latestAdd.current = add;
  useEffect(() => {
    if (open && incoming && !readOnly) {
      onConsumed?.();
      void latestAdd.current(incoming);
    }
  }, [open, incoming, onConsumed, readOnly]);
  const close = () => {
    ++generation.current;
    if (importing.current) setProgress("正在核对上一份 PDF 的保存结果…");
    onClose();
  };
  return (
    <dialog
      ref={dialog}
      onCancel={(e) => {
        e.preventDefault();
        close();
      }}
      className="w-[min(920px,94vw)] max-h-[90vh] rounded-2xl border border-border bg-card p-0 text-foreground shadow-2xl backdrop:bg-black/35"
      aria-labelledby={titleId}
    >
      <header className="flex items-center justify-between border-b border-border px-5 py-4">
        <h2
          id={titleId}
          className="flex items-center gap-2 text-lg font-semibold"
        >
          <FileText size={20} className="text-primary" />
          PDF 文字预览
        </h2>
        <button
          type="button"
          className="chat-icon"
          aria-label="关闭 PDF 预览"
          onClick={close}
        >
          <X size={18} />
        </button>
      </header>
      <div className="grid max-h-[72vh] min-h-80 grid-cols-1 overflow-auto sm:grid-cols-[200px_minmax(0,1fr)]">
        <aside className="space-y-3 border-b border-border bg-muted/20 p-4 sm:border-b-0 sm:border-r">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium">本地文档</h3>
            <button
              type="button"
              className="chat-icon"
              aria-label="刷新文档列表"
              disabled={working || index.isFetching}
              onClick={() => void index.refetch({ cancelRefetch: false })}
            >
              <RefreshCw size={14} />
            </button>
          </div>
          {!readOnly && <><input
            ref={picker}
            type="file"
            accept=".pdf"
            aria-label="选择 PDF 文档"
            className="sr-only"
            tabIndex={-1}
            disabled={working}
            onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (file) void add(file);
            }}
          />
          <button
            type="button"
            className={`${button} w-full`}
            disabled={working}
            onClick={() => picker.current?.click()}
          >
            选择 PDF
          </button></>}
          {index.isPending && (
            <p role="status" className="text-xs text-muted-foreground">
              正在读取文档列表…
            </p>
          )}
          {index.isError && (
            <p role="alert" className="text-xs text-destructive">
              列表读取失败：{String(index.error)}
            </p>
          )}
          {index.data && !index.data.complete && (
            <p role="alert" className="text-xs text-destructive">
              有 {index.data.unreadable} 条记录不可读，未当作空列表。
            </p>
          )}
          <ul aria-label="当前输入的 PDF 文档" className="space-y-1">
            {index.data?.documents.map((doc) => (
              <li key={doc.id}>
                <button
                  type="button"
                  aria-pressed={selected === doc.id}
                  className={`w-full rounded-lg px-2 py-2 text-left text-sm ${selected === doc.id ? "bg-primary/10" : "hover:bg-muted"}`}
                  disabled={working}
                  onClick={() => {
                    setSelected(doc.id);
                    setError("");
                  }}
                >
                  <span className="block truncate" title={doc.name}>
                    {doc.name}
                  </span>
                  <span className="text-xs text-muted-foreground">
                    {doc.source_bytes.toLocaleString()} 字节
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {index.data?.uploads.map((upload) => (
            <p
              key={upload.document_id}
              className="break-words text-xs text-muted-foreground"
            >
              {upload.name} · 已保存 {upload.received.length}/{upload.parts}{" "}
              块，重新选择同一文件继续。
            </p>
          ))}
          {index.data?.complete &&
            !index.data.documents.length &&
            !index.data.uploads.length && (
              <p className="text-xs text-muted-foreground">
                {readOnly ? "这个微信会话还没有保存的 PDF。" : "选择文件后先在本机预览；不会自动发送。"}
              </p>
            )}
          {working && (
            <p role="status" className="flex items-center gap-2 text-xs">
              <LoaderCircle size={14} className="shrink-0 animate-spin" />
              {progress}
            </p>
          )}
        </aside>
        <section className="min-w-0 p-4">
          {error && (
            <p
              role="alert"
              className="mb-3 break-words text-sm text-destructive"
            >
              {error}
            </p>
          )}
          {selected && open ? (
            <DocumentPreview
              key={selected}
              scope={scope}
              id={selected}
              disabled={working}
              readOnly={readOnly}
              onAdd={(file) => {
                onAdd?.(file);
                close();
              }}
            />
          ) : (
            <div className="flex min-h-64 flex-col items-center justify-center gap-3 text-center text-sm text-muted-foreground">
              <FileText size={30} />
              <p>{readOnly ? "选择文档查看已提取的文字" : "选择一份 PDF，提取并预览文字"}</p>
              <p className="text-xs">
                至多 4 MiB · 仅本地提取文字，不含图片或 OCR
              </p>
            </div>
          )}
        </section>
      </div>
      <footer className="border-t border-border px-5 py-3 text-xs text-muted-foreground">
        {readOnly
          ? "微信来稿保存在原会话。这里查看已有解析记录。"
          : "所选文字随消息发送，助理可按需读取本文件其他页。关闭窗口保留已保存来源和原解析，不会自动加入消息。"}
      </footer>
    </dialog>
  );
}

type ExtractionAction =
  | { kind: "extract"; pages: PageRange }
  | { kind: "retry"; pages: PageRange | null; originalId: string }
  | { kind: "cancel"; originalId: string };

function DocumentPreview({
  scope,
  id,
  disabled,
  readOnly,
  onAdd,
}: {
  scope: string;
  id: string;
  disabled: boolean;
  readOnly: boolean;
  onAdd: (file: TextAttachment) => void;
}) {
  const client = useQueryClient();
  const [key, setKey] = useState("");
  const [from, setFrom] = useState("1"),
    [to, setTo] = useState("1");
  const [textFrom, setTextFrom] = useState(""),
    [textTo, setTextTo] = useState("");
  const [localWorking, setWorking] = useState(false),
    [error, setError] = useState("");
  const alive = useRef(true);
  const generation = useRef(0);
  useLayoutEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      ++generation.current;
    };
  }, []);
  const detailsKey = [...documentKey(scope), id];
  const mutationKey = [...detailsKey, "extraction-action"];
  const mutationFilter = { mutationKey, exact: true };
  const writing = useIsMutating(mutationFilter) > 0;
  const working = localWorking || writing;
  const { mutateAsync: writeExtraction } = useMutation({
    mutationKey,
    retry: false,
    mutationFn: async (request: ExtractionAction) => {
      const reply = request.kind === "cancel"
        ? await documentsApi.cancel(scope, id, request.originalId)
        : await documentsApi.extract(scope, id, request.pages, request.kind === "retry");
      if (reply.extraction && (
        reply.extraction.document_id !== id ||
        (request.kind !== "extract" && reply.extraction.id !== request.originalId)
      )) throw Error("提取回执与原文档或原编号不一致，请核对记录。");
      return reply;
    },
    onSettled: async (reply, _error, request) => {
      // Mutation callbacks run even if the preview has closed. Refresh only the
      // original document and discard reads started before its write settled.
      const detailFilter = { queryKey: detailsKey, exact: true };
      const receipt = reply?.extraction;
      const receiptId = receipt?.id ?? (request.kind === "extract" ? null : request.originalId);
      if (receiptId) {
        const queryKey = [...detailsKey, "status", receiptId];
        const statusFilter = { queryKey, exact: true };
        await client.cancelQueries(statusFilter);
        if (receipt) {
          client.setQueryData<Extraction>(queryKey, (previous) =>
            previous && previous.revision > receipt.revision ? previous : receipt,
          );
        } else {
          await client.invalidateQueries(statusFilter);
        }
      }
      await client.cancelQueries(detailFilter);
      await client.invalidateQueries(detailFilter);
    },
  });
  const details = useQuery({
    queryKey: detailsKey,
    queryFn: () => documentsApi.get(scope, id),
    staleTime: readOnly ? 0 : Infinity,
    retry: false,
  });
  const defaultReceipt =
    details.data?.extractions.find((e) => documentActive(e.phase)) ||
    details.data?.extractions[0];
  const activeKey = key || defaultReceipt?.id || "";
  const statusKey = [...documentKey(scope), id, "status", activeKey];
  const status = useQuery({
    queryKey: statusKey,
    queryFn: () => documentsApi.status(scope, id, activeKey),
    enabled: !!activeKey && details.isSuccess && !working,
    retry: false,
    refetchInterval: (q) =>
      q.state.status !== "error" && documentActive(q.state.data?.phase)
        ? 1500
        : false,
  });
  const receipt = status.data;
  const result = useQuery({
    queryKey: [
      ...documentKey(scope),
      id,
      "result",
      activeKey,
      receipt?.attempt,
    ],
    queryFn: () => documentsApi.result(scope, id, activeKey),
    enabled: receipt?.phase === "ready" && !status.isError && details.isSuccess,
    staleTime: Infinity,
    retry: false,
  });
  const parsed = result.data?.result;
  const running = documentActive(receipt?.phase);
  const receiptUnconfirmed =
    !details.isSuccess || (!!activeKey && (!receipt || status.isError));
  const retryable =
    receipt && ["unknown", "failed", "cancelled"].includes(receipt.phase);
  const invalidRange =
    !/^\d+$/.test(from) ||
    !/^\d+$/.test(to) ||
    !Number.isSafeInteger(+from) ||
    !Number.isSafeInteger(+to) ||
    +from < 1 ||
    +to < +from ||
    +to - +from >= 40 ||
    !!(receipt?.total_pages && +to > receipt.total_pages);
  async function action(kind: "extract" | "retry" | "cancel" | "add") {
    if (
      working || client.isMutating(mutationFilter) || disabled || readOnly ||
      receiptUnconfirmed || (kind === "retry" && !retryable)
    ) return;
    const token = ++generation.current;
    setWorking(true);
    setError("");
    try {
      if (kind === "add") {
        if (invalidRange || !activeKey)
          throw Error("请选择已经提取的有效页范围。");
        const hasTextRange = textFrom !== "" || textTo !== "";
        if (
          hasTextRange &&
          (!/^\d+$/.test(textFrom) ||
            !/^\d+$/.test(textTo) ||
            !Number.isSafeInteger(+textFrom) ||
            !Number.isSafeInteger(+textTo))
        )
          throw Error("文字区间需要同时填写有效的起止字节位置。");
        const file = await documentsApi.excerpt(
          scope,
          id,
          activeKey,
          { from: +from, to: +to },
          hasTextRange ? +textFrom : null,
          hasTextRange ? +textTo : null,
        );
        if (alive.current && token === generation.current) onAdd(file);
      } else {
        if (kind === "extract" && invalidRange)
          throw Error("页范围须为最多 40 个实际连续页。");
        const reply = await writeExtraction(
          kind === "extract"
            ? { kind, pages: { from: +from, to: +to } }
            : kind === "retry"
              ? { kind, originalId: activeKey, pages: receipt?.pages ?? null }
              : { kind, originalId: activeKey },
        );
        if (!alive.current || token !== generation.current) return;
        if (reply.extraction) setKey(reply.extraction.id);
        if (!reply.extraction || reply.status === "not_running_here")
          setError(reply.message);
      }
    } catch (e) {
      if (alive.current && token === generation.current) setError(String(e));
    } finally {
      if (alive.current && token === generation.current) setWorking(false);
    }
  }
  const selectedText = parsed?.pages
    ?.filter((p) => readOnly || (p.number >= +from && p.number <= +to))
    .map((p) => `【第${p.number}页】\n${p.text}`)
    .join("\n\n");
  const selectionReady =
    receipt?.phase === "ready" &&
    !!selectedText &&
    !!parsed?.pages?.some(
      (p) => (readOnly || (p.number >= +from && p.number <= +to)) && !p.empty,
    ) &&
    (readOnly || (+from >= (parsed.page_from || 1) && +to <= (parsed.page_to || 0)));
  return (
    <div className="space-y-3">
      <h3 className="truncate font-medium" title={details.data?.document.name}>
        {details.data?.document.name || "PDF 文档"}
      </h3>
      {details.isError && (
        <div role="alert" className="text-sm text-destructive">
          文档读取失败：{String(details.error)}
          <button
            type="button"
            className={`${button} ml-2`}
            disabled={details.isFetching || working}
            onClick={() => void details.refetch({ cancelRefetch: false })}
          >
            重新读取文档
          </button>
        </div>
      )}
      {!readOnly && <div className="flex flex-wrap items-center gap-2 text-sm">
        <span>页范围</span>
        <label className="flex items-center gap-1">
          从
          <input
            aria-label="PDF 起始页"
            type="number"
            min={1}
            value={from}
            onChange={(e) => {
              setFrom(e.target.value);
              setTextFrom("");
              setTextTo("");
            }}
            className={`${field} w-20`}
          />
        </label>
        <label className="flex items-center gap-1">
          到
          <input
            aria-label="PDF 结束页"
            type="number"
            min={1}
            value={to}
            onChange={(e) => {
              setTo(e.target.value);
              setTextFrom("");
              setTextTo("");
            }}
            className={`${field} w-20`}
          />
        </label>
        <button
          type="button"
          className={button}
          disabled={working || disabled || receiptUnconfirmed || running || invalidRange}
          onClick={() => void action("extract")}
        >
          提取文字
        </button>
        {receipt?.total_pages != null && (
          <span className="text-xs text-muted-foreground">
            共 {receipt.total_pages} 页
          </span>
        )}
      </div>}
      <p className="text-xs text-muted-foreground">
        仅本地提取文字，不含图片或 OCR。每次至多 40 页。
      </p>
      {!!details.data?.extractions.length && (
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          原提取记录
          <select
            aria-label="PDF 提取记录"
            className={`${field} max-w-full`}
            value={activeKey}
            disabled={working}
            onChange={(e) => {
              setKey(e.target.value);
              setTextFrom("");
              setTextTo("");
            }}
          >
            {[
              ...(key &&
              !details.data.extractions.some((e) => e.id === key) &&
              receipt
                ? [receipt]
                : []),
              ...details.data.extractions,
            ].map((e) => (
              <option key={e.id} value={e.id}>
                {e.pages ? `${e.pages.from}–${e.pages.to} 页` : "整份文档"} ·{" "}
                {e.id.slice(0, 8)}
              </option>
            ))}
          </select>
        </label>
      )}
      {error && (
        <p role="alert" className="break-words text-sm text-destructive">
          {error}
        </p>
      )}
      {writing && (
        <p role="status" className="text-xs text-muted-foreground">
          正在核对原提取操作…
        </p>
      )}
      {(status.isError || result.isError) && (
        <div role="alert" className="text-sm text-destructive">
          {status.isError ? "原提取状态读取失败" : "原提取文字读取失败"}：
          {String(status.error || result.error)}
          <button
            type="button"
            className={`${button} ml-2`}
            disabled={working || status.isFetching || result.isFetching}
            onClick={() => {
              if (status.isError) void status.refetch({ cancelRefetch: false });
              else void result.refetch({ cancelRefetch: false });
            }}
          >
            重新查询
          </button>
        </div>
      )}
      <div
        className="max-h-64 min-h-48 overflow-auto whitespace-pre-wrap break-words rounded-xl border border-border bg-background p-4 text-sm"
        aria-label="PDF 提取文字"
      >
        {selectedText && selectionReady ? (
          selectedText
        ) : (
          <div role="status" className="space-y-2 text-muted-foreground">
            <p>
              {status.isError
                ? "原提取状态待核对"
                : receipt
                  ? documentPhase[receipt.phase]
                  : details.isPending || (!!activeKey && status.isPending)
                    ? "正在读取文档…"
                    : "尚未提取文字"}
            </p>
            <p className="text-xs">
              {status.isError
                ? "重新查询后再继续操作。"
                : receipt?.message || (readOnly ? "原文档暂无可读取的解析回执。" : "选择页范围后点击提取；打开窗口不会自动解析。")}
            </p>
            {receipt?.phase === "no_text" && (
              <p className="text-xs">可能是扫描件或空白页；当前不支持 OCR。</p>
            )}
            {receipt?.phase === "ready" && !selectionReady && !result.isError && (
              <p className="text-xs">
                {result.isPending
                  ? "正在读取原提取结果…"
                  : "当前页范围没有可加入的文字，请选择已提取页或提取新范围。"}
              </p>
            )}
          </div>
        )}
      </div>
      {!!parsed?.missing_text_pages?.length && (
        <p className="text-xs text-amber-700 dark:text-amber-400">
          这些页没有文字：{parsed.missing_text_pages.join("、")}
          ；未将它们当作已识别内容。
        </p>
      )}
      {!readOnly && selectionReady && (
        <details className="rounded-lg border border-border p-3 text-xs">
          <summary className="cursor-pointer">
            高级选项：仅加入所选页的部分文字
          </summary>
          <p className="my-2 text-muted-foreground">
            UTF-8 字节位置，包含页标题。留空使用完整页；中文与 emoji
            须保持完整字符。文字与来源说明共用单附件 16 KiB 限额。
          </p>
          <div className="flex flex-wrap gap-2">
            <label>
              起始字节
              <input
                aria-label="PDF 文字起始字节"
                type="number"
                min={0}
                value={textFrom}
                onChange={(e) => setTextFrom(e.target.value)}
                className={`${field} ml-2 w-28`}
              />
            </label>
            <label>
              结束字节
              <input
                aria-label="PDF 文字结束字节"
                type="number"
                min={1}
                value={textTo}
                onChange={(e) => setTextTo(e.target.value)}
                className={`${field} ml-2 w-28`}
              />
            </label>
          </div>
        </details>
      )}
      <div className="flex flex-wrap justify-end gap-2">
        {!readOnly && running && (
          <button
            type="button"
            className={button}
            disabled={
              working || disabled || receiptUnconfirmed || receipt?.phase === "cancel_requested"
            }
            onClick={() => void action("cancel")}
          >
            停止原解析
          </button>
        )}
        {!readOnly && receipt && retryable && (
          <button
            type="button"
            className={button}
            disabled={working || disabled || receiptUnconfirmed}
            onClick={() => void action("retry")}
          >
            显式重试本地提取（
            {receipt.pages ? `${receipt.pages.from}–${receipt.pages.to} 页` : "整份文档"}）
          </button>
        )}
        {receipt?.phase === "unknown" && !status.isError && (
          <button
            type="button"
            className={button}
            disabled={working || status.isFetching}
            onClick={() => void status.refetch({ cancelRefetch: false })}
          >
            核对原编号
          </button>
        )}
        {!readOnly && <button
          type="button"
          className="rounded-lg bg-primary px-4 py-2 text-sm text-primary-foreground disabled:opacity-40"
          disabled={
            working ||
            disabled ||
            receiptUnconfirmed ||
            !selectionReady ||
            invalidRange ||
            !!status.error ||
            !!result.error
          }
          onClick={() => void action("add")}
        >
          {working ? (
            <LoaderCircle size={16} className="animate-spin" />
          ) : (
            "加入消息"
          )}
        </button>}
      </div>
    </div>
  );
}
