// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

#![allow(non_upper_case_globals)]

use serde::{Deserialize, Serialize};

pub const TypeIDConversationItem: &str = "cxdb.ConversationItem";
pub const TypeVersionConversationItem: u32 = 3;
pub const TypeIDConversationItemLegacy: &str = "cxdb.v3:ConversationItem";

pub type ItemType = String;

pub const ItemTypeUserInput: &str = "user_input";
pub const ItemTypeAssistantTurn: &str = "assistant_turn";
pub const ItemTypeSystem: &str = "system";
pub const ItemTypeHandoff: &str = "handoff";
pub const ItemTypeAssistant: &str = "assistant";
pub const ItemTypeToolCall: &str = "tool_call";
pub const ItemTypeToolResult: &str = "tool_result";

pub type ItemStatus = String;

pub const ItemStatusPending: &str = "pending";
pub const ItemStatusStreaming: &str = "streaming";
pub const ItemStatusComplete: &str = "complete";
pub const ItemStatusError: &str = "error";
pub const ItemStatusCancelled: &str = "cancelled";

pub type ToolCallStatus = String;

pub const ToolCallStatusPending: &str = "pending";
pub const ToolCallStatusExecuting: &str = "executing";
pub const ToolCallStatusComplete: &str = "complete";
pub const ToolCallStatusError: &str = "error";
pub const ToolCallStatusSkipped: &str = "skipped";

pub type SystemKind = String;

pub const SystemKindInfo: &str = "info";
pub const SystemKindWarning: &str = "warning";
pub const SystemKindError: &str = "error";
pub const SystemKindGuardrail: &str = "guardrail";
pub const SystemKindRateLimit: &str = "rate_limit";
pub const SystemKindRewind: &str = "rewind";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationItem {
    #[serde(rename = "1")]
    pub item_type: ItemType,
    #[serde(rename = "2", skip_serializing_if = "String::is_empty")]
    pub status: ItemStatus,
    #[serde(rename = "3", skip_serializing_if = "is_zero_i64")]
    pub timestamp: i64,
    #[serde(rename = "4", skip_serializing_if = "String::is_empty")]
    pub id: String,

    #[serde(rename = "10")]
    pub user_input: Option<UserInput>,
    #[serde(rename = "11")]
    pub turn: Option<AssistantTurn>,
    #[serde(rename = "12")]
    pub system: Option<SystemMessage>,
    #[serde(rename = "13")]
    pub handoff: Option<HandoffInfo>,

    #[serde(rename = "20")]
    pub assistant: Option<Assistant>,
    #[serde(rename = "21")]
    pub tool_call: Option<ToolCall>,
    #[serde(rename = "22")]
    pub tool_result: Option<ToolResult>,

    #[serde(rename = "30")]
    pub context_metadata: Option<ContextMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserInput {
    #[serde(rename = "1")]
    pub text: String,
    #[serde(rename = "2", skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AssistantTurn {
    #[serde(rename = "1")]
    pub text: String,
    #[serde(rename = "2", skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallItem>,
    #[serde(rename = "3", skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    #[serde(rename = "4")]
    pub metrics: Option<TurnMetrics>,
    #[serde(rename = "5", skip_serializing_if = "String::is_empty")]
    pub agent: String,
    #[serde(rename = "6", skip_serializing_if = "is_zero_i64")]
    pub turn_number: i64,
    #[serde(rename = "7", skip_serializing_if = "is_zero_i64")]
    pub max_turns: i64,
    #[serde(rename = "8", skip_serializing_if = "String::is_empty")]
    pub finish_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallItem {
    #[serde(rename = "1")]
    pub id: String,
    #[serde(rename = "2")]
    pub name: String,
    #[serde(rename = "3")]
    pub args: String,
    #[serde(rename = "4")]
    pub status: ToolCallStatus,
    #[serde(rename = "5", skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(rename = "6", skip_serializing_if = "String::is_empty")]
    pub streaming_output: String,
    #[serde(rename = "7", skip_serializing_if = "is_false")]
    pub streaming_output_truncated: bool,
    #[serde(rename = "8")]
    pub result: Option<ToolCallResult>,
    #[serde(rename = "9")]
    pub error: Option<ToolCallError>,
    #[serde(rename = "10", skip_serializing_if = "is_zero_i64")]
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallResult {
    #[serde(rename = "1")]
    pub content: String,
    #[serde(rename = "2", skip_serializing_if = "is_false")]
    pub content_truncated: bool,
    #[serde(rename = "3")]
    pub success: bool,
    #[serde(rename = "4")]
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCallError {
    #[serde(rename = "1", skip_serializing_if = "String::is_empty")]
    pub code: String,
    #[serde(rename = "2")]
    pub message: String,
    #[serde(rename = "3")]
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TurnMetrics {
    #[serde(rename = "1")]
    pub input_tokens: i64,
    #[serde(rename = "2")]
    pub output_tokens: i64,
    #[serde(rename = "3")]
    pub total_tokens: i64,
    #[serde(rename = "4")]
    pub cached_tokens: Option<i64>,
    #[serde(rename = "5")]
    pub reasoning_tokens: Option<i64>,
    #[serde(rename = "6")]
    pub duration_ms: Option<i64>,
    #[serde(rename = "7", skip_serializing_if = "String::is_empty")]
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemMessage {
    #[serde(rename = "1")]
    pub kind: SystemKind,
    #[serde(rename = "2", skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(rename = "3")]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffInfo {
    #[serde(rename = "1")]
    pub from_agent: String,
    #[serde(rename = "2")]
    pub to_agent: String,
    #[serde(rename = "3", skip_serializing_if = "String::is_empty")]
    pub tool_name: String,
    #[serde(rename = "4", skip_serializing_if = "String::is_empty")]
    pub input: String,
    #[serde(rename = "5", skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Assistant {
    #[serde(rename = "1")]
    pub text: String,
    #[serde(rename = "2", skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    #[serde(rename = "3", skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(rename = "4", skip_serializing_if = "is_zero_i64")]
    pub input_tokens: i64,
    #[serde(rename = "5", skip_serializing_if = "is_zero_i64")]
    pub output_tokens: i64,
    #[serde(rename = "6", skip_serializing_if = "String::is_empty")]
    pub stop_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    #[serde(rename = "1")]
    pub call_id: String,
    #[serde(rename = "2")]
    pub name: String,
    #[serde(rename = "3")]
    pub args: String,
    #[serde(rename = "4", skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    #[serde(rename = "1")]
    pub call_id: String,
    #[serde(rename = "2")]
    pub content: String,
    #[serde(rename = "3")]
    pub is_error: bool,
    #[serde(rename = "4")]
    pub exit_code: Option<i64>,
    #[serde(rename = "5", skip_serializing_if = "String::is_empty")]
    pub streaming_output: String,
    #[serde(rename = "6", skip_serializing_if = "is_false")]
    pub output_truncated: bool,
    #[serde(rename = "7", skip_serializing_if = "is_zero_i64")]
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextMetadata {
    #[serde(rename = "1", skip_serializing_if = "String::is_empty")]
    pub client_tag: String,
    #[serde(rename = "2", skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(rename = "3", skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(rename = "4", skip_serializing_if = "map_is_empty")]
    pub custom: std::collections::HashMap<String, String>,
    #[serde(rename = "10")]
    pub provenance: Option<super::provenance::Provenance>,
}

#[allow(non_snake_case)]
pub fn Now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn map_is_empty(map: &std::collections::HashMap<String, String>) -> bool {
    map.is_empty()
}
