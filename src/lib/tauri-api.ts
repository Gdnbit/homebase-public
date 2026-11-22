/**
 * Tauri commands 类型化封装（v3 §8.1 + §20.3）
 *
 * 命名规则：每个模块一个 namespace 对象，方法名对齐 Rust command 名。
 */
import { invoke } from "@tauri-apps/api/core";

export interface ServiceCredentialStatus {
  service_id: string;
  name: string;
  credential_ref: string;
  state: "linked" | "not_configured" | "blocked";
  reason: string;
  message: string;
  region: string | null;
  backend_route: string;
  remote_state: "not_checked";
  wrapper_cli_version: string | null;
}
export const serviceCredentialsApi = {
  status: () => invoke<ServiceCredentialStatus[]>("service_credentials_status"),
};

// ============== Secrets ==============
/** 密钥分类（v3 §6.3 + key-naming-convention.md）*/
export type SecretCategory =
  | "core" // Core 5：MCP 已映射，日常使用
  | "future_ready" // 占位，待 SOPS 加入后激活
  | "gdnbit_project" // GDNbit 项目 5 把
  | "unknown"; // 出现在 secrets.yaml 但未注册到 meta 表

export interface SecretInfo {
  short_name: string;
  mcp_long_name: string | null;
  used_by: string[];
  category: SecretCategory;
  last_health: HealthStatus | null;
  expires_at: string | null;
  /** 用户自定义显示名（secrets-meta.json 的 display_name）；
   *  前端优先用此名展示，缺失时 fallback service-registry */
  display_name: string | null;
  /** 掩码值；未装的 key 显示 "(missing)" */
  masked_value: string;
  has_health_check: boolean;
  rotate_url: string | null;
  configured: boolean;
  value_state: "missing" | "empty" | "filled" | "invalid";
  revision: number;
  vault_version: string;
  service_id: string | null;
  expiry_policy: "warn" | "block";
  rotations: SecretRotation[];
}

export interface SecretRotation {
  id: string;
  changed_at: string;
  validation: string;
  revoked_at: string | null;
  rolled_back_at: string | null;
}
export interface SaveSecretRequest {
  short_name: string;
  expected_revision: number;
  expected_vault_version: string;
  value?: string | null;
  expires_at?: string | null;
  display_name?: string | null;
  service_id?: string | null;
  rotate_url?: string | null;
  expiry_policy?: "warn" | "block";
  operation?: "save" | "rotate" | "confirm_revocation" | "rollback_rotation";
  candidate_verified?: boolean;
  rotation_id?: string;
}
export interface SaveSecretResult {
  revision: number;
  vault_version: string;
  backup_path: string;
  verified: boolean;
}
export interface SecretLifecycleAlert {
  short_name: string;
  display_name: string;
  expires_at: string | null;
  expiry_policy: "warn" | "block";
  pending_revocation: boolean;
}

export interface HealthStatus {
  status: string;
  checked_at: string;
  response_ms: number | null;
  error: string | null;
}

export interface HealthResult {
  success: boolean;
  /** "valid" | "unauthorized" | "forbidden" | "not_found" | "network_error" | "no_check" | "unknown" */
  status: string;
  response_ms: number;
  http_code: number | null;
  error: string | null;
}

/** 写入结果（secrets_set 返回）— backup 路径便于失败回滚，verified 是 round-trip 验证 */
export interface SetResult {
  success: boolean;
  backup_path: string | null;
  verified: boolean;
  value_length: number;
  error: string | null;
}

/** 元数据写入结果（secrets_set_meta 返回）*/
export interface SetMetaResult {
  success: boolean;
  error: string | null;
}

export const secretsApi = {
  save: (request: SaveSecretRequest) => invoke<SaveSecretResult>("secrets_save", { request }),
  lifecycle: () => invoke<SecretLifecycleAlert[]>("secrets_lifecycle_status"),
  testCandidate: (serviceId: string, value: string) => invoke<HealthResult>("secrets_test_candidate", { serviceId, value }),
  list: () => invoke<SecretInfo[]>("secrets_list"),
  test: (shortName: string) =>
    invoke<HealthResult>("secrets_test", { shortName }),
  rotateUrl: (shortName: string) =>
    invoke<string | null>("secrets_rotate_url", { shortName }),
  /** 解密单 key 返回明文（前端 60s 内必须清 state） */
  reveal: (shortName: string) =>
    invoke<string>("secrets_reveal", { shortName }),
  /** 加密回写（backup + sops --set + round-trip verify） */
  set: (shortName: string, value: string) =>
    invoke<SetResult>("secrets_set", { shortName, value }),
  /** 写入 secrets-meta.json 的元数据（merge 语义）。
   *  - 参数 = `null` → 不动该字段（保留旧值）
   *  - 参数 = `""` → 清空该字段
   *  - 参数 = `"xxx"` → 设值 */
  setMeta: (
    shortName: string,
    expiresAt: string | null,
    displayName: string | null = null,
  ) =>
    invoke<SetMetaResult>("secrets_set_meta", {
      shortName,
      expiresAt,
      displayName,
    }),
};

// ============== Agents ==============

/** Agent 状态 4 态语义（v3 §6.2）— 用于颜色徽章
 *  - running: 进程在跑（绿）
 *  - stopped: 已配置但未启动（灰）
 *  - misconfigured: 配置缺失但 agent 装了（黄）
 *  - not_installed: agent 完全没装（红）
 */
export type AgentState = "running" | "stopped" | "misconfigured" | "not_installed";

export interface AgentInfo {
  id: string;
  name: string;
  kind: string;
  config_path: string | null;
  config_exists: boolean;
  /** agent-env.ps1 子命令（如 "claude"）；null 表示 agent 不通过 agent-env 启动 */
  launch_subcommand: string | null;
  /** 进程名匹配关键字 */
  process_match: string[];
}

export interface AgentStatus {
  id: string;
  state: AgentState;
  running: boolean;
  pid: number | null;
  config_exists: boolean;
  config_path: string | null;
  mcp_servers: McpServerStatus[];
  last_seen: string | null;
  error: string | null;
}

export interface McpServerStatus {
  name: string;
  healthy: boolean;
}

export interface ModelPreset {
  /** 实际写入 config 的字符串值 */
  value: string;
  /** UI 显示名 */
  label: string;
  /** 一句话特征 */
  hint: string;
}

export interface AgentModelInfo {
  agent_id: string;
  /** 当前模型；config 文件不存在或字段缺失时 null */
  current: string | null;
  /** 该 backend 支持的预设模型选项 */
  presets: ModelPreset[];
  /** 是否支持模型修改（false = openclaw/hermes 等 disabled） */
  supported: boolean;
  /** 不支持时的原因 */
  unsupported_reason: string | null;
}

// ---- Endpoint Config（Phase B · 2026-05-04）------
// 当前仅 claude-code 支持。写到 ~/.claude/settings.json 的 env 块（不污染 system env）

/** Provider 渠道 */
export type EndpointProvider =
  | "anthropic-direct"
  | "openrouter"
  | "custom"
  | "unset";

export interface EndpointConfig {
  provider: EndpointProvider | string;
  base_url: string | null;
  /**
   * SOPS secret 引用（短名）。前端只见到这个；明文不进 schema。
   * Phase F（2026-05-05）：取代旧的 `auth_token` 明文字段。
   */
  secret_short_name: string | null;
  /** 写入 settings.json env 块时使用的变量名 */
  auth_token_var: string;
  /** alias → 具体 model ID */
  default_opus: string | null;
  default_sonnet: string | null;
  default_haiku: string | null;
}

export interface ModelIdEntry {
  id: string;
  label: string;
  context: string; // "1M" / "200K"
  pricing: string; // "$5 / $25"
  tier: "opus" | "sonnet" | "haiku" | string;
}

export interface EndpointPresets {
  openrouter_anthropic: ModelIdEntry[];
}

/** 单个 LLM-capable secret 的前端展示元数据（明文不在此 struct） */
export interface AvailableSecret {
  short_name: string;
  /** 用户友好显示名（如 "OpenRouter（推荐）"） */
  display_label: string;
  /** 推荐 provider */
  suggested_provider: EndpointProvider | string;
  /** 推荐写入的 env 变量名 */
  suggested_token_var: string;
  suggested_base_url: string | null;
  /** SOPS yaml 中是否实际存在该 short_name（false = 注册了但未导入值） */
  present: boolean;
}

export interface AgentEndpointInfo {
  agent_id: string;
  supported: boolean;
  unsupported_reason: string | null;
  current: EndpointConfig;
  /** settings.json 中检测到的 token 是 SOPS 之外的（用户手动粘贴 / 旧值残留） */
  external_token_detected: boolean;
  /**
   * 当前 settings.json 中 token 的 masked 预览：
   * - external 时为 "abc...****...xyz"
   * - SOPS 命中时统一为 "(via SOPS)"
   * - 无值时为空字符串
   */
  current_token_masked: string;
  presets: EndpointPresets;
  /** 可用的 LLM secret 列表（前端 dropdown 用） */
  available_secrets: AvailableSecret[];
}

export interface EndpointTestResult {
  ok: boolean;
  status_code: number | null;
  latency_ms: number;
  error: string | null;
  body_preview: string | null;
}

