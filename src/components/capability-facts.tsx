import type { EvidenceInfo, PriceInfo } from "@/lib/capability-facts";

const rateNames = { prompt: "输入", completion: "输出", request: "请求" };
function time(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? "时间未知"
    : date.toLocaleString("zh-CN", { hour12: false });
}

export function CapabilityPricing({
  pricing,
  routeId,
}: {
  pricing?: PriceInfo | null;
  routeId: string;
}) {
  if (
    !pricing ||
    pricing.route_id !== routeId ||
    pricing.reference_only !== true
  ) {
    return (
      <p className="mt-3 text-xs text-muted-foreground">
        参考价格未知，调用前需核对所选渠道的费用。
      </p>
    );
  }
  return (
    <section
      aria-label="参考价格"
      className="mt-3 space-y-2 rounded-lg bg-secondary/40 p-3 text-xs text-muted-foreground"
    >
      <p className="font-medium text-foreground">
        渠道参考价格
        {pricing.state === "partial"
          ? " · 部分维度"
          : pricing.state === "invalid"
            ? " · 无有效报价"
            : ""}
      </p>
      {pricing.rates.map((rate) => (
        <p key={rate.kind} className="break-all">
          {rateNames[rate.kind]}：
          {rate.kind === "request"
            ? `${rate.amount} USD / 次`
            : `${rate.per_million_tokens ?? "未知"} USD / 百万 token`}
        </p>
      ))}
      {pricing.conditional && (
        <p>含条件价或折扣，以上仅为基础报价，未套用条件。</p>
      )}
      {pricing.unmodeled_dimensions > 0 && (
        <p>
          另有 {pricing.unmodeled_dimensions}{" "}
          个未覆盖计费维度，不能据此估算总费用。
        </p>
      )}
      {pricing.missing_dimensions > 0 && (
        <p>
          {pricing.missing_dimensions} 个基础维度未提供，缺失价格不按零计算。
        </p>
      )}
      {pricing.invalid_dimensions > 0 && (
        <p>{pricing.invalid_dimensions} 个报价无效，未用于比较。</p>
      )}
      <p>
        目录参考价不代表账号权限、额度或最终账单；零报价也不保证可免费调用。
      </p>
      <p>
        {pricing.source_url.includes("/models/user")
          ? "账号筛选目录"
          : "公开模型目录"}{" "}
        · 采集于 {time(pricing.observed_at)} ·{" "}
        <a
          className="underline"
          href="https://openrouter.ai/docs/client-sdks/typescript/api-reference/models/models"
          target="_blank"
          rel="noreferrer"
        >
          官方说明
        </a>
      </p>
    </section>
  );
}

export function CapabilityEvidence({
  provenance,
}: {
  provenance?: EvidenceInfo | null;
}) {
  if (!provenance) return null;
  return (
    <details className="mt-3 break-words text-xs text-muted-foreground">
      <summary>
        {provenance.state === "records_checked"
          ? "登记与验收记录一致"
          : "验收登记 · 运行前核对"}
      </summary>
      <p className="mt-2">
        版本 {provenance.package_version} · 登记于{" "}
        {time(provenance.registered_at)}
      </p>
      {provenance.checked_at && (
        <p>原验收时间：{time(provenance.checked_at)}</p>
      )}
      <p className="mt-2 break-all">来源 commit：{provenance.source_commit}</p>
      <p className="mt-2 break-all">
        清单 SHA-256：{provenance.manifest_sha256}
      </p>
      <p className="mt-2 break-all">
        验收记录 SHA-256：{provenance.verification_sha256}
      </p>
      <p className="mt-2">
        此处未检查当前安装代码完整性；执行入口会再次校验，不代表本轮任务已执行成功。
      </p>
    </details>
  );
}
