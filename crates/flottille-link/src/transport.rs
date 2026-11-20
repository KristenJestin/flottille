//! The transport under the link (`docs/remote.md`, "The transport"): QUIC, whose TLS runs on the
//! machines' own Ed25519 keys, sent as raw public keys (RFC 7250) rather than certificates. The
//! only module that names the libraries it is made of: the rest of the crate, and everything
//! above it, sees Flottille's types, so that changing the transport changes this file.
//!
//! It reaches what it is given: an endpoint listens on the address it is configured with, and
//! dials the addresses its caller names for a key. No relay, no NAT traversal, no discovery: how
//! two machines reach each other (a LAN, a VPN, a public address, a tunnel) is the user's.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use quinn::crypto::rustls::{HandshakeData, QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer,
    UnixTime,
};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use crate::link::FRAME_LIMIT;
use zeroize::Zeroizing;

/// An Ed25519 public key's SubjectPublicKeyInfo, before its 32 bytes (RFC 8410).
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];
/// An Ed25519 private key in PKCS #8, before its 32 bytes (RFC 8410).
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
/// A link with nothing to say still says so this often, so that it stays open.
const KEEP_ALIVE: Duration = Duration::from_secs(5);
/// A link that hears nothing for this long is lost: the other side went without closing it.
const IDLE: Duration = Duration::from_secs(15);
/// The name a client gives in its hello. Nothing checks it: the key is the identity.
const SERVER_NAME: &str = "flottille";
/// The most connections that came in and are not taken yet, and what they may hold buffered all
/// together: a host that opens many and finishes none cannot fill the machine's memory.
const MOST_INCOMING: usize = 32;
const INCOMING_BUFFERED: u64 = 1 << 20;

/// Why the transport failed: the other side closed with a reason of its own, or the connection
/// was lost or never made.
pub(crate) enum Failure {
    Closed(String),
    Lost(String),
}

/// One machine's endpoint: a UDP socket that accepts links and dials them; or, when it listens
/// nowhere, a socket opened at its first dial, which accepts nothing.
pub(crate) struct Transport {
    endpoint: std::sync::Mutex<Option<quinn::Endpoint>>,
    listens: bool,
    key: Arc<CertifiedKey>,
    provider: Arc<CryptoProvider>,
}

