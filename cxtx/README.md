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
./target/release/cxtx --local codex -- --model gpt-5

# Wrap Claude and send captured turns to the local CXDB HTTP endpoint
./target/release/cxtx --local claude -- --print stream

# Wrap Claude against a specific CXDB server
./target/release/cxtx --url http://127.0.0.1:9010 claude -- --print stream
```

`cxtx` preserves child stdin, stdout, stderr, and exit status. On successful execution it does not write wrapper-authored stdout. If CXDB is unavailable, it still launches the child, enters queued-delivery mode, and records delivery state in the local ledger until delivery recovers or shutdown drain completes.

For `codex`, `cxtx` now hardens the launch contract so interactive traffic stays on the local proxy path that OSS capture depends on. It injects both `OPENAI_*` and `CXTX_OPENAI_*` proxy base-url env vars, removes inherited upstream base-url overrides before spawning the child, and prepends websocket-disabling flags unless the caller already supplied explicit overrides.

## Resulting Artifacts

- CXDB receives canonical `system`, `user_input`, `assistant_turn`, and tool-related items for the wrapped session.
- Interactive `codex` Responses traffic no longer leaks wrapper/bootstrap scaffolding into uploaded `user_input` turns.
- Websocket-backed provider traffic is captured through the local proxy instead of bypassing ledger and CXDB upload paths.
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
  - The wrapper could not reach the configured default URL, `--local`, or `--url`. Check the CXDB server, but the child session is still running and the ledger will show queue state.
- No captured turns appear in CXDB:
  - Confirm the child CLI honors the injected provider base URL variables. `codex` should see both the legacy `OPENAI_*` variables and the `CXTX_OPENAI_*` aliases, and `claude` should see the Anthropic/Claude base-url overrides.
- `codex` still reaches the public OpenAI endpoint directly:
  - Check whether the caller explicitly re-enabled websocket features or overrode the proxy base URL in child args. `cxtx` only injects its websocket-disable defaults when the caller did not already make an explicit choice.

## Verification

```bash
cargo run -p cxtx -- --help
cargo test -p cxtx
cargo test -p cxtx --test integration
```
