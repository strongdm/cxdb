# CXDB Examples

This directory contains example applications demonstrating CXDB usage patterns.

## Prerequisites

- **Running CXDB server**: Start the server before running any examples
  ```bash
  # From the repository root
  cargo run --release
  ```

  The server will listen on:
  - Binary protocol: `localhost:9009`
  - HTTP API: `localhost:9010`

- **Language runtimes**:
  - Go 1.22+ (for Go examples)
  - Rust 1.75+ (for Rust examples)
  - Python 3.9+ (for Python examples)
  - Node.js 18+ (for renderer example)

## Examples

### 1. [basic-go/](basic-go/) - Basic Go Client

**What it demonstrates**: Core CXDB operations using the Go client SDK

**Operations**:
- Connect to CXDB binary protocol
- Create a context
- Append multiple turns (user input, assistant response, tool call)
- Retrieve turn history

**Run it**:
```bash
cd basic-go
go run main.go
```

**Use case**: Command-line tools, backend services, data pipelines

---

### 2. [basic-rust/](basic-rust/) - Basic Rust Client

**What it demonstrates**: Same operations as basic-go but using the Rust client SDK

**Operations**:
- Connect via binary protocol
- Context and turn management
- Type-safe turn appending

**Run it**:
```bash
cd basic-rust
cargo run
```

**Use case**: High-performance systems, embedded tools, CLI applications

---

### 3. [type-registration/](type-registration/) - Type Registry

**What it demonstrates**: Defining custom types with numeric field tags for forward-compatible schema evolution

**Concepts**:
- Msgpack encoding with numeric field tags
- Type registry bundle creation
- Publishing type descriptors to the server
- Semantic hints (timestamps, URLs, etc.)

**Run it**:
```bash
cd type-registration
go run main.go
```

**Use case**: Production systems with evolving data schemas, multi-version compatibility

---

### 4. [renderer-custom/](renderer-custom/) - Custom Renderer

**What it demonstrates**: Building a JavaScript renderer for rich turn visualization in the UI

**Concepts**:
- React component renderer API
- ESM module format
- Self-hosting on CDN
- Syntax-highlighted JSON display

**Build it**:
```bash
cd renderer-custom
npm install
npm run build
```

**Deploy it**:
```bash
# Upload dist/renderer.js to your CDN
aws s3 cp dist/renderer.js s3://your-bucket/renderers/log-entry@1.0.0.js --acl public-read
```

**Use case**: Custom data visualization, domain-specific UI components

---

### 5. [fstree-snapshot/](fstree-snapshot/) - Filesystem Snapshots

**What it demonstrates**: Capturing filesystem state and tracking changes across turns

**Operations**:
- Capture directory tree with merkle hashing
- Upload trees/files to blob store
- Attach filesystem to turn
- Diff snapshots to find changes

**Run it**:
```bash
cd fstree-snapshot
go run main.go
```

**Use case**: Build systems, deployment tracking, code generation auditing

---

### 6. [agent-integration/](agent-integration/) - AI Agent Integration

**What it demonstrates**: Simulating an AI agent conversation using the HTTP API

**Operations**:
- Create context with provenance metadata
- Append conversation turns (user, assistant, tool calls, tool results)
- Use canonical ConversationItem types
- Query and display formatted conversation

**Run it**:
```bash
cd agent-integration
pip install -r requirements.txt
python agent.py
```

**Use case**: AI agent frameworks, chatbot backends, conversation logging

---

### 7. [../cxtx/README.md](../cxtx/README.md) - CLI Session Capture Wrapper

**What it demonstrates**: Wrapping an existing `codex` or `claude` CLI so provider traffic is captured, normalized into canonical `cxdb.ConversationItem` turns, and uploaded through the CXDB HTTP API.

**Operations**:
- Launch a child CLI through a local reverse proxy
- Preserve child stdin, stdout, stderr, and exit code
- Append searchable context metadata and provenance on the first turn
- Publish the bundled canonical `cxdb.ConversationItem` registry descriptor automatically when needed
- Persist raw provider request, response, and stream evidence under `.scratch/cxtx/sessions/`
- Depend on the child honoring injected provider base URL variables for transparent traffic capture

**Run it**:
```bash
cargo run -p cxtx -- --help
```

**Use case**: Operator capture of interactive CLI sessions, local debugging, provenance-aware session ingest

---

## Quick Start

1. **Start the CXDB server** (required for all examples):
   ```bash
   # Terminal 1 - Start server
   cd /path/to/cxdb
   cargo run --release
   ```

2. **Run any example** (in a new terminal):
   ```bash
   cd examples/basic-go
   go run main.go
   ```