impl Transport {
    /// Listens nowhere: it only dials, with the key whose private half is `seed`.
    pub(crate) fn dialler(seed: &[u8; 32], public: &[u8; 32]) -> Result<Transport, String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = certified(seed, public, &provider)?;
        Ok(Transport {
            endpoint: std::sync::Mutex::new(None),
            listens: false,
            key,
            provider,
        })
    }

    /// Listens at `listen` with the key whose private half is `seed`, for the protocol names
    /// `alpns`.
    pub(crate) fn bind(
        seed: &[u8; 32],
        public: &[u8; 32],
        listen: SocketAddr,
        alpns: &[&[u8]],
    ) -> Result<Transport, String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = certified(seed, public, &provider)?;
        let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| error.to_string())?
            .with_client_cert_verifier(Arc::new(AnyKey(provider.signature_verification_algorithms)))
            .with_cert_resolver(Arc::new(
                rustls::server::AlwaysResolvesServerRawPublicKeys::new(key.clone()),
            ));
        tls.alpn_protocols = alpns.iter().map(|alpn| alpn.to_vec()).collect();
        let quic = QuicServerConfig::try_from(tls).map_err(|error| error.to_string())?;
        let mut server = quinn::ServerConfig::with_crypto(Arc::new(quic));
        server.transport_config(Arc::new(transport_config()));
        server
            .max_incoming(MOST_INCOMING)
            .incoming_buffer_size_total(INCOMING_BUFFERED);
        let endpoint = quinn::Endpoint::server(server, listen)
            .map_err(|error| format!("cannot listen on {listen}: {error}"))?;
        Ok(Transport {
            endpoint: std::sync::Mutex::new(Some(endpoint)),
            listens: true,
            key,
            provider,
        })
    }

    /// The socket it listens on.
    pub(crate) fn local(&self) -> Option<SocketAddr> {
        if !self.listens {
            return None;
        }
        self.endpoint.lock().unwrap().as_ref()?.local_addr().ok()
    }

    /// The endpoint, its socket opened now when it listens nowhere and never dialled yet.
    fn endpoint(&self) -> Result<quinn::Endpoint, String> {
        let mut endpoint = self.endpoint.lock().unwrap();
        if let Some(endpoint) = endpoint.as_ref() {
            return Ok(endpoint.clone());
        }
        // Both families when the system allows it, IPv4 alone otherwise.
        let opened = quinn::Endpoint::client(SocketAddr::from(([0u16; 8], 0)))
            .or_else(|_| quinn::Endpoint::client(SocketAddr::from(([0u8; 4], 0))))
            .map_err(|error| format!("cannot open a socket to dial: {error}"))?;
        *endpoint = Some(opened.clone());
        Ok(opened)
    }

    /// A connection to the key `peer` at `address`, for the protocol name `alpn`: refused unless
    /// the other side proves it holds that key.
    pub(crate) async fn connect(
        &self,
        peer: &[u8; 32],
        address: SocketAddr,
        alpn: &[u8],
    ) -> Result<Conn, Failure> {
        let verifier = ExactKey {
            spki: spki(peer),
            algorithms: self.provider.signature_verification_algorithms,
        };
        let mut tls = rustls::ClientConfig::builder_with_provider(self.provider.clone())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|error| Failure::Lost(error.to_string()))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_client_cert_resolver(Arc::new(
                rustls::client::AlwaysResolvesClientRawPublicKeys::new(self.key.clone()),
            ));
        tls.alpn_protocols = vec![alpn.to_vec()];
        let quic =
            QuicClientConfig::try_from(tls).map_err(|error| Failure::Lost(error.to_string()))?;
        let mut client = quinn::ClientConfig::new(Arc::new(quic));
        client.transport_config(Arc::new(transport_config()));
        let connecting = self
            .endpoint()
            .map_err(Failure::Lost)?
            .connect_with(client, address, SERVER_NAME)
            .map_err(|error| Failure::Lost(error.to_string()))?;
        connecting.await.map(Conn::new).map_err(failure)
    }

    /// The next connection that comes from an address that proved it is the sender's, `None`
    /// once the endpoint is closed or when it listens nowhere. One from an address not proved yet
    /// is asked to prove it (a QUIC retry) and is not handed over: a forged source costs nothing
    /// but one packet.
    pub(crate) async fn accept(&self) -> Option<Incoming> {
        if !self.listens {
            return None;
        }
        let endpoint = self.endpoint.lock().unwrap().clone()?;
        loop {
            let incoming = endpoint.accept().await?;
            if incoming.remote_address_validated() {
                return Some(Incoming(incoming));
            }
            if let Err(error) = incoming.retry() {
                error.into_incoming().ignore();
            }
        }
    }

    /// Closes every connection, and waits a little for the other sides to be told.
    pub(crate) async fn close(&self) {
        let Some(endpoint) = self.endpoint.lock().unwrap().clone() else {
            return;
        };
        endpoint.close(0u32.into(), b"this machine stops");
        let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
    }
}

fn transport_config() -> quinn::TransportConfig {
    let mut config = quinn::TransportConfig::default();
    config.keep_alive_interval(Some(KEEP_ALIVE));
    config.max_idle_timeout(Some(IDLE.try_into().expect("a short idle timeout")));
    config
}

/// The private key as rustls signs with it, and the public key it shows.
fn certified(
    seed: &[u8; 32],
    public: &[u8; 32],
    provider: &CryptoProvider,
) -> Result<Arc<CertifiedKey>, String> {
    let mut der = Zeroizing::new(Vec::with_capacity(PKCS8_PREFIX.len() + seed.len()));
    der.extend_from_slice(&PKCS8_PREFIX);
    der.extend_from_slice(seed);
    let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der.as_slice()).clone_key());
    let signing = provider
        .key_provider
        .load_private_key(private)
        .map_err(|_| "the machine key cannot sign for the link".to_owned())?;
    Ok(Arc::new(CertifiedKey::new(
        vec![CertificateDer::from(spki(public))],
        signing,
    )))
}

