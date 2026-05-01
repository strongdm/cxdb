// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use super::conversation::*;
use super::provenance::Provenance;

impl ConversationItem {
    pub fn with_context_metadata(&mut self, meta: ContextMetadata) -> &mut Self {
        self.context_metadata = Some(meta);
        self
    }

    pub fn with_client_tag(&mut self, tag: impl Into<String>) -> &mut Self {
        if self.context_metadata.is_none() {
            self.context_metadata = Some(ContextMetadata {
                client_tag: String::new(),
                title: String::new(),
                labels: Vec::new(),
                custom: std::collections::HashMap::new(),
                tenant: None,
                provenance: None,
            });
        }
        if let Some(meta) = &mut self.context_metadata {
            meta.client_tag = tag.into();
        }
        self
    }
}

pub fn new_user_input(text: impl Into<String>, files: Vec<String>) -> ConversationItem {
    ConversationItem {
        item_type: ItemTypeUserInput.to_string(),
        status: ItemStatusComplete.to_string(),
        timestamp: Now(),
        id: String::new(),
        user_input: Some(UserInput {
            text: text.into(),
            files,
        }),
        turn: None,
        system: None,
        handoff: None,
        assistant: None,
        tool_call: None,
        tool_result: None,
        context_metadata: None,
    }
}

pub fn new_assistant_turn(text: impl Into<String>) -> ConversationItem {
    ConversationItem {
        item_type: ItemTypeAssistantTurn.to_string(),
        status: ItemStatusComplete.to_string(),
        timestamp: Now(),
        id: String::new(),
        user_input: None,
        turn: Some(AssistantTurn {
            text: text.into(),
            tool_calls: Vec::new(),
            reasoning: String::new(),
            metrics: None,
            agent: String::new(),
            turn_number: 0,
            max_turns: 0,
            finish_reason: String::new(),
        }),
        system: None,
        handoff: None,
        assistant: None,
        tool_call: None,
        tool_result: None,
        context_metadata: None,
    }
}

pub struct AssistantTurnBuilder {
    item: ConversationItem,
}

pub fn build_assistant_turn(text: impl Into<String>) -> AssistantTurnBuilder {
    AssistantTurnBuilder {
        item: new_assistant_turn(text),
    }
}

impl AssistantTurnBuilder {
    pub fn with_reasoning(&mut self, reasoning: impl Into<String>) -> &mut Self {
        if let Some(turn) = &mut self.item.turn {
            turn.reasoning = reasoning.into();
        }
        self
    }

    pub fn with_agent(&mut self, agent: impl Into<String>) -> &mut Self {
        if let Some(turn) = &mut self.item.turn {
            turn.agent = agent.into();
        }
        self
    }

    pub fn with_turn_number(&mut self, turn: i64, max_turns: i64) -> &mut Self {
        if let Some(turn_item) = &mut self.item.turn {
            turn_item.turn_number = turn;
            turn_item.max_turns = max_turns;
        }
        self
    }

    pub fn with_finish_reason(&mut self, reason: impl Into<String>) -> &mut Self {
        if let Some(turn) = &mut self.item.turn {
            turn.finish_reason = reason.into();
        }
        self
    }

    pub fn with_metrics(&mut self, input_tokens: i64, output_tokens: i64) -> &mut Self {
        if let Some(turn) = &mut self.item.turn {
            turn.metrics = Some(TurnMetrics {
                input_tokens,
                output_tokens,
                total_tokens: input_tokens + output_tokens,
                cached_tokens: None,
                reasoning_tokens: None,
                duration_ms: None,
                model: String::new(),
                usage_status: None,
            });
        }
        self
    }

    pub fn with_full_metrics(&mut self, metrics: TurnMetrics) -> &mut Self {
        if let Some(turn) = &mut self.item.turn {
            turn.metrics = Some(metrics);
        }
        self
    }

