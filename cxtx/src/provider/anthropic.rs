use serde_json::Value;
use std::collections::BTreeMap;

use crate::provider::{ExchangeState, PreparedExchange};
use crate::session::SessionRuntime;
use crate::turns::{tool_call_record, ArtifactRefs, HistoryItem, TurnEnvelope};

pub use crate::provider::openai::{parse_sse_buffer, SseFrame};

#[derive(Debug)]
pub struct AnthropicExchange {
    pub exchange_id: String,
    pub model: Option<String>,
    blocks: BTreeMap<usize, PartialBlock>,
    finish_reason: Option<String>,
    parse_errors: Vec<String>,
}

#[derive(Debug, Clone)]
enum PartialBlock {
    Text(String),
    ToolUse(PartialToolUse),
}

#[derive(Debug, Clone, Default)]
struct PartialToolUse {
    id: String,
    name: String,
    input_json: String,
}

pub fn prepare_exchange(
    session: &SessionRuntime,
    exchange_id: String,
    body: &[u8],
    artifact_refs: &ArtifactRefs,
) -> PreparedExchange {
    let payload = match serde_json::from_slice::<Value>(body) {
        Ok(payload) => payload,
        Err(err) => {
            let turns = vec![session.provider_error_turn(
                &exchange_id,
                "request_parse_error",
                &format!("failed to parse Anthropic request body: {err}"),
                None,
                artifact_refs,
            )];
            return PreparedExchange {
                exchange_id: exchange_id.clone(),
                model: None,
                request_turns: turns,
                state: ExchangeState::Anthropic(AnthropicExchange::new(exchange_id, None)),
            };
        }
    };

    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let request_turns = match parse_request_history(&payload) {
        Ok(history) => session.observe_request_history(&exchange_id, history, artifact_refs),
        Err(err) => vec![session.provider_error_turn(
            &exchange_id,
            "request_history_parse_error",
            &err,
            None,
            artifact_refs,
        )],
    };

    PreparedExchange {
        exchange_id: exchange_id.clone(),
        model: model.clone(),
        request_turns,
        state: ExchangeState::Anthropic(AnthropicExchange::new(exchange_id, model)),
    }
}

pub fn finalize_json(
    session: &SessionRuntime,
    exchange: AnthropicExchange,
    status: u16,
    request_id: Option<String>,
    body: &[u8],
    artifact_refs: &ArtifactRefs,
) -> Vec<TurnEnvelope> {
    if status >= 400 {
        let body_excerpt = String::from_utf8_lossy(body);
        return vec![session.provider_error_turn(
            &exchange.exchange_id,
            "provider_error_response",
            &format!(
                "Anthropic upstream returned HTTP {status}: {}",
                body_excerpt.trim()
            ),
            request_id.as_deref(),
            artifact_refs,
        )];
    }

    let payload = match serde_json::from_slice::<Value>(body) {
        Ok(payload) => payload,
        Err(err) => {
            return vec![session.provider_error_turn(
                &exchange.exchange_id,
                "response_parse_error",
                &format!("failed to parse Anthropic response body: {err}"),
                request_id.as_deref(),
                artifact_refs,
            )];
        }
    };

    match parse_assistant_content(
        payload.get("content").unwrap_or(&Value::Null),
        payload
            .get("model")
            .and_then(Value::as_str)
            .or(exchange.model.as_deref()),
        payload.get("stop_reason").and_then(Value::as_str),
    ) {
        Ok(Some(item)) => vec![session.append_history_item(&exchange.exchange_id, item)],
        Ok(None) => Vec::new(),
        Err(err) => vec![session.provider_error_turn(
            &exchange.exchange_id,
            "response_extract_error",
            &err,
            request_id.as_deref(),
            artifact_refs,
        )],
    }
}

