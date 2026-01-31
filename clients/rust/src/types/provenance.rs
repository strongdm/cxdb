// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

#![allow(non_upper_case_globals)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Provenance {
    #[serde(rename = "1")]
    pub parent_context_id: Option<u64>,
    #[serde(rename = "2", skip_serializing_if = "String::is_empty")]
    pub spawn_reason: String,
    #[serde(rename = "3")]
    pub root_context_id: Option<u64>,

    #[serde(rename = "10", skip_serializing_if = "String::is_empty")]
    pub trace_id: String,
    #[serde(rename = "11", skip_serializing_if = "String::is_empty")]
    pub span_id: String,
    #[serde(rename = "12", skip_serializing_if = "String::is_empty")]
    pub correlation_id: String,

    #[serde(rename = "20", skip_serializing_if = "String::is_empty")]
    pub on_behalf_of: String,
    #[serde(rename = "21", skip_serializing_if = "String::is_empty")]
    pub on_behalf_of_source: String,
    #[serde(rename = "22", skip_serializing_if = "String::is_empty")]
    pub on_behalf_of_email: String,

    #[serde(rename = "30", skip_serializing_if = "String::is_empty")]
    pub writer_method: String,
    #[serde(rename = "31", skip_serializing_if = "String::is_empty")]
    pub writer_subject: String,
    #[serde(rename = "32", skip_serializing_if = "String::is_empty")]
    pub writer_issuer: String,

    #[serde(rename = "40", skip_serializing_if = "String::is_empty")]
    pub service_name: String,
    #[serde(rename = "41", skip_serializing_if = "String::is_empty")]
    pub service_version: String,
    #[serde(rename = "42", skip_serializing_if = "String::is_empty")]
    pub service_instance_id: String,
    #[serde(rename = "43", skip_serializing_if = "is_zero_i64")]
    pub process_pid: i64,
    #[serde(rename = "44", skip_serializing_if = "String::is_empty")]
    pub process_owner: String,
    #[serde(rename = "45", skip_serializing_if = "String::is_empty")]
    pub host_name: String,
    #[serde(rename = "46", skip_serializing_if = "String::is_empty")]
    pub host_arch: String,

    #[serde(rename = "50", skip_serializing_if = "String::is_empty")]
    pub client_address: String,
    #[serde(rename = "51", skip_serializing_if = "is_zero_i64")]
    pub client_port: i64,

    #[serde(rename = "60")]
    pub env_vars: Option<HashMap<String, String>>,

    #[serde(rename = "70", skip_serializing_if = "String::is_empty")]
    pub sdk_name: String,
    #[serde(rename = "71", skip_serializing_if = "String::is_empty")]
    pub sdk_version: String,

    #[serde(rename = "80", skip_serializing_if = "is_zero_i64")]
    pub captured_at: i64,
}


pub static DefaultEnvAllowlist: &[&str] = &[
    "K8S_NAMESPACE",
    "K8S_POD_NAME",
    "K8S_NODE_NAME",
    "KUBERNETES_SERVICE_HOST",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "AWS_EXECUTION_ENV",
    "GOOGLE_CLOUD_PROJECT",
    "GCP_PROJECT",
    "DEPLOYMENT",
    "ENVIRONMENT",
    "ENV",
    "STAGE",
    "REGION",
    "HOSTNAME",
    "USER",
    "HOME",
    "GOVERSION",
    "GO_VERSION",
    "SERVICE_NAME",
    "SERVICE_VERSION",
    "APP_NAME",
    "APP_VERSION",
];

pub type ProvenanceOption = Arc<dyn Fn(&mut Provenance) + Send + Sync>;

pub fn capture_process_provenance(
    service_name: impl Into<String>,
    service_version: impl Into<String>,
    opts: impl IntoIterator<Item = ProvenanceOption>,
) -> Provenance {
    let mut p = Provenance {
        service_name: service_name.into(),
        service_version: service_version.into(),
        service_instance_id: uuid::Uuid::new_v4().to_string(),
        process_pid: std::process::id() as i64,
        process_owner: whoami::username(),
        host_name: whoami::fallible::hostname().unwrap_or_default(),
        host_arch: normalize_arch(std::env::consts::ARCH),
        captured_at: now_ms(),
        ..Provenance::default()
    };

    for opt in opts {
        opt(&mut p);
    }

    p
}

