// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { useState } from 'react';
import { cn } from '@/lib/utils';
import {
  ChevronDown,
  ChevronRight,
  Clock,
  User,
  Hash,
  Folder,
  GitBranch,
  MessageSquare,
  CheckCircle,
  XCircle,
  AlertTriangle,
  ExternalLink,
  Wrench,
  Database,
} from './icons';
import type {
  QuestEvent,
  QuestSnapshot,
  QuestStartedData,
  PendingReviewData,
  ReviewApprovedData,
  ReviewRejectedData,
  ReviewEscalatedData,
  QuestCompletedData,
  QuestFailedData,
  QuestCancelledData,
  SteeringReceivedData,
  JudgeInterventionData,
  ReviewEvidence,
  ActionSummary,
  SnapshotRecord,
} from '@/types/quest';
import {
  getQuestEventLabel,
  getQuestEventColors,
  getQuestEventEmoji,
  getShortQuestName,
  formatBytes,
  formatTimeOnly,
} from '@/types/quest';

// ============================================================================
// Shared Components
// ============================================================================

function CollapsibleSection({
  title,
  defaultOpen = false,
  children,
  badge,
}: {
  title: string;
  defaultOpen?: boolean;
  children: React.ReactNode;
  badge?: React.ReactNode;
}) {
  const [isOpen, setIsOpen] = useState(defaultOpen);

  return (
    <div className="border border-theme-border/50 rounded-lg overflow-hidden">
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="w-full px-3 py-2 flex items-center justify-between text-left bg-theme-bg-tertiary/30 hover:bg-theme-bg-tertiary/50 transition-colors"
      >
        <div className="flex items-center gap-2">
          {isOpen ? (
            <ChevronDown className="w-4 h-4 text-theme-text-dim" />
          ) : (
            <ChevronRight className="w-4 h-4 text-theme-text-dim" />
          )}
          <span className="text-xs text-theme-text-muted font-medium">{title}</span>
        </div>
        {badge}
      </button>
      {isOpen && (
        <div className="p-3 border-t border-theme-border/50 bg-theme-bg-secondary/50">{children}</div>
      )}
    </div>
  );
}

function ContextLinks({
  parentContextId,
  rootContextId,
}: {
  parentContextId?: string | number;
  rootContextId?: string | number;
}) {
  if (!parentContextId && !rootContextId) return null;

  return (
    <div className="flex flex-wrap items-center gap-3 text-xs">
      {parentContextId && (
        <span className="flex items-center gap-1.5 text-theme-text-muted">
          <GitBranch className="w-3.5 h-3.5" />
          Parent:
          <span className="font-mono text-theme-text-secondary bg-theme-bg-tertiary/50 px-1.5 py-0.5 rounded">
            {parentContextId}
          </span>
        </span>
      )}
      {rootContextId && rootContextId !== parentContextId && (
        <span className="flex items-center gap-1.5 text-theme-text-muted">
          Root:
          <span className="font-mono text-theme-text-secondary bg-theme-bg-tertiary/50 px-1.5 py-0.5 rounded">
            {rootContextId}
          </span>
        </span>
      )}
    </div>
  );
}

function ReplyChannel({
  platform,
  channelId,
  threadTs,
}: {
  platform?: string;
  channelId?: string;
  threadTs?: string;
}) {
  if (!platform) return null;

  return (
    <div className="flex items-center gap-2 text-xs text-theme-text-dim">
      <MessageSquare className="w-3.5 h-3.5" />
      <span className="capitalize font-medium">{platform}</span>
      {channelId && <span className="font-mono text-theme-text-faint">{channelId}</span>}
      {threadTs && <span className="font-mono text-theme-border">{threadTs}</span>}
    </div>
  );
}

function WorkerBadge({ workerId }: { workerId?: string }) {
  if (!workerId) return null;
  return (
    <span className="text-xs bg-theme-bg-hover/50 text-theme-text-muted px-2 py-0.5 rounded">
      Worker {workerId}
    </span>
  );
}

// ============================================================================
// Action Summary Component
// ============================================================================

