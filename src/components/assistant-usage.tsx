import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw } from "lucide-react";
interface UsageTotals {
  samples: number;
  reported_cost_usd: number | null;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  cost_unknown_samples: number;
  prompt_unknown_samples: number;
  completion_unknown_samples: number;
}
interface UsageGroup {
  kind: "model" | "unidentified" | "other";
  model: string | null;
  model_count: number;
  totals: UsageTotals;
}
interface Usage extends UsageTotals {
  scope: string;
  source: string;
  turns: number;
  active: number;
  unreported_turns: number;
  complete: boolean;
  groups?: UsageGroup[];
}

function formatCost(value: number | null | undefined) {
  if (value == null) return undefined;
  const rounded = value.toFixed(6);
  return value !== 0 && Number(rounded) === 0 ? value.toExponential(2) : rounded;
}

function UsageCell({ value, missing }: { value: string | undefined; missing: number }) {
  return (
    <td className="px-3 py-2 text-right tabular-nums">
      {value ?? "未知"}
      {missing > 0 && <span className="block text-xs text-muted-foreground">{missing} 条缺失</span>}
    </td>
  );
}

function UsageBreakdown({ groups }: { groups: UsageGroup[] }) {
  return (
    <div className="overflow-x-auto rounded-lg border border-border bg-card">
      <table aria-label="按模型查看用量" className="w-full min-w-[36rem] text-sm">
        <thead className="bg-muted/40 text-xs text-muted-foreground">
          <tr>
            <th scope="col" className="px-3 py-2 text-left font-medium">服务商上报模型</th>
            {["记录数", "输入 Token", "输出 Token", "已上报费用（USD）"].map((label) => (
              <th scope="col" key={label} className="px-3 py-2 text-right font-medium">{label}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {groups.map(({ kind, model, model_count, totals }) => (
            <tr key={model ?? kind} className="border-t border-border">
              <th scope="row" className="max-w-72 break-words px-3 py-2 text-left font-medium">
                {model ?? (kind === "other" ? `其余 ${model_count} 个模型` : "模型未上报")}
              </th>
              <td className="px-3 py-2 text-right tabular-nums">{totals.samples.toLocaleString()}</td>
              <UsageCell value={totals.prompt_tokens?.toLocaleString()} missing={totals.prompt_unknown_samples} />
              <UsageCell value={totals.completion_tokens?.toLocaleString()} missing={totals.completion_unknown_samples} />
              <UsageCell value={formatCost(totals.reported_cost_usd)} missing={totals.cost_unknown_samples} />
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
export function AssistantUsage() {
  const usage = useQuery({
    queryKey: ["assistant", "usage"],
    queryFn: () => invoke<Usage>("assistant_usage_get"),
    staleTime: 60_000,
  });
  const v = usage.isError ? undefined : usage.data;
  return (
    <section aria-label="助理任务与用量" className="space-y-3">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-medium">模型用量</h2>
        <button
          type="button"
          disabled={usage.isFetching}
          onClick={() => void usage.refetch({ cancelRefetch: false })}
          className="inline-flex items-center gap-2 rounded-lg border border-border px-3 py-2 text-xs hover:bg-secondary disabled:opacity-60"
        >
          <RefreshCw size={14} aria-hidden="true" className={usage.isFetching ? "animate-spin" : undefined} />
          刷新用量
        </button>
      </header>
      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        {[
          ["进行中的任务", v ? String(v.active) : "—", "桌面和微信的共享任务"],
          [
            "输入 Token",
            v?.prompt_tokens?.toLocaleString() ?? "未知",
            v?.prompt_unknown_samples
              ? `${v.prompt_unknown_samples} 次调用未上报输入用量`
              : "服务商已上报的合计",
          ],
          [
            "输出 Token",
            v?.completion_tokens?.toLocaleString() ?? "未知",
            v?.completion_unknown_samples
              ? `${v.completion_unknown_samples} 次调用未上报输出用量`
              : "服务商已上报的合计",
          ],
          [
            "已上报费用",
            v?.reported_cost_usd == null
              ? "未知"
              : `$${formatCost(v.reported_cost_usd)}`,
            v?.complete ? "USD · 服务商报告" : "USD · 部分统计，不代表完整账单",
          ],
        ].map(([label, value, hint]) => (
          <article
            key={label}
            className="rounded-xl border border-border bg-card p-5"
          >
            <h3 className="text-sm text-muted-foreground">{label}</h3>
            <p className="mt-3 font-mono text-2xl">
              {usage.isPending ? "…" : value}
            </p>
            <p className="mt-2 text-xs text-muted-foreground">{hint}</p>
          </article>
        ))}
      </div>
      {!!v?.groups?.length && <UsageBreakdown groups={v.groups} />}
      {usage.isError && (
        <p role="alert" className="text-sm">
          用量读取失败。{String(usage.error)}
        </p>
      )}
      {v && (
        <p className="text-xs text-muted-foreground">
          {v.scope}。
          {v.samples > 0 && `已取得 ${v.samples} 条上报记录。`}
          {v.unreported_turns > 0 &&
            `${v.unreported_turns} 个轮次没有用量记录。`}
          未知用量不会按零统计。
        </p>
      )}
    </section>
  );
}
