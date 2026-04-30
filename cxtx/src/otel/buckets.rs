//! Derived-bucket validation for `gen_ai.client.token.usage`.
//!
//! Per `OTEL_SPEC.md` §"Derived bucket validation":
//! - `input = raw.input_tokens - raw.cached_tokens` (negative → invalid)
//! - `output = raw.output_tokens - raw.reasoning_tokens` when
//!   `reasoning_tokens > 0`, else raw (negative → invalid)
//! - Anthropic cache-write:
//!   * aggregate-only (no `cache_creation` breakdown) → emit `CacheWrite`
//!   * breakdown present AND parts sum to aggregate (or aggregate absent)
//!     → emit `CacheWrite5m` / `CacheWrite1h`
//!   * breakdown present AND parts don't sum to aggregate → mismatch
//! - Zero-valued buckets are skipped (not present in the returned Vec).

use crate::provider::usage::RawUsage;

/// Non-overlapping token-type buckets emitted as samples of
/// `gen_ai.client.token.usage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenType {
    Input,
    Cached,
    Output,
    Reasoning,
    CacheWrite5m,
    CacheWrite1h,
    /// Aggregate cache-write bucket used when the provider reports the
    /// total without a per-TTL breakdown.
    CacheWrite,
}

impl TokenType {
    /// String representation used as the `gen_ai.token.type` tag value.
    pub fn as_str(self) -> &'static str {
        match self {
            TokenType::Input => "input",
            TokenType::Cached => "cached",
            TokenType::Output => "output",
            TokenType::Reasoning => "reasoning",
            TokenType::CacheWrite5m => "cache_write_5m",
            TokenType::CacheWrite1h => "cache_write_1h",
            TokenType::CacheWrite => "cache_write",
        }
    }
}

impl cxdb_otel::gen_ai::TokenTypeName for TokenType {
    fn token_type_name(&self) -> &str {
        self.as_str()
    }
}

/// Why a usage payload was rejected by `derive_and_validate`. The string
/// form of the variant is stamped as the `llm.usage_invalid_reason` span
/// attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    NegativeInput,
    NegativeOutput,
    CacheBreakdownMismatch,
    Other(String),
}

impl InvalidReason {
    /// Tag value for `llm.usage_invalid_reason`.
    pub fn as_str(&self) -> &str {
        match self {
            InvalidReason::NegativeInput => "negative_input",
            InvalidReason::NegativeOutput => "negative_output",
            InvalidReason::CacheBreakdownMismatch => "cache_breakdown_mismatch",
            InvalidReason::Other(s) => s.as_str(),
        }
    }
}

