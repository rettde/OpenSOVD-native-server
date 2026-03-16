// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// demo-ecu — Mock ECU exposing SOVD-compatible REST endpoints
//
// This example simulates a Battery Management System (BMS) and a Cabin
// Climate Controller. Start it alongside the OpenSOVD-native-server to
// demonstrate the gateway use-case:
//
//   1. cargo run -p demo-ecu                    (starts on :3001)
//   2. cargo run -p opensovd-native-server      (starts on :8080, forwards to :3001)
//   3. curl http://localhost:8080/sovd/v1/components | jq
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::{fmt, EnvFilter};

// ─────────────────────────────────────────────────────────────────────────────
// Data model
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct Component {
    id: String,
    name: String,
    category: String,
    #[serde(rename = "connectionState")]
    connection_state: String,
}

#[derive(Clone, Serialize)]
struct DataItem {
    id: String,
    name: String,
    value: serde_json::Value,
    unit: Option<String>,
    access: String,
}

#[derive(Clone, Serialize)]
struct Fault {
    id: String,
    #[serde(rename = "displayCode")]
    display_code: String,
    message: String,
    severity: String,
    #[serde(rename = "isActive")]
    is_active: bool,
    timestamp: String,
}

#[derive(Clone, Serialize)]
struct Operation {
    id: String,
    name: String,
    description: String,
}

#[derive(Deserialize)]
struct OperationRequest {
    #[serde(default)]
    parameters: Option<serde_json::Value>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared state
// ─────────────────────────────────────────────────────────────────────────────

struct EcuState {
    components: Vec<Component>,
    data: Vec<(String, DataItem)>,        // (component_id, item)
    faults: Vec<(String, Fault)>,         // (component_id, fault)
    operations: Vec<(String, Operation)>, // (component_id, op)
}

type SharedState = Arc<RwLock<EcuState>>;

#[allow(clippy::too_many_lines)]
fn build_initial_state() -> EcuState {
    let components = vec![
        Component {
            id: "bms".into(),
            name: "Battery Management System".into(),
            category: "powertrain".into(),
            connection_state: "connected".into(),
        },
        Component {
            id: "climate".into(),
            name: "Cabin Climate Controller".into(),
            category: "body".into(),
            connection_state: "connected".into(),
        },
    ];

    let data = vec![
        // BMS data
        (
            "bms".into(),
            DataItem {
                id: "soc".into(),
                name: "State of Charge".into(),
                value: serde_json::json!(78.5),
                unit: Some("%".into()),
                access: "read-only".into(),
            },
        ),
        (
            "bms".into(),
            DataItem {
                id: "voltage".into(),
                name: "Pack Voltage".into(),
                value: serde_json::json!(396.2),
                unit: Some("V".into()),
                access: "read-only".into(),
            },
        ),
        (
            "bms".into(),
            DataItem {
                id: "cell-temp-max".into(),
                name: "Max Cell Temperature".into(),
                value: serde_json::json!(34.1),
                unit: Some("°C".into()),
                access: "read-only".into(),
            },
        ),
        (
            "bms".into(),
            DataItem {
                id: "charge-limit".into(),
                name: "Charge Limit".into(),
                value: serde_json::json!(80),
                unit: Some("%".into()),
                access: "read-write".into(),
            },
        ),
        // Climate data
        (
            "climate".into(),
            DataItem {
                id: "cabin-temp".into(),
                name: "Cabin Temperature".into(),
                value: serde_json::json!(22.3),
                unit: Some("°C".into()),
                access: "read-only".into(),
            },
        ),
        (
            "climate".into(),
            DataItem {
                id: "target-temp".into(),
                name: "Target Temperature".into(),
                value: serde_json::json!(21.0),
                unit: Some("°C".into()),
                access: "read-write".into(),
            },
        ),
        (
            "climate".into(),
            DataItem {
                id: "fan-speed".into(),
                name: "Fan Speed".into(),
                value: serde_json::json!(3),
                unit: None,
                access: "read-write".into(),
            },
        ),
    ];

    let faults = vec![(
        "bms".into(),
        Fault {
            id: "bms-f001".into(),
            display_code: "P0A80".into(),
            message: "Cell voltage imbalance detected".into(),
            severity: "warning".into(),
            is_active: true,
            timestamp: chrono::Utc::now().to_rfc3339(),
        },
    )];

    let operations = vec![
        (
            "bms".into(),
            Operation {
                id: "self-test".into(),
                name: "Battery Self Test".into(),
                description: "Run full cell balance and isolation test".into(),
            },
        ),
        (
            "bms".into(),
            Operation {
                id: "cell-balance".into(),
                name: "Cell Balancing".into(),
                description: "Trigger active cell balancing cycle".into(),
            },
        ),
        (
            "climate".into(),
            Operation {
                id: "recirculate".into(),
                name: "Recirculation Flush".into(),
                description: "Flush cabin air through recirculation filter".into(),
            },
        ),
    ];

    EcuState {
        components,
        data,
        faults,
        operations,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SOVD-compatible REST handlers
// ─────────────────────────────────────────────────────────────────────────────

fn odata_collection<T: Serialize>(context: &str, items: &[T]) -> serde_json::Value {
    serde_json::json!({
        "@odata.context": context,
        "@odata.count": items.len(),
        "value": items,
    })
}

async fn server_info() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "serverName": "demo-ecu",
        "serverVersion": "0.5.0",
        "sovdVersion": "1.1.0",
        "supportedProtocols": ["http"],
    }))
}

async fn list_components(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(odata_collection("$metadata#components", &s.components))
}