pub fn finalize_stream(
    session: &SessionRuntime,
    exchange: AnthropicExchange,
    status: u16,
    request_id: Option<String>,
    artifact_refs: &ArtifactRefs,
    malformed_remainder: Option<String>,
) -> Vec<TurnEnvelope> {
    if let Some(remainder) = malformed_remainder.filter(|remainder| !remainder.trim().is_empty()) {
        return vec![session.provider_error_turn(
            &exchange.exchange_id,
            "malformed_sse_remainder",
            &format!("Anthropic stream ended with leftover buffer: {remainder}"),
            request_id.as_deref(),
            artifact_refs,
        )];
    }

    if !exchange.parse_errors.is_empty() {
        return vec![session.provider_error_turn(
            &exchange.exchange_id,
            "stream_parse_error",
            &exchange.parse_errors.join("; "),
            request_id.as_deref(),
            artifact_refs,
        )];
    }

    if status >= 400 {
        return vec![session.provider_error_turn(
            &exchange.exchange_id,
            "provider_error_stream",
            &format!("Anthropic upstream returned HTTP {status} during stream"),
            request_id.as_deref(),
            artifact_refs,
        )];
    }

    let mut blocks = exchange.blocks.into_iter().collect::<Vec<_>>();
    blocks.sort_by_key(|(index, _)| *index);
    if blocks.is_empty() {
        return Vec::new();
    }

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for (_, block) in blocks {
        match block {
            PartialBlock::Text(value) => text.push_str(&value),
            PartialBlock::ToolUse(tool) => {
                tool_calls.push(tool_call_record(tool.id, tool.name, tool.input_json))
            }
        }
    }

    vec![session.append_history_item(
        &exchange.exchange_id,
        HistoryItem::AssistantTurn {
            text,
            tool_calls,
            model: exchange.model,
            finish_reason: exchange.finish_reason,
        },
    )]
}

impl AnthropicExchange {
    fn new(exchange_id: String, model: Option<String>) -> Self {
        Self {
            exchange_id,
            model,
            blocks: BTreeMap::new(),
            finish_reason: None,
            parse_errors: Vec::new(),
        }
    }

    pub fn absorb_sse_frame(&mut self, frame: &SseFrame) {
        let payload = match serde_json::from_str::<Value>(&frame.data) {
            Ok(payload) => payload,
            Err(err) => {
                self.parse_errors
                    .push(format!("failed to parse Anthropic stream frame: {err}"));
                return;
            }
        };

        match frame.event.as_deref() {
            Some("message_start") => {
                if self.model.is_none() {
                    self.model = payload
                        .get("message")
                        .and_then(|message| message.get("model"))
                        .and_then(Value::as_str)
                        .map(|value| value.to_string());
                }
            }
            Some("content_block_start") => {
                let index = payload.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let block = payload.get("content_block").unwrap_or(&Value::Null);
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        self.blocks.insert(
                            index,
                            PartialBlock::Text(
                                block
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                        );
                    }
                    Some("tool_use") => {
                        self.blocks.insert(
                            index,
                            PartialBlock::ToolUse(PartialToolUse {
                                id: block
                                    .get("id")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                name: block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                input_json: block
                                    .get("input")
                                    .map(jsonish_to_string)
                                    .unwrap_or_default(),
                            }),
                        );
                    }
                    _ => {}
                }
            }
            Some("content_block_delta") => {
                let index = payload.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                if let Some(delta) = payload.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = delta
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let entry = self
                                .blocks
                                .entry(index)
                                .or_insert_with(|| PartialBlock::Text(String::new()));
                            if let PartialBlock::Text(value) = entry {
                                value.push_str(text);
                            }
                        }
                        Some("input_json_delta") => {
                            let partial = delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let entry = self.blocks.entry(index).or_insert_with(|| {
                                PartialBlock::ToolUse(PartialToolUse::default())
                            });
                            if let PartialBlock::ToolUse(value) = entry {
                                value.input_json.push_str(partial);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("message_delta") => {
                if self.finish_reason.is_none() {
                    self.finish_reason = payload
                        .get("delta")
                        .and_then(|delta| delta.get("stop_reason"))
                        .and_then(Value::as_str)
                        .map(|value| value.to_string());
                }
            }
            _ => {}
        }
    }
}

fn parse_request_history(payload: &Value) -> Result<Vec<HistoryItem>, String> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "Anthropic request is missing messages array".to_string())?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    let mut history = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "Anthropic request message missing role".to_string())?;
        let content = message.get("content").unwrap_or(&Value::Null);
        match role {
            "user" => history.extend(parse_user_blocks(content)?),
            "assistant" => {
                if let Some(item) = parse_assistant_content(content, model.as_deref(), None)? {
                    history.push(item);
                }
            }
            _ => {}
        }
    }
    Ok(history)
}

