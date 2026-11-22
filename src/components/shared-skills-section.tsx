/**
 * Shared Skills Pool Section · S4 · 2026-05-06
 *
 * 列 ~/.homebase/shared/skills/ 下的 skill · 让 USER 用资源管理器打开编辑 ·
 * Apply 后复制到 ~/.claude/skills/homebase-<id>/ + ~/.openclaw/skills/homebase-<id>/
 *
 * 不在 HomeBase 内做编辑器（让 USER 用 VSCode/Obsidian · 因为 SKILL.md 含 frontmatter
 * + 可能引用 supporting files · 文件管理器更方便）。
 */

import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertCircle,
  CheckCircle2,
  ExternalLink,
  FileText,
  Loader2,
  RefreshCw,
  Sparkles,
  XCircle,
} from "lucide-react";
import {
  sharedSkillsApi,
  type SharedSkillInfo,
  type SkillsReconcileEntry,
} from "@/lib/tauri-api";
import { cn } from "@/lib/utils";
import { refreshCapabilityCatalog } from "@/lib/capability-index";
import { InfoTooltip } from "@/components/info-tooltip";

const AGENT_DISPLAY: Record<string, string> = {
  "claude-code": "Claude Code",
  openclaw: "OpenClaw",
};

export function SharedSkillsSection() {
  const queryClient = useQueryClient();
  const skillsQuery = useQuery({
    queryKey: ["shared-skills"],
    queryFn: () => sharedSkillsApi.list(),
    refetchOnWindowFocus: false,
  });

  const [report, setReport] = useState<SkillsReconcileEntry[] | null>(null);

  const applyMut = useMutation({
    mutationFn: () => sharedSkillsApi.apply(),
    onSuccess: (r) => {
      setReport(r.agents);
      queryClient.invalidateQueries({ queryKey: ["shared-skills"] });
      refreshCapabilityCatalog(queryClient);
    },
  });

  const openDirMut = useMutation({
    mutationFn: () => sharedSkillsApi.openDir(),
  });

  const skills = skillsQuery.data ?? [];

  return (
    <section aria-label="共享 Skills 池">
      <div className="mb-3 flex items-center gap-2">
        <h2 className="font-serif text-lg font-semibold">共享 Skills 池</h2>
        <InfoTooltip width={320}>
          在 <code className="font-mono">~/.homebase/shared/skills/&lt;id&gt;/SKILL.md</code> 编辑 ·
          点“应用”后复制到{" "}
          <code className="font-mono">~/.claude/skills/homebase-&lt;id&gt;/</code> 和{" "}
          <code className="font-mono">~/.openclaw/skills/homebase-&lt;id&gt;/</code>。仅推送给
          Claude / OpenClaw（Codex / Gemini 本身不支持 skills）。
        </InfoTooltip>
      </div>

      {/* 操作按钮 */}
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => openDirMut.mutate()}
          disabled={openDirMut.isPending}
          className="flex items-center gap-1.5 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-foreground transition-colors hover:bg-secondary"
        >
          <ExternalLink className="h-3.5 w-3.5" />
          在资源管理器打开 · 编辑 SKILL.md
        </button>
        <button
          type="button"
          onClick={() => skillsQuery.refetch()}
          disabled={skillsQuery.isFetching}
          className="flex items-center gap-1.5 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        >
          <RefreshCw className={cn("h-3.5 w-3.5", skillsQuery.isFetching && "animate-spin")} />
          刷新列表
        </button>
        <button
          type="button"
          onClick={() => applyMut.mutate()}
          disabled={applyMut.isPending || skills.length === 0}
          className={cn(
            "flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition-colors",
            skills.length === 0
              ? "cursor-not-allowed bg-secondary text-muted-foreground"
              : "bg-primary-600 text-white hover:bg-primary-700 dark:bg-primary-500 dark:hover:bg-primary-600",
          )}
        >
          {applyMut.isPending ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <CheckCircle2 className="h-3.5 w-3.5" />
          )}
          {skills.length === 0 ? "无 skill 可应用" : `应用 ${skills.length} 个 skill 到 2 agent`}
        </button>
      </div>

      {/* Skill 列表 */}
      {skillsQuery.isLoading && (
        <div className="flex items-center gap-2 rounded-lg border border-border bg-card p-4 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          扫描 ~/.homebase/shared/skills/...
        </div>
      )}

      {skillsQuery.isError && (
        <div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive">
          <AlertCircle className="h-3.5 w-3.5 shrink-0" />
          <div>列出失败：{String(skillsQuery.error)}</div>
        </div>
      )}

      {skills.length === 0 && !skillsQuery.isLoading && (
        <div className="rounded-lg border border-dashed border-border bg-card p-6 text-center text-sm text-muted-foreground">
          <Sparkles className="mx-auto mb-2 h-6 w-6 opacity-50" />
          <p>共享 skill 池为空</p>
          <p className="mt-1 text-xs">点上方“在资源管理器打开”· 创建子目录并写 SKILL.md 即可。</p>
        </div>
      )}

      {skills.length > 0 && (
        <ul className="space-y-1">
          {skills.map((s) => (
            <SkillRow key={s.id} skill={s} />
          ))}
        </ul>
      )}

      {/* 错误 */}
      {applyMut.isError && (
        <div className="mt-3 flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive">
          <AlertCircle className="h-3.5 w-3.5 shrink-0" />
          <div>
            <p className="font-medium">应用失败</p>
            <p className="mt-0.5">{String(applyMut.error)}</p>
          </div>
        </div>
      )}

      {/* Report */}
      {report && (
        <div className="mt-3 rounded-lg border border-border bg-card p-3">
          <div className="mb-2 flex items-center justify-between">
            <h3 className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
              推送报告
            </h3>
            <button
              type="button"
              onClick={() => setReport(null)}
              className="text-[10px] text-muted-foreground hover:text-foreground"
            >
              关闭
            </button>
          </div>
          <ul className="space-y-1.5">
            {report.map((r) => {
              const display = AGENT_DISPLAY[r.agent_id] ?? r.agent_id;
              const Icon =
                r.status === "applied"
                  ? CheckCircle2
                  : r.status === "skipped"
                    ? AlertCircle
                    : XCircle;
              const color =
                r.status === "applied"
                  ? "text-green-600 dark:text-green-500"
                  : r.status === "skipped"
                    ? "text-amber-600 dark:text-amber-500"
                    : "text-destructive";
              return (
                <li key={r.agent_id} className="flex items-start gap-2 text-xs">
                  <Icon className={cn("h-3.5 w-3.5 shrink-0 mt-0.5", color)} />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-baseline gap-2">
                      <span className="font-medium">{display}</span>
                      <span className={cn("font-mono uppercase text-[10px]", color)}>
                        {r.status}
                      </span>
                      {r.status === "applied" && (
                        <span className="text-muted-foreground">
                          {r.copied_count} skill
                        </span>
                      )}
                    </div>
                    <div className="truncate font-mono text-[10px] text-muted-foreground">
                      {r.path}
                    </div>
                    {r.message && (
                      <div className="mt-0.5 text-[11px] text-muted-foreground">
                        {r.message}
                      </div>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </section>
  );
}

function SkillRow({ skill }: { skill: SharedSkillInfo }) {
  const sizeKB = (skill.size_bytes / 1024).toFixed(1);
  const modified = skill.modified_at
    ? new Date(skill.modified_at * 1000).toLocaleDateString()
    : null;

  return (
    <li className="flex items-center gap-3 rounded-lg border border-border bg-card px-3 py-2">
      <FileText className="h-4 w-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="font-mono text-sm font-medium">{skill.id}</span>
          {!skill.has_skill_md && (
            <span className="rounded-md border border-amber-500/30 bg-amber-500/5 px-1.5 py-0.5 font-mono text-[10px] text-amber-700 dark:text-amber-400">
              无 SKILL.md
            </span>
          )}
        </div>
        <div className="font-mono text-[10px] text-muted-foreground">
          {skill.has_skill_md ? `${sizeKB} KB` : "—"}
          {modified && <span> · 修改于 {modified}</span>}
        </div>
      </div>
    </li>
  );
}
