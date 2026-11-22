import type { MachineProfile } from "@/lib/model-recommender";

export const ASR_PORTABLE_SCHEMA_VERSION = 2 as const;
const LEGACY_ASR_PORTABLE_SCHEMA_VERSION = 1 as const;

export const ASR_PORTABLE_ENGINE_IDS = [
  "faster_whisper_large_v3_cpu",
  "sensevoice_small",
  "openai_gpt_4o_transcribe_diarize",
  "deepgram_nova_3",
  "assemblyai_universal",
  "elevenlabs_scribe",
] as const;

export const BUILT_IN_ASR_PRESET_IDS = [
  "asr_local_sprint",
  "asr_zh_balanced",
  "asr_accuracy_max",
  "asr_speaker_meeting",
  "asr_realtime_caption",
  "asr_full_audit",
] as const;

export type AsrPortableFusionMode = "auto" | "llm" | "manual";

export interface AsrPortableCustomPreset {
  id: string;
  name: string;
  description: string;
  engine_ids: string[];
  fusion_mode: AsrPortableFusionMode;
  scene_id: string;
  language: string;
}

export interface AsrPortableRoots {
  aihub_root: string;
  model_root: string;
  runtime_root: string;
  output_root: string;
  cache_root: string;
}

export interface AsrPortableConfig {
  schema_version: typeof ASR_PORTABLE_SCHEMA_VERSION;
  profile_id: string;
  profile_name: string;
  roots: AsrPortableRoots;
  catalog_rel_path: string;
  manifest_rel_path: string;
  preset_rel_path: string;
  default_preset_id: string;
  enabled_preset_ids: string[];
  custom_presets: AsrPortableCustomPreset[];
  provider_secret_refs: string[];
  policies: {
    avoid_system_drive_for_models: boolean;
    keep_original_audio: boolean;
    require_preflight_before_download: boolean;
    require_smoke_test_after_install: boolean;
  };
}

export interface AsrPortableBundle {
  format: "homebase-asr-portable-profile";
  schema_version: typeof ASR_PORTABLE_SCHEMA_VERSION;
  exported_at: string;
  source_hints: {
    os_family: "windows" | "macos" | "linux" | "unknown";
    profile_name: string;
  };
  path_tokens: Record<keyof AsrPortableRoots, string>;
  config: Omit<AsrPortableConfig, "roots"> & {
    roots: Record<keyof AsrPortableRoots, string>;
  };
  migration_checks: string[];
}

export interface PortabilityIssue {
  code: string;
  severity: "error" | "warning" | "info";
  field: string;
  message: string;
  fix: string;
}

export interface PortabilityValidation {
  ready: boolean;
  status: "ready" | "needs_attention" | "blocked";
  issues: PortabilityIssue[];
}

const ROOT_TOKEN_BY_KEY: Record<keyof AsrPortableRoots, string> = {
  aihub_root: "${AIHUB_ROOT}",
  model_root: "${MODEL_ROOT}",
  runtime_root: "${RUNTIME_ROOT}",
  output_root: "${OUTPUT_ROOT}",
  cache_root: "${CACHE_ROOT}",
};