    pub fn with_tool_call(&mut self, tool_call: ToolCallItem) -> &mut Self {
        if let Some(turn) = &mut self.item.turn {
            turn.tool_calls.push(tool_call);
        }
        self
    }

    pub fn with_status(&mut self, status: impl Into<String>) -> &mut Self {
        self.item.status = status.into();
        self
    }

    pub fn with_id(&mut self, id: impl Into<String>) -> &mut Self {
        self.item.id = id.into();
        self
    }

    pub fn with_context_metadata(&mut self, meta: ContextMetadata) -> &mut Self {
        self.item.context_metadata = Some(meta);
        self
    }

    pub fn with_client_tag(&mut self, tag: impl Into<String>) -> &mut Self {
        self.item.with_client_tag(tag);
        self
    }

    pub fn build(self) -> ConversationItem {
        self.item
    }
}

pub fn new_tool_call_item(
    id: impl Into<String>,
    name: impl Into<String>,
    args: impl Into<String>,
) -> ToolCallItem {
    ToolCallItem {
        id: id.into(),
        name: name.into(),
        args: args.into(),
        status: ToolCallStatusPending.to_string(),
        description: String::new(),
        streaming_output: String::new(),
        streaming_output_truncated: false,
        result: None,
        error: None,
        duration_ms: 0,
    }
}

pub struct ToolCallItemBuilder {
    item: ToolCallItem,
}

pub fn build_tool_call_item(
    id: impl Into<String>,
    name: impl Into<String>,
    args: impl Into<String>,
) -> ToolCallItemBuilder {
    ToolCallItemBuilder {
        item: new_tool_call_item(id, name, args),
    }
}

impl ToolCallItemBuilder {
    pub fn with_description(&mut self, desc: impl Into<String>) -> &mut Self {
        self.item.description = desc.into();
        self
    }

    pub fn with_status(&mut self, status: impl Into<String>) -> &mut Self {
        self.item.status = status.into();
        self
    }

    pub fn with_streaming_output(
        &mut self,
        output: impl Into<String>,
        truncated: bool,
    ) -> &mut Self {
        self.item.streaming_output = output.into();
        self.item.streaming_output_truncated = truncated;
        self
    }

    pub fn with_result(&mut self, content: impl Into<String>, exit_code: Option<i64>) -> &mut Self {
        self.item.status = ToolCallStatusComplete.to_string();
        self.item.result = Some(ToolCallResult {
            content: content.into(),
            content_truncated: false,
            success: true,
            exit_code,
        });
        self
    }

    pub fn with_error(&mut self, message: impl Into<String>, exit_code: Option<i64>) -> &mut Self {
        self.item.status = ToolCallStatusError.to_string();
        self.item.error = Some(ToolCallError {
            code: String::new(),
            message: message.into(),
            exit_code,
        });
        self
    }

    pub fn with_duration(&mut self, duration_ms: i64) -> &mut Self {
        self.item.duration_ms = duration_ms;
        self
    }

    pub fn build(self) -> ToolCallItem {
        self.item
    }
}

pub fn new_handoff(from_agent: impl Into<String>, to_agent: impl Into<String>) -> ConversationItem {
    ConversationItem {
        item_type: ItemTypeHandoff.to_string(),
        status: ItemStatusComplete.to_string(),
        timestamp: Now(),
        id: String::new(),
        user_input: None,
        turn: None,
        system: None,
        handoff: Some(HandoffInfo {
            from_agent: from_agent.into(),
            to_agent: to_agent.into(),
            tool_name: String::new(),
            input: String::new(),
            reason: String::new(),
        }),
        assistant: None,
        tool_call: None,
        tool_result: None,
        context_metadata: None,
    }
}

pub struct HandoffBuilder {
    item: ConversationItem,
}

pub fn build_handoff(from_agent: impl Into<String>, to_agent: impl Into<String>) -> HandoffBuilder {
    HandoffBuilder {
        item: new_handoff(from_agent, to_agent),
    }
}

