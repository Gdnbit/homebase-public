/**
 * 服务注册表 — short_name → { 展示名、logo 源、品牌色 }
 *
 * 使用策略（两层）：
 * 1. **精确匹配**：16 个 KNOWN_SHORT_NAMES 直接命中 SERVICE_META
 * 2. **前缀/启发式匹配**：短名前缀 / 关键字推断服务（如 any `*_figma_*` → Figma）
 * 3. **兜底**：displayName = 短名转 Title Case；logo = initial 字符
 *
 * 新增 key 时优先走第 1 条；若属于已支持服务的变种（如 openrouter_backup），
 * 前缀匹配会自动命中，不需改表。
 */

export type LogoKind =
  /** @lobehub/icons 的 AI 厂商图标（Anthropic/OpenAI/Gemini/DeepSeek/OpenRouter 等） */
  | "lobe"
  /** react-icons/si 的 simple-icons 服务图标（GitHub/Figma/Obsidian/Docker 等） */
  | "si"
  /** 自定义品牌色 + 首字符（GDNbit 自有项目 / 新服务） */
  | "initial";

export interface ServiceMeta {
  /** 用户可读显示名（"GitHub" / "Figma" / "OpenAI"） */
  displayName: string;
  /** logo 来源 */
  logoKind: LogoKind;
  /**
   * logo 标识：
   * - lobe：`@lobehub/icons` 的组件名（"Claude" / "OpenAI" / "Gemini" ...）
   * - si：`react-icons/si` 的组件名（"SiGithub" / "SiFigma" ...）
   * - initial：忽略（渲染首字符）
   */
  logoKey: string;
  /**
   * 品牌色（hex，不含 #）：
   * - lobe/si：可选，覆盖 lobehub/simple-icons 默认色
   * - initial：必填，用作背景色
   */
  brandColor?: string;
  /** Add Wizard 预填：推荐短名（如 "github_token"） */
  suggestedShortName?: string;
  /** Add Wizard 预填：推荐长名（环境变量名，如 "GITHUB_PERSONAL_ACCESS_TOKEN"） */
  suggestedLongName?: string;
  /** Add Wizard 预填：轮换页 URL（如 "https://github.com/settings/tokens"） */
  suggestedRotateUrl?: string;
  /** Add Wizard 预填：简短用途说明（用于卡片下一行） */
  description?: string;
  /**
   * 服务接入层级：
   * - `1`（默认）：HomeBase 已预填短名 / 长名 / 轮换页，Add Wizard 选了即可填密钥
   * - `2`：常见服务，仅有 logo + 显示名，需要用户自己起短名（用 key 作为建议占位）
   */
  tier?: 1 | 2;
}

