// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use byteorder::{LittleEndian, WriteBytesExt};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection};

use crate::error::{Error, Result};
use crate::protocol::{
    read_frame, write_frame, Frame, DEFAULT_DIAL_TIMEOUT, DEFAULT_REQUEST_TIMEOUT, MSG_ERROR,
    MSG_HELLO,
};

pub type ClientOption = Arc<dyn Fn(&mut ClientOptions) + Send + Sync>;

#[derive(Debug, Clone)]
pub struct ClientOptions {
    pub dial_timeout: Duration,
    pub request_timeout: Duration,
    pub client_tag: String,
    pub(crate) tls_config: std::option::Option<Arc<ClientConfig>>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            dial_timeout: DEFAULT_DIAL_TIMEOUT,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            client_tag: String::new(),
            tls_config: None,
        }
    }
}

pub fn with_dial_timeout(timeout: Duration) -> ClientOption {
    Arc::new(move |opts| opts.dial_timeout = timeout)
}

pub fn with_request_timeout(timeout: Duration) -> ClientOption {
    Arc::new(move |opts| opts.request_timeout = timeout)
}

pub fn with_client_tag(tag: impl Into<String>) -> ClientOption {
    let tag = tag.into();
    Arc::new(move |opts| opts.client_tag = tag.clone())
}

#[cfg(test)]
pub(crate) fn with_tls_config(config: Arc<ClientConfig>) -> ClientOption {
    Arc::new(move |opts| opts.tls_config = Some(config.clone()))
}

#[derive(Clone, Debug)]
pub struct RequestContext {
    deadline: std::option::Option<Instant>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub struct CancelHandle {
    cancelled: Arc<AtomicBool>,
}

impl CancelHandle {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

impl RequestContext {
    pub fn background() -> Self {
        Self {
            deadline: None,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_deadline(deadline: Instant) -> Self {
        Self {
            deadline: Some(deadline),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self::with_deadline(Instant::now() + timeout)
    }

    pub fn cancellable() -> (Self, CancelHandle) {
        let cancelled = Arc::new(AtomicBool::new(false));
        (
            Self {
                deadline: None,
                cancelled: cancelled.clone(),
            },
            CancelHandle { cancelled },
        )
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn deadline(&self) -> std::option::Option<Instant> {
        self.deadline
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self::background()
    }
}

pub struct Client {
    conn: Mutex<Connection>,
    req_id: AtomicU64,
    closed: AtomicBool,
    timeout: Duration,
    session_id: AtomicU64,
    client_tag: String,
}

impl Client {
    pub fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let mut conn = self.conn.lock().map_err(|_| Error::ClientClosed)?;
        conn.close()
    }

    pub fn session_id(&self) -> u64 {
        self.session_id.load(Ordering::SeqCst)
    }

    pub fn client_tag(&self) -> &str {
        &self.client_tag
    }

    pub(crate) fn send_request(
        &self,
        ctx: &RequestContext,
        msg_type: u16,
        payload: &[u8],
    ) -> Result<Frame> {
        self.send_request_with_flags(ctx, msg_type, 0, payload)
    }

    pub(crate) fn send_request_with_flags(
        &self,
        ctx: &RequestContext,
        msg_type: u16,
        flags: u16,
        payload: &[u8],
    ) -> Result<Frame> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::ClientClosed);
        }

        if ctx.is_cancelled() {
            return Err(Error::Cancelled);
        }

        let effective_deadline = self.compute_deadline(ctx)?;

        let mut conn = self.conn.lock().map_err(|_| Error::ClientClosed)?;
        conn.set_deadline(Some(effective_deadline))?;

        let req_id = self.req_id.fetch_add(1, Ordering::SeqCst) + 1;
        write_frame(&mut *conn, msg_type, flags, req_id, payload)?;
        let frame = read_frame(&mut *conn)?;

        conn.set_deadline(None)?;

        if frame.header.msg_type == MSG_ERROR {
            return Err(parse_server_error(&frame.payload));
        }

        Ok(frame)
    }

    fn compute_deadline(&self, ctx: &RequestContext) -> Result<Instant> {
        let now = Instant::now();
        let mut deadline = now + self.timeout;
        if let Some(ctx_deadline) = ctx.deadline() {
            if ctx_deadline < deadline {
                deadline = ctx_deadline;
            }
        }
        if deadline <= now {
            return Err(Error::Timeout);
        }
        Ok(deadline)
    }

