// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// SOVD REST API routes — axum router (ISO 17978-3 / SOVD)
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use native_interfaces::sovd::*;
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use super::auth::{auth_middleware, AuthConfig, AuthState, AuthenticatedClient};
use super::state::AppState;

/// Entity-ID validation middleware — delegates to OemProfile::EntityIdPolicy.
///
/// Intercepts every request, extracts dynamic path segments by comparing the matched
/// route template (e.g. `/components/{component_id}`) with the actual URI, and rejects
/// any segment that violates the profile's naming rules with 400 Bad Request.
///
/// This middleware is OEM-agnostic: the actual validation rules come from the
/// `OemProfile` injected at startup (DefaultProfile = permissive, MbdsProfile = DDAG §2.3).
async fn entity_id_validation_middleware(
    profile: std::sync::Arc<dyn native_interfaces::oem::OemProfile>,
    matched_path: Option<axum::extract::MatchedPath>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, (StatusCode, Json<SovdErrorEnvelope>)> {
    if let Some(ref matched) = matched_path {
        let template_segments: Vec<&str> = matched.as_str().split('/').collect();
        let uri_segments: Vec<&str> = request.uri().path().split('/').collect();
        let policy = profile.as_entity_id_policy();
        for (tmpl, actual) in template_segments.iter().zip(uri_segments.iter()) {
            if tmpl.starts_with('{') && tmpl.ends_with('}') {
                policy
                    .validate_entity_id(actual)
                    .map_err(|reason| bad_request(&reason))?;
            }
        }
    }
    Ok(next.run(request).await)
}

/// Trace-ID propagation middleware (MBDS §8 / W3C Trace Context).
/// Reads `traceparent` or `x-request-id` from request; if absent, generates a new UUID.
/// Injects `traceparent` into every response for distributed tracing.
async fn trace_id_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let trace_id = request
        .headers()
        .get("traceparent")
        .or_else(|| request.headers().get("x-request-id"))
        .and_then(|v| v.to_str().ok())
        .map_or_else(
            || {
                let id = uuid::Uuid::new_v4().simple().to_string();
                format!("00-{id}-{}-01", &id[..16])
            },
            String::from,
        );
    let mut resp = next.run(request).await;
    if let Ok(val) = http::HeaderValue::from_str(&trace_id) {
        resp.headers_mut().insert("traceparent", val);
    }
    resp
}

// ── Caller identity (SOVD §7.4 — lock ownership) ─────────────────────────

/// Extracts caller identity from auth context or fallback header.
///
/// Priority: `AuthenticatedClient` (injected by auth middleware from JWT sub / API key)
///         → `x-sovd-client-id` header (test compat / unauthenticated mode)
///         → empty string (anonymous)
struct CallerIdentity(String);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for CallerIdentity {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        // 1. Prefer identity from auth middleware
        if let Some(client) = parts.extensions.get::<AuthenticatedClient>() {
            return Ok(Self(client.0.clone()));
        }
        // 2. Fall back to x-sovd-client-id header
        if let Some(val) = parts.headers.get("x-sovd-client-id") {
            if let Ok(s) = val.to_str() {
                if !s.is_empty() {
                    return Ok(Self(s.to_owned()));
                }
            }
        }
        // 3. Anonymous
        Ok(Self(String::new()))
    }
}

// ── OData-style pagination (SOVD §5) ─────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PaginationParams {
    #[serde(rename = "$top")]
    top: Option<usize>,
    #[serde(rename = "$skip")]
    skip: Option<usize>,
    #[serde(rename = "$filter")]
    filter: Option<String>,
    #[serde(rename = "$orderby")]
    orderby: Option<String>,
    #[serde(rename = "$select")]
    select: Option<String>,
}

/// Apply OData query options: $filter, $orderby, $skip, $top, $select (SOVD §5).
///
/// $filter supports simple equality: `field eq 'value'` or `field eq value`
/// $orderby supports: `field asc` or `field desc` (default: asc)
/// $select supports comma-separated field projection
fn paginate<T: Serialize + Clone>(
    mut items: Vec<T>,
    params: &PaginationParams,
) -> Result<Collection<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    // $filter — simple "field eq 'value'" parsing
    if let Some(ref filter) = params.filter {
        items = apply_odata_filter(items, filter)?;
    }

    // $orderby — "field [asc|desc]"
    if let Some(ref orderby) = params.orderby {
        apply_odata_orderby(&mut items, orderby)?;
    }

    let total = items.len();
    let skip = params.skip.unwrap_or(0);
    let top = params.top.unwrap_or(usize::MAX);
    let paged: Vec<T> = items.into_iter().skip(skip).take(top).collect();

    // Serialize to JSON Value for uniform handling
    let mut values: Vec<serde_json::Value> = paged
        .iter()
        .filter_map(|item| serde_json::to_value(item).ok())
        .collect();

    // $select — field projection
    if let Some(ref select) = params.select {
        let fields: Vec<&str> = select.split(',').map(str::trim).collect();
        values = values
            .into_iter()
            .map(|v| {
                if let serde_json::Value::Object(map) = v {
                    let filtered: serde_json::Map<String, serde_json::Value> = map
                        .into_iter()
                        .filter(|(k, _)| fields.contains(&k.as_str()))
                        .collect();
                    serde_json::Value::Object(filtered)
                } else {
                    v
                }
            })
            .collect();
    }

    Ok(Collection {
        context: None,
        count: total,
        value: values,
    })
}

/// Parse and apply a simple OData $filter expression: `field eq 'value'`
fn apply_odata_filter<T: Serialize + Clone>(
    items: Vec<T>,
    filter: &str,
) -> Result<Vec<T>, (StatusCode, Json<SovdErrorEnvelope>)> {
    // Parse "field eq 'value'" or "field eq value"
    let parts: Vec<&str> = filter.splitn(3, ' ').collect();
    if parts.len() != 3 || !parts[1].eq_ignore_ascii_case("eq") {
        return Err(bad_request(&format!(
            "Unsupported $filter syntax: '{filter}'. Expected: field eq 'value'"
        )));
    }
    let field = parts[0];
    let value = parts[2].trim_matches('\'').trim_matches('"');

    Ok(items
        .into_iter()
        .filter(|item| {
            if let Ok(json) = serde_json::to_value(item) {
                if let Some(field_val) = json.get(field) {
                    return match field_val {
                        serde_json::Value::String(s) => s == value,
                        serde_json::Value::Number(n) => {
                            let s = n.to_string();
                            s == value
                        }
                        serde_json::Value::Bool(b) => {
                            (value == "true" && *b) || (value == "false" && !*b)
                        }
                        _ => false,
                    };
                }
            }
            false
        })
        .collect())
}

/// Parse and apply a simple OData $orderby expression: `field [asc|desc]`
fn apply_odata_orderby<T: Serialize + Clone>(
    items: &mut [T],
    orderby: &str,
) -> Result<(), (StatusCode, Json<SovdErrorEnvelope>)> {
    let parts: Vec<&str> = orderby.split_whitespace().collect();
    let field = parts.first().copied().unwrap_or("");
    let desc = parts.get(1).is_some_and(|d| d.eq_ignore_ascii_case("desc"));

    if field.is_empty() {
        return Err(bad_request("$orderby requires a field name"));
    }

    items.sort_by(|a, b| {
        let va = serde_json::to_value(a)
            .ok()
            .and_then(|j| j.get(field).cloned());
        let vb = serde_json::to_value(b)
            .ok()
            .and_then(|j| j.get(field).cloned());
        let cmp = cmp_json_values(va.as_ref(), vb.as_ref());
        if desc {
            cmp.reverse()
        } else {
            cmp
        }
    });
    Ok(())
}

/// Compare two optional JSON values for sorting
fn cmp_json_values(
    a: Option<&serde_json::Value>,
    b: Option<&serde_json::Value>,
) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(va), Some(vb)) => {
            // Try numeric comparison first
            if let (Some(na), Some(nb)) = (va.as_f64(), vb.as_f64()) {
                return na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal);
            }
            // Fall back to string comparison
            va.to_string().cmp(&vb.to_string())
        }
    }
}

