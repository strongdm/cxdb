#!/usr/bin/env bash
# Generate test fixtures for CXDB
# Creates sample data for tests, examples, and CI

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

FIXTURES_DIR="${FIXTURES_DIR:-$REPO_ROOT/fixtures}"
mkdir -p "$FIXTURES_DIR"

echo "Generating CXDB test fixtures..."
echo "Output directory: $FIXTURES_DIR"
echo ""

# Generate basic protocol message fixtures using Go client
if command -v go &> /dev/null && [ -d clients/go ]; then
    echo "==> Generating protocol message fixtures (Go)"

    # Check if fixture generator exists
    if [ -f clients/go/cmd/cxdb-fixtures/main.go ]; then
        cd clients/go
        go run ./cmd/cxdb-fixtures -out "$FIXTURES_DIR/protocol" || echo "  (fixture generator not ready, skipping)"
        cd "$REPO_ROOT"
    else
        echo "  (fixture generator not implemented yet, skipping)"
    fi
fi

# Generate fstree fixtures using Go client
if command -v go &> /dev/null && [ -d clients/go/fstree ]; then
    echo ""
    echo "==> Generating fstree fixtures (Go)"

    if [ -f clients/go/cmd/cxdb-fstree-fixtures/main.go ]; then
        cd clients/go
        go run ./cmd/cxdb-fstree-fixtures -out "$FIXTURES_DIR/fstree" || echo "  (fstree fixture generator not ready, skipping)"
        cd "$REPO_ROOT"
    else
        echo "  (fstree fixture generator not implemented yet, skipping)"
    fi
fi

# Generate canonical conversation type fixtures using Go
if command -v go &> /dev/null; then
    echo ""
    echo "==> Generating conversation type fixtures (Go)"

    if [ -f clients/go/cmd/cxdb-msgpack-fixtures/main.go ]; then
        cd clients/go
        go run ./cmd/cxdb-msgpack-fixtures -out "$FIXTURES_DIR/types" || echo "  (msgpack fixture generator not ready, skipping)"
        cd "$REPO_ROOT"
    else
        echo "  (msgpack fixture generator not implemented yet, skipping)"
    fi
fi

# Generate example type bundles for type registry
echo ""
echo "==> Generating type registry fixtures"
mkdir -p "$FIXTURES_DIR/registry"

# Simple LogEntry type bundle
cat > "$FIXTURES_DIR/registry/log-entry-bundle.json" <<'EOF'
{
  "types": [
    {
      "type_id": "com.example.LogEntry",
      "type_version": 1,
      "description": "Structured log entry with level, message, and tags",
      "fields": [
        {"tag": 1, "name": "timestamp", "type": "uint64", "semantic": "unix_ms", "required": true},
        {"tag": 2, "name": "level", "type": "uint8", "semantic": "enum", "required": true},
        {"tag": 3, "name": "message", "type": "string", "required": true},
        {"tag": 4, "name": "tags", "type": "map", "required": false}
      ]
    }
  ],
  "renderers": []
}
EOF

echo "  Created: log-entry-bundle.json"

# ConversationItem type bundle
cp "$ROOT_DIR/cxtx/src/conversation_registry_bundle.json" "$FIXTURES_DIR/registry/conversation-bundle.json"

echo "  Created: conversation-bundle.json"

# Generate sample filesystem snapshot fixture
echo ""
echo "==> Generating filesystem snapshot fixtures"
mkdir -p "$FIXTURES_DIR/fstree/sample"

# Create a small example directory structure
mkdir -p "$FIXTURES_DIR/fstree/sample/src"
mkdir -p "$FIXTURES_DIR/fstree/sample/tests"

cat > "$FIXTURES_DIR/fstree/sample/README.md" <<'EOF'
# Sample Project
This is a sample directory for fstree snapshot testing.
EOF

cat > "$FIXTURES_DIR/fstree/sample/src/main.go" <<'EOF'
package main

func main() {
    println("Hello, CXDB!")
}
EOF

cat > "$FIXTURES_DIR/fstree/sample/tests/main_test.go" <<'EOF'
package main

import "testing"

func TestExample(t *testing.T) {
    // Example test
}
EOF

echo "  Created: sample project directory"

# Generate empty marker file
echo ""
echo "==> Generating fixture index"
cat > "$FIXTURES_DIR/README.md" <<'EOF'
# CXDB Test Fixtures

Generated test data for CXDB development and testing.

## Directories

- `protocol/` - Binary protocol message fixtures (Go-generated)
- `fstree/` - Filesystem snapshot fixtures (Go-generated + sample directory)
- `types/` - Canonical conversation type fixtures (Go-generated)
- `registry/` - Type registry bundle examples (hand-crafted)

## Regenerating Fixtures

To regenerate all fixtures:

```bash
./scripts/generate-fixtures.sh
```

To regenerate specific fixture sets, use the Go fixture generator programs directly:

```bash
# Protocol messages
cd clients/go && go run ./cmd/cxdb-fixtures -out ../../fixtures/protocol

# Filesystem snapshots
cd clients/go && go run ./cmd/cxdb-fstree-fixtures -out ../../fixtures/fstree

# Msgpack types
cd clients/go && go run ./cmd/cxdb-msgpack-fixtures -out ../../fixtures/types
```

## Usage in Tests

Rust tests load fixtures from relative paths:
```rust
let fixture = include_bytes!("../../fixtures/protocol/hello.bin");
```

Go tests load fixtures from relative paths:
```go
data, err := os.ReadFile("../../fixtures/protocol/hello.bin")
```
EOF

echo "  Created: fixtures/README.md"

echo ""
echo "✓ Fixture generation complete!"
echo ""
echo "Fixtures created in: $FIXTURES_DIR"
find "$FIXTURES_DIR" -type f | sort
