# Binary Protocol

CXDB's binary protocol provides high-throughput, low-latency access for writer clients over persistent TCP connections.

## Connection

**Endpoint:** `:9009` (configurable via `CXDB_BIND`)

**Plain TCP** (development):
```go
conn, err := net.Dial("tcp", "localhost:9009")
```

**TLS** (production):
```go
conn, err := tls.Dial("tcp", "cxdb.example.com:9009", &tls.Config{})
```

## Frame Format

All messages use length-prefixed frames:

```
┌─────────────────────────────────────┐
│       Frame Header (16 bytes)       │
├─────────────────────────────────────┤
│       Payload (len bytes)           │
└─────────────────────────────────────┘
```

**Frame Header** (little-endian):

```rust
struct FrameHeader {
  len: u32        // Payload length in bytes
  msg_type: u16   // Message type code
  flags: u16      // Frame flags
  req_id: u64     // Request ID (for multiplexing)
}
```

## Message Types

| Code | Name | Direction | Description |
|------|------|-----------|-------------|
| 1 | HELLO | C→S, S→C | Handshake |
| 2 | CTX_CREATE | C→S, S→C | Create empty context |
| 3 | CTX_FORK | C→S, S→C | Fork from existing turn |
| 4 | GET_HEAD | C→S, S→C | Get current head |
| 5 | APPEND_TURN | C→S, S→C | Append new turn |
| 6 | GET_LAST | C→S, S→C | Get last N turns |
| 9 | GET_BLOB | C→S, S→C | Fetch blob by hash |
| 10 | ATTACH_FS | C→S, S→C | Attach filesystem tree to turn |
| 11 | PUT_BLOB | C→S, S→C | Store blob explicitly |
| 255 | ERROR | S→C | Error response |

## Message Flows

### 1. HELLO (Handshake)

**Request** (client → server):

```
msg_type: 1
len: variable
payload:
  protocol_version: u32       // 1
  client_tag_len: u32
  client_tag: [bytes]         // E.g., "myapp-v1.2.3"
```

**Response** (server → client):

```
msg_type: 1
len: variable
payload:
  protocol_version: u32       // 1
  session_id: u64
  server_tag_len: u32
  server_tag: [bytes]         // E.g., "cxdb-v1.0.0"
```

### 2. CTX_CREATE (Create Context)

**Request:**

```
msg_type: 2
len: 8
payload:
  base_turn_id: u64           // 0 for empty context
```

**Response:**

```
msg_type: 2
len: 20
payload:
  context_id: u64
  head_turn_id: u64
  head_depth: u32
```

**Example:**

```
Request:  [len=8] [type=2] [flags=0] [req_id=1] [base_turn_id=0]
Response: [len=20] [type=2] [flags=0] [req_id=1] [context_id=1] [head_turn_id=0] [head_depth=0]
```

### 3. CTX_FORK (Fork Context)

**Request:**

```
msg_type: 3
len: 8
payload:
  base_turn_id: u64           // Turn to fork from
```

**Response:**

```
msg_type: 3
len: 20
payload:
  context_id: u64             // New context ID
  head_turn_id: u64           // = base_turn_id
  head_depth: u32
```

### 4. GET_HEAD (Get Context Head)

**Request:**

```
msg_type: 4
len: 8
payload:
  context_id: u64
```

**Response:**

```
msg_type: 4
len: 20
payload:
  context_id: u64
  head_turn_id: u64
  head_depth: u32
```

### 5. APPEND_TURN (Append Turn to Context)

**Request:**

```
msg_type: 5
len: variable
flags: bit 0 = has_fs_root (optional filesystem attachment)
payload:
  context_id: u64
  parent_turn_id: u64              // 0 = use current head

  declared_type_id_len: u32
  declared_type_id: [bytes]        // E.g., "com.example.Message"
  declared_type_version: u32

  encoding: u32                    // 1 = msgpack
  compression: u32                 // 0 = none, 1 = zstd
  uncompressed_len: u32
  content_hash_b3_256: [32]u8      // BLAKE3-256

  payload_len: u32
  payload_bytes: [payload_len]     // Compressed if compression != 0

  idempotency_key_len: u32
  idempotency_key: [bytes]         // Optional but recommended

  // If flags & 1:
  fs_root_hash: [32]u8             // Filesystem tree root hash
```

**Response:**

```
msg_type: 5
len: 52
payload:
  context_id: u64
  new_turn_id: u64
  new_depth: u32
  content_hash_b3_256: [32]u8
```

