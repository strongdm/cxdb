use anyhow::{anyhow, Context, Result};
use cxdb::types::{
    Assistant, AssistantTurn, ContextMetadata, ConversationItem, HandoffInfo, Provenance,
    SystemMessage, ToolCall, ToolCallError, ToolCallItem, ToolCallResult, ToolResult, TurnMetrics,
    TypeIDConversationItem, TypeVersionConversationItem, UserInput,
};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use url::Url;

const CONVERSATION_REGISTRY_BUNDLE: &str = include_str!("conversation_registry_bundle.json");

#[derive(Debug, Clone)]
pub struct CxdbHttpClient {
    base_url: Url,
    client: Client,
    client_tag: String,
    registry_ready: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub enum CxdbError {
    Retriable(String),
    Permanent(String),
}

#[derive(Debug, Deserialize)]
struct CreateContextResponse {
    context_id: FlexibleId,
}

#[derive(Debug, Deserialize)]
pub struct AppendResponse {
    #[allow(dead_code)]
    turn_id: FlexibleId,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FlexibleId {
    String(String),
    Number(u64),
}

impl FlexibleId {
    fn into_u64(self) -> Result<u64> {
        match self {
            Self::String(value) => value
                .parse::<u64>()
                .with_context(|| format!("failed to parse identifier {value}")),
            Self::Number(value) => Ok(value),
        }
    }
}

impl CxdbHttpClient {
    pub fn new(base_url: Url, client_tag: String) -> Result<Self> {
        let client = Client::builder()
            .build()
            .context("failed to construct reqwest client")?;
        Ok(Self {
            base_url,
            client,
            client_tag,
            registry_ready: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn create_context(&self) -> std::result::Result<u64, CxdbError> {
        let url = self
            .base_url
            .join("/v1/contexts/create")
            .map_err(|err| CxdbError::Permanent(err.to_string()))?;
        let response = self
            .client
            .post(url)
            .header("X-CXDB-Client-Tag", &self.client_tag)
            .json(&json!({ "base_turn_id": "0" }))
            .send()
            .await
            .map_err(classify_reqwest_error)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify_status(status, body));
        }

        let created = response
            .json::<CreateContextResponse>()
            .await
            .map_err(|err| CxdbError::Permanent(err.to_string()))?;
        created
            .context_id
            .into_u64()
            .map_err(|err| CxdbError::Permanent(err.to_string()))
    }

    pub async fn append_turn(
        &self,
        context_id: u64,
        item: &ConversationItem,
    ) -> std::result::Result<AppendResponse, CxdbError> {
        self.ensure_conversation_type_registered().await?;
        let url = self
            .base_url
            .join(&format!("/v1/contexts/{context_id}/append"))
            .map_err(|err| CxdbError::Permanent(err.to_string()))?;
        let response = self
            .client
            .post(url)
            .header("X-CXDB-Client-Tag", &self.client_tag)
            .json(&json!({
                "type_id": TypeIDConversationItem,
                "type_version": TypeVersionConversationItem,
                "data": conversation_item_payload(item),
            }))
            .send()
            .await
            .map_err(classify_reqwest_error)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify_status(status, body));
        }

        response
            .json::<AppendResponse>()
            .await
            .map_err(|err| CxdbError::Permanent(err.to_string()))
    }

    pub async fn list_contexts(&self) -> Result<serde_json::Value> {
        let url = self
            .base_url
            .join("/v1/contexts?include_provenance=1")
            .context("failed to build contexts URL")?;
        let response = self
            .client
            .get(url)
            .header("X-CXDB-Client-Tag", &self.client_tag)
            .send()
            .await
            .context("request to list contexts failed")?;
        if !response.status().is_success() {
            return Err(anyhow!("list contexts failed: {}", response.status()));
        }
        response
            .json()
            .await
            .context("failed to decode list contexts response")
    }

    pub async fn get_provenance(&self, context_id: u64) -> Result<serde_json::Value> {
        let url = self
            .base_url
            .join(&format!("/v1/contexts/{context_id}/provenance"))
            .context("failed to build provenance URL")?;
        let response = self
            .client
            .get(url)
            .header("X-CXDB-Client-Tag", &self.client_tag)
            .send()
            .await
            .context("request to get provenance failed")?;
        if !response.status().is_success() {
            return Err(anyhow!("provenance lookup failed: {}", response.status()));
        }
        response
            .json()
            .await
            .context("failed to decode provenance response")
    }

