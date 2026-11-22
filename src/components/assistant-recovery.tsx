import type { AssistantStepSummary, AssistantTurn } from "@/lib/assistant";
import { useState, type ReactNode } from "react";

interface OriginalInspection {
  steps: AssistantStepSummary[];
  unverifiedJobs: number;
}

export function completedPlanNeedsReview(
  turn: Pick<AssistantTurn, "state" | "progress">,
) {
  const progress = turn.progress;
  return turn.state === "completed" && !!progress &&
    progress.completed_stages < progress.total_stages;
}

export const needsRecoveryNote = (
  turn: Pick<AssistantTurn, "state" | "progress" | "model_outcome_unknown" | "pending_accepted_jobs">,
) =>
  completedPlanNeedsReview(turn) ||
  (turn.state === "canceled" &&
    (turn.model_outcome_unknown === true || (turn.pending_accepted_jobs ?? 0) > 0)) ||
  [
    "failed",
    "interrupted_unknown",
    "paused_budget",
    "needs_input",
    "unknown",
  ].includes(turn.state);

export function AssistantRecovery({
  turn,
  busy,
  modelEnabled,
  onInspect,
  renderStep,
  onResume,
}: {
  turn: AssistantTurn;
  busy: boolean;
  modelEnabled: boolean;
  onInspect: () => Promise<OriginalInspection>;
  renderStep: (step: AssistantStepSummary) => ReactNode;
  onResume: () => void;
}) {
  const [reading, setReading] = useState(false);
  const [readError, setReadError] = useState("");
  const [inspection, setInspection] = useState<OriginalInspection | null>(null);
  async function inspect() {
    if (busy || reading) return;
    setReading(true);
    setReadError("");
    setInspection(null);
    try {
      setInspection(await onInspect());
    } catch (error) {
      setReadError(String(error));
    } finally {
      setReading(false);
    }
  }
  if (!needsRecoveryNote(turn)) return null;
  const unverifiedJobs = inspection?.unverifiedJobs ?? 0;
  const unverified = Math.min(unverifiedJobs, turn.pending_accepted_jobs ?? unverifiedJobs);
  const pending = turn.state === "canceled" ? (turn.pending_accepted_jobs ?? 0) - unverified : 0;
  const reason =
    turn.progress?.blocked ||
    turn.task?.stop?.message ||
    (turn.state === "canceled" && (pending > 0 || unverified > 0) ? "已停止等待。" : "这件事暂时停下了，原记录和已有结果保留。");
  const next =
    turn.progress?.next ||
    turn.task?.stop?.next_action ||
    "先核对原任务的状态和已有结果。";
  const button =
    "rounded-lg border border-border px-3 py-2 text-xs hover:bg-secondary disabled:opacity-50";
  return (
    <section
      aria-label="任务恢复说明"
      className="mt-3 space-y-2 rounded-xl border border-border bg-muted/20 p-3 text-sm"
    >
      <p>{reason}</p>
      {pending > 0 && <p>还有 {pending} 项原任务尚未结束。</p>}
      <p className="text-xs text-muted-foreground">{next}</p>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className={button}
          disabled={busy || reading}
          onClick={() => void inspect()}
        >
          核对原任务
        </button>
        {turn.resumable && (
          <button
            type="button"
            className={button}
            disabled={busy || reading || !modelEnabled}
            onClick={onResume}
          >
            继续原任务
          </button>
        )}
        {turn.resumable && !modelEnabled && (
          <span className="text-xs text-muted-foreground">
            模型尚未启用，暂时只能核对原记录。
          </span>
        )}
      </div>
      <div
        role="status"
        aria-atomic="true"
        className={inspection ? "space-y-1 text-xs text-muted-foreground" : "sr-only"}
      >
        {inspection && (
          <p>{inspection.steps.length ? "已核对原记录。" : "已核对原记录。原记录没有保存步骤。"}</p>
        )}
        {unverified > 0 && (
          <p>{unverified} 项原任务暂未查到最新状态，可以稍后再核对。已确认的结果仍可查看。</p>
        )}
      </div>
      {!!inspection?.steps.length && (
        <details className="text-xs text-muted-foreground">
          <summary className="cursor-pointer">原步骤记录 · {inspection.steps.length} 步</summary>
          <div className="mt-2 space-y-2">{inspection.steps.map(renderStep)}</div>
        </details>
      )}
      {readError && (
        <p role="alert" className="break-words text-xs text-destructive">
          {readError}
        </p>
      )}
    </section>
  );
}