export const agentsApi = {
  list: () => invoke<AgentInfo[]>("agents_list"),
  status: (id: string) => invoke<AgentStatus>("agents_status", { id }),
  /** 一键启动：在新 PowerShell 窗口跑 `agent-env <subcommand>` */
  start: (id: string) => invoke<void>("agents_start", { id }),
  /** 用 OS 默认编辑器打开 agent 配置文件 */
  openConfig: (id: string) => invoke<void>("agents_open_config", { id }),
  /** 读 agent 配置文件中的 model 字段 + 该 backend 已知 preset */
  getModel: (id: string) => invoke<AgentModelInfo>("agents_get_model", { id }),
  /** 写回 model 字段（保留其他字段）；返回新的 AgentModelInfo */
  setModel: (id: string, model: string) =>
    invoke<AgentModelInfo>("agents_set_model", { id, model }),
  /** 读 ~/.claude/settings.json 的 env 块 + 内嵌模型 ID 预设清单 */
  getEndpoint: (id: string) =>
    invoke<AgentEndpointInfo>("agents_get_endpoint", { id }),
  /** 写 ~/.claude/settings.json 的 env 块（保留非 ANTHROPIC_ 前缀字段） */
  setEndpoint: (id: string, config: EndpointConfig) =>
    invoke<AgentEndpointInfo>("agents_set_endpoint", { id, config }),
  /** 调 base_url + auth_token 发 1 条最小 messages 请求探测连通 */
  testEndpoint: (id: string, config: EndpointConfig) =>
    invoke<EndpointTestResult>("agents_test_endpoint", { id, config }),
  /** 列出 agent 历史会话（Phase D） */
  historyList: (id: string) =>
    invoke<HistoryListResult>("agents_history_list", { id }),
  /** 读单个会话完整消息流 */
  historyRead: async (id: string, sessionId: string) => {
    const result = await invoke<HistorySession>("agents_history_read", { id, sessionId });
    if (!result || result.agent_id !== id || result.session_id !== sessionId) {
      throw new Error("原对话标识不匹配，请重试。");
    }
    if (
      !Array.isArray(result.messages) ||
      result.messages.some(
        (message) => !message || typeof message.role !== "string" || typeof message.content !== "string",
      )
    ) {
      throw new Error("原对话消息格式无效，请重试。");
    }
    return result;
  },
  /** Phase H · 跨 agent 历史总览（按 last_active_at 倒序）*/
  historyAll: () => invoke<HistoryAllResult>("agents_history_all"),
};

// ============== OpenRouter（2026-06-07 · API 模型目录）==============

export interface OpenRouterModelOption {
  id: string;
  name: string;
  context_length: number | null;
  input_modalities: string[];
  output_modalities: string[];
  supported_parameters: string[];
  prompt_price_per_m: number | null;
  completion_price_per_m: number | null;
  image_price: number | null;
  request_price: number | null;
  price_label: string;
  capability_score: number;
}

export interface OpenRouterModelGroup {
  provider: string;
  label: string;
  models: OpenRouterModelOption[];
}

export interface OpenRouterModelCatalog {
  fetched_at: string;
  source_url: string;
  groups: OpenRouterModelGroup[];
}

export const openrouterApi = {
  modelsCatalog: async () => {
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      return invoke<OpenRouterModelCatalog>("openrouter_models_catalog");
    }
    return fetchOpenRouterModelCatalog();
  },
};

interface OpenRouterRawModelsResponse {
  data: OpenRouterRawModel[];
}

interface OpenRouterRawModel {
  id: string;
  name: string;
  context_length?: number | null;
  architecture?: {
    input_modalities?: string[] | null;
    output_modalities?: string[] | null;
  } | null;
  pricing?: {
    prompt?: string | null;
    completion?: string | null;
    image?: string | null;
    request?: string | null;
  } | null;
  supported_parameters?: string[] | null;
}

async function fetchOpenRouterModelCatalog(): Promise<OpenRouterModelCatalog> {
  const sourceUrl = "https://openrouter.ai/api/v1/models?output_modalities=all";
  const response = await fetch(sourceUrl);
  if (!response.ok) {
    throw new Error(`OpenRouter 模型列表返回 HTTP ${response.status}`);
  }
  const raw = (await response.json()) as OpenRouterRawModelsResponse;
  const groups = new Map<string, OpenRouterModelOption[]>();

  for (const model of raw.data) {
    const provider = model.id.split("/")[0] || "unknown";
    const option = openRouterRawModelToOption(model);
    groups.set(provider, [...(groups.get(provider) ?? []), option]);
  }

  return {
    fetched_at: new Date().toISOString(),
    source_url: sourceUrl,
    groups: Array.from(groups.entries())
      .map(([provider, models]) => ({
        provider,
        label: openRouterProviderLabel(provider),
        models: models.sort(openRouterModelSort),
      }))
      .sort((a, b) => {
        const rankDelta = openRouterProviderRank(a.provider) - openRouterProviderRank(b.provider);
        return rankDelta || a.label.localeCompare(b.label);
      }),
  };
}

function openRouterRawModelToOption(model: OpenRouterRawModel): OpenRouterModelOption {
  const inputModalities = model.architecture?.input_modalities ?? [];
  const outputModalities = model.architecture?.output_modalities ?? [];
  const supportedParameters = model.supported_parameters ?? [];
  const promptPricePerM = parseOpenRouterPricePerM(model.pricing?.prompt);
  const completionPricePerM = parseOpenRouterPricePerM(model.pricing?.completion);
  const imagePrice = parseOpenRouterPrice(model.pricing?.image);
  const requestPrice = parseOpenRouterPrice(model.pricing?.request);

  return {
    id: model.id,
    name: model.name,
    context_length: model.context_length ?? null,
    input_modalities: inputModalities,
    output_modalities: outputModalities,
    supported_parameters: supportedParameters,
    prompt_price_per_m: promptPricePerM,
    completion_price_per_m: completionPricePerM,
    image_price: imagePrice,
    request_price: requestPrice,
    price_label: openRouterPriceLabel(promptPricePerM, completionPricePerM, imagePrice, requestPrice),
    capability_score: openRouterCapabilityScore(
      model.id,
      model.name,
      model.context_length ?? null,
      inputModalities,
      outputModalities,
      supportedParameters,
    ),
  };
}

function parseOpenRouterPrice(value?: string | null): number | null {
  if (value == null) return null;
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : null;
}

function parseOpenRouterPricePerM(value?: string | null): number | null {
  const parsed = parseOpenRouterPrice(value);
  return parsed == null ? null : parsed * 1_000_000;
}

function openRouterPriceLabel(
  prompt: number | null,
  completion: number | null,
  image: number | null,
  request: number | null,
): string {
  if (image != null && image > 0) return `image $${image.toFixed(4)}`;
  if (request != null && request > 0) return `request $${request.toFixed(4)}`;
  if (prompt === 0 && completion === 0) return "free";
  if (prompt != null && completion != null) return `$${prompt.toFixed(3)}/$${completion.toFixed(3)} per 1M`;
  if (prompt != null) return `input $${prompt.toFixed(3)} per 1M`;
  if (completion != null) return `output $${completion.toFixed(3)} per 1M`;
  return "price n/a";
}

function openRouterProviderLabel(provider: string): string {
  return (
    {
      openai: "OpenAI",
      anthropic: "Anthropic",
      google: "Google",
      "meta-llama": "Meta",
      mistralai: "Mistral",
      qwen: "Qwen",
      deepseek: "DeepSeek",
      "x-ai": "xAI",
      cohere: "Cohere",
      perplexity: "Perplexity",
      microsoft: "Microsoft",
      nvidia: "NVIDIA",
      minimax: "MiniMax",
      recraft: "Recraft",
      sourceful: "Sourceful",
      stepfun: "StepFun",
      "z-ai": "Z.ai",
    }[provider] ?? provider
  );
}

function openRouterProviderRank(provider: string): number {
  return (
    {
      openai: 0,
      anthropic: 1,
      google: 2,
      "x-ai": 3,
      mistralai: 4,
      qwen: 5,
      deepseek: 6,
      "meta-llama": 7,
      microsoft: 8,
      nvidia: 9,
    }[provider] ?? 100
  );
}

function openRouterModelSort(a: OpenRouterModelOption, b: OpenRouterModelOption): number {
  return (
    b.capability_score - a.capability_score ||
    (b.context_length ?? 0) - (a.context_length ?? 0) ||
    a.name.localeCompare(b.name)
  );
}

function openRouterCapabilityScore(
  id: string,
  name: string,
  contextLength: number | null,
  inputModalities: string[],
  outputModalities: string[],
  supportedParameters: string[],
): number {
  const haystack = `${id} ${name}`.toLowerCase();
  let score = 0;
  const terms: Array<[string, number]> = [
    ["opus", 100],
    ["gpt-5", 98],
    ["gpt-4.5", 94],
    ["gemini-3", 94],
    ["pro", 85],
    ["sonnet", 82],
    ["grok", 78],
    ["reasoning", 72],
    ["plus", 68],
    ["max", 65],
    ["medium", 60],
    ["flash", 45],
    ["mini", 35],
    ["nano", 25],
    ["free", -10],
  ];

  for (const [term, points] of terms) {
    if (haystack.includes(term)) score += points;
  }
  if (inputModalities.includes("image")) score += 8;
  if (inputModalities.includes("audio")) score += 8;
  if (inputModalities.includes("video")) score += 10;
  if (outputModalities.includes("image")) score += 7;
  if (outputModalities.includes("video")) score += 9;
  if (supportedParameters.includes("tools")) score += 5;
  if (supportedParameters.includes("response_format")) score += 5;
  if (contextLength != null) {
    if (contextLength >= 1_000_000) score += 12;
    else if (contextLength >= 200_000) score += 8;
    else if (contextLength >= 100_000) score += 4;
  }
  return score;
}

