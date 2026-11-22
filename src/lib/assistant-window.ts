import { invoke } from "@tauri-apps/api/core";
import {
  assistantActive,
  assistantApi,
  type AssistantConversation,
  type AssistantTurn,
} from "./assistant";

export function assistantTurnKey(sessionId: string | null, turnId: string | null) {
  return ["assistant", "turn", turnId, sessionId] as const;
}

export function assistantTurnProgressKey(sessionId: string | null, turnId: string | null) {
  return [...assistantTurnKey(sessionId, turnId), "progress"] as const;
}

export function assistantTurnStepKey(sessionId: string, turnId: string, stepId: string) {
  return [...assistantTurnKey(sessionId, turnId), "step", stepId] as const;
}

export function observedTurn(query: {
  data: AssistantTurn | undefined;
  isError: boolean;
  isFetching: boolean;
}) {
  if (
    query.isError ||
    (query.isFetching && query.data && !assistantActive(query.data.state))
  ) return undefined;
  return query.data;
}

export async function scopedTurn(sessionId: string, turnId: string, includeDetails = true) {
  const result = await assistantApi.turn(turnId, includeDetails);
  if (result.id !== turnId || result.session_id !== sessionId)
    throw new Error(
      "任务详情来源不匹配，未显示其他会话参数或允许确认；请刷新原会话。",
    );
  if (!includeDetails && result.details_omitted !== true)
    throw new Error("进度响应缺少精简标记，请刷新原任务。");
  return result;
}

export async function scopedTurnInspection(sessionId: string, turnId: string) {
  const result = await assistantApi.inspectTurn(sessionId, turnId);
  if (result.id !== turnId || result.session_id !== sessionId)
    throw new Error("任务详情来源不匹配，未显示其他会话结果。");
  const steps = result.step_summaries;
  if (result.details_omitted !== true || !Array.isArray(result.steps) || result.steps.length !== 0 || !Array.isArray(steps)
      || steps.length !== result.step_count || new Set(steps.map(step => step.id)).size !== steps.length)
    throw new Error("步骤清单不完整，请重试读取原任务。");
  return result;
}

export async function scopedTurnStep(sessionId: string, turnId: string, stepId: string) {
  const result = await assistantApi.turnStep(sessionId, turnId, stepId);
  if (result.session_id !== sessionId || result.turn_id !== turnId || result.step?.id !== stepId)
    throw new Error("步骤来源不匹配，未显示其他任务的结果。");
  return result.step;
}

export interface SessionWindow {
  id: string;
  title: string;
  source: string;
  queue_resume_generation?: number;
  messages: AssistantConversation["messages"];
  turns: {
    turn: AssistantTurn;
    details_omitted: boolean;
    step_count: number;
    job_count: number;
    artifact_count: number;
  }[];
  page: {
    version: "homebase-session-window-v1";
    captured_revision: number;
    total_messages: number;
    start_index: number;
    end_index: number;
    returned: number;
    requested_limit: number;
    before_message_id: string | null;
    has_more: boolean;
  };
}
export async function sessionWindow(
  sessionId: string,
  before: string | null,
  limit = 40,
) {
  const result = await invoke<SessionWindow>("assistant_session_window_get", {
    request: { session_id: sessionId, before_message_id: before, limit },
  });
  if (
    result.id !== sessionId ||
    (result.queue_resume_generation !== undefined &&
      (!Number.isSafeInteger(result.queue_resume_generation) ||
        result.queue_resume_generation < 0)) ||
    result.page.version !== "homebase-session-window-v1" ||
    result.page.returned !== result.messages.length ||
    result.page.returned > 40 ||
    result.page.end_index - result.page.start_index !==
      result.messages.length ||
    (before && result.messages.some((message) => message.id === before)) ||
    result.page.has_more !== !!result.page.before_message_id ||
    (result.page.has_more &&
      result.page.before_message_id !== result.messages[0]?.id)
  )
    throw new Error("会话窗口响应不一致，未混入其他历史；请刷新原会话。");
  return result;
}