export function createDefaultAsrPortableConfig(machine: MachineProfile): AsrPortableConfig {
  const modelRoot = machine.preferredModelRoot;
  const drive = extractWindowsDrive(modelRoot) ?? machine.preferredModelDrive;
  const dataRoot = drive ? `${drive}\\HomeBaseData` : "HomeBaseData";

  return {
    schema_version: ASR_PORTABLE_SCHEMA_VERSION,
    profile_id: "homebase-asr-default",
    profile_name: "HomeBase ASR 默认配置",
    roots: {
      aihub_root: "C:\\HomeBase\\AIHub",
      model_root: modelRoot,
      runtime_root: drive ? `${drive}\\Runtimes` : `${dataRoot}\\Runtimes`,
      output_root: "C:\\HomeBase\\Wiki\\raw\\audio",
      cache_root: drive ? `${drive}\\Temp\\homebase-model-cache` : `${dataRoot}\\Cache`,
    },
    catalog_rel_path: "capabilities/model-download-catalog.json",
    manifest_rel_path: "models/installed-models.json",
    preset_rel_path: "capabilities/asr-presets.json",
    default_preset_id: "asr_accuracy_max",
    enabled_preset_ids: [
      "asr_local_sprint",
      "asr_zh_balanced",
      "asr_accuracy_max",
      "asr_speaker_meeting",
      "asr_realtime_caption",
      "asr_full_audit",
    ],
    custom_presets: [],
    provider_secret_refs: [
      "OPENROUTER_API_KEY",
      "DEEPGRAM_API_KEY",
      "ASSEMBLYAI_API_KEY",
      "ELEVENLABS_API_KEY",
      "OPENAI_API_KEY",
    ],
    policies: {
      avoid_system_drive_for_models: true,
      keep_original_audio: true,
      require_preflight_before_download: true,
      require_smoke_test_after_install: true,
    },
  };
}

