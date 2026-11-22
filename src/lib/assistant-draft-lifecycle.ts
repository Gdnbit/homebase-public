import { isTauri } from "@tauri-apps/api/core";
import { useSyncExternalStore } from "react";
import { flushAllDrafts, hasUnsavedDrafts } from "./assistant-drafts";
import { flushInboxSource, hasUnsavedInboxSource } from "./inbox-workspace";
import { isNativeDraftClosePending, setNativeDraftClosePending } from "./draft-close-state";
import {
  flushDevelopmentWorkspace,
  hasUnsavedDevelopmentWorkspace,
} from "./development-workspace";

const flushInputs = () =>
  Promise.allSettled([
    flushAllDrafts(),
    flushInboxSource(),
    flushDevelopmentWorkspace(),
  ]);
const hasPendingAutosaves = () =>
  hasUnsavedDrafts() ||
  hasUnsavedInboxSource() ||
  hasUnsavedDevelopmentWorkspace();

type VaultCloseStatus = "clean" | "draft" | "saving";
const vaultCloseGuards = new Set<() => VaultCloseStatus>();

// Only the state of the editor is shared with the close flow.
export function registerVaultCloseStatus(readStatus: () => VaultCloseStatus) {
  vaultCloseGuards.add(readStatus);
  return () => { vaultCloseGuards.delete(readStatus); };
}

function vaultCloseStatus(): VaultCloseStatus {
  let status: VaultCloseStatus = "clean";
  for (const readStatus of vaultCloseGuards) {
    const current = readStatus();
    if (current === "saving") return current;
    if (current === "draft") status = current;
  }
  return status;
}

const hasUnsavedInputs = () =>
  hasPendingAutosaves() || vaultCloseStatus() !== "clean";

type CloseNotice = { text: string; warning: boolean; canRetryExit: boolean };
type CloseRegistration =
  "idle" | "registering" | "retrying" | "ready" | "failed";
const CLOSE_FEEDBACK_DELAY_MS = 10_000;
let closeNotice: CloseNotice | null = null;
let registration: CloseRegistration = "idle";
let flushListenersInstalled = false;
let closing = false;
const closeNoticeListeners = new Set<() => void>();

function updateCloseNotice(
  text: string | null,
  warning = false,
  canRetryExit = false,
) {
  closeNotice = text === null ? null : { text, warning, canRetryExit };
  closeNoticeListeners.forEach((listener) => listener());
}

function subscribeToCloseNotice(listener: () => void) {
  closeNoticeListeners.add(listener);
  return () => {
    closeNoticeListeners.delete(listener);
  };
}

export function useDraftCloseNotice() {
  return useSyncExternalStore(subscribeToCloseNotice, () => closeNotice);
}

export function isNativeClosePending() {
  return isNativeDraftClosePending();
}

export function useNativeClosePending() {
  return useSyncExternalStore(subscribeToCloseNotice, isNativeClosePending);
}

function updateRegistration(state: CloseRegistration) {
  registration = state;
  closeNoticeListeners.forEach((listener) => listener());
}

export function useDraftCloseRegistration() {
  return useSyncExternalStore(subscribeToCloseNotice, () => registration);
}

async function flushBeforeClose() {
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try {
    const deadline = new Promise<never>((_, reject) => {
      timeout = setTimeout(() => reject(Error("保存尚未确认")), 10_000);
    });
    do {
      const saves = await Promise.race([flushInputs(), deadline]);
      if (saves.some((save) => save.status === "rejected")) {
        throw Error("部分草稿保存失败");
      }
      if (!hasPendingAutosaves()) return;
      // Let the timer and window events run when another save is still needed.
      await Promise.race([
        new Promise<void>((resolve) => setTimeout(resolve, 0)),
        deadline,
      ]);
    } while (hasPendingAutosaves());
  } finally {
    clearTimeout(timeout);
  }
}

export function retryDraftClose() {
  if (!closeNotice?.canRetryExit) return;
  closing = false;
  return requestDraftClose();
}

function deferCloseDuringVaultSave() {
  if (vaultCloseStatus() !== "saving") return false;
  updateCloseNotice("密钥保存仍在等待确认。完成后可以再次退出。", true);
  return true;
}

