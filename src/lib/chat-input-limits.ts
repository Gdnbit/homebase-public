// Match the protected desktop store and the execution adapters respectively.
// JS string length counts UTF-16 units; wire budgets count UTF-8 bytes.
export const MAX_DRAFT_BYTES = 64 * 1024;
export const MAX_MESSAGE_BYTES = 8000;
export const inputBytes = (text: string) =>
  new TextEncoder().encode(text).length;
export function utf8Prefix(text: string, bytes: number) {
  let size = 0,
    result = "";
  for (const character of text) {
    const next = inputBytes(character);
    if (size + next > bytes) break;
    result += character;
    size += next;
  }
  return result;
}
export function inputBudgetMessage(bytes: number, attachments = true) {
  if (bytes > MAX_DRAFT_BYTES)
    return `草稿 ${bytes.toLocaleString()} 字节，超过 64 KiB，尚未保存。请缩短内容；已保存的原稿保留。`;
  if (bytes > MAX_MESSAGE_BYTES)
    return `草稿 ${bytes.toLocaleString()} 字节，发送上限为 8,000 字节。${attachments ? "请缩短内容或分成文本附件后发送。" : "请缩短内容或分段发送。"}`;
  return "";
}