export function validateAsrPortableConfig(
  config: AsrPortableConfig,
  machine?: MachineProfile,
): PortabilityValidation {
  const issues: PortabilityIssue[] = [];

  if (config.schema_version !== ASR_PORTABLE_SCHEMA_VERSION) {
    issues.push({
      code: "schema_unsupported",
      severity: "error",
      field: "schema_version",
      message: `不支持配置版本 ${String(config.schema_version)}。`,
      fix: `先迁移到 schema_version ${ASR_PORTABLE_SCHEMA_VERSION}。`,
    });
  }

  for (const [key, value] of Object.entries(config.roots) as Array<[
    keyof AsrPortableRoots,
    string,
  ]>) {
    if (!value.trim()) {
      issues.push({
        code: "root_empty",
        severity: "error",
        field: `roots.${key}`,
        message: "根目录不能为空。",
        fix: "在目标机选择一个明确的绝对目录。",
      });
      continue;
    }
    if (!isAbsoluteLocalPath(value)) {
      issues.push({
        code: "root_not_absolute",
        severity: "error",
        field: `roots.${key}`,
        message: `“${value}”不是可识别的绝对路径。`,
        fix: "使用盘符路径、UNC 路径或 POSIX 绝对路径。",
      });
    }
    if (hasUnsafePathSegment(value)) {
      issues.push({
        code: "root_unsafe_segment",
        severity: "error",
        field: `roots.${key}`,
        message: "路径包含 Windows 保留名称或非法字符。",
        fix: "重新选择不含保留名称和非法字符的目录。",
      });
    }
  }

  const normalizedRoots = Object.entries(config.roots).map(([key, value]) => [
    key,
    normalizePath(value),
  ] as const);
  for (let left = 0; left < normalizedRoots.length; left += 1) {
    for (let right = left + 1; right < normalizedRoots.length; right += 1) {
      if (normalizedRoots[left][1] === normalizedRoots[right][1]) {
        issues.push({
          code: "root_collision",
          severity: "error",
          field: `roots.${normalizedRoots[right][0]}`,
          message: `${normalizedRoots[left][0]} 与 ${normalizedRoots[right][0]} 指向同一目录。`,
          fix: "模型、运行时、输出和缓存必须使用可区分的目录。",
        });
      }
    }
  }

  const output = normalizePath(config.roots.output_root);
  const model = normalizePath(config.roots.model_root);
  const runtime = normalizePath(config.roots.runtime_root);
  const cache = normalizePath(config.roots.cache_root);
  if (isWithin(output, model) || isWithin(output, runtime) || isWithin(output, cache)) {
    issues.push({
      code: "output_nested_in_runtime_data",
      severity: "error",
      field: "roots.output_root",
      message: "用户输出不能放在模型、运行时或缓存目录内。",
      fix: "把录音任务包绑定到独立的 Wiki/数据目录。",
    });
  }

  if (
    config.policies.avoid_system_drive_for_models &&
    extractWindowsDrive(config.roots.model_root)?.toUpperCase() === "C:"
  ) {
    issues.push({
      code: "model_root_on_system_drive",
      severity: "warning",
      field: "roots.model_root",
      message: "模型目录位于系统盘。",
      fix: "目标机有其他可写盘时，优先重新绑定到数据盘。",
    });
  }

  if (machine) {
    const modelDrive = extractWindowsDrive(config.roots.model_root);
    const disk = machine.disks.find(
      (item) => normalizeDrive(item.drive) === normalizeDrive(modelDrive ?? ""),
    );
    if (!disk) {
      issues.push({
        code: "model_drive_not_detected",
        severity: "warning",
        field: "roots.model_root",
        message: "目标机未检测到模型目录对应的磁盘。",
        fix: "重新绑定模型目录，再执行写入和空间预检。",
      });
    } else if (disk.free_gb < 10) {
      issues.push({
        code: "model_drive_low_space",
        severity: "error",
        field: "roots.model_root",
        message: `模型盘只剩约 ${Math.round(disk.free_gb)} GB。`,
        fix: "释放空间或改绑其他磁盘。",
      });
    } else if (disk.writable === false) {
      issues.push({
        code: "model_drive_not_writable",
        severity: "error",
        field: "roots.model_root",
        message: "模型目录对应磁盘不可写。",
        fix: "修复权限或选择其他目录。",
      });
    }

    if (machine.source === "preview") {
      issues.push({
        code: "preview_profile",
        severity: "warning",
        field: "machine",
        message: "当前仅有浏览器预览环境，不能完成目标机预检。",
        fix: "在 HomeBase 桌面应用中导入并再次检查。",
      });
    }
  }

  const builtInPresetIds = new Set<string>(BUILT_IN_ASR_PRESET_IDS);
  const knownEngineIds = new Set<string>(ASR_PORTABLE_ENGINE_IDS);
  const customPresetIds = new Set<string>();
  const customPresetNames = new Set<string>();
  const supportedSceneIds = new Set(["auto", "daily_note", "meeting_noisy", "reverb_room", "low_volume"]);
  const supportedLanguages = new Set(["zh", "auto", "en", "ja", "ko"]);
  if (config.custom_presets.length > 24) {
    issues.push({
      code: "too_many_custom_presets",
      severity: "error",
      field: "custom_presets",
      message: "自定义组合超过 24 个。",
      fix: "删除不再使用的组合，避免列表失控。",
    });
  }
  for (const [index, preset] of config.custom_presets.entries()) {
    const field = `custom_presets.${index}`;
    const normalizedName = preset.name.trim().toLocaleLowerCase();
    if (!/^custom_[a-z0-9][a-z0-9_-]{2,47}$/.test(preset.id)) {
      issues.push({
        code: "custom_preset_id_invalid",
        severity: "error",
        field: `${field}.id`,
        message: `自定义组合 ID“${preset.id}”不符合规则。`,
        fix: "使用 custom_ 开头、3-48 位的小写字母、数字、下划线或短横线。",
      });
    }
    if (builtInPresetIds.has(preset.id) || customPresetIds.has(preset.id)) {
      issues.push({
        code: "custom_preset_id_collision",
        severity: "error",
        field: `${field}.id`,
        message: `组合 ID“${preset.id}”发生冲突。`,
        fix: "为每个自定义组合生成唯一 ID。",
      });
    }
    customPresetIds.add(preset.id);

    if (!preset.name.trim() || preset.name.trim().length > 40) {
      issues.push({
        code: "custom_preset_name_invalid",
        severity: "error",
        field: `${field}.name`,
        message: "组合名称必须为 1-40 个字符。",
        fix: "填写简短、可识别的名称。",
      });
    } else if (customPresetNames.has(normalizedName)) {
      issues.push({
        code: "custom_preset_name_duplicate",
        severity: "error",
        field: `${field}.name`,
        message: `组合名称“${preset.name}”重复。`,
        fix: "使用不同名称，避免运行时选错。",
      });
    }
    customPresetNames.add(normalizedName);

    if (preset.description.length > 160) {
      issues.push({
        code: "custom_preset_description_too_long",
        severity: "warning",
        field: `${field}.description`,
        message: "组合说明超过 160 个字符。",
        fix: "压缩为一行场景说明。",
      });
    }
    if (preset.engine_ids.length === 0 || preset.engine_ids.length > 8) {
      issues.push({
        code: "custom_preset_engine_count_invalid",
        severity: "error",
        field: `${field}.engine_ids`,
        message: "每个组合必须选择 1-8 个引擎。",
        fix: "至少选择一个有效引擎，并移除无必要的重复路线。",
      });
    }
    if (new Set(preset.engine_ids).size !== preset.engine_ids.length) {
      issues.push({
        code: "custom_preset_engine_duplicate",
        severity: "error",
        field: `${field}.engine_ids`,
        message: "组合中存在重复引擎。",
        fix: "每个引擎只保留一次。",
      });
    }
    for (const engineId of preset.engine_ids) {
      if (!knownEngineIds.has(engineId)) {
        issues.push({
          code: "custom_preset_engine_unknown",
          severity: "warning",
          field: `${field}.engine_ids`,
          message: `引擎“${engineId}”已未知或可能下架。`,
          fix: "在目标机重新选择受支持的引擎；运行前预检会阻止误执行。",
        });
      }
    }
    if (preset.fusion_mode === "llm" && preset.engine_ids.length < 2) {
      issues.push({
        code: "custom_preset_fusion_without_comparison",
        severity: "warning",
        field: `${field}.fusion_mode`,
        message: "单引擎组合无需执行大模型融合。",
        fix: "改用手动/自动整理，或至少选择两个引擎。",
      });
    }
    if (!supportedSceneIds.has(preset.scene_id)) {
      issues.push({
        code: "custom_preset_scene_unknown",
        severity: "warning",
        field: `${field}.scene_id`,
        message: `场景“${preset.scene_id}”已未知。`,
        fix: "在目标机改为自动判断或重新选择现有场景。",
      });
    }
    if (!supportedLanguages.has(preset.language)) {
      issues.push({
        code: "custom_preset_language_unknown",
        severity: "warning",
        field: `${field}.language`,
        message: `语言“${preset.language}”已未知。`,
        fix: "在目标机改为自动检测或重新选择支持的语言。",
      });
    }
  }

  if (config.enabled_preset_ids.length === 0 && config.custom_presets.length === 0) {
    issues.push({
      code: "no_enabled_presets",
      severity: "warning",
      field: "enabled_preset_ids",
      message: "没有启用任何 ASR 方案。",
      fix: "至少启用一个本地或云端预设。",
    });
  }
  const defaultPresetAvailable =
    config.enabled_preset_ids.includes(config.default_preset_id) ||
    customPresetIds.has(config.default_preset_id);
  if (!defaultPresetAvailable) {
    issues.push({
      code: "default_preset_disabled",
      severity: "error",
      field: "default_preset_id",
      message: "默认方案不在启用列表中。",
      fix: "启用该方案，或更换默认方案。",
    });
  }
  if (new Set(config.enabled_preset_ids).size !== config.enabled_preset_ids.length) {
    issues.push({
      code: "enabled_preset_duplicate",
      severity: "warning",
      field: "enabled_preset_ids",
      message: "启用列表存在重复方案。",
      fix: "移除重复 ID，保持导入导出结果稳定。",
    });
  }
  for (const presetId of config.enabled_preset_ids) {
    if (!builtInPresetIds.has(presetId) && !customPresetIds.has(presetId)) {
      issues.push({
        code: "enabled_preset_unknown",
        severity: "warning",
        field: "enabled_preset_ids",
        message: `启用方案“${presetId}”已未知或可能下架。`,
        fix: "从启用列表移除该 ID，或在目标机重新建立组合。",
      });
    }
  }

  for (const ref of config.provider_secret_refs) {
    if (looksLikeSecretValue(ref)) {
      issues.push({
        code: "secret_value_in_config",
        severity: "error",
        field: "provider_secret_refs",
        message: "迁移配置疑似包含密钥明文。",
        fix: "只保留 Secrets 中的短名引用，不导出密钥值。",
      });
    } else if (!/^[A-Z][A-Z0-9_]{2,}$/.test(ref)) {
      issues.push({
        code: "secret_ref_invalid",
        severity: "warning",
        field: "provider_secret_refs",
        message: `密钥引用“${ref}”不符合统一短名规则。`,
        fix: "使用类似 OPENROUTER_API_KEY 的短名。",
      });
    }
  }

  if (!config.policies.keep_original_audio) {
    issues.push({
      code: "original_audio_not_retained",
      severity: "warning",
      field: "policies.keep_original_audio",
      message: "当前配置不保证保留原始录音。",
      fix: "生产环境应启用原始输入不可变留存。",
    });
  }
  if (!config.policies.require_preflight_before_download) {
    issues.push({
      code: "download_preflight_disabled",
      severity: "warning",
      field: "policies.require_preflight_before_download",
      message: "下载前预检已关闭。",
      fix: "启用空间、权限、网络和来源检查。",
    });
  }

  const hasErrors = issues.some((issue) => issue.severity === "error");
  const hasWarnings = issues.some((issue) => issue.severity === "warning");
  return {
    ready: !hasErrors,
    status: hasErrors ? "blocked" : hasWarnings ? "needs_attention" : "ready",
    issues,
  };
}

