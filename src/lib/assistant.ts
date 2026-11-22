import { invoke } from "@tauri-apps/api/core";
import type { DocumentReference } from "./document-types";
export interface AssistantSettings {
  revision: number;
  enabled: boolean;
  route_id: string;
  wechat_enabled: boolean;
  max_rounds: number;
  max_tokens: number;
  credential_source: "default" | "sops" | "api_key_manager";
  free_only: boolean;
  runtime?: "legacy" | "deepagents" | "pi";
}
export interface AssistantConfig {
  settings: AssistantSettings;
  workspace_root: string;
  model_state: string;
  message: string;
}
export interface AssistantSession {
  id: string;
  title: string;
  source: string;
  updated_at: string;
  message_count: number;
  active_turn_id: string | null;
  queue_depth?: number;
  queue_paused?: boolean;
}
export interface AssistantArtifact {
  id: string;
  job_id: string;
  name: string;
  path: string;
  bytes: number;
  sha256: string;
  mime_type: string;
}
export interface AssistantStepSummary {
  id: string;
  name: string;
  state: string;
}
export interface AssistantStep extends AssistantStepSummary {
  arguments: unknown;
  result: unknown;
  job_id: string | null;
}
export interface AssistantTurn {
  details_omitted?: boolean;
  step_count?: number;
  step_summaries?: AssistantStepSummary[];
  job_count?: number;
  artifact_count?: number;
  id: string;
  session_id: string;
  state: string;
  interaction?: "chat" | "task" | "";
  parent_turn_id?: string | null;
  requirements_revision?: number;
  steered_turn_id?: string | null;
  requirements?: {
    revision: number;
    request_id: string;
    text: string;
    received_at: string;
  }[];
  progress?: {
    goal: string;
    completed: string[];
    current: string | null;
    blocked: string | null;
    next: string | null;
    completed_stages: number;
    total_stages: number;
  };
  runtime?: string;
  runtime_version?: string;
  resumable?: boolean;
  resume_generation?: number;
  model_outcome_unknown?: boolean;
  pending_accepted_jobs?: number;
  unverified_accepted_jobs?: number;
  model_failure?: ModelFailureDiagnostic;
  model_diagnostics?: {
    attempts: number;
    elapsed_ms: number;
    retries: ModelFailureDiagnostic[];
  }[];
  queue_paused?: boolean;
  live_progress?: {
    state: string;
    output?: string;
    message?: string;
    event?: unknown;
  };
  task?: {
    plan: {
      revision: number;
      goal: string;
      items: {
        id: string;
        title: string;
        done_when: string;
        kind: string;
        state: string;
        note: string;
        evidence: string[];
      }[];
    } | null;
    stop: {
      kind: string;
      message: string;
      next_action: string;
      retry_safe: boolean;
    } | null;
    limits: {
      model_calls_used: number;
      max_model_calls: number;
      steps_used: number;
      max_steps: number;
      active_seconds: number;
      max_active_seconds: number;
    };
  };
  usage?: {
    model: string | null;
    cost: number | null;
    prompt_tokens: number | null;
    completion_tokens: number | null;
  }[];
  text: string;
  route_id: string;
  created_at: string;
  finished_at: string | null;
  steps: AssistantStep[];
  pending: {
    id: string;
    description: string;
    arguments: unknown;
    approved: boolean;
  } | null;
  jobs: { id: string; command: string; state: string; result: unknown }[];
  artifacts: AssistantArtifact[];
}
export interface ModelFailureDiagnostic {
  previous?: Pick<
    ModelFailureDiagnostic,
    "kind" | "detail" | "http_status" | "provider_status" | "elapsed_ms"
  >[];
  kind:
    | "build"
    | "connect"
    | "connect_timeout"
    | "timeout"
    | "send"
    | "http"
    | "provider"
    | "read"
    | "invalid_response"
    | "limit";
  acceptance: "not_submitted" | "rejected" | "unknown";
  detail: "dns" | "tls" | "refused" | "reset" | null;
  http_status: number | null;
  provider_status: number | null;
  retry_after_ms: number | null;
  attempts: number;
  elapsed_ms: number;
}
export interface AssistantConversation {
  id: string;
  title: string;
  source: string;
  queue_resume_generation?: number;
  messages: {
    id: string;
    role: string;
    content: string;
    turn_id: string | null;
    attachments?: {
      name: string;
      bytes: number;
      sha256: string;
      document?: DocumentReference;
    }[];
  }[];
  turns: AssistantTurn[];
}
export interface AssistantAck {
  session_id: string;
  turn_id: string;
  state: string;
}
export const assistantActive = (state: string) =>
  [
    "queued",
    "running",
    "waiting_jobs",
    "awaiting_confirmation",
    "waiting_input_queue",
  ].includes(state);