/** 16 个 KNOWN_SHORT_NAMES 精确映射（与 secrets.rs meta_for 同源） */
const SERVICE_META: Record<string, ServiceMeta> = {
  // Core 5
  openrouter_key: {
    displayName: "OpenRouter",
    logoKind: "lobe",
    logoKey: "OpenRouter",
    suggestedShortName: "openrouter_key",
    suggestedLongName: "OPENROUTER_API_KEY",
    suggestedRotateUrl: "https://openrouter.ai/keys",
    description: "LLM 网关（Codex / OpenClaw）",
  },
  github_token: {
    displayName: "GitHub",
    logoKind: "si",
    logoKey: "SiGithub",
    brandColor: "181717",
    suggestedShortName: "github_token",
    suggestedLongName: "GITHUB_PERSONAL_ACCESS_TOKEN",
    suggestedRotateUrl: "https://github.com/settings/tokens",
    description: "Personal Access Token（github-mcp）",
  },
  firecrawl_token: {
    displayName: "Firecrawl",
    logoKind: "initial",
    logoKey: "",
    brandColor: "F56565",
    suggestedShortName: "firecrawl_token",
    suggestedLongName: "FIRECRAWL_API_KEY",
    suggestedRotateUrl: "https://www.firecrawl.dev/app/api-keys",
    description: "网页抓取（firecrawl-mcp）",
  },
  tavily_token: {
    displayName: "Tavily",
    logoKind: "initial",
    logoKey: "",
    brandColor: "6366F1",
    suggestedShortName: "tavily_token",
    suggestedLongName: "TAVILY_API_KEY",
    suggestedRotateUrl: "https://app.tavily.com/home",
    description: "AI 搜索（tavily-mcp）",
  },
  figma_token: {
    displayName: "Figma",
    logoKind: "si",
    logoKey: "SiFigma",
    brandColor: "F24E1E",
    suggestedShortName: "figma_token",
    suggestedLongName: "FIGMA_ACCESS_TOKEN",
    suggestedRotateUrl: "https://www.figma.com/settings",
    description: "Design API（figma-developer-mcp）",
  },

  // Future-ready 6
  tryallai_key: {
    displayName: "TryAllAI",
    logoKind: "initial",
    logoKey: "",
    brandColor: "8B5CF6",
    suggestedShortName: "tryallai_key",
    suggestedLongName: "TRYALLAI_API_KEY",
    description: "备用 LLM 网关",
  },
  anthropic_key: {
    displayName: "Anthropic",
    logoKind: "lobe",
    logoKey: "Anthropic",
    suggestedShortName: "anthropic_key",
    suggestedLongName: "ANTHROPIC_API_KEY",
    suggestedRotateUrl: "https://console.anthropic.com/settings/keys",
    description: "直调 Anthropic API（非 Claude Code）",
  },
  openai_key: {
    displayName: "OpenAI",
    logoKind: "lobe",
    logoKey: "OpenAI",
    suggestedShortName: "openai_key",
    suggestedLongName: "OPENAI_API_KEY",
    suggestedRotateUrl: "https://platform.openai.com/api-keys",
    description: "直调 OpenAI",
  },
  deepgram_key: {
    displayName: "Deepgram",
    logoKind: "initial",
    logoKey: "",
    brandColor: "13EF93",
    suggestedShortName: "deepgram_key",
    suggestedLongName: "DEEPGRAM_API_KEY",
    suggestedRotateUrl: "https://console.deepgram.com/",
    description: "ASR Nova-3 / realtime captions",
  },
  assemblyai_key: {
    displayName: "AssemblyAI",
    logoKind: "initial",
    logoKey: "",
    brandColor: "6C5CE7",
    suggestedShortName: "assemblyai_key",
    suggestedLongName: "ASSEMBLYAI_API_KEY",
    suggestedRotateUrl: "https://www.assemblyai.com/dashboard/signup",
    description: "ASR Universal / diarization",
  },
  elevenlabs_key: {
    displayName: "ElevenLabs",
    logoKind: "initial",
    logoKey: "",
    brandColor: "111827",
    suggestedShortName: "elevenlabs_key",
    suggestedLongName: "ELEVENLABS_API_KEY",
    suggestedRotateUrl: "https://elevenlabs.io/app/settings/api-keys",
    description: "Scribe ASR / text to speech",
  },
  hf_token: {
    displayName: "Hugging Face",
    logoKind: "lobe",
    logoKey: "HuggingFace",
    suggestedShortName: "hf_token",
    suggestedLongName: "HF_TOKEN",
    suggestedRotateUrl: "https://huggingface.co/settings/tokens",
    description: "Model downloads / pyannote diarization",
  },
  gemini_key: {
    displayName: "Google Gemini",
    logoKind: "lobe",
    logoKey: "Gemini",
    suggestedShortName: "gemini_key",
    suggestedLongName: "GEMINI_API_KEY",
    suggestedRotateUrl: "https://aistudio.google.com/apikey",
    description: "Gemini CLI / Gemini MCP",
  },
  deepseek_key: {
    displayName: "DeepSeek",
    logoKind: "lobe",
    logoKey: "DeepSeek",
    suggestedShortName: "deepseek_key",
    suggestedLongName: "DEEPSEEK_API_KEY",
    suggestedRotateUrl: "https://platform.deepseek.com/api_keys",
    description: "DeepSeek V4 直调",
  },
  obsidian_api_key_raw: {
    displayName: "Obsidian",
    logoKind: "si",
    logoKey: "SiObsidian",
    brandColor: "7C3AED",
    suggestedShortName: "obsidian_api_key_raw",
    suggestedLongName: "OBSIDIAN_API_KEY",
    description: "Local REST API plugin",
  },

  // GDNbit 5（同一项目共用 logo）
  gdnbit_content_api_key: {
    displayName: "GDNbit Content API",
    logoKind: "initial",
    logoKey: "",
    brandColor: "0EA5E9",
    suggestedShortName: "gdnbit_content_api_key",
    suggestedRotateUrl: "https://example.invalid/admin",
    description: "publishing-kit / GDNbit Web",
  },
  gdnbit_mongodb_password: {
    displayName: "GDNbit MongoDB",
    logoKind: "si",
    logoKey: "SiMongodb",
    brandColor: "47A248",
    suggestedShortName: "gdnbit_mongodb_password",
    description: "MongoDB gdnbit 用户 / 备份脚本",
  },
  gdnbit_nextauth_secret: {
    displayName: "GDNbit NextAuth",
    logoKind: "si",
    logoKey: "SiNextdotjs",
    brandColor: "000000",
    suggestedShortName: "gdnbit_nextauth_secret",
    description: "NextAuth.js session 加密",
  },
  gdnbit_jwt_secret: {
    displayName: "GDNbit JWT",
    logoKind: "si",
    logoKey: "SiJsonwebtokens",
    brandColor: "000000",
    suggestedShortName: "gdnbit_jwt_secret",
    description: "JWT 签名",
  },
  gdnbit_admin_secret_key: {
    displayName: "GDNbit Admin",
    logoKind: "initial",
    logoKey: "",
    brandColor: "0EA5E9",
    suggestedShortName: "gdnbit_admin_secret_key",
    suggestedRotateUrl: "https://example.invalid/admin",
    description: "内部管理 API 校验",
  },
};

