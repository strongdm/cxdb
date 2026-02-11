// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Map, Value as JsonValue};
use tiny_http::{Header, Method, Response, Server, StatusCode};
use url::Url;

use crate::error::{Result, StoreError};
use crate::events::EventBus;
use crate::fs_store::EntryKind;
use crate::metrics::{Metrics, SessionTracker};
use crate::projection::{BytesRender, EnumRender, RenderOptions, TimeRender, U64Format};
use crate::registry::{PutOutcome, Registry, RegistryBundle, RendererSpec, TypeVersionSpec};
use crate::store::Store;

type HttpResponse = (u16, Response<std::io::Cursor<Vec<u8>>>);

pub fn start_http(
    bind_addr: String,
    store: Arc<Mutex<Store>>,
    registry: Arc<Mutex<Registry>>,
    metrics: Arc<Metrics>,
    session_tracker: Arc<SessionTracker>,
    event_bus: Arc<EventBus>,
) -> Result<thread::JoinHandle<()>> {
    let server = Server::http(&bind_addr)
        .map_err(|e| StoreError::InvalidInput(format!("http bind error: {e}")))?;
    let handle = thread::spawn(move || {
        for request in server.incoming_requests() {
            if let Err(err) = handle_request(
                request,
                &store,
                &registry,
                &metrics,
                &session_tracker,
                &event_bus,
            ) {
                eprintln!("http error: {err}");
            }
        }
    });
    Ok(handle)
}

