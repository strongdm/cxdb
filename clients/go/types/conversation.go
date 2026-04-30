// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

// Package types defines canonical conversation types for ai-cxdb visualization.
//
// These types provide a well-defined schema that the ai-cxdb frontend can render
// with certainty. SDKs that want rich conversation visualization should serialize
// their data using these types.
//
// The frontend checks for the presence of the "item_type" field to determine if
// a turn uses canonical types. Non-canonical turns fall back to best-effort rendering.
package types

import "time"

// TypeID constants for registering these types with the type registry.
const (
	// TypeIDConversationItem is the type ID for ConversationItem.
	// Uses namespace.TypeName format with versions managed in the registry bundle.
	TypeIDConversationItem = "cxdb.ConversationItem"

	// TypeVersionConversationItem is the current schema version.
	// Version 3 maintains full backward compatibility with existing data.
	TypeVersionConversationItem uint32 = 3

	// Legacy type ID - kept for backward compatibility with existing logged data.
	// New code should use TypeIDConversationItem instead.
	// Deprecated: Use TypeIDConversationItem for new code.
	TypeIDConversationItemLegacy = "cxdb.v3:ConversationItem"
)

// =============================================================================
// Enums
// =============================================================================

// ItemType discriminates which variant of ConversationItem is populated.
type ItemType string

const (
	// ItemTypeUserInput indicates a user message.
	ItemTypeUserInput ItemType = "user_input"

	// ItemTypeAssistantTurn indicates a complete assistant response with nested tool calls.
	// This is the preferred type for new code - it preserves semantic grouping.
	ItemTypeAssistantTurn ItemType = "assistant_turn"

	// ItemTypeSystem indicates a system message (info, warning, error, guardrail).
	ItemTypeSystem ItemType = "system"

	// ItemTypeHandoff indicates an agent-to-agent handoff event.
	ItemTypeHandoff ItemType = "handoff"

	// Legacy types - kept for backward compatibility
	// New code should use ItemTypeAssistantTurn instead of these flat types.

	// ItemTypeAssistant indicates an assistant response (legacy flat type).
	// Deprecated: Use ItemTypeAssistantTurn for new code.
	ItemTypeAssistant ItemType = "assistant"

	// ItemTypeToolCall indicates a tool invocation request (legacy flat type).
	// Deprecated: Use ItemTypeAssistantTurn with nested ToolCalls for new code.
	ItemTypeToolCall ItemType = "tool_call"

	// ItemTypeToolResult indicates the result of a tool invocation (legacy flat type).
	// Deprecated: Use ItemTypeAssistantTurn with nested ToolCalls for new code.
	ItemTypeToolResult ItemType = "tool_result"
)

// ItemStatus represents the lifecycle state of an item.
type ItemStatus string

const (
	// ItemStatusPending indicates the item is queued but not started.
	ItemStatusPending ItemStatus = "pending"

	// ItemStatusStreaming indicates the item is actively receiving data.
	ItemStatusStreaming ItemStatus = "streaming"

	// ItemStatusComplete indicates the item finished successfully.
	ItemStatusComplete ItemStatus = "complete"

	// ItemStatusError indicates the item encountered an error.
	ItemStatusError ItemStatus = "error"

	// ItemStatusCancelled indicates the item was cancelled before completion.
	ItemStatusCancelled ItemStatus = "cancelled"
)

// ToolCallStatus provides finer-grained tool execution state.
type ToolCallStatus string

const (
	// ToolCallStatusPending indicates the tool call is queued.
	ToolCallStatusPending ToolCallStatus = "pending"

	// ToolCallStatusExecuting indicates the tool is currently running.
	ToolCallStatusExecuting ToolCallStatus = "executing"

	// ToolCallStatusComplete indicates successful completion.
	ToolCallStatusComplete ToolCallStatus = "complete"

	// ToolCallStatusError indicates the tool failed.
	ToolCallStatusError ToolCallStatus = "error"

	// ToolCallStatusSkipped indicates the tool was skipped (e.g., guardrail block).
	ToolCallStatusSkipped ToolCallStatus = "skipped"
)

// SystemKind categorizes system messages.
type SystemKind string

