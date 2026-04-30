//! `CallContext` + `AppAttribution` — transient per-exchange telemetry
//! plumbing.
//!
//! Design Decision 1 (see sprint doc): `CallContext` threads via
//! `ExchangeState`, NOT via `HistoryItem`. Replay dedup normalization in
//! `cxtx/src/session.rs` keeps comparing semantic conversation content
//! only; `CallContext.t_start` never appears as a `HistoryItem` field.

use std::time::Instant;

use cxdb::types::ContextMetadata;

/// Flattened attribution pulled from `ContextMetadata` — one copy per
/// exchange so downstream emit sites don't have to repeatedly index into
/// `HashMap<String, String>` at call time.
#[derive(Debug, Clone)]
pub struct AppAttribution {
    pub client_tag: String,
    pub wrapper_command: String,
    pub wrapper_version: String,
    pub provider_kind: String,
    pub session_id: String,
    pub user: Option<String>,
    /// Tenant: tenant label (`app.tenant`) sourced from
    /// `ContextMetadata.tenant`. `None` means the caller did not set a
    /// tenant — emit sites MUST omit the attribute entirely. No
    /// sentinel, no empty string.
    pub tenant: Option<String>,
}

impl AppAttribution {
    /// Build from a fully-populated `ContextMetadata`. Missing custom
    /// fields fall back to empty strings rather than panicking — cxtx's
    /// `context_metadata()` always populates them, but tests may construct
    /// bare metadata.
    pub fn from_metadata(metadata: &ContextMetadata) -> Self {
        let client_tag = metadata.client_tag.clone();
        let custom = &metadata.custom;
        let wrapper_command = custom
            .get("wrapper_command")
            .cloned()
            .unwrap_or_default();
        let wrapper_version = custom
            .get("wrapper_version")
            .cloned()
            .unwrap_or_default();
        let provider_kind = custom.get("provider_kind").cloned().unwrap_or_default();
        let session_id = custom
            .get("stable_session_id")
            .cloned()
            .unwrap_or_default();

        // `app.user` comes from provenance.on_behalf_of (falls back to
        // $USER per spec "Application attribution" row).
        let user = metadata
            .provenance
            .as_ref()
            .map(|p| p.on_behalf_of.clone())
            .filter(|s| !s.is_empty());

        // Decision #8: tenant flows through `AppAttribution`
        // (NOT as a sibling on `CallContext`). Empty string is treated
        // as `None` — the missing-tenant rule is applied at the
        // flattening seam so downstream emit sites never have to
        // re-filter.
        let tenant = metadata
            .tenant
            .as_ref()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        Self {
            client_tag,
            wrapper_command,
            wrapper_version,
            provider_kind,
            session_id,
            user,
            tenant,
        }
    }
}

/// Per-exchange call context. Produced at upstream-connect time
/// (`proxy.rs`), consumed by `finalize_llm_call` after `UsageOutcome` is
/// parsed.
///
/// `t_start` is an `Instant` so span duration math never depends on wall
/// clock skew. `t_end` is captured by `finalize_llm_call` itself.
#[derive(Debug, Clone)]
pub struct CallContext {
    pub t_start: Instant,
    pub request_model: String,
    pub provider_system: &'static str,
    pub attribution: AppAttribution,
    pub is_stream: bool,
}

impl CallContext {
    pub fn new(
        t_start: Instant,
        request_model: impl Into<String>,
        provider_system: &'static str,
        attribution: AppAttribution,
        is_stream: bool,
    ) -> Self {
        Self {
            t_start,
            request_model: request_model.into(),
            provider_system,
            attribution,
            is_stream,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn attribution_from_metadata_copies_custom_fields() {
        let mut custom = HashMap::new();
        custom.insert("stable_session_id".to_string(), "sess-123".to_string());
        custom.insert("wrapper_command".to_string(), "claude".to_string());
        custom.insert("wrapper_version".to_string(), "0.1.0".to_string());
        custom.insert("provider_kind".to_string(), "anthropic".to_string());
        let metadata = ContextMetadata {
            client_tag: "cxtx/claude".to_string(),
            title: String::new(),
            labels: Vec::new(),
            custom,
            tenant: None,
            provenance: None,
        };
        let a = AppAttribution::from_metadata(&metadata);
        assert_eq!(a.client_tag, "cxtx/claude");
        assert_eq!(a.session_id, "sess-123");
        assert_eq!(a.wrapper_command, "claude");
        assert_eq!(a.wrapper_version, "0.1.0");
        assert_eq!(a.provider_kind, "anthropic");
        assert_eq!(a.user, None);
        assert_eq!(a.tenant, None);
    }

    /// Tenant: tenant on `ContextMetadata` flows through to
    /// `AppAttribution.tenant` at the flattening seam.
    #[test]
    fn attribution_from_metadata_copies_tenant_when_present() {
        let metadata = ContextMetadata {
            client_tag: "cxtx/claude".to_string(),
            title: String::new(),
            labels: Vec::new(),
            custom: HashMap::new(),
            tenant: Some("tenant-x".to_string()),
            provenance: None,
        };
        let a = AppAttribution::from_metadata(&metadata);
        assert_eq!(a.tenant.as_deref(), Some("tenant-x"));
    }

    /// Tenant: empty-string tenant on the wire is treated as absent
    /// (no sentinel, no empty-string stamp).
    #[test]
    fn attribution_from_metadata_treats_empty_tenant_as_none() {
        let metadata = ContextMetadata {
            client_tag: "cxtx/claude".to_string(),
            title: String::new(),
            labels: Vec::new(),
            custom: HashMap::new(),
            tenant: Some(String::new()),
            provenance: None,
        };
        let a = AppAttribution::from_metadata(&metadata);
        assert_eq!(a.tenant, None);
    }
}
