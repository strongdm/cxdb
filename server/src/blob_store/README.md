# Blob Store Module

Content-addressed storage for turn payloads with deduplication and compression.

## Overview

The blob store provides immutable, content-addressed storage for arbitrary binary blobs. Each blob is identified by its BLAKE3-256 hash, enabling automatic deduplication.

## Architecture

```
┌──────────────────────────────────────────┐
│           BlobStore                      │
├──────────────────────────────────────────┤
│                                          │
│  ┌────────────┐      ┌───────────────┐  │
│  │            │      │               │  │
│  │  In-Memory │◄────►│  blobs.pack   │  │
│  │   Index    │      │  (append-only)│  │
│  │            │      │               │  │
│  │ hash → loc │      │ [blob][blob]  │  │
│  │            │      │ [blob]...     │  │
│  └────────────┘      └───────────────┘  │
│        │                                 │
│        └─────────┬───────────────────────┘
│                  │
│           ┌──────▼─────┐
│           │ blobs.idx  │
│           │ (on-disk   │
│           │  index)    │
│           └────────────┘
└──────────────────────────────────────────┘
```

## Storage Format

### Pack File (`blobs.pack`)

Append-only file containing blob records:

```rust
BlobRecord {
  magic: u32 = 0x42534C42      // 'B''S''L''B'
  version: u16 = 1
  codec: u16                    // 0=none, 1=zstd
  raw_len: u32                  // Uncompressed size
  stored_len: u32               // Compressed size (or raw if codec=0)
  hash: [32]u8                  // BLAKE3-256
  stored_bytes: [stored_len]    // Actual data
  crc32: u32                    // CRC-32 of header+data
}
```

**Record layout:**

```
Offset  Size  Field
0       4     magic
4       2     version
6       2     codec
8       4     raw_len
12      4     stored_len
16      32    hash
48      N     stored_bytes
48+N    4     crc32
```

### Index File (`blobs.idx`)

Fixed-size entries mapping hash → location:

```rust
BlobIndexEntry {
  hash: [32]u8        // BLAKE3-256
  pack_offset: u64    // Byte offset in blobs.pack
  raw_len: u32        // Uncompressed size
  stored_len: u32     // Compressed size
  codec: u16          // Codec used
  reserved: u16       // Future use
}
```

**Entry size:** 52 bytes

## API

### Opening the Store

```rust
use blob_store::BlobStore;

let store = BlobStore::open(Path::new("./data/blobs"))?;
```

Creates or opens:
- `./data/blobs/blobs.pack`
- `./data/blobs/blobs.idx`

### Storing a Blob

```rust
let data = b"Hello, world!";
let hash = blake3::hash(data);

// Put will compress and deduplicate
let was_new = store.put(&hash, data)?;

if was_new {
    println!("New blob stored");
} else {
    println!("Blob already exists (deduplicated)");
}
```

**Compression:**
- If `compressed_size >= raw_size`, stores uncompressed (codec=0)
- Otherwise stores compressed (codec=1, Zstd level 3)

### Retrieving a Blob

```rust
let hash: [u8; 32] = /* ... */;

match store.get(&hash)? {
    Some(data) => println!("Found: {} bytes", data.len()),
    None => println!("Blob not found"),
}
```

Returns decompressed bytes.

### Checking Existence

```rust
if store.contains(&hash) {
    println!("Blob exists");
}
```

**Fast:** O(1) hash table lookup in memory.

## Deduplication

The blob store automatically deduplicates identical content:

```rust
let data1 = b"foo";
let hash1 = blake3::hash(data1);

store.put(&hash1, data1)?;  // Writes to pack file
store.put(&hash1, data1)?;  // No-op (already exists)
```

**Thread safety:**
- Uses sharded locks (16 shards by hash prefix)
- Double-checked locking: check index, acquire lock, check again, write if missing
- Safe under concurrent writes

## Compression

### Zstd Level 3

Default compression level trades speed for compression ratio:

- **Compression speed:** ~400 MB/s
- **Decompression speed:** ~1000 MB/s
- **Ratio:** ~70% reduction on JSON/msgpack data

