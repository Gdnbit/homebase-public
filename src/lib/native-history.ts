import type { NativeReply, NativeThread } from "./conversations";

type NativeTurn = NonNullable<NativeThread["turns"]>[number];

export interface NativeHistoryWindow {
  turns: NativeTurn[];
  nextCursor: string | null;
}

export function nativeHistoryWindow(reply: NativeReply): NativeHistoryWindow {
  return { turns: reply.thread?.turns ?? [], nextCursor: reply.next_cursor ?? null };
}

// A shared turn anchors the windows. Keep a gap when their order cannot be
// established from the overlap. Cursor contents remain opaque to the UI.
function joinOverlapping(
  before: NativeHistoryWindow,
  after: NativeHistoryWindow,
): NativeHistoryWindow | undefined {
  const positions = new Map(before.turns.map((turn, index) => [turn.id, index]));
  const anchor = after.turns.findIndex((turn) => positions.has(turn.id));
  if (anchor < 0) return undefined;
  const offset = positions.get(after.turns[anchor].id)! - anchor;
  const overlapStart = Math.max(0, offset);
  const overlapEnd = Math.min(before.turns.length, offset + after.turns.length);
  for (let index = overlapStart; index < overlapEnd; index++) {
    if (before.turns[index].id !== after.turns[index - offset].id) return undefined;
  }
  const start = Math.min(0, offset);
  const end = Math.max(before.turns.length, offset + after.turns.length);
  const turns = Array.from({ length: end - start }, (_, index) => {
    const position = start + index;
    return after.turns[position - offset] ?? before.turns[position];
  });
  return { turns, nextCursor: offset > 0 ? before.nextCursor : after.nextCursor };
}

export function appendNativeHistory(
  windows: NativeHistoryWindow[],
  latest: NativeHistoryWindow,
): NativeHistoryWindow[] {
  if (!latest.turns.length) return windows;
  let boundary = windows.length;
  while (boundary > 0) {
    const joined = joinOverlapping(windows[boundary - 1], latest);
    if (!joined) break;
    latest = joined;
    boundary--;
  }
  return [...windows.slice(0, boundary), latest];
}

export function prependNativeHistory(
  windows: NativeHistoryWindow[],
  cursor: string,
  earlier: NativeHistoryWindow,
): NativeHistoryWindow[] {
  const boundary = windows.findIndex((window) => window.nextCursor === cursor);
  if (boundary < 0) return windows;
  if (earlier.nextCursor === cursor ||
    (earlier.nextCursor !== null && windows.some((window, index) => index >= boundary && window.nextCursor === earlier.nextCursor)))
    throw Error("原生分页游标没有推进，已读内容保留。");

  const current = windows[boundary];
  const joined = joinOverlapping(earlier, current);
  const known = new Set(current.turns.map((turn) => turn.id));
  if (!joined && earlier.turns.some((turn) => known.has(turn.id)))
    throw Error("原生分页的原轮次顺序不一致，已读内容保留。");
  const expanded = joined ?? { turns: [...earlier.turns, ...current.turns], nextCursor: earlier.nextCursor };
  return [...appendNativeHistory(windows.slice(0, boundary), expanded), ...windows.slice(boundary + 1)];
}
