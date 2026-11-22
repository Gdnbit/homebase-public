/**
 * Secrets Consent Manager · S3 · 2026-05-06
 *
 * 全局组件 · 挂在 App.tsx 顶层 · 任何页面都能弹。
 *
 * 工作流：
 * 1. listen("secrets:consent_request") · 后端 IPC 收到 mcp-secrets 请求时 emit
 * 2. push 到 queue · 同时只显示 head
 * 3. USER 点 "仅这一次 / 永久允许 / 拒绝" → invoke shared_secrets_consent_resolve
 * 4. 移除 head · 显示下一个（若有）
 *
 * 60s 不回应：后端会自动 deny · 前端这边显示倒计时给 USER 看（不主动调 deny · 让后端兜底）。
 */

import { useEffect, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { AlertTriangle, Check, Clock, Shield, X } from "lucide-react";
import {
  sharedSecretsApi,
  type ConsentAction,
  type ConsentRequestEvent,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";

const AGENT_DISPLAY: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex CLI",
  "gemini-cli": "Gemini CLI",
  openclaw: "OpenClaw",
};

export function SecretsConsentManager() {
  const [queue, setQueue] = useState<ConsentRequestEvent[]>([]);
  // 永远只看 head；用 ref 防止重复触发
  const inFlightRef = useRef<string | null>(null);

  useEffect(() => {
    if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
      return;
    }
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    (async () => {
      try {
        unlisten = await listen<ConsentRequestEvent>(
          "secrets:consent_request",
          (event) => {
            const payload = event.payload;
            if (!payload?.request_id) return;
            setQueue((prev) => {
              // 去重
              if (prev.some((p) => p.request_id === payload.request_id))
                return prev;
              return [...prev, payload];
            });
          },
        );
        if (cancelled) {
          unlisten?.();
          unlisten = null;
        }
      } catch (e) {
        console.error("[secrets-consent] listen 注册失败:", e);
      }
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const head = queue[0];

  async function decide(action: ConsentAction) {
    if (!head) return;
    if (inFlightRef.current === head.request_id) return;
    inFlightRef.current = head.request_id;
    try {
      await sharedSecretsApi.consentResolve(head.request_id, action);
    } catch (e) {
      console.error("[secrets-consent] resolve 失败:", e);
    } finally {
      inFlightRef.current = null;
      setQueue((prev) => prev.filter((p) => p.request_id !== head.request_id));
    }
  }

  if (!head) return null;
  return <ConsentDialog event={head} onDecide={decide} queueLen={queue.length} />;
}

// =====================================================================
// 单条弹窗
// =====================================================================

function ConsentDialog({
  event,
  onDecide,
  queueLen,
}: {
  event: ConsentRequestEvent;
  onDecide: (action: ConsentAction) => void;
  queueLen: number;
}) {
  const [secondsLeft, setSecondsLeft] = useState(60);

  // 60s 倒计时（仅展示 · 后端真正负责超时 deny）
  useEffect(() => {
    setSecondsLeft(60);
    const t = setInterval(() => {
      setSecondsLeft((s) => Math.max(0, s - 1));
    }, 1000);
    return () => clearInterval(t);
  }, [event.request_id]);

  const display = AGENT_DISPLAY[event.agent_id] ?? event.agent_id;
  const danger = secondsLeft <= 10;

  return (
    <div
      className="fixed inset-0 z-[1000] flex items-center justify-center bg-black/40 p-6 backdrop-blur-sm"
      role="dialog"
      aria-modal="true"
      aria-labelledby="consent-title"
    >
      <div className="w-full max-w-md overflow-hidden rounded-xl border border-border bg-background shadow-2xl">
        {/* 头部 */}
        <div className="flex items-center gap-3 border-b border-border bg-amber-500/5 px-5 py-4">
          <Shield className="h-5 w-5 shrink-0 text-amber-600 dark:text-amber-500" />
          <div className="min-w-0 flex-1">
            <h2 id="consent-title" className="font-serif text-base font-semibold">
              Secret 访问请求
            </h2>
            <p className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
              S3 · runtime consent
            </p>
          </div>
          {queueLen > 1 && (
            <span className="rounded-md border border-border bg-card px-2 py-0.5 font-mono text-[10px] text-muted-foreground">
              排队 +{queueLen - 1}
            </span>
          )}
        </div>

        {/* 主内容 */}
        <div className="space-y-4 px-5 py-5">
          <div>
            <div className="text-xs uppercase tracking-wider text-muted-foreground">
              Agent
            </div>
            <div className="font-medium">{display}</div>
          </div>
          <div>
            <div className="text-xs uppercase tracking-wider text-muted-foreground">
              请求的 secret
            </div>
            <code className="font-mono text-sm text-foreground">
              {event.secret_name}
            </code>
          </div>
          <div>
            <div className="text-xs uppercase tracking-wider text-muted-foreground">
              用途说明（agent 提交）
            </div>
            <div className="rounded-md border border-border bg-muted/30 p-2 text-sm">
              {event.purpose || (
                <span className="italic text-muted-foreground">（未提供）</span>
              )}
            </div>
          </div>
          <div className="flex items-center gap-2 text-xs">
            <Clock
              className={cn(
                "h-3.5 w-3.5",
                danger
                  ? "text-rose-600 dark:text-rose-400"
                  : "text-muted-foreground",
              )}
            />
            <span
              className={cn(
                danger
                  ? "font-medium text-rose-600 dark:text-rose-400"
                  : "text-muted-foreground",
              )}
            >
              {secondsLeft > 0
                ? `${secondsLeft}s 后未回应自动拒绝`
                : "已超时 · 后端将拒绝"}
            </span>
          </div>
        </div>

        {/* 按钮 */}
        <div className="flex flex-col gap-2 border-t border-border bg-card/50 px-5 py-4">
          <button
            type="button"
            onClick={() => onDecide("allow_persistent")}
            className="flex items-center justify-center gap-2 rounded-md border border-green-500/30 bg-green-500/10 px-4 py-2 text-sm font-medium text-green-700 transition-colors hover:bg-green-500/20 dark:text-green-400"
          >
            <Check className="h-4 w-4" />
            永久允许
            <span className="font-mono text-[10px] opacity-60">
              下次同 agent + secret 不再询问
            </span>
          </button>
          <button
            type="button"
            onClick={() => onDecide("allow_once")}
            className="flex items-center justify-center gap-2 rounded-md border border-border bg-card px-4 py-2 text-sm font-medium text-foreground transition-colors hover:bg-accent"
          >
            <Check className="h-4 w-4 opacity-60" />
            仅这一次
          </button>
          <button
            type="button"
            onClick={() => onDecide("deny")}
            className="flex items-center justify-center gap-2 rounded-md border border-rose-500/30 bg-rose-500/5 px-4 py-2 text-sm font-medium text-rose-700 transition-colors hover:bg-rose-500/10 dark:text-rose-400"
          >
            <X className="h-4 w-4" />
            拒绝
          </button>
          <p className="mt-1 flex items-start gap-1.5 text-[11px] text-muted-foreground">
            <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
            选择前请确认 agent 名 / 用途真实可信。本机 IPC 仅 127.0.0.1 + token 鉴权。
          </p>
        </div>
      </div>
    </div>
  );
}
