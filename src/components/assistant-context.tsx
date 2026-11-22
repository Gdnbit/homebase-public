import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Layers, X } from "lucide-react";
import { conversationsApi } from "@/lib/conversations";

export function AssistantContext({ sessionId }: { sessionId?: string | null }) {
  const [open, setOpen] = useState(false);
  const context = useQuery({
    queryKey: ["assistant", "context", sessionId],
    queryFn: () => conversationsApi.context(sessionId ?? undefined),
    enabled: open,
    staleTime: 30_000,
  });
  return (
    <div className="relative shrink-0">
      <button
        type="button"
        className="inline-flex items-center gap-1.5 rounded-lg px-2 py-2 text-xs text-muted-foreground hover:bg-secondary hover:text-foreground"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <Layers size={14} />
        上下文自动组织
      </button>
      {open && (
        <section
          aria-label="本次上下文"
          className="absolute right-0 top-full z-40 mt-2 w-[min(440px,calc(100vw-90px))] rounded-xl border border-border bg-card p-5 text-sm shadow-xl"
        >
          <div className="flex items-center justify-between">
            <h2 className="font-medium">这次对话可以使用什么</h2>
            <button
              type="button"
              aria-label="关闭上下文"
              onClick={() => setOpen(false)}
            >
              <X size={16} />
            </button>
          </div>
          <p className="mt-2 text-muted-foreground">
            每轮按当前会话组织任务、相关记忆和能力索引；资料与工具说明按需读取，权限由后端实时核对。
          </p>
          {context.isPending && <p role="status">正在读取上下文摘要…</p>}
          {context.isError && (
            <p role="alert">上下文摘要读取失败：{String(context.error)}</p>
          )}
          {context.data && (
            <dl className="mt-3 grid grid-cols-2 gap-2 text-xs">
              <dt>当前会话任务</dt>
              <dd>
                {context.data.tasks_total ?? context.data.tasks.length} 条
                {context.data.tasks_has_more &&
                  ` · 摘要展示 ${context.data.tasks.length} 条`}
              </dd>
              {!!context.data.queued_tasks && (
                <>
                  <dt>等候处理</dt>
                  <dd>{context.data.queued_tasks} 条 · 按原队列继续</dd>
                </>
              )}
              <dt>已授权工作区</dt>
              <dd>
                {context.data.authorized_workspaces_total ??
                  context.data.authorized_workspaces.length}{" "}
                个
                {context.data.authorized_workspaces_has_more &&
                  ` · 摘要展示 ${context.data.authorized_workspaces.length} 个`}{" "}
                · 默认会话工作区始终可查
              </dd>
              <dt>记忆</dt>
              <dd>发送消息后按内容检索，不整库注入</dd>
              <dt>资料与扩展</dt>
              <dd>按需发现文档、MCP 与 Skill</dd>
              <dt>权限版本</dt>
              <dd>{context.data.policy_revision}</dd>
              {context.data.summary_only && (
                <>
                  <dt>摘要范围</dt>
                  <dd>
                    {context.data.context_has_more
                      ? "部分内容已省略；完整进度和结果按原任务查询"
                      : "当前会话工作摘要；完整内容仍在原任务中"}
                  </dd>
                </>
              )}
            </dl>
          )}
        </section>
      )}
    </div>
  );
}
