import { useLayoutEffect, useRef, type ReactNode } from "react";
import { Sidebar } from "./Sidebar";
import { PreflightBanner } from "@/components/preflight-banner";
import {
  installDraftLifecycle,
  retryDraftClose,
  useDraftCloseNotice,
  useDraftCloseRegistration,
  useNativeClosePending,
} from "@/lib/assistant-draft-lifecycle";
import { useLocation } from "react-router-dom";

interface AppLayoutProps {
  children: ReactNode;
}

function NativeCloseDialog({ children }: AppLayoutProps) {
  const dialog = useRef<HTMLDialogElement>(null);
  useLayoutEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, []);
  return (
    <dialog
      ref={dialog}
      aria-label="正在退出 HomeBase"
      onCancel={(event) => event.preventDefault()}
      className="m-auto w-[calc(100%-2rem)] max-w-md rounded-xl border border-border bg-card p-5 text-foreground shadow-xl backdrop:bg-black/50"
    >
      <h2 className="px-3 py-1 font-serif text-lg font-semibold">正在退出 HomeBase</h2>
      {children}
    </dialog>
  );
}

export function AppLayout({ children }: AppLayoutProps) {
  const { pathname } = useLocation();
  const closeNotice = useDraftCloseNotice();
  const closeRegistration = useDraftCloseRegistration();
  const nativeClosePending = useNativeClosePending();
  const retryingClose = closeRegistration === "retrying";
  const chat = pathname === "/" || pathname === "/assistant";
  const closeFeedback = closeNotice && (
    <div
      className={`mx-3 my-2 flex shrink-0 flex-wrap items-center gap-2 rounded-md border px-3 py-2 text-sm ${closeNotice.warning ? "border-warning/30 bg-warning/5 text-warning-strong" : "border-border text-muted-foreground"}`}
    >
      <p role={closeNotice.warning ? "alert" : "status"}>{closeNotice.text}</p>
      {closeNotice.canRetryExit && (
        <button
          type="button"
          className="rounded-md border border-warning/30 px-2 py-1 hover:bg-warning/10"
          onClick={() => void retryDraftClose()}
        >
          重试退出
        </button>
      )}
    </div>
  );
  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background">
      <Sidebar />
      <main
        className={`min-w-0 flex-1 ${chat ? "flex min-h-0 flex-col overflow-hidden" : "overflow-auto"}`}
      >
        <div
          className={
            chat
              ? "flex min-h-0 flex-1 flex-col"
              : "mx-auto w-full max-w-7xl px-4 py-8 sm:px-6 lg:px-8"
          }
        >
          <PreflightBanner />
          {(closeRegistration === "failed" || retryingClose) && (
            <section className="mx-3 my-2 flex shrink-0 flex-wrap items-center gap-2 rounded-md border border-warning/30 bg-warning/5 px-3 py-2 text-sm text-warning-strong">
              <p role={retryingClose ? "status" : "alert"}>
                {retryingClose
                  ? "正在重试标题栏关闭保护。"
                  : "标题栏关闭保护未就绪。请确认草稿已保存，或使用左侧的「退出 HomeBase」。"}
              </p>
              <button
                type="button"
                className="rounded-md border border-warning/30 px-2 py-1 hover:bg-warning/10 disabled:opacity-50"
                disabled={retryingClose}
                onClick={() => void installDraftLifecycle()}
              >
                重试关闭保护
              </button>
            </section>
          )}
          {nativeClosePending ? <NativeCloseDialog>{closeFeedback}</NativeCloseDialog> : closeFeedback}
          {children}
        </div>
      </main>
    </div>
  );
}
