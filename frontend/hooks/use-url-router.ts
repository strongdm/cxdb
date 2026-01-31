// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { useEffect, useCallback } from 'react';

export interface RouteState {
  contextId: string | null;
  turnId: string | null;
}

export interface UseUrlRouterOptions {
  onRouteChange: (state: RouteState) => void;
}

export function parseUrl(): RouteState {
  if (typeof window === 'undefined') {
    return { contextId: null, turnId: null };
  }

  const params = new URLSearchParams(window.location.search);
  return {
    contextId: params.get('context') || null,
    turnId: params.get('turn') || null,
  };
}

export function useUrlRouter(options: UseUrlRouterOptions) {
  const { onRouteChange } = options;

  useEffect(() => {
    // Parse initial URL
    const initial = parseUrl();
    onRouteChange(initial);

    // Listen for popstate (browser back/forward)
    const handlePopState = () => {
      const state = parseUrl();
      onRouteChange(state);
    };

    window.addEventListener('popstate', handlePopState);
    return () => window.removeEventListener('popstate', handlePopState);
  }, [onRouteChange]);

  const navigateToContext = useCallback((contextId: string, turnId?: string | null) => {
    const url = new URL(window.location.href);
    url.searchParams.set('context', contextId);
    if (turnId) {
      url.searchParams.set('turn', turnId);
    } else {
      url.searchParams.delete('turn');
    }
    window.history.pushState({}, '', url.toString());
  }, []);

  const navigateHome = useCallback(() => {
    const url = new URL(window.location.href);
    url.searchParams.delete('context');
    url.searchParams.delete('turn');
    window.history.pushState({}, '', url.toString());
  }, []);

  const setTurn = useCallback((contextId: string, turnId: string, replace: boolean = false) => {
    const url = new URL(window.location.href);
    url.searchParams.set('context', contextId);
    url.searchParams.set('turn', turnId);

    if (replace) {
      window.history.replaceState({}, '', url.toString());
    } else {
      window.history.pushState({}, '', url.toString());
    }
  }, []);

  return {
    navigateToContext,
    navigateHome,
    setTurn,
  };
}