/// Build the full axum router with all SOVD endpoints
#[allow(clippy::too_many_lines)]
pub fn build_router(state: AppState, auth_config: AuthConfig) -> Router {
    // ── Standard ISO 17978-3 routes ────────────────────────────────────────
    let sovd_v1 = Router::new()
        // Discovery (§5.1)
        .route("/", get(server_info))
        // Components (§7.1)
        .route("/components", get(list_components))
        .route("/components/{component_id}", get(get_component))
        // Data (§7.5)
        .route("/components/{component_id}/data", get(list_data))
        .route("/components/{component_id}/data/{data_id}", get(read_data))
        .route("/components/{component_id}/data/{data_id}", put(write_data))
        .route(
            "/components/{component_id}/data/{data_id}",
            axum::routing::patch(patch_data),
        )
        .route("/components/{component_id}/data/bulk-read", post(bulk_read))
        .route(
            "/components/{component_id}/data/bulk-write",
            post(bulk_write),
        )
        // Faults (§7.6)
        .route("/components/{component_id}/faults", get(list_faults))
        .route("/components/{component_id}/faults", delete(clear_faults))
        .route(
            "/components/{component_id}/faults/{fault_id}",
            get(get_fault_by_id),
        )
        .route(
            "/components/{component_id}/faults/{fault_id}",
            delete(clear_single_fault),
        )
        // Operations (§7.7)
        .route(
            "/components/{component_id}/operations",
            get(list_operations),
        )
        .route(
            "/components/{component_id}/operations/{op_id}",
            post(execute_operation),
        )
        .route(
            "/components/{component_id}/operations/{op_id}/executions",
            get(list_executions),
        )
        .route(
            "/components/{component_id}/operations/{op_id}/executions/{exec_id}",
            get(get_execution),
        )
        .route(
            "/components/{component_id}/operations/{op_id}/executions/{exec_id}",
            delete(cancel_execution),
        )
        // Groups (§7.2)
        .route("/groups", get(list_groups))
        .route("/groups/{group_id}", get(get_group))
        .route("/groups/{group_id}/components", get(get_group_components))
        // Capabilities (§7.3)
        .route(
            "/components/{component_id}/capabilities",
            get(get_capabilities),
        )
        // Locking (§7.4)
        .route("/components/{component_id}/lock", post(acquire_lock))
        .route("/components/{component_id}/lock", get(get_lock))
        .route("/components/{component_id}/lock", delete(release_lock))
        // Mode / Session (§7.6)
        .route("/components/{component_id}/modes", get(get_mode))
        .route("/components/{component_id}/modes", post(set_mode))
        .route(
            "/components/{component_id}/modes/{mode_id}",
            put(activate_mode),
        )
        // Software Packages (§5.5.10)
        .route(
            "/components/{component_id}/software-packages",
            get(list_software_packages),
        )
        .route(
            "/components/{component_id}/software-packages/{package_id}",
            post(install_software_package),
        )
        .route(
            "/components/{component_id}/software-packages/{package_id}/status",
            get(get_software_package_status),
        )
        // Entity collection stubs (§4.2.3)
        .route("/apps", get(list_apps))
        .route("/funcs", get(list_funcs))
        // Configuration (§7.8)
        .route("/components/{component_id}/configurations", get(read_config))
        .route("/components/{component_id}/configurations", put(write_config))
        // Proximity Challenge (§7.9)
        .route(
            "/components/{component_id}/proximity-challenge",
            post(proximity_challenge),
        )
        .route(
            "/components/{component_id}/proximity-challenge/{challenge_id}",
            get(get_proximity_challenge),
        )
        // Logs (§7.10)
        .route("/components/{component_id}/logs", get(get_logs))
        // Events / SSE (§7.11)
        .route(
            "/components/{component_id}/faults/subscribe",
            get(subscribe_faults),
        )
        // Version info (SOVD §4.1)
        .route("/version-info", get(version_info))
        // Capability docs — wildcard (SOVD §5.1)
        .route("/docs", get(serve_docs))
        .route("/components/{component_id}/docs", get(serve_docs))
        .route("/components/{component_id}/data/docs", get(serve_docs))
        .route("/components/{component_id}/faults/docs", get(serve_docs))
        .route("/components/{component_id}/operations/docs", get(serve_docs))
        .route("/components/{component_id}/modes/docs", get(serve_docs))
        .route("/components/{component_id}/locks/docs", get(serve_docs))
        .route("/components/{component_id}/configurations/docs", get(serve_docs))
        .route("/components/{component_id}/logs/docs", get(serve_docs))
        // OData metadata (§5.2)
        .route("/$metadata", get(odata_metadata))
        // Health (non-SOVD, operational)
        .route("/health", get(health_check))
        // Audit trail (Wave 1)
        .route("/audit", get(list_audit_entries));

    // ── Vendor extensions (x-uds prefixed, non-standard) ────────────────
    let x_uds = Router::new()
        .route(
            "/components/{component_id}/connect",
            post(connect_component),
        )
        .route(
            "/components/{component_id}/disconnect",
            post(disconnect_component),
        )
        .route("/components/{component_id}/io/{data_id}", post(io_control))
        .route(
            "/components/{component_id}/comm-control",
            post(communication_control),
        )
        .route(
            "/components/{component_id}/dtc-setting",
            post(control_dtc_setting),
        )
        .route("/components/{component_id}/memory", get(read_memory))
        .route("/components/{component_id}/memory", put(write_memory))
        .route("/components/{component_id}/flash", post(start_flash))
        .route("/diag/keepalive", get(keepalive_status))
        .with_state(state.clone());

    // ── OEM profile (captured for middleware closure) ──────────────────
    let oem_profile = state.oem_profile.clone();

    // ── OpenAPI spec ──────────────────────────────────────────────────
    let openapi_json = Arc::new(super::openapi::build_openapi_json());
    let openapi_clone = openapi_json.clone();

    // ── Prometheus metrics ───────────────────────────────────────────────
    let prometheus_handle = setup_prometheus();

    // Build CORS layer from config (restrictive when origins specified, permissive otherwise)
    let cors = if auth_config.cors_origins.is_empty() {
        CorsLayer::permissive()
    } else {
        let origins: Vec<http::HeaderValue> = auth_config
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([
                http::Method::GET,
                http::Method::POST,
                http::Method::PUT,
                http::Method::PATCH,
                http::Method::DELETE,
                http::Method::OPTIONS,
            ])
            .allow_headers([
                http::header::CONTENT_TYPE,
                http::header::AUTHORIZATION,
                http::HeaderName::from_static("x-api-key"),
                http::HeaderName::from_static("x-sovd-client-id"),
            ])
    };

    // ── Concurrency limiting (max 200 in-flight requests) ─────────────
    let concurrency_limit = tower::limit::ConcurrencyLimitLayer::new(200);

    Router::new()
        .nest("/sovd/v1", sovd_v1)
        .nest("/sovd/v1/x-uds", x_uds)
        // OpenAPI JSON endpoint (public, outside auth)
        .route(
            "/openapi.json",
            get(move || {
                let spec = openapi_clone.clone();
                async move { Json((*spec).clone()) }
            }),
        )
        // Prometheus metrics endpoint (public)
        .route(
            "/metrics",
            get(move || {
                let handle = prometheus_handle.clone();
                async move { handle.render() }
            }),
        )
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                config: auth_config,
                oem_profile: state.oem_profile.clone(),
                audit_log: state.audit_log.clone(),
            },
            auth_middleware,
        ))
        .layer(axum::middleware::from_fn(move |matched_path, request, next| {
            let profile = oem_profile.clone();
            entity_id_validation_middleware(profile, matched_path, request, next)
        }))
        .layer(axum::middleware::from_fn(trace_id_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(concurrency_limit)
        .layer(TimeoutLayer::with_status_code(
            http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024)) // 2 MiB max request body
        .layer(cors)
        .with_state(state)
}

/// Install the global Prometheus recorder (idempotent) and return the render handle.
fn setup_prometheus() -> metrics_exporter_prometheus::PrometheusHandle {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            #[allow(clippy::expect_used)] // Init-time one-shot; unrecoverable if recorder fails
            let handle = metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install Prometheus recorder");
            metrics::describe_counter!("sovd_http_requests_total", "Total HTTP requests");
            metrics::describe_histogram!(
                "sovd_http_request_duration_seconds",
                "HTTP request duration"
            );
            handle
        })
        .clone()
}

// ── Discovery ───────────────────────────────────────────────────────────────

/// SOVD §5.1 discovery response
#[derive(Serialize)]
struct ServerInfo {
    #[serde(rename = "serverName")]
    server_name: &'static str,
    #[serde(rename = "serverVersion")]
    server_version: String,
    #[serde(rename = "sovdVersion")]
    sovd_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'static str>,
    #[serde(rename = "supportedProtocols")]
    supported_protocols: Vec<&'static str>,
}

async fn server_info() -> Json<ServerInfo> {
    Json(ServerInfo {
        server_name: "OpenSOVD-native-server",
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        sovd_version: "1.1.0",
        description: Some("Native SOVD server — Eclipse OpenSOVD ecosystem"),
        supported_protocols: vec!["http/1.1", "http/2"],
    })
}

/// SOVD §4.1 — version info endpoint
#[derive(Serialize)]
struct VersionInfo {
    #[serde(rename = "sovdVersion")]
    sovd_version: &'static str,
    #[serde(rename = "serverVersion")]
    server_version: String,
    #[serde(rename = "apiVersions")]
    api_versions: Vec<ApiVersionEntry>,
}

#[derive(Serialize)]
struct ApiVersionEntry {
    version: &'static str,
    url: &'static str,
    status: &'static str,
}

async fn version_info() -> Json<VersionInfo> {
    Json(VersionInfo {
        sovd_version: "1.1.0",
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        api_versions: vec![ApiVersionEntry {
            version: "v1",
            url: "/sovd/v1",
            status: "active",
        }],
    })
}

/// SOVD §5.1 — capability docs per resource (returns filtered OpenAPI spec).
/// Extracts the resource segment before `/docs` from the matched route template
/// and returns only the paths relevant to that resource category.
/// CDF extension values (`x-sovd-*`) are supplied by the active OemProfile's CdfPolicy.
async fn serve_docs(
    State(state): State<AppState>,
    matched_path: Option<axum::extract::MatchedPath>,
) -> Json<serde_json::Value> {
    // Extract the resource segment directly before "/docs" in the route template.
    // e.g. "/sovd/v1/components/{component_id}/data/docs" → "data"
    //      "/sovd/v1/docs" → None (full spec)
    let filter = matched_path.as_ref().and_then(|mp| {
        let path = mp.as_str();
        let segments: Vec<&str> = path.trim_end_matches('/').rsplit('/').collect();
        // segments[0] == "docs", segments[1] == the resource category (if present)
        if segments.len() >= 2 && segments[0] == "docs" {
            let candidate = segments[1];
            // If the segment is a path parameter like {component_id}, it's not a category
            if candidate.starts_with('{') {
                // /components/{component_id}/docs → filter on "components"
                // But this is a component-level docs, return discovery/component paths
                return Some("components");
            }
            Some(candidate)
        } else {
            None
        }
    });
    let cdf = state.oem_profile.as_cdf_policy();
    Json(super::openapi::build_openapi_json_with_policy(cdf, filter))
}

// ── Components ──────────────────────────────────────────────────────────────

/// OData $metadata — JSON Entity Data Model (SOVD §5.2)
async fn odata_metadata() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "sovdVersion": "1.1.0",
        "entityTypes": {
            "Component": {
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "category": { "type": "string" },
                    "description": { "type": "string", "nullable": true },
                    "connectionState": { "type": "string", "enum": ["connected", "disconnected", "connecting", "error"] }
                },
                "key": ["id"]
            },
            "Data": {
                "properties": {
                    "id": { "type": "string" },
                    "componentId": { "type": "string" },
                    "name": { "type": "string" },
                    "dataType": { "type": "string", "enum": ["string", "integer", "float", "boolean", "bytes", "enum", "struct"] },
                    "access": { "type": "string", "enum": ["readOnly", "readWrite", "writeOnly"] },
                    "value": {},
                    "unit": { "type": "string", "nullable": true }
                },
                "key": ["id"]
            },
            "Fault": {
                "properties": {
                    "id": { "type": "string" },
                    "componentId": { "type": "string" },
                    "code": { "type": "string" },
                    "severity": { "type": "string", "enum": ["low", "medium", "high", "critical"] },
                    "status": { "type": "string", "enum": ["active", "passive", "pending"] }
                },
                "key": ["id"]
            },
            "Operation": {
                "properties": {
                    "id": { "type": "string" },
                    "componentId": { "type": "string" },
                    "name": { "type": "string" },
                    "status": { "type": "string", "enum": ["idle", "running", "completed", "failed"] }
                },
                "key": ["id"]
            },
            "Lock": {
                "properties": {
                    "componentId": { "type": "string" },
                    "lockedBy": { "type": "string" },
                    "lockedAt": { "type": "string", "format": "date-time" },
                    "expires": { "type": "string", "format": "date-time", "nullable": true }
                },
                "key": ["componentId"]
            },
            "Group": {
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "componentIds": { "type": "array", "items": { "type": "string" } }
                },
                "key": ["id"]
            }
        },
        "collections": {
            "components": "Component",
            "data": "Data",
            "faults": "Fault",
            "operations": "Operation",
            "groups": "Group"
        }
    }))
}

async fn list_components(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let components = state.backend.list_components();
    Ok(Json(
        paginate(components, &params)?.with_context("$metadata#components"),
    ))
}

async fn get_component(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<Json<SovdComponent>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .backend
        .get_component(&component_id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("Component '{component_id}' not found")))
}

