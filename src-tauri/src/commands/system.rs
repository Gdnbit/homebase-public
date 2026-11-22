//! System 模块命令（v3 §6.6）— Disks / PATH / Processes / Env Vars / Metrics
//!
//! Day 8（已实现）：disks 接 PowerShellAdapter::get_psdrives 真数据
//! 2026-05-03 增量：system_metrics（CPU + RAM 实时） + system_gpu（GPU 利用率，单独命令避免阻塞主流程）
//! 2026-05-23 增量：system_path_list（User/Machine PATH + 死链检测） + system_processes（关键进程一览）

use crate::adapters::powershell::PowerShellAdapter;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, ProcessesToUpdate, System};

const BYTES_PER_GB: f64 = 1_073_741_824.0; // 1024^3

/// Phase G-8 · 启动自检目标路径（与 secrets.rs / agents.rs 保持一致）
const SOPS_EXE_PATH: &str = r"C:\HomeBase\Tools\sops\bin\sops.exe";
const SECRETS_YAML_PATH: &str = r"C:\HomeBase\Secrets\secrets.yaml";
const AGENT_ENV_PS1_PATH: &str = r"C:\HomeBase\Tools\bin\agent-env.ps1";

/// 警告阈值：剩余 < 10 GB 或 使用率 > 85% 时标 warning
const FREE_WARN_GB: f64 = 10.0;
const USAGE_WARN_PCT: f64 = 85.0;

#[derive(Debug, Serialize, Deserialize)]
pub struct DiskInfo {
    pub drive: String, // "C:" | "D:" | "E:" | "F:"
    pub label: Option<String>,
    pub total_gb: f64,
    pub free_gb: f64,
    pub usage_percent: f64,
    pub warning: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub issues: Vec<crate::query::QueryIssue>,
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub hostname: String,
    pub username: String,
    // 以下为供择字段 · 拉不到走 None · 前端容错
    /// CPU 品牌全名 · 如 "13th Gen Intel(R) Core(TM) i9-13900K"
    #[serde(default)]
    pub cpu_brand: Option<String>,
    /// 物理核心数
    #[serde(default)]
    pub cpu_physical_cores: Option<usize>,
    /// 逻辑核心数（线程数）
    #[serde(default)]
    pub cpu_logical_cores: Option<usize>,
    /// 内存总量 GB
    #[serde(default)]
    pub memory_total_gb: Option<f64>,
    /// GPU 名称（过滤虚拟适配器后优先选独显）
    #[serde(default)]
    pub gpu_name: Option<String>,
    pub gpu_status: GpuStatus,
    pub gpu_message: String,
    /// 主板 / 整机制造商
    #[serde(default)]
    pub computer_manufacturer: Option<String>,
    /// 整机型号
    #[serde(default)]
    pub computer_model: Option<String>,
}

/// 列所有 FileSystem PSDrive 的容量（C/D/E/F/...）
#[tauri::command]
pub async fn system_disks() -> Result<Vec<DiskInfo>, String> {
    tracing::debug!("system_disks 调用");
    let ps = PowerShellAdapter::new();
    let drives = ps.get_psdrives().await.map_err(|e| e.to_string())?;
    disk_info_from_drives(drives)
}

fn disk_info_from_drives(
    drives: Vec<crate::adapters::powershell::PsDrive>,
) -> Result<Vec<DiskInfo>, String> {
    Ok(drives
        .into_iter()
        .map(|d| {
            // total/free 任一缺失显式报错，避免把不完整清单说成全部健康。
            let (used, free) = match (d.used, d.free) {
                (Some(u), Some(f)) => (u, f),
                _ => {
                    return Err(
                        "QUERY_INVALID: 磁盘容量缺失，不能省略该磁盘或当作零容量".to_string()
                    )
                }
            };
            let total_bytes = used
                .checked_add(free)
                .ok_or("QUERY_INVALID: 磁盘容量溢出")?;
            if total_bytes == 0 {
                return Err("QUERY_INVALID: 磁盘总容量为零，无法判定容量状态".to_string());
            }
            let total_gb = (total_bytes as f64) / BYTES_PER_GB;
            let free_gb = (free as f64) / BYTES_PER_GB;
            let used_gb = (used as f64) / BYTES_PER_GB;
            let usage_percent = (used_gb / total_gb) * 100.0;
            let warning = free_gb < FREE_WARN_GB || usage_percent > USAGE_WARN_PCT;

            Ok(DiskInfo {
                drive: format!("{}:", d.name.to_uppercase()),
                label: None, // PSDrive 本身不带 VolumeLabel；Phase 13.2 用 Get-Volume 补
                total_gb,
                free_gb,
                usage_percent,
                warning,
            })
        })
        .collect::<Result<Vec<_>, String>>()?)
}

