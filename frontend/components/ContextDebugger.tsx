'use client';

import { useEffect, useMemo, useRef, useState, useCallback } from 'react';
import type { Turn, TurnResponse, DebugEvent } from '@/types';
import { Layers, Hash, X, Copy, Search, Loader2, AlertCircle, GitBranch, ChevronDown, ChevronRight, Terminal, MessageSquare, Wrench, CheckCircle, XCircle, Folder, Zap, Database } from './icons';
import { cn, trunc, safeStringify, formatTime, contentPreview } from '@/lib/utils';
import { fetchTurns, fetchFsDirectory, ApiError } from '@/lib/api';
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

const TURNS_PAGE_SIZE = 100;

export function ContextDebugger({ contextId, isOpen, onClose, lastEvent, initialTurnId, onTurnChange, onNavigateToContext }: ContextDebuggerProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const turnListRef = useRef<HTMLDivElement | null>(null);
  const [query, setQuery] = useState('');
  const [selectedIdx, setSelectedIdx] = useState(0);
  const [copied, setCopied] = useState<'context' | 'event' | null>(null);
  const [initialTurnApplied, setInitialTurnApplied] = useState(false);

  // Data fetching state
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [data, setData] = useState<TurnResponse | null>(null);

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
        limit: TURNS_PAGE_SIZE,
        view: 'typed',
        include_unknown: true,
      });
      setData(response);
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

  // Load older turns using pagination cursor
  const loadMore = useCallback(async () => {
    if (!contextId || !data?.next_before_turn_id) return;

    setLoadingMore(true);
    try {
      const response = await fetchTurns(contextId, {
        limit: TURNS_PAGE_SIZE,
        before_turn_id: data.next_before_turn_id,
        view: 'typed',
        include_unknown: true,
      });
      const prepended = response.turns.length;
      setData(prev => prev ? {
        ...prev,
        turns: [...response.turns, ...prev.turns],
        next_before_turn_id: response.next_before_turn_id,
      } : response);
      setSelectedIdx(prev => prev + prepended);
    } catch {
      // Keep existing data on failure
    } finally {
      setLoadingMore(false);
    }
  }, [contextId, data?.next_before_turn_id]);

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
  const filteredTurns = useMemo(() => {
    if (!data?.turns) return [];
    const q = query.trim().toLowerCase();
    if (!q) return data.turns;

    return data.turns.filter(turn => {
      const content = extractContent(turn)?.toLowerCase() ?? '';
      const toolCalls = extractToolCalls(turn);
      const toolNames = toolCalls.map(tc => tc.name.toLowerCase()).join(' ');
      const kind = detectTurnKind(turn);
      return content.includes(q) || toolNames.includes(q) || kind.includes(q);
    });
  }, [data, query]);

  // Selected turn
  const selectedTurn = filteredTurns[selectedIdx] ?? null;

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
      // Find and select the specified turn
      const idx = filteredTurns.findIndex(t => t.turn_id === initialTurnId);
      if (idx >= 0) {
        setSelectedIdx(idx);
        setInitialTurnApplied(true);
      }
    } else if (filteredTurns.length > 0) {
      // No initial turn specified, notify parent of first turn
      const firstTurn = filteredTurns[0];
      if (firstTurn && onTurnChange) {
        onTurnChange(firstTurn.turn_id);
      }
      setInitialTurnApplied(true);
    }
  }, [data, initialTurnId, initialTurnApplied, filteredTurns, onTurnChange]);

  // Count stats - count both tool_call turns AND tool_calls embedded in assistant turns
  const stats = useMemo(() => {
    if (!data?.turns) return { total: 0, loaded: 0, toolCalls: 0, errors: 0 };
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
    const totalDepth = (data.meta?.head_depth ?? -1) + 1;
    return { total: totalDepth, loaded: data.turns.length, toolCalls, errors };
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
    if (!isOpen) return;
    setQuery('');
    // Only reset to 0 if no initialTurnId; otherwise let the initialTurn effect handle it
    if (!initialTurnId) {
      setSelectedIdx(0);
    }
    setInitialTurnApplied(false);
    setCopied(null);
    requestAnimationFrame(() => containerRef.current?.focus());
  }, [isOpen, contextId, initialTurnId]);

  // Clear copied state after delay
  useEffect(() => {
    if (!copied) return;
    const t = window.setTimeout(() => setCopied(null), 1200);
    return () => window.clearTimeout(t);
  }, [copied]);

  if (!isOpen) return null;

  const handleCopy = async (kind: 'context' | 'event') => {
    try {
      const text = kind === 'context'
        ? safeStringify(data ?? { error: 'No data' })
        : safeStringify(selectedTurn ?? {});
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
        className="h-full w-full outline-none"
        data-context-debugger
      >
        {/* Header - more compact */}
        <div className="h-12 px-4 border-b border-theme-border bg-theme-bg-secondary flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="flex items-center gap-2">
              <Layers className="w-5 h-5 text-theme-accent" />
              <span className="text-sm font-semibold text-theme-text">Context {contextId}</span>
            </div>
            {data && (
              <div className="flex items-center gap-3 text-xs text-theme-text-dim">
                <span>{stats.loaded < stats.total ? `${stats.loaded} of ${stats.total} turns` : `${stats.total} turns`}</span>
                <span>{stats.toolCalls} tool calls</span>
                {stats.errors > 0 && (
                  <span className="text-red-400">{stats.errors} errors</span>
                )}
              </div>
            )}
          </div>

          <div className="flex items-center gap-2">
            <button
              onClick={() => handleCopy('context')}
              disabled={loading || !data}
              className={cn(
                'px-2.5 py-1 text-xs rounded border transition-colors inline-flex items-center gap-1',
                copied === 'context'
                  ? 'bg-emerald-600/20 border-emerald-500/30 text-emerald-300'
                  : 'bg-theme-bg-tertiary border-theme-border text-theme-text-secondary hover:bg-theme-bg-hover disabled:opacity-50'
              )}
            >
              <Copy className="w-3 h-3" />
              {copied === 'context' ? 'Copied!' : 'Copy all'}
            </button>
            <button
              onClick={loadTurns}
              disabled={loading}
              className="px-2.5 py-1 text-xs rounded border bg-theme-bg-tertiary border-theme-border text-theme-text-secondary hover:bg-theme-bg-hover disabled:opacity-50"
            >
              {loading ? 'Loading...' : 'Refresh'}
            </button>
            <button
              onClick={onClose}
              className="p-1.5 text-theme-text-muted hover:text-theme-text-secondary hover:bg-theme-bg-tertiary rounded transition-colors"
            >
              <X className="w-5 h-5" />
            </button>
          </div>
        </div>

        {/* Body */}
        <div className="h-[calc(100%-3rem)] flex">
          {/* Left: Turn list - more compact */}
          <div className="w-80 border-r border-theme-border bg-theme-bg-secondary/40 flex flex-col">
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
              ) : filteredTurns.length === 0 ? (
                <div className="p-6 text-xs text-theme-text-dim text-center">
                  {data?.turns.length === 0 ? 'No turns.' : 'No matches.'}
                </div>
              ) : (
                <>
                {data && data.turns.length > 0 && data.turns[0].depth > 0 && (
                  <button
                    onClick={loadMore}
                    disabled={loadingMore}
                    className="w-full px-3 py-2 text-xs text-theme-accent hover:bg-theme-bg-tertiary/40 border-b border-theme-border-dim/60 transition-colors disabled:opacity-50"
                  >
                    {loadingMore ? 'Loading...' : `Load older turns (${data.turns[0].depth} remaining)`}
                  </button>
                )}
                {filteredTurns.map((turn, idx) => {
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
                })}
                </>
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
          <div className="flex-1 bg-theme-bg flex flex-col overflow-hidden relative">
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
                <div className="flex border-b border-theme-border-dim bg-theme-bg-secondary/50">
                  <button
                    onClick={() => setDetailView('turn')}
                    className={cn(
                      'px-4 py-2 text-xs uppercase tracking-wide transition-colors',
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
                      'px-4 py-2 text-xs uppercase tracking-wide transition-colors',
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
                      className={cn(
                        'mr-2 my-1 px-2 py-1 text-xs rounded border transition-colors inline-flex items-center gap-1',
                        copied === 'event'
                          ? 'bg-emerald-600/20 border-emerald-500/30 text-emerald-300'
                          : 'bg-theme-bg-tertiary border-theme-border text-theme-text-muted hover:text-theme-text-secondary'
                      )}
                    >
                      <Copy className="w-3 h-3" />
                      {copied === 'event' ? 'Copied' : 'Copy'}
                    </button>
                  )}
                </div>

                {/* Turn header (when viewing turn) */}
                {detailView === 'turn' && (
                  <div className="px-4 py-2 border-b border-theme-border-dim/50 bg-theme-bg-secondary/30 flex items-center gap-3">
                    <div className={cn(
                      'px-2 py-0.5 rounded text-xs font-medium',
                      getKindColors(detectTurnKind(selectedTurn)).badge
                    )}>
                      {getKindLabel(detectTurnKind(selectedTurn))}
                    </div>
                    <span className="text-xs text-theme-text-dim font-mono">
                      Turn #{selectedTurn.turn_id} • Depth {selectedTurn.depth}
                    </span>
                  </div>
                )}

                {/* Content area - Turn view */}
                {detailView === 'turn' && (
                  <div className="flex-1 overflow-y-auto p-4 space-y-3">
                    {/* Primary content view - uses dynamic renderer registry */}
                    <DynamicRenderer
                      data={selectedTurn.data}
                      typeId={selectedTurn.declared_type?.type_id ?? ''}
                      typeVersion={selectedTurn.declared_type?.type_version ?? 1}
                      manifest={manifest}
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
                      <div className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
                        <div className="text-theme-text-dim">Turn ID</div>
                        <div className="text-theme-text-secondary font-mono">{selectedTurn.turn_id}</div>
                        <div className="text-theme-text-dim">Parent</div>
                        <div className="text-theme-text-secondary font-mono">{selectedTurn.parent_turn_id || '(root)'}</div>
                        <div className="text-theme-text-dim">Depth</div>
                        <div className="text-theme-text-secondary">{selectedTurn.depth}</div>
                        {selectedTurn.declared_type && (
                          <>
                            <div className="text-theme-text-dim">Type</div>
                            <div className="text-theme-text-secondary font-mono text-[11px]">
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
                <div className="px-4 py-1.5 border-t border-theme-border-dim bg-theme-bg-secondary/50 text-[11px] text-theme-text-faint flex items-center gap-4">
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
