//! Runtime layout and first-run bootstrap for distributable HomeBase builds.

use chrono::Local;
use serde::Serialize;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Manager};
#[path = "../distribution_status.rs"]
mod status;
use status::initialization_record;

const LEGACY_AIHUB_ROOT: &str = r"C:\HomeBase\AIHub";
const LEGACY_MODEL_ROOT: &str = r"C:\HomeBase\Models";
const LEGACY_RUNTIME_ROOT: &str = r"C:\HomeBase\Runtimes";
const LEGACY_OUTPUT_ROOT: &str = r"C:\HomeBase\Wiki\raw\audio";
const LEGACY_CACHE_ROOT: &str = r"C:\HomeBase\Cache\homebase-model-cache";
const LEGACY_PYTHON: &str = r"C:\HomeBase\Tools\Python313\python.exe";
const LEGACY_FFMPEG: &str = r"C:\HomeBase\Runtimes\ffmpeg\bin\ffmpeg.exe";
const LEGACY_SOPS: &str = r"C:\HomeBase\Tools\sops\bin\sops.exe";
const LEGACY_AGE_KEYGEN: &str = r"C:\HomeBase\Tools\age\age-keygen.exe";
const LEGACY_SECRETS: &str = r"C:\HomeBase\Secrets\secrets.yaml";
const LEGACY_SECRETS_META: &str = r"C:\HomeBase\Secrets\secrets-meta.json";
const BOOTSTRAP_MARKER: &str = "homebase_initialized";

#[derive(Debug, Clone)]
pub struct DistributionRuntime {
    pub packaged: bool,
    pub resource_root: PathBuf,
    pub data_root: PathBuf,
    pub aihub_root: PathBuf,
    pub model_root: PathBuf,
    pub runtime_root: PathBuf,
    pub output_root: PathBuf,
    pub cache_root: PathBuf,
    pub python_path: PathBuf,
    pub ffmpeg_path: PathBuf,
    pub sops_path: PathBuf,
    pub age_keygen_path: PathBuf,
    pub reveal_path: PathBuf,
    pub secrets_path: PathBuf,
    pub secrets_meta_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionRoots {
    pub aihub_root: String,
    pub model_root: String,
    pub runtime_root: String,
    pub output_root: String,
    pub cache_root: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DistributionInfo {
    pub schema_version: u32,
    pub package_version: String,
    pub mode: String,
    pub resources_ready: bool,
    pub data_root: String,
    pub resource_root: String,
    pub roots: DistributionRoots,
    pub python_path: String,
    pub ffmpeg_path: String,
    pub reveal_path: String,
    pub secrets_path: String,
    pub allow_system_drive: bool,
    pub initialized_at: Option<String>,
    pub initialization_status: String,
    pub warnings: Vec<String>,
}

fn path_from_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn resolve_file_candidate(
    explicit: Option<&Path>,
    environment: Option<&Path>,
    packaged: &Path,
    legacy: &Path,
) -> PathBuf {
    for candidate in [explicit, environment, Some(packaged), Some(legacy)]
        .into_iter()
        .flatten()
    {
        if candidate.is_file() {
            return candidate.to_path_buf();
        }
    }
    explicit
        .or(environment)
        .unwrap_or(packaged)
        .to_path_buf()
}

fn choose_user_path(legacy: &Path, fallback: &Path) -> PathBuf {
    if legacy.exists() {
        legacy.to_path_buf()
    } else {
        fallback.to_path_buf()
    }
}

fn extract_age_recipient(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let trimmed = line.trim();
        let lowered = trimmed.to_ascii_lowercase();
        let value = if lowered.starts_with("public key:") {
            trimmed.split_once(':').map(|(_, value)| value.trim())
        } else if lowered.starts_with("# public key:") {
            trimmed.split_once(':').map(|(_, value)| value.trim())
        } else {
            None
        }?;
        value.starts_with("age1").then(|| value.to_string())
    })
}

fn path_is_on_system_drive(path: &Path) -> bool {
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    path.to_string_lossy()
        .replace('/', "\\")
        .to_ascii_uppercase()
        .starts_with(&format!("{}\\", system_drive.to_ascii_uppercase()))
}

