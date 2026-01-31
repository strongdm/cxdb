// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

export interface ContextEntry {
  context_id: string;
  client_tag?: string;
  session_id?: string;
  is_live?: boolean;
  last_activity_at?: number;
  title?: string;
  labels?: Record<string, string>;
}

export type StoreEvent =
  | { type: 'context_created'; data: { context_id: string; client_tag: string; session_id: string; created_at: number } }
  | { type: 'turn_appended'; data: { context_id: string; turn_id: string; parent_id: string } }
  | { type: 'context_metadata_updated'; data: { context_id: string; client_tag?: string; title?: string; labels?: Record<string, string> } }
  | { type: 'client_disconnected'; data: { contexts: string[] } };
