import { useEffect, useRef, useState } from "react";
import {
  useIsMutating,
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import {
  Box,
  Check,
  Copy,
  ExternalLink,
  Github,
  Loader2,
  Play,
  RefreshCw,
  X,
} from "lucide-react";

type Failure = { code: string; message: string };
type Project = {
  id: string;
  name: string;
  repository: string;
  phase: string;
  revision: number;
  version?: string;
  observed_state?: string;
  notice?: string;
  error?: Failure;
  capability_id?: string;
  operation?: { request_id: string };
  source?: {
    supported: boolean;
    description: string;
    license: string;
    source_commit: string;
    reason: string;
    package?: { name: string; version: string };
    profile?: { examples: unknown[] };
    views: { id: string; name: string; url: string; pinned: boolean }[];
  };
  verification?: { passed: boolean; tests: number };
};
type Capability = {
  id: string;
  name: string;
  description: string;
  version: string;
  repository: string;
  license: string;
  state: string;
};
type Page<T> = {
  status: string;
  entries: T[];
  total: number;
  next_offset: number | null;
  error?: Failure;
};
type Result = {
  status: string;
  run_id?: string;
  stdout?: string;
  message?: string;
  error?: Failure;
  artifact?: { path: string; sha256: string };
};
type Description = {
  status: string;
  capability: Capability & {
    input_schema: unknown;
    examples: { input: unknown }[];
    notice: string;
  };
  verification: { passed: boolean };
};
const active = new Set([
  "analyzing",
  "preflighting",
  "installing",
  "wrapping",
  "verifying",
  "registering",
]);
const labels: Record<string, string> = {
  analyzing: "正在分析",
  analyzed: "待安装验收",
  preflighting: "正在检查隔离环境",
  installing: "正在安装",
  wrapping: "正在封装",
  verifying: "正在验收",
  registering: "正在注册",
  registered: "已通过验收",
  needs_adapter: "需要补充驱动",
  failed: "未完成",
  canceled: "已取消",
  interrupted_unknown: "原操作结果待核对",
};
async function api<T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  const value = await invoke<T & { status?: string; error?: Failure }>(
    command,
    args,
  );
  if (!value || (value.status === "failed" && value.error))
    throw Error(value?.error?.message || "能力接口响应无效");
  return value;
}
function remembered() {
  try {
    return sessionStorage.getItem("homebase.repo.selected") || "";
  } catch {
    return "";
  }
}