export function exportAsrPortableBundle(config: AsrPortableConfig): AsrPortableBundle {
  const roots = Object.fromEntries(
    (Object.keys(config.roots) as Array<keyof AsrPortableRoots>).map((key) => [
      key,
      ROOT_TOKEN_BY_KEY[key],
    ]),
  ) as Record<keyof AsrPortableRoots, string>;

  return {
    format: "homebase-asr-portable-profile",
    schema_version: ASR_PORTABLE_SCHEMA_VERSION,
    exported_at: new Date().toISOString(),
    source_hints: {
      os_family: "windows",
      profile_name: config.profile_name,
    },
    path_tokens: { ...config.roots },
    config: {
      ...config,
      roots,
      custom_presets: config.custom_presets.map((preset) => ({
        ...preset,
        engine_ids: [...preset.engine_ids],
      })),
      provider_secret_refs: [...config.provider_secret_refs],
    },
    migration_checks: [
      "Rebind every path token on the target machine.",
      "Verify model/runtime/output/cache roots are writable and have enough free space.",
      "Re-register API secrets by short name; secret values are intentionally not exported.",
      "Verify or download model snapshots, then run one smoke transcription before enabling presets.",
    ],
  };
}

export function importAsrPortableBundle(
  value: unknown,
  bindings?: Partial<AsrPortableRoots>,
): AsrPortableConfig {
  if (!isRecord(value) || value.format !== "homebase-asr-portable-profile") {
    throw new Error("不是 HomeBase ASR 迁移配置。 ");
  }
  if (
    ![LEGACY_ASR_PORTABLE_SCHEMA_VERSION, ASR_PORTABLE_SCHEMA_VERSION].includes(
      value.schema_version as 1 | 2,
    ) ||
    !isRecord(value.config)
  ) {
    throw new Error("ASR 迁移配置版本不受支持。 ");
  }

  const rawConfig = value.config;
  if (!isRecord(rawConfig.roots)) throw new Error("迁移配置缺少 roots。 ");
  const sourceTokens = isRecord(value.path_tokens) ? value.path_tokens : {};
  const roots = {} as AsrPortableRoots;
  for (const key of Object.keys(ROOT_TOKEN_BY_KEY) as Array<keyof AsrPortableRoots>) {
    const rebound = bindings?.[key];
    const hinted = sourceTokens[key];
    const raw = rawConfig.roots[key];
    roots[key] =
      typeof rebound === "string"
        ? rebound
        : typeof hinted === "string"
          ? hinted
          : typeof raw === "string"
            ? raw
            : "";
  }

  return {
    schema_version: ASR_PORTABLE_SCHEMA_VERSION,
    profile_id: stringField(rawConfig, "profile_id", "imported-asr-profile"),
    profile_name: stringField(rawConfig, "profile_name", "导入的 ASR 配置"),
    roots,
    catalog_rel_path: stringField(rawConfig, "catalog_rel_path", "capabilities/model-download-catalog.json"),
    manifest_rel_path: stringField(rawConfig, "manifest_rel_path", "models/installed-models.json"),
    preset_rel_path: stringField(rawConfig, "preset_rel_path", "capabilities/asr-presets.json"),
    default_preset_id: stringField(rawConfig, "default_preset_id", "asr_accuracy_max"),
    enabled_preset_ids: stringArrayField(rawConfig, "enabled_preset_ids"),
    custom_presets: parseCustomPresets(rawConfig.custom_presets),
    provider_secret_refs: stringArrayField(rawConfig, "provider_secret_refs"),
    policies: parsePolicies(rawConfig.policies),
  };
}

