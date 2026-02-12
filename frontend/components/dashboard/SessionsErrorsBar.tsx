'use client';

import { useState, useCallback } from 'react';
import { cn } from '@/lib/utils';
import { AlertTriangle, AlertOctagon, ChevronDown, ChevronUp } from '@/components/icons';
import type { SessionMetrics, ErrorMetrics, PerfMetrics, ErrorEntry } from '@/types';

const API_BASE = process.env.NEXT_PUBLIC_API_BASE || '/v1';

export interface SessionsErrorsBarProps {
  sessions: SessionMetrics;
  errors: ErrorMetrics;
  perf: PerfMetrics;
  className?: string;
}

function formatTimestamp(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

export function SessionsErrorsBar({ sessions, errors, perf, className }: SessionsErrorsBarProps) {
  const errorRate = perf.http_errors_tps_1m;
  const isElevated = errorRate > 0.5;
  const isHigh = errorRate > 2.0;
  const [expanded, setExpanded] = useState(false);
  const [recentErrors, setRecentErrors] = useState<ErrorEntry[]>([]);
  const [loading, setLoading] = useState(false);

  const toggleErrors = useCallback(async () => {
    if (expanded) {
      setExpanded(false);
      return;
    }
    setLoading(true);
    try {
      const resp = await fetch(`${API_BASE}/errors?limit=50`);
      if (resp.ok) {
        const data = await resp.json();
        setRecentErrors(data.errors || []);
      }
    } catch {
      // fetch failed - show empty
    }
    setLoading(false);
    setExpanded(true);
  }, [expanded]);

  // Format error breakdown
  const errorBreakdown = Object.entries(errors.by_type)
    .map(([type, count]) => `${count} ${type}`)
    .join(', ');

  const hasErrors = errors.total > 0;

  return (
    <div className={cn('bg-theme-bg-tertiary/50 border border-theme-border rounded-lg', className)}>
      <div className="px-4 py-2 flex items-center justify-between flex-wrap gap-2">
        {/* Sessions info */}
        <div className="flex items-center gap-4 text-sm">
          <span className="text-theme-text-secondary">
            Sessions:{' '}
            <span className="text-emerald-400 font-medium">{sessions.active}</span>
            {' active'}
            {sessions.idle > 0 && (
              <span className="text-theme-text-dim">, {sessions.idle} idle</span>
            )}
            <span className="text-theme-text-dim"> ({sessions.total.toLocaleString()} total)</span>
          </span>
        </div>

        {/* Divider */}
        <div className="hidden sm:block w-px h-4 bg-theme-bg-hover" />

        {/* Errors info - clickable when errors exist */}
        <div className="flex items-center gap-2 text-sm">
          <button
            onClick={hasErrors ? toggleErrors : undefined}
            disabled={!hasErrors}
            className={cn(
              'flex items-center gap-1 text-theme-text-secondary',
              hasErrors && 'hover:text-theme-text-primary cursor-pointer',
              !hasErrors && 'cursor-default'
            )}
          >
            Errors: <span className="text-theme-text-muted">{errors.total}</span>
            {errorBreakdown && (
              <span className="text-theme-text-dim"> ({errorBreakdown})</span>
            )}
            {hasErrors && (
              expanded
                ? <ChevronUp className="w-3 h-3 ml-1 text-theme-text-dim" />
                : <ChevronDown className="w-3 h-3 ml-1 text-theme-text-dim" />
            )}
          </button>

          {/* Error rate indicator */}
          {isHigh && (
            <span className="flex items-center gap-1 px-2 py-0.5 bg-red-900/30 border border-red-500/50 rounded text-red-400 text-xs">
              <AlertOctagon className="w-3 h-3" />
              {errorRate.toFixed(1)}/s (high)
            </span>
          )}
          {isElevated && !isHigh && (
            <span className="flex items-center gap-1 px-2 py-0.5 bg-amber-900/30 border border-amber-500/50 rounded text-amber-400 text-xs">
              <AlertTriangle className="w-3 h-3" />
              {errorRate.toFixed(1)}/s (elevated)
            </span>
          )}
        </div>
      </div>

      {/* Expandable error detail list */}
      {expanded && (
        <div className="border-t border-theme-border px-4 py-2 max-h-64 overflow-y-auto">
          {loading ? (
            <div className="text-xs text-theme-text-dim py-1">Loading...</div>
          ) : recentErrors.length === 0 ? (
            <div className="text-xs text-theme-text-dim py-1">No recent error details available.</div>
          ) : (
            <table className="w-full text-xs">
              <thead>
                <tr className="text-theme-text-dim text-left">
                  <th className="py-1 pr-3 font-normal">Time</th>
                  <th className="py-1 pr-3 font-normal">Type</th>
                  <th className="py-1 pr-3 font-normal">Status</th>
                  <th className="py-1 pr-3 font-normal">Path</th>
                  <th className="py-1 font-normal">Message</th>
                </tr>
              </thead>
              <tbody>
                {recentErrors.map((err, i) => (
                  <tr key={i} className="border-t border-theme-border/50">
                    <td className="py-1 pr-3 text-theme-text-dim whitespace-nowrap font-mono">
                      {formatTimestamp(err.timestamp_ms)}
                    </td>
                    <td className="py-1 pr-3 text-theme-text-muted">{err.kind}</td>
                    <td className="py-1 pr-3 font-mono text-amber-400">{err.status_code}</td>
                    <td className="py-1 pr-3 text-theme-text-muted font-mono truncate max-w-[200px]">
                      {err.path || '—'}
                    </td>
                    <td className="py-1 text-theme-text-secondary truncate max-w-[250px]">
                      {err.message}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}
