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
//
// All JSON responses conform to ISO 17978-3 (SOVD) data types so that the
// SovdHttpBackend can parse them without errors.
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
// Data model — mirrors native-interfaces/src/sovd.rs structs exactly
// ─────────────────────────────────────────────────────────────────────────────

/// Matches `SovdComponent` from native-interfaces
#[derive(Clone, Serialize)]
struct Component {
    id: String,
    name: String,
    category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "connectionState")]
    connection_state: String,
}

/// Matches `SovdDataCatalogEntry` from native-interfaces
#[derive(Clone, Serialize)]
struct DataCatalogEntry {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    access: String,
    #[serde(rename = "dataType")]
    data_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
}

/// Runtime data value (not part of the SOVD catalog, but returned on read)
#[derive(Clone, Serialize)]
struct DataValue {
    id: String,
    name: String,
    value: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    access: String,
}

/// Matches `SovdFault` from native-interfaces
#[derive(Clone, Serialize)]
struct Fault {
    id: String,
    #[serde(rename = "componentId")]
    component_id: String,
    code: String,
    #[serde(skip_serializing_if = "Option::is_none", rename = "displayCode")]
    display_code: Option<String>,
    severity: String,
    status: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

/// Matches `SovdOperation` from native-interfaces
#[derive(Clone, Serialize)]
struct Operation {
    id: String,
    #[serde(rename = "componentId")]
    component_id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    status: String,
}

/// Matches `SovdGroup` from native-interfaces
#[derive(Clone, Serialize)]
struct Group {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "componentIds")]
    component_ids: Vec<String>,
}

#[derive(Deserialize)]
struct OperationRequest {
    #[serde(default)]
    params: Option<String>,
}