/// 系统基础信息 · 含 CPU / 内存 / GPU / 主板 · 为概览页提供一闪看全的机型资料
#[tauri::command]
pub async fn system_info() -> Result<SystemInfo, String> {
    // === sysinfo 部分 · 本进程内快 · 不会失败 ===
    let mut sys = System::new();
    sys.refresh_cpu_specifics(CpuRefreshKind::new());
    sys.refresh_memory_specifics(MemoryRefreshKind::new().with_ram());

    let cpus = sys.cpus();
    let cpu_brand = cpus
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|s| !s.is_empty());
    let cpu_logical_cores = if cpus.is_empty() {
        None
    } else {
        Some(cpus.len())
    };
    let cpu_physical_cores = sys.physical_core_count();
    let memory_total_bytes = sys.total_memory();
    let memory_total_gb = if memory_total_bytes == 0 {
        None
    } else {
        Some(memory_total_bytes as f64 / BYTES_PER_GB)
    };

    // === PowerShell 部分 · GPU + 主板 · 并发 · 任一失败不报错 ===
    let ps = PowerShellAdapter::new();
    let (gpu_res, cs_res) = tokio::join!(fetch_gpu_info(&ps), fetch_computer_system(&ps),);
    let (gpu_status, gpu_message) = gpu_probe_status(&gpu_res);
    let gpu_name = gpu_res.ok().flatten();
    let mut issues = Vec::new();
    if cs_res.is_err() {
        issues.push(crate::query::issue(
            "computer",
            "整机信息未知：查询失败或结果无效",
        ));
    }
    let (computer_manufacturer, computer_model) = cs_res.unwrap_or((None, None));
    if computer_manufacturer.is_none()
        && computer_model.is_none()
        && !issues.iter().any(|i| i.field == "computer")
    {
        issues.push(crate::query::issue(
            "computer",
            "整机信息未知：系统未提供厂商和型号",
        ));
    }
    if cpu_brand.is_none() {
        issues.push(crate::query::issue(
            "cpu_brand",
            "处理器型号未知：系统未提供名称",
        ));
    }
    if cpu_logical_cores.is_none() {
        issues.push(crate::query::issue(
            "cpu_cores",
            "处理器核心数未知：采样未返回数据",
        ));
    }
    if memory_total_gb.is_none() {
        issues.push(crate::query::issue(
            "memory",
            "内存总量未知：采样未返回数据",
        ));
    }
    if cpu_physical_cores.is_none() {
        issues.push(crate::query::issue(
            "cpu_physical_cores",
            "物理核心数未知：系统未提供数据",
        ));
    }
    if computer_manufacturer.is_none() && computer_model.is_some() {
        issues.push(crate::query::issue(
            "computer_manufacturer",
            "整机厂商未知：系统未提供数据",
        ));
    }
    if computer_model.is_none() && computer_manufacturer.is_some() {
        issues.push(crate::query::issue(
            "computer_model",
            "整机型号未知：系统未提供数据",
        ));
    }
    if gpu_status == GpuStatus::Unknown {
        issues.push(crate::query::issue("gpu", &gpu_message));
    }
    let os_version = System::long_os_version()
        .or_else(System::os_version)
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            issues.push(crate::query::issue(
                "os_version",
                "系统版本未知：系统未返回版本",
            ));
            "未知".into()
        });
    let hostname = System::host_name()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            issues.push(crate::query::issue(
                "hostname",
                "主机名未知：系统未返回名称",
            ));
            "未知".into()
        });
    let username = std::env::var("USERNAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            issues.push(crate::query::issue(
                "username",
                "用户名未知：环境未提供名称",
            ));
            "未知".into()
        });

    Ok(SystemInfo {
        issues,
        os: std::env::consts::OS.to_string(),
        os_version,
        arch: std::env::consts::ARCH.to_string(),
        hostname,
        username,
        cpu_brand,
        cpu_physical_cores,
        cpu_logical_cores,
        memory_total_gb,
        gpu_name,
        gpu_status,
        gpu_message,
        computer_manufacturer,
        computer_model,
    })
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GpuStatus {
    Detected,
    NotDetected,
    Unknown,
}

fn gpu_probe_status(result: &Result<Option<String>, &'static str>) -> (GpuStatus, String) {
    match result {
        Ok(Some(_)) => (GpuStatus::Detected, "已识别物理 GPU".to_string()),
        Ok(None) => (
            GpuStatus::NotDetected,
            "未检测到物理 GPU（可能没有显卡或仅有虚拟显示适配器）".to_string(),
        ),
        Err(reason) => (GpuStatus::Unknown, format!("GPU 信息未知：{reason}")),
    }
}

// PowerShellAdapter 固定使用 Windows PowerShell 5.1，不支持 ConvertTo-Json -AsArray。
// 使用 -InputObject 保留零/单/多项数组；Stop 防止查询失败被误报为未检测到。
const GPU_INFO_SCRIPT: &str = "$gpus = @(Get-CimInstance Win32_VideoController -ErrorAction Stop | Select-Object Name, AdapterRAM, AdapterCompatibility, PNPDeviceID); ConvertTo-Json -InputObject $gpus -Compress";

#[derive(Debug, Deserialize, Clone)]
struct GpuRecord {
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "AdapterRAM", default)]
    adapter_ram: Option<i64>,
    #[serde(rename = "AdapterCompatibility", default)]
    adapter_compatibility: Option<String>,
    #[serde(rename = "PNPDeviceID", default)]
    pnp_device_id: Option<String>,
}

fn is_virtual_gpu(record: &GpuRecord) -> bool {
    let identity = format!(
        "{} {} {}",
        record.name.as_deref().unwrap_or_default(),
        record.adapter_compatibility.as_deref().unwrap_or_default(),
        record.pnp_device_id.as_deref().unwrap_or_default(),
    )
    .to_lowercase();
    let virtual_markers = [
        "virtual",
        "remote display",
        "basic display",
        "basic render",
        "microsoft remote",
        "parsec",
        "oray",
        "iddriver",
        "indirect display",
    ];
    virtual_markers
        .iter()
        .any(|marker| identity.contains(marker))
        || record
            .pnp_device_id
            .as_deref()
            .map(|id| id.to_ascii_uppercase().starts_with("ROOT\\DISPLAY"))
            .unwrap_or(false)
}

fn gpu_rank(record: &GpuRecord) -> (i32, i64) {
    let identity = format!(
        "{} {}",
        record.name.as_deref().unwrap_or_default(),
        record.adapter_compatibility.as_deref().unwrap_or_default(),
    )
    .to_lowercase();
    let vendor_rank = if identity.contains("nvidia")
        || identity.contains("geforce")
        || identity.contains("quadro")
        || identity.contains("tesla")
    {
        500
    } else if identity.contains("amd") || identity.contains("radeon") {
        400
    } else if identity.contains("intel") && identity.contains("arc") {
        350
    } else if identity.contains("intel") {
        200
    } else {
        100
    };
    (vendor_rank, record.adapter_ram.unwrap_or(0).max(0))
}

fn physical_gpu_records(records: &[GpuRecord]) -> Vec<&GpuRecord> {
    records
        .iter()
        .filter(|record| {
            record
                .name
                .as_deref()
                .map(|name| !name.trim().is_empty())
                .unwrap_or(false)
                && !is_virtual_gpu(record)
        })
        .collect()
}

fn select_best_gpu(records: &[GpuRecord]) -> Option<&GpuRecord> {
    physical_gpu_records(records)
        .into_iter()
        .max_by_key(|record| gpu_rank(record))
}

