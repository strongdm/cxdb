use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::session::SessionRuntime;

#[derive(Debug, Serialize, Clone, Default)]
pub struct ExchangeArtifactRecord {
    pub exchange_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_path: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct SessionLedger {
    pub session_id: String,
    pub provider_kind: String,
    pub child_command: String,
    pub child_args: Vec<String>,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cxdb_context_id: Option<u64>,
    pub delivery_state: String,
    pub queue_depth: usize,
    pub appended_sequences: Vec<u64>,
    pub provider_request_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_delivery_error: Option<String>,
    pub artifacts_root: String,
    pub exchanges: Vec<ExchangeArtifactRecord>,
}

#[derive(Clone, Debug)]
pub struct SessionLedgerWriter {
    root: PathBuf,
    path: PathBuf,
    exchanges_dir: PathBuf,
    state: Arc<Mutex<SessionLedger>>,
}

impl SessionLedgerWriter {
    pub async fn create(session: &SessionRuntime) -> Result<Self> {
        let root = Path::new(".scratch")
            .join("cxtx")
            .join("sessions")
            .join(session.session_id());
        let exchanges_dir = root.join("exchanges");
        fs::create_dir_all(&exchanges_dir)
            .await
            .with_context(|| format!("failed to create {}", exchanges_dir.display()))?;

        let ledger = SessionLedger {
            session_id: session.session_id().to_string(),
            provider_kind: session.provider().provider_name().to_string(),
            child_command: session.session().child_command.clone(),
            child_args: session.session().child_args.clone(),
            started_at: session.session().started_at,
            ended_at: None,
            child_pid: None,
            child_exit_code: None,
            cxdb_context_id: None,
            delivery_state: "starting".to_string(),
            queue_depth: 0,
            appended_sequences: Vec::new(),
            provider_request_ids: Vec::new(),
            last_delivery_error: None,
            artifacts_root: exchanges_dir.display().to_string(),
            exchanges: Vec::new(),
        };
        let path = root.join("ledger.json");
        let writer = Self {
            root,
            path,
            exchanges_dir,
            state: Arc::new(Mutex::new(ledger)),
        };
        writer.persist().await?;
        Ok(writer)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn note_child_pid(&self, child_pid: Option<u32>) -> Result<()> {
        let mut state = self.state.lock().await;
        state.child_pid = child_pid;
        drop(state);
        self.persist().await
    }

    pub async fn note_context_created(&self, context_id: u64) -> Result<()> {
        let mut state = self.state.lock().await;
        state.cxdb_context_id = Some(context_id);
        drop(state);
        self.persist().await
    }

    pub async fn note_append_sequence(&self, sequence: u64) -> Result<()> {
        let mut state = self.state.lock().await;
        if !state.appended_sequences.contains(&sequence) {
            state.appended_sequences.push(sequence);
        }
        drop(state);
        self.persist().await
    }

    pub async fn note_delivery_state(
        &self,
        delivery_state: impl Into<String>,
        queue_depth: usize,
        error: Option<String>,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        state.delivery_state = delivery_state.into();
        state.queue_depth = queue_depth;
        state.last_delivery_error = error;
        drop(state);
        self.persist().await
    }

    pub async fn note_request_id(&self, request_id: String) -> Result<()> {
        let mut state = self.state.lock().await;
        if !state.provider_request_ids.contains(&request_id) {
            state.provider_request_ids.push(request_id);
        }
        drop(state);
        self.persist().await
    }

    pub async fn note_child_exit(&self, exit_code: i32) -> Result<()> {
        let mut state = self.state.lock().await;
        state.child_exit_code = Some(exit_code);
        state.ended_at = Some(Utc::now());
        drop(state);
        self.persist().await
    }

    pub async fn record_request(
        &self,
        exchange_id: &str,
        endpoint: &str,
        model: Option<&str>,
        content_type: Option<&str>,
        body: &[u8],
        parsed_json: Option<&Value>,
    ) -> Result<String> {
        let exchange_dir = self.exchange_dir(exchange_id).await?;
        let path =
            write_body_artifact(&exchange_dir, "request", content_type, body, parsed_json).await?;
        self.update_exchange(exchange_id, |record| {
            record.endpoint = Some(endpoint.to_string());
            record.model = model.map(|value| value.to_string());
            record.request_path = Some(path.display().to_string());
        })
        .await?;
        Ok(path.display().to_string())
    }

    pub async fn record_response(
        &self,
        exchange_id: &str,
        status_code: u16,
        provider_request_id: Option<&str>,
        content_type: Option<&str>,
        body: &[u8],
        parsed_json: Option<&Value>,
    ) -> Result<String> {
        let exchange_dir = self.exchange_dir(exchange_id).await?;
        let path =
            write_body_artifact(&exchange_dir, "response", content_type, body, parsed_json).await?;
        self.update_exchange(exchange_id, |record| {
            record.status_code = Some(status_code);
            record.provider_request_id = provider_request_id.map(|value| value.to_string());
            record.response_path = Some(path.display().to_string());
        })
        .await?;
        if let Some(request_id) = provider_request_id {
            self.note_request_id(request_id.to_string()).await?;
        }
        Ok(path.display().to_string())
    }

    pub async fn append_stream_frame(&self, exchange_id: &str, raw_frame: &str) -> Result<String> {
        let exchange_dir = self.exchange_dir(exchange_id).await?;
        let path = exchange_dir.join("stream.ndjson");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .with_context(|| format!("failed to open {}", path.display()))?;
        let payload = serde_json::to_vec(&serde_json::json!({ "frame": raw_frame }))
            .context("failed to encode stream frame")?;
        file.write_all(&payload)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        file.write_all(b"\n")
            .await
            .with_context(|| format!("failed to write newline to {}", path.display()))?;
        file.flush().await.ok();
        self.update_exchange(exchange_id, |record| {
            record.stream_path = Some(path.display().to_string());
        })
        .await?;
        Ok(path.display().to_string())
    }

    pub async fn finalize(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.ended_at.get_or_insert_with(Utc::now);
        drop(state);
        self.persist().await
    }

    async fn update_exchange(
        &self,
        exchange_id: &str,
        mut update: impl FnMut(&mut ExchangeArtifactRecord),
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        let record = if let Some(record) = state
            .exchanges
            .iter_mut()
            .find(|record| record.exchange_id == exchange_id)
        {
            record
        } else {
            state.exchanges.push(ExchangeArtifactRecord {
                exchange_id: exchange_id.to_string(),
                ..ExchangeArtifactRecord::default()
            });
            state
                .exchanges
                .last_mut()
                .expect("exchange record inserted")
        };
        update(record);
        drop(state);
        self.persist().await
    }

    async fn exchange_dir(&self, exchange_id: &str) -> Result<PathBuf> {
        let path = self.exchanges_dir.join(exchange_id);
        fs::create_dir_all(&path)
            .await
            .with_context(|| format!("failed to create {}", path.display()))?;
        Ok(path)
    }

    async fn persist(&self) -> Result<()> {
        let state = self.state.lock().await.clone();
        let payload =
            serde_json::to_vec_pretty(&state).context("failed to serialize session ledger")?;
        fs::create_dir_all(&self.root)
            .await
            .with_context(|| format!("failed to create {}", self.root.display()))?;
        fs::write(&self.path, payload)
            .await
            .with_context(|| format!("failed to write {}", self.path.display()))
    }
}

async fn write_body_artifact(
    exchange_dir: &Path,
    stem: &str,
    content_type: Option<&str>,
    body: &[u8],
    parsed_json: Option<&Value>,
) -> Result<PathBuf> {
    let path = if let Some(json) = parsed_json {
        let path = exchange_dir.join(format!("{stem}.json"));
        let payload = serde_json::to_vec_pretty(json).context("failed to encode json artifact")?;
        fs::write(&path, payload)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        path
    } else if content_type
        .map(|value| value.starts_with("text/") || value.contains("event-stream"))
        .unwrap_or(false)
    {
        let path = exchange_dir.join(format!("{stem}.txt"));
        fs::write(&path, body)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        path
    } else {
        let path = exchange_dir.join(format!("{stem}.bin"));
        fs::write(&path, body)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
        path
    };
    Ok(path)
}
