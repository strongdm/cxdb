// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{
    bounded, Receiver, RecvTimeoutError, SendTimeoutError, Sender, TrySendError,
};

use crate::client::RequestContext;
use crate::context::ContextHead;
use crate::error::Error;
use crate::events::decode_turn_appended;
use crate::subscribe::Event;
use crate::turn::{GetLastOptions, TurnRecord};

#[derive(Debug)]
pub enum FollowError {
    Cancelled,
    Timeout,
    Decode(String),
    Client(Error),
    Other(String),
}

impl std::fmt::Display for FollowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FollowError::Cancelled => write!(f, "context canceled"),
            FollowError::Timeout => write!(f, "context deadline exceeded"),
            FollowError::Decode(msg) => write!(f, "{}", msg),
            FollowError::Client(err) => write!(f, "{}", err),
            FollowError::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for FollowError {}

impl From<Error> for FollowError {
    fn from(err: Error) -> Self {
        FollowError::Client(err)
    }
}

pub trait TurnClient: Send + Sync {
    fn get_head(&self, ctx: &RequestContext, context_id: u64) -> Result<ContextHead, Error>;
    fn get_last(
        &self,
        ctx: &RequestContext,
        context_id: u64,
        opts: GetLastOptions,
    ) -> Result<Vec<TurnRecord>, Error>;
}

impl TurnClient for crate::client::Client {
    fn get_head(&self, ctx: &RequestContext, context_id: u64) -> Result<ContextHead, Error> {
        self.get_head(ctx, context_id)
    }

    fn get_last(
        &self,
        ctx: &RequestContext,
        context_id: u64,
        opts: GetLastOptions,
    ) -> Result<Vec<TurnRecord>, Error> {
        self.get_last(ctx, context_id, opts)
    }
}

#[derive(Clone, Copy)]
struct FollowOptions {
    buffer_size: usize,
    max_seen_per_context: usize,
}

impl Default for FollowOptions {
    fn default() -> Self {
        Self {
            buffer_size: DEFAULT_FOLLOW_BUFFER,
            max_seen_per_context: DEFAULT_MAX_SEEN_PER_CONTEXT,
        }
    }
}

#[derive(Clone)]
pub struct FollowOption(Arc<dyn Fn(&mut FollowOptions) + Send + Sync>);

impl FollowOption {
    fn apply(&self, opts: &mut FollowOptions) {
        (self.0)(opts);
    }
}

pub fn with_follow_buffer(size: usize) -> FollowOption {
    FollowOption(Arc::new(move |opts| opts.buffer_size = size))
}

pub fn with_max_seen_per_context(limit: usize) -> FollowOption {
    FollowOption(Arc::new(move |opts| opts.max_seen_per_context = limit))
}

const DEFAULT_FOLLOW_BUFFER: usize = 128;
const DEFAULT_MAX_SEEN_PER_CONTEXT: usize = 2048;

#[derive(Debug, Clone)]
pub struct FollowTurn {
    pub context_id: u64,
    pub turn: TurnRecord,
}

pub fn follow_turns(
    ctx: &RequestContext,
    events: Receiver<Event>,
    client: Arc<dyn TurnClient>,
    opts: impl IntoIterator<Item = FollowOption>,
) -> (Receiver<FollowTurn>, Receiver<FollowError>) {
    let mut options = FollowOptions::default();
    for opt in opts {
        opt.apply(&mut options);
    }

    let (out_tx, out_rx) = bounded(options.buffer_size);
    let (err_tx, err_rx) = bounded(options.buffer_size);
    let ctx = ctx.clone();

    thread::spawn(move || {
        let mut states: HashMap<u64, FollowState> = HashMap::new();
        loop {
            if ctx_status(&ctx).is_some() {
                return;
            }

            let ev = match events.recv_timeout(Duration::from_millis(100)) {
                Ok(ev) => ev,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };

            if ev.event_type != "turn_appended" {
                continue;
            }

            let turn_event = match decode_turn_appended_event(&ev.data) {
                Ok(event) => event,
                Err(err) => {
                    non_blocking_send(&err_tx, err);
                    continue;
                }
            };

            let state = states
                .entry(turn_event.context_id)
                .or_insert_with(|| FollowState::new(options.max_seen_per_context));
            if let Err(err) =
                state.sync_context(&ctx, client.as_ref(), turn_event.context_id, &out_tx)
            {
                non_blocking_send(&err_tx, err);
            }
        }
    });

    (out_rx, err_rx)
}

struct FollowState {
    has_last: bool,
    last_seen_turn_id: u64,
    last_seen_depth: u32,
    seen: HashSet<u64>,
    seen_order: VecDeque<u64>,
    max_seen: usize,
}

impl FollowState {
    fn new(max_seen: usize) -> Self {
        let max_seen = if max_seen == 0 {
            DEFAULT_MAX_SEEN_PER_CONTEXT
        } else {
            max_seen
        };
        Self {
            has_last: false,
            last_seen_turn_id: 0,
            last_seen_depth: 0,
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            max_seen,
        }
    }

