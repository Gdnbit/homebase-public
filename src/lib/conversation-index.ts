import { useQuery, type QueryClient } from "@tanstack/react-query";
import { useMemo } from "react";
import {
  agentsApi,
  type HistoryAllResult,
  type HistoryListResult,
} from "./tauri-api";
import { conversationsApi } from "./conversations";

export interface ConversationIndexRow {
  provider: string;
  id: string;
  title: string;
  source: string;
  updated: string;
  active: boolean;
}

const historyAllKey = ["agents", "history", "all"] as const;
const historyKey = (provider: string) =>
  ["agents", "history", "list", provider] as const;
const managedKey = (provider: string) =>
  ["conversations", "managed-sessions", provider] as const;
const isNativeFilter = (filter: string) =>
  ["codex", "claude-code", "gemini-cli"].includes(filter);

/** Same-provider records win in original order; IDs alone never join agents. */
export function uniqueConversationRows(rows: ConversationIndexRow[]) {
  const seen = new Map<string, Set<string>>();
  return rows.filter((row) => {
    const ids = seen.get(row.provider) ?? new Set<string>();
    if (ids.has(row.id)) return false;
    ids.add(row.id);
    seen.set(row.provider, ids);
    return true;
  });
}

export async function invalidateConversationProvider(
  cache: QueryClient,
  provider: string,
) {
  // Disabled observers are marked stale, never force-read with refetch().
  await Promise.all([
    cache.invalidateQueries(
      { queryKey: historyAllKey, exact: true },
      { cancelRefetch: false },
    ),
    cache.invalidateQueries(
      { queryKey: historyKey(provider), exact: true },
      { cancelRefetch: false },
    ),
    cache.invalidateQueries(
      { queryKey: managedKey(provider), exact: true },
      { cancelRefetch: false },
    ),
  ]);
}

function entries<T extends HistoryAllResult | HistoryListResult>(value: T): T {
  if (
    !value ||
    !Array.isArray(value.entries) ||
    value.entries.some(
      (row) =>
        !row ||
        typeof row.agent_id !== "string" ||
        !row.agent_id ||
        typeof row.session_id !== "string" ||
        !row.session_id,
    )
  ) {
    throw Error("历史会话目录返回无效；没有按空目录处理");
  }
  return value;
}

function useManagedIndex(
  provider: "claude-code" | "gemini-cli",
  enabled: boolean,
) {
  return useQuery({
    queryKey: managedKey(provider),
    queryFn: async () => {
      const result = await conversationsApi.native("list", { provider });
      if (
        !Array.isArray(result?.data) ||
        result.data.some(
          (row) =>
            !row ||
            typeof row.id !== "string" ||
            !row.id ||
            (row.provider != null && row.provider !== provider) ||
            (row.preview != null && typeof row.preview !== "string") ||
            (row.updatedAt != null && typeof row.updatedAt !== "string"),
        )
      ) {
        throw Error(`${provider}：原生会话目录返回无效；没有按空目录处理`);
      }
      if (result.nextCursor != null || result.next_cursor != null) {
        throw Error(`${provider}：原生目录返回未适配的续页，未当作完整目录`);
      }
      return result.data.map((row): ConversationIndexRow => ({
        provider,
        id: row.id,
        title: row.preview || "新会话",
        source: provider === "claude-code" ? "Claude Code" : "Gemini CLI",
        updated: row.updatedAt ?? "",
        active: false,
      }));
    },
    enabled,
    staleTime: 60_000,
  });
}

export function useConversationIndex(filter: string) {
  const allEnabled = filter === "all";
  const scopedEnabled = isNativeFilter(filter);
  const claudeEnabled = allEnabled || filter === "claude-code";
  const geminiEnabled = allEnabled || filter === "gemini-cli";
  const all = useQuery({
    queryKey: historyAllKey,
    queryFn: async () => {
      const result = entries(await agentsApi.historyAll());
      if (!Array.isArray(result.buckets))
        throw Error("历史会话来源状态无效；没有按空目录处理");
      return result;
    },
    enabled: allEnabled,
    staleTime: 60_000,
  });
  const scoped = useQuery({
    queryKey: historyKey(filter),
    queryFn: async () => {
      const result = entries(await agentsApi.historyList(filter));
      if (
        result.agent_id !== filter ||
        typeof result.supported !== "boolean" ||
        result.entries.some((r) => r.agent_id !== filter)
      ) {
        throw Error("历史会话来源不匹配；没有读取其他来源或按空目录处理");
      }
      return result;
    },
    enabled: scopedEnabled,
    staleTime: 60_000,
  });
  const claude = useManagedIndex("claude-code", claudeEnabled);
  const gemini = useManagedIndex("gemini-cli", geminiEnabled);
  const active = [
    allEnabled ? all : null,
    scopedEnabled ? scoped : null,
    claudeEnabled ? claude : null,
    geminiEnabled ? gemini : null,
  ].filter((q) => q !== null);
  const original = useMemo(
    () =>
      (allEnabled
        ? all.data?.entries
        : scopedEnabled && scoped.data?.supported
          ? scoped.data.entries
          : []) ?? [],
    [allEnabled, scopedEnabled, all.data, scoped.data],
  );
  const managed = useMemo(
    () => [
      ...(claudeEnabled ? (claude.data ?? []) : []),
      ...(geminiEnabled ? (gemini.data ?? []) : []),
    ],
    [claudeEnabled, geminiEnabled, claude.data, gemini.data],
  );
  const warnings = allEnabled
    ? (all.data?.buckets ?? [])
        .filter((b) => !b.supported)
        .map(
          (b) =>
            `${b.agent_id}：${b.unsupported_reason || "此来源未提供历史读取"}`,
        )
    : scopedEnabled && scoped.data && !scoped.data.supported
      ? [
          `${filter}：${scoped.data.unsupported_reason || "此来源未提供历史读取"}`,
        ]
      : [];
  return {
    history: original,
    managed,
    loading: active.some((q) => q.isLoading),
    fetching: active.some((q) => q.isFetching),
    errors: active.filter((q) => q.error).map((q) => String(q.error)),
    warnings,
    refresh: () =>
      Promise.all(active.map((q) => q.refetch({ cancelRefetch: false }))),
  };
}
