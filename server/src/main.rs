// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use byteorder::WriteBytesExt;
use cxdb_server::config::Config;
use cxdb_server::error::{Result, StoreError};
use cxdb_server::events::{EventBus, StoreEvent};
use cxdb_server::http::start_http;
use cxdb_server::metrics::Metrics;
use cxdb_server::metrics::SessionTracker;
use cxdb_server::protocol::{
    encode_append_ack, encode_attach_fs_resp, encode_ctx_create_resp, encode_error,
    encode_hello_resp, encode_put_blob_resp, parse_append_turn, parse_attach_fs, parse_ctx_create,
    parse_ctx_fork, parse_get_blob, parse_get_head, parse_get_last, parse_hello, parse_put_blob,
    read_frame, write_frame, MsgType,
};
use cxdb_server::registry::Registry;
use cxdb_server::s3_sync::{S3Sync, S3SyncConfig, S3SyncHandle};
use cxdb_server::store::Store;

fn main() -> Result<()> {
    // Create tokio runtime for async S3 operations
    let rt =
        tokio::runtime::Runtime::new().map_err(|e| StoreError::Io(std::io::Error::other(e)))?;

    let config = Config::from_env();
    std::fs::create_dir_all(&config.data_dir)?;

    // S3 sync: restore from S3 if local data is empty
    let s3_sync_handle: Option<S3SyncHandle> = if let Some(s3_config) = S3SyncConfig::from_env() {
        // Run restore synchronously before opening stores
        let restored = rt.block_on(async {
            let s3_sync = S3Sync::new(s3_config.clone(), config.data_dir.clone()).await;
            match s3_sync.maybe_restore().await {
                Ok(true) => {
                    eprintln!("Restored data from S3");
                    true
                }
                Ok(false) => false,
                Err(e) => {
                    eprintln!("S3 restore check failed: {e}");
                    false
                }
            }
        });

        if restored {
            eprintln!("Data restored from S3, continuing startup");
        }

        // Start background sync task
        let handle = rt.block_on(async {
            let s3_sync = S3Sync::new(s3_config, config.data_dir.clone()).await;
            s3_sync.start_background_sync()
        });

        Some(handle)
    } else {
        eprintln!("S3 sync disabled (set CXDB_S3_SYNC_ENABLED=1 to enable)");
        None
    };

    let store = Arc::new(Mutex::new(Store::open(&config.data_dir)?));
    let registry = Arc::new(Mutex::new(Registry::open(
        &config.data_dir.join("registry"),
    )?));
    let metrics = Arc::new(Metrics::new(config.data_dir.clone()));
    let session_tracker = Arc::new(SessionTracker::new());
    let event_bus = Arc::new(EventBus::new());

    let _http = start_http(
        config.http_bind_addr.clone(),
        Arc::clone(&store),
        Arc::clone(&registry),
        Arc::clone(&metrics),
        Arc::clone(&session_tracker),
        Arc::clone(&event_bus),
    )?;

    // Setup graceful shutdown on SIGTERM/SIGINT
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = Arc::clone(&shutdown);
    ctrlc::set_handler(move || {
        eprintln!("\nReceived shutdown signal");
        shutdown_clone.store(true, Ordering::SeqCst);
    })
    .expect("Error setting signal handler");

    let listener = TcpListener::bind(&config.bind_addr)?;
    listener
        .set_nonblocking(true)
        .expect("Cannot set non-blocking");
    eprintln!("cxdb listening on {}", config.bind_addr);

    // Accept loop with shutdown check
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer_addr)) => {
                // Set blocking mode for client handling (listener is non-blocking for shutdown checks)
                if let Err(e) = stream.set_nonblocking(false) {
                    eprintln!("failed to set blocking mode: {e}");
                    continue;
                }
                let store = Arc::clone(&store);
                let metrics = Arc::clone(&metrics);
                let session_tracker = Arc::clone(&session_tracker);
                let event_bus = Arc::clone(&event_bus);
                let peer_addr_str = peer_addr.to_string();
                thread::spawn(move || {
                    if let Err(err) = handle_client(
                        stream,
                        store,
                        metrics,
                        session_tracker,
                        event_bus,
                        peer_addr_str,
                    ) {
                        eprintln!("connection error: {err}");
                    }
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No incoming connection, sleep briefly and check shutdown
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                eprintln!("accept error: {e}");
            }
        }
    }

    eprintln!("Shutting down...");

    // Graceful S3 sync shutdown (performs final sync)
    if let Some(handle) = s3_sync_handle {
        rt.block_on(async {
            handle.shutdown().await;
        });
    }

    eprintln!("Shutdown complete");
    Ok(())
}

