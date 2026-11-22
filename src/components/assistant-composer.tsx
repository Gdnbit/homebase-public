import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowUp,
  Check,
  ChevronDown,
  FileText,
  Image as ImageIcon,
  LoaderCircle,
  Plus,
  Search,
  X,
} from "lucide-react";
import {
  modelCapabilitiesApi,
  modelCapabilitiesKey,
  modelSupportsVision,
  readLocalPngBase64,
} from "@/lib/model-capabilities";
import {
  useDraftState,
  updateDraft,
  appendDraftFiles,
  readDraft,
  type ImageReference,
  type TextAttachment,
} from "@/lib/assistant-drafts";
import type { AssistantSettings } from "@/lib/assistant";
import { AssistantVoice } from "./assistant-voice-session";
import { AssistantDocuments } from "./assistant-documents";
import { readTextAttachments } from "@/lib/text-attachments";
import {
  legacyCapabilityConversion,
  replaceCapabilityTask,
} from "@/lib/capability-context";
import { readAssistantTaskContext } from "@/lib/assistant-task-context";
import { AssistantTaskContextDetails } from "./assistant-task-context";
import {
  MAX_DRAFT_BYTES,
  MAX_MESSAGE_BYTES,
  inputBytes,
  inputBudgetMessage,
} from "@/lib/chat-input-limits";

