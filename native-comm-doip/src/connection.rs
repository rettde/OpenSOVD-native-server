// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// DoIP connection — TCP connection to a DoIP gateway using doip-codec
// Follows cda-comm-doip patterns (DoipDiagGateway, DoipConnection, etc.)
// Uses doip-codec + doip-definitions (same crates as CDA)
// ─────────────────────────────────────────────────────────────────────────────

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use openssl::ssl::{SslConnector, SslFiletype, SslMethod, SslVerifyMode};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_openssl::SslStream;
use tokio_util::codec::Framed;
use tracing::{debug, info, warn};

use doip_codec::DoipCodec;
use doip_definitions::builder::DoipMessageBuilder;
use doip_definitions::header::ProtocolVersion;
use doip_definitions::payload::{
    ActivationCode, ActivationType, DiagnosticMessage, DoipPayload, RoutingActivationRequest,
};

use async_trait::async_trait;
use native_interfaces::{
    ConnectionError, DiagTransport, EcuConnectionState, ServicePayload, UdsResponse,
};

use super::config::DoipConfig;

// ── Stream abstraction (plain TCP or TLS) ────────────────────────────────

enum DoipStream {
    Plain(TcpStream),
    Tls(SslStream<TcpStream>),
}

impl AsyncRead for DoipStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            DoipStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            DoipStream::Tls(s) => Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for DoipStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            DoipStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            DoipStream::Tls(s) => Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            DoipStream::Plain(s) => Pin::new(s).poll_flush(cx),
            DoipStream::Tls(s) => Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            DoipStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            DoipStream::Tls(s) => Pin::new(s).poll_shutdown(cx),
        }
    }
}

/// A single DoIP TCP connection to a gateway.
/// Wraps a tokio-util Framed<DoipStream, DoipCodec> for encoding/decoding.
/// Supports both plain TCP and TLS connections.
pub struct DoipConnection {
    config: DoipConfig,
    framed: Arc<Mutex<Option<Framed<DoipStream, DoipCodec>>>>,
    state: Arc<Mutex<EcuConnectionState>>,
    target_address: u16,
}

impl DoipConnection {
    pub fn new(config: DoipConfig, target_address: u16) -> Self {
        Self {
            config,
            framed: Arc::new(Mutex::new(None)),
            state: Arc::new(Mutex::new(EcuConnectionState::Disconnected)),
            target_address,
        }
    }

