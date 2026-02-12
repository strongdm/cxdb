// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

//! Event broadcasting for real-time SSE updates.
//!
//! This module provides an EventBus that broadcasts store events to SSE subscribers.
//! Events originate from the binary protocol handler and are fanned out to all
//! connected HTTP SSE clients.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// Store events that can be broadcast to SSE subscribers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StoreEvent {
    /// A new context was created.
    ContextCreated {
        context_id: String,
        session_id: String,
        client_tag: String,
        created_at: u64,
    },
    /// Context metadata was extracted from the first turn.
    ContextMetadataUpdated {
        context_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_tag: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        labels: Option<Vec<String>>,
        has_provenance: bool,
    },
    /// A context was linked to a parent context (cross-context lineage).
    ContextLinked {
        child_context_id: String,
        parent_context_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        root_context_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        spawn_reason: Option<String>,
    },
    /// A turn was appended to a context.
    TurnAppended {
        context_id: String,
        turn_id: String,
        parent_turn_id: String,
        depth: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        declared_type_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        declared_type_version: Option<u32>,
    },
    /// A binary protocol client connected.
    ClientConnected {
        session_id: String,
        client_tag: String,
    },
    /// A binary protocol client disconnected.
    ClientDisconnected {
        session_id: String,
        client_tag: String,
        contexts: Vec<String>,
    },
    /// An error occurred (HTTP or binary protocol).
    ErrorOccurred {
        timestamp_ms: u64,
        kind: String,
        status_code: u16,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        path: Option<String>,
    },
}

impl StoreEvent {
    /// Convert event to SSE format: (event_type, json_data).
    pub fn to_sse(&self) -> (&'static str, String) {
        let event_type = match self {
            StoreEvent::ContextCreated { .. } => "context_created",
            StoreEvent::ContextMetadataUpdated { .. } => "context_metadata_updated",
            StoreEvent::ContextLinked { .. } => "context_linked",
            StoreEvent::TurnAppended { .. } => "turn_appended",
            StoreEvent::ClientConnected { .. } => "client_connected",
            StoreEvent::ClientDisconnected { .. } => "client_disconnected",
            StoreEvent::ErrorOccurred { .. } => "error_occurred",
        };

        // Serialize without the type tag (frontend expects flat structure)
        let data = match self {
            StoreEvent::ContextCreated {
                context_id,
                session_id,
                client_tag,
                created_at,
            } => serde_json::json!({
                "context_id": context_id,
                "session_id": session_id,
                "client_tag": client_tag,
                "created_at": created_at,
            }),
            StoreEvent::ContextMetadataUpdated {
                context_id,
                client_tag,
                title,
                labels,
                has_provenance,
            } => {
                let mut obj = serde_json::json!({
                    "context_id": context_id,
                    "has_provenance": has_provenance,
                });
                if let Some(tag) = client_tag {
                    obj["client_tag"] = serde_json::Value::String(tag.clone());
                }
                if let Some(t) = title {
                    obj["title"] = serde_json::Value::String(t.clone());
                }
                if let Some(l) = labels {
                    obj["labels"] = serde_json::json!(l);
                }
                obj
            }
            StoreEvent::ContextLinked {
                child_context_id,
                parent_context_id,
                root_context_id,
                spawn_reason,
            } => {
                let mut obj = serde_json::json!({
                    "child_context_id": child_context_id,
                    "parent_context_id": parent_context_id,
                });
                if let Some(root) = root_context_id {
                    obj["root_context_id"] = serde_json::Value::String(root.clone());
                }
                if let Some(reason) = spawn_reason {
                    obj["spawn_reason"] = serde_json::Value::String(reason.clone());
                }
                obj
            }
            StoreEvent::TurnAppended {
                context_id,
                turn_id,
                parent_turn_id,
                depth,
                declared_type_id,
                declared_type_version,
            } => {
                let mut obj = serde_json::json!({
                    "context_id": context_id,
                    "turn_id": turn_id,
                    "parent_turn_id": parent_turn_id,
                    "depth": depth,
                });
                if let Some(id) = declared_type_id {
                    obj["declared_type_id"] = serde_json::Value::String(id.clone());
                }
                if let Some(ver) = declared_type_version {
                    obj["declared_type_version"] = serde_json::json!(ver);
                }
                obj
            }
            StoreEvent::ClientConnected {
                session_id,
                client_tag,
            } => serde_json::json!({
                "session_id": session_id,
                "client_tag": client_tag,
            }),
            StoreEvent::ClientDisconnected {
                session_id,
                client_tag,
                contexts,
            } => serde_json::json!({
                "session_id": session_id,
                "client_tag": client_tag,
                "contexts": contexts,
            }),
            StoreEvent::ErrorOccurred {
                timestamp_ms,
                kind,
                status_code,
                message,
                path,
            } => {
                let mut obj = serde_json::json!({
                    "timestamp_ms": timestamp_ms,
                    "kind": kind,
                    "status_code": status_code,
                    "message": message,
                });
                if let Some(p) = path {
                    obj["path"] = serde_json::Value::String(p.clone());
                }
                obj
            }
        };

        (event_type, data.to_string())
    }
}