    async fn ensure_conversation_type_registered(&self) -> std::result::Result<(), CxdbError> {
        if self.registry_ready.load(Ordering::Acquire) {
            return Ok(());
        }

        let descriptor_url = self
            .base_url
            .join(&format!(
                "/v1/registry/types/{TypeIDConversationItem}/versions/{TypeVersionConversationItem}"
            ))
            .map_err(|err| CxdbError::Permanent(err.to_string()))?;
        let response = self
            .client
            .get(descriptor_url)
            .header("X-CXDB-Client-Tag", &self.client_tag)
            .send()
            .await
            .map_err(classify_reqwest_error)?;
        match response.status() {
            status if status.is_success() => {
                self.registry_ready.store(true, Ordering::Release);
                return Ok(());
            }
            StatusCode::NOT_FOUND => {}
            status => {
                let body = response.text().await.unwrap_or_default();
                return Err(classify_status(status, body));
            }
        }

        let bundle: Value = serde_json::from_str(CONVERSATION_REGISTRY_BUNDLE)
            .map_err(|err| CxdbError::Permanent(format!("invalid bundled registry json: {err}")))?;
        let bundle_id = bundle
            .get("bundle_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CxdbError::Permanent("bundled registry missing bundle_id".to_string())
            })?;
        let bundle_url = self
            .base_url
            .join(&format!("/v1/registry/bundles/{bundle_id}"))
            .map_err(|err| CxdbError::Permanent(err.to_string()))?;
        let response = self
            .client
            .put(bundle_url)
            .header("X-CXDB-Client-Tag", &self.client_tag)
            .json(&bundle)
            .send()
            .await
            .map_err(classify_reqwest_error)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify_status(status, body));
        }

        self.registry_ready.store(true, Ordering::Release);
        Ok(())
    }
}

fn classify_reqwest_error(err: reqwest::Error) -> CxdbError {
    if err.is_connect() || err.is_timeout() || err.is_request() {
        CxdbError::Retriable(err.to_string())
    } else {
        CxdbError::Permanent(err.to_string())
    }
}

fn classify_status(status: StatusCode, body: String) -> CxdbError {
    if status.is_server_error() {
        CxdbError::Retriable(format!("{status}: {body}"))
    } else {
        CxdbError::Permanent(format!("{status}: {body}"))
    }
}

fn conversation_item_payload(item: &ConversationItem) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "item_type".to_string(),
        Value::String(item.item_type.clone()),
    );
    if !item.status.is_empty() {
        obj.insert("status".to_string(), Value::String(item.status.clone()));
    }
    if item.timestamp != 0 {
        obj.insert("timestamp".to_string(), Value::from(item.timestamp));
    }
    if !item.id.is_empty() {
        obj.insert("id".to_string(), Value::String(item.id.clone()));
    }
    if let Some(value) = item.user_input.as_ref() {
        obj.insert("user_input".to_string(), user_input_payload(value));
    }
    if let Some(value) = item.turn.as_ref() {
        obj.insert("turn".to_string(), assistant_turn_payload(value));
    }
    if let Some(value) = item.system.as_ref() {
        obj.insert("system".to_string(), system_message_payload(value));
    }
    if let Some(value) = item.handoff.as_ref() {
        obj.insert("handoff".to_string(), handoff_payload(value));
    }
    if let Some(value) = item.assistant.as_ref() {
        obj.insert("assistant".to_string(), assistant_payload(value));
    }
    if let Some(value) = item.tool_call.as_ref() {
        obj.insert("tool_call".to_string(), tool_call_payload(value));
    }
    if let Some(value) = item.tool_result.as_ref() {
        obj.insert("tool_result".to_string(), tool_result_payload(value));
    }
    if let Some(value) = item.context_metadata.as_ref() {
        obj.insert(
            "context_metadata".to_string(),
            context_metadata_payload(value),
        );
    }
    Value::Object(obj)
}

