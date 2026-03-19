// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// BridgeTransport — Cloud bridge abstraction (Wave 3, A3.4)
//
// Defines the trait for brokered remote diagnostics. The cloud instance
// accepts incoming sessions from vehicle instances and forwards SOVD
// requests through a persistent tunnel (WebSocket, gRPC, MQTT).
//
// See ADR A3.1 for topology decisions.
// ─────────────────────────────────────────────────────────────────────────────

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Errors from bridge operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BridgeError {
    /// Connection to remote peer failed or was lost
    ConnectionLost(String),
    /// Authentication / authorization failure
    AuthenticationFailed(String),
    /// Remote peer returned an error
    RemoteError { code: String, message: String },
    /// Request timed out waiting for remote response
    Timeout(String),
    /// Bridge is not enabled or not configured
    NotConfigured(String),
    /// Session not found or expired
    SessionNotFound(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionLost(msg) => write!(f, "bridge connection lost: {msg}"),
            Self::AuthenticationFailed(msg) => write!(f, "bridge auth failed: {msg}"),
            Self::RemoteError { code, message } => {
                write!(f, "bridge remote error [{code}]: {message}")
            }
            Self::Timeout(msg) => write!(f, "bridge timeout: {msg}"),
            Self::NotConfigured(msg) => write!(f, "bridge not configured: {msg}"),
            Self::SessionNotFound(msg) => write!(f, "bridge session not found: {msg}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Unique identifier for a bridge session (vehicle ↔ cloud).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BridgeSessionId(pub String);

impl fmt::Display for BridgeSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An active bridge session between cloud and vehicle instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeSession {
    /// Unique session identifier
    pub id: BridgeSessionId,
    /// Vehicle identifier (e.g. VIN or device ID)
    pub vehicle_id: String,
    /// Tenant context for the session (if multi-tenant)
    pub tenant_id: Option<String>,
    /// When the session was established (ISO 8601)
    pub connected_at: String,
    /// Remote peer address (for logging)
    pub remote_addr: Option<String>,
}

/// A SOVD request forwarded through the bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdBridgeRequest {
    /// Unique request ID for correlation
    pub request_id: String,
    /// HTTP method (GET, POST, PUT, DELETE)
    pub method: String,
    /// SOVD path (e.g. "/sovd/v1/components/hpc-main/data/0xF190")
    pub path: String,
    /// Optional request body (JSON)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
    /// Optional headers to forward
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub headers: std::collections::HashMap<String, String>,
}

/// A SOVD response returned through the bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SovdBridgeResponse {
    /// Correlation ID matching the request
    pub request_id: String,
    /// HTTP status code
    pub status: u16,
    /// Response body (JSON)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<serde_json::Value>,
}

/// Bridge transport abstraction for cloud-to-vehicle communication.
///
/// Implementations provide the actual tunnel mechanism:
/// - `WsBridgeTransport` — WebSocket relay (default, ADR A3.1)
/// - Future: gRPC tunnel, MQTT bridge
///
/// The cloud instance uses `accept_remote()` to wait for vehicle connections.
/// The vehicle instance uses an external connect mechanism, then the cloud
/// forwards requests via `forward_to_vehicle()`.
#[async_trait]
pub trait BridgeTransport: Send + Sync {
    /// Wait for and accept a new remote vehicle connection.
    ///
    /// Returns a `BridgeSession` representing the connected vehicle.
    /// This is called by the cloud instance's bridge relay loop.
    async fn accept_remote(&self) -> Result<BridgeSession, BridgeError>;

    /// Forward a SOVD request to the vehicle through the bridge tunnel.
    ///
    /// The bridge serializes the request, sends it to the vehicle instance,
    /// waits for the response, and returns it.
    async fn forward_to_vehicle(
        &self,
        session: &BridgeSession,
        request: SovdBridgeRequest,
    ) -> Result<SovdBridgeResponse, BridgeError>;

    /// Send a heartbeat/keepalive to the vehicle.
    ///
    /// Returns `Ok(())` if the vehicle is reachable, `Err` if the session
    /// has been lost.
    async fn heartbeat(&self, session: &BridgeSession) -> Result<(), BridgeError>;

    /// Gracefully disconnect a bridge session.
    async fn disconnect(&self, session: &BridgeSession) -> Result<(), BridgeError>;

    /// List all currently active bridge sessions.
    fn active_sessions(&self) -> Vec<BridgeSession>;
}

/// Bridge configuration (for config file).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// Enable bridge mode (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// Bridge listen address for incoming vehicle connections (cloud mode)
    /// e.g. "0.0.0.0:8443"
    #[serde(default)]
    pub listen_addr: Option<String>,
    /// Bridge connect URL for outgoing connection to cloud (vehicle mode)
    /// e.g. "wss://fleet.example.com/bridge"
    #[serde(default)]
    pub connect_url: Option<String>,
    /// Heartbeat interval in seconds (default: 30)
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_secs: u64,
    /// Reconnect backoff base in seconds (default: 5)
    #[serde(default = "default_reconnect_backoff")]
    pub reconnect_backoff_secs: u64,
    /// Maximum reconnect attempts (0 = unlimited)
    #[serde(default)]
    pub max_reconnect_attempts: u32,
    /// Vehicle identifier (for vehicle-side bridge client)
    #[serde(default)]
    pub vehicle_id: Option<String>,
}

