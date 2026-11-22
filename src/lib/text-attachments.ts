import type { TextAttachment } from "./assistant-drafts";

function readLocalFile(file: File, signal: AbortSignal): Promise<ArrayBuffer> {
  signal.throwIfAborted();
  const reader = new FileReader();
  const abort = () => reader.abort();
  return new Promise<ArrayBuffer>((resolve, reject) => {
    reader.onload = () => {
      if (reader.result instanceof ArrayBuffer) resolve(reader.result);
      else reject(Error(`“${file.name}”未返回完整的文件内容。`));
    };
    reader.onerror = () => reject(reader.error ?? Error(`无法读取“${file.name}”。`));
    reader.onabort = () => reject(signal.reason ?? Error("附件读取已取消。"));
    signal.addEventListener("abort", abort, { once: true });
    reader.readAsArrayBuffer(file);
  }).finally(() => signal.removeEventListener("abort", abort));
}

// No upload or model call. The caller invalidates reads on cancel/unmount.
export async function readTextAttachments(
  files: readonly File[],
  signal: AbortSignal,
  check: () => void,
): Promise<TextAttachment[]> {
  const result: TextAttachment[] = [];
  for (const file of files) {
    check();
    if (!/\.(txt|md|markdown|csv|json|log|yaml|yml|tsv)$/i.test(file.name))
      throw Error(
        `“${file.name}”暂不支持。可添加文本文件、PDF 或 PNG。`,
      );
    if (file.size === 0 || file.size > 16 * 1024)
      throw Error(`“${file.name}”须为非空文本，且不超过 16 KiB。`);
    const bytes = await readLocalFile(file, signal);
    check();
    const content = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    if (
      bytes.byteLength !== file.size ||
      !content.length ||
      content.includes("\0")
    )
      throw Error(`“${file.name}”不是可读取的文本。`);
    result.push({ name: file.name, content });
  }
  return result;
}
