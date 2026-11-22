export type ModelRecommendationStatus =
  | "installed"
  | "recommended"
  | "optional"
  | "api_preferred"
  | "needs_review"
  | "partial_install"
  | "missing_runtime"
  | "runtime_mismatch"
  | "storage_blocked"
  | "space_blocked"
  | "unsupported";

export type ModelLocalPriority = "recommended" | "optional" | "deferred";
export type ProbeSource = "live" | "preview" | "imported";
export type ProbeCompleteness = "complete" | "partial" | "failed" | "preview";
export type GpuState = "available" | "absent" | "unknown";
export type GpuKind = "discrete" | "integrated" | "none" | "unknown";
export type EvidenceConfidence = "verified" | "reported" | "inferred" | "unknown";
export type RuntimeCapabilityState = "available" | "missing" | "unknown" | "mismatch";
export type ManifestState = "loaded" | "missing" | "invalid" | "unavailable";
export type InstalledModelState = "installed" | "probed" | "partial" | "corrupt";

export interface RecommenderModelInput {
  id: string;
  name: string;
  category: string;
  status: string;
  hardware_fit: string;
  estimated_size_gb?: number;
  min_ram_gb?: number;
  min_logical_cores?: number;
  recommended_vram_gb?: number;
  runtime_requirements?: string[];
  local_priority?: ModelLocalPriority;
  api_fallback?: boolean;
}

export interface RecommenderDiskInput {
  drive: string;
  free_gb: number;
  total_gb?: number;
  usage_percent?: number;
  warning?: boolean;
  writable?: boolean | null;
}

export interface RecommenderSystemInfoInput {
  cpu_brand?: string | null;
  cpu_logical_cores?: number | null;
  memory_total_gb?: number | null;
  gpu_name?: string | null;
}

export interface RecommenderGpuInput {
  available: boolean;
  gpu_name: string | null;
  error?: string | null;
  dedicated_memory_gb?: number | null;
  probe_state?: GpuState;
  vendor?: string | null;
}

export interface RecommenderPathEntryInput {
  path: string;
  exists: boolean;
}

export interface RuntimeProbeInput {
  state: RuntimeCapabilityState;
  version?: string | null;
  detail?: string | null;
}

export interface InstalledModelInput {
  id: string;
  state: InstalledModelState;
  local_path?: string | null;
  path_exists?: boolean | null;
  detail?: string | null;
}

export interface DeriveMachineProfileInput {
  info: RecommenderSystemInfoInput;
  gpu: RecommenderGpuInput;
  disks: RecommenderDiskInput[];
  pathEntries: RecommenderPathEntryInput[];
  installedModelIds?: string[];
  installedModels?: InstalledModelInput[];
  runtimeProbes?: Record<string, RuntimeProbeInput>;
  manifestState?: ManifestState;
  source?: ProbeSource;
  probeCompleteness?: ProbeCompleteness;
  preferredModelRoot?: string | null;
}

export interface MachineProfile {
  source: ProbeSource;
  probeCompleteness: ProbeCompleteness;
  cpuLabel: string;
  cpuLogicalCores: number | null;
  ramGb: number | null;
  gpuLabel: string | null;
  gpuState: GpuState;
  gpuKind: GpuKind;
  gpuVendor: string | null;
  gpuAvailable: boolean;
  inferredVramGb: number | null;
  vramConfidence: EvidenceConfidence;
  disks: RecommenderDiskInput[];
  preferredModelRoot: string;
  preferredModelDrive: string | null;
  preferredFreeGb: number | null;
  preferredDiskWritable: boolean | null;
  systemDriveOnly: boolean;
  hasPython: boolean;
  hasCuda: boolean;
  runtimeStates: Record<string, RuntimeProbeInput>;
  manifestState: ManifestState;
  installedModelIds: string[];
  installedModels: InstalledModelInput[];
  diagnostics: string[];
}

export interface ModelRecommendation {
  id: string;
  recommendation_status: ModelRecommendationStatus;
  reason: string;
  next_action: string;
  confidence: "high" | "medium" | "low";
  warnings: string[];
  blockers: string[];
  decision_basis: string[];
  missing_dependencies: string[];
  unknown_dependencies: string[];
  preferred_install_root: string;
  estimated_space_required_gb: number | null;
}

export interface RecommendationSummary {
  state: "local_ready" | "api_ready" | "needs_input" | "blocked";
  title: string;
  detail: string;
  primary_model_id: string | null;
  counts: Record<ModelRecommendationStatus, number>;
}

