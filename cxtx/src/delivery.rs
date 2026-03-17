use anyhow::{anyhow, Result};
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
    Enqueue(QueueItem),
    Shutdown(oneshot::Sender<()>),
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
        self.tx
            .send(WorkerMessage::Enqueue(QueueItem::CreateContext))
            .await
            .map_err(|_| anyhow!("delivery worker is no longer running"))
    }

    pub async fn enqueue_turn(&self, turn: TurnEnvelope) -> Result<()> {
        self.tx
            .send(WorkerMessage::Enqueue(QueueItem::Append(turn)))
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
    queue: VecDeque<QueueItem>,
    context_id: Option<u64>,
    degraded: bool,
    retry_delay: Duration,
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

            let Some(item) = self.queue.front().cloned() else {
                continue;
            };

            match self.process_item(item.clone()).await {
                Ok(()) => {
                    self.queue.pop_front();
                    self.retry_delay = INITIAL_RETRY_DELAY;

                    if self.degraded && self.queue.is_empty() && !self.recovery_turn_enqueued {
                        self.recovery_turn_enqueued = true;
                        self.queue
                            .push_back(QueueItem::Append(self.session.ingest_recovered_turn(0)));
                    } else if self.degraded
                        && self.recovery_turn_enqueued
                        && matches!(item, QueueItem::Append(_))
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
            WorkerMessage::Enqueue(item) => {
                self.queue.push_back(item);
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

    async fn process_item(&mut self, item: QueueItem) -> std::result::Result<(), String> {
        match item {
            QueueItem::CreateContext => match self.client.create_context().await {
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
                match self.client.append_turn(context_id, &turn.item).await {
                    Ok(_) => {
                        self.ledger.note_append_sequence(turn.ordinal).await.ok();
                        Ok(())
                    }
                    Err(err) => Err(error_string(err)),
                }
            }
        }
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
        self.queue.push_back(QueueItem::Append(
            self.session.ingest_degraded_turn(self.queue.len(), error),
        ));
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