fn spki(public: &[u8; 32]) -> Vec<u8> {
    let mut der = SPKI_PREFIX.to_vec();
    der.extend_from_slice(public);
    der
}

/// The Ed25519 key a raw public key is, when it is one.
fn key_of(der: &[u8]) -> Option<[u8; 32]> {
    der.strip_prefix(SPKI_PREFIX.as_slice())?.try_into().ok()
}

/// A connection that came in, its TLS not done yet.
pub(crate) struct Incoming(quinn::Incoming);

impl Incoming {
    /// The address it came from, proved by the retry.
    pub(crate) fn from(&self) -> SocketAddr {
        self.0.remote_address()
    }

    /// Refuses it at once, telling the other side.
    pub(crate) fn refuse(self) {
        self.0.refuse();
    }

    pub(crate) async fn finish(self) -> Result<Conn, Failure> {
        self.0.await.map(Conn::new).map_err(failure)
    }
}

/// A connection, its TLS done: the other side holds the key it showed.
///
/// The streams the other side opens are taken as they come, each read in a task of its own: one
/// that is slow to say what it is (or never does) holds up no other, and is given up on after
/// `OPENING`.
#[derive(Clone)]
pub(crate) struct Conn(quinn::Connection, Arc<Inbox>);

/// What the other side opened, read, and not taken yet.
struct Inbox(tokio::sync::Mutex<mpsc::Receiver<Inbound>>);

/// The streams read and waiting to be taken: the others wait in the connection's own limit.
const WAITING: usize = 16;

impl Conn {
    fn new(connection: quinn::Connection) -> Conn {
        let (inbound, queue) = mpsc::channel(WAITING);
        let accepting = connection.clone();
        tokio::spawn(async move {
            // It ends with the connection, or when nobody holds the connection any more: a
            // handle kept here would keep it open.
            loop {
                let accepted = tokio::select! {
                    accepted = accepting.accept_bi() => accepted,
                    _ = inbound.closed() => break,
                };
                let Ok((send, recv)) = accepted else { break };
                let inbound = inbound.clone();
                tokio::spawn(async move {
                    if let Some(opened) = Conn::read_opening(send, recv).await {
                        let _ = inbound.send(opened).await;
                    }
                });
            }
        });
        Conn(connection, Arc::new(Inbox(tokio::sync::Mutex::new(queue))))
    }

    /// The other side's key.
    pub(crate) fn peer(&self) -> Option<[u8; 32]> {
        let identity = self.0.peer_identity()?;
        let shown = identity.downcast::<Vec<CertificateDer<'static>>>().ok()?;
        key_of(shown.first()?)
    }

    /// The protocol name both sides agreed on.
    pub(crate) fn alpn(&self) -> Option<Vec<u8>> {
        let data = self.0.handshake_data()?;
        data.downcast::<HandshakeData>().ok()?.protocol
    }

    /// Sends `frame` on a stream of its own, and reads the other side's answer on it.
    pub(crate) async fn request(&self, frame: &[u8], limit: usize) -> Result<Vec<u8>, Failure> {
        let (mut send, mut recv) = self.0.open_bi().await.map_err(failure)?;
        send.write_all(frame).await.map_err(write_failure)?;
        let _ = send.finish();
        recv.read_to_end(limit).await.map_err(read_failure)
    }

    /// Sends `frame` on a stream of its own, and gives the stream the other side answers on, read
    /// frame by frame (`Frames`): a request whose answer is followed by events.
    pub(crate) async fn open(&self, frame: &[u8]) -> Result<Frames, Failure> {
        let (mut send, recv) = self.0.open_bi().await.map_err(failure)?;
        send.write_all(frame).await.map_err(write_failure)?;
        let _ = send.finish();
        Ok(Frames {
            recv,
            buffer: Vec::new(),
        })
    }