const ALL_RECOMMENDATION_STATUSES: ModelRecommendationStatus[] = [
  "installed",
  "recommended",
  "optional",
  "api_preferred",
  "needs_review",
  "partial_install",
  "missing_runtime",
  "runtime_mismatch",
  "storage_blocked",
  "space_blocked",
  "unsupported",
];

export function deriveMachineProfile(input: DeriveMachineProfileInput): MachineProfile {
  const source = input.source ?? "live";
  const gpuLabel = input.gpu.gpu_name ?? input.info.gpu_name ?? null;
  const gpuState = deriveGpuState(input.gpu, gpuLabel, source);
  const gpuVendor = input.gpu.vendor ?? inferGpuVendor(gpuLabel);
  const gpuKind = inferGpuKind(gpuLabel, gpuState);
  const vram = deriveVram(input.gpu, gpuLabel, gpuKind);
  const requestedModelRoot = input.preferredModelRoot?.trim() || null;
  const requestedDrive = requestedModelRoot ? extractWindowsDrive(requestedModelRoot) : null;
  const preferredDisk = requestedModelRoot
    ? requestedDrive
      ? input.disks.find((disk) => normalizeDrive(disk.drive) === requestedDrive) ?? null
      : null
    : choosePreferredModelDisk(input.disks);
  const preferredDrive = requestedModelRoot
    ? requestedDrive
    : preferredDisk
      ? normalizeDrive(preferredDisk.drive)
      : null;
  const preferredModelRoot = requestedModelRoot ?? (preferredDrive ? `${preferredDrive}\\AIModels` : "AIModels");
  const pathText = input.pathEntries
    .filter((entry) => entry.exists)
    .map((entry) => entry.path.toLowerCase())
    .join(";");
  const runtimeStates = deriveRuntimeStates(
    input.runtimeProbes,
    pathText,
    source,
    gpuState,
    gpuVendor,
  );
  const installedModels = mergeInstalledModels(input.installedModels, input.installedModelIds);
  const manifestState = input.manifestState ?? (installedModels.length > 0 ? "loaded" : "unavailable");
  const diagnostics: string[] = [];

  if (source === "preview") diagnostics.push("当前是浏览器预览数据，未连接本机探测。 ");
  const ramGb = positiveFiniteOrNull(input.info.memory_total_gb);
  const cpuLogicalCores = positiveIntegerOrNull(input.info.cpu_logical_cores);
  if (ramGb == null) diagnostics.push("内存容量未探测或无效，不能自动批准本地模型。 ");
  if (cpuLogicalCores == null) diagnostics.push("CPU 逻辑核心数未探测或无效，不能估计本地处理速度。 ");
  if (gpuState === "unknown") diagnostics.push("GPU 状态不明确，GPU 模型只给条件建议。 ");
  if (!preferredDisk) diagnostics.push("没有可用磁盘容量数据，下载前必须选择并验证目录。 ");
  if (requestedModelRoot && !requestedDrive) diagnostics.push("模型根目录不是受支持的 Windows 盘符路径，无法关联容量探针。 ");
  if (requestedDrive && !preferredDisk) diagnostics.push(`模型根目录位于 ${requestedDrive}，但该盘没有可信容量数据。 `);
  if (preferredDisk?.writable === false) diagnostics.push("推荐模型盘不可写。 ");
  if (manifestState !== "loaded") diagnostics.push("安装清单未验证，界面不会把 probe 记录当成已安装。 ");

  const probeCompleteness =
    input.probeCompleteness ??
    (source === "preview"
      ? "preview"
      : diagnostics.length === 0
        ? "complete"
        : "partial");
  const usableDrives = input.disks.filter((disk) => Number.isFinite(disk.free_gb) && disk.free_gb > 0);

  return {
    source,
    probeCompleteness,
    cpuLabel: input.info.cpu_brand ?? "CPU 未知",
    cpuLogicalCores,
    ramGb,
    gpuLabel,
    gpuState,
    gpuKind,
    gpuVendor,
    gpuAvailable: gpuState === "available",
    inferredVramGb: vram.value,
    vramConfidence: vram.confidence,
    disks: input.disks,
    preferredModelRoot,
    preferredModelDrive: preferredDrive,
    preferredFreeGb: preferredDisk?.free_gb ?? null,
    preferredDiskWritable: preferredDisk?.writable ?? null,
    systemDriveOnly:
      preferredDrive?.toUpperCase() === "C:" ||
      (preferredDrive == null &&
        usableDrives.length > 0 &&
        usableDrives.every((disk) => normalizeDrive(disk.drive).toUpperCase() === "C:")),
    hasPython: runtimeStates.python.state === "available",
    hasCuda: runtimeStates.cuda.state === "available",
    runtimeStates,
    manifestState,
    installedModelIds: installedModels
      .filter((item) => item.state === "installed" && item.path_exists !== false)
      .map((item) => item.id),
    installedModels,
    diagnostics: diagnostics.map((item) => item.trim()),
  };
}

