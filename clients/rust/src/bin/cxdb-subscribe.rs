// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::env;
use std::sync::Arc;

use crossbeam_channel::TryRecvError;

use cxdb::types::ConversationItem;
use cxdb::{
    decode_msgpack_into, dial, dial_tls, follow_turns, subscribe_events, with_client_tag,
    CompressionNone, EncodingMsgpack, Event, FollowError, FollowTurn, RequestContext,
    SubscribeError, TurnClient,
};

#[derive(Default)]
struct Config {
    events_url: String,
    bin_addr: String,
    follow_turns: bool,
    use_tls: bool,
    client_tag: String,
    max_events: usize,
    max_turns: usize,
    max_errors: usize,
}

fn main() {
    let config = match parse_args() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("{err}");
            eprintln!();
            print_usage();
            std::process::exit(2);
        }
    };

    if config.events_url.is_empty() {
        eprintln!("--cxdb-events-url is required");
        std::process::exit(2);
    }
    if config.follow_turns && config.bin_addr.is_empty() {
        eprintln!("--cxdb-bin-addr is required when --follow-turns is set");
        std::process::exit(2);
    }

    let (ctx, cancel_handle) = RequestContext::cancellable();
    let cancel_handle = Arc::new(cancel_handle);
    let cancel_handle_clone = Arc::clone(&cancel_handle);
    let _ = ctrlc::set_handler(move || {
        cancel_handle_clone.cancel();
    });

    let (events, errs) = subscribe_events(&ctx, &config.events_url, Vec::new());

    if config.follow_turns {
        let mut client_opts = Vec::new();
        if !config.client_tag.is_empty() {
            client_opts.push(with_client_tag(config.client_tag));
        }
        let client = if config.use_tls {
            match dial_tls(&config.bin_addr, client_opts) {
                Ok(client) => client,
                Err(err) => {
                    eprintln!("dial cxdb: {err}");
                    std::process::exit(1);
                }
            }
        } else {
            match dial(&config.bin_addr, client_opts) {
                Ok(client) => client,
                Err(err) => {
                    eprintln!("dial cxdb: {err}");
                    std::process::exit(1);
                }
            }
        };

        let client = Arc::new(client);
        let (event_out, follow_events) = tee_events(events);
        let turn_client: Arc<dyn TurnClient> = client;
        let (turns, turn_errs) = follow_turns(&ctx, follow_events, turn_client, Vec::new());

        let error_count = consume(
            &ctx,
            &cancel_handle,
            ConsumeChannels {
                events: event_out,
                errs,
                turn_errs: Some(turn_errs),
                turns: Some(turns),
            },
            ConsumeOptions {
                max_events: config.max_events,
                max_turns: config.max_turns,
                max_errors: config.max_errors,
            },
        );
        if config.max_errors > 0 && error_count >= config.max_errors {
            std::process::exit(1);
        }
        return;
    }

    let error_count = consume(
        &ctx,
        &cancel_handle,
        ConsumeChannels {
            events,
            errs,
            turn_errs: None,
            turns: None,
        },
        ConsumeOptions {
            max_events: config.max_events,
            max_turns: config.max_turns,
            max_errors: config.max_errors,
        },
    );
    if config.max_errors > 0 && error_count >= config.max_errors {
        std::process::exit(1);
    }
}

struct ConsumeChannels {
    events: crossbeam_channel::Receiver<Event>,
    errs: crossbeam_channel::Receiver<SubscribeError>,
    turn_errs: Option<crossbeam_channel::Receiver<FollowError>>,
    turns: Option<crossbeam_channel::Receiver<FollowTurn>>,
}

struct ConsumeOptions {
    max_events: usize,
    max_turns: usize,
    max_errors: usize,
}

