// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Bridge relay — Cloud-to-vehicle SOVD request forwarding (Wave 3, W3.1)
//
// Implements a WebSocket-based bridge relay that allows cloud instances to
// forward SOVD requests to vehicle instances through a persistent tunnel.
//
// Architecture (ADR A3.1):
//   Cloud native-server (bridge=true) ←── WebSocket ──→ Vehicle native-server
//
// The bridge relay exposes REST endpoints under /sovd/v1/x-bridge/ for:
//   - Listing active bridge sessions
//   - Forwarding requests to a specific vehicle
//   - Health/heartbeat status of connected vehicles
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use dashmap::DashMap;
use native_interfaces::bridge::{
    BridgeConfig, BridgeError, BridgeSession, BridgeTransport, SovdBridgeRequest,
    SovdBridgeResponse,
};
use native_interfaces::sovd::SovdErrorEnvelope;
use serde::{Deserialize, Serialize};

/// In-memory bridge transport for development/testing.
///
/// Production deployments would use `WsBridgeTransport` (WebSocket over TLS)
/// or a gRPC-based transport. This implementation stores sessions in a DashMap
/// and simulates request forwarding with configurable responses.
pub struct InMemoryBridgeTransport {
    sessions: DashMap<String, BridgeSession>,
}

impl InMemoryBridgeTransport {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Register a vehicle session (for testing or local bridge mode).
    pub fn register_session(&self, session: BridgeSession) {
        self.sessions.insert(session.id.0.clone(), session);
    }

    /// Remove a session.
    pub fn remove_session(&self, session_id: &str) -> Option<BridgeSession> {
        self.sessions.remove(session_id).map(|(_, s)| s)
    }
}

impl Default for InMemoryBridgeTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl BridgeTransport for InMemoryBridgeTransport {
    async fn accept_remote(&self) -> Result<BridgeSession, BridgeError> {
        // In-memory transport doesn't accept real connections
        Err(BridgeError::NotConfigured(
            "in-memory transport does not accept remote connections".into(),
        ))
    }

    async fn forward_to_vehicle(
        &self,
        session: &BridgeSession,
        _request: SovdBridgeRequest,
    ) -> Result<SovdBridgeResponse, BridgeError> {
        if !self.sessions.contains_key(&session.id.0) {
            return Err(BridgeError::SessionNotFound(session.id.0.clone()));
        }
        // In-memory: return a stub "forwarded" response
        Ok(SovdBridgeResponse {
            request_id: _request.request_id,
            status: 200,
            body: Some(serde_json::json!({
                "bridged": true,
                "vehicle_id": session.vehicle_id,
                "note": "In-memory bridge stub — replace with WebSocket transport for production"
            })),
        })
    }

    async fn heartbeat(&self, session: &BridgeSession) -> Result<(), BridgeError> {
        if self.sessions.contains_key(&session.id.0) {
            Ok(())
        } else {
            Err(BridgeError::SessionNotFound(session.id.0.clone()))
        }
    }

    async fn disconnect(&self, session: &BridgeSession) -> Result<(), BridgeError> {
        self.sessions
            .remove(&session.id.0)
            .map(|_| ())
            .ok_or_else(|| BridgeError::SessionNotFound(session.id.0.clone()))
    }

    fn active_sessions(&self) -> Vec<BridgeSession> {
        self.sessions.iter().map(|e| e.value().clone()).collect()
    }
}

// ── Bridge state for axum handlers ──────────────────────────────────────────

/// Shared bridge state injected into bridge route handlers.
#[derive(Clone)]
pub struct BridgeState {
    pub transport: Arc<dyn BridgeTransport>,
    pub config: BridgeConfig,
}

// ── Bridge REST API handlers (/sovd/v1/x-bridge/) ──────────────────────────

