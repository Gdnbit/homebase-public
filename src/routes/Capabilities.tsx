import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Box, Plus, X, Copy, ArrowRight } from "lucide-react";
import { FunctionalCapabilities } from "@/components/functional-capabilities";
import {
  capabilityDraftKey,
  capabilityTask,
  readCapabilityDrafts,
  writeCapabilityDraft,
  type CapabilityDraft,
} from "@/lib/capability-drafts";

const ModelCapabilityLibrary = lazy(() => import("@/components/model-capability-library")
  .then(({ ModelCapabilityLibrary }) => ({ default: ModelCapabilityLibrary })));
const RepositoryCapabilities = lazy(() => import("@/components/repository-capabilities")
  .then(({ RepositoryCapabilities }) => ({ default: RepositoryCapabilities })));
const HyperframesCapability = lazy(() => import("@/components/hyperframes-capability")
  .then(({ HyperframesCapability }) => ({ default: HyperframesCapability })));
const SharedMcpSection = lazy(() => import("@/components/shared-mcp-section")
  .then(({ SharedMcpSection }) => ({ default: SharedMcpSection })));
const SharedSkillsSection = lazy(() => import("@/components/shared-skills-section")
  .then(({ SharedSkillsSection }) => ({ default: SharedSkillsSection })));

type Library = {
  raw: string | null;
  entries: CapabilityDraft[];
  error: string;
};
function load(): Library {
  try {
    return { ...readCapabilityDrafts(), error: "" };
  } catch {
    return {
      raw: null,
      entries: [],
      error: "无法读取能力草稿，原数据已保留。请检查本地存储后重试。",
    };
  }
}
const empty = (): CapabilityDraft => ({
  id: crypto.randomUUID(),
  name: "",
  repository: "",
  revision: "",
  purpose: "",
  transport: "CLI",
  input: "",
  output: "",
});

export function Capabilities() {
  const [tab, setTab] = useState(() => {
    try { const view = sessionStorage.getItem("homebase.capabilities.view.v2"); return ["models", "projects", "extensions"].includes(view ?? "") ? view! : "functions"; } catch { return "functions"; }
  });
  useEffect(() => { try { sessionStorage.setItem("homebase.capabilities.view.v2", tab); } catch { /* View preference only; business data is unaffected. */ } }, [tab]);
  return <div className="animate-fade-in space-y-6">
    <header className="hb-page-header"><span className="hb-section-label">Capabilities</span><h1>能力库</h1><p>按功能选择模型、服务与本地能力。</p></header>
    <div className="flex gap-5 border-b border-border" role="tablist" aria-label="能力类型">
      <button role="tab" aria-selected={tab === "functions"} onClick={() => setTab("functions")} className={`border-b-2 px-1 pb-3 text-sm ${tab === "functions" ? "border-primary font-semibold" : "border-transparent text-muted-foreground"}`}>按功能发现</button>
      <button role="tab" aria-selected={tab === "models"} onClick={() => setTab("models")} className={`border-b-2 px-1 pb-3 text-sm ${tab === "models" ? "border-primary font-semibold" : "border-transparent text-muted-foreground"}`}>模型与服务</button>
      <button role="tab" aria-selected={tab === "projects"} onClick={() => setTab("projects")} className={`border-b-2 px-1 pb-3 text-sm ${tab === "projects" ? "border-primary font-semibold" : "border-transparent text-muted-foreground"}`}>开源项目</button>
      <button role="tab" aria-selected={tab === "extensions"} onClick={() => setTab("extensions")} className={`border-b-2 px-1 pb-3 text-sm ${tab === "extensions" ? "border-primary font-semibold" : "border-transparent text-muted-foreground"}`}>工具与工作流</button>
    </div>
    <Suspense fallback={<p role="status" className="text-muted-foreground">正在加载能力页…</p>}>
      {tab === "functions" ? <FunctionalCapabilities /> : tab === "models" ? <ModelCapabilityLibrary /> : tab === "extensions" ? <div className="space-y-8"><SharedMcpSection /><SharedSkillsSection /></div> : <OpenSourceCapabilities />}
    </Suspense>
  </div>;
}