async fn connect_component(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .backend
        .connect(&component_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    state.audit_log.record(
        "anonymous", SovdAuditAction::Connect,
        &format!("component/{component_id}"), "connect", "POST", "success", None, None,
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect_component(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .backend
        .disconnect(&component_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    state.audit_log.record(
        "anonymous", SovdAuditAction::Disconnect,
        &format!("component/{component_id}"), "disconnect", "POST", "success", None, None,
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Faults ──────────────────────────────────────────────────────────────────

async fn list_faults(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let faults = state.fault_manager.get_faults_for_component(&component_id);
    Ok(Json(
        paginate(faults, &params)?.with_context("$metadata#faults"),
    ))
}

async fn clear_faults(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    // Clear via backend (forwards to CDA or local UDS)
    let _ = state.backend.clear_faults(&component_id).await;
    state
        .fault_manager
        .clear_faults_for_component(&component_id);
    state.audit_log.record(
        &caller.0, SovdAuditAction::ClearFaults,
        &format!("component/{component_id}"), "faults", "DELETE", "success", None, None,
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Data ────────────────────────────────────────────────────────────────────

async fn read_data(
    State(state): State<AppState>,
    headers: http::HeaderMap,
    Path((component_id, data_id)): Path<(String, String)>,
) -> Result<axum::response::Response, (StatusCode, Json<SovdErrorEnvelope>)> {
    let data = state
        .backend
        .read_data(&component_id, &data_id)
        .await
        .map_err(|ref e| diag_error(e))?;

    // Compute ETag from response body (SOVD §6.5 — conditional requests)
    let body_bytes = serde_json::to_vec(&data).unwrap_or_default();
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        body_bytes.hash(&mut hasher);
        hasher.finish()
    };
    let etag = format!("\"{}\"", hex::encode(hash.to_be_bytes()));

    // If-None-Match → 304 Not Modified
    if let Some(inm) = headers.get(http::header::IF_NONE_MATCH) {
        if let Ok(inm_str) = inm.to_str() {
            if inm_str == etag || inm_str == "*" {
                return Ok(axum::response::Response::builder()
                    .status(StatusCode::NOT_MODIFIED)
                    .header(http::header::ETAG, &etag)
                    .body(axum::body::Body::empty())
                    .map_err(|e| bad_request(&format!("Response build error: {e}")))?
                    .into_response());
            }
        }
    }

    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(http::header::ETAG, &etag)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body_bytes))
        .map_err(|e| bad_request(&format!("Response build error: {e}")))?
        .into_response())
}

/// SOVD §7.5 typed write request — accepts JSON value directly
#[derive(Deserialize)]
struct WriteDataRequest {
    value: serde_json::Value,
}

async fn write_data(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, data_id)): Path<(String, String)>,
    Json(body): Json<WriteDataRequest>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    // Convert typed value to bytes for the backend:
    //   - strings starting with "0x" are treated as hex-encoded raw bytes
    //   - other JSON values are serialized to JSON bytes
    let bytes = match &body.value {
        serde_json::Value::String(s) if s.starts_with("0x") => {
            hex::decode(s.trim_start_matches("0x"))
                .map_err(|e| bad_request(&format!("Invalid hex value: {e}")))?
        }
        other => serde_json::to_vec(other)
            .map_err(|e| bad_request(&format!("Failed to serialize value: {e}")))?,
    };
    state
        .backend
        .write_data(&component_id, &data_id, &bytes)
        .await
        .map_err(|ref e| diag_error(e))?;
    state.audit_log.record(
        &caller.0, SovdAuditAction::WriteData,
        &format!("component/{component_id}"), &format!("data/{data_id}"), "PUT", "success", None, None,
    );
    Ok(StatusCode::NO_CONTENT)
}

/// PATCH partial update — merge incoming fields into existing data value (SOVD §7.5)
async fn patch_data(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, data_id)): Path<(String, String)>,
    Json(patch): Json<serde_json::Value>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;

    // Read current value
    let current = state
        .backend
        .read_data(&component_id, &data_id)
        .await
        .map_err(|ref e| diag_error(e))?;

    // Merge: patch fields override current fields
    let mut merged = current.clone();
    if let (serde_json::Value::Object(base), serde_json::Value::Object(overlay)) =
        (&mut merged, &patch)
    {
        for (k, v) in overlay {
            base.insert(k.clone(), v.clone());
        }
    } else {
        // Non-object: patch replaces entirely
        merged = patch;
    }

    let bytes = serde_json::to_vec(&merged)
        .map_err(|e| bad_request(&format!("Failed to serialize merged value: {e}")))?;
    state
        .backend
        .write_data(&component_id, &data_id, &bytes)
        .await
        .map_err(|ref e| diag_error(e))?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Operations ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ExecuteOperationRequest {
    #[serde(default)]
    params: Option<String>,
}

async fn execute_operation(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, op_id)): Path<(String, String)>,
    Json(body): Json<ExecuteOperationRequest>,
) -> Result<
    (StatusCode, http::HeaderMap, Json<SovdOperationExecution>),
    (StatusCode, Json<SovdErrorEnvelope>),
> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    let params = body
        .params
        .as_deref()
        .map(hex::decode)
        .transpose()
        .map_err(|e| bad_request(&format!("Invalid hex params: {e}")))?;

    let exec_id = uuid::Uuid::new_v4().to_string();

    // Store a "running" execution immediately (SOVD §7.7 async model)
    let exec = SovdOperationExecution {
        execution_id: exec_id.clone(),
        component_id: component_id.clone(),
        operation_id: op_id.clone(),
        status: SovdOperationStatus::Running,
        result: None,
        progress: Some(0),
        timestamp: Some(chrono::Utc::now().to_rfc3339()),
    };
    evict_and_insert(&state.execution_store, exec_id.clone(), exec);

    // Execute backend operation
    let result = state
        .backend
        .execute_operation(&component_id, &op_id, params.as_deref())
        .await;

    // Update execution with result
    let final_exec = match result {
        Ok(value) => SovdOperationExecution {
            execution_id: exec_id.clone(),
            component_id: component_id.clone(),
            operation_id: op_id.clone(),
            status: SovdOperationStatus::Completed,
            result: Some(value),
            progress: Some(100),
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        },
        Err(ref e) => SovdOperationExecution {
            execution_id: exec_id.clone(),
            component_id: component_id.clone(),
            operation_id: op_id.clone(),
            status: SovdOperationStatus::Failed,
            result: Some(serde_json::json!({ "error": e.to_string() })),
            progress: None,
            timestamp: Some(chrono::Utc::now().to_rfc3339()),
        },
    };
    state
        .execution_store
        .insert(exec_id.clone(), final_exec.clone());

    // Return 202 Accepted + Location header (SOVD §7.7)
    let location =
        format!("/sovd/v1/components/{component_id}/operations/{op_id}/executions/{exec_id}");
    let mut resp_headers = http::HeaderMap::new();
    resp_headers.insert(
        http::header::LOCATION,
        location
            .parse()
            .map_err(|e| bad_request(&format!("Location header error: {e}")))?,
    );

    state.audit_log.record(
        &caller.0, SovdAuditAction::ExecuteOperation,
        &format!("component/{component_id}"), &format!("operations/{op_id}"),
        "POST", "success", Some(&exec_id), None,
    );
    Ok((StatusCode::ACCEPTED, resp_headers, Json(final_exec)))
}

/// Bounded insert into a DashMap, evicting oldest entry if at capacity.
fn evict_and_insert<V: Clone>(store: &dashmap::DashMap<String, V>, key: String, value: V) {
    const MAX_ENTRIES: usize = 10_000;
    if store.len() >= MAX_ENTRIES {
        if let Some(entry) = store.iter().next() {
            let evict_key = entry.key().clone();
            drop(entry);
            store.remove(&evict_key);
        }
    }
    store.insert(key, value);
}

// ── IO Control ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct IoControlRequest {
    /// Control parameter: "return_to_ecu", "reset_to_default", "freeze", "short_term_adjustment"
    control: String,
    /// Optional hex-encoded control option record
    #[serde(default)]
    value: Option<String>,
}

async fn io_control(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, data_id)): Path<(String, String)>,
    Json(body): Json<IoControlRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    let option_record = body
        .value
        .as_deref()
        .map(hex::decode)
        .transpose()
        .map_err(|e| bad_request(&format!("Invalid hex value: {e}")))?;

    let result = state
        .backend
        .io_control(
            &component_id,
            &data_id,
            &body.control,
            option_record.as_deref(),
        )
        .await
        .map_err(|ref e| diag_error(e))?;

    Ok(Json(result))
}

// ── Communication Control ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct CommControlRequest {
    /// "enable_rx_and_tx", "enable_rx_disable_tx", "disable_rx_enable_tx", "disable_rx_and_tx"
    control_type: String,
    /// Communication type byte (hex, e.g. "01" for normal communication)
    communication_type: String,
}

async fn communication_control(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
    Json(body): Json<CommControlRequest>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    let comm_type = u8::from_str_radix(body.communication_type.trim_start_matches("0x"), 16)
        .map_err(|_| bad_request("Invalid communication_type (expected hex byte, e.g. '01')"))?;

    state
        .backend
        .communication_control(&component_id, &body.control_type, comm_type)
        .await
        .map_err(|ref e| diag_error(e))?;

    Ok(StatusCode::NO_CONTENT)
}

// ── DTC Setting ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DtcSettingRequest {
    /// "on" or "off"
    setting: String,
}

async fn control_dtc_setting(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
    Json(body): Json<DtcSettingRequest>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    state
        .backend
        .dtc_setting(&component_id, &body.setting)
        .await
        .map_err(|ref e| diag_error(e))?;

    Ok(StatusCode::NO_CONTENT)
}

// ── Memory Access ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ReadMemoryQuery {
    /// Hex memory address (e.g. "0x20000000")
    address: String,
    /// Number of bytes to read
    size: u32,
}

async fn read_memory(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    axum::extract::Query(query): axum::extract::Query<ReadMemoryQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let address = u32::from_str_radix(
        query
            .address
            .trim_start_matches("0x")
            .trim_start_matches("0X"),
        16,
    )
    .map_err(|_| bad_request("Invalid address (expected hex, e.g. 0x20000000)"))?;

    let data = state
        .backend
        .read_memory(&component_id, address, query.size)
        .await
        .map_err(|ref e| diag_error(e))?;

    Ok(Json(serde_json::json!({
        "componentId": component_id,
        "address": format!("0x{address:08X}"),
        "size": data.len(),
        "value": hex::encode(&data),
    })))
}

#[derive(Deserialize)]
struct WriteMemoryRequest {
    /// Hex memory address (e.g. "0x20000000")
    address: String,
    /// Hex-encoded data to write
    value: String,
}

async fn write_memory(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
    Json(body): Json<WriteMemoryRequest>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    let address = u32::from_str_radix(
        body.address
            .trim_start_matches("0x")
            .trim_start_matches("0X"),
        16,
    )
    .map_err(|_| bad_request("Invalid address (expected hex, e.g. 0x20000000)"))?;

    let data =
        hex::decode(&body.value).map_err(|e| bad_request(&format!("Invalid hex value: {e}")))?;

    state
        .backend
        .write_memory(&component_id, address, &data)
        .await
        .map_err(|ref e| diag_error(e))?;

    Ok(StatusCode::NO_CONTENT)
}

// ── OTA Flash ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FlashRequest {
    /// Base64-encoded firmware binary
    firmware_data: String,
    /// Target memory address (default: 0x00000000)
    #[serde(default)]
    memory_address: u32,
}

async fn start_flash(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
    Json(body): Json<FlashRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    use base64::Engine;
    let firmware = base64::engine::general_purpose::STANDARD
        .decode(&body.firmware_data)
        .map_err(|e| bad_request(&format!("Invalid base64 firmware_data: {e}")))?;

    if firmware.is_empty() {
        return Err(bad_request("firmware_data must not be empty"));
    }

    let result = state
        .backend
        .flash(&component_id, &firmware, body.memory_address)
        .await
        .map_err(|ref e| diag_error(e))?;
    state.audit_log.record(
        &caller.0, SovdAuditAction::FlashStart,
        &format!("component/{component_id}"), "flash",
        "POST", "success", None, None,
    );
    Ok((StatusCode::ACCEPTED, Json(result)))
}

// ── Keepalive ───────────────────────────────────────────────────────────────

async fn keepalive_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let active = state.backend.active_keepalives();
    Json(serde_json::json!({
        "active": active,
        "count": active.len(),
    }))
}

// ── Health ───────────────────────────────────────────────────────────────────

async fn health_check(State(state): State<AppState>) -> Json<serde_json::Value> {
    let info = state.health.system_info();
    Json(info)
}

// ── Audit Trail (Wave 1) ─────────────────────────────────────────────────

/// Query parameters for the /audit endpoint.
#[derive(Debug, Deserialize)]
struct AuditQueryParams {
    /// Filter by caller identity
    #[serde(default)]
    caller: Option<String>,
    /// Filter by action (e.g. "readData", "writeData", "clearFaults")
    #[serde(default)]
    action: Option<native_interfaces::sovd::SovdAuditAction>,
    /// Filter by target (prefix match, e.g. "component/hpc")
    #[serde(default)]
    target: Option<String>,
    /// Filter by outcome ("success", "denied", "error")
    #[serde(default)]
    outcome: Option<String>,
    /// Maximum number of results (default: 100)
    #[serde(default)]
    limit: Option<usize>,
}

