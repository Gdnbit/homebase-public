import { invoke } from "@tauri-apps/api/core";
import type { PriceInfo } from "@/lib/capability-facts";

export interface CapabilityCredential {
  provider_id: string;
  provider_name: string;
  credential_ref: string;
  source: string;
  state: string;
  message: string;
  remote_state: string;
  expires_at: string | null;
  expiry_warning: boolean;
}
export interface ModelCapability {
  route_id: string;
  model_id: string;
  name: string;
  publisher: string;
  canonical_id: string;
  provider_id: string;
  functions: string[];
  protocol: string;
  source: string;
  source_url: string;
  account_state: string;
  input_schema: Record<string, unknown>;
  input_example: Record<string, unknown>;
  pricing?: PriceInfo | null;
}
export interface ModelCapabilityCatalog {
  credentials: CapabilityCredential[];
  functions: { id: string; name: string }[];
  models: ModelCapability[];
  services: {
    id: string;
    name: string;
    function: string;
    credential_ref: string;
    state: string;
    message: string;
  }[];
  sources: {
    provider_id: string;
    state: string;
    message: string;
    fetched_at: string | null;
  }[];
  issues: string[];
  total_models: number;
  next_offset: number | null;
}
export const MAX_PNG_BYTES = 384 * 1024;
export function modelSupportsVision(route?: ModelCapability) {
  const modes = route?.input_schema.anyOf;
  return Array.isArray(modes) && modes.some(
    (option) => Array.isArray(option?.required) && option.required.includes("vision"),
  );
}

export async function readLocalPngBase64(file: File) {
  if (!file.size || file.size > MAX_PNG_BYTES)
    throw Error("请选择不超过 384 KiB 的 PNG 图片。");
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  }
  return btoa(binary);
}
export interface ModelCapabilityRequest {
  route_id: string;
  input: Record<string, unknown>;
  output_root: string | null;
  credential_source?: "default" | "sops" | "api_key_manager";
}
export interface ModelCapabilityPlan {
  route: ModelCapability;
  credential: CapabilityCredential;
  method: string;
  endpoint: string;
  request: Record<string, unknown>;
  callable: boolean;
  message: string;
}
export interface ModelCapabilityResult {
  provider_id: string;
  model_id: string;
  credential_ref: string;
  status: string;
  task_id: string | null;
  output: unknown;
  message: string;
  artifact: {
    path: string;
    bytes: number;
    sha256: string;
    mime_type: string;
  } | null;
}
export const modelCapabilitiesKey = ["model-capabilities", "catalog"] as const;
async function completeCatalog(first: ModelCapabilityCatalog) {
  const models = [...first.models];
  const seen = new Set<number>();
  let offset = first.next_offset;
  while (offset !== null && offset !== undefined) {
    if (seen.has(offset) || seen.size >= 50)
      throw new Error("模型目录分页异常；未将不完整列表标为完整目录");
    seen.add(offset);
    const page = await invoke<ModelCapabilityCatalog>(
      "model_capabilities_catalog",
      { offset, limit: 200 },
    );
    if (page.total_models !== first.total_models)
      throw new Error("同步期间目录发生变化，请重新读取；已保留原目录");
    models.push(...page.models);
    offset = page.next_offset;
  }
  return { ...first, models, next_offset: null };
}
export const modelCapabilitiesApi = {
  catalog: async () =>
    completeCatalog(
      await invoke<ModelCapabilityCatalog>("model_capabilities_catalog", {
        offset: 0,
        limit: 200,
      }),
    ),
  refresh: async (
    providerIds: string[],
    credentialSource: "default" | "sops" | "api_key_manager" = "default",
  ) =>
    completeCatalog(
      await invoke<ModelCapabilityCatalog>("model_capabilities_refresh", {
        providerIds,
        credentialSource,
      }),
    ),
  resolve: (request: ModelCapabilityRequest) =>
    invoke<ModelCapabilityPlan>("model_capability_resolve", { request }),
  call: (request: ModelCapabilityRequest) =>
    invoke<ModelCapabilityResult>("model_capability_call", { request }),
  task: (
    routeId: string,
    taskId: string,
    outputRoot: string | null,
    credentialSource: "default" | "sops" | "api_key_manager" = "default",
  ) =>
    invoke<ModelCapabilityResult>("model_capability_task", {
      routeId,
      taskId,
      outputRoot,
      credentialSource,
    }),
};
export const credentialLabel = (state: string) =>
  ({
    configured: "已配置",
    linked: "已关联",
    not_configured: "未配置",
    blocked: "已阻断",
    unknown: "状态未知",
    expired: "已到期",
    invalid: "配置或到期日期无效",
  })[state] ?? "状态未知";
export function modelGroups(rows: ModelCapability[]) {
  const groups = new Map<string, ModelCapability[]>();
  for (const row of rows) {
    const routes = groups.get(row.canonical_id) ?? [];
    routes.push(row);
    groups.set(row.canonical_id, routes);
  }
  return [...groups.entries()];
}
