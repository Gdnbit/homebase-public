import format from "../../control/capability-selection-format.json";
import type { CapabilityPage, Implementation } from "./capability-index";
import { inputBytes, MAX_DRAFT_BYTES } from "./chat-input-limits";

export const CAPABILITY_SELECTION_MARKER = format.json_marker;
export const CAPABILITY_FRAME_END = format.end;
export const capabilityFunctionNames: Record<string, string> =
  format.function_names;
const prefix = format.start + format.intro + format.rules + format.json_marker;
type SelectionEntry = Omit<Implementation, "entry_key" | "evidence"> & {
  entry_key: string | null;
  evidence: string | null;
};
export interface CapabilitySelectionData {
  version: string;
  source: Pick<
    CapabilityPage,
    | "catalog_id"
    | "assembled_at"
    | "expires_at"
    | "source_complete"
    | "issues_total"
  >;
  selection: { function: string; implementation: SelectionEntry | null };
}
export interface CapabilityDraftView {
  task: string;
  frame: string;
  json: string;
  payload: CapabilitySelectionData;
}
function record(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}
function keys(
  value: unknown,
  required: string[],
  optional: string[] = [],
): value is Record<string, unknown> {
  return (
    record(value) &&
    required.every((k) => Object.hasOwn(value, k)) &&
    Object.keys(value).every(
      (k) => required.includes(k) || optional.includes(k),
    )
  );
}
const stringOrNull = (v: unknown) => v === null || typeof v === "string";
const count = (v: unknown) =>
  typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
function validPrice(p: unknown) {
  return (
    p === null ||
    (keys(p, [
      "route_id",
      "state",
      "source_url",
      "observed_at",
      "reference_only",
      "rates",
      "unmodeled_dimensions",
      "missing_dimensions",
      "invalid_dimensions",
      "conditional",
    ]) &&
      [p.route_id, p.state, p.source_url, p.observed_at].every(
        (v) => typeof v === "string",
      ) &&
      ["reported", "partial", "invalid"].includes(p.state as string) &&
      typeof p.reference_only === "boolean" &&
      typeof p.conditional === "boolean" &&
      [
        p.unmodeled_dimensions,
        p.missing_dimensions,
        p.invalid_dimensions,
      ].every(count) &&
      Array.isArray(p.rates) &&
      p.rates.every(
        (r) =>
          keys(r, ["kind", "amount", "unit"], ["per_million_tokens"]) &&
          [r.kind, r.amount, r.unit].every((v) => typeof v === "string") &&
          ["prompt", "completion", "request"].includes(r.kind as string) &&
          ["usd_per_token", "usd_per_request"].includes(r.unit as string) &&
          (r.per_million_tokens === undefined ||
            stringOrNull(r.per_million_tokens)),
      ))
  );
}
function validProof(p: unknown) {
  return (
    p === null ||
    (keys(
      p,
      [
        "state",
        "source_commit",
        "package_version",
        "manifest_sha256",
        "verification_sha256",
        "registered_at",
        "code_integrity_checked",
      ],
      ["checked_at"],
    ) &&
      [
        p.state,
        p.source_commit,
        p.package_version,
        p.manifest_sha256,
        p.verification_sha256,
        p.registered_at,
      ].every((v) => typeof v === "string") &&
      ["registry_recorded", "records_checked"].includes(p.state as string) &&
      (p.checked_at === undefined || stringOrNull(p.checked_at)) &&
      typeof p.code_integrity_checked === "boolean")
  );
}
function validData(v: unknown): v is CapabilitySelectionData {
  if (
    !keys(v, ["version", "source", "selection"]) ||
    v.version !== format.payload_version ||
    !keys(v.source, [
      "catalog_id",
      "assembled_at",
      "expires_at",
      "source_complete",
      "issues_total",
    ]) ||
    ![v.source.catalog_id, v.source.assembled_at, v.source.expires_at].every(
      (x) => typeof x === "string",
    ) ||
    !v.source.catalog_id ||
    typeof v.source.source_complete !== "boolean" ||
    !count(v.source.issues_total) ||
    !keys(v.selection, ["function", "implementation"]) ||
    typeof v.selection.function !== "string" ||
    !v.selection.function
  )
    return false;
  const e = v.selection.implementation;
  return (
    e === null ||
    (keys(e, [
      "entry_key",
      "id",
      "kind",
      "function",
      "provider",
      "target",
      "name",
      "state",
      "cost",
      "discover",
      "credential_binding",
      "prerequisites",
      "evidence",
      "pricing",
      "provenance",
      "summary_truncated",
    ]) &&
      [
        e.id,
        e.kind,
        e.function,
        e.provider,
        e.target,
        e.name,
        e.state,
        e.cost,
        e.discover,
      ].every((x) => typeof x === "string") &&
      e.function === v.selection.function &&
      ["model", "service", "repository", "local", "mcp", "skill"].includes(
        e.kind as string,
      ) &&
      ["unknown", "local_compute", "depends_on_tools"].includes(
        e.cost as string,
      ) &&
      stringOrNull(e.entry_key) &&
      stringOrNull(e.credential_binding) &&
      stringOrNull(e.evidence) &&
      Array.isArray(e.prerequisites) &&
      e.prerequisites.every((x) => typeof x === "string") &&
      typeof e.summary_truncated === "boolean" &&
      validPrice(e.pricing) &&
      validProof(e.provenance))
  );
}

