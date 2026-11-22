import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useClock } from "@/lib/use-clock";
import { listen } from "@tauri-apps/api/event";
import {
  AlertCircle,
  AlertTriangle,
  Bot,
  Check,
  ChevronRight,
  Edit3,
  FileSearch,
  FileText,
  Hammer,
  Image as ImageIcon,
  Loader2,
  MessagesSquare,
  PenLine,
  Plus,
  RotateCcw,
  Search,
  Send,
  Sparkles,
  Square,
  User,
  X,
} from "lucide-react";
import {
  studioApi,
  type ChatMessage,
  type ImageProgress,
  type StudioEvent,
  type StudioSessionMeta,
  type TurnPart,
} from "@/lib/tauri-api";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { cn, formatRelativeTime } from "@/lib/utils";
import { WorkflowAssistantEntry } from "@/components/workflow-assistant-entry";
import { studioHandoff } from "@/lib/workflow-context";
import { useWorkflowVisibility } from "@/lib/workflow-visibility";

/** 当前流式回合的本地 state · completed 时合并成 ChatMessage 入 messages 数组 */
interface TurnState {
  /** 有序子部分 · text + tool 按 claude 输出顺序排列 */
  parts: TurnPart[];
  /** 任务统计（result_stats 事件填）*/
  stats: {
    total_cost_usd: number;
    duration_ms: number;
    num_turns: number;
  } | null;
}

const EMPTY_TURN: TurnState = { parts: [], stats: null };

/**
 * Studio — 写作 · AI 生成（Phase D · 2026-05-06 启动）
 *
 * 双流工坊：
 * - 新建对话：空白开始 → 可选指定资料源 → Claude CLI 驱动 → 生成文章 + 配图
 * - 改现有 wiki：从 Knowledge 跳入 → Tiptap 编辑 + 侧栏 AI（Phase D4）
 *
 * 产出落：C:\HomeBase\Wiki\_raw\studio-<ts>-<slug>.md
 * 对话持久化：C:\HomeBase\Wiki\_raw\studio-<ts>-<slug>.chat.jsonl
 *
 * 当前状态：
 * - **D2.1 已完成**（2026-05-06 · 后端 studio_list_sessions + 前端 useQuery 接入）
 * - D2.2 进行中：studio_chat_send 流式 + 对话 UI
 */