pub fn new_provenance(
    base: Option<&Provenance>,
    opts: impl IntoIterator<Item = ProvenanceOption>,
) -> Provenance {
    let mut p = if let Some(base) = base {
        let mut cloned = base.clone();
        if let Some(env) = &base.env_vars {
            cloned.env_vars = Some(env.clone());
        }
        cloned
    } else {
        Provenance::default()
    };

    p.captured_at = now_ms();
    for opt in opts {
        opt(&mut p);
    }
    p
}

pub fn with_parent_context(parent_id: u64, root_id: u64) -> ProvenanceOption {
    Arc::new(move |p| {
        p.parent_context_id = Some(parent_id);
        p.root_context_id = Some(if root_id == 0 { parent_id } else { root_id });
    })
}

pub fn with_spawn_reason(reason: impl Into<String>) -> ProvenanceOption {
    let reason = reason.into();
    Arc::new(move |p| p.spawn_reason = reason.clone())
}

pub fn with_trace_context(
    trace_id: impl Into<String>,
    span_id: impl Into<String>,
) -> ProvenanceOption {
    let trace_id = trace_id.into();
    let span_id = span_id.into();
    Arc::new(move |p| {
        p.trace_id = trace_id.clone();
        p.span_id = span_id.clone();
    })
}

pub fn with_correlation_id(id: impl Into<String>) -> ProvenanceOption {
    let id = id.into();
    Arc::new(move |p| p.correlation_id = id.clone())
}

pub fn with_on_behalf_of(
    user_id: impl Into<String>,
    source: impl Into<String>,
    email: impl Into<String>,
) -> ProvenanceOption {
    let user_id = user_id.into();
    let source = source.into();
    let email = email.into();
    Arc::new(move |p| {
        p.on_behalf_of = user_id.clone();
        p.on_behalf_of_source = source.clone();
        p.on_behalf_of_email = email.clone();
    })
}

pub fn with_writer_identity(
    method: impl Into<String>,
    subject: impl Into<String>,
    issuer: impl Into<String>,
) -> ProvenanceOption {
    let method = method.into();
    let subject = subject.into();
    let issuer = issuer.into();
    Arc::new(move |p| {
        p.writer_method = method.clone();
        p.writer_subject = subject.clone();
        p.writer_issuer = issuer.clone();
    })
}

pub fn with_env_vars(allowlist: Option<Vec<String>>) -> ProvenanceOption {
    let allowlist =
        allowlist.unwrap_or_else(|| DefaultEnvAllowlist.iter().map(|s| s.to_string()).collect());
    Arc::new(move |p| {
        let env = capture_env_vars(&allowlist);
        p.env_vars = if env.is_empty() { None } else { Some(env) };
    })
}

pub fn with_sdk(name: impl Into<String>, version: impl Into<String>) -> ProvenanceOption {
    let name = name.into();
    let version = version.into();
    Arc::new(move |p| {
        p.sdk_name = name.clone();
        p.sdk_version = version.clone();
    })
}

pub fn with_service(
    name: impl Into<String>,
    version: impl Into<String>,
    instance_id: impl Into<String>,
) -> ProvenanceOption {
    let name = name.into();
    let version = version.into();
    let instance_id = instance_id.into();
    Arc::new(move |p| {
        p.service_name = name.clone();
        p.service_version = version.clone();
        if !instance_id.is_empty() {
            p.service_instance_id = instance_id.clone();
        }
    })
}

fn capture_env_vars(allowlist: &[String]) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for key in allowlist {
        if let Ok(val) = std::env::var(key) {
            if !val.is_empty() {
                vars.insert(key.clone(), val);
            }
        }
    }
    vars
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn normalize_arch(arch: &str) -> String {
    match arch {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        other => other.to_string(),
    }
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
