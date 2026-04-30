use anyhow::{anyhow, Result};
use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, Context as OtelContext, KeyValue};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, Instant};
use url::Url;

use crate::cxdb_http::{CxdbError, CxdbHttpClient};
use crate::ledger::SessionLedgerWriter;
use crate::session::SessionRuntime;
use crate::turns::TurnEnvelope;

const INITIAL_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub struct DeliveryHandle {
    tx: mpsc::Sender<WorkerMessage>,
}

#[derive(Debug)]
enum WorkerMessage {
    Enqueue(QueuedWork),
    Shutdown(oneshot::Sender<()>),
}

/// Sprint 018 P3.3: every queue entry pairs the payload with the
/// originating OTEL `Context` so retries + delayed re-attempts land as
/// children of the originating request rather than orphan traces.
///
/// CRITICAL invariant (Design Decision 9): `parent_context` lives
/// alongside the payload — it MUST NEVER be embedded inside
/// `TurnEnvelope` / `HistoryItem` / any semantic content, so that
/// `cxtx/src/session.rs::normalize_history_item` keeps dedup
/// content-addressable.
#[derive(Debug, Clone)]
struct QueuedWork {
    item: QueueItem,
    parent_context: OtelContext,
}

#[derive(Debug, Clone)]
enum QueueItem {
    CreateContext,
    Append(TurnEnvelope),
}

impl DeliveryHandle {
    pub async fn start(
        base_url: Url,
        session: SessionRuntime,
        ledger: SessionLedgerWriter,
        client_tag: String,
    ) -> Result<Self> {
        let client = CxdbHttpClient::new(base_url, client_tag)?;
        let (tx, rx) = mpsc::channel(1024);
        let worker = DeliveryWorker::new(client, session, ledger, rx);
        tokio::spawn(worker.run());
        Ok(Self { tx })
    }

    pub async fn enqueue_create_context(&self) -> Result<()> {
        // P3.3: capture the originating OTEL context at enqueue time.
        let parent_context = OtelContext::current();
        self.tx
            .send(WorkerMessage::Enqueue(QueuedWork {
                item: QueueItem::CreateContext,
                parent_context,
            }))
            .await
            .map_err(|_| anyhow!("delivery worker is no longer running"))
    }

    pub async fn enqueue_turn(&self, turn: TurnEnvelope) -> Result<()> {
        let parent_context = OtelContext::current();
        self.tx
            .send(WorkerMessage::Enqueue(QueuedWork {
                item: QueueItem::Append(turn),
                parent_context,
            }))
            .await
            .map_err(|_| anyhow!("delivery worker is no longer running"))
    }

    pub async fn shutdown(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(WorkerMessage::Shutdown(tx))
            .await
            .map_err(|_| anyhow!("delivery worker is no longer running"))?;
        rx.await
            .map_err(|_| anyhow!("delivery worker shutdown acknowledgement dropped"))
    }
}

struct DeliveryWorker {
    client: CxdbHttpClient,
    session: SessionRuntime,
    ledger: SessionLedgerWriter,
    queue: VecDeque<QueuedWork>,
    context_id: Option<u64>,
    degraded: bool,
    retry_delay: Duration,
    retry_count: u32,
    rx: mpsc::Receiver<WorkerMessage>,
    shutdown: Option<oneshot::Sender<()>>,
    shutdown_deadline: Option<Instant>,
    recovery_turn_enqueued: bool,
}

impl DeliveryWorker {
    fn new(
        client: CxdbHttpClient,
        session: SessionRuntime,
        ledger: SessionLedgerWriter,
        rx: mpsc::Receiver<WorkerMessage>,
    ) -> Self {
        Self {
            client,
            session,
            ledger,
            queue: VecDeque::new(),
            context_id: None,
            degraded: false,
            retry_delay: INITIAL_RETRY_DELAY,
            retry_count: 0,
            rx,
            shutdown: None,
            shutdown_deadline: None,
            recovery_turn_enqueued: false,
        }
    }

    async fn run(mut self) {
        loop {
            if self.maybe_finish_shutdown().await {
                return;
            }

            if self.queue.is_empty() {
                match self.rx.recv().await {
                    Some(message) => self.handle_message(message).await,
                    None => return,
                }
                continue;
            }

            while let Ok(message) = self.rx.try_recv() {
                self.handle_message(message).await;
            }

            let Some(work) = self.queue.front().cloned() else {
                continue;
            };

            match self.process_item(work.clone()).await {
                Ok(()) => {
                    self.queue.pop_front();
                    self.retry_delay = INITIAL_RETRY_DELAY;
                    // Reset retry counter for the next queue entry.
                    self.retry_count = 0;

                    if self.degraded && self.queue.is_empty() && !self.recovery_turn_enqueued {
                        self.recovery_turn_enqueued = true;
                        // Recovery synthetic turn — captures fresh context
                        // since the original parent is long gone.
                        self.queue.push_back(QueuedWork {
                            item: QueueItem::Append(self.session.ingest_recovered_turn(0)),
                            parent_context: OtelContext::current(),
                        });
                    } else if self.degraded
                        && self.recovery_turn_enqueued
                        && matches!(work.item, QueueItem::Append(_))
                        && self.queue.is_empty()
                    {
                        self.degraded = false;
                        self.recovery_turn_enqueued = false;
                        self.ledger
                            .note_delivery_state("healthy", 0, None)
                            .await
                            .ok();
                        eprintln!("cxtx: CXDB ingest recovered; queued turns delivered");
                    }
                }
                Err(err) => {
                    self.enter_degraded(&err).await;
                    // P3.4: bump retry count for the SAME queue entry.
                    self.retry_count = self.retry_count.saturating_add(1);
                    let deadline = self
                        .shutdown_deadline
                        .map(|deadline| deadline.saturating_duration_since(Instant::now()));
                    let sleep_for = deadline
                        .map(|remaining| remaining.min(self.retry_delay))
                        .unwrap_or(self.retry_delay);
                    sleep(sleep_for).await;
                    self.retry_delay = (self.retry_delay * 2).min(MAX_RETRY_DELAY);
                }
            }
        }
    }