export function Studio() {
  const active=useWorkflowVisibility();
  const queryClient = useQueryClient();
  // 视图状态：empty 显示 EmptyStudio · chat 显示 ChatView
  const [activeView, setActiveView] = useState<"empty" | "chat">("empty");
  // 当前会话 ID（首次发送前为 null · 后端首次响应后赋值）
  const [activeSessionId, setActiveSessionId] = useState<string | null>(null);
  // 当前会话的消息列表（D2.2 仅前端 state · D2.3 jsonl 加持久化 + 切换会话恢复）
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  // 流式状态
  const [currentTaskId, setCurrentTaskId] = useState<string | null>(null);
  // **D2.7**：流式当前回合 · text + tool 按时序排成 parts · completed 时落到 messages
  const [streamingTurn, setStreamingTurn] = useState<TurnState>(EMPTY_TURN);
  const [streamingError, setStreamingError] = useState<string | null>(null);
  const [studioInput, setStudioInput] = useState("");

  // 用 ref 避免 listen 闭包捕获旧 state
  const taskIdRef = useRef<string | null>(null);
  const turnRef = useRef<TurnState>(EMPTY_TURN);

  // D2.1 真实接 studio_list_sessions（扫 C:\HomeBase\Wiki\_raw\studio-*.chat.jsonl）
  const sessions = useQuery({
    queryKey: ["studio", "sessions"],
    queryFn: () => studioApi.listSessions(),
    enabled: active,
    refetchInterval: active ? 30_000 : false, // 离页保留原任务监听，暂停列表读取
  });

  // 订阅 studio:event · 流式接收 Claude CLI 输出
  // **D2.7**：处理 4 个结构化事件（assistant_text/tool_use/tool_result/result_stats）·
  //         按时序累积到 turnRef.parts · completed 时合并成 assistant ChatMessage
  useEffect(() => {
    // 把 text append 到当前 turn · 智能合并到末尾 text part（如有）· 减少 parts 数量
    const appendText = (text: string) => {
      const t = turnRef.current;
      const lastPart = t.parts[t.parts.length - 1];
      const newParts =
        lastPart?.kind === "text"
          ? [...t.parts.slice(0, -1), { kind: "text" as const, text: lastPart.text + text }]
          : [...t.parts, { kind: "text" as const, text }];
      const newTurn: TurnState = { ...t, parts: newParts };
      turnRef.current = newTurn;
      setStreamingTurn(newTurn);
    };

    const resetTurn = () => {
      turnRef.current = EMPTY_TURN;
      setStreamingTurn(EMPTY_TURN);
      taskIdRef.current = null;
      setCurrentTaskId(null);
    };

    const unlistenPromise = listen<StudioEvent>("studio:event", (e) => {
      const ev = e.payload;
      // 只处理当前活动 task 的事件 · 避免历史任务残留 emit 干扰
      if (taskIdRef.current === null || ev.task_id !== taskIdRef.current) return;

      switch (ev.kind) {
        case "started":
          // started 已由 chatSend 调用方处理 · 这里 noop
          break;

        case "assistant_text":
          appendText(ev.text);
          break;

        case "tool_use": {
          const t = turnRef.current;
          const newTurn: TurnState = {
            ...t,
            parts: [
              ...t.parts,
              {
                kind: "tool",
                tool_use_id: ev.tool_use_id,
                name: ev.name,
                input_summary: ev.input_summary,
                result_preview: null,
                is_error: false,
                finished: false,
              },
            ],
          };
          turnRef.current = newTurn;
          setStreamingTurn(newTurn);
          break;
        }

        case "tool_result": {
          const t = turnRef.current;
          const newParts = t.parts.map((p) =>
            p.kind === "tool" && p.tool_use_id === ev.tool_use_id
              ? {
                  ...p,
                  result_preview: ev.content_preview,
                  is_error: ev.is_error,
                  finished: true,
                }
              : p,
          );
          const newTurn: TurnState = { ...t, parts: newParts };
          turnRef.current = newTurn;
          setStreamingTurn(newTurn);
          break;
        }

        case "result_stats": {
          const t = turnRef.current;
          const newTurn: TurnState = {
            ...t,
            stats: {
              total_cost_usd: ev.total_cost_usd,
              duration_ms: ev.duration_ms,
              num_turns: ev.num_turns,
            },
          };
          turnRef.current = newTurn;
          setStreamingTurn(newTurn);
          break;
        }

        case "stream":
          // fallback raw 行 · 也累到 text part（debug · 一般不会进这里）
          appendText(ev.line + "\n");
          break;

        case "stderr":
          // stderr 通常是 Claude 的调试信息 · 不进消息流 · 留 console
          console.warn("[studio:stderr]", ev.line);
          break;

        case "completed": {
          // 流式结束 · 把 turn 合并成 ChatMessage 落到 messages
          const turn = turnRef.current;
          const content = turn.parts
            .filter((p): p is Extract<TurnPart, { kind: "text" }> => p.kind === "text")
            .map((p) => p.text)
            .join("\n\n")
            .trim();
          let persist: Promise<unknown> = Promise.resolve();
          if (turn.parts.length > 0) {
            const assistantMsg: ChatMessage = {
              ts: Date.now(),
              role: "assistant",
              content,
              model: "claude",
              parts: turn.parts,
              stats: turn.stats,
            };
            setMessages((prev) => [...prev, assistantMsg]);
            // **D2.7 持久化**：append assistant message 到 jsonl · 失败仅警告不阻塞 UI
            const sid = ev.session_id;
            if (sid) {
              persist = studioApi.persistMessage(sid, assistantMsg);
            }
          }
          resetTurn();
          // Refresh after the append completes, so a fresh cache cannot capture
          // the old list between task completion and persistence.
          void persist.catch(() => {
            setStreamingError("回复已生成，但会话保存失败；请核对保存状态。");
          }).finally(() => {
            void queryClient.invalidateQueries({ queryKey: ["studio", "sessions"] });
            void queryClient.invalidateQueries({ queryKey: ["wiki"] });
            void queryClient.invalidateQueries({ queryKey: ["docs"] });
          });
          break;
        }

        case "failed":
          setStreamingError(`Claude 失败 (exit ${ev.exit_code}): ${ev.message}`);
          resetTurn();
          break;

        case "cancelled":
          setStreamingError("已取消");
          resetTurn();
          break;
      }
    });

    return () => {
      unlistenPromise.then((fn) => fn()).catch(() => {});
    };
  }, [queryClient]);

  // 点「新建对话」· 进入空 chat 视图
  const handleNewChat = () => {
    setActiveView("chat");
    setActiveSessionId(null);
    setMessages([]);
    setStreamingTurn(EMPTY_TURN);
    setStreamingError(null);
    turnRef.current = EMPTY_TURN;
    taskIdRef.current = null;
    setCurrentTaskId(null);
  };

  // 发送一条 USER 消息
  const handleSend = async (userMsg: string) => {
    if (!userMsg.trim() || currentTaskId !== null) return;

    // 多轮上下文（D2.6）· 当前 messages 作为 history（不含本轮 user_message）
    // 注意：必须先快照再 append · 否则 history 会带本轮自己造成重复
    const historySnapshot: ChatMessage[] = [...messages];

    // 立即把 user 消息添加到列表（乐观更新）
    const userMessage: ChatMessage = {
      ts: Date.now(),
      role: "user",
      content: userMsg,
    };
    setMessages((prev) => [...prev, userMessage]);
    setStreamingError(null);
    setStreamingTurn(EMPTY_TURN);
    turnRef.current = EMPTY_TURN;

    try {
      const result = await studioApi.chatSend({
        session_id: activeSessionId,
        user_message: userMsg,
        history: historySnapshot,
      });
      // 后端可能回传新生成的 session_id · 更新前端
      if (result.session_id !== activeSessionId) {
        setActiveSessionId(result.session_id);
      }
      taskIdRef.current = result.task_id;
      setCurrentTaskId(result.task_id);
      // **E1 Checkpoint**：把 task_id 写入这条 user message 的 checkpoint_id
      // 用 ts 作 key 找到刚才插入的那条 · 替换为带 checkpoint_id 的版本
      const userMessageWithCheckpoint: ChatMessage = {
        ...userMessage,
        checkpoint_id: result.task_id,
      };
      setMessages((prev) =>
        prev.map((m) => (m.ts === userMessage.ts && m.role === "user" ? userMessageWithCheckpoint : m))
      );
      // **D2.7 持久化** + **E1**：append 带 checkpoint_id 的 user message 到 jsonl
      studioApi.persistMessage(result.session_id, userMessageWithCheckpoint).catch((e) => {
        console.warn("[studio] persist user message 失败", e);
      });
    } catch (err) {
      setStreamingError(`发送失败: ${String(err)}`);
      taskIdRef.current = null;
      setCurrentTaskId(null);
    }
  };

  // **E1**：撚回某条 user message 引发的所有改动 + 删除该条之后的所有消息
  const handleRestoreCheckpoint = async (msg: ChatMessage) => {
    if (!msg.checkpoint_id || !activeSessionId || currentTaskId !== null) return;
    if (
      !window.confirm(
        `确定撚回这条消息引发的 AI 改动？\n该消息及之后所有消息都会被删除 · _raw 文件会还原到发送前。`
      )
    ) {
      return;
    }
    try {
      const restored = await studioApi.restoreCheckpoint(
        activeSessionId,
        msg.checkpoint_id,
        msg.ts
      );
      setMessages(restored);
      setStreamingError(null);
      setStreamingTurn(EMPTY_TURN);
      turnRef.current = EMPTY_TURN;
      sessions.refetch();
    } catch (e) {
      setStreamingError(`撚回失败: ${String(e)}`);
    }
  };

  // 取消当前流式任务
  const handleCancel = async () => {
    if (currentTaskId === null) return;
    try {
      await studioApi.cancel(currentTaskId);
    } catch (err) {
      console.error("studio_cancel 失败", err);
    }
  };

  // 点会话列表项 · **D2.7**：从 jsonl 载回 messages
  const handleSelectSession = async (sessionId: string) => {
    if (currentTaskId !== null) return; // 流式中不允许切换
    setActiveSessionId(sessionId);
    setActiveView("chat");
    setMessages([]); // 先清空 · 避免闪旧会话残影
    setStreamingTurn(EMPTY_TURN);
    turnRef.current = EMPTY_TURN;
    setStreamingError(null);
    try {
      const loaded = await studioApi.loadSession(sessionId);
      setMessages(loaded);
    } catch (e) {
      setStreamingError(`加载会话失败: ${String(e)}`);
    }
  };

  return (
    <div className="animate-fade-in">
      <header className="hb-page-header mb-6">
        <h1>文章生成</h1>
        <p>
          空白对话开始 · 可选让 AI 读指定 Wiki 资料 · 生成新文章和配图 · 产出落到{" "}
          <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
            C:\HomeBase\Wiki\_raw\
          </code>{" "}
          作为草稿
        </p>
      </header>

      <div className="mb-6"><WorkflowAssistantEntry
        title="在助理中继续文稿"
        label={currentTaskId ? "交给助理跟进" : "交给助理"}
        description={currentTaskId ? "原任务继续运行；保存其编号和当前内容为草稿，仅查询原任务，不重新生成。" : "将文稿摘录与未发送的要求保存为独立草稿；预览、修改和发布可在助理中继续。"}
        getText={() => studioHandoff({ session: activeSessionId, task: currentTaskId, input: studioInput, messages, error: streamingError })}
      /></div>

      <div className="grid min-h-[calc(100vh-14rem)] grid-cols-1 gap-6 md:grid-cols-[240px_1fr]">
        {/* 左栏：会话列表 + 新建 */}
        <aside className="hb-card flex flex-col overflow-hidden p-0">
          <div className="border-b border-border p-3">
            <button
              type="button"
              onClick={handleNewChat}
              disabled={currentTaskId !== null}
              className="inline-flex w-full items-center justify-center gap-2 rounded-md bg-primary-600 px-3 py-2 text-sm font-medium text-white transition-colors hover:bg-primary-700 active:bg-primary-800 disabled:cursor-not-allowed disabled:opacity-50"
              title={currentTaskId !== null ? "流式中 · 等当前完成或取消" : "新建对话"}
            >
              <Plus className="h-4 w-4" aria-hidden />
              新建对话
            </button>
          </div>
          <div className="flex-1 overflow-y-auto p-2">
            <p className="flex items-center justify-between px-3 pt-1 pb-2 text-[10px] font-medium uppercase tracking-widest text-muted-foreground">
              <span>最近对话</span>
              {sessions.data && sessions.data.length > 0 && (
                <span className="font-mono text-muted-foreground/60">
                  {sessions.data.length}
                </span>
              )}
            </p>
            {sessions.isLoading ? (
              <div className="flex items-center justify-center px-4 py-8">
                <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" aria-hidden />
              </div>
            ) : sessions.isError ? (
              <div className="px-4 py-6 text-center">
                <AlertCircle
                  className="mx-auto mb-2 h-6 w-6 text-destructive/60"
                  aria-hidden
                />
                <p className="text-xs text-destructive">读取会话失败</p>
                <p className="mt-1 break-words text-[10px] text-muted-foreground/70">
                  {String(sessions.error ?? "未知错误")}
                </p>
              </div>
            ) : !sessions.data || sessions.data.length === 0 ? (
              <div className="px-4 py-6 text-center">
                <MessagesSquare
                  className="mx-auto mb-2 h-6 w-6 text-muted-foreground/40"
                  aria-hidden
                />
                <p className="text-xs text-muted-foreground">暂无对话</p>
                <p className="mt-1 text-[10px] text-muted-foreground/70">
                  点上方新建开始
                </p>
              </div>
            ) : (
              <ul className="space-y-0.5">
                {sessions.data.map((s) => (
                  <SessionItem
                    key={s.session_id}
                    session={s}
                    active={s.session_id === activeSessionId}
                    onClick={() => handleSelectSession(s.session_id)}
                  />
                ))}
              </ul>
            )}
          </div>
        </aside>

        {/* 右栏：对话 / 空状态 */}
        <section className="hb-card flex min-h-[500px] flex-col overflow-hidden p-0">
          {activeView === "empty" ? (
            <EmptyStudio onNewChat={handleNewChat} />
          ) : active ? (
            <ChatView
              sessionId={activeSessionId}
              messages={messages}
              streamingTurn={streamingTurn}
              isStreaming={currentTaskId !== null}
              streamingError={streamingError}
              onSend={handleSend}
              onCancel={handleCancel}
              onRestore={handleRestoreCheckpoint}
              input={studioInput}
              onInput={setStudioInput}
            />
          ) : null}
        </section>
      </div>
    </div>
  );
}