**Server Behavior:**

1. Resolve parent: If `parent_turn_id != 0`, use it; else use current head
2. Decompress payload if `compression != 0`
3. Verify `uncompressed_len` matches decompressed size
4. Compute `BLAKE3(uncompressed_bytes)` and verify against `content_hash_b3_256`
5. Store blob in CAS (deduplicated)
6. Append turn record to `turns.log`
7. Update context head to new turn
8. Return new `turn_id` and `depth`

**Idempotency:**
- If `idempotency_key` is provided and matches an existing append, return the existing turn
- Idempotency keys are unique per context and expire after 24 hours

### 6. GET_LAST (Get Last N Turns)

**Request:**

```
msg_type: 6
len: 16
payload:
  context_id: u64
  limit: u32                       // Max turns to return
  include_payload: u32             // 0 = metadata only, 1 = include payloads
```

**Response:**

```
msg_type: 6
len: variable
payload:
  count: u32
  items[count]:
    turn_id: u64
    parent_turn_id: u64
    depth: u32
    declared_type_id_len: u32
    declared_type_id: [bytes]
    declared_type_version: u32
    encoding: u32
    compression: u32               // Always 0 in response (uncompressed)
    uncompressed_len: u32
    content_hash_b3_256: [32]u8
    payload_len: u32               // Only if include_payload=1
    payload_bytes: [payload_len]   // Only if include_payload=1
```

**Notes:**
- Turns are returned oldest → newest (chronological order)
- If `include_payload=1`, payloads are decompressed by the server
- For paging, use `GET_BEFORE` (not yet in v1 - use HTTP API for paging)

### 7. GET_BLOB (Fetch Blob by Hash)

**Request:**

```
msg_type: 9
len: 32
payload:
  content_hash_b3_256: [32]u8
```

**Response:**

```
msg_type: 9
len: variable
payload:
  raw_len: u32
  raw_bytes: [raw_len]             // Uncompressed
```

**Error Response:**
- If blob not found, returns ERROR frame with code 404

### 8. ATTACH_FS (Attach Filesystem Tree)

Attach a filesystem tree to an existing turn (post-hoc).

**Request:**

```
msg_type: 10
len: 40
payload:
  turn_id: u64
  fs_root_hash: [32]u8             // Root hash of merkle tree
```

**Response:**

```
msg_type: 10
len: 40
payload:
  turn_id: u64
  fs_root_hash: [32]u8
```

**Notes:**
- Filesystem trees are stored separately from turn payloads
- The tree must be uploaded via `PUT_BLOB` calls before attaching
- See filesystem tree spec (future doc) for merkle tree format

### 9. PUT_BLOB (Store Blob Explicitly)

Store a blob without creating a turn (useful for pre-uploading large blobs or filesystem trees).

**Request:**

```
msg_type: 11
len: variable
payload:
  content_hash_b3_256: [32]u8
  raw_len: u32
  raw_bytes: [raw_len]             // Uncompressed
```

**Response:**

```
msg_type: 11
len: 33
payload:
  content_hash_b3_256: [32]u8
  was_new: u8                      // 1 = newly stored, 0 = already existed
```

**Server Behavior:**
1. Verify `BLAKE3(raw_bytes) == content_hash_b3_256`
2. Check if blob exists (dedup)
3. If new, compress and write to blob store
4. Return `was_new` flag

### 10. ERROR (Error Response)

**Response:**

```
msg_type: 255
len: variable
payload:
  code: u32                        // HTTP-style error code
  detail_len: u32
  detail_bytes: [detail_len]       // UTF-8 JSON or plain text
```

**Common Error Codes:**

| Code | Meaning |
|------|---------|
| 400 | Bad request (malformed frame) |
| 404 | Not found (context/turn/blob) |
| 409 | Conflict (hash mismatch, invalid parent) |
| 422 | Unprocessable (invalid type_id, missing registry) |
| 500 | Internal error (storage failure, corruption) |

**Example Error:**

```json
{
  "code": "HASH_MISMATCH",
  "message": "Content hash verification failed",
  "details": {
    "expected": "a3f5b8c2...",
    "actual": "b4e6c9d3..."
  }
}
```

## Client Implementation Guide

### Connection Management

**Keep connections alive:**
- Binary protocol uses persistent connections
- Send HELLO on connect
- Reuse connection for multiple requests
- Implement reconnect with exponential backoff

