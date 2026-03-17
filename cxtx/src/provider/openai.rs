use serde_json::Value;

use crate::provider::{ExchangeState, PreparedExchange};
use crate::session::SessionRuntime;
use crate::turns::{tool_call_record, ArtifactRefs, HistoryItem, ToolCallRecord, TurnEnvelope};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub event: Option<String>,
    pub data: String,
    pub raw: String,
}

#[derive(Debug)]
pub struct OpenAiExchange {
    pub exchange_id: String,
    pub model: Option<String>,
    content: String,
    tool_calls: Vec<PartialToolCall>,
    finish_reason: Option<String>,
    parse_errors: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct PartialToolCall {
    call_id: String,
    name: String,
    args: String,
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
                &format!("failed to parse OpenAI request body: {err}"),
                None,
                artifact_refs,
            )];
            return PreparedExchange {
                exchange_id: exchange_id.clone(),
                model: None,
                request_turns: turns,
                state: ExchangeState::OpenAi(OpenAiExchange::new(exchange_id, None)),
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
        state: ExchangeState::OpenAi(OpenAiExchange::new(exchange_id, model)),
    }
}

pub fn finalize_json(
    session: &SessionRuntime,
    exchange: OpenAiExchange,
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
                "OpenAI upstream returned HTTP {status}: {}",
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
                &format!("failed to parse OpenAI response body: {err}"),
                request_id.as_deref(),
                artifact_refs,
            )];
        }
    };

    match parse_assistant_message(
        payload
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .unwrap_or(&Value::Null),
        payload
            .get("model")
            .and_then(Value::as_str)
            .or(exchange.model.as_deref()),
        payload
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("finish_reason"))
            .and_then(Value::as_str),
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
    exchange: OpenAiExchange,
    status: u16,
    request_id: Option<String>,
    artifact_refs: &ArtifactRefs,
    malformed_remainder: Option<String>,
) -> Vec<TurnEnvelope> {
    if let Some(remainder) = malformed_remainder.filter(|remainder| !remainder.trim().is_empty()) {
        return vec![session.provider_error_turn(
            &exchange.exchange_id,
            "malformed_sse_remainder",
            &format!("OpenAI stream ended with leftover buffer: {remainder}"),
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
            &format!("OpenAI upstream returned HTTP {status} during stream"),
            request_id.as_deref(),
            artifact_refs,
        )];
    }

    if exchange.content.is_empty() && exchange.tool_calls.is_empty() {
        return Vec::new();
    }

    let tool_calls = exchange
        .tool_calls
        .into_iter()
        .map(|tool| tool_call_record(tool.call_id, tool.name, tool.args))
        .collect::<Vec<_>>();
    vec![session.append_history_item(
        &exchange.exchange_id,
        HistoryItem::AssistantTurn {
            text: exchange.content,
            tool_calls,
            model: exchange.model,
            finish_reason: exchange.finish_reason,
        },
    )]
}

impl OpenAiExchange {
    fn new(exchange_id: String, model: Option<String>) -> Self {
        Self {
            exchange_id,
            model,
            content: String::new(),
            tool_calls: Vec::new(),
            finish_reason: None,
            parse_errors: Vec::new(),
        }
    }

    pub fn absorb_sse_frame(&mut self, frame: &SseFrame) {
        if frame.data.trim() == "[DONE]" {
            return;
        }
        let payload = match serde_json::from_str::<Value>(&frame.data) {
            Ok(payload) => payload,
            Err(err) => {
                self.parse_errors
                    .push(format!("failed to parse OpenAI stream frame: {err}"));
                return;
            }
        };

        if self.model.is_none() {
            self.model = payload
                .get("model")
                .and_then(Value::as_str)
                .map(|value| value.to_string());
        }

        if let Some(choice) = payload
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        {
            if let Some(delta) = choice.get("delta") {
                if let Some(content) = delta.get("content") {
                    self.content.push_str(&content_to_text(content));
                }
                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tool_call in tool_calls {
                        absorb_tool_call_delta(&mut self.tool_calls, tool_call);
                    }
                }
            }
            if self.finish_reason.is_none() {
                self.finish_reason = choice
                    .get("finish_reason")
                    .and_then(Value::as_str)
                    .map(|value| value.to_string());
            }
        }
    }
}

pub fn parse_sse_buffer(buffer: &mut String) -> Vec<SseFrame> {
    let normalized = buffer.replace("\r\n", "\n");
    let mut frames = Vec::new();
    let mut consumed = 0usize;

    for block in normalized.split_inclusive("\n\n") {
        if !block.ends_with("\n\n") {
            break;
        }
        consumed += block.len();
        if let Some(frame) = parse_block(block.trim_end_matches('\n')) {
            frames.push(frame);
        }
    }

    let remaining = normalized[consumed..].to_string();
    *buffer = remaining;
    frames
}

