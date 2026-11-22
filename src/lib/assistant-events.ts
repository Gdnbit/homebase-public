import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { assistantTurnKey, assistantTurnProgressKey, cachedWindowTurn, type TimelinePage } from "./assistant-window";
import { invalidateSessionCatalog } from "./assistant-sessions";
import { refreshAssistantReads } from "./assistant-refresh";
import { refreshCapabilityCatalog } from "./capability-index";
import {
  assistantActive,
  assistantApi,
  type AssistantConversation,
} from "./assistant";
import {
  isPermissionGranted,
  sendNotification,
} from "@tauri-apps/plugin-notification";

// Keep cache invalidation alive across routes, without starting off-page polling.
export function useAssistantUpdates() {
  const client = useQueryClient();
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    void listen("capabilities:changed", () => {
      if (disposed || timer !== undefined) return;
      timer = setTimeout(() => {
        timer = undefined;
        void client.invalidateQueries({ queryKey: ["repo-capabilities"] });
        refreshCapabilityCatalog(client);
      }, 100);
    })
      .then((fn) => {
        if (disposed) fn();
        else stop = fn;
      })
      .catch(() => {
        /* Manual refresh remains available. */
      });
    return () => {
      disposed = true;
      stop?.();
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [client]);
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let stopPreview: (() => void) | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const sessions = new Set<string>();
    const turns = new Map<string, Set<string>>();
    const contextSessions = new Set<string>();
    let config = false;
    let personal = false;
    let usage = false;
    const notified = new Set<string>();
    async function notifyReminders() {
      try {
        if (
          localStorage.getItem("homebase.personal.notifications") !== "on" ||
          !(await isPermissionGranted())
        )
          return;
        const state = await client.fetchQuery({
          queryKey: ["assistant", "personal"],
          queryFn: assistantApi.personal,
          staleTime: 1000,
        });
        for (const r of state.reminders.filter(
          (r) => r.delivery === "desktop_due",
        )) {
          const id = `${r.id}:${r.last_due_at}`;
          if (notified.has(id) || disposed) continue;
          sendNotification({ title: "HomeBase 提醒", body: r.text });
          notified.add(id);
        }
      } catch {
        if (!disposed)
          client.setQueryData(
            ["assistant", "personal-notification-error"],
            "系统通知未发送，应用内提醒仍然有效",
          );
      }
    }
    // Catch due reminders restored before frontend event listeners were mounted.
    void notifyReminders();
    void listen<{session_id: string; turn_id: string}>("assistant:preview", ({ payload }) => {
      if (disposed || !payload.session_id || !payload.turn_id) return;
      refreshAssistantReads(client, {
        queryKey: assistantTurnProgressKey(payload.session_id, payload.turn_id),
        exact: true,
        type: "active",
      });
    }).then((stop) => {
      if (disposed) stop();
      else stopPreview = stop;
    }).catch(() => {
      /* The existing status polling still reads the latest preview. */
    });
    void listen<{
      session_id: string | null;
      turn_id: string | null;
      config: boolean;
      personal?: boolean;
    }>("assistant:changed", (e) => {
      const { session_id, turn_id } = e.payload;
      // New-message ownership may archive a source from the root draft. Only
      // invalidate small indexes; immutable PDF text is not reread per progress.
      void client.invalidateQueries({
        predicate: (q) =>
          q.queryKey[0] === "assistant-documents" && q.queryKey.length === 2,
        refetchType: "none",
      });
      usage ||= !!(session_id || turn_id);
      if (session_id) {
        contextSessions.add(session_id);
        if (turn_id) {
          let changed = turns.get(session_id);
          if (!changed) {
            changed = new Set();
            turns.set(session_id, changed);
          }
          changed.add(turn_id);
        }
        const cached = client.getQueryData<
          AssistantConversation | { pages: TimelinePage[] }
        >(["assistant", "session", session_id]);
        // A terminal turn has no mounted turn query. An external resume must
        // refresh its session once so the page discovers that it is active again.
        const known =
          turn_id && cached
            ? "pages" in cached
              ? cachedWindowTurn(cached.pages, turn_id)
              : cached.turns.find((t) => t.id === turn_id)
            : undefined;
        const observingTurn = !!turn_id && client.getQueryCache().findAll({
          queryKey: assistantTurnProgressKey(session_id, turn_id),
          exact: true,
          type: "active",
        }).length > 0;
        // A cached running turn is not an active reader. Off-page completion
        // still needs a small index/session refresh; never reopen or resume it.
        if (!(known && assistantActive(known.state) && observingTurn))
          sessions.add(session_id);
      }
      config ||= e.payload.config;
      personal ||= !!e.payload.personal;
      if (timer) return;
      timer = setTimeout(() => {
        timer = undefined;
        if (usage) {
          refreshAssistantReads(client, {
            queryKey: ["assistant", "usage"],
            exact: true,
          });
        }
        // A dormant detail must be stale before a window discovers the resumed
        // turn and mounts its observer, even if the old detail was just read.
        for (const [session, ids] of turns)
          for (const id of ids)
            refreshAssistantReads(client, {
              queryKey: assistantTurnKey(session, id),
            });
        if (sessions.size)
          void invalidateSessionCatalog(client);
        for (const id of sessions)
          refreshAssistantReads(client, {
            queryKey: ["assistant", "session", id],
          });
        if (config) {
          refreshCapabilityCatalog(client);
          void client.invalidateQueries({ queryKey: ["assistant", "config"] });
          void client.invalidateQueries({ queryKey: ["assistant", "local"] });
        }
        if (personal) {
          void client.invalidateQueries({
            queryKey: ["assistant", "personal"],
          });
          void notifyReminders();
        }
        // Refresh an open summary once per relevant event batch. Closed
        // summaries only become stale; another conversation causes no read.
        refreshAssistantReads(client, {
          predicate: (q) =>
            q.queryKey[0] === "assistant" &&
            q.queryKey[1] === "context" &&
            (config || personal || contextSessions.has(String(q.queryKey[2]))),
        });
        sessions.clear();
        turns.clear();
        contextSessions.clear();
        config = false;
        personal = false;
        usage = false;
      }, 400);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((e) => {
        if (!disposed)
          client.setQueryData(
            ["assistant", "events-error"],
            `状态事件不可用，页面仍可手动刷新：${String(e)}`,
          );
      });
    return () => {
      disposed = true;
      unlisten?.();
      stopPreview?.();
      if (timer) clearTimeout(timer);
    };
  }, [client]);
}