impl HandoffBuilder {
    pub fn with_tool_name(&mut self, tool_name: impl Into<String>) -> &mut Self {
        if let Some(handoff) = &mut self.item.handoff {
            handoff.tool_name = tool_name.into();
        }
        self
    }

    pub fn with_input(&mut self, input: impl Into<String>) -> &mut Self {
        if let Some(handoff) = &mut self.item.handoff {
            handoff.input = input.into();
        }
        self
    }

    pub fn with_reason(&mut self, reason: impl Into<String>) -> &mut Self {
        if let Some(handoff) = &mut self.item.handoff {
            handoff.reason = reason.into();
        }
        self
    }

    pub fn with_id(&mut self, id: impl Into<String>) -> &mut Self {
        self.item.id = id.into();
        self
    }

    pub fn build(self) -> ConversationItem {
        self.item
    }
}

pub fn new_system_info(content: impl Into<String>) -> ConversationItem {
    new_system_message(SystemKindInfo.to_string(), content)
}

pub fn new_system_warning(content: impl Into<String>) -> ConversationItem {
    new_system_message(SystemKindWarning.to_string(), content)
}

pub fn new_system_error(content: impl Into<String>) -> ConversationItem {
    new_system_message(SystemKindError.to_string(), content)
}

fn new_system_message(kind: String, content: impl Into<String>) -> ConversationItem {
    ConversationItem {
        item_type: ItemTypeSystem.to_string(),
        status: ItemStatusComplete.to_string(),
        timestamp: Now(),
        id: String::new(),
        user_input: None,
        turn: None,
        system: Some(SystemMessage {
            kind,
            title: String::new(),
            content: content.into(),
        }),
        handoff: None,
        assistant: None,
        tool_call: None,
        tool_result: None,
        context_metadata: None,
    }
}

pub struct SystemBuilder {
    item: ConversationItem,
}

pub fn build_system(kind: impl Into<String>, content: impl Into<String>) -> SystemBuilder {
    let kind = kind.into();
    SystemBuilder {
        item: new_system_message(kind, content),
    }
}

impl SystemBuilder {
    pub fn with_title(&mut self, title: impl Into<String>) -> &mut Self {
        if let Some(system) = &mut self.item.system {
            system.title = title.into();
        }
        self
    }

    pub fn with_id(&mut self, id: impl Into<String>) -> &mut Self {
        self.item.id = id.into();
        self
    }

    pub fn build(self) -> ConversationItem {
        self.item
    }
}

pub fn new_assistant(text: impl Into<String>) -> ConversationItem {
    ConversationItem {
        item_type: ItemTypeAssistant.to_string(),
        status: ItemStatusComplete.to_string(),
        timestamp: Now(),
        id: String::new(),
        user_input: None,
        turn: None,
        system: None,
        handoff: None,
        assistant: Some(Assistant {
            text: text.into(),
            reasoning: String::new(),
            model: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: String::new(),
        }),
        tool_call: None,
        tool_result: None,
        context_metadata: None,
    }
}

pub struct AssistantBuilder {
    item: ConversationItem,
}

pub fn build_assistant(text: impl Into<String>) -> AssistantBuilder {
    AssistantBuilder {
        item: new_assistant(text),
    }
}

impl AssistantBuilder {
    pub fn with_reasoning(&mut self, reasoning: impl Into<String>) -> &mut Self {
        if let Some(assistant) = &mut self.item.assistant {
            assistant.reasoning = reasoning.into();
        }
        self
    }

    pub fn with_model(&mut self, model: impl Into<String>) -> &mut Self {
        if let Some(assistant) = &mut self.item.assistant {
            assistant.model = model.into();
        }
        self
    }

    pub fn with_tokens(&mut self, input: i64, output: i64) -> &mut Self {
        if let Some(assistant) = &mut self.item.assistant {
            assistant.input_tokens = input;
            assistant.output_tokens = output;
        }
        self
    }

