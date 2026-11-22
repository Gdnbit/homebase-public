//! Trusted adapter: private workspace policy and existing MCP/Skill catalogs.
use crate::{
    assistant_local::{LocalConfig, McpGrant, WorkspaceGrant},
    assistant_local_host::LocalHost,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

static HOST: Mutex<Option<Arc<LocalHost>>> = Mutex::const_new(None);
static POLICY_GATE: Mutex<()> = Mutex::const_new(());
static CANCELED: Mutex<std::collections::BTreeSet<String>> =
    Mutex::const_new(std::collections::BTreeSet::new());
fn runtime() -> Result<PathBuf, String> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    [
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("distribution/agent-runtime")))
            .unwrap_or_default(),
        manifest.join("../agent-runtime"),
        manifest.join("../../agent-runtime"),
    ]
    .into_iter()
    .find(|p| p.join("local/host.mjs").is_file())
    .ok_or_else(|| "本机工具运行包缺失".into())
}
async fn host() -> Result<Arc<LocalHost>, String> {
    let mut host = HOST.lock().await;
    if let Some(current) = host.as_ref().filter(|h| h.healthy()) {
        return Ok(current.clone());
    }
    let engine = super::service().await?;
    let current =
        LocalHost::start(&runtime()?, &engine.workspace.join("local-host-profile")).await?;
    *host = Some(current.clone());
    Ok(current)
}
fn protected() -> Vec<PathBuf> {
    let home = crate::adapters::homebase_home_dir();
    let mut paths = vec![
        home.join(".codex"),
        home.join(".agents"),
        home.join(".claude"),
        home.join(".openclaw"),
        home.join(".ssh"),
        home.join(".aws"),
        home.join(".azure"),
        home.join(".gnupg"),
        home.join(".kube"),
        home.join(".config/sops"),
        home.join("AppData/Roaming/sops"),
        home.join(".homebase/assistant"),
        home.join(".homebase/control"),
        home.join(".homebase/shared"),
        home.join(".homebase/wechat/channel.dpapi"),
    ];
    if let Some(real_home)=dirs::home_dir().filter(|p|p!=&home) {
        for relative in [".codex",".agents",".claude",".openclaw",".homebase",".ssh",".aws",".azure",".gnupg",".kube",".config/sops","AppData/Roaming/sops"] {paths.push(real_home.join(relative));}
    }
    {
        paths.extend([
            PathBuf::from(r"C:\HomeBase\Secrets"),
            PathBuf::from(r"C:\HomeBase\ServiceCredentials\providers"),
            PathBuf::from(r"C:\HomeBase\Private"),
            PathBuf::from(r"C:\HomeBase\Private"),
            PathBuf::from(r"C:\HomeBase\Wiki\raw"),
            PathBuf::from(r"C:\HomeBase\Wiki\_raw"),
        ]);
    }
    paths
}
fn overlap(a: &Path, b: &Path) -> bool {
    let normalize = |p: &Path| {
        let value = p
            .to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase();
        value.strip_prefix("//?/").unwrap_or(&value).to_string()
    };
    let a = normalize(a);
    let b = normalize(b);
    a == b || a.starts_with(&(b.clone() + "/")) || b.starts_with(&(a + "/"))
}
pub async fn save(mut config: LocalConfig) -> Result<Value, String> {
    let gate = POLICY_GATE.lock().await;
    crate::assistant_local::validate_mcp(&config)?;
    if let Some(host) = HOST.lock().await.as_ref() {
        if host.busy().await {
            return Err("本机任务正在执行，请先停止或等其完成后再调整授权；原授权未修改".into());
        }
    }
    if config.workspaces.len() > 32 || config.mcp_grants.len() > 64 {
        return Err("授权条目过多，请缩小范围".into());
    }
    let home = crate::adapters::homebase_home_dir();
    let mut ids = std::collections::BTreeSet::new();
    for workspace in &mut config.workspaces {
        if workspace.id == "scratch"
            || workspace.id.starts_with("skill-")
            || workspace.id.is_empty()
            || workspace.id.len() > 64
            || !workspace
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
            || !ids.insert(workspace.id.clone())
        {
            return Err("工作区编号无效、重复或使用保留编号".into());
        }
        if !matches!(workspace.access.as_str(), "read" | "write")
            || workspace.access == "read" && workspace.auto_write
        {
            return Err("工作区读写授权无效".into());
        }
        let path = PathBuf::from(&workspace.path);
        if !path.is_absolute() {
            return Err("工作区必须为绝对目录".into());
        }
        let path = path
            .canonicalize()
            .map_err(|_| "工作区目录不存在或不可访问")?;
        if !path.is_dir()
            || path.parent().is_none()
            || overlap(&path, &home) && path == home.canonicalize().unwrap_or(home.clone())
            || protected().iter().any(|p| overlap(&path, p))
        {
            return Err("该目录覆盖凭据、配置、私人根目录或磁盘根，请选择具体项目目录".into());
        }
        workspace.path = path.to_string_lossy().into();
    }
    ids.clear();
    for grant in &config.mcp_grants {
        if grant.id == "homebase-secrets"
            || grant.id.len() > 128
            || !ids.insert(grant.id.clone())
            || grant.fingerprint.len() != 64
            || !grant.fingerprint.bytes().all(|c| c.is_ascii_hexdigit())
            || grant.tools.len() > 300
            || grant.tools.iter().any(|t| t.is_empty() || t.len() > 256)
        {
            return Err("MCP 授权无效；不向助理开放密钥导出服务器".into());
        }
    }
    super::service().await?.save_local_config(config).await?;
    if let Some(host) = HOST.lock().await.as_ref() {
        host.reset_connections()
            .await
            .map_err(|_| "授权已保存，但MCP旧连接未确认关闭；请重启后继续，不要重写旧配置")?;
    }
    drop(gate);
    config_get().await
}
async fn workspaces(config: &LocalConfig) -> Result<Vec<WorkspaceGrant>, String> {
    let scratch = super::service().await?.workspace.join("agent");
    tokio::fs::create_dir_all(&scratch)
        .await
        .map_err(|_| "助理专属工作目录不可创建")?;
    let mut roots = vec![WorkspaceGrant {
        id: "scratch".into(),
        path: scratch.to_string_lossy().into(),
        access: "write".into(),
        auto_write: true,
    }];
    roots.extend(config.workspaces.clone());
    Ok(roots)
}