// ---- Agent History（Phase D · 2026-05-04）--------------

export interface HistoryEntry {
  agent_id: string;
  session_id: string;
  /** Claude / Codex 有 cwd（Phase H 后两者都有）；Gemini CLI / OpenClaw / Hermes 无 */
  cwd: string | null;
  first_user_prompt: string | null;
  /** ISO8601 UTC */
  started_at: string;
  last_active_at: string;
  message_count: number;
  file_path: string;
}

/** Phase H · 跨 agent 聚合 · 单 agent 状态桌 */
export interface AgentBucket {
  agent_id: string;
  supported: boolean;
  unsupported_reason: string | null;
  entry_count: number;
  data_root: string;
}

/** Phase H · 跨 agent 聚合返回 */
export interface HistoryAllResult {
  /** 所有 agent 的会话合并后按 last_active_at 倒序 */
  entries: HistoryEntry[];
  /** 5 个 agent 的状态桌（supported 与否 / 条目数 / 数据源路径）*/
  buckets: AgentBucket[];
}

export interface HistoryListResult {
  agent_id: string;
  supported: boolean;
  unsupported_reason: string | null;
  entries: HistoryEntry[];
  data_root: string;
}

export interface HistoryMessage {
  uuid: string;
  parent_uuid: string | null;
  /** user / assistant / system */
  role: string;
  /** 扁平化后的文本（tool_use / tool_result 已标记） */
  content: string;
  timestamp: string;
  model: string | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cache_read_tokens: number | null;
  cache_write_tokens: number | null;
}

export interface SessionStats {
  message_count: number;
  user_count: number;
  assistant_count: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cache_read_tokens: number;
  total_cache_write_tokens: number;
  duration_seconds: number;
}

export interface HistorySession {
  agent_id: string;
  session_id: string;
  cwd: string | null;
  git_branch: string | null;
  version: string | null;
  messages: HistoryMessage[];
  stats: SessionStats;
  file_path: string;
}

// ============== System ==============
export interface DiskInfo {
  drive: string;
  label: string | null;
  total_gb: number;
  free_gb: number;
  usage_percent: number;
  warning: boolean;
}
export type GpuStatus = "detected" | "not_detected" | "unknown";

export interface SystemInfo {
  issues?: { field: string; status: "unknown"; message: string }[];
  os: string;
  os_version: string;
  arch: string;
  hostname: string;
  username: string;
  /** CPU 品牌全名 · 如 "13th Gen Intel(R) Core(TM) i9-13900K" */
  cpu_brand?: string | null;
  /** 物理核心数 */
  cpu_physical_cores?: number | null;
  /** 逻辑核心数（线程数） */
  cpu_logical_cores?: number | null;
  /** 内存总量 GB */
  memory_total_gb?: number | null;
  /** GPU 名称（过滤虚拟适配器后优先选独显） */
  gpu_name?: string | null;
  gpu_status?: GpuStatus;
  gpu_message?: string;
  /** 主板 / 整机制造商 */
  computer_manufacturer?: string | null;
  /** 整机型号 */
  computer_model?: string | null;
}

/** CPU + RAM 实时指标（system_metrics 命令；轮询 5s）*/
export interface SystemMetrics {
  cpu_percent: number;
  cpu_cores: number;
  memory_used_gb: number;
  memory_total_gb: number;
  memory_percent: number;
  cpu_warning: boolean;
  memory_warning: boolean;
}

/** GPU 利用率（system_gpu 命令；perf counter 慢，轮询 10s 单独拉）*/
export interface GpuMetrics {
  available: boolean;
  gpu_percent: number | null;
  gpu_name: string | null;
  gpu_status?: GpuStatus;
  gpu_message?: string;
  /** 已过滤远程/虚拟显示适配器的物理 GPU 列表 */
  gpu_names: string[];
  /** 适配器报告的专用显存；无法可靠获取时为 null */
  dedicated_memory_gb: number | null;
  gpu_warning: boolean;
  error: string | null;
}

/** Phase G-8 · 启动自检单项（SOPS 五件套 / homebase-reveal 其中一项）*/
export interface PreflightCheck {
  retryable?: boolean;
  /** 稳定 ID：sops_exe | secrets_yaml | age_key | reveal_exe | agent_env */
  id: string;
  /** 人类可读中文标签（如 "SOPS 可执行文件"）*/
  label: string;
  /** 是否通过 */
  ok: boolean;
  /** "critical"（影响核心功能）/ "warning"（可选/降级可用） */
  severity: "critical" | "warning" | string;
  /** 成功时可带补充（版本号/路径） */
  detail: string | null;
  /** 失败时的修复建议；成功项为 null */
  hint: string | null;
}

/** PATH 单项（system_path_list 命令；2026-05-23）*/
export interface PathEntry {
  /** 展开 %VAR% 后的绝对路径 */
  path: string;
  /** 原始值（含 %VAR% 占位符） */
  raw: string;
  /** 路径是否真实存在（false = 死链） */
  exists: boolean;
  /** 来源：user（HKCU）/ machine（HKLM） */
  source: "user" | "machine" | string;
}

/** 关键进程（system_processes 命令；2026-05-23）*/
export interface ProcessInfo {
  pid: number;
  name: string;
  display_name: string;
  /** homebase / agent / tool */
  category: "homebase" | "agent" | "tool" | string;
  cpu_percent: number;
  memory_mb: number;
  run_secs: number;
}

export const systemApi = {
  disks: () => invoke<DiskInfo[]>("system_disks"),
  info: () => invoke<SystemInfo>("system_info"),
  metrics: () => invoke<SystemMetrics>("system_metrics"),
  gpu: () => invoke<GpuMetrics>("system_gpu"),
  /** 启动时自检 SOPS 五件套（sops / secrets.yaml / age key / reveal / agent-env） */
  preflight: () => invoke<PreflightCheck[]>("system_preflight"),
  /** User + Machine 两级 PATH 项目，含死链检测 */
  pathList: () => invoke<PathEntry[]>("system_path_list"),
  /** HomeBase / 5 agent / sops 关键进程一览（含 CPU% RAM 运行秒） */
  processes: () => invoke<ProcessInfo[]>("system_processes"),
};

// ============== Distribution Runtime ==============

export interface DistributionRoots {
  aihubRoot: string;
  modelRoot: string;
  runtimeRoot: string;
  outputRoot: string;
  cacheRoot: string;
}

export interface DistributionInfo {
  schemaVersion: number;
  packageVersion: string;
  mode: "packaged" | "development";
  resourcesReady: boolean;
  dataRoot: string;
  resourceRoot: string;
  roots: DistributionRoots;
  pythonPath: string;
  ffmpegPath: string;
  revealPath: string;
  secretsPath: string;
  allowSystemDrive: boolean;
  initializedAt: string | null;
  initializationStatus: "known" | "missing" | "unreadable" | "invalid";
  warnings: string[];
}

export const distributionApi = {
  info: () => invoke<DistributionInfo>("distribution_info"),
};

// ============== ASR Control Plane ==============

export interface AsrEnvironmentPayload {
  sourcePath?: string | null;
  outputRoot?: string | null;
  presetId?: string | null;
  engineIds?: string[] | null;
  runnerPath?: string | null;
  pythonPath?: string | null;
  allowSystemDrive?: boolean;
}

export interface AsrEngineProbe {
  engine_id: string;
  status: "ready" | "blocked";
  reason: string | null;
  repair: string | null;
  model_path?: string;
  runtime_path?: string;
}

export interface AsrPreflightReport {
  status: "ready" | "blocked";
  preset_id: string;
  source: string | null;
  output_root: string;
  ready_engine_count: number;
  engine_count: number;
  engines: AsrEngineProbe[];
  blockers: Array<{ code: string; message: string }>;
  checked_at: string;
}

export interface AsrEnvironmentReport {
  available: boolean;
  checkedAt: string;
  pythonPath: string;
  runnerPath: string;
  outputRoot: string;
  report: AsrPreflightReport | null;
  error: string | null;
}

export interface AsrTaskStartPayload {
  sourcePath: string;
  outputRoot?: string | null;
  taskName?: string | null;
  presetId: string;
  engineIds: string[];
  language?: string | null;
  fusionMode?: "manual" | "llm" | "auto" | null;
  preprocessProfile?: string | null;
  runnerPath?: string | null;
  pythonPath?: string | null;
  allowSystemDrive?: boolean;
}

export interface AsrTaskStartResult {
  jobId: string;
  taskDir: string;
  jobStatePath: string;
  commandPreview: string;
  preflight: AsrPreflightReport;
}

export interface AsrTaskOrganizePayload {
  taskDir: string;
  outputRoot?: string | null;
  fusionMode?: "manual" | "llm" | "auto" | null;
  fusionModel?: string | null;
  runnerPath?: string | null;
  pythonPath?: string | null;
}

export interface TaskRuntimeObservation {
  /** Read-only, never written into a task record. Missing observations are unknown. */
  runtime_state?: "managed" | "unmanaged" | "terminal" | "unknown";
  runtime_message?: string;
}

