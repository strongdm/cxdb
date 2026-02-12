'use client';

import { useState, useEffect, useCallback, useRef } from 'react';
import type {
  StoreEvent,
  ActivityItem,
  ConnectionState,
  ContextCreatedEvent,
  ContextMetadataUpdatedEvent,
  ContextLinkedEvent,
  TurnAppendedEvent,
  ClientConnectedEvent,
  ClientDisconnectedEvent,
  ErrorOccurredEvent,
} from '@/types';

const API_BASE = process.env.NEXT_PUBLIC_API_BASE || '/v1';

interface UseEventStreamOptions {
  contextId?: string; // Filter to specific context
  enabled?: boolean;
  mockMode?: boolean; // For local testing without backend SSE
  onEvent?: (event: StoreEvent) => void;
}

interface UseEventStreamResult {
  connectionState: ConnectionState;
  lastEvent: StoreEvent | null;
  activityFeed: ActivityItem[];
  error: Error | null;
  // Mock controls (only available in mock mode)
  mockEmit?: (event: StoreEvent) => void;
}

// Generate unique IDs for activity items
let activityIdCounter = 0;
const generateActivityId = () => `activity-${++activityIdCounter}-${Date.now()}`;

export function useEventStream(options: UseEventStreamOptions = {}): UseEventStreamResult {
  const { contextId, enabled = true, mockMode = false, onEvent } = options;

  const [connectionState, setConnectionState] = useState<ConnectionState>('disconnected');
  const [lastEvent, setLastEvent] = useState<StoreEvent | null>(null);
  const [activityFeed, setActivityFeed] = useState<ActivityItem[]>([]);
  const [error, setError] = useState<Error | null>(null);

  const eventSourceRef = useRef<EventSource | null>(null);
  const reconnectTimeoutRef = useRef<NodeJS.Timeout | null>(null);
  const reconnectAttemptsRef = useRef(0);

  const maxActivityItems = 100;

  const addActivityItem = useCallback((event: StoreEvent) => {
    const item: ActivityItem = {
      id: generateActivityId(),
      timestamp: Date.now(),
      event,
    };
    setActivityFeed(prev => {
      const next = [item, ...prev];
      return next.slice(0, maxActivityItems);
    });
  }, []);

  const handleEvent = useCallback(
    (event: StoreEvent) => {
      // Filter by context if specified
      if (contextId) {
        if (event.type === 'turn_appended' && event.data.context_id !== contextId) {
          return;
        }
        if (event.type === 'context_created' && event.data.context_id !== contextId) {
          return;
        }
        if (event.type === 'context_metadata_updated' && event.data.context_id !== contextId) {
          return;
        }
        if (event.type === 'context_linked') {
          if (
            event.data.child_context_id !== contextId &&
            event.data.parent_context_id !== contextId
          ) {
            return;
          }
        }
      }

      setLastEvent(event);
      addActivityItem(event);
      onEvent?.(event);
    },
    [contextId, addActivityItem, onEvent]
  );

  // Mock emit function for testing
  const mockEmit = useCallback(
    (event: StoreEvent) => {
      if (!mockMode) return;
      handleEvent(event);
    },
    [mockMode, handleEvent]
  );

  // Real SSE connection (when backend supports it)
  useEffect(() => {
    if (!enabled || mockMode) {
      setConnectionState(mockMode ? 'connected' : 'disconnected');
      return;
    }

    const connect = () => {
      setConnectionState('connecting');
      setError(null);

      const url = contextId
        ? `${API_BASE}/contexts/${contextId}/events`
        : `${API_BASE}/events`;

      try {
        const eventSource = new EventSource(url);
        eventSourceRef.current = eventSource;

        eventSource.onopen = () => {
          console.log('[SSE] Connection opened');
          setConnectionState('connected');
          reconnectAttemptsRef.current = 0;
        };

        eventSource.onerror = (e) => {
          console.error('[SSE] Connection error:', e, 'readyState:', eventSource.readyState);
          setConnectionState('reconnecting');
          eventSource.close();

          // Exponential backoff for reconnection
          const delay = Math.min(1000 * Math.pow(2, reconnectAttemptsRef.current), 30000);
          reconnectAttemptsRef.current++;
          console.log('[SSE] Reconnecting in', delay, 'ms');

          reconnectTimeoutRef.current = setTimeout(connect, delay);
        };

        // Handle the connected event from server
        eventSource.addEventListener('connected', (e: MessageEvent) => {
          console.log('[SSE] Received connected event:', e.data);
        });

        // Handle specific event types
        eventSource.addEventListener('context_created', (e: MessageEvent) => {
          try {
            const data: ContextCreatedEvent = JSON.parse(e.data);
            handleEvent({ type: 'context_created', data });
          } catch (err) {
            console.error('Failed to parse context_created event:', err);
          }
        });

        eventSource.addEventListener('context_metadata_updated', (e: MessageEvent) => {
          try {
            const data: ContextMetadataUpdatedEvent = JSON.parse(e.data);
            handleEvent({ type: 'context_metadata_updated', data });
          } catch (err) {
            console.error('Failed to parse context_metadata_updated event:', err);
          }
        });

        eventSource.addEventListener('context_linked', (e: MessageEvent) => {
          try {
            const data: ContextLinkedEvent = JSON.parse(e.data);
            handleEvent({ type: 'context_linked', data });
          } catch (err) {
            console.error('Failed to parse context_linked event:', err);
          }
        });

        eventSource.addEventListener('turn_appended', (e: MessageEvent) => {
          try {
            const data: TurnAppendedEvent = JSON.parse(e.data);
            handleEvent({ type: 'turn_appended', data });
          } catch (err) {
            console.error('Failed to parse turn_appended event:', err);
          }
        });

        eventSource.addEventListener('client_connected', (e: MessageEvent) => {
          try {
            const data: ClientConnectedEvent = JSON.parse(e.data);
            handleEvent({ type: 'client_connected', data });
          } catch (err) {
            console.error('Failed to parse client_connected event:', err);
          }
        });

        eventSource.addEventListener('client_disconnected', (e: MessageEvent) => {
          try {
            const data: ClientDisconnectedEvent = JSON.parse(e.data);
            handleEvent({ type: 'client_disconnected', data });
          } catch (err) {
            console.error('Failed to parse client_disconnected event:', err);
          }
        });

        eventSource.addEventListener('error_occurred', (e: MessageEvent) => {
          try {
            const data: ErrorOccurredEvent = JSON.parse(e.data);
            handleEvent({ type: 'error_occurred', data });
          } catch (err) {
            console.error('Failed to parse error_occurred event:', err);
          }
        });
      } catch (err) {
        setError(err instanceof Error ? err : new Error('Failed to connect to event stream'));
        setConnectionState('disconnected');
      }
    };

    connect();

    return () => {
      eventSourceRef.current?.close();
      if (reconnectTimeoutRef.current) {
        clearTimeout(reconnectTimeoutRef.current);
      }
    };
  }, [enabled, mockMode, contextId, handleEvent]);

  return {
    connectionState,
    lastEvent,
    activityFeed,
    error,
    ...(mockMode && { mockEmit }),
  };
}

