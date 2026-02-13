# cxdb (Rust client)

Rust-native CXDB client with 1:1 wire parity to the Go client. This crate provides a synchronous TCP/TLS client, reconnection wrapper, filesystem snapshot helpers (`fstree`), and canonical conversation types.

## Quick Start

```rust
use cxdb::types::{new_user_input, TypeIDConversationItem, TypeVersionConversationItem};
use cxdb::{dial, encode_msgpack, AppendRequest, GetLastOptions, RequestContext};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = dial("127.0.0.1:9009", Vec::new())?;
    let ctx = RequestContext::background();

    let head = client.create_context(&ctx, 0)?;
    let payload = encode_msgpack(&new_user_input("Hello", Vec::new()))?;
    client.append_turn(
        &ctx,
        &AppendRequest::new(head.context_id, TypeIDConversationItem, TypeVersionConversationItem, payload),
    )?;

    let turns = client.get_last(&ctx, head.context_id, GetLastOptions::default())?;
    println!("fetched {} turns", turns.len());
    Ok(())
}
```

## Fstree snapshots

```rust
use cxdb::fstree;
use cxdb::types::{new_user_input, TypeIDConversationItem, TypeVersionConversationItem};
use cxdb::{dial, encode_msgpack, AppendRequest, RequestContext};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = dial("127.0.0.1:9009", Vec::new())?;
    let ctx = RequestContext::background();
    let head = client.create_context(&ctx, 0)?;

    let snapshot = fstree::capture(".", vec![fstree::with_exclude(vec![".git", "target"])])?;
    snapshot.upload(&ctx, &client)?;

    let payload = encode_msgpack(&new_user_input("Snapshot attached", Vec::new()))?;
    client.append_turn_with_fs(
        &ctx,
        &AppendRequest::new(head.context_id, TypeIDConversationItem, TypeVersionConversationItem, payload),
        Some(snapshot.root_hash),
    )?;
    Ok(())
}
```

## Reconnecting client

```rust
use cxdb::{dial_reconnecting, RequestContext, ReconnectOption};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = dial_reconnecting(
        "127.0.0.1:9009",
        Vec::<ReconnectOption>::new(),
        Vec::new(),
    )?;
    let ctx = RequestContext::background();
    let _ = client.create_context(&ctx, 0)?;
    Ok(())
}
```

## SSE subscriptions

```rust
use cxdb::{subscribe_events, RequestContext};

fn main() {
    let ctx = RequestContext::background();
    let (events, errs) = subscribe_events(&ctx, "http://127.0.0.1:9010/v1/events", Vec::new());
    for ev in events.iter() {
        println!("event: {}", ev.event_type);
    }
    for err in errs.iter() {
        eprintln!("subscribe error: {}", err);
    }
}
```

## cxdb-subscribe CLI

```bash
cargo run -p cxdb --bin cxdb-subscribe -- \
  --cxdb-events-url http://127.0.0.1:9010/v1/events \
  --follow-turns \
  --cxdb-bin-addr 127.0.0.1:9009
```

## Msgpack helpers

- `encode_msgpack` emits deterministic map ordering (matching Go’s `SetSortMapKeys(true)`).
- Struct field tags use digit-strings (e.g., `"1"`, `"30"`) so encoded payloads match Go.
- Optional fields serialize as explicit `nil`, matching Go’s msgpack behavior.

## Examples

Run the bundled examples from this crate:

```bash
cargo run --example basic
cargo run --example fstree_snapshot
```

## Integration tests

Integration tests are gated by environment variables:

```bash
export CXDB_INTEGRATION=1
export CXDB_TEST_ADDR=127.0.0.1:9009
export CXDB_TEST_HTTP_ADDR=http://127.0.0.1:9010
cargo test -p cxdb
```

## Parity notes

- Wire format and message types follow `docs/protocol.md` and the Go client implementation.
- Fstree tree serialization is validated against Go-generated fixtures.
- Canonical types use the same msgpack tags and optional-field semantics as Go.
