# `cxtx`

`cxtx` wraps `codex` or `claude`, routes provider traffic through a local reverse proxy, uploads canonical `cxdb.ConversationItem` turns into CXDB, and keeps raw provider evidence under `.scratch/cxtx/sessions/`.

## Prerequisites

- A running CXDB HTTP endpoint, typically `http://127.0.0.1:9010`
- Either the `codex` or `claude` CLI installed and discoverable on `PATH`
- Existing provider credentials for the child CLI in your environment

## Build

```bash
cargo build --release -p cxtx
```

## Usage

```bash
# Wrap Codex and send captured turns to the local CXDB HTTP endpoint
./target/release/cxtx codex -- --model gpt-5

# Wrap Claude against a specific CXDB server
./target/release/cxtx --url http://127.0.0.1:9010 claude -- --print stream
```

`cxtx` preserves child stdin, stdout, stderr, and exit status. On successful execution it does not write wrapper-authored stdout. If CXDB is unavailable, it still launches the child, enters queued-delivery mode, and records delivery state in the local ledger until delivery recovers or shutdown drain completes.

## Resulting Artifacts

- CXDB receives canonical `system`, `user_input`, `assistant_turn`, and tool-related items for the wrapped session.
- The first uploaded turn carries `ContextMetadata` and `Provenance`, so the context is queryable in CXDB listings.
- `cxtx` publishes the bundled canonical `cxdb.ConversationItem` registry descriptor automatically before the first append when the server does not already have it.
- Local evidence is written under `.scratch/cxtx/sessions/<stable-session-id>/`:
  - `ledger.json`
  - `exchanges/<exchange-id>/request.json`
  - `exchanges/<exchange-id>/response.json`
  - `exchanges/<exchange-id>/stream.ndjson`

## Troubleshooting

- `failed to launch codex` or `failed to launch claude`:
  - The child binary is missing from `PATH` or is not executable.
- `cxtx: CXDB ingest unavailable, entering queued-delivery mode`:
  - The wrapper could not reach the configured `--url`. Check the CXDB server, but the child session is still running and the ledger will show queue state.
- No captured turns appear in CXDB:
  - Confirm the child CLI honors the injected provider base URL variables. `cxtx` depends on those environment overrides for transparent capture.

## Verification

```bash
cargo run -p cxtx -- --help
cargo test -p cxtx
cargo test -p cxtx --test integration
```
