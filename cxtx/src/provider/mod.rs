pub mod anthropic;
pub mod openai;

use anyhow::{anyhow, Context, Result};
use http::Uri;
use serde_json::Value;
use std::collections::BTreeMap;
use std::env;
use url::Url;

use crate::session::SessionRuntime;
use crate::turns::{ArtifactRefs, TurnEnvelope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Codex,
    Claude,
}

#[derive(Debug)]
pub struct PreparedExchange {
    pub exchange_id: String,
    pub model: Option<String>,
    pub request_turns: Vec<TurnEnvelope>,
    pub state: ExchangeState,
}

#[derive(Debug)]
pub enum ExchangeState {
    OpenAi(openai::OpenAiExchange),
    Anthropic(anthropic::AnthropicExchange),
}

impl ProviderKind {
    pub fn command_name(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
        }
    }

    pub fn provider_name(self) -> &'static str {
        match self {
            Self::Codex => "openai",
            Self::Claude => "anthropic",
        }
    }

    pub fn client_tag(self) -> &'static str {
        match self {
            Self::Codex => "cxtx/codex",
            Self::Claude => "cxtx/claude",
        }
    }

    pub fn child_args(self, args: &[String]) -> Vec<String> {
        self.child_args_for_proxy(args, None)
    }

    pub fn child_args_for_proxy(
        self,
        args: &[String],
        _proxy_base_url: Option<&Url>,
    ) -> Vec<String> {
        match self {
            Self::Codex => {
                let mut out = Vec::new();
                if !has_codex_config_override(args, "prefer_websockets") {
                    out.extend(["-c".to_string(), "prefer_websockets=false".to_string()]);
                }
                if !has_codex_feature_override(args, "responses_websockets") {
                    out.extend([
                        "--disable".to_string(),
                        "responses_websockets".to_string(),
                    ]);
                }
                if !has_codex_feature_override(args, "responses_websockets_v2") {
                    out.extend([
                        "--disable".to_string(),
                        "responses_websockets_v2".to_string(),
                    ]);
                }
                out.extend(args.iter().cloned());
                out
            }
            Self::Claude => args.to_vec(),
        }
    }

    pub fn labels(self) -> Vec<String> {
        vec![
            "cxtx".to_string(),
            match self {
                Self::Codex => "codex",
                Self::Claude => "claude",
            }
            .to_string(),
            "interactive".to_string(),
        ]
    }

    pub fn resolve_upstream_base(self) -> Result<Url> {
        let env_names = self.upstream_base_env_names();
        let value = env_names
            .iter()
            .find_map(|name| env::var(name).ok().filter(|v| !v.trim().is_empty()));
        let default = match self {
            Self::Codex => "https://api.openai.com/v1",
            Self::Claude => "https://api.anthropic.com",
        };
        let parsed = Url::parse(value.as_deref().unwrap_or(default)).with_context(|| {
            format!(
                "failed to parse upstream URL from {}",
                value.unwrap_or_else(|| default.to_string())
            )
        })?;
        Ok(parsed)
    }

    pub fn capture_env_allowlist(self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for name in [
            "HOME",
            "LOGNAME",
            "PWD",
            "SHELL",
            "TERM",
            "USER",
            "OPENAI_BASE_URL",
            "OPENAI_API_BASE",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_API_URL",
            "ANTHROPIC_API_BASE",
            "CLAUDE_BASE_URL",
            "CLAUDE_API_BASE",
            "CLAUDE_CODE_BASE_URL",
        ] {
            if let Ok(value) = env::var(name) {
                if !value.trim().is_empty() {
                    out.insert(name.to_string(), value);
                }
            }
        }
        out
    }

    pub fn injected_env(self, proxy_base_url: &Url) -> Vec<(String, String)> {
        let value = proxy_base_url.as_str().trim_end_matches('/').to_string();
        match self {
            Self::Codex => vec![
                ("OPENAI_BASE_URL".to_string(), value.clone()),
                ("OPENAI_API_BASE".to_string(), value.clone()),
                ("CXTX_OPENAI_BASE_URL".to_string(), value.clone()),
                ("CXTX_OPENAI_API_BASE".to_string(), value),
            ],
            Self::Claude => {
                let root = proxy_base_url.origin().unicode_serialization();
                vec![
                    ("ANTHROPIC_BASE_URL".to_string(), root.clone()),
                    ("ANTHROPIC_API_URL".to_string(), root.clone()),
                    ("ANTHROPIC_API_BASE".to_string(), root.clone()),
                    ("CLAUDE_BASE_URL".to_string(), root.clone()),
                    ("CLAUDE_API_BASE".to_string(), root.clone()),
                    ("CLAUDE_CODE_BASE_URL".to_string(), root),
                ]
            }
        }
    }

    pub fn proxy_mount_path(self, upstream_base: &Url) -> String {
        match self {
            Self::Codex => normalize_path(upstream_base.path()),
            Self::Claude => "/".to_string(),
        }
    }

    pub fn build_upstream_url(self, upstream_base: &Url, uri: &Uri) -> Result<Url> {
        let path = uri.path();
        let mount_path = self.proxy_mount_path(upstream_base);
        let relative_path = match self {
            Self::Codex => strip_mount_path(path, &mount_path).ok_or_else(|| {
                anyhow!("request path {path} does not match proxy mount {mount_path}")
            })?,
            Self::Claude => path.to_string(),
        };

        join_url(upstream_base, &relative_path, uri.query())
    }

    pub fn request_id_from_headers(self, headers: &reqwest::header::HeaderMap) -> Option<String> {
        let names: &[&str] = match self {
            Self::Codex => &["x-request-id", "request-id"],
            Self::Claude => &["request-id", "anthropic-request-id", "x-request-id"],
        };
        names.iter().find_map(|name| {
            headers
                .get(*name)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.to_string())
        })
    }

    pub fn allowlisted_headers(
        self,
        headers: &reqwest::header::HeaderMap,
    ) -> BTreeMap<String, String> {
        let names: &[&str] = match self {
            Self::Codex => &[
                "accept",
                "content-length",
                "content-type",
                "openai-processing-ms",
                "request-id",
                "user-agent",
                "x-request-id",
            ],
            Self::Claude => &[
                "accept",
                "anthropic-version",
                "anthropic-request-id",
                "content-length",
                "content-type",
                "request-id",
                "user-agent",
                "x-request-id",
            ],
        };

        let mut out = BTreeMap::new();
        for name in names {
            if let Some(value) = headers.get(*name).and_then(|value| value.to_str().ok()) {
                out.insert((*name).to_string(), value.to_string());
            }
        }
        out
    }

    pub fn model_from_payload(self, payload: &Value) -> Option<String> {
        find_model_field(payload)
    }

    pub fn prepare_exchange(
        self,
        session: &SessionRuntime,
        exchange_id: String,
        body: &[u8],
        artifact_refs: &ArtifactRefs,
    ) -> PreparedExchange {
        match self {
            Self::Codex => openai::prepare_exchange(session, exchange_id, body, artifact_refs),
            Self::Claude => anthropic::prepare_exchange(session, exchange_id, body, artifact_refs),
        }
    }

    pub fn upstream_base_env_names(self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["OPENAI_BASE_URL", "OPENAI_API_BASE"],
            Self::Claude => &[
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_API_URL",
                "ANTHROPIC_API_BASE",
                "CLAUDE_BASE_URL",
                "CLAUDE_API_BASE",
                "CLAUDE_CODE_BASE_URL",
            ],
        }
    }
}

