import { assistantApi } from "./assistant";
import { prepareHandoffDraft, clearHandoffCopy } from "./assistant-drafts";
import { inputBytes, MAX_DRAFT_BYTES } from "./chat-input-limits";

export async function prepareAssistantHandoff(text: string, requestId: string) {
  if (typeof text !== "string" || !text.trim())
    throw Error("先写下希望助理处理的内容。");
  if (inputBytes(text) > MAX_DRAFT_BYTES)
    throw Error("交接内容超过64 KiB，原内容保留；请缩短或分开处理。");
  const outbox = `handoff:${requestId}`;
  await prepareHandoffDraft(outbox, text);
  const session = await assistantApi.create(requestId);
  if (session.id !== requestId || session.source !== "desktop")
    throw Error("创建会话的标识或来源未确认，未写入其他会话。");
  if (session.message_count > 0) return session.id;
  await prepareHandoffDraft(session.id, text);
  // A failed cleanup keeps a protected copy; it must never undo a saved target.
  await clearHandoffCopy(outbox, text).catch(() => {});
  return session.id;
}

export function incomingDraftText(
  old: string,
  incoming: unknown,
  blocked: boolean,
) {
  if (typeof incoming !== "string")
    throw Error("转入内容格式无效，当前草稿保留。");
  if (blocked)
    throw Error("原请求或录音尚待核对，转入内容未合并；可单独保存为助理草稿。");
  const combined = old && old !== incoming ? `${old}\n\n${incoming}` : incoming;
  if (inputBytes(combined) > MAX_DRAFT_BYTES)
    throw Error("合并后超过64 KiB，当前草稿未修改；请单独保存或缩短转入内容。");
  return combined;
}