fn adapter_memory_gb(record: &GpuRecord) -> Option<f64> {
    record
        .adapter_ram
        .filter(|bytes| *bytes > 0)
        .map(|bytes| bytes as f64 / BYTES_PER_GB)
}

/// GPU 名称 · Win32_VideoController · 排除虚拟显示器后优先选物理独显
async fn fetch_gpu_info(ps: &PowerShellAdapter) -> Result<Option<String>, &'static str> {
    let out = ps
        .run(GPU_INFO_SCRIPT)
        .await
        .map_err(|_| "系统显卡查询失败或超时，请重试")?;
    gpu_name_from_json(&out.stdout)
}

fn gpu_name_from_json(output: &str) -> Result<Option<String>, &'static str> {
    let trimmed = output.trim().trim_start_matches('\u{feff}');
    let records: Vec<GpuRecord> = serde_json::from_str::<Vec<GpuRecord>>(trimmed)
        .or_else(|_| serde_json::from_str::<GpuRecord>(trimmed).map(|r| vec![r]))
        .map_err(|_| "系统显卡查询结果格式无效")?;
    if records.iter().any(|record| {
        record
            .name
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
    }) {
        return Err("系统显卡查询结果缺少名称");
    }
    Ok(select_best_gpu(&records).and_then(|record| {
        record
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }))
}

/// 主板 / 整机制造商与型号 · Win32_ComputerSystem
async fn fetch_computer_system(
    ps: &PowerShellAdapter,
) -> Result<(Option<String>, Option<String>), ()> {
    #[derive(Deserialize)]
    struct CsRecord {
        #[serde(rename = "Manufacturer", default)]
        manufacturer: Option<String>,
        #[serde(rename = "Model", default)]
        model: Option<String>,
    }
    let script = "Get-CimInstance Win32_ComputerSystem -ErrorAction Stop | Select-Object Manufacturer, Model | ConvertTo-Json -Compress";
    let out = ps.run(script).await.map_err(|_| ())?;
    let trimmed = out.stdout.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    let r: CsRecord = serde_json::from_str(trimmed).map_err(|_| ())?;
    let normalize = |s: Option<String>| s.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    Ok((normalize(r.manufacturer), normalize(r.model)))
}

// =====================================================================
// system_metrics — CPU + RAM 实时（Dashboard 看板用）
// =====================================================================

/// 系统资源实时指标（CPU + RAM；GPU 单独命令）
#[derive(Debug, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu_percent: f64, // 0..=100，所有逻辑核心平均
    pub cpu_cores: usize, // 逻辑核心数
    pub memory_used_gb: f64,
    pub memory_total_gb: f64,
    pub memory_percent: f64,  // 0..=100
    pub cpu_warning: bool,    // > 85% 标 warning
    pub memory_warning: bool, // > 85% 标 warning
}

const METRIC_WARN_PCT: f64 = 85.0;

/// CPU + RAM 实时指标
///
/// 注意：sysinfo 的 CPU 利用率必须 refresh 两次（间隔 ~200ms）才有效。
/// 第一次 refresh 拿基线，sleep 后第二次 refresh 计算差值。
#[tauri::command]
pub async fn system_metrics() -> Result<SystemMetrics, String> {
    tracing::debug!("system_metrics 调用");

    let mut sys = System::new();
    // 第一次 refresh：拿基线
    sys.refresh_cpu_usage();
    // sysinfo 文档要求等 ~200ms 让计算窗口形成
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_cpu_usage();
    sys.refresh_memory_specifics(MemoryRefreshKind::new().with_ram());

    let cpus = sys.cpus();
    let cpu_cores = cpus.len();
    let cpu_percent = if cpu_cores == 0 {
        0.0
    } else {
        let total: f32 = cpus.iter().map(|c| c.cpu_usage()).sum();
        (total / cpu_cores as f32) as f64
    };

    let memory_used_bytes = sys.used_memory();
    let memory_total_bytes = sys.total_memory();
    crate::query::validate_metrics(cpu_cores, memory_used_bytes, memory_total_bytes)?;
    if !cpu_percent.is_finite() || !(0.0..=100.0).contains(&cpu_percent) {
        return Err("QUERY_INVALID: CPU 采样无效，不能以零值代替".into());
    }
    let memory_used_gb = memory_used_bytes as f64 / BYTES_PER_GB;
    let memory_total_gb = memory_total_bytes as f64 / BYTES_PER_GB;
    let memory_percent = if memory_total_bytes == 0 {
        0.0
    } else {
        (memory_used_bytes as f64 / memory_total_bytes as f64) * 100.0
    };

    Ok(SystemMetrics {
        cpu_percent,
        cpu_cores,
        memory_used_gb,
        memory_total_gb,
        memory_percent,
        cpu_warning: cpu_percent > METRIC_WARN_PCT,
        memory_warning: memory_percent > METRIC_WARN_PCT,
    })
}

// =====================================================================
// system_gpu — GPU 利用率（单独命令，避免拖慢 system_metrics）
// =====================================================================

/// GPU 实时利用率（Windows perf counter）
///
/// 实现注意：
/// - Windows 11 上 `Get-Counter '\GPU Engine(*engtype_3D)\Utilization Percentage'` 是最通用的途径
///   （NVIDIA / AMD / Intel 集显都有），不依赖 nvidia-smi。
/// - perf counter 启动慢（首次 ~1.5s），所以独立命令、独立轮询，不阻塞主 dashboard。
/// - 多个 GPU 进程引擎累加 → total（可能 >100，前端裁剪到 100）。
#[derive(Debug, Serialize, Deserialize)]
pub struct GpuMetrics {
    pub available: bool,          // false = 拉取失败 / 无 GPU
    pub gpu_percent: Option<f64>, // 0..=100，None = 不可用
    pub gpu_name: Option<String>, // 如 "NVIDIA GeForce RTX 4060"
    pub gpu_status: GpuStatus,
    pub gpu_message: String,
    pub gpu_names: Vec<String>, // 已过滤虚拟显示器的物理 GPU 列表
    pub dedicated_memory_gb: Option<f64>, // 适配器报告的专用显存；未知时为 None
    pub gpu_warning: bool,      // > 85% 标 warning
    pub error: Option<String>,  // 拉取失败原因（提示用户）
}