/// Derive the non-overlapping bucket vec from a `RawUsage`. Returns
/// `Err(InvalidReason)` when the payload fails validation and should
/// route through the `usage_missing{reason=invalid}` path.
///
/// Zero-valued buckets are NOT included in the returned Vec. An all-zero
/// payload returns `Ok(vec![])` — the caller still stamps `gen_ai.calls`.
pub fn derive_and_validate(raw: &RawUsage) -> Result<Vec<(TokenType, u64)>, InvalidReason> {
    // Input = input_tokens - cached_tokens.
    let input = (raw.input_tokens as i128) - (raw.cached_tokens as i128);
    if input < 0 {
        return Err(InvalidReason::NegativeInput);
    }

    // Output = output_tokens - reasoning_tokens (only when reasoning > 0).
    let output_i = if raw.reasoning_tokens > 0 {
        (raw.output_tokens as i128) - (raw.reasoning_tokens as i128)
    } else {
        raw.output_tokens as i128
    };
    if output_i < 0 {
        return Err(InvalidReason::NegativeOutput);
    }

    // Anthropic cache-write reconciliation.
    //
    // Single rule set (spec §"Derived bucket validation"):
    // - aggregate only (no breakdown)    → emit `CacheWrite`
    // - breakdown present, parts sum to aggregate (or aggregate == 0)
    //                                    → emit `CacheWrite5m`/`CacheWrite1h`
    // - breakdown present, parts do NOT sum to aggregate → mismatch
    let has_breakdown = raw.cache_creation_5m > 0 || raw.cache_creation_1h > 0;
    let breakdown_sum = raw.cache_creation_5m.saturating_add(raw.cache_creation_1h);
    let mut cache_writes: Vec<(TokenType, u64)> = Vec::new();

    if has_breakdown {
        if raw.cache_creation_total > 0 && raw.cache_creation_total != breakdown_sum {
            return Err(InvalidReason::CacheBreakdownMismatch);
        }
        if raw.cache_creation_5m > 0 {
            cache_writes.push((TokenType::CacheWrite5m, raw.cache_creation_5m));
        }
        if raw.cache_creation_1h > 0 {
            cache_writes.push((TokenType::CacheWrite1h, raw.cache_creation_1h));
        }
    } else if raw.cache_creation_total > 0 {
        cache_writes.push((TokenType::CacheWrite, raw.cache_creation_total));
    }

    let mut out: Vec<(TokenType, u64)> = Vec::new();
    if input > 0 {
        out.push((TokenType::Input, input as u64));
    }
    if raw.cached_tokens > 0 {
        out.push((TokenType::Cached, raw.cached_tokens));
    }
    if output_i > 0 {
        out.push((TokenType::Output, output_i as u64));
    }
    if raw.reasoning_tokens > 0 {
        out.push((TokenType::Reasoning, raw.reasoning_tokens));
    }
    out.extend(cache_writes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(input: u64, output: u64) -> RawUsage {
        RawUsage {
            input_tokens: input,
            output_tokens: output,
            ..RawUsage::default()
        }
    }

    /// P1-T5: Happy-path bucket derivation.
    #[test]
    fn happy_path_openai_style_buckets() {
        let r = RawUsage {
            input_tokens: 100,
            output_tokens: 50,
            cached_tokens: 20,
            reasoning_tokens: 10,
            ..RawUsage::default()
        };
        let buckets = derive_and_validate(&r).unwrap();
        // input = 100 - 20 = 80; cached = 20; output = 50 - 10 = 40; reasoning = 10
        assert_eq!(
            buckets,
            vec![
                (TokenType::Input, 80),
                (TokenType::Cached, 20),
                (TokenType::Output, 40),
                (TokenType::Reasoning, 10),
            ]
        );
    }

    /// P1-T5 (provider variant): Anthropic-style path (no reasoning).
    #[test]
    fn happy_path_anthropic_without_cache_breakdown() {
        let r = RawUsage {
            input_tokens: 100,
            output_tokens: 20,
            cached_tokens: 30,
            cache_creation_total: 15,
            ..RawUsage::default()
        };
        let buckets = derive_and_validate(&r).unwrap();
        // input = 100 - 30 = 70; cached = 30; output = 20; cache_write = 15
        assert_eq!(
            buckets,
            vec![
                (TokenType::Input, 70),
                (TokenType::Cached, 30),
                (TokenType::Output, 20),
                (TokenType::CacheWrite, 15),
            ]
        );
    }

    /// P1-T6: Negative-input guard.
    #[test]
    fn negative_input_routes_to_invalid() {
        let r = RawUsage {
            input_tokens: 5,
            output_tokens: 10,
            cached_tokens: 8,
            ..RawUsage::default()
        };
        assert_eq!(
            derive_and_validate(&r).unwrap_err(),
            InvalidReason::NegativeInput
        );
    }

    /// P1-T7: Negative-output guard.
    #[test]
    fn negative_output_routes_to_invalid() {
        let r = RawUsage {
            input_tokens: 10,
            output_tokens: 4,
            reasoning_tokens: 9,
            ..RawUsage::default()
        };
        assert_eq!(
            derive_and_validate(&r).unwrap_err(),
            InvalidReason::NegativeOutput
        );
    }

    /// P1-T8: Anthropic aggregate-only → `CacheWrite` emitted.
    #[test]
    fn anthropic_aggregate_only_emits_cache_write() {
        let r = RawUsage {
            input_tokens: 50,
            output_tokens: 10,
            cache_creation_total: 40,
            ..RawUsage::default()
        };
        let buckets = derive_and_validate(&r).unwrap();
        assert!(buckets.contains(&(TokenType::CacheWrite, 40)));
        assert!(!buckets.iter().any(|(t, _)| *t == TokenType::CacheWrite5m));
        assert!(!buckets.iter().any(|(t, _)| *t == TokenType::CacheWrite1h));
    }

    /// P1-T9: Breakdown-matching → 5m + 1h, no `CacheWrite`.
    #[test]
    fn anthropic_breakdown_matches_aggregate() {
        let r = RawUsage {
            input_tokens: 50,
            output_tokens: 10,
            cache_creation_total: 40,
            cache_creation_5m: 25,
            cache_creation_1h: 15,
            ..RawUsage::default()
        };
        let buckets = derive_and_validate(&r).unwrap();
        assert!(buckets.contains(&(TokenType::CacheWrite5m, 25)));
        assert!(buckets.contains(&(TokenType::CacheWrite1h, 15)));
        assert!(!buckets.iter().any(|(t, _)| *t == TokenType::CacheWrite));
    }

    /// P1-T9 (variant): Breakdown present but aggregate absent — parts sum
    /// is treated as valid.
    #[test]
    fn anthropic_breakdown_without_aggregate_is_valid() {
        let r = RawUsage {
            input_tokens: 50,
            output_tokens: 10,
            cache_creation_5m: 25,
            cache_creation_1h: 15,
            cache_creation_total: 0,
            ..RawUsage::default()
        };
        let buckets = derive_and_validate(&r).unwrap();
        assert!(buckets.contains(&(TokenType::CacheWrite5m, 25)));
        assert!(buckets.contains(&(TokenType::CacheWrite1h, 15)));
    }

    /// P1-T10: Breakdown-mismatch → `CacheBreakdownMismatch`.
    #[test]
    fn anthropic_breakdown_mismatch_is_invalid() {
        let r = RawUsage {
            input_tokens: 50,
            output_tokens: 10,
            cache_creation_total: 100, // aggregate claims 100...
            cache_creation_5m: 25,     // ...but parts sum to 40.
            cache_creation_1h: 15,
            ..RawUsage::default()
        };
        assert_eq!(
            derive_and_validate(&r).unwrap_err(),
            InvalidReason::CacheBreakdownMismatch
        );
    }

    /// P1-T11: All-zero raw → empty Vec (valid, caller still emits
    /// `gen_ai.calls`).
    #[test]
    fn all_zero_raw_returns_empty_ok() {
        let r = RawUsage::default();
        let buckets = derive_and_validate(&r).unwrap();
        assert!(buckets.is_empty());
    }

    /// Additional: zero-valued reasoning doesn't trigger the "subtract"
    /// branch (so output_tokens passes through raw).
    #[test]
    fn zero_reasoning_does_not_change_output() {
        let r = raw(100, 10);
        let buckets = derive_and_validate(&r).unwrap();
        assert_eq!(buckets, vec![(TokenType::Input, 100), (TokenType::Output, 10)]);
    }
}
