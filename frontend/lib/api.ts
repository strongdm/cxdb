import type { TurnResponse, FetchTurnsOptions, ErrorResponse, ContextEntry, SessionInfo, Provenance } from '@/types';
import type { FsListResponse, FsFileResponse } from '@/types/filesystem';

const API_BASE = '/v1';

export interface ContextsResponse {
  contexts: ContextEntry[];
  count: number;
  active_sessions?: SessionInfo[];
  active_tags?: string[];
}

export class ApiError extends Error {
  constructor(
    message: string,
    public code?: number,
    public response?: ErrorResponse
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

/**
 * Fetch turns for a context from the HTTP gateway.
 */
export async function fetchTurns(
  contextId: string,
  options: FetchTurnsOptions = {}
): Promise<TurnResponse> {
  const params = new URLSearchParams();

  if (options.limit !== undefined) {
    params.set('limit', String(options.limit));
  }
  if (options.before_turn_id !== undefined) {
    params.set('before_turn_id', options.before_turn_id);
  }
  if (options.view !== undefined) {
    params.set('view', options.view);
  }
  if (options.type_hint_mode !== undefined) {
    params.set('type_hint_mode', options.type_hint_mode);
  }
  if (options.bytes_render !== undefined) {
    params.set('bytes_render', options.bytes_render);
  }
  if (options.u64_format !== undefined) {
    params.set('u64_format', options.u64_format);
  }
  if (options.enum_render !== undefined) {
    params.set('enum_render', options.enum_render);
  }
  if (options.time_render !== undefined) {
    params.set('time_render', options.time_render);
  }
  if (options.include_unknown !== undefined) {
    params.set('include_unknown', String(options.include_unknown));
  }

  const queryString = params.toString();
  const url = `${API_BASE}/contexts/${encodeURIComponent(contextId)}/turns${queryString ? `?${queryString}` : ''}`;

  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}

/**
 * Fetch a specific blob by hash (for raw inspection).
 */
export async function fetchBlob(hash: string): Promise<ArrayBuffer> {
  const url = `${API_BASE}/blobs/${encodeURIComponent(hash)}`;
  const response = await fetch(url);

  if (!response.ok) {
    throw new ApiError(`HTTP ${response.status}`, response.status);
  }

  return response.arrayBuffer();
}

/**
 * Check if the API is reachable.
 */
export async function healthCheck(): Promise<boolean> {
  try {
    const response = await fetch('/healthz');
    return response.ok;
  } catch {
    return false;
  }
}

export interface FetchContextsOptions {
  limit?: number;
  tag?: string;
  /** Include full provenance data for each context. */
  include_provenance?: boolean;
  /** Include parent/root/children lineage summary for each context. */
  include_lineage?: boolean;
}

/**
 * Fetch recent contexts from the HTTP gateway.
 */
export async function fetchContexts(limitOrOptions: number | FetchContextsOptions = 20): Promise<ContextsResponse> {
  const options: FetchContextsOptions = typeof limitOrOptions === 'number'
    ? { limit: limitOrOptions }
    : limitOrOptions;

  const params = new URLSearchParams();
  if (options.limit !== undefined) {
    params.set('limit', String(options.limit));
  }
  if (options.tag) {
    params.set('tag', options.tag);
  }
  if (options.include_provenance) {
    params.set('include_provenance', '1');
  }
  if (options.include_lineage) {
    params.set('include_lineage', '1');
  }

  const queryString = params.toString();
  const url = `${API_BASE}/contexts${queryString ? `?${queryString}` : ''}`;
  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}

export interface FetchContextOptions {
  include_provenance?: boolean;
  include_lineage?: boolean;
}

/**
 * Fetch details for a specific context.
 */
export async function fetchContext(
  contextId: string,
  options: FetchContextOptions = {}
): Promise<ContextEntry> {
  const params = new URLSearchParams();
  if (options.include_provenance !== false) {
    params.set('include_provenance', '1');
  }
  if (options.include_lineage !== false) {
    params.set('include_lineage', '1');
  }

  const queryString = params.toString();
  const url = `${API_BASE}/contexts/${encodeURIComponent(contextId)}${queryString ? `?${queryString}` : ''}`;
  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}

export interface FetchContextChildrenOptions {
  recursive?: boolean;
  limit?: number;
  include_provenance?: boolean;
  include_lineage?: boolean;
}

export interface ContextChildrenResponse {
  context_id: string;
  recursive: boolean;
  count: number;
  children: ContextEntry[];
}

/**
 * Fetch child contexts for a parent context.
 */
export async function fetchContextChildren(
  contextId: string,
  options: FetchContextChildrenOptions = {}
): Promise<ContextChildrenResponse> {
  const params = new URLSearchParams();
  if (options.recursive) {
    params.set('recursive', '1');
  }
  if (options.limit !== undefined) {
    params.set('limit', String(options.limit));
  }
  if (options.include_provenance !== false) {
    params.set('include_provenance', '1');
  }
  if (options.include_lineage !== false) {
    params.set('include_lineage', '1');
  }

  const queryString = params.toString();
  const url = `${API_BASE}/contexts/${encodeURIComponent(contextId)}/children${queryString ? `?${queryString}` : ''}`;
  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}

/**
 * Search response from CQL query.
 */
export interface SearchResponse {
  contexts: ContextEntry[];
  total_count: number;
  elapsed_ms: number;
  query: string;
}

/**
 * CQL search error response.
 */
export interface CqlErrorResponse {
  error: string;
  error_type: string;
  position?: number;
  field?: string;
}

/**
 * Search contexts using CQL query.
 */
export async function searchContexts(
  query: string,
  limit?: number
): Promise<SearchResponse> {
  const params = new URLSearchParams();
  params.set('q', query);
  if (limit !== undefined) {
    params.set('limit', String(limit));
  }

  const url = `${API_BASE}/contexts/search?${params.toString()}`;
  const response = await fetch(url);

  if (!response.ok) {
    let errorData: CqlErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error || `HTTP ${response.status}`,
      response.status,
      { error: { message: errorData?.error || 'Search failed', code: response.status } }
    );
  }

  return response.json();
}

/**
 * Fetch filesystem directory listing for a turn.
 * Returns entries at the given path, or root if path is empty.
 */
export async function fetchFsDirectory(
  turnId: string,
  path: string = ''
): Promise<FsListResponse> {
  const params = new URLSearchParams();
  if (path) {
    params.set('path', path);
  }

  const queryString = params.toString();
  const url = `${API_BASE}/turns/${encodeURIComponent(turnId)}/fs${queryString ? `?${queryString}` : ''}`;

  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}

/**
 * Fetch filesystem file content for a turn.
 * Returns file metadata and base64-encoded content.
 */
export async function fetchFsFile(
  turnId: string,
  filePath: string
): Promise<FsFileResponse> {
  const url = `${API_BASE}/turns/${encodeURIComponent(turnId)}/fs/${filePath}?format=json`;

  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}

/**
 * Provenance response from the HTTP gateway.
 */
export interface ProvenanceResponse {
  context_id: string;
  provenance: Provenance | null;
}

/**
 * Fetch provenance for a specific context.
 */
export async function fetchProvenance(contextId: string): Promise<ProvenanceResponse> {
  const url = `${API_BASE}/contexts/${encodeURIComponent(contextId)}/provenance`;
  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}

// ============================================================================
// Renderer Manifest
// ============================================================================

import type { RendererManifest } from './renderer-registry';

/**
 * Fetch the renderer manifest from the backend.
 * Returns a mapping of type IDs to renderer specifications.
 */
export async function fetchRendererManifest(): Promise<RendererManifest> {
  const url = `${API_BASE}/registry/renderers`;
  const response = await fetch(url);

  if (!response.ok) {
    let errorData: ErrorResponse | undefined;
    try {
      errorData = await response.json();
    } catch {
      // Ignore JSON parse errors
    }
    throw new ApiError(
      errorData?.error?.message || `HTTP ${response.status}`,
      errorData?.error?.code || response.status,
      errorData
    );
  }

  return response.json();
}