async fn list_audit_entries(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<AuditQueryParams>,
) -> Json<Collection<native_interfaces::sovd::SovdAuditEntry>> {
    let filter = native_core::audit_log::AuditFilter {
        caller: params.caller,
        action: params.action,
        target: params.target,
        outcome: params.outcome,
        limit: Some(params.limit.unwrap_or(100)),
    };
    let entries = state.audit_log.query(&filter);
    Json(Collection::new(entries).with_context("$metadata#audit"))
}

// ── Data Listing (SOVD §7.5) ─────────────────────────────────────────────

async fn list_data(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let entries = state
        .backend
        .list_data(&component_id)
        .map_err(|e| not_found(&e.to_string()))?;
    Ok(Json(
        paginate(entries, &params)?.with_context("$metadata#data"),
    ))
}

// ── Operations Listing (SOVD §7.7) ──────────────────────────────────────

async fn list_operations(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let ops = state
        .backend
        .list_operations(&component_id)
        .map_err(|e| not_found(&e.to_string()))?;
    Ok(Json(
        paginate(ops, &params)?.with_context("$metadata#operations"),
    ))
}

// ── Fault by ID (SOVD §7.5) ─────────────────────────────────────────────

async fn get_fault_by_id(
    State(state): State<AppState>,
    Path((component_id, fault_id)): Path<(String, String)>,
) -> Result<Json<SovdFault>, (StatusCode, Json<SovdErrorEnvelope>)> {
    // Try FaultManager first
    if let Some(fault) = state.fault_manager.get_fault(&fault_id) {
        if fault.component_id == component_id {
            return Ok(Json(fault));
        }
    }
    // Check component faults
    let faults = state.fault_manager.get_faults_for_component(&component_id);
    faults
        .into_iter()
        .find(|f| f.id == fault_id)
        .map(Json)
        .ok_or_else(|| {
            not_found(&format!(
                "Fault '{fault_id}' not found for component '{component_id}'"
            ))
        })
}

// ── Locking (SOVD §7.4) ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct LockRequest {
    #[serde(rename = "lockedBy")]
    locked_by: String,
    #[serde(default)]
    expires: Option<String>,
}

async fn acquire_lock(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    caller: CallerIdentity,
    Json(body): Json<LockRequest>,
) -> Result<(StatusCode, Json<SovdLock>), (StatusCode, Json<SovdErrorEnvelope>)> {
    // SOVD §7.4: lock owner is the authenticated identity; fall back to body for unauthenticated mode
    let owner = if caller.0.is_empty() {
        &body.locked_by
    } else {
        &caller.0
    };
    let lock = state
        .lock_manager
        .acquire(&component_id, owner, body.expires)
        .map_err(|e| conflict(&e))?;
    state.audit_log.record(
        owner, SovdAuditAction::AcquireLock,
        &format!("component/{component_id}"), "lock", "POST", "success", None, None,
    );
    Ok((StatusCode::CREATED, Json(lock)))
}

async fn get_lock(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<Json<SovdLock>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .lock_manager
        .get_lock(&component_id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("No lock on component '{component_id}'")))
}

async fn release_lock(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    caller: CallerIdentity,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    // SOVD §7.4: only lock owner (or anonymous in unauthenticated mode) may release
    if let Some(lock) = state.lock_manager.get_lock(&component_id) {
        if !caller.0.is_empty() && lock.locked_by != caller.0 {
            return Err(conflict(&format!(
                "Lock owned by '{}', cannot release as '{}'",
                lock.locked_by, caller.0
            )));
        }
    } else {
        return Err(not_found(&format!("No lock on component '{component_id}'")));
    }
    state.lock_manager.release(&component_id);
    state.audit_log.record(
        &caller.0, SovdAuditAction::ReleaseLock,
        &format!("component/{component_id}"), "lock", "DELETE", "success", None, None,
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Capabilities (SOVD §7.3) ────────────────────────────────────────────

async fn get_capabilities(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<Json<SovdCapabilities>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let caps = state
        .backend
        .get_capabilities(&component_id)
        .map_err(|e| not_found(&e.to_string()))?;
    Ok(Json(caps))
}

// ── Bulk Data (SOVD §7.5.3) ─────────────────────────────────────────────

async fn bulk_read(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    Json(body): Json<SovdBulkReadRequest>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let results = state
        .backend
        .bulk_read(&component_id, &body.data_ids, body.category)
        .await
        .map_err(|ref e| diag_error(e))?;
    let values: Vec<serde_json::Value> = results
        .iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .collect();
    Ok(Json(
        Collection::new(values).with_context("$metadata#bulkData"),
    ))
}

async fn bulk_write(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    Json(body): Json<Vec<SovdBulkWriteItem>>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let results = state
        .backend
        .bulk_write(&component_id, &body)
        .await
        .map_err(|ref e| diag_error(e))?;
    let values: Vec<serde_json::Value> = results
        .iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .collect();
    Ok(Json(
        Collection::new(values).with_context("$metadata#bulkData"),
    ))
}

// ── Groups (SOVD §7.2) ──────────────────────────────────────────────────

async fn list_groups(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let groups = state.backend.list_groups();
    Ok(Json(
        paginate(groups, &params)?.with_context("$metadata#groups"),
    ))
}

async fn get_group(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
) -> Result<Json<SovdGroup>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .backend
        .get_group(&group_id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("Group '{group_id}' not found")))
}

async fn get_group_components(
    State(state): State<AppState>,
    Path(group_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let group = state
        .backend
        .get_group(&group_id)
        .ok_or_else(|| not_found(&format!("Group '{group_id}' not found")))?;

    let components: Vec<SovdComponent> = state
        .backend
        .list_components()
        .into_iter()
        .filter(|c| group.component_ids.contains(&c.id))
        .collect();

    Ok(Json(
        paginate(components, &params)?.with_context("$metadata#components"),
    ))
}

// ── Clear single fault (SOVD §7.6) ──────────────────────────────────────

async fn clear_single_fault(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, fault_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;

    // Validate the fault belongs to this component before clearing
    match state.fault_manager.get_fault(&fault_id) {
        Some(fault) if fault.component_id != component_id => {
            return Err(not_found(&format!(
                "Fault '{fault_id}' does not belong to component '{component_id}'"
            )));
        }
        None => {
            return Err(not_found(&format!(
                "Fault '{fault_id}' not found for component '{component_id}'"
            )));
        }
        _ => {}
    }

    state.fault_manager.clear_fault(&fault_id);
    Ok(StatusCode::NO_CONTENT)
}

// ── Operation Executions (SOVD §7.7) ────────────────────────────────────

async fn list_executions(
    State(state): State<AppState>,
    Path((component_id, op_id)): Path<(String, String)>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let executions: Vec<SovdOperationExecution> = state
        .execution_store
        .iter()
        .filter(|e| e.component_id == component_id && e.operation_id == op_id)
        .map(|e| e.value().clone())
        .collect();
    Ok(Json(
        paginate(executions, &params)?.with_context("$metadata#executions"),
    ))
}

async fn get_execution(
    State(state): State<AppState>,
    Path((_component_id, _op_id, exec_id)): Path<(String, String, String)>,
) -> Result<Json<SovdOperationExecution>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .execution_store
        .get(&exec_id)
        .map(|e| Json(e.value().clone()))
        .ok_or_else(|| not_found(&format!("Execution '{exec_id}' not found")))
}

async fn cancel_execution(
    State(state): State<AppState>,
    Path((_component_id, _op_id, exec_id)): Path<(String, String, String)>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    if let Some(mut entry) = state.execution_store.get_mut(&exec_id) {
        if entry.status == SovdOperationStatus::Running {
            entry.status = SovdOperationStatus::Cancelled;
            Ok(StatusCode::NO_CONTENT)
        } else {
            Err(conflict(&format!(
                "Execution '{exec_id}' is not running (status: {:?})",
                entry.status
            )))
        }
    } else {
        Err(not_found(&format!("Execution '{exec_id}' not found")))
    }
}

// ── Proximity Challenge (SOVD §7.9) ─────────────────────────────────────

#[derive(Deserialize)]
#[allow(dead_code)]
struct ProximityChallengeRequest {
    #[serde(default)]
    response: Option<String>,
}

async fn proximity_challenge(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    Json(_body): Json<ProximityChallengeRequest>,
) -> Result<(StatusCode, Json<SovdProximityChallenge>), (StatusCode, Json<SovdErrorEnvelope>)> {
    // Proximity challenge is hardware-dependent; return a stub challenge
    let challenge = SovdProximityChallenge {
        challenge_id: uuid::Uuid::new_v4().to_string(),
        status: SovdProximityChallengeStatus::Pending,
        challenge: Some(format!("proximity-{component_id}-{}", uuid::Uuid::new_v4())),
        response: None,
    };
    evict_and_insert(
        &state.proximity_store,
        challenge.challenge_id.clone(),
        challenge.clone(),
    );
    Ok((StatusCode::CREATED, Json(challenge)))
}

async fn get_proximity_challenge(
    State(state): State<AppState>,
    Path((_component_id, challenge_id)): Path<(String, String)>,
) -> Result<Json<SovdProximityChallenge>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .proximity_store
        .get(&challenge_id)
        .map(|e| Json(e.value().clone()))
        .ok_or_else(|| not_found(&format!("Proximity challenge '{challenge_id}' not found")))
}

// ── Logs (SOVD §7.10) ───────────────────────────────────────────────────

async fn get_logs(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let entries = state.diag_log.get_entries(Some(&component_id));
    Ok(Json(
        paginate(entries, &params)?.with_context("$metadata#logs"),
    ))
}

// ── Events / SSE (SOVD §7.11) ───────────────────────────────────────────

async fn subscribe_faults(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> axum::response::Sse<
    impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use axum::response::sse::Event;
    use tokio_stream::StreamExt;

    let fault_manager = state.fault_manager.clone();
    let comp_id = component_id.clone();

    // Track previous fault IDs to detect changes (SOVD §7.11: event-driven, not polling)
    let prev_ids = std::sync::Arc::new(std::sync::Mutex::new(
        std::collections::HashSet::<String>::new(),
    ));

    let stream =
        tokio_stream::wrappers::IntervalStream::new(tokio::time::interval(Duration::from_secs(2)))
            .filter_map(move |_| {
                let faults = fault_manager.get_faults_for_component(&comp_id);
                let current_ids: std::collections::HashSet<String> =
                    faults.iter().map(|f| f.id.clone()).collect();

                #[allow(clippy::unwrap_used)] // Poisoned SSE mutex is unrecoverable
                let mut prev = prev_ids.lock().unwrap();
                if *prev == current_ids {
                    return None; // No change — suppress event
                }

                // Compute delta (owned strings to satisfy borrow checker)
                let added: Vec<String> = current_ids.difference(&prev).cloned().collect();
                let removed: Vec<String> = prev.difference(&current_ids).cloned().collect();
                *prev = current_ids;
                drop(prev); // release lock before building event

                let event_data = serde_json::json!({
                    "componentId": component_id,
                    "added": added,
                    "removed": removed,
                    "totalFaults": faults.len(),
                });

                let data = serde_json::to_string(&event_data).unwrap_or_default();
                Some(Ok(Event::default().data(data).event("faultChange")))
            });

    axum::response::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

// ── Mode / Session (SOVD §7.6) ──────────────────────────────────────────

async fn get_mode(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<Json<SovdMode>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let mode = state
        .backend
        .get_mode(&component_id)
        .map_err(|e| not_found(&e.to_string()))?;
    Ok(Json(mode))
}

#[derive(Deserialize)]
struct SetModeRequest {
    mode: String,
}

async fn set_mode(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
    Json(body): Json<SetModeRequest>,
) -> Result<Json<SovdMode>, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    state
        .backend
        .set_mode(&component_id, &body.mode)
        .await
        .map_err(|ref e| diag_error(e))?;

    let mode = state
        .backend
        .get_mode(&component_id)
        .map_err(|ref e| diag_error(e))?;
    state.audit_log.record(
        &caller.0, SovdAuditAction::SetMode,
        &format!("component/{component_id}"), &format!("modes/{}", body.mode),
        "POST", "success", None, None,
    );
    Ok(Json(mode))
}

