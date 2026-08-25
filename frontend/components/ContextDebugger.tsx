// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { useEffect, useMemo, useRef, useState, useCallback } from 'react';
import type { Turn, TurnResponse, DebugEvent } from '@/types';
import { Layers, Hash, X, Copy, Search, Loader2, AlertCircle, GitBranch, ChevronDown, ChevronRight, Terminal, MessageSquare, Wrench, CheckCircle, XCircle, Folder, Zap, Database } from './icons';
import { cn, trunc, safeStringify, formatTime, contentPreview } from '@/lib/utils';
import { fetchTurn, fetchTurns, fetchFsDirectory, ApiError } from '@/lib/api';
import { FileBrowser } from './FileBrowser';
import { FileViewer } from './FileViewer';
import { TryRenderCanonical, isConversationItem } from './ConversationRenderer';
import { MessageRenderer, isAgentMessage, extractMessageText } from './MessageRenderer';
import { QuestEventRenderer, QuestSnapshotRenderer, isQuestEvent, isQuestSnapshot } from './QuestRenderer';
import { FallbackRenderer } from './FallbackRenderer';
import { ProvenancePanel } from './ProvenancePanel';
import { DynamicRenderer } from './DynamicRenderer';
import { useRendererManifest } from '@/lib/use-renderer';
import { getItemTypeLabel, getItemTypeColors } from '@/types/conversation';
import type { ConversationItem, ItemType } from '@/types/conversation';

const TURN_PAGE_SIZE = 100;
const TURN_LIST_STRING_LIMIT = 512;

// View tabs for the right panel
type DetailView = 'turn' | 'provenance';

// Detect turn type from declared_type or data
type TurnKind = 'user' | 'assistant' | 'tool_call' | 'tool_result' | 'system' | 'quest_event' | 'quest_snapshot' | 'unknown';

// Map canonical item_type to TurnKind
function canonicalToTurnKind(itemType: ItemType): TurnKind {
  switch (itemType) {
    case 'user_input': return 'user';
    case 'assistant': return 'assistant';
    case 'assistant_turn': return 'assistant';
    case 'tool_call': return 'tool_call';
    case 'tool_result': return 'tool_result';
    case 'system': return 'system';
    case 'handoff': return 'system'; // Handoffs show as system for now
    default: return 'unknown';
  }
}

function detectTurnKind(turn: Turn): TurnKind {
  const data = turn.data as Record<string, unknown> | undefined;

  // First check for canonical item_type (highest priority)
  if (data && isConversationItem(data)) {
    return canonicalToTurnKind(data.item_type);
  }

  // Check for quest types
  if (data && isQuestEvent(data)) {
    return 'quest_event';
  }
  if (data && isQuestSnapshot(data)) {
    return 'quest_snapshot';
  }

  // Fall back to legacy detection
  const typeId = turn.declared_type?.type_id ?? '';

  if (typeId.includes('ToolResult') || data?.tool_call_id || data?.ToolCallID) return 'tool_result';
  if (typeId.includes('ToolCall')) return 'tool_call';
  if (typeId.includes('Assistant') || data?.tool_calls) return 'assistant';

  // Check for role field (lowercase for legacy, PascalCase for ai-agents-sdk.Message)
  const role = (data?.role ?? data?.Role) as string | undefined;
  if (role === 'user') return 'user';
  if (role === 'assistant') return 'assistant';
  if (role === 'system') return 'system';
  if (role === 'tool') return 'tool_result';

  return 'unknown';
}

// Extract text content from turn
function extractContent(turn: Turn): string | null {
  const data = turn.data as Record<string, unknown> | undefined;
  if (!data) return null;

  // Check for canonical types first (codergen-sdk)
  if (isConversationItem(data)) {
    if (data.item_type === 'user_input' && data.user_input) {
      return data.user_input.text;
    }
    if (data.item_type === 'assistant' && data.assistant) {
      return data.assistant.text;
    }
    if (data.item_type === 'assistant_turn' && data.turn) {
      return data.turn.text;
    }
    if (data.item_type === 'tool_result' && data.tool_result) {
      return data.tool_result.content;
    }
    if (data.item_type === 'system' && data.system) {
      return data.system.content;
    }
    if (data.item_type === 'handoff' && data.handoff) {
      return data.handoff.reason ?? null;
    }
  }

  // Check for ai-agents-sdk.Message format
  if (isAgentMessage(data)) {
    return extractMessageText(data);
  }

  // Check for quest.Event format
  if (isQuestEvent(data)) {
    // Return event_type and description if available
    const eventData = data.data as Record<string, unknown> | undefined;
    if (eventData?.description && typeof eventData.description === 'string') {
      return eventData.description;
    }
    return data.event_type;
  }

  // Check for quest.Snapshot format
  if (isQuestSnapshot(data)) {
    return `${data.file_count} files, ${data.trigger}`;
  }

  // Legacy extraction
  if (typeof data.content === 'string') return data.content;
  if (typeof data.text === 'string') return data.text;
  if (typeof data.message === 'string') return data.message;
  if (typeof data.description === 'string') return data.description;

  return null;
}

// Extract tool calls - handles canonical types, named keys, and numeric msgpack keys
function extractToolCalls(turn: Turn): Array<{ id: string; name: string; arguments: string }> {
  const data = turn.data as Record<string, unknown> | undefined;
  if (!data) return [];

  // Check for canonical tool_call type - returns single item as array
  if (isConversationItem(data) && data.item_type === 'tool_call' && data.tool_call) {
    return [{
      id: data.tool_call.call_id,
      name: data.tool_call.name,
      arguments: data.tool_call.args,
    }];
  }

  // Check for v2 assistant_turn with nested tool_calls
  if (isConversationItem(data) && data.item_type === 'assistant_turn' && data.turn?.tool_calls) {
    return data.turn.tool_calls.map((tc, idx) => ({
      id: tc.id ?? `tc-${idx}`,
      name: tc.name,
      arguments: tc.args,
    }));
  }

  // A legacy ToolCall is often stored as its own turn rather than inside an
  // assistant message. Treat that one payload as a single call so its result
  // can be matched by tool_call_id as well.
  if (turn.declared_type?.type_id.includes('ToolCall')) {
    const id = data.id ?? data.call_id ?? data['1'];
    const name = data.name ?? data['2'];
    if (id !== undefined && name !== undefined) {
      return [{
        id: String(id),
        name: String(name),
        arguments: String(data.arguments ?? data.args ?? data['3'] ?? '{}'),
      }];
    }
  }

  // Legacy extraction
  const toolCalls = data.tool_calls as Array<Record<string, unknown>> | undefined;
  if (!Array.isArray(toolCalls)) return [];

  return toolCalls.map((tc, idx) => ({
    // Handle both named keys and numeric msgpack keys (1, 2, 3)
    id: String(tc.id ?? tc['1'] ?? `tc-${idx}`),
    name: String(tc.name ?? tc['2'] ?? 'unknown'),
    arguments: String(tc.arguments ?? tc.args ?? tc['3'] ?? '{}'),
  }));
}