#[derive(Deserialize)]
struct SetModeRequest {
    mode: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared state
// ─────────────────────────────────────────────────────────────────────────────

struct EcuState {
    components: Vec<Component>,
    groups: Vec<Group>,
    data_catalog: Vec<(String, DataCatalogEntry)>, // (component_id, entry)
    data_values: Vec<(String, DataValue)>,         // (component_id, value)
    faults: Vec<Fault>,
    operations: Vec<Operation>,
    modes: std::collections::HashMap<String, String>, // component_id → current mode
}

type SharedState = Arc<RwLock<EcuState>>;

#[allow(clippy::too_many_lines)]
fn build_initial_state() -> EcuState {
    let components = vec![
        Component {
            id: "bms".into(),
            name: "Battery Management System".into(),
            category: "powertrain".into(),
            description: Some("HV battery management ECU".into()),
            connection_state: "connected".into(),
        },
        Component {
            id: "climate".into(),
            name: "Cabin Climate Controller".into(),
            category: "body".into(),
            description: Some("HVAC and cabin comfort ECU".into()),
            connection_state: "connected".into(),
        },
    ];

    let groups = vec![
        Group {
            id: "powertrain".into(),
            name: "Powertrain".into(),
            description: Some("Powertrain domain ECUs".into()),
            component_ids: vec!["bms".into()],
        },
        Group {
            id: "body".into(),
            name: "Body".into(),
            description: Some("Body domain ECUs".into()),
            component_ids: vec!["climate".into()],
        },
    ];

    let data_catalog = vec![
        (
            "bms".into(),
            DataCatalogEntry {
                id: "soc".into(),
                name: "State of Charge".into(),
                description: None,
                access: "readOnly".into(),
                data_type: "float".into(),
                unit: Some("%".into()),
            },
        ),
        (
            "bms".into(),
            DataCatalogEntry {
                id: "voltage".into(),
                name: "Pack Voltage".into(),
                description: None,
                access: "readOnly".into(),
                data_type: "float".into(),
                unit: Some("V".into()),
            },
        ),
        (
            "bms".into(),
            DataCatalogEntry {
                id: "cell-temp-max".into(),
                name: "Max Cell Temperature".into(),
                description: None,
                access: "readOnly".into(),
                data_type: "float".into(),
                unit: Some("°C".into()),
            },
        ),
        (
            "bms".into(),
            DataCatalogEntry {
                id: "charge-limit".into(),
                name: "Charge Limit".into(),
                description: None,
                access: "readWrite".into(),
                data_type: "integer".into(),
                unit: Some("%".into()),
            },
        ),
        (
            "climate".into(),
            DataCatalogEntry {
                id: "cabin-temp".into(),
                name: "Cabin Temperature".into(),
                description: None,
                access: "readOnly".into(),
                data_type: "float".into(),
                unit: Some("°C".into()),
            },
        ),
        (
            "climate".into(),
            DataCatalogEntry {
                id: "target-temp".into(),
                name: "Target Temperature".into(),
                description: None,
                access: "readWrite".into(),
                data_type: "float".into(),
                unit: Some("°C".into()),
            },
        ),
        (
            "climate".into(),
            DataCatalogEntry {
                id: "fan-speed".into(),
                name: "Fan Speed".into(),
                description: None,
                access: "readWrite".into(),
                data_type: "integer".into(),
                unit: None,
            },
        ),
    ];

    let data_values = vec![
        (
            "bms".into(),
            DataValue {
                id: "soc".into(),
                name: "State of Charge".into(),
                value: serde_json::json!(78.5),
                unit: Some("%".into()),
                access: "readOnly".into(),
            },
        ),
        (
            "bms".into(),
            DataValue {
                id: "voltage".into(),
                name: "Pack Voltage".into(),
                value: serde_json::json!(396.2),
                unit: Some("V".into()),
                access: "readOnly".into(),
            },
        ),
        (
            "bms".into(),
            DataValue {
                id: "cell-temp-max".into(),
                name: "Max Cell Temperature".into(),
                value: serde_json::json!(34.1),
                unit: Some("°C".into()),
                access: "readOnly".into(),
            },
        ),
        (
            "bms".into(),
            DataValue {
                id: "charge-limit".into(),
                name: "Charge Limit".into(),
                value: serde_json::json!(80),
                unit: Some("%".into()),
                access: "readWrite".into(),
            },
        ),
        (
            "climate".into(),
            DataValue {
                id: "cabin-temp".into(),
                name: "Cabin Temperature".into(),
                value: serde_json::json!(22.3),
                unit: Some("°C".into()),
                access: "readOnly".into(),
            },
        ),
        (
            "climate".into(),
            DataValue {
                id: "target-temp".into(),
                name: "Target Temperature".into(),
                value: serde_json::json!(21.0),
                unit: Some("°C".into()),
                access: "readWrite".into(),
            },
        ),
        (
            "climate".into(),
            DataValue {
                id: "fan-speed".into(),
                name: "Fan Speed".into(),
                value: serde_json::json!(3),
                unit: None,
                access: "readWrite".into(),
            },
        ),
    ];

    let faults = vec![Fault {
        id: "bms-f001".into(),
        component_id: "bms".into(),
        code: "P0A80".into(),
        display_code: Some("P0A80".into()),
        severity: "high".into(),
        status: "active".into(),
        name: "Cell Voltage Imbalance".into(),
        description: Some("Cell voltage imbalance detected in module 3".into()),
    }];

    let operations = vec![
        Operation {
            id: "self-test".into(),
            component_id: "bms".into(),
            name: "Battery Self Test".into(),
            description: Some("Run full cell balance and isolation test".into()),
            status: "idle".into(),
        },
        Operation {
            id: "cell-balance".into(),
            component_id: "bms".into(),
            name: "Cell Balancing".into(),
            description: Some("Trigger active cell balancing cycle".into()),
            status: "idle".into(),
        },
        Operation {
            id: "recirculate".into(),
            component_id: "climate".into(),
            name: "Recirculation Flush".into(),
            description: Some("Flush cabin air through recirculation filter".into()),
            status: "idle".into(),
        },
    ];

    let mut modes = std::collections::HashMap::new();
    modes.insert("bms".into(), "default".into());
    modes.insert("climate".into(), "default".into());

    EcuState {
        components,
        groups,
        data_catalog,
        data_values,
        faults,
        operations,
        modes,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OData helpers
// ─────────────────────────────────────────────────────────────────────────────

fn odata_collection<T: Serialize>(context: &str, items: &[T]) -> serde_json::Value {
    serde_json::json!({
        "@odata.context": context,
        "@odata.count": items.len(),
        "value": items,
    })
}

fn sovd_error(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message } })
}

// ─────────────────────────────────────────────────────────────────────────────
// SOVD-conformant REST handlers (ISO 17978-3)
// ─────────────────────────────────────────────────────────────────────────────

// §5.1 Discovery
async fn server_info() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "serverName": "demo-ecu",
        "serverVersion": "0.5.0",
        "sovdVersion": "1.1.0",
        "description": "Mock ECU backend — BMS + Climate Controller",
        "supportedProtocols": ["http/1.1"],
    }))
}