export interface AsrJobRecord extends TaskRuntimeObservation {
  schemaVersion: number;
  jobId: string;
  status: string;
  sourcePath: string;
  outputRoot: string;
  taskDir: string;
  presetId: string;
  engineIds: string[];
  pid: number | null;
  createdAt: string;
  updatedAt: string;
  completedAt: string | null;
  exitCode: number | null;
  error: string | null;
}

export interface AsrTaskView {
  taskDir: string;
  taskMeta: Record<string, unknown>;
  job: AsrJobRecord | null;
  artifacts: AsrArtifactSummary[];
  preprocessMeta: Record<string, unknown> | null;
  runMatrix: Record<string, unknown> | null;
  fusionMeta: Record<string, unknown> | null;
  summaryMeta: Record<string, unknown> | null;
}

export interface AsrArtifactSummary {
  relativePath: string;
  absolutePath: string;
  category: "preprocess" | "runs" | "compare" | "final" | "exports" | string;
  engineId: string | null;
  sizeBytes: number;
}

export interface AsrArtifactContent {
  relativePath: string;
  absolutePath: string;
  content: string;
  sizeBytes: number;
}

export type AsrJobEvent =
  | { kind: "started"; jobId: string; taskDir: string; status: string }
  | { kind: "progress"; jobId: string; payload: Record<string, unknown> }
  | { kind: "diagnostic"; jobId: string; message: string }
  | {
      kind: "finished";
      jobId: string;
      taskDir: string;
      status: string;
      exitCode?: number | null;
      message?: string | null;
    };

export const asrApi = {
  probe: (payload: AsrEnvironmentPayload) =>
    invoke<AsrEnvironmentReport>("asr_environment_probe", { payload }),
  start: (payload: AsrTaskStartPayload) =>
    invoke<AsrTaskStartResult>("asr_task_start", { payload }),
  get: (taskDir: string, outputRoot: string, jobId?: string | null) =>
    invoke<AsrTaskView>("asr_task_get", { taskDir, outputRoot, jobId: jobId ?? null }),
  readArtifact: (taskDir: string, relativePath: string, outputRoot: string) =>
    invoke<AsrArtifactContent>("asr_task_read_artifact", { taskDir, relativePath, outputRoot }),
  organize: (payload: AsrTaskOrganizePayload) =>
    invoke<AsrTaskView>("asr_task_organize", { payload }),
  list: (outputRoot: string, limit?: number | null) =>
    invoke<AsrTaskView[]>("asr_task_list", { outputRoot, limit: limit ?? null }),
  reconcile: (outputRoot: string) =>
    invoke<AsrJobRecord[]>("asr_task_reconcile", { outputRoot }),
  cancel: (jobId: string) => invoke<void>("asr_task_cancel", { jobId }),
};

// ============== Model Center ==============

export type ModelManifestState = "loaded" | "missing" | "invalid" | "unavailable";
export type ModelInstallState = "installed" | "probed" | "partial" | "corrupt";

export interface ModelInstallProbe {
  id: string;
  state: ModelInstallState;
  local_path: string | null;
  path_exists: boolean | null;
  detail: string | null;
}

export interface ModelManifestProbe {
  source_path: string;
  manifest_state: ModelManifestState;
  schema_version: number | null;
  updated_at: string | null;
  models: ModelInstallProbe[];
  error: string | null;
}

export interface ModelDownloadStartPayload {
  modelId: string;
  managerPath?: string | null;
  pythonPath?: string | null;
  catalogPath?: string | null;
  manifestPath?: string | null;
  jobsRoot?: string | null;
  modelRoot?: string | null;
  allowLarge?: boolean;
  allowSystemDrive?: boolean;
}

export interface ModelDownloadStartResult {
  jobId: string;
  statePath: string;
  targetRoot: string;
  commandPreview: string;
}

export interface ModelDownloadJob extends TaskRuntimeObservation {
  schema_version: number;
  job_id: string;
  model_id: string;
  model_name: string;
  status: string;
  phase: string;
  progress_percent: number;
  files_completed: number;
  files_total: number | null;
  bytes_expected: number | null;
  target_dir: string;
  current_file?: string | null;
  created_at: string;
  updated_at: string;
  completed_at?: string | null;
  revision: number;
  error: string | null;
  manifest_record_status?: string;
}

export type ModelDownloadEvent =
  | { kind: "started"; jobId: string; modelId: string }
  | { kind: "progress"; jobId: string; payload: Record<string, unknown> }
  | { kind: "diagnostic"; jobId: string; message: string }
  | {
      kind: "finished";
      jobId: string;
      status: string;
      exitCode?: number | null;
      message?: string | null;
    };

export const modelCenterApi = {
  manifest: (manifestPath: string) =>
    invoke<ModelManifestProbe>("model_center_manifest", { manifestPath }),
  downloadStart: (payload: ModelDownloadStartPayload) =>
    invoke<ModelDownloadStartResult>("model_download_start", { payload }),
  downloadGet: (jobsRoot: string, jobId: string) =>
    invoke<ModelDownloadJob>("model_download_get", { jobsRoot, jobId }),
  downloadList: (jobsRoot: string, limit?: number | null) =>
    invoke<ModelDownloadJob[]>("model_download_list", { jobsRoot, limit: limit ?? null }),
  downloadReconcile: (jobsRoot: string) =>
    invoke<ModelDownloadJob[]>("model_download_reconcile", { jobsRoot }),
  downloadCancel: (jobId: string) => invoke<void>("model_download_cancel", { jobId }),
};

// ============== Docs ==============
export interface DocInfo {
  doc_id: string;
  path: string;
  role: string | null;
  updated: string | null;
  starred: boolean;
  is_new: boolean;
}

/** Obsidian vault 4 态（v3 §22.3）— 用于诊断 Q11 实测 B' 变体 */
export type ObsidianVariant = "healthy" | "not_initialized" | "dangling" | "not_a_vault";

export interface ObsidianVaultInfo {
  id: string;
  path: string;
  exists: boolean;
  is_obsidian: boolean;
  open: boolean;
  matches_expected: boolean;
}

export interface ObsidianHealth {
  config_path: string;
  config_exists: boolean;
  vaults: ObsidianVaultInfo[];
  expected_vault_path: string;
  expected_vault_exists: boolean;
  expected_vault_is_obsidian: boolean;
  variant: ObsidianVariant;
  recommendation: string;
}

/** 单篇文档全文（docs_read 返回） */
export interface DocContent {
  doc_id: string;
  /** INDEX.md 中登记的原始 path */
  path: string;
  /** 解析后绝对路径（用于「在编辑器中打开」） */
  absolute_path: string;
  exists: boolean;
  /** 字节大小 */
  size: number;
  /** 文件全文；缺失 / 过大 / 后缀不对时为 null */
  content: string | null;
  /** 不可读时的原因 */
  error: string | null;
}

export const docsApi = {
  list: () => invoke<DocInfo[]>("docs_list"),
  /** 读取单篇文档全文（仅 .md / .markdown / .txt，≤ 5 MB） */
  read: (docId: string) => invoke<DocContent>("docs_read", { docId }),
  obsidianHealth: () => invoke<ObsidianHealth>("obsidian_health"),
};

// ============== LLM Wiki（Phase 14.5）==============

export type WikiKind =
  | "root"
  | "raw"
  | "note"
  | "moc"
  | "journal"
  | "project"
  | "template"
  | "derived_index"
  | "meta"
  | "wiki_other"
  | "other";

/** Wiki 文档元数据（递归扫 C:\HomeBase\Wiki\ 后的每条结果） */
export interface WikiDoc {
  /** 相对 C:\HomeBase\Wiki 的 POSIX 路径，如 "wiki/notes/llm/transformers.md" */
  rel_path: string;
  /** 绝对路径（Windows 反斜杠） */
  absolute_path: string;
  kind: WikiKind;
  /** frontmatter title > 第一个 # heading > 文件名 */
  title: string;
  /** frontmatter doc_id（如有） */
  doc_id: string | null;
  size: number;
  /** Unix 毫秒时间戳，前端可排序「最近修改」 */
  modified_ms: number;
}

export interface WikiContent {
  rel_path: string;
  absolute_path: string;
  exists: boolean;
  size: number;
  content: string | null;
  error: string | null;
}

export type TyporaSource = "typora" | "fallback_shell";

export interface TyporaResult {
  opened_with: TyporaSource;
  /** 实际用的 Typora.exe 路径（typora 时） */
  path_used: string | null;
}

export const wikiApi = {
  /** 递归扫 C:\HomeBase\Wiki，列出所有 .md / .markdown / .txt */
  list: () => invoke<WikiDoc[]>("wiki_list"),
  /**
   * 递归列出 C:\HomeBase\Wiki 下所有目录（**含空骨架**）的 POSIX 相对路径
   *
   * 跟 list() 互补：list() 只返回有 markdown 的目录，dirs() 返回全部目录
   * 让前端 sidebar 能显示 wiki/notes/llm 等 LLM 待填充的骨架
   */
  dirs: () => invoke<string[]>("wiki_dirs"),
  /** 读单篇 wiki 文档（rel_path 必须在 C:\HomeBase\Wiki 子树内） */
  read: (relPath: string) =>
    invoke<WikiContent>("wiki_read", { relPath }),
  /**
   * 用 Typora 打开 markdown（候选路径 C:\HomeBase\Tools\Typora\Typora.exe → Program Files → fallback shell）
   *
   * 若返回 `opened_with: "fallback_shell"`，前端应继续用 `@tauri-apps/plugin-shell` 的 `open()`
   * 让 OS 默认应用打开
   */
  openInTypora: (absolutePath: string) =>
    invoke<TyporaResult>("docs_open_in_typora", { absolutePath }),
};

