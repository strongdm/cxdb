// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { cn } from '@/lib/utils';
import { TrendIndicator } from './TrendIndicator';
import type { ObjectMetrics, FilesystemMetrics } from '@/types';

export interface ObjectCountsCardProps {
  objects: ObjectMetrics;
  previousObjects?: ObjectMetrics;
  filesystem?: FilesystemMetrics;
  className?: string;
}

function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`;
  return n.toString();
}

export function ObjectCountsCard({ objects, previousObjects, filesystem, className }: ObjectCountsCardProps) {
  return (
    <div className={cn('bg-theme-bg-tertiary/50 border border-theme-border rounded-lg p-4', className)}>
      <div className="flex items-center justify-between mb-4">
        <h3 className="text-sm font-medium text-theme-text-secondary">Objects</h3>
        <TrendIndicator
          current={objects.turns_total}
          previous={previousObjects?.turns_total}
        />
      </div>

      {/* Main counts */}
      <div className="mb-4 grid grid-cols-3 gap-2 sm:gap-4">
        <div className="text-center">
          <div className="text-xl font-bold text-theme-text sm:text-2xl">
            {formatCount(objects.contexts_total)}
          </div>
          <div className="text-xs text-theme-text-dim">contexts</div>
        </div>
        <div className="text-center">
          <div className="text-xl font-bold text-theme-text sm:text-2xl">
            {formatCount(objects.turns_total)}
          </div>
          <div className="text-xs text-theme-text-dim">turns</div>
        </div>
        <div className="text-center">
          <div className="text-xl font-bold text-theme-text sm:text-2xl">
            {formatCount(objects.blobs_total)}
          </div>
          <div className="text-xs text-theme-text-dim">blobs</div>
        </div>
      </div>

      {/* Footer info */}
      <div className="pt-3 border-t border-theme-border/50">
        <div className="text-sm text-theme-text-muted">
          {filesystem && (
            <span>{filesystem.snapshots_total.toLocaleString()} fs snapshots &middot; </span>
          )}
          {objects.registry_types_total} types &middot; {objects.registry_bundles_total} bundles
        </div>
      </div>
    </div>
  );
}