export async function requestDraftClose() {
  if (!isTauri() || closing) return;
  closing = true;
  const earlierClosePending = isNativeDraftClosePending();
  let nativeCloseAccepted = false;
  let exitFeedbackTimer: ReturnType<typeof setTimeout> | undefined;
  const acceptedNotice = "退出请求已受理，窗口仍然打开。已保存的草稿会保留。";
  try {
    if (deferCloseDuringVaultSave()) return;
    if (!earlierClosePending) updateCloseNotice("正在核对草稿保存，完成后会关闭窗口。");
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const windowHandle = getCurrentWindow();
    let saveFailed = false;
    if (!earlierClosePending) {
      try {
        await flushBeforeClose();
      } catch {
        saveFailed = true;
      }
    }
    if (deferCloseDuringVaultSave()) return;
    if (!earlierClosePending && (saveFailed || vaultCloseStatus() === "draft")) {
      updateCloseNotice("部分输入尚未确认保存，请查看关闭确认。", true);
      try {
        const { confirm } = await import("@tauri-apps/plugin-dialog");
        if (deferCloseDuringVaultSave()) return;
        const vaultDraft = vaultCloseStatus() === "draft";
        const discard = await confirm(
          vaultDraft
            ? "窗口里还有未保存的密钥编辑。退出会放弃未保存的输入。已保存的密钥和原请求会保留。是否仍然关闭？"
            : "部分输入尚未确认保存。关闭可能丢失这些未保存的修改；已保存的原请求仍会保留。是否仍然关闭？",
          {
            title: "HomeBase · 草稿尚未保存",
            kind: "warning",
            okLabel: "仍然关闭",
            cancelLabel: "返回核对",
          },
        );
        if (!discard) {
          updateCloseNotice(null);
          return;
        }
      } catch {
        updateCloseNotice(
          "无法显示关闭确认，当前输入仍在窗口。请核对草稿后再次关闭。",
          true,
        );
        return;
      }
    }
    if (deferCloseDuringVaultSave()) return;
    setNativeDraftClosePending(true);
    updateCloseNotice("正在关闭窗口…");
    exitFeedbackTimer = setTimeout(() => {
      exitFeedbackTimer = undefined;
      updateCloseNotice("退出请求仍未得到回应。窗口暂不可编辑。", true);
    }, CLOSE_FEEDBACK_DELAY_MS);
    await windowHandle.destroy();
    // Tauri acknowledges enqueueing the request before the window is gone.
    nativeCloseAccepted = true;
    if (exitFeedbackTimer === undefined)
      updateCloseNotice(acceptedNotice, true);
    clearTimeout(exitFeedbackTimer);
    exitFeedbackTimer = setTimeout(() => {
      updateCloseNotice(acceptedNotice, true, true);
    }, CLOSE_FEEDBACK_DELAY_MS);
  } catch {
    setNativeDraftClosePending(earlierClosePending);
    updateCloseNotice(
      earlierClosePending
        ? "再次退出请求遭到拒绝。仍在等待先前的退出请求。"
        : hasUnsavedInputs()
        ? "窗口未能关闭，当前输入仍在窗口。请核对草稿后再次关闭。"
        : "草稿已保存，但窗口未能关闭。请再次关闭窗口。",
      true,
      earlierClosePending,
    );
  } finally {
    if (!nativeCloseAccepted) {
      clearTimeout(exitFeedbackTimer);
      closing = false;
    }
  }
}

// One application-level listener: switching routes must not install close handlers.
export async function installDraftLifecycle() {
  if (!flushListenersInstalled) {
    const flush = () => {
      if (!closing && !isNativeDraftClosePending()) void flushInputs().catch(() => {});
    };
    window.addEventListener("blur", flush);
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") flush();
    });
    window.addEventListener("beforeunload", (event) => {
      if (hasUnsavedInputs()) {
        event.preventDefault();
        event.returnValue = "";
      }
    });
    flushListenersInstalled = true;
  }

  if (!isTauri() || (registration !== "idle" && registration !== "failed"))
    return;
  updateRegistration(registration === "failed" ? "retrying" : "registering");
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().onCloseRequested((event) => {
      event.preventDefault();
      return requestDraftClose();
    });
    updateRegistration("ready");
  } catch {
    updateRegistration("failed");
  }
}