function ActionSummaryCard({ summary }: { summary: ActionSummary }) {
  const total = Number(summary.total) || 0;
  const completed = Number(summary.completed) || 0;
  const failed = Number(summary.failed) || 0;
  const skipped = Number(summary.skipped) || 0;

  return (
    <div className="grid grid-cols-2 gap-2 text-center sm:grid-cols-4">
      <div className="bg-theme-bg-tertiary/50 rounded-lg p-2">
        <div className="text-lg font-bold text-theme-text-secondary">{total}</div>
        <div className="text-[10px] text-theme-text-dim uppercase">Total</div>
      </div>
      <div className="bg-green-500/10 rounded-lg p-2">
        <div className="text-lg font-bold text-green-400">{completed}</div>
        <div className="text-[10px] text-green-500/70 uppercase">Done</div>
      </div>
      <div className="bg-red-500/10 rounded-lg p-2">
        <div className="text-lg font-bold text-red-400">{failed}</div>
        <div className="text-[10px] text-red-500/70 uppercase">Failed</div>
      </div>
      <div className="bg-theme-bg-hover/30 rounded-lg p-2">
        <div className="text-lg font-bold text-theme-text-muted">{skipped}</div>
        <div className="text-[10px] text-theme-text-faint uppercase">Skip</div>
      </div>
    </div>
  );
}

// ============================================================================
// Snapshots List Component
// ============================================================================