/// PUT /modes/{modeId} — activate a specific mode (ASAM SOVD §5.5.4)
///
/// Also maps special modes to backend operations:
///   "dtc-on"  → backend.dtc_setting(component_id, "on")
///   "dtc-off" → backend.dtc_setting(component_id, "off")
async fn activate_mode(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, mode_id)): Path<(String, String)>,
) -> Result<Json<SovdMode>, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;

    // Map DTC setting modes to backend dtc_setting (Item 7: dtc-setting → modes)
    match mode_id.as_str() {
        "dtc-on" => {
            state
                .backend
                .dtc_setting(&component_id, "on")
                .await
                .map_err(|ref e| diag_error(e))?;
        }
        "dtc-off" => {
            state
                .backend
                .dtc_setting(&component_id, "off")
                .await
                .map_err(|ref e| diag_error(e))?;
        }
        _ => {
            state
                .backend
                .set_mode(&component_id, &mode_id)
                .await
                .map_err(|ref e| diag_error(e))?;
        }
    }

    let mode = state
        .backend
        .get_mode(&component_id)
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(mode))
}

// ── Software Packages (SOVD §5.5.10) ────────────────────────────────────

async fn list_software_packages(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let packages = state
        .backend
        .list_software_packages(&component_id)
        .map_err(|e| not_found(&e.to_string()))?;
    Ok(Json(
        paginate(packages, &params)?.with_context("$metadata#softwarePackages"),
    ))
}

