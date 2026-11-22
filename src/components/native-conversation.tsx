import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { skipToken, useQuery, useQueryClient } from "@tanstack/react-query";
import { invalidateConversationProvider } from "@/lib/conversation-index";
import { Link } from "react-router-dom";
import { RefreshCw, ArrowUp, Square } from "lucide-react";
import { AssistantVoice } from "./assistant-voice-session";
import {
  useDraftState,
  ensureDraft,
  updateDraft,
  readDraft,
  flushDraft,
  retryDraftSave,
  sendNativeDraft,
  resolveNativeDraft,
  pendingNativeSend,
  savedDraft,
  adoptSavedDraft,
  type DesktopEntry,
} from "@/lib/assistant-drafts";
import {
  conversationsApi,
  nativeItems,
  type NativeReply,
  type NativeSnapshot,
} from "@/lib/conversations";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { nativeMessageView } from "@/lib/native-message-view";
import {
  appendNativeHistory,
  nativeHistoryWindow,
  prependNativeHistory,
  type NativeHistoryWindow,
} from "@/lib/native-history";
import {
  MAX_DRAFT_BYTES,
  MAX_MESSAGE_BYTES,
  inputBytes,
  inputBudgetMessage,
} from "@/lib/chat-input-limits";

function isActiveNativeTurn(state: NativeSnapshot["state"] | undefined) {
  return state === "accepted" || state === "inProgress";
}

