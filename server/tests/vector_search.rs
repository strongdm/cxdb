// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for semantic search, token-budget window, and usage aggregation.

use blake3::Hasher;
use cxdb_server::store::Store;
use rmpv::Value;
use tempfile::tempdir;

/// Helper: encode a msgpack payload with a text field and optional usage metadata.
fn encode_payload(text: &str) -> Vec<u8> {
    let root = Value::Map(vec![
        (Value::from(1), Value::from(text)),
    ]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &root).expect("encode payload");
    buf
}

/// Helper: encode a msgpack payload with usage metadata (key 20).
fn encode_payload_with_usage(
    text: &str,
    input_tokens: u64,
    output_tokens: u64,
    model: &str,
    provider: &str,
) -> Vec<u8> {
    let usage = Value::Map(vec![
        (Value::from(1), Value::from(input_tokens)),
        (Value::from(2), Value::from(output_tokens)),
        (Value::from(3), Value::from(model)),
        (Value::from(4), Value::from(provider)),
    ]);
    let root = Value::Map(vec![
        (Value::from(1), Value::from(text)),
        (Value::from(20), usage),
    ]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &root).expect("encode payload");
    buf
}

/// Helper: append a turn to a context and return its turn_id.
fn append_turn(store: &mut Store, context_id: u64, payload: &[u8]) -> u64 {
    let mut hasher = Hasher::new();
    hasher.update(payload);
    let hash = hasher.finalize();

    let (record, _meta) = store
        .append_turn(
            context_id,
            0,
            "com.example.Message".to_string(),
            1,
            1, // msgpack
            0, // uncompressed
            payload.len() as u32,
            *hash.as_bytes(),
            payload,
            None,
        )
        .expect("append turn");

    record.turn_id
}

#[test]
fn semantic_search_finds_nearest_turns() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");

    // Append 3 turns with different payloads
    let p1 = encode_payload("hello world");
    let p2 = encode_payload("goodbye moon");
    let p3 = encode_payload("hello planet");

    let t1 = append_turn(&mut store, ctx.context_id, &p1);
    let t2 = append_turn(&mut store, ctx.context_id, &p2);
    let t3 = append_turn(&mut store, ctx.context_id, &p3);

    // Insert embeddings: t1 and t3 are similar, t2 is different
    store.insert_embedding(t1, vec![1.0, 0.0, 0.0]);
    store.insert_embedding(t2, vec![0.0, 1.0, 0.0]);
    store.insert_embedding(t3, vec![0.9, 0.1, 0.0]);

    // Search for something similar to t1 ([1,0,0])
    let results = store
        .semantic_search(ctx.context_id, &[1.0, 0.0, 0.0], 3, 0.0)
        .expect("semantic search");

    assert!(!results.is_empty(), "should have results");

    // The first result should be t1 (exact match, similarity ~1.0)
    assert_eq!(results[0].0, t1, "first result should be turn 1 (exact match)");
    assert!(
        (results[0].1 - 1.0).abs() < 1e-5,
        "exact match should have similarity ~1.0, got {}",
        results[0].1
    );

    // t3 should appear before t2 (closer to [1,0,0] than [0,1,0])
    let t3_pos = results.iter().position(|(tid, _)| *tid == t3);
    let t2_pos = results.iter().position(|(tid, _)| *tid == t2);
    assert!(
        t3_pos.unwrap() < t2_pos.unwrap(),
        "t3 should rank higher than t2"
    );
}

#[test]
fn semantic_search_min_score_filters() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");

    let p1 = encode_payload("alpha");
    let p2 = encode_payload("beta");

    let t1 = append_turn(&mut store, ctx.context_id, &p1);
    let t2 = append_turn(&mut store, ctx.context_id, &p2);

    // t1 similar to query, t2 orthogonal
    store.insert_embedding(t1, vec![1.0, 0.0, 0.0]);
    store.insert_embedding(t2, vec![0.0, 1.0, 0.0]);

    // With min_score=0.5, only t1 should appear (similarity ~1.0)
    let results = store
        .semantic_search(ctx.context_id, &[1.0, 0.0, 0.0], 10, 0.5)
        .expect("semantic search");

    for (tid, score) in &results {
        assert!(
            *score >= 0.5,
            "turn {} has score {} which is below min_score 0.5",
            tid,
            score
        );
    }
}