/**
 * A v2 assistant turn can carry its result in the same payload as the call.
 * Such a result is already exact when this turn has been hydrated. Do not
 * replace it with a lookup for a separate result turn.
 */
function hasEmbeddedToolResult(turn: Turn, toolCallId: string): boolean {
  const data = turn.data as Record<string, unknown> | undefined;
  if (!data) return false;

  if (isConversationItem(data) && data.item_type === 'assistant_turn' && data.turn?.tool_calls) {
    return data.turn.tool_calls.some(toolCall => {
      if (toolCall.id !== toolCallId) return false;
      return toolCall.result !== undefined
        || toolCall.error !== undefined
        || toolCall.streaming_output !== undefined;
    });
  }

  // Keep compatibility with legacy payloads that use numeric msgpack keys.
  const toolCalls = data.tool_calls as Array<Record<string, unknown>> | undefined;
  if (!Array.isArray(toolCalls)) return false;

  return toolCalls.some(toolCall => {
    const id = String(toolCall.id ?? toolCall['1'] ?? '');
    if (id !== toolCallId) return false;
    return toolCall.result !== undefined
      || toolCall.error !== undefined
      || toolCall.streaming_output !== undefined
      || toolCall['9'] !== undefined
      || toolCall['10'] !== undefined;
  });
}

// Extract tool result info - handles canonical types and legacy formats
function extractToolResult(turn: Turn): { toolCallId: string; content: string; isError: boolean } | null {
  const data = turn.data as Record<string, unknown> | undefined;
  if (!data) return null;

  // Check for canonical tool_result type
  if (isConversationItem(data) && data.item_type === 'tool_result' && data.tool_result) {
    return {
      toolCallId: data.tool_result.call_id,
      content: data.tool_result.streaming_output || data.tool_result.content,
      isError: data.tool_result.is_error,
    };
  }

  // Legacy extraction
  const toolCallId = data.tool_call_id as string | undefined;
  const content = data.content as string | undefined;
  const isError = data.is_error as boolean | undefined;

  if (!toolCallId && !content) return null;

  return {
    toolCallId: toolCallId ?? 'unknown',
    content: content ?? '',
    isError: isError ?? false,
  };
}

// Get label for turn kind
function getKindLabel(kind: TurnKind): string {
  switch (kind) {
    case 'user': return 'User';
    case 'assistant': return 'Assistant';
    case 'tool_call': return 'Tool Call';
    case 'tool_result': return 'Tool Result';
    case 'system': return 'System';
    case 'quest_event': return 'Quest Event';
    case 'quest_snapshot': return 'Snapshot';
    default: return 'Turn';
  }
}

// Get color classes for turn kind - uses theme-aware colors for common roles
function getKindColors(kind: TurnKind): { badge: string; text: string; border: string } {
  switch (kind) {
    case 'user':
      return { badge: 'bg-theme-role-user-muted text-theme-role-user', text: 'text-theme-role-user', border: 'border-l-theme-role-user' };
    case 'assistant':
      return { badge: 'bg-theme-role-assistant-muted text-theme-role-assistant', text: 'text-theme-role-assistant', border: 'border-l-theme-role-assistant' };
    case 'tool_call':
      return { badge: 'bg-theme-role-tool-muted text-theme-role-tool', text: 'text-theme-role-tool', border: 'border-l-theme-role-tool' };
    case 'tool_result':
      return { badge: 'bg-theme-success-muted text-theme-success', text: 'text-theme-success', border: 'border-l-theme-success' };
    case 'system':
      return { badge: 'bg-theme-role-system-muted text-theme-role-system', text: 'text-theme-role-system', border: 'border-l-theme-role-system' };
    case 'quest_event':
      return { badge: 'bg-theme-info-muted text-theme-info', text: 'text-theme-info', border: 'border-l-theme-info' };
    case 'quest_snapshot':
      return { badge: 'bg-cyan-500/20 text-cyan-300', text: 'text-cyan-400', border: 'border-l-cyan-500' };
    default:
      return { badge: 'bg-theme-tag-default-bg text-theme-tag-default', text: 'text-theme-text-dim', border: 'border-l-theme-border' };
  }
}

// Get icon for turn kind
function KindIcon({ kind, className }: { kind: TurnKind; className?: string }) {
  switch (kind) {
    case 'user':
      return <MessageSquare className={className} />;
    case 'assistant':
      return <Layers className={className} />;
    case 'tool_call':
      return <Wrench className={className} />;
    case 'tool_result':
      return <Terminal className={className} />;
    case 'system':
      return <Hash className={className} />;
    case 'quest_event':
      return <Zap className={className} />;
    case 'quest_snapshot':
      return <Database className={className} />;
    default:
      return <Hash className={className} />;
  }
}

// Build summary for sidebar
function buildSummary(turn: Turn, kind: TurnKind): string {
  const content = extractContent(turn);
  if (content) return contentPreview(content, 80);

  const toolCalls = extractToolCalls(turn);
  if (toolCalls.length > 0) {
    const names = toolCalls.map(tc => tc.name).join(', ');
    return `→ ${names}`;
  }

  const toolResult = extractToolResult(turn);
  if (toolResult) {
    if (toolResult.isError) return `✗ Error`;
    return contentPreview(toolResult.content, 80) || '✓ Success';
  }

  return `Depth ${turn.depth}`;
}

// Collapsible section component
function CollapsibleSection({
  title,
  defaultOpen = false,
  children,
  badge,
  ...props
}: {
  title: string;
  defaultOpen?: boolean;
  children: React.ReactNode;
  badge?: React.ReactNode;
} & React.HTMLAttributes<HTMLDivElement>) {
  const [isOpen, setIsOpen] = useState(defaultOpen);

  return (
    <div className="border border-theme-border/50 rounded-lg overflow-hidden" {...props}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="w-full px-3 py-2 flex items-center justify-between text-left bg-theme-bg-tertiary/30 hover:bg-theme-bg-tertiary/50 transition-colors"
      >
        <div className="flex items-center gap-2">
          {isOpen ? (
            <ChevronDown className="w-4 h-4 text-theme-text-dim" />
          ) : (
            <ChevronRight className="w-4 h-4 text-theme-text-dim" />
          )}
          <span className="text-xs text-theme-text-muted font-medium">{title}</span>
        </div>
        {badge}
      </button>
      {isOpen && (
        <div className="p-3 border-t border-theme-border/50 bg-theme-bg-secondary/50">
          {children}
        </div>
      )}
    </div>
  );
}

