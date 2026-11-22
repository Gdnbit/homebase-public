import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  Activity as ActivityIcon,
  Network,
  RotateCcw,
  ShieldCheck,
  TableProperties,
} from "lucide-react";
import { EmptyPlaceholder } from "@/components/empty-placeholder";
import {
  governanceApi,
  type SnapshotManifest,
  type AuditEvent,
  type CapabilityMatrix,
  type ConfigOwnership,
  type InterfaceEntry,
  type InterfaceProbeResult,
} from "@/lib/tauri-api";
import { formatRelativeTime } from "@/lib/utils";

export function Activity() {
  const [message, setMessage] = useState<string | null>(null);
  const [probeMessage, setProbeMessage] = useState<string | null>(null);
  const [probeResult, setProbeResult] = useState<InterfaceProbeResult | null>(null);
  const [customBaseUrl, setCustomBaseUrl] = useState("");
  const [customSecret, setCustomSecret] = useState("");

  const auditQuery = useQuery({
    queryKey: ["governance", "audit"],
    queryFn: () => governanceApi.auditRecent(80),
  });
  const snapshotsQuery = useQuery({
    queryKey: ["governance", "snapshots"],
    queryFn: () => governanceApi.snapshotsList(30),
  });
  const capabilityQuery = useQuery({
    queryKey: ["governance", "capability"],
    queryFn: () => governanceApi.capabilityMatrix(),
  });
  const ownershipQuery = useQuery({
    queryKey: ["governance", "ownership"],
    queryFn: () => governanceApi.configOwnership(),
  });
  const interfaceQuery = useQuery({
    queryKey: ["governance", "interfaces"],
    queryFn: () => governanceApi.interfaceRegistry(),
  });

  const exportManifest = async () => {
    setMessage(null);
    const manifest = await governanceApi.machineManifestExport();
    setMessage(
      `Machine manifest exported · ${manifest.runtimes.length} runtimes · ${manifest.agents.length} agents`,
    );
  };

  const rollback = async (snapshot: SnapshotManifest) => {
    const ok = window.confirm(
      `Rollback snapshot ${snapshot.snapshot_id}? This restores ${snapshot.files.length} recorded path(s).`,
    );
    if (!ok) return;
    setMessage(null);
    const result = await governanceApi.snapshotRollback(snapshot.snapshot_id);
    setMessage(`Rollback completed · ${result.files.length} path(s) restored`);
    await Promise.all([snapshotsQuery.refetch(), auditQuery.refetch()]);
  };

  const probeInterface = async (
    entry: Pick<InterfaceEntry, "kind" | "base_url" | "display_name">,
    overrideBaseUrl?: string,
    secretShortName?: string,
  ) => {
    const baseUrl = (overrideBaseUrl ?? entry.base_url ?? "").trim();
    if (!baseUrl) {
      setProbeMessage("需要先填写 base_url");
      return;
    }
    setProbeMessage(`Probing ${entry.display_name}...`);
    setProbeResult(null);
    try {
      const result = await governanceApi.interfaceProbe({
        kind: entry.kind,
        base_url: baseUrl,
        secret_short_name: secretShortName?.trim() || null,
      });
      setProbeResult(result);
      setProbeMessage(
        result.ok
          ? `Probe ok · ${result.models.length} model(s) · ${result.latency_ms}ms`
          : `Probe failed · ${result.error ?? "unknown error"}`,
      );
      await auditQuery.refetch();
    } catch (error) {
      setProbeMessage(String(error));
    }
  };

  return (
    <div className="animate-fade-in space-y-8">
      <header className="hb-page-header">
        <span className="hb-section-label">Activity</span>
        <h1>治理与追溯</h1>
        <p>审计日志、配置快照、能力矩阵与迁移清单的 P0 控制面。</p>
      </header>

      <section className="flex flex-wrap items-center gap-2">
        <button className="hb-btn-primary" type="button" onClick={exportManifest}>
          <ShieldCheck className="h-4 w-4" />
          导出 Machine Manifest
        </button>
        <button
          className="hb-btn-outline"
          type="button"
          disabled={[auditQuery, snapshotsQuery, capabilityQuery, ownershipQuery, interfaceQuery].some(query => query.isFetching)}
          onClick={() => {
            auditQuery.refetch({ cancelRefetch: false });
            snapshotsQuery.refetch({ cancelRefetch: false });
            capabilityQuery.refetch({ cancelRefetch: false });
            ownershipQuery.refetch({ cancelRefetch: false });
            interfaceQuery.refetch({ cancelRefetch: false });
          }}
        >
          刷新
        </button>
        {message && <span className="font-mono text-xs text-success">{message}</span>}
      </section>

      <CapabilitySection
        rows={capabilityQuery.data?.rows ?? []}
        loading={capabilityQuery.isLoading}
        error={capabilityQuery.error}
      />

      <OwnershipSection
        ownership={ownershipQuery.data}
        loading={ownershipQuery.isLoading}
        error={ownershipQuery.error}
      />

      <InterfaceSection
        interfaces={interfaceQuery.data?.interfaces ?? []}
        loading={interfaceQuery.isLoading}
        error={interfaceQuery.error}
        customBaseUrl={customBaseUrl}
        customSecret={customSecret}
        probeMessage={probeMessage}
        probeResult={probeResult}
        onCustomBaseUrlChange={setCustomBaseUrl}
        onCustomSecretChange={setCustomSecret}
        onProbe={probeInterface}
      />

      <SnapshotsSection
        snapshots={snapshotsQuery.data ?? []}
        loading={snapshotsQuery.isLoading}
        error={snapshotsQuery.error}
        onRollback={rollback}
      />

      <AuditSection
        events={auditQuery.data ?? []}
        loading={auditQuery.isLoading}
        error={auditQuery.error}
      />
    </div>
  );
}