/**
 * Tier 2 — 常见服务清单（仅 logo + 显示名 + 简介）
 *
 * 与 Tier 1 区别：
 * - **不预填短名**：用户自己起（短名输入框 placeholder = key 作为参考）
 * - **不预填长名 / 轮换页**：因为同一服务有多种 token 类型（access / publishable / secret），
 *   不强 push 一个特定的环境变量名
 * - **getServiceMeta() 不会查这个表**：Tier 2 服务的 short_name 任意，启发式匹配处理
 *
 * 30+ 服务覆盖 LLM / 云 / SaaS / 数据库 / 搜索 / 工具，覆盖大部分日常场景
 */
const TIER2_SERVICES: Record<string, ServiceMeta> = {
  // LLM / AI 厂商（lobe icons）
  huggingface_token: { displayName: "HuggingFace", logoKind: "lobe", logoKey: "HuggingFace", description: "AI 模型 Hub" },
  mistral_key: { displayName: "Mistral", logoKind: "lobe", logoKey: "Mistral", description: "Mistral / Mixtral 系列" },
  xai_key: { displayName: "xAI / Grok", logoKind: "lobe", logoKey: "XAI", description: "Grok 系列" },
  cohere_key: { displayName: "Cohere", logoKind: "lobe", logoKey: "Cohere", description: "Command / Embed" },
  together_key: { displayName: "Together AI", logoKind: "lobe", logoKey: "Together", description: "开源模型聚合" },
  groq_key: { displayName: "Groq", logoKind: "lobe", logoKey: "Groq", description: "极速推理（LPU）" },
  perplexity_key: { displayName: "Perplexity", logoKind: "lobe", logoKey: "Perplexity", description: "AI 搜索 / Sonar" },

  // 云 / 基建（lobe + si 混用）
  aws_access_key: { displayName: "AWS", logoKind: "lobe", logoKey: "Aws", description: "Amazon Web Services" },
  azure_key: { displayName: "Azure", logoKind: "lobe", logoKey: "Azure", description: "Microsoft Cloud" },
  gcp_key: { displayName: "Google Cloud", logoKind: "si", logoKey: "SiGooglecloud", brandColor: "4285F4", description: "GCP" },
  vercel_token: { displayName: "Vercel", logoKind: "si", logoKey: "SiVercel", brandColor: "000000", description: "前端部署" },
  cloudflare_token: { displayName: "Cloudflare", logoKind: "si", logoKey: "SiCloudflare", brandColor: "F38020", description: "CDN / DNS / Workers" },
  supabase_key: { displayName: "Supabase", logoKind: "si", logoKey: "SiSupabase", brandColor: "3FCF8E", description: "Postgres + Auth + Storage" },

  // SaaS
  stripe_key: { displayName: "Stripe", logoKind: "si", logoKey: "SiStripe", brandColor: "635BFF", description: "支付" },
  notion_token: { displayName: "Notion", logoKind: "si", logoKey: "SiNotion", brandColor: "000000", description: "Notion API" },
  linear_key: { displayName: "Linear", logoKind: "si", logoKey: "SiLinear", brandColor: "5E6AD2", description: "项目管理" },
  slack_token: { displayName: "Slack", logoKind: "si", logoKey: "SiSlack", brandColor: "4A154B", description: "团队消息" },
  discord_token: { displayName: "Discord", logoKind: "si", logoKey: "SiDiscord", brandColor: "5865F2", description: "社区 bot" },
  twilio_token: { displayName: "Twilio", logoKind: "si", logoKey: "SiTwilio", brandColor: "F22F46", description: "SMS / 通话" },
  sendgrid_key: { displayName: "SendGrid", logoKind: "si", logoKey: "SiSendgrid", brandColor: "1A82E2", description: "邮件投递" },
  resend_key: { displayName: "Resend", logoKind: "si", logoKey: "SiResend", brandColor: "000000", description: "现代邮件 API" },
  auth0_token: { displayName: "Auth0", logoKind: "si", logoKey: "SiAuth0", brandColor: "EB5424", description: "身份认证" },

  // 数据库 / 存储
  postgres_password: { displayName: "Postgres", logoKind: "si", logoKey: "SiPostgresql", brandColor: "4169E1", description: "数据库密码" },
  redis_password: { displayName: "Redis", logoKind: "si", logoKey: "SiRedis", brandColor: "DC382D", description: "缓存 / KV" },
  mysql_password: { displayName: "MySQL", logoKind: "si", logoKey: "SiMysql", brandColor: "4479A1", description: "数据库密码" },
  elastic_password: { displayName: "Elasticsearch", logoKind: "si", logoKey: "SiElasticsearch", brandColor: "005571", description: "搜索引擎" },

  // 搜索 API
  brave_search_key: { displayName: "Brave Search", logoKind: "si", logoKey: "SiBrave", brandColor: "FB542B", description: "搜索 API" },
  bing_search_key: { displayName: "Bing", logoKind: "lobe", logoKey: "Bing", description: "Microsoft 搜索 API" },

  // 工具
  bitwarden_token: { displayName: "Bitwarden", logoKind: "si", logoKey: "SiBitwarden", brandColor: "175DDC", description: "密码管理器" },
  docker_token: { displayName: "Docker Hub", logoKind: "si", logoKey: "SiDocker", brandColor: "2496ED", description: "镜像仓库" },
  npm_token: { displayName: "NPM", logoKind: "si", logoKey: "SiNpm", brandColor: "CB3837", description: "包发布" },
};