fn user_input_payload(value: &UserInput) -> Value {
    let mut obj = Map::new();
    obj.insert("text".to_string(), Value::String(value.text.clone()));
    if !value.files.is_empty() {
        obj.insert(
            "files".to_string(),
            Value::Array(
                value
                    .files
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    Value::Object(obj)
}

fn assistant_turn_payload(value: &AssistantTurn) -> Value {
    let mut obj = Map::new();
    obj.insert("text".to_string(), Value::String(value.text.clone()));
    if !value.tool_calls.is_empty() {
        obj.insert(
            "tool_calls".to_string(),
            Value::Array(
                value
                    .tool_calls
                    .iter()
                    .map(tool_call_item_payload)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if !value.reasoning.is_empty() {
        obj.insert(
            "reasoning".to_string(),
            Value::String(value.reasoning.clone()),
        );
    }
    if let Some(metrics) = value.metrics.as_ref() {
        obj.insert("metrics".to_string(), turn_metrics_payload(metrics));
    }
    if !value.agent.is_empty() {
        obj.insert("agent".to_string(), Value::String(value.agent.clone()));
    }
    if value.turn_number != 0 {
        obj.insert("turn_number".to_string(), Value::from(value.turn_number));
    }
    if value.max_turns != 0 {
        obj.insert("max_turns".to_string(), Value::from(value.max_turns));
    }
    if !value.finish_reason.is_empty() {
        obj.insert(
            "finish_reason".to_string(),
            Value::String(value.finish_reason.clone()),
        );
    }
    Value::Object(obj)
}

fn tool_call_item_payload(value: &ToolCallItem) -> Value {
    let mut obj = Map::new();
    obj.insert("id".to_string(), Value::String(value.id.clone()));
    obj.insert("name".to_string(), Value::String(value.name.clone()));
    obj.insert("args".to_string(), Value::String(value.args.clone()));
    obj.insert("status".to_string(), Value::String(value.status.clone()));
    if !value.description.is_empty() {
        obj.insert(
            "description".to_string(),
            Value::String(value.description.clone()),
        );
    }
    if !value.streaming_output.is_empty() {
        obj.insert(
            "streaming_output".to_string(),
            Value::String(value.streaming_output.clone()),
        );
    }
    if value.streaming_output_truncated {
        obj.insert("streaming_output_truncated".to_string(), Value::Bool(true));
    }
    if let Some(result) = value.result.as_ref() {
        obj.insert("result".to_string(), tool_call_result_payload(result));
    }
    if let Some(error) = value.error.as_ref() {
        obj.insert("error".to_string(), tool_call_error_payload(error));
    }
    if value.duration_ms != 0 {
        obj.insert("duration_ms".to_string(), Value::from(value.duration_ms));
    }
    Value::Object(obj)
}

fn tool_call_result_payload(value: &ToolCallResult) -> Value {
    let mut obj = Map::new();
    obj.insert("content".to_string(), Value::String(value.content.clone()));
    if value.content_truncated {
        obj.insert("content_truncated".to_string(), Value::Bool(true));
    }
    obj.insert("success".to_string(), Value::Bool(value.success));
    if let Some(exit_code) = value.exit_code {
        obj.insert("exit_code".to_string(), Value::from(exit_code));
    }
    Value::Object(obj)
}

fn tool_call_error_payload(value: &ToolCallError) -> Value {
    let mut obj = Map::new();
    if !value.code.is_empty() {
        obj.insert("code".to_string(), Value::String(value.code.clone()));
    }
    obj.insert("message".to_string(), Value::String(value.message.clone()));
    if let Some(exit_code) = value.exit_code {
        obj.insert("exit_code".to_string(), Value::from(exit_code));
    }
    Value::Object(obj)
}

fn turn_metrics_payload(value: &TurnMetrics) -> Value {
    let mut obj = Map::new();
    obj.insert("input_tokens".to_string(), Value::from(value.input_tokens));
    obj.insert(
        "output_tokens".to_string(),
        Value::from(value.output_tokens),
    );
    obj.insert("total_tokens".to_string(), Value::from(value.total_tokens));
    if let Some(cached_tokens) = value.cached_tokens {
        obj.insert("cached_tokens".to_string(), Value::from(cached_tokens));
    }
    if let Some(reasoning_tokens) = value.reasoning_tokens {
        obj.insert(
            "reasoning_tokens".to_string(),
            Value::from(reasoning_tokens),
        );
    }
    if let Some(duration_ms) = value.duration_ms {
        obj.insert("duration_ms".to_string(), Value::from(duration_ms));
    }
    if !value.model.is_empty() {
        obj.insert("model".to_string(), Value::String(value.model.clone()));
    }
    Value::Object(obj)
}

fn system_message_payload(value: &SystemMessage) -> Value {
    let mut obj = Map::new();
    obj.insert("kind".to_string(), Value::String(value.kind.clone()));
    if !value.title.is_empty() {
        obj.insert("title".to_string(), Value::String(value.title.clone()));
    }
    obj.insert("content".to_string(), Value::String(value.content.clone()));
    Value::Object(obj)
}

fn handoff_payload(value: &HandoffInfo) -> Value {
    let mut obj = Map::new();
    obj.insert(
        "from_agent".to_string(),
        Value::String(value.from_agent.clone()),
    );
    obj.insert(
        "to_agent".to_string(),
        Value::String(value.to_agent.clone()),
    );
    if !value.tool_name.is_empty() {
        obj.insert(
            "tool_name".to_string(),
            Value::String(value.tool_name.clone()),
        );
    }
    if !value.input.is_empty() {
        obj.insert("input".to_string(), Value::String(value.input.clone()));
    }
    if !value.reason.is_empty() {
        obj.insert("reason".to_string(), Value::String(value.reason.clone()));
    }
    Value::Object(obj)
}

fn assistant_payload(value: &Assistant) -> Value {
    let mut obj = Map::new();
    obj.insert("text".to_string(), Value::String(value.text.clone()));
    if !value.reasoning.is_empty() {
        obj.insert(
            "reasoning".to_string(),
            Value::String(value.reasoning.clone()),
        );
    }
    if !value.model.is_empty() {
        obj.insert("model".to_string(), Value::String(value.model.clone()));
    }
    if value.input_tokens != 0 {
        obj.insert("input_tokens".to_string(), Value::from(value.input_tokens));
    }
    if value.output_tokens != 0 {
        obj.insert(
            "output_tokens".to_string(),
            Value::from(value.output_tokens),
        );
    }
    if !value.stop_reason.is_empty() {
        obj.insert(
            "stop_reason".to_string(),
            Value::String(value.stop_reason.clone()),
        );
    }
    Value::Object(obj)
}

fn tool_call_payload(value: &ToolCall) -> Value {
    let mut obj = Map::new();
    obj.insert("call_id".to_string(), Value::String(value.call_id.clone()));
    obj.insert("name".to_string(), Value::String(value.name.clone()));
    obj.insert("args".to_string(), Value::String(value.args.clone()));
    if !value.description.is_empty() {
        obj.insert(
            "description".to_string(),
            Value::String(value.description.clone()),
        );
    }
    Value::Object(obj)
}

fn tool_result_payload(value: &ToolResult) -> Value {
    let mut obj = Map::new();
    obj.insert("call_id".to_string(), Value::String(value.call_id.clone()));
    obj.insert("content".to_string(), Value::String(value.content.clone()));
    obj.insert("is_error".to_string(), Value::Bool(value.is_error));
    if let Some(exit_code) = value.exit_code {
        obj.insert("exit_code".to_string(), Value::from(exit_code));
    }
    if !value.streaming_output.is_empty() {
        obj.insert(
            "streaming_output".to_string(),
            Value::String(value.streaming_output.clone()),
        );
    }
    if value.output_truncated {
        obj.insert("output_truncated".to_string(), Value::Bool(true));
    }
    if value.duration_ms != 0 {
        obj.insert("duration_ms".to_string(), Value::from(value.duration_ms));
    }
    Value::Object(obj)
}

fn context_metadata_payload(value: &ContextMetadata) -> Value {
    let mut obj = Map::new();
    if !value.client_tag.is_empty() {
        obj.insert(
            "client_tag".to_string(),
            Value::String(value.client_tag.clone()),
        );
    }
    if !value.title.is_empty() {
        obj.insert("title".to_string(), Value::String(value.title.clone()));
    }
    if !value.labels.is_empty() {
        obj.insert(
            "labels".to_string(),
            Value::Array(
                value
                    .labels
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if !value.custom.is_empty() {
        obj.insert("custom".to_string(), json!(value.custom));
    }
    if let Some(provenance) = value.provenance.as_ref() {
        obj.insert("provenance".to_string(), provenance_payload(provenance));
    }
    Value::Object(obj)
}

fn provenance_payload(value: &Provenance) -> Value {
    let mut obj = Map::new();
    if let Some(parent_context_id) = value.parent_context_id {
        obj.insert(
            "parent_context_id".to_string(),
            Value::from(parent_context_id),
        );
    }
    if !value.spawn_reason.is_empty() {
        obj.insert(
            "spawn_reason".to_string(),
            Value::String(value.spawn_reason.clone()),
        );
    }
    if let Some(root_context_id) = value.root_context_id {
        obj.insert("root_context_id".to_string(), Value::from(root_context_id));
    }
    if !value.trace_id.is_empty() {
        obj.insert(
            "trace_id".to_string(),
            Value::String(value.trace_id.clone()),
        );
    }
    if !value.span_id.is_empty() {
        obj.insert("span_id".to_string(), Value::String(value.span_id.clone()));
    }
    if !value.correlation_id.is_empty() {
        obj.insert(
            "correlation_id".to_string(),
            Value::String(value.correlation_id.clone()),
        );
    }
    if !value.on_behalf_of.is_empty() {
        obj.insert(
            "on_behalf_of".to_string(),
            Value::String(value.on_behalf_of.clone()),
        );
    }
    if !value.on_behalf_of_source.is_empty() {
        obj.insert(
            "on_behalf_of_source".to_string(),
            Value::String(value.on_behalf_of_source.clone()),
        );
    }
    if !value.on_behalf_of_email.is_empty() {
        obj.insert(
            "on_behalf_of_email".to_string(),
            Value::String(value.on_behalf_of_email.clone()),
        );
    }
    if !value.writer_method.is_empty() {
        obj.insert(
            "writer_method".to_string(),
            Value::String(value.writer_method.clone()),
        );
    }
    if !value.writer_subject.is_empty() {
        obj.insert(
            "writer_subject".to_string(),
            Value::String(value.writer_subject.clone()),
        );
    }
    if !value.writer_issuer.is_empty() {
        obj.insert(
            "writer_issuer".to_string(),
            Value::String(value.writer_issuer.clone()),
        );
    }
    if !value.service_name.is_empty() {
        obj.insert(
            "service_name".to_string(),
            Value::String(value.service_name.clone()),
        );
    }
    if !value.service_version.is_empty() {
        obj.insert(
            "service_version".to_string(),
            Value::String(value.service_version.clone()),
        );
    }
    if !value.service_instance_id.is_empty() {
        obj.insert(
            "service_instance_id".to_string(),
            Value::String(value.service_instance_id.clone()),
        );
    }
    if value.process_pid != 0 {
        obj.insert("process_pid".to_string(), Value::from(value.process_pid));
    }
    if !value.process_owner.is_empty() {
        obj.insert(
            "process_owner".to_string(),
            Value::String(value.process_owner.clone()),
        );
    }
    if !value.host_name.is_empty() {
        obj.insert(
            "host_name".to_string(),
            Value::String(value.host_name.clone()),
        );
    }
    if !value.host_arch.is_empty() {
        obj.insert(
            "host_arch".to_string(),
            Value::String(value.host_arch.clone()),
        );
    }
    if !value.client_address.is_empty() {
        obj.insert(
            "client_address".to_string(),
            Value::String(value.client_address.clone()),
        );
    }
    if value.client_port != 0 {
        obj.insert("client_port".to_string(), Value::from(value.client_port));
    }
    if let Some(env_vars) = value.env_vars.as_ref() {
        obj.insert("env_vars".to_string(), json!(env_vars));
    }
    if !value.sdk_name.is_empty() {
        obj.insert(
            "sdk_name".to_string(),
            Value::String(value.sdk_name.clone()),
        );
    }
    if !value.sdk_version.is_empty() {
        obj.insert(
            "sdk_version".to_string(),
            Value::String(value.sdk_version.clone()),
        );
    }
    if value.captured_at != 0 {
        obj.insert("captured_at".to_string(), Value::from(value.captured_at));
    }
    Value::Object(obj)
}
