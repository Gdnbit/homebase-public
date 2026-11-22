import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { X } from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { invoke } from "@tauri-apps/api/core";
import {
  secretsApi,
  type HealthResult,
  type SaveSecretRequest,
  type SecretInfo,
} from "@/lib/tauri-api";
import {
  expiryStatus,
  expiryToLocalDate,
  isConfigured,
  localDateToExpiry,
} from "@/lib/secret-lifecycle";
import { useClock } from "@/lib/use-clock";
import { listKnownServices } from "@/lib/service-registry";
import { refreshCapabilityCatalog } from "@/lib/capability-index";
import { isNativeClosePending, registerVaultCloseStatus, useNativeClosePending } from "@/lib/assistant-draft-lifecycle";
import { ServiceLogo } from "./service-logo";

function initialForm(secret: SecretInfo) {
  return {
    display_name: secret.display_name ?? "",
    expires_at: secret.expires_at ?? "",
    service_id: secret.service_id ?? "",
    rotate_url: secret.rotate_url ?? "",
    expiry_policy: secret.expiry_policy || "warn",
  };
}

export function SecretLifecycleRow({
  secret,
  onDirtyChange,
}: {
  secret: SecretInfo;
  onDirtyChange?: (name: string, dirty: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const now = useClock();
  const nativeClosePending = useNativeClosePending();
  const [expanded, setExpanded] = useState(false);
  const dialog = useRef<HTMLDialogElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  useEffect(() => {
    if (!expanded) return;
    const element = dialog.current;
    element?.showModal();
    return () => {
      element?.close();
      trigger.current?.focus();
    };
  }, [expanded]);
  const [form, setForm] = useState(() => initialForm(secret));
  const [saved, setSaved] = useState(() => JSON.stringify(initialForm(secret)));
  const [candidate, setCandidate] = useState("");
  const [candidateExpiry, setCandidateExpiry] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef<false | "saving" | "testing">(false);
  const mounted = useRef(true);
  const [error, setError] = useState("");
  const [feedback, setFeedback] = useState("");
  const [health, setHealth] = useState<HealthResult | null>(null);
  const [plain, setPlain] = useState<string | null>(null);
  const [revealing, setRevealing] = useState(false);
  const revealEpoch = useRef(0);
  const [remote, setRemote] = useState<SecretInfo | null>(null);
  const version = useRef({
    revision: secret.revision,
    version: secret.vault_version,
  });
  const serialized = JSON.stringify(form);
  const dirty = serialized !== saved;
  const hasDraft = dirty || !!candidate || !!candidateExpiry;
  const expiry = expiryStatus(secret.expires_at, now);
  const configured = isConfigured(secret);
  const pending = secret.rotations.find(
    (r) => !r.revoked_at && !r.rolled_back_at,
  );

  useEffect(() => {
    onDirtyChange?.(secret.short_name, hasDraft);
    return () => onDirtyChange?.(secret.short_name, false);
  }, [hasDraft, onDirtyChange, secret.short_name]);

  useLayoutEffect(() => {
    if (!expanded) return;
    return registerVaultCloseStatus(() => {
      if (busyRef.current === "saving") return "saving";
      return hasDraft ? "draft" : "clean";
    });
  }, [expanded, hasDraft]);

  useEffect(() => {
    mounted.current = true;
    const hide = () => {
      revealEpoch.current++;
      setPlain(null);
    };
    window.addEventListener("blur", hide);
    document.addEventListener("visibilitychange", hide);
    return () => {
      mounted.current = false;
      revealEpoch.current++;
      window.removeEventListener("blur", hide);
      document.removeEventListener("visibilitychange", hide);
    };
  }, []);

  useEffect(() => {
    if (!dirty && !busyRef.current && !error) {
      const next = initialForm(secret);
      setForm(next);
      setSaved(JSON.stringify(next));
      version.current = {
        revision: secret.revision,
        version: secret.vault_version,
      };
    }
    setHealth(null);
    // Only server updates synchronize a clean editor. Draft changes stay local.
  }, [secret]);

  useEffect(() => {
    setConfirmed(false);
    setHealth(null);
  }, [form.service_id, candidateExpiry]);

  useEffect(() => {
    if (!plain) return;
    const hide = () => {
      revealEpoch.current++;
      setPlain(null);
    };
    const timer = setTimeout(hide, 30_000);
    window.addEventListener("blur", hide);
    document.addEventListener("visibilitychange", hide);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("blur", hide);
      document.removeEventListener("visibilitychange", hide);
    };
  }, [plain]);

  async function save(
    extra: Partial<SaveSecretRequest> = {},
    automatic = false,
  ) {
    if (busyRef.current || isNativeClosePending()) return;
    busyRef.current = "saving";
    setBusy(true);
    setError("");
    setFeedback(automatic ? "元数据自动保存中…" : "保存中…");
    const captured = JSON.stringify(form);
    const hasValue = typeof extra.value === "string";
    try {
      const result = await secretsApi.save({
        short_name: secret.short_name,
        expected_revision: version.current.revision,
        expected_vault_version: version.current.version,
        ...form,
        ...extra,
      });
      if (hasValue && !result.verified)
        throw new Error("服务未确认写后校验，请刷新核对；草稿已保留");
      version.current = {
        revision: result.revision,
        version: result.vault_version,
      };
      if (mounted.current) {
        if (
          extra.operation !== "rollback_rotation" &&
          extra.operation !== "confirm_revocation"
        )
          setSaved(captured);
        if (hasValue) {
          const committed = {
            ...form,
            expires_at: extra.expires_at ?? form.expires_at,
          };
          setForm(committed);
          setSaved(JSON.stringify(committed));
          setCandidate("");
          setCandidateExpiry("");
          setConfirmed(false);
          revealEpoch.current++;
          setPlain(null);
          setHealth(null);
        }
        setFeedback(
          hasValue
            ? "密钥与元数据已一起保存，逐字节校验通过"
            : extra.operation === "confirm_revocation"
              ? "已记录你对服务商停用旧密钥的确认"
              : extra.operation === "rollback_rotation"
                ? "已撤回本次切换，其他密钥未改变"
                : "元数据已保存",
        );
      }
      refreshCapabilityCatalog(queryClient);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["secrets", "list"] }),
        queryClient.invalidateQueries({ queryKey: ["model-capabilities", "catalog"] }),
        queryClient.invalidateQueries({ queryKey: ["secrets", "lifecycle"] }),
      ]);
      if (extra.operation === "rollback_rotation" && mounted.current) {
        const latest = await secretsApi.list();
        const current = latest.find((s) => s.short_name === secret.short_name);
        if (current) {
          const next = initialForm(current);
          setForm(next);
          setSaved(JSON.stringify(next));
          version.current = {
            revision: current.revision,
            version: current.vault_version,
          };
        }
      }
    } catch (error) {
      if (mounted.current) {
        setError(String(error));
        setFeedback("保存未确认，草稿保留；请查看原因后重试");
      }
    } finally {
      busyRef.current = false;
      if (mounted.current) setBusy(false);
    }
  }

  useEffect(() => {
    if (!expanded || !dirty || busy || error || nativeClosePending) return;
    const timer = setTimeout(() => {
      void save({}, true);
    }, 800);
    return () => clearTimeout(timer);
    // save captures exactly the draft represented by serialized.
  }, [expanded, serialized, saved, busy, error, nativeClosePending]);

  async function refreshVersion() {
    try {
      const list = await secretsApi.list();
      const current = list.find((s) => s.short_name === secret.short_name);
      if (!current) throw new Error("服务端已移除此密钥，请刷新列表");
      version.current = {
        revision: current.revision,
        version: current.vault_version,
      };
      setRemote(current);
      setFeedback("已读取最新版本，请对照服务端元数据，再决定是否按草稿重试");
    } catch (error) {
      setError(String(error));
    }
  }

  async function testCandidate() {
    if (busyRef.current) return;
    busyRef.current = "testing";
    setBusy(true);
    setError("");
    try {
      const result = await secretsApi.testCandidate(
        form.service_id || secret.short_name,
        candidate,
      );
      if (mounted.current) setHealth(result);
    } catch (error) {
      if (mounted.current) setError(String(error));
    } finally {
      busyRef.current = false;
      if (mounted.current) setBusy(false);
    }
  }

  function toggle() {
    if (busyRef.current) return;
    if (
      expanded &&
      hasDraft &&
      !window.confirm("有未保存内容。关闭编辑会丢弃草稿，是否继续？")
    )
      return;
    if (expanded) {
      const next = initialForm(secret);
      setForm(next);
      setSaved(JSON.stringify(next));
      setCandidate("");
      setCandidateExpiry("");
      setConfirmed(false);
      revealEpoch.current++;
      setPlain(null);
      setError("");
    }
    setExpanded(!expanded);
  }

  return (
    <li
      className="hb-card min-w-0 min-h-[176px] space-y-4"
      data-secret-name={secret.short_name}
    >
      <button
        ref={trigger}
        type="button"
        className="flex w-full items-start gap-3 text-left"
        onClick={toggle}
        disabled={busy}
        aria-expanded={expanded}
        aria-haspopup="dialog"
      >
        <ServiceLogo shortName={secret.service_id || secret.short_name} />
        <span className="min-w-0 flex-1">
          <strong className="block break-words leading-snug">
            {secret.display_name || secret.short_name}
          </strong>
          <span className="mt-1 block break-all text-xs text-muted-foreground">
            {secret.short_name}
          </span>
          <span className="mt-3 block text-xs text-muted-foreground">
            {!configured
              ? secret.value_state === "invalid"
                ? "值格式异常"
                : "未填"
              : expiry.warning
                ? "已填 · 到期告警"
                : "已填 · 未证明有效"}
          </span>
        </span>
        <span className="shrink-0 text-muted-foreground" aria-hidden>
          ↗
        </span>
      </button>
      <p
        className={
          expiry.warning
            ? "text-sm text-warning-strong"
            : "text-xs text-muted-foreground"
        }
      >
        {expiry.label}
        {secret.expiry_policy === "block" ? " · 到期停止提供" : " · 仅提醒"}
      </p>
      {expanded && (
        <dialog
          ref={dialog}
          aria-labelledby={titleId}
          aria-modal="true"
          onCancel={(event) => {
            event.preventDefault();
            toggle();
          }}
          onClick={(event) => {
            if (event.target === event.currentTarget) toggle();
          }}
          className="hb-editor-dialog !m-auto max-h-[88vh] w-[calc(100%-2rem)] max-w-2xl overflow-auto rounded-xl border border-border bg-card p-0 text-foreground shadow-2xl backdrop:bg-black/50"
        >
          <div className="space-y-4 p-5 sm:p-6">
            <header className="flex items-start justify-between gap-4 border-b border-border pb-4">
              <div className="min-w-0">
                <h2
                  id={titleId}
                  className="break-words font-serif text-xl font-semibold"
                >
                  {secret.display_name || secret.short_name}
                </h2>
                <p className="mt-1 break-all text-xs text-muted-foreground">
                  {secret.short_name} · 编辑密钥
                </p>
              </div>
              <button
                type="button"
                autoFocus
                aria-label="关闭密钥编辑"
                disabled={busy}
                onClick={toggle}
                className="shrink-0 rounded p-1.5 text-muted-foreground hover:bg-secondary disabled:opacity-50"
              >
                <X className="h-4 w-4" />
              </button>
            </header>
            <p className="break-all font-mono text-sm">
              {plain ?? (configured ? secret.masked_value : "当前无可用密钥值")}
            </p>
            {configured && (
              <button
                type="button"
                className="hb-btn-outline text-xs"
                disabled={busy || revealing}
                onClick={async () => {
                  if (plain) {
                    revealEpoch.current++;
                    setPlain(null);
                    return;
                  }
                  const epoch = ++revealEpoch.current;
                  setRevealing(true);
                  try {
                    const value = await secretsApi.reveal(secret.short_name);
                    if (mounted.current && revealEpoch.current === epoch)
                      setPlain(value);
                  } catch (error) {
                    if (mounted.current) setError(String(error));
                  } finally {
                    if (mounted.current) setRevealing(false);
                  }
                }}
              >
                {plain ? "隐藏密钥" : "显示密钥（30 秒后隐藏）"}
              </button>
            )}
            <fieldset className="grid gap-3 sm:grid-cols-2" disabled={busy}>
              <legend className="mb-2 text-sm font-medium">
                元数据自动保存；密钥正文需点击保存
              </legend>
              <label className="text-sm">
                显示名称
                <input
                  className="hb-input mt-1 w-full"
                  value={form.display_name}
                  onChange={(e) =>
                    setForm({ ...form, display_name: e.target.value })
                  }
                  maxLength={120}
                />
              </label>
              <label className="text-sm">
                到期日期（本地日末）
                <input
                  type="date"
                  className="hb-input mt-1 w-full"
                  value={expiryToLocalDate(form.expires_at)}
                  onChange={(e) => {
                    try {
                      setForm({
                        ...form,
                        expires_at: localDateToExpiry(e.target.value),
                      });
                    } catch (error) {
                      setError(String(error));
                    }
                  }}
                />
              </label>
              <label className="text-sm">
                到期策略
                <select
                  className="hb-input mt-1 w-full"
                  value={form.expiry_policy}
                  onChange={(e) =>
                    setForm({
                      ...form,
                      expiry_policy: e.target.value as "warn" | "block",
                    })
                  }
                >
                  <option value="warn">仅提醒，保持现有行为</option>
                  <option value="block">到期停止向消费者提供</option>
                </select>
              </label>
              <label className="text-sm">
                服务配置
                <select
                  className="hb-input mt-1 w-full"
                  value={form.service_id}
                  onChange={(e) =>
                    setForm({ ...form, service_id: e.target.value })
                  }
                >
                  <option value="">按短名识别／自定义</option>
                  {listKnownServices()
                    .filter((s) => s.tier === 1)
                    .map((s) => (
                      <option
                        key={s.key}
                        value={s.meta.suggestedShortName || s.key}
                      >
                        {s.meta.displayName}
                      </option>
                    ))}
                </select>
              </label>
              <label className="text-sm sm:col-span-2">
                服务商密钥管理链接（HTTPS）
                <input
                  type="url"
                  className="hb-input mt-1 w-full"
                  value={form.rotate_url}
                  onChange={(e) =>
                    setForm({ ...form, rotate_url: e.target.value })
                  }
                />
              </label>
            </fieldset>
            <p className="text-xs text-muted-foreground">
              服务配置用于检测接口与服务商链接；不会自动改写 Agent／MCP
              对密钥短名的绑定。
            </p>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                className="hb-btn-outline text-xs"
                disabled={busy}
                onClick={() => {
                  setForm({ ...form, expires_at: "" });
                }}
              >
                清除到期日期
              </button>
              <button
                type="button"
                className="hb-btn-outline text-xs"
                disabled={busy || !dirty}
                onClick={() => void save()}
              >
                立即保存元数据／重试
              </button>
              {secret.rotate_url && (
                <button
                  type="button"
                  className="hb-btn-outline text-xs"
                  onClick={async () => {
                    try {
                      const url = new URL(secret.rotate_url!);
                      if (
                        url.protocol !== "https:" ||
                        url.username ||
                        url.password
                      )
                        throw new Error(
                          "服务商链接必须是无登录凭据的 HTTPS 地址",
                        );
                      await openExternal(url.toString());
                    } catch {
                      setError("服务商链接无效或无法打开，请检查链接");
                    }
                  }}
                >
                  打开服务商密钥页面
                </button>
              )}
            </div>
            <section className="space-y-3 rounded-md border p-3">
              <h3 className="font-medium">
                {configured ? "轮换密钥" : "保存密钥"}
              </h3>
              <p className="text-xs text-muted-foreground">
                新值先加密暂存并验证，提交成功前当前密钥不变。密钥正文不会自动保存或持久化为明文草稿。
              </p>
              <label className="block text-sm">
                {configured ? "新密钥值" : "密钥值"}
                <input
                  type="password"
                  autoComplete="new-password"
                  className="hb-input mt-1 w-full"
                  value={candidate}
                  disabled={busy}
                  onChange={(e) => {
                    setCandidate(e.target.value);
                    setConfirmed(false);
                    setHealth(null);
                  }}
                />
              </label>
              <button
                type="button"
                className="hb-btn-outline text-xs"
                disabled={busy}
                onClick={async () => {
                  try {
                    const result = await invoke<
                      string | { plainText?: string } | null
                    >("plugin:clipboard-manager|read_text");
                    if (mounted.current) {
                      setCandidate(
                        typeof result === "string"
                          ? result
                          : (result?.plainText ?? ""),
                      );
                      setConfirmed(false);
                      setHealth(null);
                    }
                  } catch (error) {
                    if (mounted.current) setError(String(error));
                  }
                }}
              >
                从剪贴板填入草稿
              </button>
              <label className="block text-sm">
                {configured
                  ? "新密钥到期日期（留空为未记录）"
                  : "密钥到期日期（留空为未记录）"}
                <input
                  type="date"
                  className="hb-input mt-1 w-full"
                  value={candidateExpiry}
                  disabled={busy}
                  onChange={(e) => setCandidateExpiry(e.target.value)}
                />
              </label>
              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  className="hb-btn-outline text-xs"
                  disabled={busy || !candidate.trim()}
                  onClick={() => void testCandidate()}
                >
                  测试候选密钥（可能产生服务费用）
                </button>
                <button
                  type="button"
                  className="hb-btn-outline text-xs"
                  disabled={busy}
                  onClick={() => {
                    setCandidate("");
                    setConfirmed(false);
                    setHealth(null);
                  }}
                >
                  清除密钥草稿
                </button>
              </div>
              {health && (
                <p role="status" className="text-sm">
                  {health.success
                    ? "本次候选密钥检测通过"
                    : `未确认有效：${health.status}`}{" "}
                  {health.error}
                </p>
              )}
              {configured && (
                <label className="flex gap-2 text-sm">
                  <input
                    type="checkbox"
                    checked={confirmed}
                    disabled={busy}
                    onChange={(e) => setConfirmed(e.target.checked)}
                  />
                  我已通过检测或服务商确认新密钥可用，并核对了新到期日期；旧密钥暂未撤销
                </label>
              )}
              <button
                type="button"
                className="hb-btn-primary"
                disabled={
                  busy ||
                  !candidate.trim() ||
                  (configured && (!confirmed || !!pending))
                }
                onClick={() =>
                  void save({
                    value: candidate,
                    expires_at: localDateToExpiry(candidateExpiry),
                    operation: configured ? "rotate" : "save",
                    candidate_verified: confirmed,
                  })
                }
              >
                {busy
                  ? "操作中…"
                  : configured
                    ? "验证并切换到新密钥"
                    : "加密保存密钥"}
              </button>
            </section>
            {pending && (
              <section className="space-y-3 rounded-md border border-warning/40 p-3">
                <h3 className="font-medium">旧密钥待停用</h3>
                <p className="text-sm">
                  新密钥已切换。请验证下游使用情况，再到服务商处停用旧密钥；本应用不会自动申请或撤销服务商凭据。
                </p>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    className="hb-btn-outline text-xs"
                    disabled={busy}
                    onClick={() => {
                      if (
                        window.confirm(
                          "请仅在你已在服务商处停用旧密钥后确认。此操作只记录确认，不会调用服务商撤销接口。",
                        )
                      )
                        void save({
                          operation: "confirm_revocation",
                          rotation_id: pending.id,
                        });
                    }}
                  >
                    确认旧密钥已在服务商停用
                  </button>
                  <button
                    type="button"
                    className="hb-btn-outline text-xs"
                    disabled={busy}
                    onClick={() => {
                      if (
                        window.confirm(
                          "请确认旧密钥仍有效且未在服务商停用。将只恢复这把密钥及其原到期策略，不覆盖其他密钥。",
                        )
                      )
                        void save({
                          operation: "rollback_rotation",
                          rotation_id: pending.id,
                          candidate_verified: true,
                        });
                    }}
                  >
                    撤回切换，恢复旧密钥
                  </button>
                </div>
              </section>
            )}
            {secret.rotations.length > 0 && (
              <details>
                <summary className="text-sm">
                  轮换记录（{secret.rotations.length}）
                </summary>
                <ul className="mt-2 space-y-1 text-xs">
                  {secret.rotations
                    .slice()
                    .reverse()
                    .map((r) => (
                      <li key={r.id}>
                        {new Date(r.changed_at).toLocaleString()} ·{" "}
                        {r.rolled_back_at
                          ? "已撤回切换"
                          : r.revoked_at
                            ? "用户已确认旧密钥停用"
                            : "待确认旧密钥停用"}{" "}
                        · 新值验证来源：人工确认
                      </li>
                    ))}
                </ul>
              </details>
            )}
            <p role="status" aria-live="polite" className="text-sm">
              {feedback ||
                (dirty ? "有元数据更改待保存" : "元数据与服务端一致")}
            </p>
            {error && (
              <div
                role="alert"
                className="space-y-2 text-sm text-destructive-strong"
              >
                <p>{error}</p>
                <button
                  type="button"
                  className="hb-btn-outline text-xs"
                  disabled={busy}
                  onClick={() => void refreshVersion()}
                >
                  刷新服务器状态，保留草稿
                </button>
                {remote && (
                  <>
                    <p>
                      服务端：{remote.display_name || remote.short_name} ·{" "}
                      {expiryStatus(remote.expires_at, now).label}
                    </p>
                    <button
                      type="button"
                      className="hb-btn-outline text-xs"
                      disabled={busy}
                      onClick={() => void save()}
                    >
                      已核对，按当前元数据草稿重试
                    </button>
                  </>
                )}
              </div>
            )}
          </div>
        </dialog>
      )}
    </li>
  );
}