// This recognizes a reversible text presentation, never an authenticated source.
// Noncanonical/unknown input remains fully visible and is never rewritten here.
export function readCapabilityDraft(text: string): CapabilityDraftView | null {
  if (inputBytes(text) > MAX_DRAFT_BYTES) return null;
  const start = text.indexOf(format.start);
  if (
    start < 0 ||
    text.indexOf(format.start, start + format.start.length) >= 0 ||
    !text.startsWith(prefix, start)
  )
    return null;
  if (start && !text.slice(0, start).endsWith("\n\n")) return null;
  const jsonStart = start + prefix.length;
  const end = text.indexOf(format.end, jsonStart);
  if (end < 0 || text.indexOf(format.end, end + format.end.length) >= 0)
    return null;
  const json = text.slice(jsonStart, end);
  try {
    const data: unknown = JSON.parse(json);
    if (!validData(data) || JSON.stringify(data, null, 2) !== json) return null;
    const after = text.slice(end + format.end.length);
    if (after && !after.startsWith("\n")) return null;
    const before = start ? text.slice(0, start - 2) : "";
    const tail = after ? after.slice(1) : "";
    return {
      task: before + (before && tail ? "\n" : "") + tail,
      frame: text.slice(start, end + format.end.length),
      json,
      payload: data,
    };
  } catch {
    return null;
  }
}
export function frameCapabilityData(data: CapabilitySelectionData) {
  return prefix + JSON.stringify(data, null, 2) + format.end;
}
export function replaceCapabilityTask(
  view: Pick<CapabilityDraftView, "frame">,
  task: string,
) {
  return task ? `${task}\n\n${view.frame}` : view.frame;
}
export function legacyCapabilityConversion(text: string): string | null {
  if (inputBytes(text) > MAX_DRAFT_BYTES || text.includes(format.start))
    return null;
  const marker = text.indexOf(format.json_marker);
  if (
    marker < 0 ||
    text.indexOf(format.json_marker, marker + format.json_marker.length) >= 0
  )
    return null;
  try {
    const json = text.slice(marker + format.json_marker.length);
    const data: unknown = JSON.parse(json);
    if (!validData(data) || JSON.stringify(data, null, 2) !== json) return null;
    const id = data.selection.function;
    const label = Object.hasOwn(capabilityFunctionNames, id)
      ? capabilityFunctionNames[id]
      : "所选能力";
    const begin = `请帮我准备${label}任务，具体目标由我补充。\n\n我的任务：\n\n`;
    const before = text.slice(0, marker);
    if (!before.startsWith(begin) || !before.endsWith(format.rules))
      return null;
    const task = before.slice(begin.length, -format.rules.length);
    return replaceCapabilityTask({ frame: frameCapabilityData(data) }, task);
  } catch {
    return null;
  }
}