fn parse_user_blocks(content: &Value) -> Result<Vec<HistoryItem>, String> {
    match content {
        Value::String(value) => Ok(vec![HistoryItem::UserInput {
            text: value.clone(),
            files: Vec::new(),
        }]),
        Value::Array(blocks) => {
            let mut history = Vec::new();
            let mut text_buffer = String::new();
            let content_start = blocks
                .iter()
                .enumerate()
                .find_map(|(index, block)| (!is_bootstrap_user_block(block)).then_some(index))
                .unwrap_or(blocks.len());
            for block in blocks.iter().skip(content_start) {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if !text_buffer.is_empty() {
                            text_buffer.push('\n');
                        }
                        text_buffer.push_str(text);
                    }
                    Some("tool_result") => {
                        if !text_buffer.is_empty() {
                            history.push(HistoryItem::UserInput {
                                text: std::mem::take(&mut text_buffer),
                                files: Vec::new(),
                            });
                        }
                        history.push(HistoryItem::ToolResult {
                            call_id: block
                                .get("tool_use_id")
                                .or_else(|| block.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            content: block
                                .get("content")
                                .map(content_to_text)
                                .unwrap_or_default(),
                            is_error: block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        });
                    }
                    _ => {}
                }
            }
            if !text_buffer.is_empty() {
                history.push(HistoryItem::UserInput {
                    text: text_buffer,
                    files: Vec::new(),
                });
            }
            Ok(history)
        }
        _ => Ok(Vec::new()),
    }
}

fn is_bootstrap_user_block(block: &Value) -> bool {
    if block.get("type").and_then(Value::as_str) != Some("text") {
        return false;
    }

    let text = block
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_start();

    text.starts_with("<system-reminder>")
        && (text.contains("SessionStart hook additional context")
            || text.contains("The following skills are available for use with the Skill tool:")
            || text.contains("As you answer the user's questions, you can use the following context:"))
}

fn parse_assistant_content(
    content: &Value,
    model: Option<&str>,
    finish_reason: Option<&str>,
) -> Result<Option<HistoryItem>, String> {
    match content {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(HistoryItem::AssistantTurn {
            text: value.clone(),
            tool_calls: Vec::new(),
            model: model.map(|value| value.to_string()),
            finish_reason: finish_reason.map(|value| value.to_string()),
        })),
        Value::Array(blocks) => {
            let mut text = String::new();
            let mut tool_calls = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => text.push_str(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                    Some("tool_use") => tool_calls.push(tool_call_record(
                        block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        block
                            .get("input")
                            .map(jsonish_to_string)
                            .unwrap_or_default(),
                    )),
                    _ => {}
                }
            }
            Ok(Some(HistoryItem::AssistantTurn {
                text,
                tool_calls,
                model: model.map(|value| value.to_string()),
                finish_reason: finish_reason.map(|value| value.to_string()),
            }))
        }
        _ => Err("unsupported Anthropic content shape".to_string()),
    }
}

fn content_to_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .iter()
            .map(content_to_text)
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(|value| value.to_string())
            .or_else(|| map.get("content").map(content_to_text))
            .unwrap_or_else(|| jsonish_to_string(value)),
        _ => jsonish_to_string(value),
    }
}

