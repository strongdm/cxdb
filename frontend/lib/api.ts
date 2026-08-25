// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import type { TurnResponse, FetchTurnsOptions, ErrorResponse, ContextEntry, SessionInfo, Provenance, APITokenMetadata, CurrentUser, Turn } from '@/types';
import type { FsListResponse, FsFileResponse } from '@/types/filesystem';

const API_BASE = '/v1';
const AUTH_API_BASE = '/api/v1';

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

async function authApiError(response: Response): Promise<ApiError> {
  let message = `HTTP ${response.status}`;
  let errorData: ErrorResponse | undefined;
  try {
    const body: unknown = await response.json();
    if (typeof body === 'object' && body !== null) {
      const value = body as { error?: unknown; message?: unknown };
      if (typeof value.error === 'string') message = value.error;
      else if (typeof value.message === 'string') message = value.message;
      else if (typeof value.error === 'object' && value.error !== null) {
        const detail = value.error as { message?: unknown; code?: unknown };
        if (typeof detail.message === 'string') message = detail.message;
        errorData = { error: { message, code: typeof detail.code === 'number' ? detail.code : response.status } };
      }
      if (!errorData) errorData = { error: { message, code: response.status } };
    }
  } catch {
    try {
      const text = await response.text();
      if (text.trim()) message = text.trim();
    } catch {
      // Keep the status message.
    }
  }
  return new ApiError(message, response.status, errorData);
}

/** Fetch the browser identity and its session-bound CSRF token. */
export async function fetchCurrentUser(): Promise<CurrentUser> {
  const response = await fetch(`${AUTH_API_BASE}/me`, { credentials: 'same-origin' });
  if (!response.ok) throw await authApiError(response);
  return response.json() as Promise<CurrentUser>;
}

/** List token metadata. Token plaintext is never returned by this endpoint. */
export async function fetchAPITokens(): Promise<APITokenMetadata[]> {
  const response = await fetch(`${AUTH_API_BASE}/tokens`, { credentials: 'same-origin' });
  if (!response.ok) throw await authApiError(response);
  const payload = await response.json() as { tokens?: APITokenMetadata[] };
  return Array.isArray(payload.tokens) ? payload.tokens : [];
}

export interface CreateAPITokenInput {
  name: string;
  scopes: string[];
  expires_at?: string;
}

export interface CreateAPITokenResponse {
  token: APITokenMetadata;
  plaintext: string;
}

/** Create a token. The gateway returns its plaintext once. */
export async function createAPIToken(csrfToken: string, input: CreateAPITokenInput): Promise<CreateAPITokenResponse> {
  const response = await fetch(`${AUTH_API_BASE}/tokens`, {
    method: 'POST', credentials: 'same-origin',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': csrfToken },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw await authApiError(response);
  return response.json() as Promise<CreateAPITokenResponse>;
}

/** Revoke a token with the session-bound CSRF token. */
export async function revokeAPIToken(csrfToken: string, tokenId: string): Promise<void> {
  const response = await fetch(`${AUTH_API_BASE}/tokens/${encodeURIComponent(tokenId)}`, {
    method: 'DELETE', credentials: 'same-origin', headers: { 'X-CSRF-Token': csrfToken },
  });
  if (!response.ok) throw await authApiError(response);
}

function normalizeContextEntry(context: ContextEntry): ContextEntry {
  return {
    ...context,
    context_id: String(context.context_id),
    head_turn_id: context.head_turn_id !== undefined ? String(context.head_turn_id) : undefined,
    lineage: context.lineage ? {
      ...context.lineage,
      parent_context_id: context.lineage.parent_context_id !== undefined
        ? String(context.lineage.parent_context_id)
        : undefined,
      root_context_id: context.lineage.root_context_id !== undefined
        ? String(context.lineage.root_context_id)
        : undefined,
      child_context_ids: context.lineage.child_context_ids.map(String),
    } : context.lineage,
  };
}

function normalizeTurnResponse(response: TurnResponse): TurnResponse {
  return {
    ...response,
    meta: {
      ...response.meta,
      context_id: String(response.meta.context_id),
      head_turn_id: String(response.meta.head_turn_id),
    },
    next_before_turn_id: response.next_before_turn_id !== undefined
      ? String(response.next_before_turn_id)
      : undefined,
    turns: response.turns.map(turn => ({
      ...turn,
      turn_id: String(turn.turn_id),
      parent_turn_id: String(turn.parent_turn_id),
    })),
  };
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
  if (options.string_limit !== undefined) params.set('string_limit', String(options.string_limit));
  if (options.turn_id !== undefined) params.set('turn_id', options.turn_id);

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

  return normalizeTurnResponse(await response.json());
}

/** Fetch one complete turn after a bounded list response. */
export async function fetchTurn(contextId: string, turnId: string): Promise<Turn> {
  const response = await fetchTurns(contextId, { turn_id: turnId, limit: 1, view: 'typed', include_unknown: true });
  const turn = response.turns[0];
  if (!turn) throw new ApiError('Turn not found', 404);
  return turn;
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

  const payload = await response.json() as ContextsResponse;
  return {
    ...payload,
    contexts: payload.contexts.map(normalizeContextEntry),
  };
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

  return normalizeContextEntry(await response.json());
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

  const payload = await response.json() as ContextChildrenResponse;
  return {
    ...payload,
    context_id: String(payload.context_id),
    children: payload.children.map(normalizeContextEntry),
  };
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

  const payload = await response.json() as SearchResponse;
  return {
    ...payload,
    contexts: payload.contexts.map(normalizeContextEntry),
  };
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
