# Protocol Module

Binary protocol frame parsing and message handling.

## Overview

The protocol module implements CXDB's binary wire protocol: length-prefixed frames over persistent TCP connections. It handles frame serialization/deserialization, request routing, and response generation.

## Frame Format

All messages use a 16-byte header followed by a variable-length payload:

```
┌────────────────────────┬──────────────────┐
│   Frame Header (16B)   │   Payload (N)    │
└────────────────────────┴──────────────────┘
```

### Header Structure

```rust
struct FrameHeader {
  len: u32        // Payload length in bytes
  msg_type: u16   // Message type code
  flags: u16      // Frame flags
  req_id: u64     // Request ID for multiplexing
}
```

**Little-endian:** All fields use little-endian byte order.

## Message Types

| Code | Name | Description |
|------|------|-------------|
| 1 | `HELLO` | Handshake and version negotiation |
| 2 | `CTX_CREATE` | Create new context |
| 3 | `CTX_FORK` | Fork from existing turn |
| 4 | `GET_HEAD` | Get context head |
| 5 | `APPEND_TURN` | Append turn to context |
| 6 | `GET_LAST` | Get last N turns |
| 9 | `GET_BLOB` | Fetch blob by hash |
| 10 | `ATTACH_FS` | Attach filesystem tree |
| 11 | `PUT_BLOB` | Store blob |
| 255 | `ERROR` | Error response |

## API

### Reading a Frame

```rust
use protocol::{FrameHeader, read_frame};

async fn handle_connection(mut stream: TcpStream) -> Result<()> {
    loop {
        // Read header
        let header = FrameHeader::read_from(&mut stream).await?;

        // Read payload
        let mut payload = vec![0u8; header.len as usize];
        stream.read_exact(&mut payload).await?;

        // Dispatch based on msg_type
        match header.msg_type {
            MSG_APPEND_TURN => handle_append(header.req_id, &payload).await?,
            MSG_GET_LAST => handle_get_last(header.req_id, &payload).await?,
            _ => return Err(Error::UnknownMessageType(header.msg_type)),
        }
    }
}
```

### Writing a Frame

```rust
use protocol::{FrameHeader, write_frame};

async fn send_response(stream: &mut TcpStream, req_id: u64, data: &[u8]) -> Result<()> {
    let header = FrameHeader {
        len: data.len() as u32,
        msg_type: MSG_APPEND_TURN_ACK,
        flags: 0,
        req_id,
    };

    header.write_to(stream).await?;
    stream.write_all(data).await?;
    stream.flush().await?;

    Ok(())
}
```

## Message Handlers

### HELLO

Establishes connection and exchanges version info:

```rust
// Client → Server
HelloRequest {
  protocol_version: u32,
  client_tag: String,
}

// Server → Client
HelloResponse {
  protocol_version: u32,
  session_id: u64,
  server_tag: String,
}
```

### APPEND_TURN

Appends a new turn:

```rust
AppendTurnRequest {
  context_id: u64,
  parent_turn_id: u64,
  declared_type_id: String,
  declared_type_version: u32,
  encoding: u32,
  compression: u32,
  uncompressed_len: u32,
  content_hash: [u8; 32],
  payload: Vec<u8>,
  idempotency_key: Option<String>,
  fs_root_hash: Option<[u8; 32]>,  // If flags & 1
}

AppendTurnResponse {
  context_id: u64,
  new_turn_id: u64,
  new_depth: u32,
  content_hash: [u8; 32],
}
```

### GET_LAST

Retrieves last N turns:

```rust
GetLastRequest {
  context_id: u64,
  limit: u32,
  include_payload: bool,
}

GetLastResponse {
  turns: Vec<TurnData>,
}
```

## Error Handling

Errors are returned as `ERROR` frames:

```rust
ErrorResponse {
  code: u32,       // HTTP-style error code
  detail: String,  // JSON or plain text
}
```

Send errors:

```rust
async fn send_error(stream: &mut TcpStream, req_id: u64, code: u32, msg: &str) -> Result<()> {
    let error = ErrorResponse {
        code,
        detail: msg.to_string(),
    };

    let payload = serde_json::to_vec(&error)?;

    let header = FrameHeader {
        len: payload.len() as u32,
        msg_type: MSG_ERROR,
        flags: 0,
        req_id,
    };

    header.write_to(stream).await?;
    stream.write_all(&payload).await?;
    stream.flush().await?;

    Ok(())
}
```

## Connection Management

### Keep-Alive

Connections are persistent. No explicit keep-alive messages needed.

### Multiplexing

Multiple requests can be in-flight using different `req_id` values:

```
Client                           Server
  │                                │
  ├─ APPEND (req_id=1) ─────────→ │
  ├─ GET_LAST (req_id=2) ────────→ │
  │                                │
  │ ←────────── ACK (req_id=2) ────┤  (out of order OK)
  │ ←────────── ACK (req_id=1) ────┤
```

### Graceful Shutdown

Server signals shutdown by closing the socket after responding to pending requests.

## Performance

**Frame overhead:** 16 bytes per message

**Throughput:**
- Small messages (<1KB): ~100K msg/sec
- Large messages (10KB): ~50K msg/sec
- Limited by CPU (serialization) on small messages
- Limited by network on large messages

## Thread Safety

The protocol module is stateless. Each connection is handled by a dedicated task/thread:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:9009").await?;

    loop {
        let (stream, addr) = listener.accept().await?;
        tokio::spawn(async move {
            handle_connection(stream, addr).await
        });
    }
}
```

## Testing

```bash
# Run protocol tests
cargo test --package ai-cxdb-store --lib protocol

# Test frame parsing
cargo test test_frame_header

# Test message handlers
cargo test test_append_turn_handler
```

## Debugging

Enable protocol tracing:

```bash
CXDB_LOG_LEVEL=debug CXDB_TRACE_PROTOCOL=1 cargo run
```

Output:

```
DEBUG: → APPEND_TURN req_id=1 len=1234
DEBUG: ← APPEND_TURN_ACK req_id=1 turn_id=42
```

Capture traffic:

```bash
tcpdump -i lo0 -w cxdb.pcap port 9009
wireshark cxdb.pcap
```

## See Also

- [Protocol Spec](../../docs/protocol.md) - Complete wire format
- [Architecture](../../docs/architecture.md) - System design
- [Client SDK](../../../clients/go/README.md) - Go client implementation
