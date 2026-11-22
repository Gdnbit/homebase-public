import { lazy, Suspense, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  queryOptions,
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import {
  assistantTurnKey,
  assistantTurnProgressKey,
  assistantTurnStepKey,
  observedTurn,
  selectSessionWindows,
  timelinePage,
  scopedTurn,
  scopedTurnInspection,
  scopedTurnStep,
  type TimelinePage,
} from "@/lib/assistant-window";
import {
  Link,
  useLocation,
  useSearchParams,
  useNavigate,
} from "react-router-dom";
import { AssistantContext } from "@/components/assistant-context";
import { AssistantArtifacts } from "@/components/assistant-artifacts";
import { invalidateSessionCatalog, useSessionCatalog } from "@/lib/assistant-sessions";
import { refreshAssistantReads } from "@/lib/assistant-refresh";
import {
  Plus,
  Settings,
  X,
  FileVideo,
  RefreshCw,
  Square,
  MoreHorizontal,
  ChevronDown,
  Copy,
  Check,
} from "lucide-react";
import {
  AssistantComposer,
  type ComposerOptions,
} from "@/components/assistant-composer";
import {
  readDraft,
  updateDraft,
  pendingSend,
  useDraftState,
  ensureDraft,
  retryDraftSave,
  sendDraft,
  resolveDraft,
  savedDraft,
  adoptSavedDraft,
  draftIndex,
  type DesktopEntry,
} from "@/lib/assistant-drafts";
import logoUrl from "@/assets/logo-small.png";
import {
  assistantActive,
  assistantApi,
  assistantState,
  needsReminderAttention,
  type AssistantSettings,
  type AssistantStep,
  type AssistantStepSummary,
  type AssistantTurn,
} from "@/lib/assistant";
import {
  modelCapabilitiesApi,
  modelCapabilitiesKey,
} from "@/lib/model-capabilities";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { AssistantTaskContextDetails } from "@/components/assistant-task-context";
import { readAssistantTaskContext } from "@/lib/assistant-task-context";
import { AssistantDocuments } from "@/components/assistant-documents";
import { AssistantRecovery, completedPlanNeedsReview, needsRecoveryNote } from "@/components/assistant-recovery";
import { incomingDraftText, prepareAssistantHandoff } from "@/lib/assistant-handoff";
import { HandoffStatus, useAssistantHandoff } from "@/components/workflow-assistant-entry";

const AssistantLocal = lazy(() => import("@/components/assistant-local")
  .then(({ AssistantLocal }) => ({ default: AssistantLocal })));
const AssistantPersonal = lazy(() => import("@/components/assistant-personal")
  .then(({ AssistantPersonal }) => ({ default: AssistantPersonal })));

const key = ["assistant"];
const button =
  "inline-flex items-center justify-center gap-2 rounded-lg border border-border px-3 py-2 text-sm hover:bg-secondary disabled:opacity-50";
const field =
  "w-full rounded-lg border border-border bg-background px-3 py-2 text-sm";

function liveTurnOptions(sessionId: string | null, turnId: string | null) {
  return queryOptions({
    queryKey: assistantTurnProgressKey(sessionId, turnId),
    queryFn: () => scopedTurn(sessionId!, turnId!, false),
    enabled: !!turnId,
    staleTime: 1500,
    retry: false,
    refetchInterval: (q) =>
      q.state.data &&
      (!assistantActive(q.state.data.state) ||
        (q.state.data.state === "waiting_input_queue" && q.state.data.queue_paused))
        ? false
        : 2500,
  });
}

function streamPreview(turn: AssistantTurn | undefined, kind: "dialogue_stream" | "task_stream") {
  return turn?.state === "running" && turn.live_progress?.state === kind
    ? turn.live_progress.output
    : undefined;
}

export function Assistant({
  session: requestedSession,
  embedded = false,
}: { session?: string; embedded?: boolean } = {}) {
  const [params] = useSearchParams();
  const location = useLocation();
  const navigate = useNavigate();
  const client = useQueryClient();
  const newRequest = useRef<string | null>(null);
  const events = useQuery<string>({
    queryKey: [...key, "events-error"],
    queryFn: async () => "",
    enabled: false,
  });
  const [selected, select] = useState<string | null>(
    requestedSession ?? params.get("session"),
  );
  const newWorkspace = useDraftState(null);
  const newInputIntent = useRef(false);
  const [comparison, setComparison] = useState<{
    session: string | null;
    entry: DesktopEntry;
  } | null>(null);
  const incomingDraft = useRef<{ key: string; text: string | undefined }>({
    key: location.key,
    text: location.state?.draft ?? params.get("draft") ?? undefined,
  });
  const [incomingProblem, setIncomingProblem] = useState<{key:string;text:string;message:string} | null>(null);
  const attemptedIncoming = useRef<string | undefined>(undefined);
  const separateIncoming = useAssistantHandoff(() => incomingProblem?.text ?? "");
  const scope = useRef<string | null>(null);
  const transcript = useRef<HTMLDivElement>(null);
  const followLatest = useRef(true);
  const [newMessages, setNewMessages] = useState(false);
  const [error, setError] = useState("");
  const refreshInFlight = useRef(false);
  const [refreshing, setRefreshing] = useState(false);
  const [operations, setOperations] = useState<Record<string, number>>({});
  const [settings, setSettings] = useState<AssistantSettings | null>(null);
  const earlierAnchor = useRef<{
    session: string;
    height: number;
    top: number;
  } | null>(null);
  const lastTail = useRef<{ session: string | null; signature: string } | null>(
    null,
  );
  const [personalOpen, setPersonalOpen] = useState(false);
  const [localOpen, setLocalOpen] = useState(false);
  const [draftsOpen, setDraftsOpen] = useState(false);
  const draftsDialog = useRef<HTMLDialogElement>(null);
  const drafts = useQuery({
    queryKey: [...key, "desktop-index"],
    queryFn: draftIndex,
    enabled: draftsOpen,
    staleTime: 0,
    retry: false,
  });
  useEffect(() => {
    if (draftsOpen) draftsDialog.current?.showModal();
    else draftsDialog.current?.close();
  }, [draftsOpen]);
  const recovered = useRef<string | null>(null);
  const config = useQuery({
    queryKey: [...key, "config"],
    queryFn: assistantApi.config,
    staleTime: 60_000,
    retry: false,
  });
  const sessions = useSessionCatalog({ enabled: !requestedSession || draftsOpen });
  const needsDesktop = !requestedSession && !selected && sessions.hasMore && !!sessions.data && !sessions.data.some(s=>s.source==='desktop');
  const desktopSessions = useSessionCatalog({ source:'desktop', enabled:needsDesktop });
  const defaultSession = sessions.data?.find(s=>s.source==='desktop') ??
    (needsDesktop ? desktopSessions.data?.[0] ?? (desktopSessions.complete ? sessions.data?.[0] : undefined) : sessions.data?.[0]);
  const personal = useQuery({
    queryKey: [...key, "personal"],
    queryFn: assistantApi.personal,
    staleTime: 60_000,
    retry: false,
  });
  const reminderStatusId = useId();
  const hasPendingReminder =
    personal.data?.reminders?.some((row) => needsReminderAttention(row.delivery)) ||
    personal.data?.matters?.entries.some((row) => needsReminderAttention(row.delivery));
  const sessionId = selected;
  const busy = (operations[sessionId ?? "new"] ?? 0) > 0;
  const workspace = useDraftState(sessionId);
  const { text, mode } = workspace.draft;
  useEffect(() => {
    void ensureDraft(null);
  }, []);
  useEffect(() => {
    const fromUrl = params.get("session");
    if (!requestedSession && fromUrl) select(fromUrl);
  }, [params, requestedSession]);
  scope.current = sessionId;
  function editText(value: string, expectedText?: string) {
    if (sessionId === null) newInputIntent.current = true;
    updateDraft(sessionId, { text: value }, expectedText);
  }
  useEffect(() => {
    if (
      !selected &&
      !newInputIntent.current &&
      newWorkspace.loaded &&
      !pendingSend(null) &&
      !readDraft(null).text &&
      !readDraft(null).files.length &&
      !readDraft(null).image &&
      !newWorkspace.voices.some(
        (v) => !["applied", "dismissed"].includes(v.phase),
      ) &&
      incomingDraft.current.text === undefined &&
      defaultSession
    )
      select(defaultSession.id);
  }, [selected, defaultSession, newWorkspace.loaded]);
  useEffect(() => {
    if (requestedSession) {
      select(requestedSession);
    }
  }, [requestedSession]);
  const windows = useInfiniteQuery({
    queryKey: [...key, "session", sessionId],
    queryFn: ({ pageParam }) => {
      const previous = client.getQueryData<{ pages: TimelinePage[] }>([
        ...key,
        "session",
        sessionId,
      ]);
      return timelinePage(
        sessionId!,
        pageParam,
        pageParam === null ? previous?.pages[0] : undefined,
      );
    },
    initialPageParam: null as string | null,
    getNextPageParam: (last) => last.page.before_message_id ?? undefined,
    select: selectSessionWindows,
    enabled: !!sessionId,
    staleTime: 60_000,
    retry: false,
  });
  const conversation = {
    data: windows.data?.combined,
    error: windows.error,
    isError: windows.isError,
    isPending: windows.isPending,
  };
  const queueGeneration = conversation.data?.queue_resume_generation;
  const dialogueId = conversation.data?.turns.find(
    (t) => t.interaction === "chat" && assistantActive(t.state) && t.state !== "waiting_input_queue",
  )?.id ?? null;
  const activeId =
    conversation.data?.turns.find(
      (t) => t.interaction !== "chat" && assistantActive(t.state) && t.state !== "waiting_input_queue",
    )?.id ??
    dialogueId ??
    conversation.data?.turns.find((t) => t.state === "waiting_input_queue")
      ?.id ??
    null;
  const turn = useQuery(liveTurnOptions(sessionId, activeId));
  const dialogue = useQuery(liveTurnOptions(sessionId, dialogueId === activeId ? null : dialogueId));
  const currentTurn = observedTurn(turn);
  const currentDialogue = observedTurn(dialogue);
  const preview = streamPreview(currentDialogue ?? currentTurn, "dialogue_stream");
  const taskPreview = streamPreview(currentTurn, "task_stream");
  const latestMessage = conversation.data?.messages.at(-1);
  useEffect(() => {
    earlierAnchor.current = null;
    followLatest.current = true;
    setNewMessages(false);
    setError("");
    setComparison(null);
  }, [sessionId]);
  useLayoutEffect(() => {
    const anchor = earlierAnchor.current;
    const node = transcript.current;
    if (anchor && node && anchor.session === sessionId) {
      node.scrollTop = anchor.top + node.scrollHeight - anchor.height;
      earlierAnchor.current = null;
    }
  }, [sessionId, conversation.data?.messages[0]?.id]);
  useEffect(() => {
    const signature = JSON.stringify([
      latestMessage?.id,
      latestMessage?.content,
      currentTurn?.state,
      currentTurn?.text,
      currentDialogue?.state,
      currentDialogue?.text,
      preview,
      taskPreview,
    ]);
    const changed =
      lastTail.current?.session === sessionId &&
      lastTail.current.signature !== signature;
    lastTail.current = { session: sessionId, signature };
    if (!followLatest.current) {
      if (latestMessage && changed) setNewMessages(true);
      return;
    }
    const frame = requestAnimationFrame(() => {
      if (transcript.current)
        transcript.current.scrollTop = transcript.current.scrollHeight;
    });
    return () => cancelAnimationFrame(frame);
  }, [
    sessionId,
    conversation.data?.messages.length,
    latestMessage?.id,
    latestMessage?.content,
    currentTurn?.state,
    currentTurn?.text,
    currentTurn?.progress?.current,
    currentDialogue?.state,
    currentDialogue?.text,
    preview,
    taskPreview,
  ]);
  const catalog = useQuery({
    queryKey: modelCapabilitiesKey,
    queryFn: modelCapabilitiesApi.catalog,
    enabled: !!settings,
    staleTime: 60_000,
    retry: false,
  });
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    if (settings) dialog.current?.showModal();
    else dialog.current?.close();
  }, [!!settings]);
  const terminalTurns = [currentTurn, currentDialogue]
    .filter((item) => item && !assistantActive(item.state))
    .map((item) => item!.id)
    .join(",");
  useEffect(() => {
    if (terminalTurns) {
      refreshAssistantReads(client, {
        queryKey: [...key, "session", sessionId],
      });
      void invalidateSessionCatalog(client);
    }
  }, [terminalTurns, client, sessionId]);
  async function action(run: () => Promise<unknown>) {
    const target = scope.current;
    const operationScope = target ?? "new";
    setOperations((values) => ({
      ...values,
      [operationScope]: (values[operationScope] ?? 0) + 1,
    }));
    setError("");
    try {
      await run();
    } catch (e) {
      if (scope.current === target) setError(String(e));
    } finally {
      setOperations((values) => ({
        ...values,
        [operationScope]: Math.max(0, (values[operationScope] ?? 1) - 1),
      }));
    }
  }
  async function refresh() {
    if (refreshInFlight.current) return;
    refreshInFlight.current = true;
    setRefreshing(true);
    try {
      await Promise.all([
        ...(identityReady && !readOnly ? [ensureDraft(sessionId, true)] : []),
        client.invalidateQueries(
          { predicate: (q) => q.queryKey[0] === "assistant" && q.queryKey[1] !== "sessions" },
          { cancelRefetch: false },
        ),
        invalidateSessionCatalog(client),
        client.invalidateQueries({
          queryKey: modelCapabilitiesKey,
          refetchType: "none",
        }),
      ]);
    } catch (cause) {
      if (scope.current === sessionId) setError(String(cause));
    } finally {
      refreshInFlight.current = false;
      setRefreshing(false);
    }
  }
  async function refreshConversation(target: string, turnId?: string) {
    refreshAssistantReads(client, { queryKey: [...key, "session", target] });
    if (turnId) refreshAssistantReads(client, { queryKey: assistantTurnKey(target, turnId) });
    await invalidateSessionCatalog(client);
  }
  useEffect(() => {
    const token = location.state?.newConversation as string | undefined;
    if (!token || token === newRequest.current || embedded) return;
    newRequest.current = token;
    void action(async () => {
      const s = await assistantApi.create();
      select(s.id);
      await refreshConversation(s.id);
    });
  }, [location.state?.newConversation]);
  const turns = useMemo(() => conversation.data?.turns.map((t) =>
    currentTurn?.id === t.id ? currentTurn : currentDialogue?.id === t.id ? currentDialogue : t,
  ) ?? [], [conversation.data?.turns, currentTurn, currentDialogue]);
  const turnsById = useMemo(() => new Map(turns.map((t) => [t.id, t])), [turns]);
  const userContexts = useMemo(() => {
    const contexts = new Map<string, NonNullable<ReturnType<typeof readAssistantTaskContext>>>();
    for (const message of conversation.data?.messages ?? []) {
      if (message.role !== "user") continue;
      const context = readAssistantTaskContext(message.content || "");
      if (context) contexts.set(message.id, context);
    }
    return contexts;
  }, [conversation.data?.messages]);
  const readOnly = conversation.data?.source === "wechat";
  const identityReady = !sessionId || !!conversation.data;
  function mergeIncoming(incoming: unknown, explicit = false) {
    const blocked = readOnly || !workspace.loaded || workspace.status === "error" || workspace.unconfirmed || !!workspace.pending || !!workspace.nativePending || workspace.voices.some(v => !["applied", "dismissed", "cancelled"].includes(v.phase));
    try {
      const old = readDraft(sessionId).text;
      const combined = incomingDraftText(old, incoming, blocked);
      if (!explicit && old && old !== incoming) throw Error("当前已有草稿，转入内容已保留；请确认合并，或单独保存，避免重复添加旧链接内容。");
      updateDraft(sessionId, { text: combined });
      incomingDraft.current.text = undefined;
      setIncomingProblem(null);
    } catch(reason) {
      setIncomingProblem({key: location.key, text: typeof incoming === "string" ? incoming : "", message: String(reason)});
    }
  }
  useEffect(() => {
    if (identityReady && !readOnly) void ensureDraft(sessionId);
  }, [sessionId, identityReady, readOnly]);
  useEffect(() => {
    if (incomingDraft.current.key !== location.key)
      incomingDraft.current = {
        key: location.key,
        text: location.state?.draft ?? params.get("draft") ?? undefined,
      };
    if (
      (!workspace.loaded && !readOnly) ||
      !identityReady ||
      (sessionId === null && sessions.isPending)
    )
      return;
    const incoming = incomingDraft.current.text;
    const attempt = location.key;
    if (incoming !== undefined && attemptedIncoming.current !== attempt) {
      attemptedIncoming.current = attempt;
      mergeIncoming(incoming);
    }
  }, [
    sessionId,
    location.key,
    workspace.loaded,
    identityReady,
    readOnly,
    sessions.isPending,
  ]);
  async function checkReceipt(target: string | null) {
    const ack = await resolveDraft(target);
    if (ack && scope.current === target) select(ack.session_id);
    if (ack) await refreshConversation(ack.session_id);
  }
  useEffect(() => {
    if (
      !workspace.loaded ||
      !workspace.pending ||
      workspace.pendingState !== "prepared" ||
      readOnly
    )
      return;
    const token = `${sessionId ?? "new"}:${workspace.pending.id}`;
    if (recovered.current === token) return;
    recovered.current = token;
    void checkReceipt(sessionId).catch(() => {});
  }, [sessionId, workspace.loaded]);
  const issue =
    error ||
    (!readOnly && workspace.error) ||
    events.data ||
    String(
      config.error ?? sessions.error ?? desktopSessions.error ?? conversation.error ?? turn.error ?? "",
    );
  function send(options: ComposerOptions) {
    if (
      (!text.trim() && !options.attachments?.length) ||
      busy ||
      !workspace.loaded ||
      !identityReady ||
      readOnly ||
      !config.data?.settings.enabled
    )
      return;
    void action(async () => {
      followLatest.current = true;
      setNewMessages(false);
      const previous = pendingSend(sessionId);
      const request =
        previous?.text === text &&
        previous.mode === mode &&
        JSON.stringify(previous.options) === JSON.stringify(options) &&
        previous.session === sessionId
          ? previous
          : {
              text,
              session: sessionId,
              id: crypto.randomUUID(),
              mode,
              options,
            };
      const ack = await sendDraft(request);
      if (scope.current === request.session) {
        select(ack.session_id);
      }
      await refreshConversation(ack.session_id);
    });
  }
  return (
    <div
      className={`assistant-workspace flex ${embedded ? "h-full" : "h-full"} min-h-0 min-w-0 flex-col`}
      data-empty={!conversation.data?.messages.length && !conversation.isError}
    >
      <Suspense fallback={<p role="status" className="px-4 py-2 text-sm text-muted-foreground">正在加载助理面板…</p>}>
        {localOpen && <AssistantLocal shown close={() => setLocalOpen(false)} />}
        {personalOpen && <AssistantPersonal onClose={() => setPersonalOpen(false)} />}
      </Suspense>
      <header className="chat-header">
        <div className="flex min-w-0 items-center gap-2">
          <details className="relative">
            <summary className="chat-agent-trigger">
              <h1 className="font-sans text-base font-semibold">AI 助理</h1>
              <ChevronDown size={14} />
            </summary>
            <div className="chat-agent-menu">
              <p className="px-3 py-2 text-xs text-muted-foreground">
                HomeBase 助理 · 当前会话
              </p>
              <Link
                className="block rounded-lg px-3 py-2 text-sm hover:bg-secondary"
                to="/conversations"
              >
                切换到 Codex / Claude / Gemini 会话
              </Link>
            </div>
          </details>
          {readOnly && (
            <span className="chat-channel">微信会话 · 在此查看和确认</span>
          )}
        </div>
        <div className="flex items-center gap-2">
          <AssistantContext sessionId={sessionId} />
          {readOnly && sessionId && <WechatDocuments key={sessionId} scope={sessionId} />}
          <button
            type="button"
            className="chat-header-action"
            aria-describedby={hasPendingReminder ? reminderStatusId : undefined}
            onClick={() => setPersonalOpen(true)}
          >
            记忆与提醒
            {hasPendingReminder && (
              <span
                aria-hidden="true"
                className="h-1.5 w-1.5 rounded-full bg-red-500"
              />
            )}
          </button>
          <span id={reminderStatusId} className="sr-only" role="status" aria-atomic="true">
            {hasPendingReminder ? "有提醒待处理" : ""}
          </span>
          <button
            type="button"
            className="chat-icon"
            aria-label="刷新助理"
            aria-busy={refreshing}
            disabled={refreshing}
            title="刷新助理"
            onClick={() => void refresh()}
          >
            <RefreshCw size={16} />
          </button>
          <details className="relative">
            <summary className="chat-icon" aria-label="更多对话操作">
              <MoreHorizontal size={20} />
            </summary>
            <div className="chat-agent-menu right-0">
              <button
                disabled={busy}
                onClick={() =>
                  void action(async () => {
                    const s = await assistantApi.create();
                    select(s.id);
                    await refreshConversation(s.id);
                  })
                }
              >
                <Plus size={15} />
                新对话
              </button>
              {!embedded && <Link to="/conversations">全部会话</Link>}
              <button onClick={() => setDraftsOpen(true)}>
                未发送与待核对
              </button>
              <button onClick={() => setLocalOpen(true)}>本机能力</button>
              <button
                disabled={!config.data}
                onClick={() => setSettings(config.data!.settings)}
              >
                <Settings size={15} />
                模型设置
              </button>
            </div>
          </details>
        </div>
      </header>
      {incomingProblem?.key === location.key && <section className="rounded-xl border border-border bg-card p-4" aria-label="待处理的转入内容">
        <p role="alert" className="text-sm">{incomingProblem.message}</p>
        <textarea aria-label="转入内容" className={`${field} mt-3 max-h-48`} value={incomingProblem.text} onChange={e => setIncomingProblem({...incomingProblem,text:e.target.value})} />
        <div className="mt-3 flex flex-wrap gap-2"><button className={button} disabled={separateIncoming.busy} onClick={() => void separateIncoming.prepare()}>单独保存为助理草稿</button><button className={button} onClick={() => mergeIncoming(incomingProblem.text, true)}>重新合并到当前草稿</button></div>
        <HandoffStatus handoff={separateIncoming}/>
      </section>}
      {issue && (
        <p
          role="alert"
          className="rounded-lg border border-destructive/30 p-3 text-sm text-destructive"
        >
          {issue}
        </p>
      )}
      <div className="flex min-h-0 flex-1 flex-col">
        <section className="relative flex min-h-0 min-w-0 flex-1 flex-col">
          <div
            className="chat-transcript min-h-0 flex-1 overflow-y-auto"
            ref={transcript}
            onScroll={(e) => {
              const node = e.currentTarget;
              followLatest.current =
                node.scrollHeight - node.clientHeight - node.scrollTop < 100;
              if (followLatest.current) setNewMessages(false);
            }}
            aria-label="对话内容"
          >
            {conversation.isPending && sessionId && (
              <p role="status">正在读取对话…</p>
            )}
            {!conversation.data?.messages.length && !conversation.isError && (
              <div className="chat-welcome">
                <img
                  src={logoUrl}
                  alt=""
                  className="mb-5 h-11 w-11 rounded-xl"
                />
                <h2>今天想一起完成什么？</h2>
                <p>发个想法、文件或任务，我来处理。</p>
                <div className="mt-6 flex flex-wrap justify-center gap-2">
                  {[
                    "查询当前系统状态",
                    "有哪些已接入的能力？",
                    "生成一个 6 秒的 HomeBase 动画",
                  ].map((s) => (
                    <button
                      key={s}
                      className="chat-suggestion"
                      onClick={() => editText(s)}
                    >
                      {s}
                    </button>
                  ))}
                </div>
                <details className="mt-3 text-xs text-muted-foreground">
                  <summary className="cursor-pointer">更多开始方式</summary>
                  <div className="mt-3 flex flex-wrap justify-center gap-2">
                    {[
                      "帮我收录一份资料，先读取资料、文稿与快捷任务 Skill",
                      "帮我写一份 Markdown 草稿，保存后给我预览和文件",
                    ].map((s) => (
                      <button
                        key={s}
                        className="chat-suggestion"
                        onClick={() => editText(s)}
                      >
                        {s}
                      </button>
                    ))}
                  </div>
                </details>
                {!config.data?.settings.enabled && (
                  <button
                    className="mt-7 text-sm text-muted-foreground underline underline-offset-4"
                    disabled={!config.data}
                    onClick={() => setSettings(config.data!.settings)}
                  >
                    {config.isError
                      ? "设置读取失败，请刷新重试"
                      : config.isPending
                        ? "正在读取助理设置…"
                        : "模型尚未启用 · 配置默认模型"}
                  </button>
                )}
              </div>
            )}
            {windows.hasNextPage && (
              <button
                className={button}
                disabled={windows.isFetching}
                onClick={() => {
                  const node = transcript.current;
                  if (!sessionId || !node || windows.isFetching) return;
                  earlierAnchor.current = {
                    session: sessionId,
                    height: node.scrollHeight,
                    top: node.scrollTop,
                  };
                  followLatest.current = false;
                  const target = sessionId;
                  void windows
                    .fetchNextPage({ cancelRefetch: false })
                    .then((result) => {
                      if (result.isError && scope.current === target)
                        earlierAnchor.current = null;
                    });
                }}
              >
                {windows.isFetchingNextPage
                  ? "正在读取更早消息…"
                  : windows.isFetchNextPageError
                    ? "重试读取更早消息"
                    : "显示更早消息"}
              </button>
            )}
            {windows.data &&
              windows.data.pages.length > 1 &&
              !windows.hasNextPage && (
                <p className="text-center text-xs text-muted-foreground">
                  已到会话开头
                </p>
              )}
            {conversation.data?.messages.map((m) => {
              const context = userContexts.get(m.id);
              const t = m.role === "assistant" && m.turn_id !== null
                ? turnsById.get(m.turn_id)
                : undefined;
              const partial = streamPreview(t, "dialogue_stream");
              const taskPartial = streamPreview(t, "task_stream");
              return (
                <article
                  key={m.id}
                  className={
                    m.role === "user"
                      ? "chat-message chat-message-user"
                      : "chat-message chat-message-assistant"
                  }
                >
                  {m.role === "assistant" && (
                    <img
                      src={logoUrl}
                      alt="HomeBase 助理"
                      className="chat-avatar"
                    />
                  )}
                  {m.attachments?.map((a, i) => (
                    <div
                      key={`${a.sha256}-${i}`}
                      className="mb-3 flex items-center gap-2 rounded-xl border border-border bg-background/60 px-3 py-2 text-sm"
                    >
                      <FileVideo size={17} />
                      <span>{a.name}</span>
                      <small className="text-muted-foreground">
                        {a.bytes.toLocaleString()} 字节
                      </small>
                    </div>
                  ))}
                  {context ? (
                    <div className="space-y-3">
                      <MarkdownRenderer source={context.task || "尚未填写具体需求。"} />
                      <AssistantTaskContextDetails view={context} sent />
                    </div>
                  ) : <MarkdownRenderer
                    source={
                      partial ||
                      (t && assistantActive(t.state) && t.progress && !t.pending
                        ? [
                            t.progress.completed.length
                              ? `已完成：${t.progress.completed.slice(-3).join("、")}`
                              : "",
                            t.progress.current
                              ? `正在做：${t.progress.current}`
                              : "",
                            t.progress.blocked
                              ? `目前卡在：${t.progress.blocked}`
                              : "",
                            t.progress.next ? `下一步：${t.progress.next}` : "",
                          ]
                            .filter(Boolean)
                            .join("\n\n")
                        : "") ||
                      t?.text ||
                      m.content ||
                      (m.role === "user" && m.attachments?.length
                        ? "请阅读这些附件。"
                        : "") ||
                      (t ? assistantState(t.state) : "暂无内容")
                    }
                  />}
                  {taskPartial && (
                    <div className="mt-3 rounded-lg border border-border bg-muted/20 p-3" aria-label="模型回复片段">
                      <p className="text-xs text-muted-foreground">模型回复片段 · 任务仍在执行</p>
                      <p className="mt-1 whitespace-pre-wrap break-words text-sm">{taskPartial}</p>
                    </div>
                  )}
                  {partial && t && (
                    <div className="mt-2 flex items-center gap-2 text-xs text-muted-foreground">
                      <span role="status">正在生成回复</span>
                      {!readOnly && (
                        <button
                          type="button"
                          className={button}
                          disabled={busy}
                          onClick={() => void action(async () => {
                            await assistantApi.cancel(t.id);
                            await refreshConversation(t.session_id, t.id);
                          })}
                        >
                          <Square size={12} aria-hidden="true" />
                          停止回复
                        </button>
                      )}
                    </div>
                  )}
                  {t && (
                    <AssistantRecovery
                      key={`${t.session_id}:${t.id}`}
                      turn={t}
                      busy={busy}
                      modelEnabled={!!config.data?.settings.enabled}
                      onInspect={async () => {
                        const turnKey = assistantTurnKey(t.session_id, t.id);
                        const observed = client.getQueryCache().find({ queryKey: turnKey, exact: true });
                        const version = observed?.state.dataUpdateCount;
                        const original = await scopedTurnInspection(t.session_id, t.id);
                        // Reuse this read only if no newer observation or active
                        // fetch has taken ownership of the same cache entry.
                        if (observed && client.getQueryCache().find({ queryKey: turnKey, exact: true }) === observed
                          && observed.state.dataUpdateCount === version && !observed.state.isInvalidated
                          && observed.state.fetchStatus !== "fetching") {
                          client.setQueryData(turnKey, original);
                        }
                        await Promise.all([
                          client.invalidateQueries({ queryKey: [...key, "session", t.session_id] }),
                          invalidateSessionCatalog(client),
                        ]);
                        return {
                          steps: original.step_summaries ?? [],
                          unverifiedJobs: original.unverified_accepted_jobs ?? 0,
                        };
                      }}
                      renderStep={(step) => (
                        <AssistantStepDetails
                          key={step.id}
                          step={step}
                          sessionId={t.session_id}
                          turnId={t.id}
                          expanded
                        />
                      )}
                      onResume={() => void action(async () => {
                        try {
                          await assistantApi.resume(t.id, t.resume_generation);
                        } finally {
                          // An acknowledgement can be lost after acceptance. Read
                          // the original state even on error, never send a new task.
                          await Promise.all([
                            client.invalidateQueries({ queryKey: [...key, "session", t.session_id] }),
                            client.invalidateQueries({ queryKey: assistantTurnKey(t.session_id, t.id) }),
                            invalidateSessionCatalog(client),
                          ]);
                        }
                      })}
                    />
                  )}
                  {m.role === "assistant" &&
                    !!(t?.text || m.content) &&
                    (!t || !assistantActive(t.state)) && (
                      <CopyMessage text={t?.text || m.content} />
                    )}
                  {context && <CopyMessage text={m.content} kind="完整消息" />}
                  {t?.interaction === "chat" && (
                    <p className="mt-2 text-xs text-muted-foreground">
                      {t.steered_turn_id
                        ? "补充已记入原任务 · 已受理的操作会先核对结果"
                        : "只读讨论 · 没有修改执行中的任务"}
                    </p>
                  )}
                  {t && t.interaction !== "chat" && (
                    <TurnCard
                      turn={t}
                      live={t === currentTurn || t === currentDialogue}
                      busy={busy}
                      act={action}
                      refresh={() => refreshConversation(t.session_id, t.id)}
                    />
                  )}
                </article>
              );
            })}
          </div>
          <div className="chat-composer-dock">
            <div className="chat-composer-width">
              {newMessages && (
                <button
                  type="button"
                  className="mb-3 inline-flex items-center gap-2 rounded-full border border-border px-3 py-1 text-xs text-muted-foreground hover:bg-secondary"
                  onClick={() => {
                    followLatest.current = true;
                    setNewMessages(false);
                    if (transcript.current)
                      transcript.current.scrollTop =
                        transcript.current.scrollHeight;
                  }}
                >
                  <span
                    className="h-1.5 w-1.5 rounded-full bg-primary"
                    aria-hidden="true"
                  />
                  回到最新回复
                </button>
              )}
              {turns.some(
                (t) => t.state === "waiting_input_queue" && t.queue_paused,
              ) && (
                <div className="mb-3 flex items-center justify-between gap-3 text-sm">
                  <span>
                    后续消息队列已暂停，原结果未知的任务不会自动重做。
                    {!readOnly && queueGeneration === undefined && " 请刷新原会话后再继续。"}
                  </span>
                  {!readOnly && (
                    <button
                      type="button"
                      className={button}
                      disabled={busy || queueGeneration === undefined}
                      onClick={() =>
                        void action(async () => {
                          if (!sessionId || queueGeneration === undefined) return;
                          try {
                            await assistantApi.resumeQueue(sessionId, queueGeneration);
                          } finally {
                            await Promise.all([
                              client.invalidateQueries({ queryKey: [...key, "session", sessionId], exact: true }),
                              client.invalidateQueries({ queryKey: assistantTurnProgressKey(sessionId, activeId), exact: true }),
                              invalidateSessionCatalog(client),
                            ]);
                          }
                        })
                      }
                    >
                      继续后续消息
                    </button>
                  )}
                </div>
              )}
              {!readOnly && (
                <div
                  className="mb-2 flex flex-wrap items-center gap-x-3 gap-y-2 text-xs text-muted-foreground"
                  aria-label="草稿保存状态"
                >
                  <span role="status">
                    {!workspace.loaded
                      ? workspace.loading
                        ? "正在恢复输入…"
                        : "输入尚未恢复"
                      : workspace.status === "saving"
                        ? "正在保存草稿…"
                        : workspace.status === "error"
                          ? "草稿保存或回执需要核对"
                          : workspace.dirty
                            ? "有尚未保存的修改"
                            : workspace.revision === 0
                              ? "输入就绪"
                              : "草稿已受保护保存"}
                  </span>
                  {(workspace.status === "error" || !workspace.loaded) && (
                    <button
                      type="button"
                      className="underline underline-offset-4"
                      disabled={busy || workspace.loading}
                      onClick={() =>
                        void action(() =>
                          workspace.loaded
                            ? retryDraftSave(sessionId)
                            : ensureDraft(sessionId, true),
                        )
                      }
                    >
                      重试保存或读取
                    </button>
                  )}
                  {(workspace.pending || workspace.unconfirmed) && (
                    <button
                      type="button"
                      className="underline underline-offset-4"
                      disabled={busy}
                      onClick={() => void action(() => checkReceipt(sessionId))}
                    >
                      查询原请求
                    </button>
                  )}
                  {workspace.status === "error" && workspace.loaded && (
                    <button
                      type="button"
                      className="underline underline-offset-4"
                      disabled={busy}
                      onClick={() =>
                        void action(async () =>
                          setComparison({
                            session: sessionId,
                            entry: await savedDraft(sessionId),
                          }),
                        )
                      }
                    >
                      查看已保存版本
                    </button>
                  )}
                  {!!workspace.warning && <span>{workspace.warning}</span>}
                </div>
              )}
              {comparison && comparison.session === sessionId && (
                <section
                  className="mb-3 rounded-xl border border-border p-3 text-sm"
                  aria-label="已保存草稿版本"
                >
                  <p>本机已保存版本（当前输入会保留到你确认替换）：</p>
                  <pre className="my-2 max-h-40 overflow-auto whitespace-pre-wrap font-sans">
                    {comparison.entry.body.text || "（无文字）"}
                  </pre>
                  <p className="mb-2 text-xs text-muted-foreground">
                    附件 {comparison.entry.body.files.length + Number(!!comparison.entry.body.image)} 个 ·{" "}
                    {comparison.entry.body.route || "默认模型"} · 版本{" "}
                    {comparison.entry.revision}
                  </p>
                  <button
                    type="button"
                    className={button}
                    onClick={() =>
                      void action(async () => {
                        adoptSavedDraft(sessionId, comparison.entry);
                        setComparison(null);
                        setError("");
                      })
                    }
                  >
                    使用已保存版本
                  </button>
                  <button
                    type="button"
                    className="ml-3 text-xs"
                    onClick={() => setComparison(null)}
                  >
                    保留当前输入
                  </button>
                </section>
              )}
              <AssistantComposer
                key={sessionId ?? "new"}
                session={sessionId}
                text={text}
                setText={editText}
                onIntent={() => {
                  if (sessionId === null) newInputIntent.current = true;
                }}
                mode={mode}
                setMode={(mode) => updateDraft(sessionId, { mode })}
                settings={config.isError ? undefined : config.data?.settings}
                busy={busy || !workspace.loaded || !identityReady}
                readOnly={readOnly}
                sendLabel={
                  mode === "task" && turns.some((t) => assistantActive(t.state))
                    ? "加入后续队列"
                    : "发送"
                }
                onSend={send}
                onError={setError}
              />
              <p className="chat-composer-caption">
                <span>
                  {config.isError
                    ? "设置读取失败"
                    : config.isPending
                      ? "正在读取设置…"
                      : !config.data?.settings.enabled
                        ? "模型未启用"
                        : config.data.settings.free_only
                          ? "已启用 · 仅免费模型"
                          : "模型请求可能产生费用"}
                </span>
                <span>
                  {" "}
                  ·{" "}
                  {readOnly
                    ? "请在微信继续交流，这里可以查看和确认。"
                    : !config.data?.settings.enabled
                      ? "可编辑已恢复的草稿 · Enter 换行；启用模型后再发送"
                      : "重要操作会请你确认 · Enter 发送，Shift+Enter 换行"}
                </span>
              </p>
            </div>
          </div>
        </section>
      </div>
      <dialog
        ref={draftsDialog}
        onCancel={() => setDraftsOpen(false)}
        className="w-[min(560px,92vw)] rounded-xl border border-border bg-card p-5 text-foreground shadow-xl backdrop:bg-black/35"
      >
        <div className="mb-4 flex items-center justify-between">
          <h2 className="font-serif text-xl">未发送与待核对</h2>
          <button
            className={button}
            aria-label="关闭草稿列表"
            onClick={() => setDraftsOpen(false)}
          >
            <X size={16} />
          </button>
        </div>
        <p className="mb-3 text-xs text-muted-foreground">
          这里只查看已保存摘要；打开不会发送消息或恢复任务。
        </p>
        {drafts.isPending && <p role="status">正在读取已保存草稿…</p>}
        {drafts.isError && (
          <p role="alert">
            {String(drafts.error)}{" "}
            <button className="underline" onClick={() => void drafts.refetch()}>
              重试读取
            </button>
          </p>
        )}
        {drafts.data && !drafts.data.complete && (
          <p role="alert" className="mb-3 text-sm text-destructive">
            有 {drafts.data.unreadable}{" "}
            份草稿不可读，原文件已保留；下面不是完整列表。
          </p>
        )}
        {drafts.data?.complete &&
          !drafts.data.entries.some(
            (e) => e.has_draft || e.pending_state || e.voice_pending_count,
          ) && <p className="text-sm">没有已保存的未发送内容或待核对请求。</p>}
        <div className="max-h-80 space-y-2 overflow-auto">
          {drafts.data?.entries
            .filter(
              (e) => e.has_draft || e.pending_state || e.voice_pending_count,
            )
            .map((entry) => (
              <button
                key={entry.scope}
                className="block w-full rounded-xl border border-border p-3 text-left hover:bg-secondary"
                disabled={busy}
                onClick={() => {
                  if (entry.scope.startsWith("handoff:")) {
                    const target = scope.current;
                    void action(async () => {
                      const original = await savedDraft(entry.scope);
                      if (original.scope !== entry.scope || !original.body.text) throw Error("交接暂存为空或标识未确认，未创建会话。");
                      const id = await prepareAssistantHandoff(original.body.text, entry.scope.slice("handoff:".length));
                      void invalidateSessionCatalog(client,false);
                      if (scope.current === target) { select(id); setDraftsOpen(false); }
                      void drafts.refetch();
                    });
                    return;
                  }
                  if (entry.scope.startsWith("native:")) {
                    const [, provider, id] = entry.scope.split(":");
                    navigate("/conversations", {
                      state: {
                        openConversation: {
                          provider,
                          id,
                          title: `${provider} · ${id.slice(0, 8)}`,
                        },
                      },
                    });
                  } else select(entry.scope === "new" ? null : entry.scope);
                  setDraftsOpen(false);
                }}
              >
                <span className="block text-sm">
                  {entry.scope === "new"
                    ? "尚未建立会话的输入"
                    : entry.scope.startsWith("handoff:")
                      ? `待转入助理 · ${entry.scope.slice(8,16)}`
                    : entry.scope.startsWith("native:")
                      ? `${entry.scope.split(":")[1]} · ${entry.scope.split(":")[2].slice(0, 8)}`
                      : sessions.data?.find((s) => s.id === entry.scope)
                          ?.title || `会话 ${entry.scope.slice(0, 8)}`}
                </span>
                <span className="text-xs text-muted-foreground">
                  {entry.pending_state === "prepared"
                    ? "发送受理待核对"
                    : entry.voice_pending_count
                      ? "有录音待核对"
                      : entry.pending_state === "rejected"
                        ? "上次未受理，草稿已保留"
                        : "有未发送草稿"}
                </span>
              </button>
            ))}
        </div>
      </dialog>
      <dialog
        ref={dialog}
        onCancel={() => setSettings(null)}
        className="w-[min(620px,92vw)] rounded-xl border border-border bg-card p-6 text-foreground shadow-xl backdrop:bg-black/35"
      >
        <div className="flex items-center justify-between">
          <h2 className="font-serif text-xl">助理模型设置</h2>
          <button
            className={button}
            aria-label="关闭模型设置"
            onClick={() => setSettings(null)}
          >
            <X size={16} />
          </button>
        </div>
        {settings && (
          <form
            className="mt-5 space-y-5"
            onSubmit={(e) => {
              e.preventDefault();
              void action(async () => {
                await assistantApi.save(settings);
                setSettings(null);
                await refresh();
              });
            }}
          >
            <label className="flex gap-3 text-sm">
              <input
                type="checkbox"
                checked={settings.enabled}
                onChange={(e) =>
                  setSettings({ ...settings, enabled: e.target.checked })
                }
              />
              启用默认模型（桌面和微信共用）
            </label>
            <label className="block text-sm">
              模型与调用渠道
              <select
                aria-label="默认模型渠道"
                className={`${field} mt-2`}
                value={settings.route_id}
                onChange={(e) =>
                  setSettings({ ...settings, route_id: e.target.value })
                }
              >
                <option value="">暂不选择</option>
                {catalog.data?.models
                  .filter((m) => ["chat", "responses"].includes(m.protocol))
                  .map((m) => (
                    <option key={m.route_id} value={m.route_id}>
                      {m.name} · {m.provider_id} · {m.protocol}
                    </option>
                  ))}
              </select>
            </label>
            <label className="block text-sm">
              凭据来源
              <select
                aria-label="助理凭据来源"
                className={`${field} mt-2`}
                value={settings.credential_source ?? "default"}
                onChange={(e) =>
                  setSettings({
                    ...settings,
                    credential_source: e.target
                      .value as AssistantSettings["credential_source"],
                  })
                }
              >
                <option value="default">沿用渠道默认优先级</option>
                <option value="sops">明确使用 SOPS 引用</option>
                <option value="api_key_manager">
                  明确使用独立凭据（API Key Manager）
                </option>
              </select>
            </label>
            <p className="text-xs text-muted-foreground">
              只引用已有凭据；所选来源失败就停止，不回退、不覆盖其他
              Key。桌面与微信共用此设置。
            </p>
            <label className="flex gap-3 text-sm">
              <input
                type="checkbox"
                checked={settings.free_only ?? false}
                onChange={(e) =>
                  setSettings({ ...settings, free_only: e.target.checked })
                }
              />
              仅免费模型（零价格上限，禁止付费回退）
            </label>
            {catalog.isPending && (
              <p role="status" className="text-sm">
                正在读取已有模型目录…
              </p>
            )}
            {catalog.error && (
              <p className="text-sm text-destructive">
                模型目录读取失败：{String(catalog.error)}
              </p>
            )}
            <p className="text-xs leading-6 text-muted-foreground">
              只保存已有 Key
              的引用，不在此填写密钥。保存设置仅检查本机可用性，不发送测试推理；实际工具调用支持以厂商响应为准。未启用时不会请求模型。
            </p>
            <label className="flex gap-3 text-sm">
              <input
                type="checkbox"
                checked={settings.wechat_enabled}
                onChange={(e) =>
                  setSettings({ ...settings, wechat_enabled: e.target.checked })
                }
              />
              微信普通消息使用此助理（明确指令继续兼容）
            </label>
            <div className="grid grid-cols-2 gap-4">
              <label className="text-sm">
                每轮工具循环上限
                <input
                  className={`${field} mt-2`}
                  type="number"
                  min="1"
                  max="12"
                  value={settings.max_rounds}
                  onChange={(e) =>
                    setSettings({
                      ...settings,
                      max_rounds: Number(e.target.value),
                    })
                  }
                />
              </label>
              <label className="text-sm">
                单次回复 Token 上限
                <input
                  className={`${field} mt-2`}
                  type="number"
                  min="256"
                  max="8192"
                  value={settings.max_tokens}
                  onChange={(e) =>
                    setSettings({
                      ...settings,
                      max_tokens: Number(e.target.value),
                    })
                  }
                />
              </label>
            </div>
            <p className="break-all text-xs text-muted-foreground">
              生成结果目录：{config.data?.workspace_root}
            </p>
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
            <div className="flex justify-end">
              <button
                className={button}
                disabled={busy || (settings.enabled && !settings.route_id)}
              >
                保存设置
              </button>
            </div>
          </form>
        )}
      </dialog>
    </div>
  );
}