// §7.1 Components
async fn list_components(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(odata_collection("$metadata#components", &s.components))
}

async fn get_component(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Component>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    s.components
        .iter()
        .find(|c| c.id == id)
        .cloned()
        .map(Json)
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Component '{id}' not found"),
            )),
        ))
}

// §7.2 Groups
async fn list_groups(State(state): State<SharedState>) -> Json<serde_json::Value> {
    let s = state.read().await;
    Json(odata_collection("$metadata#groups", &s.groups))
}

async fn get_group(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<Group>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    s.groups
        .iter()
        .find(|g| g.id == id)
        .cloned()
        .map(Json)
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Group '{id}' not found"),
            )),
        ))
}

async fn get_group_components(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    let group = s.groups.iter().find(|g| g.id == id).ok_or((
        StatusCode::NOT_FOUND,
        Json(sovd_error(
            "SOVD-ERR-404",
            &format!("Group '{id}' not found"),
        )),
    ))?;
    let members: Vec<_> = s
        .components
        .iter()
        .filter(|c| group.component_ids.contains(&c.id))
        .collect();
    Ok(Json(odata_collection("$metadata#components", &members)))
}

// §7.3 Capabilities
async fn get_capabilities(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    if !s.components.iter().any(|c| c.id == component_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Component '{component_id}' not found"),
            )),
        ));
    }
    let data_count = s
        .data_catalog
        .iter()
        .filter(|(cid, _)| *cid == component_id)
        .count();
    let op_count = s
        .operations
        .iter()
        .filter(|o| o.component_id == component_id)
        .count();
    Ok(Json(serde_json::json!({
        "componentId": component_id,
        "supportedCategories": ["data", "faults", "operations"],
        "dataCount": data_count,
        "operationCount": op_count,
        "features": ["faults", "operations", "data"],
    })))
}

// §7.5 Data
async fn list_data(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    if !s.components.iter().any(|c| c.id == component_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Component '{component_id}' not found"),
            )),
        ));
    }
    let items: Vec<_> = s
        .data_catalog
        .iter()
        .filter(|(cid, _)| *cid == component_id)
        .map(|(_, d)| d)
        .collect();
    Ok(Json(odata_collection("$metadata#data", &items)))
}

async fn read_data(
    State(state): State<SharedState>,
    Path((component_id, data_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    s.data_values
        .iter()
        .find(|(cid, d)| *cid == component_id && d.id == data_id)
        .map(|(_, d)| Json(serde_json::to_value(d).unwrap_or_default()))
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Data '{data_id}' not found"),
            )),
        ))
}

async fn write_data(
    State(state): State<SharedState>,
    Path((component_id, data_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let mut s = state.write().await;
    if let Some((_, item)) = s
        .data_values
        .iter_mut()
        .find(|(cid, d)| *cid == component_id && d.id == data_id)
    {
        if item.access == "readOnly" {
            return Err((
                StatusCode::FORBIDDEN,
                Json(sovd_error("SOVD-ERR-403", "Data item is read-only")),
            ));
        }
        if let Some(val) = body.get("value") {
            item.value = val.clone();
            info!(component = %component_id, data = %data_id, value = %val, "Data written");
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err((
                StatusCode::BAD_REQUEST,
                Json(sovd_error("SOVD-ERR-400", "Missing 'value' field")),
            ))
        }
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(sovd_error("SOVD-ERR-404", "Data item not found")),
        ))
    }
}

// §7.6 Faults
async fn list_faults(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    if !s.components.iter().any(|c| c.id == component_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Component '{component_id}' not found"),
            )),
        ));
    }
    let items: Vec<_> = s
        .faults
        .iter()
        .filter(|f| f.component_id == component_id)
        .collect();
    Ok(Json(odata_collection("$metadata#faults", &items)))
}

async fn clear_faults(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> StatusCode {
    let mut s = state.write().await;
    s.faults.retain(|f| f.component_id != component_id);
    info!(component = %component_id, "Faults cleared");
    StatusCode::NO_CONTENT
}

// §7.7 Operations
async fn list_operations(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    if !s.components.iter().any(|c| c.id == component_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Component '{component_id}' not found"),
            )),
        ));
    }
    let items: Vec<_> = s
        .operations
        .iter()
        .filter(|o| o.component_id == component_id)
        .collect();
    Ok(Json(odata_collection("$metadata#operations", &items)))
}

