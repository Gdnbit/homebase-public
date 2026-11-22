import { invoke } from "@tauri-apps/api/core";
import type { TextAttachment } from "./assistant-drafts";
export interface DocumentMeta {
  id: string;
  origin_scope: string;
  name: string;
  source_sha256: string;
  source_bytes: number;
}
export interface UploadStatus {
  document_id: string;
  scope: string;
  name: string;
  source_sha256: string;
  source_bytes: number;
  parts: number;
  received: number[];
  missing: number[];
  bytes_received: number;
  complete: boolean;
  committed: boolean;
}
export interface DocumentIndex {
  documents: DocumentMeta[];
  uploads: UploadStatus[];
  unreadable: number;
  complete: boolean;
}
export interface PageRange {
  from: number;
  to: number;
}
export type DocumentPhase =
  | "processing"
  | "cancel_requested"
  | "ready"
  | "no_text"
  | "awaiting_pages"
  | "failed"
  | "cancelled"
  | "unknown";
export interface Extraction {
  document_id: string;
  id: string;
  attempt: string;
  revision: number;
  pages: PageRange | null;
  phase: DocumentPhase;
  code: string | null;
  message: string;
  total_pages: number | null;
  text_bytes: number | null;
  partial: boolean | null;
  missing_text_pages: number[];
}
export interface DocumentDetails {
  document: DocumentMeta;
  extractions: Extraction[];
}
export interface DocumentStart {
  status: string;
  extraction: Extraction | null;
  message: string;
}
export interface DocumentResult {
  extraction: Extraction;
  error: string | null;
  message: string;
  result: null | {
    status: string;
    message: string;
    total_pages: number | null;
    page_from: number | null;
    page_to: number | null;
    pages:
      null | { number: number; text: string; bytes: number; empty: boolean }[];
    text_bytes: number | null;
    partial: boolean | null;
    missing_text_pages: number[] | null;
  };
}
export const documentKey = (scope: string) =>
  ["assistant-documents", scope] as const;
export const documentsApi = {
  index: (scope: string) =>
    invoke<DocumentIndex>("assistant_document_index", { scope }),
  get: (scope: string, documentId: string) =>
    invoke<DocumentDetails>("assistant_document_get", { scope, documentId }),
  part: (request: Record<string, unknown>) =>
    invoke<UploadStatus>("assistant_document_import", { request }),
  commit: (scope: string, documentId: string) =>
    invoke<DocumentMeta>("assistant_document_import_commit", {
      scope,
      documentId,
    }),
  extract: (
    scope: string,
    documentId: string,
    pages: PageRange | null,
    retry = false,
  ) =>
    invoke<DocumentStart>("assistant_document_extract", {
      scope,
      documentId,
      pages,
      retry,
    }),
  status: (scope: string, documentId: string, extractionId: string) =>
    invoke<Extraction>("assistant_document_status", {
      scope,
      documentId,
      extractionId,
    }),
  result: (scope: string, documentId: string, extractionId: string) =>
    invoke<DocumentResult>("assistant_document_result", {
      scope,
      documentId,
      extractionId,
    }),
  cancel: (scope: string, documentId: string, extractionId: string) =>
    invoke<DocumentStart>("assistant_document_cancel", {
      scope,
      documentId,
      extractionId,
    }),
  excerpt: (
    scope: string,
    documentId: string,
    extractionId: string,
    pages: PageRange,
    textFrom: number | null,
    textTo: number | null,
  ) =>
    invoke<TextAttachment>("assistant_document_excerpt", {
      scope,
      documentId,
      extractionId,
      pages,
      textFrom,
      textTo,
    }),
};
export const documentActive = (phase?: string) =>
  phase === "processing" || phase === "cancel_requested";
export const documentPhase: Record<DocumentPhase, string> = {
  processing: "正在提取",
  cancel_requested: "正在确认停止",
  ready: "已提取文字",
  no_text: "没有提取到文字",
  awaiting_pages: "请选择页范围",
  failed: "提取失败",
  cancelled: "已停止",
  unknown: "原结果待核对",
};

const importingScopes = new Set<string>();

/** Re-selecting the exact source resumes only its missing chunks. No parse/send. */
export async function importDocument(
  file: File,
  scope: string,
  {
    check,
    progress,
    beforeWrite,
  }: {
    check: () => void;
    progress: (saved: number, total: number) => void;
    beforeWrite: () => void;
  },
) {
  if (
    !/\.pdf$/i.test(file.name) ||
    file.size < 1 ||
    file.size > 4 * 1024 * 1024
  )
    throw Error("请选择非空 PDF，文件不能超过 4 MiB。");
  if (
    new TextEncoder().encode(file.name).length > 180 ||
    [...file.name].some(
      (c) =>
        c.charCodeAt(0) < 32 ||
        (c.charCodeAt(0) >= 127 && c.charCodeAt(0) <= 159) ||
        "/\\:".includes(c),
    )
  )
    throw Error("PDF 名称须不含路径或控制字符，且至多 180 字节。");
  // The same input can remount while a previous window is still saving a chunk.
  if (importingScopes.has(scope))
    throw Error("这条会话的上一份 PDF 仍在保存，请稍后重新选择。");
  importingScopes.add(scope);
  let loadedBytes: Uint8Array | undefined;
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    loadedBytes = bytes;
    check();
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    const sha = [...new Uint8Array(digest)]
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
    check();
    const index = await documentsApi.index(scope);
    check();
    if (!index.complete)
      throw Error("文档目录有不可读记录，请先核对；未另建来源或覆盖。");
    const match = (v: {
      name: string;
      source_sha256: string;
      source_bytes: number;
    }) =>
      v.name === file.name &&
      v.source_sha256 === sha &&
      v.source_bytes === bytes.length;
    const existing = index.documents.find(match);
    if (existing) return existing;
    const id = index.uploads.find(match)?.document_id || crypto.randomUUID();
    const total = Math.ceil(bytes.length / (256 * 1024));
    let missing = Array.from({ length: total }, (_, i) => i);
    const pending = index.uploads.find((v) => v.document_id === id);
    if (pending) missing = pending.missing;
    progress(total - missing.length, total);
    for (const part of missing) {
      check();
      const chunk = bytes.subarray(
        part * 256 * 1024,
        Math.min((part + 1) * 256 * 1024, bytes.length),
      );
      // Avoid spreading 256KiB into a function's argument stack.
      let binary = "";
      for (let i = 0; i < chunk.length; i += 8192)
        binary += String.fromCharCode(...chunk.subarray(i, i + 8192));
      beforeWrite();
      const status = await documentsApi.part({
        scope,
        document_id: id,
        name: file.name,
        source_sha256: sha,
        source_bytes: bytes.length,
        part,
        data_base64: btoa(binary),
      });
      check();
      if (
        status.document_id !== id ||
        status.source_sha256 !== sha ||
        status.source_bytes !== bytes.length
      )
        throw Error("分块回执与原文件不一致，未继续提交。");
      progress(status.received.length, total);
    }
    check();
    beforeWrite();
    const committed = await documentsApi.commit(scope, id);
    check();
    if (committed.id !== id || committed.source_sha256 !== sha)
      throw Error("来源提交回执无法核对，请保留原编号查询。");
    return committed;
  } finally {
    loadedBytes?.fill(0);
    importingScopes.delete(scope);
  }
}