function WechatDocuments({ scope }: { scope: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="chat-header-action" onClick={() => setOpen(true)}>
        微信 PDF
      </button>
      {open && <AssistantDocuments scope={scope} open readOnly onClose={() => setOpen(false)} />}
    </>
  );
}

function CopyMessage({ text, kind = "回复" }: { text: string; kind?: "回复" | "完整消息" }) {
  const [copied, setCopied] = useState(false);
  const [failed, setFailed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  return (
    <span className="mt-3 inline-flex items-center gap-2">
      <button
        type="button"
        className="chat-icon"
        aria-label={copied ? `已复制${kind}` : `复制${kind}`}
        onClick={() => {
          void navigator.clipboard
            .writeText(text)
            .then(() => {
              setCopied(true);
              setFailed(false);
              if (timer.current) clearTimeout(timer.current);
              timer.current = setTimeout(() => setCopied(false), 2000);
            })
            .catch(() => setFailed(true));
        }}
      >
        {copied ? <Check size={15} /> : <Copy size={15} />}
      </button>
      {failed && (
        <small role="status" className="text-muted-foreground">
          复制失败，可选中文字复制。
        </small>
      )}
    </span>
  );
}

const stepLabels: Record<string, string> = {
  homebase_files: "处理工作区文件",
  homebase_terminal: "执行隔离命令",
  homebase_mcp: "使用已授权连接",
  homebase_installed_skill: "使用本机技能",
  homebase_skill: "查看使用说明",
  homebase_video: "制作动画",
  homebase_memory: "管理长期记忆",
  homebase_reminder: "管理提醒",
  homebase_deliver: "准备结果文件",
  homebase_discover: "查找可用能力",
  homebase_describe: "核对接口",
  homebase_call: "调用能力",
  homebase_job: "查询任务",
  homebase_jobs: "查看最近任务",
};

function AssistantStepDetails({
  step,
  sessionId,
  turnId,
  expanded,
}: {
  step: AssistantStepSummary | AssistantStep;
  sessionId: string;
  turnId: string;
  expanded: boolean;
}) {
  const [open, setOpen] = useState(false);
  return (
    <details
      className="rounded-lg border border-border p-3"
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="cursor-pointer">
        {stepLabels[step.name] || step.name} · {assistantState(step.state)}
      </summary>
      {expanded && open && (
        <AssistantStepBody step={step} sessionId={sessionId} turnId={turnId} />
      )}
    </details>
  );
}

function AssistantStepBody({
  step,
  sessionId,
  turnId,
}: {
  step: AssistantStepSummary | AssistantStep;
  sessionId: string;
  turnId: string;
}) {
  const inline = "arguments" in step ? step : undefined;
  const query = useQuery({
    queryKey: assistantTurnStepKey(sessionId, turnId, step.id),
    queryFn: () => scopedTurnStep(sessionId, turnId, step.id),
    enabled: !inline,
    staleTime: Infinity,
    retry: false,
  });
  const original = inline ?? (query.isFetching || query.isError ? undefined : query.data);
  const body = useMemo(
    () => original
      ? JSON.stringify({ parameters: original.arguments, job_id: original.job_id, result: original.result }, null, 2)
      : null,
    [original],
  );
  if (!inline && (query.isPending || query.isFetching))
    return <p className="mt-2" role="status">正在读取该步骤的原结果…</p>;
  if (!inline && query.isError)
    return (
      <p className="mt-2" role="alert">
        步骤结果暂不可读。{String(query.error)}
        <button type="button" className="ml-2 underline" onClick={() => void query.refetch()}>
          重试读取步骤
        </button>
      </p>
    );
  return (
    <pre className="mt-2 max-h-56 overflow-auto whitespace-pre-wrap break-all text-xs">
      {body}
    </pre>
  );
}

function TurnCard({
  turn: initialTurn,
  live,
  busy: externalBusy,
  act,
  refresh,
}: {
  turn: AssistantTurn;
  live: boolean;
  busy: boolean;
  act: (run: () => Promise<unknown>) => Promise<void>;
  refresh: () => Promise<void>;
}) {
  const client = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const full = useQuery({
    queryKey: assistantTurnKey(initialTurn.session_id, initialTurn.id),
    queryFn: () => scopedTurnInspection(initialTurn.session_id, initialTurn.id),
    enabled: expanded && !!initialTurn.details_omitted,
    staleTime: 1500,
    retry: false,
  });
  const detail = observedTurn(full);
  const turn = !live && initialTurn.details_omitted && !assistantActive(initialTurn.state)
    ? detail ?? initialTurn
    : initialTurn;
  const busy = externalBusy || (!live && full.isError);
  return (
    <div className="mt-3 space-y-3 text-sm">
      <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
        <span>{completedPlanNeedsReview(turn) ? "结果待核对" : assistantState(turn.state)}</span>
        {!!turn.usage?.length && (
          <span>
            {turn.usage.every((u) => u.cost !== null)
              ? `厂商报告费用：$${turn.usage.reduce((n, u) => n + (u.cost ?? 0), 0).toFixed(6)}`
              : "部分模型请求未返回费用"}
          </span>
        )}
        {assistantActive(turn.state) && (
          <button
            className={button}
            disabled={busy}
            onClick={() =>
              void act(async () => {
                await assistantApi.cancel(turn.id);
                await refresh();
              })
            }
          >
            <Square size={12} />
            停止后续步骤
          </button>
        )}
      </div>
      {turn.task?.stop && !needsRecoveryNote(turn) && (
        <div className="rounded-lg border border-border p-3" role="status">
          <p>{turn.task.stop.message}</p>
          <p className="mt-1 text-xs text-muted-foreground">
            {turn.task.stop.next_action}
          </p>
        </div>
      )}
      <details
        className="text-xs text-muted-foreground"
        onToggle={(event) => setExpanded(event.currentTarget.open)}
      >
        <summary className="cursor-pointer">
          执行详情 · {turn.step_count ?? turn.steps.length} 步
        </summary>
        {initialTurn.details_omitted && full.isPending && expanded && (
          <p role="status" className="mt-2">
            正在读取原任务步骤清单…
          </p>
        )}
        {initialTurn.details_omitted && full.isError && (
          <p role="alert" className="mt-2">
            详情读取失败，原任务未修改。{String(full.error)}
            <button
              className="ml-2 underline"
              onClick={() => void full.refetch()}
            >
              重试读取
            </button>
          </p>
        )}
        <p className="mt-2 select-all">任务：{turn.id}</p>
        <p>
          {turn.runtime === "deepagents"
            ? "Deep Agents / LangGraph"
            : turn.runtime}
        </p>
        {turn.task?.plan && (
          <div
            className="rounded-lg border border-border bg-muted/20 p-3"
            aria-label="任务计划"
          >
            <p className="font-medium">{turn.task.plan.goal}</p>
            <ol className="mt-2 space-y-2">
              {turn.task.plan.items.map((item) => (
                <li key={item.id} className="flex items-start gap-2">
                  <span
                    className={
                      item.state === "completed"
                        ? "text-primary"
                        : "text-muted-foreground"
                    }
                    aria-hidden="true"
                  >
                    {item.state === "completed"
                      ? "✓"
                      : item.state === "in_progress"
                        ? "◉"
                        : "○"}
                  </span>
                  <div className="min-w-0">
                    <p>
                      {item.title}{" "}
                      <span className="text-xs text-muted-foreground">
                        {assistantState(item.state)}
                      </span>
                    </p>
                    {item.note && (
                      <p className="text-xs text-muted-foreground">
                        {item.note}
                      </p>
                    )}
                    <details className="mt-1 text-xs text-muted-foreground">
                      <summary className="cursor-pointer">
                        完成条件与证据
                      </summary>
                      <p>{item.done_when}</p>
                      <p className="break-all">
                        {item.evidence.length
                          ? item.evidence.join("、")
                          : "尚无完成证据"}
                      </p>
                    </details>
                  </div>
                </li>
              ))}
            </ol>
            <p className="mt-3 text-xs text-muted-foreground">
              推理 {turn.task.limits.model_calls_used}/
              {turn.task.limits.max_model_calls} · 步骤{" "}
              {turn.task.limits.steps_used}/{turn.task.limits.max_steps} ·
              本次执行 {Math.floor(turn.task.limits.active_seconds / 60)}/
              {Math.floor(turn.task.limits.max_active_seconds / 60)} 分钟
            </p>
          </div>
        )}
        {turn.live_progress && turn.live_progress.state !== "task_stream" && (
          <details className="rounded-lg border border-border p-3">
            <summary className="cursor-pointer">最近执行输出</summary>
            <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-all text-xs">
              {turn.live_progress.output ||
                turn.live_progress.message ||
                "正在处理，暂未产生输出"}
            </pre>
          </details>
        )}
        {(full.data?.step_summaries ?? turn.steps).map((step) => (
          <AssistantStepDetails
            key={step.id}
            step={step}
            sessionId={initialTurn.session_id}
            turnId={initialTurn.id}
            expanded={expanded && !full.isError}
          />
        ))}
      </details>
      {turn.state === "awaiting_confirmation" && turn.pending && (
        <div className="rounded-lg border border-primary/30 bg-primary/5 p-4">
          <p className="font-medium">
            需要你的确认：{turn.pending.description}
          </p>
          <pre className="my-3 max-h-60 overflow-auto whitespace-pre-wrap break-all text-xs">
            {JSON.stringify(turn.pending.arguments, null, 2)}
          </pre>
          <p className="mb-3 text-xs text-muted-foreground">
            只批准此处展示的具体操作。请核对对象和参数后继续。
          </p>
          <div className="flex gap-2">
            {[true, false].map((approve) => (
              <button
                key={String(approve)}
                className={button}
                disabled={busy}
                onClick={() =>
                  void act(async () => {
                    try {
                      await assistantApi.confirm(turn.id, turn.pending!.id, approve);
                    } finally {
                      await Promise.all([
                        client.invalidateQueries({ queryKey: [...key, "session", turn.session_id], exact: true }),
                        client.invalidateQueries({ queryKey: assistantTurnKey(turn.session_id, turn.id) }),
                        invalidateSessionCatalog(client),
                      ]);
                    }
                  })
                }
              >
                {approve ? "确认执行" : "拒绝"}
              </button>
            ))}
          </div>
        </div>
      )}
      {(initialTurn.artifact_count ?? 0) > turn.artifacts.length && (
        <p className="text-xs text-muted-foreground">
          当前展示 {turn.artifacts.length}/{initialTurn.artifact_count}{" "}
          个结果，展开执行详情可查看其余结果。
        </p>
      )}
      <AssistantArtifacts artifacts={turn.artifacts} />
    </div>
  );
}