export const assistantState = (state: string) =>
  ({
    queued: "排队中",
    waiting_input_queue: "后续消息已排队",
    running: "处理中",
    waiting_jobs: "等待任务结果",
    awaiting_confirmation: "等待你的确认",
    completed: "已完成",
    disabled: "模型未启用",
    canceled: "已停止",
    rejected: "已拒绝",
    failed: "执行失败",
    needs_input: "等待补充或继续",
    paused_budget: "已到本次执行预算",
    pending: "待处理",
    in_progress: "进行中",
    timed_out: "执行超时",
    cancel_requested: "正在请求停止",
    blocked: "需要授权或准备",
    interrupted_unknown: "已中断，请核对原任务",
    succeeded: "成功",
    unknown: "状态未知",
  })[state] ?? state;
export const assistantApi = {
  steer: (request: {
    turn_id: string;
    revision: number;
    request_id: string;
    text: string;
  }) => invoke("assistant_turn_steer", { request }),
  memorySearch: (query: string, offset = 0) =>
    invoke("assistant_memory_search", { query, offset }),
  memoryCorrect: (request: {
    revision: number;
    id: string;
    expected_sha256: string;
    content: string;
    reason: string;
  }) => invoke("assistant_memory_correct", { request }),
  matterReviewAck: (matterId: string, revision: number) =>
    invoke<void>("assistant_matter_review_ack", { matterId, revision }),
  followupCancel: (followupId: string, revision: number) =>
    invoke<void>("assistant_followup_cancel", { followupId, revision }),
  personal: () => invoke<PersonalAssistantState>("assistant_personal_get"),
  memorySave: (request: {
    revision: number;
    id: string | null;
    title: string;
    content: string;
    expires_at: string | null;
  }) => invoke("assistant_memory_save", { request }),
  memoryForget: (memoryId: string, revision: number) =>
    invoke("assistant_memory_forget", { memoryId, revision }),
  reminderSave: (request: {
    revision: number;
    id: string | null;
    text: string;
    due_at: string;
    interval_minutes: number | null;
    enabled: boolean;
  }) => invoke("assistant_reminder_save", { request }),
  reminderAck: (reminderId: string, cancel: boolean) =>
    invoke("assistant_reminder_ack", { reminderId, cancel }),
  resume: (turnId: string, expectedGeneration?: number) =>
    invoke<AssistantAck>("assistant_resume", { turnId, expectedGeneration }),
  resumeQueue: (sessionId: string, expectedGeneration: number) =>
    invoke("assistant_queue_resume", { sessionId, expectedGeneration }),
  config: () => invoke<AssistantConfig>("assistant_config_get"),
  save: (settings: AssistantSettings) =>
    invoke<AssistantConfig>("assistant_config_save", { settings }),
  sessions: () => invoke<AssistantSession[]>("assistant_sessions_list"),
  create: (requestId?: string) => invoke<AssistantSession>("assistant_session_create", { requestId }),
  session: (sessionId: string) =>
    invoke<AssistantConversation>("assistant_session_get", { sessionId }),
  turn: (turnId: string, includeDetails = true) =>
    invoke<AssistantTurn>("assistant_turn_get", {
      turnId,
      ...(!includeDetails && { includeDetails: false }),
    }),
  inspectTurn: (sessionId: string, turnId: string) =>
    invoke<AssistantTurn>("assistant_turn_inspect", { sessionId, turnId }),
  turnStep: (sessionId: string, turnId: string, stepId: string) =>
    invoke<{ session_id: string; turn_id: string; step: AssistantStep }>(
      "assistant_turn_step_get",
      { sessionId, turnId, stepId },
    ),
  send: (
    sessionId: string | null,
    text: string,
    requestId: string,
    mode: "auto" | "chat" | "task" = "auto",
    options: {
      route_id?: string;
      attachments?: {
        name: string;
        content: string;
        document?: DocumentReference;
      }[];
    } = {},
  ) =>
    invoke<AssistantAck>("assistant_send", {
      request: {
        session_id: sessionId,
        text,
        request_id: requestId,
        ...(mode === "auto" ? {} : { mode }),
        ...options,
      },
    }),
  cancel: (turnId: string) => invoke<void>("assistant_cancel", { turnId }),
  confirm: (turnId: string, confirmationId: string, approve: boolean) =>
    invoke<AssistantAck>("assistant_confirm", {
      turnId,
      confirmationId,
      approve,
    }),
  preview: (artifactId: string) =>
    invoke<{
      name: string;
      content: string;
      sha256: string;
      markdown: boolean;
    }>("assistant_artifact_preview", { artifactId }),
  previewImage: (artifactId: string) =>
    invoke<ArrayBuffer>("assistant_artifact_image_preview", { artifactId }),
  open: (artifactId: string) =>
    invoke<void>("assistant_artifact_open", { artifactId }),
  export: (artifactId: string) =>
    invoke<Pick<AssistantArtifact, "name" | "bytes" | "sha256"> | null>(
      "assistant_artifact_export",
      { artifactId },
    ),
};