export function recommendModels(
  models: RecommenderModelInput[],
  machine: MachineProfile,
): ModelRecommendation[] {
  return models.map((model) => recommendModel(model, machine));
}

export function recommendModel(
  model: RecommenderModelInput,
  machine: MachineProfile,
): ModelRecommendation {
  const warnings: string[] = [];
  const blockers: string[] = [];
  const decisionBasis: string[] = [];
  const estimatedSizeGb = positiveFiniteOrNull(model.estimated_size_gb);
  const minRamGb = positiveFiniteOrNull(model.min_ram_gb);
  const minLogicalCores = positiveIntegerOrNull(model.min_logical_cores);
  const recommendedVramGb = nonNegativeFiniteOrNull(model.recommended_vram_gb);
  const installed = machine.installedModels.find((item) => item.id === model.id);
  const dependencyState = inspectDependencies(model, machine);

  if (machine.systemDriveOnly && installed?.state !== "installed") {
    warnings.push("当前只有系统盘可选；下载前应确认不会挤占系统更新和临时空间。 ");
  }
  if (
    machine.preferredDiskWritable == null &&
    machine.preferredModelDrive &&
    installed?.state !== "installed"
  ) {
    warnings.push("目标盘写入权限尚未执行实际探针，下载动作仍需做一次预检。 ");
  }

  if (installed?.state === "probed") {
    return result(model, machine, {
      status: "partial_install",
      reason: "当前只有来源探测记录，模型权重尚未验证为完整安装。",
      nextAction: "运行完整下载并校验目标目录、哈希和最小推理样例。",
      confidence: "high",
      warnings,
      blockers: ["模型仅完成 probe，不能直接执行推理。"],
      decisionBasis: ["安装清单状态：probed"],
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (installed && (installed.state === "partial" || installed.state === "corrupt" || installed.path_exists === false)) {
    return result(model, machine, {
      status: "partial_install",
      reason: installed.state === "corrupt" ? "模型安装校验失败。" : "模型安装不完整或登记路径不存在。",
      nextAction: "修复或重新下载后再执行最小推理验证。",
      confidence: "high",
      warnings,
      blockers: [installed.detail ?? "安装路径或文件完整性未通过。"],
      decisionBasis: [`安装清单状态：${installed.state}`],
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (installed?.state === "installed" && machine.manifestState !== "loaded") {
    return result(model, machine, {
      status: "partial_install",
      reason: "存在安装记录，但安装清单本身没有通过加载和校验。",
      nextAction: "修复 installed-models.json，并重新执行路径、哈希和最小推理验证。",
      confidence: "high",
      warnings,
      blockers: [`安装清单状态：${machine.manifestState}`],
      decisionBasis: [`安装记录：installed`, `安装清单：${machine.manifestState}`],
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (installed?.state === "installed" && installed.path_exists !== false) {
    const runtimeBlock = runtimeRecommendation(model, machine, dependencyState, warnings, true);
    if (runtimeBlock) return runtimeBlock;

    return result(model, machine, {
      status: "installed",
      reason: "模型权重、登记路径和最小推理已经验证。",
      nextAction: "可加入本地方案；首次使用仍执行一段最小样例。",
      confidence: "high",
      warnings,
      blockers,
      decisionBasis: ["安装清单状态：installed", "路径存在", "最小推理验证通过"],
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (
    estimatedSizeGb == null ||
    minRamGb == null ||
    minLogicalCores == null ||
    recommendedVramGb == null
  ) {
    const invalidMetadata = [
      estimatedSizeGb == null ? "模型大小" : null,
      minRamGb == null ? "最低内存" : null,
      minLogicalCores == null ? "最低 CPU 逻辑核心数" : null,
      recommendedVramGb == null ? "建议显存" : null,
    ].filter((item): item is string => item != null);
    return fallbackOrReview(model, machine, {
      reason: `模型目录缺少或包含无效硬件元数据：${invalidMetadata.join("、")}。`,
      nextAction: "修复并重新发布模型目录后刷新推荐。",
      warning: "模型元数据不完整，不能自动批准下载。",
      dependencyState,
      estimatedSizeGb,
      decisionBasis: ["目录元数据校验失败"],
    });
  }

  if (!machine.preferredModelDrive || machine.preferredFreeGb == null) {
    return fallbackOrReview(model, machine, {
      reason: "没有可信的模型盘容量数据，不能自动批准本地下载。",
      nextAction: "选择一个可写的数据盘并刷新环境诊断。",
      warning: "磁盘容量或目标根目录未知。",
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (machine.preferredDiskWritable === false) {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "storage_blocked",
      reason: model.api_fallback ? "当前推荐模型目录不可写，本地下载已阻断。" : "当前推荐模型目录不可写。",
      nextAction: model.api_fallback
        ? "先使用 API；重新绑定可写目录或修复权限后再启用本地模型。"
        : "重新绑定可写目录，或修复目录权限后再下载。",
      confidence: "high",
      warnings,
      blockers: ["目标目录写入探针失败。"],
      decisionBasis: [`目标根目录：${machine.preferredModelRoot}`],
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (estimatedSizeGb != null) {
    const reserveGb = storageReserveGb(machine, model);
    const downloadOverheadGb = Math.max(1, estimatedSizeGb * 0.25);
    const requiredGb = estimatedSizeGb + downloadOverheadGb + reserveGb;
    decisionBasis.push(
      `模型约 ${formatGb(estimatedSizeGb)}，下载开销约 ${formatGb(downloadOverheadGb)}，保留余量 ${formatGb(reserveGb)}`,
    );
    if (requiredGb > machine.preferredFreeGb) {
      return result(model, machine, {
        status: model.api_fallback ? "api_preferred" : "space_blocked",
        reason: `安全下载约需 ${formatGb(requiredGb)}，当前推荐盘仅剩 ${formatGb(machine.preferredFreeGb)}。`,
        nextAction: model.api_fallback ? "先使用 API；清理空间或切换模型盘后再启用本地模型。" : "清理空间或切换模型盘。",
        confidence: "high",
        warnings,
        blockers: ["安全余量不足，下载中途失败或挤满磁盘的风险过高。"],
        decisionBasis,
        dependencyState,
        estimatedSizeGb,
      });
    }
  }

  if (machine.ramGb == null) {
    return fallbackOrReview(model, machine, {
      reason: `模型最低内存要求为 ${formatGb(minRamGb)}，但本机内存容量未知。`,
      nextAction: "完成内存探测后重新推荐。",
      warning: "内存信息缺失。",
      dependencyState,
      estimatedSizeGb,
      decisionBasis,
    });
  }

  if (machine.ramGb < minRamGb) {
    const reason = `本机内存约 ${formatGb(machine.ramGb)}，低于最低 ${formatGb(minRamGb)}。`;
    if (model.api_fallback) {
      return result(model, machine, {
        status: "api_preferred",
        reason: `${reason} 本地不作为默认链路。`,
        nextAction: "使用云端方案，或换到内存更充足的机器。",
        confidence: "high",
        warnings,
        blockers: [reason],
        decisionBasis,
        dependencyState,
        estimatedSizeGb,
      });
    }
    return result(model, machine, {
      status: "unsupported",
      reason,
      nextAction: "选择更轻量的本地模型。",
      confidence: "high",
      warnings,
      blockers: [reason],
      decisionBasis,
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (machine.cpuLogicalCores == null) {
    return fallbackOrReview(model, machine, {
      reason: `模型最低要求 ${minLogicalCores} 个 CPU 逻辑核心，但本机核心数未知。`,
      nextAction: "完成 CPU 核心数探测后重新推荐。",
      warning: "CPU 核心数信息缺失。",
      dependencyState,
      estimatedSizeGb,
      decisionBasis,
    });
  }

  if (machine.cpuLogicalCores < minLogicalCores) {
    const reason = `模型最低要求 ${minLogicalCores} 个 CPU 逻辑核心，本机仅检测到 ${machine.cpuLogicalCores} 个。`;
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "unsupported",
      reason,
      nextAction: model.api_fallback ? "优先使用 API，或选择 CPU 要求更低的模型。" : "选择 CPU 要求更低的本地模型。",
      confidence: "high",
      warnings,
      blockers: [reason],
      decisionBasis,
      dependencyState,
      estimatedSizeGb,
    });
  }

  const gpuBlock = gpuRecommendation(model, machine, dependencyState, warnings, decisionBasis);
  if (gpuBlock) return gpuBlock;

  const runtimeBlock = runtimeRecommendation(model, machine, dependencyState, warnings, false, decisionBasis);
  if (runtimeBlock) return runtimeBlock;

  if (machine.source === "preview" || machine.probeCompleteness === "failed") {
    return fallbackOrReview(model, machine, {
      reason: "当前不是可信的本机诊断结果，不能自动批准本地安装。",
      nextAction: "在 HomeBase 桌面应用中运行环境诊断。",
      warning: "浏览器预览或探测失败。",
      dependencyState,
      estimatedSizeGb,
      decisionBasis,
    });
  }

  if (model.local_priority === "recommended") {
    return result(model, machine, {
      status: "recommended",
      reason: "已知硬件、存储和运行时满足基础条件，适合作为默认本地链路。",
      nextAction: "先 probe 来源，再下载并执行最小推理验证。",
      confidence: warnings.length > 0 ? "medium" : "high",
      warnings,
      blockers,
      decisionBasis: [...decisionBasis, "本地优先级：recommended"],
      dependencyState,
      estimatedSizeGb,
    });
  }

  if (model.local_priority === "deferred") {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "optional",
      reason: model.api_fallback ? "本地条件基本满足，但该模型不是默认链路，日常优先 API。" : "可作为专项评测，不进入默认组合。",
      nextAction: model.api_fallback ? "优先启用云端方案；仅在专项需求下安装。" : "按任务需要加入自定义组合。",
      confidence: "medium",
      warnings,
      blockers,
      decisionBasis: [...decisionBasis, "本地优先级：deferred"],
      dependencyState,
      estimatedSizeGb,
    });
  }

  return result(model, machine, {
    status: "optional",
    reason: "基础条件满足，但它不是当前机器的默认模型。",
    nextAction: "按场景加入自定义组合，先运行一段样例再长期启用。",
    confidence: warnings.length > 0 ? "medium" : "high",
    warnings,
    blockers,
    decisionBasis: [...decisionBasis, "本地优先级：optional"],
    dependencyState,
    estimatedSizeGb,
  });
}

export function summarizeRecommendations(recommendations: ModelRecommendation[]): RecommendationSummary {
  const counts = Object.fromEntries(
    ALL_RECOMMENDATION_STATUSES.map((status) => [
      status,
      recommendations.filter((item) => item.recommendation_status === status).length,
    ]),
  ) as Record<ModelRecommendationStatus, number>;
  if (recommendations.length === 0) {
    return {
      state: "needs_input",
      title: "模型目录为空",
      detail: "刷新或修复模型目录后再运行本机推荐；在此之前可直接配置云端服务。",
      primary_model_id: null,
      counts,
    };
  }
  const local = recommendations.find((item) => item.recommendation_status === "installed") ??
    recommendations.find((item) => item.recommendation_status === "recommended");
  if (local) {
    return {
      state: "local_ready",
      title: "本地路线可用",
      detail: `优先使用 ${local.id}；下载或首次运行前仍执行单模型预检。`,
      primary_model_id: local.id,
      counts,
    };
  }

  const api = recommendations.find((item) => item.recommendation_status === "api_preferred");
  if (api) {
    return {
      state: "api_ready",
      title: "先走云端路线",
      detail: "本地条件不足或尚未确认，但至少有 API 备用方案，不会留下空白推荐。",
      primary_model_id: api.id,
      counts,
    };
  }

  const optional = recommendations.find((item) => item.recommendation_status === "optional");
  if (optional) {
    return {
      state: "local_ready",
      title: "本地候选可用",
      detail: `可先试运行 ${optional.id}；它不会自动进入默认组合，需用真实样本确认速度和质量。`,
      primary_model_id: optional.id,
      counts,
    };
  }

  const review = recommendations.find((item) =>
    ["needs_review", "partial_install", "missing_runtime", "runtime_mismatch"].includes(
      item.recommendation_status,
    ),
  );
  if (review) {
    return {
      state: "needs_input",
      title: "需要补一项环境信息",
      detail: review.next_action,
      primary_model_id: review.id,
      counts,
    };
  }

  return {
    state: "blocked",
    title: "本机路线暂不可用",
    detail: "当前候选均被硬件或存储条件阻断；请切换更轻模型、释放空间或配置云端服务。",
    primary_model_id: null,
    counts,
  };
}

interface DependencyInspection {
  missing: string[];
  unknown: string[];
  mismatch: string[];
}

interface ResultOptions {
  status: ModelRecommendationStatus;
  reason: string;
  nextAction: string;
  confidence: "high" | "medium" | "low";
  warnings: string[];
  blockers: string[];
  decisionBasis: string[];
  dependencyState: DependencyInspection;
  estimatedSizeGb: number | null;
}

function result(
  model: RecommenderModelInput,
  machine: MachineProfile,
  options: ResultOptions,
): ModelRecommendation {
  return {
    id: model.id,
    recommendation_status: options.status,
    reason: options.reason,
    next_action: options.nextAction,
    confidence: options.confidence,
    warnings: unique(options.warnings.map((item) => item.trim())),
    blockers: unique(options.blockers),
    decision_basis: unique(options.decisionBasis),
    missing_dependencies: unique(options.dependencyState.missing),
    unknown_dependencies: unique([
      ...options.dependencyState.unknown,
      ...options.dependencyState.mismatch,
    ]),
    preferred_install_root: machine.preferredModelRoot,
    estimated_space_required_gb: options.estimatedSizeGb,
  };
}

function fallbackOrReview(
  model: RecommenderModelInput,
  machine: MachineProfile,
  options: {
    reason: string;
    nextAction: string;
    warning: string;
    dependencyState: DependencyInspection;
    estimatedSizeGb: number | null;
    decisionBasis?: string[];
  },
) {
  return result(model, machine, {
    status: model.api_fallback ? "api_preferred" : "needs_review",
    reason: model.api_fallback ? `${options.reason} 当前优先走 API。` : options.reason,
    nextAction: options.nextAction,
    confidence: "low",
    warnings: [options.warning],
    blockers: model.api_fallback ? [] : [options.warning],
    decisionBasis: options.decisionBasis ?? [],
    dependencyState: options.dependencyState,
    estimatedSizeGb: options.estimatedSizeGb,
  });
}

function gpuRecommendation(
  model: RecommenderModelInput,
  machine: MachineProfile,
  dependencyState: DependencyInspection,
  warnings: string[],
  decisionBasis: string[],
) {
  const requiredVramGb = model.recommended_vram_gb ?? 0;
  const requiresGpu = requiredVramGb > 0 || (model.runtime_requirements ?? []).some((item) => item.toLowerCase() === "cuda");
  if (!requiresGpu) return null;

  if (machine.gpuState === "absent" || machine.gpuKind === "none") {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "unsupported",
      reason: "未检测到可用 GPU，该模型不进入默认本地链路。",
      nextAction: model.api_fallback ? "使用 API 版本。" : "选择支持 CPU 的模型。",
      confidence: "high",
      warnings,
      blockers: ["缺少 GPU。"],
      decisionBasis,
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
    });
  }

  if (machine.gpuState === "unknown" || machine.gpuKind === "unknown") {
    return fallbackOrReview(model, machine, {
      reason: "GPU 型号或可用状态无法确认。",
      nextAction: "完成 GPU/驱动/显存探测后再决定是否本地安装。",
      warning: "GPU 信息不完整。",
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
      decisionBasis,
    });
  }

  const requiresCuda = (model.runtime_requirements ?? []).some((item) => item.toLowerCase() === "cuda");
  if (requiresCuda && machine.gpuVendor !== "nvidia") {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "unsupported",
      reason: `该模型要求 CUDA，但当前 GPU 厂商为 ${machine.gpuVendor ?? "未知"}。`,
      nextAction: model.api_fallback ? "使用 API，或在 NVIDIA/CUDA 机器上运行。" : "选择支持当前 GPU 或 CPU 的模型。",
      confidence: machine.gpuVendor && machine.gpuVendor !== "unknown" ? "high" : "medium",
      warnings,
      blockers: ["CUDA 路线需要受支持的 NVIDIA GPU。"],
      decisionBasis,
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
    });
  }

  if (machine.gpuKind === "integrated" && requiredVramGb > 0) {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "unsupported",
      reason: "当前仅检测到核显，不自动推荐需要独立显存的本地路线。",
      nextAction: model.api_fallback ? "使用 API；如需强制本地，请先做专项基准测试。" : "选择 CPU 友好模型。",
      confidence: "high",
      warnings,
      blockers: ["核显不满足独立显存型推荐规则。"],
      decisionBasis,
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
    });
  }

  if (requiredVramGb <= 0) return null;
  if (machine.inferredVramGb == null) {
    return fallbackOrReview(model, machine, {
      reason: `模型建议 ${formatGb(requiredVramGb)} 显存，但显存容量未知。`,
      nextAction: "读取显卡专用显存后重新推荐。",
      warning: "不能仅凭 GPU 名称批准显存型模型。",
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
      decisionBasis,
    });
  }

  decisionBasis.push(`显存 ${formatGb(machine.inferredVramGb)}（${machine.vramConfidence}）`);
  if (machine.inferredVramGb < requiredVramGb) {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "unsupported",
      reason: `模型建议 ${formatGb(requiredVramGb)} 显存，当前约 ${formatGb(machine.inferredVramGb)}。`,
      nextAction: model.api_fallback ? "使用 API，或换到显存更充足的机器。" : "选择更轻量模型。",
      confidence: machine.vramConfidence === "verified" || machine.vramConfidence === "reported" ? "high" : "medium",
      warnings,
      blockers: ["显存低于建议值。"],
      decisionBasis,
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
    });
  }

  if (machine.vramConfidence === "inferred") {
    return fallbackOrReview(model, machine, {
      reason: "型号推断的显存看似满足，但不是可用于自动下载的可靠证据。",
      nextAction: "用系统或驱动工具读取真实专用显存。",
      warning: "显存仅由型号推断。",
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
      decisionBasis,
    });
  }

  return null;
}

function runtimeRecommendation(
  model: RecommenderModelInput,
  machine: MachineProfile,
  dependencyState: DependencyInspection,
  warnings: string[],
  installed: boolean,
  decisionBasis: string[] = [],
) {
  if (dependencyState.mismatch.length > 0) {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : "runtime_mismatch",
      reason: `运行时版本不兼容：${dependencyState.mismatch.join(", ")}。`,
      nextAction: "切换到该模型的隔离运行时，或修复版本后重新检测。",
      confidence: "high",
      warnings,
      blockers: [`运行时不兼容：${dependencyState.mismatch.join(", ")}`],
      decisionBasis,
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
    });
  }

  if (dependencyState.missing.length > 0) {
    return result(model, machine, {
      status: model.api_fallback ? "api_preferred" : installed ? "runtime_mismatch" : "missing_runtime",
      reason: `缺少运行时：${dependencyState.missing.join(", ")}。`,
      nextAction: model.api_fallback ? "先使用 API，并按安装说明补齐运行时。" : "安装隔离运行时后重新检测。",
      confidence: "high",
      warnings,
      blockers: [`缺少运行时：${dependencyState.missing.join(", ")}`],
      decisionBasis,
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
    });
  }

  if (dependencyState.unknown.length > 0) {
    if (installed) {
      warnings.push(
        `通用运行时版本尚未单独探测（${dependencyState.unknown.join(", ")}），但该安装已有通过的最小推理证据。`,
      );
      return null;
    }
    return fallbackOrReview(model, machine, {
      reason: `运行时状态未确认：${dependencyState.unknown.join(", ")}。`,
      nextAction: "运行最小环境探针并记录版本。",
      warning: `运行时未确认：${dependencyState.unknown.join(", ")}`,
      dependencyState,
      estimatedSizeGb: model.estimated_size_gb ?? null,
      decisionBasis,
    });
  }

  return null;
}

function inspectDependencies(model: RecommenderModelInput, machine: MachineProfile): DependencyInspection {
  const state: DependencyInspection = { missing: [], unknown: [], mismatch: [] };
  for (const requirement of model.runtime_requirements ?? []) {
    const normalized = requirement.toLowerCase();
    const probe = machine.runtimeStates[normalized] ?? { state: "unknown" as const };
    if (probe.state === "missing") state.missing.push(normalized);
    if (probe.state === "unknown") state.unknown.push(normalized);
    if (probe.state === "mismatch") state.mismatch.push(normalized);
  }
  return state;
}

function deriveRuntimeStates(
  explicit: Record<string, RuntimeProbeInput> | undefined,
  pathText: string,
  source: ProbeSource,
  gpuState: GpuState,
  gpuVendor: string | null,
) {
  const states: Record<string, RuntimeProbeInput> = {};
  for (const [name, value] of Object.entries(explicit ?? {})) {
    states[name.toLowerCase()] = value;
  }

  const infer = (name: string, pattern: RegExp, missingWhenLive = true) => {
    if (states[name]) return;
    if (source === "preview") {
      states[name] = { state: "unknown", detail: "浏览器预览未执行运行时探针。" };
    } else if (pattern.test(pathText)) {
      states[name] = { state: "unknown", detail: "PATH 中发现候选目录，但尚未执行命令和版本探针。" };
    } else {
      states[name] = { state: missingWhenLive ? "missing" : "unknown" };
    }
  };

  infer("python", /python|pyenv|conda|mambaforge|miniconda|anaconda/);
  infer("ffmpeg", /ffmpeg/);
  infer("cuda", /cuda|nvidia gpu computing toolkit/, gpuState === "absent" || gpuVendor !== "nvidia");
  if (states.cuda.state === "missing" && gpuState === "available" && gpuVendor === "nvidia") {
    states.cuda = { state: "unknown", detail: "检测到 NVIDIA GPU，但 CUDA 运行时未执行版本探针。" };
  }
  return states;
}

function deriveGpuState(gpu: RecommenderGpuInput, gpuLabel: string | null, source: ProbeSource): GpuState {
  if (gpu.probe_state) return gpu.probe_state;
  if (gpuLabel) return "available";
  if (source === "preview" || gpu.error) return "unknown";
  return gpu.available ? "available" : "absent";
}

function inferGpuVendor(gpuName: string | null) {
  if (!gpuName) return null;
  const lower = gpuName.toLowerCase();
  if (/nvidia|geforce|quadro|tesla/.test(lower)) return "nvidia";
  if (/amd|radeon/.test(lower)) return "amd";
  if (/intel|iris|uhd|arc/.test(lower)) return "intel";
  if (/apple/.test(lower)) return "apple";
  return "unknown";
}

function inferGpuKind(gpuName: string | null, gpuState: GpuState): GpuKind {
  if (gpuState === "absent") return "none";
  if (gpuState === "unknown" || !gpuName) return "unknown";
  const lower = gpuName.toLowerCase();
  if (/intel.*(iris|uhd|hd graphics)|radeon graphics|vega \d/.test(lower)) return "integrated";
  if (/nvidia|geforce|quadro|tesla|radeon (rx|pro)|intel.*arc/.test(lower)) return "discrete";
  return "unknown";
}

function deriveVram(gpu: RecommenderGpuInput, gpuName: string | null, gpuKind: GpuKind) {
  const reportedVramGb = positiveFiniteOrNull(gpu.dedicated_memory_gb);
  if (reportedVramGb != null) {
    return { value: reportedVramGb, confidence: "reported" as const };
  }
  if (!gpuName || gpuKind === "integrated" || gpuKind === "none") {
    return { value: null, confidence: "unknown" as const };
  }

  const lower = gpuName.toLowerCase();
  const explicit = lower.match(/(\d{1,2})\s*gb/);
  if (explicit) {
    return { value: Number.parseInt(explicit[1], 10), confidence: "inferred" as const };
  }
  const known: Array<[RegExp, number]> = [
    [/3050/, 4],
    [/3060|4050/, 6],
    [/3070|4060|4070/, 8],
    [/3080/, 10],
    [/4080|5080/, 16],
    [/3090|4090|5090/, 24],
  ];
  const match = known.find(([pattern]) => pattern.test(lower));
  return match
    ? { value: match[1], confidence: "inferred" as const }
    : { value: null, confidence: "unknown" as const };
}

function mergeInstalledModels(
  installedModels: InstalledModelInput[] | undefined,
  installedModelIds: string[] | undefined,
) {
  const merged = new Map<string, InstalledModelInput>();
  for (const item of installedModels ?? []) merged.set(item.id, item);
  for (const id of installedModelIds ?? []) {
    if (!merged.has(id)) {
      merged.set(id, {
        id,
        state: "probed",
        path_exists: null,
        detail: "旧式 ID 清单不包含路径、完整性和最小推理证据。",
      });
    }
  }
  return [...merged.values()];
}

function choosePreferredModelDisk(disks: RecommenderDiskInput[]) {
  const usable = disks.filter((disk) => Number.isFinite(disk.free_gb) && disk.free_gb > 0);
  const writable = usable.filter((disk) => disk.writable !== false);
  const pool = writable.length > 0 ? writable : usable;
  const nonSystem = pool.filter((disk) => normalizeDrive(disk.drive).toUpperCase() !== "C:");
  const candidates = nonSystem.length > 0 ? nonSystem : pool;
  return [...candidates].sort((a, b) => b.free_gb - a.free_gb)[0] ?? null;
}

function normalizeDrive(drive: string) {
  const cleaned = drive.trim().replace(/\\+$/, "");
  if (/^[A-Za-z]$/.test(cleaned)) return `${cleaned.toUpperCase()}:`;
  if (/^[A-Za-z]:$/.test(cleaned)) return cleaned.toUpperCase();
  return cleaned;
}

function extractWindowsDrive(path: string) {
  const match = path.trim().match(/^([A-Za-z]):(?:\\|\/|$)/);
  return match ? `${match[1].toUpperCase()}:` : null;
}

function positiveFiniteOrNull(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : null;
}

function nonNegativeFiniteOrNull(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

function positiveIntegerOrNull(value: number | null | undefined) {
  return typeof value === "number" && Number.isInteger(value) && value > 0 ? value : null;
}

function storageReserveGb(machine: MachineProfile, model: RecommenderModelInput) {
  const selected = machine.disks.find(
    (disk) => normalizeDrive(disk.drive) === machine.preferredModelDrive,
  );
  const totalReserve = selected?.total_gb ? selected.total_gb * (machine.systemDriveOnly ? 0.1 : 0.05) : 0;
  const modelReserve = (model.estimated_size_gb ?? 0) * 0.25;
  return Math.max(machine.systemDriveOnly ? 20 : 10, totalReserve, modelReserve);
}

function unique(values: string[]) {
  return [...new Set(values.filter(Boolean))];
}

function formatGb(value: number) {
  return `${Math.round(value * 10) / 10} GB`;
}