#[test]
fn semantic_search_scoped_to_context() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx1 = store.create_context(0).expect("create context 1");
    let ctx2 = store.create_context(0).expect("create context 2");

    let p = encode_payload("text");
    let t1 = append_turn(&mut store, ctx1.context_id, &p);
    let t2 = append_turn(&mut store, ctx2.context_id, &p);

    store.insert_embedding(t1, vec![1.0, 0.0, 0.0]);
    store.insert_embedding(t2, vec![1.0, 0.0, 0.0]);

    // Search in ctx1 should only find t1
    let results = store
        .semantic_search(ctx1.context_id, &[1.0, 0.0, 0.0], 10, 0.0)
        .expect("search ctx1");

    let turn_ids: Vec<u64> = results.iter().map(|(tid, _)| *tid).collect();
    assert!(turn_ids.contains(&t1), "should find t1 in ctx1");
    assert!(!turn_ids.contains(&t2), "should not find t2 from ctx2");
}

#[test]
fn semantic_search_empty_context() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");

    // No turns appended, no embeddings
    let results = store
        .semantic_search(ctx.context_id, &[1.0, 0.0, 0.0], 10, 0.0)
        .expect("search empty");

    assert!(results.is_empty(), "empty context should return no results");
}

#[test]
fn token_budget_window_returns_recent_and_relevant() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");

    // Append 5 turns
    let mut turn_ids = Vec::new();
    for i in 0..5 {
        let p = encode_payload(&format!("turn {i}"));
        let tid = append_turn(&mut store, ctx.context_id, &p);
        turn_ids.push(tid);
    }

    // Add embeddings
    store.insert_embedding(turn_ids[0], vec![1.0, 0.0, 0.0]); // relevant
    store.insert_embedding(turn_ids[1], vec![0.0, 1.0, 0.0]); // not relevant
    store.insert_embedding(turn_ids[2], vec![0.0, 0.0, 1.0]); // not relevant
    store.insert_embedding(turn_ids[3], vec![0.1, 0.9, 0.0]); // not relevant
    store.insert_embedding(turn_ids[4], vec![0.8, 0.2, 0.0]); // somewhat relevant

    // Request window with budget that can hold all turns, always_include_recent=2
    let window = store
        .token_budget_window(ctx.context_id, &[1.0, 0.0, 0.0], 100_000, 2)
        .expect("token budget window");

    assert!(!window.is_empty(), "window should not be empty");

    // The most recent 2 turns should be included
    let window_turn_ids: Vec<u64> = window.iter().map(|t| t.record.turn_id).collect();
    assert!(
        window_turn_ids.contains(&turn_ids[3]) || window_turn_ids.contains(&turn_ids[4]),
        "should include recent turns"
    );

    // Turns should be in chronological order (sorted by depth)
    for w in window.windows(2) {
        assert!(
            w[0].record.depth <= w[1].record.depth,
            "turns should be ordered by depth"
        );
    }
}