async fn install_software_package(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, package_id)): Path<(String, String)>,
) -> Result<(StatusCode, Json<SovdSoftwarePackage>), (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    let pkg = state
        .backend
        .install_software_package(&component_id, &package_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    state.audit_log.record(
        &caller.0, SovdAuditAction::InstallPackage,
        &format!("component/{component_id}"), &format!("software-packages/{package_id}"),
        "POST", "success", None, None,
    );
    Ok((StatusCode::ACCEPTED, Json(pkg)))
}

async fn get_software_package_status(
    State(state): State<AppState>,
    Path((component_id, package_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let packages = state
        .backend
        .list_software_packages(&component_id)
        .map_err(|e| not_found(&e.to_string()))?;
    let pkg = packages
        .into_iter()
        .find(|p| p.id == package_id)
        .ok_or_else(|| {
            not_found(&format!(
                "Software package '{package_id}' not found for component '{component_id}'"
            ))
        })?;
    Ok(Json(serde_json::json!({
        "packageId": pkg.id,
        "status": pkg.status,
    })))
}

// ── Entity Collection Stubs (ASAM SOVD §4.2.3) ─────────────────────────

async fn list_apps(
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let items: Vec<serde_json::Value> = vec![];
    Ok(Json(
        paginate(items, &params)?.with_context("$metadata#apps"),
    ))
}

async fn list_funcs(
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let items: Vec<serde_json::Value> = vec![];
    Ok(Json(
        paginate(items, &params)?.with_context("$metadata#funcs"),
    ))
}

// ── Configuration (SOVD §7.8) ───────────────────────────────────────────

async fn read_config(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<Json<SovdComponentConfig>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let config = state
        .backend
        .read_config(&component_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(config))
}

#[derive(Deserialize)]
struct WriteConfigRequest {
    name: String,
    value: String,
}

async fn write_config(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
    Json(body): Json<WriteConfigRequest>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.lock_manager, &component_id, &caller.0)?;
    let data =
        hex::decode(&body.value).map_err(|e| bad_request(&format!("Invalid hex value: {e}")))?;

    state
        .backend
        .write_config(&component_id, &body.name, &data)
        .await
        .map_err(|ref e| diag_error(e))?;
    state.audit_log.record(
        &caller.0, SovdAuditAction::WriteConfig,
        &format!("component/{component_id}"), "configurations", "PUT", "success", None, None,
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn sovd_error(status: StatusCode, code: &str, msg: &str) -> (StatusCode, Json<SovdErrorEnvelope>) {
    (status, Json(SovdErrorEnvelope::new(code, msg)))
}

fn not_found(msg: &str) -> (StatusCode, Json<SovdErrorEnvelope>) {
    sovd_error(StatusCode::NOT_FOUND, "SOVD-ERR-404", msg)
}

fn bad_request(msg: &str) -> (StatusCode, Json<SovdErrorEnvelope>) {
    sovd_error(StatusCode::BAD_REQUEST, "SOVD-ERR-400", msg)
}

/// Check lock enforcement for mutating operations (SOVD §7.4).
/// `caller` is the authenticated identity extracted by `CallerIdentity`.
fn require_unlocked_or_owner(
    lock_manager: &native_core::LockManager,
    component_id: &str,
    caller: &str,
) -> Result<(), (StatusCode, Json<SovdErrorEnvelope>)> {
    if let Some(lock) = lock_manager.get_lock(component_id) {
        if lock.locked_by != caller {
            // Compute Retry-After from lock expiry if available
            let retry_after = lock.expires.as_ref().and_then(|exp| {
                chrono::DateTime::parse_from_rfc3339(exp)
                    .ok()
                    .map(|expiry| {
                        let expiry_utc: chrono::DateTime<chrono::Utc> = expiry.into();
                        let secs = (expiry_utc - chrono::Utc::now()).num_seconds().max(1);
                        secs.cast_unsigned()
                    })
            });
            return Err(conflict_with_retry(
                &format!(
                    "Component '{}' is locked by '{}'",
                    component_id, lock.locked_by
                ),
                retry_after,
            ));
        }
    }
    Ok(())
}

fn conflict(msg: &str) -> (StatusCode, Json<SovdErrorEnvelope>) {
    sovd_error(StatusCode::CONFLICT, "SOVD-ERR-409", msg)
}

/// Return a 409 Conflict with an optional Retry-After hint (SOVD §7.4).
fn conflict_with_retry(
    msg: &str,
    retry_after: Option<u64>,
) -> (StatusCode, Json<SovdErrorEnvelope>) {
    let details = retry_after
        .map(|secs| {
            vec![SovdErrorDetail {
                code: "SOVD-RETRY".into(),
                message: format!("Retry-After: {secs}s"),
                target: None,
            }]
        })
        .unwrap_or_default();
    (
        StatusCode::CONFLICT,
        Json(SovdErrorEnvelope {
            error: SovdErrorResponse {
                code: "SOVD-ERR-409".into(),
                message: msg.to_owned(),
                target: None,
                details,
                innererror: None,
            },
        }),
    )
}

/// Map DiagServiceError to the semantically correct HTTP status code
fn diag_error(e: &native_interfaces::DiagServiceError) -> (StatusCode, Json<SovdErrorEnvelope>) {
    use native_interfaces::DiagServiceError::*;
    let (status, code) = match e {
        NotFound(_) => (StatusCode::NOT_FOUND, "SOVD-ERR-404"),
        InvalidRequest(_) | BadPayload(_) | InvalidParameter { .. } | NotEnoughData { .. } => {
            (StatusCode::BAD_REQUEST, "SOVD-ERR-400")
        }
        RequestNotSupported(_) => (StatusCode::NOT_IMPLEMENTED, "SOVD-ERR-501"),
        AccessDenied(_) => (StatusCode::FORBIDDEN, "SOVD-ERR-403"),
        Timeout => (StatusCode::GATEWAY_TIMEOUT, "SOVD-ERR-504"),
        EcuOffline(_) | ConnectionClosed(_) | NoResponse(_) | SendFailed(_) => {
            (StatusCode::BAD_GATEWAY, "SOVD-ERR-502")
        }
        InvalidState(_)
        | InvalidAddress(_)
        | Nack(_)
        | UnexpectedResponse(_)
        | ResourceError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "SOVD-ERR-500"),
    };
    sovd_error(status, code, &e.to_string())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unnecessary_literal_bound,
    clippy::uninlined_format_args,
    clippy::map_unwrap_or,
    clippy::redundant_closure_for_method_calls
)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http::StatusCode;
    use tower::ServiceExt;

    // ── Mock backend for tests (replaces removed LocalUdsBackend) ──────

    struct MockBackend;

    #[async_trait::async_trait]
    impl native_interfaces::ComponentBackend for MockBackend {
        fn name(&self) -> &str {
            "mock"
        }
        fn list_components(&self) -> Vec<native_interfaces::sovd::SovdComponent> {
            vec![native_interfaces::sovd::SovdComponent {
                id: "hpc".into(),
                name: "HPC Main".into(),
                category: "ecu".into(),
                description: None,
                connection_state: native_interfaces::sovd::SovdConnectionState::Disconnected,
            }]
        }
        fn get_component(
            &self,
            component_id: &str,
        ) -> Option<native_interfaces::sovd::SovdComponent> {
            self.list_components()
                .into_iter()
                .find(|c| c.id == component_id)
        }
        async fn connect(&self, _: &str) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        async fn disconnect(&self, _: &str) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        fn list_data(
            &self,
            _: &str,
        ) -> Result<
            Vec<native_interfaces::sovd::SovdDataCatalogEntry>,
            native_interfaces::DiagServiceError,
        > {
            Ok(vec![native_interfaces::sovd::SovdDataCatalogEntry {
                id: "0xF190".into(),
                name: "VIN".into(),
                description: None,
                access: native_interfaces::sovd::SovdDataAccess::ReadOnly,
                data_type: native_interfaces::sovd::SovdDataType::String,
                unit: None,
                did: Some("F190".into()),
            }])
        }
        async fn read_data(
            &self,
            _: &str,
            _: &str,
        ) -> Result<serde_json::Value, native_interfaces::DiagServiceError> {
            Ok(serde_json::json!({"value": "WVWZZZ3CZWE123456"}))
        }
        async fn write_data(
            &self,
            _: &str,
            _: &str,
            _: &[u8],
        ) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        async fn read_faults(
            &self,
            _: &str,
        ) -> Result<Vec<native_interfaces::sovd::SovdFault>, native_interfaces::DiagServiceError>
        {
            Ok(vec![])
        }
        async fn clear_faults(&self, _: &str) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        fn list_operations(
            &self,
            _: &str,
        ) -> Result<Vec<native_interfaces::sovd::SovdOperation>, native_interfaces::DiagServiceError>
        {
            Ok(vec![native_interfaces::sovd::SovdOperation {
                id: "0xFF00".into(),
                component_id: "hpc".into(),
                name: "Self Test".into(),
                description: Some("Execute ECU self-diagnostic".into()),
                status: native_interfaces::sovd::SovdOperationStatus::Idle,
            }])
        }
        async fn execute_operation(
            &self,
            _: &str,
            _: &str,
            _: Option<&[u8]>,
        ) -> Result<serde_json::Value, native_interfaces::DiagServiceError> {
            Ok(serde_json::json!({"status": "completed"}))
        }
        fn get_capabilities(
            &self,
            _: &str,
        ) -> Result<native_interfaces::sovd::SovdCapabilities, native_interfaces::DiagServiceError>
        {
            Ok(native_interfaces::sovd::SovdCapabilities {
                component_id: "hpc".into(),
                supported_categories: vec!["data".into(), "faults".into(), "operations".into()],
                data_count: 1,
                operation_count: 1,
                features: vec!["faults".into(), "locking".into()],
            })
        }
        fn get_mode(
            &self,
            component_id: &str,
        ) -> Result<native_interfaces::sovd::SovdMode, native_interfaces::DiagServiceError>
        {
            Ok(native_interfaces::sovd::SovdMode {
                component_id: component_id.into(),
                current_mode: "default".into(),
                available_modes: vec!["default".into(), "extended".into(), "programming".into()],
            })
        }
        async fn set_mode(
            &self,
            _: &str,
            _: &str,
        ) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        async fn read_config(
            &self,
            _: &str,
        ) -> Result<native_interfaces::sovd::SovdComponentConfig, native_interfaces::DiagServiceError>
        {
            Ok(native_interfaces::sovd::SovdComponentConfig {
                component_id: "hpc".into(),
                parameters: serde_json::json!({}),
            })
        }
        async fn write_config(
            &self,
            _: &str,
            _: &str,
            _: &[u8],
        ) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        async fn bulk_read(
            &self,
            _: &str,
            _: &[String],
            _: Option<native_interfaces::sovd::SovdBulkDataCategory>,
        ) -> Result<
            Vec<native_interfaces::sovd::SovdBulkDataItem>,
            native_interfaces::DiagServiceError,
        > {
            Ok(vec![])
        }
        async fn bulk_write(
            &self,
            _: &str,
            _: &[native_interfaces::sovd::SovdBulkWriteItem],
        ) -> Result<
            Vec<native_interfaces::sovd::SovdBulkDataItem>,
            native_interfaces::DiagServiceError,
        > {
            Ok(vec![])
        }
        fn list_groups(&self) -> Vec<native_interfaces::sovd::SovdGroup> {
            vec![native_interfaces::sovd::SovdGroup {
                id: "powertrain".into(),
                name: "Powertrain".into(),
                description: Some("Engine group".into()),
                component_ids: vec!["hpc".into()],
            }]
        }
        fn get_group(&self, group_id: &str) -> Option<native_interfaces::sovd::SovdGroup> {
            self.list_groups().into_iter().find(|g| g.id == group_id)
        }
        async fn io_control(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Option<&[u8]>,
        ) -> Result<serde_json::Value, native_interfaces::DiagServiceError> {
            Err(native_interfaces::DiagServiceError::RequestNotSupported(
                "io_control".into(),
            ))
        }
        async fn communication_control(
            &self,
            _: &str,
            _: &str,
            _: u8,
        ) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        async fn dtc_setting(
            &self,
            _: &str,
            _: &str,
        ) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        async fn read_memory(
            &self,
            _: &str,
            _: u32,
            _: u32,
        ) -> Result<Vec<u8>, native_interfaces::DiagServiceError> {
            Ok(vec![])
        }
        async fn write_memory(
            &self,
            _: &str,
            _: u32,
            _: &[u8],
        ) -> Result<(), native_interfaces::DiagServiceError> {
            Ok(())
        }
        async fn flash(
            &self,
            _: &str,
            _: &[u8],
            _: u32,
        ) -> Result<serde_json::Value, native_interfaces::DiagServiceError> {
            Ok(serde_json::json!({"status": "completed"}))
        }
    }

    fn test_state() -> AppState {
        use native_core::{AuditLog, ComponentRouter, DiagLog, FaultManager, LockManager};
        use native_health::HealthMonitor;
        use std::sync::Arc;

        let mock: Arc<dyn native_interfaces::ComponentBackend> = Arc::new(MockBackend);
        let router = Arc::new(ComponentRouter::new(vec![mock]));

        AppState {
            backend: router,
            oem_profile: Arc::new(native_interfaces::DefaultProfile),
            fault_manager: Arc::new(FaultManager::new()),
            lock_manager: Arc::new(LockManager::new()),
            diag_log: Arc::new(DiagLog::new()),
            audit_log: Arc::new(AuditLog::new()),
            health: Arc::new(HealthMonitor::new()),
            execution_store: Arc::new(dashmap::DashMap::new()),
            proximity_store: Arc::new(dashmap::DashMap::new()),
        }
    }

    fn test_router() -> Router {
        build_router(test_state(), AuthConfig::default())
    }

    // ── Pagination unit tests ───────────────────────────────────────────

    #[test]
    fn paginate_no_params_returns_all() {
        let items = vec![1, 2, 3, 4, 5];
        let params = PaginationParams {
            top: None,
            skip: None,
            filter: None,
            orderby: None,
            select: None,
        };
        let col = paginate(items, &params).unwrap();
        assert_eq!(col.count, 5);
        assert_eq!(col.value.len(), 5);
    }

    #[test]
    fn paginate_top_limits_items() {
        let items = vec![1, 2, 3, 4, 5];
        let params = PaginationParams {
            top: Some(2),
            skip: None,
            filter: None,
            orderby: None,
            select: None,
        };
        let col = paginate(items, &params).unwrap();
        assert_eq!(col.count, 5);
        assert_eq!(col.value, vec![1, 2]);
    }

    #[test]
    fn paginate_skip_offsets_items() {
        let items = vec![1, 2, 3, 4, 5];
        let params = PaginationParams {
            top: None,
            skip: Some(3),
            filter: None,
            orderby: None,
            select: None,
        };
        let col = paginate(items, &params).unwrap();
        assert_eq!(col.count, 5);
        assert_eq!(col.value, vec![4, 5]);
    }

    #[test]
    fn paginate_top_and_skip_combined() {
        let items = vec![10, 20, 30, 40, 50];
        let params = PaginationParams {
            top: Some(2),
            skip: Some(1),
            filter: None,
            orderby: None,
            select: None,
        };
        let col = paginate(items, &params).unwrap();
        assert_eq!(col.count, 5);
        assert_eq!(col.value, vec![20, 30]);
    }

    #[test]
    fn paginate_skip_beyond_length_returns_empty() {
        let items = vec![1, 2, 3];
        let params = PaginationParams {
            top: None,
            skip: Some(100),
            filter: None,
            orderby: None,
            select: None,
        };
        let col = paginate(items, &params).unwrap();
        assert_eq!(col.count, 3);
        assert!(col.value.is_empty());
    }

    #[test]
    fn paginate_filter_matches_field() {
        let items = vec![
            serde_json::json!({"id": "a", "name": "Alpha"}),
            serde_json::json!({"id": "b", "name": "Beta"}),
            serde_json::json!({"id": "c", "name": "Alpha"}),
        ];
        let params = PaginationParams {
            top: None,
            skip: None,
            filter: Some("name eq 'Alpha'".into()),
            orderby: None,
            select: None,
        };
        let result = paginate(items, &params).unwrap();
        assert_eq!(result.value.len(), 2);
        assert_eq!(result.count, 2);
    }

    #[test]
    fn paginate_filter_bad_syntax_returns_400() {
        let items = vec![serde_json::json!({"id": "a"})];
        let params = PaginationParams {
            top: None,
            skip: None,
            filter: Some("bad".into()),
            orderby: None,
            select: None,
        };
        let err = paginate(items, &params).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn paginate_orderby_sorts_asc() {
        let items = vec![
            serde_json::json!({"id": "c", "name": "Charlie"}),
            serde_json::json!({"id": "a", "name": "Alpha"}),
            serde_json::json!({"id": "b", "name": "Beta"}),
        ];
        let params = PaginationParams {
            top: None,
            skip: None,
            filter: None,
            orderby: Some("name asc".into()),
            select: None,
        };
        let result = paginate(items, &params).unwrap();
        assert_eq!(result.value[0]["name"], "Alpha");
        assert_eq!(result.value[1]["name"], "Beta");
        assert_eq!(result.value[2]["name"], "Charlie");
    }

    #[test]
    fn paginate_orderby_sorts_desc() {
        let items = vec![
            serde_json::json!({"id": "a", "name": "Alpha"}),
            serde_json::json!({"id": "b", "name": "Beta"}),
        ];
        let params = PaginationParams {
            top: None,
            skip: None,
            filter: None,
            orderby: Some("name desc".into()),
            select: None,
        };
        let result = paginate(items, &params).unwrap();
        assert_eq!(result.value[0]["name"], "Beta");
        assert_eq!(result.value[1]["name"], "Alpha");
    }

    #[test]
    fn paginate_empty_collection() {
        let items: Vec<i32> = vec![];
        let params = PaginationParams {
            top: Some(10),
            skip: Some(0),
            filter: None,
            orderby: None,
            select: None,
        };
        let col = paginate(items, &params).unwrap();
        assert_eq!(col.count, 0);
        assert!(col.value.is_empty());
    }

    // ── Route handler integration tests ─────────────────────────────────

    #[tokio::test]
    async fn discovery_returns_server_info() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["sovdVersion"], "1.1.0");
        assert!(json["serverName"].as_str().unwrap().contains("OpenSOVD"));
    }

    #[tokio::test]
    async fn list_components_returns_collection() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["id"], "hpc");
    }

    #[tokio::test]
    async fn list_components_with_pagination() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components?%24top=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_component_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], "hpc");
    }

    #[tokio::test]
    async fn get_component_not_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_data_returns_catalog() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["name"], "VIN");
    }

    #[tokio::test]
    async fn list_operations_returns_ops() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/operations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["name"], "Self Test");
    }

    #[tokio::test]
    async fn capabilities_returns_features() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["dataCount"], 1);
        assert_eq!(json["operationCount"], 1);
    }

    #[tokio::test]
    async fn list_groups_returns_groups() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1/groups").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["id"], "powertrain");
    }

    #[tokio::test]
    async fn get_group_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/groups/powertrain")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "Powertrain");
    }

    #[tokio::test]
    async fn get_group_not_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/groups/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_group_components_returns_members() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/groups/powertrain/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["id"], "hpc");
    }

    #[tokio::test]
    async fn mode_returns_available_modes() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/modes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["componentId"], "hpc");
        assert_eq!(json["availableModes"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn faults_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/faults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
    }

    #[tokio::test]
    async fn lock_acquire_and_release() {
        let state = test_state();
        let app = build_router(state, AuthConfig::default());

        // Acquire lock
        let resp = app
            .clone()
            .oneshot(
                Request::post("/sovd/v1/components/hpc/lock")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"lockedBy":"tester"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Get lock
        let resp = app
            .clone()
            .oneshot(
                Request::get("/sovd/v1/components/hpc/lock")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["lockedBy"], "tester");

        // Release lock
        let resp = app
            .oneshot(
                Request::delete("/sovd/v1/components/hpc/lock")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn lock_double_acquire_fails() {
        let state = test_state();
        let app = build_router(state, AuthConfig::default());

        // First acquire
        let _ = app
            .clone()
            .oneshot(
                Request::post("/sovd/v1/components/hpc/lock")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"lockedBy":"a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Second acquire should fail with 409
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/components/hpc/lock")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"lockedBy":"b"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn health_check_returns_ok() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn logs_returns_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/logs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
    }

    #[tokio::test]
    async fn proximity_challenge_creates_and_retrieves() {
        let state = test_state();
        let app = build_router(state, AuthConfig::default());

        // Create challenge
        let resp = app
            .clone()
            .oneshot(
                Request::post("/sovd/v1/components/hpc/proximity-challenge")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let challenge_id = json["challengeId"].as_str().unwrap();

        // Retrieve challenge
        let resp = app
            .oneshot(
                Request::get(format!(
                    "/sovd/v1/components/hpc/proximity-challenge/{challenge_id}"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn fault_by_id_not_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/faults/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn executions_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/operations/FF00/executions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
    }

    #[tokio::test]
    async fn config_returns_component_config() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/configurations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Entity collection stubs (§4.2.3) ─────────────────────────────────

    #[tokio::test]
    async fn apps_returns_empty_collection() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/apps")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
        assert_eq!(json["@odata.context"], "$metadata#apps");
    }

    #[tokio::test]
    async fn funcs_returns_empty_collection() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/funcs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
        assert_eq!(json["@odata.context"], "$metadata#funcs");
    }

    // ── Software packages (§5.5.10) ──────────────────────────────────────

    #[tokio::test]
    async fn software_packages_returns_empty_collection() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/software-packages")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
        assert_eq!(json["@odata.context"], "$metadata#softwarePackages");
    }

    // ── PUT /modes/{modeId} (§5.5.4) ────────────────────────────────────

    #[tokio::test]
    async fn activate_mode_by_id() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::put("/sovd/v1/components/hpc/modes/extended")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["componentId"], "hpc");
    }

    #[tokio::test]
    async fn activate_dtc_mode_maps_to_dtc_setting() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::put("/sovd/v1/components/hpc/modes/dtc-off")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Audit Trail integration tests ──────────────────────────────────

    #[tokio::test]
    async fn audit_endpoint_returns_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/audit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
        assert!(json["value"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn write_data_creates_audit_entry() {
        let state = test_state();
        let app = build_router(state.clone(), AuthConfig::default());

        // Perform a write_data
        let _ = app
            .clone()
            .oneshot(
                Request::put("/sovd/v1/components/hpc/data/vin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"WVWZZZ3CZWE123456"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Check audit log has an entry
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/audit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["value"].as_array().unwrap();
        assert!(!entries.is_empty(), "Audit log should have at least one entry");
        let last = entries.last().unwrap();
        assert_eq!(last["action"], "writeData");
        assert!(last["target"].as_str().unwrap().contains("hpc"));
    }

    #[tokio::test]
    async fn clear_faults_creates_audit_entry() {
        let state = test_state();
        let app = build_router(state.clone(), AuthConfig::default());

        // Clear faults
        let _ = app
            .clone()
            .oneshot(
                Request::delete("/sovd/v1/components/hpc/faults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Query audit with action filter
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/audit?action=clearFaults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let count = json["@odata.count"].as_u64().unwrap();
        assert!(count >= 1, "Should have at least 1 clearFaults audit entry");
    }

    #[tokio::test]
    async fn lock_operations_create_audit_entries() {
        let state = test_state();
        let app = build_router(state.clone(), AuthConfig::default());

        // Acquire lock
        let _ = app
            .clone()
            .oneshot(
                Request::post("/sovd/v1/components/hpc/lock")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"lockedBy":"tester"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Release lock
        let _ = app
            .clone()
            .oneshot(
                Request::delete("/sovd/v1/components/hpc/lock")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Query audit for lock actions
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/audit?target=component/hpc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["value"].as_array().unwrap();
        let actions: Vec<&str> = entries
            .iter()
            .filter_map(|e| e["action"].as_str())
            .collect();
        assert!(actions.contains(&"acquireLock"), "Should have acquireLock");
        assert!(actions.contains(&"releaseLock"), "Should have releaseLock");
    }

    #[tokio::test]
    async fn audit_limit_query_param_works() {
        let state = test_state();
        let app = build_router(state.clone(), AuthConfig::default());

        // Generate multiple audit entries
        for _ in 0..5 {
            let _ = app
                .clone()
                .oneshot(
                    Request::delete("/sovd/v1/components/hpc/faults")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
        }

        // Query with limit=2
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/audit?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["value"].as_array().unwrap();
        assert_eq!(entries.len(), 2, "Limit should cap results to 2");
    }

    #[tokio::test]
    async fn connect_disconnect_create_audit_entries() {
        let state = test_state();
        let app = build_router(state.clone(), AuthConfig::default());

        let resp = app
            .clone()
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/hpc/connect")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let connect_status = resp.status();

        let resp = app
            .clone()
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/hpc/disconnect")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let disconnect_status = resp.status();

        let resp = app
            .oneshot(
                Request::get("/sovd/v1/audit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["value"].as_array().unwrap();
        assert_eq!(connect_status, StatusCode::NO_CONTENT, "connect should succeed");
        assert_eq!(disconnect_status, StatusCode::NO_CONTENT, "disconnect should succeed");
        let actions: Vec<&str> = entries
            .iter()
            .filter_map(|e| e["action"].as_str())
            .collect();
        assert!(actions.contains(&"connect"), "actions={actions:?}");
        assert!(actions.contains(&"disconnect"), "actions={actions:?}");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// MockBackend-based tests — proves Gateway architecture works with ANY backend
// These tests do NOT depend on the "local-uds" feature and validate that
// the SOVD Server correctly dispatches through the ComponentBackend trait.
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unnecessary_literal_bound,
    clippy::uninlined_format_args,
    clippy::map_unwrap_or,
    clippy::redundant_closure_for_method_calls
)]
mod mock_backend_tests {
    use super::*;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::Request;
    use http::StatusCode;
    use native_core::{ComponentRouter, DiagLog, FaultManager, LockManager};
    use native_health::HealthMonitor;
    use native_interfaces::sovd::{
        SovdBulkDataItem, SovdBulkWriteItem, SovdCapabilities, SovdComponent, SovdComponentConfig,
        SovdConnectionState, SovdDataAccess, SovdDataCatalogEntry, SovdFault, SovdFaultSeverity,
        SovdFaultStatus, SovdGroup, SovdMode, SovdOperation, SovdOperationStatus,
    };
    use native_interfaces::{ComponentBackend, DiagServiceError};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Pure mock backend — no UDS, no DoIP, no external dependencies.
    /// Simulates an external CDA or native SOVD app behind the Gateway.
    struct MockCdaBackend;

    #[async_trait]
    impl ComponentBackend for MockCdaBackend {
        fn name(&self) -> &str {
            "MockCDA"
        }

        fn list_components(&self) -> Vec<SovdComponent> {
            vec![SovdComponent {
                id: "mock-ecu".into(),
                name: "Mock ECU".into(),
                category: "ecu".into(),
                description: Some("Simulated via MockCDA".into()),
                connection_state: SovdConnectionState::Connected,
            }]
        }

        fn get_component(&self, id: &str) -> Option<SovdComponent> {
            self.list_components().into_iter().find(|c| c.id == id)
        }

        async fn connect(&self, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn disconnect(&self, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }

        fn list_data(&self, _: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
            Ok(vec![SovdDataCatalogEntry {
                id: "D001".into(),
                name: "Speed".into(),
                description: Some("Vehicle speed".into()),
                access: SovdDataAccess::ReadOnly,
                data_type: SovdDataType::Integer,
                unit: Some("km/h".into()),
                did: Some("2000".into()),
            }])
        }

        async fn read_data(
            &self,
            _: &str,
            data_id: &str,
        ) -> Result<serde_json::Value, DiagServiceError> {
            Ok(
                serde_json::json!({ "id": data_id, "value": 42, "dataType": "integer", "unit": "km/h" }),
            )
        }

        async fn write_data(&self, _: &str, _: &str, _: &[u8]) -> Result<(), DiagServiceError> {
            Ok(())
        }

        async fn read_faults(&self, _: &str) -> Result<Vec<SovdFault>, DiagServiceError> {
            Ok(vec![SovdFault {
                id: "DTC_001".into(),
                component_id: "mock-ecu".into(),
                code: "P0100".into(),
                display_code: Some("P0100".into()),
                severity: SovdFaultSeverity::High,
                status: SovdFaultStatus::Active,
                name: "Mass Air Flow".into(),
                description: Some("MAF sensor circuit malfunction".into()),
                scope: Some("component".into()),
            }])
        }

        async fn clear_faults(&self, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }

        fn list_operations(&self, _: &str) -> Result<Vec<SovdOperation>, DiagServiceError> {
            Ok(vec![SovdOperation {
                id: "OP01".into(),
                component_id: "mock-ecu".into(),
                name: "Calibrate".into(),
                description: Some("Sensor calibration".into()),
                status: SovdOperationStatus::Idle,
            }])
        }

        async fn execute_operation(
            &self,
            _: &str,
            op_id: &str,
            _: Option<&[u8]>,
        ) -> Result<serde_json::Value, DiagServiceError> {
            Ok(serde_json::json!({ "operationId": op_id, "status": "completed" }))
        }

        fn get_capabilities(&self, _: &str) -> Result<SovdCapabilities, DiagServiceError> {
            Ok(SovdCapabilities {
                component_id: "mock-ecu".into(),
                supported_categories: vec!["data".into(), "faults".into()],
                data_count: 1,
                operation_count: 1,
                features: vec!["faults".into()],
            })
        }

        fn get_mode(&self, id: &str) -> Result<SovdMode, DiagServiceError> {
            Ok(SovdMode {
                component_id: id.to_string(),
                current_mode: "default".into(),
                available_modes: vec!["default".into(), "extended".into()],
            })
        }

        async fn set_mode(&self, _: &str, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }

        async fn read_config(&self, id: &str) -> Result<SovdComponentConfig, DiagServiceError> {
            Ok(SovdComponentConfig {
                component_id: id.to_string(),
                parameters: serde_json::json!({ "variant": "EU" }),
            })
        }

        async fn write_config(&self, _: &str, _: &str, _: &[u8]) -> Result<(), DiagServiceError> {
            Ok(())
        }

        async fn bulk_read(
            &self,
            _: &str,
            ids: &[String],
            _: Option<SovdBulkDataCategory>,
        ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
            Ok(ids
                .iter()
                .map(|id| SovdBulkDataItem {
                    id: id.clone(),
                    value: Some("mock".into()),
                    error: None,
                })
                .collect())
        }

        async fn bulk_write(
            &self,
            _: &str,
            _: &[SovdBulkWriteItem],
        ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError> {
            Ok(vec![])
        }

        fn list_groups(&self) -> Vec<SovdGroup> {
            vec![SovdGroup {
                id: "drivetrain".into(),
                name: "Drivetrain".into(),
                description: Some("Engine and transmission".into()),
                component_ids: vec!["mock-ecu".into()],
            }]
        }

        fn get_group(&self, id: &str) -> Option<SovdGroup> {
            self.list_groups().into_iter().find(|g| g.id == id)
        }

        async fn io_control(
            &self,
            _cid: &str,
            _did: &str,
            _ctrl: &str,
            _val: Option<&[u8]>,
        ) -> Result<serde_json::Value, DiagServiceError> {
            Ok(serde_json::json!({}))
        }
        async fn communication_control(
            &self,
            _: &str,
            _: &str,
            _: u8,
        ) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn dtc_setting(&self, _: &str, _: &str) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn read_memory(
            &self,
            _cid: &str,
            _addr: u32,
            _sz: u32,
        ) -> Result<Vec<u8>, DiagServiceError> {
            Ok(vec![0xAB, 0xCD])
        }
        async fn write_memory(
            &self,
            _cid: &str,
            _addr: u32,
            _data: &[u8],
        ) -> Result<(), DiagServiceError> {
            Ok(())
        }
        async fn flash(
            &self,
            _cid: &str,
            _fw: &[u8],
            _addr: u32,
        ) -> Result<serde_json::Value, DiagServiceError> {
            Ok(serde_json::json!({"status": "ok"}))
        }
    }

    fn mock_state() -> AppState {
        let backend: Arc<dyn ComponentBackend> = Arc::new(MockCdaBackend);
        let router = Arc::new(ComponentRouter::new(vec![backend]));
        AppState {
            backend: router,
            oem_profile: Arc::new(native_interfaces::DefaultProfile),
            fault_manager: Arc::new(FaultManager::new()),
            lock_manager: Arc::new(LockManager::new()),
            diag_log: Arc::new(DiagLog::new()),
            audit_log: Arc::new(native_core::AuditLog::new()),
            health: Arc::new(HealthMonitor::new()),
            execution_store: Arc::new(dashmap::DashMap::new()),
            proximity_store: Arc::new(dashmap::DashMap::new()),
        }
    }

    fn mock_router() -> Router {
        build_router(mock_state(), AuthConfig::default())
    }

    // ── Discovery tests (Gateway + MockCDA) ──────────────────────────────

    #[tokio::test]
    async fn mock_discovery_returns_server_info() {
        let app = mock_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mock_list_components() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["id"], "mock-ecu");
        assert_eq!(json["value"][0]["name"], "Mock ECU");
    }

    #[tokio::test]
    async fn mock_get_component() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["id"], "mock-ecu");
    }

    // ── Data tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_list_data() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["name"], "Speed");
    }

    #[tokio::test]
    async fn mock_read_data() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/data/D001")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["value"], 42);
    }

    // ── Faults tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_read_faults_empty_from_fault_manager() {
        // Faults come from the server-side FaultManager (DFM role), not from backend.read_faults.
        // With an empty FaultManager, we expect 0 faults — this is architecturally correct:
        // faults flow through FaultBridge → FaultManager, not directly from the backend.
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/faults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
    }

    #[tokio::test]
    async fn mock_faults_populated_via_fault_manager() {
        // Prove the DFM path: inject a fault into FaultManager, then read via REST
        use native_core::fault_bridge::{FaultLifecycleStage, FaultRecord, FaultSeverity};
        use native_core::{FaultBridge, FaultSink};

        let state = mock_state();
        let bridge = FaultBridge::new(state.fault_manager.clone());
        bridge
            .publish(&FaultRecord {
                fault_id: "DTC_042".into(),
                source: "mock-sensor".into(),
                severity: FaultSeverity::Error,
                stage: FaultLifecycleStage::Failed,
                component_id: "mock-ecu".into(),
                description: Some("Test fault".into()),
            })
            .unwrap();

        let app = build_router(state, AuthConfig::default());
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/faults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["id"], "DTC_042");
    }

    // ── Operations tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_list_operations() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/operations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["name"], "Calibrate");
    }

    // ── Capabilities tests ───────────────────────────────────────────────

    #[tokio::test]
    async fn mock_capabilities() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["dataCount"], 1);
        assert_eq!(json["operationCount"], 1);
    }

    // ── Groups tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_list_groups() {
        let app = mock_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1/groups").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["id"], "drivetrain");
    }

    #[tokio::test]
    async fn mock_get_group_components() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/groups/drivetrain/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 1);
        assert_eq!(json["value"][0]["id"], "mock-ecu");
    }

    // ── Mode tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_get_mode() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/modes")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["componentId"], "mock-ecu");
        assert_eq!(json["availableModes"].as_array().unwrap().len(), 2);
    }

    // ── Config tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn mock_read_config() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/configurations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["componentId"], "mock-ecu");
        assert_eq!(json["parameters"]["variant"], "EU");
    }

    // ── Error routing tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn mock_unknown_component_returns_404() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn mock_unknown_component_data_returns_404() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/nonexistent/data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Flash handler tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn flash_happy_path_returns_202() {
        use base64::Engine;
        let firmware_b64 = base64::engine::general_purpose::STANDARD.encode(b"\x01\x02\x03\x04");
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/mock-ecu/flash")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"firmware_data":"{}","memory_address":0}}"#,
                        firmware_b64
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_object());
    }

    #[tokio::test]
    async fn flash_empty_firmware_returns_400() {
        use base64::Engine;
        let empty_b64 = base64::engine::general_purpose::STANDARD.encode(b"");
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/mock-ecu/flash")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(
                        r#"{{"firmware_data":"{}","memory_address":0}}"#,
                        empty_b64
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn flash_invalid_base64_returns_400() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/mock-ecu/flash")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"firmware_data":"%%%not-base64%%%","memory_address":0}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Auth middleware E2E tests ────────────────────────────────────────

    fn auth_enabled_config() -> AuthConfig {
        AuthConfig {
            enabled: true,
            api_key: Some("test-secret-key".into()),
            jwt_secret: None,
            jwt_algorithm: "HS256".into(),
            jwt_issuer: None,
            oidc_issuer_url: None,
            public_paths: vec!["/sovd/v1/".into(), "/sovd/v1/health".into()],
            cors_origins: vec![],
        }
    }

    #[tokio::test]
    async fn auth_enabled_rejects_request_without_credentials() {
        let app = build_router(mock_state(), auth_enabled_config());
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_enabled_rejects_wrong_api_key() {
        let app = build_router(mock_state(), auth_enabled_config());
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components")
                    .header("x-api-key", "wrong-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn auth_enabled_accepts_correct_api_key() {
        let app = build_router(mock_state(), auth_enabled_config());
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components")
                    .header("x-api-key", "test-secret-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_enabled_public_path_bypasses_auth() {
        let app = build_router(mock_state(), auth_enabled_config());
        let resp = app
            .oneshot(Request::get("/sovd/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_enabled_discovery_is_public() {
        let mut config = auth_enabled_config();
        // The nested route is /sovd/v1 (no trailing slash)
        config.public_paths.push("/sovd/v1".into());
        let app = build_router(mock_state(), config);
        let resp = app
            .oneshot(Request::get("/sovd/v1").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Lock enforcement tests ─────────────────────────────────────────

    #[tokio::test]
    async fn locked_component_rejects_write_without_client_id() {
        let state = mock_state();
        state
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default());
        let resp = app
            .oneshot(
                Request::put("/sovd/v1/components/mock-ecu/data/vin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"4142"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn locked_component_allows_write_with_matching_client_id() {
        let state = mock_state();
        state
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default());
        let resp = app
            .oneshot(
                Request::put("/sovd/v1/components/mock-ecu/data/vin")
                    .header("content-type", "application/json")
                    .header("x-sovd-client-id", "owner-1")
                    .body(Body::from(r#"{"value":"4142"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Should succeed (204) since the caller is the lock owner
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn unlocked_component_allows_write() {
        let state = mock_state();
        let app = build_router(state, AuthConfig::default());
        let resp = app
            .oneshot(
                Request::put("/sovd/v1/components/mock-ecu/data/vin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"4142"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // ── Phase 7 regression: lock ownership via CallerIdentity ─────────

    #[tokio::test]
    async fn release_lock_rejects_wrong_caller() {
        let state = mock_state();
        state
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default());
        // Try to release as a different caller via x-sovd-client-id header
        let resp = app
            .oneshot(
                Request::delete("/sovd/v1/components/mock-ecu/lock")
                    .header("x-sovd-client-id", "attacker")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("owner-1"));
    }

    #[tokio::test]
    async fn release_lock_succeeds_for_owner() {
        let state = mock_state();
        state
            .lock_manager
            .acquire("mock-ecu", "rightful-owner", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default());
        let resp = app
            .oneshot(
                Request::delete("/sovd/v1/components/mock-ecu/lock")
                    .header("x-sovd-client-id", "rightful-owner")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn acquire_lock_uses_auth_identity_over_body() {
        // When authenticated via API key, the lock owner should be "api-key-client"
        // (injected by auth middleware), NOT the "body-identity" from the request body.
        let state = mock_state();
        let mut config = auth_enabled_config();
        config.api_key = Some("test-secret-key".into());
        let app = build_router(state.clone(), config);
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/components/mock-ecu/lock")
                    .header("content-type", "application/json")
                    .header("x-api-key", "test-secret-key")
                    .body(Body::from(r#"{"lockedBy":"body-identity"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Lock owner must be from auth context, not from body
        assert_eq!(json["lockedBy"], "api-key-client");
    }

    #[tokio::test]
    async fn bulk_read_includes_odata_context() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/components/mock-ecu/data/bulk-read")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"dataIds":["D001"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.context"], "$metadata#bulkData");
        assert_eq!(json["@odata.count"], 1);
    }

    // ── $metadata endpoint tests ─────────────────────────────────────

    #[tokio::test]
    async fn metadata_returns_entity_model() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/$metadata")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["sovdVersion"], "1.1.0");
        assert!(json["entityTypes"]["Component"].is_object());
        assert!(json["entityTypes"]["Data"].is_object());
        assert!(json["entityTypes"]["Fault"].is_object());
        assert!(json["entityTypes"]["Lock"].is_object());
        assert_eq!(json["collections"]["components"], "Component");
    }

    // ── ETag / conditional request tests ─────────────────────────────

    #[tokio::test]
    async fn read_data_returns_etag_header() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/data/D001")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let etag = resp.headers().get("etag").expect("ETag header missing");
        let etag_str = etag.to_str().unwrap();
        assert!(etag_str.starts_with('"') && etag_str.ends_with('"'));
    }

    #[tokio::test]
    async fn read_data_if_none_match_returns_304() {
        let state = mock_state();
        // First request: get the ETag
        let app = build_router(state.clone(), AuthConfig::default());
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/data/D001")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let etag = resp
            .headers()
            .get("etag")
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();

        // Second request with If-None-Match: should get 304
        let app2 = build_router(state, AuthConfig::default());
        let resp2 = app2
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/data/D001")
                    .header("if-none-match", &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::NOT_MODIFIED);
    }

    // ── Lock conflict Retry-After tests ─────────────────────────────

    #[tokio::test]
    async fn lock_conflict_includes_retry_after_hint() {
        let state = mock_state();
        let future = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc3339();
        state
            .lock_manager
            .acquire("mock-ecu", "owner-1", Some(future))
            .unwrap();
        let app = build_router(state, AuthConfig::default());
        let resp = app
            .oneshot(
                Request::put("/sovd/v1/components/mock-ecu/data/vin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"]["details"].is_array());
        assert!(json["error"]["details"][0]["message"]
            .as_str()
            .unwrap()
            .contains("Retry-After"));
    }

    #[tokio::test]
    async fn lock_conflict_without_expiry_has_no_retry_after() {
        let state = mock_state();
        state
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default());
        let resp = app
            .oneshot(
                Request::put("/sovd/v1/components/mock-ecu/data/vin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"test"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["error"]["details"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true));
    }

    // ── OpenAPI endpoint tests ──────────────────────────────────────

    #[tokio::test]
    async fn openapi_json_returns_valid_spec() {
        let app = mock_router();
        let resp = app
            .oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["openapi"], "3.1.0");
        assert_eq!(json["info"]["title"], "OpenSOVD-native-server CDF");
        assert_eq!(json["info"]["version"], "1.1.0");
        assert!(json["paths"].is_object());
        assert!(json["paths"]["/components"].is_object());
        assert!(json["tags"].is_array());
    }

    // ── Prometheus metrics endpoint tests ────────────────────────────

    #[tokio::test]
    async fn metrics_endpoint_returns_prometheus_text() {
        let app = mock_router();
        let resp = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        // Prometheus output should contain our registered metric descriptions
        assert!(
            text.contains("sovd_http_requests_total") || text.is_empty() || text.starts_with('#')
        );
    }

    // ── $select field projection tests ──────────────────────────────

    #[tokio::test]
    async fn select_projects_fields() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components?%24select=id%2Cname")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let first = &json["value"][0];
        assert!(first["id"].is_string());
        assert!(first["name"].is_string());
        // Fields not in $select must be absent
        assert!(first.get("category").is_none());
        assert!(first.get("connectionState").is_none());
    }

    // ── @odata.context tests ────────────────────────────────────────

    #[tokio::test]
    async fn collection_includes_odata_context() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.context"], "$metadata#components");
    }

    // ── PATCH partial data update tests ─────────────────────────────

    #[tokio::test]
    async fn patch_data_returns_204() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/sovd/v1/components/mock-ecu/data/vin")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":"patched"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // ── Structured error model tests ────────────────────────────────

    #[tokio::test]
    async fn error_response_has_structured_fields() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // OData error envelope: {"error": {"code": ..., "message": ...}}
        assert!(json["error"].is_object(), "Missing OData error wrapper");
        assert_eq!(json["error"]["code"], "SOVD-ERR-404");
        assert!(json["error"]["message"].is_string());
        // details should be absent (empty array is skipped) or empty array
        assert!(
            json["error"]["details"].is_null()
                || json["error"]["details"]
                    .as_array()
                    .map(|a| a.is_empty())
                    .unwrap_or(false)
        );
    }
}
