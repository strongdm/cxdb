// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { RefreshCw, WifiOff } from '@/components/icons';
import { cn } from '@/lib/utils';
import { useMetrics, useRelativeTime } from '@/hooks';
import { CapacityGauge } from './CapacityGauge';
import { ObjectCountsCard } from './ObjectCountsCard';
import { StorageUsageCard } from './StorageUsageCard';
import { PerformanceCard } from './PerformanceCard';
import { FilesystemCard } from './FilesystemCard';
import { SessionsErrorsBar } from './SessionsErrorsBar';

export interface ServerHealthDashboardProps {
  enabled?: boolean;
  mockMode?: boolean;
  className?: string;
}

function DashboardSkeleton() {
  return (
    <div className="w-full max-w-4xl animate-pulse">
      {/* Gauge skeleton */}
      <div className="mb-8 flex justify-center">
        <div className="h-36 w-full max-w-72 rounded-lg bg-theme-bg-tertiary/50" />
      </div>

      {/* Cards skeleton */}
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-4">
        <div className="h-40 bg-theme-bg-tertiary/50 rounded-lg" />
        <div className="h-40 bg-theme-bg-tertiary/50 rounded-lg" />
        <div className="h-40 bg-theme-bg-tertiary/50 rounded-lg" />
      </div>

      {/* Bar skeleton */}
      <div className="h-10 bg-theme-bg-tertiary/50 rounded-lg" />
    </div>
  );
}

export function ServerHealthDashboard({
  enabled = true,
  mockMode = false,
  className,
}: ServerHealthDashboardProps) {
  const { data, previousData, status, lastFetchTime, error, retry } = useMetrics({
    enabled,
    mockMode,
    interval: 5000,
  });

  // Staleness tracking
  const lastUpdated = useRelativeTime(lastFetchTime || 0);
  const isStale = lastFetchTime ? Date.now() - lastFetchTime > 10000 : false;
  const isOffline = status === 'error' && !data;

  // Loading state
  if (status === 'loading' && !data) {
    return (
      <div className={cn('flex flex-col items-center justify-center p-3 sm:p-8', className)}>
        <DashboardSkeleton />
      </div>
    );
  }

  // Offline state (no data at all)
  if (isOffline) {
    return (
      <div className={cn('flex flex-col items-center justify-center p-3 sm:p-8', className)}>
        <div className="max-w-md rounded-xl border border-theme-border bg-theme-bg-tertiary/80 p-5 text-center sm:p-8">
          <WifiOff className="w-12 h-12 mx-auto mb-4 text-theme-text-dim" />
          <h2 className="text-lg font-semibold text-theme-text-secondary mb-2">Server Offline</h2>
          <p className="text-theme-text-muted mb-4">
            Unable to reach CXDB server at /v1/metrics
          </p>
          {error && (
            <p className="text-xs text-theme-text-dim mb-4 font-mono">
              {error.message}
            </p>
          )}
          <button
            onClick={retry}
            className="px-4 py-2 bg-theme-accent-muted border border-theme-accent/30 text-theme-accent rounded-lg hover:bg-theme-accent/30 transition-colors flex items-center gap-2 mx-auto"
          >
            <RefreshCw className="w-4 h-4" />
            Retry Now
          </button>
        </div>
      </div>
    );
  }

  // Main dashboard (with or without stale data)
  if (!data) return null;

  return (
    <div className={cn('mx-auto flex w-full max-w-4xl flex-col p-3 sm:p-6', className)}>
      {/* Stale warning */}
      {isStale && (
        <div className="mb-4 flex w-full flex-col items-start justify-between gap-2 rounded-lg border border-amber-500/30 bg-amber-900/20 px-3 py-2 sm:flex-row sm:items-center sm:px-4">
          <span className="text-sm text-amber-400">
            Data may be stale &middot; Last updated: {lastUpdated}
          </span>
          <button
            onClick={retry}
            className="text-sm text-amber-400 hover:text-amber-300 flex items-center gap-1"
          >
            <RefreshCw className="w-3 h-3" />
            Retrying...
          </button>
        </div>
      )}

      {/* Capacity Gauge - In-memory index size vs 1GB budget */}
      <CapacityGauge
        usedBytes={
          data.storage.turns_index_bytes +
          data.storage.blobs_index_bytes +
          data.storage.heads_table_bytes +
          data.storage.turns_meta_bytes
        }
        className="mb-4"
      />

      {/* Three-column cards */}
      <div className="w-full grid grid-cols-1 md:grid-cols-3 gap-4 mb-4">
        <ObjectCountsCard
          objects={data.objects}
          previousObjects={previousData?.objects}
          filesystem={data.filesystem}
        />
        <StorageUsageCard storage={data.storage} filesystem={data.filesystem} />
        <PerformanceCard
          perf={data.perf}
          appendHistory={data.perf.append_tps_history}
          getLastHistory={data.perf.get_last_tps_history}
          httpHistory={data.perf.http_req_tps_history}
        />
      </div>

      {/* Filesystem Card */}
      <FilesystemCard filesystem={data.filesystem} className="w-full mb-4" />

      {/* Sessions + Errors Bar */}
      <SessionsErrorsBar
        sessions={data.sessions}
        errors={data.errors}
        perf={data.perf}
        className="w-full"
      />

      {/* Last updated footer */}
      <div className="mt-4 text-xs text-theme-text-dim">
        Last updated: {lastUpdated}
        {mockMode && <span className="ml-2 text-amber-500">(mock data)</span>}
      </div>
    </div>
  );
}