#[test]
fn usage_aggregation_sums_tokens() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");

    // Append turns with usage metadata
    let p1 = encode_payload_with_usage("turn 1", 100, 50, "gpt-4", "openai");
    let p2 = encode_payload_with_usage("turn 2", 200, 75, "gpt-4", "openai");
    let p3 = encode_payload_with_usage("turn 3", 150, 60, "claude-3", "anthropic");

    append_turn(&mut store, ctx.context_id, &p1);
    append_turn(&mut store, ctx.context_id, &p2);
    append_turn(&mut store, ctx.context_id, &p3);

    let usage = store.aggregate_usage(ctx.context_id).expect("aggregate usage");

    assert_eq!(usage.total_input_tokens, 450);
    assert_eq!(usage.total_output_tokens, 185);
    assert_eq!(usage.turn_count, 3);

    // Check by_model
    let gpt4 = usage.by_model.get("gpt-4").expect("should have gpt-4");
    assert_eq!(gpt4.input_tokens, 300);
    assert_eq!(gpt4.output_tokens, 125);

    let claude = usage.by_model.get("claude-3").expect("should have claude-3");
    assert_eq!(claude.input_tokens, 150);
    assert_eq!(claude.output_tokens, 60);

    // Check by_provider
    let openai = usage.by_provider.get("openai").expect("should have openai");
    assert_eq!(openai.input_tokens, 300);
    assert_eq!(openai.output_tokens, 125);

    let anthropic = usage.by_provider.get("anthropic").expect("should have anthropic");
    assert_eq!(anthropic.input_tokens, 150);
    assert_eq!(anthropic.output_tokens, 60);
}

#[test]
fn usage_aggregation_no_usage_fields() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");

    // Append turns without usage metadata
    let p1 = encode_payload("no usage here");
    append_turn(&mut store, ctx.context_id, &p1);

    let usage = store.aggregate_usage(ctx.context_id).expect("aggregate usage");

    assert_eq!(usage.total_input_tokens, 0);
    assert_eq!(usage.total_output_tokens, 0);
    assert_eq!(usage.turn_count, 1);
    assert!(usage.by_model.is_empty());
    assert!(usage.by_provider.is_empty());
}

#[test]
fn usage_aggregation_nonexistent_context() {
    let dir = tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open store");

    let result = store.aggregate_usage(999);
    assert!(result.is_err(), "should error for nonexistent context");
}

#[test]
fn semantic_search_nonexistent_context() {
    let dir = tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open store");

    let result = store.semantic_search(999, &[1.0, 0.0, 0.0], 10, 0.0);
    assert!(result.is_err(), "should error for nonexistent context");
}

