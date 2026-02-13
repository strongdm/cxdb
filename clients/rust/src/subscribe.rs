// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::io::{BufRead, BufReader, Read};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, SendTimeoutError, Sender, TrySendError};

use crate::client::RequestContext;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub event_type: String,
    pub data: Vec<u8>,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubscribeErrorKind {
    Cancelled,
    Timeout,
    Eof,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeError {
    kind: SubscribeErrorKind,
    detail: String,
}

impl SubscribeError {
    fn cancelled() -> Self {
        Self {
            kind: SubscribeErrorKind::Cancelled,
            detail: "context canceled".to_string(),
        }
    }

    fn timeout() -> Self {
        Self {
            kind: SubscribeErrorKind::Timeout,
            detail: "context deadline exceeded".to_string(),
        }
    }

    fn eof() -> Self {
        Self {
            kind: SubscribeErrorKind::Eof,
            detail: "cxdb subscribe: stream closed".to_string(),
        }
    }

    fn other(detail: impl Into<String>) -> Self {
        Self {
            kind: SubscribeErrorKind::Other,
            detail: detail.into(),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.kind == SubscribeErrorKind::Cancelled
    }

    fn is_timeout(&self) -> bool {
        self.kind == SubscribeErrorKind::Timeout
    }

    fn is_eof(&self) -> bool {
        self.kind == SubscribeErrorKind::Eof
    }
}

impl std::fmt::Display for SubscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for SubscribeError {}

const DEFAULT_MAX_EVENT_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_EVENT_BUFFER: usize = 128;
const DEFAULT_ERROR_BUFFER: usize = 8;
const DEFAULT_RETRY_DELAY: Duration = Duration::from_millis(500);
const DEFAULT_MAX_RETRY_DELAY: Duration = Duration::from_secs(10);

struct SubscribeOptions {
    agent: ureq::Agent,
    headers: Vec<(String, String)>,
    max_event_bytes: usize,
    event_buffer: usize,
    error_buffer: usize,
    retry_delay: Duration,
    max_retry_delay: Duration,
}

impl Default for SubscribeOptions {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::new(),
            headers: Vec::new(),
            max_event_bytes: DEFAULT_MAX_EVENT_BYTES,
            event_buffer: DEFAULT_EVENT_BUFFER,
            error_buffer: DEFAULT_ERROR_BUFFER,
            retry_delay: DEFAULT_RETRY_DELAY,
            max_retry_delay: DEFAULT_MAX_RETRY_DELAY,
        }
    }
}

#[derive(Clone)]
pub struct SubscribeOption(Arc<dyn Fn(&mut SubscribeOptions) + Send + Sync>);

impl SubscribeOption {
    fn apply(&self, opts: &mut SubscribeOptions) {
        (self.0)(opts);
    }
}

pub fn with_http_client(agent: ureq::Agent) -> SubscribeOption {
    SubscribeOption(Arc::new(move |opts| opts.agent = agent.clone()))
}

pub fn with_headers<I, K, V>(headers: I) -> SubscribeOption
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    let headers: Vec<(String, String)> = headers
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
    SubscribeOption(Arc::new(move |opts| opts.headers = headers.clone()))
}

pub fn with_max_event_bytes(n: usize) -> SubscribeOption {
    SubscribeOption(Arc::new(move |opts| opts.max_event_bytes = n))
}

pub fn with_event_buffer(n: usize) -> SubscribeOption {
    SubscribeOption(Arc::new(move |opts| opts.event_buffer = n))
}

pub fn with_error_buffer(n: usize) -> SubscribeOption {
    SubscribeOption(Arc::new(move |opts| opts.error_buffer = n))
}

pub fn with_subscribe_retry_delay(delay: Duration) -> SubscribeOption {
    SubscribeOption(Arc::new(move |opts| opts.retry_delay = delay))
}

pub fn with_subscribe_max_retry_delay(delay: Duration) -> SubscribeOption {
    SubscribeOption(Arc::new(move |opts| opts.max_retry_delay = delay))
}

pub fn subscribe_events(
    ctx: &RequestContext,
    url: &str,
    opts: impl IntoIterator<Item = SubscribeOption>,
) -> (Receiver<Event>, Receiver<SubscribeError>) {
    let mut options = SubscribeOptions::default();
    for opt in opts {
        opt.apply(&mut options);
    }

    let (event_tx, event_rx) = bounded(options.event_buffer);
    let (err_tx, err_rx) = bounded(options.error_buffer);

    if url.trim().is_empty() {
        non_blocking_send(
            &err_tx,
            SubscribeError::other("cxdb subscribe: url is required"),
        );
        drop(event_tx);
        drop(err_tx);
        return (event_rx, err_rx);
    }

    let url = url.to_string();
    let ctx = ctx.clone();

    thread::spawn(move || {
        let mut retry_delay = options.retry_delay;
        loop {
            if ctx_status(&ctx).is_some() {
                return;
            }

            let result = subscribe_once(&ctx, &url, &options, &event_tx);
            if let Err(err) = result {
                if !err.is_cancelled() {
                    non_blocking_send(&err_tx, err.clone());
                }
                if err.is_cancelled() || err.is_timeout() {
                    return;
                }
            }

            if ctx_status(&ctx).is_some() {
                return;
            }

            if retry_delay <= Duration::ZERO {
                retry_delay = DEFAULT_RETRY_DELAY;
            }
            if options.max_retry_delay > Duration::ZERO && retry_delay > options.max_retry_delay {
                retry_delay = options.max_retry_delay;
            }

            if !sleep_with_cancel(&ctx, retry_delay) {
                return;
            }
            retry_delay = next_retry_delay(retry_delay, options.max_retry_delay);
        }
    });

    (event_rx, err_rx)
}

