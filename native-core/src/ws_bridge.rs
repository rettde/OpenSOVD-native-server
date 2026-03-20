// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// WsBridgeTransport — WebSocket-based cloud↔vehicle bridge (F3)
//
// Implements `BridgeTransport` using tokio-tungstenite for real WebSocket
// communication between cloud and vehicle SOVD server instances.
//
// Architecture (ADR A3.1):
//   Cloud: listen on `listen_addr`, accept vehicle WS connections
//   Vehicle: connect to cloud `connect_url`, register session
//
// Protocol:
//   1. Vehicle connects via WebSocket to cloud relay
//   2. Vehicle sends a JSON handshake: { "vehicle_id": "VIN...", "tenant_id": "..." }
//   3. Cloud registers a BridgeSession
//   4. Cloud forwards SovdBridgeRequest as JSON text frames
//   5. Vehicle responds with SovdBridgeResponse JSON text frames
//   6. Heartbeat: cloud sends WS Ping, expects Pong within timeout
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use native_interfaces::bridge::{
    BridgeConfig, BridgeError, BridgeSession, BridgeSessionId, BridgeTransport, SovdBridgeRequest,
    SovdBridgeResponse,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// A pending request waiting for a response from the vehicle.
struct PendingRequest {
    tx: oneshot::Sender<SovdBridgeResponse>,
}

/// Per-session state holding the WS write half and pending request map.
struct WsSession {
    info: BridgeSession,
    /// Channel to send outbound messages to the session's write task.
    outbound_tx: mpsc::Sender<Message>,
    /// Pending requests awaiting vehicle responses, keyed by request_id.
    pending: Arc<DashMap<String, PendingRequest>>,
}

/// WebSocket bridge transport for cloud↔vehicle SOVD tunneling.
///
/// The cloud instance calls `start_accept_loop()` to listen for incoming
/// vehicle connections. Each connected vehicle gets a `WsSession` with
/// a dedicated read/write task pair.
pub struct WsBridgeTransport {
    sessions: DashMap<String, Arc<WsSession>>,
    config: BridgeConfig,
    /// Listener handle — set after `start_accept_loop`
    _listener_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl WsBridgeTransport {
    /// Create a new WebSocket bridge transport.
    pub fn new(config: BridgeConfig) -> Self {
        Self {
            sessions: DashMap::new(),
            config,
            _listener_handle: Mutex::new(None),
        }
    }

    /// Start the accept loop that listens for incoming vehicle WebSocket connections.
    ///
    /// This spawns a background task that accepts TCP connections, upgrades them
    /// to WebSocket, performs the handshake, and registers sessions.
    pub async fn start_accept_loop(self: &Arc<Self>) -> Result<(), BridgeError> {
        let listen_addr = self
            .config
            .listen_addr
            .as_deref()
            .ok_or_else(|| BridgeError::NotConfigured("no listen_addr configured".into()))?;

        let listener = TcpListener::bind(listen_addr).await.map_err(|e| {
            BridgeError::ConnectionLost(format!("failed to bind {listen_addr}: {e}"))
        })?;

        tracing::info!(addr = listen_addr, "WebSocket bridge listening");
        self.run_accept_loop(listener).await;
        Ok(())
    }

    /// Start the accept loop with a pre-bound listener (useful for tests with port 0).
    pub async fn start_accept_loop_with_listener(
        self: &Arc<Self>,
        listener: TcpListener,
    ) -> Result<(), BridgeError> {
        self.run_accept_loop(listener).await;
        Ok(())
    }

    /// Internal accept loop implementation.
    async fn run_accept_loop(self: &Arc<Self>, listener: TcpListener) {
        let transport = Arc::clone(self);
        let handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        let t = Arc::clone(&transport);
                        tokio::spawn(async move {
                            if let Err(e) = t.handle_incoming(stream, addr.to_string()).await {
                                tracing::warn!(remote = %addr, error = %e, "Bridge handshake failed");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Bridge accept error");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });

        *self._listener_handle.lock().await = Some(handle);
    }

    /// Handle an incoming TCP connection: upgrade to WS, perform handshake.
    async fn handle_incoming(
        self: &Arc<Self>,
        stream: TcpStream,
        remote_addr: String,
    ) -> Result<(), BridgeError> {
        let ws_stream = tokio_tungstenite::accept_async(stream)
            .await
            .map_err(|e| BridgeError::ConnectionLost(format!("WS upgrade failed: {e}")))?;

        let (write, mut read) = ws_stream.split();

        // Wait for handshake message from vehicle
        let handshake_msg = tokio::time::timeout(Duration::from_secs(10), read.next())
            .await
            .map_err(|_| BridgeError::Timeout("handshake timeout (10s)".into()))?
            .ok_or_else(|| {
                BridgeError::ConnectionLost("connection closed before handshake".into())
            })?
            .map_err(|e| BridgeError::ConnectionLost(format!("WS read error: {e}")))?;

        let handshake: serde_json::Value = match handshake_msg {
            Message::Text(text) => serde_json::from_str(&text).map_err(|e| {
                BridgeError::AuthenticationFailed(format!("invalid handshake JSON: {e}"))
            })?,
            _ => {
                return Err(BridgeError::AuthenticationFailed(
                    "expected text frame for handshake".into(),
                ));
            }
        };

        let vehicle_id = handshake
            .get("vehicle_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                BridgeError::AuthenticationFailed("handshake missing vehicle_id".into())
            })?
            .to_owned();

        let tenant_id = handshake
            .get("tenant_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        let session_id = uuid::Uuid::new_v4().to_string();
        let session_info = BridgeSession {
            id: BridgeSessionId(session_id.clone()),
            vehicle_id: vehicle_id.clone(),
            tenant_id,
            connected_at: chrono::Utc::now().to_rfc3339(),
            remote_addr: Some(remote_addr.clone()),
        };

        // Create outbound channel for sending messages to vehicle
        let (outbound_tx, outbound_rx) = mpsc::channel::<Message>(64);
        let pending: Arc<DashMap<String, PendingRequest>> = Arc::new(DashMap::new());

        let ws_session = Arc::new(WsSession {
            info: session_info,
            outbound_tx,
            pending: Arc::clone(&pending),
        });

        self.sessions
            .insert(session_id.clone(), Arc::clone(&ws_session));

        tracing::info!(
            session = %session_id,
            vehicle = %vehicle_id,
            remote = %remote_addr,
            "Vehicle connected via WebSocket"
        );

        // Spawn write task
        let sid_write = session_id.clone();
        tokio::spawn(Self::write_loop(write, outbound_rx, sid_write));

        // Spawn read task (handles responses from vehicle)
        let transport = Arc::clone(self);
        let sid_read = session_id;
        tokio::spawn(async move {
            transport.read_loop(read, pending, sid_read).await;
        });

        Ok(())
    }

    /// Write loop: sends outbound messages from the channel to the WS stream.
    async fn write_loop(
        mut write: SplitSink<WebSocketStream<TcpStream>, Message>,
        mut rx: mpsc::Receiver<Message>,
        session_id: String,
    ) {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = write.send(msg).await {
                tracing::warn!(session = %session_id, error = %e, "WS write error");
                break;
            }
        }
        tracing::debug!(session = %session_id, "Write loop ended");
    }

    /// Read loop: receives messages from the vehicle and dispatches responses.
    async fn read_loop(
        &self,
        mut read: futures::stream::SplitStream<WebSocketStream<TcpStream>>,
        pending: Arc<DashMap<String, PendingRequest>>,
        session_id: String,
    ) {
        while let Some(msg_result) = read.next().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    match serde_json::from_str::<SovdBridgeResponse>(&text) {
                        Ok(response) => {
                            if let Some((_, req)) = pending.remove(&response.request_id) {
                                let _ = req.tx.send(response);
                            } else {
                                tracing::debug!(
                                    session = %session_id,
                                    request_id = %response.request_id,
                                    "Received response for unknown request"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                session = %session_id,
                                error = %e,
                                "Failed to parse vehicle response"
                            );
                        }
                    }
                }
                Ok(Message::Ping(data)) => {
                    // Auto-handled by tungstenite — Pong is sent automatically
                    tracing::trace!(session = %session_id, "Received Ping ({} bytes)", data.len());
                }
                Ok(Message::Pong(_)) => {
                    tracing::trace!(session = %session_id, "Received Pong");
                }
                Ok(Message::Close(_)) => {
                    tracing::info!(session = %session_id, "Vehicle sent Close frame");
                    break;
                }
                Ok(_) => {} // Binary frames etc. — ignored
                Err(e) => {
                    tracing::warn!(session = %session_id, error = %e, "WS read error");
                    break;
                }
            }
        }

        // Session ended — clean up
        self.sessions.remove(&session_id);
        tracing::info!(session = %session_id, "Vehicle session ended");
    }
}

#[async_trait::async_trait]
impl BridgeTransport for WsBridgeTransport {
    async fn accept_remote(&self) -> Result<BridgeSession, BridgeError> {
        // The accept loop runs in the background; this method waits for any new session.
        // For simplicity, return NotConfigured — real usage is via start_accept_loop.
        Err(BridgeError::NotConfigured(
            "use start_accept_loop() instead — sessions are accepted in background".into(),
        ))
    }

    async fn forward_to_vehicle(
        &self,
        session: &BridgeSession,
        request: SovdBridgeRequest,
    ) -> Result<SovdBridgeResponse, BridgeError> {
        let ws_session = self
            .sessions
            .get(&session.id.0)
            .ok_or_else(|| BridgeError::SessionNotFound(session.id.0.clone()))?;

        let request_id = request.request_id.clone();

        // Register pending request
        let (tx, rx) = oneshot::channel();
        ws_session
            .pending
            .insert(request_id.clone(), PendingRequest { tx });

        // Serialize and send request
        let json = serde_json::to_string(&request).map_err(|e| BridgeError::RemoteError {
            code: "SERIALIZE".into(),
            message: format!("failed to serialize request: {e}"),
        })?;

        ws_session
            .outbound_tx
            .send(Message::Text(json))
            .await
            .map_err(|_| BridgeError::ConnectionLost("outbound channel closed".into()))?;

        // Wait for response with timeout
        let timeout = Duration::from_secs(30);
        tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| {
                ws_session.pending.remove(&request_id);
                BridgeError::Timeout(format!("vehicle did not respond within {timeout:?}"))
            })?
            .map_err(|_| BridgeError::ConnectionLost("response channel dropped".into()))
    }

    async fn heartbeat(&self, session: &BridgeSession) -> Result<(), BridgeError> {
        let ws_session = self
            .sessions
            .get(&session.id.0)
            .ok_or_else(|| BridgeError::SessionNotFound(session.id.0.clone()))?;

        ws_session
            .outbound_tx
            .send(Message::Ping(vec![0x42]))
            .await
            .map_err(|_| BridgeError::ConnectionLost("heartbeat send failed".into()))?;

        Ok(())
    }

    async fn disconnect(&self, session: &BridgeSession) -> Result<(), BridgeError> {
        let ws_session = self
            .sessions
            .remove(&session.id.0)
            .ok_or_else(|| BridgeError::SessionNotFound(session.id.0.clone()))?;

        // Send close frame
        let _ = ws_session.1.outbound_tx.send(Message::Close(None)).await;

        tracing::info!(session = %session.id, "Bridge session disconnected");
        Ok(())
    }

    fn active_sessions(&self) -> Vec<BridgeSession> {
        self.sessions
            .iter()
            .map(|e| e.value().info.clone())
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Bind to port 0 (OS-assigned), start the accept loop, return the actual port.
    async fn start_transport() -> (Arc<WsBridgeTransport>, u16) {
        let config = BridgeConfig {
            enabled: true,
            listen_addr: Some("127.0.0.1:0".into()),
            ..BridgeConfig::default()
        };
        let transport = Arc::new(WsBridgeTransport::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        transport
            .start_accept_loop_with_listener(listener)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        (transport, port)
    }

    /// Helper: connect a mock vehicle client to the bridge, perform handshake.
    async fn connect_vehicle(
        port: u16,
        vehicle_id: &str,
    ) -> WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
        let url = format!("ws://127.0.0.1:{port}");
        let (ws, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        let (mut write, read) = ws.split();

        let handshake = serde_json::json!({ "vehicle_id": vehicle_id });
        write
            .send(Message::Text(handshake.to_string()))
            .await
            .expect("send handshake");

        tokio::time::sleep(Duration::from_millis(50)).await;
        write.reunite(read).expect("reunite")
    }

    #[tokio::test]
    async fn ws_bridge_accept_and_list_sessions() {
        let (transport, port) = start_transport().await;
        assert!(transport.active_sessions().is_empty());

        let _client = connect_vehicle(port, "VIN-TEST-001").await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sessions = transport.active_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].vehicle_id, "VIN-TEST-001");
    }

    #[tokio::test]
    async fn ws_bridge_forward_roundtrip() {
        let (transport, port) = start_transport().await;

        let client = connect_vehicle(port, "VIN-ROUNDTRIP").await;
        let (mut client_write, mut client_read) = client.split();

        tokio::time::sleep(Duration::from_millis(100)).await;
        let sessions = transport.active_sessions();
        assert_eq!(sessions.len(), 1);
        let session = sessions[0].clone();

        // Spawn vehicle responder: read request, send response
        let responder = tokio::spawn(async move {
            while let Some(Ok(msg)) = client_read.next().await {
                if let Message::Text(text) = msg {
                    let req: SovdBridgeRequest =
                        serde_json::from_str(&text).expect("parse request");
                    let resp = SovdBridgeResponse {
                        request_id: req.request_id,
                        status: 200,
                        body: Some(serde_json::json!({"vin": "VIN-ROUNDTRIP"})),
                    };
                    client_write
                        .send(Message::Text(serde_json::to_string(&resp).unwrap()))
                        .await
                        .expect("send response");
                    break;
                }
            }
        });

        let request = SovdBridgeRequest {
            request_id: "req-rt-1".into(),
            method: "GET".into(),
            path: "/sovd/v1/components".into(),
            body: None,
            headers: HashMap::new(),
        };

        let response = transport
            .forward_to_vehicle(&session, request)
            .await
            .unwrap();

        assert_eq!(response.request_id, "req-rt-1");
        assert_eq!(response.status, 200);
        assert!(response.body.is_some());

        responder.await.unwrap();
    }

    #[tokio::test]
    async fn ws_bridge_heartbeat() {
        let (transport, port) = start_transport().await;

        let _client = connect_vehicle(port, "VIN-HB").await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sessions = transport.active_sessions();
        assert_eq!(sessions.len(), 1);
        assert!(transport.heartbeat(&sessions[0]).await.is_ok());
    }

    #[tokio::test]
    async fn ws_bridge_disconnect() {
        let (transport, port) = start_transport().await;

        let _client = connect_vehicle(port, "VIN-DC").await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sessions = transport.active_sessions();
        assert_eq!(sessions.len(), 1);

        transport.disconnect(&sessions[0]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(transport.active_sessions().is_empty());
    }

    #[tokio::test]
    async fn ws_bridge_heartbeat_unknown_session() {
        let transport = Arc::new(WsBridgeTransport::new(BridgeConfig::default()));

        let fake_session = BridgeSession {
            id: BridgeSessionId("nonexistent".into()),
            vehicle_id: "VIN-X".into(),
            tenant_id: None,
            connected_at: "2026-01-01T00:00:00Z".into(),
            remote_addr: None,
        };

        assert!(transport.heartbeat(&fake_session).await.is_err());
    }

    #[tokio::test]
    async fn ws_bridge_forward_unknown_session() {
        let transport = Arc::new(WsBridgeTransport::new(BridgeConfig::default()));

        let fake_session = BridgeSession {
            id: BridgeSessionId("nonexistent".into()),
            vehicle_id: "VIN-X".into(),
            tenant_id: None,
            connected_at: "2026-01-01T00:00:00Z".into(),
            remote_addr: None,
        };

        let req = SovdBridgeRequest {
            request_id: "req-1".into(),
            method: "GET".into(),
            path: "/".into(),
            body: None,
            headers: HashMap::new(),
        };

        assert!(transport
            .forward_to_vehicle(&fake_session, req)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn ws_bridge_multiple_vehicles() {
        let (transport, port) = start_transport().await;

        let _c1 = connect_vehicle(port, "VIN-001").await;
        let _c2 = connect_vehicle(port, "VIN-002").await;
        tokio::time::sleep(Duration::from_millis(150)).await;

        let sessions = transport.active_sessions();
        assert_eq!(sessions.len(), 2);

        let vins: Vec<&str> = sessions.iter().map(|s| s.vehicle_id.as_str()).collect();
        assert!(vins.contains(&"VIN-001"));
        assert!(vins.contains(&"VIN-002"));
    }

    #[test]
    fn ws_bridge_config_defaults() {
        let config = BridgeConfig::default();
        let transport = WsBridgeTransport::new(config);
        assert!(transport.active_sessions().is_empty());
    }
}
