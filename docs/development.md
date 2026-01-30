# Development Guide

This guide covers building CXDB from source, running tests, and contributing to the project.

## Prerequisites

- **Rust**: 1.75 or later with Cargo
- **Go**: 1.22 or later
- **Node.js**: 20 or later
- **pnpm**: Latest version (via `coreutils enable`)
- **Git**: For cloning the repository

Optional:
- **Docker**: For containerized testing
- **tmux**: For running the dev stack

## Getting Started

### Clone the Repository

```bash
git clone https://github.com/strongdm/cxdb.git
cd cxdb
```

### Project Structure

```
cxdb/
├── server/                # Rust server (binary protocol + HTTP gateway)
│   ├── src/
│   │   ├── blob_store/   # Content-addressed storage
│   │   ├── turn_store/   # Turn DAG
│   │   ├── protocol/     # Binary protocol
│   │   ├── http/         # HTTP gateway
│   │   ├── registry/     # Type registry
│   │   └── projection/   # Msgpack → JSON
│   ├── Cargo.toml
│   └── tests/
├── clients/
│   ├── go/               # Go client SDK
│   │   ├── client.go
│   │   ├── turn.go
│   │   └── cmd/          # Example programs
│   └── rust/             # Rust client SDK
│       └── cxdb/
├── gateway/              # Go OAuth proxy + static serving
│   ├── cmd/server/
│   ├── internal/
│   └── pkg/
├── frontend/             # React UI
│   ├── app/              # Next.js pages
│   ├── lib/              # Shared utilities
│   └── tests/            # Playwright tests
├── docs/                 # Documentation
├── deploy/               # Deployment configs
└── scripts/              # Build and test scripts
```

## Building

### Rust Server

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Check without building
cargo check

# Run directly
cargo run
```

Binaries are output to:
- Debug: `target/debug/ai-cxdb-store`
- Release: `target/release/ai-cxdb-store`

**Environment variables:**

```bash
# Data directory
CXDB_DATA_DIR=./data

# Bind addresses
CXDB_BIND=127.0.0.1:9009
CXDB_HTTP_BIND=127.0.0.1:9010

# Logging
CXDB_LOG_LEVEL=debug
```

### Go Client SDK

```bash
cd clients/go

# Build
go build -v

# Run tests
go test -v

# Build example programs
go build -o bin/fixtures ./cmd/cxdb-fixtures
```

### Go Gateway

```bash
cd gateway

# Install dependencies
go mod download

# Build
go build -o bin/gateway ./cmd/server

# Run in dev mode (no OAuth)
make -C .. gateway-dev
```

Dev mode creates `gateway/.env` with:
```bash
DEV_MODE=true
DEV_EMAIL=dev@localhost
DEV_NAME=Developer
PUBLIC_BASE_URL=http://localhost:8080
CXDB_BACKEND_URL=http://127.0.0.1:9010
PORT=8080
```

### React Frontend

```bash
cd frontend

# Install dependencies
pnpm install

# Dev server (with hot reload)
pnpm dev

# Production build
pnpm build

# Static export
pnpm export
```

Frontend dev server runs on http://localhost:3000.

## Running the Dev Stack

### Option 1: tmux (Recommended)

Run all components in a tmux session:

```bash
make dev
```

This starts:
- Backend on :9009 (binary) and :9010 (HTTP)
- Gateway on :8080
- Frontend on :3000

**Access:**
- Frontend: http://localhost:3000
- Gateway: http://localhost:8080 (with OAuth bypass)
- Backend: http://localhost:9010 (direct)

**Attach to tmux:**

```bash
tmux attach -t cxdb

# Switch windows:
Ctrl+B, 0  # Backend
Ctrl+B, 1  # Gateway
Ctrl+B, 2  # Frontend
```

**Stop:**

```bash
make dev-stop
```

### Option 2: Manual (separate terminals)

**Terminal 1 - Backend:**

```bash
CXDB_DATA_DIR=./data \
CXDB_HTTP_BIND=127.0.0.1:9010 \
CXDB_LOG_LEVEL=debug \
cargo run --release
```

**Terminal 2 - Gateway:**

```bash
cd gateway
make -C .. gateway-dev
```

**Terminal 3 - Frontend:**

```bash
cd frontend
pnpm dev
```

## Testing

### Rust Tests

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_append_turn

# Run with output
cargo test -- --nocapture

# Run tests in specific module
cargo test --package ai-cxdb-store --lib blob_store
```

### Go Tests