fn parse_request_history(payload: &Value) -> Result<Vec<HistoryItem>, String> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| "OpenAI request is missing messages array".to_string())?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(|value| value.to_string());

    let mut history = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| "OpenAI request message missing role".to_string())?;
        match role {
            "user" => history.push(HistoryItem::UserInput {
                text: content_to_text(message.get("content").unwrap_or(&Value::Null)),
                files: Vec::new(),
            }),
            "assistant" => {
                history.push(HistoryItem::AssistantTurn {
                    text: content_to_text(message.get("content").unwrap_or(&Value::Null)),
                    tool_calls: parse_tool_calls(message.get("tool_calls")),
                    model: model.clone(),
                    finish_reason: None,
                });
            }
            "tool" => history.push(HistoryItem::ToolResult {
                call_id: message
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                content: content_to_text(message.get("content").unwrap_or(&Value::Null)),
                is_error: false,
            }),
            _ => {}
        }
    }
    Ok(history)
}

fn parse_assistant_message(
    message: &Value,
    model: Option<&str>,
    finish_reason: Option<&str>,
) -> Result<Option<HistoryItem>, String> {
    if message.is_null() {
        return Ok(None);
    }
    Ok(Some(HistoryItem::AssistantTurn {
        text: content_to_text(message.get("content").unwrap_or(&Value::Null)),
        tool_calls: parse_tool_calls(message.get("tool_calls")),
        model: model.map(|value| value.to_string()),
        finish_reason: finish_reason.map(|value| value.to_string()),
    }))
}

fn parse_tool_calls(value: Option<&Value>) -> Vec<ToolCallRecord> {
    value
        .and_then(Value::as_array)
        .map(|tool_calls| {
            tool_calls
                .iter()
                .map(|tool_call| {
                    let args = tool_call
                        .get("function")
                        .and_then(|function| function.get("arguments"))
                        .map(jsonish_to_string)
                        .unwrap_or_default();
                    tool_call_record(
                        tool_call
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        tool_call
                            .get("function")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        args,
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn absorb_tool_call_delta(slots: &mut Vec<PartialToolCall>, delta: &Value) {
    let index = delta
        .get("index")
        .and_then(Value::as_u64)
        .unwrap_or(slots.len() as u64) as usize;
    while slots.len() <= index {
        slots.push(PartialToolCall::default());
    }
    let slot = &mut slots[index];
    if let Some(id) = delta.get("id").and_then(Value::as_str) {
        slot.call_id = id.to_string();
    }
    if let Some(function) = delta.get("function") {
        if let Some(name) = function.get("name").and_then(Value::as_str) {
            slot.name = name.to_string();
        }
        if let Some(arguments) = function.get("arguments") {
            slot.args.push_str(&jsonish_to_string(arguments));
        }
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

fn parse_block(block: &str) -> Option<SseFrame> {
    let mut event = None;
    let mut data_lines = Vec::new();

    for line in block.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start().to_string());
        }
    }

    if data_lines.is_empty() {
        return None;
    }

    Some(SseFrame {
        event,
        data: data_lines.join("\n"),
        raw: block.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::{parse_request_history, parse_sse_buffer, OpenAiExchange, SseFrame};
    use crate::turns::HistoryItem;
    use serde_json::json;

    #[test]
    fn parses_openai_sse_frames() {
        let mut buffer =
            "data: {\"id\":\"evt_1\",\"choices\":[]}\n\ndata: [DONE]\n\nremainder".to_string();
        let frames = parse_sse_buffer(&mut buffer);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "{\"id\":\"evt_1\",\"choices\":[]}");
        assert_eq!(frames[1].data, "[DONE]");
        assert_eq!(buffer, "remainder");
    }

    #[test]
    fn request_history_includes_user_assistant_and_tool_messages() {
        struct Case {
            name: &'static str,
            payload: serde_json::Value,
            expected_kinds: Vec<&'static str>,
        }

        let cases = vec![
            Case {
                name: "string user, assistant, and tool messages",
                payload: json!({
                    "model": "gpt-5",
                    "messages": [
                        {"role": "user", "content": "hello"},
                        {"role": "assistant", "content": "working", "tool_calls": [{"id": "call_1", "function": {"name": "lookup", "arguments": {"q": "hello"}}}]},
                        {"role": "tool", "tool_call_id": "call_1", "content": "done"}
                    ]
                }),
                expected_kinds: vec!["user_input", "assistant_turn", "tool_result"],
            },
            Case {
                name: "array content is preserved as one user turn and one assistant turn",
                payload: json!({
                    "model": "gpt-5",
                    "messages": [
                        {"role": "user", "content": [{"text": "alpha"}, {"text": "beta"}]},
                        {"role": "assistant", "content": [{"text": "gamma"}]}
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
        }
    }

    #[test]
    fn stream_accumulator_collects_text_and_tool_calls() {
        let mut exchange =
            OpenAiExchange::new("exchange-0001".to_string(), Some("gpt-5".to_string()));
        exchange.absorb_sse_frame(&SseFrame {
            event: None,
            data: "{\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}".to_string(),
            raw: String::new(),
        });
        exchange.absorb_sse_frame(&SseFrame {
            event: None,
            data: "{\"choices\":[{\"delta\":{\"content\":\"lo\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\"}}]}}]}".to_string(),
            raw: String::new(),
        });
        exchange.absorb_sse_frame(&SseFrame {
            event: None,
            data: "{\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"hello\\\"}\"}}]}}]}".to_string(),
            raw: String::new(),
        });
        assert_eq!(exchange.content, "hello");
        assert_eq!(exchange.tool_calls.len(), 1);
        assert_eq!(exchange.tool_calls[0].name, "lookup");
    }
}
