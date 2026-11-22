//! References existing DPAPI files; never imports them into SOPS or grants IPC access.
use super::{
    dpapi,
    sops::{list_top_keys, SecureString, SopsAdapter},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

struct Service {
    id: &'static str,
    name: &'static str,
    file: &'static str,
    sops: &'static str,
    region: Option<&'static str>,
}
const SERVICES: &[Service] = &[
    Service {
        id: "fal",
        name: "fal.ai",
        file: "fal-key.dpapi",
        sops: "fal_key",
        region: None,
    },
    Service {
        id: "dashscope",
        name: "阿里云百炼（北京）",
        file: "dashscope-key.dpapi",
        sops: "dashscope_key",
        region: Some("cn-beijing"),
    },
    Service {
        id: "openrouter",
        name: "OpenRouter",
        file: "openrouter-key.dpapi",
        sops: "openrouter_key",
        region: None,
    },
    Service {
        id: "ark",
        name: "火山方舟 / Seedance",
        file: "ark-key.dpapi",
        sops: "ark_key",
        region: None,
    },
    Service {
        id: "minimax",
        name: "MiniMax 中国区",
        file: "minimax-key.dpapi",
        sops: "minimax_key",
        region: Some("cn"),
    },
    Service {
        id: "yixiaoer",
        name: "蚁小二 CLI",
        file: "api-key.dpapi",
        sops: "",
        region: None,
    },
    Service {
        id: "elevenlabs",
        name: "ElevenLabs 独立凭据",
        file: "elevenlabs-key.dpapi",
        sops: "elevenlabs_key",
        region: None,
    },
    Service {
        id: "doubao",
        name: "豆包语音独立凭据",
        file: "doubao-speech-key.dpapi",
        sops: "doubao_speech_key",
        region: None,
    },
];
const WRAPPER_HASH: &str = "c5cf2d25a456f78ca354e4baf825529f60f39dc959a85afea298234bc6f6d408";
const LAUNCHER_HASH: &str = "30bf1ae4d05ca127cfbb0872ee6094d8c83ec62a447efff3480fb9dfd0a8a335";

struct Layout {
    registry: PathBuf,
    providers: PathBuf,
    yixer: PathBuf,
    launcher: PathBuf,
}
impl Layout {
    fn local() -> Self {
        if let Some(home)=std::env::var_os("HOMEBASE_TEST_HOME") {
            let root=PathBuf::from(home).join(".homebase").join("service-credentials");
            return Self {registry:root.join("registry.json"),providers:root.join("providers"),yixer:root.join("yixiaoer"),launcher:root.join("yxer.cmd")};
        }
        Self {
            registry: r"C:\HomeBase\ServiceCredentials\registry.json".into(),
            providers: r"C:\HomeBase\ServiceCredentials\providers".into(),
            yixer: r"C:\HomeBase\Tools\Yixiaoer".into(),
            launcher: r"C:\HomeBase\Integrations\publishing\yxer.cmd".into(),
        }
    }
    fn file(&self, service: &Service) -> PathBuf {
        if service.id == "yixiaoer" {
            self.yixer.join("secrets").join(service.file)
        } else {
            self.providers.join(service.file)
        }
    }
}
#[derive(Deserialize)]
struct Registry {
    version: u32,
    providers: Vec<Reference>,
}
#[derive(Deserialize)]
struct Reference {
    id: String,
    file: String,
}
#[derive(Serialize)]
pub struct ServiceCredentialStatus {
    pub service_id: &'static str,
    pub name: &'static str,
    pub credential_ref: String,
    pub state: &'static str,
    pub reason: &'static str,
    pub message: &'static str,
    pub region: Option<&'static str>,
    pub backend_route: &'static str,
    pub remote_state: &'static str,
    pub wrapper_cli_version: Option<String>,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, &'static str> {
    let mut input = fs::File::open(path).map_err(|_| "reference_config_unreadable")?;
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(131_073)
        .read_to_end(&mut bytes)
        .map_err(|_| "reference_config_unreadable")?;
    if bytes.len() > 131_072 {
        return Err("reference_config_invalid");
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "reference_config_invalid")?;
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| "reference_config_invalid")
}
fn registered(layout: &Layout, service: &Service) -> Result<(), &'static str> {
    let registry: Registry = read_json(&layout.registry)?;
    if registry.version != 1 {
        return Err("reference_version_unsupported");
    }
    let matches = registry
        .providers
        .iter()
        .filter(|r| r.id == service.id)
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err("reference_missing_or_duplicate");
    }
    let normalized = |p: &str| p.replace('/', "\\").to_ascii_lowercase();
    if normalized(&matches[0].file) != normalized(&layout.file(service).to_string_lossy()) {
        return Err("reference_path_mismatch");
    }
    Ok(())
}
fn region_guard(layout: &Layout, service: &Service) -> Result<(), &'static str> {
    if service.id == "minimax" {
        #[derive(Deserialize)]
        struct Settings {
            #[serde(rename = "MINIMAX_REGION")]
            region: String,
        }
        let settings: Settings = read_json(&layout.providers.join("provider-settings.json"))?;
        if settings.region != "cn" {
            return Err("minimax_cn_region_not_confirmed");
        }
    }
    Ok(())
}
fn external_key(layout: &Layout, service: &Service) -> Result<SecureString, &'static str> {
    registered(layout, service)?;
    let path = layout.file(service);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("not_configured"),
        Err(_) => return Err("credential_unreadable"),
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err("credential_not_regular_file")
        }
        _ => (),
    }
    region_guard(layout, service)?;
    dpapi::read_current_user(&path)
}
fn source_has_sops(vault: &Path, alias: &str) -> Result<bool, &'static str> {
    if alias.is_empty() {
        return Ok(false);
    }
    match fs::metadata(vault) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("existing_sops_source_unreadable"),
        Ok(_) => list_top_keys(vault)
            .map(|keys| keys.iter().any(|k| k == alias))
            .map_err(|_| "existing_sops_source_invalid"),
    }
}
fn wrapper_guard(layout: &Layout) -> Result<String, &'static str> {
    for (path, expected) in [
        (layout.launcher.clone(), LAUNCHER_HASH),
        (layout.yixer.join("run-cli.ps1"), WRAPPER_HASH),
    ] {
        let bytes = fs::read(path).map_err(|_| "secure_wrapper_missing")?;
        if format!("{:x}", Sha256::digest(bytes)) != expected {
            return Err("secure_wrapper_review_required");
        }
    }
    #[derive(Deserialize)]
    struct Package {
        version: String,
    }
    let package: Package = read_json(
        &layout
            .yixer
            .join(r"cli\node_modules\@yixiaoermail\cli\package.json"),
    )?;
    let native = layout
        .yixer
        .join(r"cli\node_modules\@yixiaoermail\cli\bin-native\yxer.exe");
    if !native.is_file() || native.with_extension("exe.old").exists() {
        return Err("wrapper_native_missing_or_recovery_pending");
    }
    if package.version != "3.2.32" {
        return Err("wrapper_cli_version_review_required");
    }
    Ok(package.version)
}
fn inspect(layout: &Layout, vault: &Path) -> Vec<ServiceCredentialStatus> {
    SERVICES
        .iter()
        .map(|service| {
            // Only a boolean/state leaves the backend. Secret is immediately dropped/wiped.
            let local = external_key(layout, service).map(drop);
            let route = source_has_sops(vault, service.sops);
            let wrapper = if service.id == "yixiaoer" {
                wrapper_guard(layout).map(Some)
            } else {
                Ok(None)
            };
            let error = local
                .err()
                .or_else(|| route.as_ref().err().copied())
                .or_else(|| wrapper.as_ref().err().copied());
            ServiceCredentialStatus {
                service_id: service.id,
                name: service.name,
                credential_ref: format!("api-key-manager:{}", service.id),
                state: match error {
                    None => "linked",
                    Some("not_configured") => "not_configured",
                    _ => "blocked",
                },
                reason: error.unwrap_or("local_dpapi_readable_remote_not_checked"),
                message: match error {
                    Some("not_configured") => "独立凭据尚未配置；未借用其他服务密钥",
                    Some("minimax_cn_region_not_confirmed") => {
                        "MiniMax 中国区地域未确认，已阻断；未修改原配置"
                    }
                    Some(_) => "本机凭据引用检查未通过；请按 reason 核对，未输出或改写凭据",
                    None if service.id == "yixiaoer" => {
                        "已关联原命名管道封装；未执行账号请求、发布或远端验证"
                    }
                    None if route == Ok(true) => {
                        "DPAPI 引用本机可解密；现有 SOPS 条目仍优先并受原策略约束，远端有效性未验证"
                    }
                    None => "已关联，本机可解密；远端认证、余额及模型权限未验证",
                },
                region: service.region,
                backend_route: if service.id == "yixiaoer" {
                    "existing_secure_cli_wrapper_only"
                } else if route == Ok(true) {
                    "existing_sops_policy_has_priority"
                } else if route.is_err() {
                    "blocked_existing_sops_source"
                } else {
                    "current_user_dpapi_reference"
                },
                remote_state: "not_checked",
                wrapper_cli_version: wrapper.ok().flatten(),
            }
        })
        .collect()
}