async fn execute_operation(
    State(state): State<SharedState>,
    Path((component_id, operation_id)): Path<(String, String)>,
    Json(body): Json<OperationRequest>,
) -> Result<impl IntoResponse, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    if !s
        .operations
        .iter()
        .any(|o| o.component_id == component_id && o.id == operation_id)
    {
        return Err((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Operation '{operation_id}' not found on '{component_id}'"),
            )),
        ));
    }
    drop(s);
    let exec_id = uuid::Uuid::new_v4().to_string();
    info!(
        component = %component_id,
        operation = %operation_id,
        params = ?body.params,
        execution = %exec_id,
        "Operation executed"
    );
    Ok((
        StatusCode::ACCEPTED,
        [(
            "location",
            format!(
                "/sovd/v1/components/{component_id}/operations/{operation_id}/executions/{exec_id}"
            ),
        )],
        Json(serde_json::json!({
            "executionId": exec_id,
            "componentId": component_id,
            "operationId": operation_id,
            "status": "completed",
            "progress": 100,
            "result": { "success": true, "message": format!("Operation '{operation_id}' completed on '{component_id}'") },
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })),
    ))
}

// §7.6 Mode / Session
async fn get_mode(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    let mode = s.modes.get(&component_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(sovd_error(
            "SOVD-ERR-404",
            &format!("Component '{component_id}' not found"),
        )),
    ))?;
    Ok(Json(serde_json::json!({
        "componentId": component_id,
        "currentMode": mode,
        "availableModes": ["default", "extended", "programming"],
    })))
}

async fn set_mode(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
    Json(body): Json<SetModeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let mut s = state.write().await;
    let mode = s.modes.get_mut(&component_id).ok_or((
        StatusCode::NOT_FOUND,
        Json(sovd_error(
            "SOVD-ERR-404",
            &format!("Component '{component_id}' not found"),
        )),
    ))?;
    mode.clone_from(&body.mode);
    info!(component = %component_id, mode = %body.mode, "Mode changed");
    Ok(Json(serde_json::json!({
        "componentId": component_id,
        "currentMode": body.mode,
        "availableModes": ["default", "extended", "programming"],
    })))
}

// §7.8 Configuration
async fn read_config(
    State(state): State<SharedState>,
    Path(component_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let s = state.read().await;
    if !s.components.iter().any(|c| c.id == component_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(sovd_error(
                "SOVD-ERR-404",
                &format!("Component '{component_id}' not found"),
            )),
        ));
    }
    Ok(Json(serde_json::json!({
        "componentId": component_id,
        "parameters": {},
    })))
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
        // Discovery (§5.1)
        .route("/sovd/v1", get(server_info))
        .route("/sovd/v1/health", get(health))
        // Components (§7.1)
        .route("/sovd/v1/components", get(list_components))
        .route("/sovd/v1/components/{component_id}", get(get_component))
        // Groups (§7.2)
        .route("/sovd/v1/groups", get(list_groups))
        .route("/sovd/v1/groups/{group_id}", get(get_group))
        .route(
            "/sovd/v1/groups/{group_id}/components",
            get(get_group_components),
        )
        // Capabilities (§7.3)
        .route(
            "/sovd/v1/components/{component_id}/capabilities",
            get(get_capabilities),
        )
        // Data (§7.5)
        .route("/sovd/v1/components/{component_id}/data", get(list_data))
        .route(
            "/sovd/v1/components/{component_id}/data/{data_id}",
            get(read_data),
        )
        .route(
            "/sovd/v1/components/{component_id}/data/{data_id}",
            axum::routing::put(write_data),
        )
        // Faults (§7.6)
        .route(
            "/sovd/v1/components/{component_id}/faults",
            get(list_faults),
        )
        .route(
            "/sovd/v1/components/{component_id}/faults",
            delete(clear_faults),
        )
        // Operations (§7.7)
        .route(
            "/sovd/v1/components/{component_id}/operations",
            get(list_operations),
        )
        .route(
            "/sovd/v1/components/{component_id}/operations/{operation_id}",
            post(execute_operation),
        )
        // Mode (§7.6)
        .route("/sovd/v1/components/{component_id}/mode", get(get_mode))
        .route("/sovd/v1/components/{component_id}/mode", post(set_mode))
        // Configuration (§7.8)
        .route(
            "/sovd/v1/components/{component_id}/config",
            get(read_config),
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
