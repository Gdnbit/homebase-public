import type { SecretInfo } from "./tauri-api";

export const EXPIRY_WARNING_MS = 7 * 24 * 60 * 60 * 1000;
export type ExpiryState =
  "unknown" | "invalid" | "expired" | "expiring" | "valid";

function validTimestamp(value: string) {
  const match =
    /^(\d{4})-(\d{2})-(\d{2})T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.exec(
      value,
    );
  if (!match) return false;
  const year = Number(match[1]),
    month = Number(match[2]),
    day = Number(match[3]);
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1];
}

export function expiryStatus(expiresAt: string | null, now = Date.now()) {
  if (!expiresAt)
    return {
      state: "unknown" as ExpiryState,
      warning: false,
      label: "未记录到期时间",
    };
  const expiry = Date.parse(expiresAt);
  if (!validTimestamp(expiresAt) || !Number.isFinite(expiry))
    return {
      state: "invalid" as ExpiryState,
      warning: true,
      label: "到期时间无效，请修正",
    };
  const remaining = expiry - now;
  const date = new Date(expiry).toLocaleString("zh-CN", { hour12: false });
  if (remaining <= 0)
    return {
      state: "expired" as ExpiryState,
      warning: true,
      label: `已到期 · ${date}`,
    };
  if (remaining <= EXPIRY_WARNING_MS)
    return {
      state: "expiring" as ExpiryState,
      warning: true,
      label: `将在 ${Math.ceil(remaining / 86400000)} 天内到期 · ${date}`,
    };
  return {
    state: "valid" as ExpiryState,
    warning: false,
    label: `到期时间 · ${date}`,
  };
}

export function isConfigured(secret: Pick<SecretInfo, "configured">) {
  return secret.configured === true;
}
export function secretStatus(
  secret: Pick<SecretInfo, "configured" | "expires_at">,
  now = Date.now(),
) {
  if (!isConfigured(secret)) return "missing";
  return expiryStatus(secret.expires_at, now).warning ? "warning" : "filled";
}

/** The selected calendar day ends in the user's local timezone, not UTC. */
export function localDateToExpiry(day: string) {
  if (!day) return "";
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day);
  if (!match) throw new Error("请选择有效日期");
  const [, y, m, d] = match.map(Number);
  const value = new Date(0);
  value.setFullYear(y, m - 1, d);
  value.setHours(23, 59, 59, 999);
  if (
    value.getFullYear() !== y ||
    value.getMonth() !== m - 1 ||
    value.getDate() !== d
  )
    throw new Error("请选择有效日期");
  return value.toISOString();
}
export function expiryToLocalDate(value: string | null) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  return `${String(date.getFullYear()).padStart(4, "0")}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}