fn jsonish_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_request_history, parse_sse_buffer, AnthropicExchange, SseFrame};
    use crate::turns::HistoryItem;
    use serde_json::json;

    #[test]
    fn parses_anthropic_sse_frames() {
        let mut buffer = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\"}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\"}\n\n"
        )
        .to_string();
        let frames = parse_sse_buffer(&mut buffer);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].event.as_deref(), Some("message_start"));
        assert_eq!(frames[1].event.as_deref(), Some("message_delta"));
        assert!(buffer.is_empty());
    }

    #[test]
    fn request_history_extracts_user_and_tool_result_blocks() {
        struct Case {
            name: &'static str,
            payload: serde_json::Value,
            expected_kinds: Vec<&'static str>,
        }

        let cases = vec![
            Case {
                name: "user blocks plus assistant tool use",
                payload: json!({
                    "model": "claude-sonnet",
                    "messages": [
                        {"role": "user", "content": [{"type": "text", "text": "hello"}, {"type": "tool_result", "tool_use_id": "call_1", "content": [{"type": "text", "text": "done"}]}]},
                        {"role": "assistant", "content": [{"type": "text", "text": "working"}, {"type": "tool_use", "id": "call_1", "name": "lookup", "input": {"q": "hello"}}]}
                    ]
                }),
                expected_kinds: vec!["user_input", "tool_result", "assistant_turn"],
            },
            Case {
                name: "string user content stays a single user turn",
                payload: json!({
                    "model": "claude-sonnet",
                    "messages": [
                        {"role": "user", "content": "hello"},
                        {"role": "assistant", "content": [{"type": "text", "text": "hi"}]}
                    ]
                }),
                expected_kinds: vec!["user_input", "assistant_turn"],
            },
            Case {
                name: "leading bootstrap reminders are skipped before the real prompt",
                payload: json!({
                    "model": "claude-sonnet",
                    "messages": [
                        {"role": "user", "content": [
                            {"type": "text", "text": "<system-reminder>\nSessionStart hook additional context: ..."},
                            {"type": "text", "text": "<system-reminder>\nThe following skills are available for use with the Skill tool:\n..."},
                            {"type": "text", "text": "<system-reminder>\nAs you answer the user's questions, you can use the following context:\n..."},
                            {"type": "text", "text": "real prompt"}
                        ]},
                        {"role": "assistant", "content": [{"type": "text", "text": "done"}]}
                    ]
                }),
                expected_kinds: vec!["user_input", "assistant_turn"],
            },
        ];

        for case in cases {
            let history = parse_request_history(&case.payload).unwrap();
            let kinds = history
                .iter()
                .map(|item| match item {
                    HistoryItem::UserInput { .. } => "user_input",
                    HistoryItem::AssistantTurn { .. } => "assistant_turn",
                    HistoryItem::ToolResult { .. } => "tool_result",
                })
                .collect::<Vec<_>>();
            assert_eq!(kinds, case.expected_kinds, "case {}", case.name);
            if case.name == "leading bootstrap reminders are skipped before the real prompt" {
                match &history[0] {
                    HistoryItem::UserInput { text, .. } => assert_eq!(text, "real prompt"),
                    other => panic!("expected user input, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn stream_accumulator_collects_text_and_tool_use_blocks() {
        let mut exchange =
            AnthropicExchange::new("exchange-0001".to_string(), Some("claude".to_string()));
        exchange.absorb_sse_frame(&SseFrame {
            event: Some("content_block_start".to_string()),
            data: "{\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"hel\"}}"
                .to_string(),
            raw: String::new(),
        });
        exchange.absorb_sse_frame(&SseFrame {
            event: Some("content_block_delta".to_string()),
            data: "{\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}".to_string(),
            raw: String::new(),
        });
        exchange.absorb_sse_frame(&SseFrame {
            event: Some("content_block_start".to_string()),
            data: "{\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_1\",\"name\":\"lookup\"}}".to_string(),
            raw: String::new(),
        });
        exchange.absorb_sse_frame(&SseFrame {
            event: Some("content_block_delta".to_string()),
            data: "{\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\":\\\"hello\\\"}\"}}".to_string(),
            raw: String::new(),
        });
        assert_eq!(exchange.blocks.len(), 2);
    }
}