#[tauri::command]
pub async fn system_gpu() -> Result<GpuMetrics, String> {
    tracing::debug!("system_gpu 调用");
    let ps = PowerShellAdapter::new();

    // 一个 PowerShell 调用拿 GPU 名字 + 利用率（合并避免两次启动 PS）
    // -ErrorAction SilentlyContinue：没装 GPU / counter 不可用时返回 null 而不是抛
    let script = r#"
$counters = (Get-Counter '\GPU Engine(*engtype_3D)\Utilization Percentage' -ErrorAction SilentlyContinue).CounterSamples
if ($counters) {
    $sum = ($counters | Measure-Object -Property CookedValue -Sum).Sum
    if ($null -eq $sum) { $percent = 0.0 } else { $percent = [Math]::Min(100.0, [double]$sum) }
} else {
    $percent = $null
}
$gpus = @(Get-CimInstance Win32_VideoController -ErrorAction Stop | Select-Object Name, AdapterRAM, AdapterCompatibility, PNPDeviceID)
[PSCustomObject]@{ percent = $percent; gpus = $gpus } | ConvertTo-Json -Depth 4 -Compress
"#;

    match ps.run_json::<GpuRaw>(script).await {
        Ok(raw) => {
            let percent = raw.percent;
            let physical = physical_gpu_records(&raw.gpus);
            let gpu_names = physical
                .iter()
                .filter_map(|record| record.name.as_deref().map(str::trim))
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let best = select_best_gpu(&raw.gpus);
            let gpu_name = best
                .and_then(|record| record.name.as_deref())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string);
            let dedicated_memory_gb = best.and_then(|record| adapter_memory_gb(record));
            let available = gpu_name.is_some() || percent.is_some();
            let (gpu_status, gpu_message) = gpu_probe_status(&Ok(gpu_name.clone()));
            Ok(GpuMetrics {
                available,
                gpu_percent: percent,
                gpu_name,
                gpu_status,
                gpu_message,
                gpu_names,
                dedicated_memory_gb,
                gpu_warning: percent.map(|p| p > METRIC_WARN_PCT).unwrap_or(false),
                error: if !available {
                    Some("未检测到可用于本地计算的物理 GPU，或 GPU 探测不可用".to_string())
                } else if percent.is_none() {
                    Some("已识别物理 GPU，但实时利用率计数器不可用".to_string())
                } else {
                    None
                },
            })
        }
        Err(_) => Ok(GpuMetrics {
            available: false,
            gpu_percent: None,
            gpu_name: None,
            gpu_status: GpuStatus::Unknown,
            gpu_message: "GPU 信息未知：系统显卡查询失败、超时或结果格式无效".to_string(),
            gpu_names: Vec::new(),
            dedicated_memory_gb: None,
            gpu_warning: false,
            error: Some("GPU 信息未知：系统显卡查询失败、超时或结果格式无效".to_string()),
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GpuRaw {
    percent: Option<f64>,
    #[serde(default)]
    gpus: Vec<GpuRecord>,
}

// =====================================================================
// Phase G-8 · system_preflight — 启动时 SOPS 五件套自检
//
// 目的：用户/分享对象缺三件套（sops / secrets.yaml / age key）时，
// 不要让错误信息停留在 "command not found"，而是给一个顶部横幅
// 明确指引修复。
//
// 五项：
// 1. sops_exe      C:\HomeBase\Tools\sops\bin\sops.exe 存在 + --version 可跑 (critical)
// 2. secrets_yaml  C:\HomeBase\Secrets\secrets.yaml 存在                   (critical)
// 3. age_key       SOPS_AGE_KEY_FILE / SOPS_AGE_KEY / 默认路径可用 (critical)
// 4. reveal_exe    HomeBase 能定位到实际使用的密钥助手           (critical)
// 5. agent_env     C:\HomeBase\Tools\bin\agent-env.ps1 存在                  (warning, 仅 wrapper agent 用)
// =====================================================================

#[derive(Debug, Serialize, Deserialize)]
pub struct PreflightCheck {
    /// 稳定 ID（前端做映射用）
    pub id: String,
    /// 人类可读中文标签
    pub label: String,
    /// 是否通过
    pub ok: bool,
    /// "critical"（影响核心功能）/ "warning"（可选/降级可用）
    pub severity: String,
    /// 成功时可带补充（如版本号、路径）
    pub detail: Option<String>,
    /// 失败时的修复建议
    pub hint: Option<String>,
    #[serde(default)]
    pub retryable: bool,
}

impl PreflightCheck {
    fn ok(id: &str, label: &str, severity: &str, detail: Option<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            ok: true,
            severity: severity.into(),
            detail,
            hint: None,
            retryable: false,
        }
    }

    fn fail(id: &str, label: &str, severity: &str, hint: String) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            ok: false,
            severity: severity.into(),
            detail: None,
            hint: Some(hint),
            retryable: false,
        }
    }
}

/// 1. sops.exe 存在 + 能跑 `--version`
async fn check_sops_exe() -> PreflightCheck {
    let configured = std::env::var("HOMEBASE_SOPS_EXE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| SOPS_EXE_PATH.to_string());
    let p = Path::new(&configured);
    if !p.is_file() {
        return PreflightCheck::fail(
            "sops_exe",
            "SOPS 可执行文件",
            "critical",
            format!("{configured} 不存在 — 请重新运行 HomeBase 安装器或修复运行时目录"),
        );
    }
    match crate::adapters::sops_probe::probe(p, Default::default()).await {
        Ok(result) => PreflightCheck::ok(
            "sops_exe",
            "SOPS 可执行文件",
            "critical",
            Some(format!(
                "sops {} · {} 次探测 · {} ms{}",
                result.version,
                result.attempts,
                result.elapsed_ms,
                if result.attempts > 1 {
                    " · 首次启动超时后已恢复"
                } else {
                    ""
                }
            )),
        ),
        Err(error) => {
            let mut check =
                PreflightCheck::fail("sops_exe", "SOPS 可执行文件", "critical", error.message());
            check.retryable = error.retryable();
            check
        }
    }
}

/// 2. secrets.yaml 存在（单源真相）
fn check_secrets_yaml() -> PreflightCheck {
    let configured = std::env::var("HOMEBASE_SECRETS_FILE")
        .or_else(|_| std::env::var("HOMEBASE_SECRETS_YAML"))
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| SECRETS_YAML_PATH.to_string());
    let p = Path::new(&configured);
    if p.exists() {
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        PreflightCheck::ok(
            "secrets_yaml",
            "密钥单源文件",
            "critical",
            Some(format!("{configured}（{size} bytes）")),
        )
    } else {
        PreflightCheck::fail(
            "secrets_yaml",
            "密钥单源文件",
            "critical",
            format!("{configured} 找不到 — 请重新启动 HomeBase 以执行首次运行引导"),
        )
    }
}