fn handle_client(
    mut stream: TcpStream,
    store: Arc<Mutex<Store>>,
    metrics: Arc<Metrics>,
    session_tracker: Arc<SessionTracker>,
    event_bus: Arc<EventBus>,
    peer_addr: String,
) -> Result<()> {
    let session = metrics.register_session();
    let session_id = session.session_id();
    // Client tag will be set when HELLO is received
    let mut client_tag_received = false;
    let mut client_tag = String::new();

    loop {
        let (header, payload) = match read_frame(&mut stream) {
            Ok(v) => v,
            Err(StoreError::Io(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        };

        metrics.record_session_activity(session_id);
        session_tracker.record_activity(session_id);
        let msg_type = header.msg_type;
        let req_id = header.req_id;

        let op_start = std::time::Instant::now();
        let response = match msg_type {
            x if x == MsgType::Hello as u16 => {
                let hello = parse_hello(&payload)?;
                // Register session with client tag and peer address
                if !client_tag_received {
                    client_tag = hello.client_tag.clone();
                    session_tracker.register(
                        session_id,
                        hello.client_tag.clone(),
                        Some(peer_addr.clone()),
                    );
                    client_tag_received = true;

                    // Publish ClientConnected event
                    event_bus.publish(StoreEvent::ClientConnected {
                        session_id: session_id.to_string(),
                        client_tag: hello.client_tag.clone(),
                    });
                }
                let resp = encode_hello_resp(session_id, 1)?; // protocol version 1
                Ok((MsgType::Hello as u16, resp))
            }
            x if x == MsgType::CtxCreate as u16 => {
                // If no HELLO was sent, register with empty tag
                if !client_tag_received {
                    session_tracker.register(session_id, String::new(), Some(peer_addr.clone()));
                    client_tag_received = true;
                }
                let base_turn_id = parse_ctx_create(&payload)?;
                let mut store = store.lock().unwrap();
                let head = store.create_context(base_turn_id)?;
                // Associate context with this session
                session_tracker.add_context(session_id, head.context_id);

                // Publish ContextCreated event
                event_bus.publish(StoreEvent::ContextCreated {
                    context_id: head.context_id.to_string(),
                    session_id: session_id.to_string(),
                    client_tag: client_tag.clone(),
                    created_at: unix_ms(),
                });

                let resp =
                    encode_ctx_create_resp(head.context_id, head.head_turn_id, head.head_depth)?;
                Ok((MsgType::CtxCreate as u16, resp))
            }
            x if x == MsgType::CtxFork as u16 => {
                // If no HELLO was sent, register with empty tag
                if !client_tag_received {
                    session_tracker.register(session_id, String::new(), Some(peer_addr.clone()));
                    client_tag_received = true;
                }
                let base_turn_id = parse_ctx_fork(&payload)?;
                let mut store = store.lock().unwrap();
                let head = store.fork_context(base_turn_id)?;
                // Associate forked context with this session
                session_tracker.add_context(session_id, head.context_id);

                // Publish ContextCreated event for forked context
                event_bus.publish(StoreEvent::ContextCreated {
                    context_id: head.context_id.to_string(),
                    session_id: session_id.to_string(),
                    client_tag: client_tag.clone(),
                    created_at: unix_ms(),
                });

                let resp =
                    encode_ctx_create_resp(head.context_id, head.head_turn_id, head.head_depth)?;
                Ok((MsgType::CtxFork as u16, resp))
            }
            x if x == MsgType::GetHead as u16 => {
                let context_id = parse_get_head(&payload)?;
                let store = store.lock().unwrap();
                let head = store.get_head(context_id)?;
                let resp =
                    encode_ctx_create_resp(head.context_id, head.head_turn_id, head.head_depth)?;
                Ok((MsgType::GetHead as u16, resp))
            }
            x if x == MsgType::AppendTurn as u16 => {
                let req = parse_append_turn(&payload, header.flags)?;
                let declared_type_id_clone = req.declared_type_id.clone();
                let declared_type_version = req.declared_type_version;
                let mut store = store.lock().unwrap();
                let (record, metadata) = store.append_turn(
                    req.context_id,
                    req.parent_turn_id,
                    req.declared_type_id,
                    req.declared_type_version,
                    req.encoding,
                    req.compression,
                    req.uncompressed_len,
                    req.content_hash,
                    &req.payload_bytes,
                )?;
                // If fs_root_hash was provided, attach it to this turn
                if let Some(fs_root_hash) = req.fs_root_hash {
                    store.attach_fs(record.turn_id, fs_root_hash)?;
                }
                metrics.record_append(op_start.elapsed());

                // Publish TurnAppended event
                event_bus.publish(StoreEvent::TurnAppended {
                    context_id: req.context_id.to_string(),
                    turn_id: record.turn_id.to_string(),
                    parent_turn_id: record.parent_turn_id.to_string(),
                    depth: record.depth,
                    declared_type_id: Some(declared_type_id_clone),
                    declared_type_version: Some(declared_type_version),
                });

                // If metadata was extracted (first turn), publish ContextMetadataUpdated
                if let Some(meta) = metadata {
                    event_bus.publish(StoreEvent::ContextMetadataUpdated {
                        context_id: req.context_id.to_string(),
                        client_tag: meta.client_tag,
                        title: meta.title,
                        labels: meta.labels,
                        has_provenance: meta.provenance.is_some(),
                    });

                    if let Some(prov) = meta.provenance {
                        if let Some(parent_context_id) = prov.parent_context_id {
                            event_bus.publish(StoreEvent::ContextLinked {
                                child_context_id: req.context_id.to_string(),
                                parent_context_id: parent_context_id.to_string(),
                                root_context_id: prov.root_context_id.map(|v| v.to_string()),
                                spawn_reason: prov.spawn_reason,
                            });
                        }
                    }
                }

                let resp = encode_append_ack(
                    req.context_id,
                    record.turn_id,
                    record.depth,
                    &record.payload_hash,
                )?;
                Ok((MsgType::AppendTurn as u16, resp))
            }
            x if x == MsgType::AttachFs as u16 => {
                let req = parse_attach_fs(&payload)?;
                let mut store = store.lock().unwrap();
                store.attach_fs(req.turn_id, req.fs_root_hash)?;
                let resp = encode_attach_fs_resp(req.turn_id, &req.fs_root_hash)?;
                Ok((MsgType::AttachFs as u16, resp))
            }
            x if x == MsgType::PutBlob as u16 => {
                let req = parse_put_blob(&payload)?;
                let mut store = store.lock().unwrap();
                // Verify hash matches
                let actual_hash = blake3::hash(&req.data);
                if actual_hash.as_bytes() != &req.hash {
                    return Err(StoreError::InvalidInput("blob hash mismatch".into()));
                }
                let was_new = !store.blob_store.contains(&req.hash);
                store.blob_store.put_if_absent(req.hash, &req.data)?;
                let resp = encode_put_blob_resp(&req.hash, was_new)?;
                Ok((MsgType::PutBlob as u16, resp))
            }
            x if x == MsgType::GetLast as u16 => {
                let req = parse_get_last(&payload)?;
                let mut store = store.lock().unwrap();
                let items = store.get_last(req.context_id, req.limit, req.include_payload != 0)?;
                metrics.record_get_last(op_start.elapsed());
                let mut resp = Vec::new();
                resp.write_u32::<byteorder::LittleEndian>(items.len() as u32)?;
                for item in items {
                    resp.write_u64::<byteorder::LittleEndian>(item.record.turn_id)?;
                    resp.write_u64::<byteorder::LittleEndian>(item.record.parent_turn_id)?;
                    resp.write_u32::<byteorder::LittleEndian>(item.record.depth)?;
                    resp.write_u32::<byteorder::LittleEndian>(
                        item.meta.declared_type_id.len() as u32
                    )?;
                    resp.extend_from_slice(item.meta.declared_type_id.as_bytes());
                    resp.write_u32::<byteorder::LittleEndian>(item.meta.declared_type_version)?;
                    resp.write_u32::<byteorder::LittleEndian>(item.meta.encoding)?;
                    // always return raw payload when included
                    let compression = if item.payload.is_some() {
                        0
                    } else {
                        item.meta.compression
                    };
                    resp.write_u32::<byteorder::LittleEndian>(compression)?;
                    let uncompressed_len = item
                        .payload
                        .as_ref()
                        .map(|p| p.len() as u32)
                        .unwrap_or(item.meta.uncompressed_len);
                    resp.write_u32::<byteorder::LittleEndian>(uncompressed_len)?;
                    resp.extend_from_slice(&item.record.payload_hash);
                    if let Some(payload) = item.payload {
                        resp.write_u32::<byteorder::LittleEndian>(payload.len() as u32)?;
                        resp.extend_from_slice(&payload);
                    }
                }
                Ok((MsgType::GetLast as u16, resp))
            }
            x if x == MsgType::GetBlob as u16 => {
                let hash = parse_get_blob(&payload)?;
                let mut store = store.lock().unwrap();
                let bytes = store.get_blob(&hash)?;
                metrics.record_get_blob(op_start.elapsed());
                let mut resp = Vec::new();
                resp.write_u32::<byteorder::LittleEndian>(bytes.len() as u32)?;
                resp.extend_from_slice(&bytes);
                Ok((MsgType::GetBlob as u16, resp))
            }
            _ => Err(StoreError::InvalidInput("unknown msg_type".into())),
        };

        match response {
            Ok((resp_type, resp_payload)) => {
                write_frame(&mut stream, resp_type, 0, req_id, &resp_payload)?;
                stream.flush()?;
            }
            Err(err) => {
                let (code, detail) = map_error(&err);
                metrics.record_error("binary", code as u16, &detail, None);
                event_bus.publish(StoreEvent::ErrorOccurred {
                    timestamp_ms: unix_ms(),
                    kind: "binary".to_string(),
                    status_code: code as u16,
                    message: detail.clone(),
                    path: None,
                });
                let payload = encode_error(code, &detail)?;
                write_frame(&mut stream, MsgType::Error as u16, 0, req_id, &payload)?;
                stream.flush()?;
            }
        }
    }

    // Unregister session on disconnect and publish event
    let orphaned_contexts = session_tracker.unregister(session_id);
    event_bus.publish(StoreEvent::ClientDisconnected {
        session_id: session_id.to_string(),
        client_tag,
        contexts: orphaned_contexts.iter().map(|id| id.to_string()).collect(),
    });

    Ok(())
}

/// Get current time in milliseconds since Unix epoch.
fn unix_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn map_error(err: &StoreError) -> (u32, String) {
    match err {
        StoreError::NotFound(msg) => (404, msg.clone()),
        StoreError::InvalidInput(msg) => (422, msg.clone()),
        StoreError::Corrupt(msg) => (500, msg.clone()),
        StoreError::Io(msg) => (500, msg.to_string()),
    }
}
