use anyhow::{anyhow, Context, Result};
use async_stream::stream;
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{HeaderMap, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;
use futures_util::StreamExt;
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, RwLock};
use url::Url;

use crate::delivery::DeliveryHandle;
use crate::ledger::SessionLedgerWriter;
use crate::provider::{anthropic, openai, PreparedExchange, ProviderKind};
use crate::session::SessionRuntime;
use crate::turns::ArtifactRefs;

#[derive(Clone)]
struct ProxyState {
    provider: ProviderKind,
    upstream_base: Url,
    client: reqwest::Client,
    session: SessionRuntime,
    ledger: SessionLedgerWriter,
    delivery: Arc<RwLock<Option<DeliveryHandle>>>,
}

pub struct ProxyServer {
    proxy_base_url: Url,
    state: ProxyState,
    shutdown: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
}

impl ProxyServer {
    pub async fn start(
        provider: ProviderKind,
        upstream_base: Url,
        session: SessionRuntime,
        ledger: SessionLedgerWriter,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .context("failed to construct proxy reqwest client")?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind proxy listener")?;
        let addr = listener
            .local_addr()
            .context("missing proxy listener address")?;
        let proxy_base_url = proxy_base_url(provider, &upstream_base, addr)?;

        let state = ProxyState {
            provider,
            upstream_base,
            client,
            session,
            ledger,
            delivery: Arc::new(RwLock::new(None)),
        };
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let app = Router::new()
            .route("/*path", any(proxy_handler))
            .route("/", any(proxy_handler))
            .with_state(state.clone());
        let join = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Ok(Self {
            proxy_base_url,
            state,
            shutdown: Some(shutdown_tx),
            join,
        })
    }

    pub fn proxy_base_url(&self) -> Url {
        self.proxy_base_url.clone()
    }

    pub async fn set_delivery(&self, delivery: DeliveryHandle) {
        let mut slot = self.state.delivery.write().await;
        *slot = Some(delivery);
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.join
            .await
            .map_err(|err| anyhow!("proxy server task failed: {err}"))?;
        Ok(())
    }
}

async fn proxy_handler(State(state): State<ProxyState>, request: Request<Body>) -> Response<Body> {
    match handle_proxy_request(state, request).await {
        Ok(response) => response,
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            format!("cxtx proxy error: {err:#}"),
        )
            .into_response(),
    }
}

async fn handle_proxy_request(state: ProxyState, request: Request<Body>) -> Result<Response<Body>> {
    let delivery = state
        .delivery
        .read()
        .await
        .clone()
        .ok_or_else(|| anyhow!("delivery worker not attached"))?;

    let (parts, body) = request.into_parts();
    let body_bytes = to_bytes(body, usize::MAX)
        .await
        .context("failed to read downstream request body")?;
    let upstream_url = state
        .provider
        .build_upstream_url(&state.upstream_base, &parts.uri)
        .context("failed to derive upstream URL")?;
    let request_headers = forwardable_headers(&parts.headers);
    let request_content_type = header_value(&parts.headers, "content-type");
    let request_json = request_content_type
        .as_deref()
        .filter(|content_type| content_type.contains("json"))
        .and_then(|_| serde_json::from_slice::<Value>(&body_bytes).ok());

    let exchange_id = state.session.next_exchange_id();
    let request_artifact = state
        .ledger
        .record_request(
            &exchange_id,
            parts.uri.path(),
            request_json
                .as_ref()
                .and_then(|payload| state.provider.model_from_payload(payload))
                .as_deref(),
            request_content_type.as_deref(),
            &body_bytes,
            request_json.as_ref(),
        )
        .await?;
    let artifact_refs = ArtifactRefs::default().with_request_path(Some(request_artifact));
    let prepared = state.provider.prepare_exchange(
        &state.session,
        exchange_id.clone(),
        &body_bytes,
        &artifact_refs,
    );
    enqueue_turns(&delivery, prepared.request_turns.clone()).await;

    let mut upstream_request = state.client.request(parts.method.clone(), upstream_url);
    upstream_request = upstream_request.body(body_bytes.to_vec());
    for (name, value) in request_headers {
        upstream_request = upstream_request.header(name, value);
    }

    let upstream_response = match upstream_request.send().await {
        Ok(response) => response,
        Err(err) => {
            delivery
                .enqueue_turn(state.session.provider_error_turn(
                    &exchange_id,
                    "upstream_transport_error",
                    &format!("upstream request failed: {err}"),
                    None,
                    &artifact_refs,
                ))
                .await
                .ok();
            return Ok((
                StatusCode::BAD_GATEWAY,
                format!("upstream request failed: {err}"),
            )
                .into_response());
        }
    };

    let status = StatusCode::from_u16(upstream_response.status().as_u16())
        .unwrap_or(StatusCode::BAD_GATEWAY);
    let response_headers = upstream_response.headers().clone();
    let request_id = state.provider.request_id_from_headers(&response_headers);

    if header_value_reqwest(&response_headers, "content-type")
        .as_deref()
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false)
    {
        stream_response(
            state,
            delivery,
            status,
            response_headers,
            upstream_response,
            request_id,
            prepared,
            artifact_refs,
        )
        .await
    } else {
        body_response(
            state,
            delivery,
            status,
            response_headers,
            upstream_response,
            request_id,
            prepared,
            artifact_refs,
        )
        .await
    }
}

