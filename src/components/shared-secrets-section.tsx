/**
 * Shared Secrets Pool Section · S3 · 2026-05-06
 *
 * /agents 页面新增 · 三个区块：
 * 1. IPC server 状态 · endpoint + token 长度（不显示 token 全文 · 安全）
 * 2. Secret 名列表（不解密）+「打开 SOPS 编辑器」按钮 · USER 自己增删改
 * 3. ACL 矩阵 · 4 agent × N secret · cell ✓ 永久允许 · 点 ✓ revoke
 */

import { useMemo } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertCircle,
  Check,
  ExternalLink,
  KeyRound,
  Loader2,
  Network,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { sharedSecretsApi } from "@/lib/tauri-api";
import { cn } from "@/lib/utils";
import { InfoTooltip } from "@/components/info-tooltip";

const AGENT_DISPLAY: Record<string, string> = {
  "claude-code": "Claude",
  codex: "Codex",
  "gemini-cli": "Gemini",
  openclaw: "OpenClaw",
};

export function SharedSecretsSection() {
  const queryClient = useQueryClient();

  const ipcQuery = useQuery({
    queryKey: ["shared-secrets-ipc"],
    queryFn: () => sharedSecretsApi.ipcEndpoint(),
    refetchOnWindowFocus: false,
    refetchInterval: (query) => query.state.data ? 60_000 : 10_000,
  });

  const namesQuery = useQuery({
    queryKey: ["shared-secrets-names"],
    queryFn: () => sharedSecretsApi.listNames(),
    refetchOnWindowFocus: false,
  });

  const aclQuery = useQuery({
    queryKey: ["shared-secrets-acl"],
    queryFn: () => sharedSecretsApi.aclList(),
    refetchOnWindowFocus: false,
  });

  const openYamlMut = useMutation({
    mutationFn: () => sharedSecretsApi.openYaml(),
  });

  const revokeMut = useMutation({
    mutationFn: (vars: { agentId: string; secretName: string }) =>
      sharedSecretsApi.aclRevoke(vars.agentId, vars.secretName),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["shared-secrets-acl"] });
    },
  });

  const matrix = aclQuery.data?.matrix;
  const allowedCount = useMemo(() => {
    if (!matrix) return 0;
    return matrix.cells.flat().filter((b) => b).length;
  }, [matrix]);

  return (
    <section aria-label="共享 Secrets">
      <div className="mb-3 flex items-center gap-2">
        <h2 className="font-serif text-lg font-semibold">共享 Secrets</h2>
        <InfoTooltip width={320}>
          4 个 agent 调用 <code className="font-mono">get_secret</code> 时会弹窗询问你是否授权。
          选“永久允许”后同一个 (agent, secret) 不再询问·记录在{" "}
          <code className="font-mono">~/.homebase/shared/secrets-acl.json</code>。
        </InfoTooltip>
        <button
          type="button"
          onClick={() => {
            ipcQuery.refetch();
            namesQuery.refetch();
            aclQuery.refetch();
          }}
          disabled={ipcQuery.isFetching || namesQuery.isFetching || aclQuery.isFetching}
          title="重新检查"
          className="ml-auto text-muted-foreground hover:text-foreground disabled:opacity-50"
        >
          <RefreshCw
            className={cn(
              "h-3.5 w-3.5",
              (namesQuery.isFetching || aclQuery.isFetching) && "animate-spin",
            )}
          />
        </button>
      </div>

      {/* 1. IPC 状态 · 只说“在运行”或“未启动”· 端口 + token 进 tooltip */}
      <div className="mb-4 flex items-center gap-2 rounded-lg border border-border bg-card px-3 py-2 text-xs">
        <Network className="h-3.5 w-3.5 text-muted-foreground" />
        {ipcQuery.isLoading ? (
          <span className="text-muted-foreground">检查中...</span>
        ) : ipcQuery.data ? (
          <>
            <span className="text-green-700 dark:text-green-500">●</span>
            <span className="text-foreground">授权服务运行中</span>
            <InfoTooltip width={260}>
              <div className="font-mono text-[10px]">
                <div>endpoint: {ipcQuery.data.endpoint}</div>
                <div className="mt-0.5">token: {ipcQuery.data.token.length} 个字符 (仅本机访问)</div>
              </div>
            </InfoTooltip>
          </>
        ) : (
          <span className="flex items-center gap-1 text-amber-700 dark:text-amber-400">
            <AlertCircle className="h-3.5 w-3.5" /> 未启动 · 请重启 HomeBase
          </span>
        )}
      </div>

      {/* 2. Secret 名列表 */}
      <div className="mb-4">
        <div className="mb-2 flex items-center justify-between">
          <div className="flex items-center gap-2 text-xs uppercase tracking-wider text-muted-foreground">
            <KeyRound className="h-3.5 w-3.5" />
            Secrets （{namesQuery.data?.length ?? 0}）
          </div>
          <button
            type="button"
            onClick={() => openYamlMut.mutate()}
            disabled={openYamlMut.isPending}
            className="flex items-center gap-1.5 rounded-md border border-border bg-card px-2.5 py-1 text-[11px] font-medium hover:bg-accent disabled:opacity-50"
          >
            <ExternalLink className="h-3 w-3" />
            打开 SOPS 编辑器
          </button>
        </div>
        {namesQuery.isError && (
          <div className="rounded-md border border-rose-500/30 bg-rose-500/5 p-2 text-xs text-rose-700 dark:text-rose-400">
            {String(namesQuery.error)}
          </div>
        )}
        {namesQuery.data && namesQuery.data.length === 0 && (
          <div className="rounded-md border border-amber-500/30 bg-amber-500/5 p-2 text-xs text-amber-700 dark:text-amber-400">
            secrets.yaml 中没找到任何 secret · 请先点上方按钮编辑添加。
          </div>
        )}
        {namesQuery.data && namesQuery.data.length > 0 && (
          <ul className="flex flex-wrap gap-1.5">
            {namesQuery.data.map((n) => (
              <li
                key={n}
                className="rounded-md border border-border bg-card px-2 py-0.5 font-mono text-[11px]"
              >
                {n}
              </li>
            ))}
          </ul>
        )}
      </div>

      {/* 3. 授权矩阵 */}
      <div>
        <div className="mb-2 flex items-center gap-2 text-xs uppercase tracking-wider text-muted-foreground">
          <Check className="h-3.5 w-3.5" />
          永久允许（{allowedCount}）
          <InfoTooltip>
            鼠标移到 ✓ 上点击可撤销该授权。撤销后下次同名调用会重新弹窗询问。
          </InfoTooltip>
        </div>
        {aclQuery.isLoading && (
          <div className="flex items-center gap-2 rounded-md border border-border bg-card p-3 text-xs text-muted-foreground">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            加载...
          </div>
        )}
        {aclQuery.isError && (
          <div className="rounded-md border border-rose-500/30 bg-rose-500/5 p-2 text-xs text-rose-700 dark:text-rose-400">
            {String(aclQuery.error)}
          </div>
        )}
        {matrix && matrix.secrets.length === 0 && (
          <div className="rounded-md border border-border bg-card p-3 text-xs text-muted-foreground">
            没有 secret · 矩阵无意义。
          </div>
        )}
        {matrix && matrix.secrets.length > 0 && (
          <div className="overflow-x-auto rounded-lg border border-border bg-card">
            <table className="w-full border-collapse text-xs">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="px-3 py-2 text-left font-medium text-muted-foreground">
                    Agent
                  </th>
                  {matrix.secrets.map((s) => (
                    <th
                      key={s}
                      className="px-2 py-2 text-center font-mono text-[10px] font-medium text-muted-foreground"
                      title={s}
                    >
                      {s.length > 14 ? `${s.slice(0, 12)}…` : s}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {matrix.agents.map((agent, ai) => (
                  <tr
                    key={agent}
                    className="border-t border-border first:border-0"
                  >
                    <td className="px-3 py-2 font-medium">
                      {AGENT_DISPLAY[agent] ?? agent}
                    </td>
                    {matrix.secrets.map((secret, si) => {
                      const allowed = matrix.cells[ai]?.[si] ?? false;
                      return (
                        <td
                          key={secret}
                          className="px-2 py-1.5 text-center"
                        >
                          {allowed ? (
                            <button
                              type="button"
                              onClick={() =>
                                revokeMut.mutate({
                                  agentId: agent,
                                  secretName: secret,
                                })
                              }
                              disabled={revokeMut.isPending}
                              title={`撤销 ${agent} 对 ${secret} 的永久允许`}
                              className="group inline-flex items-center justify-center rounded-md border border-green-500/30 bg-green-500/10 px-1.5 py-0.5 transition-colors hover:border-rose-500/40 hover:bg-rose-500/10"
                            >
                              <Check className="h-3 w-3 text-green-700 group-hover:hidden dark:text-green-500" />
                              <Trash2 className="hidden h-3 w-3 text-rose-700 group-hover:block dark:text-rose-400" />
                            </button>
                          ) : (
                            <span className="text-muted-foreground">·</span>
                          )}
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>
    </section>
  );
}