/// List all active bridge sessions.
/// GET /sovd/v1/x-bridge/sessions
pub async fn list_sessions(State(bridge): State<BridgeState>) -> Json<serde_json::Value> {
    let sessions = bridge.transport.active_sessions();
    Json(serde_json::json!({
        "@odata.context": "$metadata#bridge-sessions",
        "value": sessions,
        "@odata.count": sessions.len(),
    }))
}

/// Get a specific bridge session by ID.
/// GET /sovd/v1/x-bridge/sessions/:session_id
pub async fn get_session(
    State(bridge): State<BridgeState>,
    Path(session_id): Path<String>,
) -> Result<Json<BridgeSession>, (StatusCode, Json<SovdErrorEnvelope>)> {
    bridge
        .transport
        .active_sessions()
        .into_iter()
        .find(|s| s.id.0 == session_id)
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(SovdErrorEnvelope::new(
                    "SOVD-ERR-404",
                    format!("Bridge session '{session_id}' not found"),
                )),
            )
        })
}

/// Forward a SOVD request to a vehicle through the bridge.
/// POST /sovd/v1/x-bridge/sessions/:session_id/forward
#[derive(Deserialize)]
pub struct ForwardRequest {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

pub async fn forward_to_vehicle(
    State(bridge): State<BridgeState>,
    Path(session_id): Path<String>,
    Json(payload): Json<ForwardRequest>,
) -> Result<Json<SovdBridgeResponse>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let session = bridge
        .transport
        .active_sessions()
        .into_iter()
        .find(|s| s.id.0 == session_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(SovdErrorEnvelope::new(
                    "SOVD-ERR-404",
                    format!("Bridge session '{session_id}' not found"),
                )),
            )
        })?;

    let request = SovdBridgeRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        method: payload.method,
        path: payload.path,
        body: payload.body,
        headers: payload.headers,
    };

    bridge
        .transport
        .forward_to_vehicle(&session, request)
        .await
        .map(Json)
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(SovdErrorEnvelope::new("SOVD-ERR-502", e.to_string())),
            )
        })
}

/// Send heartbeat to a vehicle session.
/// POST /sovd/v1/x-bridge/sessions/:session_id/heartbeat
pub async fn heartbeat_session(
    State(bridge): State<BridgeState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    let session = bridge
        .transport
        .active_sessions()
        .into_iter()
        .find(|s| s.id.0 == session_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(SovdErrorEnvelope::new(
                    "SOVD-ERR-404",
                    format!("Bridge session '{session_id}' not found"),
                )),
            )
        })?;

    bridge
        .transport
        .heartbeat(&session)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                Json(SovdErrorEnvelope::new("SOVD-ERR-502", e.to_string())),
            )
        })
}

/// Disconnect a vehicle bridge session.
/// DELETE /sovd/v1/x-bridge/sessions/:session_id
pub async fn disconnect_session(
    State(bridge): State<BridgeState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    let session = bridge
        .transport
        .active_sessions()
        .into_iter()
        .find(|s| s.id.0 == session_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(SovdErrorEnvelope::new(
                    "SOVD-ERR-404",
                    format!("Bridge session '{session_id}' not found"),
                )),
            )
        })?;

    bridge
        .transport
        .disconnect(&session)
        .await
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SovdErrorEnvelope::new("SOVD-ERR-500", e.to_string())),
            )
        })
}

/// Bridge status overview.
/// GET /sovd/v1/x-bridge/status
#[derive(Serialize)]
pub struct BridgeStatus {
    pub enabled: bool,
    pub active_sessions: usize,
    pub mode: String,
}

pub async fn bridge_status(State(bridge): State<BridgeState>) -> Json<BridgeStatus> {
    let mode = if bridge.config.listen_addr.is_some() {
        "cloud-relay"
    } else if bridge.config.connect_url.is_some() {
        "vehicle-client"
    } else {
        "disabled"
    };
    Json(BridgeStatus {
        enabled: bridge.config.enabled,
        active_sessions: bridge.transport.active_sessions().len(),
        mode: mode.to_owned(),
    })
}