fn handle_request(
    mut request: tiny_http::Request,
    store: &Arc<Mutex<Store>>,
    registry: &Arc<Mutex<Registry>>,
    metrics: &Arc<Metrics>,
    session_tracker: &Arc<SessionTracker>,
    event_bus: &Arc<EventBus>,
) -> Result<()> {
    let start = Instant::now();

    // Check for SSE request early - it needs special handling
    let url_str = format!("http://localhost{}", request.url());
    if let Ok(url) = Url::parse(&url_str) {
        let segments: Vec<String> = url
            .path_segments()
            .map(|c| c.map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let segments_ref: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();

        if request.method() == &Method::Get && segments_ref.as_slice() == ["v1", "events"] {
            return handle_sse_stream(request, event_bus);
        }
    }

    let result: Result<HttpResponse> = (|| {
        let method = request.method().clone();
        let url_str = format!("http://localhost{}", request.url());
        let url =
            Url::parse(&url_str).map_err(|_| StoreError::InvalidInput("invalid url".into()))?;
        let segments: Vec<String> = url
            .path_segments()
            .map(|c| c.map(|s| s.to_string()).collect())
            .unwrap_or_default();
        let segments_ref: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();

        match (method, segments_ref.as_slice()) {
            // Health check endpoint
            (Method::Get, ["healthz"]) => Ok((
                200,
                Response::from_data(b"ok".to_vec())
                    .with_status_code(StatusCode(200))
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], &b"text/plain"[..]).unwrap(),
                    ),
            )),
            (Method::Put, ["v1", "registry", "bundles", _bundle_id_raw]) => {
                let mut body = Vec::new();
                request.as_reader().read_to_end(&mut body)?;
                let bundle: RegistryBundle = serde_json::from_slice(&body)
                    .map_err(|e| StoreError::InvalidInput(format!("invalid json: {e}")))?;
                let body_id = bundle.bundle_id.clone();
                let mut registry = registry.lock().unwrap();
                match registry.put_bundle(&body_id, &body)? {
                    PutOutcome::AlreadyExists => Ok((
                        204,
                        Response::from_data(Vec::new()).with_status_code(StatusCode(204)),
                    )),
                    PutOutcome::Created => {
                        metrics.record_registry_ingest();
                        let bytes =
                            serde_json::to_vec(&json!({"bundle_id": body_id})).map_err(|e| {
                                StoreError::InvalidInput(format!("json encode error: {e}"))
                            })?;
                        Ok((
                            201,
                            Response::from_data(bytes)
                                .with_status_code(StatusCode(201))
                                .with_header(
                                    Header::from_bytes(
                                        &b"Content-Type"[..],
                                        &b"application/json"[..],
                                    )
                                    .unwrap(),
                                ),
                        ))
                    }
                }
            }
            (Method::Get, ["v1", "registry", "bundles", bundle_id]) => {
                let registry = registry.lock().unwrap();
                let bundle = registry
                    .get_bundle(bundle_id)
                    .ok_or_else(|| StoreError::NotFound("bundle".into()))?;
                let etag = format!("\"{}\"", blake3::hash(bundle).to_hex());
                if let Some(header) = request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("If-None-Match"))
                {
                    if header.value.as_str() == etag {
                        return Ok((
                            304,
                            Response::from_data(Vec::new()).with_status_code(StatusCode(304)),
                        ));
                    }
                }
                Ok((
                    200,
                    Response::from_data(bundle.to_vec())
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        )
                        .with_header(Header::from_bytes(&b"ETag"[..], etag.as_bytes()).unwrap()),
                ))
            }
            (Method::Get, ["v1", "registry", "types", type_id, "versions", version]) => {
                let version: u32 = version
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid version".into()))?;
                let registry = registry.lock().unwrap();
                let spec = registry
                    .get_type_version(type_id, version)
                    .ok_or_else(|| StoreError::NotFound("type version".into()))?;
                let json = type_version_to_json(spec);
                let bytes = serde_json::to_vec(&json)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    200,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            (Method::Get, ["v1", "registry", "renderers"]) => {
                let registry = registry.lock().unwrap();
                let renderers = registry.get_all_renderers();
                let renderers_json: serde_json::Map<String, JsonValue> = renderers
                    .into_iter()
                    .map(|(type_id, spec)| (type_id, renderer_spec_to_json(&spec)))
                    .collect();
                let resp = json!({ "renderers": JsonValue::Object(renderers_json) });
                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    200,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            (Method::Get, ["v1", "contexts"]) => {
                let params = parse_query(url.query().unwrap_or(""));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(20);
                let tag_filter = params.get("tag").cloned();
                let include_provenance = params
                    .get("include_provenance")
                    .map(|v| v == "1")
                    .unwrap_or(false);

                let mut store = store.lock().unwrap();
                let contexts = store.list_recent_contexts(limit);

                let contexts_json: Vec<JsonValue> = contexts
                    .iter()
                    .filter_map(|c| {
                        // Get session info for this context (for live status)
                        let session = session_tracker.get_session_for_context(c.context_id);
                        let session_id = session.as_ref().map(|s| s.session_id);
                        let is_live = session.is_some();
                        let last_activity_at = session.as_ref().map(|s| s.last_activity_at);
                        let session_peer_addr = session.as_ref().and_then(|s| s.peer_addr.clone());

                        // Get client_tag: prefer stored metadata, fall back to session
                        let stored_metadata = store.get_context_metadata(c.context_id);
                        let client_tag = stored_metadata
                            .as_ref()
                            .and_then(|m| m.client_tag.clone())
                            .or_else(|| session.as_ref().map(|s| s.client_tag.clone()))
                            .filter(|t| !t.is_empty());

                        // Apply tag filter if specified
                        if let Some(ref filter) = tag_filter {
                            let tag = client_tag.as_deref().unwrap_or("");
                            if tag != filter {
                                return None;
                            }
                        }

                        let mut obj = json!({
                            "context_id": c.context_id.to_string(),
                            "head_turn_id": c.head_turn_id.to_string(),
                            "head_depth": c.head_depth,
                            "created_at_unix_ms": c.created_at_unix_ms,
                            "is_live": is_live,
                        });

                        if let Some(tag) = client_tag {
                            obj["client_tag"] = JsonValue::String(tag);
                        }
                        if let Some(sid) = session_id {
                            obj["session_id"] = JsonValue::String(sid.to_string());
                        }
                        if let Some(ts) = last_activity_at {
                            obj["last_activity_at"] = JsonValue::Number(ts.into());
                        }

                        // Include provenance if requested
                        if include_provenance {
                            if let Some(ref metadata) = stored_metadata {
                                if let Some(ref prov) = metadata.provenance {
                                    // Clone provenance and inject server-side client_address if not present
                                    let mut prov_with_server_info = prov.clone();
                                    if prov_with_server_info.client_address.is_none() {
                                        prov_with_server_info.client_address =
                                            session_peer_addr.clone();
                                    }
                                    if let Ok(prov_json) =
                                        serde_json::to_value(&prov_with_server_info)
                                    {
                                        obj["provenance"] = prov_json;
                                    }
                                }
                            }
                        }

                        Some(obj)
                    })
                    .collect();

                // Get active sessions for response
                let active_sessions: Vec<JsonValue> = session_tracker
                    .get_active_sessions()
                    .iter()
                    .map(|s| {
                        let mut session_obj = json!({
                            "session_id": s.session_id.to_string(),
                            "client_tag": s.client_tag,
                            "connected_at": s.connected_at,
                            "last_activity_at": s.last_activity_at,
                            "context_count": s.contexts_created.len(),
                        });
                        if let Some(ref addr) = s.peer_addr {
                            session_obj["peer_addr"] = JsonValue::String(addr.clone());
                        }
                        session_obj
                    })
                    .collect();

                // Get unique tags for filtering
                let active_tags = session_tracker.get_active_tags();

                let resp = json!({
                    "contexts": contexts_json,
                    "count": contexts_json.len(),
                    "active_sessions": active_sessions,
                    "active_tags": active_tags,
                });

                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    200,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            // CQL search endpoint
            (Method::Get, ["v1", "contexts", "search"]) => {
                let params = parse_query(url.query().unwrap_or(""));
                let query = params.get("q").cloned().unwrap_or_default();
                let limit = params.get("limit").and_then(|v| v.parse::<u32>().ok());

                if query.is_empty() {
                    return Ok((
                        400,
                        Response::from_data(
                            serde_json::to_vec(&json!({
                                "error": "Missing required 'q' parameter"
                            }))
                            .unwrap(),
                        )
                        .with_status_code(StatusCode(400))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                    ));
                }

                // Get live context IDs from session tracker
                let live_contexts = session_tracker.get_live_context_ids();

                let store = store.lock().unwrap();
                match store.search_contexts(&query, &live_contexts, limit) {
                    Ok(result) => {
                        // Fetch full context details for matching IDs
                        let contexts_json: Vec<JsonValue> = result
                            .context_ids
                            .iter()
                            .filter_map(|&context_id| {
                                let head = store.turn_store.get_head(context_id).ok()?;
                                let session = session_tracker.get_session_for_context(context_id);
                                let is_live = session.is_some();

                                let mut obj = json!({
                                    "context_id": context_id.to_string(),
                                    "head_turn_id": head.head_turn_id.to_string(),
                                    "head_depth": head.head_depth,
                                    "created_at_unix_ms": head.created_at_unix_ms,
                                    "is_live": is_live,
                                });

                                // Add metadata if available (use cached data)
                                if let Some(metadata) = store
                                    .context_metadata_cache
                                    .get(&context_id)
                                    .and_then(|m| m.as_ref())
                                {
                                    if let Some(ref tag) = metadata.client_tag {
                                        obj["client_tag"] = JsonValue::String(tag.clone());
                                    }
                                    if let Some(ref title) = metadata.title {
                                        obj["title"] = JsonValue::String(title.clone());
                                    }
                                }

                                Some(obj)
                            })
                            .collect();

                        let resp = json!({
                            "contexts": contexts_json,
                            "total_count": result.total_count,
                            "elapsed_ms": result.elapsed_ms,
                            "query": result.query.raw,
                        });

                        let bytes = serde_json::to_vec(&resp).map_err(|e| {
                            StoreError::InvalidInput(format!("json encode error: {e}"))
                        })?;
                        Ok((
                            200,
                            Response::from_data(bytes)
                                .with_status_code(StatusCode(200))
                                .with_header(
                                    Header::from_bytes(
                                        &b"Content-Type"[..],
                                        &b"application/json"[..],
                                    )
                                    .unwrap(),
                                ),
                        ))
                    }
                    Err(cql_error) => {
                        let resp = json!({
                            "error": cql_error.message,
                            "error_type": format!("{:?}", cql_error.error_type),
                            "position": cql_error.position,
                            "field": cql_error.field,
                        });
                        let bytes = serde_json::to_vec(&resp).map_err(|e| {
                            StoreError::InvalidInput(format!("json encode error: {e}"))
                        })?;
                        Ok((
                            400,
                            Response::from_data(bytes)
                                .with_status_code(StatusCode(400))
                                .with_header(
                                    Header::from_bytes(
                                        &b"Content-Type"[..],
                                        &b"application/json"[..],
                                    )
                                    .unwrap(),
                                ),
                        ))
                    }
                }
            }
            // Get provenance for a specific context
            (Method::Get, ["v1", "contexts", context_id, "provenance"]) => {
                let context_id: u64 = context_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid context_id".into()))?;

                let mut store = store.lock().unwrap();
                let metadata = store.get_context_metadata(context_id);

                // Get session info for server-side data
                let session = session_tracker.get_session_for_context(context_id);
                let session_peer_addr = session.as_ref().and_then(|s| s.peer_addr.clone());

                let resp = if let Some(ref meta) = metadata {
                    if let Some(ref prov) = meta.provenance {
                        // Inject server-side client_address if not present
                        let mut prov_with_server_info = prov.clone();
                        if prov_with_server_info.client_address.is_none() {
                            prov_with_server_info.client_address = session_peer_addr;
                        }
                        json!({
                            "context_id": context_id.to_string(),
                            "provenance": prov_with_server_info,
                        })
                    } else {
                        json!({
                            "context_id": context_id.to_string(),
                            "provenance": null,
                        })
                    }
                } else {
                    json!({
                        "context_id": context_id.to_string(),
                        "provenance": null,
                    })
                };

                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    200,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            (Method::Get, ["v1", "contexts", context_id, "turns"]) => {
                let context_id: u64 = context_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid context_id".into()))?;
                let params = parse_query(url.query().unwrap_or(""));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(64);
                let before_turn_id = params
                    .get("before_turn_id")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(0);
                let view = params.get("view").map(|v| v.as_str()).unwrap_or("typed");
                let type_hint_mode = params
                    .get("type_hint_mode")
                    .map(|v| v.as_str())
                    .unwrap_or("inherit");

                let bytes_render = match params.get("bytes_render").map(|v| v.as_str()) {
                    Some("hex") => BytesRender::Hex,
                    Some("len_only") => BytesRender::LenOnly,
                    _ => BytesRender::Base64,
                };
                let u64_format = match params.get("u64_format").map(|v| v.as_str()) {
                    Some("string") => U64Format::String,
                    _ => U64Format::Number,
                };
                let enum_render = match params.get("enum_render").map(|v| v.as_str()) {
                    Some("number") => EnumRender::Number,
                    Some("both") => EnumRender::Both,
                    _ => EnumRender::Label,
                };
                let time_render = match params.get("time_render").map(|v| v.as_str()) {
                    Some("unix_ms") => TimeRender::UnixMs,
                    _ => TimeRender::Iso,
                };
                let include_unknown = params
                    .get("include_unknown")
                    .map(|v| v == "1")
                    .unwrap_or(false);

                let as_type_id = params.get("as_type_id").cloned();
                let as_type_version = params
                    .get("as_type_version")
                    .and_then(|v| v.parse::<u32>().ok());

                let options = RenderOptions {
                    bytes_render,
                    u64_format,
                    enum_render,
                    time_render,
                    include_unknown,
                };

                let mut store = store.lock().unwrap();
                let head = store.get_head(context_id)?;
                let t0 = Instant::now();
                let turns = if before_turn_id == 0 {
                    store.get_last(context_id, limit, true)?
                } else {
                    store.get_before(context_id, before_turn_id, limit, true)?
                };
                metrics.record_get_last(t0.elapsed());

                let registry = registry.lock().unwrap();
                let mut out_turns = Vec::new();
                for item in turns.iter() {
                    let declared_type_id = item.meta.declared_type_id.clone();
                    let declared_type_version = item.meta.declared_type_version;

                    let (decoded_type_id, decoded_type_version) = match type_hint_mode {
                        "explicit" => {
                            let id = as_type_id.clone().ok_or_else(|| {
                                StoreError::InvalidInput("as_type_id required".into())
                            })?;
                            let ver = as_type_version.ok_or_else(|| {
                                StoreError::InvalidInput("as_type_version required".into())
                            })?;
                            (id, ver)
                        }
                        "latest" => {
                            let latest = registry
                                .get_latest_type_version(&declared_type_id)
                                .ok_or_else(|| StoreError::NotFound("type descriptor".into()))?;
                            (declared_type_id.clone(), latest.version)
                        }
                        _ => (declared_type_id.clone(), declared_type_version),
                    };

                    let mut turn_obj = Map::new();
                    turn_obj.insert(
                        "turn_id".into(),
                        JsonValue::String(item.record.turn_id.to_string()),
                    );
                    turn_obj.insert(
                        "parent_turn_id".into(),
                        JsonValue::String(item.record.parent_turn_id.to_string()),
                    );
                    turn_obj.insert("depth".into(), JsonValue::Number(item.record.depth.into()));
                    turn_obj.insert(
                        "declared_type".into(),
                        json!({
                            "type_id": declared_type_id,
                            "type_version": declared_type_version,
                        }),
                    );

                    if view == "typed" || view == "both" {
                        let desc = registry
                            .get_type_version(&decoded_type_id, decoded_type_version)
                            .ok_or_else(|| StoreError::NotFound("type descriptor".into()))?;
                        let payload = item
                            .payload
                            .as_ref()
                            .ok_or_else(|| StoreError::InvalidInput("payload not loaded".into()))?;
                        let projected =
                            crate::projection::project_msgpack(payload, desc, &registry, &options)?;
                        turn_obj.insert(
                            "decoded_as".into(),
                            json!({
                                "type_id": decoded_type_id,
                                "type_version": decoded_type_version,
                            }),
                        );
                        turn_obj.insert("data".into(), projected.data);
                        if let Some(unknown) = projected.unknown {
                            turn_obj.insert("unknown".into(), unknown);
                        }
                    }

                    if view == "raw" || view == "both" {
                        let raw_payload = item
                            .payload
                            .as_ref()
                            .ok_or_else(|| StoreError::InvalidInput("payload not loaded".into()))?;
                        turn_obj.insert(
                            "content_hash_b3".into(),
                            JsonValue::String(hex::encode(item.record.payload_hash)),
                        );
                        turn_obj.insert(
                            "encoding".into(),
                            JsonValue::Number(item.meta.encoding.into()),
                        );
                        turn_obj.insert("compression".into(), JsonValue::Number(0u32.into()));
                        turn_obj.insert(
                            "uncompressed_len".into(),
                            JsonValue::Number((raw_payload.len() as u32).into()),
                        );
                        match bytes_render {
                            BytesRender::Base64 => {
                                turn_obj.insert(
                                    "bytes_b64".into(),
                                    JsonValue::String(
                                        base64::engine::general_purpose::STANDARD
                                            .encode(raw_payload),
                                    ),
                                );
                            }
                            BytesRender::Hex => {
                                turn_obj.insert(
                                    "bytes_hex".into(),
                                    JsonValue::String(hex::encode(raw_payload)),
                                );
                            }
                            BytesRender::LenOnly => {
                                turn_obj.insert(
                                    "bytes_len".into(),
                                    JsonValue::Number((raw_payload.len() as u64).into()),
                                );
                            }
                        }
                    }

                    out_turns.push(JsonValue::Object(turn_obj));
                }

                let next_before = turns.first().map(|t| t.record.turn_id.to_string());
                let meta = json!({
                    "context_id": context_id.to_string(),
                    "head_turn_id": head.head_turn_id.to_string(),
                    "head_depth": head.head_depth,
                    "registry_bundle_id": registry.last_bundle_id(),
                });

                let resp = json!({
                    "meta": meta,
                    "turns": out_turns,
                    "next_before_turn_id": next_before,
                });

                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    200,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            (Method::Get, ["v1", "metrics"]) => {
                let mut store = store.lock().unwrap();
                let registry = registry.lock().unwrap();
                let snapshot = metrics.snapshot(&mut store, &registry);
                let bytes = serde_json::to_vec(&snapshot)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    200,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            // Filesystem snapshot: list directory entries
            (Method::Get, ["v1", "turns", turn_id, "fs"]) => {
                let turn_id: u64 = turn_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid turn_id".into()))?;
                let params = parse_query(url.query().unwrap_or(""));
                let path = params.get("path").map(|s| s.as_str()).unwrap_or("");

                let mut store = store.lock().unwrap();

                // Get fs_root for this turn
                let fs_root = store
                    .get_fs_root(turn_id)
                    .ok_or_else(|| StoreError::NotFound("no fs snapshot for turn".into()))?;

                // List entries at the given path
                let entries = store.list_fs_entries(turn_id, path)?;

                let entries_json: Vec<JsonValue> = entries
                    .iter()
                    .map(|e| {
                        let kind_str = match EntryKind::from(e.kind) {
                            EntryKind::File => "file",
                            EntryKind::Directory => "dir",
                            EntryKind::Symlink => "symlink",
                        };
                        json!({
                            "name": e.name,
                            "kind": kind_str,
                            "mode": format!("{:o}", e.mode),
                            "size": e.size,
                            "hash": hex::encode(&e.hash),
                        })
                    })
                    .collect();

                let resp = json!({
                    "turn_id": turn_id.to_string(),
                    "path": path,
                    "fs_root_hash": hex::encode(fs_root),
                    "entries": entries_json,
                });

                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    200,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(200))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            // Filesystem snapshot: get file content or directory listing
            (Method::Get, ["v1", "turns", turn_id, "fs", rest @ ..]) => {
                let turn_id: u64 = turn_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid turn_id".into()))?;
                let path = rest.join("/");

                if path.is_empty() {
                    return Err(StoreError::InvalidInput("empty file path".into()));
                }

                let params = parse_query(url.query().unwrap_or(""));
                let as_json = params.get("format").map(|s| s.as_str()) == Some("json");

                let mut store = store.lock().unwrap();

                // First try to get it as a file
                match store.get_fs_file(turn_id, &path) {
                    Ok((content, entry)) => {
                        if as_json {
                            // Return as JSON with base64 content
                            let kind_str = match EntryKind::from(entry.kind) {
                                EntryKind::File => "file",
                                EntryKind::Directory => "dir",
                                EntryKind::Symlink => "symlink",
                            };
                            let resp = json!({
                                "turn_id": turn_id.to_string(),
                                "path": path,
                                "name": entry.name,
                                "kind": kind_str,
                                "mode": format!("{:o}", entry.mode),
                                "size": entry.size,
                                "hash": hex::encode(&entry.hash),
                                "content_base64": base64::Engine::encode(
                                    &base64::engine::general_purpose::STANDARD,
                                    &content
                                ),
                            });
                            let bytes = serde_json::to_vec(&resp).map_err(|e| {
                                StoreError::InvalidInput(format!("json encode error: {e}"))
                            })?;
                            Ok((
                                200,
                                Response::from_data(bytes)
                                    .with_status_code(StatusCode(200))
                                    .with_header(
                                        Header::from_bytes(
                                            &b"Content-Type"[..],
                                            &b"application/json"[..],
                                        )
                                        .unwrap(),
                                    ),
                            ))
                        } else {
                            // Return raw content
                            let content_type = guess_content_type(&path);
                            Ok((
                                200,
                                Response::from_data(content)
                                    .with_status_code(StatusCode(200))
                                    .with_header(
                                        Header::from_bytes(
                                            &b"Content-Type"[..],
                                            content_type.as_bytes(),
                                        )
                                        .unwrap(),
                                    )
                                    .with_header(
                                        Header::from_bytes(
                                            &b"X-Fs-Hash"[..],
                                            hex::encode(&entry.hash).as_bytes(),
                                        )
                                        .unwrap(),
                                    )
                                    .with_header(
                                        Header::from_bytes(
                                            &b"X-Fs-Mode"[..],
                                            format!("{:o}", entry.mode).as_bytes(),
                                        )
                                        .unwrap(),
                                    ),
                            ))
                        }
                    }
                    Err(StoreError::InvalidInput(msg)) if msg.contains("directory") => {
                        // Path is a directory - return listing instead
                        let fs_root = store.get_fs_root(turn_id).ok_or_else(|| {
                            StoreError::NotFound("no fs snapshot for turn".into())
                        })?;

                        let entries = store.list_fs_entries(turn_id, &path)?;

                        let entries_json: Vec<JsonValue> = entries
                            .iter()
                            .map(|e| {
                                let kind_str = match EntryKind::from(e.kind) {
                                    EntryKind::File => "file",
                                    EntryKind::Directory => "dir",
                                    EntryKind::Symlink => "symlink",
                                };
                                json!({
                                    "name": e.name,
                                    "kind": kind_str,
                                    "mode": format!("{:o}", e.mode),
                                    "size": e.size,
                                    "hash": hex::encode(&e.hash),
                                })
                            })
                            .collect();

                        let resp = json!({
                            "turn_id": turn_id.to_string(),
                            "path": path,
                            "fs_root_hash": hex::encode(fs_root),
                            "entries": entries_json,
                        });

                        let bytes = serde_json::to_vec(&resp).map_err(|e| {
                            StoreError::InvalidInput(format!("json encode error: {e}"))
                        })?;
                        Ok((
                            200,
                            Response::from_data(bytes)
                                .with_status_code(StatusCode(200))
                                .with_header(
                                    Header::from_bytes(
                                        &b"Content-Type"[..],
                                        &b"application/json"[..],
                                    )
                                    .unwrap(),
                                ),
                        ))
                    }
                    Err(e) => Err(e),
                }
            }
            _ => Err(StoreError::NotFound("route".into())),
        }
    })();

    match result {
        Ok((status, response)) => {
            metrics.record_http(status, start.elapsed());
            request.respond(response).map_err(StoreError::Io)
        }
        Err(err) => {
            let (status, message) = map_error(&err);
            metrics.record_http(status, start.elapsed());
            metrics.record_error("http");
            let bytes = serde_json::to_vec(&json!({"error": {"code": status, "message": message}}))
                .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
            let response = Response::from_data(bytes)
                .with_status_code(StatusCode(status))
                .with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
                );
            request.respond(response).map_err(StoreError::Io)
        }
    }
}

/// Handle SSE (Server-Sent Events) stream for /v1/events.
///
/// This function takes ownership of the request and streams events to the client.
/// It spawns a thread to handle the long-lived connection.
fn handle_sse_stream(request: tiny_http::Request, event_bus: &Arc<EventBus>) -> Result<()> {
    let event_bus = Arc::clone(event_bus);

    // Build SSE headers
    let headers = vec![
        Header::from_bytes(&b"Content-Type"[..], &b"text/event-stream"[..]).unwrap(),
        Header::from_bytes(&b"Cache-Control"[..], &b"no-cache"[..]).unwrap(),
        Header::from_bytes(&b"Connection"[..], &b"keep-alive"[..]).unwrap(),
        Header::from_bytes(&b"Access-Control-Allow-Origin"[..], &b"*"[..]).unwrap(),
    ];

    // Create a response with chunked transfer encoding
    // We use an empty data vector and will write to the underlying stream
    let response = Response::empty(200);
    let mut response = response.with_status_code(StatusCode(200));
    for header in headers {
        response = response.with_header(header);
    }

    // Get the raw writer from the request
    // tiny_http's into_writer() takes ownership and returns a Write trait object
    let mut writer = request.into_writer();

    // Write HTTP response headers manually since we're taking raw control
    let status_line = "HTTP/1.1 200 OK\r\n";
    let headers_str = "Content-Type: text/event-stream\r\n\
                       Cache-Control: no-cache\r\n\
                       Connection: keep-alive\r\n\
                       Access-Control-Allow-Origin: *\r\n\
                       Transfer-Encoding: chunked\r\n\r\n";

    if writer.write_all(status_line.as_bytes()).is_err() {
        return Ok(()); // Client disconnected
    }
    if writer.write_all(headers_str.as_bytes()).is_err() {
        return Ok(());
    }
    if writer.flush().is_err() {
        return Ok(());
    }

    // Subscribe to event bus
    let subscriber = event_bus.subscribe();

    // Spawn thread to stream events
    thread::spawn(move || {
        let heartbeat_interval = Duration::from_secs(20);
        let mut last_heartbeat = Instant::now();

        // Send initial connected event
        if write_sse_event(&mut writer, "connected", "{}").is_err() {
            return;
        }

        loop {
            // Check for events with timeout
            match subscriber.recv_timeout(Duration::from_secs(5)) {
                Some(event) => {
                    let (event_type, data) = event.to_sse();
                    if write_sse_event(&mut writer, event_type, &data).is_err() {
                        break; // Connection closed
                    }
                    last_heartbeat = Instant::now();
                }
                None => {
                    // No event, check if we need to send heartbeat
                    if last_heartbeat.elapsed() >= heartbeat_interval {
                        if write_sse_heartbeat(&mut writer).is_err() {
                            break;
                        }
                        last_heartbeat = Instant::now();
                    }
                }
            }
        }
    });

    Ok(())
}

/// Write an SSE event to the stream using chunked encoding.
fn write_sse_event<W: Write>(writer: &mut W, event_type: &str, data: &str) -> std::io::Result<()> {
    let message = format!("event: {}\ndata: {}\n\n", event_type, data);
    let chunk = format!("{:x}\r\n{}\r\n", message.len(), message);
    writer.write_all(chunk.as_bytes())?;
    writer.flush()
}

/// Write an SSE heartbeat comment to keep the connection alive.
fn write_sse_heartbeat<W: Write>(writer: &mut W) -> std::io::Result<()> {
    let message = ":heartbeat\n\n";
    let chunk = format!("{:x}\r\n{}\r\n", message.len(), message);
    writer.write_all(chunk.as_bytes())?;
    writer.flush()
}

fn parse_query(query: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn map_error(err: &StoreError) -> (u16, String) {
    match err {
        StoreError::NotFound(msg) => {
            if msg.contains("type descriptor") {
                (424, msg.clone())
            } else {
                (404, msg.clone())
            }
        }
        StoreError::InvalidInput(msg) => (422, msg.clone()),
        StoreError::Corrupt(msg) => (500, msg.clone()),
        StoreError::Io(msg) => (500, msg.to_string()),
    }
}

fn renderer_spec_to_json(spec: &RendererSpec) -> JsonValue {
    let mut obj = Map::new();
    obj.insert("esm_url".into(), JsonValue::String(spec.esm_url.clone()));
    if let Some(component) = &spec.component {
        obj.insert("component".into(), JsonValue::String(component.clone()));
    }
    if let Some(integrity) = &spec.integrity {
        obj.insert("integrity".into(), JsonValue::String(integrity.clone()));
    }
    JsonValue::Object(obj)
}

fn type_version_to_json(spec: &TypeVersionSpec) -> JsonValue {
    use crate::registry::ItemsSpec;

    let mut fields = Map::new();
    for (tag, field) in spec.fields.iter() {
        let mut obj = Map::new();
        obj.insert("name".into(), JsonValue::String(field.name.clone()));
        obj.insert("type".into(), JsonValue::String(field.field_type.clone()));
        if let Some(enum_ref) = &field.enum_ref {
            obj.insert("enum".into(), JsonValue::String(enum_ref.clone()));
        }
        if let Some(type_ref) = &field.type_ref {
            obj.insert("ref".into(), JsonValue::String(type_ref.clone()));
        }
        if let Some(items) = &field.items {
            match items {
                ItemsSpec::Simple(s) => {
                    obj.insert("items".into(), JsonValue::String(s.clone()));
                }
                ItemsSpec::Ref(r) => {
                    obj.insert("items".into(), json!({"type": "ref", "ref": r}));
                }
            }
        }
        if field.optional {
            obj.insert("optional".into(), JsonValue::Bool(true));
        }
        fields.insert(tag.to_string(), JsonValue::Object(obj));
    }
    let mut result = Map::new();
    result.insert("fields".into(), JsonValue::Object(fields));
    if let Some(renderer) = &spec.renderer {
        result.insert("renderer".into(), renderer_spec_to_json(renderer));
    }
    JsonValue::Object(result)
}

/// Guess content type from file extension.
fn guess_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "rs" => "text/x-rust",
        "go" => "text/x-go",
        "py" => "text/x-python",
        "rb" => "text/x-ruby",
        "java" => "text/x-java",
        "c" | "h" => "text/x-c",
        "cpp" | "cc" | "cxx" | "hpp" => "text/x-c++",
        "ts" => "text/typescript",
        "tsx" => "text/typescript-jsx",
        "jsx" => "text/javascript-jsx",
        "yaml" | "yml" => "text/yaml",
        "toml" => "text/toml",
        "sh" | "bash" => "text/x-shellscript",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        _ => "application/octet-stream",
    }
}
