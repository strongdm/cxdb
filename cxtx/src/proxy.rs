use anyhow::{anyhow, Context, Result};
use async_stream::stream;
use axum::body::{to_bytes, Body};
use axum::extract::ws::{Message as DownstreamWsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, State};
use axum::http::{HeaderMap, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{HeaderName, HeaderValue};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::error::ProtocolError as TungsteniteProtocolError;
use tokio_tungstenite::tungstenite::protocol::Message as UpstreamWsMessage;
use tokio_tungstenite::tungstenite::Error as TungsteniteError;
use tokio::sync::{mpsc, oneshot, RwLock};
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
    pub async fn bind(provider: ProviderKind, upstream_base: &Url) -> Result<(TcpListener, Url)> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind proxy listener")?;
        let addr = listener
            .local_addr()
            .context("missing proxy listener address")?;
        let proxy_base_url = proxy_base_url(provider, upstream_base, addr)?;
        Ok((listener, proxy_base_url))
    }

    pub async fn start(
        provider: ProviderKind,
        upstream_base: Url,
        session: SessionRuntime,
        ledger: SessionLedgerWriter,
    ) -> Result<Self> {
        let (listener, proxy_base_url) = Self::bind(provider, &upstream_base).await?;
        Self::start_with_listener(
            provider,
            upstream_base,
            session,
            ledger,
            listener,
            proxy_base_url,
        )
        .await
    }

    pub async fn start_with_listener(
        provider: ProviderKind,
        upstream_base: Url,
        session: SessionRuntime,
        ledger: SessionLedgerWriter,
        listener: TcpListener,
        proxy_base_url: Url,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .context("failed to construct proxy reqwest client")?;
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
    if is_websocket_upgrade_request(&request) {
        return handle_websocket_proxy_request(state, request).await;
    }

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

async fn handle_websocket_proxy_request(
    state: ProxyState,
    request: Request<Body>,
) -> Result<Response<Body>> {
    let maybe_delivery = state.delivery.read().await.clone();
    let (mut parts, body) = request.into_parts();
    let _ = body;
    let ws = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|err| anyhow!("invalid websocket upgrade request: {err}"))?;
    let upstream_url = websocket_upstream_url(
        &state
            .provider
            .build_upstream_url(&state.upstream_base, &parts.uri)
            .context("failed to derive upstream websocket URL")?,
    )?;
    let request_headers = websocket_forwardable_headers(&parts.headers);

    let exchange_id = state.session.next_exchange_id();
    let request_artifact = state
        .ledger
        .record_request(&exchange_id, parts.uri.path(), None, None, &[], None)
        .await?;
    let artifact_refs = ArtifactRefs::default().with_request_path(Some(request_artifact));

    let mut upstream_request = upstream_url
        .as_str()
        .into_client_request()
        .context("failed to build upstream websocket request")?;
    for (name, value) in request_headers {
        upstream_request.headers_mut().insert(name, value);
    }

    let (upstream_socket, upstream_response) =
        match tokio_tungstenite::connect_async(upstream_request).await {
            Ok(result) => result,
            Err(err) => {
                if let Some(delivery) = maybe_delivery.as_ref() {
                    delivery
                        .enqueue_turn(state.session.provider_error_turn(
                            &exchange_id,
                            "upstream_websocket_connect_error",
                            &format!("upstream websocket connect failed: {err}"),
                            None,
                            &artifact_refs,
                        ))
                        .await
                        .ok();
                }
                return Ok((
                    StatusCode::BAD_GATEWAY,
                    format!("upstream websocket connect failed: {err}"),
                )
                    .into_response());
            }
        };

    let status = StatusCode::from_u16(upstream_response.status().as_u16())
        .unwrap_or(StatusCode::SWITCHING_PROTOCOLS);
    let request_id = state.provider.request_id_from_headers(upstream_response.headers());
    let response_artifact = state
        .ledger
        .record_response(
            &exchange_id,
            status.as_u16(),
            request_id.as_deref(),
            None,
            &[],
            None,
        )
        .await?;
    let artifact_refs = artifact_refs.with_response_path(Some(response_artifact));
    let selected_protocol = websocket_selected_protocol(upstream_response.headers());
    let ledger = state.ledger.clone();
    let session = state.session.clone();
    let exchange_id_for_upgrade = exchange_id.clone();
    let request_id_for_upgrade = request_id.clone();
    let artifact_refs_for_upgrade = artifact_refs.clone();

    let upgrade = if let Some(protocol) = selected_protocol {
        ws.protocols([protocol])
    } else {
        ws
    };

    Ok(upgrade
        .on_upgrade(move |downstream_socket| async move {
            if let Err(err) = relay_websocket(
                downstream_socket,
                upstream_socket,
                ledger,
                exchange_id_for_upgrade,
                maybe_delivery,
                session,
                request_id_for_upgrade,
                artifact_refs_for_upgrade,
            )
            .await
            {
                let _ = err;
            }
        })
        .into_response())
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
    let (tx, rx) = mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(32);
    let stream = upstream_response.bytes_stream();
    tokio::spawn(async move {
        let mut parse_buffer = String::new();
        let mut raw_stream = String::new();
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
                    if tx.send(Ok(chunk)).await.is_err() {
                        // Keep draining upstream so capture completes even if downstream disconnects.
                    }
                }
                Err(err) => {
                    delivery.enqueue_turn(session.provider_error_turn(
                        &exchange_id,
                        "stream_transport_error",
                        &format!("failed to read upstream stream: {err}"),
                        request_id_for_stream.as_deref(),
                        &stream_artifact_refs,
                    )).await.ok();
                    let _ = tx
                        .send(Err(std::io::Error::other(err.to_string())))
                        .await;
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
    });

    let stream = stream! {
        let mut rx = rx;
        while let Some(item) = rx.recv().await {
            yield item;
        }
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

async fn relay_websocket(
    downstream_socket: WebSocket,
    upstream_socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    ledger: SessionLedgerWriter,
    exchange_id: String,
    maybe_delivery: Option<DeliveryHandle>,
    session: SessionRuntime,
    request_id: Option<String>,
    artifact_refs: ArtifactRefs,
) -> Result<()> {
    let (mut downstream_tx, mut downstream_rx) = downstream_socket.split();
    let (mut upstream_tx, mut upstream_rx) = upstream_socket.split();
    let mut capture = WebsocketCapture::new(
        session.provider(),
        exchange_id.clone(),
        request_id.clone(),
        artifact_refs.clone(),
    );

    loop {
        tokio::select! {
            downstream_message = downstream_rx.next() => {
                let Some(downstream_message) = downstream_message else {
                    let _ = upstream_tx.close().await;
                    break;
                };
                let downstream_message = match downstream_message {
                    Ok(message) => message,
                    Err(err) => {
                        if is_benign_downstream_websocket_read_error(&err) {
                            let _ = upstream_tx.close().await;
                            break;
                        }
                        return websocket_relay_error(
                            maybe_delivery,
                            session,
                            &exchange_id,
                            request_id.as_deref(),
                            &artifact_refs,
                            "downstream_websocket_error",
                            &format!("failed to read downstream websocket message: {err}"),
                        )
                        .await;
                    }
                };
                record_websocket_frame(&ledger, &exchange_id, "downstream", &downstream_message).await;
                if let Some(delivery) = maybe_delivery.as_ref() {
                    let turns = capture.observe_downstream_message(&session, &downstream_message);
                    enqueue_turns(delivery, turns).await;
                }
                let is_close = matches!(downstream_message, DownstreamWsMessage::Close(_));
                if let Some(upstream_message) = map_downstream_message(downstream_message) {
                    upstream_tx
                        .send(upstream_message)
                        .await
                        .context("failed to forward websocket message upstream")?;
                }
                if is_close {
                    break;
                }
            }
            upstream_message = upstream_rx.next() => {
                let Some(upstream_message) = upstream_message else {
                    let _ = downstream_tx.close().await;
                    break;
                };
                let upstream_message = match upstream_message {
                    Ok(message) => message,
                    Err(err) => {
                        if is_benign_websocket_read_error(&err) {
                            let _ = downstream_tx.close().await;
                            break;
                        }
                        return websocket_relay_error(
                            maybe_delivery,
                            session,
                            &exchange_id,
                            request_id.as_deref(),
                            &artifact_refs,
                            "upstream_websocket_error",
                            &format!("failed to read upstream websocket message: {err}"),
                        )
                        .await;
                    }
                };
                record_upstream_websocket_frame(&ledger, &exchange_id, "upstream", &upstream_message).await;
                if let Some(delivery) = maybe_delivery.as_ref() {
                    let turns = capture.observe_upstream_message(&session, &upstream_message);
                    enqueue_turns(delivery, turns).await;
                }
                let is_close = matches!(upstream_message, UpstreamWsMessage::Close(_));
                if let Some(downstream_message) = map_upstream_message(upstream_message) {
                    downstream_tx
                        .send(downstream_message)
                        .await
                        .context("failed to forward websocket message downstream")?;
                }
                if is_close {
                    break;
                }
            }
        }
    }

    if let Some(delivery) = maybe_delivery.as_ref() {
        let turns = capture.finalize_pending(&session);
        enqueue_turns(delivery, turns).await;
    }

    Ok(())
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
            if is_hop_by_hop(name.as_str())
                || name.as_str().eq_ignore_ascii_case("host")
                || name.as_str().eq_ignore_ascii_case("accept-encoding")
            {
                None
            } else {
                Some((name.clone(), value.clone()))
            }
        })
        .collect()
}