async fn body_response(
    state: ProxyState,
    delivery: DeliveryHandle,
    status: StatusCode,
    response_headers: reqwest::header::HeaderMap,
    upstream_response: reqwest::Response,
    request_id: Option<String>,
    prepared: PreparedExchange,
    artifact_refs: ArtifactRefs,
) -> Result<Response<Body>> {
    let body = upstream_response
        .bytes()
        .await
        .context("failed to read upstream response body")?;
    let content_type = header_value_reqwest(&response_headers, "content-type");
    let response_json = content_type
        .as_deref()
        .filter(|content_type| content_type.contains("json"))
        .and_then(|_| serde_json::from_slice::<Value>(&body).ok());
    let response_artifact = state
        .ledger
        .record_response(
            &prepared.exchange_id,
            status.as_u16(),
            request_id.as_deref(),
            content_type.as_deref(),
            &body,
            response_json.as_ref(),
        )
        .await?;
    let artifact_refs = artifact_refs.with_response_path(Some(response_artifact));
    let turns = prepared.state.finalize_json(
        &state.session,
        status.as_u16(),
        request_id,
        &body,
        &artifact_refs,
    );
    enqueue_turns(&delivery, turns).await;

    let mut response = Response::builder().status(status);
    for (name, value) in copyable_response_headers(&response_headers) {
        response = response.header(name, value);
    }
    response
        .body(Body::from(body))
        .context("failed to build proxied response")
}

async fn stream_response(
    state: ProxyState,
    delivery: DeliveryHandle,
    status: StatusCode,
    response_headers: reqwest::header::HeaderMap,
    upstream_response: reqwest::Response,
    request_id: Option<String>,
    prepared: PreparedExchange,
    artifact_refs: ArtifactRefs,
) -> Result<Response<Body>> {
    let provider = state.provider;
    let session = state.session.clone();
    let ledger = state.ledger.clone();
    let exchange_id = prepared.exchange_id.clone();
    let content_type = header_value_reqwest(&response_headers, "content-type");
    let mut exchange_state = prepared.state;
    let mut stream_artifact_refs = artifact_refs.clone();
    let request_id_for_stream = request_id.clone();
    let raw_stream = String::new();
    let stream = upstream_response.bytes_stream();
    let stream = stream! {
        let mut parse_buffer = String::new();
        let mut raw_stream = raw_stream;
        futures_util::pin_mut!(stream);
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    let text = String::from_utf8_lossy(&chunk);
                    raw_stream.push_str(&text);
                    parse_buffer.push_str(&text);
                    let frames = match provider {
                        ProviderKind::Codex => openai::parse_sse_buffer(&mut parse_buffer),
                        ProviderKind::Claude => anthropic::parse_sse_buffer(&mut parse_buffer),
                    };
                    for frame in frames {
                        match ledger.append_stream_frame(&exchange_id, &frame.raw).await {
                            Ok(path) => {
                                stream_artifact_refs.stream_path = Some(path);
                            }
                            Err(err) => {
                                delivery.enqueue_turn(session.provider_error_turn(
                                    &exchange_id,
                                    "artifact_write_error",
                                    &format!("failed to persist stream frame: {err}"),
                                    request_id_for_stream.as_deref(),
                                    &stream_artifact_refs,
                                )).await.ok();
                            }
                        }
                        exchange_state.absorb_sse_frame(&frame);
                    }
                    yield Ok::<bytes::Bytes, std::io::Error>(chunk);
                }
                Err(err) => {
                    delivery.enqueue_turn(session.provider_error_turn(
                        &exchange_id,
                        "stream_transport_error",
                        &format!("failed to read upstream stream: {err}"),
                        request_id_for_stream.as_deref(),
                        &stream_artifact_refs,
                    )).await.ok();
                    break;
                }
            }
        }

        match ledger.record_response(
            &exchange_id,
            status.as_u16(),
            request_id_for_stream.as_deref(),
            content_type.as_deref(),
            raw_stream.as_bytes(),
            None,
        ).await {
            Ok(path) => {
                stream_artifact_refs.response_path = Some(path);
            }
            Err(err) => {
                delivery.enqueue_turn(session.provider_error_turn(
                    &exchange_id,
                    "artifact_write_error",
                    &format!("failed to persist streamed response transcript: {err}"),
                    request_id_for_stream.as_deref(),
                    &stream_artifact_refs,
                )).await.ok();
            }
        }

        let turns = exchange_state.finalize_stream(
            &session,
            status.as_u16(),
            request_id_for_stream.clone(),
            &stream_artifact_refs,
            if parse_buffer.trim().is_empty() {
                None
            } else {
                Some(parse_buffer)
            },
        );
        enqueue_turns(&delivery, turns).await;
    };

    let mut response = Response::builder().status(status);
    for (name, value) in copyable_response_headers(&response_headers) {
        response = response.header(name, value);
    }
    response
        .body(Body::from_stream(stream))
        .context("failed to build streaming proxied response")
}

async fn enqueue_turns(delivery: &DeliveryHandle, turns: Vec<crate::turns::TurnEnvelope>) {
    for turn in turns {
        delivery.enqueue_turn(turn).await.ok();
    }
}

fn proxy_base_url(provider: ProviderKind, upstream_base: &Url, addr: SocketAddr) -> Result<Url> {
    let mut url = Url::parse(&format!("http://{addr}")).context("failed to build proxy URL")?;
    let mount_path = provider.proxy_mount_path(upstream_base);
    url.set_path(&mount_path);
    Ok(url)
}

fn forwardable_headers(headers: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            if is_hop_by_hop(name.as_str()) {
                None
            } else {
                Some((name.clone(), value.clone()))
            }
        })
        .collect()
}

fn copyable_response_headers(
    headers: &reqwest::header::HeaderMap,
) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            if is_hop_by_hop(name.as_str()) {
                None
            } else {
                Some((name.clone(), value.clone()))
            }
        })
        .collect()
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn header_value_reqwest(headers: &reqwest::header::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}
