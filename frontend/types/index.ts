// Core Turn DAG types matching the Rust HTTP gateway responses

export interface DeclaredType {
  type_id: string;
  type_version: number;
}

export interface Turn {
  turn_id: string;
  parent_turn_id: string;
  depth: number;
  declared_type?: DeclaredType;
  decoded_as?: DeclaredType;
  data?: Record<string, unknown>;
  unknown?: Record<string, unknown>;
  raw?: string; // base64-encoded raw payload when view=raw or view=both
}

export interface ContextMeta {
  context_id: string;
  head_turn_id: string;
  head_depth: number;
  registry_bundle_id?: string;
}

export interface TurnResponse {
  meta: ContextMeta;
  turns: Turn[];
  next_before_turn_id?: string;
}

export interface ErrorDetail {
  code: number;
  message: string;
}

export interface ErrorResponse {
  error: ErrorDetail;
}

// Query options for fetching turns
export interface FetchTurnsOptions {
  limit?: number;
  before_turn_id?: string;
  view?: 'typed' | 'raw' | 'both';
  type_hint_mode?: 'inherit' | 'latest' | 'explicit';
  bytes_render?: 'base64' | 'hex' | 'len_only';
  u64_format?: 'string' | 'number';
  enum_render?: 'label' | 'number' | 'both';
  time_render?: 'iso' | 'unix_ms';
  include_unknown?: boolean;
}

// Debug event types for the Context Debugger
export type DebugEventType = 'turn' | 'tool_call' | 'tool_result';

export interface DebugEvent {
  id: string;
  type: DebugEventType;
  depth: number;
  label: string;
  summary: string;
  timestamp?: string;
  payload: unknown;
  searchText: string;
}

// Tool call/result types extracted from turn payloads
export interface ToolCall {
  id: string;
  name: string;
  args?: Record<string, unknown>;
}

export interface ToolResult {
  toolCallId: string;
  output?: unknown;
  error?: string;
}

// Re-export provenance types
export type { Provenance, ContextMetadata } from './provenance';

// Context list entry for the sidebar
export interface ContextEntry {
  context_id: string;
  head_turn_id?: string;
  head_depth?: number;
  label?: string;
  // Creation timestamp (always present from backend)
  created_at_unix_ms?: number;
  // Live observer extensions (Sprint 005/006)
  session_id?: string;
  client_tag?: string;
  is_live?: boolean;
  last_activity_at?: number;
  // Filesystem snapshot indicator
  has_fs_snapshot?: boolean;
  // Context metadata (from first turn)
  title?: string;
  labels?: string[];
  // Provenance (origin story)
  provenance?: import('./provenance').Provenance;
  // Server-provided lineage summary (optional, include_lineage=1)
  lineage?: {
    parent_context_id?: string;
    root_context_id?: string;
    spawn_reason?: string;
    child_context_count: number;
    child_context_ids: string[];
  };
}

// ============================================
// Live Observer Types (Sprint 005/006)
// ============================================

// Session info for connected binary protocol clients
export interface SessionInfo {
  session_id: string;
  client_tag: string;
  connected_at: number;
  last_activity_at: number;
  context_count: number;
}

// SSE event types
export interface ContextCreatedEvent {
  context_id: string;
  session_id?: string;
  client_tag?: string;
  created_at: number;
}

export interface TurnAppendedEvent {
  context_id: string;
  turn_id: string;
  parent_turn_id: string;
  depth: number;
  declared_type_id?: string;
  declared_type_version?: number;
}

export interface ContextMetadataUpdatedEvent {
  context_id: string;
  client_tag?: string;
  title?: string;
  labels?: string[];
  has_provenance: boolean;
}

export interface ContextLinkedEvent {
  child_context_id: string;
  parent_context_id: string;
  root_context_id?: string;
  spawn_reason?: string;
}

export interface ClientConnectedEvent {
  session_id: string;
  client_tag: string;
}

export interface ClientDisconnectedEvent {
  session_id: string;
  client_tag: string;
  contexts: string[];
}