// ============== Topology（Phase 14.5b — AI 知识库磁盘架构图）==============

export type TopologySource = "default" | "user_config";

export interface TopologyRoot {
  id: string;
  label: string;
  absolute_path: string;
  note: string | null;
  /** subdir name → 中文 label override（用户在 topology.json 里配） */
  child_labels: Record<string, string>;
  /** 运行时校验的存在性 */
  exists: boolean;
}

export interface TopologyEdge {
  source: string;
  target: string;
  label: string | null;
}

/**
 * 磁盘虚拟分组（方案 A · v2 新增）
 *
 * 比如 E 盘下配：
 * ```json
 * { "id": "e-media", "label": "影视", "icon": "film", "paths": ["E:\\电影", "E:\\电视剧"] }
 * ```
 *
 * 展开 drive_e 时就显示「影视」等抽象分组节点，再点「影视」才展开到物理目录
 */
export interface VirtualGroup {
  id: string;
  label: string;
  icon: string | null;
  note: string | null;
  paths: string[];
}

export interface Topology {
  version: number;
  roots: TopologyRoot[];
  edges: TopologyEdge[];
  /**
   * 磁盘虚拟分组（v2）
   *
   * key = root.id（如 `drive_e`），value = 虚拟组列表
   * 配了就用虚拟组展开；没配则回退到 `topology_list_dir` 直接列物理目录
   */
  drive_groups: Record<string, VirtualGroup[]>;
  source: TopologySource;
  /** ~/.homebase/topology.json 的绝对路径，前端可显示「编辑配置」按钮 */
  config_path: string;
}

export interface TopologyDirEntry {
  name: string;
  absolute_path: string;
  is_dir: boolean;
}

export interface TopologyRefEdge {
  source_path: string;
  /** 目标若是 root.id（无路径分隔符） */
  target_id: string | null;
  /** 目标若是路径 */
  target_path: string | null;
  label: string | null;
}

export const topologyApi = {
  /** 加载 topology（用户配置 > 默认硬编码） */
  load: () => invoke<Topology>("topology_load"),
  /** 列出某绝对路径下的子条目（仅一级；子目录 + 顶级关键 markdown） */
  listDir: (absolutePath: string) =>
    invoke<TopologyDirEntry[]>("topology_list_dir", { absolutePath }),
  /** OS 文件管理器打开（explorer.exe / open / xdg-open） */
  openDir: (absolutePath: string) =>
    invoke<void>("topology_open_dir", { absolutePath }),
  /** 解析某目录下 INDEX.md / README.md frontmatter 的 references 字段 */
  parseRefs: (absolutePath: string) =>
    invoke<TopologyRefEdge[]>("topology_parse_refs", { absolutePath }),
};

// ============== Inbox（Phase 14.5c — 投料口：raw 层低门槛输入）==============

export interface InboxSubmitPayload {
  text: string;
  /** 不填则用首行前 40 字符 */
  title?: string;
  /** 默认会自动加 `inbox` / `raw` 两个 tag，这里写额外的 */
  extra_tags?: string[];
  /** 来源标识：`web` / `mobile-memo` / `weibo` / `chat` 等 */
  source_hint?: string;
  /** 原样使用上传返回的完整库内路径；无日期旧引用不能确定文件 */
  attachments?: string[];
  /** 单次意图的稳定UUID；恢复使用原编号及原payload，不换编号盲重做 */
  submission_id?: string;
}

export interface InboxSubmitResult {
  /** 写入的绝对路径 */
  path: string;
  /** 文件名（HHmmss-shortid.md） */
  filename: string;
  /** 归属 daily 目录 YYYY-MM-DD */
  day: string;
  /** 字符数 */
  char_count: number;
}

export interface InboxSubmissionView {
  submission_id: string;
  state: "not_found" | "prepared" | "unknown" | "completed" | "canceled";
  file_state: "present" | "modified" | "missing" | "unreadable" | null;
  result: InboxSubmitResult | null;
  message: string;
}

export interface InboxOpenObsidianResult {
  opened_with: string;
  vault: string | null;
}

export interface InboxRecentEntry {
  path: string;
  filename: string;
  day: string;
  preview: string;
  size: number;
  modified_at: string;
}

export interface InboxAttachPayload {
  filename: string;
  /** 文件字节（前端 `Array.from(new Uint8Array(buffer))` 转成 number[]） */
  bytes: number[];
  /** 归属哪一天的目录；不填用今天 */
  day?: string;
}

export interface InboxAttachResult {
  absolute_path: string;
  /** 完整库内路径 raw/inbox/YYYY-MM-DD/_attachments/文件名；不是 OS 路径 */
  relative_path: string;
  final_filename: string;
  size: number;
}

export const inboxApi = {
  /** 只查询原业务意图，不发布笔记；not_found不等于未执行 */
  submissionGet: (submissionId: string) =>
    invoke<InboxSubmissionView>("inbox_submission_get", { submissionId }),
  /** 只取消尚未越过发布边界的原意图，不删除笔记 */
  submissionCancel: (submissionId: string) =>
    invoke<InboxSubmissionView>("inbox_submission_cancel", { submissionId }),
  /** 提交一条投料 → 落到 C:\HomeBase\Wiki\raw\inbox\YYYY-MM-DD\HHmmss-xxxx.md */
  submit: (payload: InboxSubmitPayload) =>
    invoke<InboxSubmitResult>("inbox_submit", { payload }),
  /** 上传附件 → 落到 raw/inbox/<day>/_attachments/，返回相对路径供 submit 引用 */
  attach: (payload: InboxAttachPayload) =>
    invoke<InboxAttachResult>("inbox_attach", { payload }),
  /** 打开 Obsidian 应用（用 obsidian://open?vault=... URI scheme） */
  openObsidian: (vault?: string) =>
    invoke<InboxOpenObsidianResult>("inbox_open_obsidian", { vault: vault ?? null }),
  /** 列最近 N 条投料（默认 10，最多 50），按修改时间倒序 */
  listRecent: (limit?: number) =>
    invoke<InboxRecentEntry[]>("inbox_list_recent", { limit: limit ?? null }),
};

// ============== Wiki Agent（Phase 14.5c — 方案 A · v1 stub）==============

export interface WikiAgentStatus {
  /** 未消化的 raw/inbox 文件数（frontmatter 没 digested_at） */
  pending_count: number;
  /** 已消化文件数 */
  digested_count: number;
  total_count: number;
  /** 最后一条 inbox 文件的修改时间（YYYY-MM-DD HH:MM:SS） */
  last_inbox_at: string | null;
  vault_root: string;
  has_agents_md: boolean;
  /** 默认底层 backend（v1 固定 claude-code，v2 从 settings 读） */
  backend: string;
}

export interface WikiAgentPreparePromptPayload {
  /** 只针对某一天 raw/inbox/<day>/ 的未消化文件 */
  day?: string;
  /** 只针对特定 raw 路径列表 */
  paths?: string[];
}

export interface WikiAgentPreparePromptResult {
  /** 完整 prompt（可粘贴到 Claude Code / Codex） */
  prompt: string;
  /** 涉及的 raw 文件路径列表（绝对路径） */
  target_files: string[];
  /** 提示中提到的 schema 文档路径（AGENTS.md） */
  schema_path: string;
}

export interface WikiAgentConfig {
  /** 底层 AI backend：claude-code / codex / gemini-cli / hermes */
  backend: string;
  /** 是否启用自动消化（v1 只存，不真的跑） */
  auto_digest: boolean;
  /** 自动消化间隔小时（1 / 6 / 24 等；0 = 手动） */
  interval_hours: number;
  /** vault 根目录（v1 只读显示） */
  vault_root: string;
}

// ---- v2 · headless run / cancel ----
export interface WikiAgentRunIngestPayload {
  /** 只针对某一天 raw/inbox/<day>/ 的未消化文件 */
  day?: string;
  /** 只针对特定 raw 路径列表 */
  paths?: string[];
}

export interface WikiAgentRunIngestResult {
  task_id: string;
  target_count: number;
  command_preview: string;
}

/**
 * 来自后端 `wiki-agent:event` 事件的 payload（tagged union by `kind`）。
 * 前端用 `import { listen } from "@tauri-apps/api/event"` 订阅。
 */
export type WikiAgentEvent =
  | {
      kind: "started";
      task_id: string;
      target_count: number;
      backend: string;
    }
  | {
      kind: "stream";
      task_id: string;
      /** Claude Code stream-json 一行原文，前端 JSON.parse 后展开 */
      line: string;
    }
  | {
      kind: "stderr";
      task_id: string;
      line: string;
    }
  | {
      kind: "completed";
      task_id: string;
      exit_code: number;
    }
  | {
      kind: "failed";
      task_id: string;
      exit_code: number;
      message: string;
    }
  | {
      kind: "cancelled";
      task_id: string;
    };

export const WIKI_AGENT_EVENT = "wiki-agent:event";