fn default_heartbeat_interval() -> u64 {
    30
}

fn default_reconnect_backoff() -> u64 {
    5
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: None,
            connect_url: None,
            heartbeat_interval_secs: default_heartbeat_interval(),
            reconnect_backoff_secs: default_reconnect_backoff(),
            max_reconnect_attempts: 0,
            vehicle_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_error_display() {
        let err = BridgeError::ConnectionLost("peer disconnected".into());
        assert!(err.to_string().contains("peer disconnected"));
    }

    #[test]
    fn bridge_session_id_display() {
        let id = BridgeSessionId("sess-123".into());
        assert_eq!(id.to_string(), "sess-123");
    }

    #[test]
    fn bridge_config_defaults() {
        let config = BridgeConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.reconnect_backoff_secs, 5);
        assert_eq!(config.max_reconnect_attempts, 0);
    }

    #[test]
    fn sovd_bridge_request_serde_roundtrip() {
        let req = SovdBridgeRequest {
            request_id: "req-1".into(),
            method: "GET".into(),
            path: "/sovd/v1/components/hpc/data/0xF190".into(),
            body: None,
            headers: std::collections::HashMap::new(),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let parsed: SovdBridgeRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.request_id, "req-1");
        assert_eq!(parsed.method, "GET");
    }

    #[test]
    fn sovd_bridge_response_serde_roundtrip() {
        let resp = SovdBridgeResponse {
            request_id: "req-1".into(),
            status: 200,
            body: Some(serde_json::json!({"value": "WVWZZZ"})),
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        let parsed: SovdBridgeResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.status, 200);
    }

    #[test]
    fn bridge_session_serde() {
        let session = BridgeSession {
            id: BridgeSessionId("sess-abc".into()),
            vehicle_id: "VIN123".into(),
            tenant_id: Some("workshop-a".into()),
            connected_at: "2026-03-19T23:00:00Z".into(),
            remote_addr: Some("10.0.0.1:45678".into()),
        };
        let json = serde_json::to_string(&session).expect("serialize");
        let parsed: BridgeSession = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.vehicle_id, "VIN123");
        assert_eq!(parsed.tenant_id.as_deref(), Some("workshop-a"));
    }

    #[test]
    fn bridge_error_variants() {
        let errors = vec![
            BridgeError::ConnectionLost("lost".into()),
            BridgeError::AuthenticationFailed("bad cert".into()),
            BridgeError::RemoteError {
                code: "500".into(),
                message: "internal".into(),
            },
            BridgeError::Timeout("30s".into()),
            BridgeError::NotConfigured("disabled".into()),
            BridgeError::SessionNotFound("sess-x".into()),
        ];
        for err in &errors {
            assert!(!err.to_string().is_empty());
        }
    }
}