    /// Connect TCP to the DoIP gateway
    #[tracing::instrument(skip(self), fields(
        gateway = %format!("{}:{}", self.config.tester_address, self.config.gateway_port),
        target = %format!("0x{:04X}", self.target_address),
    ))]
    pub async fn connect(&self) -> Result<(), ConnectionError> {
        let addr: SocketAddr = format!(
            "{}:{}",
            self.config.tester_address, self.config.gateway_port
        )
        .parse()
        .map_err(|e| ConnectionError::ConnectionFailed(format!("Invalid address: {e}")))?;

        let send_timeout = Duration::from_millis(self.config.send_timeout_ms);

        let stream = timeout(send_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| ConnectionError::Timeout("TCP connect timeout".to_owned()))?
            .map_err(|e| ConnectionError::ConnectionFailed(format!("TCP connect failed: {e}")))?;

        let framed = Framed::new(DoipStream::Plain(stream), DoipCodec {});

        *self.framed.lock().await = Some(framed);
        *self.state.lock().await = EcuConnectionState::Connected;

        info!("DoIP TCP connected (plain)");
        Ok(())
    }

    /// Connect to the DoIP gateway over TLS (using tls_port from config)
    #[tracing::instrument(skip(self), fields(
        gateway = %format!("{}:{}", self.config.tester_address, self.config.tls_port),
        target = %format!("0x{:04X}", self.target_address),
    ))]
    pub async fn connect_tls(&self) -> Result<(), ConnectionError> {
        if self.config.tls_port == 0 {
            return Err(ConnectionError::ConnectionFailed(
                "TLS port not configured (tls_port = 0)".to_owned(),
            ));
        }

        let addr: SocketAddr = format!("{}:{}", self.config.tester_address, self.config.tls_port)
            .parse()
            .map_err(|e| ConnectionError::ConnectionFailed(format!("Invalid TLS address: {e}")))?;

        let send_timeout_dur = Duration::from_millis(self.config.send_timeout_ms);

        // Build OpenSSL connector
        let mut ssl_builder = SslConnector::builder(SslMethod::tls_client())
            .map_err(|e| ConnectionError::ConnectionFailed(format!("SSL builder: {e}")))?;

        if self.config.tls_insecure {
            warn!("TLS certificate verification DISABLED (tls_insecure=true)");
            ssl_builder.set_verify(SslVerifyMode::NONE);
        }

        if let Some(ref ca_path) = self.config.tls_ca_cert {
            ssl_builder.set_ca_file(ca_path).map_err(|e| {
                ConnectionError::ConnectionFailed(format!("CA cert '{ca_path}': {e}"))
            })?;
        }

        if let Some(ref cert_path) = self.config.tls_client_cert {
            ssl_builder
                .set_certificate_file(cert_path, SslFiletype::PEM)
                .map_err(|e| {
                    ConnectionError::ConnectionFailed(format!("Client cert '{cert_path}': {e}"))
                })?;
        }

        if let Some(ref key_path) = self.config.tls_client_key {
            ssl_builder
                .set_private_key_file(key_path, SslFiletype::PEM)
                .map_err(|e| {
                    ConnectionError::ConnectionFailed(format!("Client key '{key_path}': {e}"))
                })?;
        }

        let connector = ssl_builder.build();
        let ssl_config = connector
            .configure()
            .map_err(|e| ConnectionError::ConnectionFailed(format!("SSL configure: {e}")))?;

        let ssl = ssl_config
            .into_ssl(&self.config.tester_address)
            .map_err(|e| ConnectionError::ConnectionFailed(format!("SSL init: {e}")))?;

        // TCP connect
        let tcp_stream = timeout(send_timeout_dur, TcpStream::connect(addr))
            .await
            .map_err(|_| ConnectionError::Timeout("TLS TCP connect timeout".to_owned()))?
            .map_err(|e| ConnectionError::ConnectionFailed(format!("TLS TCP connect: {e}")))?;

        // TLS handshake
        let mut ssl_stream = SslStream::new(ssl, tcp_stream)
            .map_err(|e| ConnectionError::ConnectionFailed(format!("SSL stream create: {e}")))?;

        Pin::new(&mut ssl_stream)
            .connect()
            .await
            .map_err(|e| ConnectionError::ConnectionFailed(format!("TLS handshake failed: {e}")))?;

        let framed = Framed::new(DoipStream::Tls(ssl_stream), DoipCodec {});

        *self.framed.lock().await = Some(framed);
        *self.state.lock().await = EcuConnectionState::Connected;

        info!("DoIP TLS connected");
        Ok(())
    }

    /// Smart connect: uses TLS if tls_port > 0, otherwise plain TCP
    pub async fn auto_connect(&self) -> Result<(), ConnectionError> {
        if self.config.tls_port > 0 {
            self.connect_tls().await
        } else {
            self.connect().await
        }
    }

    /// Send routing activation request
    #[tracing::instrument(skip(self))]
    pub async fn activate_routing(&self) -> Result<(), ConnectionError> {
        let mut guard = self.framed.lock().await;
        let framed = guard.as_mut().ok_or(ConnectionError::Closed)?;

        let payload = DoipPayload::RoutingActivationRequest(RoutingActivationRequest {
            source_address: self.config.source_address.to_be_bytes(),
            activation_type: ActivationType::Default,
            buffer: [0u8; 4],
        });

        let message = DoipMessageBuilder::new()
            .protocol_version(ProtocolVersion::Iso13400_2019)
            .payload(payload)
            .build();

        framed
            .send(message)
            .await
            .map_err(|e| ConnectionError::SendFailed(format!("Routing activation send: {e}")))?;

        // Read response
        let send_timeout = Duration::from_millis(self.config.send_timeout_ms);
        let response = timeout(send_timeout, framed.next())
            .await
            .map_err(|_| ConnectionError::Timeout("Routing activation timeout".to_owned()))?
            .ok_or(ConnectionError::Closed)?
            .map_err(|e| ConnectionError::Decoding(format!("Routing activation decode: {e}")))?;

        match response.payload {
            DoipPayload::RoutingActivationResponse(resp) => {
                info!(code = ?resp.activation_code, "Routing activation response");
                match resp.activation_code {
                    ActivationCode::SuccessfullyActivated
                    | ActivationCode::ActivatedConfirmationRequired => {
                        *self.state.lock().await = EcuConnectionState::RoutingActivated;
                        Ok(())
                    }
                    _ => Err(ConnectionError::RoutingError(format!(
                        "Routing activation rejected: {:?}",
                        resp.activation_code
                    ))),
                }
            }
            _ => Err(ConnectionError::InvalidMessage(
                "Expected RoutingActivationResponse".to_owned(),
            )),
        }
    }

    /// Send a UDS diagnostic message and receive the response (internal impl)
    #[tracing::instrument(skip(self, data), fields(data_len = data.len()))]
    async fn send_diagnostic_inner(&self, data: &[u8]) -> Result<UdsResponse, ConnectionError> {
        let mut guard = self.framed.lock().await;
        let framed = guard.as_mut().ok_or(ConnectionError::Closed)?;

        let payload = DoipPayload::DiagnosticMessage(DiagnosticMessage {
            source_address: self.config.source_address.to_be_bytes(),
            target_address: self.target_address.to_be_bytes(),
            message: data.to_vec(),
        });

        let message = DoipMessageBuilder::new()
            .protocol_version(ProtocolVersion::Iso13400_2019)
            .payload(payload)
            .build();

        framed
            .send(message)
            .await
            .map_err(|e| ConnectionError::SendFailed(format!("Diagnostic send: {e}")))?;

        // Read ACK then actual response
        let send_timeout = Duration::from_millis(self.config.send_timeout_ms);

        loop {
            let response = timeout(send_timeout, framed.next())
                .await
                .map_err(|_| ConnectionError::Timeout("Diagnostic response timeout".to_owned()))?
                .ok_or(ConnectionError::Closed)?
                .map_err(|e| ConnectionError::Decoding(format!("Diagnostic decode: {e}")))?;

            match response.payload {
                DoipPayload::DiagnosticMessageAck(_) => {
                    debug!("Diagnostic message ACK received, waiting for response...");
                    continue;
                }
                DoipPayload::DiagnosticMessageNack(nack) => {
                    return Err(ConnectionError::SendFailed(format!(
                        "Diagnostic NACK: {:?}",
                        nack.nack_code
                    )));
                }
                DoipPayload::DiagnosticMessage(msg) => {
                    let src = u16::from_be_bytes(msg.source_address);
                    let tgt = u16::from_be_bytes(msg.target_address);
                    debug!(src = %format!("0x{src:04X}"), tgt = %format!("0x{tgt:04X}"), len = msg.message.len(), "Diagnostic response");

                    // Check for UDS NRC 0x78 (ResponsePending)
                    if msg.message.len() >= 3 && msg.message[0] == 0x7F && msg.message[2] == 0x78 {
                        info!("UDS ResponsePending (NRC 0x78), waiting for final response...");
                        continue;
                    }

                    return Ok(UdsResponse::Message(ServicePayload {
                        data: msg.message,
                        source_address: src,
                        target_address: tgt,
                    }));
                }
                other => {
                    warn!(?other, "Unexpected DoIP payload during diagnostic exchange");
                    continue;
                }
            }
        }
    }

    /// Disconnect from the DoIP gateway
    pub async fn disconnect(&self) {
        *self.framed.lock().await = None;
        *self.state.lock().await = EcuConnectionState::Disconnected;
        info!("DoIP connection closed");
    }

    /// Get current connection state
    pub async fn connection_state(&self) -> EcuConnectionState {
        *self.state.lock().await
    }

    pub fn target_address(&self) -> u16 {
        self.target_address
    }

    pub fn source_address(&self) -> u16 {
        self.config.source_address
    }
}

#[async_trait]
impl DiagTransport for DoipConnection {
    async fn send_diagnostic(&self, data: &[u8]) -> Result<UdsResponse, ConnectionError> {
        self.send_diagnostic_inner(data).await
    }
}