export const wikiAgentApi = {
  /** 状态：pending / digested / total + has_agents_md + backend */
  status: () => invoke<WikiAgentStatus>("wiki_agent_status"),
  /** 生成可粘贴的 ingest prompt（v1 stub 核心：人复制到 Claude Code 跑） */
  preparePrompt: (payload?: WikiAgentPreparePromptPayload) =>
    invoke<WikiAgentPreparePromptResult>("wiki_agent_prepare_prompt", {
      payload: payload ?? {},
    }),
  /** 在 vault 目录打开终端（让用户跑 `claude` / `codex`） */
  openInTerminal: () => invoke<string>("wiki_agent_open_in_terminal"),
  /** 读配置 ~/.homebase/wiki_agent.json */
  getConfig: () => invoke<WikiAgentConfig>("wiki_agent_get_config"),
  /** 写配置 ~/.homebase/wiki_agent.json */
  setConfig: (config: WikiAgentConfig) =>
    invoke<WikiAgentConfig>("wiki_agent_set_config", { config }),
  /**
   * v2 · 立即消化 · spawn `claude -p ... --output-format stream-json` 子进程。
   * 返回 task_id，前端订阅 `wiki-agent:event` 看进度，调 cancelIngest(task_id) 中止。
   */
  runIngest: (payload?: WikiAgentRunIngestPayload) =>
    invoke<WikiAgentRunIngestResult>("wiki_agent_run_ingest", {
      payload: payload ?? {},
    }),
  /** 中止某个跑着的 ingest 任务（kill 子进程）*/
  cancelIngest: (taskId: string) =>
    invoke<void>("wiki_agent_cancel_ingest", { taskId }),
};

// ============== Projects ==============
export interface ProjectInfo {
  name: string;
  path: string;
  has_agents_md: boolean;
  git_branch: string | null;
  git_dirty: boolean;
  last_commit: string | null;
}

export const projectsApi = {
  list: () => invoke<ProjectInfo[]>("projects_list"),
};

// ============== Extensions（v3 §23 改名 Plugin → Extension）==============
export interface ExtensionInfo {
  id: string;
  name: string;
  version: string;
  kind: string;
  enabled: boolean;
  manifest_path: string;
  installed_at: string | null;
}

export const extensionsApi = {
  list: () => invoke<ExtensionInfo[]>("extensions_list"),
};

// ============== Tasks（v3 §21.1 新）==============
export interface TaskInfo {
  id: number;
  title: string;
  description: string | null;
  status: string;
  priority: string;
  project_id: number | null;
  knowledge_doc_id: string | null;
  parent_task_id: number | null;
  estimated_minutes: number | null;
  actual_minutes: number | null;
  due_date: string | null;
  tags: string[];
  created_at: string;
  updated_at: string;
}

export const tasksApi = {
  list: () => invoke<TaskInfo[]>("tasks_list"),
};

// ============== Skills（v3 §21.2 新）==============
export interface SkillInfo {
  id: number;
  name: string;
  description: string | null;
  file_path: string;
  source: string;
  target_agents: string[];
  tags: string[];
  last_modified: string | null;
}

export const skillsApi = {
  list: () => invoke<SkillInfo[]>("skills_list"),
};

// ============== GDNbit（Phase C · 2026-05-04 / 2026-05-06 增强 — Wiki↔网站发布打通）==============

/**
 * GDNbit 发布配置（写入 `~/.homebase/gdnbit.json`）
 *
 * Phase C 增强 · 密钥从 SOPS `gdnbit_content_api_key` 取，不再依赖 `api_key` 字段
 * `api_key` 字段保留为兼容字段（旧 json 能读，但不再写）
 */
export interface GdnbitConfig {
  /** node.exe 可执行路径（默认 "node" 假设在 PATH） */
  node_path: string;
  /** obsidian-sync.mjs 脚本绝对路径（批量扫 vault 同步） */
  script_path: string;
  /** publishing-kit/scripts/publish.mjs 脚本绝对路径（单文件主工具）*/
  publish_kit_path: string;
  /** 网站 API URL（脚本拼 /api/content/publish）· 默认 https://example.invalid */
  api_url: string;
  /** **软弃用** · Phase G 后从 SOPS 取，不再使用此字段 */
  api_key?: string | null;
  /** Obsidian vault 根目录（默认 C:\HomeBase\Wiki） */
  vault_path: string;
  /** 默认发布状态：draft / published */
  default_status: string;
}

export interface DoctorCheck {
  /** node / publish_kit / sync_script / vault / sops_key / api */
  id: string;
  label: string;
  ok: boolean;
  detail: string;
}

export interface GdnbitDoctorResult {
  ok: boolean;
  checks: DoctorCheck[];
  suggestions: string[];
}

/** sync_vault · 批量扫 vault */
export interface GdnbitSyncVaultPayload {
  /** 只同步某一具体文件（vault 相对路径）；不填 = 扫描整个 vault */
  single_file?: string | null;
  /** 预览模式（--dry-run）· 不写入网站 */
  dry_run?: boolean;
  /** 忽略 publish: true frontmatter，强制全发（--force） */
  force?: boolean;
  /** 覆盖默认状态：draft / published */
  status?: string | null;
  /** 只同步 vault 下某子文件夹 */
  folder?: string | null;
}

/** publish_file · 单文件 preview / publish */
export interface GdnbitPublishFilePayload {
  /** 模式：preview / publish */
  mode: "preview" | "publish";
  /** markdown 文件绝对路径 */
  file: string;
  /** 仅 mode=publish 时生效 · 覆盖 status · draft/published/archived */
  status?: string | null;
}

/** render_file · 本地 HTML 预览 */
export interface GdnbitRenderFilePayload {
  /** markdown 文件绝对路径 */
  file: string;
  /** 生成后是否自动用默认浏览器打开 */
  open?: boolean;
}

/** ai_optimize · Claude CLI 对单文件做风格优化 */
export interface GdnbitAiOptimizePayload {
  /** markdown 文件绝对路径 */
  file: string;
  /** 优化 focus：简短中文（"风格" / "结构" / "完整性" / 自由文本）· 留空 = 全面 */
  focus?: string | null;
}

/** list_remote · 列服务器现有内容的过滤器 */
export interface GdnbitListRemoteFilter {
  /** blog / project / resource */
  type?: string | null;
  /** draft / published / archived */
  status?: string | null;
  /** 搜索关键词 */
  search?: string | null;
  /** 结果上限 */
  limit?: number | null;
}

/** 流式任务返回 · 含 task_id / mode / 命令预览 / 是否预览模式 */
export interface StreamingTaskResult {
  task_id: string;
  /** 脱敏命令预览（不含密钥）*/
  command_preview: string;
  /** sync_vault / publish / preview / kit_doctor */
  mode: string;
  dry_run: boolean;
}

/** 一次性命令返回 · render / list / 等不流式的 */
export interface OneShotResult {
  ok: boolean;
  exit_code: number;
  /** stdout · 含 ANSI 颜色码（前端可选转 HTML） */
  stdout: string;
  stderr: string;
}

/** 实时事件（通过 `gdnbit:event` 订阅） */
export type GdnbitEvent =
  | {
      kind: "started";
      task_id: string;
      /** sync_vault / publish / preview / kit_doctor */
      mode: string;
      dry_run: boolean;
      /** 脱敏命令预览 */
      cmd: string;
    }
  | { kind: "stream"; task_id: string; line: string }
  | { kind: "stderr"; task_id: string; line: string }
  | { kind: "completed"; task_id: string; exit_code: number }
  | {
      kind: "failed";
      task_id: string;
      exit_code: number;
      message: string;
    }
  | { kind: "cancelled"; task_id: string };

export const gdnbitApi = {
  /** 读 ~/.homebase/gdnbit.json */
  getConfig: () => invoke<GdnbitConfig>("gdnbit_get_config"),
  /** 写 ~/.homebase/gdnbit.json */
  setConfig: (config: GdnbitConfig) =>
    invoke<GdnbitConfig>("gdnbit_set_config", { config }),
  /** 本地 6 项环境检查（不联网，只看 node/脚本/vault/SOPS/URL）*/
  doctor: () => invoke<GdnbitDoctorResult>("gdnbit_doctor"),
  /** 流式 spawn `publish.mjs doctor`（6 项上游联网检查）· 返回 task_id */
  kitDoctor: () => invoke<StreamingTaskResult>("gdnbit_kit_doctor"),
  /**
   * 单文件 preview / publish · 流式
   * mode=preview · API dry-run（看自动分类）
   * mode=publish · 写库（默认 draft，可指定 published）
   */
  publishFile: (payload: GdnbitPublishFilePayload) =>
    invoke<StreamingTaskResult>("gdnbit_publish_file", { payload }),
  /** 单文件 本地 HTML 预览（不联网）· 一次性返回 stdout */
  renderFile: (payload: GdnbitRenderFilePayload) =>
    invoke<OneShotResult>("gdnbit_render_file", { payload }),
  /** 列服务器现有内容 · 一次性返回 stdout */
  listRemote: (filter: GdnbitListRemoteFilter = {}) =>
    invoke<OneShotResult>("gdnbit_list_remote", { filter }),
  /**
   * 批量扫 vault 同步（spawn `obsidian-sync.mjs ...`）· 流式
   * 适合"一键同步整个 Wiki 中所有 publish: true 文章"
   */
  syncVault: (payload: GdnbitSyncVaultPayload = {}) =>
    invoke<StreamingTaskResult>("gdnbit_sync_vault", { payload }),
  /**
   * Phase C-4 · AI 优化单文件 · 流式
   * spawn `claude -p <prompt>` · 按 STYLE_GUIDE.md 规范直接改写文件
   */
  aiOptimize: (payload: GdnbitAiOptimizePayload) =>
    invoke<StreamingTaskResult>("gdnbit_ai_optimize", { payload }),
  /** kill 任意流式任务（sync_vault / publish_file / kit_doctor / ai_optimize）*/
  cancel: (taskId: string) => invoke<void>("gdnbit_cancel", { taskId }),
};

