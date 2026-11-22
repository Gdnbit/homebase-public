import { invoke } from "@tauri-apps/api/core";

export const archiveBackupIdKey = "homebase.assistant.archive-backup-id";
export const isArchiveBackupId = (value: unknown): value is string =>
  typeof value === "string" &&
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value);

type PublishedBackup = {
  state: "published";
  backup_id: string;
  directory: string;
  generation: string;
  segments: number;
  encrypted_bytes: number;
  files?: { saved: number; unavailable: number };
};

export type BackupStatus =
  | PublishedBackup
  | { state: "not_found" | "incomplete"; backup_id: string };

function nonnegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

async function archiveRequest<T>(command: string, args: Record<string, unknown>) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      invoke<T>(command, args),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(Error("ARCHIVE_BACKUP_WAIT_TIMEOUT")), 15_000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export async function archiveBackup(
  operation: "create" | "status",
  backupId: string,
): Promise<BackupStatus> {
  const result = await archiveRequest<Partial<Omit<PublishedBackup, "state">> & { state?: unknown }>(
    operation === "create" ? "assistant_archive_backup" : "assistant_archive_backup_get",
    { backupId },
  );
  if (result?.backup_id === backupId) {
    if (result.state === "not_found" || result.state === "incomplete") {
      if (operation === "status") return { state: result.state, backup_id: backupId };
    } else if (
      result.state === "published" &&
      typeof result.directory === "string" && result.directory.trim() &&
      isArchiveBackupId(result.generation) &&
      nonnegativeInteger(result.segments) &&
      nonnegativeInteger(result.encrypted_bytes) &&
      (result.files == null || (
        nonnegativeInteger(result.files.saved) && nonnegativeInteger(result.files.unavailable)
      ))
    ) {
      return {
        state: "published",
        backup_id: backupId,
        directory: result.directory,
        generation: result.generation,
        segments: result.segments,
        encrypted_bytes: result.encrypted_bytes,
        files: result.files ?? undefined,
      };
    }
  }
  throw Error("ARCHIVE_BACKUP_INVALID_RECEIPT");
}

const fileKinds = ["document", "artifact", "draft", "recording", "image"] as const;
const fileStates = ["saved", "unavailable", "changed"] as const;
export const archiveFilePageSize = 20;

export type ArchiveFile = {
  id: string;
  kind: (typeof fileKinds)[number];
  state: (typeof fileStates)[number];
  name: string;
  original_id: string;
  sha256: string;
};

export type ArchiveFilePage = {
  indexed: boolean;
  drafts_indexed?: boolean;
  images_indexed?: boolean;
  unreadable_drafts?: number;
  total: number;
  items: ArchiveFile[];
  next_offset: number | null;
};

function validFile(file: ArchiveFile) {
  return file && isArchiveBackupId(file.id) && fileKinds.includes(file.kind) &&
    fileStates.includes(file.state) && typeof file.name === "string" && file.name.length > 0 &&
    typeof file.original_id === "string" && typeof file.sha256 === "string" &&
    /^[0-9a-f]{64}$/.test(file.sha256);
}

export async function archiveBackupFiles(backupId: string, offset: number): Promise<ArchiveFilePage> {
  const page = await archiveRequest<ArchiveFilePage & { backup_id: string }>(
    "assistant_archive_backup_files", { backupId, offset },
  );
  if (
    page?.backup_id !== backupId || typeof page.indexed !== "boolean" ||
    !nonnegativeInteger(page.total) || page.total < offset || (!page.indexed && page.total !== 0) ||
    (page.drafts_indexed !== undefined && typeof page.drafts_indexed !== "boolean") ||
    (page.images_indexed !== undefined && typeof page.images_indexed !== "boolean") ||
    (page.unreadable_drafts !== undefined && !nonnegativeInteger(page.unreadable_drafts)) ||
    !Array.isArray(page.items) || page.items.length !== Math.min(archiveFilePageSize, page.total - offset) ||
    !page.items.every(validFile) || new Set(page.items.map((file) => file.id)).size !== page.items.length ||
    (!page.images_indexed && page.items.some((file) => file.kind === "image"))
  ) throw Error("ARCHIVE_BACKUP_INVALID_FILE_PAGE");

  const end = offset + page.items.length;
  if (page.next_offset !== (end < page.total ? end : null)) throw Error("ARCHIVE_BACKUP_INVALID_FILE_PAGE");
  return page;
}

type ArchivedImage = { scope: string; name: string; sha256: string; bytes: number };
type ArchivedDraft = {
  scope: string;
  revision: number;
  body: { text: string; image?: ArchivedImage };
  pending?: { request: { text: string; request_id: string; image?: ArchivedImage } } | null;
  native_pending?: { text: string; request_id: string } | null;
  voices?: unknown[];
};

function validDraftImage(image: ArchivedImage | undefined, scope: string) {
  return image === undefined || !!image && image.scope === scope &&
    typeof image.name === "string" && /^[^/\\:\p{Cc}]*\.png$/iu.test(image.name) &&
    typeof image.sha256 === "string" && /^[0-9a-f]{64}$/.test(image.sha256) &&
    nonnegativeInteger(image.bytes) && image.bytes > 0 && image.bytes <= 384 * 1024;
}

export async function archiveBackupDraft(backupId: string, file: ArchiveFile): Promise<ArchivedDraft> {
  const view = await archiveRequest<{ backup_id: string; file_id: string; entry: ArchivedDraft }>(
    "assistant_archive_backup_draft_get", { backupId, fileId: file.id },
  );
  const draft = view?.entry;
  const pending = draft?.pending?.request ?? draft?.native_pending;
  if (
    view?.backup_id !== backupId || view.file_id !== file.id || draft?.scope !== file.original_id ||
    !nonnegativeInteger(draft.revision) || draft.revision === 0 || typeof draft.body?.text !== "string" ||
    !validDraftImage(draft.body.image, draft.scope) || !validDraftImage(draft.pending?.request?.image, draft.scope) ||
    ((draft.pending || draft.native_pending) && (typeof pending?.text !== "string" || typeof pending?.request_id !== "string")) ||
    (draft.voices !== undefined && !Array.isArray(draft.voices))
  ) throw Error("ARCHIVE_BACKUP_INVALID_DRAFT");
  return draft;
}

export async function recoverArchiveFile(backupId: string, file: ArchiveFile): Promise<string> {
  const reply = await archiveRequest<{ backup_id: string; file: ArchiveFile; path: string }>(
    "assistant_archive_backup_file_recover", { backupId, fileId: file.id },
  );
  if (
    reply?.backup_id !== backupId || !validFile(reply.file) || reply.file.id !== file.id ||
    reply.file.kind !== file.kind || reply.file.state !== "saved" || reply.file.sha256 !== file.sha256 ||
    typeof reply.path !== "string" || !reply.path.trim()
  ) throw Error("ARCHIVE_BACKUP_INVALID_FILE_RECEIPT");
  return reply.path;
}