function OpenSourceCapabilities() {
  const [library, setLibrary] = useState(load);
  const [search, setSearch] = useState("");
  const [draft, setDraft] = useState<CapabilityDraft | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [preview, setPreview] = useState("");
  const dialog = useRef<HTMLDialogElement>(null);
  const original = useRef("");
  const baseline = useRef<string | null>(null);
  const text = useRef<HTMLTextAreaElement>(null);
  const open = draft !== null;
  useEffect(() => {
    const refresh = (event: StorageEvent) => {
      if (event.key === capabilityDraftKey || event.key === null)
        setLibrary(load());
    };
    window.addEventListener("storage", refresh);
    return () => window.removeEventListener("storage", refresh);
  }, []);
  useEffect(() => {
    if (!open) return;
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, [open]);
  useEffect(() => {
    if (!draft || JSON.stringify(draft) === original.current) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [draft]);
  function edit(entry?: CapabilityDraft) {
    const current = load();
    setLibrary(current);
    if (current.error) return;
    const next = entry
      ? current.entries.find((item) => item.id === entry.id)
      : empty();
    if (!next) {
      setLibrary(load());
      return;
    }
    baseline.current = current.raw;
    original.current = JSON.stringify(next);
    setDraft({ ...next });
    setError("");
    setNotice("");
    setPreview("");
  }
  function close() {
    if (
      draft &&
      JSON.stringify(draft) !== original.current &&
      !window.confirm("有未保存的封装草稿，关闭会丢弃更改，是否继续？")
    )
      return;
    setDraft(null);
  }
  function save() {
    if (!draft) return;
    try {
      const result = writeCapabilityDraft(draft, baseline.current);
      setLibrary({ ...result, error: "" });
      baseline.current = result.raw;
      const saved = result.entries.find((entry) => entry.id === draft.id)!;
      setDraft(saved);
      original.current = JSON.stringify(saved);
      setError("");
      setNotice("草稿已保存，尚未封装与验证。");
      setPreview("");
    } catch (reason) {
      setError(`保存失败：${String(reason)}`);
    }
  }
  const entries = library.entries.filter((entry) =>
    `${entry.name} ${entry.repository} ${entry.purpose}`
      .toLowerCase()
      .includes(search.trim().toLowerCase()),
  );
  return (
    <div className="animate-fade-in space-y-6">
      <header className="hb-page-header">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <span className="hb-section-label">Open source</span>
            <h2 className="text-2xl font-semibold">开源项目</h2>
            <p>将 GitHub 项目整理为可调用能力。</p>
          </div>
          <button
            className="hb-btn-primary"
            disabled={!!library.error}
            onClick={() => edit()}
          >
            <Plus className="h-4 w-4" />
            添加项目
          </button>
        </div>
      </header>
      <RepositoryCapabilities />
      <HyperframesCapability />
      {library.error ? (
        <div role="alert" className="hb-card space-y-3">
          <p>{library.error}</p>
          <button className="hb-btn-outline" onClick={() => setLibrary(load())}>
            重新读取草稿
          </button>
        </div>
      ) : (
        <>
          {library.entries.length > 0 && (
            <label className="block text-sm">
              搜索项目
              <input
                className="hb-input mt-2 w-full"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
                placeholder="名称、仓库或用途"
              />
            </label>
          )}
          {library.entries.length === 0 ? (
            <section className="hb-card flex min-h-72 flex-col items-center justify-center gap-4 text-center">
              <Box className="h-10 w-10 text-muted-foreground" />
              <h2 className="text-xl font-semibold">尚未添加项目</h2>
              <p className="text-sm text-muted-foreground">
                从仓库链接开始，定义输入、输出与调用方式。
              </p>
              <button className="hb-btn-outline" onClick={() => edit()}>
                添加第一个项目
              </button>
            </section>
          ) : entries.length === 0 ? (
            <p className="hb-card text-sm text-muted-foreground">
              没有匹配的项目。
              <button className="ml-3 underline" onClick={() => setSearch("")}>
                清除搜索
              </button>
            </p>
          ) : (
            <ul aria-label="能力项目" className="grid gap-4 lg:grid-cols-2">
              {entries.map((entry) => (
                <li key={entry.id} className="hb-card min-w-0 space-y-3">
                  <div className="flex items-start justify-between gap-3">
                    <h2 className="break-words text-lg font-semibold">
                      {entry.name}
                    </h2>
                    <span className="shrink-0 rounded-md bg-warning/10 px-2 py-1 text-xs text-warning-strong">
                      待封装与验证
                    </span>
                  </div>
                  <p className="break-all text-xs text-muted-foreground">
                    {entry.repository}
                  </p>
                  <p className="break-words text-sm">{entry.purpose}</p>
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <span className="text-xs text-muted-foreground">
                      {entry.transport} · {entry.revision || "版本待固定"}
                    </span>
                    <button
                      className="hb-btn-outline text-xs"
                      onClick={() => edit(entry)}
                    >
                      查看封装草稿
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          )}
          <section
            aria-label="封装流程"
            className="flex flex-wrap items-center gap-3 text-sm text-muted-foreground"
          >
            <span className="hb-tag">项目与固定版本</span>
            <ArrowRight className="h-4 w-4" />
            <span className="hb-tag">输入输出与适配器</span>
            <ArrowRight className="h-4 w-4" />
            <span className="hb-tag">隔离验证与接入</span>
          </section>
          <p className="text-xs text-muted-foreground">
            其他项目可保存封装草稿并复制 AI 任务说明；目前仅 HyperFrames
            有已审阅的本机适配器。
          </p>
        </>
      )}
      {draft && (
        <dialog
          ref={dialog}
          aria-labelledby="capability-editor-title"
          aria-modal="true"
          onCancel={(event) => {
            event.preventDefault();
            close();
          }}
          onClick={(event) => {
            if (event.target === event.currentTarget) close();
          }}
          className="hb-editor-dialog !m-auto max-h-[88vh] w-[calc(100%-2rem)] max-w-2xl overflow-auto rounded-xl border border-border bg-card p-0 text-foreground shadow-2xl backdrop:bg-black/50"
        >
          <div className="space-y-4 p-5 sm:p-6">
            <header className="flex items-start justify-between gap-3">
              <div>
                <h2
                  id="capability-editor-title"
                  className="text-xl font-semibold"
                >
                  封装草稿
                </h2>
                <p className="mt-2 text-xs text-muted-foreground">
                  保存到本机。此处填写接口约定，不填写密钥正文。
                </p>
              </div>
              <button
                autoFocus
                aria-label="关闭封装草稿"
                className="rounded p-1.5 hover:bg-secondary"
                onClick={close}
              >
                <X className="h-4 w-4" />
              </button>
            </header>
            <form
              className="space-y-4"
              onSubmit={(event) => {
                event.preventDefault();
                save();
              }}
            >
              <label className="block text-sm">
                GitHub 仓库地址
                <input
                  required
                  type="url"
                  maxLength={500}
                  className="hb-input mt-1 w-full"
                  value={draft.repository}
                  onChange={(e) => {
                    setDraft({ ...draft, repository: e.target.value });
                    setPreview("");
                  }}
                  placeholder="https://github.com/owner/repository"
                />
              </label>
              <label className="block text-sm">
                能力名称
                <input
                  required
                  maxLength={120}
                  className="hb-input mt-1 w-full"
                  value={draft.name}
                  onChange={(e) => {
                    setDraft({ ...draft, name: e.target.value });
                    setPreview("");
                  }}
                />
              </label>
              <label className="block text-sm">
                用途
                <textarea
                  required
                  maxLength={2000}
                  className="hb-input mt-1 w-full"
                  value={draft.purpose}
                  onChange={(e) => {
                    setDraft({ ...draft, purpose: e.target.value });
                    setPreview("");
                  }}
                />
              </label>
              <div className="grid gap-4 sm:grid-cols-2">
                <label className="text-sm">
                  固定版本或提交
                  <input
                    maxLength={120}
                    className="hb-input mt-1 w-full"
                    value={draft.revision}
                    onChange={(e) => {
                      setDraft({ ...draft, revision: e.target.value });
                      setPreview("");
                    }}
                    placeholder="待核对后填写"
                  />
                </label>
                <label className="text-sm">
                  调用方式
                  <select
                    className="hb-input mt-1 w-full"
                    value={draft.transport}
                    onChange={(e) => {
                      setDraft({
                        ...draft,
                        transport: e.target
                          .value as CapabilityDraft["transport"],
                      });
                      setPreview("");
                    }}
                  >
                    <option>CLI</option>
                    <option>HTTP</option>
                    <option>MCP</option>
                  </select>
                </label>
              </div>
              <label className="block text-sm">
                输入约定
                <textarea
                  maxLength={8000}
                  className="hb-input mt-1 w-full"
                  value={draft.input}
                  onChange={(e) => {
                    setDraft({ ...draft, input: e.target.value });
                    setPreview("");
                  }}
                  placeholder="参数、类型、必填项及示例"
                />
              </label>
              <label className="block text-sm">
                输出约定
                <textarea
                  maxLength={8000}
                  className="hb-input mt-1 w-full"
                  value={draft.output}
                  onChange={(e) => {
                    setDraft({ ...draft, output: e.target.value });
                    setPreview("");
                  }}
                  placeholder="结果结构、文件及明确的错误信息"
                />
              </label>
              <div className="flex flex-wrap gap-3">
                <button type="submit" className="hb-btn-primary">
                  保存草稿
                </button>
                <button
                  type="button"
                  className="hb-btn-outline"
                  onClick={close}
                >
                  关闭
                </button>
              </div>
            </form>
            <button
              className="hb-btn-outline"
              disabled={
                JSON.stringify(draft) !== original.current ||
                !library.entries.some((entry) => entry.id === draft.id)
              }
              onClick={async () => {
                const value = capabilityTask(draft);
                setPreview(value);
                try {
                  await invoke("plugin:clipboard-manager|write_text", {
                    text: value,
                  });
                  setNotice("封装任务说明已复制，可交给 AI 继续实现。");
                } catch {
                  setNotice("复制失败，可选中下方全文手动复制。");
                }
              }}
            >
              <Copy className="h-4 w-4" />
              复制 AI 封装任务
            </button>
            {preview && (
              <div className="text-sm">
                <label htmlFor="capability-task-preview">封装任务说明</label>
                <textarea
                  id="capability-task-preview"
                  ref={text}
                  readOnly
                  value={preview}
                  className="hb-input mt-2 h-48 w-full font-mono text-xs"
                />
                <button
                  className="mt-2 text-xs underline"
                  onClick={() => {
                    text.current?.focus();
                    text.current?.select();
                  }}
                >
                  选中全文
                </button>
              </div>
            )}
            {notice && (
              <p role="status" className="text-sm text-muted-foreground">
                {notice}
              </p>
            )}
            {error && (
              <div
                role="alert"
                className="space-y-2 text-sm text-destructive-strong"
              >
                <p>{error}</p>
                <button
                  className="hb-btn-outline text-xs"
                  onClick={() => {
                    const current = load();
                    setLibrary(current);
                    if (!current.error) {
                      baseline.current = current.raw;
                      setError("");
                      setNotice("已重新读取，请核对后再保存；表单草稿已保留。");
                    }
                  }}
                >
                  重新读取并保留表单
                </button>
              </div>
            )}
          </div>
        </dialog>
      )}
    </div>
  );
}