3. **View results in the UI** (optional):
   ```bash
   # Terminal 3 - Start gateway (for web UI)
   cd gateway
   go run ./cmd/server
   # Open http://localhost:8080
   ```

## Common Patterns

### Creating a Context

```go
// Go
client, _ := cxdb.Dial("localhost:9009")
ctx, _ := client.CreateContext(context.Background(), 0)
```

```rust
// Rust
let client = cxdb::dial("localhost:9009", vec![])?;
let ctx = client.create_context(&RequestContext::background(), 0)?;
```

```python
# Python (HTTP API)
import requests
resp = requests.post("http://localhost:9010/v1/contexts/create")
context_id = resp.json()["context_id"]
```

### Appending a Turn

```go
// Go
payload, _ := msgpack.Marshal(myData)
turn, _ := client.AppendTurn(ctx, &cxdb.AppendRequest{
    ContextID:   contextID,
    TypeID:      "com.example.MyType",
    TypeVersion: 1,
    Payload:     payload,
})
```

```rust
// Rust
let payload = encode_msgpack(&my_data)?;
let turn = client.append_turn(&ctx, &AppendRequest::new(
    context_id,
    "com.example.MyType",
    1,
    payload,
))?;
```

```python
# Python (HTTP API)
resp = requests.post(
    f"http://localhost:9010/v1/contexts/{context_id}/append",
    json={
        "type_id": "com.example.MyType",
        "type_version": 1,
        "data": {"field": "value"}
    }
)
```

### Retrieving Turns

```go
// Go
turns, _ := client.GetLast(ctx, contextID, 10, true)
for _, turn := range turns {
    var data MyType
    msgpack.Unmarshal(turn.Payload, &data)
    fmt.Printf("Turn %d: %+v\n", turn.TurnID, data)
}
```

```python
# Python (HTTP API)
resp = requests.get(f"http://localhost:9010/v1/contexts/{context_id}/turns?limit=10")
data = resp.json()
for turn in data["turns"]:
    print(f"Turn {turn['turn_id']}: {turn['data']}")
```

## Architecture Overview

```
┌─────────────┐
│   Writer    │ (Go/Rust client)
│   (SDK)     │
└──────┬──────┘
       │ Binary Protocol (port 9009)
       │ - Msgpack payloads
       │ - BLAKE3 hashing
       │ - Zstd compression
       ▼
┌─────────────┐
│   CXDB      │ (Rust server)
│   Server    │
└──────┬──────┘
       │
       ├─ Blob Store (content-addressed)
       ├─ Turn Store (DAG)
       └─ Type Registry (descriptors)

       │ HTTP API (port 9010)
       │ - JSON projection
       │ - Typed views
       ▼
┌─────────────┐
│  Frontend   │ (React UI)
│  +Gateway   │
└─────────────┘
```

## Data Flow

1. **Writer encodes** data with msgpack + numeric tags
2. **Writer computes** BLAKE3 hash of payload
3. **Server stores** blob in content-addressed store (dedup)
4. **Server appends** turn record to DAG
5. **Server updates** context head pointer
6. **Frontend requests** typed JSON projection
7. **Server projects** msgpack → JSON using type registry
8. **Renderer displays** rich visualization (if registered)

## Troubleshooting

### Connection Refused

**Symptom**: `dial tcp 127.0.0.1:9009: connection refused`

**Solution**: Ensure the CXDB server is running:
```bash
cargo run --release
# Or check if it's already running:
lsof -i :9009
```

### Type Not Found

**Symptom**: `424 Failed Dependency: type not found`

**Solution**: Publish the type registry bundle before appending turns. See [type-registration/](type-registration/) example.

### Module Not Found (Go)

**Symptom**: `go: module github.com/strongdm/cxdb not found`

**Solution**: Use a local replace directive in go.mod:
```go
replace github.com/strongdm/cxdb => ../..
```

### Cargo Dependency Error (Rust)

**Symptom**: `error: couldn't read Cargo.toml`

**Solution**: Use a path dependency in Cargo.toml:
```toml
[dependencies]
cxdb = { path = "../../clients/rust" }
```

## Next Steps

- **[Documentation](../docs/)**: Detailed protocol and architecture docs
- **[Client SDKs](../clients/)**: Go and Rust client libraries
- **[Frontend](../frontend/)**: Web UI for viewing contexts
- **[Deployment](../.deploy/)**: Production deployment guides

## Contributing

To add a new example:

1. Create a new directory under `examples/`
2. Include a `README.md` with:
   - What it demonstrates
   - Prerequisites
   - How to run
   - Expected output
3. Ensure it works against `localhost:9009` and `localhost:9010`
4. Update this index with a description

## License

Copyright 2025 StrongDM Inc
SPDX-License-Identifier: Apache-2.0
