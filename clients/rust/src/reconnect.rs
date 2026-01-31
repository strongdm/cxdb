// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

#![allow(clippy::type_complexity)]

use std::cmp;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, select, Receiver, Sender};

use crate::client::{dial, dial_tls, Client, ClientOption, RequestContext};
use crate::error::{Error, Result};

pub const DEFAULT_MAX_RETRIES: usize = 5;
pub const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(100);
pub const DEFAULT_MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
pub const DEFAULT_QUEUE_SIZE: usize = 10_000;

pub type DialFunc = Arc<dyn Fn() -> Result<Client> + Send + Sync>;

pub type ReconnectOption = Arc<dyn Fn(&mut ReconnectConfig) + Send + Sync>;

#[derive(Clone)]
pub struct ReconnectConfig {
    pub max_retries: usize,
    pub retry_delay: Duration,
    pub max_retry_delay: Duration,
    pub queue_size: usize,
    pub on_reconnect: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    pub dial_func: Option<DialFunc>,
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            retry_delay: DEFAULT_RETRY_DELAY,
            max_retry_delay: DEFAULT_MAX_RETRY_DELAY,
            queue_size: DEFAULT_QUEUE_SIZE,
            on_reconnect: None,
            dial_func: None,
        }
    }
}

pub fn with_max_retries(n: usize) -> ReconnectOption {
    Arc::new(move |cfg| cfg.max_retries = n)
}

pub fn with_retry_delay(delay: Duration) -> ReconnectOption {
    Arc::new(move |cfg| cfg.retry_delay = delay)
}

pub fn with_max_retry_delay(delay: Duration) -> ReconnectOption {
    Arc::new(move |cfg| cfg.max_retry_delay = delay)
}

pub fn with_queue_size(size: usize) -> ReconnectOption {
    Arc::new(move |cfg| cfg.queue_size = size)
}

pub fn with_on_reconnect<F>(f: F) -> ReconnectOption
where
    F: Fn(u64) + Send + Sync + 'static,
{
    let f = Arc::new(f);
    Arc::new(move |cfg| cfg.on_reconnect = Some(f.clone()))
}

#[cfg(test)]
pub(crate) fn with_dial_func(func: DialFunc) -> ReconnectOption {
    Arc::new(move |cfg| cfg.dial_func = Some(func.clone()))
}

pub struct ReconnectingClient {
    inner: Arc<Inner>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

struct Inner {
    client: Mutex<Option<Arc<Client>>>,
    dial_func: DialFunc,

    max_retries: usize,
    retry_delay: Duration,
    max_retry_delay: Duration,
    on_reconnect: Option<Arc<dyn Fn(u64) + Send + Sync>>,