/// 3. age 私钥可用（SOPS_AGE_KEY_FILE / SOPS_AGE_KEY / 默认位置）
fn check_age_key() -> PreflightCheck {
    if let Ok(file_path) = std::env::var("SOPS_AGE_KEY_FILE") {
        let p = Path::new(&file_path);
        if p.exists() {
            return PreflightCheck::ok(
                "age_key",
                "age 私钥",
                "critical",
                Some(format!("SOPS_AGE_KEY_FILE={file_path}")),
            );
        }
        return PreflightCheck::fail(
            "age_key",
            "age 私钥",
            "critical",
            format!("SOPS_AGE_KEY_FILE 指向的文件不存在：{file_path}"),
        );
    }
    if let Ok(key) = std::env::var("SOPS_AGE_KEY") {
        if !key.trim().is_empty() {
            return PreflightCheck::ok(
                "age_key",
                "age 私钥",
                "critical",
                Some("SOPS_AGE_KEY（env 内联）".into()),
            );
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let default = PathBuf::from(appdata)
            .join("sops")
            .join("age")
            .join("keys.txt");
        if default.exists() {
            return PreflightCheck::ok(
                "age_key",
                "age 私钥",
                "critical",
                Some(default.to_string_lossy().into_owned()),
            );
        }
    }
    PreflightCheck::fail(
        "age_key",
        "age 私钥",
        "critical",
        "未检测到 age 私钥 — 请设置环境变量 SOPS_AGE_KEY_FILE 指向 keys.txt".into(),
    )
}

/// 4. 使用与运行环境和代理一致的密钥助手查找规则。
fn check_reveal_exe() -> PreflightCheck {
    let candidate = match crate::reveal_runtime::current() {
        Ok(p) => p,
        Err(e) => {
            return PreflightCheck::fail(
                "reveal_exe",
                "homebase-reveal helper",
                "critical",
                format!("无法定位 HomeBase 密钥助手：{e}"),
            );
        }
    };
    if candidate.is_file() {
        PreflightCheck::ok(
            "reveal_exe",
            "homebase-reveal helper",
            "critical",
            Some(candidate.to_string_lossy().into_owned()),
        )
    } else {
        PreflightCheck::fail(
            "reveal_exe",
            "homebase-reveal helper",
            "critical",
            format!(
                "找不到随包密钥助手，请检查 HomeBase 运行时。检查路径：{}",
                candidate.display()
            ),
        )
    }
}

/// 5. agent-env.ps1 存在（Gemini/OpenClaw/Hermes 的 wrapper exec 用）
fn check_agent_env_ps1() -> PreflightCheck {
    let p = Path::new(AGENT_ENV_PS1_PATH);
    if p.exists() {
        PreflightCheck::ok(
            "agent_env",
            "agent-env.ps1 wrapper",
            "warning",
            Some(AGENT_ENV_PS1_PATH.into()),
        )
    } else {
        PreflightCheck::fail(
            "agent_env",
            "agent-env.ps1 wrapper",
            "warning",
            format!(
                "{AGENT_ENV_PS1_PATH} 不存在 — Gemini CLI / OpenClaw / Hermes 的 SOPS 注入将不可用（Claude Code / Codex 不受影响）"
            ),
        )
    }
}

/// 启动自检 · 返回 5 项状态（不 short-circuit，每项独立返回）
///
/// 前端在 App 启动时调一次，把失败项渲染成顶部横幅。
/// 本命令不抛错（永远返回 Ok），让前端根据 `ok` + `severity` 自行分类展示。
#[tauri::command]
pub async fn system_preflight() -> Result<Vec<PreflightCheck>, String> {
    tracing::debug!("system_preflight 调用");
    let mut out = Vec::with_capacity(5);
    out.push(check_sops_exe().await);
    out.push(check_secrets_yaml());
    out.push(check_age_key());
    out.push(check_reveal_exe());
    out.push(check_agent_env_ps1());
    Ok(out)
}

// =====================================================================
// PATH 项目列表 + 死链检测（2026-05-23）
//
// 区分用户级 (HKCU\Environment\Path) 与系统级 (HKLM\...\Environment\Path)
// 两个来源 — `$env:PATH` 是合并值无法区分。展开 `%VAR%` 后做 Directory.Exists 判定。
// =====================================================================

/// 单个 PATH 条目
#[derive(Debug, Serialize, Deserialize)]
pub struct PathEntry {
    /// 展开后的绝对路径（前端显示这个）
    pub path: String,
    /// 原始值（含 `%VAR%` 占位符；与展开后不同时显示）
    pub raw: String,
    /// 该路径是否真实存在（false = 死链）
    pub exists: bool,
    /// 来源："user"（HKCU）或 "machine"（HKLM）
    pub source: String,
}

#[derive(Debug, Deserialize)]
struct PsPathItem {
    raw: String,
    expanded: String,
    exists: bool,
}

#[derive(Debug, Deserialize)]
struct PsPathListRaw {
    user: Vec<PsPathItem>,
    machine: Vec<PsPathItem>,
}

/// 列出 User + Machine 两级 PATH 项目，逐项做死链检测
#[tauri::command]
pub async fn system_path_list() -> Result<Vec<PathEntry>, String> {
    tracing::debug!("system_path_list 调用");
    let ps = PowerShellAdapter::new();
    // 用 [Environment]::GetEnvironmentVariable(name, scope) 区分来源；
    // 用 ExpandEnvironmentVariables 展开 %VAR%；
    // 用 Directory.Exists 做存在性检查（比 Test-Path 快 · 不会触发 PSProvider）
    let script = r#"
        $user = [Environment]::GetEnvironmentVariable('Path', 'User')
        $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
        function Process-Paths {
            param($raw)
            if (-not $raw) { return @() }
            @($raw.Split(';') | ForEach-Object { $_.Trim('" ') } | Where-Object { $_ } | ForEach-Object {
                $expanded = [Environment]::ExpandEnvironmentVariables($_)
                [pscustomobject]@{
                    raw = $_
                    expanded = $expanded
                    exists = [System.IO.Directory]::Exists($expanded)
                }
            })
        }
        @{
            user = @(Process-Paths $user)
            machine = @(Process-Paths $machine)
        } | ConvertTo-Json -Depth 5 -Compress
    "#;
    let raw: PsPathListRaw = ps
        .run_json(script)
        .await
        .map_err(|e| format!("读取 PATH 失败: {e}"))?;

    let mut entries = Vec::with_capacity(raw.user.len() + raw.machine.len());
    for item in raw.user {
        entries.push(PathEntry {
            path: item.expanded,
            raw: item.raw,
            exists: item.exists,
            source: "user".into(),
        });
    }
    for item in raw.machine {
        entries.push(PathEntry {
            path: item.expanded,
            raw: item.raw,
            exists: item.exists,
            source: "machine".into(),
        });
    }
    Ok(entries)
}

// =====================================================================
// 关键进程一览（2026-05-23）
//
// 用 sysinfo 拿当前进程列表，按子串白名单筛出：
// - HomeBase 自身
// - 5 个内置 agent（Claude Code / Codex / Gemini CLI / OpenClaw / Hermes）
// - sops 解密 helper
// =====================================================================

/// 单个进程信息
#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// 进程 ID
    pub pid: u32,
    /// 真实进程名（如 "claude.exe"）
    pub name: String,
    /// 友好显示名（如 "Claude Code"）
    pub display_name: String,
    /// 类别："homebase" / "agent" / "tool"
    pub category: String,
    /// CPU 占用百分比（0-100 · 单 200ms 采样窗口）
    pub cpu_percent: f32,
    /// 内存占用 MB
    pub memory_mb: f64,
    /// 进程已运行秒数
    pub run_secs: u64,
}

