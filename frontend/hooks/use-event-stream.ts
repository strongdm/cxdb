// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { useState, useEffect, useRef, useCallback } from 'react';
import type { StoreEvent } from '@/types';

export type ConnectionState = 'disconnected' | 'connecting' | 'connected' | 'error';

export interface UseEventStreamOptions {
  enabled: boolean;
  mockMode?: boolean;
  onEvent?: (event: StoreEvent) => void;
}

export interface UseEventStreamReturn {
  connectionState: ConnectionState;
  lastEvent: StoreEvent | null;
  activityFeed: StoreEvent[];
  mockEmit: (event: StoreEvent) => void;
}

export function useEventStream(options: UseEventStreamOptions): UseEventStreamReturn {
  const { enabled, mockMode = false, onEvent } = options;
  const [connectionState, setConnectionState] = useState<ConnectionState>('disconnected');
  const [lastEvent, setLastEvent] = useState<StoreEvent | null>(null);
  const [activityFeed, setActivityFeed] = useState<StoreEvent[]>([]);
  const eventSourceRef = useRef<EventSource | null>(null);

  const mockEmit = useCallback((event: StoreEvent) => {
    setLastEvent(event);
    setActivityFeed((prev) => [event, ...prev].slice(0, 100));
    onEvent?.(event);
  }, [onEvent]);

  useEffect(() => {
    if (!enabled || mockMode) {
      if (mockMode) {
        setConnectionState('connected');
      }
      return;
    }

    let reconnectTimer: NodeJS.Timeout;

    const connect = () => {
      try {
        setConnectionState('connecting');
        const eventSource = new EventSource('/v1/events');
        eventSourceRef.current = eventSource;

        eventSource.onopen = () => {
          setConnectionState('connected');
        };

        eventSource.onerror = () => {
          setConnectionState('error');
          eventSource.close();
          // Retry after 5 seconds
          reconnectTimer = setTimeout(connect, 5000);
        };

        eventSource.onmessage = (e) => {
          try {
            const event = JSON.parse(e.data) as StoreEvent;
            setLastEvent(event);
            setActivityFeed((prev) => [event, ...prev].slice(0, 100));
            onEvent?.(event);
          } catch {
            // Ignore parse errors
          }
        };
      } catch {
        setConnectionState('error');
        reconnectTimer = setTimeout(connect, 5000);
      }
    };

    connect();

    return () => {
      clearTimeout(reconnectTimer);
      if (eventSourceRef.current) {
        eventSourceRef.current.close();
        eventSourceRef.current = null;
      }
    };
  }, [enabled, mockMode, onEvent]);

  return {
    connectionState,
    lastEvent,
    activityFeed,
    mockEmit,
  };
}

export function useMockEventGenerator(mockEmit: (event: StoreEvent) => void) {
  const intervalRef = useRef<NodeJS.Timeout | null>(null);

  const startMockEvents = useCallback((intervalMs: number = 2000) => {
    if (intervalRef.current) {
      clearInterval(intervalRef.current);
    }

    let contextCounter = 1;
    let turnCounter = 1;

    intervalRef.current = setInterval(() => {
      const eventTypes = ['context_created', 'turn_appended', 'context_metadata_updated'] as const;
      const type = eventTypes[Math.floor(Math.random() * eventTypes.length)];

      const contextId = String(contextCounter);

      if (type === 'context_created') {
        contextCounter++;
        mockEmit({
          type: 'context_created',
          data: {
            context_id: contextId,
            client_tag: `mock-client-${contextCounter}`,
            session_id: String(Math.floor(Math.random() * 1000)),
            created_at: Date.now(),
          },
        });
      } else if (type === 'turn_appended') {
        mockEmit({
          type: 'turn_appended',
          data: {
            context_id: String(Math.floor(Math.random() * contextCounter) + 1),
            turn_id: String(turnCounter++),
            parent_id: String(Math.max(1, turnCounter - 2)),
          },
        });
      } else {
        mockEmit({
          type: 'context_metadata_updated',
          data: {
            context_id: String(Math.floor(Math.random() * contextCounter) + 1),
            client_tag: `updated-tag-${Math.random()}`,
            title: `Mock Context ${Math.floor(Math.random() * 100)}`,
          },
        });
      }
    }, intervalMs);
  }, [mockEmit]);

  const stopMockEvents = useCallback(() => {
    if (intervalRef.current) {
      clearInterval(intervalRef.current);
      intervalRef.current = null;
    }
  }, []);

  useEffect(() => {
    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
      }
    };
  }, []);

  return { startMockEvents, stopMockEvents };
}
