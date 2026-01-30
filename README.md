# CXDB - AI Context Store

**CXDB is an AI Context Store for agents and LLMs**, providing fast, branch-friendly storage for conversation histories and tool outputs with content-addressed deduplication.

Built on a Turn DAG + Blob CAS architecture, CXDB gives you:

- **Branch-from-any-turn**: Fork conversations at any point without copying history
- **Fast append**: Optimized for the 99% case - appending new turns
- **Content deduplication**: Identical payloads stored once via BLAKE3 hashing
- **Type-safe projections**: Msgpack storage with typed JSON views for UIs
- **Built-in UI**: React frontend with turn visualization and custom renderers

## Quick Start

The fastest way to try CXDB is with the pre-built Docker image:

```bash
# Run the server (binary protocol :9009, HTTP :9010)
docker run -p 9009:9009 -p 9010:9010 -v $(pwd)/data:/data cxdb/cxdb:latest

# Create a context and append a turn
curl -X POST http://localhost:9010/v1/contexts
# => {"context_id": "1", "head_turn_id": "0", "head_depth": 0}

curl -X POST http://localhost:9010/v1/contexts/1/turns \
  -H "Content-Type: application/json" \
  -d '{
    "type_id": "com.example.Message",
    "type_version": 1,
    "payload": {"role": "user", "text": "Hello!"}
  }'

# View in the UI
open http://localhost:9010
```

## Installation

### From Source

**Prerequisites:**
- Rust 1.75+ (for the server)
- Go 1.22+ (for the client SDK and gateway)
- Node.js 20+ with pnpm (for the frontend)

```bash
# Clone the repository
git clone https://github.com/strongdm/cxdb.git
cd cxdb

# Build the server
cargo build --release

# Run the server
./target/release/ai-cxdb-store

# Build the gateway (optional - for OAuth and frontend serving)
cd gateway
go build -o bin/gateway ./cmd/server
./bin/gateway

# Build the frontend (optional - for UI development)
cd frontend
pnpm install
pnpm build
```

### Using Docker

```bash
# Build the image
docker build -t cxdb:latest .

# Run with persistent storage
docker run -p 9009:9009 -p 9010:9010 \
  -v $(pwd)/data:/data \
  -e CXDB_DATA_DIR=/data \
  cxdb:latest
```

## Key Concepts

### Turn DAG

A **Turn** is an immutable node in a directed acyclic graph (DAG):

```
Turn {
  turn_id: u64             # Unique, monotonically increasing
  parent_turn_id: u64      # 0 for root turns
  depth: u32               # Distance from root
  payload_hash: [32]byte   # BLAKE3 hash of payload
}
```

Turns form chains:

```
root (turn_id=1) → turn_id=2 → turn_id=3 (head)
                     ↓
                   turn_id=4 → turn_id=5 (alternate branch)
```

A **Context** is a mutable branch head pointer:

```
Context {
  context_id: u64
  head_turn_id: u64       # Current tip of this branch
}
```

Forking is O(1): create a new context pointing to an existing turn.

### Blob CAS (Content-Addressed Storage)

All turn payloads are stored in a content-addressed blob store:

- Each blob is identified by `BLAKE3(payload_bytes)`
- Identical payloads are deduplicated automatically
- Blobs are compressed with Zstd level 3
- Stored in `blobs/blobs.pack` with an index at `blobs/blobs.idx`

### Type Registry

CXDB supports typed payloads with forward-compatible schema evolution:

1. **Go writers** define types with numeric field tags:
   ```go
   type Message struct {
       Role string `cxdb:"1"`
       Text string `cxdb:"2"`
   }
   ```

2. **Registry bundles** are published to the server, describing field types and names

3. **Rust server** uses the registry to project msgpack → typed JSON for readers

4. **React UI** consumes typed JSON with safe u64 handling and custom renderers

### Renderers

Renderers are JavaScript modules that visualize turn payloads:

```javascript
// Renderer for com.example.Chart turns
export default function ChartRenderer({ data }) {
  return <LineChart data={data.points} />;
}
```

Renderers are:
- Loaded from CDN (ESM modules)
- Sandboxed with CSP
- Hot-swappable without server restart

## Architecture

CXDB is a three-tier system:

```
┌─────────────────┐
│   React UI      │  (Frontend - TypeScript/Next.js)
│   :3000         │
└────────┬────────┘
         │ HTTP/JSON
         ↓
┌─────────────────┐
│   Go Gateway    │  (OAuth proxy + static serving)
│   :8080         │
└────────┬────────┘
         │ HTTP/JSON
         ↓
┌─────────────────┐
│  Rust Server    │  (Storage + binary protocol)
│  :9009 binary   │
│  :9010 HTTP     │
└─────────────────┘
         │
         ↓
┌─────────────────┐
│   Storage       │
│  - turns/       │  (Turn DAG)
│  - blobs/       │  (Blob CAS)
│  - registry/    │  (Type descriptors)
└─────────────────┘
```

