// This fixture has no native bridge, disk vault or external service access.
import React from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { Secrets } from "../../src/routes/Secrets";
import { SecretExpiryIndicator } from "../../src/components/secret-expiry-indicator";
import type { SecretInfo, SaveSecretRequest } from "../../src/lib/tauri-api";
import { installDraftLifecycle, requestDraftClose, useDraftCloseNotice } from "../../src/lib/assistant-draft-lifecycle";
import "../../src/index.css";

const state = {
  generation: 0,
  calls: [] as Array<{ command: string; args: any }>,
  failNextSave: false,
  conflictNextSave: false,
  failReads: false,
  delay: 0,
  closeChoice: false,
  closeDecision: null as Promise<void> | null,
  closeResult: null as Promise<void> | null,
  notifications: [] as unknown[],
  values: { github_token: "synthetic-current-value", empty_key: "" } as Record<
    string,
    string
  >,
  secrets: [] as SecretInfo[],
};
function entry(name: string, display: string, value: string): SecretInfo {
  return {
    short_name: name,
    display_name: display,
    mcp_long_name: null,
    used_by: [],
    category: "core",
    configured: !!value.trim(),
    value_state: value.trim() ? "filled" : "empty",
    masked_value: value.trim() ? "synthetic…****…alue" : "(empty)",
    last_health: null,
    expires_at: null,
    revision: 0,
    vault_version: "fixture-0",
    service_id: name === "github_token" ? "github_token" : null,
    expiry_policy: "warn",
    rotations: [],
    has_health_check: true,
    rotate_url: "https://example.invalid/keys",
  };
}
state.secrets = [
  entry("github_token", "GitHub 隔离测试", state.values.github_token),
  entry("empty_key", "空值隔离测试", ""),
];
(window as any).__fixture = state;
(window as any).isTauri = true;
const callbacks = new Map<number, (event: unknown) => void>();
let callbackId = 0;
let closeHandler: number | undefined;
(window as any).__secretClose = {
  request: requestDraftClose,
  ready: () => closeHandler !== undefined,
  system: () => {
    if (closeHandler === undefined) throw new Error("Expected fixture close listener");
    callbacks.get(closeHandler)?.({ id: 1, event: "tauri://close-requested", payload: null });
  },
};
(window as any).Notification = class {
  static permission = "granted";
  constructor(title: string, options: unknown) {
    state.notifications.push({ title, options });
  }
};
(window as any).__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: "main" } },
  transformCallback: (cb: (event: unknown) => void) => {
    callbacks.set(++callbackId, cb);
    return callbackId;
  },
  invoke: async (command: string, args: any = {}) => {
    state.calls.push({ command, args: structuredClone(args) });
    if (command === "plugin:event|listen" && args.event === "tauri://close-requested") {
      closeHandler = args.handler;
      return 1;
    }
    if (command === "plugin:dialog|message") {
      if (state.closeDecision) await state.closeDecision;
      return args.buttons.OkCancelCustom[state.closeChoice ? 0 : 1];
    }
    if (command === "plugin:window|destroy") return state.closeResult;
    if (state.delay)
      await new Promise((resolve) => setTimeout(resolve, state.delay));
    if (command === "secrets_list") {
      if (state.failReads) throw new Error("模拟读取失败");
      return structuredClone(state.secrets);
    }
    if (command === "secrets_lifecycle_status") {
      if (state.failReads) throw new Error("模拟读取失败");
      return state.secrets.map((s) => ({
        ...s,
        pending_revocation: s.rotations.some(
          (r) => !r.revoked_at && !r.rolled_back_at,
        ),
      }));
    }
    if (command === "secrets_save") {
      if (state.failNextSave) {
        state.failNextSave = false;
        throw new Error("模拟写入失败，原数据未改变");
      }
      if (state.conflictNextSave) {
        state.conflictNextSave = false;
        state.generation++;
        state.secrets.forEach(
          (s) => (s.vault_version = `fixture-${state.generation}`),
        );
      }
      const r = args.request as SaveSecretRequest;
      let current = state.secrets.find((s) => s.short_name === r.short_name);
      if (
        r.expected_vault_version !== `fixture-${state.generation}` ||
        (current?.revision ?? 0) !== r.expected_revision
      )
        throw new Error("密钥库已变化，请刷新并核对后重试");
      if (!current) {
        current = entry(r.short_name, r.display_name || r.short_name, "");
        state.secrets.push(current);
      }
      if (r.operation === "rollback_rotation") {
        const record = current.rotations.find(
          (item) => item.id === r.rotation_id,
        )!;
        record.rolled_back_at = new Date().toISOString();
        state.values[r.short_name] = "synthetic-current-value";
      } else if (r.operation === "confirm_revocation") {
        current.rotations.find(
          (item) => item.id === r.rotation_id,
        )!.revoked_at = new Date().toISOString();
      } else {
        for (const key of [
          "display_name",
          "expires_at",
          "service_id",
          "rotate_url",
          "expiry_policy",
        ] as const)
          if (r[key] !== undefined && r[key] !== null)
            (current as any)[key] = r[key] || null;
        if (r.value !== undefined && r.value !== null) {
          state.values[r.short_name] = r.value;
          current.configured = true;
          current.value_state = "filled";
          current.masked_value = "new…****…alue";
          if (r.operation === "rotate")
            current.rotations.push({
              id: `rotation-${current.rotations.length + 1}`,
              changed_at: new Date().toISOString(),
              validation: "user_confirmed",
              revoked_at: null,
              rolled_back_at: null,
            });
        }
      }
      current.revision++;
      state.generation++;
      state.secrets.forEach(
        (s) => (s.vault_version = `fixture-${state.generation}`),
      );
      return {
        revision: current.revision,
        vault_version: current.vault_version,
        backup_path: "ISOLATED-FIXTURE-ONLY",
        verified: r.value != null,
      };
    }
    if (command === "secrets_reveal") return state.values[args.shortName];
    if (command === "secrets_test_candidate")
      return {
        success: true,
        status: "valid",
        response_ms: 1,
        http_code: 200,
        error: null,
      };
    if (command === "plugin:clipboard-manager|read_text")
      return "synthetic-clipboard-only";
    if (command === "plugin:shell|open") return;
    if (command.includes("is_permission_granted")) return true;
    if (command.includes("request_permission")) return "granted";
    if (command.startsWith("plugin:notification")) return;
    throw new Error(`Non-fixture command rejected: ${command}`);
  },
};
void installDraftLifecycle();

function CloseFeedback() {
  const notice = useDraftCloseNotice();
  return <div data-testid="exit-notice">{notice?.text}</div>;
}
const queryClient = new QueryClient({
  defaultOptions: {
    queries: { retry: false, staleTime: 0, refetchOnWindowFocus: false },
  },
});
(window as any).__refreshFixture = () =>
  queryClient.invalidateQueries({ queryKey: ["secrets"] });
createRoot(document.getElementById("root")!).render(
  <QueryClientProvider client={queryClient}>
    <BrowserRouter>
      <main className="mx-auto max-w-5xl p-6">
        <p className="mb-4 font-bold">隔离测试数据 · 无真实密钥或服务连接</p>
        <CloseFeedback />
        <div className="relative mb-4 h-8 w-40">
          <SecretExpiryIndicator />
        </div>
        <Secrets />
      </main>
    </BrowserRouter>
  </QueryClientProvider>,
);