// Legacy fallback renderer for non-canonical turns
function LegacyTurnContentView({ turn }: { turn: Turn }) {
  const kind = detectTurnKind(turn);
  const content = extractContent(turn);
  const toolCalls = extractToolCalls(turn);
  const toolResult = extractToolResult(turn);
  const colors = getKindColors(kind);

  return (
    <div className="space-y-3">
      {/* Main content */}
      {content && (
        <div className="text-sm text-theme-text-secondary whitespace-pre-wrap leading-relaxed">
          {content}
        </div>
      )}

      {/* Tool calls */}
      {toolCalls.length > 0 && (
        <div className="space-y-2">
          {toolCalls.map((tc, idx) => (
            <div key={idx} className="border border-amber-500/30 rounded-lg overflow-hidden">
              <div className="px-3 py-2 bg-amber-500/10 flex items-center gap-2">
                <Wrench className="w-4 h-4 text-amber-400" />
                <span className="text-sm font-medium text-amber-300">{tc.name}</span>
                <span className="text-xs text-theme-text-dim font-mono">{tc.id}</span>
              </div>
              <div className="p-3 bg-theme-bg-secondary/50">
                <pre className="text-xs text-theme-text-secondary whitespace-pre-wrap break-words font-mono">
                  {formatArguments(tc.arguments)}
                </pre>
              </div>
            </div>
          ))}
        </div>
      )}

      {/* Tool result */}
      {toolResult && (
        <div className={cn(
          'border rounded-lg overflow-hidden',
          toolResult.isError ? 'border-red-500/30' : 'border-emerald-500/30'
        )}>
          <div className={cn(
            'px-3 py-2 flex items-center gap-2',
            toolResult.isError ? 'bg-red-500/10' : 'bg-emerald-500/10'
          )}>
            {toolResult.isError ? (
              <XCircle className="w-4 h-4 text-red-400" />
            ) : (
              <CheckCircle className="w-4 h-4 text-emerald-400" />
            )}
            <span className={cn(
              'text-xs font-medium',
              toolResult.isError ? 'text-red-300' : 'text-emerald-300'
            )}>
              {toolResult.isError ? 'Error' : 'Result'}
            </span>
            <span className="text-xs text-theme-text-dim font-mono">{toolResult.toolCallId}</span>
          </div>
          <div className="p-3 bg-theme-bg-secondary/50">
            <pre className="text-xs text-theme-text-secondary whitespace-pre-wrap break-words font-mono max-h-[300px] overflow-y-auto">
              {toolResult.content}
            </pre>
          </div>
        </div>
      )}

      {/* Fallback if nothing else */}
      {!content && toolCalls.length === 0 && !toolResult && (
        <div className="text-sm text-theme-text-dim italic">No content</div>
      )}
    </div>
  );
}

// Render content view for a turn - uses specialized renderers based on type detection
function TurnContentView({ turn }: { turn: Turn }) {
  // Try canonical rendering first (checks for item_type field - codergen-sdk types)
  if (isConversationItem(turn.data)) {
    return <TryRenderCanonical data={turn.data} fallback={<LegacyTurnContentView turn={turn} />} />;
  }

  // Try ai-agents-sdk.Message format (has Role and Parts fields)
  if (isAgentMessage(turn.data)) {
    return <MessageRenderer message={turn.data} />;
  }

  // Try quest.Event format (has event_type and quest_id)
  if (isQuestEvent(turn.data)) {
    return <QuestEventRenderer event={turn.data} />;
  }

  // Try quest.Snapshot format (has trigger and file_count)
  if (isQuestSnapshot(turn.data)) {
    return <QuestSnapshotRenderer snapshot={turn.data} />;
  }

  // If we have data but didn't match any known type, use smart fallback renderer
  if (turn.data !== null && turn.data !== undefined) {
    return <FallbackRenderer data={turn.data} />;
  }

  // Truly empty - use legacy view which handles this gracefully
  return <LegacyTurnContentView turn={turn} />;
}

type ToolResultHydration =
  | { state: 'loading' }
  | { state: 'ready'; turn: Turn }
  | { state: 'error' };

interface ToolResultMatchesProps {
  contextId: string;
  turn: Turn;
  resultTurns: Map<string, Turn>;
}

/**
 * Show results for calls which are represented by separate turns.
 *
 * The list endpoint is deliberately bounded for large traces. A result found
 * in that list can therefore still contain a 512-character prefix. Hydrate
 * each matched result by ID before rendering it. Missing means "not in the
 * loaded page", not "the store has no such result".
 */
function ToolResultMatches({ contextId, turn, resultTurns }: ToolResultMatchesProps) {
  const calls = useMemo(() => extractToolCalls(turn), [turn]);
  const matches = useMemo(() => calls
    .filter(call => !hasEmbeddedToolResult(turn, call.id))
    .map(call => ({
      call,
      resultTurn: resultTurns.get(call.id) ?? null,
      key: `${call.id}:${resultTurns.get(call.id)?.turn_id ?? 'missing'}`,
    })), [calls, resultTurns, turn]);
  const matchKey = matches.map(match => match.key).join('|');
  const [hydration, setHydration] = useState<Record<string, ToolResultHydration>>({});

  useEffect(() => {
    let cancelled = false;
    const initial: Record<string, ToolResultHydration> = {};
    for (const match of matches) {
      if (match.resultTurn) initial[match.key] = { state: 'loading' };
    }
    setHydration(initial);

    const matchedResults = matches.filter((match): match is typeof match & { resultTurn: Turn } => (
      match.resultTurn !== null
    ));
    if (matchedResults.length === 0) return () => { cancelled = true; };

    for (const match of matchedResults) {
      fetchTurn(contextId, match.resultTurn.turn_id)
        .then(exactTurn => {
          if (!cancelled) {
            setHydration(previous => ({
              ...previous,
              [match.key]: { state: 'ready', turn: exactTurn },
            }));
          }
        })
        .catch(() => {
          if (!cancelled) {
            setHydration(previous => ({ ...previous, [match.key]: { state: 'error' } }));
          }
        });
    }

    return () => { cancelled = true; };
  }, [contextId, matchKey, matches]);

  if (matches.length === 0) return null;

  return (
    <CollapsibleSection
      title="Tool Results"
      defaultOpen
      data-tool-result-matches
      badge={<span className="text-[10px] text-theme-text-faint">{matches.length}</span>}
    >
      <div className="space-y-3">
        {matches.map(match => {
          const result = hydration[match.key];
          return (
            <div
              key={match.key}
              data-tool-result-match={match.call.id}
              data-tool-result-state={
                !match.resultTurn ? 'missing' : result?.state ?? 'loading'
              }
              className="border border-theme-border/50 rounded-lg p-3"
            >
              <div className="flex items-center gap-2 mb-2 text-xs">
                <Terminal className="w-3.5 h-3.5 text-theme-success" />
                <span className="text-theme-text-muted">{match.call.name}</span>
                <span className="font-mono text-theme-text-faint">{match.call.id}</span>
                {match.resultTurn && (
                  <span className="ml-auto font-mono text-theme-text-faint">
                    Turn #{match.resultTurn.turn_id}
                  </span>
                )}
              </div>

              {!match.resultTurn ? (
                <div className="flex items-center gap-2 text-xs text-theme-text-dim">
                  <AlertCircle className="w-3.5 h-3.5" />
                  No separate result turn in the loaded page.
                </div>
              ) : result?.state === 'error' ? (
                <div className="flex items-center gap-2 text-xs text-red-400">
                  <AlertCircle className="w-3.5 h-3.5" />
                  Failed to load the complete result turn.
                </div>
              ) : result?.state === 'ready' ? (
                <TurnContentView turn={result.turn} />
              ) : (
                <div className="flex items-center gap-2 text-xs text-theme-text-dim">
                  <Loader2 className="w-3.5 h-3.5 animate-spin" />
                  Loading the complete result…
                </div>
              )}
            </div>
          );
        })}
      </div>
    </CollapsibleSection>
  );
}