    /// The next frame the other side sends, with the stream to answer it on; `None` once the
    /// connection is gone. A stream that fails alone is passed over, and so is a raw one
    /// (`Conn::next_stream`): this is for a link that carries frames only.
    pub(crate) async fn next(&self) -> Option<(Vec<u8>, Reply)> {
        loop {
            match self.next_stream().await? {
                Inbound::Frame(frame, reply) => return Some((frame, reply)),
                Inbound::Raw(raw, _) => raw.refuse(),
            }
        }
    }

    /// What a stream the other side opened is: a frame, read whole, or a raw stream and its
    /// opening line; `None` when it failed or was too slow.
    async fn read_opening(send: quinn::SendStream, mut recv: quinn::RecvStream) -> Option<Inbound> {
        let first = recv.read_chunk(64 * 1024, true).await.ok()?;
        let first = first.map(|chunk| chunk.bytes.to_vec()).unwrap_or_default();
        if first.first() == Some(&RAW) {
            let opened = tokio::time::timeout(OPENING, Raw::opened(send, recv, first)).await;
            return opened
                .ok()
                .flatten()
                .map(|(raw, line)| Inbound::Raw(raw, line));
        }
        let room = FRAME_LIMIT.checked_sub(first.len())?;
        let rest = recv.read_to_end(room).await.ok()?;
        let mut frame = first;
        frame.extend_from_slice(&rest);
        Some(Inbound::Frame(frame, Reply(send)))
    }

    /// The next stream the other side opens: a frame, read whole, with the stream to answer it
    /// on, or a raw stream (`Conn::open_raw`) with the line that opened it. `None` once the
    /// connection is gone. A stream that fails alone is passed over.
    pub(crate) async fn next_stream(&self) -> Option<Inbound> {
        self.1.0.lock().await.recv().await
    }

    /// Opens a raw, two-way stream, opened by one line (`header`, no newline): what follows it,
    /// either way, is bytes. A tunnel's connection (`docs/remote.md`, "Tunnels").
    pub(crate) async fn open_raw(&self, header: &[u8]) -> Result<Raw, Failure> {
        let (mut send, recv) = self.0.open_bi().await.map_err(failure)?;
        let mut line = Vec::with_capacity(header.len() + 2);
        line.push(RAW);
        line.extend_from_slice(header);
        line.push(b'\n');
        send.write_all(&line).await.map_err(write_failure)?;
        Ok(Raw {
            send,
            recv,
            early: Vec::new(),
        })
    }

    /// A stream that sends `bytes` and then nothing, held by the stream it gives back (tests).
    #[cfg(test)]
    pub(crate) async fn open_unfinished(
        &self,
        bytes: &[u8],
    ) -> (quinn::SendStream, quinn::RecvStream) {
        let (mut send, recv) = self.0.open_bi().await.unwrap();
        send.write_all(bytes).await.unwrap();
        (send, recv)
    }

    pub(crate) fn close(&self, code: u32, reason: &str) {
        self.0.close(code.into(), reason.as_bytes());
    }

    /// Waits until the connection is gone; why.
    pub(crate) async fn closed(&self) -> Failure {
        failure(self.0.closed().await)
    }
}

/// What opens a raw stream, as the first byte of its opening line: a frame is JSON, and starts
/// with something else.
const RAW: u8 = 0;
/// The longest opening line of a raw stream, newline and marker included.
const RAW_HEADER: usize = 512;
/// How long the other side has to end the opening line of a raw stream.
const OPENING: Duration = Duration::from_secs(5);

/// What a stream the other side opened is.
pub(crate) enum Inbound {
    /// A frame, read whole, and where to answer it.
    Frame(Vec<u8>, Reply),
    /// A raw stream and the line that opened it, marker and newline left out.
    Raw(Raw, Vec<u8>),
}

/// A two-way stream of bytes, past its opening line: reads what the other side writes, writes
/// what it reads. Ending its writing ends what the other side reads.
pub(crate) struct Raw {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    /// What came with the opening line, read before the stream is.
    early: Vec<u8>,
}