    pub fn with_stop_reason(&mut self, reason: impl Into<String>) -> &mut Self {
        if let Some(assistant) = &mut self.item.assistant {
            assistant.stop_reason = reason.into();
        }
        self
    }

    pub fn with_status(&mut self, status: impl Into<String>) -> &mut Self {
        self.item.status = status.into();
        self
    }

    pub fn build(self) -> ConversationItem {
        self.item
    }
}

pub fn new_tool_call(
    call_id: impl Into<String>,
    name: impl Into<String>,
    args: impl Into<String>,
) -> ConversationItem {
    ConversationItem {
        item_type: ItemTypeToolCall.to_string(),
        status: ItemStatusPending.to_string(),
        timestamp: Now(),
        id: String::new(),
        user_input: None,
        turn: None,
        system: None,
        handoff: None,
        assistant: None,
        tool_call: Some(ToolCall {
            call_id: call_id.into(),
            name: name.into(),
            args: args.into(),
            description: String::new(),
        }),
        tool_result: None,
        context_metadata: None,
    }
}

pub struct ToolCallBuilder {
    item: ConversationItem,
}

pub fn build_tool_call(
    call_id: impl Into<String>,
    name: impl Into<String>,
    args: impl Into<String>,
) -> ToolCallBuilder {
    ToolCallBuilder {
        item: new_tool_call(call_id, name, args),
    }
}

impl ToolCallBuilder {
    pub fn with_description(&mut self, desc: impl Into<String>) -> &mut Self {
        if let Some(tool_call) = &mut self.item.tool_call {
            tool_call.description = desc.into();
        }
        self
    }

    pub fn with_status(&mut self, status: impl Into<String>) -> &mut Self {
        self.item.status = status.into();
        self
    }

    pub fn build(self) -> ConversationItem {
        self.item
    }
}

pub fn new_tool_result(
    call_id: impl Into<String>,
    content: impl Into<String>,
    is_error: bool,
) -> ConversationItem {
    ConversationItem {
        item_type: ItemTypeToolResult.to_string(),
        status: ItemStatusComplete.to_string(),
        timestamp: Now(),
        id: String::new(),
        user_input: None,
        turn: None,
        system: None,
        handoff: None,
        assistant: None,
        tool_call: None,
        tool_result: Some(ToolResult {
            call_id: call_id.into(),
            content: content.into(),
            is_error,
            exit_code: None,
            streaming_output: String::new(),
            output_truncated: false,
            duration_ms: 0,
        }),
        context_metadata: None,
    }
}

pub struct ToolResultBuilder {
    item: ConversationItem,
}

pub fn build_tool_result(
    call_id: impl Into<String>,
    content: impl Into<String>,
) -> ToolResultBuilder {
    ToolResultBuilder {
        item: new_tool_result(call_id, content, false),
    }
}

impl ToolResultBuilder {
    pub fn with_error(&mut self) -> &mut Self {
        if let Some(result) = &mut self.item.tool_result {
            result.is_error = true;
        }
        self
    }

    pub fn with_exit_code(&mut self, code: i64) -> &mut Self {
        if let Some(result) = &mut self.item.tool_result {
            result.exit_code = Some(code);
        }
        self
    }

    pub fn with_streaming_output(&mut self, output: impl Into<String>) -> &mut Self {
        if let Some(result) = &mut self.item.tool_result {
            result.streaming_output = output.into();
        }
        self
    }

    pub fn with_truncated(&mut self) -> &mut Self {
        if let Some(result) = &mut self.item.tool_result {
            result.output_truncated = true;
        }
        self
    }

    pub fn with_duration(&mut self, duration_ms: i64) -> &mut Self {
        if let Some(result) = &mut self.item.tool_result {
            result.duration_ms = duration_ms;
        }
        self
    }

    pub fn build(self) -> ConversationItem {
        self.item
    }
}

pub fn attach_provenance(meta: &mut ContextMetadata, provenance: Provenance) {
    meta.provenance = Some(provenance);
}