// Hook for mock data generation (for demo/testing)
export function useMockEventGenerator(mockEmit?: (event: StoreEvent) => void) {
  const intervalRef = useRef<NodeJS.Timeout | null>(null);

  const startMockEvents = useCallback(
    (intervalMs = 3000) => {
      if (!mockEmit || intervalRef.current) return;

      let contextCounter = 100;
      let turnCounter = 1000;
      const clientTags = ['claude-code', 'dotrunner', 'test-harness', 'aider'];
      const activeContexts: string[] = [];

      intervalRef.current = setInterval(() => {
        const rand = Math.random();

        if (rand < 0.2 && activeContexts.length < 5) {
          // Create new context
          const contextId = String(contextCounter++);
          const clientTag = clientTags[Math.floor(Math.random() * clientTags.length)];
          activeContexts.push(contextId);

          mockEmit({
            type: 'context_created',
            data: {
              context_id: contextId,
              session_id: String(Math.floor(Math.random() * 100)),
              client_tag: clientTag,
              created_at: Date.now(),
            },
          });
        } else if (rand < 0.8 && activeContexts.length > 0) {
          // Append turn to random context
          const contextId = activeContexts[Math.floor(Math.random() * activeContexts.length)];

          mockEmit({
            type: 'turn_appended',
            data: {
              context_id: contextId,
              turn_id: String(turnCounter++),
              parent_turn_id: String(turnCounter - 2),
              depth: Math.floor(Math.random() * 10),
              declared_type_id: 'com.anthropic.conversation.Item',
              declared_type_version: 1,
            },
          });
        } else if (rand < 0.9) {
          // Client connected
          const clientTag = clientTags[Math.floor(Math.random() * clientTags.length)];
          mockEmit({
            type: 'client_connected',
            data: {
              session_id: String(Math.floor(Math.random() * 100)),
              client_tag: clientTag,
            },
          });
        } else {
          // Client disconnected
          const clientTag = clientTags[Math.floor(Math.random() * clientTags.length)];
          const disconnectedContext = activeContexts.length > 0
            ? activeContexts.splice(Math.floor(Math.random() * activeContexts.length), 1)
            : [];

          mockEmit({
            type: 'client_disconnected',
            data: {
              session_id: String(Math.floor(Math.random() * 100)),
              client_tag: clientTag,
              contexts: disconnectedContext,
            },
          });
        }
      }, intervalMs);
    },
    [mockEmit]
  );

  const stopMockEvents = useCallback(() => {
    if (intervalRef.current) {
      clearInterval(intervalRef.current);
      intervalRef.current = null;
    }
  }, []);

  useEffect(() => {
    return () => stopMockEvents();
  }, [stopMockEvents]);

  return { startMockEvents, stopMockEvents };
}