impl Raw {
    /// The opening line of a stream whose first bytes are `first`, read to its newline; `None`
    /// when it is too long or the stream failed.
    async fn opened(
        send: quinn::SendStream,
        mut recv: quinn::RecvStream,
        first: Vec<u8>,
    ) -> Option<(Raw, Vec<u8>)> {
        let mut buffer = first;
        loop {
            if let Some(at) = buffer.iter().position(|byte| *byte == b'\n') {
                let early = buffer.split_off(at + 1);
                buffer.pop();
                buffer.remove(0);
                return Some((Raw { send, recv, early }, buffer));
            }
            if buffer.len() > RAW_HEADER {
                return None;
            }
            match recv.read_chunk(RAW_HEADER, true).await {
                Ok(Some(chunk)) => buffer.extend_from_slice(&chunk.bytes),
                _ => return None,
            }
        }
    }

    /// Ends the stream both ways: the other side's reads and writes fail.
    pub(crate) fn refuse(mut self) {
        let _ = self.send.reset(REFUSED.into());
        let _ = self.recv.stop(REFUSED.into());
    }
}

/// The code a raw stream refused is reset with.
const REFUSED: u32 = 1;

impl AsyncRead for Raw {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.early.is_empty() {
            let count = self.early.len().min(buf.remaining());
            let rest = self.early.split_off(count);
            buf.put_slice(&std::mem::replace(&mut self.early, rest));
            return Poll::Ready(Ok(()));
        }
        AsyncRead::poll_read(Pin::new(&mut self.recv), cx, buf)
    }
}

impl AsyncWrite for Raw {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.send), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.send), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.send), cx)
    }
}

/// Where the answer to one frame goes.
pub(crate) struct Reply(quinn::SendStream);

impl Reply {
    pub(crate) async fn send(mut self, frame: &[u8]) {
        if self.0.write_all(frame).await.is_ok() {
            let _ = self.0.finish();
            // The answer is sent before the stream is let go.
            let _ = self.0.stopped().await;
        }
    }

    pub(crate) async fn send_owned(self, frame: Vec<u8>) {
        self.send(&frame).await;
    }

    /// Writes one frame of an answer given frame by frame, a newline after it; `false` once the
    /// other side stopped reading.
    pub(crate) async fn line(&mut self, frame: &[u8]) -> bool {
        self.0.write_all(frame).await.is_ok() && self.0.write_all(b"\n").await.is_ok()
    }

    /// Ends an answer given frame by frame, once the other side read it.
    pub(crate) async fn end(mut self) {
        if self.0.finish().is_ok() {
            let _ = self.0.stopped().await;
        }
    }

    /// Until the other side stops reading the answer, or the connection is gone.
    pub(crate) fn gone(&self) -> impl Future<Output = ()> + Send + 'static {
        let stopped = self.0.stopped();
        async move {
            let _ = stopped.await;
        }
    }
}

/// An answer read frame by frame, each ended by a newline.
pub(crate) struct Frames {
    recv: quinn::RecvStream,
    buffer: Vec<u8>,
}

impl Frames {
    /// The next frame; `None` once the other side ended the stream. What it wrote last without a
    /// newline is a frame too: a refusal of the whole stream is one frame, written so.
    pub(crate) async fn next(&mut self, limit: usize) -> Option<Result<Vec<u8>, Failure>> {
        loop {
            if let Some(at) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let rest = self.buffer.split_off(at + 1);
                let mut frame = std::mem::replace(&mut self.buffer, rest);
                frame.pop();
                return Some(Ok(frame));
            }
            if self.buffer.len() > limit {
                return Some(Err(Failure::Lost("a frame is too long".to_owned())));
            }
            match self.recv.read_chunk(64 * 1024, true).await {
                Ok(Some(chunk)) => self.buffer.extend_from_slice(&chunk.bytes),
                Ok(None) if self.buffer.is_empty() => return None,
                Ok(None) => return Some(Ok(std::mem::take(&mut self.buffer))),
                Err(quinn::ReadError::ConnectionLost(error)) => return Some(Err(failure(error))),
                Err(other) => return Some(Err(Failure::Lost(other.to_string()))),
            }
        }
    }
}

fn failure(error: quinn::ConnectionError) -> Failure {
    match error {
        quinn::ConnectionError::ApplicationClosed(close) => {
            Failure::Closed(String::from_utf8_lossy(&close.reason).into_owned())
        }
        other => Failure::Lost(other.to_string()),
    }
}