**Multiplexing:**
- Use unique `req_id` for each request
- Multiple requests can be in-flight simultaneously
- Match responses to requests by `req_id`

### Request Pipeline

```
1. Serialize payload → bytes
2. Compute len = length(payload)
3. Write frame header: [len, msg_type, flags, req_id]
4. Write payload bytes
5. Flush writer
6. Read response frame header
7. Read response payload (using len from header)
8. Deserialize and return
```

### Error Handling

- Check `msg_type == 255` for error responses
- Parse error code and details
- Retry idempotent operations (CREATE, FORK, APPEND with idempotency key)
- Don't retry on 4xx errors (client error)

### Compression

**Sending compressed payloads:**

```go
import "github.com/klauspost/compress/zstd"

// Compress
encoder, _ := zstd.NewWriter(nil)
compressed := encoder.EncodeAll(uncompressed, nil)

// Send APPEND_TURN
req := AppendRequest{
  Encoding:        1,  // msgpack
  Compression:     1,  // zstd
  UncompressedLen: len(uncompressed),
  ContentHash:     blake3.Sum256(uncompressed),
  Payload:         compressed,
}
```

**Receiving:**
- Server always returns uncompressed payloads (`compression=0`)
- No client-side decompression needed

## Performance Tips

**Batch operations:**
- Pipeline multiple APPEND_TURN requests (send all, then read all responses)
- Use async/concurrent clients for maximum throughput

**Compression:**
- Compress payloads >1KB
- Skip compression for tiny payloads (<128 bytes)
- Zstd level 3 is a good default (fast + decent ratio)

**Idempotency:**
- Always provide `idempotency_key` for APPEND_TURN
- Use UUIDs or `{client_id}:{timestamp}:{sequence}` format

**Connection pooling:**
- For high-concurrency apps, use a connection pool (e.g., 10 connections)
- Round-robin requests across pool

## Example Flow: Append Two Turns

```
Client                                   Server
  │
  ├─ HELLO ────────────────────────────→ │
  │                                       │
  │ ←─────────────────────────── HELLO ──┤  (session_id=12345)
  │
  ├─ APPEND_TURN (req_id=1) ───────────→ │
  │  context_id=1                         │
  │  parent_turn_id=0                     │
  │  type_id="com.example.Message"        │
  │  payload=<msgpack bytes>              │
  │                                       │
  │ ←─────────────── APPEND_TURN ACK ────┤  (req_id=1)
  │                  turn_id=1            │
  │                  depth=1              │
  │
  ├─ APPEND_TURN (req_id=2) ───────────→ │
  │  context_id=1                         │
  │  parent_turn_id=0  (use head)         │
  │  type_id="com.example.Message"        │
  │  payload=<msgpack bytes>              │
  │                                       │
  │ ←─────────────── APPEND_TURN ACK ────┤  (req_id=2)
  │                  turn_id=2            │
  │                  depth=2              │
  │
  ├─ GET_LAST (req_id=3) ──────────────→ │
  │  context_id=1                         │
  │  limit=10                             │
  │  include_payload=1                    │
  │                                       │
  │ ←──────────────── GET_LAST ──────────┤  (req_id=3)
  │                   count=2             │
  │                   turns=[1, 2]        │
  │
```

## Debugging

**Enable protocol tracing:**

```bash
CXDB_LOG_LEVEL=debug CXDB_TRACE_PROTOCOL=1 ./ai-cxdb-store
```

**Wireshark:**
- Binary protocol is not encrypted by default (plain TCP)
- Capture on port 9009: `tcpdump -i lo0 -w cxdb.pcap port 9009`

**Manual testing:**

```bash
# Connect with netcat
nc localhost 9009

# Send raw bytes (hex)
echo -n "..." | xxd -r -p | nc localhost 9009 | xxd
```

See [troubleshooting.md](troubleshooting.md) for more debugging tips.

## Future Extensions (v2)

Planned protocol additions:

- `GET_BEFORE` - Cursor-based paging
- `GET_RANGE` - Fetch turn range by depth
- `STREAM_APPEND` - Streaming turn updates
- `SUBSCRIBE` - Real-time turn notifications
- `BATCH_APPEND` - Multi-turn atomic append

## Reference Implementation

See `clients/go/client.go` for a complete Go client implementation:

```go
import "github.com/strongdm/cxdb/clients/go"

client, err := cxdb.Dial("localhost:9009")
if err != nil {
    log.Fatal(err)
}
defer client.Close()
```