export interface ComposerOptions {
  route_id?: string;
  attachments?: TextAttachment[];
  image?: ImageReference;
}
const MAX_IMAGE_PROMPT_BYTES = 4000;
export function AssistantComposer({
  session,
  text,
  setText,
  mode,
  setMode,
  settings,
  busy,
  readOnly,
  sendLabel = "发送",
  onSend,
  onError,
  onIntent,
}: {
  session: string | null;
  text: string;
  setText: (s: string, expectedText?: string) => void;
  mode: "auto" | "chat" | "task";
  setMode: (s: "auto" | "chat" | "task") => void;
  settings?: AssistantSettings;
  busy: boolean;
  readOnly: boolean;
  sendLabel?: string;
  onSend: (options: ComposerOptions) => void;
  onError: (message: string) => void;
  onIntent?: () => void;
}) {
  const workspace = useDraftState(session);
  const [fullText, setFullText] = useState(false);
  const context = useMemo(() => readAssistantTaskContext(text), [text]);
  const legacy = useMemo(
    () => (context ? null : legacyCapabilityConversion(text)),
    [text, context],
  );
  const compact = !fullText && context;
  const visibleText = compact ? compact.task : text;
  const hasTask = !!(context ? context.task : text).trim();
  const textBytes = inputBytes(text);
  const budgetMessage = inputBudgetMessage(textBytes);
  const { files, route, image } = workspace.draft;
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const [reading, setReading] = useState(false);
  const [voiceBusy, setVoiceBusy] = useState(false);
  const [documentsOpen, setDocumentsOpen] = useState(false);
  const [pdf, setPdf] = useState<File | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLDivElement>(null);
  const modelButton = useRef<HTMLButtonElement>(null);
  const fileRead = useRef<AbortController | null>(null);
  const composing = useRef(false);
  const searchComposing = useRef(false);
  const catalog = useQuery({
    queryKey: modelCapabilitiesKey,
    queryFn: modelCapabilitiesApi.catalog,
    enabled: open || !!image,
    staleTime: 60_000,
    retry: false,
  });
  const draftDisabled = busy || readOnly;
  const disabled = draftDisabled || !settings?.enabled;
  const draftWritable = useRef(false);
  draftWritable.current = !draftDisabled && !voiceBusy;
  const effectiveRoute = route || settings?.route_id || "";
  const chosen = catalog.data?.models.find(
    (m) => m.route_id === effectiveRoute,
  );
  const modelLabel =
    chosen?.name || effectiveRoute.split("::").at(-1) || "选择模型";
  let imageError = "";
  if (image) {
    if (catalog.isError) imageError = "图片能力暂时无法核对，请打开模型列表重试。";
    else if (!route || !modelSupportsVision(chosen))
      imageError = catalog.isFetching ? "正在核对模型的图片能力…" : "请为这张图片选择支持视觉的模型。";
    else if (settings?.free_only && !route.endsWith(":free"))
      imageError = "当前仅允许免费渠道，请选择支持视觉的免费模型。";
    else if (chosen && ["blocked", "not_configured", "expired"].includes(chosen.account_state))
      imageError = "所选模型的凭据当前不可用，请检查配置或更换视觉模型。";
    else if (textBytes > MAX_IMAGE_PROMPT_BYTES)
      imageError = "图片问题最多 4,000 个 UTF-8 字节。";
  }
  useEffect(() => {
    if (textarea.current) {
      textarea.current.style.height = "auto";
      textarea.current.style.height = `${Math.min(textarea.current.scrollHeight, 220)}px`;
    }
  }, [visibleText]);
  useLayoutEffect(() => () => {
    const read = fileRead.current;
    fileRead.current = null;
    read?.abort();
  }, [session]);
  useEffect(() => {
    if (!open) {
      searchComposing.current = false;
      return;
    }
    const click = (e: PointerEvent) => {
      if (
        !picker.current?.contains(e.target as Node) &&
        !modelButton.current?.contains(e.target as Node)
      )
        setOpen(false);
    };
    document.addEventListener("pointerdown", click);
    return () => document.removeEventListener("pointerdown", click);
  }, [open]);
  function putFiles(next: TextAttachment[]) {
    updateDraft(session, { files: next });
  }
  function cancelFileRead() {
    const read = fileRead.current;
    if (!read) return;
    fileRead.current = null;
    read.abort();
    setReading(false);
    textarea.current?.focus();
  }
  async function addFiles(input: FileList | File[]) {
    if (!draftWritable.current || fileRead.current) return;
    const incoming = Array.from(input);
    if (!incoming.length) return;
    const png = incoming.find((file) => /\.png$/i.test(file.name) || file.type === "image/png");
    if ((png && (incoming.length !== 1 || files.length)) || (!png && image)) {
      onError("每条图片消息只能添加一张 PNG。请先移除其他附件。");
      return;
    }
    if (incoming.some((f) => /\.pdf$/i.test(f.name))) {
      if (incoming.length !== 1) {
        onError("请单独选择一份 PDF，预览文字后再加入消息。");
        return;
      }
      setPdf(incoming[0]);
      setDocumentsOpen(true);
      return;
    }
    if (files.length + incoming.length > 4) {
      onError("一次最多添加 4 个文本附件。");
      return;
    }
    const read = new AbortController();
    fileRead.current = read;
    setReading(true);
    onError("");
    try {
      const check = () => {
        read.signal.throwIfAborted();
        if (!draftWritable.current)
          throw Error("草稿当前不可编辑，附件未加入，请稍后重新选择。");
      };
      if (png) {
        onIntent?.();
        const name = /\.png$/i.test(png.name) ? png.name : "剪贴板图片.png";
        const pngBase64 = await readLocalPngBase64(png);
        check();
        const scope = session ?? "new";
        const staged = await invoke<ImageReference>("assistant_desktop_image_stage", { scope, name, pngBase64 });
        check();
        if (staged.scope !== scope || readDraft(session).files.length) {
          throw Error("当前草稿与图片归属不一致，图片未加入。");
        }
        updateDraft(session, { image: staged, mode: "chat", route: readDraft(session).route || settings?.route_id || "" });
      } else {
        const added = await readTextAttachments(incoming, read.signal, check);
        check();
        appendDraftFiles(session, added);
      }
    } catch (e) {
      if (fileRead.current === read)
        onError(
          e instanceof TypeError && !png
            ? "文件不是 UTF-8 文本，请转换编码后重新添加。"
            : String(e),
        );
    } finally {
      if (fileRead.current === read) {
        fileRead.current = null;
        setReading(false);
      }
    }
  }
  const closePicker = () => {
    setOpen(false);
    modelButton.current?.focus();
  };
  function submit() {
    if (disabled || fileRead.current || voiceBusy || (!hasTask && !files.length)) return;
    if (image && (imageError || !hasTask || mode !== "chat")) {
      onError(imageError || "请填写要问图片的问题，并使用只聊聊模式。");
      return;
    }
    if (textBytes > MAX_MESSAGE_BYTES) {
      onError(budgetMessage);
      return;
    }
    onSend({
      ...(route ? { route_id: route } : {}),
      ...(files.length ? { attachments: files } : {}),
      ...(image ? { image } : {}),
    });
  }
  function putText(value: string) {
    try {
      setText(
        compact ? replaceCapabilityTask(compact, value) : value,
        compact ? text : undefined,
      );
    } catch (error) {
      onError(String(error));
    }
  }
  function convertLegacy() {
    if (
      draftDisabled ||
      voiceBusy ||
      !legacy ||
      workspace.pending ||
      workspace.nativePending ||
      workspace.unconfirmed
    )
      return;
    if (inputBytes(legacy) > MAX_DRAFT_BYTES) {
      onError("整理后的正文超过64 KiB，原稿未修改；请先缩短需求。");
      return;
    }
    try {
      setText(legacy, text);
      setFullText(false);
    } catch (error) {
      onError(String(error));
    }
  }
  return (
    <div
      className="chat-composer"
      role="form"
      aria-label="消息输入区"
      onFocusCapture={onIntent}
      onDragOver={(e) => {
        if (e.dataTransfer.types.includes("Files")) e.preventDefault();
      }}
      onDrop={(e) => {
        if (e.dataTransfer.files.length) {
          onIntent?.();
          e.preventDefault();
          void addFiles(e.dataTransfer.files);
        }
      }}
    >
      {context && (
        <div
          className="space-y-2 px-3 pt-3"
          aria-label={
            context.kind === "capability"
              ? "随需求保留的能力资料"
              : "随需求保留的工作流资料"
          }
        >
          <AssistantTaskContextDetails view={context} />
          <button
            type="button"
            className="text-xs text-muted-foreground underline"
            onClick={() => setFullText((v) => !v)}
          >
            {fullText ? "只编辑需求" : "全文编辑"}
          </button>
        </div>
      )}
      {legacy && (
        <div className="px-3 pt-3">
          <button
            type="button"
            className="text-xs text-muted-foreground underline"
            disabled={
              draftDisabled ||
              voiceBusy ||
              !!workspace.pending ||
              !!workspace.nativePending ||
              workspace.unconfirmed
            }
            onClick={convertLegacy}
          >
            整理为任务输入（保留资料）
          </button>
        </div>
      )}
      {(!!files.length || image) && (
        <ul className="chat-attachments" aria-label="待发送附件">
          {image && (
            <li>
              <ImageIcon size={18} />
              <span className="min-w-0">
                <span className="block truncate text-sm">{image.name}</span>
                <span className="text-[11px] text-muted-foreground">已加密 · {image.bytes.toLocaleString()} 字节</span>
              </span>
              <button
                type="button"
                className="chat-icon"
                aria-label={`移除 ${image.name}`}
                disabled={draftDisabled || reading}
                onClick={() => updateDraft(session, { image: undefined })}
              >
                <X size={14} />
              </button>
            </li>
          )}
          {files.map((f, i) => (
            <li key={`${f.name}-${i}`}>
              <FileText size={18} />
              <span className="min-w-0">
                <span className="block truncate text-sm">{f.name}</span>
                <span className="text-[11px] text-muted-foreground">
                  已准备 ·{" "}
                  {new TextEncoder().encode(f.content).length.toLocaleString()}{" "}
                  字节
                </span>
              </span>
              <button
                type="button"
                className="chat-icon"
                aria-label={`移除 ${f.name}`}
                disabled={draftDisabled}
                onClick={() =>
                  putFiles(files.filter((_, index) => index !== i))
                }
              >
                <X size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {reading && (
        <div
          role="status"
          className="flex items-center gap-2 px-2 text-xs text-muted-foreground"
        >
          <LoaderCircle size={14} className="animate-spin" />
          正在读取附件…
          <button
            type="button"
            onClick={cancelFileRead}
          >
            取消读取
          </button>
        </div>
      )}
      <label className="sr-only" htmlFor="assistant-message">
        发送消息
      </label>
      <textarea
        ref={textarea}
        id="assistant-message"
        rows={2}
        value={visibleText}
        disabled={draftDisabled || voiceBusy}
        maxLength={MAX_DRAFT_BYTES}
        aria-describedby={budgetMessage ? "assistant-input-budget" : undefined}
        placeholder={
          readOnly
            ? "请在微信继续此对话"
            : compact
              ? context?.kind === "capability"
                ? "想让它完成什么？能力资料会一并发送"
                : "继续补充需求，原工作流资料会一并发送"
              : !settings?.enabled
                ? "先写下想法，草稿会保存；启用模型后再发送"
                : "继续聊，或者交给我一件事…"
        }
        onChange={(e) => putText(e.target.value)}
        onCompositionStart={() => {
          composing.current = true;
        }}
        onCompositionEnd={() => {
          composing.current = false;
        }}
        onBlur={() => {
          composing.current = false;
        }}
        onPaste={(e) => {
          if (e.clipboardData.files.length) {
            e.preventDefault();
            void addFiles(e.clipboardData.files);
          }
        }}
        onKeyDown={(e) => {
          if (
            e.key === "Enter" &&
            !disabled &&
            !reading &&
            !voiceBusy &&
            !e.shiftKey &&
            !e.nativeEvent.isComposing &&
            !composing.current &&
            e.keyCode !== 229
          ) {
            e.preventDefault();
            submit();
          }
        }}
      />
      {budgetMessage && (
        <p
          id="assistant-input-budget"
          role="status"
          className="px-2 text-xs text-amber-700 dark:text-amber-400"
        >
          {budgetMessage}
        </p>
      )}
      <div className="chat-composer-tools">
        <input
          ref={fileInput}
          type="file"
          className="sr-only"
          tabIndex={-1}
          multiple
          accept=".txt,.md,.markdown,.csv,.json,.log,.yaml,.yml,.tsv,.pdf,.png"
          aria-label="选择文本附件"
          disabled={draftDisabled || reading || voiceBusy}
          onChange={(e) => {
            if (e.target.files) void addFiles(e.target.files);
            e.target.value = "";
          }}
        />
        <button
          type="button"
          className="chat-icon border border-border"
          aria-label="添加附件"
          title="添加文本、PDF 或 PNG；PDF 先在本地预览文字"
          disabled={draftDisabled || reading || voiceBusy}
          onClick={() => fileInput.current?.click()}
        >
          <Plus size={20} />
        </button>
        <button
          type="button"
          className="chat-icon border border-border"
          aria-label="管理 PDF 文档"
          title="PDF 本地文字预览"
          disabled={busy || readOnly || voiceBusy || !!image}
          onClick={() => setDocumentsOpen(true)}
        >
          <FileText size={18} />
        </button>
        <AssistantDocuments
          scope={session ?? "new"}
          open={documentsOpen}
          incoming={pdf}
          onConsumed={() => setPdf(null)}
          onClose={() => setDocumentsOpen(false)}
          onAdd={(file) => {
            if (!file.document)
              throw Error("PDF 摘录缺少受保护来源引用，未加入消息。");
            if (!draftWritable.current)
              throw Error("草稿当前不可编辑，PDF 摘录未加入。");
            appendDraftFiles(session, [file]);
            onError("");
          }}
        />
        <div className="relative min-w-0">
          <button
            ref={modelButton}
            type="button"
            className="chat-model-trigger"
            aria-label="选择本次对话模型"
            aria-expanded={open}
            aria-haspopup="dialog"
            disabled={busy || readOnly || voiceBusy}
            onClick={() => setOpen((v) => !v)}
          >
            <span className="truncate">{modelLabel}</span>
            <ChevronDown size={13} className="shrink-0" />
          </button>
          {open && (
            <div
              ref={picker}
              role="dialog"
              aria-label="本次对话模型"
              className="chat-model-picker"
              onKeyDown={(e) => {
                if (
                  searchComposing.current ||
                  e.nativeEvent.isComposing ||
                  e.keyCode === 229
                ) {
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  closePicker();
                } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  const options = Array.from(
                    e.currentTarget.querySelectorAll<HTMLButtonElement>(
                      '[role="option"]:not(:disabled)',
                    ),
                  );
                  if (!options.length) return;
                  e.preventDefault();
                  const at = options.indexOf(
                    document.activeElement as HTMLButtonElement,
                  );
                  const direction = e.key === "ArrowDown" ? 1 : -1;
                  if (at === -1) {
                    options[direction === 1 ? 0 : options.length - 1].focus();
                    return;
                  }
                  const next =
                    (at + direction + options.length) % options.length;
                  options[next].focus();
                }
              }}
            >
              <div className="flex items-center gap-2 border-b border-border p-3">
                <Search size={16} className="text-muted-foreground" />
                <input
                  autoFocus
                  value={search}
                  onChange={(e) => setSearch(e.target.value)}
                  onCompositionStart={() => {
                    searchComposing.current = true;
                  }}
                  onCompositionEnd={() => {
                    searchComposing.current = false;
                  }}
                  onBlur={() => {
                    searchComposing.current = false;
                  }}
                  placeholder="搜索模型或厂商"
                  aria-label="搜索模型"
                  className="min-w-0 flex-1 bg-transparent text-sm outline-none"
                />
                <button
                  type="button"
                  className="chat-icon"
                  aria-label="关闭模型选择"
                  onClick={closePicker}
                >
                  <X size={15} />
                </button>
              </div>
              <div
                role="listbox"
                aria-label="可选模型"
                className="max-h-64 overflow-auto p-1.5"
              >
                <button
                  type="button"
                  role="option"
                  aria-selected={!route}
                  onClick={() => {
                    updateDraft(session, { route: "" });
                    closePicker();
                  }}
                  className="chat-model-option"
                >
                  <span>
                    <span className="block">沿用默认模型</span>
                    <small>{settings?.route_id || "尚未配置"}</small>
                  </span>
                  {!route && <Check size={16} />}
                </button>
                {catalog.isPending && (
                  <p role="status" className="p-3 text-sm">
                    正在读取模型目录…
                  </p>
                )}
                {catalog.isError && (
                  <p role="alert" className="p-3 text-sm text-destructive">
                    模型目录读取失败。
                    <button
                      type="button"
                      onClick={() => void catalog.refetch()}
                    >
                      重试
                    </button>
                  </p>
                )}
                {catalog.data?.models
                  .filter(
                    (m) =>
                      ["chat", "responses"].includes(m.protocol) &&
                      (!image || modelSupportsVision(m)) &&
                      `${m.name} ${m.provider_id}`
                        .toLowerCase()
                        .includes(search.toLowerCase()),
                  )
                  .map((m) => {
                    const free =
                      m.route_id.startsWith("openrouter::") &&
                      (m.route_id.endsWith(":free") ||
                        m.route_id === "openrouter::openrouter/free");
                    const blocked =
                      (settings?.free_only && !free) ||
                      ["blocked", "not_configured", "expired"].includes(
                        m.account_state,
                      );
                    return (
                      <button
                        key={m.route_id}
                        type="button"
                        role="option"
                        aria-selected={route === m.route_id}
                        disabled={!!blocked}
                        className="chat-model-option"
                        onClick={() => {
                          updateDraft(session, { route: m.route_id });
                          closePicker();
                        }}
                      >
                        <span className="min-w-0">
                          <span className="block truncate">{m.name}</span>
                          <small>
                            {m.provider_id} ·{" "}
                            {free ? "免费路由" : "费用以厂商为准"}
                            {blocked ? " · 当前不可选" : ""}
                          </small>
                        </span>
                        {route === m.route_id && (
                          <Check size={16} className="shrink-0" />
                        )}
                      </button>
                    );
                  })}
                {catalog.data &&
                  !catalog.data.models.some(
                    (m) =>
                      ["chat", "responses"].includes(m.protocol) &&
                      `${m.name} ${m.provider_id}`
                        .toLowerCase()
                        .includes(search.toLowerCase()),
                  ) && (
                    <p className="p-3 text-sm text-muted-foreground">
                      没有匹配的对话模型。
                    </p>
                  )}
              </div>
              <p className="border-t border-border p-3 text-xs leading-5 text-muted-foreground">
                仅用于本次对话，不改变微信默认模型。
                {settings?.free_only
                  ? "当前仅允许免费路由。"
                  : "请求可能产生厂商 API 费用。"}
              </p>
            </div>
          )}
        </div>
        <select
          aria-label="消息方式"
          id="assistant-message-mode"
          className="chat-mode"
          value={mode}
          disabled={busy || readOnly || voiceBusy || !!image}
          onChange={(e) => setMode(e.target.value as typeof mode)}
          title="自动判断可补充当前任务；只聊聊不执行；另一个任务会排队"
        >
          <option value="auto">自动</option>
          <option value="chat">只聊聊，不执行</option>
          <option value="task">新任务（排队）</option>
        </select>
        <div className="ml-auto flex items-center gap-2">
          <AssistantVoice
            scope={session}
            disabled={draftDisabled || reading}
            onBusy={setVoiceBusy}
            onApplied={() => {
              textarea.current?.focus();
            }}
          />
          <button
            type="button"
            className="chat-send"
            aria-label={sendLabel}
            onClick={submit}
            title={
              !settings?.enabled
                ? "模型尚未启用，草稿可先保存"
                : `${sendLabel}（Enter），换行（Shift+Enter）`
            }
            disabled={
              disabled || reading || voiceBusy || (!hasTask && !files.length) || !!imageError
            }
          >
            {busy ? (
              <LoaderCircle size={19} className="animate-spin" />
            ) : (
              <ArrowUp size={21} />
            )}
          </button>
        </div>
      </div>
      {(mode !== "auto" || image) && (
        <p className="mt-2 px-2 text-xs text-muted-foreground" role="status">
          {image ? imageError || "发送后，所选模型只会收到本条问题和图片。" : mode === "task"
            ? "新事情会排队；修改当前要求请用自动判断。"
            : "可以边做边聊；讨论不会改变原任务。"}
        </p>
      )}
    </div>
  );
}