function parseCustomPresets(value: unknown): AsrPortableCustomPreset[] {
  if (!Array.isArray(value)) return [];
  return value.filter(isRecord).map((preset, index) => ({
    id: stringField(preset, "id", `custom_imported_${index + 1}`),
    name: stringField(preset, "name", `导入组合 ${index + 1}`),
    description: stringField(preset, "description", ""),
    engine_ids: stringArrayField(preset, "engine_ids"),
    fusion_mode: parseFusionMode(preset.fusion_mode),
    scene_id: stringField(preset, "scene_id", "auto"),
    language: stringField(preset, "language", "zh"),
  }));
}

function parseFusionMode(value: unknown): AsrPortableFusionMode {
  return value === "llm" || value === "manual" ? value : "auto";
}

function parsePolicies(value: unknown): AsrPortableConfig["policies"] {
  const input = isRecord(value) ? value : {};
  return {
    avoid_system_drive_for_models: booleanField(input, "avoid_system_drive_for_models", true),
    keep_original_audio: booleanField(input, "keep_original_audio", true),
    require_preflight_before_download: booleanField(input, "require_preflight_before_download", true),
    require_smoke_test_after_install: booleanField(input, "require_smoke_test_after_install", true),
  };
}

function isAbsoluteLocalPath(value: string) {
  return /^[A-Za-z]:[\\/]/.test(value) || /^\\\\[^\\]+\\[^\\]+/.test(value) || value.startsWith("/");
}