// Format arguments JSON for display
function formatArguments(args: string): string {
  try {
    const parsed = JSON.parse(args);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return args;
  }
}

interface ContextDebuggerProps {
  contextId: string;
  isOpen: boolean;
  onClose: () => void;
  lastEvent?: import('@/types').StoreEvent | null;
  /** Initial turn ID to select (from URL) */
  initialTurnId?: string | null;
  /** Callback when selected turn changes */
  onTurnChange?: (turnId: string | null) => void;
  /** Callback to navigate to a different context */
  onNavigateToContext?: (contextId: string) => void;
}

export function ContextDebugger({ contextId, isOpen, onClose, lastEvent, initialTurnId, onTurnChange, onNavigateToContext }: ContextDebuggerProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const turnListRef = useRef<HTMLDivElement | null>(null);
  const lastResetContextIdRef = useRef<string | null>(null);
  const [query, setQuery] = useState('');
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [copied, setCopied] = useState<'context' | 'event' | null>(null);
  const [initialTurnApplied, setInitialTurnApplied] = useState(false);

  // Data fetching state
  const [loading, setLoading] = useState(false);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<TurnResponse | null>(null);
  const [hasMoreTurns, setHasMoreTurns] = useState(false);
  const [selectedTurnDetail, setSelectedTurnDetail] = useState<Turn | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [searchHydrating, setSearchHydrating] = useState(false);
  const [searchHydrationError, setSearchHydrationError] = useState<string | null>(null);

  // Live observer state
  const [newTurnIds, setNewTurnIds] = useState<Set<string>>(new Set());
  // Don't auto-follow if user deep-linked to a specific turn
  const [isFollowing, setIsFollowing] = useState(!initialTurnId);

  // Filesystem browser state
  const [hasFilesystem, setHasFilesystem] = useState(false);
  const [selectedFilePath, setSelectedFilePath] = useState<string | null>(null);

  // Detail view tab state
  const [detailView, setDetailView] = useState<DetailView>('turn');

  // Renderer manifest for dynamic renderer
  const { manifest } = useRendererManifest();

  // Fetch turns when context changes
  const loadTurns = useCallback(async () => {
    if (!contextId) return;

    setLoading(true);
    setError(null);

    try {
      const response = await fetchTurns(contextId, {
        limit: TURN_PAGE_SIZE,
        view: 'typed',
        include_unknown: true,
        string_limit: TURN_LIST_STRING_LIMIT,
      });
      setData(response);
      setHasMoreTurns(response.turns.length === TURN_PAGE_SIZE);
    } catch (err) {
      if (err instanceof ApiError) {
        setError(err.message);
      } else {
        setError('Failed to fetch turns');
      }
      setData(null);
    } finally {
      setLoading(false);
    }
  }, [contextId]);

  const loadOlderTurns = useCallback(async () => {
    if (!contextId || !data?.next_before_turn_id || loadingOlder) return;

    setLoadingOlder(true);
    setError(null);

    try {
      const response = await fetchTurns(contextId, {
        limit: TURN_PAGE_SIZE,
        before_turn_id: data.next_before_turn_id,
        view: 'typed',
        include_unknown: true,
        string_limit: TURN_LIST_STRING_LIMIT,
      });
      setData(prev => {
        if (!prev) return response;
        const seen = new Set(prev.turns.map(turn => turn.turn_id));
        const olderTurns = response.turns.filter(turn => !seen.has(turn.turn_id));
        if (olderTurns.length === 0) {
          return {
            ...prev,
            next_before_turn_id: response.next_before_turn_id,
          };
        }
        setSelectedIdx(idx => idx + olderTurns.length);
        return {
          ...prev,
          turns: [...olderTurns, ...prev.turns],
          next_before_turn_id: response.next_before_turn_id,
        };
      });
      setHasMoreTurns(response.turns.length === TURN_PAGE_SIZE);
    } catch (err) {
      if (err instanceof ApiError) {
        setError(err.message);
      } else {
        setError('Failed to fetch older turns');
      }
    } finally {
      setLoadingOlder(false);
    }
  }, [contextId, data?.next_before_turn_id, loadingOlder]);

  useEffect(() => {
    if (isOpen && contextId) {
      loadTurns();
    }
  }, [isOpen, contextId, loadTurns]);

  // Handle incoming turn events for live updates
  useEffect(() => {
    if (!lastEvent || lastEvent.type !== 'turn_appended') return;
    if (lastEvent.data.context_id !== contextId) return;

    // Mark the new turn for animation
    const newTurnId = lastEvent.data.turn_id;
    setNewTurnIds(prev => new Set(prev).add(newTurnId));

    // Clear the animation class after animation completes
    const timer = setTimeout(() => {
      setNewTurnIds(prev => {
        const next = new Set(prev);
        next.delete(newTurnId);
        return next;
      });
    }, 3000); // Match highlight-fade animation duration

    // Reload turns to get the new one
    loadTurns();

    return () => clearTimeout(timer);
  }, [lastEvent, contextId, loadTurns]);

  // Handle scroll to detect user scrolling away
  const handleTurnListScroll = useCallback((e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    const isAtBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 50;
    if (!isAtBottom && isFollowing) {
      setIsFollowing(false);
    }
  }, [isFollowing]);

  // Resume following
  const resumeFollowing = useCallback(() => {
    setIsFollowing(true);
    if (turnListRef.current) {
      turnListRef.current.scrollTop = turnListRef.current.scrollHeight;
    }
  }, []);

  // Filter turns by search query
  const hasSearchQuery = query.trim().length > 0;
  const isSummaryPage = typeof data?.meta.string_limit === 'number';
  const filteredTurns = useMemo(() => {
    if (!data?.turns) return [];
    const q = query.trim().toLowerCase();
    if (!q) return data.turns;
    // Never present prefix-only filtering as a complete search result.
    if (typeof data.meta.string_limit === 'number') return [];

    return data.turns.filter(turn => {
      const content = extractContent(turn)?.toLowerCase() ?? '';
      const toolCalls = extractToolCalls(turn);
      const toolNames = toolCalls.map(tc => tc.name.toLowerCase()).join(' ');
      const kind = detectTurnKind(turn);
      return content.includes(q) || toolNames.includes(q) || kind.includes(q);
    });
  }, [data, query]);

  // Selected turn
  const selectedListTurn = filteredTurns[selectedIdx] ?? null;
  const selectedTurn = selectedTurnDetail?.turn_id === selectedListTurn?.turn_id
    ? selectedTurnDetail
    : selectedListTurn;
  const selectedTurnIsExact = !isSummaryPage
    || selectedTurnDetail?.turn_id === selectedListTurn?.turn_id;

  // Build a bounded index from the turns already loaded. Matching must not
  // fetch the complete context history, especially for large traces.
  const separateToolResultTurns = useMemo(() => {
    const resultTurns = new Map<string, Turn>();
    for (const turn of data?.turns ?? []) {
      const result = extractToolResult(turn);
      if (result && result.toolCallId !== 'unknown') {
        resultTurns.set(result.toolCallId, turn);
      }
    }
    return resultTurns;
  }, [data]);

  // List pages carry bounded string prefixes. Fetch only the selected turn's
  // complete payload for the detail renderer.
  useEffect(() => {
    const turnId = selectedListTurn?.turn_id;
    if (!turnId || typeof data?.meta.string_limit !== 'number') {
      setSelectedTurnDetail(null);
      setDetailLoading(false);
      setDetailError(null);
      return;
    }

    let cancelled = false;
    let requestStarted = false;
    setSelectedTurnDetail(null);
    setDetailLoading(true);
    setDetailError(null);
    // Selection can move from the first rendered row to the followed tail in
    // the same render cycle. Debouncing avoids transferring the discarded
    // detail and also coalesces rapid keyboard navigation.
    const timer = window.setTimeout(() => {
      requestStarted = true;
      fetchTurn(contextId, turnId)
        .then(turn => {
          if (!cancelled) setSelectedTurnDetail(turn);
        })
        .catch(() => {
          if (!cancelled) setDetailError('Failed to load the complete turn.');
        })
        .finally(() => {
          if (!cancelled) setDetailLoading(false);
        });
    }, 25);
    return () => {
      cancelled = true;
      if (!requestStarted) window.clearTimeout(timer);
    };
  }, [contextId, data?.meta.string_limit, selectedListTurn?.turn_id]);

  // Preserve full-text filtering semantics: the common browsing path uses
  // summaries, while entering a query hydrates every currently loaded turn.
  useEffect(() => {
    if (
      !hasSearchQuery
      || !data
      || typeof data.meta.string_limit !== 'number'
    ) {
      if (!hasSearchQuery) {
        setSearchHydrating(false);
        setSearchHydrationError(null);
      }
      return;
    }
    let cancelled = false;
    setSearchHydrating(true);
    setSearchHydrationError(null);
    fetchTurns(contextId, {
      limit: data.turns.length,
      view: 'typed',
      include_unknown: true,
    })
      .then(response => {
        if (!cancelled) {
          setData(response);
          setSelectedTurnDetail(null);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setSearchHydrationError('Failed to load complete turns for search.');
        }
      })
      .finally(() => {
        if (!cancelled) setSearchHydrating(false);
      });
    return () => { cancelled = true; };
  }, [contextId, data, hasSearchQuery]);

  // Detect filesystem for selected turn
  const selectedTurnId = selectedTurn?.turn_id;
  useEffect(() => {
    if (!selectedTurnId) {
      setHasFilesystem(false);
      setSelectedFilePath(null);
      return;
    }

    let cancelled = false;

    async function checkFilesystem() {
      try {
        await fetchFsDirectory(selectedTurnId, '');
        if (!cancelled) {
          setHasFilesystem(true);
        }
      } catch {
        if (!cancelled) {
          setHasFilesystem(false);
          setSelectedFilePath(null);
        }
      }
    }

    checkFilesystem();
    return () => { cancelled = true; };
  }, [selectedTurnId]);

  // Helper to select turn by index and notify parent
  const selectTurn = useCallback((idx: number) => {
    setSelectedIdx(idx);
    const turn = filteredTurns[idx];
    if (turn && onTurnChange) {
      onTurnChange(turn.turn_id);
    }
  }, [filteredTurns, onTurnChange]);

  // Apply initial turn ID from URL when data loads
  useEffect(() => {
    if (!data?.turns || initialTurnApplied) return;

    if (initialTurnId) {
      const idx = filteredTurns.findIndex(t => t.turn_id === initialTurnId);
      if (idx >= 0) {
        setSelectedIdx(idx);
        setInitialTurnApplied(true);
        return;
      }

      // A deep link can target a turn outside the bounded first page. Hydrate
      // that exact turn and add it to the list so the URL and visible selection
      // cannot disagree.
      let cancelled = false;
      setDetailLoading(true);
      setDetailError(null);
      fetchTurn(contextId, initialTurnId)
        .then(turn => {
          if (cancelled) return;
          setData(previous => {
            if (!previous || previous.turns.some(item => item.turn_id === turn.turn_id)) {
              return previous;
            }
            return { ...previous, turns: [turn, ...previous.turns] };
          });
          setSelectedIdx(0);
          setSelectedTurnDetail(turn);
          setInitialTurnApplied(true);
        })
        .catch(() => {
          if (!cancelled) {
            setDetailError('Failed to load the linked turn.');
            setInitialTurnApplied(true);
          }
        })
        .finally(() => {
          if (!cancelled) setDetailLoading(false);
        });
      return () => { cancelled = true; };
    } else if (filteredTurns.length > 0) {
      // No initial turn specified, notify parent of first turn
      const firstTurn = filteredTurns[0];
      if (firstTurn && onTurnChange) {
        onTurnChange(firstTurn.turn_id);
      }
      setInitialTurnApplied(true);
    }
  }, [contextId, data, initialTurnId, initialTurnApplied, filteredTurns, onTurnChange]);

  // Count stats - count both tool_call turns AND tool_calls embedded in assistant turns
  const stats = useMemo(() => {
    if (!data?.turns) return { loaded: 0, total: 0, toolCalls: 0, errors: 0 };
    let toolCalls = 0;
    let errors = 0;
    for (const turn of data.turns) {
      const kind = detectTurnKind(turn);
      // Count tool_call turns (each is one tool invocation)
      if (kind === 'tool_call') {
        toolCalls++;
      }
      // Also count embedded tool_calls in assistant turns
      toolCalls += extractToolCalls(turn).length;
      // Count errors from tool results
      const result = extractToolResult(turn);
      if (result?.isError) errors++;
    }
    return {
      loaded: data.turns.length,
      total: data.meta.head_turn_id === '0' ? 0 : data.meta.head_depth + 1,
      toolCalls,
      errors,
    };
  }, [data]);

  // Auto-select last turn when following and new turns arrive
  useEffect(() => {
    if (!isFollowing || filteredTurns.length === 0) return;

    // Select the last turn (newest)
    const lastIdx = filteredTurns.length - 1;
    setSelectedIdx(lastIdx);

    // Notify parent of selection change
    const lastTurn = filteredTurns[lastIdx];
    if (lastTurn && onTurnChange) {
      onTurnChange(lastTurn.turn_id);
    }
  }, [filteredTurns.length, isFollowing, filteredTurns, onTurnChange]);

  // Scroll selected turn into view when selection changes
  useEffect(() => {
    if (!turnListRef.current || filteredTurns.length === 0) return;

    const selectedTurnId = filteredTurns[selectedIdx]?.turn_id;
    if (!selectedTurnId) return;

    const selectedEl = turnListRef.current.querySelector(`[data-turn-id="${selectedTurnId}"]`);
    if (selectedEl) {
      selectedEl.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }
  }, [selectedIdx, filteredTurns]);

  // Reset state when modal opens/closes
  useEffect(() => {
    if (!isOpen) {
      lastResetContextIdRef.current = null;
      return;
    }

    // Don't reset state on URL turn-id changes while open; only reset on open/context changes.
    if (lastResetContextIdRef.current === contextId) {
      return;
    }
    lastResetContextIdRef.current = contextId;

    setQuery('');
    // Only reset to 0 if no initialTurnId; otherwise let the initialTurn effect handle it
    if (!initialTurnId) {
      setSelectedIdx(0);
    }
    setCopied(null);
    requestAnimationFrame(() => containerRef.current?.focus());
  }, [isOpen, contextId, initialTurnId]);

  // Allow URL-driven turn selection changes (e.g. browser back/forward) to re-apply without
  // wiping the user's current filter query.
  useEffect(() => {
    if (!isOpen) return;
    setInitialTurnApplied(false);
  }, [isOpen, initialTurnId]);

  // Clear copied state after delay
  useEffect(() => {
    if (!copied) return;
    const t = window.setTimeout(() => setCopied(null), 1200);
    return () => window.clearTimeout(t);
  }, [copied]);

  if (!isOpen) return null;

  const handleCopy = async (kind: 'context' | 'event') => {
    try {
      let value: unknown;
      if (kind === 'context' && data && typeof data.meta.string_limit === 'number') {
        const complete = await fetchTurns(contextId, {
          limit: data.turns.length,
          view: 'typed',
          include_unknown: true,
        });
        setData(complete);
        value = complete;
      } else if (
        kind === 'event'
        && selectedListTurn
        && typeof data?.meta.string_limit === 'number'
        && selectedTurn?.turn_id !== selectedTurnDetail?.turn_id
      ) {
        const complete = await fetchTurn(contextId, selectedListTurn.turn_id);
        setSelectedTurnDetail(complete);
        value = complete;
      } else {
        value = kind === 'context'
          ? data ?? { error: 'No data' }
          : selectedTurn ?? {};
      }
      const text = safeStringify(value);
      await navigator.clipboard.writeText(text);
      setCopied(kind);
    } catch {
      // Ignore clipboard errors
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      // Close file viewer first if open, otherwise close debugger
      if (selectedFilePath) {
        setSelectedFilePath(null);
      } else {
        onClose();
      }
      e.preventDefault();
      return;
    }

    if ((e.key === 'k' || e.key === 'K') && (e.metaKey || e.ctrlKey)) {
      const input = containerRef.current?.querySelector<HTMLInputElement>('input[data-debug-search]');
      input?.focus();
      e.preventDefault();
      return;
    }

    if ((e.key === 'r' || e.key === 'R') && (e.metaKey || e.ctrlKey)) {
      loadTurns();
      e.preventDefault();
      return;
    }

    const hasModifier = e.metaKey || e.ctrlKey || e.altKey;
    if (!hasModifier && (e.key === 'j' || e.key === 'ArrowDown')) {
      selectTurn(Math.min(selectedIdx + 1, filteredTurns.length - 1));
      e.preventDefault();
      return;
    }
    if (!hasModifier && (e.key === 'k' || e.key === 'ArrowUp')) {
      selectTurn(Math.max(selectedIdx - 1, 0));
      e.preventDefault();
      return;
    }
    // Resume following with 'F'
    if (!hasModifier && (e.key === 'f' || e.key === 'F')) {
      resumeFollowing();
      e.preventDefault();
      return;
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/70 backdrop-blur-sm" role="dialog" aria-modal="true">
      <div
        ref={containerRef}
        tabIndex={-1}
        onKeyDown={handleKeyDown}
        className="flex h-[100dvh] w-full flex-col outline-none"
        data-context-debugger
      >
        {/* Header - more compact */}
        <div className="flex min-h-12 shrink-0 items-center justify-between gap-2 border-b border-theme-border bg-theme-bg-secondary px-2 py-2 sm:px-4">
          <div className="flex min-w-0 items-center gap-3">
            <div className="flex min-w-0 items-center gap-2">
              <Layers className="h-5 w-5 shrink-0 text-theme-accent" />
              <span className="truncate text-sm font-semibold text-theme-text">Context {contextId}</span>
            </div>
            {data && (
              <div className="hidden items-center gap-3 text-xs text-theme-text-dim sm:flex">
                <span>{stats.loaded} of {stats.total} turns loaded</span>
                <span className="hidden lg:inline">{stats.toolCalls} tool calls</span>
                {stats.errors > 0 && (
                  <span className="hidden text-red-400 lg:inline">{stats.errors} errors</span>
                )}
              </div>
            )}
          </div>

          <div className="flex shrink-0 items-center gap-1 sm:gap-2">
            <button
              onClick={() => handleCopy('context')}
              disabled={loading || !data}
              data-copy-all
              aria-label="Copy all context data"
              className={cn(
                'inline-flex min-h-9 items-center gap-1 rounded border px-2 text-xs transition-colors sm:min-h-0 sm:px-2.5 sm:py-1',
                copied === 'context'
                  ? 'bg-emerald-600/20 border-emerald-500/30 text-emerald-300'
                  : 'bg-theme-bg-tertiary border-theme-border text-theme-text-secondary hover:bg-theme-bg-hover disabled:opacity-50'
              )}
            >
              <Copy className="w-3 h-3" />
              <span className="hidden sm:inline">{copied === 'context' ? 'Copied!' : 'Copy all'}</span>
            </button>
            <button
              onClick={loadTurns}
              disabled={loading}
              aria-label="Refresh turns"
              className="min-h-9 rounded border border-theme-border bg-theme-bg-tertiary px-2 text-xs text-theme-text-secondary hover:bg-theme-bg-hover disabled:opacity-50 sm:min-h-0 sm:px-2.5 sm:py-1"
            >
              <span className="sm:hidden">↻</span>
              <span className="hidden sm:inline">{loading ? 'Loading...' : 'Refresh'}</span>
            </button>
            <button
              onClick={onClose}
              aria-label="Close context debugger"
              className="rounded p-2 text-theme-text-muted transition-colors hover:bg-theme-bg-tertiary hover:text-theme-text-secondary sm:p-1.5"
            >
              <X className="w-5 h-5" />
            </button>
          </div>
        </div>

        {/* Body */}
        <div className="flex min-h-0 flex-1 flex-col md:flex-row">
          {/* Left: Turn list - more compact */}
          <div className="flex h-[38%] min-h-[12rem] w-full shrink-0 flex-col border-b border-theme-border bg-theme-bg-secondary/40 md:h-auto md:min-h-0 md:w-80 md:border-b-0 md:border-r">
            <div className="p-2 border-b border-theme-border">
              <div className="relative">
                <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-theme-text-dim" />
                <input
                  data-debug-search
                  value={query}
                  onChange={(e) => { setQuery(e.target.value); setSelectedIdx(0); }}  // Note: URL will update on next selection
                  placeholder="Filter turns..."
                  className="w-full pl-9 pr-3 py-1.5 bg-theme-bg-secondary border border-theme-border rounded text-sm text-theme-text-secondary placeholder:text-theme-text-faint focus:outline-none focus:ring-1 focus:ring-theme-accent/50"
                />
              </div>
            </div>

            <div
              ref={turnListRef}
              onScroll={handleTurnListScroll}
              className={cn('overflow-y-auto relative', hasFilesystem ? 'flex-1 min-h-0' : 'flex-1')}
              data-debug-event-list
            >
              {!loading && !error && data && hasMoreTurns && !query.trim() && (
                <div className="p-2 border-b border-theme-border-dim/60 bg-theme-bg-secondary/50">
                  <button
                    onClick={loadOlderTurns}
                    disabled={loadingOlder}
                    className="w-full px-3 py-1.5 rounded border border-theme-border bg-theme-bg-tertiary text-xs text-theme-text-secondary hover:bg-theme-bg-hover disabled:opacity-50 inline-flex items-center justify-center gap-2"
                  >
                    {loadingOlder ? (
                      <>
                        <Loader2 className="w-3.5 h-3.5 animate-spin" />
                        Loading older turns...
                      </>
                    ) : (
                      'Load older turns'
                    )}
                  </button>
                </div>
              )}
              {loading ? (
                <div className="p-6 flex flex-col items-center justify-center text-theme-text-dim">
                  <Loader2 className="w-6 h-6 animate-spin mb-2" />
                  <span className="text-xs">Loading...</span>
                </div>
              ) : error ? (
                <div className="p-6 flex flex-col items-center justify-center text-red-400">
                  <AlertCircle className="w-6 h-6 mb-2" />
                  <span className="text-xs">{error}</span>
                </div>
              ) : searchHydrating ? (
                <div className="p-6 flex flex-col items-center justify-center text-theme-text-dim">
                  <Loader2 className="w-6 h-6 animate-spin mb-2" />
                  <span className="text-xs">Loading complete turns for search…</span>
                </div>
              ) : searchHydrationError ? (
                <div className="p-6 flex flex-col items-center justify-center text-red-400">
                  <AlertCircle className="w-6 h-6 mb-2" />
                  <span className="text-xs">{searchHydrationError}</span>
                </div>
              ) : filteredTurns.length === 0 ? (
                <div className="p-6 text-xs text-theme-text-dim text-center">
                  {data?.turns.length === 0 ? 'No turns.' : 'No matches.'}
                </div>
              ) : (
                filteredTurns.map((turn, idx) => {
                  const kind = detectTurnKind(turn);
                  const colors = getKindColors(kind);
                  const isSelected = idx === selectedIdx;
                  const summary = buildSummary(turn, kind);
                  const toolCalls = extractToolCalls(turn);
                  const toolResult = extractToolResult(turn);
                  const isNewTurn = newTurnIds.has(turn.turn_id);

                  return (
                    <button
                      key={turn.turn_id}
                      data-turn-id={turn.turn_id}
                      onClick={() => selectTurn(idx)}
                      className={cn(
                        'w-full text-left px-3 py-2 border-l-2 border-b border-theme-border-dim/60 transition-all',
                        isSelected ? 'bg-theme-bg-tertiary/70' : 'hover:bg-theme-bg-tertiary/40',
                        colors.border,
                        // Animation classes for new turns
                        isNewTurn && 'animate-slide-up animate-highlight-fade'
                      )}
                    >
                      <div className="flex items-center gap-2 mb-0.5">
                        <KindIcon kind={kind} className={cn('w-3.5 h-3.5', colors.text)} />
                        <span className={cn('text-[11px] font-medium uppercase tracking-wide', colors.text)}>
                          {getKindLabel(kind)}
                        </span>
                        {toolCalls.length > 0 && (
                          <span className="text-[10px] text-amber-400 font-mono">
                            {toolCalls.map(tc => tc.name).join(', ')}
                          </span>
                        )}
                        {toolResult?.isError && (
                          <XCircle className="w-3 h-3 text-red-400" />
                        )}
                        <span className="ml-auto text-[10px] text-theme-text-faint font-mono">
                          #{turn.turn_id}
                        </span>
                      </div>
                      <div className="text-xs text-theme-text-secondary leading-snug truncate">
                        {summary}
                      </div>
                    </button>
                  );
                })
              )}

              {/* Resume following indicator */}
              {!isFollowing && filteredTurns.length > 0 && (
                <div className="sticky bottom-2 left-0 right-0 flex justify-center pointer-events-none">
                  <button
                    onClick={resumeFollowing}
                    className="pointer-events-auto flex items-center gap-2 px-3 py-1.5 bg-theme-bg-tertiary/90 backdrop-blur-sm border border-theme-border rounded-full text-xs text-theme-text-secondary hover:bg-theme-bg-hover/90 hover:text-white hover:border-theme-accent/50 transition-all shadow-lg animate-slide-up"
                  >
                    <ChevronDown className="w-3.5 h-3.5 text-theme-accent" />
                    <span>Resume following</span>
                    <kbd className="px-1 py-0.5 text-[10px] bg-theme-bg-secondary rounded border border-theme-text-faint">F</kbd>
                  </button>
                </div>
              )}
            </div>

            {/* Filesystem browser (when available) */}
            {hasFilesystem && selectedTurn && (
              <div className="flex-1 min-h-0 border-t border-theme-border flex flex-col">
                <div className="px-3 py-2 border-b border-theme-border/50 flex items-center gap-2 flex-shrink-0">
                  <Folder className="w-3.5 h-3.5 text-amber-400" />
                  <span className="text-xs text-theme-text-muted font-medium">Filesystem</span>
                </div>
                <FileBrowser
                  turnId={selectedTurn.turn_id}
                  onFileSelect={setSelectedFilePath}
                  className="flex-1 min-h-0"
                />
              </div>
            )}
          </div>

          {/* Right: Detail view */}
          <div className="relative flex min-h-0 flex-1 flex-col overflow-hidden bg-theme-bg">
            {/* File viewer overlay */}
            {selectedFilePath && selectedTurn && (
              <FileViewer
                turnId={selectedTurn.turn_id}
                filePath={selectedFilePath}
                onClose={() => setSelectedFilePath(null)}
              />
            )}

            {!selectedTurn ? (
              <div className="flex-1 flex items-center justify-center text-theme-text-dim text-sm">
                Select a turn to view details
              </div>
            ) : (
              <>
                {/* Detail view tabs */}
                <div className="flex shrink-0 overflow-x-auto border-b border-theme-border-dim bg-theme-bg-secondary/50">
                  <button
                    onClick={() => setDetailView('turn')}
                    className={cn(
                      'shrink-0 px-3 py-2 text-xs uppercase tracking-wide transition-colors sm:px-4',
                      detailView === 'turn'
                        ? 'text-theme-accent border-b-2 border-theme-accent bg-theme-accent-muted'
                        : 'text-theme-text-dim hover:text-theme-text-muted'
                    )}
                  >
                    Turn
                  </button>
                  <button
                    onClick={() => setDetailView('provenance')}
                    className={cn(
                      'shrink-0 px-3 py-2 text-xs uppercase tracking-wide transition-colors sm:px-4',
                      detailView === 'provenance'
                        ? 'text-theme-accent border-b-2 border-theme-accent bg-theme-accent-muted'
                        : 'text-theme-text-dim hover:text-theme-text-muted'
                    )}
                  >
                    Provenance
                  </button>
                  <div className="flex-1" />
                  {detailView === 'turn' && (
                    <button
                      onClick={() => handleCopy('event')}
                      data-copy-event
                      aria-label="Copy selected turn"
                      className={cn(
                        'mr-1 my-1 shrink-0 px-2 py-1 text-xs rounded border transition-colors inline-flex items-center gap-1 sm:mr-2',
                        copied === 'event'
                          ? 'bg-emerald-600/20 border-emerald-500/30 text-emerald-300'
                          : 'bg-theme-bg-tertiary border-theme-border text-theme-text-muted hover:text-theme-text-secondary'
                      )}
                    >
                      <Copy className="w-3 h-3" />
                      <span className="hidden sm:inline">{copied === 'event' ? 'Copied' : 'Copy'}</span>
                    </button>
                  )}
                </div>

                {/* Turn header (when viewing turn) */}
                {detailView === 'turn' && (
                  <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-theme-border-dim/50 bg-theme-bg-secondary/30 px-3 py-2 sm:gap-3 sm:px-4">
                    <div className={cn(
                      'px-2 py-0.5 rounded text-xs font-medium',
                      getKindColors(detectTurnKind(selectedTurn)).badge
                    )}>
                      {getKindLabel(detectTurnKind(selectedTurn))}
                    </div>
                    <span className="break-all font-mono text-xs text-theme-text-dim">
                      Turn #{selectedTurn.turn_id} • Depth {selectedTurn.depth}
                    </span>
                  </div>
                )}

                {/* Content area - Turn view */}
                {detailView === 'turn' && (
                  <div className="flex-1 space-y-3 overflow-y-auto p-3 sm:p-4">
                    {!selectedTurnIsExact ? (
                      <div className={cn(
                        'flex items-center gap-2 text-sm',
                        detailError ? 'text-red-400' : 'text-theme-text-dim'
                      )}>
                        {detailError ? (
                          <AlertCircle className="w-4 h-4" />
                        ) : (
                          <Loader2 className="w-4 h-4 animate-spin" />
                        )}
                        {detailError ?? (detailLoading
                          ? 'Loading full turn…'
                          : 'Waiting for full turn…')}
                      </div>
                    ) : (
                      <>
                        {/* Primary content view - uses dynamic renderer registry */}
                        <DynamicRenderer
                          data={selectedTurn.data}
                          typeId={selectedTurn.declared_type?.type_id ?? ''}
                          typeVersion={selectedTurn.declared_type?.type_version ?? 1}
                          manifest={manifest}
                        />

                        <ToolResultMatches
                          contextId={contextId}
                          turn={selectedTurn}
                          resultTurns={separateToolResultTurns}
                        />

                        {/* Collapsible metadata */}
                        <CollapsibleSection
                          title="Turn Metadata"
                          badge={
                            <span className="text-[10px] text-theme-text-faint font-mono">
                              {selectedTurn.declared_type?.type_id?.split('.').pop()}
                            </span>
                          }
                        >
                          <div className="grid grid-cols-[auto,minmax(0,1fr)] gap-x-3 gap-y-1 text-xs sm:gap-x-4">
                            <div className="text-theme-text-dim">Turn ID</div>
                            <div className="text-theme-text-secondary font-mono">{selectedTurn.turn_id}</div>
                            <div className="text-theme-text-dim">Parent</div>
                            <div className="text-theme-text-secondary font-mono">{selectedTurn.parent_turn_id || '(root)'}</div>
                            <div className="text-theme-text-dim">Depth</div>
                            <div className="text-theme-text-secondary">{selectedTurn.depth}</div>
                            {selectedTurn.declared_type && (
                              <>
                                <div className="text-theme-text-dim">Type</div>
                                <div className="break-all font-mono text-[11px] text-theme-text-secondary">
                                  {selectedTurn.declared_type.type_id}@{selectedTurn.declared_type.type_version}
                                </div>
                              </>
                            )}
                          </div>
                        </CollapsibleSection>

                        {/* Collapsible raw payload */}
                        <CollapsibleSection title="Raw Payload" data-raw-payload-section>
                          <pre data-raw-payload className="text-[11px] text-theme-text-muted whitespace-pre-wrap break-words font-mono leading-relaxed max-h-[300px] overflow-y-auto">
                            {safeStringify(selectedTurn.data)}
                          </pre>
                        </CollapsibleSection>
                      </>
                    )}
                  </div>
                )}

                {/* Content area - Provenance view */}
                {detailView === 'provenance' && (
                  <div className="flex-1 overflow-y-auto">
                    <ProvenancePanel
                      contextId={contextId}
                      className="divide-y divide-theme-border-dim/60"
                      onContextClick={(linkedContextId) => {
                        // Navigate to the linked context via SPA routing
                        if (onNavigateToContext) {
                          onNavigateToContext(linkedContextId);
                        }
                      }}
                    />
                  </div>
                )}

                {/* Footer */}
                <div className="hidden items-center gap-4 border-t border-theme-border-dim bg-theme-bg-secondary/50 px-4 py-1.5 text-[11px] text-theme-text-faint sm:flex">
                  <span><kbd className="px-1 py-0.5 bg-theme-bg-tertiary rounded">j</kbd>/<kbd className="px-1 py-0.5 bg-theme-bg-tertiary rounded">k</kbd> Navigate</span>
                  <span><kbd className="px-1 py-0.5 bg-theme-bg-tertiary rounded">F</kbd> Follow</span>
                  <span><kbd className="px-1 py-0.5 bg-theme-bg-tertiary rounded">⌘K</kbd> Search</span>
                  <span><kbd className="px-1 py-0.5 bg-theme-bg-tertiary rounded">Esc</kbd> Close</span>
                </div>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export default ContextDebugger;