    fn send_hello(&self, client_tag: &str) -> Result<()> {
        let mut payload = Vec::with_capacity(2 + 2 + client_tag.len() + 4);
        payload.write_u16::<LittleEndian>(1)?; // protocol version
        payload.write_u16::<LittleEndian>(client_tag.len() as u16)?;
        payload.extend_from_slice(client_tag.as_bytes());
        payload.write_u32::<LittleEndian>(0)?; // no metadata

        let ctx = RequestContext::with_timeout(self.timeout);
        let frame = self.send_request_with_flags(&ctx, MSG_HELLO, 0, &payload)?;

        if frame.header.msg_type != MSG_HELLO {
            return Err(Error::invalid_response(format!(
                "unexpected response type: {}",
                frame.header.msg_type
            )));
        }

        if frame.payload.len() >= 8 {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&frame.payload[0..8]);
            let session = u64::from_le_bytes(bytes);
            self.session_id.store(session, Ordering::SeqCst);
        }

        Ok(())
    }
}

pub fn dial(addr: &str, opts: impl IntoIterator<Item = ClientOption>) -> Result<Client> {
    let mut options = ClientOptions::default();
    for opt in opts {
        opt(&mut options);
    }

    let stream = connect_tcp(addr, options.dial_timeout)?;
    let conn = Connection::Plain(stream);

    let client = Client {
        conn: Mutex::new(conn),
        req_id: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        timeout: options.request_timeout,
        session_id: AtomicU64::new(0),
        client_tag: options.client_tag.clone(),
    };

    if let Err(err) = client.send_hello(&options.client_tag) {
        let _ = client.close();
        return Err(err);
    }

    Ok(client)
}

pub fn dial_tls(addr: &str, opts: impl IntoIterator<Item = ClientOption>) -> Result<Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut options = ClientOptions::default();
    for opt in opts {
        opt(&mut options);
    }

    let stream = connect_tcp(addr, options.dial_timeout)?;
    let config = match options.tls_config.take() {
        Some(cfg) => cfg,
        None => Arc::new(default_tls_config()?),
    };

    let server_name = server_name_from_addr(addr)?;
    let conn =
        ClientConnection::new(config, server_name).map_err(|err| Error::Tls(err.to_string()))?;

    let stream = rustls::StreamOwned::new(conn, stream);

    let client = Client {
        conn: Mutex::new(Connection::Tls(Box::new(stream))),
        req_id: AtomicU64::new(0),
        closed: AtomicBool::new(false),
        timeout: options.request_timeout,
        session_id: AtomicU64::new(0),
        client_tag: options.client_tag.clone(),
    };

    if let Err(err) = client.send_hello(&options.client_tag) {
        let _ = client.close();
        return Err(err);
    }

    Ok(client)
}

fn connect_tcp(addr: &str, timeout: Duration) -> Result<TcpStream> {
    let addrs = addr
        .to_socket_addrs()
        .map_err(Error::Io)?
        .collect::<Vec<_>>();

    let mut last_err = None;
    for socket_addr in addrs {
        match TcpStream::connect_timeout(&socket_addr, timeout) {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err
        .map(Error::Io)
        .unwrap_or(Error::Io(std::io::Error::other(
            "no addresses resolved",
        ))))
}

fn default_tls_config() -> Result<ClientConfig> {
    let mut root_store = rustls::RootCertStore::empty();
    let certs = rustls_native_certs::load_native_certs();
    for cert in certs.certs {
        root_store
            .add(cert)
            .map_err(|err| Error::Tls(err.to_string()))?;
    }
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Ok(config)
}

fn server_name_from_addr(addr: &str) -> Result<ServerName<'static>> {
    let host = if addr.starts_with('[') {
        // IPv6 in brackets
        addr.split(']')
            .next()
            .unwrap_or("[]")
            .trim_start_matches('[')
    } else {
        addr.rsplit_once(':').map(|(host, _)| host).unwrap_or(addr)
    };

    ServerName::try_from(host.to_string())
        .map_err(|_| Error::Tls(format!("invalid server name: {host}")))
}