function hasUnsafePathSegment(value: string) {
  const withoutDrive = value.replace(/^[A-Za-z]:/, "");
  // Windows paths must reject ASCII control characters as well as reserved symbols.
  // eslint-disable-next-line no-control-regex
  if (/[<>"|?*\u0000-\u001f]/.test(withoutDrive)) return true;
  const reserved = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i;
  return withoutDrive.split(/[\\/]+/).some((segment) => reserved.test(segment));
}

function normalizePath(value: string) {
  return value.trim().replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
}

function isWithin(child: string, parent: string) {
  return child === parent || child.startsWith(`${parent}\\`);
}

function extractWindowsDrive(value: string) {
  const match = value.trim().match(/^([A-Za-z]):/);
  return match ? `${match[1].toUpperCase()}:` : null;
}

function normalizeDrive(value: string) {
  return value.trim().replace(/\\+$/, "").toUpperCase();
}

function looksLikeSecretValue(value: string) {
  return /^(sk-|or-v1-|dg_|xi_|api[-_]?key[-_:])/i.test(value) || value.length > 80;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function stringField(value: Record<string, unknown>, key: string, fallback: string) {
  return typeof value[key] === "string" ? value[key] : fallback;
}

function stringArrayField(value: Record<string, unknown>, key: string) {
  return Array.isArray(value[key]) ? value[key].filter((item): item is string => typeof item === "string") : [];
}

function booleanField(value: Record<string, unknown>, key: string, fallback: boolean) {
  return typeof value[key] === "boolean" ? value[key] : fallback;
}
