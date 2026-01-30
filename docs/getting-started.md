# Getting Started with CXDB

This guide walks you through installing CXDB, creating your first context, appending turns, and viewing results in the UI.

## Prerequisites

Choose one of:

**Option A: Docker** (easiest)
- Docker 20.10+ or Docker Desktop

**Option B: From source**
- Rust 1.75+ with Cargo
- Go 1.22+ (for gateway and client SDK)
- Node.js 20+ with pnpm (for UI development)

## Installation

### Option A: Using Docker

Pull the pre-built image:

```bash
docker pull cxdb/cxdb:latest
```

Or build locally:

```bash
git clone https://github.com/strongdm/cxdb.git
cd cxdb
docker build -t cxdb:latest .
```

Run the server:

```bash
docker run -d \
  --name cxdb \
  -p 9009:9009 \
  -p 9010:9010 \
  -v $(pwd)/data:/data \
  cxdb:latest
```

This starts:
- Binary protocol server on `:9009`
- HTTP gateway on `:9010`
- Data persisted to `./data`

### Option B: From Source

Clone and build:

```bash
git clone https://github.com/strongdm/cxdb.git
cd cxdb

# Build the Rust server
cargo build --release

# Run the server
CXDB_DATA_DIR=./data ./target/release/ai-cxdb-store
```

The server will start on:
- `:9009` (binary protocol)
- `:9010` (HTTP gateway)

## Your First Context

### Step 1: Create a Context

A context is a branch head that tracks the latest turn in a conversation:

```bash
curl -X POST http://localhost:9010/v1/contexts/create
```

Response:

```json
{
  "context_id": "1",
  "head_turn_id": "0",
  "head_depth": 0
}
```

The `context_id` is your branch identifier. `head_turn_id` of `0` means it's empty.

### Step 2: Append a Turn (HTTP)

Add a user message:

```bash
curl -X POST http://localhost:9010/v1/contexts/1/append \
  -H "Content-Type: application/json" \
  -d '{
    "type_id": "com.example.Message",
    "type_version": 1,
    "data": {
      "role": "user",
      "text": "What is the capital of France?"
    }
  }'
```

Response:

```json
{
  "context_id": "1",
  "turn_id": "1",
  "depth": 1,
  "content_hash": "a3f5b8c2..."
}
```

Add an assistant response:

```bash
curl -X POST http://localhost:9010/v1/contexts/1/append \
  -H "Content-Type: application/json" \
  -d '{
    "type_id": "com.example.Message",
    "type_version": 1,
    "data": {
      "role": "assistant",
      "text": "The capital of France is Paris."
    }
  }'
```

### Step 3: Retrieve Turns

Get the conversation history:

```bash
curl http://localhost:9010/v1/contexts/1/turns?limit=10
```

Response:

```json
{
  "meta": {
    "context_id": "1",
    "head_turn_id": "2",
    "head_depth": 2
  },
  "turns": [
    {
      "turn_id": "1",
      "parent_turn_id": "0",
      "depth": 1,
      "declared_type": {
        "type_id": "com.example.Message",
        "type_version": 1
      },
      "data": {
        "role": "user",
        "text": "What is the capital of France?"
      }
    },
    {
      "turn_id": "2",
      "parent_turn_id": "1",
      "depth": 2,
      "declared_type": {
        "type_id": "com.example.Message",
        "type_version": 1
      },
      "data": {
        "role": "assistant",
        "text": "The capital of France is Paris."
      }
    }
  ]
}
```

## Using the Go Client SDK

For production use, the Go client provides a more efficient binary protocol:

### Install the SDK

```bash
go get github.com/strongdm/cxdb/clients/go
```

### Write Your First Client

Create `main.go`:

```go
package main

import (
    "context"
    "fmt"
    "log"

    "github.com/strongdm/cxdb/clients/go"
    "github.com/vmihailenco/msgpack/v5"
)

type Message struct {
    Role string `msgpack:"1"`
    Text string `msgpack:"2"`
}

func main() {
    // Connect to CXDB
    client, err := cxdb.Dial("localhost:9009")
    if err != nil {
        log.Fatal(err)
    }
    defer client.Close()

    // Create a context
    ctx, err := client.CreateContext(context.Background(), 0)
    if err != nil {
        log.Fatal(err)
    }

    fmt.Printf("Created context %d\n", ctx.ContextID)

    // Append a user turn
    userMsg := Message{
        Role: "user",
        Text: "What is 2+2?",
    }

    userPayload, _ := msgpack.Marshal(userMsg)

    userTurn, err := client.AppendTurn(context.Background(), &cxdb.AppendRequest{
        ContextID:   ctx.ContextID,
        TypeID:      "com.example.Message",
        TypeVersion: 1,
        Payload:     userPayload,
    })
    if err != nil {
        log.Fatal(err)
    }

    fmt.Printf("User turn: %d at depth %d\n", userTurn.TurnID, userTurn.Depth)

    // Append an assistant turn
    assistantMsg := Message{
        Role: "assistant",
        Text: "2+2 equals 4.",
    }

    assistantPayload, _ := msgpack.Marshal(assistantMsg)

    assistantTurn, err := client.AppendTurn(context.Background(), &cxdb.AppendRequest{
        ContextID:   ctx.ContextID,
        TypeID:      "com.example.Message",
        TypeVersion: 1,
        Payload:     assistantPayload,
    })
    if err != nil {
        log.Fatal(err)
    }

    fmt.Printf("Assistant turn: %d at depth %d\n", assistantTurn.TurnID, assistantTurn.Depth)

    // Retrieve last 10 turns
    turns, err := client.GetLast(context.Background(), ctx.ContextID, 10, true)
    if err != nil {
        log.Fatal(err)
    }

    fmt.Printf("\nRetrieved %d turns:\n", len(turns))
    for _, turn := range turns {
        var msg Message
        msgpack.Unmarshal(turn.Payload, &msg)
        fmt.Printf("  Turn %d: %s: %s\n", turn.TurnID, msg.Role, msg.Text)
    }
}
```

Run it:

```bash
go run main.go
```

Output:

```
Created context 1
User turn: 1 at depth 1
Assistant turn: 2 at depth 2

Retrieved 2 turns:
  Turn 1: user: What is 2+2?
  Turn 2: assistant: 2+2 equals 4.
```

## Using the Rust Client SDK

For Rust applications:

### Add the Dependency

```toml
[dependencies]
cxdb = { path = "clients/rust/cxdb" }
tokio = { version = "1", features = ["full"] }
```

### Example

```rust
use cxdb::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect
    let client = Client::connect("localhost:9009").await?;

    // Create context
    let ctx = client.create_context(0).await?;
    println!("Created context {}", ctx.context_id);

    // Append a turn
    let payload = serde_json::json!({
        "role": "user",
        "text": "Hello from Rust!"
    });

    let turn = client.append_turn(
        ctx.context_id,
        "com.example.Message",
        1,
        &rmp_serde::to_vec(&payload)?
    ).await?;

    println!("Appended turn {} at depth {}", turn.turn_id, turn.depth);

    Ok(())
}
```

## View in the UI

If you're running with the gateway:

```bash
# Start the gateway (in a new terminal)
cd gateway
go run ./cmd/server
```

Then open http://localhost:8080 in your browser.

The UI provides:
- Context list and search
- Turn-by-turn visualization
- DAG view for branches
- Raw and typed views
- Custom renderers for rich content

## Branching (Forking)

Create an alternate conversation path:

```bash
# Fork from turn 1
curl -X POST http://localhost:9010/v1/contexts/fork \
  -H "Content-Type: application/json" \
  -d '{"base_turn_id": 1}'
```

Response:

```json
{
  "context_id": "2",
  "head_turn_id": "1",
  "head_depth": 1
}
```

Now context 2 shares history with context 1 up to turn 1, but can diverge:

```bash
curl -X POST http://localhost:9010/v1/contexts/2/append \
  -H "Content-Type: application/json" \
  -d '{
    "type_id": "com.example.Message",
    "type_version": 1,
    "data": {
      "role": "assistant",
      "text": "Actually, let me give you more detail about Paris..."
    }
  }'
```

The DAG now looks like:

```
turn 1 (user) → turn 2 (assistant, context 1)
              ↘ turn 3 (assistant, context 2)
```

## Next Steps

- **[Architecture](architecture.md)**: Understand the system design
- **[Type Registry](type-registry.md)**: Define custom types for your payloads
- **[Renderers](renderers.md)**: Create custom visualizations for your turns
- **[HTTP API Reference](http-api.md)**: Complete API documentation
- **[Development Guide](development.md)**: Set up a development environment

## Common Issues

### Connection refused

Ensure the server is running:

```bash
# Check if the server is listening
netstat -an | grep 9009
netstat -an | grep 9010
```

### Permission denied on ./data

The Docker container runs as a non-root user. Fix permissions:

```bash
mkdir -p data
chmod 755 data
```

### "Type not found" errors

Publish your type registry bundle before appending turns. See [type-registry.md](type-registry.md).