impl ExchangeState {
    pub fn finalize_json(
        self,
        session: &SessionRuntime,
        status: u16,
        request_id: Option<String>,
        body: &[u8],
        artifact_refs: &ArtifactRefs,
    ) -> Vec<TurnEnvelope> {
        match self {
            Self::OpenAi(state) => {
                openai::finalize_json(session, state, status, request_id, body, artifact_refs)
            }
            Self::Anthropic(state) => {
                anthropic::finalize_json(session, state, status, request_id, body, artifact_refs)
            }
        }
    }

    pub fn absorb_sse_frame(&mut self, frame: &openai::SseFrame) {
        match self {
            Self::OpenAi(state) => state.absorb_sse_frame(frame),
            Self::Anthropic(state) => state.absorb_sse_frame(frame),
        }
    }

    pub fn finalize_stream(
        self,
        session: &SessionRuntime,
        status: u16,
        request_id: Option<String>,
        artifact_refs: &ArtifactRefs,
        malformed_remainder: Option<String>,
    ) -> Vec<TurnEnvelope> {
        match self {
            Self::OpenAi(state) => openai::finalize_stream(
                session,
                state,
                status,
                request_id,
                artifact_refs,
                malformed_remainder,
            ),
            Self::Anthropic(state) => anthropic::finalize_stream(
                session,
                state,
                status,
                request_id,
                artifact_refs,
                malformed_remainder,
            ),
        }
    }
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn strip_mount_path(path: &str, mount_path: &str) -> Option<String> {
    if mount_path == "/" {
        return Some(path.to_string());
    }
    let suffix = path.strip_prefix(mount_path)?;
    if suffix.is_empty() {
        Some("/".to_string())
    } else if suffix.starts_with('/') {
        Some(suffix.to_string())
    } else {
        Some(format!("/{suffix}"))
    }
}

fn join_url(base: &Url, path: &str, query: Option<&str>) -> Result<Url> {
    let mut joined = base.clone();
    let base_path = base.path().trim_end_matches('/');
    let extra_path = path.trim_start_matches('/');
    let final_path = if base_path.is_empty() {
        format!("/{}", extra_path)
    } else if extra_path.is_empty() {
        base_path.to_string()
    } else {
        format!("{base_path}/{extra_path}")
    };
    joined.set_path(&final_path);
    joined.set_query(query);
    Ok(joined)
}

fn find_model_field(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(model) = map.get("model").and_then(Value::as_str) {
                return Some(model.to_string());
            }
            map.values().find_map(find_model_field)
        }
        Value::Array(values) => values.iter().find_map(find_model_field),
        _ => None,
    }
}