/// Local decryption capability only, no account/API/balance/generation requests.
pub async fn status() -> Result<Vec<ServiceCredentialStatus>, String> {
    tokio::task::spawn_blocking(|| inspect(&Layout::local(), &crate::secret_policy::vault_path()))
        .await
        .map_err(|_| "QUERY_FAILED: 无法取得服务凭据引用状态；未输出凭据正文".into())
}

/// Public reference names only. No decrypt, version probe, network or credential value.
pub fn configured_reference_names() -> Vec<(&'static str,&'static str,&'static str)> {
    let layout=Layout::local();
    SERVICES.iter().filter(|s|s.id!="yixiaoer").filter_map(|s|match fs::metadata(layout.file(s)) {
        Ok(metadata) if metadata.is_file()=>Some((s.id,s.name,"configured_file_only")),
        Ok(_)=>Some((s.id,s.name,"not_a_file")),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>None,
        Err(_)=>Some((s.id,s.name,"unknown")),
    }).collect()
}

/// Backend only: not a Tauri command, not a CLI export and not added to Secrets IPC ACL.
pub async fn resolve_api_key(service_id: &str) -> Result<SecureString, String> {
    resolve_selected_api_key(service_id, false).await
}

/// Explicit DPAPI selection is scoped to this consumer; it never alters SOPS or falls back.
pub async fn resolve_selected_api_key(
    service_id: &str,
    explicit_reference: bool,
) -> Result<SecureString, String> {
    let service = SERVICES
        .iter()
        .find(|s| s.id == service_id)
        .ok_or("未知服务凭据引用")?;
    if service.id == "yixiaoer" {
        return Err("蚁小二必须使用原有安全 CLI 封装，禁止以环境变量/API Key 方式替代".into());
    }
    let layout = Layout::local();
    resolve_with_layout(
        &layout,
        &crate::secret_policy::vault_path(),
        service,
        explicit_reference,
    )
    .await
}
async fn resolve_with_layout(
    layout: &Layout,
    vault: &Path,
    service: &Service,
    explicit_reference: bool,
) -> Result<SecureString, String> {
    region_guard(&layout, service).map_err(|_| "服务地域配置尚未确认，已拒绝提供凭据")?;
    if !explicit_reference
        && source_has_sops(&vault, service.sops)
            .map_err(|_| "现有 SOPS 来源不可读取，未回退其他凭据")?
    {
        // Includes the existing expiry/revocation/type checks. Never bypass a failure.
        let key = SopsAdapter::for_vault(vault)
            .decrypt_key(&vault, service.sops)
            .await
            .map_err(|_| "现有 SOPS 凭据不可用，未回退 DPAPI；请核对其现有策略")?;
        if key.is_empty() {
            return Err("现有 SOPS 凭据为空，未回退其他来源".into());
        }
        return Ok(key);
    }
    external_key(&layout, service).map_err(|_| {
        "本机服务凭据引用不可用，请查询 service_credentials_status 获取非敏感原因".into()
    })
}

