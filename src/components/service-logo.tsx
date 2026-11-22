/**
 * ServiceLogo — 按 short_name 渲染服务官方 logo
 *
 * 三层来源（由 service-registry.getServiceMeta 决定）：
 * 1. **lobe**：@lobehub/icons（AI 厂商：Anthropic/OpenAI/Gemini/DeepSeek/OpenRouter）
 * 2. **si**：react-icons 中可用的品牌图标（GitHub/Figma/Slack/Twilio 等）
 * 3. **initial**：品牌色 + 首字符（GDNbit 自有 / 新服务兜底）
 */

// 通过本地 barrel 引入 lobehub Mono 子模块，避开顶层导出的 antd 依赖
// 详见 @/lib/lobehub-icons 的注释
import {
  Anthropic,
  Aws,
  Azure,
  Bing,
  Claude,
  Cohere,
  DeepSeek,
  Gemini,
  Grok,
  Groq,
  HuggingFace,
  Mistral,
  OpenAI,
  OpenRouter,
  Perplexity,
  Together,
  XAI,
} from "@/lib/lobehub-icons";
import type { IconType } from "react-icons";
import { FaSlack as SiSlack } from "react-icons/fa6";
import { TbBrandTwilio as SiTwilio } from "react-icons/tb";
import {
  SiAuth0,
  SiBitwarden,
  SiBrave,
  SiCloudflare,
  SiDiscord,
  SiDocker,
  SiElasticsearch,
  SiFigma,
  SiGithub,
  SiGooglecloud,
  SiJsonwebtokens,
  SiLinear,
  SiMongodb,
  SiMysql,
  SiNextdotjs,
  SiNotion,
  SiNpm,
  SiObsidian,
  SiPostgresql,
  SiRedis,
  SiResend,
  SiStripe,
  SiSupabase,
  SiVercel,
} from "react-icons/si";
import { cn } from "@/lib/utils";
import { getServiceMeta, type LogoKind } from "@/lib/service-registry";

// ========= lobehub 图标映射表 =========
// 覆盖 Tier 1 + Tier 2 中所有 logoKind="lobe" 的服务
const LOBE_LOGOS: Record<string, React.FC<{ size?: number }>> = {
  Anthropic,
  Aws,
  Azure,
  Bing,
  Claude,
  Cohere,
  DeepSeek,
  Gemini,
  Grok,
  Groq,
  HuggingFace,
  Mistral,
  OpenAI,
  OpenRouter,
  Perplexity,
  Together,
  XAI,
};

// ========= simple-icons 映射表 =========
// 覆盖 Tier 1 + Tier 2 中所有 logoKind="si" 的服务
// SendGrid 已从上游图标集移除，继续使用 renderLogo 的品牌首字母兜底。
const SI_LOGOS: Record<string, IconType> = {
  SiAuth0,
  SiBitwarden,
  SiBrave,
  SiCloudflare,
  SiDiscord,
  SiDocker,
  SiElasticsearch,
  SiFigma,
  SiGithub,
  SiGooglecloud,
  SiJsonwebtokens,
  SiLinear,
  SiMongodb,
  SiMysql,
  SiNextdotjs,
  SiNotion,
  SiNpm,
  SiObsidian,
  SiPostgresql,
  SiRedis,
  SiResend,
  SiSlack,
  SiStripe,
  SiSupabase,
  SiTwilio,
  SiVercel,
};

interface ServiceLogoProps {
  /** 密钥短名（如 "figma_token"） */
  shortName: string;
  /** 尺寸（像素，默认 32） */
  size?: number;
  /** 附加 className */
  className?: string;
}

export function ServiceLogo({ shortName, size = 32, className }: ServiceLogoProps) {
  const meta = getServiceMeta(shortName);
  return renderLogo(meta.logoKind, meta.logoKey, meta.brandColor, meta.displayName, size, className);
}

function renderLogo(
  kind: LogoKind,
  logoKey: string,
  brandColor: string | undefined,
  displayName: string,
  size: number,
  className?: string,
) {
  const containerClass = cn(
    "flex shrink-0 items-center justify-center rounded-lg",
    className,
  );
  const style = { width: size, height: size };

  // 1. lobehub
  if (kind === "lobe") {
    const LobeComponent = LOBE_LOGOS[logoKey];
    if (LobeComponent) {
      return (
        <div className={cn(containerClass, "bg-muted/30 p-1")} style={style} aria-label={displayName}>
          <LobeComponent size={Math.floor(size * 0.72)} />
        </div>
      );
    }
  }

  // 2. simple-icons（react-icons/si）
  if (kind === "si") {
    const SiComponent = SI_LOGOS[logoKey];
    if (SiComponent) {
      const color = brandColor ? `#${brandColor}` : undefined;
      return (
        <div className={cn(containerClass, "bg-muted/30 p-1")} style={style} aria-label={displayName}>
          <SiComponent size={Math.floor(size * 0.68)} style={{ color }} />
        </div>
      );
    }
  }

  // 3. initial（品牌色背景 + 首字符）
  const initial = displayName.charAt(0).toUpperCase();
  const bg = brandColor ?? "64748B";
  return (
    <div
      className={cn(containerClass, "font-serif text-sm font-bold text-white")}
      style={{ ...style, backgroundColor: `#${bg}` }}
      aria-label={displayName}
    >
      {initial}
    </div>
  );
}