export function needsReminderAttention(delivery: string) {
  return delivery === "desktop_due" || delivery === "unknown";
}

export interface PersonalAssistantState {
  revision: number;
  warning?: string | null;
  matters?: {
    revision: number;
    entries: {
      id: string;
      title: string;
      goal: string;
      summary: string;
      next_action: string;
      state: string;
      review_at: string | null;
      delivery: string;
      source: string;
      updated_at: string;
      turn_ids: string[];
      previous: {
        summary: string;
        next_action: string;
        state: string;
        updated_at: string;
      }[];
      latest_task: {
        turn_id: string;
        state: string;
        progress: AssistantTurn["progress"];
      } | null;
    }[];
  };
  skills?: { id: string; name: string; description: string; version: number }[];
  followups?: {
    revision: number;
    entries: {
      id: string;
      title: string;
      instruction: string;
      due_at: string;
      interval_minutes: number | null;
      max_runs: number;
      enabled: boolean;
      state: string;
      notice: string;
      notice_delivery?: string;
      source: string;
      matter_id?: string | null;
      trigger?: { turn_id: string; on: "completed" | "needs_attention" } | null;
      triggered_at?: string | null;
      event_deadline?: string | null;
      notification_policy?: "all" | "meaningful";
      runs: {
        ordinal: number;
        scheduled_at: string;
        turn_id: string | null;
        state: string;
      }[];
    }[];
  };
  memories: {
    id: string;
    title: string;
    content: string;
    expires_at: string | null;
    updated_at: string;
    status: "active" | "expired" | "invalid";
    previous?: {
      content: string;
      expires_at: string | null;
      updated_at: string;
    }[];
    origin?: { source: string; turn_id: string | null } | null;
    corrections?: {
      reason: string;
      previous_title: string;
      previous_content: string;
      previous_expires_at: string | null;
      previous_origin: { source: string; turn_id: string | null } | null;
      corrected_at: string;
      origin: { source: string; turn_id: string | null };
    }[];
  }[];
  reminders: {
    id: string;
    text: string;
    due_at: string;
    interval_minutes: number | null;
    enabled: boolean;
    delivery: string;
    last_due_at: string | null;
    source: string;
  }[];
}