    fn sync_context(
        &mut self,
        ctx: &RequestContext,
        client: &dyn TurnClient,
        context_id: u64,
        out: &Sender<FollowTurn>,
    ) -> Result<(), FollowError> {
        let head = client.get_head(ctx, context_id)?;
        if self.has_last && head.head_depth < self.last_seen_depth {
            return Err(FollowError::Other(format!(
                "follow turns: head depth regressed (context {})",
                context_id
            )));
        }

        let missing = if self.has_last && !self.seen.is_empty() {
            head.head_depth.saturating_sub(self.last_seen_depth)
        } else {
            head.head_depth + 1
        };

        if missing == 0 {
            return Ok(());
        }

        let turns = client.get_last(
            ctx,
            context_id,
            GetLastOptions {
                limit: missing,
                include_payload: true,
            },
        )?;

        for turn in turns {
            if self.seen_turn(turn.turn_id) {
                continue;
            }
            send_follow_turn(
                ctx,
                out,
                FollowTurn {
                    context_id,
                    turn: turn.clone(),
                },
            )?;
            self.record_turn(&turn);
        }

        Ok(())
    }

    fn seen_turn(&self, turn_id: u64) -> bool {
        self.seen.contains(&turn_id)
    }

    fn record_turn(&mut self, turn: &TurnRecord) {
        self.seen.insert(turn.turn_id);
        self.seen_order.push_back(turn.turn_id);
        while self.seen_order.len() > self.max_seen {
            if let Some(oldest) = self.seen_order.pop_front() {
                self.seen.remove(&oldest);
            }
        }
        if !self.has_last || turn.depth >= self.last_seen_depth {
            self.last_seen_depth = turn.depth;
            self.last_seen_turn_id = turn.turn_id;
            self.has_last = true;
        }
    }
}

fn decode_turn_appended_event(
    data: &[u8],
) -> Result<crate::events::TurnAppendedEvent, FollowError> {
    if data.is_empty() {
        return Err(FollowError::Decode(
            "turn_appended: empty payload".to_string(),
        ));
    }
    let event = decode_turn_appended(data)
        .map_err(|err| FollowError::Decode(format!("turn_appended: decode: {}", err)))?;
    if event.context_id == 0 {
        return Err(FollowError::Decode(
            "turn_appended: missing context_id".to_string(),
        ));
    }
    if event.turn_id == 0 {
        return Err(FollowError::Decode(
            "turn_appended: missing turn_id".to_string(),
        ));
    }
    Ok(event)
}