/// A subscriber to the event bus.
pub struct EventSubscriber {
    rx: Receiver<StoreEvent>,
}

impl EventSubscriber {
    /// Receive the next event, blocking until available.
    pub fn recv(&self) -> Option<StoreEvent> {
        self.rx.recv().ok()
    }

    /// Try to receive an event without blocking.
    pub fn try_recv(&self) -> Option<StoreEvent> {
        self.rx.try_recv().ok()
    }

    /// Receive with timeout.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<StoreEvent> {
        self.rx.recv_timeout(timeout).ok()
    }
}

/// Thread-safe event bus for broadcasting store events to SSE subscribers.
pub struct EventBus {
    subscribers: Arc<Mutex<Vec<Sender<StoreEvent>>>>,
}

impl EventBus {
    /// Create a new event bus.
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Subscribe to events. Returns a subscriber that receives all future events.
    pub fn subscribe(&self) -> EventSubscriber {
        let (tx, rx) = mpsc::channel();
        let mut subs = self.subscribers.lock().unwrap();
        subs.push(tx);
        EventSubscriber { rx }
    }

    /// Publish an event to all subscribers.
    /// Disconnected subscribers are automatically removed.
    pub fn publish(&self, event: StoreEvent) {
        let mut subs = self.subscribers.lock().unwrap();
        // Send to all, remove disconnected
        subs.retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Get the current number of subscribers.
    pub fn subscriber_count(&self) -> usize {
        let subs = self.subscribers.lock().unwrap();
        subs.len()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_event_bus_basic() {
        let bus = EventBus::new();
        let sub = bus.subscribe();

        bus.publish(StoreEvent::ClientConnected {
            session_id: "123".to_string(),
            client_tag: "test".to_string(),
        });

        let event = sub.recv_timeout(Duration::from_millis(100));
        assert!(event.is_some());
        match event.unwrap() {
            StoreEvent::ClientConnected { session_id, .. } => {
                assert_eq!(session_id, "123");
            }
            _ => panic!("wrong event type"),
        }
    }

    #[test]
    fn test_event_bus_multiple_subscribers() {
        let bus = EventBus::new();
        let sub1 = bus.subscribe();
        let sub2 = bus.subscribe();

        bus.publish(StoreEvent::ContextCreated {
            context_id: "1".to_string(),
            session_id: "2".to_string(),
            client_tag: "tag".to_string(),
            created_at: 12345,
        });

        assert!(sub1.recv_timeout(Duration::from_millis(100)).is_some());
        assert!(sub2.recv_timeout(Duration::from_millis(100)).is_some());
    }

    #[test]
    fn test_event_to_sse() {
        let event = StoreEvent::ContextMetadataUpdated {
            context_id: "123".to_string(),
            client_tag: Some("claude-code".to_string()),
            title: Some("Fix bug".to_string()),
            labels: Some(vec!["urgent".to_string()]),
            has_provenance: true,
        };

        let (event_type, data) = event.to_sse();
        assert_eq!(event_type, "context_metadata_updated");
        assert!(data.contains("\"context_id\":\"123\""));
        assert!(data.contains("\"title\":\"Fix bug\""));
    }

    #[test]
    fn test_context_linked_event_to_sse() {
        let event = StoreEvent::ContextLinked {
            child_context_id: "12".to_string(),
            parent_context_id: "5".to_string(),
            root_context_id: Some("1".to_string()),
            spawn_reason: Some("sub_agent".to_string()),
        };

        let (event_type, data) = event.to_sse();
        assert_eq!(event_type, "context_linked");
        assert!(data.contains("\"child_context_id\":\"12\""));
        assert!(data.contains("\"parent_context_id\":\"5\""));
    }

    #[test]
    fn test_subscriber_cleanup() {
        let bus = EventBus::new();

        {
            let _sub = bus.subscribe();
            assert_eq!(bus.subscriber_count(), 1);
        }
        // Subscriber dropped, but won't be cleaned up until next publish

        bus.publish(StoreEvent::ClientConnected {
            session_id: "1".to_string(),
            client_tag: "test".to_string(),
        });

        // Now the dead subscriber should be removed
        assert_eq!(bus.subscriber_count(), 0);
    }
}