fn websocket_forwardable_headers(headers: &HeaderMap) -> Vec<(HeaderName, HeaderValue)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let lower = name.as_str().to_ascii_lowercase();
            if is_hop_by_hop(&lower)
                || lower == "host"
                || matches!(
                    lower.as_str(),
                    "sec-websocket-key" | "sec-websocket-version" | "sec-websocket-extensions"
                )
            {
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

fn header_contains_token(headers: &HeaderMap, name: &str, token: &str) -> bool {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
        .unwrap_or(false)
}

fn is_websocket_upgrade_request(request: &Request<Body>) -> bool {
    request.method() == http::Method::GET
        && header_contains_token(request.headers(), "connection", "upgrade")
        && request
            .headers()
            .get("upgrade")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

fn websocket_upstream_url(url: &Url) -> Result<Url> {
    let mut url = url.clone();
    match url.scheme() {
        "http" => url
            .set_scheme("ws")
            .map_err(|_| anyhow!("failed to switch upstream scheme from http to ws"))?,
        "https" => url
            .set_scheme("wss")
            .map_err(|_| anyhow!("failed to switch upstream scheme from https to wss"))?,
        "ws" | "wss" => {}
        scheme => return Err(anyhow!("unsupported websocket upstream scheme: {scheme}")),
    }
    Ok(url)
}

fn websocket_selected_protocol(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get("sec-websocket-protocol")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn record_websocket_frame(
    ledger: &SessionLedgerWriter,
    exchange_id: &str,
    direction: &str,
    message: &DownstreamWsMessage,
) {
    if let Some(frame) = summarize_downstream_websocket_frame(direction, message) {
        let _ = ledger.append_stream_frame(exchange_id, &frame).await;
    }
}

async fn record_upstream_websocket_frame(
    ledger: &SessionLedgerWriter,
    exchange_id: &str,
    direction: &str,
    message: &UpstreamWsMessage,
) {
    if let Some(frame) = summarize_upstream_websocket_frame(direction, message) {
        let _ = ledger.append_stream_frame(exchange_id, &frame).await;
    }
}

fn summarize_downstream_websocket_frame(
    direction: &str,
    message: &DownstreamWsMessage,
) -> Option<String> {
    match message {
        DownstreamWsMessage::Text(text) => Some(format!("{direction}:text:{text}")),
        DownstreamWsMessage::Binary(bytes) => {
            Some(format!("{direction}:binary:{} bytes", bytes.len()))
        }
        DownstreamWsMessage::Ping(bytes) => Some(format!("{direction}:ping:{} bytes", bytes.len())),
        DownstreamWsMessage::Pong(bytes) => Some(format!("{direction}:pong:{} bytes", bytes.len())),
        DownstreamWsMessage::Close(_) => Some(format!("{direction}:close")),
    }
}

fn summarize_upstream_websocket_frame(
    direction: &str,
    message: &UpstreamWsMessage,
) -> Option<String> {
    match message {
        UpstreamWsMessage::Text(text) => Some(format!("{direction}:text:{text}")),
        UpstreamWsMessage::Binary(bytes) => {
            Some(format!("{direction}:binary:{} bytes", bytes.len()))
        }
        UpstreamWsMessage::Ping(bytes) => Some(format!("{direction}:ping:{} bytes", bytes.len())),
        UpstreamWsMessage::Pong(bytes) => Some(format!("{direction}:pong:{} bytes", bytes.len())),
        UpstreamWsMessage::Close(_) => Some(format!("{direction}:close")),
        UpstreamWsMessage::Frame(_) => None,
    }
}

fn map_downstream_message(message: DownstreamWsMessage) -> Option<UpstreamWsMessage> {
    match message {
        DownstreamWsMessage::Text(text) => Some(UpstreamWsMessage::Text(text.to_string())),
        DownstreamWsMessage::Binary(bytes) => Some(UpstreamWsMessage::Binary(bytes.to_vec())),
        DownstreamWsMessage::Ping(bytes) => Some(UpstreamWsMessage::Ping(bytes.to_vec())),
        DownstreamWsMessage::Pong(bytes) => Some(UpstreamWsMessage::Pong(bytes.to_vec())),
        DownstreamWsMessage::Close(_) => Some(UpstreamWsMessage::Close(None)),
    }
}

fn map_upstream_message(message: UpstreamWsMessage) -> Option<DownstreamWsMessage> {
    match message {
        UpstreamWsMessage::Text(text) => Some(DownstreamWsMessage::Text(text.to_string().into())),
        UpstreamWsMessage::Binary(bytes) => {
            Some(DownstreamWsMessage::Binary(bytes.to_vec().into()))
        }
        UpstreamWsMessage::Ping(bytes) => Some(DownstreamWsMessage::Ping(bytes.to_vec().into())),
        UpstreamWsMessage::Pong(bytes) => Some(DownstreamWsMessage::Pong(bytes.to_vec().into())),
        UpstreamWsMessage::Close(_) => Some(DownstreamWsMessage::Close(None)),
        UpstreamWsMessage::Frame(_) => None,
    }
}

#[derive(Debug)]
struct WebsocketCapture {
    provider: ProviderKind,
    exchange_id: String,
    request_id: Option<String>,
    artifact_refs: ArtifactRefs,
    current_state: Option<crate::provider::ExchangeState>,
}

impl WebsocketCapture {
    fn new(
        provider: ProviderKind,
        exchange_id: String,
        request_id: Option<String>,
        artifact_refs: ArtifactRefs,
    ) -> Self {
        Self {
            provider,
            exchange_id,
            request_id,
            artifact_refs,
            current_state: None,
        }
    }

    fn observe_downstream_message(
        &mut self,
        session: &SessionRuntime,
        message: &DownstreamWsMessage,
    ) -> Vec<crate::turns::TurnEnvelope> {
        let DownstreamWsMessage::Text(text) = message else {
            return Vec::new();
        };
        self.observe_downstream_text(session, text.as_str())
    }

    fn observe_upstream_message(
        &mut self,
        session: &SessionRuntime,
        message: &UpstreamWsMessage,
    ) -> Vec<crate::turns::TurnEnvelope> {
        let UpstreamWsMessage::Text(text) = message else {
            return Vec::new();
        };
        self.observe_upstream_text(session, text.as_str())
    }

    fn observe_downstream_text(
        &mut self,
        session: &SessionRuntime,
        text: &str,
    ) -> Vec<crate::turns::TurnEnvelope> {
        if self.provider != ProviderKind::Codex {
            return Vec::new();
        }
        let Ok(payload) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        if payload.get("type").and_then(Value::as_str) != Some("response.create") {
            return Vec::new();
        }

        let mut turns = self.finalize_pending(session);
        let prepared = self.provider.prepare_exchange(
            session,
            self.exchange_id.clone(),
            text.as_bytes(),
            &self.artifact_refs,
        );
        turns.extend(prepared.request_turns);
        self.current_state = Some(prepared.state);
        turns
    }

    fn observe_upstream_text(
        &mut self,
        session: &SessionRuntime,
        text: &str,
    ) -> Vec<crate::turns::TurnEnvelope> {
        let Some(state) = self.current_state.as_mut() else {
            return Vec::new();
        };
        let Ok(payload) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .map(|value| value.to_string());
        state.absorb_sse_frame(&openai::SseFrame {
            event: event_type.clone(),
            data: text.to_string(),
            raw: text.to_string(),
        });
        if event_type.as_deref() == Some("response.completed") {
            return self.finalize_pending(session);
        }
        Vec::new()
    }

    fn finalize_pending(&mut self, session: &SessionRuntime) -> Vec<crate::turns::TurnEnvelope> {
        let Some(state) = self.current_state.take() else {
            return Vec::new();
        };
        state.finalize_stream(
            session,
            200,
            self.request_id.clone(),
            &self.artifact_refs,
            None,
        )
    }
}

fn is_benign_websocket_read_error(err: &TungsteniteError) -> bool {
    matches!(
        err,
        TungsteniteError::ConnectionClosed
            | TungsteniteError::AlreadyClosed
            | TungsteniteError::Protocol(TungsteniteProtocolError::ResetWithoutClosingHandshake)
            | TungsteniteError::Protocol(TungsteniteProtocolError::HandshakeIncomplete)
    )
}

fn is_benign_downstream_websocket_read_error(err: &axum::Error) -> bool {
    let message = err.to_string();
    message.contains("Connection closed normally")
        || message.contains("Trying to work with closed connection")
        || message.contains("Connection reset without closing handshake")
        || message.contains("Handshake not finished")
}

async fn websocket_relay_error(
    maybe_delivery: Option<DeliveryHandle>,
    session: SessionRuntime,
    exchange_id: &str,
    request_id: Option<&str>,
    artifact_refs: &ArtifactRefs,
    title: &str,
    message: &str,
) -> Result<()> {
    if let Some(delivery) = maybe_delivery {
        delivery
            .enqueue_turn(session.provider_error_turn(
                exchange_id,
                title,
                message,
                request_id,
                artifact_refs,
            ))
            .await
            .ok();
    }
    Err(anyhow!(message.to_string()))
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

#[cfg(test)]
mod tests {
    use super::{
        forwardable_headers, is_benign_websocket_read_error, is_websocket_upgrade_request,
        websocket_forwardable_headers, websocket_upstream_url, ProxyServer, WebsocketCapture,
    };
    use axum::body::Body;
    use axum::http::{HeaderMap, HeaderValue};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::error::ProtocolError as TungsteniteProtocolError;
    use tokio_tungstenite::tungstenite::handshake::server::{
        Request as WsRequest, Response as WsResponse,
    };
    use tokio_tungstenite::tungstenite::Error as TungsteniteError;
    use tokio_tungstenite::tungstenite::Message as WsMessage;
    use url::Url;

    use crate::ledger::SessionLedgerWriter;
    use crate::provider::ProviderKind;
    use crate::session::SessionRuntime;

    #[test]
    fn forwardable_headers_drop_host_but_keep_authorization() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("127.0.0.1:12345"));
        headers.insert("authorization", HeaderValue::from_static("Bearer test"));
        headers.insert("accept-encoding", HeaderValue::from_static("gzip, br"));

        let forwarded = forwardable_headers(&headers);
        assert!(
            !forwarded
                .iter()
                .any(|(name, _)| name.as_str().eq_ignore_ascii_case("host"))
        );
        assert!(
            !forwarded
                .iter()
                .any(|(name, _)| name.as_str().eq_ignore_ascii_case("accept-encoding"))
        );
        assert!(
            forwarded
                .iter()
                .any(|(name, value)| name == "authorization" && value == "Bearer test")
        );
    }

    #[test]
    fn websocket_forwardable_headers_drop_client_handshake_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer test"));
        headers.insert("sec-websocket-key", HeaderValue::from_static("abc"));
        headers.insert("sec-websocket-version", HeaderValue::from_static("13"));

        let forwarded = websocket_forwardable_headers(&headers);
        assert!(
            forwarded
                .iter()
                .any(|(name, value)| name == "authorization" && value == "Bearer test")
        );
        assert!(
            !forwarded
                .iter()
                .any(|(name, _)| name.as_str().eq_ignore_ascii_case("sec-websocket-key"))
        );
    }

    #[test]
    fn websocket_upgrade_detection_matches_standard_headers() {
        let request = http::Request::builder()
            .method(http::Method::GET)
            .uri("/v1/responses")
            .header("connection", "keep-alive, Upgrade")
            .header("upgrade", "websocket")
            .body(Body::empty())
            .unwrap();
        assert!(is_websocket_upgrade_request(&request));
    }

    #[test]
    fn websocket_upstream_url_switches_http_scheme() {
        let upstream = Url::parse("https://api.openai.com/v1/responses").unwrap();
        assert_eq!(
            websocket_upstream_url(&upstream).unwrap().as_str(),
            "wss://api.openai.com/v1/responses"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn websocket_upgrade_requests_are_relayed_to_upstream() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(None::<String>));
        let seen_for_server = Arc::clone(&seen);
        let upstream = tokio::spawn(async move {
            let (socket, _) = upstream_listener.accept().await.unwrap();
            let callback = move |request: &WsRequest, response: WsResponse| {
                *seen_for_server.lock().unwrap() = Some(request.uri().path().to_string());
                Ok(response)
            };
            let mut socket = tokio_tungstenite::accept_hdr_async(socket, callback)
                .await
                .unwrap();
            let message = socket.next().await.unwrap().unwrap();
            socket.send(message).await.unwrap();
        });

        let session =
            SessionRuntime::new(ProviderKind::Codex, Vec::new(), BTreeMap::new()).unwrap();
        let ledger = SessionLedgerWriter::create(&session).await.unwrap();
        let proxy = ProxyServer::start(
            ProviderKind::Codex,
            Url::parse(&format!("ws://{upstream_addr}/v1")).unwrap(),
            session,
            ledger.clone(),
        )
        .await
        .unwrap();

        let mut proxy_url = proxy.proxy_base_url();
        proxy_url.set_scheme("ws").unwrap();
        proxy_url.set_path("/v1/responses");

        let (mut socket, _) = tokio_tungstenite::connect_async(proxy_url.as_str())
            .await
            .unwrap();
        socket
            .send(WsMessage::Text("hello websocket".to_string()))
            .await
            .unwrap();
        let echoed = socket.next().await.unwrap().unwrap();
        assert_eq!(echoed.into_text().unwrap(), "hello websocket");
        socket.close(None).await.unwrap();

        upstream.await.unwrap();
        proxy.shutdown().await.unwrap();

        assert_eq!(
            seen.lock().unwrap().clone(),
            Some("/v1/responses".to_string())
        );

        let ledger_json: Value =
            serde_json::from_str(&tokio::fs::read_to_string(ledger.path()).await.unwrap()).unwrap();
        assert_eq!(ledger_json["exchanges"][0]["status_code"], 101);
        assert_eq!(ledger_json["exchanges"][0]["endpoint"], "/v1/responses");
    }

    #[test]
    fn websocket_capture_turns_real_prompt_into_history_and_answer() {
        let session =
            SessionRuntime::new(ProviderKind::Codex, Vec::new(), BTreeMap::new()).unwrap();
        let mut capture = WebsocketCapture::new(
            ProviderKind::Codex,
            "exchange-0001".to_string(),
            Some("req_123".to_string()),
            crate::turns::ArtifactRefs::default(),
        );

        let bootstrap_turns = capture.observe_downstream_text(
            &session,
            r#"{"type":"response.create","model":"gpt-5.4","instructions":"bootstrap","input":[]}"#,
        );
        assert!(bootstrap_turns.is_empty());
        let bootstrap_answer = capture.observe_upstream_text(
            &session,
            r#"{"type":"response.completed","response":{"model":"gpt-5.4","status":"completed","output":[]}}"#,
        );
        assert!(bootstrap_answer.is_empty());

        let request_turns = capture.observe_downstream_text(
            &session,
            r#"{
                "type":"response.create",
                "model":"gpt-5.4",
                "input":[
                    {"type":"message","role":"developer","content":[{"type":"input_text","text":"mode"}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"yooo dawg"}]}
                ]
            }"#,
        );
        assert_eq!(request_turns.len(), 1);
        assert_eq!(request_turns[0].item.item_type, "user_input");
        assert_eq!(
            request_turns[0].item.user_input.as_ref().unwrap().text,
            "yooo dawg"
        );

        assert!(capture
            .observe_upstream_text(
                &session,
                r#"{"type":"response.output_text.delta","delta":"yoo"}"#,
            )
            .is_empty());
        assert!(capture
            .observe_upstream_text(
                &session,
                r#"{"type":"response.output_text.delta","delta":", what's up?"}"#,
            )
            .is_empty());
        let answer_turns = capture.observe_upstream_text(
            &session,
            r#"{
                "type":"response.completed",
                "response":{
                    "model":"gpt-5.4",
                    "status":"completed",
                    "output":[
                        {
                            "type":"message",
                            "role":"assistant",
                            "content":[{"type":"output_text","text":"yoo, what's up?"}]
                        }
                    ]
                }
            }"#,
        );
        assert_eq!(answer_turns.len(), 1);
        assert_eq!(answer_turns[0].item.item_type, "assistant_turn");
        assert_eq!(
            answer_turns[0].item.turn.as_ref().unwrap().text,
            "yoo, what's up?"
        );
    }

    #[test]
    fn websocket_reset_without_close_is_treated_as_benign() {
        assert!(is_benign_websocket_read_error(&TungsteniteError::Protocol(
            TungsteniteProtocolError::ResetWithoutClosingHandshake
        )));
        assert!(is_benign_websocket_read_error(&TungsteniteError::Protocol(
            TungsteniteProtocolError::HandshakeIncomplete
        )));
        assert!(is_benign_websocket_read_error(
            &TungsteniteError::ConnectionClosed
        ));
    }
}