fn write_failure(error: quinn::WriteError) -> Failure {
    match error {
        quinn::WriteError::ConnectionLost(error) => failure(error),
        other => Failure::Lost(other.to_string()),
    }
}

fn read_failure(error: quinn::ReadToEndError) -> Failure {
    match error {
        quinn::ReadToEndError::Read(quinn::ReadError::ConnectionLost(error)) => failure(error),
        quinn::ReadToEndError::TooLong => Failure::Lost("a frame is too long".to_owned()),
        other => Failure::Lost(other.to_string()),
    }
}

/// A client checks that the server holds the very key it dialled.
#[derive(Debug)]
struct ExactKey {
    spki: Vec<u8>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for ExactKey {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.spki.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not spoken".to_owned()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// A server takes any Ed25519 key a client proves it holds: whether it trusts that key is the
/// link's to say, which can then tell the other side why it refuses.
#[derive(Debug)]
struct AnyKey(WebPkiSupportedAlgorithms);

impl ClientCertVerifier for AnyKey {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        match key_of(end_entity) {
            Some(_) => Ok(ClientCertVerified::assertion()),
            None => Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::BadEncoding,
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not spoken".to_owned()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

fn verify(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
    algorithms: &WebPkiSupportedAlgorithms,
) -> Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature_with_raw_key(
        message,
        &SubjectPublicKeyInfoDer::from(cert.as_ref()),
        dss,
        algorithms,
    )
}

/// TLS over a TCP stream to a hub (`control.rs`): TLS 1.3, the certificate checked against the
/// system's roots, or against `roots` (certificates in DER) when given. Why it failed, otherwise.
pub(crate) async fn tls_client(
    stream: tokio::net::TcpStream,
    host: &str,
    roots: Option<&[Vec<u8>]>,
) -> Result<Box<dyn crate::Stream>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut store = rustls::RootCertStore::empty();
    match roots {
        None => {
            for certificate in rustls_native_certs::load_native_certs().certs {
                let _ = store.add(certificate);
            }
        }
        Some(roots) => {
            for certificate in roots {
                let _ = store.add(CertificateDer::from(certificate.clone()));
            }
        }
    }
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_root_certificates(store)
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_owned())
        .map_err(|_| format!("{host} is not a name TLS takes"))?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let secured = connector
        .connect(name, stream)
        .await
        .map_err(|error| format!("its TLS failed: {error}"))?;
    Ok(Box::new(secured))
}

/// TLS on a certificate the user gives, as the hub serves it: TLS 1.3.
#[derive(Clone)]
pub struct TlsServer(tokio_rustls::TlsAcceptor);

impl TlsServer {
    /// The certificate (its chain after it) and its private key, in PEM files. Why not, in a
    /// sentence that names the files and never the key's bytes.
    pub fn from_pem(
        certificate: &std::path::Path,
        key: &std::path::Path,
    ) -> Result<TlsServer, String> {
        use rustls::pki_types::pem::PemObject;
        let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(certificate)
            .and_then(|found| found.collect::<Result<_, _>>())
            .map_err(|error| {
                format!(
                    "{} is not a certificate in PEM: {error}",
                    certificate.display()
                )
            })?;
        if certificates.is_empty() {
            return Err(format!("{} holds no certificate", certificate.display()));
        }
        let private = PrivateKeyDer::from_pem_file(key).map_err(|_| {
            format!(
                "{} is not a private key in PEM, or cannot be read",
                key.display()
            )
        })?;
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| error.to_string())?
        .with_no_client_auth()
        .with_single_cert(certificates, private)
        .map_err(|error| {
            format!(
                "the certificate {} and the key {} do not go together: {error}",
                certificate.display(),
                key.display()
            )
        })?;
        Ok(TlsServer(tokio_rustls::TlsAcceptor::from(Arc::new(config))))
    }

    /// TLS over a stream that came in.
    pub async fn accept(
        &self,
        stream: tokio::net::TcpStream,
    ) -> std::io::Result<Box<dyn crate::Stream>> {
        Ok(Box::new(self.0.accept(stream).await?))
    }
}