fn subscribe_once(
    ctx: &RequestContext,
    url: &str,
    options: &SubscribeOptions,
    events: &Sender<Event>,
) -> Result<(), SubscribeError> {
    let mut req = options.agent.get(url);
    for (key, value) in &options.headers {
        req = req.set(key, value);
    }

    let response = req.call();
    let response = match response {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, resp)) => {
            let body = read_body_snippet(resp.into_reader(), 1024);
            return Err(SubscribeError::other(format!(
                "cxdb subscribe: unexpected status {}: {}",
                code,
                body.trim()
            )));
        }
        Err(ureq::Error::Transport(err)) => {
            return Err(SubscribeError::other(format!(
                "cxdb subscribe: request failed: {}",
                err
            )));
        }
    };

    let status = response.status();
    if status != 200 {
        let body = read_body_snippet(response.into_reader(), 1024);
        return Err(SubscribeError::other(format!(
            "cxdb subscribe: unexpected status {}: {}",
            status,
            body.trim()
        )));
    }

    let reader = response.into_reader();
    match read_event_stream(ctx, reader, options.max_event_bytes, |ev| {
        send_event(ctx, events, ev)
    }) {
        Ok(()) => Ok(()),
        Err(err) => {
            if err.is_eof() {
                return Err(SubscribeError::eof());
            }
            Err(err)
        }
    }
}

fn read_event_stream<R, F>(
    ctx: &RequestContext,
    reader: R,
    max_event_bytes: usize,
    mut emit: F,
) -> Result<(), SubscribeError>
where
    R: Read,
    F: FnMut(Event) -> Result<(), SubscribeError>,
{
    let mut br = BufReader::new(reader);

    let mut event_type = String::new();
    let mut data_lines: Vec<String> = Vec::new();
    let mut last_id = String::new();
    let mut data_size: usize = 0;

    loop {
        if let Some(status) = ctx_status(ctx) {
            return Err(status.into());
        }

        let mut line = String::new();
        let bytes = match br.read_line(&mut line) {
            Ok(bytes) => bytes,
            Err(err) => {
                return Err(SubscribeError::other(format!(
                    "cxdb subscribe: read error: {}",
                    err
                )))
            }
        };

        if bytes == 0 {
            return Err(SubscribeError::eof());
        }

        let eof = !line.ends_with('\n');
        line = line.trim_end_matches(['\r', '\n']).to_string();

        if line.is_empty() {
            flush_event(
                &mut event_type,
                &mut data_lines,
                &mut last_id,
                &mut data_size,
                &mut emit,
            )?;
            if eof {
                return Err(SubscribeError::eof());
            }
            continue;
        }

        if line.starts_with(':') {
            if eof {
                return Err(SubscribeError::eof());
            }
            continue;
        }

        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value),
            None => (line.as_str(), ""),
        };

        if field.is_empty() || field.contains([' ', '\t']) {
            return Err(SubscribeError::other(format!(
                "cxdb subscribe: malformed field {:?}",
                field
            )));
        }

        let value = value.strip_prefix(' ').unwrap_or(value);

        match field {
            "event" => event_type = value.to_string(),
            "data" => {
                data_lines.push(value.to_string());
                data_size += value.len();
                if max_event_bytes > 0 && data_size > max_event_bytes {
                    return Err(SubscribeError::other(format!(
                        "cxdb subscribe: event exceeds max size ({} bytes)",
                        data_size
                    )));
                }
            }
            "id" => last_id = value.to_string(),
            "retry" => {}
            _ => {}
        }

        if eof {
            flush_event(
                &mut event_type,
                &mut data_lines,
                &mut last_id,
                &mut data_size,
                &mut emit,
            )?;
            return Err(SubscribeError::eof());
        }
    }
}

fn reset_state(
    event_type: &mut String,
    data_lines: &mut Vec<String>,
    last_id: &mut String,
    data_size: &mut usize,
) {
    event_type.clear();
    data_lines.clear();
    last_id.clear();
    *data_size = 0;
}

