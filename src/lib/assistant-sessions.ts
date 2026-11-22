import { useMemo } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";
import type { AssistantSession } from "./assistant";

type Source = "desktop" | "wechat";
type Filters = { query: string; source: Source | null };
type Request = Filters & {
  snapshot_id?: string;
  offset: number;
  limit: number;
};
export interface SessionCatalogPage extends Filters {
  version: "homebase-session-catalog-v1";
  snapshot_id: string;
  captured_revision: number;
  captured_at: string;
  expires_in_seconds: number;
  total: number;
  offset: number;
  next_offset: number | null;
  complete: boolean;
  items: AssistantSession[];
}
type BrowserState = {
  generation: number;
  pinned: SessionCatalogPage | null;
  pages: SessionCatalogPage[];
  loading: boolean;
  error: string | null;
};
const emptyBrowser = (generation = 0): BrowserState => ({
  generation,
  pinned: null,
  pages: [],
  loading: false,
  error: null,
});
const prefix = ["assistant", "sessions"] as const;
const headKey = (filters: Filters) =>
  !filters.query && !filters.source
    ? prefix
    : [...prefix, filters.query, filters.source];
type Flight = {
  generation: number;
  pending?: Promise<SessionCatalogPage>;
  refresh?: ReturnType<typeof setTimeout>;
};
const clients = new WeakMap<QueryClient, Map<string, Flight>>();
function flights(client: QueryClient) {
  let map = clients.get(client);
  if (!map) {
    map = new Map();
    clients.set(client, map);
  }
  return map;
}
function pruneFlights(map: Map<string, Flight>) {
  for (const [key, flight] of map) {
    if (map.size <= 32) break;
    if (!flight.pending && flight.refresh === undefined) map.delete(key);
  }
}
const invalid = () =>
  Error(
    "会话分页结果无法核对，原列表和输入已保留；请刷新列表，不要重新发送任务。",
  );