/// Helper: serialize an f32 embedding vector to little-endian bytes.
fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for &v in embedding {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

/// Helper: append a turn with an embedding hash set on the TurnRecord and
/// the embedding blob stored in blob_store, then insert into vector index.
fn append_turn_with_embedding(
    store: &mut Store,
    context_id: u64,
    payload: &[u8],
    embedding: Vec<f32>,
) -> u64 {
    // Serialize embedding to bytes and compute BLAKE3 hash
    let emb_blob = embedding_to_bytes(&embedding);
    let emb_hash = blake3::hash(&emb_blob);

    // Store embedding blob in blob_store
    store
        .blob_store
        .put_if_absent(*emb_hash.as_bytes(), &emb_blob)
        .expect("store embedding blob");

    // Compute payload hash
    let mut hasher = Hasher::new();
    hasher.update(payload);
    let hash = hasher.finalize();

    // Append turn with embedding_hash
    let (record, _meta) = store
        .append_turn(
            context_id,
            0,
            "com.example.Message".to_string(),
            1,
            1, // msgpack
            0, // uncompressed
            payload.len() as u32,
            *hash.as_bytes(),
            payload,
            Some(*emb_hash.as_bytes()),
        )
        .expect("append turn with embedding");

    // Also insert embedding into vector index for search
    store.insert_embedding(record.turn_id, embedding);

    record.turn_id
}

#[test]
fn append_with_embedding_and_search_roundtrip() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");

    // Append 2 turns: one with embedding, one without
    let p1 = encode_payload_with_usage("hello from model", 1500, 800, "kimi-k2p5", "fireworks");
    let p2 = encode_payload("unrelated turn");

    let emb1 = vec![0.8, 0.6, 0.0];
    let t1 = append_turn_with_embedding(&mut store, ctx.context_id, &p1, emb1.clone());
    let t2 = append_turn(&mut store, ctx.context_id, &p2);

    // Also give t2 an embedding that's far from our query
    store.insert_embedding(t2, vec![0.0, 0.0, 1.0]);

    // 1. Verify embedding_hash is set on TurnRecord for t1
    let record1 = store.turn_store.get_turn(t1).expect("get turn t1");
    assert!(
        record1.embedding_hash.is_some(),
        "TurnRecord for t1 should have embedding_hash set"
    );

    // Verify the embedding blob is in blob_store
    let emb_hash = record1.embedding_hash.unwrap();
    assert!(
        store.blob_store.contains(&emb_hash),
        "embedding blob should exist in blob_store"
    );

    // Verify we can reconstruct the embedding from blob_store
    let stored_blob = store.blob_store.get(&emb_hash).expect("get embedding blob");
    let reconstructed: Vec<f32> = stored_blob
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(
        reconstructed, emb1,
        "reconstructed embedding should match original"
    );

    // 2. Verify t2 has no embedding_hash
    let record2 = store.turn_store.get_turn(t2).expect("get turn t2");
    assert!(
        record2.embedding_hash.is_none(),
        "TurnRecord for t2 should not have embedding_hash"
    );

    // 3. Search for embedding similar to t1 ([0.8, 0.6, 0.0])
    let results = store
        .semantic_search(ctx.context_id, &[1.0, 0.5, 0.0], 3, 0.0)
        .expect("semantic search");

    assert!(!results.is_empty(), "search should return results");
    assert_eq!(
        results[0].0, t1,
        "first search result should be t1 (closest to query)"
    );

    // 4. Verify usage aggregation includes the usage from t1
    let usage = store
        .aggregate_usage(ctx.context_id)
        .expect("aggregate usage");

    assert_eq!(usage.total_input_tokens, 1500);
    assert_eq!(usage.total_output_tokens, 800);
    assert_eq!(usage.turn_count, 2);

    let fireworks = usage
        .by_provider
        .get("fireworks")
        .expect("should have fireworks provider");
    assert_eq!(fireworks.input_tokens, 1500);
    assert_eq!(fireworks.output_tokens, 800);

    let kimi = usage
        .by_model
        .get("kimi-k2p5")
        .expect("should have kimi-k2p5 model");
    assert_eq!(kimi.input_tokens, 1500);
    assert_eq!(kimi.output_tokens, 800);
}

#[test]
fn embedding_hash_persists_across_reopen() {
    let dir = tempdir().expect("tempdir");

    let (context_id, turn_id, expected_emb_hash) = {
        let mut store = Store::open(dir.path()).expect("open store");
        let ctx = store.create_context(0).expect("create context");

        let p = encode_payload("persistent embedding");
        let emb = vec![0.5, 0.5, 0.0];
        let tid = append_turn_with_embedding(&mut store, ctx.context_id, &p, emb);

        let record = store.turn_store.get_turn(tid).expect("get turn");
        let emb_hash = record.embedding_hash.expect("should have embedding_hash");

        (ctx.context_id, tid, emb_hash)
    }; // store dropped

    // Reopen and verify embedding_hash is still there
    let store = Store::open(dir.path()).expect("reopen store");
    let record = store.turn_store.get_turn(turn_id).expect("get turn after reopen");
    assert_eq!(
        record.embedding_hash,
        Some(expected_emb_hash),
        "embedding_hash should persist across store reopen"
    );

    // Verify the embedding blob is still in blob_store
    assert!(
        store.blob_store.contains(&expected_emb_hash),
        "embedding blob should persist across store reopen"
    );

    // Verify we can still read the last turns
    let last = store
        .get_last(context_id, 10, true)
        .expect("get last after reopen");
    assert_eq!(last.len(), 1);
    assert_eq!(last[0].record.turn_id, turn_id);
}