fn flush_event<F>(
    event_type: &mut String,
    data_lines: &mut Vec<String>,
    last_id: &mut String,
    data_size: &mut usize,
    emit: &mut F,
) -> Result<(), SubscribeError>
where
    F: FnMut(Event) -> Result<(), SubscribeError>,
{
    if data_lines.is_empty() && event_type.is_empty() && last_id.is_empty() {
        reset_state(event_type, data_lines, last_id, data_size);
        return Ok(());
    }

    let data = data_lines.join("\n");
    if data.is_empty() {
        reset_state(event_type, data_lines, last_id, data_size);
        return Ok(());
    }

    if event_type.is_empty() {
        *event_type = "message".to_string();
    }

    let event = Event {
        event_type: event_type.clone(),
        data: data.into_bytes(),
        id: last_id.clone(),
    };
    emit(event)?;
    reset_state(event_type, data_lines, last_id, data_size);
    Ok(())
}

fn next_retry_delay(current: Duration, max: Duration) -> Duration {
    if current <= Duration::ZERO {
        return DEFAULT_RETRY_DELAY;
    }
    let next = current * 2;
    if max > Duration::ZERO && next > max {
        return max;
    }
    next
}

fn send_event(
    ctx: &RequestContext,
    events: &Sender<Event>,
    event: Event,
) -> Result<(), SubscribeError> {
    let mut event = Some(event);
    loop {
        if let Some(status) = ctx_status(ctx) {
            return Err(status.into());
        }
        match events.send_timeout(
            event.take().expect("event present"),
            Duration::from_millis(50),
        ) {
            Ok(()) => return Ok(()),
            Err(SendTimeoutError::Timeout(ev)) => {
                event = Some(ev);
            }
            Err(SendTimeoutError::Disconnected(_)) => {
                return Err(SubscribeError::other(
                    "cxdb subscribe: event channel closed",
                ));
            }
        }
    }
}

fn non_blocking_send<T>(ch: &Sender<T>, value: T) {
    match ch.try_send(value) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {}
        Err(TrySendError::Disconnected(_)) => {}
    }
}

fn read_body_snippet(mut reader: impl Read, limit: usize) -> String {
    let mut buf = Vec::with_capacity(limit);
    let _ = reader.by_ref().take(limit as u64).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

#[derive(Clone, Copy)]
enum CtxStatus {
    Cancelled,
    Timeout,
}

impl From<CtxStatus> for SubscribeError {
    fn from(value: CtxStatus) -> Self {
        match value {
            CtxStatus::Cancelled => SubscribeError::cancelled(),
            CtxStatus::Timeout => SubscribeError::timeout(),
        }
    }
}

fn ctx_status(ctx: &RequestContext) -> Option<CtxStatus> {
    if ctx.is_cancelled() {
        return Some(CtxStatus::Cancelled);
    }
    if let Some(deadline) = ctx.deadline() {
        if Instant::now() >= deadline {
            return Some(CtxStatus::Timeout);
        }
    }
    None
}

fn sleep_with_cancel(ctx: &RequestContext, mut delay: Duration) -> bool {
    let step = Duration::from_millis(50);
    while delay > Duration::ZERO {
        if ctx_status(ctx).is_some() {
            return false;
        }
        let sleep_for = if delay > step { step } else { delay };
        thread::sleep(sleep_for);
        delay = delay.saturating_sub(sleep_for);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_event_stream_multi_line() {
        let input = "event: turn_appended\n\
data: {\"a\":1}\n\
data: {\"b\":2}\n\n";
        let ctx = RequestContext::background();
        let mut events = Vec::new();
        let err = read_event_stream(&ctx, input.as_bytes(), 1024, |ev| {
            events.push(ev);
            Ok(())
        })
        .unwrap_err();
        assert!(err.is_eof());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "turn_appended");
        assert_eq!(
            String::from_utf8_lossy(&events[0].data),
            "{\"a\":1}\n{\"b\":2}"
        );
    }

    #[test]
    fn read_event_stream_default_type_and_comments() {
        let input = ": heartbeat\n\
data: {\"ok\":true}\n\n";
        let ctx = RequestContext::background();
        let mut events = Vec::new();
        let err = read_event_stream(&ctx, input.as_bytes(), 1024, |ev| {
            events.push(ev);
            Ok(())
        })
        .unwrap_err();
        assert!(err.is_eof());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "message");
        assert_eq!(String::from_utf8_lossy(&events[0].data), "{\"ok\":true}");
    }

    #[test]
    fn read_event_stream_oversize() {
        let input = format!("event: big\ndata: {}\n\n", "x".repeat(20));
        let ctx = RequestContext::background();
        let err = read_event_stream(&ctx, input.as_bytes(), 10, |_| Ok(()))
            .expect_err("expected oversize error");
        assert!(!err.is_eof());
    }

    #[test]
    fn read_event_stream_malformed_field() {
        let input = "bad field\n\n";
        let ctx = RequestContext::background();
        let err = read_event_stream(&ctx, input.as_bytes(), 1024, |_| Ok(()))
            .expect_err("expected malformed field error");
        assert!(err.detail.contains("malformed field"));
    }

    #[test]
    fn subscribe_events_invalid_url() {
        let ctx = RequestContext::background();
        let (_events, errs) = subscribe_events(&ctx, "", Vec::new());
        let err = errs.recv().expect("error");
        assert!(err.to_string().contains("url is required"));
    }
}
