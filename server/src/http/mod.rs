// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use rmpv::Value as MsgpackValue;
use serde_json::{json, Map, Value as JsonValue};
use tiny_http::{Header, Method, Response, Server, StatusCode};
use url::Url;

use crate::error::{Result, StoreError};
use crate::events::{EventBus, StoreEvent};
use crate::fs_store::EntryKind;
use crate::metrics::{Metrics, SessionTracker};
use crate::projection::{BytesRender, EnumRender, RenderOptions, TimeRender, U64Format};
use crate::registry::{
    FieldSpec, ItemsSpec, PutOutcome, Registry, RegistryBundle, RendererSpec, TypeVersionSpec,
};
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
    let request_path = request.url().to_string();

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
                let include_lineage = params
                    .get("include_lineage")
                    .map(|v| v == "1")
                    .unwrap_or(false);

                let mut store = store.lock().unwrap();
                let contexts = store.list_recent_contexts(limit);

                let contexts_json: Vec<JsonValue> = contexts
                    .iter()
                    .filter_map(|c| {
                        let obj = context_to_json(
                            &mut store,
                            session_tracker,
                            c.context_id,
                            include_provenance,
                            include_lineage,
                        )
                        .ok()?;

                        let client_tag = obj.get("client_tag").and_then(|v| v.as_str());
                        if let Some(ref filter) = tag_filter {
                            let tag = client_tag.unwrap_or("");
                            if tag != filter {
                                return None;
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
            (Method::Post, ["v1", "contexts"]) => {
                let base_turn_id = parse_base_turn_id(&mut request, 0, false)?;
                let client_tag = extract_http_client_tag(&request);

                let head = {
                    let mut store = store.lock().unwrap();
                    store.create_context(base_turn_id)?
                };

                event_bus.publish(StoreEvent::ContextCreated {
                    context_id: head.context_id.to_string(),
                    session_id: "http".to_string(),
                    client_tag,
                    created_at: unix_ms(),
                });

                let resp = json!({
                    "context_id": head.context_id.to_string(),
                    "head_turn_id": head.head_turn_id.to_string(),
                    "head_depth": head.head_depth,
                });
                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    201,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(201))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            (Method::Post, ["v1", "contexts", "create"]) => {
                let base_turn_id = parse_base_turn_id(&mut request, 0, false)?;
                let client_tag = extract_http_client_tag(&request);

                let head = {
                    let mut store = store.lock().unwrap();
                    store.create_context(base_turn_id)?
                };

                event_bus.publish(StoreEvent::ContextCreated {
                    context_id: head.context_id.to_string(),
                    session_id: "http".to_string(),
                    client_tag,
                    created_at: unix_ms(),
                });

                let resp = json!({
                    "context_id": head.context_id.to_string(),
                    "head_turn_id": head.head_turn_id.to_string(),
                    "head_depth": head.head_depth,
                });
                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    201,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(201))
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                                .unwrap(),
                        ),
                ))
            }
            (Method::Post, ["v1", "contexts", "fork"]) => {
                let base_turn_id = parse_base_turn_id(&mut request, 0, true)?;
                let client_tag = extract_http_client_tag(&request);

                let head = {
                    let mut store = store.lock().unwrap();
                    store.fork_context(base_turn_id)?
                };

                event_bus.publish(StoreEvent::ContextCreated {
                    context_id: head.context_id.to_string(),
                    session_id: "http".to_string(),
                    client_tag,
                    created_at: unix_ms(),
                });

                let resp = json!({
                    "context_id": head.context_id.to_string(),
                    "head_turn_id": head.head_turn_id.to_string(),
                    "head_depth": head.head_depth,
                });
                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    201,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(201))
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
            // Get context details
            (Method::Get, ["v1", "contexts", context_id]) => {
                let context_id: u64 = context_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid context_id".into()))?;
                let params = parse_query(url.query().unwrap_or(""));
                let include_provenance = params
                    .get("include_provenance")
                    .map(|v| v == "1")
                    .unwrap_or(true);
                let include_lineage = params
                    .get("include_lineage")
                    .map(|v| v == "1")
                    .unwrap_or(true);

                let mut store = store.lock().unwrap();
                let obj = context_to_json(
                    &mut store,
                    session_tracker,
                    context_id,
                    include_provenance,
                    include_lineage,
                )?;

                let bytes = serde_json::to_vec(&obj)
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
            // Get children/descendants for a specific context
            (Method::Get, ["v1", "contexts", context_id, "children"]) => {
                let context_id: u64 = context_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid context_id".into()))?;
                let params = parse_query(url.query().unwrap_or(""));
                let recursive = params
                    .get("recursive")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let include_provenance = params
                    .get("include_provenance")
                    .map(|v| v == "1")
                    .unwrap_or(true);
                let include_lineage = params
                    .get("include_lineage")
                    .map(|v| v == "1")
                    .unwrap_or(true);
                let limit = params
                    .get("limit")
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(256);

                let mut store = store.lock().unwrap();
                // Validate parent context exists
                store.get_head(context_id)?;

                let child_ids = if recursive {
                    store.descendant_context_ids(context_id, Some(limit))
                } else {
                    let mut ids = store.child_context_ids(context_id);
                    ids.truncate(limit as usize);
                    ids
                };

                let children: Vec<JsonValue> = child_ids
                    .iter()
                    .filter_map(|child_id| {
                        context_to_json(
                            &mut store,
                            session_tracker,
                            *child_id,
                            include_provenance,
                            include_lineage,
                        )
                        .ok()
                    })
                    .collect();

                let resp = json!({
                    "context_id": context_id.to_string(),
                    "recursive": recursive,
                    "count": children.len(),
                    "children": children,
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
            // Get provenance for a specific context
            (Method::Get, ["v1", "contexts", context_id, "provenance"]) => {
                let context_id: u64 = context_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid context_id".into()))?;

                let mut store = store.lock().unwrap();
                store.get_head(context_id)?;
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
            (Method::Post, ["v1", "contexts", context_id, "append"])
            | (Method::Post, ["v1", "contexts", context_id, "turns"]) => {
                let context_id: u64 = context_id
                    .parse()
                    .map_err(|_| StoreError::InvalidInput("invalid context_id".into()))?;

                let body = parse_json_body(&mut request)?;
                let type_id = get_required_string(&body, "type_id")?;
                let type_version = get_required_u32(&body, "type_version")?;
                let parent_turn_id = get_optional_u64(&body, "parent_turn_id")?.unwrap_or(0);
                let payload_json = body
                    .get("data")
                    .or_else(|| body.get("payload"))
                    .ok_or_else(|| {
                        StoreError::InvalidInput("missing required field: data or payload".into())
                    })?;

                let payload_bytes = {
                    let registry = registry.lock().unwrap();
                    encode_http_payload(payload_json, &type_id, type_version, &registry)?
                };

                let hash = blake3::hash(&payload_bytes);
                let (record, metadata) = {
                    let mut store = store.lock().unwrap();
                    store.append_turn(
                        context_id,
                        parent_turn_id,
                        type_id.clone(),
                        type_version,
                        1, // msgpack
                        0, // uncompressed
                        payload_bytes.len() as u32,
                        *hash.as_bytes(),
                        &payload_bytes,
                    )?
                };

                event_bus.publish(StoreEvent::TurnAppended {
                    context_id: context_id.to_string(),
                    turn_id: record.turn_id.to_string(),
                    parent_turn_id: record.parent_turn_id.to_string(),
                    depth: record.depth,
                    declared_type_id: Some(type_id.clone()),
                    declared_type_version: Some(type_version),
                });

                if let Some(meta) = metadata {
                    event_bus.publish(StoreEvent::ContextMetadataUpdated {
                        context_id: context_id.to_string(),
                        client_tag: meta.client_tag,
                        title: meta.title,
                        labels: meta.labels,
                        has_provenance: meta.provenance.is_some(),
                    });

                    if let Some(prov) = meta.provenance {
                        if let Some(parent_context_id) = prov.parent_context_id {
                            event_bus.publish(StoreEvent::ContextLinked {
                                child_context_id: context_id.to_string(),
                                parent_context_id: parent_context_id.to_string(),
                                root_context_id: prov.root_context_id.map(|v| v.to_string()),
                                spawn_reason: prov.spawn_reason,
                            });
                        }
                    }
                }

                let resp = json!({
                    "context_id": context_id.to_string(),
                    "turn_id": record.turn_id.to_string(),
                    "depth": record.depth,
                    "content_hash": hex::encode(hash.as_bytes()),
                });
                let bytes = serde_json::to_vec(&resp)
                    .map_err(|e| StoreError::InvalidInput(format!("json encode error: {e}")))?;
                Ok((
                    201,
                    Response::from_data(bytes)
                        .with_status_code(StatusCode(201))
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
                    Some("number") => U64Format::Number,
                    _ => U64Format::String,
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
            (Method::Get, ["v1", "errors"]) => {
                let params = parse_query(url.query().unwrap_or(""));
                let limit: usize = params
                    .get("limit")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(50)
                    .min(256);
                let entries = metrics.recent_errors(limit);
                let bytes = serde_json::to_vec(&json!({ "errors": entries }))
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
            metrics.record_error("http", status, &message, Some(&request_path));
            event_bus.publish(StoreEvent::ErrorOccurred {
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                kind: "http".to_string(),
                status_code: status,
                message: message.clone(),
                path: Some(request_path.clone()),
            });
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

fn context_to_json(
    store: &mut Store,
    session_tracker: &SessionTracker,
    context_id: u64,
    include_provenance: bool,
    include_lineage: bool,
) -> Result<JsonValue> {
    let head = store.get_head(context_id)?;
    let session = session_tracker.get_session_for_context(context_id);
    let session_id = session.as_ref().map(|s| s.session_id);
    let is_live = session.is_some();
    let last_activity_at = session.as_ref().map(|s| s.last_activity_at);
    let session_peer_addr = session.as_ref().and_then(|s| s.peer_addr.clone());

    let stored_metadata = store.get_context_metadata(context_id);
    let client_tag = stored_metadata
        .as_ref()
        .and_then(|m| m.client_tag.clone())
        .or_else(|| session.as_ref().map(|s| s.client_tag.clone()))
        .filter(|t| !t.is_empty());

    let mut obj = json!({
        "context_id": head.context_id.to_string(),
        "head_turn_id": head.head_turn_id.to_string(),
        "head_depth": head.head_depth,
        "created_at_unix_ms": head.created_at_unix_ms,
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
    if let Some(metadata) = &stored_metadata {
        if let Some(title) = &metadata.title {
            obj["title"] = JsonValue::String(title.clone());
        }
        if let Some(labels) = &metadata.labels {
            obj["labels"] = serde_json::to_value(labels).unwrap_or(JsonValue::Null);
        }
    }

    if include_provenance {
        if let Some(ref metadata) = stored_metadata {
            if let Some(ref prov) = metadata.provenance {
                let mut prov_with_server_info = prov.clone();
                if prov_with_server_info.client_address.is_none() {
                    prov_with_server_info.client_address = session_peer_addr.clone();
                }
                if let Ok(prov_json) = serde_json::to_value(&prov_with_server_info) {
                    obj["provenance"] = prov_json;
                }
            }
        }
    }

    if include_lineage {
        let (parent_context_id, root_context_id, spawn_reason) = stored_metadata
            .as_ref()
            .and_then(|m| m.provenance.as_ref())
            .map(|p| {
                (
                    p.parent_context_id,
                    p.root_context_id,
                    p.spawn_reason.clone(),
                )
            })
            .unwrap_or((None, None, None));

        let child_context_ids = store.child_context_ids(context_id);
        let child_context_ids_json: Vec<JsonValue> = child_context_ids
            .iter()
            .map(|id| JsonValue::String(id.to_string()))
            .collect();

        obj["lineage"] = json!({
            "parent_context_id": parent_context_id.map(|v| v.to_string()),
            "root_context_id": root_context_id.map(|v| v.to_string()),
            "spawn_reason": spawn_reason,
            "child_context_count": child_context_ids.len(),
            "child_context_ids": child_context_ids_json,
        });
    }

    Ok(obj)
}

fn parse_base_turn_id(
    request: &mut tiny_http::Request,
    default: u64,
    required: bool,
) -> Result<u64> {
    let body = parse_json_body(request)?;
    if let Some(value) = body.get("base_turn_id") {
        parse_json_u64(value, "base_turn_id")
    } else if required {
        Err(StoreError::InvalidInput(
            "missing required field: base_turn_id".into(),
        ))
    } else {
        Ok(default)
    }
}

fn parse_json_body(request: &mut tiny_http::Request) -> Result<JsonValue> {
    let mut body = Vec::new();
    request.as_reader().read_to_end(&mut body)?;
    if body.is_empty() {
        return Ok(JsonValue::Object(Map::new()));
    }
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(JsonValue::Object(Map::new()));
    }
    serde_json::from_slice(&body)
        .map_err(|e| StoreError::InvalidInput(format!("invalid json: {e}")))
}

fn parse_json_u64(value: &JsonValue, field_name: &str) -> Result<u64> {
    match value {
        JsonValue::String(s) => s
            .parse::<u64>()
            .map_err(|_| StoreError::InvalidInput(format!("invalid {field_name}"))),
        JsonValue::Number(n) => n
            .as_u64()
            .or_else(|| {
                n.as_i64()
                    .and_then(|i| if i >= 0 { Some(i as u64) } else { None })
            })
            .ok_or_else(|| StoreError::InvalidInput(format!("invalid {field_name}"))),
        _ => Err(StoreError::InvalidInput(format!("invalid {field_name}"))),
    }
}

fn get_required_string(body: &JsonValue, key: &str) -> Result<String> {
    body.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| StoreError::InvalidInput(format!("missing required field: {key}")))
}

fn get_required_u32(body: &JsonValue, key: &str) -> Result<u32> {
    let value = body
        .get(key)
        .ok_or_else(|| StoreError::InvalidInput(format!("missing required field: {key}")))?;
    let parsed = parse_json_u64(value, key)?;
    if parsed > u32::MAX as u64 {
        return Err(StoreError::InvalidInput(format!("{key} out of range")));
    }
    Ok(parsed as u32)
}

fn get_optional_u64(body: &JsonValue, key: &str) -> Result<Option<u64>> {
    match body.get(key) {
        Some(value) => parse_json_u64(value, key).map(Some),
        None => Ok(None),
    }
}

fn extract_http_client_tag(request: &tiny_http::Request) -> String {
    for name in ["X-CXDB-Client-Tag", "X-Client-Tag"] {
        if let Some(header) = request.headers().iter().find(|h| h.field.equiv(name)) {
            let value = header.value.as_str().trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "http".to_string()
}

fn encode_http_payload(
    payload_json: &JsonValue,
    type_id: &str,
    type_version: u32,
    registry: &Registry,
) -> Result<Vec<u8>> {
    if let Some(desc) = registry.get_type_version(type_id, type_version) {
        if let JsonValue::Object(obj) = payload_json {
            let value = encode_object_with_descriptor(obj, desc, registry)?;
            let mut out = Vec::new();
            rmpv::encode::write_value(&mut out, &value)
                .map_err(|e| StoreError::InvalidInput(format!("msgpack encode error: {e}")))?;
            return Ok(out);
        }
    }

    let value = json_to_msgpack_value(payload_json)?;
    let mut out = Vec::new();
    rmpv::encode::write_value(&mut out, &value)
        .map_err(|e| StoreError::InvalidInput(format!("msgpack encode error: {e}")))?;
    Ok(out)
}

fn encode_object_with_descriptor(
    obj: &Map<String, JsonValue>,
    desc: &TypeVersionSpec,
    registry: &Registry,
) -> Result<MsgpackValue> {
    let mut entries: Vec<(MsgpackValue, MsgpackValue)> = Vec::new();

    let mut tagged_fields: Vec<(u64, &FieldSpec)> =
        desc.fields.iter().map(|(t, f)| (*t, f)).collect();
    tagged_fields.sort_unstable_by_key(|(tag, _)| *tag);

    for (tag, field) in &tagged_fields {
        if let Some(value) = obj.get(&field.name) {
            entries.push((
                MsgpackValue::from(*tag),
                encode_field_value(value, field, registry)?,
            ));
        } else if !field.optional {
            return Err(StoreError::InvalidInput(format!(
                "missing required field: {}",
                field.name
            )));
        }
    }

    // Preserve unknown fields instead of silently dropping them.
    for (key, value) in obj {
        if tagged_fields
            .iter()
            .any(|(_, field)| field.name.as_str() == key.as_str())
        {
            continue;
        }
        let key_value = key
            .parse::<u64>()
            .map(MsgpackValue::from)
            .unwrap_or_else(|_| MsgpackValue::String(key.clone().into()));
        entries.push((key_value, json_to_msgpack_value(value)?));
    }

    Ok(MsgpackValue::Map(entries))
}

fn encode_field_value(
    value: &JsonValue,
    field: &FieldSpec,
    registry: &Registry,
) -> Result<MsgpackValue> {
    if value.is_null() {
        return Ok(MsgpackValue::Nil);
    }

    if let Some(enum_ref) = &field.enum_ref {
        if let Some(num) = parse_json_u64_opt(value) {
            return Ok(MsgpackValue::from(num));
        }
        if let Some(label) = value.as_str() {
            if let Some(enum_map) = registry.get_enum(enum_ref) {
                if let Some((num_str, _)) = enum_map.iter().find(|(_, v)| v.as_str() == label) {
                    let num = num_str.parse::<u64>().map_err(|_| {
                        StoreError::InvalidInput(format!("invalid enum value for {}", field.name))
                    })?;
                    return Ok(MsgpackValue::from(num));
                }
            }
            return Err(StoreError::InvalidInput(format!(
                "unknown enum label for {}",
                field.name
            )));
        }
    }

    match field.field_type.as_str() {
        "string" => value
            .as_str()
            .map(|s| MsgpackValue::String(s.to_string().into()))
            .ok_or_else(|| StoreError::InvalidInput(format!("expected string for {}", field.name))),
        "bool" => value
            .as_bool()
            .map(MsgpackValue::Boolean)
            .ok_or_else(|| StoreError::InvalidInput(format!("expected bool for {}", field.name))),
        "u64" | "uint64" | "u32" | "uint32" | "u8" | "uint8" => parse_json_u64_opt(value)
            .map(MsgpackValue::from)
            .ok_or_else(|| {
                StoreError::InvalidInput(format!("expected integer for {}", field.name))
            }),
        "int64" | "int32" => parse_json_i64_opt(value)
            .map(MsgpackValue::from)
            .ok_or_else(|| {
                StoreError::InvalidInput(format!("expected integer for {}", field.name))
            }),
        "bytes" | "typed_blob" => parse_bytes_value(value).map(MsgpackValue::Binary),
        "array" => {
            let items = value.as_array().ok_or_else(|| {
                StoreError::InvalidInput(format!("expected array for {}", field.name))
            })?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let encoded = match &field.items {
                    Some(ItemsSpec::Simple(item_type)) => encode_value_for_type(item, item_type)?,
                    Some(ItemsSpec::Ref(type_ref)) => encode_ref_value(item, type_ref, registry)?,
                    None => json_to_msgpack_value(item)?,
                };
                out.push(encoded);
            }
            Ok(MsgpackValue::Array(out))
        }
        "ref" => {
            if let Some(type_ref) = &field.type_ref {
                encode_ref_value(value, type_ref, registry)
            } else {
                json_to_msgpack_value(value)
            }
        }
        _ => encode_value_for_type(value, &field.field_type),
    }
}

fn encode_ref_value(
    value: &JsonValue,
    type_ref: &str,
    registry: &Registry,
) -> Result<MsgpackValue> {
    let obj = value
        .as_object()
        .ok_or_else(|| StoreError::InvalidInput(format!("expected object for ref {type_ref}")))?;
    let desc = registry
        .get_latest_type_version(type_ref)
        .ok_or_else(|| StoreError::NotFound("type descriptor".into()))?;
    encode_object_with_descriptor(obj, desc, registry)
}

fn encode_value_for_type(value: &JsonValue, field_type: &str) -> Result<MsgpackValue> {
    match field_type {
        "string" => value
            .as_str()
            .map(|s| MsgpackValue::String(s.to_string().into()))
            .ok_or_else(|| StoreError::InvalidInput("expected string".into())),
        "bool" => value
            .as_bool()
            .map(MsgpackValue::Boolean)
            .ok_or_else(|| StoreError::InvalidInput("expected bool".into())),
        "u64" | "uint64" | "u32" | "uint32" | "u8" | "uint8" => parse_json_u64_opt(value)
            .map(MsgpackValue::from)
            .ok_or_else(|| StoreError::InvalidInput("expected integer".into())),
        "int64" | "int32" | "unix_ms" | "time_ms" | "timestamp_ms" => parse_json_i64_opt(value)
            .map(MsgpackValue::from)
            .ok_or_else(|| StoreError::InvalidInput("expected integer".into())),
        "bytes" | "typed_blob" => parse_bytes_value(value).map(MsgpackValue::Binary),
        _ => json_to_msgpack_value(value),
    }
}

fn parse_json_u64_opt(value: &JsonValue) -> Option<u64> {
    match value {
        JsonValue::String(s) => s.parse::<u64>().ok(),
        JsonValue::Number(n) => n.as_u64().or_else(|| {
            n.as_i64()
                .and_then(|i| if i >= 0 { Some(i as u64) } else { None })
        }),
        _ => None,
    }
}

fn parse_json_i64_opt(value: &JsonValue) -> Option<i64> {
    match value {
        JsonValue::String(s) => s.parse::<i64>().ok(),
        JsonValue::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok())),
        _ => None,
    }
}

fn parse_bytes_value(value: &JsonValue) -> Result<Vec<u8>> {
    match value {
        JsonValue::String(s) => {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(s) {
                return Ok(bytes);
            }
            if let Ok(bytes) = hex::decode(s) {
                return Ok(bytes);
            }
            Ok(s.as_bytes().to_vec())
        }
        JsonValue::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                let num = parse_json_u64_opt(item)
                    .ok_or_else(|| StoreError::InvalidInput("invalid byte array value".into()))?;
                if num > 255 {
                    return Err(StoreError::InvalidInput("byte out of range".into()));
                }
                out.push(num as u8);
            }
            Ok(out)
        }
        JsonValue::Object(obj) => {
            if let Some(JsonValue::String(b64)) = obj.get("base64") {
                return base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| StoreError::InvalidInput(format!("invalid base64 bytes: {e}")));
            }
            if let Some(JsonValue::String(hex_str)) = obj.get("hex") {
                return hex::decode(hex_str)
                    .map_err(|e| StoreError::InvalidInput(format!("invalid hex bytes: {e}")));
            }
            Err(StoreError::InvalidInput(
                "bytes object must contain base64 or hex".into(),
            ))
        }
        _ => Err(StoreError::InvalidInput("invalid bytes value".into())),
    }
}

fn json_to_msgpack_value(value: &JsonValue) -> Result<MsgpackValue> {
    match value {
        JsonValue::Null => Ok(MsgpackValue::Nil),
        JsonValue::Bool(v) => Ok(MsgpackValue::Boolean(*v)),
        JsonValue::Number(n) => {
            if let Some(v) = n.as_i64() {
                Ok(MsgpackValue::from(v))
            } else if let Some(v) = n.as_u64() {
                Ok(MsgpackValue::from(v))
            } else if let Some(v) = n.as_f64() {
                Ok(MsgpackValue::from(v))
            } else {
                Err(StoreError::InvalidInput("invalid numeric value".into()))
            }
        }
        JsonValue::String(s) => Ok(MsgpackValue::String(s.clone().into())),
        JsonValue::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(json_to_msgpack_value(item)?);
            }
            Ok(MsgpackValue::Array(out))
        }
        JsonValue::Object(obj) => {
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            let mut out = Vec::with_capacity(obj.len());
            for key in keys {
                let key_value = key
                    .parse::<u64>()
                    .map(MsgpackValue::from)
                    .unwrap_or_else(|_| MsgpackValue::String(key.clone().into()));
                let value = obj
                    .get(key)
                    .ok_or_else(|| StoreError::InvalidInput("missing object key".into()))?;
                out.push((key_value, json_to_msgpack_value(value)?));
            }
            Ok(MsgpackValue::Map(out))
        }
    }
}

fn parse_query(query: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn map_error(err: &StoreError) -> (u16, String) {
    match err {
        StoreError::NotFound(msg) => {
            if msg.contains("type descriptor") {
                (424, msg.clone())
            } else if msg.contains("parent turn") || msg.contains("base turn") {
                (409, msg.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn encode_http_payload_uses_registry_tags_when_descriptor_exists() {
        let dir = tempdir().expect("tempdir");
        let mut registry = Registry::open(dir.path()).expect("open registry");
        let bundle = serde_json::json!({
            "registry_version": 1,
            "bundle_id": "test-bundle#1",
            "types": {
                "com.example.Message": {
                    "versions": {
                        "1": {
                            "fields": {
                                "1": { "name": "role", "type": "string" },
                                "2": { "name": "text", "type": "string" }
                            }
                        }
                    }
                }
            }
        });
        let raw = serde_json::to_vec(&bundle).expect("bundle json");
        registry
            .put_bundle("test-bundle#1", &raw)
            .expect("put bundle");

        let payload = serde_json::json!({
            "role": "user",
            "text": "hello",
        });
        let encoded = encode_http_payload(&payload, "com.example.Message", 1, &registry)
            .expect("encode payload");

        let value =
            rmpv::decode::read_value(&mut std::io::Cursor::new(&encoded)).expect("decode msgpack");
        let map = match value {
            MsgpackValue::Map(m) => m,
            other => panic!("expected map, got {other:?}"),
        };

        assert!(map
            .iter()
            .any(|(k, v)| *k == MsgpackValue::from(1) && *v == MsgpackValue::from("user")));
        assert!(map
            .iter()
            .any(|(k, v)| *k == MsgpackValue::from(2) && *v == MsgpackValue::from("hello")));
    }

    #[test]
    fn encode_http_payload_falls_back_to_plain_json_shape_without_descriptor() {
        let dir = tempdir().expect("tempdir");
        let registry = Registry::open(dir.path()).expect("open registry");
        let payload = serde_json::json!({
            "role": "user",
            "text": "hello",
        });

        let encoded =
            encode_http_payload(&payload, "com.example.UnknownType", 1, &registry).expect("encode");

        let value =
            rmpv::decode::read_value(&mut std::io::Cursor::new(&encoded)).expect("decode msgpack");
        let map = match value {
            MsgpackValue::Map(m) => m,
            other => panic!("expected map, got {other:?}"),
        };

        assert!(map.iter().any(|(k, v)| {
            *k == MsgpackValue::from("role") && *v == MsgpackValue::from("user")
        }));
        assert!(map.iter().any(|(k, v)| {
            *k == MsgpackValue::from("text") && *v == MsgpackValue::from("hello")
        }));
    }
}