**Go writers** (clients) connect via binary protocol (:9009) for high-throughput turn appends.

**HTTP readers** (UI, tooling) use the JSON gateway (:9010) for typed projections and registry access.

## Documentation

- **Getting Started**: [docs/getting-started.md](docs/getting-started.md)
- **Architecture**: [docs/architecture.md](docs/architecture.md)
- **Binary Protocol**: [docs/protocol.md](docs/protocol.md)
- **HTTP API**: [docs/http-api.md](docs/http-api.md)
- **Type Registry**: [docs/type-registry.md](docs/type-registry.md)
- **Renderers**: [docs/renderers.md](docs/renderers.md)
- **Deployment**: [docs/deployment.md](docs/deployment.md)
- **Troubleshooting**: [docs/troubleshooting.md](docs/troubleshooting.md)
- **Development**: [docs/development.md](docs/development.md)

## Examples

### Go Writer (Binary Protocol)

```go
package main

import (
    "context"
    "log"

    "github.com/strongdm/cxdb/clients/go"
)

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

    // Append a turn
    payload := map[string]interface{}{
        "role": "user",
        "text": "What is the weather?",
    }

    turn, err := client.AppendTurn(context.Background(), &cxdb.AppendRequest{
        ContextID:   ctx.ContextID,
        TypeID:      "com.example.Message",
        TypeVersion: 1,
        Payload:     payload,
    })
    if err != nil {
        log.Fatal(err)
    }

    log.Printf("Appended turn %d at depth %d", turn.TurnID, turn.Depth)
}
```

### HTTP Reader (cURL)

```bash
# List contexts
curl http://localhost:9010/v1/contexts

# Get turns from a context (typed JSON projection)
curl http://localhost:9010/v1/contexts/1/turns?limit=10

# Get raw msgpack bytes
curl http://localhost:9010/v1/contexts/1/turns?view=raw

# Page through history
curl "http://localhost:9010/v1/contexts/1/turns?limit=10&before_turn_id=100"
```

### React UI

```typescript
import { useTurns } from '@/lib/hooks/useTurns';

function ConversationView({ contextId }: { contextId: string }) {
  const { turns, loading, error } = useTurns(contextId);

  if (loading) return <div>Loading...</div>;
  if (error) return <div>Error: {error.message}</div>;

  return (
    <div>
      {turns.map(turn => (
        <TurnCard key={turn.turn_id} turn={turn} />
      ))}
    </div>
  );
}
```

## Configuration

CXDB is configured via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `CXDB_DATA_DIR` | `./data` | Storage directory |
| `CXDB_BIND` | `127.0.0.1:9009` | Binary protocol bind address |
| `CXDB_HTTP_BIND` | `127.0.0.1:9010` | HTTP gateway bind address |
| `CXDB_LOG_LEVEL` | `info` | Log level (debug, info, warn, error) |
| `CXDB_ENABLE_METRICS` | `false` | Enable Prometheus metrics |

See [docs/deployment.md](docs/deployment.md) for production configuration.

## Development

```bash
# Run all tests
make test

# Format code
make fmt

# Lint
make clippy

# Run dev stack (backend + gateway + frontend in tmux)
make dev

# Stop dev stack
make dev-stop
```

See [docs/development.md](docs/development.md) for details.

## Contributing

We welcome contributions! Please:

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

Before submitting:

```bash
# Run pre-commit checks
make precommit
```

Please read our [Code of Conduct](CODE_OF_CONDUCT.md) and [Contributing Guide](CONTRIBUTING.md).

## Security

For security issues, please email security@strongdm.com instead of using the public issue tracker.

See [SECURITY.md](SECURITY.md) for our security policy.

## License

CXDB is licensed under the Apache License 2.0. See [LICENSE](LICENSE) for details.

Copyright 2025 StrongDM Inc.

## Acknowledgments

Built with:

- [Rust](https://www.rust-lang.org/) - Server implementation
- [Go](https://go.dev/) - Client SDK and gateway
- [React](https://react.dev/) + [Next.js](https://nextjs.org/) - Frontend
- [BLAKE3](https://github.com/BLAKE3-team/BLAKE3) - Content hashing
- [MessagePack](https://msgpack.org/) - Binary serialization
- [Zstd](https://facebook.github.io/zstd/) - Compression

---

**[strongdm.com](https://www.strongdm.com)** | **[Documentation](docs/)** | **[Issues](https://github.com/strongdm/cxdb/issues)**