fn installed_skill_roots() -> Value {
    let home = crate::adapters::homebase_home_dir();
    json!([
        {"id":"shared","path":home.join(".homebase/shared/skills")},
        {"id":"codex","path":home.join(".codex/skills")},
        {"id":"agents","path":home.join(".agents/skills")},
        {"id":"claude","path":home.join(".claude/skills")},
        {"id":"openclaw","path":home.join(".openclaw/skills")},
    ])
}

pub(in crate::commands) async fn installed_skill_index() -> Result<Value, String> {
    host()
        .await?
        .execute(
            &format!("skill-index-{}", uuid::Uuid::new_v4()),
            "skill_index",
            json!({"skill_roots":installed_skill_roots()}),
            json!({}),
        )
        .await
}

fn configured_mcp_servers(config: &LocalConfig) -> Result<Vec<Value>, String> {
    let pool = crate::adapters::mcp_pool::load().map_err(|_| "MCP池读取失败，原配置未修改")?;
    let mut servers: Vec<Value> = pool.servers
        .into_iter()
        .map(|s| serde_json::to_value(s).unwrap())
        .collect();
    for server in &config.http_servers {
        if servers.iter().any(|s| s["id"] == server.id) {
            return Err("HTTP MCP编号与原共享池重复，请修改新增连接编号".into());
        }
        servers.push(server.definition());
    }
    Ok(servers)
}

async fn mcp_catalog(servers: &Value, grants: &[McpGrant]) -> Result<Value, String> {
    host().await?.execute(
        &format!("mcp-catalog-{}", uuid::Uuid::new_v4()),
        "mcp",
        json!({"mcp_servers":servers,"mcp_grants":grants}),
        json!({"action":"list"}),
    ).await
}

pub(in crate::commands) async fn mcp_index(config: &LocalConfig) -> Result<Value, String> {
    mcp_catalog(&json!(configured_mcp_servers(config)?), &config.mcp_grants).await
}