// =====================================================================
// EmptyStudio — 空状态引导（D1 骨架主视觉）
// =====================================================================

function EmptyStudio({ onNewChat }: { onNewChat: () => void }) {
  return (
    <div className="flex flex-1 items-center justify-center p-10">
      <div className="max-w-lg text-center">
        <div className="mb-5 inline-flex h-14 w-14 items-center justify-center rounded-full bg-primary-50 dark:bg-primary-900/20">
          <PenLine
            className="h-6 w-6 text-primary-600 dark:text-primary-400"
            aria-hidden
          />
        </div>
        <span className="font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
          Studio · 文章生成
        </span>
        <h2 className="mt-2 font-serif text-2xl font-semibold tracking-tight">
          开始一段新对话
        </h2>
        <p className="mx-auto mt-3 max-w-md text-sm leading-relaxed text-muted-foreground">
          空白对话 · 让 AI 按你的要求生成新文章
          <br />
          支持 Markdown · 多轮上下文 · 走 Claude CLI · 自动落盘 jsonl 持久化
        </p>

        <button
          type="button"
          onClick={onNewChat}
          className="mt-6 inline-flex items-center gap-2 rounded-md bg-primary-600 px-5 py-2.5 text-sm font-medium text-white transition-colors hover:bg-primary-700 active:bg-primary-800"
        >
          <Plus className="h-4 w-4" aria-hidden />
          新建对话
        </button>

        <div className="mt-8 grid grid-cols-1 gap-2 text-left sm:grid-cols-3">
          <FeatureChip
            icon={<Sparkles className="h-3.5 w-3.5" />}
            label="Claude CLI"
            desc="对话驱动 · 流式输出"
          />
          <FeatureChip
            icon={<FileText className="h-3.5 w-3.5" />}
            label="多轮上下文"
            desc="自动持久化 · 撤回检查点"
          />
          <FeatureChip
            icon={<PenLine className="h-3.5 w-3.5" />}
            label="文章产出"
            desc={`落到 C:\\HomeBase\\Wiki\\_raw\\ 草稿`}
          />
        </div>
      </div>
    </div>
  );
}

