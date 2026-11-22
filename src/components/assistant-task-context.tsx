import type { AssistantTaskContext } from "@/lib/assistant-task-context";
import { CapabilityContextDetails } from "./capability-context-view";

export function AssistantTaskContextDetails({
  view,
  sent = false,
}: {
  view: AssistantTaskContext;
  sent?: boolean;
}) {
  if (view.kind === "capability")
    return <CapabilityContextDetails view={view} sent={sent} />;
  const payload = view.payload;
  return (
    <details
      className="min-w-0 rounded-lg border border-border bg-secondary/20 px-3 py-2 text-xs"
      aria-label="原工作流资料"
    >
      <summary className="cursor-pointer break-words text-muted-foreground">
        <span>{sent ? "附带工作流资料" : "工作流资料已保留"}</span>
        <span className="ml-2">
          {payload.workflow === "inbox"
            ? "资料收录"
            : payload.source.task_id
              ? "文稿原任务"
              : "文稿草稿"}
        </span>
        {payload.workflow === "inbox" && payload.source.uncertain && (
          <span className="ml-2 text-amber-600 dark:text-amber-400">
            待核对
          </span>
        )}
      </summary>
      <div className="mt-3 space-y-2">
        {payload.workflow === "inbox" && payload.source.uncertain && (
          <p className="text-muted-foreground">
            原收录或附件保存尚未确认，需先核对原记录与原附件，不自动重新收录或上传。
          </p>
        )}
        {payload.workflow === "studio" && (
          <p className="break-words">
            历史仅携带 {payload.source.excerpts.length} 条有界摘录；原页共{" "}
            {payload.source.total_messages} 条，不代表已读取完整记录。
          </p>
        )}
        <p className="text-muted-foreground">
          资料来自输入文本，不认证来源或授予权限。已有任务先查询原编号，结果未明时不重复提交；附件仅保留原引用。完整正文可通过全文编辑查看。
        </p>
        <pre
          className="max-h-48 overflow-auto whitespace-pre-wrap break-all rounded-md bg-background/70 p-3 text-[11px]"
          aria-label="工作流资料原文"
        >
          {view.json}
        </pre>
      </div>
    </details>
  );
}
