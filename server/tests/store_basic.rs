// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use blake3::Hasher;
use cxdb_server::store::Store;
use tempfile::tempdir;

#[test]
fn append_and_fork() {
    let dir = tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open store");

    let ctx = store.create_context(0).expect("create context");
    assert_eq!(ctx.head_turn_id, 0);

    let payload = b"hello world".to_vec();
    let mut hasher = Hasher::new();
    hasher.update(&payload);
    let hash = hasher.finalize();

    let (first, _metadata) = store
        .append_turn(
            ctx.context_id,
            0,
            "com.example.Test".to_string(),
            1,
            1,
            0,
            payload.len() as u32,
            *hash.as_bytes(),
            &payload,
        )
        .expect("append first");

    let fork = store.fork_context(first.turn_id).expect("fork context");

    let second_payload = b"hello world".to_vec();
    let mut hasher2 = Hasher::new();
    hasher2.update(&second_payload);
    let hash2 = hasher2.finalize();

    let _second = store
        .append_turn(
            fork.context_id,
            0,
            "com.example.Test".to_string(),
            1,
            1,
            0,
            second_payload.len() as u32,
            *hash2.as_bytes(),
            &second_payload,
        )
        .expect("append second");

    assert!(store.blob_store.contains(hash.as_bytes()));

    let last = store.get_last(fork.context_id, 10, true).expect("get last");
    assert_eq!(last.len(), 2);
    assert_eq!(last[0].record.turn_id, first.turn_id);
}

#[test]
fn data_persists_across_reopen() {
    let dir = tempdir().expect("tempdir");

    let payload = b"persist me".to_vec();
    let mut hasher = Hasher::new();
    hasher.update(&payload);
    let hash = hasher.finalize();

    let (context_id, turn_id) = {
        let mut store = Store::open(dir.path()).expect("open store");
        let ctx = store.create_context(0).expect("create context");
        let (turn, _meta) = store
            .append_turn(
                ctx.context_id,
                0,
                "com.example.Persist".to_string(),
                1,
                1,
                0,
                payload.len() as u32,
                *hash.as_bytes(),
                &payload,
            )
            .expect("append turn");
        (ctx.context_id, turn.turn_id)
    }; // store dropped, files closed

    // Reopen the same directory — data should still be there.
    let mut store = Store::open(dir.path()).expect("reopen store");
    let contexts = store.list_recent_contexts(100);
    assert!(!contexts.is_empty(), "expected at least one context after reopen");
    let last = store.get_last(context_id, 10, true).expect("get last after reopen");
    assert_eq!(last.len(), 1, "expected one turn after reopen");
    assert_eq!(last[0].record.turn_id, turn_id);
    assert!(store.blob_store.contains(hash.as_bytes()), "blob should persist after reopen");
}
