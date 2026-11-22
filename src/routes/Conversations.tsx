import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { useLocation } from "react-router-dom";
import { Plus, Search, RefreshCw } from "lucide-react";
import { assistantApi } from "@/lib/assistant";
import { savedDraft, useDraftState } from "@/lib/assistant-drafts";
import { invalidateSessionCatalog, useSessionCatalog } from "@/lib/assistant-sessions";
import { sessionWindow } from "@/lib/assistant-window";
import { agentsApi } from "@/lib/tauri-api";
import {
  conversationsApi,
  handoffText,
  nativeItems,
} from "@/lib/conversations";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { NativeConversation } from "@/components/native-conversation";
import { nativeMessageText } from "@/lib/native-message-view";
import { invalidateConversationProvider, uniqueConversationRows, useConversationIndex } from "@/lib/conversation-index";
import { inputBytes, MAX_MESSAGE_BYTES } from "@/lib/chat-input-limits";
import { HandoffStatus, useAssistantHandoff } from "@/components/workflow-assistant-entry";
import { AssistantArchiveBackup } from "@/components/assistant-archive-backup";
import { Assistant } from "./Assistant";

type Selection = { provider: string; id: string; title: string; source?: string };
export function Conversations() {
  const cache = useQueryClient();
  const location = useLocation();
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [appliedSearch, setAppliedSearch] = useState("");
  const [composingSearch, setComposingSearch] = useState(false);
  const [visibleOffset, setVisibleOffset] = useState(0);
  const searchPending = search.trim().toLowerCase() !== appliedSearch.trim().toLowerCase();
  useEffect(()=>{if(composingSearch)return;const timer=setTimeout(()=>setAppliedSearch(search),250);return()=>clearTimeout(timer);},[search,composingSearch]);
  const previous = cache.getQueryData<{
    selected: Selection;
    interactive: boolean;
  }>(["conversations", "selection"]);
  const target = location.state?.openConversation as Selection | undefined;
  const requested =
    target &&
    ["codex", "claude-code", "gemini-cli"].includes(target.provider) &&
    typeof target.id === "string"
      ? target
      : undefined;
  const [selected, setSelected] = useState<Selection | undefined>(
    requested ?? previous?.selected,
  );
  const [interactive, setInteractive] = useState(
    requested !== undefined || (previous?.interactive ?? false),
  );
  useEffect(() => {
    if (requested) {
      setSelected(requested);
      setInteractive(true);
    }
  }, [location.key, requested]);
  const [provider, setProvider] = useState("homebase");
  const [credentialRef, setCredentialRef] = useState("");
  const credentialChoices = useQuery({
    queryKey: ["assistant", "local"],
    queryFn: () =>
      invoke<{ credential_references?: { id: string; name: string }[] }>(
        "assistant_local_config_get",
      ),
    enabled: provider === "claude-code" || provider === "gemini-cli",
    staleTime: 60_000,
  });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [handoff, setHandoff] = useState(false);
  const [goal, setGoal] = useState("");
  const [carry, setCarry] = useState(true);
  const [draftSourceChoice, setDraftSourceChoice] = useState<string | null>(null);
  const createRequest = useRef<string | null>(
    cache.getQueryData<string>([
      "conversations",
      "pending",
      "create",
      "homebase",
      "",
    ]) ?? null,
  );
  useEffect(() => {
    if (selected)
      cache.setQueryData(["conversations", "selection"], {
        selected,
        interactive,
      });
  }, [cache, selected, interactive]);
  const homebaseEnabled = ['all','homebase','wechat'].includes(filter);
  const catalogSearch = appliedSearch.trim() === '微信' ? 'wechat' : appliedSearch.trim() === '桌面' ? 'desktop' : appliedSearch;
  const sessions = useSessionCatalog({ query:catalogSearch,source:filter==='wechat'?'wechat':undefined,enabled:homebaseEnabled,browse:true });
  const index = useConversationIndex(filter);
  const providers = useQuery({
    queryKey: ["conversations", "providers"],
    queryFn: conversationsApi.providers,
    staleTime: 60_000,
  });
  const viewingOriginal = !!selected && selected.provider !== "homebase" && !interactive;
  const detail = useQuery({
    queryKey: ["agents", "history", "read", selected?.provider, selected?.id],
    queryFn: () => agentsApi.historyRead(selected!.provider, selected!.id),
    enabled: viewingOriginal,
    retryOnMount: false,
    staleTime: 60_000,
  });
  const ownDetail = useQuery({
    queryKey: ["assistant", "session", selected?.id, "handoff-window"],
    queryFn: () => sessionWindow(selected!.id, null, 6),
    enabled: handoff && carry && selected?.provider === "homebase",
    staleTime: 60_000,
  });
  const nativeHandoff = useQuery({
    queryKey: ["conversations", selected?.provider, selected?.id],
    queryFn: () => conversationsApi.read(selected!.provider, selected!.id),
    enabled:
      handoff && carry && interactive && selected?.provider !== "homebase",
    staleTime: Infinity,
  });
  const rows = useMemo(
    () =>
      uniqueConversationRows([
        ...(searchPending ? [] : sessions.data ?? []).map((s) => ({
          provider: "homebase",
          id: s.id,
          title: s.title,
          source: s.source === "wechat" ? "微信" : s.source === "desktop" ? "桌面" : s.source || "来源未知",
          updated: s.updated_at,
          active: !!s.active_turn_id,
        })),
        ...index.history.map((s) => ({
          provider: s.agent_id,
          id: s.session_id,
          title: s.first_user_prompt ?? "未命名会话",
          source: s.agent_id,
          updated: s.last_active_at,
          active: false,
        })),
        ...index.managed,
      ])
        .filter(
          (s) =>
            (filter === "all" ||
              s.provider === filter ||
              (filter === "wechat" && s.source === "微信")) &&
            (s.provider === 'homebase' || `${s.title} ${s.source}`
              .toLowerCase()
              .includes(search.trim().toLowerCase())),
        )
        .sort((a, b) => (b.updated ?? "").localeCompare(a.updated ?? "")),
    [sessions.data, index.history, index.managed, filter, search, searchPending],
  );
  const listOffset = Math.min(visibleOffset,Math.max(0,Math.floor((rows.length-1)/100)*100));
  const selectedRow = rows.find(
    (row) => row.provider === selected?.provider && row.id === selected?.id,
  );
  const selectedTitle =
    selectedRow?.title ??
    selected?.title ??
    "原生会话";
  const desktopSelection = selected?.provider === "homebase" &&
    (selected.source ?? selectedRow?.source) === "桌面";
  const nativeSelection = interactive && selected &&
    ["codex", "claude-code", "gemini-cli"].includes(selected.provider);
  const sourceDraftScope = desktopSelection ? selected.id :
    nativeSelection ? `native:${selected.provider}:${selected.id}` : null;
  const carrySavedDraft = sourceDraftScope !== null && draftSourceChoice === sourceDraftScope;
  const localDraft = useDraftState(sourceDraftScope);
  const handoffDraft = useQuery({
    queryKey: ["conversations", "handoff-saved-draft", sourceDraftScope],
    enabled: handoff && carrySavedDraft,
    staleTime: 0,
    retry: false,
    refetchOnWindowFocus: false,
    queryFn: async () => {
      if (!sourceDraftScope) throw Error("请选择拥有本机草稿的原会话。");
      const entry = await savedDraft(sourceDraftScope);
      if (entry.pending || entry.native_pending)
        throw Error("原草稿的发送回执尚未核对，暂不能携带。请先核对原请求。");
      const { body } = entry;
      if (!Number.isSafeInteger(entry.revision) || entry.revision < 0 ||
          typeof body?.text !== "string" || !Array.isArray(body.files) ||
          body.files.some((file) => typeof file?.name !== "string" || typeof file?.content !== "string"))
        throw Error("已保存草稿的格式无效，未采用这份内容。");
      const attachments = body.files.filter((file) => !file.document).map(({ name, content }) => ({ name, content }));
      return {
        revision: entry.revision,
        input: { text: body.text, attachments },
        omittedPdf: body.files.length - attachments.length,
        omittedImage: !!body.image,
      };
    },
  });
  const draftBlocked = !!(localDraft.pending || localDraft.nativePending || localDraft.unconfirmed);
  const savedSource = handoffDraft.data;
  const draftVersionChanged = !!savedSource && localDraft.revision > savedSource.revision;
  const canCarrySavedDraft = !!savedSource && !handoffDraft.isFetching &&
    !handoffDraft.error && !draftBlocked && !draftVersionChanged;
  async function create() {
    setBusy(true);
    setError("");
    try {
      if (provider === "homebase") {
        const session = await assistantApi.create();
        setSelected({ provider, id: session.id, title: session.title, source: "桌面" });
        setInteractive(false);
        await invalidateSessionCatalog(cache);
      } else {
        createRequest.current ??= crypto.randomUUID();
        cache.setQueryData(
          ["conversations", "pending", "create", provider, credentialRef],
          createRequest.current,
        );
        const r = await conversationsApi.native("create", {
          request_id: createRequest.current,
          provider,
          ...(credentialRef ? { credential_ref: credentialRef } : {}),
        });
        if (!r.thread?.id)
          throw new Error(
            "原生创建结果未知，保留原请求编号；请先核对原生会话，不会自动重建",
          );
        if (r.thread.provider !== undefined && r.thread.provider !== provider)
          throw Error("原生会话标识不匹配，未采用其他来源的内容。");
        createRequest.current = null;
        cache.removeQueries({
          queryKey: [
            "conversations",
            "pending",
            "create",
            provider,
            credentialRef,
          ],
          exact: true,
        });
        if (Array.isArray(r.thread.turns) && r.thread.turns.length === 0)
          cache.setQueryData(["conversations", provider, r.thread.id], r);
        setSelected({
          provider,
          id: r.thread.id,
          title: `新 ${providers.data?.providers.find((p) => p.id === provider)?.name ?? provider} 会话`,
        });
        setInteractive(true);
        void invalidateConversationProvider(cache, provider);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }
  const messages =
    selected?.provider === "homebase"
      ? ownDetail.data?.messages
      : interactive
        ? nativeHandoff.data?.thread &&
          nativeItems(nativeHandoff.data.thread)
            .filter((i) => ["userMessage", "agentMessage"].includes(i.type))
            .map((i) => ({
              role: i.type === "userMessage" ? "user" : "assistant",
              content: nativeMessageText(i),
            }))
        : detail.data?.messages;
  const handoffReadError =
    selected?.provider === "homebase"
      ? ownDetail.error
      : interactive
        ? nativeHandoff.error
        : detail.error;
  const readingExcerpts = selected?.provider === "homebase" ? ownDetail.isFetching
    : interactive ? nativeHandoff.isFetching : detail.isFetching;
  const canCarry = !!messages && !handoffReadError && !readingExcerpts;
  const transfer = useAssistantHandoff(() => {
      if (!goal.trim() || (carry && !canCarry) || (carrySavedDraft && !canCarrySavedDraft))
        throw Error("请先填写目标并核对需要携带的内容。");
      const draft = carry || carrySavedDraft
        ? handoffText(selectedTitle, carry ? messages ?? [] : [], goal, carrySavedDraft ? savedSource?.input : undefined)
        : goal.trim();
      if (inputBytes(draft) > MAX_MESSAGE_BYTES)
        throw Error(
          "新目标超过8000字节，请缩短后交接；未创建新会话或截断目标。",
        );
      return draft;
  }, `${selected?.provider ?? ""}:${selected?.id ?? ""}:${carry}:${carrySavedDraft}`);
  return (
    <div className="flex min-h-[650px] flex-col gap-5">
      <header className="hb-page-header flex flex-wrap items-end justify-between gap-3">
        <div>
          <span className="hb-section-label">Conversations</span>
          <h1>会话</h1>
          <p>在同一个地方继续工作。原生会话各自保留，换助理时明确交接。</p>
        </div>
        <AssistantArchiveBackup />
      </header>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      )}
      <div className="grid min-h-[420px] flex-1 gap-5 lg:h-[calc(100dvh-250px)] lg:flex-none lg:grid-cols-[280px_minmax(0,1fr)]">
        <aside className="flex min-w-0 flex-col rounded-xl border border-border bg-card p-3">
          <div className="flex gap-2">
            <select
              aria-label="新会话使用的 Agent"
              className="min-w-0 flex-1 rounded-lg border border-border bg-background p-2 text-sm"
              value={provider}
              onChange={(e) => {
                setProvider(e.target.value);
                setCredentialRef("");
                createRequest.current =
                  cache.getQueryData<string>([
                    "conversations",
                    "pending",
                    "create",
                    e.target.value,
                    "",
                  ]) ?? null;
              }}
            >
              {(
                providers.data?.providers ?? [
                  { id: "homebase", name: "HomeBase 助理", interactive: true },
                ]
              ).map((p) => (
                <option key={p.id} value={p.id} disabled={!p.interactive}>
                  {p.name}
                  {!p.interactive ? " · 只读历史" : ""}
                </option>
              ))}
            </select>
            <button
              className="hb-btn-primary"
              aria-label="新建会话"
              disabled={busy}
              onClick={() => void create()}
            >
              <Plus size={16} />
            </button>
          </div>
          {(provider === "claude-code" || provider === "gemini-cli") && (
            <label className="mt-3 block text-xs text-muted-foreground">
              原生账号 / 凭据
              <select
                aria-label="原生 Agent 凭据引用"
                className="mt-1 w-full rounded-lg border border-border bg-background p-2 text-sm"
                value={credentialRef}
                disabled={busy}
                onChange={(e) => {
                  setCredentialRef(e.target.value);
                  createRequest.current =
                    cache.getQueryData<string>([
                      "conversations",
                      "pending",
                      "create",
                      provider,
                      e.target.value,
                    ]) ?? null;
                }}
              >
                <option value="">
                  {provider === "claude-code"
                    ? "沿用原生账号 / 当前环境"
                    : "当前进程环境（独立运行目录）"}
                </option>
                {credentialChoices.data?.credential_references?.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
              <span className="mt-1 block">
                仅保存已有 Key 的引用；选择与厂商匹配的 Key。
              </span>
            </label>
          )}
          {providers.isError && (
            <p role="alert" className="mt-2 text-xs">
              Agent 状态读取失败。
              <button className="hb-btn-ghost" onClick={()=>void providers.refetch({cancelRefetch:false})}>重试 Agent 状态</button>
            </p>
          )}
          <div className="mt-3 flex gap-2">
            <select
              aria-label="按来源筛选会话"
              className="min-w-0 flex-1 rounded-lg border border-border bg-background p-2 text-sm"
              value={filter}
              onChange={(e) => {setFilter(e.target.value);setVisibleOffset(0);}}
            >
              <option value="all">全部会话</option>
              <option value="homebase">HomeBase</option>
              <option value="wechat">微信</option>
              <option value="codex">Codex</option>
              <option value="claude-code">Claude Code</option>
              <option value="gemini-cli">Gemini CLI</option>
            </select>
            <button
              className="hb-btn-ghost"
              aria-label="刷新会话列表"
              onClick={() => {
                if(homebaseEnabled) void sessions.refetch();
                setVisibleOffset(0);
                void index.refresh();
              }}
            >
              <RefreshCw size={14} />
            </button>
          </div>
          <label className="mt-3 flex items-center gap-2 rounded-lg border border-border px-2">
            <Search size={14} />
            <input
              aria-label="搜索会话"
              value={search}
              onChange={(e) => {setSearch(e.target.value);setVisibleOffset(0);}}
              onCompositionStart={()=>setComposingSearch(true)}
              onCompositionEnd={()=>setComposingSearch(false)}
              className="min-w-0 bg-transparent py-2 text-sm outline-none"
              placeholder="找一件聊过的事…"
            />
          </label>
          {((homebaseEnabled && (sessions.isPending || searchPending)) || index.loading) && (
            <p role="status" className="p-3 text-sm">
              正在读取会话…
            </p>
          )}
          {((homebaseEnabled && sessions.error) || index.errors.length>0) && (
            <p role="alert" className="p-3 text-xs">
              部分会话读取失败：
              {[homebaseEnabled ? sessions.error : null,...index.errors].filter(Boolean).map(String).join("；")}
            </p>
          )}
          {index.warnings.length>0 && <p role="status" className="p-3 text-xs text-muted-foreground">部分来源未完整读取：{index.warnings.join("；")}</p>}
          <div className="mt-3 max-h-[65vh] space-y-1 overflow-auto">
            {rows.slice(listOffset,listOffset+100).map((r) => (
              <button
                key={`${r.provider}:${r.id}`}
                className={`w-full rounded-lg p-3 text-left ${selected?.provider === r.provider && selected.id === r.id ? "bg-secondary" : "hover:bg-secondary/60"}`}
                onClick={() => {
                  setSelected(r);
                  setInteractive(
                    index.managed.some(
                      (s) => s.provider === r.provider && s.id === r.id,
                    ),
                  );
                  setHandoff(false);
                  setGoal("");
                  setDraftSourceChoice(null);
                }}
              >
                <span className="block truncate text-sm">{r.title}</span>
                <span className="mt-1 block text-xs text-muted-foreground">
                  {r.source} · {r.active ? "进行中" : "已保存"}
                </span>
              </button>
            ))}
            {!rows.length && !(homebaseEnabled&&(sessions.isPending||searchPending||sessions.error)) && !index.loading && !index.errors.length && !index.warnings.length && (
              <p className="p-3 text-sm text-muted-foreground">
                还没有符合条件的会话。
              </p>
            )}
            {rows.length > 100 && (
              <div className="flex items-center justify-between gap-2 p-2 text-xs">
                <button className="hb-btn-ghost" disabled={listOffset===0} onClick={()=>setVisibleOffset(Math.max(0,listOffset-100))}>上一组会话</button>
                <span>已读取 {rows.length} 条</span>
                <button className="hb-btn-ghost" disabled={listOffset+100>=rows.length} onClick={()=>setVisibleOffset(listOffset+100)}>下一组会话</button>
              </div>
            )}
            {homebaseEnabled && sessions.updatesAvailable && <button className="hb-btn-ghost w-full text-xs" onClick={()=>{setVisibleOffset(0);void sessions.refetch();}}>列表有更新，刷新查看</button>}
            {homebaseEnabled && sessions.moreError && <p role="alert" className="p-3 text-xs">更早会话未读取：{sessions.moreError} 已读列表和当前输入保留，请刷新会话列表。</p>}
            {homebaseEnabled && sessions.hasMore && <button className="hb-btn-ghost w-full" disabled={sessions.loadingMore||searchPending} onClick={()=>void sessions.loadMore()}>{sessions.loadingMore?'正在读取更早会话…':'加载更早的 HomeBase 会话'}</button>}
            {homebaseEnabled && sessions.total!==undefined && <p className="px-3 py-2 text-xs text-muted-foreground">HomeBase 已读取 {sessions.data?.length??0} / {sessions.total} 条匹配会话{sessions.hasMore?'，还有更早记录':''}。</p>}
          </div>
        </aside>
        <div className="flex min-h-0 min-w-0 flex-col">
          {!selected ? (
            <div className="flex h-full min-h-[500px] items-center justify-center rounded-xl border border-dashed border-border p-8 text-muted-foreground">
              选一件聊过的事，或开始新对话。
            </div>
          ) : (
            <>
              <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                <p
                  className="max-w-lg truncate text-sm font-medium"
                  title={selectedTitle}
                >
                  {selectedTitle}
                </p>
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  {viewingOriginal && (
                    <button
                      type="button"
                      className="hb-btn-ghost"
                      aria-label="刷新原对话"
                      title="刷新原对话"
                      disabled={detail.isFetching}
                      onClick={() => void detail.refetch({ cancelRefetch: false })}
                    >
                      <RefreshCw size={14} aria-hidden="true" />
                    </button>
                  )}
                  {["codex", "claude-code", "gemini-cli"].includes(
                    selected.provider,
                  ) &&
                    !interactive && (
                      <button
                        className="text-sm text-primary"
                        onClick={() => setInteractive(true)}
                      >
                        在这里继续{" "}
                        {providers.data?.providers.find(
                          (p) => p.id === selected.provider,
                        )?.name ?? selected.provider}
                      </button>
                    )}
                  <button
                    className="text-sm text-muted-foreground underline"
                    onClick={() => {
                      setHandoff((v) => !v);
                      setDraftSourceChoice(null);
                    }}
                  >
                    交接给 HomeBase
                  </button>
                </div>
              </div>
              {handoff && (
                <section
                  className="mb-4 space-y-3 rounded-xl border border-border bg-card p-4"
                  aria-label="交接会话"
                >
                  <h2 className="text-sm font-medium">希望接下来完成什么？</h2>
                  <textarea
                    aria-label="交接目标"
                    value={goal}
                    onChange={(e) => setGoal(e.target.value)}
                    className="w-full rounded-lg border border-border bg-background p-3 text-sm"
                  />
                  <label className="flex items-center gap-2 text-sm">
                    <input
                      type="checkbox"
                      checked={carry}
                      onChange={(e) => setCarry(e.target.checked)}
                    />
                    携带最近对话摘录（最多 8 条，可在发送前编辑）
                  </label>
                  {sourceDraftScope && (
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={carrySavedDraft}
                        onChange={(e) => setDraftSourceChoice(e.target.checked ? sourceDraftScope : null)}
                      />
                      携带已保存的未发草稿和普通文本附件
                    </label>
                  )}
                  {carrySavedDraft && handoffDraft.isFetching && <p role="status" className="text-xs">正在读取原会话已保存的草稿…</p>}
                  {carrySavedDraft && canCarrySavedDraft && savedSource && (
                    <p className="text-xs text-muted-foreground">
                      已读取第 {savedSource.revision} 版草稿。{savedSource.omittedPdf > 0 && (
                        <span>{savedSource.omittedPdf} 个 PDF 引用留在原会话。</span>
                      )}
                      {savedSource.omittedImage && <span>PNG 图片留在原会话。</span>}
                    </p>
                  )}
                  {carrySavedDraft && (handoffDraft.error || draftBlocked || draftVersionChanged) && (
                    <div className="flex flex-wrap items-center gap-2 text-xs">
                      <p role="alert">{draftBlocked ? "原请求尚待核对，暂不能携带草稿。"
                        : draftVersionChanged ? "原草稿已有新的保存版本，请重新读取。" : String(handoffDraft.error)}</p>
                      <button type="button" className="text-primary underline" disabled={handoffDraft.isFetching || draftBlocked}
                        onClick={() => void handoffDraft.refetch({ cancelRefetch: false })}>重读已保存草稿</button>
                    </div>
                  )}
                  <p className="text-xs text-muted-foreground">
                    选中的内容会作为引用文字写入新草稿。原会话和原资料保留，确认发送后才调用模型。
                  </p>
                  {carry && handoffReadError && (
                    <p role="alert" className="text-xs">
                      读取交接内容失败。{String(handoffReadError)} 可重试或取消携带。
                    </p>
                  )}
                  <button
                    className="hb-btn-primary"
                    disabled={
                      busy ||
                      transfer.busy ||
                      !goal.trim() ||
                      (carry && !canCarry) ||
                      (carrySavedDraft && !canCarrySavedDraft)
                    }
                    onClick={() => void transfer.prepare()}
                  >
                    准备交接草稿
                  </button>
                  <HandoffStatus handoff={transfer} />
                </section>
              )}
              {selected.provider === "homebase" ? (
                <Assistant key={selected.id} session={selected.id} embedded />
              ) : interactive && selected.provider !== "homebase" ? (
                <NativeConversation
                  key={`${selected.provider}:${selected.id}`}
                  id={selected.id}
                  provider={selected.provider}
                />
              ) : (
                <section
                  className="space-y-5 rounded-xl border border-border bg-card p-5"
                  aria-label="已保存的原生对话"
                >
                  <p className="text-xs text-muted-foreground">
                    历史只读 · {selected.provider} ·{" "}
                    {detail.data?.cwd ?? "工作目录未知"}
                  </p>
                  {detail.isFetching && (
                    <p role="status" className="text-sm text-muted-foreground">
                      {detail.data ? "正在刷新原对话…" : "正在读取原对话…"}
                    </p>
                  )}
                  {detail.isError && (
                    <p role="alert" className="text-sm text-destructive">
                      原对话读取失败：{String(detail.error)}
                      {detail.data && " 上次读取的内容仍可查看。"}
                    </p>
                  )}
                  {detail.isSuccess && detail.data.messages.length === 0 && (
                    <p className="text-sm text-muted-foreground">这段对话没有可显示的消息。</p>
                  )}
                  {detail.data?.messages.map((m) => (
                    <article
                      key={m.uuid}
                      className={
                        m.role === "user"
                          ? "ml-auto max-w-[85%] rounded-xl bg-secondary p-4"
                          : "max-w-3xl"
                      }
                    >
                      <p className="mb-2 text-xs text-muted-foreground">
                        {m.role === "user" ? "你" : selected.provider}
                      </p>
                      <MarkdownRenderer source={m.content} />
                    </article>
                  ))}
                </section>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
