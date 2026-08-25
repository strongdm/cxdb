// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { cn } from '@/lib/utils';
import { AlertTriangle, AlertOctagon } from '@/components/icons';

// Fixed 1GB budget for in-memory index storage
const CAPACITY_BUDGET_BYTES = 1024 * 1024 * 1024; // 1 GiB

export interface CapacityGaugeProps {
  /** In-memory index size in bytes (sum of index files) */
  usedBytes: number;
  className?: string;
}

function formatBytes(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
  let i = 0;
  let size = bytes;

  while (size >= 1024 && i < units.length - 1) {
    size /= 1024;
    i++;
  }

  return `${size.toFixed(1)} ${units[i]}`;
}

function getCapacityLevel(ratio: number): 'OK' | 'WARN' | 'HOT' | 'CRITICAL' {
  if (ratio < 0.60) return 'OK';
  if (ratio < 0.80) return 'WARN';
  if (ratio < 0.92) return 'HOT';
  return 'CRITICAL';
}

export function CapacityGauge({
  usedBytes,
  className,
}: CapacityGaugeProps) {
  const capacityRatio = usedBytes / CAPACITY_BUDGET_BYTES;
  const capacityLevel = getCapacityLevel(capacityRatio);
  const fillPct = Math.min(capacityRatio * 100, 100);

  const showSpillWarning = capacityRatio >= 0.85 && capacityRatio < 0.95;
  const showCriticalWarning = capacityRatio >= 0.95;

  const levelColor = {
    OK: 'text-theme-gauge-ok',
    WARN: 'text-theme-gauge-warn',
    HOT: 'text-theme-gauge-hot',
    CRITICAL: 'text-theme-gauge-critical',
  }[capacityLevel];

  const barColor = {
    OK: 'bg-theme-gauge-ok',
    WARN: 'bg-theme-gauge-warn',
    HOT: 'bg-theme-gauge-hot',
    CRITICAL: 'bg-theme-gauge-critical',
  }[capacityLevel];

  return (
    <div className={cn('w-full', className)}>
      {/* Warning banners */}
      {showCriticalWarning && (
        <div className="mb-3 px-3 py-2 bg-red-900/30 border border-red-500/50 rounded-lg flex items-center gap-2 text-red-400">
          <AlertOctagon className="w-4 h-4 shrink-0" />
          <div className="text-sm">
            <span className="font-semibold">CRITICAL:</span> Index at {(capacityRatio * 100).toFixed(0)}% of 1GB budget
          </div>
        </div>
      )}
      {showSpillWarning && !showCriticalWarning && (
        <div className="mb-3 px-3 py-2 bg-amber-900/30 border border-amber-500/50 rounded-lg flex items-center gap-2 text-amber-400">
          <AlertTriangle className="w-4 h-4 shrink-0" />
          <div className="text-sm">
            <span className="font-semibold">HIGH CAPACITY:</span> Index exceeds 85% of 1GB budget
          </div>
        </div>
      )}

      {/* Main horizontal gauge */}
      <div className="bg-theme-bg-tertiary/50 border border-theme-border rounded-lg p-4">
        <div className="mb-2 flex flex-col items-start justify-between gap-1 sm:flex-row sm:items-center">
          <h3 className="text-sm font-medium text-theme-text-secondary">In-Memory Index Capacity</h3>
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span className={cn('text-lg font-bold', levelColor)}>
              {(capacityRatio * 100).toFixed(0)}%
            </span>
            <span className="text-sm text-theme-text-muted">
              {formatBytes(usedBytes)} / 1 GiB
            </span>
          </div>
        </div>

        {/* Horizontal bar */}
        <div className="relative h-4 bg-theme-bg-hover/50 rounded-full overflow-hidden">
          {/* Zone markers (background) */}
          <div className="absolute inset-0 flex">
            <div className="w-[60%] bg-emerald-500/10" />
            <div className="w-[20%] bg-yellow-500/10" />
            <div className="w-[12%] bg-orange-500/10" />
            <div className="w-[8%] bg-red-500/10" />
          </div>

          {/* Fill bar */}
          <div
            className={cn(barColor, 'absolute top-0 left-0 h-full rounded-full transition-all duration-500 ease-out')}
            style={{ width: `${fillPct}%` }}
          />

          {/* Zone dividers */}
          <div className="absolute top-0 left-[60%] w-px h-full bg-theme-text-faint" />
          <div className="absolute top-0 left-[80%] w-px h-full bg-theme-text-faint" />
          <div className="absolute top-0 left-[92%] w-px h-full bg-theme-text-faint" />
        </div>

        {/* Zone labels */}
        <div className="flex mt-1 text-[10px] text-theme-text-dim">
          <div className="w-[60%] text-center">OK</div>
          <div className="w-[20%] text-center">WARN</div>
          <div className="w-[12%] text-center">HOT</div>
          <div className="w-[8%] text-center">CRIT</div>
        </div>
      </div>
    </div>
  );
}
