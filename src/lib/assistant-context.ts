/** Read-only bounded summary. Original task interfaces own complete results. */
export interface AssistantContextProjection {
  version: string;
  fingerprint: string;
  state_revision: number;
  policy_revision: number;
  // Optional for older running backends; current schema revision is 2.
  projection_schema_revision?: number;
  summary_only?: boolean;
  context_byte_limit?: number;
  context_has_more?: boolean;
  tasks_total?: number;
  tasks_has_more?: boolean;
  queued_tasks?: number;
  discussion_turns?: number;
  memory_candidates?: number;
  memory_scope?: string;
  memories_has_more?: boolean;
  authorized_workspaces_total?: number;
  authorized_workspaces_has_more?: boolean;
  memories: {
    id?: string;
    title: string;
    content?: string;
    content_truncated?: boolean;
  }[];
  tasks: {
    id: string;
    state: string;
    requirements_revision?: number;
    requirements?: {
      revision: number;
      text: string;
      text_truncated: boolean;
    }[];
    runtime_features?: { steering: boolean; documents: boolean };
    progress?: {
      goal: string;
      current: string | null;
      blocked: string | null;
      next: string | null;
    };
    result?: string;
    result_truncated?: boolean;
  }[];
  authorized_workspaces: { id: string; access: string; auto_write?: boolean }[];
}