fn send_follow_turn(
    ctx: &RequestContext,
    out: &Sender<FollowTurn>,
    turn: FollowTurn,
) -> Result<(), FollowError> {
    let mut turn = Some(turn);
    loop {
        if let Some(status) = ctx_status(ctx) {
            return Err(status);
        }
        match out.send_timeout(
            turn.take().expect("turn present"),
            Duration::from_millis(50),
        ) {
            Ok(()) => return Ok(()),
            Err(SendTimeoutError::Timeout(item)) => {
                turn = Some(item);
            }
            Err(SendTimeoutError::Disconnected(_)) => {
                return Err(FollowError::Other(
                    "follow turns: output channel closed".to_string(),
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

fn ctx_status(ctx: &RequestContext) -> Option<FollowError> {
    if ctx.is_cancelled() {
        return Some(FollowError::Cancelled);
    }
    if let Some(deadline) = ctx.deadline() {
        if Instant::now() >= deadline {
            return Some(FollowError::Timeout);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use crate::error::ErrContextNotFound;

    #[derive(Default)]
    struct StubTurnClient {
        turns: Mutex<HashMap<u64, Vec<TurnRecord>>>,
        heads: Mutex<HashMap<u64, ContextHead>>,
    }

    impl StubTurnClient {
        fn set_context(&self, context_id: u64, turns: Vec<TurnRecord>) {
            let mut turns_lock = self.turns.lock().unwrap();
            turns_lock.insert(context_id, turns.clone());

            let mut heads_lock = self.heads.lock().unwrap();
            if turns.is_empty() {
                heads_lock.insert(
                    context_id,
                    ContextHead {
                        context_id,
                        head_turn_id: 0,
                        head_depth: 0,
                    },
                );
            } else {
                let head = turns.last().unwrap();
                heads_lock.insert(
                    context_id,
                    ContextHead {
                        context_id,
                        head_turn_id: head.turn_id,
                        head_depth: head.depth,
                    },
                );
            }
        }
    }

    impl TurnClient for StubTurnClient {
        fn get_head(&self, _ctx: &RequestContext, context_id: u64) -> Result<ContextHead, Error> {
            let heads = self.heads.lock().unwrap();
            heads.get(&context_id).cloned().ok_or(ErrContextNotFound)
        }

        fn get_last(
            &self,
            _ctx: &RequestContext,
            context_id: u64,
            opts: GetLastOptions,
        ) -> Result<Vec<TurnRecord>, Error> {
            let turns = self.turns.lock().unwrap();
            let list = turns.get(&context_id).ok_or(ErrContextNotFound)?;
            let limit = if opts.limit == 0 || opts.limit as usize > list.len() {
                list.len()
            } else {
                opts.limit as usize
            };
            let start = list.len().saturating_sub(limit);
            Ok(list[start..].to_vec())
        }
    }

    fn make_turn_event(context_id: u64, turn_id: u64, depth: u32) -> Event {
        let payload = serde_json::json!({
            "context_id": context_id,
            "turn_id": turn_id,
            "parent_turn_id": 0,
            "depth": depth,
        });
        Event {
            event_type: "turn_appended".to_string(),
            data: serde_json::to_vec(&payload).unwrap(),
            id: String::new(),
        }
    }

    #[test]
    fn follow_turns_backfill_and_dedupe() {
        let client = Arc::new(StubTurnClient::default());
        let context_id = 1;
        client.set_context(
            context_id,
            vec![
                TurnRecord {
                    turn_id: 1,
                    parent_id: 0,
                    depth: 0,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
                TurnRecord {
                    turn_id: 2,
                    parent_id: 1,
                    depth: 1,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
            ],
        );

        let (event_tx, event_rx) = bounded(10);
        let ctx = RequestContext::background();
        let (out, errs) =
            follow_turns(&ctx, event_rx, client.clone(), vec![with_follow_buffer(10)]);

        event_tx.send(make_turn_event(context_id, 2, 1)).unwrap();

        client.set_context(
            context_id,
            vec![
                TurnRecord {
                    turn_id: 1,
                    parent_id: 0,
                    depth: 0,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
                TurnRecord {
                    turn_id: 2,
                    parent_id: 1,
                    depth: 1,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
                TurnRecord {
                    turn_id: 3,
                    parent_id: 2,
                    depth: 2,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
            ],
        );
        event_tx.send(make_turn_event(context_id, 3, 2)).unwrap();
        event_tx.send(make_turn_event(context_id, 3, 2)).unwrap();
        drop(event_tx);

        let got: Vec<u64> = out.iter().map(|turn| turn.turn.turn_id).collect();
        if let Some(err) = errs.try_iter().next() {
            panic!("unexpected error: {}", err);
        }

        assert_eq!(got, vec![1, 2, 3]);
    }

    #[test]
    fn follow_turns_out_of_order() {
        let client = Arc::new(StubTurnClient::default());
        let context_id = 2;
        client.set_context(
            context_id,
            vec![
                TurnRecord {
                    turn_id: 10,
                    parent_id: 0,
                    depth: 0,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
                TurnRecord {
                    turn_id: 11,
                    parent_id: 10,
                    depth: 1,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
            ],
        );

        let (event_tx, event_rx) = bounded(10);
        let ctx = RequestContext::background();
        let (out, errs) =
            follow_turns(&ctx, event_rx, client.clone(), vec![with_follow_buffer(10)]);

        event_tx.send(make_turn_event(context_id, 11, 1)).unwrap();
        event_tx.send(make_turn_event(context_id, 10, 0)).unwrap();
        drop(event_tx);

        let got: Vec<u64> = out.iter().map(|turn| turn.turn.turn_id).collect();
        if let Some(err) = errs.try_iter().next() {
            panic!("unexpected error: {}", err);
        }

        assert_eq!(got, vec![10, 11]);
    }

    #[test]
    fn follow_turns_multiple_contexts() {
        let client = Arc::new(StubTurnClient::default());
        client.set_context(
            1,
            vec![
                TurnRecord {
                    turn_id: 1,
                    parent_id: 0,
                    depth: 0,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
                TurnRecord {
                    turn_id: 2,
                    parent_id: 1,
                    depth: 1,
                    type_id: String::new(),
                    type_version: 0,
                    encoding: 0,
                    compression: 0,
                    payload_hash: [0; 32],
                    payload: Vec::new(),
                },
            ],
        );
        client.set_context(
            2,
            vec![TurnRecord {
                turn_id: 10,
                parent_id: 0,
                depth: 0,
                type_id: String::new(),
                type_version: 0,
                encoding: 0,
                compression: 0,
                payload_hash: [0; 32],
                payload: Vec::new(),
            }],
        );

        let (event_tx, event_rx) = bounded(10);
        let ctx = RequestContext::background();
        let (out, errs) =
            follow_turns(&ctx, event_rx, client.clone(), vec![with_follow_buffer(10)]);

        event_tx.send(make_turn_event(2, 10, 0)).unwrap();
        event_tx.send(make_turn_event(1, 2, 1)).unwrap();
        drop(event_tx);

        let got: Vec<u64> = out.iter().map(|turn| turn.turn.turn_id).collect();
        if let Some(err) = errs.try_iter().next() {
            panic!("unexpected error: {}", err);
        }

        assert_eq!(got.len(), 3);
    }
}