/// 关键进程白名单：(进程名子串小写, 友好名, 类别)
///
/// 顺序无关；命中第一个匹配即停。子串需足够特征化避免误伤
/// （e.g. "claude" 不会匹配到 "VSClaude" 这种虚构名，但若实际有冲突可加入更具体过滤）
const PROCESS_TARGETS: &[(&str, &str, &str)] = &[
    ("homebase", "HomeBase", "homebase"),
    ("claude", "Claude Code", "agent"),
    ("codex", "Codex", "agent"),
    ("gemini", "Gemini CLI", "agent"),
    ("openclaw", "OpenClaw", "agent"),
    ("hermes", "Hermes", "agent"),
    ("sops", "sops", "tool"),
];

/// 列出当前在跑的关键进程（HomeBase 自身 / 5 agent / sops）
#[tauri::command]
pub async fn system_processes() -> Result<Vec<ProcessInfo>, String> {
    tracing::debug!("system_processes 调用");
    let mut sys = System::new();
    // CPU% 需要两次采样间隔才有意义：先建基线 → 等一窗口 → 第二次刷新
    sys.refresh_processes(ProcessesToUpdate::All, true);
    tokio::time::sleep(Duration::from_millis(200)).await;
    sys.refresh_processes(ProcessesToUpdate::All, true);

    if sys.processes().is_empty() {
        return Err("QUERY_FAILED: 进程采样未返回数据，不能判定为没有运行中的进程".into());
    }
    let mut results: Vec<ProcessInfo> = sys
        .processes()
        .iter()
        .filter_map(|(_, p)| {
            let name_full = p.name().to_string_lossy().into_owned();
            let name_lower = name_full.to_lowercase();
            // 跳过非可执行（避免匹配到 "claude.txt" 之类虚构情况）
            if !name_lower.ends_with(".exe") {
                return None;
            }
            let target = PROCESS_TARGETS
                .iter()
                .find(|(key, _, _)| name_lower.contains(*key))?;
            Some(ProcessInfo {
                pid: p.pid().as_u32(),
                name: name_full,
                display_name: target.1.to_string(),
                category: target.2.to_string(),
                cpu_percent: p.cpu_usage(),
                memory_mb: (p.memory() as f64) / 1024.0 / 1024.0,
                run_secs: p.run_time(),
            })
        })
        .collect();

    // 排序：先按类别（homebase → agent → tool），同类按 display_name，同名按 PID
    results.sort_by(|a, b| {
        category_order(&a.category)
            .cmp(&category_order(&b.category))
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.pid.cmp(&b.pid))
    });

    Ok(results)
}