function FeatureChip({
  icon,
  label,
  desc,
}: {
  icon: React.ReactNode;
  label: string;
  desc: string;
}) {
  return (
    <div className="flex items-center gap-2.5 rounded-md border border-border bg-muted/20 px-3 py-2">
      <span className="text-primary-600 dark:text-primary-400">{icon}</span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-xs font-medium text-foreground">{label}</p>
        <p className="truncate text-[10px] text-muted-foreground">{desc}</p>
      </div>
    </div>
  );
}

// =====================================================================
// SessionItem — 左栏会话列表项（D1 占位 · D2 接真数据）
// =====================================================================

function SessionItem({
  session,
  active,
  onClick,
}: {
  session: StudioSessionMeta;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className={cn(
          "w-full rounded-md px-3 py-2 text-left transition-colors",
          active
            ? "bg-primary-50 text-primary-800 dark:bg-primary-900/30 dark:text-primary-200"
            : "text-muted-foreground hover:bg-secondary hover:text-foreground",
        )}
        title={session.session_id}
      >
        <div className="flex items-baseline justify-between gap-2">
          <p className="truncate text-sm font-medium">{session.title}</p>
          {session.article_path && (
            <span
              className="flex-none rounded-full bg-success/15 px-1 py-0.5 font-mono text-[8px] uppercase tracking-wider text-success-strong"
              aria-label="已生成文章"
              title="已生成 .md 文章"
            >
              md
            </span>
          )}
        </div>
        <p className="mt-0.5 flex items-baseline gap-2 font-mono text-[10px] text-muted-foreground">
          <span>{session.message_count} msg</span>
          <span aria-hidden>·</span>
          <span>{formatRelativeTime(session.updated_ms)}</span>
        </p>
      </button>
    </li>
  );
}

// =====================================================================
// ChatView — 对话视图（D2.2 主体）
// =====================================================================

interface ChatViewProps {
  input: string;
  onInput: (value: string) => void;
  sessionId: string | null;
  messages: ChatMessage[];
  streamingTurn: TurnState;
  isStreaming: boolean;
  streamingError: string | null;
  onSend: (msg: string) => void;
  onCancel: () => void;
  /** **E1**：撚回某条 user message · None 表示不启用 撚回 UI */
  onRestore?: (msg: ChatMessage) => void;
}

