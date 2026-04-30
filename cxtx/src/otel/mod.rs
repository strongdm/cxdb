//! cxtx-local OpenTelemetry domain logic for LLM call emission.
//!
//! Responsibilities:
//!
//! - `finish_reasons` — provider-native → canonical finish-reason mapping
//!   (the 14-row table from `OTEL_SPEC.md` §"Finish-reason mapping").
//! - `buckets` — derived token-bucket validation producing a non-overlapping
//!   `Vec<(TokenType, u64)>` consumed by the `gen_ai.client.token.usage`
//!   histogram.
//! - `call_context` — `CallContext` + `AppAttribution` wiring threaded
//!   through the exchange runtime; deliberately NOT part of `HistoryItem`
//!   so replay dedup is unaffected.
//! - `llm_call` — the single `finalize_llm_call` emit site that every
//!   provider finalize block routes through.

pub mod buckets;
pub mod call_context;
pub mod finish_reasons;
pub mod llm_call;
