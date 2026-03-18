'use client';

import { memo, useMemo, useState, useEffect, useCallback, useRef } from 'react';
import type { ContextEntry, StoreEvent } from '@/types';
import { cn, formatTimestamp } from '@/lib/utils';
import { Database, GitBranch, GitFork, ChevronRight, Folder, User, Tag } from './icons';
import { PresenceIndicator, LiveTimestamp } from './live';
import type { PresenceState } from './live';
import { getSourceStyle } from '@/types/provenance';

// Tag color mapping - uses theme-aware classes
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

function getContextBadgeLabel(context: ContextEntry): string | null {
  const provenance = context.provenance;
  const username =
    provenance?.on_behalf_of ||
    provenance?.process_owner ||
    provenance?.on_behalf_of_email ||
    null;
  const hostname = provenance?.host_name || null;

  if (username && hostname) {
    return `${username}@${hostname}`;
  }
  if (username) {
    return username;
  }
  if (hostname) {
    return hostname;
  }
  return context.client_tag || null;
}

function basename(path?: string): string | null {
  if (!path) return null;
  const trimmed = path.replace(/\/+$/, '');
  const segments = trimmed.split('/').filter(Boolean);
  return segments.length > 0 ? segments[segments.length - 1] : null;
}

function getContextTitleLabel(context: ContextEntry): string | null {
  const provider = context.client_tag?.split('/').pop() || null;
  const worktreeName = basename(context.provenance?.env?.PWD);
  const timestampSource = context.provenance?.captured_at ?? context.created_at_unix_ms;
  const timestamp = timestampSource ? formatTimestamp(timestampSource) : null;

  if (provider && worktreeName && timestamp) {
    return `${provider}: ${worktreeName} ${timestamp}`;
  }

  return context.title || null;
}

interface ContextListProps {
  contexts: ContextEntry[];
  selectedId?: string;
  focusedIndex?: number;
  onSelect: (contextId: string) => void;
  lastEvent?: StoreEvent | null;
}

interface AnimationState {
  [contextId: string]: {
    isNew?: boolean;
    isUpdated?: boolean;
  };
}

const ContextListItem = memo(function ContextListItem({
  context,
  isSelected,
  isFocused,
  onSelect,
  isNew,
  isUpdated,
  itemRef,
}: {
  context: ContextEntry;
  isSelected: boolean;
  isFocused?: boolean;
  onSelect: () => void;
  isNew?: boolean;
  isUpdated?: boolean;
  itemRef?: React.RefObject<HTMLButtonElement>;
}) {
  const presenceState: PresenceState = useMemo(() => {
    if (context.is_live) {
      // Check for recent activity
      if (context.last_activity_at && Date.now() - context.last_activity_at < 5000) {
        return 'active';
      }
      return 'live';
    }
    if (context.session_id) {
      return 'idle';
    }
    return 'disconnected';
  }, [context.is_live, context.session_id, context.last_activity_at]);

  // Extract provenance info for display
  const provenance = context.provenance;
  const hasParent = !!(provenance?.parent_context_id);
  const onBehalfOf = provenance?.on_behalf_of || provenance?.on_behalf_of_email;
  const sourceStyle = provenance?.on_behalf_of_source ? getSourceStyle(provenance.on_behalf_of_source) : null;
  const badgeLabel = getContextBadgeLabel(context);
  const titleLabel = getContextTitleLabel(context);

  return (
    <button
      ref={itemRef}
      onClick={onSelect}
      data-context-id={context.context_id}
      className={cn(
        'w-full text-left px-3 py-2.5 border-b border-theme-border-dim/60 transition-all group',
        isSelected
          ? 'bg-theme-accent-muted border-l-2 border-l-theme-accent'
          : isFocused
            ? 'bg-theme-bg-tertiary/50 border-l-2 border-l-theme-text-dim ring-1 ring-inset ring-theme-text-faint'
            : 'hover:bg-theme-bg-tertiary/40 border-l-2 border-l-transparent',
        // Animation classes
        isNew && 'animate-slide-in-left',
        isUpdated && !isSelected && 'animate-activity-flash'
      )}
    >
      {/* Title row (if available) */}
      {titleLabel && (
        <div className="flex items-center gap-1.5 mb-1">
          <span className={cn(
            'text-sm font-medium truncate',
            isSelected ? 'text-theme-text' : 'text-theme-text-secondary'
          )}>
            {titleLabel}
          </span>
        </div>
      )}

      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 min-w-0">
          {/* Presence indicator */}
          <PresenceIndicator
            state={presenceState}
            size="md"
            className="shrink-0"
          />
          {/* Lineage indicator (fork icon if has parent) */}
          {hasParent && (
            <span title={`Forked from #${provenance?.parent_context_id}`}>
              <GitFork className="w-3 h-3 text-emerald-400 shrink-0" />
            </span>
          )}
          <span className={cn(
            'text-sm font-mono truncate',
            isSelected ? 'text-theme-text' : 'text-theme-text-secondary'
          )}>
            {context.context_id}
          </span>
          {/* Context provenance badge */}
          {badgeLabel && (
            <span className={cn(
              'px-1.5 py-0.5 rounded text-[10px] font-medium truncate',
              getTagColor(context.client_tag || 'default').bg,
              getTagColor(context.client_tag || 'default').text
            )}
            title={badgeLabel}
            >
              {badgeLabel}
            </span>
          )}
          {/* Filesystem snapshot indicator */}
          {context.has_fs_snapshot && (
            <span title="Has filesystem snapshot">
              <Folder className="w-3.5 h-3.5 text-amber-400 shrink-0" />
            </span>
          )}
        </div>
        <div className="flex items-center gap-2 shrink-0">
          {/* Activity timestamp - use last_activity_at for active, created_at_unix_ms as fallback */}
          {(context.last_activity_at || context.created_at_unix_ms) && (
            <LiveTimestamp
              timestamp={context.last_activity_at ?? context.created_at_unix_ms!}
              showSparkle={!!context.last_activity_at}
              sparkleThreshold={60000}
            />
          )}
          <ChevronRight className={cn(
            'w-4 h-4 transition-transform',
            isSelected ? 'text-theme-accent' : 'text-theme-text-faint group-hover:text-theme-text-muted'
          )} />
        </div>
      </div>

      {/* On-behalf-of row */}
      {onBehalfOf && (
        <div className="mt-1 flex items-center gap-1.5 text-xs text-theme-text-muted">
          {sourceStyle && (
            <span className={cn('text-sm', sourceStyle.color)} title={sourceStyle.label}>
              {sourceStyle.icon}
            </span>
          )}
          <User className="w-3 h-3 text-theme-text-dim" />
          <span className="truncate">{onBehalfOf}</span>
        </div>
      )}

      {/* Labels row */}
      {context.labels && context.labels.length > 0 && (
        <div className="mt-1 flex items-center gap-1 flex-wrap">
          {context.labels.slice(0, 3).map((label) => (
            <span
              key={label}
              className="inline-flex items-center gap-0.5 px-1 py-0.5 bg-theme-bg-tertiary/60 rounded text-[10px] text-theme-text-muted"
            >
              <Tag className="w-2.5 h-2.5" />
              {label}
            </span>
          ))}
          {context.labels.length > 3 && (
            <span className="text-[10px] text-theme-text-dim">+{context.labels.length - 3}</span>
          )}
        </div>
      )}

      {/* Secondary info row */}
      {(context.head_depth !== undefined || context.head_turn_id) && (
        <div className="mt-1 flex items-center gap-3 text-xs text-theme-text-dim">
          {context.head_depth !== undefined && (
            <span className="inline-flex items-center gap-1">
              <GitBranch className="w-3 h-3" />
              depth {context.head_depth}
            </span>
          )}
          {context.head_turn_id && (
            <span className="font-mono truncate">
              head: {context.head_turn_id}
            </span>
          )}
        </div>
      )}
    </button>
  );
});

