import { useCallback, useEffect, useRef, useState } from "react";
import { Download, FileText, FileVideo, PanelRightClose } from "lucide-react";
import { MarkdownRenderer } from "@/components/markdown-renderer";
import { assistantApi, type AssistantArtifact } from "@/lib/assistant";

const button =
  "inline-flex items-center justify-center gap-2 rounded-lg border border-border px-3 py-2 text-sm hover:bg-secondary disabled:opacity-50";
type Preview = { artifact: AssistantArtifact } & (
  | { status: "loading" }
  | { status: "failed"; message: string }
  | { status: "ready"; content: string; markdown: boolean }
  | { status: "image"; url: string }
);
type ExportStatus = { id: string } & (
  | { status: "pending" | "saved" }
  | { status: "failed"; message: string }
);

function imageType(name: string) {
  if (/\.png$/i.test(name)) return "image/png";
  if (/\.jpe?g$/i.test(name)) return "image/jpeg";
  if (/\.webp$/i.test(name)) return "image/webp";
  return null;
}

export function AssistantArtifacts({
  artifacts,
}: {
  artifacts: AssistantArtifact[];
}) {
  const [preview, setPreview] = useState<Preview | null>(null);
  const [openingId, setOpeningId] = useState<string | null>(null);
  const [openError, setOpenError] = useState<{
    id: string;
    message: string;
  } | null>(null);
  const [imageHandedOff, setImageHandedOff] = useState<string | null>(null);
  const [exportStatus, setExportStatus] = useState<ExportStatus | null>(null);
  const previewRequest = useRef(0);
  const opening = useRef(false);
  const exporting = useRef(false);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const closeButton = useRef<HTMLButtonElement | null>(null);
  const imageUrl = useRef<string | null>(null);
  const releaseImage = useCallback(() => {
    if (imageUrl.current) {
      URL.revokeObjectURL(imageUrl.current);
      imageUrl.current = null;
    }
  }, []);

  useEffect(
    () => () => {
      previewRequest.current++;
      releaseImage();
    },
    [releaseImage],
  );

  async function readPreview(artifact: AssistantArtifact) {
    const request = ++previewRequest.current;
    releaseImage();
    setPreview({ artifact, status: "loading" });
    try {
      const mime = imageType(artifact.name);
      if (mime) {
        const bytes = await assistantApi.previewImage(artifact.id);
        if (request !== previewRequest.current) return;
        if (!(bytes instanceof ArrayBuffer) || bytes.byteLength !== artifact.bytes) {
          throw new Error("预览回执与原图片大小不匹配。");
        }
        const digest = await crypto.subtle.digest("SHA-256", bytes);
        if (request !== previewRequest.current) return;
        const sha256 = [...new Uint8Array(digest)]
          .map((byte) => byte.toString(16).padStart(2, "0"))
          .join("");
        if (sha256 !== artifact.sha256) {
          throw new Error("预览回执与原图片摘要不匹配。");
        }
        const url = URL.createObjectURL(new Blob([bytes], { type: mime }));
        imageUrl.current = url;
        setPreview({ artifact, status: "image", url });
        return;
      }
      const result = await assistantApi.preview(artifact.id);
      if (
        result?.name !== artifact.name ||
        result.sha256 !== artifact.sha256 ||
        typeof result.content !== "string" ||
        typeof result.markdown !== "boolean"
      ) {
        throw new Error("预览回执与原产物不匹配，未显示正文。");
      }
      if (request === previewRequest.current) {
        setPreview({
          artifact,
          status: "ready",
          content: result.content,
          markdown: result.markdown,
        });
      }
    } catch (error) {
      if (request === previewRequest.current) {
        setPreview({
          artifact,
          status: "failed",
          message: error instanceof Error ? error.message : String(error),
        });
      }
    }
  }

  function closePreview() {
    previewRequest.current++;
    releaseImage();
    setPreview(null);
    trigger.current?.focus();
    trigger.current = null;
  }

  async function openResult(artifact: AssistantArtifact) {
    if (opening.current) return;
    opening.current = true;
    setOpeningId(artifact.id);
    setOpenError(null);
    setImageHandedOff(null);
    try {
      await assistantApi.open(artifact.id);
      if (imageType(artifact.name)) setImageHandedOff(artifact.id);
    } catch (error) {
      setOpenError({
        id: artifact.id,
        message: error instanceof Error ? error.message : String(error),
      });
    } finally {
      opening.current = false;
      setOpeningId(null);
    }
  }

  async function exportResult(artifact: AssistantArtifact) {
    if (exporting.current) return;
    exporting.current = true;
    setExportStatus({ id: artifact.id, status: "pending" });
    try {
      const receipt = await assistantApi.export(artifact.id);
      if (receipt === null) {
        setExportStatus(null);
        return;
      }
      if (
        receipt?.name !== artifact.name ||
        receipt.sha256 !== artifact.sha256 ||
        receipt.bytes !== artifact.bytes
      ) {
        throw new Error("导出回执与原产物不匹配，请检查所选目录。");
      }
      setExportStatus({ id: artifact.id, status: "saved" });
    } catch (error) {
      setExportStatus({
        id: artifact.id,
        status: "failed",
        message: error instanceof Error ? error.message : String(error),
      });
    } finally {
      exporting.current = false;
    }
  }

  return (
    <>
      {artifacts.map((artifact) => (
        <div key={artifact.id} className="rounded-lg border border-border p-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="min-w-0">
              <p className="flex items-center gap-2 break-all">
                {artifact.mime_type?.startsWith("video/") ? (
                  <FileVideo size={18} className="shrink-0" />
                ) : (
                  <FileText size={18} className="shrink-0" />
                )}
                {artifact.name}
              </p>
              <p className="mt-1 text-xs text-muted-foreground">
                {artifact.bytes.toLocaleString()} 字节 · 已核对 SHA-256
              </p>
            </div>
            <div className="flex flex-wrap gap-2">
              {(imageType(artifact.name) || /\.(md|markdown|txt|json|csv|srt)$/i.test(artifact.name)) && (
                <button
                  type="button"
                  className={button}
                  onClick={(event) => {
                    if (
                      preview?.artifact.id === artifact.id &&
                      preview.status === "loading"
                    )
                      return;
                    trigger.current = event.currentTarget;
                    void readPreview(artifact);
                    closeButton.current?.focus();
                  }}
                >
                  预览
                </button>
              )}
              <button
                type="button"
                className={button}
                disabled={openingId !== null}
                onClick={() => void openResult(artifact)}
              >
                {openingId === artifact.id ? "正在打开…" : "打开结果"}
              </button>
              <button
                type="button"
                className={button}
                disabled={exportStatus?.status === "pending"}
                onClick={() => void exportResult(artifact)}
              >
                <Download size={16} aria-hidden="true" />
                {exportStatus?.id === artifact.id && exportStatus.status === "pending"
                  ? "正在导出…"
                  : "导出副本"}
              </button>
            </div>
          </div>
          {openError?.id === artifact.id && (
            <p role="alert" className="mt-2 text-xs text-destructive">
              打开结果失败。{openError.message}
            </p>
          )}
          {imageHandedOff === artifact.id && (
            <p role="status" className="mt-2 text-xs text-muted-foreground">
              已交给系统应用。若窗口未显示，可点「预览」查看图片。
            </p>
          )}
          {exportStatus?.id === artifact.id && exportStatus.status === "saved" && (
            <p role="status" className="mt-2 text-xs text-muted-foreground">
              副本已保存到所选目录。
            </p>
          )}
          {exportStatus?.id === artifact.id && exportStatus.status === "failed" && (
            <p role="alert" className="mt-2 break-words text-xs text-destructive">
              {exportStatus.message}
            </p>
          )}
        </div>
      ))}
      {preview && (
        <aside
          aria-label="结果预览"
          className="chat-preview"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              closePreview();
            }
          }}
        >
          <div className="mb-5 flex items-center justify-between gap-4">
            <h2 className="min-w-0 break-all font-semibold">
              {preview.artifact.name}
            </h2>
            <button
              ref={closeButton}
              type="button"
              autoFocus
              className={button + " shrink-0"}
              aria-label="关闭预览"
              onClick={closePreview}
            >
              <PanelRightClose size={16} />
              关闭预览
            </button>
          </div>
          {preview.status === "loading" && <p role="status">正在读取原结果…</p>}
          {preview.status === "failed" && (
            <div className="space-y-3">
              <p role="alert" className="break-words text-destructive">
                预览读取失败。{preview.message}
              </p>
              <button
                type="button"
                className={button}
                onClick={() => {
                  closeButton.current?.focus();
                  void readPreview(preview.artifact);
                }}
              >
                重试预览
              </button>
            </div>
          )}
          {preview.status === "image" && (
            <img
              alt={preview.artifact.name}
              className="h-auto max-w-full rounded-lg"
              src={preview.url}
              onError={() => {
                if (imageUrl.current !== preview.url) return;
                releaseImage();
                setPreview({
                  artifact: preview.artifact,
                  status: "failed",
                  message: "无法解码这张图片。可以重试或导出副本。",
                });
              }}
            />
          )}
          {preview.status === "ready" &&
            (!preview.content.trim() ? (
              <p role="status">结果文件没有可预览的文本。</p>
            ) : preview.markdown ? (
              <MarkdownRenderer source={preview.content} />
            ) : (
              <pre className="whitespace-pre-wrap break-words text-sm">
                {preview.content}
              </pre>
            ))}
        </aside>
      )}
    </>
  );
}
