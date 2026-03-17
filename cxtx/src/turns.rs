use chrono::{DateTime, Utc};
use cxdb::types::{
    attach_provenance, build_assistant_turn, build_system, build_tool_call_item, build_tool_result,
    capture_process_provenance, new_user_input, with_env_vars, with_on_behalf_of, with_sdk,
    ContextMetadata, ConversationItem, SystemKindError, SystemKindInfo, SystemKindRewind,
    ToolCallStatusPending,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

use crate::provider::ProviderKind;
use crate::session::CapturedSession;

pub const WRAPPER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryItem {
    UserInput {
        text: String,
        files: Vec<String>,
    },
    AssistantTurn {
        text: String,
        tool_calls: Vec<ToolCallRecord>,
        model: Option<String>,
        finish_reason: Option<String>,
    },
    ToolResult {
        call_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCallRecord {
    pub call_id: String,
    pub name: String,
    pub args: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ArtifactRefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_path: Option<String>,
}

impl ArtifactRefs {
    pub fn with_request_path(mut self, path: Option<String>) -> Self {
        self.request_path = path;
        self
    }

    pub fn with_response_path(mut self, path: Option<String>) -> Self {
        self.response_path = path;
        self
    }

    pub fn with_stream_path(mut self, path: Option<String>) -> Self {
        self.stream_path = path;
        self
    }
}

#[derive(Debug, Clone)]
pub struct TurnEnvelope {
    pub ordinal: u64,
    pub item: ConversationItem,
}

pub fn context_metadata(
    provider: ProviderKind,
    session: &CapturedSession,
    allowlisted_env: &BTreeMap<String, String>,
) -> ContextMetadata {
    let mut custom = HashMap::new();
    custom.insert("stable_session_id".to_string(), session.session_id.clone());
    custom.insert(
        "provider_kind".to_string(),
        provider.provider_name().to_string(),
    );
    custom.insert("wrapper_command".to_string(), session.child_command.clone());
    custom.insert("wrapper_version".to_string(), WRAPPER_VERSION.to_string());

    let env_keys = allowlisted_env.keys().cloned().collect::<Vec<_>>();
    let owner = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    let provenance = capture_process_provenance(
        "cxtx",
        WRAPPER_VERSION,
        vec![
            with_on_behalf_of(owner, "cli", ""),
            with_env_vars(Some(env_keys)),
            with_sdk("cxtx", WRAPPER_VERSION),
        ],
    );

    let mut metadata = ContextMetadata {
        client_tag: provider.client_tag().to_string(),
        title: format!(
            "{} {} {}",
            provider.client_tag(),
            session.child_command,
            session.started_at.format("%Y-%m-%dT%H:%M:%SZ")
        ),
        labels: provider.labels(),
        custom,
        provenance: None,
    };
    attach_provenance(&mut metadata, provenance);
    metadata
}

pub fn session_start_item(
    session: &CapturedSession,
    provider: ProviderKind,
    ordinal: u64,
    child_pid: Option<u32>,
    metadata: &ContextMetadata,
) -> ConversationItem {
    let payload = json!({
        "stable_session_id": session.session_id,
        "provider_kind": provider.provider_name(),
        "child_command": session.child_command,
        "child_args": session.child_args,
        "child_pid": child_pid,
        "started_at": session.started_at,
    });
    let mut item = system_item(
        session,
        ordinal,
        "wrapper-session-start",
        SystemKindInfo,
        "session_start",
        payload,
    );
    item.with_context_metadata(metadata.clone());
    item
}

pub fn session_end_item(
    session: &CapturedSession,
    ordinal: u64,
    exit_code: i32,
    success: bool,
) -> ConversationItem {
    system_item(
        session,
        ordinal,
        "wrapper-session-end",
        if success {
            SystemKindInfo
        } else {
            SystemKindError
        },
        "session_end",
        json!({
            "stable_session_id": session.session_id,
            "child_exit_code": exit_code,
            "success": success,
        }),
    )
}

pub fn ingest_state_item(
    session: &CapturedSession,
    ordinal: u64,
    title: &str,
    kind: &str,
    queue_depth: usize,
    error: Option<&str>,
) -> ConversationItem {
    let exchange_id = if title == "ingest_degraded" {
        "wrapper-ingest-degraded"
    } else {
        "wrapper-ingest-recovered"
    };
    system_item(
        session,
        ordinal,
        exchange_id,
        kind,
        title,
        json!({
            "stable_session_id": session.session_id,
            "queue_depth": queue_depth,
            "error": error,
        }),
    )
}

pub fn rewrite_item(
    session: &CapturedSession,
    ordinal: u64,
    exchange_id: &str,
    previous_len: usize,
    new_len: usize,
    artifact_refs: &ArtifactRefs,
) -> ConversationItem {
    system_item(
        session,
        ordinal,
        exchange_id,
        SystemKindRewind,
        "history_rewrite_detected",
        json!({
            "stable_session_id": session.session_id,
            "previous_history_len": previous_len,
            "new_history_len": new_len,
            "artifacts": artifact_refs,
        }),
    )
}

pub fn provider_error_item(
    session: &CapturedSession,
    ordinal: u64,
    exchange_id: &str,
    title: &str,
    message: &str,
    provider_request_id: Option<&str>,
    artifact_refs: &ArtifactRefs,
) -> ConversationItem {
    system_item(
        session,
        ordinal,
        exchange_id,
        SystemKindError,
        title,
        json!({
            "stable_session_id": session.session_id,
            "message": message,
            "provider_request_id": provider_request_id,
            "artifacts": artifact_refs,
        }),
    )
}

pub fn history_item_to_conversation_item(
    session: &CapturedSession,
    ordinal: u64,
    exchange_id: &str,
    item: &HistoryItem,
) -> ConversationItem {
    let id = turn_id(&session.session_id, ordinal, exchange_id);
    match item {
        HistoryItem::UserInput { text, files } => {
            let mut item = new_user_input(text.clone(), files.clone());
            item.id = id;
            item
        }
        HistoryItem::AssistantTurn {
            text,
            tool_calls,
            model,
            finish_reason,
        } => {
            let mut builder = build_assistant_turn(text.clone());
            for tool_call in tool_calls {
                let mut tool_builder = build_tool_call_item(
                    tool_call.call_id.clone(),
                    tool_call.name.clone(),
                    tool_call.args.clone(),
                );
                tool_builder.with_status(ToolCallStatusPending);
                let built = tool_builder.build();
                builder.with_tool_call(built);
            }
            if let Some(reason) = finish_reason.as_ref().filter(|reason| !reason.is_empty()) {
                builder.with_finish_reason(reason.clone());
            }
            if let Some(model) = model.as_ref().filter(|model| !model.is_empty()) {
                builder.with_full_metrics(cxdb::types::TurnMetrics {
                    input_tokens: 0,
                    output_tokens: 0,
                    total_tokens: 0,
                    cached_tokens: None,
                    reasoning_tokens: None,
                    duration_ms: None,
                    model: model.clone(),
                });
            }
            builder.with_id(id);
            builder.build()
        }
        HistoryItem::ToolResult {
            call_id,
            content,
            is_error,
        } => {
            let mut builder = build_tool_result(call_id.clone(), content.clone());
            if *is_error {
                builder.with_error();
            }
            let mut item = builder.build();
            item.id = id;
            item
        }
    }
}

pub fn tool_call_record(call_id: String, name: String, args: String) -> ToolCallRecord {
    ToolCallRecord {
        call_id,
        name,
        args,
    }
}

pub fn preview_text(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        trimmed.to_string()
    } else {
        let mut out = trimmed.chars().take(limit).collect::<String>();
        out.push_str("...");
        out
    }
}

pub fn turn_id(session_id: &str, ordinal: u64, exchange_id: &str) -> String {
    format!("{session_id}:{ordinal}:{exchange_id}")
}

pub fn timestamp_ms(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

fn system_item(
    session: &CapturedSession,
    ordinal: u64,
    exchange_id: &str,
    kind: &str,
    title: &str,
    payload: Value,
) -> ConversationItem {
    let mut builder = build_system(kind.to_string(), pretty_json(payload));
    builder.with_title(title.to_string());
    builder.with_id(turn_id(&session.session_id, ordinal, exchange_id));
    builder.build()
}

fn pretty_json(value: Value) -> String {
    serde_json::to_string_pretty(&value)
        .unwrap_or_else(|_| "{\"message\":\"failed to encode system payload\"}".to_string())
}