async function read(request: Request): Promise<SessionCatalogPage> {
  const page = await invoke<SessionCatalogPage>("assistant_sessions_query", {
    request,
  });
  const end =
    page?.offset + (Array.isArray(page?.items) ? page.items.length : 0);
  if (
    !page ||
    page.version !== "homebase-session-catalog-v1" ||
    typeof page.snapshot_id !== "string" ||
    !page.snapshot_id ||
    (request.snapshot_id && request.snapshot_id !== page.snapshot_id) ||
    page.query !== request.query ||
    page.source !== request.source ||
    !Number.isSafeInteger(page.captured_revision) ||
    typeof page.captured_at !== "string" ||
    !Number.isInteger(page.total) ||
    page.total < 0 ||
    page.total > 10_000 ||
    page.offset !== request.offset ||
    typeof page.complete !== "boolean" ||
    page.captured_revision < 0 ||
    !Number.isInteger(page.expires_in_seconds) ||
    page.expires_in_seconds < 0 ||
    !Array.isArray(page.items) ||
    page.items.length > request.limit ||
    end > page.total ||
    (page.next_offset === null
      ? !page.complete || end !== page.total
      : page.complete ||
        page.next_offset !== end ||
        end >= page.total ||
        !page.items.length) ||
    new TextEncoder().encode(JSON.stringify(page)).length > 9000
  )
    throw invalid();
  const ids = new Set<string>();
  for (const row of page.items) {
    if (
      !row ||
      typeof row.id !== "string" ||
      !row.id ||
      ids.has(row.id) ||
      typeof row.title !== "string" ||
      typeof row.source !== "string" ||
      typeof row.updated_at !== "string" ||
      !Number.isInteger(row.message_count) ||
      row.message_count < 0 ||
      (row.active_turn_id !== null && typeof row.active_turn_id !== "string")
    )
      throw invalid();
    ids.add(row.id);
  }
  return page;
}
function readHead(client: QueryClient, filters: Filters) {
  const id = JSON.stringify(headKey(filters));
  const map = flights(client);
  let flight = map.get(id);
  if (!flight) {
    flight = { generation: 0 };
    map.set(id, flight);
  } else {
    map.delete(id);
    map.set(id, flight);
  }
  if (flight.pending) return flight.pending;
  const current = flight;
  if (current.refresh !== undefined) {
    clearTimeout(current.refresh);
    current.refresh = undefined;
  }
  const generation = current.generation;
  current.pending = read({ ...filters, offset: 0, limit: 20 })
    .then((result) => {
      // Publish each finished snapshot. Continuous progress must not keep this
      // promise pending forever; coalesce a new read only after cache adoption.
      if (generation !== current.generation) {
        current.refresh = setTimeout(() => {
          current.refresh = undefined;
          void client
            .invalidateQueries({
              queryKey: headKey(filters),
              exact: true,
              refetchType: "none",
            })
            .then(() =>
              client.refetchQueries(
                { queryKey: headKey(filters), exact: true, type: "active" },
                { cancelRefetch: false },
              ),
            );
        }, 0);
      }
      return result;
    })
    .finally(() => {
      current.pending = undefined;
      pruneFlights(map);
    });
  pruneFlights(map);
  return current.pending;
}
/** Mark inactive heads stale without reading. Active subscribers share each read. */
export async function invalidateSessionCatalog(
  client: QueryClient,
  refetch = true,
) {
  for (const flight of flights(client).values()) flight.generation++;
  await client.invalidateQueries({ queryKey: prefix, refetchType: "none" });
  if (refetch)
    await client.refetchQueries(
      { queryKey: prefix, type: "active" },
      { cancelRefetch: false },
    );
}
export function useSessionCatalog(
  options: {
    query?: string;
    source?: Source;
    enabled?: boolean;
    browse?: boolean;
  } = {},
) {
  const client = useQueryClient();
  const filters: Filters = {
    query: (options.query ?? "").trim().toLowerCase(),
    source: options.source ?? null,
  };
  const key = headKey(filters);
  const browserKey = ["session-catalog-browser", filters.query, filters.source];
  const head = useQuery({
    queryKey: key,
    queryFn: () => readHead(client, filters),
    enabled: options.enabled ?? true,
    staleTime: 60_000,
    retry: false,
  });
  const browser = useQuery({
    queryKey: browserKey,
    queryFn: async () => emptyBrowser(),
    enabled: false,
    initialData: () => emptyBrowser(),
    gcTime: 60_000,
  });
  const state = browser.data;
  const pinned = options.browse ? state.pinned : null;
  const first = pinned ?? head.data;
  const pages = useMemo(
    () => (first ? [first, ...(pinned ? state.pages : [])] : []),
    [first, pinned, state.pages],
  );
  const data = useMemo(
    () => (pages.length ? pages.flatMap((page) => page.items) : undefined),
    [pages],
  );
  const next = pages.at(-1)?.next_offset ?? null;
  async function refresh() {
    client.setQueryData<BrowserState>(browserKey, (old) =>
      emptyBrowser((old?.generation ?? 0) + 1),
    );
    const flight = flights(client).get(JSON.stringify(key));
    if (flight) flight.generation++;
    await client.invalidateQueries({
      queryKey: key,
      exact: true,
      refetchType: "none",
    });
    await client.refetchQueries(
      { queryKey: key, exact: true, type: "active" },
      { cancelRefetch: false },
    );
  }
  async function loadMore() {
    if (!options.browse) return;
    const latest =
      client.getQueryData<BrowserState>(browserKey) ?? emptyBrowser();
    if (latest.loading) return;
    const original =
      latest.pinned ?? client.getQueryData<SessionCatalogPage>(key);
    const offset = latest.pages.length
      ? latest.pages.at(-1)!.next_offset
      : original?.next_offset;
    if (!original || offset == null) return;
    const generation = latest.generation;
    client.setQueryData<BrowserState>(browserKey, {
      ...latest,
      pinned: original,
      loading: true,
      error: null,
    });
    const stillCurrent = () => {
      const now = client.getQueryData<BrowserState>(browserKey);
      return (
        now?.generation === generation &&
        now.pinned?.snapshot_id === original.snapshot_id
      );
    };
    try {
      const page = await read({
        ...filters,
        snapshot_id: original.snapshot_id,
        offset,
        limit: 20,
      });
      const ids = new Set(
        [original, ...latest.pages].flatMap((p) =>
          p.items.map((row) => row.id),
        ),
      );
      if (
        page.total !== original.total ||
        page.captured_revision !== original.captured_revision ||
        page.items.some((row) => ids.has(row.id))
      )
        throw invalid();
      if (stillCurrent())
        client.setQueryData<BrowserState>(browserKey, {
          ...latest,
          pinned: original,
          pages: [...latest.pages, page],
          loading: false,
          error: null,
        });
    } catch (error) {
      if (stillCurrent())
        client.setQueryData<BrowserState>(browserKey, {
          ...latest,
          pinned: original,
          loading: false,
          error: String(error),
        });
    }
  }
  return {
    ...head,
    data,
    total: first?.total,
    hasMore: next !== null,
    complete: first !== undefined && next === null,
    loadingMore: options.browse && state.loading,
    moreError: options.browse ? state.error : null,
    updatesAvailable:
      !!pinned && !!head.data && head.data.snapshot_id !== pinned.snapshot_id,
    loadMore,
    refetch: refresh,
  };
}