export function ContextList({ contexts, selectedId, focusedIndex = 0, onSelect, lastEvent }: ContextListProps) {
  const [animationState, setAnimationState] = useState<AnimationState>({});
  const focusedRef = useRef<HTMLButtonElement | null>(null);

  // Scroll focused item into view
  useEffect(() => {
    if (focusedRef.current) {
      focusedRef.current.scrollIntoView({
        behavior: 'smooth',
        block: 'nearest',
      });
    }
  }, [focusedIndex]);

  // Track new contexts and updates for animations
  useEffect(() => {
    if (!lastEvent) return;

    if (lastEvent.type === 'context_created') {
      const contextId = lastEvent.data.context_id;
      setAnimationState(prev => ({
        ...prev,
        [contextId]: { isNew: true },
      }));

      // Clear animation state after animation completes
      const timer = setTimeout(() => {
        setAnimationState(prev => {
          const next = { ...prev };
          if (next[contextId]) {
            next[contextId] = { ...next[contextId], isNew: false };
          }
          return next;
        });
      }, 300);

      return () => clearTimeout(timer);
    }

    if (lastEvent.type === 'turn_appended') {
      const contextId = lastEvent.data.context_id;
      // Only flash if not currently selected
      if (contextId !== selectedId) {
        setAnimationState(prev => ({
          ...prev,
          [contextId]: { ...prev[contextId], isUpdated: true },
        }));

        // Clear animation state after animation completes
        const timer = setTimeout(() => {
          setAnimationState(prev => {
            const next = { ...prev };
            if (next[contextId]) {
              next[contextId] = { ...next[contextId], isUpdated: false };
            }
            return next;
          });
        }, 1000);

        return () => clearTimeout(timer);
      }
    }
  }, [lastEvent, selectedId]);

  if (contexts.length === 0) {
    return (
      <div className="p-6 text-center">
        <Database className="w-10 h-10 mx-auto mb-3 text-theme-text-faint opacity-50" />
        <p className="text-sm text-theme-text-dim">No contexts available.</p>
        <p className="text-xs text-theme-text-faint mt-1">Enter a context ID above to get started.</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col">
      {contexts.map((context, index) => (
        <ContextListItem
          key={context.context_id}
          context={context}
          isSelected={selectedId === context.context_id}
          isFocused={index === focusedIndex}
          onSelect={() => onSelect(context.context_id)}
          isNew={animationState[context.context_id]?.isNew}
          isUpdated={animationState[context.context_id]?.isUpdated}
          itemRef={index === focusedIndex ? focusedRef as React.RefObject<HTMLButtonElement> : undefined}
        />
      ))}
    </div>
  );
}

export default ContextList;