export function NativeConversation({
  id,
  provider = "codex",
}: {
  id: string;
  provider?: string;
}) {
  const label =
    (
      {
        codex: "Codex",
        "claude-code": "Claude Code",
        "gemini-cli": "Gemini CLI",
      } as Record<string, string>
    )[provider] ?? provider;
  const cache = useQueryClient();
  const draftScope = `native:${provider}:${id}`;
  const workspace = useDraftState(draftScope);
  const text = workspace.draft.text;
  const textBytes = inputBytes(text);
  const budgetMessage = inputBudgetMessage(textBytes, false);
  const setText = (value: string | ((old: string) => string)) =>
    updateDraft(draftScope, {
      text:
        typeof value === "function" ? value(readDraft(draftScope).text) : value,
    });
  useEffect(() => {
    void ensureDraft(draftScope);
  }, [draftScope]);
  const [busy, setBusy] = useState(false);
  const [voiceBusy, setVoiceBusy] = useState(false);
  const composing = useRef(false);
  const [watch, setWatch] = useState(() =>
    isActiveNativeTurn(
      cache.getQueryData<NativeSnapshot>([
        "conversations",
        provider,
        id,
        "live",
      ])?.state,
    ),
  );
  const [error, setError] = useState("");
  const [comparison, setComparison] = useState<DesktopEntry | null>(null);
  const [delegated, setDelegated] = useState<string | undefined>(
    () =>
      cache.getQueryData<NativeReply>(["conversations", provider, id])
        ?.assistant_session_id ?? undefined,
  );
  const checkedRequest = useRef("");
  const historyKey = useMemo(() => ["conversations", provider, id, "reading-window"], [provider, id]);
  const history = useQuery<NativeHistoryWindow[]>({ queryKey: historyKey, queryFn: skipToken });
  const transcript = useRef<HTMLDivElement>(null);
  const followLatest = useRef(true);
  const readingAnchor = useRef<{ id?: string; offset?: number; height: number; top: number } | null>(null);
  const rememberReadingAnchor = useCallback(() => {
    const node = transcript.current;
    if (!node) return;
    const top = node.getBoundingClientRect().top;
    const first = Array.from(node.querySelectorAll<HTMLElement>("[data-message-id]"))
      .find((row) => row.getBoundingClientRect().bottom > top);
    readingAnchor.current = {
      id: first?.dataset.messageId,
      offset: first?.getBoundingClientRect().top,
      height: node.scrollHeight,
      top: node.scrollTop,
    };
  }, []);
  const thread = useQuery({
    queryKey: ["conversations", provider, id],
    queryFn: () => conversationsApi.read(provider, id),
    staleTime: Infinity,
  });
  useLayoutEffect(() => {
    const previous = cache.getQueryData<NativeHistoryWindow[]>(historyKey);
    if (!thread.data || !previous) return;
    const latest = nativeHistoryWindow(thread.data);
    if (!followLatest.current) rememberReadingAnchor();
    const merged = cache.setQueryData(historyKey, appendNativeHistory(previous, latest));
    if (merged === previous) readingAnchor.current = null;
  }, [cache, historyKey, rememberReadingAnchor, thread.data]);
  useEffect(() => {
    if (thread.data?.assistant_session_id)
      setDelegated(thread.data.assistant_session_id);
    else if (thread.data?.context_link_state) setDelegated(undefined);
  }, [thread.data?.assistant_session_id, thread.data?.context_link_state]);
  const live = useQuery({
    queryKey: ["conversations", provider, id, "live"],
    queryFn: () => conversationsApi.status(id, provider),
    enabled: true,
    staleTime: 0,
    refetchInterval: (q) =>
      q.state.status !== "error" && isActiveNativeTurn(q.state.data?.state)
        ? 1500
        : false,
  });
  const windows = history.data;
  const hasGap = (windows?.length ?? 0) > 1;
  const cursor = windows ? windows.at(-1)?.nextCursor : thread.data?.next_cursor;
  const savedThread = thread.data?.thread;
  const items = useMemo(() => nativeItems(
    windows && savedThread ? { ...savedThread, turns: windows.flatMap((window) => window.turns) } : savedThread,
    live.data,
  ), [windows, savedThread, live.data]);
  const visualKey = useMemo(
    () =>
      JSON.stringify(
        items
          .filter((item) => ["userMessage", "agentMessage"].includes(item.type))
          .map((item) => {
            const view = nativeMessageView(item);
            return [
              item.id,
              item.type,
              view.body,
              view.additional,
              view.nonText,
              view.invalid,
            ];
          }),
      ),
    [items],
  );
  const [away, setAway] = useState(false);
  const [newMessages, setNewMessages] = useState(false);
  const previousVisual = useRef<string | undefined>(undefined);
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      const node = transcript.current;
      if (!node) return;
      const changed = previousVisual.current !== visualKey;
      previousVisual.current = visualKey;
      const anchor = readingAnchor.current;
      if (anchor) {
        const row = anchor.id && node.querySelector<HTMLElement>(`[data-message-id="${CSS.escape(anchor.id)}"]`);
        if (row && anchor.offset !== undefined)
          node.scrollTop += row.getBoundingClientRect().top - anchor.offset;
        else node.scrollTop = anchor.top + node.scrollHeight - anchor.height;
        readingAnchor.current = null;
      } else if (changed && followLatest.current) node.scrollTop = node.scrollHeight;
      else if (changed && items.length) setNewMessages(true);
    });
    return () => cancelAnimationFrame(frame);
  }, [visualKey, windows, hasGap, cursor]);
  useEffect(() => {
    const node = transcript.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (followLatest.current) node.scrollTop = node.scrollHeight;
    });
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  const directoryNeedsRefresh = useRef(false);
  useEffect(() => {
    if (isActiveNativeTurn(live.data?.state)) {
      directoryNeedsRefresh.current = true;
    } else if (directoryNeedsRefresh.current &&
      ["completed", "failed", "interrupted", "unknown"].includes(live.data?.state ?? "")) {
      directoryNeedsRefresh.current = false;
      // Directory metadata may still be stale even when the transcript is fresh.
      void invalidateConversationProvider(cache, provider);
    }
  }, [cache, provider, live.data]);
  const refreshedTerminal = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (
      provider === "codex" ||
      thread.isPending ||
      live.isError ||
      !live.data ||
      !["completed", "failed", "interrupted", "unknown"].includes(
        live.data.state,
      )
    )
      return;
    const signature = JSON.stringify([
      live.data.state,
      live.data.item_order,
      live.data.error,
    ]);
    if (refreshedTerminal.current === signature) return;
    const textOf = (i: { text?: string; content?: { text?: string }[] }) =>
      i.text ?? i.content?.map((c) => c.text ?? "").join("\n") ?? "";
    const saved = new Map(
      nativeItems(thread.data?.thread).map((i) => [i.id, textOf(i)]),
    );
    if (
      !Object.values(live.data.items ?? {}).some(
        (i) => saved.get(i.id) !== textOf(i),
      )
    ) {
      refreshedTerminal.current = signature;
      return;
    }
    // Wait for the current read, then compare its actual data. Reuse it when it
    // contains this terminal turn; otherwise one fresh read is still necessary.
    if (
      cache.getQueryState(["conversations", provider, id])?.fetchStatus ===
      "fetching"
    )
      return;
    refreshedTerminal.current = signature;
    // The live projection contains only four turns. Fetch the bounded history once
    // after completion so long conversations do not silently lose older replies.
    void cache.invalidateQueries(
      {
        queryKey: ["conversations", provider, id],
        exact: true,
      },
      { cancelRefetch: false },
    );
  }, [
    cache,
    provider,
    id,
    thread.data,
    thread.isPending,
    thread.isFetching,
    live.data,
    live.isError,
  ]);
  useEffect(() => {
    if (isActiveNativeTurn(live.data?.state)) setWatch(true);
  }, [live.data?.state]);
  const refreshAccepted = useCallback(
    (ack: NativeReply) => {
      setDelegated(ack.assistant_session_id ?? undefined);
      setWatch(true);
      directoryNeedsRefresh.current = true;
      void cache.invalidateQueries(
        { queryKey: ["conversations", provider, id] },
        { cancelRefetch: false },
      );
      void invalidateConversationProvider(cache, provider);
    },
    [cache, provider, id],
  );
  useEffect(() => {
    if (!workspace.loaded || !workspace.nativePending || workspace.unconfirmed)
      return;
    const token = `${draftScope}:${workspace.nativePending.request_id}`;
    if (checkedRequest.current === token) return;
    checkedRequest.current = token;
    let alive = true;
    void resolveNativeDraft(draftScope)
      .then((ack) => {
        if (alive && ack?.state === "accepted") refreshAccepted(ack);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [
    draftScope,
    workspace.loaded,
    workspace.nativePending?.request_id,
    workspace.unconfirmed,
    refreshAccepted,
  ]);
  async function queryOriginal() {
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      const ack = await resolveNativeDraft(draftScope);
      if (ack?.state === "accepted") refreshAccepted(ack);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  async function earlier() {
    if (!cursor || !thread.data || busy) return;
    setBusy(true);
    setError("");
    try {
      const current = nativeHistoryWindow(thread.data);
      cache.setQueryData<NativeHistoryWindow[]>(historyKey, (previous) => previous ?? [current]);
      const page = await conversationsApi.read(provider, id, cursor);
      const earlier = nativeHistoryWindow(page);
      const loaded = cache.getQueryData<NativeHistoryWindow[]>(historyKey) ?? [current];
      const next = prependNativeHistory(loaded, cursor, earlier);
      if (next !== loaded) {
        rememberReadingAnchor();
        cache.setQueryData(historyKey, next);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  const activeTurn = isActiveNativeTurn(live.data?.state);
  const canSend =
    workspace.loaded &&
    !busy &&
    !voiceBusy &&
    !thread.isPending &&
    !thread.isError &&
    !live.isPending &&
    !live.isError &&
    !activeTurn;
  async function send(original = false) {
    const frozen = pendingNativeSend(draftScope);
    if ((!text.trim() && !original) || !canSend) return;
    if (!original && textBytes > MAX_MESSAGE_BYTES) {
      setError(budgetMessage);
      return;
    }
    if (!original && frozen && frozen.text !== text) {
      setError(
        "原请求尚待核对，新输入已保留。请先查询原请求；重试原请求会使用保存的原内容。",
      );
      return;
    }
    if (original && !frozen) return;
    const request = frozen ?? {
      provider,
      thread_id: id,
      text,
      request_id: crypto.randomUUID(),
    };
    setBusy(true);
    setError("");
    try {
      if (!frozen) await flushDraft(draftScope);
      const ack = await sendNativeDraft(request);
      refreshAccepted(ack);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section
      className="assistant-workspace flex min-h-[420px] min-w-0 flex-1 flex-col overflow-hidden rounded-xl border border-border lg:min-h-0"
      aria-label={`${label} 原生对话`}
    >
      <header className="flex flex-wrap items-center justify-between gap-2 border-b border-border p-4">
        <div>
          <h2 className="font-serif text-lg">{label}</h2>
          <p className="text-xs text-muted-foreground">
            原生会话 · 保留原有工具与账号
          </p>
        </div>
        <button
          className="hb-btn-ghost"
          aria-label="刷新原生会话"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            setError("");
            void (async () => {
              await ensureDraft(draftScope, true);
              const ack = pendingNativeSend(draftScope)
                ? await resolveNativeDraft(draftScope)
                : null;
              if (ack?.state === "accepted") refreshAccepted(ack);
              else
                await Promise.all([
                  thread.refetch({ cancelRefetch: false }),
                  live.refetch({ cancelRefetch: false }),
                  invalidateConversationProvider(cache, provider),
                ]);
            })()
              .catch((e) => setError(String(e)))
              .finally(() => setBusy(false));
          }}
        >
          <RefreshCw size={16} />
        </button>
      </header>
      <details className="px-4 py-2 text-xs leading-5 text-muted-foreground">
        <summary className="cursor-pointer">账号、费用与本机权限</summary>
        <p className="mt-2">
          {provider === "gemini-cli"
            ? "独立 Gemini 运行目录，使用进程环境或你选定的 Key 引用；全局旧版配置保留。"
            : `使用 ${label} 原生账号或所选凭据引用；费用不受 HomeBase 免费模型限制。`}
          本机工作交给已挂载的 HomeBase 工具，共用原执行器与确认。
        </p>
        <p className="mt-2 break-all">
          工作目录：
          {thread.data?.thread?.cwd ??
            (thread.isPending ? "正在读取" : "原生客户端未记录")}
        </p>
      </details>
      {thread.data?.context_link_message && (
        <p
          className="mx-4 mb-3 text-xs text-muted-foreground"
          role={
            thread.data.context_link_state === "unavailable"
              ? "alert"
              : "status"
          }
        >
          {thread.data.context_link_message}
        </p>
      )}
      {(error || workspace.error || thread.error) && (
        <p
          role="alert"
          className="m-4 rounded-lg border border-destructive/30 p-3 text-sm"
        >
          {error || workspace.error || String(thread.error)}
        </p>
      )}
      {live.isError && (
        <div
          role="alert"
          className="m-4 flex flex-wrap items-center gap-2 rounded-lg border border-destructive/30 p-3 text-sm"
        >
          <p>原生状态读取失败，草稿仍可编辑。</p>
          <button
            type="button"
            className="underline"
            disabled={busy || live.isFetching}
            onClick={() => void live.refetch({ cancelRefetch: false })}
          >
            重查原生状态
          </button>
        </div>
      )}
      {workspace.nativePending && (
        <div
          className="mx-4 mb-3 flex flex-wrap items-center gap-3 rounded-lg border border-border p-3 text-xs"
          aria-label="原生发送回执"
        >
          <span>
            原消息受理待核对 · {workspace.nativePending.request_id.slice(0, 12)}{" "}
            · 打开页面不会重发
          </span>
          <button
            type="button"
            className="underline"
            disabled={busy}
            onClick={() => void queryOriginal()}
          >
            查询原请求
          </button>
          <button
            type="button"
            className="underline"
            disabled={!canSend}
            title="仅使用保存的原编号和原文；已有未知回执时不会再次提交"
            onClick={() => void send(true)}
          >
            重试原请求
          </button>
          <details className="w-full">
            <summary className="cursor-pointer">查看原消息</summary>
            <p className="mt-2 max-h-36 overflow-auto whitespace-pre-wrap break-words">
              {workspace.nativePending.text}
            </p>
          </details>
        </div>
      )}
      {comparison && (
        <section
          aria-label="已保存原生草稿版本"
          className="mx-4 mb-3 space-y-3 rounded-lg border border-border p-3 text-sm"
        >
          <p>已保存版本 {comparison.revision} · 当前输入仍保留</p>
          <textarea
            aria-label="已保存原生正文"
            readOnly
            value={comparison.body.text}
            className="max-h-40 w-full rounded border border-border bg-background p-2"
          />
          <button
            type="button"
            className="mr-4 underline"
            onClick={() => {
              try {
                adoptSavedDraft(draftScope, comparison);
                setComparison(null);
                setError("");
              } catch (e) {
                setError(String(e));
              }
            }}
          >
            使用已保存版本
          </button>
          <button
            type="button"
            className="underline"
            onClick={() => setComparison(null)}
          >
            关闭对比
          </button>
        </section>
      )}
      <div
        ref={transcript}
        className="chat-transcript min-h-0 flex-1 overflow-auto"
        aria-live="polite"
        aria-label="原生对话消息"
        onScroll={() => {
          const node = transcript.current;
          if (!node) return;
          const atEnd =
            node.scrollHeight - node.clientHeight - node.scrollTop < 100;
          followLatest.current = atEnd;
          setAway(!atEnd);
          if (atEnd) setNewMessages(false);
        }}
      >
        {thread.isPending && <p role="status">正在读取原生会话…</p>}
        {hasGap && (
          <p className="text-xs text-muted-foreground" role="status">
            已读历史和较新的消息之间还有未载入的轮次。
          </p>
        )}
        {cursor && (
          <button
            className="hb-btn-ghost"
            disabled={busy}
            onClick={() => void earlier()}
          >
            {hasGap ? "补读中间的原生对话" : "读取更早的原生对话"}
          </button>
        )}
        {!thread.isPending && !items.length && (
          <p className="py-12 text-center text-muted-foreground">
            {live.data?.state === "completed"
              ? "原生轮次已结束，未返回可显示的消息。请核对原客户端记录。"
              : "从这里开始，或接着聊刚才的事情。"}
          </p>
        )}
        {items
          .filter((i) => ["userMessage", "agentMessage"].includes(i.type))
          .map((i) => {
            const view = nativeMessageView(i);
            return (
              <article
                key={i.id}
                data-message-id={i.id}
                className={
                  i.type === "userMessage"
                    ? "chat-message chat-message-user"
                    : "chat-message"
                }
              >
                <p className="mb-2 text-xs text-muted-foreground">
                  {i.type === "userMessage" ? "你" : label}
                </p>
                {view.body.trim() ? (
                  <MarkdownRenderer source={view.body} />
                ) : (
                  <p className="text-sm text-muted-foreground">
                    原生客户端未提供文字内容。
                  </p>
                )}
                {view.invalid && (
                  <p role="alert" className="mt-2 text-xs text-destructive">
                    原生消息格式未能确认，原始记录保留，请核对原客户端。
                  </p>
                )}
                {!!view.additional.length && (
                  <details
                    className="mt-3 rounded-lg border border-border p-3 text-sm"
                    onToggle={(event) => {
                      const node = transcript.current;
                      if (!node) return;
                      const pause = event.currentTarget.open;
                      followLatest.current =
                        !pause &&
                        node.scrollHeight - node.clientHeight - node.scrollTop <
                          100;
                      setAway(!followLatest.current);
                    }}
                  >
                    <summary className="cursor-pointer text-muted-foreground">
                      附带原生输入 · {view.additional.length} 段（完整保留）
                    </summary>
                    <p className="mt-2 text-xs text-muted-foreground">
                      这些内容可能来自用户或SDK附带资料，未作为额外指令或授权。
                    </p>
                    {view.additional.map((text, index) => (
                      <pre
                        key={index}
                        className="mt-3 max-h-60 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-3 text-xs"
                      >
                        {text}
                      </pre>
                    ))}
                  </details>
                )}
                {!!view.nonText.length && (
                  <p className="mt-3 text-xs text-muted-foreground">
                    包含 {view.nonText.length}{" "}
                    个非文本内容，当前不预览；请在原客户端查看。
                  </p>
                )}
              </article>
            );
          })}
        {watch && (
          <p role="status" className="text-sm text-muted-foreground">
            {live.isError ? "原生任务状态待核对" : (
              {
                idle: "还没有发送消息",
                not_loaded: "尚未加载原生轮次，发送后才会续接",
                accepted: "已受理，正在准备对话…",
                inProgress: "正在处理你的事情…",
                completed: "这轮原生对话已结束",
                failed: "这次没能完成，请核对错误后继续",
                interrupted: "已停止这轮对话",
                unknown: "连接中断，结果未知；请核对原会话",
              } as Record<string, string>
            )[live.data?.state ?? ""] ?? "正在核对原生任务进展…"}
          </p>
        )}
        {live.data?.error && (
          <p role="alert">
            {live.data.error.message ?? "原生任务返回错误，未按成功处理"}
          </p>
        )}
        {live.data?.truncated && (
          <p className="text-xs text-muted-foreground">
            这里只保留最近的实时事件；原会话仍然保存，可刷新读取完整的最近轮次。
          </p>
        )}
        {delegated && (
          <Link
            className="text-sm underline"
            to={`/assistant?session=${encodeURIComponent(delegated)}`}
          >
            查看共享执行任务、结果与待确认操作
          </Link>
        )}
      </div>
      {away && (
        <button
          type="button"
          className="hb-btn-ghost mx-auto my-2 text-xs"
          aria-label="查看原生最新消息"
          onClick={() => {
            followLatest.current = true;
            if (transcript.current)
              transcript.current.scrollTop = transcript.current.scrollHeight;
            setAway(false);
            setNewMessages(false);
          }}
        >
          {newMessages ? "查看新消息" : "回到最新"} ↓
        </button>
      )}
      <div className="chat-composer-dock">
        <p
          className="chat-composer-width mb-2 text-xs text-muted-foreground"
          role="status"
          aria-label="原生草稿保存状态"
        >
          {!workspace.loaded
            ? "正在恢复本机输入…"
            : workspace.status === "saving"
              ? "正在保存草稿…"
              : workspace.status === "error"
                ? "草稿保存需要核对"
                : workspace.revision === 0
                  ? "输入就绪"
                  : "草稿已受保护保存"}
          {workspace.status === "error" && (
            <>
              <button
                type="button"
                className="ml-2 underline"
                onClick={() =>
                  void retryDraftSave(draftScope).catch((e) => setError(String(e)))
                }
              >
                重试保存
              </button>
              <button
                type="button"
                className="ml-2 underline"
                onClick={() => {
                  void savedDraft(draftScope)
                    .then(setComparison)
                    .catch((e) => setError(String(e)));
                }}
              >
                查看已保存版本
              </button>
            </>
          )}
          {workspace.warning && (
            <span className="ml-2">{workspace.warning}</span>
          )}
        </p>
        <form
          className="chat-composer chat-composer-width"
          onSubmit={(e) => {
            e.preventDefault();
            void send();
          }}
        >
          <label className="sr-only" htmlFor="native-message">
            给 {label} 发消息
          </label>
          <textarea
            id="native-message"
            className="w-full resize-none rounded-xl border border-border bg-background p-3 text-sm"
            value={text}
            onChange={(e) => setText(e.target.value)}
            maxLength={MAX_DRAFT_BYTES}
            aria-describedby={budgetMessage ? "native-input-budget" : undefined}
            placeholder="继续这件事…"
            disabled={
              busy ||
              voiceBusy ||
              !workspace.loaded ||
              thread.isPending ||
              thread.isError
            }
            onCompositionStart={() => {
              composing.current = true;
            }}
            onCompositionEnd={() => {
              composing.current = false;
            }}
            onBlur={() => {
              composing.current = false;
            }}
            onKeyDown={(e) => {
              if (
                e.key === "Enter" &&
                !e.shiftKey &&
                !e.nativeEvent.isComposing &&
                !composing.current &&
                e.keyCode !== 229
              ) {
                e.preventDefault();
                if (!voiceBusy) void send();
              }
            }}
          />
          {budgetMessage && (
            <p
              id="native-input-budget"
              role="status"
              className="px-2 text-xs text-amber-700 dark:text-amber-400"
            >
              {budgetMessage}
            </p>
          )}
          <div className="chat-composer-tools">
            <span className="px-2 text-xs text-muted-foreground">
              {label} · 原生会话
            </span>
            <div className="ml-auto flex items-center gap-2">
              <AssistantVoice
                scope={draftScope}
                disabled={
                  busy ||
                  !workspace.loaded ||
                  thread.isPending ||
                  thread.isError ||
                  live.isPending ||
                  activeTurn
                }
                onBusy={setVoiceBusy}
              />
              {activeTurn && !live.isError && (
                <button
                  type="button"
                  className="hb-btn-ghost"
                  disabled={busy}
                  onClick={() =>
                    void conversationsApi
                      .native("interrupt", {
                        thread_id: id,
                        provider,
                        request_id: crypto.randomUUID(),
                      })
                      .catch((e) => setError(String(e)))
                  }
                >
                  <Square size={14} />
                  停止
                </button>
              )}
              <button
                className="chat-send"
                aria-label="发送"
                disabled={!canSend || !text.trim()}
              >
                <ArrowUp size={18} />
              </button>
            </div>
          </div>
        </form>
        <p className="chat-composer-caption">
          原生账号与模型费用按所选服务计算 · Enter 发送，Shift+Enter 换行
        </p>
      </div>
    </section>
  );
}