fn consume(
    ctx: &RequestContext,
    cancel_handle: &Arc<cxdb::client::CancelHandle>,
    channels: ConsumeChannels,
    options: ConsumeOptions,
) -> usize {
    let mut events = Some(channels.events);
    let mut errs = Some(channels.errs);
    let mut turn_errs = channels.turn_errs;
    let mut turns = channels.turns;
    let mut event_count = 0usize;
    let mut turn_count = 0usize;
    let mut error_count = 0usize;

    loop {
        if ctx.is_cancelled() {
            return error_count;
        }

        if events.is_none() && errs.is_none() && turn_errs.is_none() && turns.is_none() {
            return error_count;
        }

        let mut progressed = false;

        if let Some(rx) = events.as_ref() {
            match rx.try_recv() {
                Ok(ev) => {
                    print_event(&ev);
                    event_count += 1;
                    progressed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    events = None;
                    progressed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        if let Some(rx) = errs.as_ref() {
            match rx.try_recv() {
                Ok(err) => {
                    eprintln!("subscribe error: {err}");
                    error_count += 1;
                    progressed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    errs = None;
                    progressed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        if let Some(rx) = turn_errs.as_ref() {
            match rx.try_recv() {
                Ok(err) => {
                    eprintln!("follow error: {err}");
                    error_count += 1;
                    progressed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    turn_errs = None;
                    progressed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        if let Some(rx) = turns.as_ref() {
            match rx.try_recv() {
                Ok(turn) => {
                    print_turn(&turn);
                    turn_count += 1;
                    progressed = true;
                }
                Err(TryRecvError::Disconnected) => {
                    turns = None;
                    progressed = true;
                }
                Err(TryRecvError::Empty) => {}
            }
        }

        stop_if_done(
            cancel_handle,
            &mut event_count,
            &mut turn_count,
            &mut error_count,
            options.max_events,
            options.max_turns,
            options.max_errors,
        );

        if !progressed {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

fn stop_if_done(
    cancel_handle: &Arc<cxdb::client::CancelHandle>,
    event_count: &mut usize,
    turn_count: &mut usize,
    error_count: &mut usize,
    max_events: usize,
    max_turns: usize,
    max_errors: usize,
) {
    if max_errors > 0 && *error_count >= max_errors {
        cancel_handle.cancel();
        return;
    }

    let stop_on_events = max_events > 0;
    let stop_on_turns = max_turns > 0;

    if stop_on_events || stop_on_turns {
        let events_ok = !stop_on_events || *event_count >= max_events;
        let turns_ok = !stop_on_turns || *turn_count >= max_turns;
        if events_ok && turns_ok {
            cancel_handle.cancel();
        }
    }
}

fn print_event(ev: &Event) {
    let data: serde_json::Value = match serde_json::from_slice(&ev.data) {
        Ok(value) => value,
        Err(err) => {
            eprintln!("encode event: {err}");
            return;
        }
    };
    let output = serde_json::json!({
        "kind": "event",
        "type": ev.event_type,
        "data": data,
    });
    println!("{}", output);
}

fn print_turn(turn: &FollowTurn) {
    let mut output = serde_json::Map::new();
    output.insert(
        "kind".to_string(),
        serde_json::Value::String("turn".to_string()),
    );
    output.insert(
        "context_id".to_string(),
        serde_json::Value::Number(turn.context_id.into()),
    );
    output.insert(
        "turn_id".to_string(),
        serde_json::Value::Number(turn.turn.turn_id.into()),
    );
    output.insert(
        "depth".to_string(),
        serde_json::Value::Number((turn.turn.depth as u64).into()),
    );

    if !turn.turn.type_id.is_empty() {
        output.insert(
            "declared_type_id".to_string(),
            serde_json::Value::String(turn.turn.type_id.clone()),
        );
    }
    if turn.turn.type_version != 0 {
        output.insert(
            "declared_type_version".to_string(),
            serde_json::Value::Number((turn.turn.type_version as u64).into()),
        );
    }

    let mut decode_error: Option<String> = None;
    let mut item: Option<ConversationItem> = None;
    if turn.turn.encoding != EncodingMsgpack {
        decode_error = Some("unsupported encoding".to_string());
    } else if turn.turn.compression != CompressionNone {
        decode_error = Some("unsupported compression".to_string());
    } else {
        match decode_msgpack_into::<ConversationItem>(&turn.turn.payload) {
            Ok(decoded) => item = Some(decoded),
            Err(err) => decode_error = Some(err.to_string()),
        }
    }

    if let Some(err) = decode_error {
        output.insert("decode_error".to_string(), serde_json::Value::String(err));
    }
    if let Some(item) = item {
        output.insert(
            "item".to_string(),
            serde_json::to_value(item).unwrap_or(serde_json::Value::Null),
        );
    }

    let output = serde_json::Value::Object(output);
    println!("{}", output);
}

fn tee_events(
    events: crossbeam_channel::Receiver<Event>,
) -> (
    crossbeam_channel::Receiver<Event>,
    crossbeam_channel::Receiver<Event>,
) {
    let (out_tx, out_rx) = crossbeam_channel::bounded(128);
    let (follow_tx, follow_rx) = crossbeam_channel::bounded(128);
    std::thread::spawn(move || {
        for ev in events.iter() {
            if out_tx.send(ev.clone()).is_err() {
                break;
            }
            if follow_tx.send(ev).is_err() {
                break;
            }
        }
    });
    (out_rx, follow_rx)
}

fn parse_args() -> Result<Config, String> {
    let mut cfg = Config::default();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "--cxdb-events-url" => {
                cfg.events_url = next_value(&mut args, &arg)?;
            }
            "--cxdb-bin-addr" => {
                cfg.bin_addr = next_value(&mut args, &arg)?;
            }
            "--follow-turns" => {
                cfg.follow_turns = true;
            }
            "--tls" => {
                cfg.use_tls = true;
            }
            "--client-tag" => {
                cfg.client_tag = next_value(&mut args, &arg)?;
            }
            "--max-events" => {
                cfg.max_events = next_value(&mut args, &arg)?
                    .parse()
                    .map_err(|_| "invalid --max-events value".to_string())?;
            }
            "--max-turns" => {
                cfg.max_turns = next_value(&mut args, &arg)?
                    .parse()
                    .map_err(|_| "invalid --max-turns value".to_string())?;
            }
            "--max-errors" => {
                cfg.max_errors = next_value(&mut args, &arg)?
                    .parse()
                    .map_err(|_| "invalid --max-errors value".to_string())?;
            }
            _ => {
                return Err(format!("unknown argument: {}", arg));
            }
        }
    }
    Ok(cfg)
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {}", flag))
}

fn print_usage() {
    eprintln!("Usage: cxdb-subscribe --cxdb-events-url URL [options]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --cxdb-events-url URL   CXDB SSE events URL (required)");
    eprintln!("  --cxdb-bin-addr ADDR    CXDB binary address (required for --follow-turns)");
    eprintln!("  --follow-turns          Follow turns via binary protocol");
    eprintln!("  --tls                   Use TLS for binary protocol");
    eprintln!("  --client-tag TAG        Optional client tag for binary protocol");
    eprintln!("  --max-events N          Stop after N SSE events (0 = no limit)");
    eprintln!("  --max-turns N           Stop after N decoded turns (0 = no limit)");
    eprintln!("  --max-errors N          Stop after N errors (0 = no limit)");
}
