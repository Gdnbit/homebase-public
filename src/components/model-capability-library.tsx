import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Box,
  Copy,
  FileText,
  Image,
  Video,
  Mic,
  AudioLines,
  Layers,
  MoreHorizontal,
  RefreshCw,
  X,
  ArrowRight,
  Search,
} from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { CapabilityPricing } from "@/components/capability-facts";
import { refreshCapabilityCatalog } from "@/lib/capability-index";
import {
  modelCapabilitiesApi as api,
  modelCapabilitiesKey,
  modelGroups,
  credentialLabel,
  MAX_PNG_BYTES,
  modelSupportsVision,
  readLocalPngBase64,
  type CapabilityCredential,
  type ModelCapability,
  type ModelCapabilityPlan,
  type ModelCapabilityResult,
} from "@/lib/model-capabilities";

const icons = {
  text: FileText,
  image: Image,
  video: Video,
  tts: AudioLines,
  asr: Mic,
  embedding: Layers,
};
const errorText = (reason: unknown) =>
  reason instanceof Error ? reason.message : String(reason);
const statusLabel = (status: string) =>
  ({
    succeeded: "业务已完成",
    queued: "已排队，尚未完成",
    running: "任务进行中",
    failed: "业务失败",
    unknown: "结果状态未知",
  })[status] ?? "结果状态未知";