const (
	// SystemKindInfo is an informational message.
	SystemKindInfo SystemKind = "info"

	// SystemKindWarning is a warning message.
	SystemKindWarning SystemKind = "warning"

	// SystemKindError is an error message.
	SystemKindError SystemKind = "error"

	// SystemKindGuardrail indicates a guardrail was triggered.
	SystemKindGuardrail SystemKind = "guardrail"

	// SystemKindRateLimit indicates a rate limit was hit.
	SystemKindRateLimit SystemKind = "rate_limit"

	// SystemKindRewind indicates a session rewind occurred.
	SystemKindRewind SystemKind = "rewind"
)

// =============================================================================
// Core Conversation Item
// =============================================================================

// ConversationItem is the canonical turn type for conversation visualization.
//
// Exactly one of the variant fields will be populated based on the ItemType discriminator.
// The frontend uses the item_type field to determine which renderer to use.
type ConversationItem struct {
	// ItemType discriminates which variant is populated. REQUIRED.
	ItemType ItemType `msgpack:"1" json:"item_type"`

	// Status indicates the item's lifecycle state.
	Status ItemStatus `msgpack:"2" json:"status,omitempty"`

	// Timestamp is when this item was created (Unix milliseconds).
	Timestamp int64 `msgpack:"3" json:"timestamp,omitempty"`

	// ID is an optional unique identifier for this item.
	ID string `msgpack:"4" json:"id,omitempty"`

	// Primary variants (v2 schema)
	UserInput *UserInput      `msgpack:"10" json:"user_input,omitempty"`
	Turn      *AssistantTurn  `msgpack:"11" json:"turn,omitempty"`
	System    *SystemMessage  `msgpack:"12" json:"system,omitempty"`
	Handoff   *HandoffInfo    `msgpack:"13" json:"handoff,omitempty"`

	// Legacy variants (v1 schema - kept for backward compatibility)
	Assistant  *Assistant  `msgpack:"20" json:"assistant,omitempty"`
	ToolCall   *ToolCall   `msgpack:"21" json:"tool_call,omitempty"`
	ToolResult *ToolResult `msgpack:"22" json:"tool_result,omitempty"`

	// ContextMetadata is optional metadata about the context.
	// By convention, only included in the first turn (depth=1) of a context.
	// The server extracts this to enable efficient context listing with metadata.
	ContextMetadata *ContextMetadata `msgpack:"30" json:"context_metadata,omitempty"`
}

// =============================================================================
// User Input
// =============================================================================

// UserInput represents user-provided input to the conversation.
type UserInput struct {
	// Text is the primary text content from the user.
	Text string `msgpack:"1" json:"text"`

	// Files lists file paths included with the input.
	Files []string `msgpack:"2" json:"files,omitempty"`

	// Synthetic means programmatically injected.
	Synthetic bool `msgpack:"3" json:"synthetic,omitempty"`
}

// =============================================================================
// Assistant Turn (v2 - nested tool calls)
// =============================================================================

// AssistantTurn represents one complete assistant response.
// A turn may include text, tool calls, reasoning, and metrics as a unified cognitive unit.
type AssistantTurn struct {
	// Text is the assistant's response text.
	Text string `msgpack:"1" json:"text"`

	// ToolCalls contains all tool invocations made during this turn.
	ToolCalls []ToolCallItem `msgpack:"2" json:"tool_calls,omitempty"`

	// Reasoning contains extended thinking/reasoning output (if enabled).
	Reasoning string `msgpack:"3" json:"reasoning,omitempty"`

	// Metrics contains token usage for this turn.
	Metrics *TurnMetrics `msgpack:"4" json:"metrics,omitempty"`

	// Agent is the name of the agent that produced this turn (for multi-agent).
	Agent string `msgpack:"5" json:"agent,omitempty"`

	// TurnNumber is the sequential turn number within the run (0-indexed).
	TurnNumber int `msgpack:"6" json:"turn_number,omitempty"`

	// MaxTurns is the maximum allowed turns (for progress indication).
	MaxTurns int `msgpack:"7" json:"max_turns,omitempty"`

	// FinishReason indicates why generation stopped.
	FinishReason string `msgpack:"8" json:"finish_reason,omitempty"`
}