fn parse_server_error(payload: &[u8]) -> Error {
    if payload.len() < 8 {
        return Error::server(0, "unknown error");
    }
    let code = u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default());
    let detail_len = u32::from_le_bytes(payload[4..8].try_into().unwrap_or_default()) as usize;
    let detail = if payload.len() >= 8 + detail_len {
        String::from_utf8_lossy(&payload[8..8 + detail_len]).to_string()
    } else {
        String::new()
    };
    Error::server(code, detail)
}

pub(crate) enum Connection {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<ClientConnection, TcpStream>>),
}

impl Connection {
    fn set_deadline(&mut self, deadline: std::option::Option<Instant>) -> Result<()> {
        let timeout = deadline.map(|d| d.saturating_duration_since(Instant::now()));
        match self {
            Connection::Plain(stream) => {
                stream.set_read_timeout(timeout).map_err(Error::Io)?;
                stream.set_write_timeout(timeout).map_err(Error::Io)?;
            }
            Connection::Tls(stream) => {
                let tcp = stream.get_mut();
                tcp.set_read_timeout(timeout).map_err(Error::Io)?;
                tcp.set_write_timeout(timeout).map_err(Error::Io)?;
            }
        }
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        match self {
            Connection::Plain(stream) => {
                stream.shutdown(std::net::Shutdown::Both).map_err(Error::Io)
            }
            Connection::Tls(stream) => stream
                .get_mut()
                .shutdown(std::net::Shutdown::Both)
                .map_err(Error::Io),
        }
    }
}

impl std::io::Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.read(buf),
            Connection::Tls(stream) => stream.read(buf),
        }
    }
}

impl std::io::Write for Connection {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Connection::Plain(stream) => stream.write(buf),
            Connection::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Connection::Plain(stream) => stream.flush(),
            Connection::Tls(stream) => stream.flush(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_frame, write_frame, FrameHeader, MSG_HELLO};
    use crate::test_util::{decode_hex, load_fixture};
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
    use rustls::ServerConfig;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn hello_payload_matches_go_format() {
        let tag = "";
        let mut payload = Vec::new();
        payload.write_u16::<LittleEndian>(1).unwrap();
        payload.write_u16::<LittleEndian>(0).unwrap();
        payload.write_u32::<LittleEndian>(0).unwrap();
        assert_eq!(payload, hello_payload(tag));

        let tag = "test-client";
        let mut payload = Vec::new();
        payload.write_u16::<LittleEndian>(1).unwrap();
        payload.write_u16::<LittleEndian>(tag.len() as u16).unwrap();
        payload.extend_from_slice(tag.as_bytes());
        payload.write_u32::<LittleEndian>(0).unwrap();
        assert_eq!(payload, hello_payload(tag));
    }

    #[test]
    fn hello_payloads_match_fixtures() {
        let fixture = load_fixture("hello_empty");
        assert_eq!(fixture.msg_type, MSG_HELLO);
        assert_eq!(fixture.flags, 0);
        assert_eq!(decode_hex(&fixture.payload_hex), hello_payload(""));

        let fixture = load_fixture("hello_tag");
        assert_eq!(fixture.msg_type, MSG_HELLO);
        assert_eq!(fixture.flags, 0);
        assert_eq!(
            decode_hex(&fixture.payload_hex),
            hello_payload("test-client")
        );
    }