```bash
cd clients/go

# Run all tests
go test -v ./...

# Run specific test
go test -v -run TestAppendTurn

# With race detector
go test -race -v ./...

# Generate coverage
go test -coverprofile=coverage.out ./...
go tool cover -html=coverage.out
```

### Frontend Tests

```bash
cd frontend

# Lint
pnpm lint

# Type check
pnpm type-check

# Unit tests (if any)
pnpm test

# E2E tests (Playwright)
pnpm test:e2e

# E2E with UI
pnpm test:e2e:ui
```

**Playwright tests require server running:**

```bash
# Terminal 1
cargo run --release

# Terminal 2
cd frontend
pnpm test:e2e
```

### Integration Tests

Run full-stack tests:

```bash
# Start stack
make dev

# In another terminal
cd clients/go
go test -v -tags=integration ./...
```

## Code Style

### Rust

Use `rustfmt` and `clippy`:

```bash
# Format code
cargo fmt --all

# Check formatting
cargo fmt --all -- --check

# Lint with clippy
cargo clippy --workspace -- -D warnings

# Fix clippy warnings
cargo clippy --fix
```

**rustfmt.toml** (already configured):

```toml
max_width = 100
hard_tabs = false
tab_spaces = 4
newline_style = "Unix"
```

### Go

Use `gofmt` and `golangci-lint`:

```bash
# Format code
gofmt -w .

# Check formatting
gofmt -l .

# Lint (if golangci-lint installed)
golangci-lint run
```

### TypeScript

Use ESLint and Prettier:

```bash
cd frontend

# Lint
pnpm lint

# Fix lint errors
pnpm lint:fix

# Format
pnpm format
```

## Pre-commit Checks

Run all checks before committing:

```bash
make precommit
```

This runs:
- `cargo fmt --check`
- `cargo clippy`
- `cargo test`

## Fixtures and Test Data

### Generate Test Data

```bash
cd clients/go

# Build fixture generator
go build -o bin/fixtures ./cmd/cxdb-fixtures

# Generate fixtures
./bin/fixtures -addr localhost:9009 -count 100
```

This creates:
- 10 contexts
- 100 turns with various types
- Msgpack + registry bundles

### Registry Bundles

Example registry for testing:

```go
// clients/go/cmd/cxdb-fixtures/registry.go
bundle := map[string]interface{}{
    "registry_version": 1,
    "bundle_id": "test-2025-01-30",
    "types": map[string]interface{}{
        "com.test.Message": map[string]interface{}{
            "versions": map[string]interface{}{
                "1": map[string]interface{}{
                    "fields": map[string]interface{}{
                        "1": map[string]interface{}{"name": "role", "type": "string"},
                        "2": map[string]interface{}{"name": "text", "type": "string"},
                    },
                },
            },
        },
    },
}
```

## Debugging

### Rust Debugger (LLDB)

```bash
# Build with debug symbols
cargo build

# Run with debugger
rust-lldb target/debug/ai-cxdb-store

# Set breakpoint
(lldb) b blob_store::mod::put
(lldb) run

# Inspect variables
(lldb) p blob_hash
(lldb) bt
```

### VSCode Debugging

**.vscode/launch.json:**

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "lldb",
      "request": "launch",
      "name": "Debug CXDB",
      "cargo": {
        "args": ["build", "--package=ai-cxdb-store"]
      },
      "args": [],
      "cwd": "${workspaceFolder}",
      "env": {
        "CXDB_DATA_DIR": "./data",
        "CXDB_LOG_LEVEL": "debug"
      }
    }
  ]
}
```

### Go Debugging (Delve)

```bash
# Install delve
go install github.com/go-delve/delve/cmd/dlv@latest

# Debug test
cd clients/go
dlv test -- -test.run TestAppendTurn

# Set breakpoint
(dlv) b client.go:123
(dlv) c
(dlv) p payload
```

### Browser DevTools

```bash
# Frontend with source maps
cd frontend
pnpm dev

# Open http://localhost:3000
# Press F12 → Sources
# Set breakpoints in .tsx files
```

## Profiling

### Rust CPU Profiling

```bash
# Install cargo-flamegraph
cargo install flamegraph

# Profile
cargo flamegraph --bin ai-cxdb-store

# Opens flamegraph.svg in browser
```

### Go CPU Profiling

```bash
# Add to test
import _ "net/http/pprof"

func TestWithProfile(t *testing.T) {
    go func() {
        log.Println(http.ListenAndServe("localhost:6060", nil))
    }()
    // ... test code
}

# Profile
go test -cpuprofile=cpu.prof -bench=.
go tool pprof cpu.prof
```

### Memory Profiling

```bash
# Rust
cargo install cargo-valgrind
cargo valgrind run

