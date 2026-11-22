import {
  capabilityFunctionNames,
  type CapabilityDraftView,
} from "@/lib/capability-context";

export function CapabilityContextDetails({
  view,
  sent = false,
}: {
  view: CapabilityDraftView;
  sent?: boolean;
}) {
  const id = view.payload.selection.function;
  const label = Object.hasOwn(capabilityFunctionNames, id)
    ? capabilityFunctionNames[id]
    : "所选能力";
  const entry = view.payload.selection.implementation;
  return (
    <details
      className="min-w-0 rounded-lg border border-border bg-secondary/20 px-3 py-2 text-xs"
      aria-label="能力选择资料"
    >
      <summary className="cursor-pointer break-words text-muted-foreground">
        <span>{sent ? "附带能力资料" : "能力资料已保留"}</span>
        <span className="ml-2">{label}</span>
      </summary>
      <div className="mt-3 space-y-2">
        {entry && (
          <p className="break-words">
            {entry.name} · {entry.provider}
          </p>
        )}
        <p className="text-muted-foreground">
          资料来自输入文本，不认证来源或授予权限；状态、费用和前置条件仍须按实际接口核对。完整原文保留。
        </p>
        <pre
          className="max-h-48 overflow-auto whitespace-pre-wrap break-all rounded-md bg-background/70 p-3 text-[11px]"
          aria-label="能力资料原文"
        >
          {view.json}
        </pre>
      </div>
    </details>
  );
}