// ToolCallItem represents a single tool invocation with full lifecycle.
// This captures the complete tool call from request to result.
type ToolCallItem struct {
	// ID is the unique identifier for this tool call (from the model).
	ID string `msgpack:"1" json:"id"`

	// Name is the tool/function name.
	Name string `msgpack:"2" json:"name"`

	// Args contains the JSON-encoded tool arguments.
	Args string `msgpack:"3" json:"args"`

	// Status indicates the tool execution state.
	Status ToolCallStatus `msgpack:"4" json:"status"`

	// Description is a human-readable description of what the tool is doing.
	// E.g., "Running `ls -la` in /tmp" or "Reading config.yaml"
	Description string `msgpack:"5" json:"description,omitempty"`

	// StreamingOutput accumulates real-time output from the tool (e.g., shell output).
	StreamingOutput string `msgpack:"6" json:"streaming_output,omitempty"`

	// StreamingOutputTruncated indicates if streaming output was truncated.
	StreamingOutputTruncated bool `msgpack:"7" json:"streaming_output_truncated,omitempty"`

	// Result contains the final tool result (on success).
	Result *ToolCallResult `msgpack:"8" json:"result,omitempty"`

	// Error contains error details (on failure).
	Error *ToolCallError `msgpack:"9" json:"error,omitempty"`

	// DurationMs is the execution duration in milliseconds.
	DurationMs int64 `msgpack:"10" json:"duration_ms,omitempty"`
}

// ToolCallResult captures successful tool execution.
type ToolCallResult struct {
	// Content is the tool output (may be truncated for display).
	Content string `msgpack:"1" json:"content"`

	// ContentTruncated indicates if Content was truncated.
	ContentTruncated bool `msgpack:"2" json:"content_truncated,omitempty"`

	// Success indicates whether the tool completed successfully.
	Success bool `msgpack:"3" json:"success"`

	// ExitCode is the exit code for shell commands (nil if not applicable).
	ExitCode *int `msgpack:"4" json:"exit_code,omitempty"`
}

// ToolCallError captures failed tool execution.
type ToolCallError struct {
	// Code is a machine-readable error code.
	Code string `msgpack:"1" json:"code,omitempty"`

	// Message is a human-readable error description.
	Message string `msgpack:"2" json:"message"`

	// ExitCode is the exit code for shell commands.
	ExitCode *int `msgpack:"3" json:"exit_code,omitempty"`
}

// TurnMetrics captures token usage and timing for a turn.
type TurnMetrics struct {
	// InputTokens is the number of input/prompt tokens.
	InputTokens int64 `msgpack:"1" json:"input_tokens"`

	// OutputTokens is the number of output/completion tokens.
	OutputTokens int64 `msgpack:"2" json:"output_tokens"`

	// TotalTokens is InputTokens + OutputTokens.
	TotalTokens int64 `msgpack:"3" json:"total_tokens"`

	// CachedTokens is the number of cached input tokens (if applicable).
	CachedTokens *int64 `msgpack:"4" json:"cached_tokens,omitempty"`

	// ReasoningTokens is tokens used for extended thinking (if applicable).
	ReasoningTokens *int64 `msgpack:"5" json:"reasoning_tokens,omitempty"`

	// DurationMs is the total turn duration in milliseconds.
	DurationMs *int64 `msgpack:"6" json:"duration_ms,omitempty"`

	// Model is the model used for this turn.
	Model string `msgpack:"7" json:"model,omitempty"`

	// UsageStatus tags non-happy-path usage parses for OTEL emit.
	// Empty / missing on the happy "Reported" path; "not_reported" when
	// the stream finished cleanly with no usage object;
	// "error:<class>" when the finalize path classified the call as an
	// upstream error. Additive-only field; pre-existing records leave
	// it unset.
	UsageStatus string `msgpack:"8" json:"usage_status,omitempty"`
}

// =============================================================================
// System Message
// =============================================================================

// SystemMessage represents a system-level message or event.
type SystemMessage struct {
	// Kind categorizes this system message.
	Kind SystemKind `msgpack:"1" json:"kind"`

	// Title is a short summary (optional).
	Title string `msgpack:"2" json:"title,omitempty"`

	// Content is the message content.
	Content string `msgpack:"3" json:"content"`
}

// =============================================================================
// Handoff
// =============================================================================

// HandoffInfo captures agent-to-agent handoff details.
type HandoffInfo struct {
	// FromAgent is the source agent name.
	FromAgent string `msgpack:"1" json:"from_agent"`

	// ToAgent is the destination agent name.
	ToAgent string `msgpack:"2" json:"to_agent"`

	// ToolName is the handoff tool that was invoked.
	ToolName string `msgpack:"3" json:"tool_name,omitempty"`

	// Input is the input passed to the target agent.
	Input string `msgpack:"4" json:"input,omitempty"`

	// Reason explains why the handoff occurred.
	Reason string `msgpack:"5" json:"reason,omitempty"`
}

