//! Functional model library. All credentials are resolved in the backend at use time.
use crate::model_capability_io::{
    authorized, bytes, client, discover, response_result, validate_output_root, ResponseMode,
};
use crate::{
    adapters::{service_credentials, sops::SopsAdapter},
    model_capabilities::*,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

#[derive(Default)]
struct RemoteCatalog {
    models: BTreeMap<String, Vec<ModelCapability>>,
    sources: BTreeMap<String, ModelCatalogSource>,
    pins: BTreeMap<String, ModelCapability>,
    pin_error: Option<String>,
    ordered_models: Option<Vec<ModelCapability>>,
}
impl RemoteCatalog {
    fn ordered_models(&mut self) -> &[ModelCapability] {
        let pins = &self.pins;
        let remotes = &self.models;
        self.ordered_models.get_or_insert_with(|| {
            let mut models = reference_models().to_vec();
            for route in pins.values() {
                models.retain(|model| model.route_id != route.route_id);
                models.push(route.clone());
            }
            for (provider, rows) in remotes {
                merge_models(&mut models, provider, rows.clone());
            }
            models
        })
    }
}
fn reference_models() -> &'static [ModelCapability] {
    static MODELS: OnceLock<Vec<ModelCapability>> = OnceLock::new();
    MODELS.get_or_init(seed_models)
}
fn pin_path() -> std::path::PathBuf {
    crate::adapters::homebase_home_dir()
        .join(".homebase")
        .join("assistant")
        .join("model-routes.json")
}
fn cache() -> &'static Mutex<RemoteCatalog> {
    static CACHE: OnceLock<Mutex<RemoteCatalog>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let mut store = RemoteCatalog::default();
        let read = || -> std::io::Result<Vec<u8>> {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(pin_path())?
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        match read() {
            Ok(bytes) => match parse_pins(&bytes) {
                Ok(pins) => store.pins = pins,
                Err(e) => store.pin_error = Some(e),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => store.pin_error = Some("所选模型元数据读取失败；未覆盖或静默丢弃".into()),
        }
        Mutex::new(store)
    })
}
fn parse_pins(bytes: &[u8]) -> Result<BTreeMap<String, ModelCapability>, String> {
    parse_route_pins(bytes)
}
pub(crate) async fn remember_route(route: ModelCapability) -> Result<(), String> {
    static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _guard = SERIAL
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let mut pins = {
        let store = cache().lock().map_err(|_| "模型元数据不可读取")?;
        if let Some(e) = &store.pin_error {
            return Err(e.clone());
        }
        store.pins.clone()
    };
    pins.insert(route.route_id.clone(), route);
    let bytes = serde_json::to_vec(&pins).map_err(|_| "模型元数据编码失败")?;
    parse_pins(&bytes)?;
    let file = pin_path();
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|_| "模型元数据目录不可创建")?;
    crate::control_protocol::publish_private(&file, &bytes)
        .await
        .map_err(|_| "所选模型元数据保存失败，未启用助理")?;
    let mut store = cache().lock().map_err(|_| "模型元数据不可读取")?;
    store.pins = pins;
    store.ordered_models = None;
    Ok(())
}
async fn credential(
    provider: &str,
    source: CredentialSource,
) -> Result<crate::adapters::sops::SecureString, String> {
    if source == CredentialSource::ApiKeyManager {
        return service_credentials::resolve_selected_api_key(provider, true).await;
    }
    if [
        "openrouter",
        "fal",
        "dashscope",
        "ark",
        "minimax",
        "elevenlabs",
        "doubao",
    ]
    .contains(&provider)
        && source == CredentialSource::Default
    {
        return service_credentials::resolve_api_key(provider).await;
    }
    let alias = PROVIDERS
        .iter()
        .find(|p| p.0 == provider)
        .ok_or("未登记的模型渠道")?
        .2;
    SopsAdapter::for_vault(&crate::secret_policy::vault_path())
        .decrypt_key(&crate::secret_policy::vault_path(), alias)
        .await
        .map_err(|_| "所选渠道密钥不可用；请检查配置和生命周期策略，未回退其他渠道".into())
}
async fn inventory() -> (
    Vec<CapabilityCredential>,
    Vec<ServiceCapability>,
    Vec<String>,
) {
    let mut issues = Vec::new();
    let secrets = match super::secrets::secrets_list().await {
        Ok(v) => Some(v),
        Err(_) => {
            issues.push("SOPS 密钥状态读取失败；不能判定未配置，已有目录仍可查看".into());
            None
        }
    };
    let references = match service_credentials::status().await {
        Ok(v) => Some(v),
        Err(_) => {
            issues.push("独立凭据引用状态读取失败；不能判定未配置".into());
            None
        }
    };
    let credentials = PROVIDERS
        .iter()
        .map(|(id, name, alias)| {
            let entry = secrets
                .as_ref()
                .and_then(|s| s.iter().find(|s| s.short_name == *alias));
            let external = references
                .as_ref()
                .and_then(|r| r.iter().find(|s| s.service_id == *id));
            let expired = entry.is_some_and(|e| {
                e.expires_at.as_deref().is_some_and(|s| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map(|d| d.timestamp() <= chrono::Utc::now().timestamp())
                        .unwrap_or(true)
                })
            });
            let (source, state, reference, message) = binding_choice(
                secrets.is_some(),
                entry.map(|e| e.value_state.as_str()),
                external.map(|e| (e.state, e.credential_ref.as_str())),
                alias,
                expired,
                entry.map(|e| e.expiry_policy.as_str()).unwrap_or("warn"),
            );
            CapabilityCredential {
                provider_id: id.to_string(),
                provider_name: name.to_string(),
                credential_ref: reference,
                source,
                state,
                message,
                remote_state: "not_checked".into(),
                expires_at: entry.and_then(|e| e.expires_at.clone()),
                expiry_warning: expired,
            }
        })
        .collect();
    let mut services = Vec::new();
    if let Some(secrets) = secrets {
        for s in secrets {
            if PROVIDERS.iter().any(|p| p.2 == s.short_name) {
                continue;
            }
            let (name, function, message) = match s.short_name.as_str() {
                "tavily_token" => ("Tavily", "搜索", "网页搜索服务，不是独立生成模型"),
                "firecrawl_token" => (
                    "Firecrawl",
                    "网页抓取",
                    "网页抓取与结构化提取，不是独立生成模型",
                ),
                "github_token" => (
                    "GitHub",
                    "代码仓库",
                    "代码仓库服务；配置状态不代表令牌仍有效",
                ),
                "figma_token" => ("Figma", "设计协作", "设计文件服务，不是独立生成模型"),
                "hf_token" => (
                    "Hugging Face",
                    "模型资源",
                    "下载授权不等于已购买云端推理或具备所有模型权限",
                ),
                name if name.starts_with("gdnbit_") => (
                    "GDNbit 项目内部",
                    "内部配置",
                    "项目数据库/签名/内部 API 凭据，不参与生成模型路由",
                ),
                "obsidian_api_key_raw" => ("Obsidian", "知识库", "本机知识库接口，不参与模型路由"),
                _ => (
                    "未归类凭据",
                    "待确认",
                    "未识别服务用途；不根据 Key 外观猜测厂商或接口",
                ),
            };
            services.push(ServiceCapability {
                id: s.short_name.clone(),
                name: s.display_name.unwrap_or_else(|| name.into()),
                function: function.into(),
                credential_ref: format!("sops:{}", s.short_name),
                state: service_configuration_state(
                    s.configured, &s.value_state, s.expires_at.as_deref(),
                    chrono::Utc::now().timestamp(),
                ).into(),
                message: message.into(),
            });
        }
    }
    if let Some(r) = references
        .as_ref()
        .and_then(|r| r.iter().find(|r| r.service_id == "yixiaoer"))
    {
        services.push(ServiceCapability {
            id: "yixiaoer".into(),
            name: "蚁小二".into(),
            function: "内容分发".into(),
            credential_ref: r.credential_ref.clone(),
            state: r.state.into(),
            message: "复用已有安全 CLI 封装；不将其作为模型 Key 或自动发布".into(),
        });
    }
    (credentials, services, issues)
}