export interface ErrorOccurredEvent {
  timestamp_ms: number;
  kind: string;
  status_code: number;
  message: string;
  path?: string;
}

// Union type for all SSE events
export type StoreEvent =
  | { type: 'context_created'; data: ContextCreatedEvent }
  | { type: 'context_metadata_updated'; data: ContextMetadataUpdatedEvent }
  | { type: 'context_linked'; data: ContextLinkedEvent }
  | { type: 'turn_appended'; data: TurnAppendedEvent }
  | { type: 'client_connected'; data: ClientConnectedEvent }
  | { type: 'client_disconnected'; data: ClientDisconnectedEvent }
  | { type: 'error_occurred'; data: ErrorOccurredEvent };

// Activity feed item (derived from SSE events)
export interface ActivityItem {
  id: string;
  timestamp: number;
  event: StoreEvent;
}

// Connection state for SSE
export type ConnectionState = 'connecting' | 'connected' | 'reconnecting' | 'disconnected';

// Extended contexts response with session info
export interface ContextsResponse {
  contexts: ContextEntry[];
  active_sessions?: SessionInfo[];
  active_tags?: string[];
}

// ============================================
// Server Metrics Types (Sprint 008)
// ============================================

export interface LatencyStats {
  p50: number;
  p95: number;
  p99: number;
  max: number;
  count: number;
}

export interface MemoryMetrics {
  sys_total_bytes: number;
  sys_available_bytes: number;
  sys_free_bytes: number;
  sys_cached_bytes: number;
  sys_swap_total_bytes: number;
  sys_swap_free_bytes: number;
  process_rss_bytes: number;
  process_vmem_bytes: number;
  process_heap_bytes: number | null;
  process_open_fds: number | null;
  budget_bytes: number;
  budget_pct: number;
  hard_cap_bytes: number;
  pressure_ratio: number;
  pressure_level: 'OK' | 'WARN' | 'HOT' | 'CRITICAL';
  spill_threshold_bytes: number;
  spill_critical_bytes: number;
}

export interface SessionMetrics {
  total: number;
  active: number;
  idle: number;
  last_activity_unix_ms: number;
}

export interface ObjectMetrics {
  contexts_total: number;
  turns_total: number;
  blobs_total: number;
  registry_types_total: number;
  registry_bundles_total: number;
  heads_total: number;
}

export interface StorageMetrics {
  turns_log_bytes: number;
  turns_index_bytes: number;
  turns_meta_bytes: number;
  heads_table_bytes: number;
  blobs_pack_bytes: number;
  blobs_index_bytes: number;
  data_dir_total_bytes: number;
  data_dir_free_bytes: number;
}

export interface PerfMetrics {
  append_tps_1m: number;
  append_tps_5m: number;
  append_tps_history: number[];
  get_last_tps_1m: number;
  get_last_tps_5m: number;
  get_last_tps_history: number[];
  get_blob_tps_1m: number;
  get_blob_tps_5m: number;
  get_blob_tps_history: number[];
  registry_ingest_tps_1m: number;
  registry_ingest_tps_5m: number;
  http_req_tps_1m: number;
  http_req_tps_5m: number;
  http_req_tps_history: number[];
  http_errors_tps_1m: number;
  http_errors_tps_5m: number;
  append_latency_ms: LatencyStats;
  get_last_latency_ms: LatencyStats;
  get_blob_latency_ms: LatencyStats;
  http_latency_ms: LatencyStats;
}

export interface ErrorMetrics {
  total: number;
  by_type: Record<string, number>;
}

export interface ErrorEntry {
  timestamp_ms: number;
  kind: string;
  status_code: number;
  message: string;
  path?: string;
}

export interface FilesystemMetrics {
  snapshots_total: number;
  index_bytes: number;
  content_bytes: number;
}

export interface MetricsSnapshot {
  ts: string;
  uptime_seconds: number;
  memory: MemoryMetrics;
  sessions: SessionMetrics;
  objects: ObjectMetrics;
  storage: StorageMetrics;
  filesystem: FilesystemMetrics;
  perf: PerfMetrics;
  errors: ErrorMetrics;
}