// ============== Studio（Phase D · 2026-05-06 启动 — AI 写作工坊）==============
//
// 数据落地约定：
// - 对话：C:\HomeBase\Wiki\_raw\studio-<ts>-<slug>.chat.jsonl（JSON Lines · 每行一条 ChatMessage）
// - 文章：C:\HomeBase\Wiki\_raw\studio-<ts>-<slug>.md（可选 · AI 生成后落盘 · frontmatter source: studio）
//
// D2.1 当前：只暴露 list_sessions · D2.2+ 补 create/send/cancel/get 等

/** 会话元信息（左栏列表用 · 不含完整消息内容）*/
export interface StudioSessionMeta {
  /** 文件名主干 · 唯一 ID · 如 "studio-20260506-0320-react-hooks" */
  session_id: string;
  /** 标题 · 第一条 user 消息前 30 字 · 或 session_id */
  title: string;
  /** 创建时间 Unix 毫秒 */
  created_ms: number;
  /** 最后活动 Unix 毫秒 */
  updated_ms: number;
  /** 消息总数 */
  message_count: number;
  /** 对应生成文章的绝对路径（`.md` 存在时填 · 不存在则 null）*/
  article_path: string | null;
  /** `.chat.jsonl` 绝对路径（debug / 导出用）*/
  chat_path: string;
}

/** 一段 assistant 输出的子部分（D2.7 · 工具调用可视化）
 *  text part 是连续 assistant 文本 · tool part 是一次完整工具调用（含开始 + 结果） */
export type TurnPart =
  | { kind: "text"; text: string }
  | {
      kind: "tool";
      tool_use_id: string;
      name: string;
      /** 输入摘要 · 如 file_path / pattern */
      input_summary: string;
      /** 结果预览（前 300 字 · null 表示还在跑） */
      result_preview: string | null;
      is_error: boolean;
      finished: boolean;
    };

/** 对话消息（jsonl 每行一条 · 前后端共用结构）*/
export interface ChatMessage {
  /** Unix 毫秒时间戳 */
  ts: number;
  /** user / assistant / tool / system */
  role: string;
  /** 正文（Markdown 允许）· assistant 类型也填合并后的纯文本 · 兼容旧版 */
  content: string;
  /** 模型名（assistant 消息时填）*/
  model?: string | null;
  /** token 消耗 */
  tokens?: number | null;
  /** **D2.7**：assistant 消息的有序子部分（text + 工具调用穿插）·
   *  存在时优先于 content 渲染 · 缺省时 fallback content */
  parts?: TurnPart[] | null;
  /** 任务统计（assistant 完成时可选）*/
  stats?: {
    total_cost_usd: number;
    duration_ms: number;
    num_turns: number;
  } | null;
  /** **E1**：user 消息发送前生成的 checkpoint id（= task_id）·
   *  使用 “撚回这条” 按钮时 · 后端 unzip 快照 + truncate jsonl */
  checkpoint_id?: string | null;
}

/** chat_send 入参 */
export interface StudioChatSendPayload {
  /** 会话 ID · 不填表示新建（后端生成 timestamp ID） */
  session_id?: string | null;
  /** USER 本轮消息（必填） */
  user_message: string;
  /** D2.6 多轮：历史 messages（不含本轮 user_message）· 后端拼到 prompt 头 */
  history?: ChatMessage[] | null;
  /** D2.4 启用：资料源文件绝对路径列表 · D2.2 暂不传 */
  source_paths?: string[] | null;
}

/** chat_send 返回 */
export interface StudioChatSendResult {
  /** 流式任务 ID · 前端订阅 studio:event 时按此过滤 */
  task_id: string;
  /** 实际生效的 session_id（前端传空时由后端生成） */
  session_id: string;
  /** 命令预览（脱敏 · 不含完整 prompt） */
  cmd_preview: string;
}

/** Studio 流式事件 · listen("studio:event") 接
 *  D2.7 加结构化变体（assistant_text / tool_use / tool_result / result_stats）·
 *  stream 变体保留作 fallback（parse JSON 失败时的 raw 行）*/
export type StudioEvent =
  | {
      kind: "started";
      task_id: string;
      session_id: string;
      cmd_preview: string;
    }
  /** D2.7 · 一段完整 assistant 文本（来自 stream-json message.content[*].text）*/
  | { kind: "assistant_text"; task_id: string; text: string }
  /** D2.7 · 工具调用开始 */
  | {
      kind: "tool_use";
      task_id: string;
      tool_use_id: string;
      name: string;
      input_summary: string;
    }
  /** D2.7 · 工具调用结果 */
  | {
      kind: "tool_result";
      task_id: string;
      tool_use_id: string;
      content_preview: string;
      is_error: boolean;
    }
  /** D2.7 · 任务统计（claude --output-format stream-json 的 result 事件）*/
  | {
      kind: "result_stats";
      task_id: string;
      total_cost_usd: number;
      duration_ms: number;
      num_turns: number;
    }
  /** stdout 一行 · 仅 parse JSON 失败时 fallback（debug 用）*/
  | { kind: "stream"; task_id: string; line: string }
  | { kind: "stderr"; task_id: string; line: string }
  | {
      kind: "completed";
      task_id: string;
      session_id: string;
      exit_code: number;
    }
  | {
      kind: "failed";
      task_id: string;
      exit_code: number;
      message: string;
    }
  | { kind: "cancelled"; task_id: string };

export const studioApi = {
  /**
   * 列出所有 Studio 会话 · 按最后活动时间降序
   * 扫 `<vault>/_raw/studio-*.chat.jsonl` · 不存在返回空数组（非错误）
   */
  listSessions: () => invoke<StudioSessionMeta[]>("studio_list_sessions"),

  /**
   * **D2.7**：从 jsonl 读会话历史 messages · 切会话恢复用
   * 文件不存在 → 空数组（新会话）· parse 失败行跳过
   */
  loadSession: (sessionId: string) =>
    invoke<ChatMessage[]>("studio_load_session", { sessionId }),

  /**
   * 发送一条 USER 消息 · 调 Claude CLI 流式生成 assistant 回复
   * 前端用 task_id 订阅 `studio:event` listener · 收 stream/completed/failed
   */
  chatSend: (payload: StudioChatSendPayload) =>
    invoke<StudioChatSendResult>("studio_chat_send", { payload }),

  /**
   * **D2.7**：append 一条 ChatMessage 到 session 的 jsonl
   * 前端在 user 发送后 + assistant completed 后调
   */
  persistMessage: (sessionId: string, message: ChatMessage) =>
    invoke<void>("studio_persist_message", { sessionId, message }),

  /**
   * **E1**：撚回 AI 改动 · unzip checkpoint 覆盖 _raw + truncate jsonl
   * 返回截断后剩余的 messages · 前端直接 setMessages 替换
   */
  restoreCheckpoint: (sessionId: string, taskId: string, cutoffTs: number) =>
    invoke<ChatMessage[]>("studio_restore_checkpoint", {
      sessionId,
      taskId,
      cutoffTs,
    }),

  /**
   * **E2**：读 attachments/ 下图片返回 base64 data URL · 给 ToolCard 嵌图预览
   * 安全：后端校验 path 必须在 vault 的 _raw/attachments/ 下
   */
  readAttachmentB64: (path: string) =>
    invoke<string>("studio_read_attachment_b64", { path }),

  /**
   * **E2.6**：列出当前所有 in-progress 生图任务（按 started_at_ms 倒序）
   * 前端 ToolCard 在 generate_image 工具未完成时 1Hz polling 拿阶段
   */
  listImageProgress: () =>
    invoke<ImageProgress[]>("studio_list_image_progress"),

  /** 取消正在跑的 chat 任务 · kill child + emit Cancelled */
  cancel: (taskId: string) => invoke<void>("studio_cancel", { taskId }),
};

/** **E2.6**：生图阶段进度（mcp-image binary 写到 ~/.homebase/img-progress/<pid>.json）*/
export interface ImageProgress {
  /** mcp-image 子进程 pid */
  pid: number;
  /** 阶段：preparing / sending / waiting / decoding / saving / done / error */
  stage: string;
  started_at_ms: number;
  updated_at_ms: number;
  elapsed_ms: number;
  model: string;
  prompt_preview: string;
  /** done 阶段 = 输出文件绝对路径 */
  output_path: string | null;
  /** error 阶段 = 错误信息 */
  error: string | null;
}

// =====================================================================
// Shared MCP Pool · S2 · 2026-05-06
// =====================================================================

export interface McpServerDef {
  /** Server 唯一 id（强制 `homebase-` 前缀） */
  id: string;
  name: string;
  description: string;
  /** stdio · 目前只支持这一种 */
  transport: string;
  /** 可执行文件绝对路径 */
  command: string;
  args: string[];
  /** 值可用 ${VAR} 占位符 · agent 启动时从父进程 env 替换 */
  env: Record<string, string>;
}