    queue_tx: Sender<QueuedRequest>,
    queue_rx: Receiver<QueuedRequest>,
    shutdown_tx: Sender<()>,
    shutdown_rx: Receiver<()>,
    closed: AtomicBool,
}

struct QueuedRequest {
    ctx: RequestContext,
    op: Arc<dyn Fn(&Client) -> Result<()> + Send + Sync>,
    result_tx: Sender<Result<()>>,
}

pub fn dial_reconnecting(
    addr: &str,
    reconnect_opts: impl IntoIterator<Item = ReconnectOption>,
    opts: impl IntoIterator<Item = ClientOption>,
) -> Result<ReconnectingClient> {
    dial_reconnecting_inner(addr, false, reconnect_opts, opts)
}

pub fn dial_tls_reconnecting(
    addr: &str,
    reconnect_opts: impl IntoIterator<Item = ReconnectOption>,
    opts: impl IntoIterator<Item = ClientOption>,
) -> Result<ReconnectingClient> {
    dial_reconnecting_inner(addr, true, reconnect_opts, opts)
}

fn dial_reconnecting_inner(
    addr: &str,
    use_tls: bool,
    reconnect_opts: impl IntoIterator<Item = ReconnectOption>,
    opts: impl IntoIterator<Item = ClientOption>,
) -> Result<ReconnectingClient> {
    let mut cfg = ReconnectConfig::default();
    for opt in reconnect_opts {
        opt(&mut cfg);
    }

    let options: Vec<ClientOption> = opts.into_iter().collect();

    let dial_func: DialFunc = cfg.dial_func.clone().unwrap_or_else(|| {
        let addr = addr.to_string();
        let opts = options.clone();
        Arc::new(move || {
            if use_tls {
                dial_tls(&addr, opts.clone())
            } else {
                dial(&addr, opts.clone())
            }
        })
    });

    let (queue_tx, queue_rx) = bounded(cfg.queue_size);
    let (shutdown_tx, shutdown_rx) = bounded(1);

    let client = Arc::new(dial_func()?);

    let inner = Arc::new(Inner {
        client: Mutex::new(Some(client)),
        dial_func: dial_func.clone(),
        max_retries: cfg.max_retries,
        retry_delay: cfg.retry_delay,
        max_retry_delay: cfg.max_retry_delay,
        on_reconnect: cfg.on_reconnect.clone(),
        queue_tx,
        queue_rx: queue_rx.clone(),
        shutdown_tx: shutdown_tx.clone(),
        shutdown_rx: shutdown_rx.clone(),
        closed: AtomicBool::new(false),
    });

    let worker_inner = inner.clone();
    let handle = thread::spawn(move || sender_loop(worker_inner));

    Ok(ReconnectingClient {
        inner,
        worker: Mutex::new(Some(handle)),
    })
}

impl ReconnectingClient {
    pub fn close(&self) -> Result<()> {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let _ = self.inner.shutdown_tx.send(());
        if let Some(handle) = self.worker.lock().ok().and_then(|mut h| h.take()) {
            let _ = handle.join();
        }
        if let Some(client) = self.inner.client.lock().ok().and_then(|mut c| c.take()) {
            client.close()?;
        }
        Ok(())
    }

