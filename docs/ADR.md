# Architecture Decision Record Log

## ADR-001 - `cxtx` Uses Local Reverse Proxy Capture and CXDB HTTP Ingest

- Status: Accepted
- Date: 2026-03-17

### Context

CXDB already exposes HTTP context creation and append routes that are easy for tools to consume, while the adjacent `tc` project demonstrates a practical wrapper pattern for `claude` and `codex`: launch the child process, inject provider base URL environment variables, and observe OpenAI/Anthropic traffic through a localhost reverse proxy.

The new `cxtx` CLI needs to preserve the child CLI experience while transmitting useful session context into CXDB with minimal operator friction. The implementation also needs to fit naturally into this repository's existing Rust workspace and developer workflow.

### Decision

Implement `cxtx` as a new Rust workspace member that:

1. launches `claude` or `codex` as a child process while preserving stdin/stdout/stderr and exit status;
2. starts a localhost reverse proxy and injects provider base URL environment variables so provider traffic traverses `cxtx`;
3. captures provider requests, responses, stream frames, and wrapper lifecycle transitions, but uses that traffic only to extract newly observed turns within one stable wrapper session;
4. writes the captured session to CXDB through the existing HTTP create and append endpoints rather than introducing a second ingest path for this sprint;
5. uses a real-time delivery worker for CXDB ingest that attempts immediate transmission, but when the ingest endpoint is unavailable, queues unsent messages locally in memory and retries in order until delivery succeeds or the wrapper reaches its documented shutdown boundary.

`cxtx` will create one CXDB context per CLI invocation, mint one stable session ID for that invocation, attach `ContextMetadata` and `Provenance` on the first appended turn, and append only newly extracted conversation turns for that stable session rather than uploading every replayed provider payload verbatim.

The primary CXDB wire contract for this sprint is the existing canonical `cxdb.ConversationItem` type rather than a new `cxtx`-specific top-level turn type. `cxtx` will convert extracted user input, assistant output, tool activity, and lifecycle events into canonical conversation items so the server's metadata extraction and the frontend's built-in conversation renderer work without extra registry or renderer work. Session-specific correlation data such as stable session IDs, wrapper identity, and provider-exchange references will be attached through first-turn context metadata, per-item IDs, and system-message content.

Raw provider request bodies, response bodies, and stream-frame evidence will remain local session-ledger artifacts under `.scratch/cxtx/`, keyed by exchange correlation IDs. CXDB remains the extracted conversation and lifecycle record; the ledger remains the debugging and verification evidence store.

Provider-specific child handling is explicit for this sprint. `codex` receives `OPENAI_BASE_URL` and `OPENAI_API_BASE` pointed at the local proxy with the OpenAI-compatible upstream base path preserved. `claude` receives `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_URL`, `ANTHROPIC_API_BASE`, `CLAUDE_BASE_URL`, `CLAUDE_API_BASE`, and `CLAUDE_CODE_BASE_URL` pointed at the local proxy root. Arguments after the wrapper's `--` separator are forwarded verbatim to the child command in original order, and existing auth environment such as `OPENAI_API_KEY` or `ANTHROPIC_API_KEY` is inherited unchanged.

`cxtx` will not emit wrapper-originated stdout during successful execution. Child process stdout and stderr remain authoritative, and wrapper-authored terminal output is reserved for fatal preflight errors and explicitly documented failure conditions.

This sprint does not introduce a separate "capture-only, no ingest" mode. If `--url` is omitted, `cxtx` still targets the documented local default, but a failed CXDB connection is handled as a degraded queued-delivery condition rather than a cryptic immediate abort. The child process still launches, and `cxtx` emits a clear stderr status message describing the degraded ingest state.

### Consequences

#### Positive

- Reuses an already-proven wrapper technique instead of inventing a new interception model.
- Avoids requiring downstream users to understand the CXDB binary protocol for this workflow.
- Produces a CXDB session shape that more closely matches how operators expect to review a conversation.
- Uses a stable session ID plus provider correlation state to upload only newly observed turns even when providers replay full history on every request.
- Keeps the new feature inside the Rust workspace, where the proxy-oriented implementation fits naturally.
- Preserves the interactive feel of `claude` and `codex` because the wrapper avoids competing terminal chatter.
- Avoids losing early session events when CXDB is temporarily unavailable because delivery is retried from an in-memory queue.
- Reuses the repo's existing canonical conversation type, metadata extraction path, and frontend renderer instead of creating a second primary browsing experience.

#### Negative

- Turn extraction reintroduces provider-specific interpretation logic and replay-handling complexity that must be specified and tested carefully.
- HTTP ingest duplicates some client behavior that already exists in the binary SDKs.
- The wrapper only works for CLIs that honor provider base URL environment variables.
- In-memory queued delivery introduces queue-management and shutdown-drain complexity that the implementation must make explicit and test thoroughly.
- Operators who need raw provider payloads must correlate CXDB system messages with local session-ledger artifacts instead of reading raw payloads directly from CXDB turns.

#### Follow-Up

- If downstream consumers later need full raw-traffic preservation in addition to extracted conversation turns, a later ADR can add optional raw-capture persistence alongside the extracted-conversation path.
- If prolonged CXDB outages or very large sessions make memory-only buffering unsafe, a later ADR can add disk-backed spooling. That is out of scope for this sprint.
