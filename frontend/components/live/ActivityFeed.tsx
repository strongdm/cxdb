'use client';

import { useMemo } from 'react';
import { AlertCircle, Folder, MessageSquare, RefreshCw, User, UserMinus } from 'lucide-react';
import type { ActivityItem, StoreEvent } from '@/types';
import { LiveTimestamp } from './LiveTimestamp';
import { PresenceIndicator } from './PresenceIndicator';
import { cn } from '@/lib/utils';

interface ActivityFeedProps {
  items: ActivityItem[];
  maxItems?: number;
  onContextClick?: (contextId: string) => void;
  className?: string;
}

export function ActivityFeed({
  items,
  maxItems = 50,
  onContextClick,
  className,
}: ActivityFeedProps) {
  const displayItems = useMemo(() => items.slice(0, maxItems), [items, maxItems]);

  if (displayItems.length === 0) {
    return (
      <div className={cn('flex flex-col items-center justify-center py-8 text-theme-text-dim', className)}>
        <MessageSquare size={24} className="mb-2 opacity-50" />
        <p className="text-sm">No activity yet</p>
        <p className="text-xs text-theme-text-faint">Events will appear here in real-time</p>
      </div>
    );
  }

  return (
    <div className={cn('flex flex-col gap-1', className)}>
      {displayItems.map((item, index) => (
        <ActivityItemRow
          key={item.id}
          item={item}
          isNew={index === 0}
          onContextClick={onContextClick}
        />
      ))}
    </div>
  );
}

interface ActivityItemRowProps {
  item: ActivityItem;
  isNew?: boolean;
  onContextClick?: (contextId: string) => void;
}

function ActivityItemRow({ item, isNew, onContextClick }: ActivityItemRowProps) {
  const { icon, label, contextId, clientTag, isConnect } = useMemo(
    () => getEventDisplay(item.event),
    [item.event]
  );

  return (
    <div
      className={cn(
        'flex items-start gap-2 px-2 py-1.5 rounded-md',
        'transition-all duration-300',
        isNew && 'animate-slide-in-left bg-theme-accent-muted',
        !isNew && 'hover:bg-theme-bg-tertiary/50',
        contextId && onContextClick && 'cursor-pointer'
      )}
      onClick={() => contextId && onContextClick?.(contextId)}
    >
      <div className="flex-shrink-0 mt-0.5">
        {isConnect !== undefined ? (
          <PresenceIndicator
            state={isConnect ? 'active' : 'disconnected'}
            size="sm"
          />
        ) : (
          <span className="text-theme-text-dim">{icon}</span>
        )}
      </div>

      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          {clientTag && (
            <span className="text-xs font-medium text-theme-text-muted truncate">
              {clientTag}
            </span>
          )}
          {contextId && (
            <span className="text-xs font-mono text-theme-text-dim">
              ctx:{contextId}
            </span>
          )}
        </div>
        <p className="text-xs text-theme-text-dim truncate">{label}</p>
      </div>

      <LiveTimestamp
        timestamp={item.timestamp}
        className="flex-shrink-0"
      />
    </div>
  );
}

function getEventDisplay(event: StoreEvent): {
  icon: React.ReactNode;
  label: string;
  contextId?: string;
  clientTag?: string;
  isConnect?: boolean;
} {
  switch (event.type) {
    case 'context_created':
      return {
        icon: <Folder size={12} />,
        label: 'Context created',
        contextId: event.data.context_id,
        clientTag: event.data.client_tag,
      };

    case 'turn_appended':
      return {
        icon: <MessageSquare size={12} />,
        label: `Turn ${event.data.turn_id} appended`,
        contextId: event.data.context_id,
      };

    case 'client_connected':
      return {
        icon: <User size={12} />,
        label: 'Session connected',
        clientTag: event.data.client_tag,
        isConnect: true,
      };

    case 'client_disconnected':
      return {
        icon: <UserMinus size={12} />,
        label: `Disconnected${event.data.contexts.length ? ` (${event.data.contexts.length} contexts)` : ''}`,
        clientTag: event.data.client_tag,
        isConnect: false,
      };

    case 'context_metadata_updated':
      return {
        icon: <RefreshCw size={12} />,
        label: event.data.title ? `Updated: "${event.data.title}"` : 'Metadata updated',
        contextId: event.data.context_id,
        clientTag: event.data.client_tag,
      };

    case 'error_occurred':
      return {
        icon: <AlertCircle size={12} />,
        label: `${event.data.kind} ${event.data.status_code}: ${event.data.message}`,
      };

    case 'context_linked':
      return {
        icon: <Folder size={12} />,
        label: `Linked to parent ${event.data.parent_context_id}`,
        contextId: event.data.child_context_id,
      };
  }
}

// Compact activity indicator (for headers)
interface ActivitySparkProps {
  recentCount: number;
  className?: string;
}

export function ActivitySpark({ recentCount, className }: ActivitySparkProps) {
  if (recentCount === 0) return null;

  return (
    <span
      className={cn(
        'inline-flex items-center justify-center',
        'w-4 h-4 rounded-full',
        'bg-theme-accent text-white',
        'text-[10px] font-bold',
        'animate-pulse-once',
        className
      )}
    >
      {recentCount > 9 ? '9+' : recentCount}
    </span>
  );
}
