'use client';

import { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { ContextDebugger } from '@/components/ContextDebugger';
import { ContextList } from '@/components/ContextList';
import type { ContextEntry, StoreEvent } from '@/types';
import { Database, Layers, Plus, X, AlertCircle, Check, Zap, Radio, ChevronDown, Filter } from '@/components/icons';
import { ThemeSelector } from '@/components/ThemeSelector';
import { cn, normalizeContextId } from '@/lib/utils';
import { healthCheck, fetchContexts, searchContexts } from '@/lib/api';
import { parse as parseCql, validate as validateCql, formatError as formatCqlError, buildFallbackQuery } from '@/lib/cql';
import { useEventStream, useMockEventGenerator, useUrlRouter, parseUrl, type RouteState } from '@/hooks';
import { ConnectionStatus, ActivityFeed } from '@/components/live';
import { ServerHealthDashboard } from '@/components/dashboard';

// Predefined colors for tags - using theme-aware classes
const TAG_COLORS: Record<string, { bg: string; text: string; border: string }> = {
  dotrunner: { bg: 'bg-theme-tag-dotrunner-bg', text: 'text-theme-tag-dotrunner', border: 'border-theme-tag-dotrunner/30' },
  'claude-code': { bg: 'bg-theme-tag-claude-code-bg', text: 'text-theme-tag-claude-code', border: 'border-theme-tag-claude-code/30' },
  gen: { bg: 'bg-theme-tag-gen-bg', text: 'text-theme-tag-gen', border: 'border-theme-tag-gen/30' },
  test: { bg: 'bg-theme-tag-test-bg', text: 'text-theme-tag-test', border: 'border-theme-tag-test/30' },
};

const DEFAULT_TAG_COLOR = { bg: 'bg-theme-tag-default-bg', text: 'text-theme-tag-default', border: 'border-theme-tag-default/30' };

function getTagColor(tag: string) {
  return TAG_COLORS[tag.toLowerCase()] || DEFAULT_TAG_COLOR;
}

export default function Home() {
  const [contexts, setContexts] = useState<ContextEntry[]>([]);
  const [selectedContextId, setSelectedContextId] = useState<string | null>(null);
  const [selectedTurnId, setSelectedTurnId] = useState<string | null>(null);
  const [debuggerOpen, setDebuggerOpen] = useState(false);
  const [serverStatus, setServerStatus] = useState<'checking' | 'online' | 'offline'>('checking');
  const [showActivityFeed, setShowActivityFeed] = useState(false);
  const [mockMode, setMockMode] = useState(false); // Default to live mode in production
  const [activeTags, setActiveTags] = useState<string[]>([]);
  const [selectedTag, setSelectedTag] = useState<string | null>(null);
  const [sortByTag, setSortByTag] = useState(false);
  const [tagFilterOpen, setTagFilterOpen] = useState(false);
  const [urlInitialized, setUrlInitialized] = useState(false);
  const [focusedContextIndex, setFocusedContextIndex] = useState<number>(0);

  // CQL Search state
  const [searchQuery, setSearchQuery] = useState('');
  const [searchError, setSearchError] = useState<string | null>(null);
  const [isSearching, setIsSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<{ contexts: ContextEntry[]; total: number } | null>(null);

  // Environment filter state
  const [selectedEnv, setSelectedEnv] = useState<'all' | 'prod' | 'stage' | 'dev'>('all');

  // URL routing - parse URL on mount and handle changes
  const handleRouteChange = useCallback((state: RouteState) => {
    if (state.contextId) {
      setSelectedContextId(state.contextId);
      setSelectedTurnId(state.turnId);
      setDebuggerOpen(true);
    } else {
      setSelectedContextId(null);
      setSelectedTurnId(null);
      setDebuggerOpen(false);
    }
    setUrlInitialized(true);
  }, []);

  const { navigateToContext, navigateHome, setTurn } = useUrlRouter({
    onRouteChange: handleRouteChange,
  });

  // Handle turn changes from ContextDebugger
  const handleTurnChange = useCallback((turnId: string | null) => {
    if (selectedContextId && turnId) {
      setSelectedTurnId(turnId);
      // Replace history state to avoid polluting browser history with every keystroke
      setTurn(selectedContextId, turnId, true);
    }
  }, [selectedContextId, setTurn]);

  // Stable callback for SSE events - prevents useEffect re-runs
  const handleSSEEvent = useCallback((event: StoreEvent) => {
    // Handle new contexts from SSE
    if (event.type === 'context_created') {
      const newContext: ContextEntry = {
        context_id: event.data.context_id,
        client_tag: event.data.client_tag,
        session_id: event.data.session_id,
        is_live: true,
        last_activity_at: event.data.created_at,
      };
      setContexts(prev => {
        // Add to top if not exists
        if (prev.some(c => c.context_id === event.data.context_id)) {
          return prev;
        }
        return [newContext, ...prev];
      });
    }

    // Update context metadata when extracted from first turn
    if (event.type === 'context_metadata_updated') {
      setContexts(prev =>
        prev.map(c =>
          c.context_id === event.data.context_id
            ? {
                ...c,
                client_tag: event.data.client_tag ?? c.client_tag,
                title: event.data.title ?? c.title,
                labels: event.data.labels ?? c.labels,
              }
            : c
        )
      );
    }

    // Link child context to parent context lineage
    if (event.type === 'context_linked') {
      setContexts(prev =>
        prev.map(c => {
          if (c.context_id === event.data.child_context_id) {
            return {
              ...c,
              lineage: {
                parent_context_id: event.data.parent_context_id,
                root_context_id: event.data.root_context_id,
                spawn_reason: event.data.spawn_reason,
                child_context_count: c.lineage?.child_context_count ?? 0,
                child_context_ids: c.lineage?.child_context_ids ?? [],
              },
              provenance: {
                ...(c.provenance ?? {}),
                parent_context_id: Number(event.data.parent_context_id),
                root_context_id: event.data.root_context_id
                  ? Number(event.data.root_context_id)
                  : c.provenance?.root_context_id,
                spawn_reason: event.data.spawn_reason ?? c.provenance?.spawn_reason,
              },
            };
          }

          if (c.context_id === event.data.parent_context_id) {
            const existingChildren = c.lineage?.child_context_ids ?? [];
            const childContextIds = existingChildren.includes(event.data.child_context_id)
              ? existingChildren
              : [...existingChildren, event.data.child_context_id];
            return {
              ...c,
              lineage: {
                parent_context_id: c.lineage?.parent_context_id,
                root_context_id: c.lineage?.root_context_id,
                spawn_reason: c.lineage?.spawn_reason,
                child_context_count: childContextIds.length,
                child_context_ids: childContextIds,
              },
            };
          }

          return c;
        })
      );
    }

    // Update context activity timestamp on turn append
    if (event.type === 'turn_appended') {
      setContexts(prev =>
        prev.map(c =>
          c.context_id === event.data.context_id
            ? { ...c, last_activity_at: Date.now(), is_live: true }
            : c
        )
      );
    }

    // Handle client disconnects
    if (event.type === 'client_disconnected') {
      setContexts(prev =>
        prev.map(c =>
          event.data.contexts.includes(c.context_id)
            ? { ...c, is_live: false }
            : c
        )
      );
    }
  }, []); // Empty deps - setContexts is stable

  // Event stream hook
  const {
    connectionState,
    lastEvent,
    activityFeed,
    mockEmit,
  } = useEventStream({
    enabled: serverStatus === 'online' || mockMode,
    mockMode,
    onEvent: handleSSEEvent,
  });

  // Mock event generator for demo
  const { startMockEvents, stopMockEvents } = useMockEventGenerator(mockEmit);

  // Fetch contexts helper
  const fetchContextsData = useCallback(async () => {
    try {
      const response = await fetchContexts({ limit: 1000, include_provenance: true });
      setContexts(response.contexts);
      // Derive tags from all fetched contexts (not just active sessions)
      const allTags = new Set<string>();
      for (const ctx of response.contexts) {
        if (ctx.client_tag) {
          allTags.add(ctx.client_tag);
        }
      }
      setActiveTags(Array.from(allTags).sort());
    } catch {
      // Ignore errors - contexts list just stays empty
    }
  }, []);

  // Check server health and load contexts on mount
  useEffect(() => {
    const checkServer = async () => {
      const isOnline = await healthCheck();
      setServerStatus(isOnline ? 'online' : 'offline');

      // Fetch recent contexts if server is online
      if (isOnline) {
        await fetchContextsData();
      }
    };
    checkServer();
    // Re-check every 30 seconds
    const interval = setInterval(checkServer, 30000);
    return () => clearInterval(interval);
  }, [fetchContextsData]);

  // Fallback polling when SSE is not connected
  // This provides real-time-ish updates when SSE fails
  useEffect(() => {
    if (serverStatus !== 'online') return;
    if (connectionState === 'connected') return; // SSE working, no fallback needed

    // Poll every 15 seconds when SSE is disconnected or reconnecting
    const pollInterval = setInterval(async () => {
      console.log('[Fallback] Polling for updates (SSE state:', connectionState, ')');
      await fetchContextsData();
    }, 15000);

    return () => clearInterval(pollInterval);
  }, [serverStatus, connectionState, fetchContextsData]);

  // Filter and sort contexts
  const filteredContexts = useMemo(() => {
    // If we have search results, use those instead
    if (searchResults) {
      return searchResults.contexts;
    }

    let result = contexts;

    // Filter by selected tag
    if (selectedTag) {
      result = result.filter(c => c.client_tag === selectedTag);
    }

    // Sort by tag if enabled, otherwise sort by most recent activity
    if (sortByTag) {
      result = [...result].sort((a, b) => {
        const tagA = a.client_tag || '';
        const tagB = b.client_tag || '';
        if (tagA !== tagB) {
          return tagA.localeCompare(tagB);
        }
        // Secondary sort by context_id descending
        return Number(b.context_id) - Number(a.context_id);
      });
    } else {
      // Default: sort by most recent activity (descending)
      result = [...result].sort((a, b) => {
        const timeA = a.last_activity_at ?? 0;
        const timeB = b.last_activity_at ?? 0;
        if (timeA !== timeB) {
          return timeB - timeA; // Most recent first
        }
        // Secondary sort by context_id descending for stable ordering
        return Number(b.context_id) - Number(a.context_id);
      });
    }

    return result;
  }, [contexts, selectedTag, sortByTag, searchResults]);

  // Reset focused index when filtered contexts change
  useEffect(() => {
    setFocusedContextIndex(prev =>
      Math.min(prev, Math.max(0, filteredContexts.length - 1))
    );
  }, [filteredContexts.length]);

  const handleSelectContext = useCallback((contextId: string) => {
    setSelectedContextId(contextId);
    setSelectedTurnId(null);
    setDebuggerOpen(true);
    navigateToContext(contextId);
  }, [navigateToContext]);

  const handleRemoveContext = useCallback((contextId: string) => {
    setContexts(prev => prev.filter(c => c.context_id !== contextId));
    if (selectedContextId === contextId) {
      setSelectedContextId(null);
      setSelectedTurnId(null);
      setDebuggerOpen(false);
      navigateHome();
    }
  }, [selectedContextId, navigateHome]);

  // Debounced live search - triggers automatically as user types
  const searchAbortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    const query = searchQuery.trim();

    // Clear results if query is empty
    if (!query) {
      setSearchResults(null);
      setSearchError(null);
      setIsSearching(false);
      return;
    }

    // Debounce: wait 300ms after user stops typing
    const debounceTimer = setTimeout(async () => {
      // Cancel any in-flight request
      if (searchAbortRef.current) {
        searchAbortRef.current.abort();
      }
      searchAbortRef.current = new AbortController();

      // Try to validate as CQL first
      const validation = validateCql(query);
      let effectiveQuery = query;

      // If not valid CQL, treat as keyword search across all fields
      if (!validation.ok) {
        effectiveQuery = buildFallbackQuery(query);
      }

      setIsSearching(true);
      setSearchError(null);

      try {
        const results = await searchContexts(effectiveQuery, 100);
        // Only update if this request wasn't aborted
        if (!searchAbortRef.current?.signal.aborted) {
          setSearchResults({
            contexts: results.contexts,
            total: results.total_count,
          });
        }
      } catch (err) {
        // Ignore abort errors
        if (err instanceof Error && err.name === 'AbortError') return;
        if (!searchAbortRef.current?.signal.aborted) {
          setSearchError(err instanceof Error ? err.message : 'Search failed');
          setSearchResults(null);
        }
      } finally {
        if (!searchAbortRef.current?.signal.aborted) {
          setIsSearching(false);
        }
      }
    }, 300);

    // Cleanup: cancel debounce timer on new input
    return () => {
      clearTimeout(debounceTimer);
    };
  }, [searchQuery]);

  const handleSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') {
      // Clear search
      setSearchQuery('');
      setSearchResults(null);
      setSearchError(null);
    }
  };

  const clearSearch = useCallback(() => {
    setSearchQuery('');
    setSearchResults(null);
    setSearchError(null);
    setSelectedEnv('all');
  }, []);

  // Handle env pill selection
  const handleEnvSelect = useCallback((env: 'all' | 'prod' | 'stage' | 'dev') => {
    setSelectedEnv(env);
    if (env === 'all') {
      setSearchQuery('');
      setSearchResults(null);
      setSearchError(null);
    } else {
      setSearchQuery(`label = "env=${env}"`);
    }
  }, []);

  // Keyboard shortcuts
  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return;
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      // Only handle j/k/o when debugger is closed and viewing contexts (not activity)
      if (!debuggerOpen && !showActivityFeed) {
        if (e.key === 'j' || e.key === 'ArrowDown') {
          e.preventDefault();
          setFocusedContextIndex(prev =>
            Math.min(prev + 1, filteredContexts.length - 1)
          );
          return;
        }
        if (e.key === 'k' || e.key === 'ArrowUp') {
          e.preventDefault();
          setFocusedContextIndex(prev => Math.max(prev - 1, 0));
          return;
        }
        if (e.key === 'o' || e.key === 'Enter') {
          e.preventDefault();
          const context = filteredContexts[focusedContextIndex];
          if (context) {
            handleSelectContext(context.context_id);
          }
          return;
        }
      }

      // Activity feed toggle (works anytime)
      if (e.key === 'a') {
        setShowActivityFeed(prev => !prev);
      }
    };
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [debuggerOpen, showActivityFeed, filteredContexts, focusedContextIndex, handleSelectContext]);

  return (
    <div className="h-screen flex flex-col overflow-hidden">
      {/* Header */}
      <header className="h-14 px-4 border-b border-theme-border-dim bg-theme-bg-secondary/50 flex items-center justify-between shrink-0">
        {/* Left: Logo + Title + Env Pills */}
        <div className="flex items-center gap-3">
          <div className="w-8 h-8 rounded-lg bg-theme-accent-muted border border-theme-accent/30 flex items-center justify-center">
            <Database className="w-4 h-4 text-theme-accent" />
          </div>
          <div>
            <h1 className="text-sm font-semibold text-theme-text">CXDB</h1>
            <p className="text-xs text-theme-text-dim">AI Context Store</p>
          </div>

          {/* Environment Filter Pills - vertically centered with logo */}
          <div className="flex items-center gap-1 p-0.5 bg-theme-bg-tertiary/50 rounded-lg ml-4">
            {(['all', 'prod', 'stage', 'dev'] as const).map((env) => (
              <button
                key={env}
                onClick={() => handleEnvSelect(env)}
                className={cn(
                  'px-3 py-1.5 text-xs font-medium rounded-md transition-all',
                  selectedEnv === env
                    ? env === 'prod'
                      ? 'bg-red-600/20 text-red-400 shadow-sm'
                      : env === 'stage'
                      ? 'bg-amber-600/20 text-amber-400 shadow-sm'
                      : env === 'dev'
                      ? 'bg-emerald-600/20 text-emerald-400 shadow-sm'
                      : 'bg-theme-bg-secondary text-theme-text-secondary shadow-sm'
                    : 'text-theme-text-dim hover:text-theme-text-muted hover:bg-theme-bg-tertiary/50'
                )}
              >
                {env === 'all' ? 'All' : env.charAt(0).toUpperCase() + env.slice(1)}
              </button>
            ))}
          </div>
        </div>

        {/* Right: Controls */}
        <div className="flex items-center gap-3">
          {/* Theme selector */}
          <ThemeSelector />

          {/* Mock mode toggle */}
          <button
            onClick={() => {
              if (!mockMode) {
                setMockMode(true);
              } else {
                stopMockEvents();
                setMockMode(false);
              }
            }}
            className={cn(
              'flex items-center gap-2 px-2.5 py-1 rounded-full text-xs transition-colors',
              mockMode
                ? 'bg-amber-600/20 text-amber-400 border border-amber-500/30'
                : 'bg-theme-bg-hover/50 text-theme-text-muted border border-theme-border'
            )}
          >
            <Zap className="w-3 h-3" />
            {mockMode ? 'Mock Mode' : 'Live Mode'}
          </button>

          {/* Demo button (mock mode only) */}
          {mockMode && (
            <button
              onClick={() => startMockEvents(2000)}
              className="flex items-center gap-2 px-2.5 py-1 rounded-full text-xs bg-theme-accent-muted text-theme-accent border border-theme-accent/30 hover:bg-theme-accent/30 transition-colors"
            >
              <Radio className="w-3 h-3" />
              Start Demo
            </button>
          )}

          {/* Connection status */}
          <ConnectionStatus
            state={mockMode ? 'connected' : connectionState}
            variant="badge"
          />

          {/* Server status indicator */}
          <div className={cn(
            'flex items-center gap-2 px-2.5 py-1 rounded-full text-xs',
            serverStatus === 'online' && 'bg-emerald-600/20 text-emerald-400',
            serverStatus === 'offline' && 'bg-red-600/20 text-red-400',
            serverStatus === 'checking' && 'bg-theme-bg-hover/50 text-theme-text-muted'
          )}>
            <span className={cn(
              'w-1.5 h-1.5 rounded-full',
              serverStatus === 'online' && 'bg-emerald-400',
              serverStatus === 'offline' && 'bg-red-400',
              serverStatus === 'checking' && 'bg-theme-text-muted animate-pulse'
            )} />
            {serverStatus === 'online' ? 'Server online' :
             serverStatus === 'offline' ? 'Server offline' :
             'Checking...'}
          </div>
        </div>
      </header>

      {/* Main content */}
      <div className="flex-1 flex min-h-0">
        {/* Sidebar */}
        <aside className="w-72 border-r border-theme-border-dim bg-theme-bg-secondary/30 flex flex-col min-h-0">
          {/* CQL Search */}
          <div className="p-3 border-b border-theme-border-dim">
            <label className="text-xs text-theme-text-dim mb-1.5 block">
              Search (CQL)
              <a
                href="https://github.com/microsoft/ai-cxdb/blob/main/docs/CQL_REFERENCE.md"
                target="_blank"
                rel="noopener noreferrer"
                className="ml-2 text-theme-accent hover:text-theme-accent-dim"
              >
                ?
              </a>
            </label>
            <div className="relative">
              {/* Loading spinner or decorative indicator */}
              {isSearching ? (
                <div className="absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 border-2 border-theme-text-faint border-t-theme-accent rounded-full animate-spin" />
              ) : (
                <div className="absolute left-2.5 top-1/2 -translate-y-1/2 w-1.5 h-1.5 rounded-full bg-theme-text-faint" />
              )}
              <input
                type="text"
                value={searchQuery}
                onChange={(e) => {
                  setSearchQuery(e.target.value);
                  if (searchError) setSearchError(null);
                }}
                onKeyDown={handleSearchKeyDown}
                placeholder='Search: tag = "amplifier" or just type keywords...'
                className={cn(
                  'w-full pl-8 pr-8 py-2 bg-theme-bg-secondary border rounded-lg text-sm text-theme-text-secondary placeholder:text-theme-text-faint focus:outline-none focus:ring-2',
                  searchError
                    ? 'border-red-500/50 focus:ring-red-500/30'
                    : 'border-theme-border focus:ring-theme-accent/30'
                )}
              />
              {searchQuery && (
                <button
                  onClick={clearSearch}
                  className="absolute right-2.5 top-1/2 -translate-y-1/2 text-theme-text-dim hover:text-theme-text-secondary"
                >
                  <X className="w-4 h-4" />
                </button>
              )}
            </div>
            {searchError && (
              <div className="mt-2 text-xs text-red-400 flex items-start gap-1.5">
                <AlertCircle className="w-3.5 h-3.5 mt-0.5 flex-shrink-0" />
                <span>{searchError}</span>
              </div>
            )}
            {searchResults && (
              <div className="mt-2 flex items-center justify-between text-xs">
                <span className="text-theme-text-muted">
                  Found <span className="text-theme-accent font-medium">{searchResults.total}</span> contexts
                </span>
                <button
                  onClick={clearSearch}
                  className="text-theme-text-dim hover:text-theme-text-secondary"
                >
                  Clear
                </button>
              </div>
            )}
          </div>

          {/* Tab toggle: Contexts / Activity */}
          <div className="flex border-b border-theme-border-dim">
            <button
              onClick={() => setShowActivityFeed(false)}
              className={cn(
                'flex-1 px-3 py-2 text-xs uppercase tracking-wide transition-colors',
                !showActivityFeed
                  ? 'text-theme-accent border-b-2 border-theme-accent bg-theme-accent-muted'
                  : 'text-theme-text-dim hover:text-theme-text-muted'
              )}
            >
              Contexts
            </button>
            <button
              onClick={() => setShowActivityFeed(true)}
              className={cn(
                'flex-1 px-3 py-2 text-xs uppercase tracking-wide transition-colors',
                showActivityFeed
                  ? 'text-theme-accent border-b-2 border-theme-accent bg-theme-accent-muted'
                  : 'text-theme-text-dim hover:text-theme-text-muted'
              )}
            >
              Activity
              {activityFeed.length > 0 && (
                <span className="ml-1.5 px-1.5 py-0.5 bg-theme-accent-muted text-theme-accent rounded-full text-[10px]">
                  {activityFeed.length > 99 ? '99+' : activityFeed.length}
                </span>
              )}
            </button>
          </div>

          {/* Tag filter (Contexts tab only) */}
          {!showActivityFeed && activeTags.length > 0 && (
            <div className="p-2 border-b border-theme-border-dim/60 flex items-center gap-2">
              <div className="relative flex-1">
                <button
                  onClick={() => setTagFilterOpen(!tagFilterOpen)}
                  className={cn(
                    'w-full flex items-center justify-between gap-2 px-2.5 py-1.5 rounded-md text-xs transition-colors',
                    selectedTag
                      ? 'bg-theme-bg-tertiary border border-theme-border text-theme-text-secondary'
                      : 'bg-theme-bg-tertiary/50 border border-theme-border/50 text-theme-text-muted hover:text-theme-text-secondary'
                  )}
                >
                  <div className="flex items-center gap-2">
                    <Filter className="w-3 h-3" />
                    {selectedTag ? (
                      <span className={cn(
                        'px-1.5 py-0.5 rounded text-[10px] font-medium',
                        getTagColor(selectedTag).bg,
                        getTagColor(selectedTag).text
                      )}>
                        {selectedTag}
                      </span>
                    ) : (
                      <span>All tags</span>
                    )}
                  </div>
                  <ChevronDown className={cn(
                    'w-3 h-3 transition-transform',
                    tagFilterOpen && 'rotate-180'
                  )} />
                </button>

                {tagFilterOpen && (
                  <div className="absolute top-full left-0 right-0 mt-1 bg-theme-bg-secondary border border-theme-border rounded-md shadow-lg z-10 py-1">
                    <button
                      onClick={() => { setSelectedTag(null); setTagFilterOpen(false); }}
                      className={cn(
                        'w-full text-left px-3 py-1.5 text-xs hover:bg-theme-bg-tertiary transition-colors',
                        !selectedTag ? 'text-theme-accent' : 'text-theme-text-muted'
                      )}
                    >
                      All tags
                    </button>
                    {activeTags.map(tag => {
                      const colors = getTagColor(tag);
                      return (
                        <button
                          key={tag}
                          onClick={() => { setSelectedTag(tag); setTagFilterOpen(false); }}
                          className={cn(
                            'w-full text-left px-3 py-1.5 text-xs hover:bg-theme-bg-tertiary transition-colors flex items-center gap-2',
                            selectedTag === tag ? 'text-theme-accent' : 'text-theme-text-muted'
                          )}
                        >
                          <span className={cn(
                            'px-1.5 py-0.5 rounded text-[10px] font-medium',
                            colors.bg,
                            colors.text
                          )}>
                            {tag}
                          </span>
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>

              {/* Sort by tag toggle */}
              <button
                onClick={() => setSortByTag(!sortByTag)}
                className={cn(
                  'px-2 py-1.5 rounded-md text-xs transition-colors',
                  sortByTag
                    ? 'bg-theme-accent-muted text-theme-accent border border-theme-accent/30'
                    : 'bg-theme-bg-tertiary/50 text-theme-text-dim border border-theme-border/50 hover:text-theme-text-muted'
                )}
                title="Group by tag"
              >
                A-Z
              </button>
            </div>
          )}

          {/* Content area - scrollable */}
          <div className="flex-1 min-h-0 overflow-y-auto">
            {showActivityFeed ? (
              <ActivityFeed
                items={activityFeed}
                onContextClick={handleSelectContext}
                className="p-2"
              />
            ) : (
              <>
                {filteredContexts.length === 0 ? (
                  <div className="p-6 text-center">
                    <Layers className="w-10 h-10 mx-auto mb-3 text-theme-text-faint opacity-50" />
                    <p className="text-sm text-theme-text-dim">
                      {selectedTag ? `No contexts with tag "${selectedTag}"` : 'No contexts yet.'}
                    </p>
                    <p className="text-xs text-theme-text-faint mt-1">
                      {selectedTag ? (
                        <button
                          onClick={() => setSelectedTag(null)}
                          className="text-theme-accent hover:underline"
                        >
                          Clear filter
                        </button>
                      ) : mockMode ? (
                        'Click "Start Demo" to see live events'
                      ) : (
                        'Enter a context ID above.'
                      )}
                    </p>
                  </div>
                ) : (
                  <ContextList
                    contexts={filteredContexts}
                    selectedId={selectedContextId || undefined}
                    focusedIndex={focusedContextIndex}
                    onSelect={handleSelectContext}
                    lastEvent={lastEvent}
                  />
                )}
              </>
            )}
          </div>
        </aside>

        {/* Main area */}
        <main className="flex-1 flex items-start justify-center bg-theme-bg pt-8 overflow-y-auto">
          {!debuggerOpen && (
            <ServerHealthDashboard
              enabled={serverStatus === 'online' || mockMode}
              mockMode={mockMode}
            />
          )}
        </main>
      </div>

      {/* Keyboard hints footer */}
      <footer className="h-8 px-4 border-t border-theme-border-dim bg-theme-bg-secondary/30 flex items-center text-xs text-theme-text-faint">
        <span><kbd>j</kbd>/<kbd>k</kbd> Navigate</span>
        <span className="mx-3 text-theme-border">|</span>
        <span><kbd>o</kbd> Open context</span>
        <span className="mx-3 text-theme-border">|</span>
        <span><kbd>a</kbd> Activity feed</span>
      </footer>

      {/* Context Debugger Modal */}
      {selectedContextId && (
        <ContextDebugger
          contextId={selectedContextId}
          isOpen={debuggerOpen}
          onClose={() => {
            setDebuggerOpen(false);
            navigateHome();
          }}
          lastEvent={lastEvent}
          initialTurnId={selectedTurnId}
          onTurnChange={handleTurnChange}
          onNavigateToContext={handleSelectContext}
        />
      )}
    </div>
  );
}