/**
 * 列出所有预设服务 — 给 Add Wizard 的服务选择器 grid 用
 *
 * 返回顺序：Tier 1（已接入预填，16 个） → Tier 2（常见服务，31 个）
 * 每项带 `tier` 字段，UI 用于区分卡片样式 + Step 2 行为（预填 vs 自填）
 */
export function listKnownServices(): Array<{ key: string; meta: ServiceMeta; tier: 1 | 2 }> {
  const tier1 = Object.entries(SERVICE_META).map(([key, meta]) => ({ key, meta, tier: 1 as const }));
  const tier2 = Object.entries(TIER2_SERVICES).map(([key, meta]) => ({ key, meta, tier: 2 as const }));
  return [...tier1, ...tier2];
}

/** 高优先级启发式（覆盖 Tier 1 服务的变种名 + GDNbit 自有项目） */
const HEURISTIC_RULES: Array<{ test: (n: string) => boolean; meta: ServiceMeta }> = [
  { test: (n) => n.startsWith("openrouter"), meta: { displayName: "OpenRouter", logoKind: "lobe", logoKey: "OpenRouter" } },
  { test: (n) => n.startsWith("github"), meta: { displayName: "GitHub", logoKind: "si", logoKey: "SiGithub", brandColor: "181717" } },
  { test: (n) => n.startsWith("anthropic") || n.startsWith("claude"), meta: { displayName: "Anthropic", logoKind: "lobe", logoKey: "Anthropic" } },
  { test: (n) => n.startsWith("openai"), meta: { displayName: "OpenAI", logoKind: "lobe", logoKey: "OpenAI" } },
  { test: (n) => n.startsWith("deepgram"), meta: { displayName: "Deepgram", logoKind: "initial", logoKey: "", brandColor: "13EF93" } },
  { test: (n) => n.startsWith("assemblyai"), meta: { displayName: "AssemblyAI", logoKind: "initial", logoKey: "", brandColor: "6C5CE7" } },
  { test: (n) => n.startsWith("elevenlabs"), meta: { displayName: "ElevenLabs", logoKind: "initial", logoKey: "", brandColor: "111827" } },
  { test: (n) => n === "hf_token" || n.startsWith("hf_") || n.startsWith("huggingface"), meta: { displayName: "Hugging Face", logoKind: "lobe", logoKey: "HuggingFace" } },
  { test: (n) => n.startsWith("gemini") || n.startsWith("google"), meta: { displayName: "Google Gemini", logoKind: "lobe", logoKey: "Gemini" } },
  { test: (n) => n.startsWith("deepseek"), meta: { displayName: "DeepSeek", logoKind: "lobe", logoKey: "DeepSeek" } },
  { test: (n) => n.startsWith("figma"), meta: { displayName: "Figma", logoKind: "si", logoKey: "SiFigma", brandColor: "F24E1E" } },
  { test: (n) => n.startsWith("obsidian"), meta: { displayName: "Obsidian", logoKind: "si", logoKey: "SiObsidian", brandColor: "7C3AED" } },
  { test: (n) => n.startsWith("gdnbit"), meta: { displayName: "GDNbit", logoKind: "initial", logoKey: "", brandColor: "0EA5E9" } },
  { test: (n) => n.startsWith("tavily"), meta: { displayName: "Tavily", logoKind: "initial", logoKey: "", brandColor: "6366F1" } },
  { test: (n) => n.startsWith("firecrawl"), meta: { displayName: "Firecrawl", logoKind: "initial", logoKey: "", brandColor: "F56565" } },
];