export function ModelCapabilityLibrary() {
  const client = useQueryClient();
  const catalog = useQuery({
    queryKey: modelCapabilitiesKey,
    queryFn: api.catalog,
    staleTime: 5 * 60_000,
  });
  const [category, setCategory] = useState("text");
  const [search, setSearch] = useState("");
  const [provider, setProvider] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [syncError, setSyncError] = useState("");
  const [limit, setLimit] = useState(24);
  const data = catalog.data;
  const rows = useMemo(
    () =>
      (data?.models ?? []).filter(
        (m) =>
          m.functions.includes(category) &&
          (!provider || m.provider_id === provider) &&
          `${m.name} ${m.model_id} ${m.publisher}`
            .toLowerCase()
            .includes(search.trim().toLowerCase()),
      ),
    [data, category, provider, search],
  );
  const groups = useMemo(() => modelGroups(rows), [rows]);
  const selectedRoutes = useMemo(
    () =>
      (data?.models ?? []).filter(
        (m) =>
          m.canonical_id === selected &&
          m.functions.includes(category) &&
          (!provider || m.provider_id === provider),
      ),
    [data, selected, category, provider],
  );
  async function sync() {
    if (syncing) return;
    setSyncing(true);
    setSyncError("");
    const ids = provider
      ? [provider]
      : (data?.credentials ?? [])
          .filter(
            (c) =>
              c.state === "configured" &&
              [
                "openrouter",
                "openai",
                "fal",
                "elevenlabs",
                "dashscope",
              ].includes(c.provider_id),
          )
          .map((c) => c.provider_id);
    try {
      const next = await api.refresh(ids);
      client.setQueryData(modelCapabilitiesKey, next);
      refreshCapabilityCatalog(client);
      const failures = next.sources.filter(
        (s) => ids.includes(s.provider_id) && s.state === "error",
      );
      if (failures.length)
        setSyncError(
          failures
            .map(
              (s) =>
                `${next.credentials.find((c) => c.provider_id === s.provider_id)?.provider_name ?? s.provider_id}：${s.message}`,
            )
            .join("；"),
        );
    } catch (reason) {
      setSyncError(`${errorText(reason)}；已保留当前目录。`);
    } finally {
      setSyncing(false);
    }
  }
  return (
    <section aria-label="模型与服务" className="space-y-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-sm text-muted-foreground">
          选择功能和模型，按调用渠道自动匹配已有密钥。
        </p>
        <div className="flex flex-wrap gap-2">
          <button
            className="hb-btn-outline text-xs"
            disabled={catalog.isFetching}
            onClick={() => void catalog.refetch()}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            刷新配置
          </button>
          <button
            className="hb-btn-outline text-xs"
            disabled={syncing || !data}
            onClick={() => void sync()}
          >
            <RefreshCw
              className={`h-3.5 w-3.5 ${syncing ? "animate-spin" : ""}`}
            />
            {syncing ? "正在同步目录" : "同步模型目录"}
          </button>
        </div>
      </div>
      {!data && catalog.isPending && (
        <p role="status" className="hb-card text-sm text-muted-foreground">
          正在读取模型与凭据状态…
        </p>
      )}
      {catalog.isError && (
        <div role="alert" className="hb-card space-y-2 text-sm">
          <p>读取失败，不能将其判断为没有模型：{errorText(catalog.error)}</p>
          <button
            className="hb-btn-outline"
            onClick={() => void catalog.refetch()}
          >
            重新读取
          </button>
        </div>
      )}
      {data && (
        <>
          <div
            role="tablist"
            aria-label="功能分组"
            className="flex flex-wrap gap-2"
          >
            {data.functions.map((group) => {
              const Icon = icons[group.id as keyof typeof icons] ?? Box;
              return (
                <button
                  key={group.id}
                  role="tab"
                  aria-selected={category === group.id}
                  className={
                    category === group.id
                      ? "hb-btn-primary text-sm"
                      : "hb-btn-outline text-sm"
                  }
                  onClick={() => {
                    setCategory(group.id);
                    setLimit(24);
                  }}
                >
                  <Icon className="h-4 w-4" />
                  {group.name}
                </button>
              );
            })}
            <button
              role="tab"
              aria-selected={category === "services"}
              className={
                category === "services"
                  ? "hb-btn-primary text-sm"
                  : "hb-btn-outline text-sm"
              }
              onClick={() => {
                setCategory("services");
                setLimit(24);
              }}
            >
              <MoreHorizontal className="h-4 w-4" />
              其他服务与凭据
            </button>
          </div>
          {(data.issues.length > 0 || syncError) && (
            <div
              role="alert"
              className="rounded-lg border border-warning/30 bg-warning/5 px-4 py-3 text-sm"
            >
              {[...data.issues, syncError].filter(Boolean).join("；")}
            </div>
          )}
          {category !== "services" ? (
            <>
              <div className="grid gap-3 sm:grid-cols-[1fr_240px]">
                <label className="relative">
                  <span className="sr-only">搜索模型</span>
                  <Search className="pointer-events-none absolute left-3 top-3 h-4 w-4 text-muted-foreground" />
                  <input
                    className="hb-input w-full pl-9"
                    placeholder="搜索模型名称、编号或厂商"
                    value={search}
                    onChange={(e) => {
                      setSearch(e.target.value);
                      setLimit(24);
                    }}
                  />
                </label>
                <label>
                  <span className="sr-only">调用渠道筛选</span>
                  <select
                    className="hb-input w-full"
                    value={provider}
                    onChange={(e) => {
                      setProvider(e.target.value);
                      setLimit(24);
                    }}
                  >
                    <option value="">全部调用渠道</option>
                    {data.credentials.map((c) => (
                      <option key={c.provider_id} value={c.provider_id}>
                        {c.provider_name} · {credentialLabel(c.state)}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
              {provider && (
                <p className="text-xs text-muted-foreground">
                  {
                    data.sources.find((s) => s.provider_id === provider)
                      ?.message
                  }
                </p>
              )}
              {groups.length === 0 ? (
                <div className="hb-card flex min-h-48 flex-col items-center justify-center gap-3 text-center">
                  <Box className="h-8 w-8 text-muted-foreground" />
                  <p>此分组暂无匹配模型</p>
                  <p className="text-xs text-muted-foreground">
                    可清除筛选，或同步已配置渠道的模型目录。未配置与尚未同步是不同状态。
                  </p>
                </div>
              ) : (
                <ul aria-label="功能模型" className="grid gap-4 lg:grid-cols-2">
                  {groups.slice(0, limit).map(([id, routes]) => {
                    const model = routes[0];
                    const bindings = routes.map((r) =>
                      data.credentials.find(
                        (c) => c.provider_id === r.provider_id,
                      ),
                    );
                    const configured = bindings.some(
                      (c) => c?.state === "configured",
                    );
                    const warning = bindings.some((c) => c?.expiry_warning);
                    return (
                      <li
                        key={id}
                        className="hb-card flex min-w-0 flex-col gap-3"
                      >
                        <div className="flex items-start gap-3">
                          <div className="rounded-lg bg-secondary p-2.5">
                            <Box className="h-5 w-5" />
                          </div>
                          <div className="min-w-0">
                            <h2 className="break-words text-lg font-semibold">
                              {model.name}
                            </h2>
                            <p className="mt-1 break-all text-xs text-muted-foreground">
                              {model.model_id}
                            </p>
                          </div>
                        </div>
                        <p className="text-sm text-muted-foreground">
                          调用渠道：
                          {bindings
                            .map((c) => c?.provider_name ?? "未知渠道")
                            .join(" / ")}
                        </p>
                        <div className="mt-auto flex flex-wrap items-center justify-between gap-2">
                          <span
                            className={`rounded-md px-2 py-1 text-xs ${warning ? "bg-warning/10 text-warning-strong" : "bg-secondary text-muted-foreground"}`}
                          >
                            {configured
                              ? warning
                                ? "已配置 · 有到期提醒"
                                : "已配置 · 账号权限未验证"
                              : "当前渠道未配置或不可用"}
                          </span>
                          <button
                            className="inline-flex items-center gap-1 text-sm hover:underline"
                            onClick={() => setSelected(id)}
                          >
                            查看调用方式
                            <ArrowRight className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </li>
                    );
                  })}
                </ul>
              )}
              {groups.length > limit && (
                <button
                  className="hb-btn-outline"
                  onClick={() => setLimit((v) => v + 24)}
                >
                  显示更多模型（剩余 {groups.length - limit} 项）
                </button>
              )}
              <p className="text-xs text-muted-foreground">
                官方参考与同步目录均不代表账号生成权限或余额已验证；模型发布商与调用渠道分开记录。
              </p>
            </>
          ) : (
            <>
              <div className="grid gap-4 lg:grid-cols-2">
                {data.services.map((service) => (
                  <article key={service.id} className="hb-card space-y-2">
                    <div className="flex items-start justify-between gap-3">
                      <h2 className="font-semibold">{service.name}</h2>
                      <span className="hb-tag">{service.function}</span>
                    </div>
                    <p className="text-sm text-muted-foreground">
                      {service.message}
                    </p>
                    <p className="break-all text-xs">
                      {service.credential_ref} ·{" "}
                      {credentialLabel(service.state)}
                    </p>
                  </article>
                ))}
              </div>
              <details className="hb-card text-sm">
                <summary className="cursor-pointer">
                  全部模型渠道与密钥配置状态
                </summary>
                <ul className="mt-4 grid gap-4 sm:grid-cols-2">
                  {data.credentials.map((c) => (
                    <li key={c.provider_id} className="space-y-1">
                      <p>
                        {c.provider_name} · {credentialLabel(c.state)}
                      </p>
                      <p className="break-all text-xs text-muted-foreground">
                        {c.credential_ref}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {c.message}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {
                          data.sources.find(
                            (s) => s.provider_id === c.provider_id,
                          )?.message
                        }
                      </p>
                    </li>
                  ))}
                </ul>
              </details>
            </>
          )}
          {selected && selectedRoutes.length > 0 && (
            <ModelCallDialog
              key={selected}
              routes={selectedRoutes}
              credentials={data.credentials}
              onClose={() => setSelected(null)}
            />
          )}
        </>
      )}
    </section>
  );
}

function ModelCallDialog({
  routes,
  credentials,
  onClose,
}: {
  routes: ModelCapability[];
  credentials: CapabilityCredential[];
  onClose: () => void;
}) {
  const client = useQueryClient();
  const initial =
    routes.find(
      (r) =>
        credentials.find((c) => c.provider_id === r.provider_id)?.state ===
        "configured",
    ) ?? routes[0];
  const [routeId, setRouteId] = useState(initial.route_id);
  const route = routes.find((r) => r.route_id === routeId)!;
  const binding = credentials.find((c) => c.provider_id === route.provider_id);
  const [input, setInput] = useState(
    JSON.stringify(initial.input_example, null, 2),
  );
  const acceptsImage = modelSupportsVision(route);
  const [imageMode, setImageMode] = useState(false);
  const [imagePrompt, setImagePrompt] = useState("");
  const [picture, setPicture] = useState<{
    name: string;
    base64: string;
  } | null>(null);
  const [outputRoot, setOutputRoot] = useState("");
  const [plan, setPlan] = useState<ModelCapabilityPlan | null>(null);
  const [result, setResult] = useState<ModelCapabilityResult | null>(
    () =>
      client.getQueryData(["model-capabilities", "result", initial.route_id]) ??
      null,
  );
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const running = useRef(false);
  const mounted = useRef(true);
  const generation = useRef(0);
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    mounted.current = true;
    dialog.current?.showModal();
    return () => {
      mounted.current = false;
      generation.current++;
      dialog.current?.close();
    };
  }, []);
  async function selectPicture(file?: File) {
    const version = ++generation.current;
    setPlan(null);
    setPicture(null);
    setError("");
    if (!file) return;
    if (
      !file.name.toLowerCase().endsWith(".png") ||
      !file.size ||
      file.size > MAX_PNG_BYTES
    ) {
      setError("请选择不超过 384 KiB 的 PNG 图片。");
      return;
    }
    try {
      const base64 = await readLocalPngBase64(file);
      if (mounted.current && version === generation.current) {
        setPicture({ name: file.name, base64 });
      }
    } catch {
      if (mounted.current && version === generation.current)
        setError("图片读取失败。");
    }
  }
  function request() {
    if (imageMode) {
      if (!acceptsImage || !picture || !imagePrompt.trim()) {
        throw new Error("请选择 PNG 并填写图片问题。");
      }
      return {
        route_id: routeId,
        input: { vision: { prompt: imagePrompt, png_base64: picture.base64 } },
        output_root: null,
      };
    }
    const value: unknown = JSON.parse(input);
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error("调用参数必须为 JSON 对象");
    return {
      route_id: routeId,
      input: value as Record<string, unknown>,
      output_root: outputRoot.trim() || null,
    };
  }
  async function run(kind: "resolve" | "call" | "task" | "copy") {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError("");
    setNotice("");
    const version = ++generation.current;
    try {
      const req = request();
      if (kind === "copy") {
        const command = { request: req };
        await invoke("plugin:clipboard-manager|write_text", {
          text: `${JSON.stringify(command, null, 2)}\n\n保存为 request.json 后：\nhb validate model_capability_call --input request.json\nhb call model_capability_call --allow-write --allow-private --allow-external --input request.json --request-id <本次唯一且固定编号>`,
        });
        if (mounted.current)
          setNotice("调用参数和 CLI 示例已复制，尚未执行生成。");
      } else if (kind === "resolve") {
        const value = await api.resolve(req);
        if (mounted.current && version === generation.current) setPlan(value);
      } else {
        const value =
          kind === "task" && result?.task_id
            ? await api.task(routeId, result.task_id, req.output_root)
            : await api.call(req);
        client.setQueryData(["model-capabilities", "result", routeId], value);
        if (mounted.current && version === generation.current) setResult(value);
      }
    } catch (reason) {
      if (mounted.current && version === generation.current)
        setError(errorText(reason));
    } finally {
      running.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  function changeRoute(next: string) {
    const r = routes.find((r) => r.route_id === next)!;
    generation.current++;
    setRouteId(next);
    setImageMode(false);
    setImagePrompt("");
    setPicture(null);
    setInput(JSON.stringify(r.input_example, null, 2));
    setPlan(null);
    setResult(
      client.getQueryData(["model-capabilities", "result", next]) ?? null,
    );
    setError("");
    setNotice("");
  }
  return (
    <dialog
      ref={dialog}
      aria-labelledby="model-call-title"
      onCancel={(e) => {
        if (busy) e.preventDefault();
        else onClose();
      }}
      className="hb-editor-dialog !m-auto max-h-[88vh] w-[calc(100%-2rem)] max-w-2xl overflow-auto rounded-xl border border-border bg-card p-0 text-foreground shadow-2xl backdrop:bg-black/50"
    >
      <div className="space-y-4 p-5 sm:p-6">
        <header className="flex justify-between gap-3">
          <div>
            <h2 id="model-call-title" className="text-xl font-semibold">
              选择调用方式
            </h2>
            <p className="mt-1 text-sm text-muted-foreground">{route.name}</p>
          </div>
          <button
            disabled={busy}
            autoFocus
            aria-label="关闭模型调用"
            className="rounded p-1.5 hover:bg-secondary"
            onClick={onClose}
          >
            <X className="h-4 w-4" />
          </button>
        </header>
        <label className="block text-sm">
          调用渠道
          <select
            aria-label="调用渠道"
            disabled={busy}
            className="hb-input mt-2 w-full"
            value={routeId}
            onChange={(e) => changeRoute(e.target.value)}
          >
            {routes.map((r) => (
              <option key={r.route_id} value={r.route_id}>
                {credentials.find((c) => c.provider_id === r.provider_id)
                  ?.provider_name ?? r.provider_id}{" "}
                · {r.model_id}
              </option>
            ))}
          </select>
        </label>
        <div className="rounded-lg border border-border bg-secondary/30 p-3 text-sm">
          <p>
            自动匹配密钥：
            <span className="break-all">
              {binding?.credential_ref ?? "未匹配"}
            </span>
          </p>
          <p className="mt-1 text-xs text-muted-foreground">
            {binding
              ? `${credentialLabel(binding.state)} · ${binding.message}`
              : "渠道状态未知"}
          </p>
        </div>
        <CapabilityPricing pricing={route.pricing} routeId={route.route_id} />
        {route.protocol === "unsupported" && (
          <p role="status" className="text-sm text-warning-strong">
            此模型协议尚未接入，当前仅可查看目录；不会使用其他模型或 Key 替代。
          </p>
        )}
        {acceptsImage && (
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={imageMode}
              disabled={busy}
              onChange={(event) => {
                generation.current++;
                setImageMode(event.target.checked);
                setPicture(null);
                setPlan(null);
                setError("");
              }}
            />
            使用本机 PNG
          </label>
        )}
        {imageMode ? (
          <>
            <label className="block text-sm">
              选择 PNG
              <input
                type="file"
                accept=".png,image/png"
                disabled={busy}
                className="hb-input mt-2 w-full"
                onChange={(event) =>
                  void selectPicture(event.target.files?.[0])
                }
              />
            </label>
            {picture && (
              <p className="break-all text-xs text-muted-foreground">
                {picture.name}
              </p>
            )}
            <label className="block text-sm">
              图片问题
              <textarea
                disabled={busy}
                className="hb-input mt-2 min-h-24 w-full"
                value={imagePrompt}
                onChange={(event) => {
                  setImagePrompt(event.target.value);
                  setPlan(null);
                }}
              />
            </label>
            <p className="text-xs text-muted-foreground">
              支持单张静态 PNG，至多 384 KiB。调用时将图片发送至所选 OpenRouter
              渠道。
            </p>
          </>
        ) : (
          <>
            <label className="block text-sm">
              调用参数
              <textarea
                disabled={busy}
                className="hb-input mt-2 min-h-40 w-full font-mono text-xs"
                value={input}
                onChange={(e) => {
                  setInput(e.target.value);
                  setPlan(null);
                }}
                spellCheck={false}
              />
            </label>
            <p className="text-xs text-muted-foreground">
              示例内容需替换为本次输入。模型自动绑定；音色、材料 URL
              等参数请按服务商接口填写。
              <a
                className="ml-1 underline"
                href={route.source_url}
                target="_blank"
                rel="noreferrer"
              >
                官方参数说明
              </a>
            </p>
            <label className="block text-sm">
              输出目录（音频或较大结果必填）
              <input
                disabled={busy}
                className="hb-input mt-2 w-full"
                value={outputRoot}
                onChange={(e) => setOutputRoot(e.target.value)}
                placeholder="专用绝对目录，只新建独立结果子目录"
              />
            </label>
          </>
        )}
        <div className="flex flex-wrap gap-2">
          <button
            disabled={
              busy ||
              route.protocol === "unsupported" ||
              (imageMode && (!picture || !imagePrompt.trim()))
            }
            className="hb-btn-outline"
            onClick={() => void run("resolve")}
          >
            检查调用方式
          </button>
          <button
            disabled={busy || route.protocol === "unsupported" || imageMode}
            className="hb-btn-outline"
            onClick={() => void run("copy")}
          >
            <Copy className="h-4 w-4" />
            复制 CLI 示例
          </button>
          <button
            disabled={
              busy ||
              (imageMode && (!picture || !imagePrompt.trim())) ||
              binding?.state !== "configured" ||
              (result !== null &&
                ["queued", "running"].includes(result.status)) ||
              route.protocol === "unsupported"
            }
            className="hb-btn-primary"
            onClick={() => void run("call")}
          >
            {busy ? "正在处理" : "调用模型"}
          </button>
        </div>
        <p className="text-xs text-muted-foreground">
          调用模型会向所选服务商提交本次参数，可能产生费用；不会自动更换渠道、密钥或重试。
        </p>
        {plan && (
          <div role="status" className="rounded-lg bg-secondary/40 p-3 text-xs">
            <p>{plan.message}</p>
            <p className="mt-1 break-all">
              {plan.method} {plan.endpoint}
            </p>
            <p className="mt-1">密钥绑定：{plan.credential.credential_ref}</p>
          </div>
        )}
        {result && (
          <section
            aria-label="模型调用结果"
            className="space-y-2 rounded-lg border border-border p-3 text-sm"
          >
            <p>{statusLabel(result.status)}</p>
            <p className="text-xs text-muted-foreground">{result.message}</p>
            {result.task_id && (
              <p className="break-all text-xs">任务编号：{result.task_id}</p>
            )}
            {result.task_id &&
              !["succeeded", "failed"].includes(result.status) && (
                <button
                  disabled={busy}
                  className="hb-btn-outline text-xs"
                  onClick={() => void run("task")}
                >
                  查询原任务状态与结果
                </button>
              )}
            {result.artifact && (
              <p className="break-all text-xs">
                结果文件：{result.artifact.path}
                <br />
                {result.artifact.bytes} 字节 · SHA-256 {result.artifact.sha256}
              </p>
            )}
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-all text-xs">
              {JSON.stringify(result.output, null, 2)}
            </pre>
          </section>
        )}
        {notice && (
          <p role="status" className="text-sm text-muted-foreground">
            {notice}
          </p>
        )}
        {error && (
          <p
            role="alert"
            className="whitespace-pre-wrap text-sm text-destructive-strong"
          >
            {error}
          </p>
        )}
      </div>
    </dialog>
  );
}
