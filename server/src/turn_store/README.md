# Turn Store Module

Append-only DAG storage for turn metadata with crash-safe operations.

## Overview

The turn store manages the Turn DAG - an immutable directed acyclic graph where each turn has exactly one parent (or is a root). It stores turn metadata (parent links, depth, payload hash) separate from the actual payload bytes (which live in the blob store).

## Architecture

```
┌────────────────────────────────────────────┐
│            TurnStore                       │
├────────────────────────────────────────────┤
│                                            │
│  ┌──────────┐     ┌──────────────────┐    │
│  │ Contexts │     │   turns.log      │    │
│  │ (in mem) │     │   (append-only)  │    │
│  │          │     │                  │    │
│  │ ctx_id   │     │ [turn][turn]...  │    │
│  │   ↓      │     │                  │    │
│  │ head_id  │     └──────────────────┘    │
│  └──────────┘              │               │
│       │                    │               │
│       │             ┌──────▼────────┐      │
│       │             │  turns.idx    │      │
│       │             │  (turn_id →   │      │
│       │             │   offset)     │      │
│       │             └───────────────┘      │
│       │                                    │
│       │             ┌───────────────┐      │
│       └────────────►│  heads.tbl    │      │
│                     │  (append-only)│      │
│                     │  [ctx][ctx]..│      │
│                     └───────────────┘      │
│                                            │
│                     ┌───────────────┐      │
│                     │  turns.meta   │      │
│                     │  (type info)  │      │
│                     └───────────────┘      │
└────────────────────────────────────────────┘
```

## Storage Format

### Turn Log (`turns.log`)

Fixed-size turn records (104 bytes each):

```rust
TurnRecordV1 {
  turn_id: u64               // Unique, monotonic
  parent_turn_id: u64        // 0 for root
  depth: u32                 // parent.depth + 1
  codec: u32                 // Reserved (unused in v1)
  type_tag: u64              // Reserved (unused in v1)
  payload_hash: [32]u8       // BLAKE3 of payload
  flags: u32                 // Reserved for tombstone/overlay
  created_at_unix_ms: u64    // Timestamp
  crc32: u32                 // CRC-32 checksum
}
```

### Turn Index (`turns.idx`)

Fixed-size entries (16 bytes each):

```rust
TurnIndexEntry {
  turn_id: u64         // Turn ID
  offset: u64          // Byte offset in turns.log
}
```

### Turn Metadata (`turns.meta`)

Variable-length records:

```rust
TurnMeta {
  turn_id: u64
  declared_type_id_len: u32
  declared_type_id: [bytes]      // E.g., "com.example.Message"
  declared_type_version: u32
  encoding: u32                  // 1 = msgpack
  compression: u32               // 0 = none, 1 = zstd (historical, unused at rest)
  uncompressed_len: u32
}
```

### Context Heads (`heads.tbl`)

Append-only, last-write-wins:

```rust
ContextHeadRecord {
  context_id: u64
  head_turn_id: u64
  head_depth: u32
  flags: u32
  created_at_unix_ms: u64
  crc32: u32
}
```

## API

### Creating a Context

```rust
use turn_store::TurnStore;

let mut store = TurnStore::open(Path::new("./data/turns"))?;

// Create empty context
let ctx = store.create_context(0)?;  // 0 = empty
println!("Context {} created", ctx.context_id);

// Create context from existing turn
let ctx2 = store.create_context(42)?;  // Start from turn 42
```

### Appending a Turn

```rust
let payload_hash: [u8; 32] = /* computed elsewhere */;

let turn = store.append_turn(
    context_id,
    parent_turn_id,  // 0 = use current head
    payload_hash,
    "com.example.Message",  // type_id
    1,                       // type_version
)?;

println!("Appended turn {} at depth {}", turn.turn_id, turn.depth);
```

**Atomicity:**
1. Allocate unique `turn_id` (atomic increment)
2. Resolve parent (if parent_turn_id == 0, use context head)
3. Write turn record to `turns.log`
4. Write metadata to `turns.meta`
5. Update context head in `heads.tbl`
6. Update in-memory index

### Getting Last N Turns

```rust
let turns = store.get_last(context_id, 10)?;

for turn in turns {
    println!("Turn {}: parent={}, depth={}",
        turn.turn_id,
        turn.parent_turn_id,
        turn.depth
    );
}
```

Returns turns in chronological order (oldest → newest).

### Walking the Chain

```rust
// Get full chain from root to head
let chain = store.walk_to_root(turn_id)?;

// Reverse to get chronological order
for turn in chain.iter().rev() {
    println!("{} → ", turn.turn_id);
}
```

## Turn ID Allocation

Turn IDs are allocated from a global atomic counter:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TURN_ID: AtomicU64 = AtomicU64::new(1);

fn allocate_turn_id() -> u64 {
    NEXT_TURN_ID.fetch_add(1, Ordering::SeqCst)
}
```

**Properties:**
- Monotonically increasing
- Globally unique
- Thread-safe
- Never reused

## Context Head Management

Context heads are mutable pointers to the latest turn in a branch:

```rust
// Get current head
let head = store.get_head(context_id)?;
println!("Head: turn_id={}, depth={}", head.head_turn_id, head.head_depth);