fn sync_tree(source: &Path, target: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!("随包 AIHub 资源不存在：{}", source.display()));
    }
    fs::create_dir_all(target).map_err(|error| format!("创建 AIHub 数据目录失败：{error}"))?;
    for entry in fs::read_dir(source).map_err(|error| format!("读取随包资源失败：{error}"))? {
        let entry = entry.map_err(|error| format!("读取随包资源项失败：{error}"))?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| format!("读取随包资源类型失败：{error}"))?
            .is_dir()
        {
            sync_tree(&source_path, &target_path)?;
        } else {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("创建随包资源目录失败：{error}"))?;
            }
            fs::copy(&source_path, &target_path)
                .map_err(|error| format!("更新随包资源失败：{error}"))?;
        }
    }
    Ok(())
}

fn set_default_environment(name: &str, value: &Path) {
    if std::env::var_os(name).is_none() {
        std::env::set_var(name, value);
    }
}

fn set_resolved_tool_environment(name: &str, value: &Path) {
    let configured = path_from_env(name);
    if configured.as_deref() != Some(value) && (configured.is_none() || value.is_file()) {
        std::env::set_var(name, value);
    }
}

fn initialize_secrets(runtime: &DistributionRuntime) -> Result<(), String> {
    if runtime.secrets_path.is_file() {
        return Ok(());
    }
    if !runtime.sops_path.is_file() || !runtime.age_keygen_path.is_file() {
        return Err("首次创建密钥库需要随包的 SOPS 和 age-keygen。".to_string());
    }

    let secrets_dir = runtime
        .secrets_path
        .parent()
        .ok_or_else(|| "密钥文件没有父目录。".to_string())?;
    let identity_dir = secrets_dir.join(".age-keys");
    let identity_path = identity_dir.join("primary.txt");
    fs::create_dir_all(&identity_dir).map_err(|error| format!("创建 age 目录失败：{error}"))?;

    if !identity_path.is_file() {
        let output = Command::new(&runtime.age_keygen_path)
            .args(["-o", identity_path.to_string_lossy().as_ref()])
            .output()
            .map_err(|error| format!("启动 age-keygen 失败：{error}"))?;
        if !output.status.success() {
            return Err(format!(
                "生成 age 身份失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }

    let recipient_output = Command::new(&runtime.age_keygen_path)
        .args(["-y", identity_path.to_string_lossy().as_ref()])
        .output()
        .map_err(|error| format!("读取 age 公钥失败：{error}"))?;
    if !recipient_output.status.success() {
        return Err(format!(
            "读取 age 公钥失败：{}",
            String::from_utf8_lossy(&recipient_output.stderr).trim()
        ));
    }
    let recipient = String::from_utf8_lossy(&recipient_output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("age1"))
        .map(str::to_string)
        .or_else(|| extract_age_recipient(&String::from_utf8_lossy(&recipient_output.stderr)))
        .ok_or_else(|| "age-keygen 没有返回可识别的公钥。".to_string())?;

    let temporary = secrets_dir.join(format!(".secrets-bootstrap-{}.yaml", uuid::Uuid::new_v4()));
    fs::write(&temporary, format!("{BOOTSTRAP_MARKER}: \"true\"\n"))
        .map_err(|error| format!("写入密钥库引导文件失败：{error}"))?;
    let encrypted = Command::new(&runtime.sops_path)
        .args([
            "--encrypt",
            "--age",
            &recipient,
            "--input-type",
            "yaml",
            "--output-type",
            "yaml",
            "--output",
            runtime.secrets_path.to_string_lossy().as_ref(),
            temporary.to_string_lossy().as_ref(),
        ])
        .env("SOPS_AGE_KEY_FILE", &identity_path)
        .output()
        .map_err(|error| format!("启动 SOPS 失败：{error}"))?;
    let _ = fs::remove_file(&temporary);
    if !encrypted.status.success() || !runtime.secrets_path.is_file() {
        return Err(format!(
            "创建加密密钥库失败：{}",
            String::from_utf8_lossy(&encrypted.stderr).trim()
        ));
    }
    Ok(())
}

fn resolve_runtime(app: &AppHandle) -> Result<DistributionRuntime, String> {
    let resource_root = app
        .path()
        .resource_dir()
        .map_err(|error| format!("解析应用资源目录失败：{error}"))?
        .join("distribution");
    let packaged = resource_root.join("bundle-manifest.json").is_file();
    let data_root = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("解析应用数据目录失败：{error}"))?
        .join("distribution");
    let documents = dirs::document_dir().unwrap_or_else(|| data_root.clone());

    let aihub_root = path_from_env("HOMEBASE_AIHUB_ROOT").unwrap_or_else(|| {
        if packaged {
            data_root.join("AIHub")
        } else {
            choose_user_path(Path::new(LEGACY_AIHUB_ROOT), &data_root.join("AIHub"))
        }
    });
    let model_root = path_from_env("HOMEBASE_MODEL_ROOT").unwrap_or_else(|| {
        choose_user_path(Path::new(LEGACY_MODEL_ROOT), &data_root.join("Models"))
    });
    let runtime_root = path_from_env("HOMEBASE_RUNTIME_ROOT").unwrap_or_else(|| {
        choose_user_path(Path::new(LEGACY_RUNTIME_ROOT), &data_root.join("Runtimes"))
    });
    let output_root = path_from_env("HOMEBASE_ASR_OUTPUT_ROOT").unwrap_or_else(|| {
        choose_user_path(
            Path::new(LEGACY_OUTPUT_ROOT),
            &documents.join("HomeBase").join("Wiki").join("raw").join("audio"),
        )
    });
    let cache_root = path_from_env("HOMEBASE_MODEL_CACHE").unwrap_or_else(|| {
        choose_user_path(Path::new(LEGACY_CACHE_ROOT), &data_root.join("Cache"))
    });
    let explicit_vault = path_from_env("HOMEBASE_SECRETS_FILE").or_else(|| path_from_env("HOMEBASE_SECRETS_YAML"));
    let custom_vault = explicit_vault.is_some();
    let secrets_path = explicit_vault.unwrap_or_else(|| {
        if Path::new(LEGACY_SECRETS).is_file() {
            PathBuf::from(LEGACY_SECRETS)
        } else {
            data_root.join("Secrets").join("secrets.yaml")
        }
    });
    let secrets_meta_path = path_from_env("HOMEBASE_SECRETS_META").unwrap_or_else(|| {
        if custom_vault {
            secrets_path.with_file_name("secrets-meta.json")
        } else if Path::new(LEGACY_SECRETS_META).is_file() {
            PathBuf::from(LEGACY_SECRETS_META)
        } else {
            data_root.join("Secrets").join("secrets-meta.json")
        }
    });

    Ok(DistributionRuntime {
        packaged,
        python_path: resolve_file_candidate(
            None,
            path_from_env("HOMEBASE_ASR_PYTHON").as_deref(),
            &resource_root.join("runtime").join("python").join("python.exe"),
            Path::new(LEGACY_PYTHON),
        ),
        ffmpeg_path: resolve_file_candidate(
            None,
            path_from_env("HOMEBASE_FFMPEG").as_deref(),
            &resource_root.join("tools").join("ffmpeg.exe"),
            Path::new(LEGACY_FFMPEG),
        ),
        sops_path: resolve_file_candidate(
            None,
            path_from_env("HOMEBASE_SOPS_EXE").as_deref(),
            &resource_root.join("tools").join("sops.exe"),
            Path::new(LEGACY_SOPS),
        ),
        age_keygen_path: resolve_file_candidate(
            None,
            path_from_env("HOMEBASE_AGE_KEYGEN").as_deref(),
            &resource_root.join("tools").join("age-keygen.exe"),
            Path::new(LEGACY_AGE_KEYGEN),
        ),
        reveal_path: crate::reveal_runtime::current()
            .map_err(|error| format!("无法定位密钥助手：{error}"))?,
        resource_root,
        data_root,
        aihub_root,
        model_root,
        runtime_root,
        output_root,
        cache_root,
        secrets_path,
        secrets_meta_path,
    })
}

pub fn bootstrap(app: &AppHandle) -> Result<DistributionRuntime, String> {
    let runtime = resolve_runtime(app)?;
    if runtime.packaged {
        sync_tree(
            &runtime.resource_root.join("aihub"),
            &runtime.aihub_root,
        )?;
    }
    for directory in [
        &runtime.data_root,
        &runtime.model_root,
        &runtime.runtime_root,
        &runtime.output_root,
        &runtime.cache_root,
        &runtime.aihub_root.join("models"),
        &runtime.aihub_root.join("models").join("_jobs"),
    ] {
        fs::create_dir_all(directory)
            .map_err(|error| format!("创建运行目录 {} 失败：{error}", directory.display()))?;
    }

    let manifest_path = runtime.aihub_root.join("models").join("installed-models.json");
    if !manifest_path.is_file() {
        let legacy_manifest = Path::new(LEGACY_AIHUB_ROOT)
            .join("models")
            .join("installed-models.json");
        if runtime.packaged && legacy_manifest.is_file() {
            fs::copy(&legacy_manifest, &manifest_path)
                .map_err(|error| format!("迁移本机模型清单失败：{error}"))?;
        } else {
            let initial = json!({
                "schema_version": 1,
                "updated_at": Local::now().to_rfc3339(),
                "models": {}
            });
            fs::write(
                &manifest_path,
                serde_json::to_vec_pretty(&initial)
                    .map_err(|error| format!("序列化初始模型清单失败：{error}"))?,
            )
            .map_err(|error| format!("创建初始模型清单失败：{error}"))?;
        }
    }

    initialize_secrets(&runtime)?;
    if let Some(parent) = runtime.secrets_meta_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建密钥元数据目录失败：{error}"))?;
    }
    if !runtime.secrets_meta_path.is_file() {
        fs::write(&runtime.secrets_meta_path, b"{}\n")
            .map_err(|error| format!("创建密钥元数据失败：{error}"))?;
    }

    set_default_environment("HOMEBASE_AIHUB_ROOT", &runtime.aihub_root);
    set_default_environment("HOMEBASE_ASR_RUNNER", &asr_runner_path(&runtime));
    set_default_environment("HOMEBASE_ASR_PRESETS", &asr_preset_path(&runtime));
    set_resolved_tool_environment("HOMEBASE_ASR_PYTHON", &runtime.python_path);
    set_default_environment("HOMEBASE_MODEL_PYTHON", &runtime.python_path);
    set_default_environment("HOMEBASE_MODEL_MANAGER", &model_manager_path(&runtime));
    set_default_environment("HOMEBASE_MODEL_CATALOG", &model_catalog_path(&runtime));
    set_default_environment("HOMEBASE_MODEL_MANIFEST", &model_manifest_path(&runtime));
    set_default_environment("HOMEBASE_MODEL_JOBS", &model_jobs_path(&runtime));
    set_default_environment("HOMEBASE_MODEL_ROOT", &runtime.model_root);
    set_default_environment("HOMEBASE_ASR_OUTPUT_ROOT", &runtime.output_root);
    set_default_environment("HOMEBASE_MODEL_CACHE", &runtime.cache_root);
    set_resolved_tool_environment("HOMEBASE_FFMPEG", &runtime.ffmpeg_path);
    set_resolved_tool_environment("HOMEBASE_REVEAL_EXE", &runtime.reveal_path);
    set_resolved_tool_environment("HOMEBASE_SOPS_EXE", &runtime.sops_path);
    set_resolved_tool_environment("HOMEBASE_AGE_KEYGEN", &runtime.age_keygen_path);
    set_default_environment("HOMEBASE_SECRETS_FILE", &runtime.secrets_path);
    set_default_environment("HOMEBASE_SECRETS_YAML", &runtime.secrets_path);
    set_default_environment("HOMEBASE_SECRETS_META", &runtime.secrets_meta_path);
    set_default_environment(
        "HOMEBASE_SOPS_AGE_KEY_FILE",
        &runtime
            .secrets_path
            .parent()
            .unwrap_or(&runtime.data_root)
            .join(".age-keys")
            .join("primary.txt"),
    );
    set_default_environment(
        "SOPS_AGE_KEY_FILE",
        &runtime
            .secrets_path
            .parent()
            .unwrap_or(&runtime.data_root)
            .join(".age-keys")
            .join("primary.txt"),
    );
    set_default_environment(
        "HOMEBASE_FASTER_WHISPER_MODEL",
        &runtime
            .model_root
            .join("asr")
            .join("faster-whisper-large-v3"),
    );

    let state_path = runtime.data_root.join("distribution-state.json");
    let state = json!({
        "schema_version": 1,
        "packaged": runtime.packaged,
        "resource_root": runtime.resource_root,
        "aihub_root": runtime.aihub_root,
        "model_root": runtime.model_root,
        "runtime_root": runtime.runtime_root,
        "output_root": runtime.output_root,
        "cache_root": runtime.cache_root,
        "python_path": runtime.python_path,
        "ffmpeg_path": runtime.ffmpeg_path,
        "secrets_path": runtime.secrets_path,
        "initialized_at": Local::now().to_rfc3339(),
    });
    fs::write(
        &state_path,
        serde_json::to_vec_pretty(&state)
            .map_err(|error| format!("序列化发行状态失败：{error}"))?,
    )
    .map_err(|error| format!("写入发行状态失败：{error}"))?;

    Ok(runtime)
}

pub fn asr_runner_path(runtime: &DistributionRuntime) -> PathBuf {
    runtime
        .aihub_root
        .join("tools")
        .join("local-asr")
        .join("runner")
        .join("asr_task_runner.py")
}

pub fn asr_preset_path(runtime: &DistributionRuntime) -> PathBuf {
    runtime
        .aihub_root
        .join("capabilities")
        .join("asr-presets.json")
}

pub fn model_manager_path(runtime: &DistributionRuntime) -> PathBuf {
    runtime
        .aihub_root
        .join("tools")
        .join("model-manager")
        .join("model_manager.py")
}

pub fn model_catalog_path(runtime: &DistributionRuntime) -> PathBuf {
    runtime
        .aihub_root
        .join("capabilities")
        .join("model-download-catalog.json")
}

pub fn model_manifest_path(runtime: &DistributionRuntime) -> PathBuf {
    runtime
        .aihub_root
        .join("models")
        .join("installed-models.json")
}

pub fn model_jobs_path(runtime: &DistributionRuntime) -> PathBuf {
    runtime.aihub_root.join("models").join("_jobs")
}

pub fn resolve_runtime_file(
    _runtime: &DistributionRuntime,
    explicit: Option<&str>,
    env_name: &str,
    packaged: &Path,
    legacy: &Path,
) -> PathBuf {
    resolve_file_candidate(
        explicit
            .filter(|value| !value.trim().is_empty())
            .map(Path::new),
        path_from_env(env_name).as_deref(),
        packaged,
        legacy,
    )
}

fn runtime_info(app: &AppHandle, runtime: &DistributionRuntime) -> DistributionInfo {
    let required = [
        runtime.python_path.clone(),
        runtime.ffmpeg_path.clone(),
        runtime.sops_path.clone(),
        runtime.age_keygen_path.clone(),
        runtime.reveal_path.clone(),
        asr_runner_path(runtime),
        asr_preset_path(runtime),
        model_manager_path(runtime),
        model_catalog_path(runtime),
        runtime.secrets_path.clone(),
    ];
    let missing = required
        .iter()
        .filter(|path| !path.is_file())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    let (initialized_at, initialization_status) = initialization_record(&runtime.data_root);
    if initialization_status != "known" {
        warnings.push(match initialization_status {
            "missing" => "未找到运行目录初始化记录；初始化时间未知，查询未创建或修复文件。",
            "unreadable" => "运行目录初始化记录读取失败；初始化时间未知，原文件未覆盖。",
            _ => "运行目录初始化记录无效或过大；初始化时间未知，原文件未覆盖。",
        }.to_string());
    }
    if !runtime.packaged {
        warnings.push("当前运行的是开发布局；发行包验收应使用安装版或便携版。".to_string());
    }
    if !missing.is_empty() {
        warnings.push(format!("缺少 {} 个必要运行文件。", missing.len()));
    }
    if path_is_on_system_drive(&runtime.model_root) {
        warnings.push("模型目录位于系统盘；有数据盘时建议在便携配置中改绑。".to_string());
    }

    DistributionInfo {
        schema_version: 1,
        package_version: app.package_info().version.to_string(),
        mode: if runtime.packaged {
            "packaged".to_string()
        } else {
            "development".to_string()
        },
        resources_ready: missing.is_empty(),
        data_root: runtime.data_root.to_string_lossy().into_owned(),
        resource_root: runtime.resource_root.to_string_lossy().into_owned(),
        roots: DistributionRoots {
            aihub_root: runtime.aihub_root.to_string_lossy().into_owned(),
            model_root: runtime.model_root.to_string_lossy().into_owned(),
            runtime_root: runtime.runtime_root.to_string_lossy().into_owned(),
            output_root: runtime.output_root.to_string_lossy().into_owned(),
            cache_root: runtime.cache_root.to_string_lossy().into_owned(),
        },
        python_path: runtime.python_path.to_string_lossy().into_owned(),
        ffmpeg_path: runtime.ffmpeg_path.to_string_lossy().into_owned(),
        reveal_path: runtime.reveal_path.to_string_lossy().into_owned(),
        secrets_path: runtime.secrets_path.to_string_lossy().into_owned(),
        allow_system_drive: path_is_on_system_drive(&runtime.output_root),
        initialized_at,
        initialization_status: initialization_status.to_string(),
        warnings,
    }
}

#[tauri::command]
pub fn distribution_info(app: AppHandle) -> Result<DistributionInfo, String> {
    // Initialization belongs to application startup. A status query must never
    // resync bundled files, generate a vault, or rewrite distribution metadata.
    let runtime = resolve_runtime(&app)?;
    Ok(runtime_info(&app, &runtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_file_replaces_a_missing_machine_specific_path() {
        let temp = tempfile::tempdir().unwrap();
        let packaged = temp.path().join("distribution/runtime/python/python.exe");
        fs::create_dir_all(packaged.parent().unwrap()).unwrap();
        fs::write(&packaged, b"python").unwrap();

        let resolved = resolve_file_candidate(
            Some(Path::new(r"Z:\missing\python.exe")),
            None,
            &packaged,
            Path::new(r"C:\HomeBase\Tools\Python313\python.exe"),
        );

        assert_eq!(resolved, packaged);
    }

    #[test]
    fn an_existing_explicit_file_stays_authoritative() {
        let temp = tempfile::tempdir().unwrap();
        let explicit = temp.path().join("custom-python.exe");
        let packaged = temp.path().join("python.exe");
        fs::write(&explicit, b"custom").unwrap();
        fs::write(&packaged, b"packaged").unwrap();

        let resolved = resolve_file_candidate(
            Some(&explicit),
            None,
            &packaged,
            Path::new(r"D:\legacy\python.exe"),
        );

        assert_eq!(resolved, explicit);
    }

    #[test]
    fn existing_user_data_is_reused_but_a_missing_drive_is_not() {
        let temp = tempfile::tempdir().unwrap();
        let existing = temp.path().join("existing");
        let fallback = temp.path().join("fallback");
        fs::create_dir_all(&existing).unwrap();

        assert_eq!(choose_user_path(&existing, &fallback), existing);
        assert_eq!(
            choose_user_path(&temp.path().join("missing"), &fallback),
            fallback
        );
    }

    #[test]
    fn age_recipient_is_parsed_from_both_supported_outputs() {
        assert_eq!(
            extract_age_recipient("Public key: age1example\r\n").as_deref(),
            Some("age1example")
        );
        assert_eq!(
            extract_age_recipient("# created: now\n# public key: age1comment\nAGE-SECRET-KEY-1ABC\n")
                .as_deref(),
            Some("age1comment")
        );
        assert_eq!(extract_age_recipient("no recipient here"), None);
    }

    #[test]
    fn resource_sync_updates_files_without_removing_user_data() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("capabilities")).unwrap();
        fs::create_dir_all(target.join("models")).unwrap();
        fs::write(source.join("capabilities/catalog.json"), b"v2").unwrap();
        fs::write(target.join("models/installed-models.json"), b"user").unwrap();

        sync_tree(&source, &target).unwrap();

        assert_eq!(fs::read(target.join("capabilities/catalog.json")).unwrap(), b"v2");
        assert_eq!(fs::read(target.join("models/installed-models.json")).unwrap(), b"user");
    }
}
