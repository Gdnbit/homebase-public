import { invoke } from "@tauri-apps/api/core";
import type { QueryClient } from "@tanstack/react-query";
import type { EvidenceInfo, PriceInfo } from "@/lib/capability-facts";
import { refreshAssistantReads } from "./assistant-refresh";

export const capabilityCatalogKey = ["capability-index"] as const;

export function refreshCapabilityCatalog(client: QueryClient) {
  // Search and implementation pages belong to immutable catalog IDs.
  refreshAssistantReads(client, { queryKey: capabilityCatalogKey, exact: true });
}
export type ImplementationKind =
  "model" | "service" | "repository" | "local" | "mcp" | "skill";
export interface Implementation {
  entry_key?: string;
  id: string;
  function: string;
  kind: ImplementationKind;
  name: string;
  provider: string;
  state: string;
  cost: "unknown" | "local_compute" | "depends_on_tools";
  discover: string;
  target: string;
  credential_binding?: string | null;
  prerequisites?: string[];
  evidence?: string;
  pricing?: PriceInfo | null;
  provenance?: EvidenceInfo | null;
  summary_truncated?: boolean;
}
export interface CapabilityPage {
  version: string;
  catalog_id: string;
  snapshot_id: string;
  assembled_at: string;
  expires_at: string;
  filters: { query: string; function: string | null; kind: ImplementationKind | null };
  total: number;
  offset: number;
  next_offset: number | null;
  entries: Implementation[];
  groups: { function: string; total: number; kinds: ImplementationKind[] }[];
  groups_has_more: boolean;
  issues: { source: string; message: string }[];
  issues_total: number;
  issues_has_more: boolean;
  source_complete: boolean;
  routing: string;
}
interface CapabilityRequest {
  catalog_id?: string;
  snapshot_id?: string;
  query?: string;
  function?: string;
  kind?: ImplementationKind;
  offset?: number;
  limit?: number;
}

export async function queryCapabilities(options: CapabilityRequest = {}) {
  const request = { ...options, offset: options.offset ?? 0, limit: options.limit ?? 20 };
  const page = await invoke<CapabilityPage>("capability_index_query", { request });
  if (page?.version !== "homebase-capability-discovery-v1" ||
      typeof page.catalog_id !== "string" || !page.catalog_id ||
      typeof page.snapshot_id !== "string" || !page.snapshot_id ||
      (request.catalog_id && page.catalog_id !== request.catalog_id) ||
      (request.snapshot_id && page.snapshot_id !== request.snapshot_id) ||
      page.filters?.query !== (request.query ?? "").trim().toLowerCase() ||
      page.filters.function !== (request.function ?? null) ||
      page.filters.kind !== (request.kind ?? null)) {
    throw Error("目录、快照或筛选不一致，请刷新能力目录。");
  }
  if (!Array.isArray(page.entries) || !Array.isArray(page.groups) || !Array.isArray(page.issues) ||
      !Number.isSafeInteger(page.total) || page.offset !== request.offset ||
      page.entries.length > request.limit || page.offset + page.entries.length > page.total) {
    throw Error("能力页的条目或分页格式无效，请刷新能力目录。");
  }
  const end = page.offset + page.entries.length;
  if (page.next_offset !== (end < page.total ? end : null) ||
      (end < page.total && !page.entries.length)) {
    throw Error("能力页的续页位置无效，请刷新能力目录。");
  }
  for (const entry of page.entries) {
    if (!entry || [entry.id, entry.kind, entry.function, entry.target, entry.provider, entry.name, entry.discover]
      .some((value) => typeof value !== "string" || !value) ||
      (request.function && entry.function !== request.function) ||
      (request.kind && entry.kind !== request.kind)) {
      throw Error("能力实现与所选筛选不一致，或缺少必要标识。");
    }
    if (entry.pricing && (entry.kind !== "model" || entry.pricing.route_id !== entry.target ||
        !entry.pricing.reference_only || !Array.isArray(entry.pricing.rates) ||
        typeof entry.pricing.source_url !== "string")) {
      throw Error("参考报价与实现不一致，请刷新能力目录后核对原渠道。");
    }
  }
  return page;
}