export function RepositoryCapabilities() {
  const client = useQueryClient();
  const [url, setUrl] = useState("");
  const [selected, setSelected] = useState(remembered);
  const [offset, setOffset] = useState(0);
  const [capOffset, setCapOffset] = useState(0);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [examples, setExamples] = useState("");
  const [trying, setTrying] = useState<Capability | null>(null);
  const gate = useRef(false);
  const [submitting, setSubmitting] = useState(false);
  const sharedBusy =
    useIsMutating({ mutationKey: ["repo-capabilities", "operation"] }) > 0;
  const list = useQuery({
    queryKey: ["repo-capabilities", "projects", offset],
    queryFn: async () => {
      const r = await api<Page<Project>>("repo_projects_list", {
        offset,
        limit: 6,
      });
      if (!Array.isArray(r.entries)) throw Error("项目列表格式无效");
      return r;
    },
    refetchInterval: (q) =>
      submitting ||
      sharedBusy ||
      q.state.data?.entries.some(
        (p) =>
          active.has(p.phase) && p.observed_state !== "interrupted_unknown",
      )
        ? 2500
        : false,
  });
  const caps = useQuery({
    queryKey: ["repo-capabilities", "catalog", capOffset],
    queryFn: async () => {
      const r = await api<Page<Capability>>("repo_capabilities_list", {
        offset: capOffset,
        limit: 6,
      });
      if (!Array.isArray(r.entries)) throw Error("能力列表格式无效");
      return r;
    },
  });
  const detail = useQuery({
    queryKey: ["repo-capabilities", "project", selected],
    queryFn: () =>
      api<{ project: Project }>("repo_project_get", { projectId: selected }),
    enabled: !!selected,
    refetchInterval: (q) =>
      submitting ||
      sharedBusy ||
      (q.state.data &&
        active.has(q.state.data.project.phase) &&
        q.state.data.project.observed_state !== "interrupted_unknown")
        ? 2000
        : false,
  });
  useEffect(() => {
    try {
      sessionStorage.setItem("homebase.repo.selected", selected);
    } catch {
      /* View preference only. */
    }
    setExamples("");
  }, [selected]);
  const operation = useMutation({
    mutationKey: ["repo-capabilities", "operation"],
    mutationFn: async (
      kind: "analyze" | "install" | "cancel" | "reconcile",
    ) => {
      if (gate.current) throw Error("已有操作正在进行");
      gate.current = true;
      setSubmitting(true);
      setError("");
      setNotice("");
      try {
        const requestId = crypto.randomUUID();
        let r: { project?: Project; status?: string; message?: string };
        if (kind === "analyze") {
          r = await api("repo_project_analyze", {
            request: { repository_url: url.trim(), request_id: requestId },
          });
        } else {
          const p = detail.data?.project;
          if (!p) throw Error("请先读取项目详情");
          if (kind === "install") {
            let tests: unknown;
            try {
              tests = examples.trim() ? JSON.parse(examples) : undefined;
            } catch {
              throw Error("验收用例须为有效 JSON 数组");
            }
            r = await api("repo_project_install", {
              request: {
                project_id: p.id,
                revision: p.revision,
                request_id: requestId,
                ...(tests ? { examples: tests } : {}),
              },
            });
          } else if (kind === "cancel")
            r = await api("repo_operation_cancel", {
              request: {
                project_id: p.id,
                operation_id: p.operation?.request_id,
              },
            });
          else
            r = await api("repo_operation_reconcile", {
              operationId: p.operation?.request_id,
            });
        }
        if (r.project) {
          setSelected(r.project.id);
          client.setQueryData(
            ["repo-capabilities", "project", r.project.id],
            r,
          );
          if (r.project.error) setError(r.project.error.message);
        }
        if (r.status === "canceled" && r.message) setNotice(r.message);
      } finally {
        gate.current = false;
        setSubmitting(false);
      }
    },
    onError: (e) => setError(String(e)),
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ["repo-capabilities"] });
    },
  });
  const p = detail.isError ? undefined : detail.data?.project;
  const phase = p?.observed_state || p?.phase;
  const busy = operation.isPending || sharedBusy;
  const running = p && active.has(p.phase) && !p.observed_state;
  const cancel = useMutation({
    mutationFn: () =>
      api("repo_operation_cancel", {
        request: { project_id: p?.id, operation_id: p?.operation?.request_id },
      }),
    onError: (e) => setError(String(e)),
    onSettled: () => {
      void client.invalidateQueries({ queryKey: ["repo-capabilities"] });
    },
  });
  return (
    <section aria-label="GitHub 项目自动接入" className="space-y-5">
      <div className="hb-card space-y-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-lg font-semibold">
              <Github className="h-5 w-5" />从 GitHub 接入能力
            </h2>
            <p className="mt-1 text-sm text-muted-foreground">
              分析公开仓库，隔离安装、封装并验收，桌面和微信共用。
            </p>
          </div>
          <span className="hb-tag">npm CLI · 无需仓库密钥</span>
        </div>
        <form
          className="flex flex-wrap items-end gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (url.trim() && !busy) operation.mutate("analyze");
          }}
        >
          <label className="min-w-0 flex-1 text-sm">
            仓库地址
            <input
              className="mt-2 w-full rounded-lg border border-input bg-background px-3 py-2 text-sm"
              type="url"
              required
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://github.com/markedjs/marked"
            />
          </label>
          <button className="hb-btn-primary" disabled={busy || !url.trim()}>
            {busy ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Github className="h-4 w-4" />
            )}
            分析仓库
          </button>
        </form>
        <p className="text-xs text-muted-foreground">
          目前自动支持有唯一标准 npm CLI
          入口的公开项目；其他运行时会标明所需适配。安装不运行包脚本，实际调用在无网络隔离环境中执行。
        </p>
        {error && (
          <p role="alert" className="text-sm text-destructive-strong">
            {error}
          </p>
        )}
        {notice && <p role="status" className="text-sm text-muted-foreground">{notice}</p>}
      </div>
      <div className="flex items-center justify-between">
        <h3 className="font-semibold">接入进度</h3>
        <button
          className="hb-btn-outline"
          disabled={list.isFetching || caps.isFetching || detail.isFetching}
          onClick={() => {
            void client.invalidateQueries({ queryKey: ["repo-capabilities"] });
          }}
        >
          <RefreshCw className="h-4 w-4" />
          刷新
        </button>
      </div>
      {list.isError ? (
        <p role="alert" className="hb-card">
          项目读取失败，原数据已保留。请刷新重试。
        </p>
      ) : list.isPending ? (
        <p role="status">正在读取项目…</p>
      ) : (
        <>
          <div className="flex flex-wrap gap-2">
            {list.data.entries.map((item) => (
              <button
                key={item.id}
                className={`rounded-lg border px-3 py-2 text-left text-sm ${selected === item.id ? "border-primary bg-accent" : "border-border"}`}
                onClick={() => {
                  setSelected(item.id);
                  setError("");
                  setNotice("");
                }}
              >
                {item.name}
                <span className="ml-2 text-xs text-muted-foreground">
                  {labels[item.observed_state || item.phase] || "状态待核对"}
                </span>
              </button>
            ))}
            {list.data.total === 0 && (
              <p className="text-sm text-muted-foreground">
                从仓库链接开始。分析不会安装或执行项目代码。
              </p>
            )}
          </div>
          <Pager
            offset={offset}
            next={list.data.next_offset}
            onChange={setOffset}
          />
        </>
      )}
      {selected && detail.isPending && <p role="status">正在读取项目详情…</p>}
      {detail.isError && (
        <p role="alert" className="hb-card">
          项目详情读取失败，请刷新；没有重新安装。
        </p>
      )}
      {p && (
        <article className="hb-card space-y-4" aria-label="项目详情">
          <header className="flex flex-wrap justify-between gap-3">
            <div>
              <h3 className="font-semibold">
                {p.source?.package?.name || p.name}
                <span className="ml-2 text-sm font-normal text-muted-foreground">
                  {p.source?.package?.version || ""}
                </span>
              </h3>
              <p className="mt-1 text-sm text-muted-foreground">
                {p.source?.description || p.repository}
              </p>
            </div>
            <span className="hb-tag">
              {labels[phase || ""] || "状态待核对"}
            </span>
          </header>
          <ol
            aria-label="接入阶段"
            className="flex flex-wrap gap-x-5 gap-y-2 text-sm text-muted-foreground"
          >
            {["分析", "安装", "封装", "验收", "注册"].map((v, i) => (
              <li key={v} className="flex items-center gap-1">
                {p.phase === "registered" ? (
                  <Check className="h-3 w-3" />
                ) : (
                  <span>{i + 1}.</span>
                )}
                {v}
              </li>
            ))}
          </ol>
          {p.source && (
            <>
              <p className="break-all text-xs text-muted-foreground">
                {p.source.license} · 固定提交 {p.source.source_commit}
              </p>
              <nav
                className="flex flex-wrap gap-4 text-xs"
                aria-label="仓库辅助视图"
              >
                {p.source.views.map((v) => (
                  <a
                    key={v.id}
                    href={v.url}
                    target="_blank"
                    rel="noreferrer"
                    className="inline-flex items-center gap-1 underline"
                  >
                    {v.name}
                    <ExternalLink className="h-3 w-3" />
                  </a>
                ))}
              </nav>
              <p className="text-xs text-muted-foreground">
                文档视图可能基于默认分支；安装只使用上方固定发布提交。外部内容不作为执行指令。
              </p>
            </>
          )}
          {p.error && (
            <p role="alert" className="text-sm text-destructive-strong">
              {p.error.message}
            </p>
          )}
          {p.notice && <p className="text-sm">{p.notice}</p>}
          {p.source && !p.source.supported && (
            <p className="text-sm">{p.source.reason}</p>
          )}
          {p.source?.supported && p.phase !== "registered" && !running && (
            <details>
              <summary className="cursor-pointer text-sm">
                自定义验收用例（有默认用例时可留空）
              </summary>
              <textarea
                aria-label="验收用例"
              className="mt-2 min-h-24 w-full rounded-lg border border-input bg-background px-3 py-2 font-mono text-xs"
                value={examples}
                onChange={(e) => setExamples(e.target.value)}
                placeholder={
                  '[{"input":{"stdin":"# Hello"},"contains":["<h1>Hello</h1>"]}]'
                }
              />
            </details>
          )}
          <div className="flex flex-wrap gap-3">
            {p.source?.supported && p.phase !== "registered" && (
              <button
                className="hb-btn-primary"
                disabled={busy || !!running || phase === "interrupted_unknown"}
                onClick={() => operation.mutate("install")}
              >
                {running ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Box className="h-4 w-4" />
                )}
                安装、封装并验收
              </button>
            )}
            {running && (
              <button
                className="hb-btn-outline"
                disabled={cancel.isPending}
                onClick={() => cancel.mutate()}
              >
                取消此操作
              </button>
            )}
            {phase === "interrupted_unknown" && (
              <button
                className="hb-btn-outline"
                disabled={busy}
                onClick={() => operation.mutate("reconcile")}
              >
                核对中断操作
              </button>
            )}
            {p.verification?.passed && (
              <p className="text-sm text-muted-foreground">
                已通过 {p.verification.tests} 条行为验收，已注册为可调用接口。
              </p>
            )}
          </div>
        </article>
      )}
      <h3 className="font-semibold">已注册能力</h3>
      {caps.isError ? (
        <p role="alert" className="hb-card">
          能力列表读取失败，请刷新重试。
        </p>
      ) : caps.isPending ? (
        <p role="status">正在读取能力…</p>
      ) : (
        <>
          <div className="grid gap-4 md:grid-cols-2">
            {caps.data.entries.map((c) => (
              <article key={c.id} className="hb-card min-w-0 space-y-3">
                <div className="flex items-center justify-between gap-2">
                  <h4 className="font-semibold">{c.name}</h4>
                  <span className="hb-tag">已验收</span>
                </div>
                <p className="text-sm text-muted-foreground">{c.description}</p>
                <p className="text-xs text-muted-foreground">
                  {c.version} · {c.license}
                </p>
                <button className="hb-btn-outline" onClick={() => setTrying(c)}>
                  <Play className="h-4 w-4" />
                  调用与接口
                </button>
              </article>
            ))}
          </div>
          {caps.data.total === 0 && (
            <p className="text-sm text-muted-foreground">
              通过安装和验收后，能力会出现在这里。
            </p>
          )}
          <Pager
            offset={capOffset}
            next={caps.data.next_offset}
            onChange={setCapOffset}
          />
        </>
      )}
      {trying && (
        <TryCapability capability={trying} close={() => setTrying(null)} />
      )}
    </section>
  );
}
function Pager({
  offset,
  next,
  onChange,
}: {
  offset: number;
  next: number | null;
  onChange: (v: number) => void;
}) {
  if (!offset && next === null) return null;
  return (
    <div className="flex gap-3 text-xs">
      <button
        disabled={!offset}
        onClick={() => onChange(Math.max(0, offset - 6))}
      >
        上一页
      </button>
      <button
        disabled={next === null}
        onClick={() => next !== null && onChange(next)}
      >
        下一页
      </button>
    </div>
  );
}
function TryCapability({
  capability,
  close,
}: {
  capability: Capability;
  close: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [input, setInput] = useState("");
  const client = useQueryClient();
  const resultKey = ["repo-capability-call", capability.id];
  const cached = useQuery<Result>({
    queryKey: resultKey,
    queryFn: async () => {
      throw Error("结果只由原调用更新");
    },
    enabled: false,
    staleTime: Infinity,
  });
  const result = cached.data;
  const setResult = (value: Result) => client.setQueryData(resultKey, value);
  const [error, setError] = useState("");
  const gate = useRef(false);
  const detail = useQuery({
    queryKey: ["repo-capabilities", "describe", capability.id],
    queryFn: () =>
      api<Description>("repo_capability_describe", {
        capabilityId: capability.id,
      }),
  });
  useEffect(() => {
    dialog.current?.showModal();
    return () => dialog.current?.close();
  }, []);
  useEffect(() => {
    if (detail.data)
      setInput(
        JSON.stringify(
          detail.data.capability.examples[0]?.input || { stdin: "" },
          null,
          2,
        ),
      );
  }, [detail.data]);
  const run = useMutation({
    mutationFn: async () => {
      if (
        gate.current ||
        client.getQueryData<Result>(resultKey)?.status === "running"
      )
        throw Error("已有调用正在进行");
      gate.current = true;
      setError("");
      try {
        let args: unknown;
        try {
          args = JSON.parse(input);
        } catch {
          throw Error("输入须为有效 JSON");
        }
        setResult({
          status: "running",
          message: "原任务正在隔离执行；切换页面不会重新提交。",
        });
        const r = await api<Result>("repo_capability_run", {
          request: {
            capability_id: capability.id,
            input: args,
            request_id: crypto.randomUUID(),
          },
        });
        setResult(r);
        if (r.status !== "succeeded")
          throw Error(r.error?.message || "结果尚未确认，请查询原任务");
      } catch (e) {
        if (client.getQueryData<Result>(resultKey)?.status === "running")
          setResult({
            status: "unknown",
            message:
              "原调用未返回可确认结果，不能自动重跑。请通过 CLI 核对原控制任务。",
          });
        throw e;
      } finally {
        gate.current = false;
      }
    },
    onError: (e) => setError(String(e)),
  });
  return (
    <dialog
      ref={dialog}
      onCancel={(e) => {
        if (run.isPending) e.preventDefault();
        else close();
      }}
      className="w-[calc(100%-2rem)] max-w-2xl rounded-xl border border-border bg-background p-6 text-foreground backdrop:bg-black/25"
    >
      <div className="space-y-4">
        <header className="flex items-start justify-between gap-4">
          <h2 className="text-lg font-semibold">{capability.name}</h2>
          <button
            aria-label="关闭能力调用"
            disabled={run.isPending}
            onClick={close}
          >
            <X className="h-5 w-5" />
          </button>
        </header>
        {detail.isError ? (
          <p role="alert">接口读取失败，未执行能力。</p>
        ) : detail.isPending ? (
          <p>正在读取接口…</p>
        ) : (
          <>
            <p className="text-sm text-muted-foreground">
              {detail.data.capability.notice}
            </p>
            <label className="block text-sm">
              调用输入（JSON）
              <textarea
                className="mt-2 min-h-36 w-full rounded-lg border border-input bg-background px-3 py-2 font-mono text-xs"
                value={input}
                onChange={(e) => setInput(e.target.value)}
                disabled={run.isPending}
              />
            </label>
            <details>
              <summary className="cursor-pointer text-sm">查看接口契约</summary>
              <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-all text-xs">
                {JSON.stringify(detail.data.capability.input_schema, null, 2)}
              </pre>
              <code className="block break-all text-xs">{capability.id}</code>
              <button
                className="hb-btn-outline mt-2"
                onClick={() => {
                  void navigator.clipboard
                    .writeText(capability.id)
                    .catch(() => setError("复制失败，请手动复制接口编号"));
                }}
              >
                <Copy className="h-3 w-3" />
                复制接口编号
              </button>
            </details>
            <button
              className="hb-btn-primary"
              disabled={
                run.isPending ||
                result?.status === "running" ||
                result?.status === "unknown"
              }
              onClick={() => run.mutate()}
            >
              {run.isPending ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Play className="h-4 w-4" />
              )}
              隔离调用
            </button>
          </>
        )}
        {error && (
          <p role="alert" className="text-sm text-destructive-strong">
            {error}
          </p>
        )}
        {result && (
          <div className="space-y-2">
            <p className="text-sm">{result.message || result.status}</p>
            <pre
              aria-label="能力输出"
              className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-muted p-3 text-xs"
            >
              {result.stdout ?? "尚无成功输出"}
            </pre>
            <p className="break-all text-xs text-muted-foreground">
              运行编号：{result.run_id || "未生成"}
              {result.artifact && ` · 结果文件：${result.artifact.path}`}
            </p>
            {result.run_id && (
              <button
                className="hb-btn-outline"
                onClick={() => {
                  void api<Result>("repo_capability_result", {
                    runId: result.run_id,
                  })
                    .then(setResult)
                    .catch((e) => setError(String(e)));
                }}
              >
                查询原任务结果
              </button>
            )}
          </div>
        )}
      </div>
    </dialog>
  );
}