/// 只读本机凭据状态和已缓存模型元数据；不请求厂商、不发起生成、不返回 Key 或掩码。
#[tauri::command]
pub async fn model_capabilities_catalog(
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<ModelCapabilityCatalog, String> {
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(200).clamp(1, 200);
    model_catalog_window(offset, Some(limit)).await
}

/// One coherent in-process view for aggregators. Public IPC remains paged.
pub(super) async fn model_capabilities_snapshot() -> Result<ModelCapabilityCatalog, String> {
    model_catalog_window(0, None).await
}

async fn model_catalog_window(
    offset: usize,
    limit: Option<usize>,
) -> Result<ModelCapabilityCatalog, String> {
    // Credentials are evaluated afresh on page zero, including for the aggregator.
    let (credentials, services, mut issues) = if offset == 0 {
        inventory().await
    } else {
        (vec![], vec![], vec![])
    };
    let mut store = cache().lock().map_err(|_| "模型目录不可读取")?;
    if let Some(error) = &store.pin_error {
        issues.push(error.clone());
    }
    let ordered = store.ordered_models();
    let total_models = ordered.len();
    let end = limit.map_or(total_models, |limit| offset.saturating_add(limit));
    let models = ordered.get(offset..end.min(total_models)).unwrap_or_default().to_vec();
    let sources: Vec<ModelCatalogSource> = PROVIDERS
        .iter()
        .map(|p| {
            store
                .sources
                .get(p.0)
                .cloned()
                .unwrap_or(ModelCatalogSource {
                    provider_id: p.0.into(),
                    state: "not_synced".into(),
                    message: match p.0 {
                        "tryallai" => "网关地址与模型目录未登记；不猜测端点",
                        "doubao" => "需要独立语音凭据、应用/资源配置；不借用方舟 Key",
                        "gemini" | "anthropic" => "当前未接入该直连协议；可查看已接入聚合渠道",
                        _ => "当前为官方参考目录，非当前账号模型权限清单；可同步支持的渠道",
                    }
                    .into(),
                    fetched_at: None,
                })
        })
        .collect();
    for s in &sources {
        if s.state == "error" {
            issues.push(format!("{}：{}", s.provider_id, s.message));
        }
    }
    Ok(ModelCapabilityCatalog {
        credentials,
        functions: functions(),
        models,
        services,
        sources,
        issues,
        total_models,
        next_offset: (end < total_models).then_some(end),
    })
}
pub(crate) fn find_route(id: &str) -> Result<ModelCapability, String> {
    let store = cache().lock().map_err(|_| "模型目录不可读取")?;
    if let Some(error) = &store.pin_error {
        return Err(error.clone());
    }
    for rows in store.models.values() {
        if let Some(row) = rows.iter().find(|m| m.route_id == id) {
            return Ok(row.clone());
        }
    }
    let provider = id.split("::").next().unwrap_or("");
    if store.models.contains_key(provider) {
        return Err("模型已不在最近同步的目录；不能用旧模型静默替换".into());
    }
    store
        .pins
        .get(id)
        .cloned()
        .or_else(|| reference_models().iter().find(|m| m.route_id == id).cloned())
        .ok_or("模型路由不在当前目录，请先查询或同步目录".into())
}

/// 显式同步只读厂商模型目录；不会调用生成。失败保留上次成功内容并标记错误。
#[tauri::command]
pub async fn model_capabilities_refresh(
    provider_ids: Option<Vec<String>>,
    credential_source: Option<CredentialSource>,
) -> Result<ModelCapabilityCatalog, String> {
    static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let _guard = SERIAL
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let selected = provider_ids.unwrap_or_else(|| {
        vec![
            "openrouter".into(),
            "openai".into(),
            "fal".into(),
            "elevenlabs".into(),
            "dashscope".into(),
        ]
    });
    if selected.len() > 14
        || selected
            .iter()
            .any(|p| !PROVIDERS.iter().any(|known| known.0 == p))
    {
        return Err("请选择已登记的模型渠道".into());
    }
    let http = client()?;
    for provider in selected {
        let url = match provider.as_str() {
            "openrouter" => "https://openrouter.ai/api/v1/models/user?output_modalities=all",
            "openai" => "https://api.openai.com/v1/models",
            "elevenlabs" => "https://api.elevenlabs.io/v1/models",
            "fal" => "https://api.fal.ai/v1/models?limit=100",
            "dashscope" => "https://dashscope.aliyuncs.com/compatible-mode/v1/models",
            _ => {
                cache()
                    .lock()
                    .map_err(|_| "模型缓存不可读取")?
                    .sources
                    .insert(
                    provider.clone(),
                    ModelCatalogSource {
                        provider_id: provider,
                        state: "manual_reference".into(),
                        message:
                            "该渠道没有已接入的可完整自动枚举接口；保留官方参考，不伪造账号模型权限"
                                .into(),
                        fetched_at: None,
                    },
                );
                continue;
            }
        };
        let fetched: Result<Vec<ModelCapability>, String> = async {
            let key = Some(credential(&provider, credential_source.unwrap_or_default()).await?);
            let mut all = Vec::new();
            let mut cursor: Option<String> = None;
            for page in 0..20 {
                let mut request = http.get(url).timeout(std::time::Duration::from_secs(25));
                if let Some(c) = &cursor {
                    request = request.query(&[("cursor", c)]);
                }
                if let Some(k) = &key {
                    request = authorized(request, &provider, k.as_str());
                }
                let response = request
                    .send()
                    .await
                    .map_err(|_| "模型目录请求失败或超时，未回显认证信息")?;
                let raw: Value = serde_json::from_slice(&bytes(response, 4 * 1024 * 1024).await?)
                    .map_err(|_| "模型目录不是有效 JSON")?;
                all.extend(discover(&provider, &raw, url)?);
                if provider != "fal" || raw["has_more"] != true {
                    break;
                }
                cursor = Some(
                    raw["next_cursor"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .ok_or("模型目录分页游标缺失")?
                        .to_string(),
                );
                if page == 19 {
                    return Err(
                        "模型目录超过本次分页上限；保留上次完整目录，请缩小渠道范围后重试".into(),
                    );
                }
            }
            if all.len() > 2000 {
                return Err("模型目录超过安全上限，保留上次目录".into());
            }
            Ok(all)
        }
        .await;
        let mut store = cache().lock().map_err(|_| "模型缓存不可读取")?;
        match fetched {
            Ok(rows) => {
                let count = rows.len();
                store.models.insert(provider.clone(), rows);
                store.ordered_models = None;
                store.sources.insert(
                    provider.clone(),
                    ModelCatalogSource {
                        provider_id: provider,
                        state: "synced".into(),
                        message: format!(
                            "已读取 {count} 个模型；目录收录不代表生成权限或余额已验证"
                        ),
                        fetched_at: Some(chrono::Utc::now().to_rfc3339()),
                    },
                );
            }
            Err(message) => {
                let previous = store
                    .sources
                    .get(&provider)
                    .and_then(|s| s.fetched_at.clone());
                store.sources.insert(
                    provider.clone(),
                    ModelCatalogSource {
                        provider_id: provider,
                        state: "error".into(),
                        message: format!("{message}；保留上次目录"),
                        fetched_at: previous,
                    },
                );
            }
        }
    }
    model_capabilities_catalog(None, None).await
}

/// 解析功能/模型/渠道与实际密钥引用；只做本机计划，不解出或返回密钥，不发起生成。
#[tauri::command]
pub async fn model_capability_resolve(
    request: ModelCapabilityRequest,
) -> Result<ModelCapabilityPlan, String> {
    let vision = request.input.get("vision").is_some();
    let mut plan = plan_model_request(request).await?;
    if vision {
        plan.request = serde_json::json!({"model":plan.route.model_id,"vision":{"media_type":"image/png","validated":true}});
    }
    Ok(plan)
}

async fn plan_model_request(request: ModelCapabilityRequest) -> Result<ModelCapabilityPlan, String> {
    let route = find_route(&request.route_id)?;
    let (credentials, _, _) = inventory().await;
    let mut credential = credentials
        .into_iter()
        .find(|c| c.provider_id == route.provider_id)
        .ok_or("没有登记该渠道的凭据绑定")?;
    match request.credential_source {
        CredentialSource::Default => (),
        CredentialSource::Sops => {
            let alias = PROVIDERS
                .iter()
                .find(|p| p.0 == route.provider_id)
                .unwrap()
                .2;
            if credential.source != "sops" {
                credential.state = "not_configured".into();
                credential.message = "所选 SOPS 来源未配置；未改用独立凭据".into();
            }
            credential.source = "sops".into();
            credential.credential_ref = format!("sops:{alias}");
        }
        CredentialSource::ApiKeyManager => {
            let ready = service_credentials::check_explicit_reference(&route.provider_id);
            credential.source = "dpapi_reference".into();
            credential.credential_ref = format!("api-key-manager:{}", route.provider_id);
            credential.expires_at = None;
            credential.expiry_warning = false;
            credential.state = if ready.is_ok() {
                "configured"
            } else {
                "blocked"
            }
            .into();
            credential.message = ready.err().unwrap_or_else(|| {
                "已显式选择独立凭据；SOPS 和其他 Key 保持原样，远端权限仍需实测".into()
            });
        }
    }
    let (endpoint, body) = prepare(&route, &request.input)?;
    let callable = credential.state == "configured";
    let message = if callable {
        "密钥引用已匹配；账号模型权限、配额和生成结果仍需实际调用验收"
    } else {
        "所选渠道凭据不可用；不自动切换厂商或其他 Key"
    }
    .into();
    Ok(ModelCapabilityPlan {
        route,
        credential,
        method: "POST".into(),
        endpoint,
        request: body,
        callable,
        message,
    })
}

/// Select desktop streaming support before sending.
/// A started stream can never fall back to another request.
pub(crate) async fn assistant_chat_stream(
    request: crate::assistant_protocol::ModelRequest,
    publish: crate::assistant::TextDelta,
) -> Result<Option<crate::assistant_protocol::ModelReply>, String> {
    let route = find_route(&request.route_id)?;
    let Some(protocol) = crate::model_capability_io::dialogue_stream::Protocol::for_route(&route, &request) else {
        return Ok(None);
    };
    let input = crate::assistant_protocol::body(&route.protocol, &route.provider_id, &request)?;
    let mut plan = model_capability_resolve(ModelCapabilityRequest {
        route_id: request.route_id,
        input,
        output_root: None,
        credential_source: request.credential_source,
    }).await?;
    if plan.route.protocol != route.protocol || plan.route.provider_id != route.provider_id
        || plan.route.model_id != route.model_id
    {
        return Err("模型路由已变化，未发起请求".into());
    }
    if !plan.callable {
        return Err("所选模型渠道的密钥不可用，未发起请求".into());
    }
    let provider = &plan.route.provider_id;
    let key = credential(provider, request.credential_source).await?;
    protocol.enable(provider, &mut plan.request);
    let http = client()?;
    let response = authorized(http.post(&plan.endpoint), provider, key.as_str())
        .json(&plan.request)
        .send()
        .await
        .map_err(crate::model_capability_io::model_transport_error)?;
    protocol.read(response, |text| publish(text)).await.map(Some)
}

/// 执行已绑定模型调用，可能产生厂商费用。凭据仅后端解析，不跨渠道重试。
#[tauri::command]
pub async fn model_capability_call(
    request: ModelCapabilityRequest,
) -> Result<ModelCapabilityResult, String> {
    model_capability_call_bounded(request, None).await
}

pub(crate) async fn model_capability_call_bounded(
    request: ModelCapabilityRequest,
    vision_token_limit: Option<u32>,
) -> Result<ModelCapabilityResult, String> {
    let vision = request.input.get("vision").is_some();
    let mut plan = plan_model_request(ModelCapabilityRequest {
        route_id: request.route_id,
        input: request.input,
        output_root: request.output_root.clone(),
        credential_source: request.credential_source,
    })
    .await?;
    if let Some(limit) = vision_token_limit.filter(|_| vision) {
        plan.request["max_tokens"] = serde_json::json!(limit.clamp(1, 1200));
    }
    if !plan.callable {
        return Err("所选模型渠道的密钥不可用，未发起请求".into());
    }
    if ["speech_openai", "speech_elevenlabs"].contains(&plan.route.protocol.as_str())
        && request.output_root.is_none()
    {
        return Err("请指定 output_root 保存生成音频，未发起收费请求".into());
    }
    // Validate output location before any billable request, without creating files.
    if let Some(root) = request.output_root.as_deref() {
        validate_output_root(root)?;
    }
    let key = credential(&plan.route.provider_id, request.credential_source).await?;
    let http = client()?;
    let mut builder = authorized(
        http.post(&plan.endpoint),
        &plan.route.provider_id,
        key.as_str(),
    )
    .json(&plan.request);
    if plan.route.protocol.starts_with("dashscope_") {
        builder = builder.header("X-DashScope-Async", "enable");
    }
    let allow_retry=!vision && crate::model_capability_io::reliability::text_inference(&plan.route.functions) && crate::model_capability_io::reliability::retry_eligible(&plan.route.provider_id,&plan.route.protocol,&plan.route.route_id,&plan.request,request.output_root.is_some());
    let started=std::time::Instant::now();
    let (response,mut diagnostic)=crate::model_capability_io::reliability::send(builder,allow_retry).await?;
    let result=response_result(
        &plan.route,
        plan.credential.credential_ref,
        response,
        request.output_root.as_deref(),
        ResponseMode::for_submission(&plan.route, &plan.request, request.output_root.as_deref()),
    )
    .await;
    diagnostic.elapsed_ms=started.elapsed().as_millis() as u64;
    let mut result=result.map_err(|error|{
        if let Some(mut failure)=crate::model_capability_io::reliability::Failure::decode(&error){failure.attempts=diagnostic.attempts;failure.elapsed_ms=diagnostic.elapsed_ms;failure.previous=diagnostic.retries.iter().map(crate::model_capability_io::reliability::Attempt::from).collect();failure.encode()}else{error}
    })?;
    if vision && result.status == "succeeded" && !crate::model_capabilities::vision::has_complete_text(&result.output) {
        result.status = "failed".into();
        result.message = "图片理解未返回完整文字结果；保留原回执。".into();
    }
    if result.output.is_object(){result.output["homebase_transport"]=serde_json::json!(diagnostic);}
    Ok(result)
}

/// 查询指定模型渠道的异步任务；不创建/取消/恢复任务，不信任服务端返回的任意查询 URL。
#[tauri::command]
pub async fn model_capability_task(
    route_id: String,
    task_id: String,
    output_root: Option<String>,
    credential_source: Option<CredentialSource>,
) -> Result<ModelCapabilityResult, String> {
    let route = find_route(&route_id)?;
    let (credentials, _, _) = inventory().await;
    let mut binding = credentials
        .into_iter()
        .find(|c| c.provider_id == route.provider_id)
        .ok_or("渠道凭据绑定不存在")?;
    let url = task_endpoint(&route, &task_id, false)?;
    let selected = credential_source.unwrap_or_default();
    match selected {
        CredentialSource::Default => (),
        CredentialSource::ApiKeyManager => {
            binding.credential_ref = format!("api-key-manager:{}", route.provider_id)
        }
        CredentialSource::Sops => {
            binding.credential_ref = format!(
                "sops:{}",
                PROVIDERS
                    .iter()
                    .find(|p| p.0 == route.provider_id)
                    .ok_or("未知渠道")?
                    .2
            )
        }
    }
    let key = credential(&route.provider_id, selected).await?;
    let http = client()?;
    let response = authorized(http.get(url), &route.provider_id, key.as_str())
        .send()
        .await
        .map_err(|_| "任务查询失败或超时；未修改任务或重跑")?;
    let mut result = response_result(
        &route,
        binding.credential_ref.clone(),
        response,
        output_root.as_deref(),
        ResponseMode::Poll,
    )
    .await?;
    if route.protocol == "fal_queue" && result.status == "succeeded" {
        let response = authorized(
            http.get(task_endpoint(&route, &task_id, true)?),
            &route.provider_id,
            key.as_str(),
        )
        .send()
        .await
        .map_err(|_| "fal 结果读取失败；保留原任务编号，不重新提交")?;
        result = response_result(
            &route,
            binding.credential_ref,
            response,
            output_root.as_deref(),
            ResponseMode::Submit,
        )
        .await?;
    }
    result.task_id = Some(task_id);
    Ok(result)
}