    #[test]
    fn frame_header_roundtrip() {
        let mut buf = Vec::new();
        let payload = b"abc";
        write_frame(&mut buf, 5, 0, 42, payload).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let frame = read_frame(&mut cursor).unwrap();
        assert_eq!(
            frame.header,
            FrameHeader {
                len: payload.len() as u32,
                msg_type: 5,
                flags: 0,
                req_id: 42,
            }
        );
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn truncated_header_is_invalid_response() {
        let data = vec![0u8; 3];
        let mut cursor = std::io::Cursor::new(data);
        let err = read_frame(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::InvalidResponse(_)));
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut buf = Vec::new();
        buf.write_u32::<LittleEndian>(crate::protocol::MAX_FRAME_SIZE + 1)
            .unwrap();
        buf.write_u16::<LittleEndian>(MSG_HELLO).unwrap();
        buf.write_u16::<LittleEndian>(0).unwrap();
        buf.write_u64::<LittleEndian>(1).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let err = read_frame(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::InvalidResponse(_)));
    }

    #[test]
    fn truncated_payload_is_invalid_response() {
        let mut buf = Vec::new();
        buf.write_u32::<LittleEndian>(4).unwrap();
        buf.write_u16::<LittleEndian>(MSG_HELLO).unwrap();
        buf.write_u16::<LittleEndian>(0).unwrap();
        buf.write_u64::<LittleEndian>(1).unwrap();
        buf.extend_from_slice(&[1, 2]);
        let mut cursor = std::io::Cursor::new(buf);
        let err = read_frame(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::InvalidResponse(_)));
    }

    #[test]
    fn tls_dial_uses_local_server() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (cert, key) = generate_cert();
        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .unwrap();
        let server_config = Arc::new(server_config);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server_handle = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(server_config).unwrap();
            let mut stream = rustls::StreamOwned::new(conn, tcp.try_clone().unwrap());

            let frame = read_frame(&mut stream).unwrap();
            assert_eq!(frame.header.msg_type, MSG_HELLO);

            let mut resp = Vec::new();
            resp.write_u64::<LittleEndian>(123).unwrap();
            resp.write_u16::<LittleEndian>(1).unwrap();
            write_frame(&mut stream, MSG_HELLO, 0, frame.header.req_id, &resp).unwrap();
        });

        let mut root = rustls::RootCertStore::empty();
        root.add(cert).unwrap();
        let client_config = ClientConfig::builder()
            .with_root_certificates(root)
            .with_no_client_auth();
        let client_config = Arc::new(client_config);

        let addr_str = format!("localhost:{}", addr.port());
        let client = dial_tls(&addr_str, vec![with_tls_config(client_config)]).unwrap();
        assert_eq!(client.session_id(), 123);

        server_handle.join().unwrap();
    }

    #[test]
    fn default_timeouts_match_go() {
        let opts = ClientOptions::default();
        assert_eq!(opts.dial_timeout, DEFAULT_DIAL_TIMEOUT);
        assert_eq!(opts.request_timeout, DEFAULT_REQUEST_TIMEOUT);
    }

    #[test]
    fn error_response_yields_server_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            let frame = read_frame(&mut stream).unwrap();
            assert_eq!(frame.header.msg_type, MSG_HELLO);
            let mut resp = Vec::new();
            resp.write_u64::<LittleEndian>(1).unwrap();
            resp.write_u16::<LittleEndian>(1).unwrap();
            write_frame(&mut stream, MSG_HELLO, 0, frame.header.req_id, &resp).unwrap();

            let req = read_frame(&mut stream).unwrap();
            let mut err_payload = Vec::new();
            err_payload.write_u32::<LittleEndian>(404).unwrap();
            let detail = b"not found";
            err_payload
                .write_u32::<LittleEndian>(detail.len() as u32)
                .unwrap();
            err_payload.extend_from_slice(detail);
            write_frame(
                &mut stream,
                crate::protocol::MSG_ERROR,
                0,
                req.header.req_id,
                &err_payload,
            )
            .unwrap();
        });

        let client = dial(&addr.to_string(), Vec::new()).unwrap();
        let ctx = RequestContext::background();
        let payload = 0u64.to_le_bytes();
        let err = client
            .send_request(&ctx, crate::protocol::MSG_CTX_CREATE, &payload)
            .unwrap_err();
        match err {
            Error::Server(server) => {
                assert_eq!(server.code, 404);
                assert_eq!(server.detail, "not found");
            }
            other => panic!("expected server error, got {other:?}"),
        }

        handle.join().unwrap();
    }

    fn hello_payload(tag: &str) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.write_u16::<LittleEndian>(1).unwrap();
        payload.write_u16::<LittleEndian>(tag.len() as u16).unwrap();
        payload.extend_from_slice(tag.as_bytes());
        payload.write_u32::<LittleEndian>(0).unwrap();
        payload
    }

    fn generate_cert() -> (
        rustls::pki_types::CertificateDer<'static>,
        rustls::pki_types::PrivateKeyDer<'static>,
    ) {
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "cxdb-test");
        params.distinguished_name = dn;
        let key_pair = KeyPair::generate().unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_der = cert.der().to_vec();
        let key_der = key_pair.serialize_der();
        (
            rustls::pki_types::CertificateDer::from(cert_der),
            rustls::pki_types::PrivateKeyDer::from(rustls::pki_types::PrivatePkcs8KeyDer::from(
                key_der,
            )),
        )
    }
}