### When to Skip Compression

```rust
// Small blobs (<128 bytes) stored uncompressed
if data.len() < 128 {
    codec = BlobCodec::None;
}

// Already compressed data (e.g., PNG, JPEG)
if is_already_compressed(data) {
    codec = BlobCodec::None;
}
```

## Hash Computation

Always use BLAKE3 on **uncompressed** data:

```rust
use blake3;

let data = b"...";
let hash = blake3::hash(data);  // Hash before compression

// Compress for storage
let compressed = zstd::encode(data, 3)?;

// Store with original hash
store.put(&hash, data)?;  // Handles compression internally
```

**Why BLAKE3?**
- Fast: 3-4x faster than SHA-256
- Secure: 256-bit output, collision-resistant
- Deterministic: Same input always produces same hash

## Crash Recovery

On startup, the store:

1. Loads `blobs.idx` into memory
2. Scans for partial/corrupt index entries
3. Truncates to last valid entry
4. Rebuilds if necessary

**CRC verification:**
- Each blob record has a CRC-32
- On read, CRC is verified
- Corrupted blobs return error (caller must handle)

## Performance Characteristics

| Operation | Complexity | Typical Latency |
|-----------|------------|-----------------|
| `put()` (new) | O(1) + compress | ~1ms (10KB) |
| `put()` (exists) | O(1) | ~0.01ms |
| `get()` | O(1) + decompress | ~0.5ms (10KB) |
| `contains()` | O(1) | <0.01ms |

**Assumptions:**
- SSD storage (500 MB/s sequential write)
- 10KB average blob size
- 70% compression ratio
- Warm in-memory index

## Memory Usage

**In-memory index:**
- Each blob: 52 bytes (hash + metadata)
- 1M blobs: ~50 MB
- 10M blobs: ~500 MB

## Example Usage

### Complete Flow

```rust
use blob_store::{BlobStore, BlobCodec};
use blake3;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Open store
    let mut store = BlobStore::open(Path::new("./data/blobs"))?;

    // Prepare data
    let payload = serde_json::json!({
        "role": "user",
        "text": "Hello, CXDB!"
    });
    let msgpack_bytes = rmp_serde::to_vec(&payload)?;

    // Compute hash (before compression)
    let hash = blake3::hash(&msgpack_bytes);

    // Store (will compress internally)
    let was_new = store.put(&hash, &msgpack_bytes)?;
    println!("Stored: was_new={}", was_new);

    // Retrieve
    let retrieved = store.get(&hash)?
        .expect("Blob should exist");

    assert_eq!(retrieved, msgpack_bytes);
    println!("Retrieved: {} bytes", retrieved.len());

    Ok(())
}
```

## Limitations (v1)

- **No garbage collection:** Blobs are never deleted
- **No replication:** Single-node only
- **No sub-blob dedup:** Entire blob must match for deduplication
- **No encryption:** Blobs stored in plaintext (use disk encryption)

## Future Enhancements (v2)

- **Garbage collection:** Remove unreferenced blobs
- **Content-defined chunking:** Deduplicate similar blobs
- **Encryption:** Optional at-rest encryption
- **Replication:** Multi-node blob storage
- **S3 backend:** Use object storage instead of local disk

## Testing

```bash
# Run blob store tests
cargo test --package ai-cxdb-store --lib blob_store

# With output
cargo test --package ai-cxdb-store --lib blob_store -- --nocapture

# Specific test
cargo test test_blob_deduplication
```

## Debugging

Enable debug logs:

```bash
RUST_LOG=blob_store=debug cargo run
```

Inspect files:

```bash
# Check pack file size
ls -lh data/blobs/blobs.pack

# Count blobs in index
stat -c%s data/blobs/blobs.idx
# Divide by 52 (entry size) for count

# Verify pack file integrity
xxd data/blobs/blobs.pack | head -20
# Should start with: 0x42534C42 (magic)
```

## See Also

- [Storage Format](../../docs/storage.md) - Detailed file format spec
- [Architecture](../../docs/architecture.md) - Overall system design
- [Turn Store](../turn_store/README.md) - Related metadata storage