fn category_order(cat: &str) -> u8 {
    match cat {
        "homebase" => 0,
        "agent" => 1,
        "tool" => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_safety_disk_capacity_missing_is_not_omitted_or_zero() {
        use crate::adapters::powershell::PsDrive;
        let disk = |used, free| PsDrive {
            name: "T".into(),
            root: "T:\\".into(),
            used,
            free,
        };
        assert!(disk_info_from_drives(vec![disk(None, Some(100))]).is_err());
        assert!(disk_info_from_drives(vec![disk(Some(0), Some(0))]).is_err());
        assert!(disk_info_from_drives(vec![disk(Some(u64::MAX), Some(1))]).is_err());
        let valid = disk_info_from_drives(vec![disk(Some(0), Some(100))]).unwrap();
        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].usage_percent, 0.0);
    }

    #[tokio::test]
    async fn system_disks_returns_at_least_c_drive() {
        let disks = system_disks().await.expect("ok");
        assert!(!disks.is_empty(), "至少应有 C: 盘");
        let c = disks.iter().find(|d| d.drive == "C:");
        assert!(c.is_some(), "C: 盘必存在; got drives: {disks:?}");
        let c = c.unwrap();
        assert!(c.total_gb > 0.0);
        assert!(c.free_gb >= 0.0);
        assert!(c.usage_percent >= 0.0 && c.usage_percent <= 100.0);
    }

    #[tokio::test]
    async fn system_info_returns_basic_fields() {
        let info = system_info().await.expect("ok");
        assert!(!info.os.is_empty());
        assert!(!info.arch.is_empty());
        // hostname / username 在测试环境可能 fallback 到 "unknown"，不强 assert
    }

    #[tokio::test]
    async fn system_metrics_returns_sane_cpu_and_memory() {
        let m = system_metrics().await.expect("ok");
        assert!(m.cpu_cores > 0, "应至少有 1 个逻辑核心");
        assert!(
            m.cpu_percent >= 0.0 && m.cpu_percent <= 100.0,
            "cpu_percent 应在 0-100 范围: {}",
            m.cpu_percent
        );
        assert!(m.memory_total_gb > 0.0, "总内存应 > 0");
        assert!(
            m.memory_used_gb >= 0.0 && m.memory_used_gb <= m.memory_total_gb,
            "已用 ≤ 总: used={} total={}",
            m.memory_used_gb,
            m.memory_total_gb
        );
        assert!(m.memory_percent >= 0.0 && m.memory_percent <= 100.0);
    }

    #[tokio::test]
    async fn system_gpu_returns_either_data_or_error() {
        // GPU 可能不可用（CI / 虚拟机），只验证命令本身不 panic
        // 即使 PowerShell 调用失败，函数也应返回 Ok(GpuMetrics{available:false, error:Some})
        let g = system_gpu().await.expect("命令本身应永不 Err");
        if g.available {
            assert!(g.gpu_name.is_some() || g.gpu_percent.is_some());
            if let Some(p) = g.gpu_percent {
                assert!(p >= 0.0 && p <= 100.0, "gpu_percent 应在 0-100: {p}");
            }
            if let Some(memory) = g.dedicated_memory_gb {
                assert!(memory > 0.0, "已报告显存必须大于 0: {memory}");
            }
        } else {
            assert!(g.error.is_some(), "不可用时应给出原因");
        }
    }

    fn gpu_record(name: &str, memory_gb: i64, pnp: &str) -> GpuRecord {
        GpuRecord {
            name: Some(name.to_string()),
            adapter_ram: Some(memory_gb * 1024 * 1024 * 1024),
            adapter_compatibility: None,
            pnp_device_id: Some(pnp.to_string()),
        }
    }

    #[test]
    fn gpu_selector_ignores_virtual_adapters_and_prefers_discrete_gpu() {
        let records = vec![
            gpu_record("OrayIddDriver Device", 0, "ROOT\\DISPLAY\\0000"),
            gpu_record("Parsec Virtual Display Adapter", 0, "ROOT\\DISPLAY\\0001"),
            gpu_record("Intel(R) Iris(R) Xe Graphics", 1, "PCI\\VEN_8086"),
            gpu_record("NVIDIA GeForce RTX 3050 Ti Laptop GPU", 4, "PCI\\VEN_10DE"),
        ];
        let best = select_best_gpu(&records).expect("应选出物理 GPU");
        assert_eq!(
            best.name.as_deref(),
            Some("NVIDIA GeForce RTX 3050 Ti Laptop GPU")
        );
        assert_eq!(physical_gpu_records(&records).len(), 2);
    }

    #[test]
    fn gpu_selector_keeps_integrated_gpu_when_it_is_the_only_physical_adapter() {
        let records = vec![
            gpu_record("Microsoft Basic Display Adapter", 0, "ROOT\\DISPLAY\\0000"),
            gpu_record("Intel(R) Iris(R) Xe Graphics", 1, "PCI\\VEN_8086"),
        ];
        assert_eq!(
            select_best_gpu(&records).and_then(|record| record.name.as_deref()),
            Some("Intel(R) Iris(R) Xe Graphics")
        );
    }

    #[test]
    fn gpu_selector_returns_none_for_virtual_only_machine() {
        let records = vec![
            gpu_record("OrayIddDriver Device", 0, "ROOT\\DISPLAY\\0000"),
            gpu_record("Parsec Virtual Display Adapter", 0, "ROOT\\DISPLAY\\0001"),
        ];
        assert!(select_best_gpu(&records).is_none());
    }

    #[test]
    fn gpu_probe_distinguishes_detected_absent_and_unknown() {
        let detected =
            gpu_name_from_json(r#"[{"Name":"Synthetic NVIDIA GPU","AdapterRAM":4293918720}]"#);
        assert_eq!(
            detected.as_ref().unwrap().as_deref(),
            Some("Synthetic NVIDIA GPU")
        );
        assert_eq!(gpu_probe_status(&detected).0, GpuStatus::Detected);
        for json in ["[]", r#"[{"Name":"Parsec Virtual Display Adapter"}]"#] {
            let result = gpu_name_from_json(json);
            assert_eq!(result, Ok(None));
            assert_eq!(gpu_probe_status(&result).0, GpuStatus::NotDetected);
        }
        for json in [
            "",
            "null",
            "invalid",
            "{}",
            r#"[{"Name":42}]"#,
            r#"[{"Name":" "}]"#,
        ] {
            let result = gpu_name_from_json(json);
            assert!(result.is_err());
            assert_eq!(gpu_probe_status(&result).0, GpuStatus::Unknown);
            assert!(gpu_probe_status(&result).1.contains("未知"));
        }
        assert_eq!(gpu_probe_status(&Err("合成查询失败")).0, GpuStatus::Unknown);
        assert_eq!(
            gpu_name_from_json("\u{feff}{\"Name\":\" Synthetic Intel GPU \"}"),
            Ok(Some("Synthetic Intel GPU".to_string()))
        );
    }

    #[tokio::test]
    async fn gpu_probe_script_supports_windows_powershell_with_isolated_inventory() {
        let ps = PowerShellAdapter::new();
        // 遮蔽 CIM 命令，仅用合成清单验证实际生产脚本的 PS 5.1 数组序列化。
        for (json, expected) in [
            ("[]", None),
            (
                r#"[{"Name":"Synthetic Intel GPU","AdapterRAM":1073741824}]"#,
                Some("Synthetic Intel GPU"),
            ),
            (
                r#"[{"Name":"Parsec Virtual Display Adapter"},{"Name":"Synthetic Intel GPU"},{"Name":"Synthetic NVIDIA GPU","AdapterRAM":4293918720}]"#,
                Some("Synthetic NVIDIA GPU"),
            ),
            (r#"[{"Name":"OrayIddDriver Device"}]"#, None),
        ] {
            let script = format!("function Get-CimInstance {{ param($ClassName, $ErrorAction) foreach ($record in (ConvertFrom-Json -InputObject '{json}')) {{ $record }} }}; {GPU_INFO_SCRIPT}");
            let out = ps.run(&script).await.expect("合成 GPU 清单查询应成功");
            assert_eq!(
                gpu_name_from_json(&out.stdout).unwrap().as_deref(),
                expected
            );
        }
        let failure = format!(
            "function Get-CimInstance {{ throw 'synthetic CIM failure' }}; {GPU_INFO_SCRIPT}"
        );
        assert!(
            ps.run(&failure).await.is_err(),
            "查询失败不能被误报为未检测到"
        );
    }

    /// Phase G-8 · preflight 命令不应 panic，且返回 5 项稳定 ID
    #[tokio::test]
    async fn system_preflight_returns_five_items_with_stable_ids() {
        let checks = system_preflight().await.expect("preflight 永不抛错");
        assert_eq!(checks.len(), 5, "应返回 5 项自检");

        let ids: Vec<_> = checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "sops_exe",
                "secrets_yaml",
                "age_key",
                "reveal_exe",
                "agent_env"
            ],
            "ID 顺序应稳定（前端依赖此顺序渲染）"
        );

        // 每项 severity 必须是 critical / warning 之一
        for c in &checks {
            assert!(
                c.severity == "critical" || c.severity == "warning",
                "severity 非法: id={} severity={}",
                c.id,
                c.severity
            );
            // 失败项必须有 hint；成功项 hint 必须为 None
            if c.ok {
                assert!(c.hint.is_none(), "成功项不应有 hint: {}", c.id);
            } else {
                assert!(
                    c.hint.as_ref().map(|h| !h.is_empty()).unwrap_or(false),
                    "失败项必须有非空 hint: {}",
                    c.id
                );
            }
        }

        // agent_env 是 warning 级别
        let agent_env = checks.iter().find(|c| c.id == "agent_env").unwrap();
        assert_eq!(agent_env.severity, "warning");

        // 其余 4 项都是 critical
        for c in checks.iter().filter(|c| c.id != "agent_env") {
            assert_eq!(c.severity, "critical", "应为 critical: {}", c.id);
        }
    }

    /// age_key 检查对 SOPS_AGE_KEY_FILE 环境变量的处理
    #[tokio::test]
    async fn preflight_age_key_respects_env_var() {
        // 仅验证逻辑：SOPS_AGE_KEY_FILE 指向不存在的路径应失败
        // 注意不能直接改 env 做并行测试，所以这里只做存在性冒烟
        let check = check_age_key();
        // 不能强 assert ok/fail（依赖开发者本机环境），但必须有合法 severity + id
        assert_eq!(check.id, "age_key");
        assert_eq!(check.severity, "critical");
        assert_eq!(check.label, "age 私钥");
    }

    /// PATH 列表必须包含 Machine 级且 System32 必存在（任何 Windows 机器）
    ///
    /// 注意：System32 可能同时出现在 user + machine 两级（用户私自复制过 PATH 项的常见情况），
    /// 测试不强制 source —— 只验证「至少有一处 System32 是 exists=true」。
    #[tokio::test]
    async fn system_path_list_returns_machine_paths_with_system32() {
        let entries = system_path_list().await.expect("ok");
        assert!(!entries.is_empty(), "至少应有若干 PATH 项");

        let has_machine = entries.iter().any(|e| e.source == "machine");
        assert!(has_machine, "Machine 级 PATH 必应存在");

        // System32 是 Windows 必备项；至少有一个 entry 包含它且 exists=true
        let any_system32_exists = entries
            .iter()
            .any(|e| e.path.to_lowercase().contains("system32") && e.exists);
        assert!(
            any_system32_exists,
            "应能找到至少一个 exists=true 的 System32 项: got={entries:?}"
        );
    }

    /// 每个 PATH 条目的字段都应合法（source ∈ {user, machine}, path 非空）
    #[tokio::test]
    async fn system_path_list_entries_have_valid_fields() {
        let entries = system_path_list().await.expect("ok");
        for e in &entries {
            assert!(!e.path.is_empty(), "path 不应为空");
            assert!(!e.raw.is_empty(), "raw 不应为空");
            assert!(
                e.source == "user" || e.source == "machine",
                "source 必须是 user 或 machine: {}",
                e.source
            );
        }
    }

    /// 进程列表命令永不 panic；空也是合法返回（CI 环境可能没任何 agent）
    #[tokio::test]
    async fn system_processes_returns_ok_with_valid_fields() {
        let procs = system_processes().await.expect("命令永不 Err");
        for p in &procs {
            assert!(p.pid > 0, "PID 应 > 0");
            assert!(!p.name.is_empty(), "name 不应为空");
            assert!(!p.display_name.is_empty(), "display_name 不应为空");
            assert!(
                matches!(p.category.as_str(), "homebase" | "agent" | "tool"),
                "category 非法: {}",
                p.category
            );
            assert!(
                p.cpu_percent >= 0.0,
                "cpu_percent 应 >= 0: {}",
                p.cpu_percent
            );
            assert!(p.memory_mb >= 0.0, "memory_mb 应 >= 0: {}", p.memory_mb);
        }
    }

    /// category_order 排序映射稳定 · 防止重构误改影响前端分组顺序
    #[test]
    fn category_order_maps_correctly() {
        assert_eq!(category_order("homebase"), 0);
        assert_eq!(category_order("agent"), 1);
        assert_eq!(category_order("tool"), 2);
        assert_eq!(category_order("unknown"), 3);
    }
}
