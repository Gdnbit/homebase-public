import { useRef, useState } from "react";
import {
  archiveBackupDraft,
  archiveBackupFiles,
  archiveFilePageSize,
  recoverArchiveFile,
  type ArchiveFile,
  type ArchiveFilePage,
} from "@/lib/assistant-archive-backup";

const kindLabel: Record<ArchiveFile["kind"], string> = {
  document: "PDF 原件",
  artifact: "任务产物",
  draft: "桌面草稿",
  recording: "录音",
  image: "PNG 图片",
};
const stateLabel: Record<ArchiveFile["state"], string> = {
  saved: "已保存",
  unavailable: "不可用",
  changed: "原件已变化",
};

type Draft = Awaited<ReturnType<typeof archiveBackupDraft>>;
type FileView = { file: ArchiveFile; draft?: Draft; path?: string; error?: string };

export function AssistantArchiveFiles({ backupId }: { backupId: string }) {
  const pending = useRef(false);
  const [expanded, setExpanded] = useState(false);
  const [pages, setPages] = useState<Record<number, ArchiveFilePage>>({});
  const [offset, setOffset] = useState(0);
  const [busy, setBusy] = useState("");
  const [pageError, setPageError] = useState(false);
  const [selected, setSelected] = useState<FileView | null>(null);
  const page = pages[offset];

  async function run(message: string, action: () => Promise<void>) {
    if (pending.current) return;
    pending.current = true;
    setBusy(message);
    try {
      await action();
    } finally {
      pending.current = false;
      setBusy("");
    }
  }

  function loadPage(next: number) {
    if (pending.current) return;
    setSelected(null);
    setPageError(false);
    setOffset(next);
    if (pages[next]) return;
    void run("正在读取文件清单…", async () => {
      try {
        const result = await archiveBackupFiles(backupId, next);
        setPages((current) => ({ ...current, [next]: result }));
      } catch {
        setPageError(true);
      }
    });
  }

  function openDraft(file: ArchiveFile) {
    if (pending.current) return;
    if (selected?.file.id === file.id && selected.draft) {
      setSelected(null);
      return;
    }
    setSelected({ file });
    void run("正在读取备份草稿…", async () => {
      try {
        setSelected({ file, draft: await archiveBackupDraft(backupId, file) });
      } catch {
        setSelected({ file, error: "暂时无法读取这份草稿，请重试。" });
      }
    });
  }

  function recover(file: ArchiveFile) {
    if (pending.current) return;
    const draft = selected?.file.id === file.id ? selected.draft : undefined;
    setSelected({ file, draft });
    void run("正在核验并取回原件…", async () => {
      try {
        setSelected({ file, draft, path: await recoverArchiveFile(backupId, file) });
      } catch (error) {
        const message = String(error);
        setSelected({
          file, draft, error: message.includes("ARCHIVE_CHANGED:")
            ? "备份原件或取回的副本与清单不一致。现有文件已保留。"
            : message.includes("ARCHIVE_BACKUP_WAIT_TIMEOUT")
              ? "取回结果待核对。可对这一项再次取回，系统会核验已有副本。"
              : "暂时无法取回这项原件，请核对备份后重试。",
        });
      }
    });
  }

  return (
    <section className="space-y-2 border-t border-border pt-3" aria-label="配套文件">
      <button
        type="button"
        className="hb-btn-outline"
        aria-expanded={expanded}
        aria-controls="archive-backup-files"
        disabled={!!busy}
        onClick={() => {
          setExpanded(!expanded);
          if (expanded) setSelected(null);
          else if (!page && !pageError) loadPage(offset);
        }}
      >
        {expanded ? "收起文件清单" : "查看文件清单"}
      </button>
      <div id="archive-backup-files" hidden={!expanded} className="space-y-2">
        {busy && <p role="status">{busy}</p>}
        {pageError && (
          <div className="space-y-2">
            <p role="alert" className="text-destructive">暂时无法读取文件清单。</p>
            <div className="flex flex-wrap gap-2">
              <button type="button" className="hb-btn-outline" onClick={() => loadPage(offset)}>重试读取</button>
              {offset > 0 && (
                <button type="button" className="hb-btn-outline"
                  onClick={() => loadPage(Math.max(0, offset - archiveFilePageSize))}>上一页</button>
              )}
            </div>
          </div>
        )}
        {page && (
          <>
            {!page.indexed && <p className="text-muted-foreground">这份旧备份没有配套文件清单。</p>}
            {page.indexed && !page.drafts_indexed && (
              <p className="text-muted-foreground">这份备份创建时尚未收录桌面草稿和录音。</p>
            )}
            {!!page.unreadable_drafts && (
              <p className="text-muted-foreground">{page.unreadable_drafts} 份草稿无法核验。</p>
            )}
            {page.indexed && page.total === 0 && <p className="text-muted-foreground">清单中暂无可列出的文件。</p>}
            {page.items.length > 0 && (
              <ul className="space-y-2">
                {page.items.map((file) => {
                  const name = file.kind !== "draft" ? file.name
                    : file.original_id === "new" ? "新会话草稿" : kindLabel.draft;
                  const item = selected?.file.id === file.id ? selected : null;
                  return (
                    <li key={file.id} className="min-w-0 space-y-2 rounded-md border border-border p-2.5">
                      <p className="break-all font-medium">{name}</p>
                      <p className="text-xs text-muted-foreground">{kindLabel[file.kind]} · {stateLabel[file.state]}</p>
                      {file.state === "saved" && (
                        <div className="flex flex-wrap gap-2">
                          {file.kind === "draft" && (
                            <button type="button" className="hb-btn-outline" disabled={!!busy}
                              aria-expanded={!!item?.draft} onClick={() => openDraft(file)}>
                              {item?.draft ? "收起草稿" : "只读查看草稿"}
                            </button>
                          )}
                          <button type="button" className="hb-btn-outline" disabled={!!busy || !!item?.path}
                            aria-label={`取回原件 ${name}`} onClick={() => recover(file)}>
                            {item?.path ? "已取回" : "取回原件"}
                          </button>
                        </div>
                      )}
                      {item?.draft && <DraftPreview draft={item.draft} imagesIndexed={page.images_indexed === true} />}
                      {item?.error && <p role="alert" className="text-destructive">{item.error}</p>}
                      {item?.path && (
                        <p role="status" className="text-muted-foreground">
                          已取回到 <span className="select-text break-all font-mono text-xs">{item.path}</span>
                        </p>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
            {(offset > 0 || page.next_offset !== null) && (
              <div className="flex flex-wrap items-center gap-2">
                <button type="button" className="hb-btn-outline" disabled={!!busy || offset === 0}
                  onClick={() => loadPage(Math.max(0, offset - archiveFilePageSize))}>上一页</button>
                <span className="text-xs text-muted-foreground">{offset + 1}–{offset + page.items.length} / {page.total}</span>
                <button type="button" className="hb-btn-outline" disabled={!!busy || page.next_offset === null}
                  onClick={() => loadPage(page.next_offset ?? offset)}>下一页</button>
              </div>
            )}
            {page.items.some((file) => file.state === "saved") && (
              <p className="text-xs text-muted-foreground">
                取回的副本保存在备份目录中。草稿保留密文，其他原件按原内容写出。
              </p>
            )}
          </>
        )}
      </div>
    </section>
  );
}

function DraftPreview({ draft, imagesIndexed }: { draft: Draft; imagesIndexed: boolean }) {
  const pending = draft.pending?.request ?? draft.native_pending;
  const pendingImage = draft.pending?.request.image;
  return (
    <div className="min-w-0 space-y-2 rounded-md bg-muted/50 p-2.5">
      <p className="break-all text-xs text-muted-foreground">草稿版本 {draft.revision} · {draft.scope}</p>
      <div>
        <p className="font-medium">已保存草稿</p>
        <p className="whitespace-pre-wrap break-words">{draft.body.text || "草稿正文为空。"}</p>
        {draft.body.image && <p className="break-all text-xs text-muted-foreground">PNG 附件 · {draft.body.image.name}</p>}
      </div>
      {pending && (
        <div>
          <p className="font-medium">原待核对请求</p>
          <p className="break-all font-mono text-xs text-muted-foreground">{pending.request_id}</p>
          <p className="whitespace-pre-wrap break-words">{pending.text}</p>
          {pendingImage && <p className="break-all text-xs text-muted-foreground">原 PNG 附件 · {pendingImage.name}</p>}
        </div>
      )}
      {!!draft.voices?.length && <p className="text-xs text-muted-foreground">包含 {draft.voices.length} 条录音回执。</p>}
      {(draft.body.image || pendingImage) && !imagesIndexed && (
        <p className="text-xs text-muted-foreground">这份备份没有图片清单，无法从中取回 PNG。</p>
      )}
    </div>
  );
}
