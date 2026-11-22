import { useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Loader2 } from "lucide-react";
import { prepareAssistantHandoff } from "@/lib/assistant-handoff";
import { prepareHandoffDraft } from "@/lib/assistant-drafts";
import { invalidateSessionCatalog } from "@/lib/assistant-sessions";

export function useAssistantHandoff(getText: () => string, contextKey = "") {
  const navigate = useNavigate(),
    location = useLocation(),
    client = useQueryClient();
  const latest = useRef(getText);
  latest.current = getText;
  const alive = useRef(true),
    working = useRef(false),
    intent = useRef<{ id: string; text: string; key: string } | undefined>(
      undefined,
    );
  const route = useRef(location.key);
  route.current = location.key;
  const context = useRef(contextKey);
  context.current = contextKey;
  const [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [saved, setSaved] = useState<string>();
  const savedPayload = useRef<
    { id: string; text: string; key: string; source: string } | undefined
  >(undefined);
  const backup = useRef<
    { id: string; text: string; key: string; source: string } | undefined
  >(undefined);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  async function prepare() {
    if (working.current) return;
    working.current = true;
    setBusy(true);
    setError("");
    setSaved(undefined);
    const key = route.current;
    const source = context.current;
    try {
      const text = latest.current();
      const identity = `${key}\u0000${source}`;
      if (
        !intent.current ||
        intent.current.text !== text ||
        intent.current.key !== identity
      )
        intent.current = { id: crypto.randomUUID(), text, key: identity };
      const id = await prepareAssistantHandoff(text, intent.current.id);
      void invalidateSessionCatalog(client,false);
      if (!alive.current || route.current !== key || context.current !== source)
        return;
      savedPayload.current = { id, text, key, source };
      setSaved(id);
      if (latest.current() !== text) {
        setError(
          "先前内容已保存为独立草稿，当前修改保留。需要时可打开已保存草稿。",
        );
        return;
      }
      navigate(`/assistant?session=${encodeURIComponent(id)}`);
    } catch (reason) {
      if (alive.current && route.current === key && context.current === source)
        setError(String(reason));
    } finally {
      working.current = false;
      if (alive.current) setBusy(false);
    }
  }
  async function openSaved() {
    const original = savedPayload.current;
    if (
      working.current ||
      !original ||
      original.id !== saved ||
      route.current !== original.key ||
      context.current !== original.source
    )
      return;
    working.current = true;
    setBusy(true);
    setError("");
    try {
      const current = latest.current();
      if (current.trim() && current !== original.text) {
        if (
          !backup.current ||
          backup.current.text !== current ||
          backup.current.key !== original.key ||
          backup.current.source !== original.source
        )
          backup.current = {
            id: crypto.randomUUID(),
            text: current,
            key: original.key,
            source: original.source,
          };
        // Opening an older saved draft must not discard edits made on the source.
        await prepareHandoffDraft(`handoff:${backup.current.id}`, current);
      }
      if (
        !alive.current ||
        route.current !== original.key ||
        context.current !== original.source
      )
        return;
      if (latest.current() !== current)
        throw Error(
          "输入再次变化，先前版本已受保护保存；当前输入保留，请核对后再打开。",
        );
      navigate(`/assistant?session=${encodeURIComponent(original.id)}`);
    } catch (reason) {
      if (
        alive.current &&
        route.current === original.key &&
        context.current === original.source
      )
        setError(String(reason));
    } finally {
      working.current = false;
      if (alive.current) setBusy(false);
    }
  }
  const visibleSaved =
    savedPayload.current?.key === location.key &&
    savedPayload.current?.source === contextKey
      ? saved
      : undefined;
  return { busy, error, saved: visibleSaved, prepare, openSaved };
}
export function HandoffStatus({
  handoff,
}: {
  handoff: ReturnType<typeof useAssistantHandoff>;
}) {
  return (
    <>
      {handoff.error && (
        <p
          role="alert"
          className="mt-2 break-words text-sm text-muted-foreground"
        >
          {handoff.error}
        </p>
      )}
      {handoff.saved && (
        <button
          type="button"
          disabled={handoff.busy}
          className="mt-2 text-sm underline"
          onClick={() => void handoff.openSaved()}
        >
          打开已保存草稿
        </button>
      )}
    </>
  );
}
export function WorkflowAssistantEntry({
  title,
  description,
  getText,
  label = "交给助理",
  disabled = false,
}: {
  title: string;
  description: string;
  getText: () => string;
  label?: string;
  disabled?: boolean;
}) {
  const handoff = useAssistantHandoff(getText);
  return (
    <section className="hb-card" aria-label={title}>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="min-w-0">
          <h2 className="text-sm font-medium">{title}</h2>
          <p className="mt-1 text-sm text-muted-foreground">{description}</p>
        </div>
        <button
          type="button"
          className="hb-btn-outline shrink-0"
          disabled={disabled || handoff.busy}
          onClick={() => void handoff.prepare()}
        >
          {handoff.busy ? (
            <Loader2 size={16} className="animate-spin" />
          ) : (
            <ArrowRight size={16} />
          )}{" "}
          {handoff.busy ? "正在保存草稿…" : label}
        </button>
      </div>
      <HandoffStatus handoff={handoff} />
    </section>
  );
}
