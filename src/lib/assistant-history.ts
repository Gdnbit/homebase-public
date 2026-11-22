import { invoke } from "@tauri-apps/api/core";

export type AssistantHistoryKind = "messages" | "turns";
export interface AssistantHistoryQuery {
  session_id: string;
  kind?: AssistantHistoryKind;
  query?: string;
  snapshot_id?: string | null;
  through_turn_id?: string | null;
  offset?: number;
  limit?: number;
}
export interface AssistantHistoryRead {
  session_id: string;
  snapshot_id: string;
  item_id: string;
  offset?: number;
  limit?: number;
}
export interface AssistantHistoryItem {
  id: string;
  turn_id: string | null;
  role: string | null;
  state: string | null;
  created_at: string;
  preview: string;
  preview_only: boolean;
  json_bytes: number;
  json_sha256: string;
}
export interface AssistantHistoryPage {
  version: "homebase-history-v1";
  session_id: string;
  snapshot_id: string;
  kind: AssistantHistoryKind;
  query: string;
  through_turn_id: string | null;
  captured_at: string;
  expires_in_seconds: number;
  total: number;
  offset: number;
  next_offset: number | null;
  complete: boolean;
  omitted_unassigned_messages: number;
  items: AssistantHistoryItem[];
}
export interface AssistantHistoryChunk {
  version: "homebase-history-v1";
  session_id: string;
  snapshot_id: string;
  kind: AssistantHistoryKind;
  item_id: string;
  format: "message_json" | "turn_view_json";
  json_sha256: string;
  total_bytes: number;
  offset: number;
  next_offset: number | null;
  complete: boolean;
  content: string;
}
export const assistantHistoryApi = {
  query: (request: AssistantHistoryQuery) =>
    invoke<AssistantHistoryPage>("assistant_history_query", { request }),
  read: (request: AssistantHistoryRead) =>
    invoke<AssistantHistoryChunk>("assistant_history_read", { request }),
};