function ChatView({
  input,
  onInput,
  sessionId,
  messages,
  streamingTurn,
  isStreaming,
  streamingError,
  onSend,
  onCancel,
  onRestore,
}: ChatViewProps) {
  const active=useWorkflowVisibility();
  // 自动滚到底部（新消息或流式更新时）
  // turn.parts 变化时也触发· 以允许 partial assistant text + tool calls 动态增长时贴底
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (active && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [active, messages.length, streamingTurn.parts]);

  const isEmpty = messages.length === 0 && !isStreaming && !streamingError;

  return (
    <div className="flex h-full flex-col">
      {/* 顶部：会话信息条 */}
      <div className="flex items-center justify-between border-b border-border px-4 py-2.5">
        <div className="min-w-0 flex-1">
          <p className="truncate font-mono text-xs text-muted-foreground">
            {sessionId ?? <span className="italic">未命名 · 发送后自动生成 ID</span>}
          </p>
        </div>
        <span className="flex-none font-mono text-[10px] uppercase tracking-widest text-muted-foreground">
          {messages.length} msg
        </span>
      </div>

      {/* 消息区：可滚动 */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto px-6 py-6"
      >
        {isEmpty ? (
          <div className="flex h-full items-center justify-center">
            <div className="max-w-md text-center">
              <Bot className="mx-auto mb-3 h-8 w-8 text-muted-foreground/40" aria-hidden />
              <p className="text-sm text-muted-foreground">
                输入你的需求 · 按 <kbd className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px]">Enter</kbd> 发送
              </p>
              <p className="mt-1 font-mono text-[10px] text-muted-foreground/60">
                多轮上下文 · 自动持久化到 jsonl
              </p>
            </div>
          </div>
        ) : (
          <div className="mx-auto max-w-3xl space-y-5">
            {messages.map((msg, i) => (
              <MessageBubble
                key={i}
                message={msg}
                onRestore={onRestore && msg.role === "user" && msg.checkpoint_id ? () => onRestore(msg) : undefined}
              />
            ))}
            {/* 流式中的 assistant 消息 · 直接用 turn.parts 渲染（text + tool 穿插）*/}
            {isStreaming && (
              <MessageBubble
                message={{
                  ts: Date.now(),
                  role: "assistant",
                  // content 合并仅作后退（如果 parts 是纯 text 也能背包渲染）
                  content: streamingTurn.parts
                    .filter((p): p is Extract<TurnPart, { kind: "text" }> => p.kind === "text")
                    .map((p) => p.text)
                    .join("\n\n"),
                  model: "claude",
                  parts: streamingTurn.parts,
                  stats: streamingTurn.stats,
                }}
                isStreaming
              />
            )}
            {streamingError && (
              <div className="flex items-start gap-3 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive">
                <AlertTriangle className="mt-0.5 h-4 w-4 flex-none" aria-hidden />
                <p className="break-words">{streamingError}</p>
              </div>
            )}
          </div>
        )}
      </div>

      {/* 底部：输入框 */}
      <ChatInput
        value={input}
        onInput={onInput}
        isStreaming={isStreaming}
        onSend={onSend}
        onCancel={onCancel}
      />
    </div>
  );
}

// =====================================================================
// MessageBubble — 单条消息气泡
// =====================================================================

