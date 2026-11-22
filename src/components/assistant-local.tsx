import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderPlus, RefreshCw, X } from "lucide-react";

interface Workspace { id: string; path: string; access: "read" | "write"; auto_write: boolean }
interface Binding { variable: string; reference: string }
interface Grant { id: string; fingerprint: string; tools: string[]; resources: boolean; prompts: boolean; variables?: Record<string, string>; credential_bindings?: Binding[] }
interface HttpServer { id: string; name: string; description: string; url: string; headers: Record<string, string> }
interface Config { revision: number; enabled: boolean; workspaces: Workspace[]; mcp_grants: Grant[]; http_servers?: HttpServer[] }
interface Catalog {
  config: Config;
  workspaces: Workspace[];
  mcp_servers: { id: string; name: string; description: string; state: string; fingerprint: string; required_references?: string[]; credential_references?: string[] }[];
  credential_references?: { id: string; name: string }[];
  credential_reference_notice?: string;
  skill_sources: { id: string }[];
  terminal: { binary_present: boolean; notice: string };
}
const queryKey = ["assistant", "local"];
const button = "inline-flex items-center justify-center gap-2 rounded-lg border border-border px-3 py-2 text-sm hover:bg-secondary disabled:opacity-50";

export function AssistantLocal({ shown, close }: { shown: boolean; close: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const client = useQueryClient();
  const [draft, setDraft] = useState<Config | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [httpName, setHttpName] = useState("");
  const [httpUrl, setHttpUrl] = useState("");
  const [httpAuth, setHttpAuth] = useState("none");
  const [editingHttp, setEditingHttp] = useState<string | null>(null);
  const catalog = useQuery({ queryKey, queryFn: () => invoke<Catalog>("assistant_local_config_get"), enabled: shown, staleTime: 60_000, retry: false });
  useEffect(() => {
    if (shown) dialog.current?.showModal();
    else { dialog.current?.close(); setDraft(null); setError(""); setSaved(false); setHttpName(""); setHttpUrl(""); setHttpAuth("none"); setEditingHttp(null); }
  }, [shown]);
  useEffect(() => { if (shown && catalog.data && !draft) setDraft(structuredClone(catalog.data.config)); }, [shown, catalog.data, draft]);
  const dirty = !!draft && JSON.stringify(draft) !== JSON.stringify(catalog.data?.config);
  async function addWorkspace() {
    setError("");
    try {
      const path = await open({ directory: true, multiple: false, title: "选择助理可以访问的项目目录" });
      if (typeof path === "string") setDraft(old => old && ({ ...old, workspaces: [...old.workspaces, { id: `workspace-${crypto.randomUUID().slice(0, 8)}`, path, access: "read", auto_write: false }] }));
    } catch (error) { setError(String(error)); }
  }
  async function save() {
    if (!draft) return;
    if (httpName.trim() || httpUrl.trim()) { setError("请先加入或更新待保存的 HTTP 连接，或清空该表单，再保存授权"); return; }
    if (draft.mcp_grants.some(grant => grant.credential_bindings?.some(binding => !binding.reference))) { setError("请选择要提供给目标连接的具体凭据，或将该变量设为暂不提供"); return; }
    setBusy(true); setError(""); setSaved(false);
    try {
      const result = await invoke<Catalog>("assistant_local_config_save", { config: draft });
      client.setQueryData(queryKey, result); setDraft(structuredClone(result.config)); setSaved(true);
    } catch (error) { setError(String(error)); }
    finally { setBusy(false); }
  }
  async function refresh() {
    setError("");
    const result = await catalog.refetch();
    if (result.data) setDraft(structuredClone(result.data.config));
  }
  function toggleServer(server: Catalog["mcp_servers"][number]) {
    setDraft(old => old && ({ ...old, mcp_grants: old.mcp_grants.some(g => g.id === server.id && g.fingerprint === server.fingerprint)
      ? old.mcp_grants.filter(g => g.id !== server.id)
      : [...old.mcp_grants.filter(g => g.id !== server.id), { id: server.id, fingerprint: server.fingerprint, tools: [], resources: false, prompts: false }] }));
  }
  function addHttp() {
    if (!draft || !httpName.trim() || !httpUrl.trim()) { setError("请填写连接名称和 HTTP MCP 地址"); return; }
    const headers: Record<string, string> = httpAuth === "none" ? {} : httpAuth === "bearer" ? { Authorization: "Bearer ${MCP_TOKEN}" } : { "X-API-Key": "${MCP_TOKEN}" };
    const server = { id: editingHttp || `assistant-http-${crypto.randomUUID().slice(0, 8)}`, name: httpName.trim(), description: "由本人添加的 HTTP MCP", url: httpUrl.trim(), headers };
    const previous = draft.http_servers?.find(s => s.id === editingHttp);
    const changedTarget = previous && (previous.url !== server.url || JSON.stringify(previous.headers) !== JSON.stringify(headers));
    setDraft({ ...draft, http_servers: [...(draft.http_servers || []).filter(s => s.id !== editingHttp), server], mcp_grants: changedTarget ? draft.mcp_grants.filter(g => g.id !== editingHttp) : draft.mcp_grants });
    setEditingHttp(null); setHttpName(""); setHttpUrl(""); setError("");
  }
  return <dialog ref={dialog} aria-labelledby="assistant-local-title" onCancel={close} className="m-auto w-[min(850px,calc(100vw-32px))] rounded-2xl border border-border bg-card p-0 text-foreground shadow-xl backdrop:bg-black/30">
    <div className="flex max-h-[85vh] flex-col">
      <header className="flex items-start justify-between border-b border-border p-5">
        <div><h2 id="assistant-local-title" className="font-serif text-xl">本机能力</h2><p className="mt-1 text-sm text-muted-foreground">微信与桌面共用。只开放你选择的目录和连接。</p></div>
        <button className={button} aria-label="关闭本机能力" onClick={close}><X size={16} /></button>
      </header>
      <div className="space-y-6 overflow-auto p-5">
        {(error || catalog.error) && <p role="alert" className="text-sm text-destructive">{error || String(catalog.error)}</p>}
        {catalog.isPending && <p role="status">正在读取本机能力…</p>}
        {draft && <>
          <label className="flex items-center gap-2 text-sm"><input type="checkbox" checked={draft.enabled} onChange={event => setDraft({ ...draft, enabled: event.target.checked })} />允许助理使用本机工具</label>
          <section aria-label="工作区授权" className="space-y-3">
            <div className="flex items-center justify-between"><h3 className="font-medium">文件与终端工作区</h3><button className={button} onClick={() => void addWorkspace()} disabled={busy}><FolderPlus size={16} />添加目录</button></div>
            <div className="rounded-xl border border-border p-3 text-sm"><span className="font-medium">助理专属目录 · scratch</span><p className="mt-1 break-all text-muted-foreground">{catalog.data?.workspaces.find(w => w.id === "scratch")?.path}</p><p className="mt-2 text-muted-foreground">可直接新建文件、运行隔离命令。修改已有文件、移动和回收仍会具体确认。</p></div>
            {draft.workspaces.map(workspace => <div key={workspace.id} className="space-y-2 rounded-xl border border-border p-3 text-sm">
              <div className="flex items-start justify-between gap-3"><div className="min-w-0"><p className="break-all font-medium">{workspace.path}</p><p className="mt-1 text-xs text-muted-foreground">{workspace.id}</p></div><button className={button} aria-label={`移除工作区 ${workspace.id}`} onClick={() => setDraft({ ...draft, workspaces: draft.workspaces.filter(w => w.id !== workspace.id) })}>移除</button></div>
              <label className="flex items-center gap-2">访问方式<select aria-label={`工作区 ${workspace.id} 访问方式`} value={workspace.access} className="rounded-lg border border-border bg-background p-1" onChange={event => setDraft({ ...draft, workspaces: draft.workspaces.map(w => w.id === workspace.id ? { ...w, access: event.target.value as Workspace["access"], auto_write: false } : w) })}><option value="read">只读</option><option value="write">允许写入，逐次确认</option></select></label>
              {workspace.access === "write" && <label className="flex items-start gap-2"><input type="checkbox" className="mt-1" checked={workspace.auto_write} onChange={event => setDraft({ ...draft, workspaces: draft.workspaces.map(w => w.id === workspace.id ? { ...w, auto_write: event.target.checked } : w) })} /><span>允许在此目录自动新建与执行隔离命令。命令可以修改该目录内现有文件；文件工具的编辑、移动和回收仍需确认。</span></label>}
            </div>)}
            <p className="text-xs leading-6 text-muted-foreground">终端：{catalog.data?.terminal.binary_present ? "执行器文件已找到，运行时检查 WSL 与依赖" : "执行器文件缺失，运行会明确失败"}。{catalog.data?.terminal.notice}</p>
          </section>
          <section aria-label="MCP连接授权" className="space-y-3">
            <h3 className="font-medium">已有 MCP</h3><p className="text-sm leading-6 text-muted-foreground">本机 MCP 按当前 Windows 用户权限运行，不受终端沙箱限制。请先核对共享配置中的程序和参数。这里不改原池；连接授权不自动提供 Key，可另选已有凭据引用，仅在连接时由后端注入。工具调用默认逐次确认，资源与提示词可单独授权。</p>
            {!catalog.data?.mcp_servers.length && <p className="text-sm text-muted-foreground">当前没有已登记 MCP。</p>}
            {catalog.data?.mcp_servers.map(server => {
              const grant = draft.mcp_grants.find(g => g.id === server.id && g.fingerprint === server.fingerprint);
              const secretExport = server.state === "blocked_secret_export";
              const literalCredential = server.state === "needs_secure_reference";
              const blocked = secretExport || literalCredential;
              return <div key={server.id} className="rounded-xl border border-border p-3 text-sm">
                <div className="flex items-center justify-between gap-3"><div><p className="font-medium">{server.name}</p><p className="mt-1 text-muted-foreground">{server.description}</p></div><button className={button} disabled={blocked || busy} onClick={() => toggleServer(server)}>{secretExport ? "凭据导出不开放" : literalCredential ? "请先改为凭据引用" : grant ? "撤销连接" : "信任此连接"}</button></div>
                {literalCredential && <p className="mt-2 text-xs text-muted-foreground">原连接配置含凭据字面量。请在 MCP 管理中改为变量占位，再在这里绑定已有 Key 引用；当前不会启动该连接。</p>}
                <p className="mt-2 break-all text-xs text-muted-foreground">配置指纹：{server.fingerprint}</p>
                {grant && <div className="mt-3 flex flex-wrap gap-4">{(["resources", "prompts"] as const).map(kind => <label key={kind} className="flex items-center gap-2"><input type="checkbox" checked={grant[kind]} onChange={event => setDraft({ ...draft, mcp_grants: draft.mcp_grants.map(g => g.id === server.id ? { ...g, [kind]: event.target.checked } : g) })} />{kind === "resources" ? "读取资源" : "读取提示词"}</label>)}</div>}
                {grant && !!server.required_references?.length && <McpReferences grant={grant} names={server.required_references} credentialNames={server.credential_references || []} choices={catalog.data?.credential_references || []} update={next => setDraft({ ...draft, mcp_grants: draft.mcp_grants.map(g => g.id === server.id ? next : g) })} />}
              </div>;
            })}
            {catalog.data?.credential_reference_notice && <p className="text-xs text-muted-foreground">{catalog.data.credential_reference_notice}</p>}
            <details className="rounded-xl border border-border p-3 text-sm"><summary className="cursor-pointer font-medium">添加 HTTP MCP</summary>
              <div className="mt-3 space-y-3">
                <p className="text-xs leading-6 text-muted-foreground">使用 Streamable HTTP。远端须 HTTPS，本机可用回环 HTTP；URL 不填 Key、账号或查询参数。保存后再核对并信任连接，需要认证时选择已有 Key 引用；OAuth 登录暂需另行完成。</p>
                <label className="block">连接名称<input aria-label="HTTP MCP名称" className="mt-1 w-full rounded-lg border border-border bg-background p-2" value={httpName} onChange={event => setHttpName(event.target.value)} /></label>
                <label className="block">服务地址<input aria-label="HTTP MCP地址" placeholder="https://example.com/mcp" className="mt-1 w-full rounded-lg border border-border bg-background p-2" value={httpUrl} onChange={event => setHttpUrl(event.target.value)} /></label>
                <label className="block">认证方式<select aria-label="HTTP MCP认证方式" className="ml-2 rounded-lg border border-border bg-background p-2" value={httpAuth} onChange={event => setHttpAuth(event.target.value)}><option value="none">无需认证</option><option value="bearer">Bearer 凭据引用</option><option value="api-key">X-API-Key 凭据引用</option></select></label>
                <button className={button} onClick={addHttp}>{editingHttp ? "更新待保存连接" : "加入待保存连接"}</button>
                {(draft.http_servers || []).map(server => <div key={server.id} className="flex items-start justify-between gap-3 rounded-lg border border-border p-2"><div className="min-w-0"><p>{server.name}</p><p className="break-all text-xs text-muted-foreground">{server.url}</p></div><div className="flex gap-2"><button className={button} aria-label={`编辑HTTP连接 ${server.name}`} onClick={() => { setEditingHttp(server.id); setHttpName(server.name); setHttpUrl(server.url); setHttpAuth(server.headers.Authorization ? "bearer" : server.headers["X-API-Key"] ? "api-key" : "none"); }}>编辑</button><button className={button} aria-label={`移除HTTP连接 ${server.name}`} onClick={() => setDraft({ ...draft, http_servers: draft.http_servers?.filter(s => s.id !== server.id), mcp_grants: draft.mcp_grants.filter(g => g.id !== server.id) })}>移除</button></div></div>)}
              </div>
            </details>
          </section>
          <section className="space-y-2"><h3 className="font-medium">已安装 Skill</h3><p className="text-sm text-muted-foreground">已登记来源：{catalog.data?.skill_sources.map(s => s.id).join("、") || "暂无"}。助理按需发现和读取说明、包内资源；执行脚本前查看资源指纹并确认，只在新的隔离目录运行。平台、登录、网络或依赖不满足时会说明。</p></section>
        </>}
      </div>
      <footer className="flex items-center justify-between gap-3 border-t border-border p-5">
        <span role="status" className="text-sm text-muted-foreground">{saved && !dirty ? "授权已保存，微信与桌面共用" : dirty ? "有未保存的授权更改" : "授权不会修改模型设置或已有密钥"}</span>
        <div className="flex gap-2"><button className={button} disabled={busy || dirty || catalog.isFetching} aria-label="刷新本机能力" onClick={() => void refresh()}><RefreshCw size={16} /></button><button className={`${button} bg-primary text-primary-foreground hover:bg-primary/90`} disabled={!draft || busy || !dirty} onClick={() => void save()}>{busy ? "正在保存…" : "保存授权"}</button></div>
      </footer>
    </div>
  </dialog>;
}

function McpReferences({ grant, names, credentialNames, choices, update }: { grant: Grant; names: string[]; credentialNames: string[]; choices: { id: string; name: string }[]; update: (grant: Grant) => void }) {
  function change(name: string, kind: string) {
    const variables = { ...(grant.variables || {}) }; delete variables[name];
    const bindings = (grant.credential_bindings || []).filter(b => b.variable !== name);
    if (kind === "literal") variables[name] = "";
    if (kind === "credential") bindings.push({ variable: name, reference: "" });
    update({ ...grant, variables, credential_bindings: bindings });
  }
  return <div className="mt-3 space-y-3 border-t border-border pt-3">
    <p className="text-xs leading-6 text-muted-foreground">只保存引用，不显示或改写真实 Key。所选来源在连接时验证，失败不换 Key；提供给 MCP 的凭据可能用于其外部调用并产生费用，请只授权可信服务。</p>
    {names.map(name => {
      const binding = grant.credential_bindings?.find(b => b.variable === name);
      const literal = Object.hasOwn(grant.variables || {}, name);
      const sensitive = credentialNames.includes(name) || /token|secret|password|authorization|key(?:_id)?$/i.test(name);
      return <div key={name} className="space-y-2"><label className="flex flex-wrap items-center gap-2"><span className="break-all">{name}</span><select aria-label={`${name} 提供方式`} value={binding ? "credential" : literal ? "literal" : "missing"} className="rounded-lg border border-border bg-background p-1" onChange={event => change(name, event.target.value)}><option value="missing">暂不提供</option>{!sensitive && <option value="literal">普通运行变量</option>}<option value="credential" disabled={!choices.length && !binding}>已有 Key 引用</option></select></label>
        {literal && <input aria-label={`${name} 运行变量`} placeholder="只填非敏感文本，不填Key" className="w-full rounded-lg border border-border bg-background p-2" value={grant.variables?.[name] || ""} onChange={event => update({ ...grant, variables: { ...grant.variables, [name]: event.target.value } })} />}
        {binding && <select aria-label={`${name} 凭据引用`} className="max-w-full rounded-lg border border-border bg-background p-2" value={binding.reference} onChange={event => update({ ...grant, credential_bindings: grant.credential_bindings?.map(b => b.variable === name ? { ...b, reference: event.target.value } : b) })}><option value="">请选择要提供给此连接的具体凭据</option>{binding.reference && !choices.some(choice => choice.id === binding.reference) && <option value={binding.reference}>已保存引用 · {binding.reference}（目录暂未读取到）</option>}{choices.map(choice => <option key={choice.id} value={choice.id}>{choice.name}</option>)}</select>}
      </div>;
    })}
  </div>;
}
