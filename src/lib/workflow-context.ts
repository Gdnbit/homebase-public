import format from "../../control/workflow-context-format.json";
import {
  inputBytes,
  MAX_DRAFT_BYTES,
  MAX_MESSAGE_BYTES,
  utf8Prefix,
} from "./chat-input-limits";

export interface InboxContext {
  text: string;
  title: string;
  tags: string;
  source: string;
  attachments: { rel: string; name: string; size: number }[];
  uncertain: boolean;
  receipt?: {
    submission_id: string;
    state: "not_found" | "prepared" | "unknown" | "completed" | "canceled";
    matches_current: boolean;
  };
}
interface StudioContext {
  session_id: string | null;
  task_id: string | null;
  previous_error: string | null;
  total_messages: number;
  selected_messages: number;
  partial: true;
  excerpts: { role: string; content: string; truncated: boolean }[];
}
export type WorkflowPayload = { version: string } & (
  | { workflow: "inbox"; source: InboxContext }
  | { workflow: "studio"; source: StudioContext }
);
export interface WorkflowDraftView {
  task: string;
  frame: string;
  json: string;
  payload: WorkflowPayload;
}
const prefix = format.start + format.intro + format.rules + format.json_marker;
const count = (v: unknown): v is number =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const nullable = (v: unknown) => v === null || typeof v === "string";
function keys(v: unknown, names: string[]): v is Record<string, unknown> {
  return (
    !!v &&
    typeof v === "object" &&
    !Array.isArray(v) &&
    Object.keys(v).length === names.length &&
    names.every((k) => Object.hasOwn(v, k))
  );
}
function metadata(v: unknown): v is WorkflowPayload {
  if (
    !keys(v, ["version", "workflow", "source"]) ||
    v.version !== format.payload_version
  )
    return false;
  const s = v.source;
  if (v.workflow === "inbox")
    return (
      (keys(s, [
        "text",
        "title",
        "tags",
        "source",
        "attachments",
        "uncertain",
      ]) || keys(s, ["text", "title", "tags", "source", "attachments", "uncertain", "receipt"])) &&
      (!Object.hasOwn(s, "receipt") || (keys(s.receipt, ["submission_id", "state", "matches_current"]) && typeof s.receipt.submission_id === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(s.receipt.submission_id) && ["not_found", "prepared", "unknown", "completed", "canceled"].includes(String(s.receipt.state)) && typeof s.receipt.matches_current === "boolean")) &&
      [s.text, s.title, s.tags, s.source].every((x) => typeof x === "string") &&
      typeof s.uncertain === "boolean" &&
      Array.isArray(s.attachments) &&
      s.attachments.every(
        (a) =>
          keys(a, ["rel", "name", "size"]) &&
          typeof a.rel === "string" &&
          typeof a.name === "string" &&
          count(a.size),
      )
    );
  return (
    v.workflow === "studio" &&
    keys(s, [
      "session_id",
      "task_id",
      "previous_error",
      "total_messages",
      "selected_messages",
      "partial",
      "excerpts",
    ]) &&
    [s.session_id, s.task_id, s.previous_error].every(nullable) &&
    count(s.total_messages) &&
    count(s.selected_messages) &&
    s.selected_messages <= 8 &&
    s.selected_messages <= s.total_messages &&
    s.partial === true &&
    Array.isArray(s.excerpts) &&
    s.excerpts.length <= s.selected_messages &&
    s.excerpts.every(
      (e) =>
        keys(e, ["role", "content", "truncated"]) &&
        typeof e.role === "string" &&
        inputBytes(e.role) <= 64 &&
        typeof e.content === "string" &&
        inputBytes(e.content) <= 1200 &&
        typeof e.truncated === "boolean",
    )
  );
}
export function workflowFrame(payload: WorkflowPayload) {
  return prefix + JSON.stringify(payload, null, 2) + format.end;
}
export function readWorkflowDraft(text: string): WorkflowDraftView | null {
  if (inputBytes(text) > MAX_DRAFT_BYTES) return null;
  const start = text.indexOf(format.start);
  if (
    start < 0 ||
    text.indexOf(format.start, start + format.start.length) >= 0 ||
    !text.startsWith(prefix, start) ||
    (start > 0 && !text.slice(0, start).endsWith("\n\n"))
  )
    return null;
  const jsonStart = start + prefix.length,
    end = text.indexOf(format.end, jsonStart);
  if (end < 0 || text.indexOf(format.end, end + format.end.length) >= 0)
    return null;
  try {
    const json = text.slice(jsonStart, end),
      payload: unknown = JSON.parse(json);
    if (!metadata(payload) || JSON.stringify(payload, null, 2) !== json)
      return null;
    const after = text.slice(end + format.end.length);
    if (after && !after.startsWith("\n")) return null;
    const before = start ? text.slice(0, start - 2) : "",
      tail = after ? after.slice(1) : "";
    return {
      task: before + (before && tail ? "\n" : "") + tail,
      frame: text.slice(start, end + format.end.length),
      json,
      payload,
    };
  } catch {
    return null;
  }
}
export function inboxHandoff(
  data: Omit<InboxContext, "uncertain"> & { uncertain?: boolean },
) {
  const task = data.receipt
    ? `请先用 inbox_submission_get 核对原收录 ${data.receipt.submission_id}，不要换编号重复收录。${data.receipt.matches_current ? "当前资料与原提交一致。" : "当前输入已与原提交不同；原回执不代表这些新内容已经收录。请分别核对，再按我的要求整理。"}`
    : data.uncertain
    ? "请整理这份资料；之前提交的结果还没确认，先查已有记录，不要重复收录。"
    : "请帮我整理并收录这份资料，先核对保存位置和已有记录，确认后再保存。";
  // Copy only the documented public source fields. Do not read attachment paths.
  const source: InboxContext = {
    text: data.text,
    title: data.title,
    tags: data.tags,
    source: data.source,
    attachments: data.attachments.map((a) => ({
      rel: a.rel,
      name: a.name,
      size: a.size,
    })),
    uncertain: data.uncertain ?? false,
    ...(data.receipt ? { receipt: { ...data.receipt } } : {}),
  };
  return (
    task +
    "\n\n" +
    workflowFrame({
      version: format.payload_version,
      workflow: "inbox",
      source,
    })
  );
}
export function studioHandoff(data: {
  session: string | null;
  task: string | null;
  input: string;
  messages: { role: string; content: string }[];
  error?: string | null;
}) {
  const goal = data.task
    ? "请跟进这篇文稿的原任务，核对当前进度和结果，不重新生成。"
    : data.error
      ? "请先核对这份文稿之前是否受理、结果是什么，再确定如何继续；不要重复生成。"
      : "请继续完善这份文稿，保存新稿后给我预览和文件；发布前再确认。";
  const task = goal + (data.input ? "\n\n补充要求：\n" + data.input : "");
  const selected = data.messages.slice(-8);
  const source: StudioContext = {
    session_id: data.session,
    task_id: data.task,
    previous_error: data.error ?? null,
    total_messages: data.messages.length,
    selected_messages: selected.length,
    partial: true,
    excerpts: [],
  };
  const encode = () =>
    task +
    "\n\n" +
    workflowFrame({
      version: format.payload_version,
      workflow: "studio",
      source,
    });
  if (inputBytes(encode()) > MAX_MESSAGE_BYTES)
    throw Error(
      "未发送的要求或工作流引用太长，超过8,000字节；原内容保留，未创建新会话。请缩短要求后重试。",
    );
  // Reserve the entire task and source identity before fitting recent excerpts.
  for (const message of [...selected].reverse()) {
    let low = 0,
      high = Math.min(1200, inputBytes(message.content));
    let fit: StudioContext["excerpts"][number] | null = null;
    const original = source.excerpts;
    while (low <= high) {
      const cap = Math.floor((low + high) / 2),
        content = utf8Prefix(message.content, cap);
      const row = {
        role: utf8Prefix(message.role, 64),
        content,
        truncated: content !== message.content,
      };
      source.excerpts = [row, ...original];
      if (inputBytes(encode()) <= MAX_MESSAGE_BYTES) {
        fit = row;
        low = cap + 1;
      } else high = cap - 1;
    }
    source.excerpts = fit?.content ? [fit, ...original] : original;
  }
  return encode();
}