# Go
go test -memprofile=mem.prof
go tool pprof mem.prof
```

## Documentation

### Rust Documentation

```bash
# Generate docs
cargo doc --open

# Document private items
cargo doc --document-private-items
```

### Go Documentation

```bash
# Generate docs
godoc -http=:6060

# View at http://localhost:6060/pkg/github.com/strongdm/cxdb/clients/go/
```

## Contributing

### Workflow

1. **Fork the repository**
2. **Create a feature branch:**
   ```bash
   git checkout -b feature/amazing-feature
   ```

3. **Make changes:**
   - Write code
   - Add tests
   - Update docs

4. **Run pre-commit checks:**
   ```bash
   make precommit
   ```

5. **Commit with conventional commit messages:**
   ```bash
   git commit -m "feat(blob_store): add compression level config"
   git commit -m "fix(protocol): handle EOF gracefully"
   git commit -m "docs: update type registry guide"
   ```

6. **Push to your fork:**
   ```bash
   git push origin feature/amazing-feature
   ```

7. **Open a Pull Request**

### Commit Message Format

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <subject>

<body>

<footer>
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only
- `style`: Formatting, missing semi-colons, etc
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `perf`: Performance improvement
- `test`: Adding or fixing tests
- `chore`: Build process, tooling, dependencies

**Examples:**

```
feat(registry): support nested type descriptors

Adds support for nested types in the type registry, allowing
for complex structured payloads.

Closes #123
```

```
fix(blob_store): prevent race condition in dedup check

Use double-checked locking pattern to avoid race when multiple
writers try to store the same blob concurrently.

Fixes #456
```

### Code Review

PRs require:
- At least one approval
- All CI checks passing
- No merge conflicts
- Conventional commit messages

## Release Process

### Versioning

CXDB uses [Semantic Versioning](https://semver.org/):

- `MAJOR.MINOR.PATCH` (e.g., `1.2.3`)
- Major: Breaking changes
- Minor: New features (backward compatible)
- Patch: Bug fixes

### Creating a Release

1. **Update version:**
   ```bash
   # server/Cargo.toml
   version = "1.2.3"

   # clients/go/version.go
   const Version = "1.2.3"

   # frontend/package.json
   "version": "1.2.3"
   ```

2. **Update CHANGELOG.md:**
   ```markdown
   ## [1.2.3] - 2025-01-30

   ### Added
   - New feature X

   ### Fixed
   - Bug in Y
   ```

3. **Commit and tag:**
   ```bash
   git add -A
   git commit -m "chore: release v1.2.3"
   git tag -a v1.2.3 -m "Release v1.2.3"
   git push origin main --tags
   ```

4. **Build and publish:**
   ```bash
   # Docker images
   docker build --platform linux/amd64 -t cxdb/cxdb:1.2.3 .
   docker push cxdb/cxdb:1.2.3
   docker tag cxdb/cxdb:1.2.3 cxdb/cxdb:latest
   docker push cxdb/cxdb:latest

   # Go module (automatic via GitHub tags)
   # Rust crate (if publishing to crates.io)
   cargo publish
   ```

5. **Create GitHub Release:**
   - Go to Releases → Draft a new release
   - Tag: v1.2.3
   - Title: CXDB v1.2.3
   - Copy CHANGELOG entry
   - Attach binaries (optional)

## Development Tips

### Fast Iteration

**Rust:**
```bash
# Use cargo-watch for auto-rebuild
cargo install cargo-watch
cargo watch -x run
```

**Frontend:**
```bash
# Hot reload is automatic with pnpm dev
cd frontend && pnpm dev
```

### Targeting Specific Tests

```bash
# Rust: test name
cargo test test_blob_dedup

# Go: test name pattern
go test -run "TestAppend*"

# Frontend: test file
pnpm test:e2e tests/turn-dag.spec.ts
```

### Clearing Data

```bash
# Remove all data (start fresh)
rm -rf data/

# Or
make clean-data
```

### Using Local Changes in Examples

```bash
# Use local Go client in examples
cd examples/my-example
go mod edit -replace github.com/strongdm/cxdb/clients/go=../../clients/go
go mod tidy
```

## Getting Help

- **GitHub Discussions:** https://github.com/strongdm/cxdb/discussions
- **Slack:** https://strongdm-community.slack.com #cxdb
- **Issues:** https://github.com/strongdm/cxdb/issues

## See Also

- [Architecture](architecture.md) - System design
- [Contributing Guidelines](../CONTRIBUTING.md) - Detailed contribution guide
- [Code of Conduct](../CODE_OF_CONDUCT.md) - Community guidelines
