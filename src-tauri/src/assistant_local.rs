//! Shared policy for local assistant tools. Model arguments cannot create grants.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc};

pub(crate) const BWRAP_SHA256: &str = "e318903862396f96de3df57264e0158682b952fd3fb53ac23d876413e7b30f71";

pub fn operation_failure(message: String) -> Value {
    let unknown=message.contains("结果未知") || message.starts_with("本机工具宿主已断开");
    json!({"status":if unknown{"unknown"}else{"failed"},"code":if unknown{"LOCAL_RESULT_UNKNOWN"}else{"LOCAL_OPERATION_REJECTED"},"message":message})
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialBinding {
    pub variable: String,
    pub reference: String,
}
pub fn credential_reference(reference: &str) -> bool {
    if let Some(name) = reference.strip_prefix("sops:") {
        return !name.is_empty()
            && name.len() <= 128
            && name != "sops"
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-.".contains(&c));
    }
    if let Some(provider) = reference.strip_prefix("api_key_manager:") {
        return !provider.is_empty()
            && provider.len() <= 64
            && provider != "yixiaoer"
            && provider
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c));
    }
    false
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpServer {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}
impl HttpServer {
    pub fn definition(&self) -> Value {
        json!({"id":self.id,"name":self.name,"description":self.description,"transport":"streamable-http","url":self.url,"headers":self.headers})
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceGrant {
    pub id: String,
    pub path: String,
    pub access: String,
    #[serde(default)]
    pub auto_write: bool,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct McpGrant {
    pub id: String,
    pub fingerprint: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub resources: bool,
    #[serde(default)]
    pub prompts: bool,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    #[serde(default)]
    pub credential_bindings: Vec<CredentialBinding>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    pub revision: u64,
    pub enabled: bool,
    pub workspaces: Vec<WorkspaceGrant>,
    pub mcp_grants: Vec<McpGrant>,
    #[serde(default)]
    pub http_servers: Vec<HttpServer>,
}
impl Default for LocalConfig {
    fn default() -> Self {
        Self {
            revision: 0,
            enabled: true,
            workspaces: vec![],
            mcp_grants: vec![],
            http_servers: vec![],
        }
    }
}
pub fn reference_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && !name.as_bytes()[0].is_ascii_digit()
}
fn credential_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("token")
        || name.contains("secret")
        || name.contains("password")
        || name.contains("authorization")
        || name.ends_with("key")
        || name.ends_with("key_id")
}
pub fn validate_mcp(config: &LocalConfig) -> Result<(), String> {
    if config.http_servers.len() > 32 {
        return Err("HTTP MCP数量超过32个".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for server in &config.http_servers {
        if !server.id.starts_with("assistant-http-")
            || server.id.len() > 80
            || !ids.insert(&server.id)
            || server.name.is_empty()
            || server.name.len() > 200
            || server.description.len() > 2000
        {
            return Err("HTTP MCP编号、名称或数量无效".into());
        }
        let url = reqwest::Url::parse(&server.url).map_err(|_| "HTTP MCP地址无效")?;
        if server.url.len() > 2000
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !(url.scheme() == "https"
                || url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
        {
            return Err(
                "远端MCP须HTTPS，本机可用回环HTTP；地址不接受账号、Token查询参数或片段".into(),
            );
        }
        if server.headers.len() > 8
            || server.headers.iter().any(|(name, value)| {
                !["authorization", "x-api-key", "api-key"]
                    .contains(&name.to_ascii_lowercase().as_str())
                    || value.len() > 200
                    || value.contains(['\r', '\n', '\0'])
                    || !value
                        .strip_prefix("Bearer ")
                        .unwrap_or(value)
                        .strip_prefix("${")
                        .and_then(|s| s.strip_suffix('}'))
                        .is_some_and(reference_name)
            })
        {
            return Err("认证头只接受已有凭据占位引用，不保存明文Key".into());
        }
        let references = required_references(&server.definition());
        if config
            .mcp_grants
            .iter()
            .filter(|g| g.id == server.id)
            .any(|g| references.iter().any(|name| g.variables.contains_key(name)))
        {
            return Err("HTTP认证头变量须选择已有Key引用，不能使用普通文本".into());
        }
    }
    for grant in &config.mcp_grants {
        if grant.variables.len() > 32 || grant.credential_bindings.len() > 12 {
            return Err("MCP运行变量或凭据引用过多".into());
        }
        let mut variables = std::collections::BTreeSet::new();
        for (name, value) in &grant.variables {
            if !reference_name(name)
                || credential_name(name)
                || value.len() > 2000
                || value.starts_with("sk-")
                || value.starts_with("Bearer ")
                || value.chars().any(char::is_control)
                || !variables.insert(name.as_str())
            {
                return Err("MCP普通变量无效；凭据变量须选择已有Key引用，不能填写明文".into());
            }
        }
        for binding in &grant.credential_bindings {
            if !reference_name(&binding.variable)
                || !variables.insert(binding.variable.as_str())
                || !credential_reference(&binding.reference)
            {
                return Err("请选择具体的已有凭据引用，不能留空或自动回退来源".into());
            }
        }
    }
    Ok(())
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map
                .iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(array) => Value::Array(array.iter().map(canonical).collect()),
        value => value.clone(),
    }
}
pub fn mcp_fingerprint(definition: &Value) -> String {
    use sha2::{Digest, Sha256};
    let mut data = json!({"transport":definition["transport"].as_str().unwrap_or("stdio"),"args":definition.get("args").cloned().unwrap_or(json!([])),"env":definition.get("env").cloned().unwrap_or(json!({})),"headers":definition.get("headers").cloned().unwrap_or(json!({}))});
    for name in ["command", "url"] {
        if let Some(value) = definition.get(name) {
            data[name] = value.clone();
        }
    }
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&canonical(&data)).unwrap())
    )
}
pub fn required_references(definition: &Value) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for field in ["env", "headers"] {
        if let Some(values) = definition[field].as_object() {
            for value in values.values().filter_map(Value::as_str) {
                let mut rest = value;
                while let Some((_, tail)) = rest.split_once("${") {
                    if let Some((name, next)) = tail.split_once('}') {
                        if reference_name(name) {
                            names.insert(name.to_string());
                        }
                        rest = next;
                    } else {
                        break;
                    }
                }
            }
        }
    }
    names.into_iter().collect()
}
pub type ResolveCredential = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync,
>;
pub async fn materialize_references(
    definition: &Value,
    grant: &McpGrant,
    resolve: ResolveCredential,
) -> Result<Value, String> {
    let command = definition["command"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    if definition["id"] == "homebase-secrets"
        || command.ends_with("homebase-mcp-secrets.exe")
        || command.ends_with("homebase-mcp-secrets")
    {
        return Err("密钥导出MCP不向助理开放".into());
    }
    if definition["id"] != grant.id || mcp_fingerprint(definition) != grant.fingerprint {
        return Err("MCP配置已变化，未解析或提供凭据".into());
    }
    let names = required_references(definition);
    let authentication_names = required_references(
        &json!({"headers":definition.get("headers").cloned().unwrap_or(json!({}))}),
    );
    // Verify the whole plan before asking any backend to materialize a credential.
    for name in &names {
        if grant.variables.contains_key(name) {
            if credential_name(name) || authentication_names.contains(name) {
                return Err("认证变量必须绑定已有Key引用，不能使用普通文本变量".into());
            }
        } else {
            let binding = grant
                .credential_bindings
                .iter()
                .find(|b| &b.variable == name)
                .ok_or("MCP所需变量未配置；未解析凭据或回退其他Key")?;
            if !credential_reference(&binding.reference) {
                return Err("MCP必须选择明确的已有凭据引用".into());
            }
        }
    }
    let mut references = serde_json::Map::new();
    for name in names {
        if let Some(value) = grant.variables.get(&name) {
            references.insert(name, Value::String(value.clone()));
            continue;
        }
        let binding=grant.credential_bindings.iter().find(|b|b.variable==name).ok_or("MCP所需变量未配置，请在本机能力选择已有凭据引用或填写非敏感运行变量；未回退其他Key")?;
        let value = resolve(binding.reference.clone())
            .await
            .map_err(|error| if error.contains("已到期") {
                "所选MCP凭据已到期，已停止提供；请轮换或更新日期，未回退或覆盖"
            } else { "所选MCP凭据引用不可用，请检查已有Key的状态与来源，未回退或覆盖" })?;
        if value.is_empty() {
            return Err("所选MCP凭据为空，未连接".into());
        }
        references.insert(name, Value::String(value));
    }
    Ok(Value::Object(references))
}
pub fn operation(name: &str) -> Option<&'static str> {
    match name {
        "homebase_files" => Some("files"),
        "homebase_terminal" => Some("terminal"),
        "homebase_mcp" => Some("mcp"),
        "homebase_installed_skill" => Some("skill"),
        _ => None,
    }
}
pub fn readonly_operation(operation:&str,request:&Value)->bool{
    match operation {
        "files"=>matches!(request["action"].as_str(),Some("workspaces"|"list"|"stat"|"read"|"search")),
        "skill"=>matches!(request["action"].as_str(),Some("list"|"read"|"resource"|"inspect_run")),
        "mcp"=>matches!(request["action"].as_str(),Some("list"|"tools"|"describe"|"resources"|"read_resource"|"prompts"|"get_prompt")),
        _=>false,
    }
}
pub fn needs_confirmation(
    config: &LocalConfig,
    operation: &str,
    request: &Value,
) -> Result<bool, String> {
    if !config.enabled {
        return Err("本机工具已停用，请在助理的本机能力设置中开启".into());
    }
    let action = request["action"].as_str().unwrap_or("");
    match operation {
        "files" | "terminal" => {
            if operation == "files" && action == "workspaces" {
                return Ok(false);
            }
            let workspace = request["workspace_id"].as_str().unwrap_or("scratch");
            let grant = config.workspaces.iter().find(|w| w.id == workspace);
            if workspace != "scratch" && grant.is_none() {
                return Err(
                    "目录尚未授权，请在本机能力设置添加工作区；助理不能自行扩大授权".into(),
                );
            }
            let write = operation == "terminal" && request["write"] != false
                || operation == "files"
                    && matches!(
                        action,
                        "write" | "edit" | "mkdir" | "copy" | "move" | "trash"
                    );
            if write && grant.is_some_and(|g| g.access != "write") {
                return Err("此工作区只读，未执行写入".into());
            }
            if operation == "files" && matches!(action, "edit" | "move" | "trash") {
                return Ok(true);
            }
            Ok(write && workspace != "scratch" && !grant.is_some_and(|g| g.auto_write))
        }
        "skill" => Ok(request["action"] == "run"),
        "mcp" => {
            if action == "list" {
                return Ok(false);
            }
            let server = request["server_id"].as_str().ok_or("缺少 MCP 服务器编号")?;
            let grant = config
                .mcp_grants
                .iter()
                .find(|g| g.id == server)
                .ok_or("此 MCP 尚未信任，请先在本机能力设置核对并授权连接")?;
            if action == "call" {
                return Ok(!grant
                    .tools
                    .iter()
                    .any(|t| Some(t.as_str()) == request["tool"].as_str()));
            }
            if matches!(action, "resources" | "read_resource") && !grant.resources {
                return Err("未授权此 MCP 的资源读取".into());
            }
            if matches!(action, "prompts" | "get_prompt") && !grant.prompts {
                return Err("未授权此 MCP 的提示词读取".into());
            }
            Ok(false)
        }
        _ => Err("本机操作类型未登记".into()),
    }
}
/// Ignore per-call receipt IDs only for effects. Fresh metadata/file reads stay fresh.
pub fn same_effect(left: &Value, right: &Value) -> bool {
    if left["operation"]!=right["operation"] || left["policyRevision"]!=right["policyRevision"] || left["request"]!=right["request"] {return false;}
    match left["operation"].as_str() {
        Some("terminal")=>true,
        Some("files")=>matches!(left["request"]["action"].as_str(),Some("write"|"edit"|"mkdir"|"copy"|"move"|"trash"|"export")),
        Some("mcp")=>left["request"]["action"]=="call",
        Some("skill")=>left["request"]["action"]=="run",
        _=>false,
    }
}
pub fn command_args(
    operation: &str,
    request: &Value,
    turn: &str,
    call: &str,
    revision: u64,
) -> Value {
    // IDs originate from the validated turn/tool pair; identical retries reuse the controller receipt.
    use sha2::{Digest, Sha256};
    json!({"operation":operation,"request":request,"operationId":format!("local-{turn}-{:x}",Sha256::digest(call.as_bytes())),"policyRevision":revision})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_fingerprint_matches_official_sdk_host_and_http_auth_never_accepts_literal_keys() {
        let definition = json!({"command":"C:/synthetic/node.exe","args":["server.js"],"env":{"TOKEN":"${TOKEN}"}});
        assert_eq!(
            mcp_fingerprint(&definition),
            "1e0991f5594770a593a3d25c870b7b0b3fdb4f1660bf573151295d3d31b54777"
        );
        let mut config = LocalConfig::default();
        config.http_servers.push(HttpServer {
            id: "assistant-http-fixture".into(),
            name: "合成HTTP".into(),
            description: String::new(),
            url: "http://127.0.0.1:12345/mcp".into(),
            headers: BTreeMap::from([("Authorization".into(), "Bearer ${MCP_TOKEN}".into())]),
        });
        assert!(validate_mcp(&config).is_ok());
        config.http_servers[0].headers.insert(
            "Authorization".into(),
            "Bearer SYNTHETIC-LITERAL-REJECTED".into(),
        );
        assert!(validate_mcp(&config).is_err());
        config.http_servers[0].headers.clear();
        config.http_servers[0].url = "https://example.invalid/mcp?token=synthetic".into();
        assert!(validate_mcp(&config).is_err());
        config.http_servers[0].url = "http://example.invalid/mcp".into();
        assert!(validate_mcp(&config).is_err());
    }
    #[tokio::test]
    async fn references_resolve_only_for_exactly_trusted_server_and_explicit_source() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let resolve: ResolveCredential = Arc::new(move |reference| {
            let counter = counter.clone();
            Box::pin(async move {
                assert_eq!(reference, "api_key_manager:openrouter");
                counter.fetch_add(1, Ordering::SeqCst);
                Ok("SYNTHETIC-KEY-NOT-REAL".into())
            })
        });
        let definition = json!({"id":"fixture","command":"C:/synthetic/node.exe","env":{"KEY":"${TOKEN}","MODEL":"${MODEL_ID}"}});
        let mut grant = McpGrant {
            id: "fixture".into(),
            fingerprint: mcp_fingerprint(&definition),
            variables: BTreeMap::from([("MODEL_ID".into(), "synthetic-model".into())]),
            credential_bindings: vec![
                CredentialBinding {
                    variable: "TOKEN".into(),
                    reference: "api_key_manager:openrouter".into(),
                },
                CredentialBinding {
                    variable: "UNUSED_TOKEN".into(),
                    reference: "sops:github_token".into(),
                },
            ],
            ..Default::default()
        };
        assert!(!serde_json::to_string(&grant)
            .unwrap()
            .contains("SYNTHETIC-KEY-NOT-REAL"));
        let values = materialize_references(&definition, &grant, resolve.clone())
            .await
            .unwrap();
        assert_eq!(values["TOKEN"], "SYNTHETIC-KEY-NOT-REAL");
        assert_eq!(values["MODEL_ID"], "synthetic-model");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let mut changed = definition.clone();
        changed["command"] = json!("C:/other.exe");
        assert!(materialize_references(&changed, &grant, resolve.clone())
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        grant.credential_bindings[0].reference = String::new();
        assert!(materialize_references(&definition, &grant, resolve.clone())
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        grant
            .variables
            .insert("TOKEN".into(), "SYNTHETIC-LITERAL".into());
        let mut config = LocalConfig::default();
        config.mcp_grants.push(grant);
        assert!(validate_mcp(&config).is_err());
    }
    #[test]
    fn grants_are_explicit_and_existing_files_require_concrete_approval() {
        let mut config = LocalConfig::default();
        assert!(!needs_confirmation(&config, "files", &json!({"action":"write"})).unwrap());
        assert!(needs_confirmation(&config, "files", &json!({"action":"edit"})).unwrap());
        assert!(needs_confirmation(&config, "terminal", &json!({"workspace_id":"other"})).is_err());
        config.workspaces.push(WorkspaceGrant {
            id: "project".into(),
            path: "unused".into(),
            access: "read".into(),
            auto_write: false,
        });
        assert!(
            needs_confirmation(&config, "terminal", &json!({"workspace_id":"project"})).is_err()
        );
        assert!(!needs_confirmation(
            &config,
            "terminal",
            &json!({"workspace_id":"project","write":false})
        )
        .unwrap());
        assert!(needs_confirmation(
            &config,
            "mcp",
            &json!({"action":"tools","server_id":"not-granted"})
        )
        .is_err());
        assert!(needs_confirmation(&config, "skill", &json!({"action":"run"})).unwrap());
    }
}
