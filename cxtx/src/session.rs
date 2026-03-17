use anyhow::Result;
use chrono::{DateTime, Utc};
use cxdb::types::ContextMetadata;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use crate::provider::ProviderKind;
use crate::turns::{
    context_metadata, history_item_to_conversation_item, ingest_state_item, provider_error_item,
    rewrite_item, session_end_item, session_start_item, ArtifactRefs, HistoryItem, TurnEnvelope,
};

#[derive(Debug, Clone, Serialize)]
pub struct CapturedSession {
    pub session_id: String,
    pub provider_kind: String,
    pub child_command: String,
    pub child_args: Vec<String>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CapturedExchange {
    pub exchange_id: String,
    pub provider_request_id: Option<String>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub status_code: Option<u16>,
}

#[derive(Debug)]
pub struct SessionRuntime {
    inner: Arc<SessionRuntimeInner>,
}

#[derive(Debug)]
struct SessionRuntimeInner {
    session: CapturedSession,
    provider: ProviderKind,
    metadata: ContextMetadata,
    child_pid: AtomicU32,
    state: Mutex<MutableState>,
}

#[derive(Debug, Default)]
struct MutableState {
    next_turn_ordinal: u64,
    next_exchange_ordinal: u64,
    observed_history: Vec<HistoryItem>,
}

impl Clone for SessionRuntime {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl SessionRuntime {
    pub fn new(
        provider: ProviderKind,
        child_args: Vec<String>,
        allowlisted_env: BTreeMap<String, String>,
    ) -> Result<Self> {
        let started_at = Utc::now();
        let session = CapturedSession {
            session_id: Uuid::new_v4().to_string(),
            provider_kind: provider.provider_name().to_string(),
            child_command: provider.command_name().to_string(),
            child_args,
            started_at,
        };
        let metadata = context_metadata(provider, &session, &allowlisted_env);
        Ok(Self {
            inner: Arc::new(SessionRuntimeInner {
                session,
                provider,
                metadata,
                child_pid: AtomicU32::new(0),
                state: Mutex::new(MutableState::default()),
            }),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.inner.session.session_id
    }

    pub fn provider(&self) -> ProviderKind {
        self.inner.provider
    }

    pub fn session(&self) -> &CapturedSession {
        &self.inner.session
    }

    pub fn metadata(&self) -> &ContextMetadata {
        &self.inner.metadata
    }

    pub fn set_child_pid(&self, pid: Option<u32>) {
        self.inner
            .child_pid
            .store(pid.unwrap_or(0), Ordering::Relaxed);
    }

    pub fn child_pid(&self) -> Option<u32> {
        match self.inner.child_pid.load(Ordering::Relaxed) {
            0 => None,
            value => Some(value),
        }
    }

    pub fn next_exchange_id(&self) -> String {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        state.next_exchange_ordinal += 1;
        format!("exchange-{:04}", state.next_exchange_ordinal)
    }

    pub fn session_start_turn(&self) -> TurnEnvelope {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        let ordinal = next_turn_ordinal(&mut state);
        TurnEnvelope {
            ordinal,
            item: session_start_item(
                &self.inner.session,
                self.inner.provider,
                ordinal,
                self.child_pid(),
                &self.inner.metadata,
            ),
        }
    }

    pub fn session_end_turn(&self, exit_code: i32, success: bool) -> TurnEnvelope {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        let ordinal = next_turn_ordinal(&mut state);
        TurnEnvelope {
            ordinal,
            item: session_end_item(&self.inner.session, ordinal, exit_code, success),
        }
    }

    pub fn ingest_degraded_turn(&self, queue_depth: usize, error: &str) -> TurnEnvelope {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        let ordinal = next_turn_ordinal(&mut state);
        TurnEnvelope {
            ordinal,
            item: ingest_state_item(
                &self.inner.session,
                ordinal,
                "ingest_degraded",
                cxdb::types::SystemKindWarning,
                queue_depth,
                Some(error),
            ),
        }
    }

    pub fn ingest_recovered_turn(&self, queue_depth: usize) -> TurnEnvelope {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        let ordinal = next_turn_ordinal(&mut state);
        TurnEnvelope {
            ordinal,
            item: ingest_state_item(
                &self.inner.session,
                ordinal,
                "ingest_recovered",
                cxdb::types::SystemKindInfo,
                queue_depth,
                None,
            ),
        }
    }

    pub fn observe_request_history(
        &self,
        exchange_id: &str,
        history: Vec<HistoryItem>,
        artifact_refs: &ArtifactRefs,
    ) -> Vec<TurnEnvelope> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        let previous = state.observed_history.clone();
        let normalized_history = history
            .iter()
            .map(normalize_history_item)
            .collect::<Vec<_>>();
        let prefix_len = common_prefix_len(&previous, &normalized_history);

        let mut turns = Vec::new();
        if prefix_len < previous.len() {
            let ordinal = next_turn_ordinal(&mut state);
            turns.push(TurnEnvelope {
                ordinal,
                item: rewrite_item(
                    &self.inner.session,
                    ordinal,
                    exchange_id,
                    previous.len(),
                    history.len(),
                    artifact_refs,
                ),
            });
        }

        for item in history.iter().skip(prefix_len) {
            let ordinal = next_turn_ordinal(&mut state);
            turns.push(TurnEnvelope {
                ordinal,
                item: history_item_to_conversation_item(
                    &self.inner.session,
                    ordinal,
                    exchange_id,
                    item,
                ),
            });
        }

        state.observed_history = normalized_history;
        turns
    }

    pub fn append_history_item(&self, exchange_id: &str, item: HistoryItem) -> TurnEnvelope {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        let ordinal = next_turn_ordinal(&mut state);
        let turn = TurnEnvelope {
            ordinal,
            item: history_item_to_conversation_item(
                &self.inner.session,
                ordinal,
                exchange_id,
                &item,
            ),
        };
        state.observed_history.push(normalize_history_item(&item));
        turn
    }

    pub fn provider_error_turn(
        &self,
        exchange_id: &str,
        title: &str,
        message: &str,
        provider_request_id: Option<&str>,
        artifact_refs: &ArtifactRefs,
    ) -> TurnEnvelope {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("session state lock poisoned");
        let ordinal = next_turn_ordinal(&mut state);
        TurnEnvelope {
            ordinal,
            item: provider_error_item(
                &self.inner.session,
                ordinal,
                exchange_id,
                title,
                message,
                provider_request_id,
                artifact_refs,
            ),
        }
    }
}

fn next_turn_ordinal(state: &mut MutableState) -> u64 {
    state.next_turn_ordinal += 1;
    state.next_turn_ordinal
}

fn common_prefix_len(left: &[HistoryItem], right: &[HistoryItem]) -> usize {
    left.iter()
        .zip(right.iter())
        .take_while(|(left, right)| left == right)
        .count()
}

fn normalize_history_item(item: &HistoryItem) -> HistoryItem {
    match item {
        HistoryItem::AssistantTurn {
            text, tool_calls, ..
        } => HistoryItem::AssistantTurn {
            text: text.clone(),
            tool_calls: tool_calls.clone(),
            model: None,
            finish_reason: None,
        },
        _ => item.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::SessionRuntime;
    use crate::provider::ProviderKind;
    use crate::turns::{ArtifactRefs, HistoryItem, ToolCallRecord};
    use std::collections::BTreeMap;

    #[test]
    fn first_turn_carries_context_metadata() {
        let session = SessionRuntime::new(
            ProviderKind::Codex,
            vec!["--help".to_string()],
            BTreeMap::new(),
        )
        .unwrap();
        let turn = session.session_start_turn();
        assert_eq!(
            turn.item.context_metadata.as_ref().unwrap().client_tag,
            "cxtx/codex"
        );
    }

    #[test]
    fn rewrite_detection_emits_system_turn_before_new_suffix() {
        let session =
            SessionRuntime::new(ProviderKind::Codex, Vec::new(), BTreeMap::new()).unwrap();
        let first = session.observe_request_history(
            "exchange-0001",
            vec![HistoryItem::UserInput {
                text: "hello".to_string(),
                files: Vec::new(),
            }],
            &ArtifactRefs::default(),
        );
        assert_eq!(first.len(), 1);

        let rewritten = session.observe_request_history(
            "exchange-0002",
            vec![HistoryItem::UserInput {
                text: "rewritten".to_string(),
                files: Vec::new(),
            }],
            &ArtifactRefs::default(),
        );
        assert_eq!(rewritten.len(), 2);
        assert_eq!(
            rewritten[0].item.system.as_ref().unwrap().title,
            "history_rewrite_detected"
        );
    }

    #[test]
    fn assistant_turn_dedup_ignores_model_and_finish_reason() {
        let session =
            SessionRuntime::new(ProviderKind::Claude, Vec::new(), BTreeMap::new()).unwrap();
        let first = session.observe_request_history(
            "exchange-0001",
            vec![HistoryItem::UserInput {
                text: "use tool".to_string(),
                files: Vec::new(),
            }],
            &ArtifactRefs::default(),
        );
        assert_eq!(first.len(), 1);

        let appended = session.append_history_item(
            "exchange-0001",
            HistoryItem::AssistantTurn {
                text: String::new(),
                tool_calls: vec![ToolCallRecord {
                    call_id: "call_1".to_string(),
                    name: "lookup".to_string(),
                    args: "{\"q\":\"use tool\"}".to_string(),
                }],
                model: Some("claude-3-7-sonnet-20250219".to_string()),
                finish_reason: Some("tool_use".to_string()),
            },
        );
        assert_eq!(appended.item.item_type, "assistant_turn");

        let replay = session.observe_request_history(
            "exchange-0002",
            vec![
                HistoryItem::UserInput {
                    text: "use tool".to_string(),
                    files: Vec::new(),
                },
                HistoryItem::AssistantTurn {
                    text: String::new(),
                    tool_calls: vec![ToolCallRecord {
                        call_id: "call_1".to_string(),
                        name: "lookup".to_string(),
                        args: "{\"q\":\"use tool\"}".to_string(),
                    }],
                    model: None,
                    finish_reason: None,
                },
                HistoryItem::ToolResult {
                    call_id: "call_1".to_string(),
                    content: "done".to_string(),
                    is_error: false,
                },
            ],
            &ArtifactRefs::default(),
        );

        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].item.item_type, "tool_result");
    }
}