function CapabilitySection({
  rows,
  loading,
  error,
}: {
  rows: CapabilityMatrix["rows"];
  loading: boolean;
  error: unknown;
}) {
  return (
    <section aria-label="Capability Matrix">
      <div className="mb-3 flex items-center gap-2">
        <TableProperties
          className="h-4 w-4 text-primary-600 dark:text-primary-400"
          aria-hidden
        />
        <h2 className="font-serif text-lg font-semibold">Capability Matrix</h2>
      </div>
      {loading ? (
        <PanelSkeleton />
      ) : error ? (
        <EmptyPlaceholder title="能力矩阵读取失败" description={String(error)} icon="⚠" />
      ) : rows.length === 0 ? (
        <EmptyPlaceholder title="暂无能力矩阵" description="后端未返回 actor 行。" icon="▦" />
      ) : (
        <div className="overflow-hidden rounded-md border border-border">
          <table className="w-full border-collapse text-sm">
            <thead className="bg-muted/60 text-left text-xs text-muted-foreground">
              <tr>
                <th className="px-3 py-2">Actor</th>
                <th className="px-3 py-2">Kind</th>
                <th className="px-3 py-2">MCP</th>
                <th className="px-3 py-2">Skills</th>
                <th className="px-3 py-2">Secrets</th>
                <th className="px-3 py-2">Risk</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.actor_id} className="border-t border-border">
                  <td className="px-3 py-2 font-mono">{row.actor_id}</td>
                  <td className="px-3 py-2 text-muted-foreground">{row.actor_kind}</td>
                  <td className="px-3 py-2">{compactList(row.mcp_servers)}</td>
                  <td className="px-3 py-2">{compactList(row.skills)}</td>
                  <td className="px-3 py-2">{compactList(row.allowed_secrets)}</td>
                  <td className="px-3 py-2 font-mono">{row.risk}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function OwnershipSection({
  ownership,
  loading,
  error,
}: {
  ownership: ConfigOwnership | undefined;
  loading: boolean;
  error: unknown;
}) {
  const entries = ownership?.entries ?? [];
  return (
    <section aria-label="Config Ownership">
      <div className="mb-3 flex items-center gap-2">
        <ShieldCheck
          className="h-4 w-4 text-primary-600 dark:text-primary-400"
          aria-hidden
        />
        <h2 className="font-serif text-lg font-semibold">Config Ownership</h2>
      </div>
      {loading ? (
        <PanelSkeleton />
      ) : error ? (
        <EmptyPlaceholder title="配置归属读取失败" description={String(error)} icon="⚠" />
      ) : entries.length === 0 ? (
        <EmptyPlaceholder title="暂无配置归属" description="后端未返回 ownership 条目。" icon="▤" />
      ) : (
        <div className="overflow-hidden rounded-md border border-border">
          <table className="w-full border-collapse text-sm">
            <thead className="bg-muted/60 text-left text-xs text-muted-foreground">
              <tr>
                <th className="px-3 py-2">Path</th>
                <th className="px-3 py-2">Owner</th>
                <th className="px-3 py-2">Policy</th>
                <th className="px-3 py-2">Secret</th>
                <th className="px-3 py-2">Exists</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((entry) => (
                <tr key={`${entry.owner}:${entry.path}`} className="border-t border-border">
                  <td className="max-w-[360px] truncate px-3 py-2 font-mono" title={entry.notes}>
                    {entry.path}
                  </td>
                  <td className="px-3 py-2">
                    <span className="font-medium">{entry.owner}</span>
                    <span className="block text-xs text-muted-foreground">{entry.managed_by}</span>
                  </td>
                  <td className="px-3 py-2 font-mono text-xs">{entry.write_policy}</td>
                  <td className="px-3 py-2">
                    {entry.plaintext_secret_allowed ? "plaintext allowed" : "no plaintext"}
                  </td>
                  <td className="px-3 py-2">{entry.exists ? "yes" : "missing"}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function InterfaceSection({
  interfaces,
  loading,
  error,
  customBaseUrl,
  customSecret,
  probeMessage,
  probeResult,
  onCustomBaseUrlChange,
  onCustomSecretChange,
  onProbe,
}: {
  interfaces: InterfaceEntry[];
  loading: boolean;
  error: unknown;
  customBaseUrl: string;
  customSecret: string;
  probeMessage: string | null;
  probeResult: InterfaceProbeResult | null;
  onCustomBaseUrlChange: (value: string) => void;
  onCustomSecretChange: (value: string) => void;
  onProbe: (
    entry: Pick<InterfaceEntry, "kind" | "base_url" | "display_name">,
    overrideBaseUrl?: string,
    secretShortName?: string,
  ) => void;
}) {
  return (
    <section aria-label="Interface Registry">
      <div className="mb-3 flex items-center gap-2">
        <Network
          className="h-4 w-4 text-primary-600 dark:text-primary-400"
          aria-hidden
        />
        <h2 className="font-serif text-lg font-semibold">Interface Registry</h2>
      </div>
      {loading ? (
        <PanelSkeleton />
      ) : error ? (
        <EmptyPlaceholder title="接口注册表读取失败" description={String(error)} icon="⚠" />
      ) : (
        <div className="space-y-3">
          <div className="grid gap-3 md:grid-cols-2">
            {interfaces.map((entry) => (
              <article key={entry.id} className="hb-card">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <h3 className="truncate text-sm font-semibold">{entry.display_name}</h3>
                    <p className="mt-1 font-mono text-xs text-muted-foreground">
                      {entry.kind} · {entry.risk}
                    </p>
                  </div>
                  {entry.base_url && (
                    <button
                      className="hb-btn-outline shrink-0"
                      type="button"
                      onClick={() =>
                        onProbe(
                          entry,
                          entry.base_url ?? undefined,
                          entry.id === "openrouter-anthropic" ? "openrouter_key" : undefined,
                        )
                      }
                    >
                      Probe
                    </button>
                  )}
                </div>
                <p className="mt-3 truncate font-mono text-xs text-muted-foreground">
                  {entry.base_url ?? "manual base_url required"}
                </p>
                <p className="mt-2 text-xs text-muted-foreground">{entry.notes}</p>
              </article>
            ))}
          </div>

          <div className="rounded-md border border-border p-3">
            <div className="grid gap-2 md:grid-cols-[1fr_180px_auto]">
              <input
                className="w-full rounded border border-border bg-background px-2.5 py-1.5 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
                value={customBaseUrl}
                onChange={(e) => onCustomBaseUrlChange(e.target.value)}
                placeholder="https://example.com/v1"
                aria-label="Custom interface base URL"
              />
              <input
                className="w-full rounded border border-border bg-background px-2.5 py-1.5 font-mono text-[11px] focus:border-primary-500 focus:outline-none focus:ring-1 focus:ring-primary-500"
                value={customSecret}
                onChange={(e) => onCustomSecretChange(e.target.value)}
                placeholder="secret_short_name"
                aria-label="SOPS secret short name"
              />
              <button
                className="hb-btn-primary"
                type="button"
                onClick={() =>
                  onProbe(
                    {
                      kind: "openai-compatible",
                      base_url: customBaseUrl,
                      display_name: "Custom Interface",
                    },
                    customBaseUrl,
                    customSecret,
                  )
                }
              >
                Probe Custom
              </button>
            </div>
          </div>

          {(probeMessage || probeResult) && (
            <div className="rounded-md border border-border bg-muted/30 p-3">
              {probeMessage && <p className="font-mono text-xs">{probeMessage}</p>}
              {probeResult && (
                <div className="mt-2 space-y-1 font-mono text-xs text-muted-foreground">
                  <p>{probeResult.attempted_urls.join(" | ")}</p>
                  <p>
                    {probeResult.models.length > 0
                      ? probeResult.models.slice(0, 12).join(", ")
                      : probeResult.body_preview ?? probeResult.error}
                  </p>
                </div>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function SnapshotsSection({
  snapshots,
  loading,
  error,
  onRollback,
}: {
  snapshots: SnapshotManifest[];
  loading: boolean;
  error: unknown;
  onRollback: (snapshot: SnapshotManifest) => void;
}) {
  return (
    <section aria-label="Snapshots">
      <div className="mb-3 flex items-center gap-2">
        <RotateCcw
          className="h-4 w-4 text-primary-600 dark:text-primary-400"
          aria-hidden
        />
        <h2 className="font-serif text-lg font-semibold">配置快照</h2>
      </div>
      {loading ? (
        <PanelSkeleton />
      ) : error ? (
        <EmptyPlaceholder title="快照读取失败" description={String(error)} icon="⚠" />
      ) : snapshots.length === 0 ? (
        <EmptyPlaceholder
          title="暂无快照"
          description="执行 MCP 或 Skills 写入后会自动生成。"
          icon="↺"
        />
      ) : (
        <div className="space-y-2">
          {snapshots.map((snapshot) => (
            <article key={snapshot.snapshot_id} className="hb-card">
              <div className="flex items-start justify-between gap-4">
                <div className="min-w-0">
                  <h3 className="truncate font-mono text-sm font-semibold">
                    {snapshot.snapshot_id}
                  </h3>
                  <p className="mt-1 text-xs text-muted-foreground">
                    {snapshot.reason} · {formatRelativeTime(Date.parse(snapshot.created_at))}
                  </p>
                  <p className="mt-2 font-mono text-xs text-muted-foreground">
                    {snapshot.files.length} path(s)
                  </p>
                </div>
                <button
                  className="hb-btn-outline shrink-0"
                  type="button"
                  onClick={() => onRollback(snapshot)}
                >
                  回滚
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function AuditSection({
  events,
  loading,
  error,
}: {
  events: AuditEvent[];
  loading: boolean;
  error: unknown;
}) {
  return (
    <section aria-label="Audit Log">
      <div className="mb-3 flex items-center gap-2">
        <ActivityIcon
          className="h-4 w-4 text-primary-600 dark:text-primary-400"
          aria-hidden
        />
        <h2 className="font-serif text-lg font-semibold">Audit Log</h2>
      </div>
      {loading ? (
        <PanelSkeleton />
      ) : error ? (
        <EmptyPlaceholder title="审计日志读取失败" description={String(error)} icon="⚠" />
      ) : events.length === 0 ? (
        <EmptyPlaceholder
          title="暂无审计事件"
          description="配置写入、回滚、manifest 导出会记录到这里。"
          icon="◎"
        />
      ) : (
        <div className="space-y-2">
          {events.map((event) => (
            <article key={event.correlation_id} className="hb-card">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span className="font-mono text-sm font-semibold">{event.event}</span>
                <span className="font-mono text-xs text-muted-foreground">{event.result}</span>
                <span className="font-mono text-xs text-muted-foreground">{event.risk}</span>
                <span className="font-mono text-xs text-muted-foreground">
                  {formatRelativeTime(Date.parse(event.ts))}
                </span>
              </div>
              <p className="mt-2 truncate font-mono text-xs text-muted-foreground">
                {event.target}
              </p>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function PanelSkeleton() {
  return (
    <div className="space-y-2" aria-busy>
      {Array.from({ length: 3 }).map((_, i) => (
        <div key={i} className="hb-card">
          <div className="h-4 w-1/3 animate-pulse rounded bg-muted" />
          <div className="mt-3 h-3 w-2/3 animate-pulse rounded bg-muted" />
        </div>
      ))}
    </div>
  );
}

function compactList(items: string[]) {
  if (items.length === 0) return <span className="text-muted-foreground">-</span>;
  if (items.length <= 2) return <span>{items.join(", ")}</span>;
  return (
    <span title={items.join(", ")}>
      {items.slice(0, 2).join(", ")} +{items.length - 2}
    </span>
  );
}