function SnapshotsList({ snapshots }: { snapshots: SnapshotRecord[] }) {
  if (snapshots.length === 0) return null;

  // Group by trigger
  const byTrigger = snapshots.reduce((acc, s) => {
    const trigger = s.trigger || 'unknown';
    if (!acc[trigger]) acc[trigger] = [];
    acc[trigger].push(s);
    return acc;
  }, {} as Record<string, SnapshotRecord[]>);

  return (
    <div className="space-y-2">
      <div className="text-xs text-theme-text-dim font-medium">
        {snapshots.length} Filesystem Snapshots
      </div>
      <div className="flex flex-wrap gap-2">
        {Object.entries(byTrigger).map(([trigger, snaps]) => (
          <div
            key={trigger}
            className="flex items-center gap-1.5 bg-cyan-500/10 text-cyan-300 px-2 py-1 rounded text-xs"
          >
            <Database className="w-3 h-3" />
            <span>{snaps.length}x</span>
            <span className="text-cyan-400/70">{trigger}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

// ============================================================================
// Evidence Panel Component
// ============================================================================

function EvidencePanel({ evidence }: { evidence: ReviewEvidence }) {
  return (
    <div className="space-y-3">
      {/* Action Summary */}
      {evidence.action_summary && (
        <div className="space-y-2">
          <div className="text-xs text-theme-text-dim font-medium flex items-center gap-1.5">
            <Wrench className="w-3.5 h-3.5" />
            Actions
          </div>
          <ActionSummaryCard summary={evidence.action_summary} />
        </div>
      )}

      {/* Snapshots */}
      {evidence.snapshots && evidence.snapshots.length > 0 && (
        <SnapshotsList snapshots={evidence.snapshots} />
      )}

      {/* Summary Content (markdown) */}
      {evidence.summary_content && (
        <CollapsibleSection
          title="Quest Summary"
          defaultOpen={false}
          badge={
            <span className="text-[10px] text-theme-text-faint">
              {evidence.summary_content.length} chars
            </span>
          }
        >
          <div className="text-xs text-theme-text-secondary whitespace-pre-wrap font-mono leading-relaxed max-h-[300px] overflow-y-auto">
            {evidence.summary_content}
          </div>
        </CollapsibleSection>
      )}
    </div>
  );
}

// ============================================================================
// Result Content Component
// ============================================================================

function ResultContent({ result, maxHeight = 300 }: { result: string; maxHeight?: number }) {
  const [expanded, setExpanded] = useState(false);
  const isLong = result.length > 500;

  // Check if result contains URLs
  const urlRegex = /(https?:\/\/[^\s]+)/g;
  const parts = result.split(urlRegex);

  return (
    <div className="space-y-2">
      <div
        className={cn(
          'text-sm text-theme-text-secondary whitespace-pre-wrap leading-relaxed',
          !expanded && isLong && 'line-clamp-6'
        )}
        style={!expanded && isLong ? {} : { maxHeight, overflowY: 'auto' }}
      >
        {parts.map((part, i) =>
          urlRegex.test(part) ? (
            <a
              key={i}
              href={part}
              target="_blank"
              rel="noopener noreferrer"
              className="text-blue-400 hover:underline inline-flex items-center gap-0.5"
            >
              {part.length > 60 ? part.slice(0, 57) + '...' : part}
              <ExternalLink className="w-3 h-3" />
            </a>
          ) : (
            <span key={i}>{part}</span>
          )
        )}
      </div>
      {isLong && (
        <button
          onClick={() => setExpanded(!expanded)}
          className="text-xs text-theme-text-dim hover:text-theme-text-secondary"
        >
          {expanded ? 'Show less' : `Show more (${result.length} chars)`}
        </button>
      )}
    </div>
  );
}

// ============================================================================
// Event-Specific Data Renderers
// ============================================================================

function QuestStartedDataView({ data }: { data: QuestStartedData }) {
  return (
    <div className="space-y-3">
      {/* Description */}
      {data.description && (
        <div className="text-sm text-theme-text-secondary leading-relaxed">{data.description}</div>
      )}

      {/* Status flags */}
      <div className="flex flex-wrap items-center gap-2">
        <WorkerBadge workerId={data.worker_id} />
        {data.resumed_from_frozen && (
          <span className="text-xs bg-amber-500/20 text-amber-300 px-2 py-0.5 rounded">
            Resumed from frozen
          </span>
        )}
      </div>

      {/* Context links */}
      <ContextLinks
        parentContextId={data.parent_context_id}
        rootContextId={data.root_context_id}
      />

      {/* Reply channel */}
      <ReplyChannel
        platform={data.reply_platform}
        channelId={data.reply_channel_id}
        threadTs={data.reply_thread_ts}
      />
    </div>
  );
}

function PendingReviewDataView({ data }: { data: PendingReviewData }) {
  return (
    <div className="space-y-4">
      {/* Result */}
      {data.result && (
        <div className="space-y-2">
          <div className="text-xs text-theme-text-dim font-medium flex items-center gap-1.5">
            Result
            {data.result_length && (
              <span className="text-theme-text-faint">({data.result_length} chars)</span>
            )}
          </div>
          <div className="bg-theme-bg-tertiary/50 rounded-lg p-3 border border-theme-border/50">
            <ResultContent result={data.result} />
          </div>
        </div>
      )}

      {/* Evidence */}
      {data.evidence && (
        <div className="space-y-2">
          <div className="text-xs text-theme-text-dim font-medium">Evidence</div>
          <EvidencePanel evidence={data.evidence} />
        </div>
      )}
    </div>
  );
}

function ReviewApprovedDataView({ data }: { data: ReviewApprovedData }) {
  return (
    <div className="space-y-3">
      {data.quality_score !== undefined && (
        <div className="flex items-center gap-3">
          <span className="text-xs text-theme-text-dim">Quality Score</span>
          <div className="flex items-center gap-1">
            <span className="text-2xl font-bold text-green-400">{data.quality_score}</span>
            <span className="text-sm text-theme-text-dim">/10</span>
          </div>
        </div>
      )}
      {data.auto_approved && (
        <div className="flex items-center gap-2 text-xs text-amber-400">
          <Clock className="w-3.5 h-3.5" />
          Auto-approved: {data.reason || 'timeout'}
        </div>
      )}
    </div>
  );
}

function ReviewRejectedDataView({ data }: { data: ReviewRejectedData }) {
  return (
    <div className="space-y-3">
      {data.feedback && (
        <div className="text-sm text-red-200/90 bg-red-500/10 px-3 py-2 rounded border border-red-500/20">
          {data.feedback}
        </div>
      )}
      {data.issues && data.issues.length > 0 && (
        <div className="space-y-1">
          <div className="text-xs text-theme-text-dim font-medium">Issues</div>
          <ul className="list-disc list-inside text-sm text-red-300/80 space-y-1">
            {data.issues.map((issue, i) => (
              <li key={i}>{issue}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function ReviewEscalatedDataView({ data }: { data: ReviewEscalatedData }) {
  return (
    <div className="space-y-3">
      {data.attempts !== undefined && (
        <div className="flex items-center gap-2">
          <AlertTriangle className="w-4 h-4 text-orange-400" />
          <span className="text-sm text-orange-300">
            Escalated after {data.attempts} review attempts
          </span>
        </div>
      )}
      {data.feedback && (
        <div className="text-sm text-theme-text-secondary bg-theme-bg-tertiary/50 px-3 py-2 rounded">
          {data.feedback}
        </div>
      )}
      {data.issues && data.issues.length > 0 && (
        <div className="space-y-1">
          <div className="text-xs text-theme-text-dim font-medium">Last Issues</div>
          <ul className="list-disc list-inside text-sm text-theme-text-muted space-y-1">
            {data.issues.map((issue, i) => (
              <li key={i}>{issue}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

function QuestCompletedDataView({ data }: { data: QuestCompletedData }) {
  return (
    <div className="space-y-3">
      {data.result_length && (
        <div className="text-xs text-theme-text-muted">
          Result: {data.result_length} characters
        </div>
      )}
      <ReplyChannel
        platform={data.reply_platform}
        channelId={data.reply_channel_id}
        threadTs={data.reply_thread_ts}
      />
    </div>
  );
}

function QuestFailedDataView({ data }: { data: QuestFailedData }) {
  return (
    <div className="space-y-3">
      {data.error && (
        <div className="flex items-start gap-2 text-sm text-red-200/90 bg-red-500/10 px-3 py-2 rounded border border-red-500/20">
          <XCircle className="w-4 h-4 text-red-400 flex-shrink-0 mt-0.5" />
          {data.error}
        </div>
      )}
      <ReplyChannel
        platform={data.reply_platform}
        channelId={data.reply_channel_id}
        threadTs={data.reply_thread_ts}
      />
    </div>
  );
}

function QuestCancelledDataView({ data }: { data: QuestCancelledData }) {
  return (
    <div className="space-y-3">
      {data.reason && (
        <div className="text-sm text-theme-text-secondary">{data.reason}</div>
      )}
      <ReplyChannel
        platform={data.reply_platform}
        channelId={data.reply_channel_id}
        threadTs={data.reply_thread_ts}
      />
    </div>
  );
}

function SteeringReceivedDataView({ data }: { data: SteeringReceivedData }) {
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        {data.steering_type && (
          <span className="text-xs bg-blue-500/20 text-blue-300 px-2 py-0.5 rounded capitalize">
            {data.steering_type.replace(/_/g, ' ')}
          </span>
        )}
        {data.source && (
          <span className="text-xs text-theme-text-dim">from {data.source}</span>
        )}
      </div>
      {data.message && (
        <div className="text-sm text-theme-text-secondary bg-blue-500/10 px-3 py-2 rounded border border-blue-500/20">
          {data.message}
        </div>
      )}
    </div>
  );
}

function JudgeInterventionDataView({ data }: { data: JudgeInterventionData }) {
  return (
    <div className="space-y-3">
      {data.intervention && (
        <div className="text-sm text-purple-200 bg-purple-500/10 px-3 py-2 rounded border border-purple-500/20">
          {data.intervention}
        </div>
      )}
      <div className="flex items-center gap-4">
        {data.confidence !== undefined && (
          <div className="text-xs text-theme-text-muted">
            Confidence: <span className="text-purple-300 font-medium">{(data.confidence * 100).toFixed(0)}%</span>
          </div>
        )}
        {data.tool_call_count !== undefined && (
          <div className="text-xs text-theme-text-muted">
            Tool calls evaluated: <span className="text-theme-text-secondary">{data.tool_call_count}</span>
          </div>
        )}
      </div>
      {data.observations && data.observations.length > 0 && (
        <div className="space-y-1">
          <div className="text-xs text-theme-text-dim font-medium">Observations</div>
          <ul className="list-disc list-inside text-sm text-purple-300/80 space-y-1">
            {data.observations.map((obs, i) => (
              <li key={i}>{obs}</li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

// Generic fallback for unknown data structures
function GenericDataView({ data }: { data: Record<string, unknown> }) {
  const entries = Object.entries(data).filter(([, v]) => v !== null && v !== undefined);
  if (entries.length === 0) return null;

  return (
    <div className="space-y-2">
      {entries.map(([key, value]) => (
        <div key={key} className="flex items-start gap-2 text-sm">
          <span className="text-theme-text-dim min-w-[100px]">{key}</span>
          <span className="text-theme-text-secondary break-all">
            {typeof value === 'object' ? JSON.stringify(value) : String(value)}
          </span>
        </div>
      ))}
    </div>
  );
}

// ============================================================================
// Event Data Dispatcher
// ============================================================================

function EventDataView({ eventType, data }: { eventType: string; data: Record<string, unknown> }) {
  switch (eventType) {
    case 'quest_started':
      return <QuestStartedDataView data={data as QuestStartedData} />;
    case 'pending_review':
      return <PendingReviewDataView data={data as PendingReviewData} />;
    case 'review_approved':
      return <ReviewApprovedDataView data={data as ReviewApprovedData} />;
    case 'review_rejected':
      return <ReviewRejectedDataView data={data as ReviewRejectedData} />;
    case 'review_escalated':
      return <ReviewEscalatedDataView data={data as ReviewEscalatedData} />;
    case 'quest_completed':
      return <QuestCompletedDataView data={data as QuestCompletedData} />;
    case 'quest_failed':
      return <QuestFailedDataView data={data as QuestFailedData} />;
    case 'quest_cancelled':
      return <QuestCancelledDataView data={data as QuestCancelledData} />;
    case 'steering_received':
      return <SteeringReceivedDataView data={data as SteeringReceivedData} />;
    case 'judge_intervention':
      return <JudgeInterventionDataView data={data as JudgeInterventionData} />;
    default:
      return <GenericDataView data={data} />;
  }
}

// ============================================================================
// Main Quest Event Renderer
// ============================================================================

interface QuestEventRendererProps {
  event: QuestEvent;
  className?: string;
}

export function QuestEventRenderer({ event, className }: QuestEventRendererProps) {
  const colors = getQuestEventColors(event.event_type);
  const label = getQuestEventLabel(event.event_type);
  const emoji = getQuestEventEmoji(event.event_type);
  const shortName = getShortQuestName(event.quest_id);
  const timeStr = formatTimeOnly(event.timestamp);

  const hasData = event.data && Object.keys(event.data).length > 0;

  return (
    <div className={cn('space-y-3', className)}>
      <div className={cn('border rounded-lg overflow-hidden', colors.border)}>
        {/* Header */}
        <div className={cn('px-4 py-3 flex items-center justify-between', colors.headerBg)}>
          <div className="flex items-center gap-3">
            <span className="text-xl">{emoji}</span>
            <div>
              <span className={cn('text-sm font-semibold', colors.badge.split(' ')[1])}>
                {label}
              </span>
              {timeStr && (
                <div className="flex items-center gap-1 text-xs text-theme-text-dim mt-0.5">
                  <Clock className="w-3 h-3" />
                  {timeStr}
                </div>
              )}
            </div>
          </div>
          <span className={cn('px-2 py-0.5 rounded text-xs font-mono', colors.badge)}>
            {event.event_type}
          </span>
        </div>

        {/* Quest & User info */}
        <div className="px-4 py-2 border-t border-theme-border/30 bg-theme-bg-secondary/30 flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
          <span className="flex items-center gap-1.5 text-theme-text-muted">
            <Hash className="w-3 h-3" />
            <span className="font-mono text-theme-text-secondary truncate max-w-[250px]" title={event.quest_id}>
              {shortName}
            </span>
          </span>
          {event.user_id && (
            <span className="flex items-center gap-1.5 text-theme-text-muted">
              <User className="w-3 h-3" />
              <span className="text-theme-text-secondary">{event.user_id}</span>
            </span>
          )}
        </div>

        {/* Event-specific data */}
        {hasData && (
          <div className="px-4 py-3 border-t border-theme-border/30">
            <EventDataView
              eventType={event.event_type}
              data={event.data as Record<string, unknown>}
            />
          </div>
        )}
      </div>
    </div>
  );
}

// ============================================================================
// Quest Snapshot Renderer
// ============================================================================

interface QuestSnapshotRendererProps {
  snapshot: QuestSnapshot;
  className?: string;
}

export function QuestSnapshotRenderer({ snapshot, className }: QuestSnapshotRendererProps) {
  const shortName = getShortQuestName(snapshot.quest_id);
  const timestamp = typeof snapshot.captured_at === 'string'
    ? parseInt(snapshot.captured_at, 10)
    : snapshot.captured_at;
  const timeStr = timestamp ? new Date(timestamp).toLocaleTimeString() : null;

  return (
    <div className={cn('space-y-3', className)}>
      <div className="border border-cyan-500/30 rounded-lg overflow-hidden">
        <div className="px-4 py-3 bg-cyan-500/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <Folder className="w-5 h-5 text-cyan-400" />
            <div>
              <span className="text-sm font-semibold text-cyan-300">Filesystem Snapshot</span>
              {timeStr && (
                <div className="flex items-center gap-1 text-xs text-theme-text-dim mt-0.5">
                  <Clock className="w-3 h-3" />
                  {timeStr}
                </div>
              )}
            </div>
          </div>
          <span className="px-2 py-0.5 rounded text-xs font-medium bg-cyan-500/20 text-cyan-300">
            {snapshot.trigger}
          </span>
        </div>

        <div className="px-4 py-3 border-t border-cyan-500/20 bg-theme-bg-secondary/30">
          <div className="grid grid-cols-2 gap-4">
            <div>
              <div className="text-2xl font-bold text-cyan-300">{snapshot.file_count}</div>
              <div className="text-xs text-theme-text-dim">Files</div>
            </div>
            <div>
              <div className="text-2xl font-bold text-cyan-300">{formatBytes(snapshot.total_bytes)}</div>
              <div className="text-xs text-theme-text-dim">Total Size</div>
            </div>
          </div>
        </div>

        <div className="px-4 py-2 border-t border-cyan-500/20 bg-cyan-500/5">
          <div className="flex items-center gap-2 text-xs text-cyan-400">
            <Folder className="w-3.5 h-3.5" />
            <span>Browse files in the sidebar below</span>
          </div>
        </div>

        <div className="px-4 py-2 border-t border-theme-border/30 flex items-center gap-2 text-xs text-theme-text-dim">
          <Hash className="w-3 h-3" />
          <span className="font-mono text-theme-text-muted truncate" title={snapshot.quest_id}>
            {shortName}
          </span>
        </div>
      </div>
    </div>
  );
}

// ============================================================================
// TurnRendererProps Wrappers
// ============================================================================

import type { TurnRendererProps } from '@/lib/renderer-registry';
import { isQuestEvent, isQuestSnapshot } from '@/types/quest';

/**
 * Wrapper for QuestEventRenderer that accepts standard TurnRendererProps.
 * Used by the dynamic renderer registry.
 */
export function QuestEventRendererWrapper({ data, className }: TurnRendererProps) {
  if (isQuestEvent(data)) {
    return <QuestEventRenderer event={data} className={className} />;
  }
  return null;
}

/**
 * Wrapper for QuestSnapshotRenderer that accepts standard TurnRendererProps.
 * Used by the dynamic renderer registry.
 */
export function QuestSnapshotRendererWrapper({ data, className }: TurnRendererProps) {
  if (isQuestSnapshot(data)) {
    return <QuestSnapshotRenderer snapshot={data} className={className} />;
  }
  return null;
}

// ============================================================================
// Exports
// ============================================================================

export { isQuestEvent, isQuestSnapshot };