async fn get_component(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let s = state.read().await;
    s.components
        .iter()
        .find(|c| c.id == id)
        .map(|c| Json(serde_json::json!({"@odata.context": "$metadata#components/$entity", "id": c.id, "name": c.name, "category": c.category, "connectionState": c.connection_state})))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn list_data(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let s = state.read().await;
    let items: Vec<_> = s
        .data
        .iter()
        .filter(|(cid, _)| *cid == component_id)
        .map(|(_, d)| d)
        .collect();
    if items.is_empty() && !s.components.iter().any(|c| c.id == component_id) {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(odata_collection("$metadata#data", &items)))
}

async fn read_data(
    State(state): State<SharedState>,
    Path((component_id, data_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let s = state.read().await;
    s.data
        .iter()
        .find(|(cid, d)| *cid == component_id && d.id == data_id)
        .map(|(_, d)| Json(serde_json::json!({"@odata.context": "$metadata#data/$entity", "id": d.id, "name": d.name, "value": d.value, "unit": d.unit, "access": d.access})))
        .ok_or(StatusCode::NOT_FOUND)
}

async fn write_data(
    State(state): State<SharedState>,
    Path((component_id, data_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut s = state.write().await;
    if let Some((_, item)) = s
        .data
        .iter_mut()
        .find(|(cid, d)| *cid == component_id && d.id == data_id)
    {
        if item.access != "read-write" {
            return Err((
                StatusCode::FORBIDDEN,
                Json(
                    serde_json::json!({"error": {"code": "SOVD-ERR-403", "message": "Data item is read-only"}}),
                ),
            ));
        }
        if let Some(val) = body.get("value") {
            item.value = val.clone();
            info!(component = %component_id, data = %data_id, value = %val, "Data written");
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err((
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({"error": {"code": "SOVD-ERR-400", "message": "Missing 'value' field"}}),
                ),
            ))
        }
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({"error": {"code": "SOVD-ERR-404", "message": "Data item not found"}}),
            ),
        ))
    }
}

async fn list_faults(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let s = state.read().await;
    let items: Vec<_> = s
        .faults
        .iter()
        .filter(|(cid, _)| *cid == component_id)
        .map(|(_, f)| f)
        .collect();
    Ok(Json(odata_collection("$metadata#faults", &items)))
}

async fn clear_faults(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> StatusCode {
    let mut s = state.write().await;
    s.faults.retain(|(cid, _)| *cid != component_id);
    info!(component = %component_id, "Faults cleared");
    StatusCode::NO_CONTENT
}

async fn list_operations(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let s = state.read().await;
    let items: Vec<_> = s
        .operations
        .iter()
        .filter(|(cid, _)| *cid == component_id)
        .map(|(_, o)| o)
        .collect();
    Ok(Json(odata_collection("$metadata#operations", &items)))
}

async fn execute_operation(
    State(_state): State<SharedState>,
    Path((component_id, operation_id)): Path<(String, String)>,
    Json(body): Json<OperationRequest>,
) -> impl IntoResponse {
    let exec_id = uuid::Uuid::new_v4().to_string();
    info!(
        component = %component_id,
        operation = %operation_id,
        params = ?body.parameters,
        execution = %exec_id,
        "Operation executed"
    );
    (
        StatusCode::ACCEPTED,
        [(
            "location",
            format!(
                "/sovd/v1/components/{component_id}/operations/{operation_id}/executions/{exec_id}"
            ),
        )],
        Json(serde_json::json!({
            "executionId": exec_id,
            "status": "completed",
            "result": { "success": true, "message": format!("Operation '{operation_id}' completed on '{component_id}'") }
        })),
    )
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "serverName": "demo-ecu",
        "uptime": "running",
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();

    let state: SharedState = Arc::new(RwLock::new(build_initial_state()));

    let app = Router::new()
        // Discovery
        .route("/sovd/v1", get(server_info))
        .route("/sovd/v1/health", get(health))
        // Components
        .route("/sovd/v1/components", get(list_components))
        .route("/sovd/v1/components/{component_id}", get(get_component))
        // Data
        .route("/sovd/v1/components/{component_id}/data", get(list_data))
        .route(
            "/sovd/v1/components/{component_id}/data/{data_id}",
            get(read_data),
        )
        .route(
            "/sovd/v1/components/{component_id}/data/{data_id}",
            axum::routing::put(write_data),
        )
        // Faults
        .route(
            "/sovd/v1/components/{component_id}/faults",
            get(list_faults),
        )
        .route(
            "/sovd/v1/components/{component_id}/faults",
            delete(clear_faults),
        )
        // Operations
        .route(
            "/sovd/v1/components/{component_id}/operations",
            get(list_operations),
        )
        .route(
            "/sovd/v1/components/{component_id}/operations/{operation_id}",
            post(execute_operation),
        )
        .with_state(state);

    let addr = "0.0.0.0:3001";
    info!("demo-ecu listening on http://{addr}/sovd/v1");
    info!("Components: bms (Battery Management), climate (Cabin Climate)");
    info!("Configure OpenSOVD-native-server to forward to this backend:");
    info!("  [[backends]]");
    info!("  name = \"demo-ecu\"");
    info!("  base_url = \"http://localhost:3001\"");
    info!("  component_ids = [\"bms\", \"climate\"]");

    #[allow(clippy::expect_used)] // Unrecoverable: cannot start without a listener
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind");
    #[allow(clippy::expect_used)] // Unrecoverable: server loop failure
    axum::serve(listener, app).await.expect("Server error");
}
