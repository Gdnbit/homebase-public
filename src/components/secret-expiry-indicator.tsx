import { useState, useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { Bell, X } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { Link, useLocation } from "react-router-dom";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { secretsApi } from "@/lib/tauri-api";
import { expiryStatus } from "@/lib/secret-lifecycle";
import { useClock } from "@/lib/use-clock";

function stored(key: string) {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function remember(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* Reminders still work without persistent storage. */
  }
}
const sentThisSession = new Map<string, string>();

export function SecretExpiryIndicator() {
  const { pathname } = useLocation();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const [position, setPosition] = useState({ left: 80, top: 80, width: 360 });
  useEffect(() => setOpen(false), [pathname]);
  const locate = () => {
    const rect = trigger.current?.getBoundingClientRect();
    if (!rect) return;
    const width = Math.min(360, window.innerWidth - 24);
    const edge =
      trigger.current?.closest("aside")?.getBoundingClientRect().right ??
      rect.right;
    setPosition({
      width,
      left: Math.max(12, Math.min(edge + 8, window.innerWidth - width - 12)),
      top: Math.max(12, Math.min(rect.top, window.innerHeight - 420)),
    });
  };
  useEffect(() => {
    if (!open) return;
    closeButton.current?.focus();
    const outside = (event: PointerEvent) => {
      if (
        !panel.current?.contains(event.target as Node) &&
        !trigger.current?.contains(event.target as Node)
      )
        setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    window.addEventListener("resize", locate);
    return () => {
      document.removeEventListener("pointerdown", outside);
      document.removeEventListener("keydown", escape);
      window.removeEventListener("resize", locate);
    };
  }, [open]);
  const now = useClock();
  const [notify, setNotify] = useState(
    () => stored("homebase.expiry.notifications") === "on",
  );
  const [notice, setNotice] = useState("");
  const query = useQuery({
    queryKey: ["secrets", "lifecycle"],
    queryFn: secretsApi.lifecycle,
    networkMode: "always", // Local metadata remains available without Internet.
    refetchInterval: 60_000,
    refetchOnWindowFocus: true,
  });
  const due = (query.data ?? []).filter(
    (s) => expiryStatus(s.expires_at, now).warning,
  );
  const pending = (query.data ?? []).filter((s) => s.pending_revocation);
  const signature = due
    .map(
      (s) =>
        `${s.short_name}:${s.expires_at}:${expiryStatus(s.expires_at, now).state}`,
    )
    .sort()
    .join("|");

  useEffect(() => {
    if (!notify || !signature || query.isError) return;
    let active = true;
    void (async () => {
      try {
        if (!(await isPermissionGranted()) || !active) return;
        const date = new Date(now).toLocaleDateString("en-CA");
        const key = `homebase.expiry.sent.${date}`;
        if (stored(key) === signature || sentThisSession.get(key) === signature)
          return;
        sendNotification({
          title: "HomeBase 密钥到期提醒",
          body: `有 ${due.length} 把密钥需要检查到期时间，请打开密钥页面处理。`,
        });
        sentThisSession.set(key, signature);
        remember(key, signature);
      } catch {
        if (active) setNotice("系统通知未发送，应用内提醒仍然有效");
      }
    })();
    return () => {
      active = false;
    };
  }, [notify, signature, now, due.length, query.isError]);

  async function enableNotifications() {
    try {
      const allowed =
        (await isPermissionGranted()) ||
        (await requestPermission()) === "granted";
      setNotify(allowed);
      remember("homebase.expiry.notifications", allowed ? "on" : "off");
      setNotice(
        allowed
          ? "已启用应用运行期间的系统提醒"
          : "未获得通知权限，应用内提醒仍然有效",
      );
    } catch {
      setNotice("当前环境不支持系统通知，应用内提醒仍然有效");
    }
  }

  const count = new Set([...due, ...pending].map((item) => item.short_name))
    .size;
  const attention = query.isError || count > 0;
  const summary = query.isPending
    ? "密钥提醒：正在检查"
    : query.isError
      ? "密钥提醒：状态未知"
      : count
        ? `密钥提醒：${count} 项待处理`
        : "密钥提醒：暂无待处理";
  return (
    <>
      <button
        ref={trigger}
        type="button"
        aria-label={summary}
        title={summary}
        aria-expanded={open}
        aria-controls="secret-reminder-panel"
        aria-haspopup="dialog"
        className="absolute -right-1 top-0 z-10 flex h-5 w-5 items-center justify-center rounded-md text-muted-foreground/50 hover:bg-muted hover:text-foreground focus-visible:outline focus-visible:outline-2 focus-visible:outline-primary-500 md:right-1 md:top-1 md:h-7 md:w-7"
        onClick={() => {
          locate();
          setOpen((value) => !value);
        }}
      >
        <Bell className="h-3 w-3 md:h-3.5 md:w-3.5" aria-hidden="true" />
        {attention && (
          <span
            data-testid="secret-reminder-dot"
            aria-hidden="true"
            className={`absolute right-0.5 top-0.5 h-1.5 w-1.5 rounded-full ring-2 ring-card md:right-1 md:top-1 ${query.isError ? "bg-warning" : "bg-destructive"}`}
          />
        )}
      </button>
      <span role="status" className="sr-only">
        {summary}
      </span>
      {open &&
        createPortal(
          <div
            ref={panel}
            id="secret-reminder-panel"
            role="dialog"
            aria-modal="false"
            aria-label="密钥提醒"
            style={{
              ...position,
              maxHeight: `calc(100vh - ${position.top + 12}px)`,
            }}
            className="fixed z-50 max-h-[calc(100vh-24px)] overflow-auto rounded-xl border border-border bg-card p-4 text-sm text-foreground shadow-lg"
          >
            <div className="mb-3 flex items-center justify-between gap-3">
              <h2 className="font-medium">密钥提醒</h2>
              <button
                ref={closeButton}
                type="button"
                aria-label="关闭密钥提醒"
                className="rounded p-1 text-muted-foreground hover:bg-muted"
                onClick={() => {
                  setOpen(false);
                  trigger.current?.focus();
                }}
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            {query.isPending ? (
              <p role="status" className="text-muted-foreground">
                正在检查到期状态…
              </p>
            ) : query.isError ? (
              <>
                <p>密钥到期状态读取失败，暂时无法确认是否有到期告警。</p>
                <button
                  type="button"
                  disabled={query.isFetching}
                  onClick={() => void query.refetch()}
                  className="hb-btn-outline mt-2 text-xs"
                >
                  重试到期检查
                </button>
              </>
            ) : (
              <div className="space-y-3">
                <p>
                  {due.length > 0
                    ? `${due.length} 把密钥已到期、即将到期或日期无效。`
                    : ""}
                  {pending.length > 0
                    ? `${pending.length} 次轮换待确认旧密钥停用。`
                    : ""}
                </p>
                {count === 0 && (
                  <p className="text-muted-foreground">暂无到期或轮换待办。</p>
                )}
                <ul className="space-y-1">
                  {due.map((s) => (
                    <li key={s.short_name}>
                      {s.display_name}：{expiryStatus(s.expires_at, now).label}
                    </li>
                  ))}
                </ul>
                <div className="flex flex-wrap items-center gap-3">
                  <Link
                    className="text-primary-700 underline dark:text-primary-300"
                    to="/secrets"
                    onClick={() => setOpen(false)}
                  >
                    前往密钥管理
                  </Link>
                  <button
                    type="button"
                    className="hb-btn-outline text-xs"
                    onClick={() => void query.refetch()}
                    disabled={query.isFetching}
                  >
                    刷新到期状态
                  </button>
                  {notify ? (
                    <button
                      type="button"
                      className="hb-btn-outline text-xs"
                      onClick={() => {
                        setNotify(false);
                        remember("homebase.expiry.notifications", "off");
                        setNotice("系统提醒已关闭，应用内提醒保留");
                      }}
                    >
                      关闭系统提醒
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="hb-btn-outline text-xs"
                      onClick={() => void enableNotifications()}
                    >
                      启用系统到期提醒
                    </button>
                  )}
                </div>
                {notice && <p role="status">{notice}</p>}
                <p className="text-xs text-muted-foreground">
                  运行期间自动检查到期状态。
                </p>
              </div>
            )}
          </div>,
          document.body,
        )}
    </>
  );
}