    async fn handle_message(&mut self, message: WorkerMessage) {
        match message {
            WorkerMessage::Enqueue(work) => {
                self.queue.push_back(work);
                self.ledger
                    .note_delivery_state(
                        if self.degraded { "degraded" } else { "healthy" },
                        self.queue.len(),
                        None,
                    )
                    .await
                    .ok();
            }
            WorkerMessage::Shutdown(tx) => {
                self.shutdown = Some(tx);
                self.shutdown_deadline = Some(Instant::now() + SHUTDOWN_DRAIN_TIMEOUT);
            }
        }
    }

    async fn process_item(&mut self, work: QueuedWork) -> std::result::Result<(), String> {
        // P3.3 + P3.4: open a `http.client.request` client-kind span as a
        // child of the enqueue-time parent context (never attached with
        // a guard — `ContextGuard` is not `Send` and cannot cross
        // `.await` in a `tokio::spawn`'d future). Instead we thread
        // the span's `Context` explicitly through the HTTP client.
        //
        // `retry.count` starts at 0 on the first attempt and increments
        // on each subsequent enter (see `run`'s Err branch).
        let tracer = global::tracer("cxtx");
        let mut builder = tracer
            .span_builder("http.client.request")
            .with_kind(SpanKind::Client);
        let op_name = match &work.item {
            QueueItem::CreateContext => "create_context",
            QueueItem::Append(_) => "append_turn",
        };
        builder.attributes = Some(vec![
            KeyValue::new("cxtx.op", op_name.to_string()),
            KeyValue::new("retry.count", self.retry_count as i64),
        ]);
        let span = tracer.build_with_context(builder, &work.parent_context);
        // Compose the retry span into a Context we pass explicitly to
        // `*_with_context` HTTP helpers so the injected `traceparent`
        // names this span as the immediate parent.
        let retry_cx = OtelContext::current_with_span(span);

        let result = match work.item {
            QueueItem::CreateContext => match self
                .client
                .create_context_with_context(&retry_cx)
                .await
            {
                Ok(context_id) => {
                    self.context_id = Some(context_id);
                    self.ledger.note_context_created(context_id).await.ok();
                    Ok(())
                }
                Err(err) => Err(error_string(err)),
            },
            QueueItem::Append(turn) => {
                let context_id = self
                    .context_id
                    .ok_or_else(|| "context creation has not completed".to_string())?;
                match self
                    .client
                    .append_turn_with_context(context_id, &turn.item, &retry_cx)
                    .await
                {
                    Ok(_) => {
                        self.ledger.note_append_sequence(turn.ordinal).await.ok();
                        Ok(())
                    }
                    Err(err) => Err(error_string(err)),
                }
            }
        };

        // Drop retry_cx (and thus the span) here so the span ends
        // before the next retry starts a new child span.
        drop(retry_cx);
        result
    }

    async fn enter_degraded(&mut self, error: &str) {
        self.ledger
            .note_delivery_state("degraded", self.queue.len(), Some(error.to_string()))
            .await
            .ok();

        if self.degraded {
            return;
        }

        self.degraded = true;
        self.recovery_turn_enqueued = false;
        self.queue.push_back(QueuedWork {
            item: QueueItem::Append(
                self.session.ingest_degraded_turn(self.queue.len(), error),
            ),
            parent_context: OtelContext::current(),
        });
        eprintln!("cxtx: CXDB ingest unavailable, entering queued-delivery mode");
    }

    async fn maybe_finish_shutdown(&mut self) -> bool {
        if self.shutdown.is_none() {
            return false;
        }

        if let Some(deadline) = self.shutdown_deadline {
            if Instant::now() >= deadline {
                self.ledger
                    .note_delivery_state(
                        if self.degraded { "degraded" } else { "healthy" },
                        self.queue.len(),
                        Some("shutdown drain deadline reached".to_string()),
                    )
                    .await
                    .ok();
            }
        }

        if self.queue.is_empty()
            || self
                .shutdown_deadline
                .map(|deadline| Instant::now() >= deadline)
                .unwrap_or(false)
        {
            if let Some(tx) = self.shutdown.take() {
                let _ = tx.send(());
            }
            return true;
        }

        false
    }
}

fn error_string(err: CxdbError) -> String {
    match err {
        CxdbError::Retriable(err) | CxdbError::Permanent(err) => err,
    }
}