async fn context(config: &LocalConfig, operation: &str) -> Result<Value, String> {
    let engine = super::service().await?;
    let runtime = runtime()?;
    let servers = if matches!(operation, "mcp" | "catalog") {
        configured_mcp_servers(config)?
    } else {
        vec![]
    };
    let roots = if matches!(operation, "skill" | "catalog") {
        installed_skill_roots()
    } else {
        json!([])
    };
    Ok(
        json!({"workspaces":workspaces(config).await?,"default_workspace":"scratch","protected_paths":protected(),"output_root":engine.workspace.join("local-results"),"mcp_servers":servers,"mcp_grants":config.mcp_grants,"references":{},"mcp_profile_root":engine.workspace.join("mcp-profiles"),"skill_roots":roots,"sandbox":{"kind":"wsl-bubblewrap","distro":"Ubuntu","executable":runtime.join("tools/bwrap"),"sha256":crate::assistant_local::BWRAP_SHA256}}),
    )
}
pub async fn config_get() -> Result<Value, String> {
    let config = super::service().await?.local_config().await;
    let context = context(&config,"catalog").await?;
    let catalog = mcp_catalog(&context["mcp_servers"], &config.mcp_grants).await?;
    let mut view = json!({"config":config,"workspaces":context["workspaces"],"mcp_servers":catalog["servers"],"skill_sources":context["skill_roots"].as_array().map(|a|a.iter().map(|r|json!({"id":r["id"]})).collect::<Vec<_>>()).unwrap_or_default(),"terminal":{"backend":"wsl-bubblewrap","binary_present":runtime()?.join("tools/bwrap").is_file(),"state":"not_probed","network":"isolated","notice":"Linux命令环境，不是Windows宿主PowerShell；只挂载所选工作区，不继承真实密钥。运行前检查依赖，未就绪不回退宿主执行"}});
    let mut references = Vec::new();
    {
        let names=crate::adapters::service_credentials::configured_reference_names();
        if names.iter().any(|n|n.2!="configured_file_only") {view["credential_reference_notice"]=json!("部分API Key Manager文件状态无法确认，不能判定未配置；保留原引用，调用时再次校验");}
        references.extend(names.into_iter().map(|(id,name,state)|json!({"id":format!("api_key_manager:{id}"),"name":format!("API Key Manager · {name}{}",match state {"unknown"=>"（文件状态未知）","not_a_file"=>"（不是普通文件）",_=>""}),"state":state})));
    }
    match crate::adapters::secrets_store::list_secret_names() {
        Ok(names)=>references.extend(names.into_iter().filter(|name|crate::assistant_local::credential_reference(&format!("sops:{name}"))).map(|name|json!({"id":format!("sops:{name}"),"name":format!("密钥管理 · {name}"),"state":"name_only"}))),
        Err(_)=>view["credential_reference_notice"]=json!("密钥名称读取失败，不能判定未配置；已保存引用仍保留，调用时按原来源验证，不回退其他Key"),
    }
    view["credential_references"] = json!(references);
    Ok(view)
}
pub async fn execute(
    operation: String,
    request: Value,
    operation_id: String,
    policy_revision: u64,
) -> Result<Value, String> {
    let gate = POLICY_GATE.lock().await;
    if operation_id.is_empty()
        || operation_id.len() > 160
        || !operation_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err("本机操作编号无效".into());
    }
    if CANCELED.lock().await.contains(&operation_id) {
        return Ok(json!({"status":"canceled","message":"此操作在启动前已取消，未执行"}));
    }
    let mut config = super::service().await?.local_config().await;
    if config.revision != policy_revision {
        return Err("本机授权已变化，请刷新操作计划；未按过期授权执行".into());
    }
    if operation == "files" {
        if let Some(workspace_id) = request["workspace_id"]
            .as_str()
            .filter(|id| id.starts_with("skill-"))
        {
            let uuid =
                uuid::Uuid::parse_str(&workspace_id[6..]).map_err(|_| "技能结果目录编号无效")?;
            let run = super::service()
                .await?
                .workspace
                .join("local-results")
                .join(uuid.to_string());
            if !run.is_dir() {
                return Err("技能结果目录不存在，未猜测其他路径".into());
            }
            config.workspaces.push(WorkspaceGrant {
                id: workspace_id.into(),
                path: run.to_string_lossy().into(),
                access: "read".into(),
                auto_write: false,
            });
        }
    }
    crate::assistant_local::needs_confirmation(&config, &operation, &request)?;
    if operation == "files" && request["action"] == "workspaces" {
        return Ok(json!({"status":"succeeded","workspaces":workspaces(&config).await?}));
    }
    let mut context = context(&config,&operation).await?;
    if operation == "mcp" && request["action"] != "list" {
        let id = request["server_id"].as_str().ok_or("缺少MCP服务器编号")?;
        let definition = context["mcp_servers"]
            .as_array()
            .and_then(|a| a.iter().find(|s| s["id"] == id))
            .cloned()
            .ok_or("MCP服务器不存在")?;
        let grant = config
            .mcp_grants
            .iter()
            .find(|g| g.id == id)
            .ok_or("MCP尚未信任，未解析凭据")?;
        let resolve: crate::assistant_local::ResolveCredential = Arc::new(|reference| {
            Box::pin(async move {
                let key = if let Some(name) = reference.strip_prefix("sops:") {
                    crate::adapters::sops::SopsAdapter::for_vault(
                        &crate::secret_policy::vault_path(),
                    )
                    .decrypt_key(&crate::secret_policy::vault_path(), name)
                    .await
                    .map_err(|error| if error.to_string().contains("密钥已到期") {
                        "所选SOPS凭据已到期，已停止提供；请轮换或更新日期，未换用其他Key"
                    } else { "所选SOPS引用不可用，请检查凭据与到期策略；未回退其他来源" })?
                } else if let Some(provider) = reference.strip_prefix("api_key_manager:") {
                    crate::adapters::service_credentials::resolve_selected_api_key(provider, true)
                        .await?
                } else {
                    return Err("凭据引用无效，未读取任何值".into());
                };
                Ok(key.as_str().to_owned())
            })
        });
        context["references"] = match crate::assistant_local::materialize_references(
            &definition,
            grant,
            resolve,
        )
        .await
        {
            Ok(references) => references,
            Err(error) => {
                if let Some(host) = HOST.lock().await.clone() {
                    host.disconnect_server(id).await.map_err(|_|"MCP所选凭据或配置不可用，旧连接关闭未确认；请停止或重启后核对，未发起新调用")?;
                }
                return Err(error);
            }
        };
    }
    // The authenticated native command has already passed controller scope/assistant confirmation.
    // Permit this exact call only; never persist a tool grant requested by the model.
    if operation == "mcp" && request["action"] == "call" {
        if let Some(grants) = context["mcp_grants"].as_array_mut() {
            if let Some(grant) = grants.iter_mut().find(|g| g["id"] == request["server_id"]) {
                grant["tools"]
                    .as_array_mut()
                    .ok_or("MCP授权格式无效")?
                    .push(request["tool"].clone());
            }
        }
    }
    let host = host().await?;
    let reply = host
        .submit(&operation_id, &operation, context, request)
        .await?;
    drop(gate);
    host.result(&operation_id, reply).await
}
pub async fn progress(operation_id: String) -> Result<Value, String> {
    let host = HOST.lock().await.clone();
    Ok(match host {
        Some(h) => h.progress(&operation_id).await,
        None => {
            json!({"state":"not_running_here","message":"没有已验证的实时进程，请查询持久任务结果"})
        }
    })
}
pub async fn cancel(operation_id: String) -> Result<Value, String> {
    if operation_id.is_empty() || operation_id.len() > 160 {
        return Err("本机操作编号无效".into());
    }
    let mut canceled = CANCELED.lock().await;
    if canceled.len() >= 10000 && !canceled.contains(&operation_id) {
        return Err("取消索引已满，请核对原任务".into());
    }
    canceled.insert(operation_id.clone());
    drop(canceled);
    let host = HOST.lock().await.clone();
    if let Some(host) = host {
        host.cancel(&operation_id).await?;
    }
    Ok(json!({"state":"cancel_requested","message":"已请求停止；实际终态以原任务结果为准"}))
}