/// Build the bridge sub-router.
///
/// Mounted at `/sovd/v1/x-bridge` in the main router when bridge mode is enabled.
pub fn build_bridge_router(state: BridgeState) -> axum::Router {
    use axum::routing::{delete, get, post};

    axum::Router::new()
        .route("/status", get(bridge_status))
        .route("/sessions", get(list_sessions))
        .route("/sessions/{session_id}", get(get_session))
        .route("/sessions/{session_id}/forward", post(forward_to_vehicle))
        .route("/sessions/{session_id}/heartbeat", post(heartbeat_session))
        .route("/sessions/{session_id}", delete(disconnect_session))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_interfaces::bridge::BridgeSessionId;

    fn test_session(id: &str, vehicle: &str) -> BridgeSession {
        BridgeSession {
            id: BridgeSessionId(id.into()),
            vehicle_id: vehicle.into(),
            tenant_id: None,
            connected_at: "2026-03-19T23:00:00Z".into(),
            remote_addr: None,
        }
    }

    #[test]
    fn in_memory_transport_register_and_list() {
        let transport = InMemoryBridgeTransport::new();
        assert!(transport.active_sessions().is_empty());

        transport.register_session(test_session("s1", "VIN-001"));
        transport.register_session(test_session("s2", "VIN-002"));

        let sessions = transport.active_sessions();
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn in_memory_transport_remove() {
        let transport = InMemoryBridgeTransport::new();
        transport.register_session(test_session("s1", "VIN-001"));

        let removed = transport.remove_session("s1");
        assert!(removed.is_some());
        assert!(transport.active_sessions().is_empty());

        let not_found = transport.remove_session("nonexistent");
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn in_memory_transport_heartbeat() {
        let transport = InMemoryBridgeTransport::new();
        let session = test_session("s1", "VIN-001");
        transport.register_session(session.clone());

        assert!(transport.heartbeat(&session).await.is_ok());

        transport.remove_session("s1");
        assert!(transport.heartbeat(&session).await.is_err());
    }

    #[tokio::test]
    async fn in_memory_transport_forward() {
        let transport = InMemoryBridgeTransport::new();
        let session = test_session("s1", "VIN-001");
        transport.register_session(session.clone());

        let req = SovdBridgeRequest {
            request_id: "req-1".into(),
            method: "GET".into(),
            path: "/sovd/v1/components".into(),
            body: None,
            headers: HashMap::new(),
        };

        let resp = transport.forward_to_vehicle(&session, req).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.request_id, "req-1");
        assert!(resp.body.is_some());
    }

    #[tokio::test]
    async fn in_memory_transport_forward_unknown_session() {
        let transport = InMemoryBridgeTransport::new();
        let session = test_session("nonexistent", "VIN-X");

        let req = SovdBridgeRequest {
            request_id: "req-2".into(),
            method: "GET".into(),
            path: "/sovd/v1/".into(),
            body: None,
            headers: HashMap::new(),
        };

        let result = transport.forward_to_vehicle(&session, req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn in_memory_transport_disconnect() {
        let transport = InMemoryBridgeTransport::new();
        let session = test_session("s1", "VIN-001");
        transport.register_session(session.clone());

        assert!(transport.disconnect(&session).await.is_ok());
        assert!(transport.active_sessions().is_empty());

        // Double disconnect should fail
        assert!(transport.disconnect(&session).await.is_err());
    }

    #[tokio::test]
    async fn in_memory_transport_accept_returns_not_configured() {
        let transport = InMemoryBridgeTransport::new();
        let result = transport.accept_remote().await;
        assert!(result.is_err());
    }

    #[test]
    fn bridge_status_serializes() {
        let status = BridgeStatus {
            enabled: true,
            active_sessions: 3,
            mode: "cloud-relay".into(),
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["active_sessions"], 3);
        assert_eq!(json["mode"], "cloud-relay");
    }
}