function MessageBubble({
  message,
  isStreaming,
  onRestore,
}: {
  message: ChatMessage;
  isStreaming?: boolean;
  /** **E1**：仅 user message 且有 checkpoint_id 时传入 · 点击后撚回该条及之后所有 AI 改动 */
  onRestore?: () => void;
}) {
  const isUser = message.role === "user";
  // **D2.7**：assistant 消息优先用 parts 渲染（text + tool 穿插按时序）· fallback content
  const hasParts = !isUser && message.parts && message.parts.length > 0;
  // 试从末尾 text part 判断“是否在出文字” · 决定在哪里放闪烁游标
  const lastTextPart = hasParts
    ? [...(message.parts ?? [])].reverse().find((p) => p.kind === "text")
    : null;

  return (
    <div className={cn("flex gap-3", isUser ? "flex-row-reverse" : "flex-row")}>
      <div
        className={cn(
          "flex h-7 w-7 flex-none items-center justify-center rounded-full",
          isUser
            ? "bg-primary-600 text-white"
            : "bg-muted text-muted-foreground",
        )}
      >
        {isUser ? (
          <User className="h-3.5 w-3.5" aria-hidden />
        ) : (
          <Bot className="h-3.5 w-3.5" aria-hidden />
        )}
      </div>
      <div className={cn("min-w-0 flex-1", isUser && "flex justify-end")}>
        {isUser ? (
          // USER 气泡 · 纯文本 · 右对齐 · **E1** · 可选撚回按钮
          <div className="flex items-start justify-end gap-1.5">
            {onRestore && (
              <button
                type="button"
                onClick={onRestore}
                title="撚回这条 · 退回发送前状态 + 删除该条及之后消息"
                className="hb-button-ghost hb-button-icon-sm mt-1 flex-none"
              >
                <RotateCcw className="h-3.5 w-3.5" aria-hidden />
              </button>
            )}
            <div className="inline-block max-w-full rounded-lg bg-primary-600 px-4 py-2.5 text-sm leading-relaxed text-white">
              <p className="whitespace-pre-wrap break-words">{message.content}</p>
            </div>
          </div>
        ) : hasParts ? (
          // 新版 · parts 按时序渲染 · text 用 Markdown · tool 用 ToolCard
          <div className="space-y-2">
            {message.parts!.map((part, idx) => {
              if (part.kind === "text") {
                const isLastText = part === lastTextPart;
                return (
                  <div
                    key={`t-${idx}`}
                    className="inline-block max-w-full rounded-lg border border-border bg-card px-4 py-2.5 text-sm leading-relaxed text-foreground"
                  >
                    {part.text.length > 0 ? (
                      <div className="prose prose-sm max-w-none dark:prose-invert prose-p:leading-relaxed prose-pre:my-2">
                        <MarkdownRenderer source={part.text} />
                      </div>
                    ) : (
                      <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                        <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
                        Claude 思考中…
                      </span>
                    )}
                    {isStreaming && isLastText && part.text.length > 0 && (
                      <span className="ml-1 inline-block h-3 w-[6px] animate-pulse bg-current align-middle opacity-60" />
                    )}
                  </div>
                );
              }
              // tool part
              return <ToolCard key={`tool-${part.tool_use_id}-${idx}`} part={part} />;
            })}
            {/* parts 为空但正在流式 · 显示 thinking */}
            {message.parts!.length === 0 && (
              <div className="inline-block rounded-lg border border-border bg-card px-4 py-2.5">
                <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                  <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
                  Claude 思考中…
                </span>
              </div>
            )}
          </div>
        ) : (
          // 序列 · 旧消息（仅 content 无 parts）· 保持向后兼容
          <div className="inline-block max-w-full rounded-lg border border-border bg-card px-4 py-2.5 text-sm leading-relaxed text-foreground">
            {message.content.length > 0 ? (
              <div className="prose prose-sm max-w-none dark:prose-invert prose-p:leading-relaxed prose-pre:my-2">
                <MarkdownRenderer source={message.content} />
              </div>
            ) : (
              <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
                <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
                Claude 思考中…
              </span>
            )}
            {isStreaming && message.content.length > 0 && (
              <span className="ml-1 inline-block h-3 w-[6px] animate-pulse bg-current align-middle opacity-60" />
            )}
          </div>
        )}
        {/* 模型名 + token + 统计 · assistant 独有 */}
        {!isUser && (message.model || message.stats) && (
          <p className="mt-1 flex items-center gap-2 font-mono text-[9px] uppercase tracking-widest text-muted-foreground/60">
            {message.model && <span>{message.model}</span>}
            {message.tokens != null && <span>· {message.tokens} tok</span>}
            {message.stats && (
              <>
                <span aria-hidden>·</span>
                <span>{(message.stats.duration_ms / 1000).toFixed(1)}s</span>
                <span aria-hidden>·</span>
                <span>{message.stats.num_turns} turns</span>
                {message.stats.total_cost_usd > 0 && (
                  <>
                    <span aria-hidden>·</span>
                    <span>${message.stats.total_cost_usd.toFixed(4)}</span>
                  </>
                )}
              </>
            )}
          </p>
        )}
      </div>
    </div>
  );
}

// =====================================================================
// ToolCard — 工具调用卡片（D2.7）
// · 未完成：转圈动效 + 无结果
// · 完成：对勾 / 财 X · 可点开看预览
// =====================================================================

/** **E2.6**：生图阶段 → 进度百分比 + 文案（前端推断 · 不依赖后端给百分比）*/
const WAITING_HINTS = [
  "🎨 模型预热...",
  "🖌️ 第一层轮廓",
  "🌈 上色构图",
  "✨ 细节优化",
  "🔍 渲染纹理",
  "💎 光影调整",
  "🎯 最后润色",
];

/** OR slug → 友好显示名 */
function modelLabel(slug: string): string {
  if (slug.includes("gpt-5.4-image-2") || slug.includes("gpt-image-2"))
    return "GPT Image 2";
  if (slug.includes("gemini-3-pro-image")) return "Nano Banana Pro";
  if (slug.includes("gemini-3.1-flash-image")) return "Nano Banana 2";
  if (slug.includes("flux.2-pro")) return "FLUX.2 Pro";
  if (slug.includes("flux")) return "FLUX";
  return slug.split("/").pop() ?? slug;
}

function computeImageProgress(p: ImageProgress | null): {
  pct: number | null;
  label: string;
  modelLabel: string;
} {
  if (!p) {
    return { pct: null, label: "进度未知：暂未检测到进度记录", modelLabel: "" };
  }
  const ml = modelLabel(p.model);
  if (p.stage === "unknown") return { pct: null, label: p.error ?? "进度未知：尚未取得可靠状态", modelLabel: ml };
  if (p.stage === "error") {
    return { pct: 100, label: `✗ ${p.error ?? "失败"}`, modelLabel: ml };
  }
  if (p.stage === "done") {
    return { pct: 100, label: "✓ 完成", modelLabel: ml };
  }
  if (p.stage === "preparing") {
    return { pct: 4, label: "📡 准备 OpenRouter API 请求...", modelLabel: ml };
  }
  if (p.stage === "sending") {
    return { pct: 11, label: `📤 提交 prompt 到 ${ml}`, modelLabel: ml };
  }
  if (p.stage === "waiting") {
    // waiting 是大头 · 平均 30s · max 90s
    // 假设头 4s 是 preparing+sending · 之后是 waiting
    const waitMs = Math.max(0, p.elapsed_ms - 4000);
    const waitFraction = Math.min(1, waitMs / 30000);
    const pct = 15 + waitFraction * 70; // 15-85%
    // 文案每 4s 切一次 · 循环
    const idx = Math.floor(waitMs / 4000) % WAITING_HINTS.length;
    return { pct, label: WAITING_HINTS[idx], modelLabel: ml };
  }
  if (p.stage === "decoding") {
    return { pct: 88, label: "🌈 解码图像数据...", modelLabel: ml };
  }
  if (p.stage === "saving") {
    return { pct: 95, label: "📦 保存到 attachments/", modelLabel: ml };
  }
  return { pct: null, label: "进度未知：无法识别当前阶段", modelLabel: ml };
}

