// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::encoding::{decode_msgpack, decode_msgpack_into, encode_msgpack};
use crate::test_util::decode_hex;
use rmpv::Value;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
struct MsgpackFixture {
    payload_hex: String,
}

fn load_msgpack_fixture(name: &str) -> MsgpackFixture {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("{name}.json"));
    let data = std::fs::read_to_string(&path).expect("read msgpack fixture");
    serde_json::from_str(&data).expect("parse msgpack fixture")
}

fn fixture_conversation_item() -> ConversationItem {
    let mut item = new_user_input("Hello from fixtures", vec!["file.txt".to_string()]);
    item.id = "item-1".to_string();
    item.timestamp = 1_700_000_000_000;
    item.with_context_metadata(ContextMetadata {
        client_tag: "fixture-tag".to_string(),
        title: "Fixture Title".to_string(),
        labels: vec!["alpha".to_string(), "beta".to_string()],
        custom: std::collections::HashMap::from([("env".to_string(), "test".to_string())]),
        tenant: None,
        provenance: None,
    });
    item
}

#[test]
fn msgpack_conversation_item_matches_fixture() {
    let fixture = load_msgpack_fixture("msgpack_conversation_item");
    let item = fixture_conversation_item();
    let payload = encode_msgpack(&item).unwrap();
    assert_eq!(hex::encode(payload), fixture.payload_hex);
}

#[test]
fn msgpack_numeric_map_matches_fixture() {
    let fixture = load_msgpack_fixture("msgpack_numeric_map");
    let fixture_bytes = decode_hex(&fixture.payload_hex);
    let decoded = decode_msgpack(&fixture_bytes).unwrap();
    assert_eq!(decoded.get(&1).and_then(|v| v.as_str()), Some("one"));
    assert_eq!(decoded.get(&2).and_then(|v| v.as_str()), Some("two"));
    assert_eq!(decoded.get(&3).and_then(|v| v.as_str()), Some("three"));

    let map = BTreeMap::from([
        (2u64, "two".to_string()),
        (1u64, "one".to_string()),
        (3u64, "three".to_string()),
    ]);
    let payload = encode_msgpack(&map).unwrap();
    let decoded = decode_msgpack(&payload).unwrap();
    assert_eq!(decoded.get(&1).and_then(|v| v.as_str()), Some("one"));
    assert_eq!(decoded.get(&2).and_then(|v| v.as_str()), Some("two"));
    assert_eq!(decoded.get(&3).and_then(|v| v.as_str()), Some("three"));
}

#[test]
fn msgpack_key_types_match_expectations() {
    let item = fixture_conversation_item();
    let payload = encode_msgpack(&item).unwrap();
    let mut cursor = std::io::Cursor::new(payload);
    let value = rmpv::decode::read_value(&mut cursor).unwrap();
    let map = match value {
        Value::Map(entries) => entries,
        _ => panic!("expected map"),
    };
    assert!(map
        .iter()
        .any(|(k, _)| matches!(k, Value::String(s) if s.as_str() == Some("1"))));

    let numeric_map = BTreeMap::from([(1u64, "one".to_string())]);
    let payload = encode_msgpack(&numeric_map).unwrap();
    let mut cursor = std::io::Cursor::new(payload);
    let value = rmpv::decode::read_value(&mut cursor).unwrap();
    let map = match value {
        Value::Map(entries) => entries,
        _ => panic!("expected map"),
    };
    assert!(map.iter().any(|(k, _)| matches!(k, Value::Integer(_))));
}

#[test]
fn decode_msgpack_accepts_string_keys() {
    let fixture = load_msgpack_fixture("msgpack_conversation_item");
    let bytes = decode_hex(&fixture.payload_hex);
    let decoded = decode_msgpack(&bytes).unwrap();
    assert!(decoded.contains_key(&1));
}

#[test]
fn decode_msgpack_into_conversation_item() {
    let fixture = load_msgpack_fixture("msgpack_conversation_item");
    let bytes = decode_hex(&fixture.payload_hex);
    let item: ConversationItem = decode_msgpack_into(&bytes).unwrap();
    assert_eq!(item.id, "item-1");
    assert_eq!(item.item_type, ItemTypeUserInput);
    assert_eq!(
        item.user_input.as_ref().unwrap().text,
        "Hello from fixtures"
    );
}

#[test]
fn capture_process_provenance_populates_fields() {
    let p = capture_process_provenance("test-service", "1.0.0", Vec::<ProvenanceOption>::new());
    assert_eq!(p.service_name, "test-service");
    assert_eq!(p.service_version, "1.0.0");
    assert!(!p.service_instance_id.is_empty());
    assert_eq!(p.process_pid, std::process::id() as i64);
    assert!(!p.host_arch.is_empty());
    assert!(p.captured_at > 0);
}

#[test]
fn new_provenance_inherits_and_overrides() {
    let base = capture_process_provenance("test-service", "1.0.0", Vec::<ProvenanceOption>::new());
    let derived = new_provenance(
        Some(&base),
        vec![
            with_on_behalf_of("user123", "slack", "user@example.com"),
            with_correlation_id("req-abc"),
        ],
    );
    assert_eq!(derived.service_name, "test-service");
    assert_eq!(derived.service_instance_id, base.service_instance_id);
    assert_eq!(derived.on_behalf_of, "user123");
    assert_eq!(derived.on_behalf_of_source, "slack");
    assert_eq!(derived.on_behalf_of_email, "user@example.com");
    assert_eq!(derived.correlation_id, "req-abc");
    assert!(derived.captured_at >= base.captured_at);
}

#[test]
fn with_parent_context_sets_root() {
    let p = new_provenance(None, vec![with_parent_context(100, 50)]);
    assert_eq!(p.parent_context_id, Some(100));
    assert_eq!(p.root_context_id, Some(50));

    let p2 = new_provenance(None, vec![with_parent_context(200, 0)]);
    assert_eq!(p2.root_context_id, Some(200));
}

#[test]
fn with_env_vars_respects_allowlist() {
    std::env::set_var("TEST_PROV_VAR", "test-value");
    let p = new_provenance(
        None,
        vec![with_env_vars(Some(vec![
            "TEST_PROV_VAR".to_string(),
            "NONEXISTENT_VAR".to_string(),
        ]))],
    );
    let env = p.env_vars.expect("env vars");
    assert_eq!(
        env.get("TEST_PROV_VAR").map(String::as_str),
        Some("test-value")
    );
    assert!(!env.contains_key("NONEXISTENT_VAR"));
    std::env::remove_var("TEST_PROV_VAR");
}

#[test]
fn provenance_env_vars_deep_copy() {
    std::env::set_var("PATH", "test-path");
    let base = new_provenance(None, vec![with_env_vars(Some(vec!["PATH".to_string()]))]);
    let mut derived = new_provenance(Some(&base), Vec::<ProvenanceOption>::new());
    if let Some(env) = derived.env_vars.as_mut() {
        env.insert("NEW_KEY".to_string(), "new_value".to_string());
    }
    if let Some(env) = base.env_vars.as_ref() {
        assert!(!env.contains_key("NEW_KEY"));
    }
    std::env::remove_var("PATH");
}
