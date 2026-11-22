import type { Query, QueryClient, QueryFilters } from "@tanstack/react-query";

const refreshers = new WeakMap<QueryClient, (query: Query) => void>();

function createRefresher(client: QueryClient) {
  const followups = new WeakSet<Query>();
  function refresh(query: Query) {
    query.invalidate();
    if (query.state.fetchStatus !== "idle") {
      followups.add(query);
      return;
    }
    if (query.isActive()) {
      void client.refetchQueries(
        { queryKey: query.queryKey, exact: true, type: "active" },
        { cancelRefetch: false },
      );
    }
  }
  client.getQueryCache().subscribe((event) => {
    if (event.type === "removed") followups.delete(event.query);
    if (
      event.type === "updated" &&
      event.query.state.fetchStatus === "idle" &&
      followups.delete(event.query)
    ) {
      // A settled read may have cleared invalidation. Reapply it even
      // off-page, where it will be consumed when the observer returns.
      refresh(event.query);
    }
  });
  return refresh;
}

export function refreshAssistantReads(client: QueryClient, filters: QueryFilters) {
  const queries = client.getQueryCache().findAll(filters);
  if (!queries.length) return;
  let refresh = refreshers.get(client);
  if (!refresh) {
    refresh = createRefresher(client);
    refreshers.set(client, refresh);
  }
  for (const query of queries) refresh(query);
}