fn has_codex_config_override(args: &[String], key: &str) -> bool {
    args.iter().enumerate().any(|(index, arg)| {
        if let Some(value) = arg.strip_prefix("--config=") {
            return value.trim_start().starts_with(&format!("{key}="));
        }
        if let Some(value) = arg.strip_prefix("-c") {
            if !value.is_empty() {
                return value.trim_start().starts_with(&format!("{key}="));
            }
        }
        matches!(arg.as_str(), "-c" | "--config")
            && args
                .get(index + 1)
                .is_some_and(|value| value.trim_start().starts_with(&format!("{key}=")))
    })
}

fn has_codex_feature_override(args: &[String], feature: &str) -> bool {
    args.iter().enumerate().any(|(index, arg)| {
        if let Some(value) = arg.strip_prefix("--enable=") {
            return value == feature;
        }
        if let Some(value) = arg.strip_prefix("--disable=") {
            return value == feature;
        }
        matches!(arg.as_str(), "--enable" | "--disable")
            && args.get(index + 1).is_some_and(|value| value == feature)
    }) || has_codex_config_override(args, &format!("features.{feature}"))
}

#[cfg(test)]
mod tests {
    use super::ProviderKind;
    use http::Uri;
    use url::Url;

    #[test]
    fn codex_preserves_upstream_base_path() {
        let upstream = Url::parse("https://example.test/custom/v1").unwrap();
        let uri: Uri = "/custom/v1/chat/completions?stream=true".parse().unwrap();
        let routed = ProviderKind::Codex
            .build_upstream_url(&upstream, &uri)
            .unwrap();
        assert_eq!(
            routed.as_str(),
            "https://example.test/custom/v1/chat/completions?stream=true"
        );
    }

    #[test]
    fn claude_routes_from_root_proxy_to_upstream_path() {
        let upstream = Url::parse("https://example.test/anthropic").unwrap();
        let uri: Uri = "/v1/messages".parse().unwrap();
        let routed = ProviderKind::Claude
            .build_upstream_url(&upstream, &uri)
            .unwrap();
        assert_eq!(
            routed.as_str(),
            "https://example.test/anthropic/v1/messages"
        );
    }

    #[test]
    fn codex_child_args_disable_websockets_by_default() {
        let args = vec!["exec".to_string(), "say hi".to_string()];
        assert_eq!(
            ProviderKind::Codex.child_args(&args),
            vec![
                "-c".to_string(),
                "prefer_websockets=false".to_string(),
                "--disable".to_string(),
                "responses_websockets".to_string(),
                "--disable".to_string(),
                "responses_websockets_v2".to_string(),
                "exec".to_string(),
                "say hi".to_string(),
            ]
        );
    }

    #[test]
    fn codex_child_args_preserve_explicit_prefer_websockets_override() {
        let args = vec![
            "--config".to_string(),
            "prefer_websockets=true".to_string(),
            "exec".to_string(),
        ];
        assert_eq!(
            ProviderKind::Codex.child_args(&args),
            vec![
                "--disable".to_string(),
                "responses_websockets".to_string(),
                "--disable".to_string(),
                "responses_websockets_v2".to_string(),
                "--config".to_string(),
                "prefer_websockets=true".to_string(),
                "exec".to_string(),
            ]
        );
    }

    #[test]
    fn codex_child_args_preserve_explicit_feature_overrides() {
        let args = vec![
            "--disable".to_string(),
            "responses_websockets".to_string(),
            "--enable".to_string(),
            "responses_websockets_v2".to_string(),
            "exec".to_string(),
        ];
        assert_eq!(
            ProviderKind::Codex.child_args(&args),
            vec![
                "-c".to_string(),
                "prefer_websockets=false".to_string(),
                "--disable".to_string(),
                "responses_websockets".to_string(),
                "--enable".to_string(),
                "responses_websockets_v2".to_string(),
                "exec".to_string(),
            ]
        );
    }

    #[test]
    fn codex_child_args_include_same_result_when_proxy_is_known() {
        let proxy = Url::parse("http://127.0.0.1:48123/v1").unwrap();
        let args = vec!["exec".to_string()];
        assert_eq!(
            ProviderKind::Codex.child_args_for_proxy(&args, Some(&proxy)),
            vec![
                "-c".to_string(),
                "prefer_websockets=false".to_string(),
                "--disable".to_string(),
                "responses_websockets".to_string(),
                "--disable".to_string(),
                "responses_websockets_v2".to_string(),
                "exec".to_string(),
            ]
        );
    }

    #[test]
    fn claude_child_args_are_unchanged() {
        let args = vec!["--print".to_string(), "stream".to_string()];
        assert_eq!(ProviderKind::Claude.child_args(&args), args);
    }
}
