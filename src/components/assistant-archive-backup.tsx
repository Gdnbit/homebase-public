import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Archive, Copy, X } from "lucide-react";
import { AssistantArchiveFiles } from "@/components/assistant-archive-files";
import {
  archiveBackup,
  archiveBackupIdKey,
  isArchiveBackupId,
  type BackupStatus,
} from "@/lib/assistant-archive-backup";

type Operation = "create" | "status";
type View = { id: string; status?: BackupStatus };

function rememberedId() {
  const value = localStorage.getItem(archiveBackupIdKey);
  if (value !== null && !isArchiveBackupId(value)) {
    throw Error("Invalid archive backup reference");
  }
  return value;
}

function backupSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const unit = bytes < 1024 ** 2 ? 1024 : 1024 ** 2;
  return `${(bytes / unit).toLocaleString("zh-CN", {
    maximumFractionDigits: 1,
  })} ${unit === 1024 ? "KiB" : "MiB"}`;
}

export function AssistantArchiveBackup() {
  const dialog = useRef<HTMLDialogElement>(null);
  const pending = useRef(false);
  const [dialogOpen, setDialogOpen] = useState(false);
  const [tracked, setTracked] = useState<string | null>(null);
  const [view, setView] = useState<View | null>(null);
  const [query, setQuery] = useState("");
  const [operation, setOperation] = useState<Operation | null>(null);
  const [error, setError] = useState("");
  const [storageError, setStorageError] = useState(false);
  const [copyFeedback, setCopyFeedback] = useState("");
  const [copying, setCopying] = useState(false);

  async function run(action: Operation, id: string) {
    if (pending.current) return;
    pending.current = true;
    setOperation(action);
    setView({ id });
    setError("");
    setCopyFeedback("");
    try {
      setView({ id, status: await archiveBackup(action, id) });
    } catch (failure) {
      setError(
        String(failure).includes("ARCHIVE_BACKUP_NO_INDEX:")
          ? "当前还没有可备份的分段档案。"
          : action === "create"
            ? "备份结果待核对。请查询原编号。"
            : "暂时无法确认备份状态，请再次查询。",
      );
    } finally {
      pending.current = false;
      setOperation(null);
    }
  }

  function open() {
    dialog.current?.showModal();
    setDialogOpen(true);
    if (pending.current || copying) return;
    setQuery("");
    setError("");
    setCopyFeedback("");
    try {
      const saved = rememberedId();
      setStorageError(false);
      setTracked(saved);
      if (saved) void run("status", saved);
      else setView(null);
    } catch {
      setStorageError(true);
      setView(null);
    }
  }

  function create(existing?: string) {
    if (pending.current) return;
    let id: string;
    try {
      const saved = rememberedId();
      // A newer action may have changed the pointer while this dialog was open.
      if (saved !== tracked) {
        setTracked(saved);
        if (saved) void run("status", saved);
        else setView(null);
        return;
      }
      id = existing ?? crypto.randomUUID();
      localStorage.setItem(archiveBackupIdKey, id);
      setStorageError(false);
      setTracked(id);
    } catch {
      setStorageError(true);
      return;
    }
    void run("create", id);
  }

  async function copyDirectory(directory: string) {
    setCopying(true);
    try {
      await invoke("plugin:clipboard-manager|write_text", { text: directory });
      setCopyFeedback("目录已复制。");
    } catch {
      setCopyFeedback("复制失败，可选中目录手动复制。");
    } finally {
      setCopying(false);
    }
  }

  const result = view?.status;
  const current = view?.id === tracked;
  const canCreate = !tracked || (current && (result?.state === "published" || result?.state === "incomplete"));
  const canContinue = !!tracked && current && result?.state === "not_found";
  const busy = operation !== null || copying;
  const lookup = query.trim();

  return (
    <>
      <button type="button" className="hb-btn-outline" onClick={open}>
        <Archive size={16} aria-hidden="true" />
        会话备份
      </button>
      <dialog
        ref={dialog}
        aria-labelledby="archive-backup-title"
        aria-describedby="archive-backup-scope"
        onClose={() => setDialogOpen(false)}
        className="m-auto max-h-[85dvh] w-[min(36rem,calc(100vw-2rem))] overflow-auto rounded-xl border border-border bg-card p-0 text-foreground shadow-2xl backdrop:bg-black/35"
      >
        <div className="space-y-4 p-4 sm:p-6">
          <header className="flex items-start justify-between gap-3">
            <h2 id="archive-backup-title" className="font-serif text-xl font-semibold">
              会话备份
            </h2>
            <button
              type="button"
              autoFocus
              className="chat-icon"
              aria-label="关闭会话备份"
              onClick={() => dialog.current?.close()}
            >
              <X size={18} />
            </button>
          </header>
          <p id="archive-backup-scope" className="text-sm text-muted-foreground">
            将 HomeBase 和微信的分段会话档案加密保存到本机，供同一台电脑上的当前 Windows 用户核验。
            已核验的 PDF 原件、任务产物、桌面草稿与录音可随备份保存。外部 Agent 的会话历史需各自保全。
          </p>
          {storageError && (
            <p role="alert" className="text-sm text-destructive">
              无法读写本次备份编号。可按编号查询已有备份。
            </p>
          )}
          {view && (
            <div className="min-w-0 space-y-2 rounded-lg border border-border p-3 text-sm">
              <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                <span className="text-muted-foreground">备份编号</span>
                <code className="break-all text-xs">{view.id}</code>
              </div>
              {operation && (
                <p role="status">{operation === "create" ? "正在创建加密备份…" : "正在查询备份状态…"}</p>
              )}
              {error && <p role="alert" className="text-destructive">{error}</p>}
              {result?.state === "not_found" && (
                <p role="status">尚未找到这个备份。</p>
              )}
              {result?.state === "incomplete" && (
                <p role="status">密文头尚未发布，可稍后查询。</p>
              )}
              {result?.state === "published" && (
                <>
                  <p role="status" className="font-medium">
                    备份已发布 · {result.segments} 个分段 · {backupSize(result.encrypted_bytes)}
                  </p>
                  <p className="text-muted-foreground">
                    {result.files
                      ? `配套文件已保存 ${result.files.saved} 项，${result.files.unavailable} 项不可用。`
                      : "这份备份暂无配套文件清单。"}
                  </p>
                  <dl className="space-y-1">
                    <div>
                      <dt className="text-muted-foreground">档案代次</dt>
                      <dd className="break-all font-mono text-xs">{result.generation}</dd>
                    </div>
                    <div>
                      <dt className="text-muted-foreground">本机目录</dt>
                      <dd className="select-text break-all font-mono text-xs">{result.directory}</dd>
                    </div>
                  </dl>
                  <button
                    type="button"
                    className="hb-btn-outline"
                    disabled={busy}
                    onClick={() => void copyDirectory(result.directory)}
                  >
                    <Copy size={14} aria-hidden="true" />
                    复制目录
                  </button>
                  {copyFeedback && <p role="status">{copyFeedback}</p>}
                  {dialogOpen && <AssistantArchiveFiles key={result.backup_id} backupId={result.backup_id} />}
                </>
              )}
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            {canCreate && (
              <button
                type="button"
                className="hb-btn-primary"
                disabled={busy || storageError}
                onClick={() => create()}
              >
                {tracked ? "另存一份备份" : "创建加密备份"}
              </button>
            )}
            {canContinue && (
              <button
                type="button"
                className="hb-btn-outline"
                disabled={busy || storageError}
                onClick={() => create(tracked)}
              >
                用原编号继续创建
              </button>
            )}
            {view && (
              <button
                type="button"
                className="hb-btn-outline"
                disabled={busy}
                onClick={() => void run("status", view.id)}
              >
                查询状态
              </button>
            )}
            {tracked && !current && (
              <button
                type="button"
                className="hb-btn-outline"
                disabled={busy}
                onClick={() => void run("status", tracked)}
              >
                查询最近备份
              </button>
            )}
          </div>
          <details className="text-sm text-muted-foreground">
            <summary className="cursor-pointer">按编号查询</summary>
            <form
              className="mt-2 flex flex-wrap items-center gap-2"
              onSubmit={(event) => {
                event.preventDefault();
                if (!busy && isArchiveBackupId(lookup)) void run("status", lookup);
              }}
            >
              <input
                aria-label="要查询的备份编号"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="备份 UUID"
                className="w-full min-w-0 flex-1 rounded-md border border-input bg-background px-3 py-2 text-foreground"
              />
              <button type="submit" className="hb-btn-outline" disabled={busy || !isArchiveBackupId(lookup)}>
                查询备份
              </button>
            </form>
          </details>
        </div>
      </dialog>
    </>
  );
}