// Appending updates head automatically
store.append_turn(context_id, 0, ...)?;
let new_head = store.get_head(context_id)?;
```

**Concurrency:**
- Per-context mutex guards head updates
- Different contexts can be updated concurrently
- Same context updates are serialized

## Crash Recovery

On startup, the turn store:

1. **Scan `turns.log`:**
   - Read records sequentially
   - Verify CRC for each record
   - Truncate to last valid record if corruption found

2. **Rebuild index:**
   - Load all valid turn records into `turn_id → offset` map
   - Write updated `turns.idx`

3. **Load context heads:**
   - Scan `heads.tbl` (last write wins)
   - Build `context_id → head_turn_id` map

4. **Load metadata:**
   - Scan `turns.meta`
   - Build `turn_id → TurnMeta` map

**Example recovery log:**

```
INFO: Loading turn store from ./data/turns
INFO: Found 1000 turn records
WARN: CRC mismatch at offset 104000, truncating
INFO: Truncated to 999 valid records
INFO: Rebuilt index with 999 entries
INFO: Loaded 50 context heads
INFO: Turn store ready
```

## Branching (Forking)

Create a new context from an existing turn:

```rust
// Context 1: main branch
//   turn_1 → turn_2 → turn_3

// Fork from turn_2
let ctx2 = store.create_context(turn_2_id)?;

// Context 2 shares history up to turn_2
//   turn_1 → turn_2 → turn_4 (new in ctx2)
```

**DAG structure:**

```
turn_1 → turn_2 → turn_3 (context 1)
              ↘
                turn_4 (context 2)
```

## Performance

| Operation | Complexity | Latency |
|-----------|------------|---------|
| `create_context()` | O(1) | <1ms |
| `append_turn()` | O(1) | <1ms |
| `get_last(N)` | O(N) | ~0.5ms for N=10 |
| `walk_to_root(depth=D)` | O(D) | ~D * 0.1ms |
| `get_head()` | O(1) | <0.01ms |

**Assumptions:**
- SSD storage
- In-memory index hot
- Typical depth < 100

## Memory Usage

**In-memory structures:**
- Turn index: 16 bytes per turn
- Context heads: ~50 bytes per context
- Metadata cache: ~100 bytes per turn (on demand)

**Example:**
- 1M turns: ~16 MB index
- 10K contexts: ~500 KB heads
- Total: ~20 MB

## Thread Safety

**Per-context locking:**

```rust
use std::sync::Mutex;
use std::collections::HashMap;

struct TurnStore {
    contexts: HashMap<u64, Mutex<ContextState>>,
    // ...
}
```

**Concurrent operations:**

```
Thread 1: append to context 1   │ OK (different context)
Thread 2: append to context 2   │

Thread 1: append to context 1   │ Serialized (same context)
Thread 2: append to context 1   │ (waits for lock)
```

## Example Usage

### Complete Flow

```rust
use turn_store::TurnStore;
use blake3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open store
    let mut store = TurnStore::open(Path::new("./data/turns"))?;

    // Create context
    let ctx = store.create_context(0)?;
    println!("Created context {}", ctx.context_id);

    // Prepare payload
    let payload = b"Hello, world!";
    let payload_hash = blake3::hash(payload);

    // Append turn
    let turn1 = store.append_turn(
        ctx.context_id,
        0,  // Use current head
        payload_hash,
        "com.example.Message",
        1,
    )?;
    println!("Turn 1: id={}, depth={}", turn1.turn_id, turn1.depth);

    // Append another turn
    let payload2 = b"Second turn";
    let payload_hash2 = blake3::hash(payload2);

    let turn2 = store.append_turn(
        ctx.context_id,
        0,
        payload_hash2,
        "com.example.Message",
        1,
    )?;
    println!("Turn 2: id={}, depth={}", turn2.turn_id, turn2.depth);

    // Get last 10 turns
    let turns = store.get_last(ctx.context_id, 10)?;
    println!("Retrieved {} turns", turns.len());

    // Fork from turn 1
    let ctx2 = store.create_context(turn1.turn_id)?;
    println!("Forked context {}", ctx2.context_id);

    Ok(())
}
```

## Testing

```bash
# Run turn store tests
cargo test --package ai-cxdb-store --lib turn_store

# Test crash recovery
cargo test test_turn_store_recovery

# Test concurrent appends
cargo test test_concurrent_appends
```

## Debugging

Enable debug logs:

```bash
RUST_LOG=turn_store=debug cargo run
```

Inspect files:

```bash
# Turn count
stat -c%s data/turns/turns.log
# Divide by 104 (record size)

# Context count
grep -c "^" data/turns/heads.tbl

# Check for corruption
xxd data/turns/turns.log | grep "00 00 00 00 00 00"
```

## Limitations (v1)

- **No deletion:** Turns are never deleted
- **No random access by depth:** Must walk from head
- **Single-process:** No distributed consensus
- **No transaction batching:** Each append is separate

## Future Enhancements (v2)

- **Skip pointers:** O(log N) access by depth
- **Batch appends:** Atomic multi-turn writes
- **Compaction:** Remove orphaned branches
- **Replication:** Multi-node turn storage

## See Also

- [Storage Format](../../docs/storage.md) - File format details
- [Blob Store](../blob_store/README.md) - Payload storage
- [Architecture](../../docs/architecture.md) - System design