function ToolCard({
  part,
}: {
  part: Extract<TurnPart, { kind: "tool" }>;
}) {
  const active=useWorkflowVisibility();
  const [open, setOpen] = useState(false);
  // **E2**：识别 MCP 生图工具 · `mcp__homebase-image__generate_image`
  const isImageGen = part.name === "mcp__homebase-image__generate_image";
  // 显示用的简短工具名
  const displayName = isImageGen ? "🎨 生图" : part.name;
  // 工具名 → 图标
  const Icon = isImageGen
    ? ImageIcon
    : part.name === "Read"
      ? FileText
      : part.name === "Write"
        ? PenLine
        : part.name === "Edit"
          ? Edit3
          : part.name === "Glob"
            ? FileSearch
            : part.name === "Grep"
              ? Search
              : Hammer;
  const canExpand = part.finished && part.result_preview && part.result_preview.length > 0;

  // **E2**：图片完成时 · 从 result_preview 中提取 Path · 异步读 base64 嵌入
  const imagePath = isImageGen && part.finished && !part.is_error
    ? part.result_preview?.match(/Path:\s*([^\r\n]+)/)?.[1]?.trim() : undefined;
  const image = useQuery({
    queryKey: ["studio", "attachment", imagePath],
    queryFn: () => studioApi.readAttachmentB64(imagePath!),
    enabled: active && Boolean(imagePath),
    staleTime: Infinity,
    gcTime: 60_000,
  });
  const imageDataUrl = image.data;
  const imageError = image.error ? String(image.error) : null;

  // **E2.6**：生图未完成时 · 1Hz polling 拿阶段进度
  // 简化版：拿 list[0]（最新启动的进度）· 单生图场景准确（多并发可能错配 · 后续优化）
  const progressQuery = useQuery({
    queryKey: ["studio", "image-progress"],
    queryFn: studioApi.listImageProgress,
    enabled: active && isImageGen && !part.finished,
    staleTime: 1_000,
    refetchInterval: active && isImageGen && !part.finished ? 1_000 : false,
  });
  const progress: ImageProgress | null = progressQuery.isError
    ? { pid: 0, started_at_ms: Date.now(), updated_at_ms: Date.now(), elapsed_ms: 0, model: "", prompt_preview: "", output_path: null, stage: "unknown", error: "进度查询失败，实际任务状态未知，请重试或核对输出" }
    : progressQuery.data?.[0] ?? null;

  return (
    <div
      className={cn(
        "rounded-md border bg-muted/30 px-3 py-2 transition-colors",
        part.is_error
          ? "border-destructive/40 bg-destructive/5"
          : "border-border",
      )}
    >
      <button
        type="button"
        onClick={() => canExpand && setOpen((v) => !v)}
        disabled={!canExpand}
        className={cn(
          "flex w-full items-center gap-2 text-left text-xs",
          canExpand && "cursor-pointer",
        )}
      >
        <Icon
          className={cn(
            "h-3.5 w-3.5 flex-none",
            part.is_error
              ? "text-destructive"
              : part.finished
                ? "text-muted-foreground"
                : "text-primary-600 dark:text-primary-400",
          )}
          aria-hidden
        />
        <span className="font-mono font-medium text-foreground">{displayName}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-muted-foreground">
          {part.input_summary}
        </span>
        {!part.finished ? (
          <Loader2 className="h-3 w-3 flex-none animate-spin text-muted-foreground" aria-hidden />
        ) : part.is_error ? (
          <X className="h-3 w-3 flex-none text-destructive" aria-hidden />
        ) : (
          <Check className="h-3 w-3 flex-none text-success-strong" aria-hidden />
        )}
        {canExpand && (
          <ChevronRight
            className={cn(
              "h-3 w-3 flex-none text-muted-foreground/60 transition-transform",
              open && "rotate-90",
            )}
            aria-hidden
          />
        )}
      </button>
      {/* **E2.6**：生图未完成 · 阶段进度条 + 文案 + 计时 */}
      {isImageGen && !part.finished && (
        <ImageGenProgressBar progress={progress} />
      )}
      {/* **E2**：生图完成后 · 直接展示图片预览（不靠 expand）*/}
      {isImageGen && part.finished && !part.is_error && (
        <div className="mt-2">
          {imageDataUrl ? (
            <img
              src={imageDataUrl}
              alt="generated"
              className="block max-h-72 rounded border border-border object-contain"
            />
          ) : imageError ? (
            <div className="text-xs text-destructive">加载失败: {imageError}</div>
          ) : (
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <Loader2 className="h-3 w-3 animate-spin" aria-hidden />
              加载图片中…
            </div>
          )}
        </div>
      )}
      {open && canExpand && (
        <pre className="mt-2 max-h-48 overflow-y-auto rounded bg-muted/40 p-2 font-mono text-[10px] leading-relaxed text-muted-foreground">
          {part.result_preview}
        </pre>
      )}
    </div>
  );
}