// =============================================================================
// Legacy Types (v1 schema - kept for backward compatibility)
// =============================================================================

// Assistant represents an assistant response (legacy flat type).
// Deprecated: Use AssistantTurn for new code.
type Assistant struct {
	// Text is the assistant's response text.
	Text string `msgpack:"1" json:"text"`

	// Reasoning contains extended thinking/reasoning output (if enabled).
	Reasoning string `msgpack:"2" json:"reasoning,omitempty"`

	// Model is the model that generated this response.
	Model string `msgpack:"3" json:"model,omitempty"`

	// InputTokens is the number of input tokens used.
	InputTokens int64 `msgpack:"4" json:"input_tokens,omitempty"`

	// OutputTokens is the number of output tokens generated.
	OutputTokens int64 `msgpack:"5" json:"output_tokens,omitempty"`

	// StopReason indicates why generation stopped.
	StopReason string `msgpack:"6" json:"stop_reason,omitempty"`
}

// ToolCall represents a tool invocation request from the assistant (legacy flat type).
// Deprecated: Use AssistantTurn.ToolCalls for new code.
type ToolCall struct {
	// CallID is the unique identifier for this tool call.
	CallID string `msgpack:"1" json:"call_id"`

	// Name is the tool/function name being invoked.
	Name string `msgpack:"2" json:"name"`

	// Args contains the JSON-encoded tool arguments.
	Args string `msgpack:"3" json:"args"`

	// Description is a human-readable description of what the tool is doing.
	// E.g., "Running `ls -la` in /tmp" or "Reading config.yaml"
	Description string `msgpack:"4" json:"description,omitempty"`
}

// ToolResult represents the outcome of a tool invocation (legacy flat type).
// Deprecated: Use AssistantTurn.ToolCalls with Result/Error for new code.
type ToolResult struct {
	// CallID links this result to its corresponding ToolCall.
	CallID string `msgpack:"1" json:"call_id"`

	// Content is the tool's output.
	Content string `msgpack:"2" json:"content"`

	// IsError indicates if the tool execution failed.
	IsError bool `msgpack:"3" json:"is_error"`

	// ExitCode is the exit code for shell commands (nil if not applicable).
	ExitCode *int `msgpack:"4" json:"exit_code,omitempty"`

	// StreamingOutput contains accumulated real-time output (e.g., shell output).
	// This may be populated during streaming before Content is finalized.
	StreamingOutput string `msgpack:"5" json:"streaming_output,omitempty"`

	// OutputTruncated indicates if the output was truncated due to size limits.
	OutputTruncated bool `msgpack:"6" json:"output_truncated,omitempty"`

	// DurationMs is the execution duration in milliseconds.
	DurationMs int64 `msgpack:"7" json:"duration_ms,omitempty"`
}

// =============================================================================
// Context Metadata
// =============================================================================

// ContextMetadata contains optional metadata about a context.
// By convention, this is included in the first turn of a context.
// The server extracts and caches this for efficient context listing.
type ContextMetadata struct {
	// ClientTag identifies the client that created this context (e.g., "claude-code", "dotrunner").
	ClientTag string `msgpack:"1" json:"client_tag,omitempty"`

	// Title is a human-readable name for this context.
	Title string `msgpack:"2" json:"title,omitempty"`

	// Labels are arbitrary tags for organization/filtering.
	Labels []string `msgpack:"3" json:"labels,omitempty"`

	// Custom contains arbitrary key-value metadata.
	Custom map[string]string `msgpack:"4" json:"custom,omitempty"`

	// Tenant labels the context for OTEL `app.tenant` cost attribution.
	// Empty / missing means no tenant — the OTEL emit path MUST omit the
	// attribute entirely (no sentinel, no empty-string stamp).
	Tenant string `msgpack:"5" json:"tenant,omitempty"`

	// Provenance captures the origin story of this context.
	// Includes process identity, user identity, trace context, and more.
	// See Provenance type for full documentation.
	Provenance *Provenance `msgpack:"10" json:"provenance,omitempty"`
}

// =============================================================================
// Utility Functions
// =============================================================================

// Now returns the current time as Unix milliseconds.
func Now() int64 {
	return time.Now().UnixMilli()
}
