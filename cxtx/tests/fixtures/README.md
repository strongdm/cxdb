# Provider usage fixtures

These fixtures exercise the `UsageOutcome` parser in `cxtx/src/provider/usage.rs`
and the usage-extraction paths in `cxtx/src/provider/{anthropic,openai}.rs`.
Each subdirectory under `usage/` represents one row of the 16-cell provider
matrix documented in Sprint 016.

## Layout

Each fixture directory contains one of:

- `stream.sse` — a single SSE event or small sequence of events, one frame
  per `\n\n`-terminated block, suitable for feeding into
  `parse_sse_buffer` and then `absorb_sse_frame`. Used by streaming tests.
- `body.json` — a JSON response body. Used by the non-streaming finalize
  paths.
- `expected.json` — the expected `UsageOutcome` shape (serde-tagged enum
  with snake-case variant names).

Some fixtures carry both `stream.sse` and `expected.json`; some carry
`body.json` and `expected.json`.

## Redaction policy

Fixtures MUST NOT contain any of:

- API keys or bearer tokens in any form (matches regex
  `(sk|OPENAI|ANTHROPIC).{0,5}[_-]?(KEY|TOKEN)`).
- StrongDM-affiliated email addresses (matches regex `@strongdm\.`).
- Actual user prompts, assistant completions, or tool I/O from real
  sessions. These fixtures are synthetic — they exist only to exercise
  `usage` / `finish_reason` / `response.status` / cache-breakdown shapes.
  If a fixture needs to contain illustrative text, it must be obviously
  fake placeholder text.

The `fixtures_lint.rs` integration test scans every committed file under
`cxtx/tests/fixtures/` and fails the build if either regex matches.

## How to add a new fixture

1. Pick a clear, hyphen-snake directory name under `usage/` describing the
   matrix cell.
2. Author the minimum terminal event or response body faithful to the
   provider's documented shape. Keep `content` / `choices[].message.content`
   fields empty or a short placeholder like `"ok"`.
3. Author `expected.json` with the expected `UsageOutcome`.
4. Run `cargo test -p cxtx --test usage_matrix` and iterate.
5. Run `cargo test -p cxtx --test fixtures_lint` to ensure no secrets.