/// Fixed no-fee local operation; no caller-supplied shell, arguments, or credentials.
pub fn check_explicit_reference(service_id: &str) -> Result<(), String> {
    let service = SERVICES
        .iter()
        .find(|s| s.id == service_id && s.id != "yixiaoer")
        .ok_or("该渠道没有登记独立凭据引用")?;
    external_key(&Layout::local(), service)
        .map(|_| ())
        .map_err(|_| "所选独立凭据不可用；未回退 SOPS 或其他来源".into())
}

/// Fixed no-fee local operation; no caller-supplied shell, arguments, or credentials.
/// Business/publishing commands are deliberately not exposed by this bridge.
pub fn yixiaoer_version_command() -> Result<tokio::process::Command, String> {
    let layout = Layout::local();
    wrapper_guard(&layout).map_err(|_| "蚁小二封装缺失或变更，需重新审查；未改用明文配置")?;
    let service = SERVICES.iter().find(|s| s.id == "yixiaoer").unwrap();
    registered(&layout, service).map_err(|_| "蚁小二凭据引用无效")?;
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(layout.yixer.join("run-cli.ps1"))
        .arg("--version");
    command
        .env_remove("YIXIAOER_API_KEY")
        .env_remove("YIXIAOER_CONFIG")
        .env_remove("DEBUG")
        .env_remove("NODE_DEBUG");
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    Ok(command)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dpapi_bridge_explicit_selection_never_falls_back_or_overwrites() {
        let (dir, layout) = fixture();
        let service = SERVICES.iter().find(|s| s.id == "openrouter").unwrap();
        let vault = dir.path().join("vault.yaml");
        fs::write(&vault, "openrouter_key: synthetic-invalid-unencrypted\n").unwrap();
        dpapi::write_synthetic(&layout.file(service), "synthetic-independent-reference");
        let before = fs::read(layout.file(service)).unwrap();
        let sops_before = fs::read(&vault).unwrap();
        assert!(resolve_with_layout(&layout, &vault, service, false)
            .await
            .is_err());
        let selected = resolve_with_layout(&layout, &vault, service, true)
            .await
            .unwrap();
        assert_eq!(selected.as_str(), "synthetic-independent-reference");
        assert_eq!(fs::read(layout.file(service)).unwrap(), before);
        assert_eq!(fs::read(&vault).unwrap(), sops_before);
        fs::write(layout.file(service), "broken-synthetic-cipher").unwrap();
        assert!(resolve_with_layout(&layout, &vault, service, true)
            .await
            .is_err());
    }
    fn fixture() -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout {
            registry: dir.path().join("registry.json"),
            providers: dir.path().join("credentials"),
            yixer: dir.path().join("yxer"),
            launcher: dir.path().join("yxer.cmd"),
        };
        fs::create_dir_all(&layout.providers).unwrap();
        fs::create_dir_all(layout.yixer.join("secrets")).unwrap();
        let registry = serde_json::json!({"version":1,"providers":SERVICES.iter().map(|s| serde_json::json!({"id":s.id,"file":layout.file(s)})).collect::<Vec<_>>()});
        fs::write(&layout.registry, serde_json::to_vec(&registry).unwrap()).unwrap();
        fs::write(
            layout.providers.join("provider-settings.json"),
            r#"{"MINIMAX_REGION":"cn"}"#,
        )
        .unwrap();
        (dir, layout)
    }
    #[test]
    fn dpapi_bridge_status_is_safe_missing_is_not_configured_and_sources_are_unchanged() {
        let (dir, layout) = fixture();
        for service in SERVICES
            .iter()
            .filter(|s| !["elevenlabs", "doubao"].contains(&s.id))
        {
            dpapi::write_synthetic(&layout.file(service), "synthetic-only-provider-key");
        }
        let before = fs::read(layout.file(&SERVICES[0])).unwrap();
        let states = inspect(&layout, &dir.path().join("absent-vault.yaml"));
        let serialized = serde_json::to_string(&states).unwrap();
        assert!(!serialized.contains("synthetic-only-provider-key"));
        assert!(!serialized.contains("[redacted]")); // no secret/masked-value field at all
        assert_eq!(
            states.iter().find(|s| s.service_id == "fal").unwrap().state,
            "linked"
        );
        assert_eq!(
            states
                .iter()
                .find(|s| s.service_id == "elevenlabs")
                .unwrap()
                .state,
            "not_configured"
        );
        assert_eq!(
            states
                .iter()
                .find(|s| s.service_id == "doubao")
                .unwrap()
                .state,
            "not_configured"
        );
        assert_eq!(
            states
                .iter()
                .find(|s| s.service_id == "yixiaoer")
                .unwrap()
                .state,
            "blocked"
        );
        assert_eq!(fs::read(layout.file(&SERVICES[0])).unwrap(), before);
        assert!(states.iter().all(|s| s.remote_state == "not_checked"));
    }
    #[test]
    fn dpapi_bridge_region_invalid_registry_and_existing_sops_do_not_silently_fallback() {
        let (dir, layout) = fixture();
        let minimax = SERVICES.iter().find(|s| s.id == "minimax").unwrap();
        dpapi::write_synthetic(&layout.file(minimax), "synthetic-only-key");
        fs::write(
            layout.providers.join("provider-settings.json"),
            r#"{"MINIMAX_REGION":"global"}"#,
        )
        .unwrap();
        assert_eq!(
            external_key(&layout, minimax).unwrap_err(),
            "minimax_cn_region_not_confirmed"
        );
        fs::write(
            &layout.registry,
            r#"{"version":1,"providers":[{"id":"fal","file":"C:\\unrelated.dpapi"}]}"#,
        )
        .unwrap();
        assert_eq!(
            registered(&layout, &SERVICES[0]).unwrap_err(),
            "reference_path_mismatch"
        );
        let vault = dir.path().join("synthetic-vault.yaml");
        fs::write(
            &vault,
            "openrouter_key: ENC[synthetic,type:str]\nsops: {}\n",
        )
        .unwrap();
        assert_eq!(source_has_sops(&vault, "openrouter_key"), Ok(true));
        fs::write(&vault, "invalid: [").unwrap();
        assert!(source_has_sops(&vault, "openrouter_key").is_err());
        assert!(!dir.path().join("absent.dpapi").exists());
    }
    #[test]
    fn dpapi_bridge_reads_replacements_without_copying_or_caching() {
        let (_dir, layout) = fixture();
        let service = &SERVICES[0];
        dpapi::write_synthetic(&layout.file(service), "synthetic-old");
        assert_eq!(
            external_key(&layout, service).unwrap().as_str(),
            "synthetic-old"
        );
        dpapi::write_synthetic(&layout.file(service), "synthetic-new");
        let bytes = fs::read(layout.file(service)).unwrap();
        assert_eq!(
            external_key(&layout, service).unwrap().as_str(),
            "synthetic-new"
        );
        assert_eq!(fs::read(layout.file(service)).unwrap(), bytes);
    }
    #[tokio::test]
    async fn dpapi_bridge_does_not_export_yixiaoer_or_accept_arbitrary_services() {
        assert!(resolve_api_key("yixiaoer")
            .await
            .unwrap_err()
            .contains("安全 CLI 封装"));
        assert!(resolve_api_key("arbitrary-secret-file").await.is_err());
        let (dir, layout) = fixture();
        fs::write(&layout.registry, "invalid synthetic registry").unwrap();
        assert!(inspect(&layout, &dir.path().join("absent-vault"))
            .iter()
            .all(|s| s.state == "blocked"));
        assert_eq!(
            fs::read_to_string(&layout.registry).unwrap(),
            "invalid synthetic registry"
        );
    }
}
