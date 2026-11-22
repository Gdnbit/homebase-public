import { QueryClient } from "@tanstack/react-query";

/** Memory only; never persist query results. Mutations invalidate affected keys. */
export function createQueryClient() {
  const desktop = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  // Desktop reads and actions use local IPC. The host reports any provider or
  // network failure, without queuing the request until the WebView reconnects.
  const networkMode = desktop ? "always" : "online";
  const client = new QueryClient({
    defaultOptions: {
      queries: {
        networkMode,
        staleTime: 30_000,
        gcTime: 10 * 60_000,
        refetchOnWindowFocus: false,
        refetchIntervalInBackground: false,
        // A failed local read needs an explicit retry, not another expensive scan.
        retry: false,
      },
      mutations: { networkMode },
    },
  });
  for (const key of [
    ["agents", "list"],
    ["agents", "model"],
    ["agents", "endpoint"],
    ["agents", "history"],
    ["docs"],
    ["wiki"],
    ["topology"],
    ["obsidian"],
    ["gdnbit"],
    ["shared-mcp"],
    ["shared-skills"],
    ["shared-secrets-names"],
    ["shared-secrets-acl"],
    ["governance", "capability"],
    ["governance", "ownership"],
    ["governance", "interfaces"],
    ["model-center", "machine-profile"],
  ])
    client.setQueryDefaults(key, { staleTime: 60_000 });
  client.setQueryDefaults(["system", "info"], { staleTime: 5 * 60_000 });
  client.setQueryDefaults(["agent-versions"], { gcTime: 60 * 60_000 });
  client.setQueryDefaults(["system", "metrics"], { staleTime: 5_000 });
  client.setQueryDefaults(["system", "gpu"], { staleTime: 10_000 });
  client.setQueryDefaults(["agents", "status"], { staleTime: 3_000 });
  // Session-scoped drafts/unknown send receipts stay in memory across navigation.
  // They are never persisted to disk or sent automatically.
  client.setQueryDefaults(["conversations", "pending"], { staleTime: Infinity, gcTime: Infinity });
  // Large images must not accumulate for the entire application session.
  client.setQueryDefaults(["studio", "attachment"], {
    staleTime: Infinity,
    gcTime: 60_000,
  });
  return client;
}