/**
 * 从 TIER2_SERVICES 的 key 自动派生前缀启发式
 *
 * 例：`huggingface_token` → 前缀 `huggingface` → 命中 `huggingface_dev_key` / `hf_huggingface` 等变种
 *
 * 逐字符前缀匹配（取 key 第一段），避免硬维护 30+ 条规则
 */
function buildTier2Heuristics(): Array<{ test: (n: string) => boolean; meta: ServiceMeta }> {
  return Object.entries(TIER2_SERVICES).map(([key, meta]) => {
    const prefix = key.split("_")[0]; // huggingface_token → "huggingface"
    return { test: (n: string) => n.startsWith(prefix), meta };
  });
}

const TIER2_HEURISTICS = buildTier2Heuristics();

/** 从短名得到服务元信息（四层 fallback：Tier1 精确 → Tier1 启发 → Tier2 精确 → Tier2 启发 → initial） */
export function getServiceMeta(shortName: string): ServiceMeta {
  // 1. Tier 1 精确
  if (SERVICE_META[shortName]) return SERVICE_META[shortName];
  // 2. Tier 1 启发式（含 GDNbit / Anthropic / OpenAI 等高优先级覆盖）
  for (const rule of HEURISTIC_RULES) {
    if (rule.test(shortName)) return rule.meta;
  }
  // 3. Tier 2 精确（HuggingFace / Mistral / Stripe / Notion 等 30+）
  if (TIER2_SERVICES[shortName]) return TIER2_SERVICES[shortName];
  // 4. Tier 2 前缀启发（huggingface_dev_key / stripe_test_key 等变种）
  for (const rule of TIER2_HEURISTICS) {
    if (rule.test(shortName)) return rule.meta;
  }
  // 5. 兜底：短名转显示名（下划线 → 空格 + Title Case）
  const titleCase = shortName
    .split("_")
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
  return {
    displayName: titleCase,
    logoKind: "initial",
    logoKey: "",
    brandColor: "64748B", // slate-500 中性兜底
  };
}