export interface TimelinePage {
  frames: SessionWindow[];
  page: SessionWindow["page"];
}
// A refreshed newest page retains its previously loaded first ID. Fill only
// the missing prefix using bounded RPCs, rather than sliding old reading away.
export async function timelinePage(
  sessionId: string,
  before: string | null,
  previous?: TimelinePage,
): Promise<TimelinePage> {
  const newest = await sessionWindow(sessionId, before);
  const frames = [newest];
  if (!before && previous) {
    const first = previous.frames[0];
    const target = first.page.start_index;
    let start = newest.page.start_index;
    while (start > target) {
      const cursor = frames[0].page.before_message_id;
      if (!cursor)
        throw new Error("原阅读范围无法衔接，请刷新原会话；未跳过记录。");
      const prefix = await sessionWindow(
        sessionId,
        cursor,
        Math.min(40, start - target),
      );
      if (prefix.page.end_index !== start || prefix.page.start_index >= start)
        throw new Error("历史窗口未向前推进，未混拼记录。");
      frames.unshift(prefix);
      start = prefix.page.start_index;
    }
    const originalId = first.messages[0]?.id;
    if (
      originalId &&
      !frames.some((frame) =>
        frame.messages.some((message) => message.id === originalId),
      )
    )
      throw new Error("原阅读锚点已不可读取，请刷新原会话；未猜测替代消息。");
  }
  const oldest = frames[0];
  return {
    frames,
    page: {
      ...newest.page,
      start_index: oldest.page.start_index,
      returned: frames.reduce((sum, frame) => sum + frame.messages.length, 0),
      before_message_id: oldest.page.before_message_id,
      has_more: oldest.page.has_more,
    },
  };
}

// Pages are newest-first; original IDs establish order, revisions choose facts.
export function mergeSessionWindows(
  pages: SessionWindow[],
): AssistantConversation {
  const first = pages[0];
  if (!first) throw new Error("没有可读取的会话窗口");
  const header = pages.reduce(
    (newest, page) =>
      page.page.captured_revision >= newest.page.captured_revision
        ? page
        : newest,
    first,
  );
  const messages = new Map<
    string,
    { revision: number; value: AssistantConversation["messages"][number] }
  >();
  const turns = new Map<string, { revision: number; value: AssistantTurn }>();
  for (const page of [...pages].reverse()) {
    if (page.id !== first.id || page.source !== first.source)
      throw new Error("会话窗口来源不一致，未合并其他会话。");
    const revision = page.page.captured_revision;
    for (const value of page.messages) {
      const previous = messages.get(value.id);
      if (
        previous &&
        (previous.value.role !== value.role ||
          previous.value.turn_id !== value.turn_id)
      )
        throw new Error("原消息编号发生冲突，未猜测消息归属。");
      if (!previous || revision >= previous.revision)
        messages.set(value.id, { revision, value });
    }
    for (const item of page.turns) {
      if (item.turn.session_id !== first.id)
        throw new Error("任务不属于当前会话，未合并结果。");
      const previous = turns.get(item.turn.id);
      if (!previous || revision >= previous.revision)
        turns.set(item.turn.id, {
          revision,
          value: {
            ...item.turn,
            details_omitted: item.details_omitted,
            step_count: item.step_count,
            job_count: item.job_count,
            artifact_count: item.artifact_count,
          },
        });
    }
  }
  return {
    id: first.id,
    title: header.title,
    source: first.source,
    queue_resume_generation: header.queue_resume_generation,
    messages: [...messages.values()].map((v) => v.value),
    turns: [...turns.values()].map((v) => v.value),
  };
}

export function selectSessionWindows<P>(data: {
  pages: TimelinePage[];
  pageParams: P[];
}) {
  return {
    ...data,
    combined: mergeSessionWindows(
      data.pages.flatMap((page) => [...page.frames].reverse()),
    ),
  };
}

export function cachedWindowTurn(pages: TimelinePage[], id: string) {
  let found: { revision: number; turn: AssistantTurn } | undefined;
  for (const page of pages.flatMap((page) => page.frames)) {
    const item = page.turns.find((item) => item.turn.id === id);
    if (item && (!found || page.page.captured_revision >= found.revision))
      found = { revision: page.page.captured_revision, turn: item.turn };
  }
  return found?.turn;
}