    pub fn session_id(&self) -> u64 {
        self.inner
            .client
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|client| client.session_id()))
            .unwrap_or(0)
    }

    pub fn client_tag(&self) -> String {
        self.inner
            .client
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|client| client.client_tag().to_string()))
            .unwrap_or_default()
    }

    pub fn queue_length(&self) -> usize {
        self.inner.queue_rx.len()
    }

    pub fn create_context(
        &self,
        ctx: &RequestContext,
        base_turn_id: u64,
    ) -> Result<crate::context::ContextHead> {
        let result = Arc::new(Mutex::new(None));
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "CreateContext", move |client| {
            let head = client.create_context(&ctx_clone, base_turn_id)?;
            *result_clone.lock().unwrap() = Some(head);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn fork_context(
        &self,
        ctx: &RequestContext,
        base_turn_id: u64,
    ) -> Result<crate::context::ContextHead> {
        let result = Arc::new(Mutex::new(None));
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "ForkContext", move |client| {
            let head = client.fork_context(&ctx_clone, base_turn_id)?;
            *result_clone.lock().unwrap() = Some(head);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn get_head(
        &self,
        ctx: &RequestContext,
        context_id: u64,
    ) -> Result<crate::context::ContextHead> {
        let result = Arc::new(Mutex::new(None));
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "GetHead", move |client| {
            let head = client.get_head(&ctx_clone, context_id)?;
            *result_clone.lock().unwrap() = Some(head);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn append_turn(
        &self,
        ctx: &RequestContext,
        req: &crate::turn::AppendRequest,
    ) -> Result<crate::turn::AppendResult> {
        let result = Arc::new(Mutex::new(None));
        let req = req.clone();
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "AppendTurn", move |client| {
            let res = client.append_turn(&ctx_clone, &req)?;
            *result_clone.lock().unwrap() = Some(res);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn get_last(
        &self,
        ctx: &RequestContext,
        context_id: u64,
        opts: crate::turn::GetLastOptions,
    ) -> Result<Vec<crate::turn::TurnRecord>> {
        let result = Arc::new(Mutex::new(None));
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "GetLast", move |client| {
            let res = client.get_last(&ctx_clone, context_id, opts)?;
            *result_clone.lock().unwrap() = Some(res);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn attach_fs(
        &self,
        ctx: &RequestContext,
        req: &crate::fs::AttachFsRequest,
    ) -> Result<crate::fs::AttachFsResult> {
        let result = Arc::new(Mutex::new(None));
        let req = req.clone();
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "AttachFs", move |client| {
            let res = client.attach_fs(&ctx_clone, &req)?;
            *result_clone.lock().unwrap() = Some(res);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn put_blob(
        &self,
        ctx: &RequestContext,
        req: &crate::fs::PutBlobRequest,
    ) -> Result<crate::fs::PutBlobResult> {
        let result = Arc::new(Mutex::new(None));
        let req = req.clone();
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "PutBlob", move |client| {
            let res = client.put_blob(&ctx_clone, &req)?;
            *result_clone.lock().unwrap() = Some(res);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn put_blob_if_absent(
        &self,
        ctx: &RequestContext,
        data: Vec<u8>,
    ) -> Result<([u8; 32], bool)> {
        let result = Arc::new(Mutex::new(None));
        let ctx_clone = ctx.clone();
        let data = Arc::new(data);
        let result_clone = result.clone();
        self.enqueue(ctx, "PutBlobIfAbsent", move |client| {
            let res = client.put_blob_if_absent(&ctx_clone, (*data).clone())?;
            *result_clone.lock().unwrap() = Some(res);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    pub fn append_turn_with_fs(
        &self,
        ctx: &RequestContext,
        req: &crate::turn::AppendRequest,
        fs_root_hash: Option<[u8; 32]>,
    ) -> Result<crate::turn::AppendResult> {
        let result = Arc::new(Mutex::new(None));
        let req = req.clone();
        let ctx_clone = ctx.clone();
        let result_clone = result.clone();
        self.enqueue(ctx, "AppendTurnWithFs", move |client| {
            let res = client.append_turn_with_fs(&ctx_clone, &req, fs_root_hash)?;
            *result_clone.lock().unwrap() = Some(res);
            Ok(())
        })?;
        let value = result.lock().unwrap().take().unwrap();
        Ok(value)
    }

    fn enqueue<F>(&self, ctx: &RequestContext, _desc: &str, op: F) -> Result<()>
    where
        F: Fn(&Client) -> Result<()> + Send + Sync + 'static,
    {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(Error::ClientClosed);
        }
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(deadline) = ctx.deadline() {
            if deadline <= Instant::now() {
                return Err(Error::Timeout);
            }
        }

        let (result_tx, result_rx) = bounded(1);
        let req = QueuedRequest {
            ctx: ctx.clone(),
            op: Arc::new(op),
            result_tx,
        };

        match self.inner.queue_tx.try_send(req) {
            Ok(_) => {}
            Err(_) => return Err(Error::QueueFull),
        }

        wait_for_result(&result_rx, ctx)
    }
}

fn sender_loop(inner: Arc<Inner>) {
    loop {
        select! {
            recv(inner.shutdown_rx) -> _ => {
                drain_queue(&inner, Error::ClientClosed);
                break;
            }
            recv(inner.queue_rx) -> msg => {
                let req = match msg {
                    Ok(req) => req,
                    Err(_) => break,
                };
                process_request(&inner, req);
            }
        }
    }
}

fn process_request(inner: &Arc<Inner>, req: QueuedRequest) {
    if req.ctx.is_cancelled() {
        let _ = req.result_tx.send(Err(Error::Cancelled));
        return;
    }
    if let Some(deadline) = req.ctx.deadline() {
        if deadline <= Instant::now() {
            let _ = req.result_tx.send(Err(Error::Timeout));
            return;
        }
    }

    let client = match inner.client.lock() {
        Ok(guard) => guard.as_ref().cloned(),
        Err(_) => None,
    };
    let client = match client {
        Some(client) => client,
        None => {
            let _ = req.result_tx.send(Err(Error::ClientClosed));
            return;
        }
    };

    let op = req.op.clone();
    let mut err = (op)(&client);
    if let Err(ref e) = err {
        if is_connection_error(e) {
            if let Err(reconn_err) = reconnect(inner, &req.ctx) {
                err = Err(reconn_err);
            } else {
                let client = inner.client.lock().ok().and_then(|c| c.as_ref().cloned());
                if let Some(client) = client {
                    err = (op)(&client);
                }
            }
        }
    }

    let _ = req.result_tx.send(err);
}

fn reconnect(inner: &Arc<Inner>, ctx: &RequestContext) -> Result<()> {
    let mut delay = inner.retry_delay;
    let mut last_err: Option<Error> = None;

    for attempt in 1..=inner.max_retries {
        if attempt > 1 {
            sleep_with_cancel(delay, ctx, inner)?;
            delay = cmp::min(delay * 2, inner.max_retry_delay);
        }

        if inner.closed.load(Ordering::SeqCst) {
            return Err(Error::ClientClosed);
        }

        if let Ok(mut guard) = inner.client.lock() {
            if let Some(client) = guard.take() {
                let _ = client.close();
            }
        }

        match (inner.dial_func)() {
            Ok(client) => {
                let client = Arc::new(client);
                let session_id = client.session_id();
                if let Ok(mut guard) = inner.client.lock() {
                    *guard = Some(client);
                }
                if let Some(cb) = &inner.on_reconnect {
                    cb(session_id);
                }
                return Ok(());
            }
            Err(err) => {
                last_err = Some(err);
            }
        }
    }

    Err(last_err.unwrap_or(Error::ClientClosed))
}

fn sleep_with_cancel(duration: Duration, ctx: &RequestContext, inner: &Arc<Inner>) -> Result<()> {
    let start = Instant::now();
    let step = Duration::from_millis(50);
    while start.elapsed() < duration {
        if inner.closed.load(Ordering::SeqCst) {
            return Err(Error::ClientClosed);
        }
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(deadline) = ctx.deadline() {
            if deadline <= Instant::now() {
                return Err(Error::Timeout);
            }
        }
        let remaining = duration.saturating_sub(start.elapsed());
        let sleep_for = if remaining < step { remaining } else { step };
        thread::sleep(sleep_for);
    }
    Ok(())
}

fn drain_queue(inner: &Arc<Inner>, _err: Error) {
    while let Ok(req) = inner.queue_rx.try_recv() {
        let _ = req.result_tx.send(Err(Error::ClientClosed));
    }
}

fn wait_for_result(result_rx: &Receiver<Result<()>>, ctx: &RequestContext) -> Result<()> {
    let deadline = ctx.deadline();
    loop {
        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let timeout = match deadline {
            Some(deadline) => {
                if deadline <= Instant::now() {
                    return Err(Error::Timeout);
                }
                Some(deadline.saturating_duration_since(Instant::now()))
            }
            None => None,
        };

        let recv_result = if let Some(timeout) = timeout {
            result_rx.recv_timeout(timeout)
        } else {
            result_rx
                .recv()
                .map_err(|_| crossbeam_channel::RecvTimeoutError::Disconnected)
        };

        match recv_result {
            Ok(result) => return result,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if ctx.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                if let Some(deadline) = deadline {
                    if deadline <= Instant::now() {
                        return Err(Error::Timeout);
                    }
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(Error::ClientClosed);
            }
        }
    }
}

pub fn is_connection_error(err: &Error) -> bool {
    match err {
        Error::ClientClosed => false,
        Error::Server(_) => false,
        Error::Timeout => false,
        Error::Cancelled => false,
        Error::QueueFull => false,
        Error::Io(io_err) => match io_err.kind() {
            std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::NotConnected => true,
            _ => contains_connection_pattern(&io_err.to_string()),
        },
        Error::Tls(msg) => contains_connection_pattern(msg),
        Error::InvalidResponse(msg) => contains_connection_pattern(msg),
        _ => contains_connection_pattern(&err.to_string()),
    }
}

#[allow(non_snake_case)]
pub fn IsConnectionError(err: &Error) -> bool {
    is_connection_error(err)
}

fn contains_connection_pattern(msg: &str) -> bool {
    let msg = msg.to_lowercase();
    let patterns = [
        "connection reset",
        "connection refused",
        "broken pipe",
        "use of closed network connection",
        "network is unreachable",
        "no route to host",
        "connection timed out",
        "i/o timeout",
    ];
    patterns.iter().any(|p| msg.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_frame, write_frame, MSG_HELLO};
    use byteorder::{LittleEndian, WriteBytesExt};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
        mpsc, Arc, Barrier,
    };
    use std::thread;
    use std::time::Duration;

    fn start_hello_server() -> (String, mpsc::Sender<()>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (stop_tx, stop_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let frame = read_frame(&mut stream).unwrap();
            assert_eq!(frame.header.msg_type, MSG_HELLO);
            let mut resp = Vec::new();
            resp.write_u64::<LittleEndian>(1).unwrap();
            resp.write_u16::<LittleEndian>(1).unwrap();
            write_frame(&mut stream, MSG_HELLO, 0, frame.header.req_id, &resp).unwrap();
            let _ = stop_rx.recv();
        });
        (addr.to_string(), stop_tx, handle)
    }

    #[test]
    fn is_connection_error_matches_basic_cases() {
        assert!(!is_connection_error(&Error::ClientClosed));
        assert!(!is_connection_error(&Error::Server(
            crate::error::ServerError {
                code: 404,
                detail: "not found".into()
            }
        )));
        assert!(is_connection_error(&Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset"
        ))));
        assert!(is_connection_error(&Error::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "timeout"
        ))));
        assert!(is_connection_error(&Error::Tls(
            "connection refused".into()
        )));
        assert!(is_connection_error(&Error::Io(std::io::Error::other(
            "use of closed network connection"
        ))));
    }

    #[test]
    fn queue_full_returns_error() {
        let (addr, stop_tx, handle) = start_hello_server();
        let dial_func: DialFunc = Arc::new({
            let addr = addr.clone();
            move || dial(&addr, Vec::<ClientOption>::new())
        });

        let client = Arc::new(
            dial_reconnecting_inner(
                &addr,
                false,
                vec![with_queue_size(1), with_dial_func(dial_func)],
                Vec::<ClientOption>::new(),
            )
            .unwrap(),
        );

        let start_barrier = Arc::new(Barrier::new(2));
        let release_barrier = Arc::new(Barrier::new(2));

        let client_clone = client.clone();
        let start_barrier_clone = start_barrier.clone();
        let release_barrier_clone = release_barrier.clone();
        let first = thread::spawn(move || {
            client_clone
                .enqueue(&RequestContext::background(), "block", move |_| {
                    start_barrier_clone.wait();
                    release_barrier_clone.wait();
                    Ok(())
                })
                .unwrap();
        });

        start_barrier.wait();

        let (queued_tx, queued_rx) = bounded(1);
        let queued_req = QueuedRequest {
            ctx: RequestContext::background(),
            op: Arc::new(|_| Ok(())),
            result_tx: queued_tx,
        };
        client.inner.queue_tx.try_send(queued_req).unwrap();

        // Third enqueue should fail because queue size is 1 and queued_req is waiting.
        let err = client
            .enqueue(&RequestContext::background(), "overflow", |_| Ok(()))
            .unwrap_err();
        assert!(matches!(err, Error::QueueFull));

        release_barrier.wait();
        first.join().unwrap();
        let _ = queued_rx.recv();
        client.close().unwrap();
        let _ = stop_tx.send(());
        handle.join().unwrap();
    }

    #[test]
    fn queue_length_reports_pending_requests() {
        let (addr, stop_tx, handle) = start_hello_server();
        let dial_func: DialFunc = Arc::new({
            let addr = addr.clone();
            move || dial(&addr, Vec::<ClientOption>::new())
        });
        let client = Arc::new(
            dial_reconnecting_inner(
                &addr,
                false,
                vec![with_dial_func(dial_func)],
                Vec::<ClientOption>::new(),
            )
            .unwrap(),
        );

        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));

        let client_clone = client.clone();
        let started_clone = started.clone();
        let release_clone = release.clone();
        let first = thread::spawn(move || {
            client_clone
                .enqueue(&RequestContext::background(), "block", move |_| {
                    started_clone.wait();
                    release_clone.wait();
                    Ok(())
                })
                .unwrap();
        });

        started.wait();

        let (queued_tx, queued_rx) = bounded(1);
        let queued_req = QueuedRequest {
            ctx: RequestContext::background(),
            op: Arc::new(|_| Ok(())),
            result_tx: queued_tx,
        };
        client.inner.queue_tx.try_send(queued_req).unwrap();
        thread::sleep(Duration::from_millis(10));
        assert_eq!(client.queue_length(), 1);

        release.wait();
        first.join().unwrap();
        let _ = queued_rx.recv();
        client.close().unwrap();
        let _ = stop_tx.send(());
        handle.join().unwrap();
    }

    #[test]
    fn concurrent_enqueues_succeed() {
        let (addr, stop_tx, handle) = start_hello_server();
        let dial_func: DialFunc = Arc::new({
            let addr = addr.clone();
            move || dial(&addr, Vec::<ClientOption>::new())
        });
        let client = Arc::new(
            dial_reconnecting_inner(
                &addr,
                false,
                vec![with_dial_func(dial_func)],
                Vec::<ClientOption>::new(),
            )
            .unwrap(),
        );

        let success_count = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..5 {
            let client_clone = client.clone();
            let success_count = success_count.clone();
            handles.push(thread::spawn(move || {
                client_clone
                    .enqueue(&RequestContext::background(), "noop", |_| Ok(()))
                    .unwrap();
                success_count.fetch_add(1, AtomicOrdering::SeqCst);
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(success_count.load(AtomicOrdering::SeqCst), 5);
        client.close().unwrap();
        let _ = stop_tx.send(());
        handle.join().unwrap();
    }

    #[test]
    fn enqueue_after_close_returns_client_closed() {
        let (addr, stop_tx, handle) = start_hello_server();
        let dial_func: DialFunc = Arc::new({
            let addr = addr.clone();
            move || dial(&addr, Vec::<ClientOption>::new())
        });
        let client = Arc::new(
            dial_reconnecting_inner(
                &addr,
                false,
                vec![with_dial_func(dial_func)],
                Vec::<ClientOption>::new(),
            )
            .unwrap(),
        );
        client.close().unwrap();
        let err = client
            .enqueue(&RequestContext::background(), "closed", |_| Ok(()))
            .unwrap_err();
        assert!(matches!(err, Error::ClientClosed));
        let _ = stop_tx.send(());
        handle.join().unwrap();
    }

    #[test]
    fn cancelled_context_stops_reconnect() {
        let (addr, stop_tx, handle) = start_hello_server();
        let dial_count = Arc::new(AtomicUsize::new(0));
        let dial_func: DialFunc = Arc::new({
            let addr = addr.clone();
            let dial_count = dial_count.clone();
            move || {
                let attempt = dial_count.fetch_add(1, AtomicOrdering::SeqCst);
                if attempt == 0 {
                    dial(&addr, Vec::<ClientOption>::new())
                } else {
                    Err(Error::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionRefused,
                        "refused",
                    )))
                }
            }
        });

        let client = Arc::new(
            dial_reconnecting_inner(
                &addr,
                false,
                vec![
                    with_dial_func(dial_func),
                    with_max_retries(3),
                    with_retry_delay(Duration::from_millis(50)),
                ],
                Vec::<ClientOption>::new(),
            )
            .unwrap(),
        );

        let (ctx, handle_cancel) = RequestContext::cancellable();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            handle_cancel.cancel();
        });

        let err = client
            .enqueue(&ctx, "force-reconnect", |_| {
                Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "reset",
                )))
            })
            .unwrap_err();
        assert!(matches!(err, Error::Cancelled));

        cancel_thread.join().unwrap();
        client.close().unwrap();
        let _ = stop_tx.send(());
        handle.join().unwrap();
    }

    #[test]
    fn queue_full_returns_error_legacy() {
        let dial_func: DialFunc = Arc::new(|| Err(Error::ClientClosed));
        let result = dial_reconnecting_inner(
            "127.0.0.1:0",
            false,
            vec![with_dial_func(dial_func)],
            Vec::<ClientOption>::new(),
        );
        assert!(matches!(result, Err(Error::ClientClosed)));
    }
}
