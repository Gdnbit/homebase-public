import type { CapabilityPage, Implementation } from "./capability-index";
import type { EvidenceInfo, PriceInfo } from "./capability-facts";
import { inputBytes, MAX_MESSAGE_BYTES } from "./chat-input-limits";
import { frameCapabilityData } from "./capability-context";
export {
  CAPABILITY_SELECTION_MARKER,
  capabilityFunctionNames,
} from "./capability-context";
export const MAX_CAPABILITY_COMPARISON = 3;

// Copy the public contract explicitly. Extra transport/debug fields must never
// silently become part of an AI request, including nested price metadata.
function pricing(p: PriceInfo | null | undefined) {
  return p
    ? {
        route_id: p.route_id,
        state: p.state,
        source_url: p.source_url,
        observed_at: p.observed_at,
        reference_only: p.reference_only,
        rates: p.rates.map((r) => ({
          kind: r.kind,
          amount: r.amount,
          unit: r.unit,
          per_million_tokens: r.per_million_tokens,
        })),
        unmodeled_dimensions: p.unmodeled_dimensions,
        missing_dimensions: p.missing_dimensions,
        invalid_dimensions: p.invalid_dimensions,
        conditional: p.conditional,
      }
    : null;
}
function evidence(p: EvidenceInfo | null | undefined) {
  return p
    ? {
        state: p.state,
        source_commit: p.source_commit,
        package_version: p.package_version,
        manifest_sha256: p.manifest_sha256,
        verification_sha256: p.verification_sha256,
        registered_at: p.registered_at,
        checked_at: p.checked_at,
        code_integrity_checked: p.code_integrity_checked,
      }
    : null;
}
export function capabilityHandoff(
  functionId: string | undefined,
  catalog: CapabilityPage | undefined,
  entry?: Implementation,
) {
  if (!catalog?.catalog_id || !functionId)
    throw Error("能力目录尚未就绪，请刷新后重试；未准备或执行任务。");
  if (entry && entry.function !== functionId)
    throw Error("所选实现与当前功能不一致，请刷新目录；未准备或执行任务。");
  const data = {
    version: "homebase-capability-selection-v1",
    source: {
      catalog_id: catalog.catalog_id,
      assembled_at: catalog.assembled_at,
      expires_at: catalog.expires_at,
      source_complete: catalog.source_complete,
      issues_total: catalog.issues_total,
    },
    selection: {
      function: functionId,
      implementation: entry
        ? {
            entry_key: entry.entry_key ?? null,
            id: entry.id,
            kind: entry.kind,
            function: entry.function,
            provider: entry.provider,
            target: entry.target,
            name: entry.name,
            state: entry.state,
            cost: entry.cost,
            discover: entry.discover,
            credential_binding: entry.credential_binding ?? null,
            prerequisites: entry.prerequisites ?? [],
            evidence: entry.evidence ?? null,
            pricing: pricing(entry.pricing),
            provenance: evidence(entry.provenance),
            summary_truncated: entry.summary_truncated ?? false,
          }
        : null,
    },
  };
  const result = frameCapabilityData(data);
  if (inputBytes(result) > MAX_MESSAGE_BYTES)
    throw Error(
      "能力选择资料超过8000字节，未裁剪标识或报价；请从助理按功能发现，原选择未执行。",
    );
  return result;
}

export function capabilityComparisonHandoff(
  page: CapabilityPage | undefined,
  choices: { entry: Implementation; offset: number }[],
) {
  const functionId = page?.filters.function;
  if (
    !page ||
    !functionId ||
    choices.length < 2 ||
    choices.length > MAX_CAPABILITY_COMPARISON
  )
    throw Error("请从同一功能的目录选择两到三项实现。");
  const selections = choices.map(({ entry, offset }) => {
    if (
      !entry.entry_key ||
      entry.function !== functionId ||
      !Number.isSafeInteger(offset) ||
      offset < 0
    )
      throw Error("所选实现的目录标识不完整，请刷新后重新选择。");
    return { entry_key: entry.entry_key, offset };
  });
  if (
    new Set(selections.map((entry) => entry.entry_key)).size !== choices.length
  )
    throw Error("所选实现的目录标识重复，请刷新后重新选择。");
  const references = JSON.stringify(
    {
      catalog_id: page.catalog_id,
      snapshot_id: page.snapshot_id,
      function: functionId,
      query: page.filters.query,
      selections,
    },
    null,
    2,
  ).replaceAll("`", "\\u0060");
  const result = [
    "请比较我选中的实现，说明各自适用场景、可用状态、费用、前置条件和验收依据。费用或耗时缺失请说明。仅查询能力目录，不运行、安装或探测实现。",
    "用 capability_index_query 一次查询原 catalog_id、function 和 selections 中的 entry_key 列表（填入 entry_keys）。首次精确查询不传旧 snapshot_id、query 或 offset。目录过期时去掉 catalog_id，以相同 function 和 entry_keys 再查一次。确有 next_offset 时才携带新 snapshot_id、原筛选和 next_offset 继续。找不到所选实现时请指出缺项，以 unmatched_entry_keys 为准。source_complete=false 时说明缺项仍待核对。仅比较原选中项。",
    "仅当接口文档未提供 entry_keys 时，才使用原 catalog_id、snapshot_id、function、query 和 selections 的旧 offset 分页核对。快照失效时先在原目录重建筛选，目录也失效时才重新发现。用新分页匹配原 entry_key。",
    "```json\n" + references + "\n```",
  ].join("\n\n");
  if (inputBytes(result) > MAX_MESSAGE_BYTES)
    throw Error("能力比较资料超过8000字节，请刷新后重新选择。");
  return result;
}