// =====================================================================
// ImageGenProgressBar — 生图阶段进度条 + 文案 + 计时（E2.6）
// =====================================================================

export function ImageGenProgressBar({ progress }: { progress: ImageProgress | null }) {
  const active=useWorkflowVisibility();
  // 仅文案展示用：每 1s tick 一次 · 让"已 23s"实时跳动 · 不依赖 polling 频率
  const tickMs = useClock(1_000, active && progress?.stage !== "done" && progress?.stage !== "error");

  const { pct, label, modelLabel: ml } = computeImageProgress(progress);

  // elapsed 优先用 Date.now() - started_at_ms（前端实时算 · 即使 polling 慢也跳）·
  // 没有 progress 就显示 0
  const elapsedSec = progress
    ? Math.max(0, Math.floor((tickMs - progress.started_at_ms) / 1000))
    : 0;

  return (
    <div className="mt-2 space-y-1.5">
      <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
        <span className="truncate font-mono">{label}</span>
        <span className="flex-none font-mono text-[10px]">
          {ml && <span className="mr-2 text-primary-700 dark:text-primary-300">{ml}</span>}
          {elapsedSec}s
        </span>
      </div>
      {pct !== null && <div role="progressbar" aria-valuenow={Math.round(pct)} aria-valuemin={0} aria-valuemax={100} className="relative h-1.5 overflow-hidden rounded-full bg-muted/50">
        <div
          className="absolute inset-y-0 left-0 rounded-full bg-primary-500 transition-[width] duration-700 ease-out"
          style={{ width: `${Math.min(100, Math.max(0, pct))}%` }}
        />
      </div>}
    </div>
  );
}

// =====================================================================
// ChatInput — 底部输入框（textarea + 发送 / 取消按钮）
// =====================================================================

function ChatInput({
  value,
  onInput,
  isStreaming,
  onSend,
  onCancel,
}: {
  value: string;
  onInput: (value: string) => void;
  isStreaming: boolean;
  onSend: (msg: string) => void;
  onCancel: () => void;
}) {
  const active=useWorkflowVisibility();
  const taRef = useRef<HTMLTextAreaElement>(null);
  const composing = useRef(false);

  // 自动调整 textarea 高度
  useEffect(() => {
    if (active && taRef.current) {
      taRef.current.style.height = "auto";
      const h = Math.min(taRef.current.scrollHeight, 200);
      taRef.current.style.height = `${h}px`;
    }
  }, [active, value]);

  const handleSubmit = () => {
    const trimmed = value.trim();
    if (!trimmed || isStreaming) return;
    onSend(trimmed);
    onInput("");
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter 发送 · Shift+Enter 换行
    if (
      e.key === "Enter" &&
      !e.shiftKey &&
      !e.nativeEvent.isComposing &&
      !composing.current &&
      e.keyCode !== 229
    ) {
      e.preventDefault();
      handleSubmit();
    }
  };

  return (
    <div className="border-t border-border bg-card/50 p-3">
      <div className="mx-auto flex max-w-3xl items-end gap-2">
        <textarea
          ref={taRef}
          value={value}
          onChange={(e) => onInput(e.target.value)}
          onKeyDown={handleKeyDown}
          onCompositionStart={() => {
            composing.current = true;
          }}
          onCompositionEnd={() => {
            composing.current = false;
          }}
          onBlur={() => {
            composing.current = false;
          }}
          rows={1}
          placeholder={
            isStreaming
              ? "Claude 流式中 · 等当前完成或点取消"
              : "输入需求 · Enter 发送 · Shift+Enter 换行"
          }
          disabled={isStreaming}
          className={cn(
            "min-h-[40px] flex-1 resize-none rounded-md border border-border bg-background px-3 py-2 text-sm leading-relaxed",
            "placeholder:text-muted-foreground/60",
            "focus:border-primary-400 focus:outline-none focus:ring-2 focus:ring-primary-200 dark:focus:ring-primary-800",
            "disabled:cursor-not-allowed disabled:opacity-60",
          )}
        />
        {isStreaming ? (
          <button
            type="button"
            onClick={onCancel}
            className="inline-flex h-10 w-10 flex-none items-center justify-center rounded-md border border-border text-destructive transition-colors hover:bg-destructive/10"
            title="取消当前任务（kill claude 进程）"
          >
            <Square className="h-4 w-4" aria-hidden fill="currentColor" />
          </button>
        ) : (
          <button
            type="button"
            onClick={handleSubmit}
            disabled={!value.trim()}
            className="inline-flex h-10 w-10 flex-none items-center justify-center rounded-md bg-primary-600 text-white transition-colors hover:bg-primary-700 active:bg-primary-800 disabled:cursor-not-allowed disabled:opacity-50"
            title="发送（Enter）"
          >
            <Send className="h-4 w-4" aria-hidden />
          </button>
        )}
      </div>
    </div>
  );
}