export interface SharedMcpPool {
  servers: McpServerDef[];
  /** agent_id → 启用的 server.id 列表 */
  assignments: Record<string, string[]>;
}

export interface AgentReconcileResult {
  agent_id: string;
  path: string;
  /** `applied` / `skipped` / `error` */
  status: "applied" | "skipped" | "error";
  applied_count: number;
  message: string | null;
}

export interface ReconcileReport {
  agents: AgentReconcileResult[];
}

export const sharedMcpApi = {
  /** 读 ~/.homebase/shared/mcp-pool.json · 不存在则返默认 */
  get: () => invoke<SharedMcpPool>("shared_mcp_get"),
  /** 写 pool · 不自动 reconcile */
  save: (pool: SharedMcpPool) => invoke<void>("shared_mcp_save", { pool }),
  /** 触发 reconcile · 把当前 pool 推到 4 agent 配置文件 */
  apply: () => invoke<ReconcileReport>("shared_mcp_apply"),
  /** 写 + 立即 reconcile（"Apply" 按钮） */
  saveAndApply: (pool: SharedMcpPool) =>
    invoke<ReconcileReport>("shared_mcp_save_and_apply", { pool }),
  /** Agent ID 列表（4 官方 agent · Hermes 暂不支持） */
  agentIds: () => invoke<string[]>("shared_mcp_agent_ids"),
};

// =====================================================================
// Agent Versions · S5 · 2026-05-06
// =====================================================================

export interface AgentVersionInfo {
  agent_id: string;
  /** `npm`（npm 全局装）/ `standalone`（裸 binary · fallback `<bin> --version` 检到）/ `fork`（hermes）/ `unknown` */
  kind: "npm" | "standalone" | "fork" | "unknown";
  package: string | null;
  current: string | null;
  latest: string | null;
  update_available: boolean;
  update_command: string | null;
  message: string | null;
}

export const agentVersionsApi = {
  /** 一次检查 5 agent 版本（带 1h 缓存）· 返结果数组（顺序固定） */
  check: () => invoke<AgentVersionInfo[]>("agent_versions_check"),
};

// =====================================================================
// Shared Skills · S4 · 2026-05-06
// =====================================================================

export interface SharedSkillInfo {
  id: string;
  size_bytes: number;
  has_skill_md: boolean;
  modified_at: number | null;
}

export interface SkillsReconcileEntry {
  agent_id: string;
  path: string;
  status: string;
  copied_count: number;
  message: string | null;
}

export interface SkillsReconcileReport {
  agents: SkillsReconcileEntry[];
}

export const sharedSkillsApi = {
  /** 列 ~/.homebase/shared/skills/ 下所有 skill */
  list: () => invoke<SharedSkillInfo[]>("shared_skills_list"),
  /** 复制到 claude / openclaw 的 skills/ 目录（带 homebase- 前缀） */
  apply: () => invoke<SkillsReconcileReport>("shared_skills_apply"),
  /** 用资源管理器打开 ~/.homebase/shared/skills/ · 让 USER 直接编辑 SKILL.md */
  openDir: () => invoke<string>("shared_skills_open_dir"),
};

// =====================================================================
// Shared Secrets · S3 · 2026-05-06
// =====================================================================

export interface IpcEndpoint {
  endpoint: string; // "127.0.0.1:51234"
  token: string; // 64-char hex
}

export interface AclEntry {
  granted_at: string;
  purpose: string;
}

export interface AclMatrix {
  agents: string[];
  secrets: string[];
  /** cells[agent_idx][secret_idx] = boolean */
  cells: boolean[][];
}

export interface AclListResult {
  /** key = `<agent>:<secret>` · 含 granted_at + purpose */
  entries: Record<string, AclEntry>;
  matrix: AclMatrix;
}

/** USER 弹窗作出的决定 */
export type ConsentAction = "allow_once" | "allow_persistent" | "deny";

/** 弹窗 event payload · 由后端 emit("secrets:consent_request") 推送 */
export interface ConsentRequestEvent {
  request_id: string;
  agent_id: string;
  secret_name: string;
  purpose: string;
}

export const sharedSecretsApi = {
  /** 列 secrets.yaml 顶层 key 名（不解密） */
  listNames: () => invoke<string[]>("shared_secrets_list_names"),
  /** 4 agent · 用作 ACL 矩阵列头 */
  agentIds: () => invoke<string[]>("shared_secrets_agent_ids"),
  /** IPC server 端点信息 · null 表示未启动 */
  ipcEndpoint: () => invoke<IpcEndpoint | null>("shared_secrets_ipc_endpoint"),
  /** 列所有 ACL · 含矩阵 */
  aclList: () => invoke<AclListResult>("shared_secrets_acl_list"),
  /** 撤销某 (agent, secret) 的永久允许 · 返回是否原本有记录 */
  aclRevoke: (agentId: string, secretName: string) =>
    invoke<boolean>("shared_secrets_acl_revoke", {
      agentId,
      secretName,
    }),
  /** 把 USER 弹窗决定送达后端 · 释放等待中的 IPC 请求 */
  consentResolve: (requestId: string, action: ConsentAction) =>
    invoke<void>("shared_secrets_consent_resolve", {
      requestId,
      action,
    }),
  /** 弹个新 cmd 窗口让 USER 编辑 secrets.yaml · 添加/删除 secret */
  openYaml: () => invoke<string>("shared_secrets_open_yaml"),
};

// =====================================================================
// Governance 路 Agent OS P0
// =====================================================================

export interface AuditEvent {
  ts: string;
  event: string;
  actor: string;
  source: string;
  target: string;
  result: string;
  risk: string;
  correlation_id: string;
  details: unknown;
}

export interface SnapshotFile {
  source: string;
  exists: boolean;
  kind: string;
  stored_as: string | null;
  size_bytes: number | null;
}

export interface SnapshotManifest {
  snapshot_id: string;
  created_at: string;
  reason: string;
  actor: string;
  files: SnapshotFile[];
}

export interface MachineManifest {
  schema_version: number;
  generated_at: string;
  homebase_home: string;
  os: {
    name: string;
    arch: string;
    hostname: string;
    username: string;
  };
  disks: Array<{
    drive: string;
    used_bytes: number | null;
    free_bytes: number | null;
    role: string;
  }>;
  directories: Array<{
    path: string;
    role: string;
    exists: boolean;
    required: boolean;
  }>;
  runtimes: Array<{
    id: string;
    command: string;
    installed: boolean;
    version: string | null;
  }>;
  agents: Array<{
    id: string;
    config_path: string;
    config_exists: boolean;
  }>;
  ai_tools: Array<{
    id: string;
    display_name: string;
    install_path: string;
    installed: boolean;
    control_level: string;
  }>;
  secrets_policy: string;
  docs_index: string;
}

export interface CapabilityMatrix {
  generated_at: string;
  rows: Array<{
    actor_id: string;
    actor_kind: string;
    mcp_servers: string[];
    skills: string[];
    allowed_secrets: string[];
    config_paths: string[];
    risk: string;
  }>;
}

export interface ConfigOwnership {
  generated_at: string;
  entries: Array<{
    path: string;
    owner: string;
    managed_by: string;
    write_policy: string;
    plaintext_secret_allowed: boolean;
    backup_required: boolean;
    exists: boolean;
    notes: string;
  }>;
}

export interface InterfaceRegistry {
  generated_at: string;
  interfaces: InterfaceEntry[];
}

export interface InterfaceEntry {
  id: string;
  display_name: string;
  kind: string;
  base_url: string | null;
  owner: string;
  auth_source: string;
  discovery_methods: string[];
  risk: string;
  notes: string;
}

export interface InterfaceProbePayload {
  kind: string;
  base_url: string;
  secret_short_name?: string | null;
}

export interface InterfaceProbeResult {
  ok: boolean;
  attempted_urls: string[];
  status_code: number | null;
  latency_ms: number;
  models: string[];
  error: string | null;
  body_preview: string | null;
}

export const governanceApi = {
  auditRecent: (limit = 100) =>
    invoke<AuditEvent[]>("governance_audit_recent", { limit }),
  snapshotsList: (limit = 50) =>
    invoke<SnapshotManifest[]>("governance_snapshots_list", { limit }),
  snapshotRollback: (snapshotId: string) =>
    invoke<SnapshotManifest>("governance_snapshot_rollback", { snapshotId }),
  machineManifestExport: () =>
    invoke<MachineManifest>("governance_machine_manifest_export"),
  capabilityMatrix: () =>
    invoke<CapabilityMatrix>("governance_capability_matrix"),
  configOwnership: () =>
    invoke<ConfigOwnership>("governance_config_ownership"),
  interfaceRegistry: () =>
    invoke<InterfaceRegistry>("governance_interface_registry"),
  interfaceProbe: (payload: InterfaceProbePayload) =>
    invoke<InterfaceProbeResult>("governance_interface_probe", { payload }),
};

export interface AiControlBootstrap {
  files: { ready: boolean; project_root: string | null; startup_file: string | null; cli_file: string | null; prompt: string | null; issue: string | null };
  service: { state: "starting" | "ready" | "error" | "disabled" | "stopped"; message: string };
  total: number;
  available: number;
  blocked: number;
  unavailable: number;
}
export const aiControlApi = {
  bootstrap: () => invoke<AiControlBootstrap>("ai_control_bootstrap"),
};
