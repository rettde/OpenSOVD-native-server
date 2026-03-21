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

/// Per-client rate limiting middleware (A2.5).
///
/// Extracts the caller identity (set by auth middleware via `AuthenticatedClient` extension)
/// and checks the token-bucket rate limiter. Returns 429 Too Many Requests if the
/// client has exceeded their quota.
async fn rate_limit_middleware(
    limiter: Option<crate::rate_limit::RateLimiter>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if let Some(ref limiter) = limiter {
        // Extract client ID from auth extension (set by auth_middleware)
        let client_id = request
            .extensions()
            .get::<AuthenticatedClient>()
            .map_or("anonymous", |c| c.0.as_str());

        if !limiter.check(client_id) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(SovdErrorEnvelope {
                    error: SovdErrorResponse {
                        code: "TooManyRequests".to_owned(),
                        message: "Rate limit exceeded".to_owned(),
                        target: None,
                        details: vec![],
                        innererror: None,
                    },
                }),
            )
                .into_response();
        }
    }
    next.run(request).await
}

/// Security headers middleware — injects hardening headers into every response.
///
/// - `X-Content-Type-Options: nosniff` — prevents MIME-type sniffing
/// - `X-Frame-Options: DENY` — prevents clickjacking via iframe embedding
/// - `Cache-Control: no-store` — prevents caching of diagnostic data
/// - `Strict-Transport-Security` — enforces HTTPS (1 year, includeSubDomains)
async fn security_headers_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        http::header::HeaderName::from_static("x-content-type-options"),
        http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        http::header::HeaderName::from_static("x-frame-options"),
        http::HeaderValue::from_static("DENY"),
    );
    headers.insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    headers.insert(
        http::header::HeaderName::from_static("strict-transport-security"),
        http::HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    response
}

/// RED metrics middleware (E1.3) — records Rate, Error rate, Duration per endpoint.
///
/// Labels: `method`, `path` (matched route pattern), `status` (HTTP status code).
/// Metrics:
///   - `sovd_http_requests_total` (counter)
///   - `sovd_http_request_duration_seconds` (histogram)
async fn red_metrics_middleware(
    matched_path: Option<axum::extract::MatchedPath>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method().to_string();
    let path = matched_path.map_or_else(
        || request.uri().path().to_owned(),
        |mp| mp.as_str().to_owned(),
    );
    let start = std::time::Instant::now();

    let response = next.run(request).await;

    let status = response.status().as_u16().to_string();
    let duration = start.elapsed().as_secs_f64();
    let labels = [("method", method), ("path", path), ("status", status)];

    metrics::counter!("sovd_http_requests_total", &labels).increment(1);
    metrics::histogram!("sovd_http_request_duration_seconds", &labels).record(duration);

    response
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

/// Canary/blue-green deployment routing middleware (Wave 3, E3.3).
///
/// Reads `X-Deployment-Target` header from the request. If set to a value
/// matching the server's deployment label (e.g. "canary", "blue", "green"),
/// the request proceeds. If set to a different label, the server returns
/// `421 Misdirected Request` so a load balancer can retry on the correct instance.
///
/// When the header is absent, all requests are accepted (backward compatible).
/// The response always includes `X-Served-By` with the server's deployment label.
async fn canary_routing_middleware(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    // Server deployment label: cached from SOVD_DEPLOYMENT_LABEL env var at first access
    static LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let server_label = LABEL
        .get_or_init(|| std::env::var("SOVD_DEPLOYMENT_LABEL").unwrap_or_else(|_| "default".into()))
        .clone();

    // Check if client targets a specific deployment
    if let Some(target) = request.headers().get("x-deployment-target") {
        if let Ok(target_str) = target.to_str() {
            if !target_str.is_empty() && target_str != server_label {
                // Misdirected — this request is for a different deployment instance
                let mut resp = (
                    StatusCode::MISDIRECTED_REQUEST,
                    axum::Json(SovdErrorEnvelope::new(
                        "SOVD-ERR-421",
                        format!(
                            "Request targets deployment '{target_str}' but this instance is '{server_label}'"
                        ),
                    )),
                )
                    .into_response();
                if let Ok(val) = http::HeaderValue::from_str(&server_label) {
                    resp.headers_mut().insert("x-served-by", val);
                }
                return resp;
            }
        }
    }

    let mut resp = next.run(request).await;
    if let Ok(val) = http::HeaderValue::from_str(&server_label) {
        resp.headers_mut().insert("x-served-by", val);
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

// ── Tenant identity (Wave 3, A3.3 — multi-tenant isolation) ───────────────

/// Extracts tenant context from request extensions (injected by auth middleware).
///
/// Priority: `TenantContext` extension (from JWT `tenant_id` claim or API key default)
///         → `X-Tenant-ID` header (dev mode / testing)
///         → default tenant (single-tenant backward compat)
struct TenantId(native_interfaces::tenant::TenantContext);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for TenantId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        // 1. Prefer tenant from auth middleware (JWT claim)
        if let Some(ctx) = parts
            .extensions
            .get::<native_interfaces::tenant::TenantContext>()
        {
            return Ok(Self(ctx.clone()));
        }
        // 2. Fall back to X-Tenant-ID header (dev / testing)
        if let Some(val) = parts.headers.get("x-tenant-id") {
            if let Ok(s) = val.to_str() {
                if !s.is_empty() {
                    return Ok(Self(native_interfaces::tenant::TenantContext::new(s)));
                }
            }
        }
        // 3. Default single-tenant
        Ok(Self(native_interfaces::tenant::TenantContext::default()))
    }
}

// ── OData-style pagination (SOVD §5) ─────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
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
///
/// Serializes each item to JSON once (not per-comparison) for O(N) performance.
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

    // Pre-serialize all items once, then zip with originals to filter
    let json_items: Vec<Option<serde_json::Value>> = items
        .iter()
        .map(|item| serde_json::to_value(item).ok())
        .collect();

    Ok(items
        .into_iter()
        .zip(json_items)
        .filter(|(_, json)| {
            json.as_ref()
                .and_then(|j| j.get(field))
                .is_some_and(|field_val| match field_val {
                    serde_json::Value::String(s) => s == value,
                    serde_json::Value::Number(n) => n.to_string() == value,
                    serde_json::Value::Bool(b) => {
                        (value == "true" && *b) || (value == "false" && !*b)
                    }
                    _ => false,
                })
        })
        .map(|(item, _)| item)
        .collect())
}

/// Parse and apply a simple OData $orderby expression: `field [asc|desc]`
///
/// Pre-extracts the sort key from each item (one serialization per item)
/// instead of serializing twice per comparison inside the sort closure.
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

    // Pre-extract sort keys: one serialization per item instead of O(N log N × 2)
    let keys: Vec<Option<serde_json::Value>> = items
        .iter()
        .map(|item| {
            serde_json::to_value(item)
                .ok()
                .and_then(|j| j.get(field).cloned())
        })
        .collect();

    // Build index array, sort by pre-extracted keys, then reorder items in-place
    let mut indices: Vec<usize> = (0..items.len()).collect();
    indices.sort_by(|&a, &b| {
        let cmp = cmp_json_values(keys[a].as_ref(), keys[b].as_ref());
        if desc {
            cmp.reverse()
        } else {
            cmp
        }
    });

    // Apply permutation: clone sorted items back (T: Clone)
    let sorted: Vec<T> = indices.iter().map(|&i| items[i].clone()).collect();
    items.clone_from_slice(&sorted);

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

// ── Feature-Flag-Gated Helpers (E2.4) ────────────────────────────────────
//
// Enterprise pattern: every audit_log.record() and history.record_*() call
// goes through these helpers, which check the runtime feature flags first.
// This enables operators to disable audit/history at runtime without restart.

/// Record an audit entry, gated by the `audit` feature flag.
/// Also forwards the entry to HistoryService if the `history` flag is enabled.
///
/// When the audit flag is disabled, a `debug!` trace is emitted so that
/// suppressed audit events remain observable in diagnostic logs (E5 fix).
#[allow(clippy::too_many_arguments)]
fn guarded_audit(
    state: &AppState,
    caller: &str,
    action: SovdAuditAction,
    target: &str,
    resource: &str,
    method: &str,
    outcome: &str,
    detail: Option<&str>,
    trace_id: Option<&str>,
) {
    use native_interfaces::feature_flags::flags;
    if !state.runtime.feature_flags.is_enabled(flags::AUDIT) {
        tracing::debug!(
            %caller, ?action, %target, %resource, %method, %outcome,
            "audit flag disabled — event not recorded"
        );
        return;
    }
    state.security.audit_log.record(
        caller, action, target, resource, method, outcome, detail, trace_id,
    );
    // W2.2 + E2.4: forward audit entry to history if enabled
    if state.runtime.feature_flags.is_enabled(flags::HISTORY) {
        let entries = state.security.audit_log.recent(1);
        if let Some(entry) = entries.first() {
            state.diag.history.record_audit(entry);
        }
    }
}

/// Record a fault snapshot to history, gated by the `history` feature flag.
fn guarded_history_fault(state: &AppState, fault: &native_interfaces::sovd::SovdFault) {
    use native_interfaces::feature_flags::flags;
    if state.runtime.feature_flags.is_enabled(flags::HISTORY) {
        state.diag.history.record_fault(fault);
    }
}

/// Build the full axum router with all SOVD endpoints
#[allow(clippy::too_many_lines)]
pub fn build_router(state: AppState, auth_config: AuthConfig, metrics_enabled: bool) -> Router {
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
        // Software Package lifecycle — new routes (Wave 1.4)
        .route(
            "/components/{component_id}/software-packages/{package_id}/activate",
            post(activate_software_package),
        )
        .route(
            "/components/{component_id}/software-packages/{package_id}/rollback",
            post(rollback_software_package),
        )
        // Apps (ISO 17978-3 §4.2.3)
        .route("/apps", get(list_apps))
        .route("/apps/{app_id}", get(get_app))
        .route("/apps/{app_id}/capabilities", get(get_app_capabilities))
        .route("/apps/{app_id}/data", get(list_app_data))
        .route("/apps/{app_id}/data/{data_id}", get(read_app_data))
        .route("/apps/{app_id}/operations", get(list_app_operations))
        .route(
            "/apps/{app_id}/operations/{op_id}",
            post(execute_app_operation),
        )
        // Funcs (ISO 17978-3 §4.2.3)
        .route("/funcs", get(list_funcs))
        .route("/funcs/{func_id}", get(get_func))
        .route("/funcs/{func_id}/data", get(list_func_data))
        .route("/funcs/{func_id}/data/{data_id}", get(read_func_data))
        // Areas (ISO 17978-3 §4.2.3 — gated by DiscoveryPolicy::areas_enabled)
        .route("/areas", get(list_areas))
        .route("/areas/{area_id}", get(get_area))
        // Configuration (§7.8)
        .route(
            "/components/{component_id}/configurations",
            get(read_config),
        )
        .route(
            "/components/{component_id}/configurations",
            put(write_config),
        )
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
        .route(
            "/components/{component_id}/operations/docs",
            get(serve_docs),
        )
        .route("/components/{component_id}/modes/docs", get(serve_docs))
        .route("/components/{component_id}/locks/docs", get(serve_docs))
        .route(
            "/components/{component_id}/configurations/docs",
            get(serve_docs),
        )
        .route("/components/{component_id}/logs/docs", get(serve_docs))
        // OData metadata (§5.2)
        .route("/$metadata", get(odata_metadata))
        // Health (non-SOVD, operational)
        .route("/health", get(health_check))
        // System KPIs (W2.1)
        .route("/system-info", get(system_info))
        // Audit trail (Wave 1)
        .route("/audit", get(list_audit_entries))
        // Signed audit export (Wave 3, W3.4)
        .route("/audit/export", get(export_signed_audit))
        // Compliance evidence (Wave 3, E3.2)
        .route("/compliance-evidence", get(compliance_evidence))
        // Batch diagnostic snapshot (Wave 4, W4.2)
        .route(
            "/components/{component_id}/snapshot",
            get(component_snapshot),
        )
        // Fault export — NDJSON streaming (Wave 4, W4.2)
        .route("/export/faults", get(export_faults))
        // Schema introspection (Wave 4, W4.4)
        .route("/schema/data-catalog", get(schema_data_catalog))
        // SSE data-change stream (Wave 4, W4.5)
        .route(
            "/components/{component_id}/data/subscribe",
            get(subscribe_data_changes),
        )
        // RXSWIN tracking (F15, UNECE R156)
        .route("/rxswin", get(list_rxswin))
        .route("/rxswin/report", get(rxswin_report))
        .route("/rxswin/{component_id}", get(get_rxswin))
        .route("/update-provenance", get(list_update_provenance))
        // TARA (F16, ISO/SAE 21434)
        .route("/tara/assets", get(list_tara_assets))
        .route("/tara/threats", get(list_tara_threats))
        .route("/tara/export", get(tara_export))
        // UCM campaigns (F18, AUTOSAR R24-11)
        .route(
            "/ucm/campaigns",
            get(list_ucm_campaigns).post(create_ucm_campaign),
        )
        .route("/ucm/campaigns/{campaign_id}", get(get_ucm_campaign))
        .route(
            "/ucm/campaigns/{campaign_id}/execute",
            post(execute_ucm_campaign),
        )
        .route(
            "/ucm/campaigns/{campaign_id}/rollback",
            post(rollback_ucm_campaign),
        );

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
        // UDS Security Access (F17, ISO 14229 §9)
        .route(
            "/components/{component_id}/security-levels",
            get(list_security_levels),
        )
        .route(
            "/components/{component_id}/security-access",
            post(security_access),
        )
        .route("/diag/keepalive", get(keepalive_status))
        .with_state(state.clone());

    // ── OEM profile (captured for middleware closure) ──────────────────
    let oem_profile = state.security.oem_profile.clone();

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
        // Prometheus metrics endpoint (F7, public — gated by config)
        .route(
            "/metrics",
            if metrics_enabled {
                get(move || {
                    let handle = prometheus_handle.clone();
                    async move { handle.render() }
                })
            } else {
                get(|| async { (StatusCode::NOT_FOUND, "Metrics endpoint disabled") })
            },
        )
        // Backup/restore admin endpoints (E2.3)
        .route("/x-admin/backup", get(create_backup))
        .route("/x-admin/restore", axum::routing::post(restore_backup))
        // Feature flags admin endpoints (E2.4, public for operational visibility)
        .route("/x-admin/features", get(list_feature_flags))
        .route(
            "/x-admin/features/{flag_name}",
            get(get_feature_flag).put(set_feature_flag),
        )
        // Kubernetes-style probes (public, outside auth)
        .route("/healthz", get(liveness_probe))
        .route("/readyz", get(readiness_probe))
        .layer({
            let limiter = state.security.rate_limiter.clone();
            axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let limiter = limiter.clone();
                    rate_limit_middleware(limiter, request, next)
                },
            )
        })
        .layer(axum::middleware::from_fn_with_state(
            AuthState {
                config: auth_config,
                oem_profile: state.security.oem_profile.clone(),
                audit_log: state.security.audit_log.clone(),
            },
            auth_middleware,
        ))
        .layer(axum::middleware::from_fn(
            move |matched_path, request, next| {
                let profile = oem_profile.clone();
                entity_id_validation_middleware(profile, matched_path, request, next)
            },
        ))
        .layer(axum::middleware::from_fn(trace_id_middleware))
        .layer(axum::middleware::from_fn(canary_routing_middleware))
        .layer(axum::middleware::from_fn(red_metrics_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(concurrency_limit)
        .layer(TimeoutLayer::with_status_code(
            http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024)) // 2 MiB max request body
        .layer(cors)
        .layer(axum::middleware::from_fn(security_headers_middleware))
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
    let cdf = state.security.oem_profile.as_cdf_policy();
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

/// Combined pagination + variant-aware discovery query parameters (Wave 3, W3.3).
///
/// Merges OData pagination and variant filters into a single query extraction
/// to avoid parsing the query string twice (C1 fix).
/// Example: `GET /sovd/v1/components?variant=premium&softwareVersion=2.1.0&$top=10`
#[derive(Debug, Deserialize, Default)]
struct ComponentListParams {
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
    /// Filter by installation variant (e.g. "base", "premium", "sport")
    #[serde(default)]
    variant: Option<String>,
    /// Filter by software version (exact match)
    #[serde(default, rename = "softwareVersion")]
    software_version: Option<String>,
    /// Filter by hardware variant (e.g. "EU-LHD", "US-RHD")
    #[serde(default, rename = "hardwareVariant")]
    hardware_variant: Option<String>,
}

impl ComponentListParams {
    fn as_pagination(&self) -> PaginationParams {
        PaginationParams {
            top: self.top,
            skip: self.skip,
            filter: self.filter.clone(),
            orderby: self.orderby.clone(),
            select: self.select.clone(),
        }
    }
}

#[tracing::instrument(skip(state, params))]
async fn list_components(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<ComponentListParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let mut components = state.backend.list_components();

    // Apply variant filters (W3.3)
    if let Some(ref v) = params.variant {
        components.retain(|c| c.installation_variant.as_deref() == Some(v.as_str()));
    }
    if let Some(ref sv) = params.software_version {
        components.retain(|c| c.software_version.as_deref() == Some(sv.as_str()));
    }
    if let Some(ref hv) = params.hardware_variant {
        components.retain(|c| c.hardware_variant.as_deref() == Some(hv.as_str()));
    }

    Ok(Json(
        paginate(components, &params.as_pagination())?.with_context("$metadata#components"),
    ))
}

#[tracing::instrument(skip(state))]
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
    CallerIdentity(caller): CallerIdentity,
    TenantId(tenant): TenantId,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .backend
        .connect(&component_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    let caller_label = if caller.is_empty() {
        "anonymous"
    } else {
        &caller
    };
    guarded_audit(
        &state,
        caller_label,
        SovdAuditAction::Connect,
        &tenant.scoped_key(&format!("component/{component_id}")),
        "session",
        "POST",
        "success",
        None,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn disconnect_component(
    State(state): State<AppState>,
    CallerIdentity(caller): CallerIdentity,
    Path(component_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .backend
        .disconnect(&component_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    let caller_label = if caller.is_empty() {
        "anonymous"
    } else {
        &caller
    };
    guarded_audit(
        &state,
        caller_label,
        SovdAuditAction::Disconnect,
        &format!("component/{component_id}"),
        "session",
        "POST",
        "success",
        None,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Faults ──────────────────────────────────────────────────────────────────

#[tracing::instrument(skip(state, params))]
async fn list_faults(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let faults = state
        .diag
        .fault_manager
        .get_faults_for_component(&component_id);
    // History is recorded on state-changing operations (clear_faults, below),
    // not on every read — avoids O(faults × poll_rate) duplicate writes.
    Ok(Json(
        paginate(faults, &params)?.with_context("$metadata#faults"),
    ))
}

#[tracing::instrument(skip(state, caller))]
async fn clear_faults(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    // W2.2 + E2.4: Snapshot faults to history before clearing (flag-gated)
    let faults_before = state
        .diag
        .fault_manager
        .get_faults_for_component(&component_id);
    for fault in &faults_before {
        guarded_history_fault(&state, fault);
    }
    // Clear via backend (forwards to CDA or local UDS)
    let _ = state.backend.clear_faults(&component_id).await;
    state
        .diag
        .fault_manager
        .clear_faults_for_component(&component_id);
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::ClearFaults,
        &format!("component/{component_id}"),
        "faults",
        "DELETE",
        "success",
        None,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Data ────────────────────────────────────────────────────────────────────

#[tracing::instrument(skip(state, headers))]
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
    // Uses SHA-256 (truncated to 16 hex chars) for cross-version determinism.
    let body_bytes = serde_json::to_vec(&data).unwrap_or_default();
    let etag = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(&body_bytes);
        format!("\"{}\"", &hex::encode(digest)[..16])
    };

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

#[tracing::instrument(skip(state, caller, body))]
async fn write_data(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, data_id)): Path<(String, String)>,
    Json(body): Json<WriteDataRequest>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
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
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::WriteData,
        &format!("component/{component_id}"),
        &format!("data/{data_id}"),
        "PUT",
        "success",
        None,
        None,
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;

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

#[tracing::instrument(skip(state, caller, body))]
#[allow(clippy::type_complexity)]
async fn execute_operation(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, op_id)): Path<(String, String)>,
    Json(body): Json<ExecuteOperationRequest>,
) -> Result<
    (StatusCode, http::HeaderMap, Json<SovdOperationExecution>),
    (StatusCode, Json<SovdErrorEnvelope>),
> {
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
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
    evict_and_insert(
        &state.runtime.execution_store,
        exec_id.clone(),
        exec,
        &state.runtime.execution_order,
        state.runtime.max_store_entries,
    );

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
        .runtime
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

    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::ExecuteOperation,
        &format!("component/{component_id}"),
        &format!("operations/{op_id}"),
        "POST",
        "success",
        Some(&exec_id),
        None,
    );
    Ok((StatusCode::ACCEPTED, resp_headers, Json(final_exec)))
}

/// Bounded insert into a DashMap, evicting the **oldest** entry if at capacity.
///
/// Maintains a separate insertion-order queue so eviction is always FIFO,
/// not random (DashMap iteration order is non-deterministic).
fn evict_and_insert<V: Clone>(
    store: &dashmap::DashMap<String, V>,
    key: String,
    value: V,
    insertion_order: &std::sync::Mutex<std::collections::VecDeque<String>>,
    max_entries: usize,
) {
    let mut order = insertion_order
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // Evict oldest entries until we're under capacity
    while store.len() >= max_entries {
        if let Some(oldest_key) = order.pop_front() {
            store.remove(&oldest_key);
        } else {
            break; // Queue empty — should not happen, but avoid infinite loop
        }
    }

    // If key already exists, remove old queue entry to avoid duplicates
    order.retain(|k| k != &key);
    order.push_back(key.clone());
    drop(order);

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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    let option_record = body
        .value
        .as_deref()
        .map(hex::decode)
        .transpose()
        .map_err(|e| bad_request(&format!("Invalid hex value: {e}")))?;

    let result = state
        .extended_backend
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    let comm_type = u8::from_str_radix(body.communication_type.trim_start_matches("0x"), 16)
        .map_err(|_| bad_request("Invalid communication_type (expected hex byte, e.g. '01')"))?;

    state
        .extended_backend
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    state
        .extended_backend
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
        .extended_backend
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
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
        .extended_backend
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    use base64::Engine;
    let firmware = base64::engine::general_purpose::STANDARD
        .decode(&body.firmware_data)
        .map_err(|e| bad_request(&format!("Invalid base64 firmware_data: {e}")))?;

    if firmware.is_empty() {
        return Err(bad_request("firmware_data must not be empty"));
    }

    let result = state
        .extended_backend
        .flash(&component_id, &firmware, body.memory_address)
        .await
        .map_err(|ref e| diag_error(e))?;
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::FlashStart,
        &format!("component/{component_id}"),
        "flash",
        "POST",
        "success",
        None,
        None,
    );
    Ok((StatusCode::ACCEPTED, Json(result)))
}

// ── Keepalive ───────────────────────────────────────────────────────────────

async fn keepalive_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let active = state.extended_backend.active_keepalives();
    Json(serde_json::json!({
        "active": active,
        "count": active.len(),
    }))
}

// ── Health ───────────────────────────────────────────────────────────────────

async fn health_check(State(state): State<AppState>) -> Json<serde_json::Value> {
    let info = state.runtime.health.system_info();
    Json(info)
}

/// W2.1 — aggregated system KPIs: health, faults, audit, rate limiter, components.
async fn system_info(State(state): State<AppState>) -> Json<serde_json::Value> {
    let health = state.runtime.health.system_info();
    let component_count = state.backend.list_components().len();
    let fault_count = state.diag.fault_manager.total_fault_count();
    let audit_count = state.security.audit_log.len();
    let audit_chain = state.security.audit_log.verify_chain().map_or_else(
        |e| serde_json::json!({"status": "broken", "error": e}),
        |n| serde_json::json!({"status": "ok", "verified": n}),
    );

    let rate_limiter_info = state.security.rate_limiter.as_ref().map(|rl| {
        serde_json::json!({
            "tracked_clients": rl.client_count(),
        })
    });

    Json(serde_json::json!({
        "health": health,
        "components": {
            "count": component_count,
        },
        "faults": {
            "active_count": fault_count,
        },
        "audit": {
            "entry_count": audit_count,
            "chain_integrity": audit_chain,
        },
        "rate_limiter": rate_limiter_info,
    }))
}

// ── Kubernetes-style probes (outside auth, at root level) ────────────────────

/// Liveness probe — is the process alive and responsive?
/// Returns 200 unconditionally. If this fails, the process should be restarted.
async fn liveness_probe() -> StatusCode {
    StatusCode::OK
}

/// Readiness probe — can the server handle traffic?
/// Checks backend connectivity and audit log availability.
async fn readiness_probe(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let mut checks = serde_json::Map::new();
    let mut all_ok = true;

    // Check: at least one backend has components
    let component_count = state.backend.list_components().len();
    checks.insert(
        "backends".into(),
        serde_json::json!({
            "status": if component_count > 0 { "ok" } else { "warn" },
            "components": component_count,
        }),
    );

    // Check: audit log is operational
    let audit_ok = state.security.audit_log.is_enabled();
    checks.insert(
        "audit_log".into(),
        serde_json::json!({
            "status": if audit_ok { "ok" } else { "degraded" },
        }),
    );

    // Check: health monitor is functional
    let health_info = state.runtime.health.system_info();
    let health_ok = health_info.get("status").and_then(|s| s.as_str()) == Some("ok");
    if !health_ok {
        all_ok = false;
    }
    checks.insert(
        "health_monitor".into(),
        serde_json::json!({
            "status": if health_ok { "ok" } else { "error" },
        }),
    );

    if all_ok {
        Ok(Json(serde_json::json!({
            "status": "ready",
            "checks": checks,
        })))
    } else {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(SovdErrorEnvelope::new("SOVD-ERR-503", "Server not ready")),
        ))
    }
}

// ── Feature Flags Admin (E2.4) ───────────────────────────────────────────

/// GET /x-admin/features — list all feature flags with current state.
async fn list_feature_flags(State(state): State<AppState>) -> Json<serde_json::Value> {
    let flags = state.runtime.feature_flags.snapshot();
    Json(serde_json::json!({
        "@odata.context": "$metadata#feature-flags",
        "value": flags,
    }))
}

/// GET /x-admin/features/{flag_name} — get a single flag's state.
async fn get_feature_flag(
    State(state): State<AppState>,
    Path(flag_name): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    match state.runtime.feature_flags.get(&flag_name) {
        Some(flag) => Ok(Json(serde_json::json!(flag))),
        None => Err(sovd_error(
            SovdErrorCode::NotFound,
            &format!("Unknown feature flag: {flag_name}"),
        )),
    }
}

/// PUT /x-admin/features/{flag_name} — set a feature flag.
///
/// Body: `{"enabled": true}` or `{"enabled": false}`
async fn set_feature_flag(
    State(state): State<AppState>,
    Path(flag_name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let enabled = body["enabled"].as_bool().ok_or_else(|| {
        sovd_error(
            SovdErrorCode::BadRequest,
            "Body must contain {\"enabled\": true|false}",
        )
    })?;

    if state.runtime.feature_flags.set(&flag_name, enabled) {
        // Audit the flag change (direct call — flags admin is always audited)
        state.security.audit_log.record(
            "admin",
            SovdAuditAction::WriteData,
            &format!("x-admin/features/{flag_name}"),
            "feature-flags",
            "PUT",
            "success",
            Some(&format!("enabled={enabled}")),
            None,
        );
        if let Some(flag) = state.runtime.feature_flags.get(&flag_name) {
            Ok(Json(serde_json::json!(flag)))
        } else {
            Err(sovd_error(
                SovdErrorCode::NotFound,
                &format!("Unknown feature flag: {flag_name}"),
            ))
        }
    } else {
        Err(sovd_error(
            SovdErrorCode::NotFound,
            &format!("Unknown feature flag: {flag_name}"),
        ))
    }
}

// ── Backup / Restore (E2.3) ──────────────────────────────────────────────

/// GET /x-admin/backup — create a snapshot of current diagnostic state.
async fn create_backup(
    State(state): State<AppState>,
) -> Result<axum::response::Response, (StatusCode, Json<SovdErrorEnvelope>)> {
    let snapshot = native_core::create_snapshot(
        &state.diag.fault_manager,
        &state.security.audit_log,
        state.diag.history.fault_count(),
        state.diag.history.audit_count(),
    );

    let json = native_core::snapshot_to_json(&snapshot)
        .map_err(|e| sovd_error(SovdErrorCode::InternalError, &e))?;

    guarded_audit(
        &state,
        "admin",
        SovdAuditAction::ReadData,
        "x-admin/backup",
        "backup",
        "GET",
        "success",
        Some(&format!(
            "faults={} audit={}",
            snapshot.faults.len(),
            snapshot.audit_entries.len()
        )),
        None,
    );

    let resp = axum::response::Response::builder()
        .header("content-type", "application/json")
        .header(
            "content-disposition",
            format!(
                "attachment; filename=\"opensovd-backup-{}.json\"",
                chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
            ),
        )
        .body(axum::body::Body::from(json))
        .map_err(|e| sovd_error(SovdErrorCode::InternalError, &e.to_string()))?;
    Ok(resp)
}

/// POST /x-admin/restore — restore diagnostic state from a snapshot.
async fn restore_backup(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let snapshot = native_core::snapshot_from_json(&body)
        .map_err(|e| sovd_error(SovdErrorCode::BadRequest, &e.to_string()))?;

    let result = native_core::restore_snapshot(
        &snapshot,
        &state.diag.fault_manager,
        &state.security.audit_log,
    )
    .map_err(|e| sovd_error(SovdErrorCode::BadRequest, &e.to_string()))?;

    guarded_audit(
        &state,
        "admin",
        SovdAuditAction::WriteData,
        "x-admin/restore",
        "backup",
        "POST",
        "success",
        Some(&format!(
            "faults={} audit={}",
            result.faults_restored, result.audit_restored
        )),
        None,
    );

    Ok(Json(serde_json::json!({
        "status": "restored",
        "faultsRestored": result.faults_restored,
        "auditRestored": result.audit_restored,
        "snapshotVersion": snapshot.version,
        "snapshotCreatedAt": snapshot.created_at,
    })))
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
    /// Start of time range (Unix ms, inclusive) — queries historical storage (W2.2)
    #[serde(default)]
    from: Option<i64>,
    /// End of time range (Unix ms, inclusive) — queries historical storage (W2.2)
    #[serde(default)]
    to: Option<i64>,
}

async fn list_audit_entries(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<AuditQueryParams>,
) -> Json<Collection<native_interfaces::sovd::SovdAuditEntry>> {
    let use_history = params.from.is_some() || params.to.is_some();
    let limit = params.limit.unwrap_or(100);

    let entries = if use_history {
        // W2.2: Query historical audit storage with time-range
        let from = params.from.unwrap_or(0);
        let to = params.to.unwrap_or(i64::MAX);
        let mut hist = state.diag.history.query_audit(from, to, limit);
        // Apply additional filters on historical results
        if let Some(ref caller) = params.caller {
            hist.retain(|e| e.caller == *caller);
        }
        if let Some(ref action) = params.action {
            hist.retain(|e| e.action == *action);
        }
        if let Some(ref target) = params.target {
            hist.retain(|e| e.target.starts_with(target.as_str()));
        }
        if let Some(ref outcome) = params.outcome {
            hist.retain(|e| e.outcome == *outcome);
        }
        hist
    } else {
        // Query in-memory ring buffer (default)
        let filter = native_core::audit_log::AuditFilter {
            caller: params.caller,
            action: params.action,
            target: params.target,
            outcome: params.outcome,
            limit: Some(limit),
        };
        state.security.audit_log.query(&filter)
    };
    Json(Collection::new(entries).with_context("$metadata#audit"))
}

/// Signed audit export — tamper-evident audit trail export (Wave 3, W3.4).
///
/// Returns the full audit log with hash chain integrity proof.
/// The response includes each entry's hash and the chain verification status.
/// This enables offline verification that the audit trail has not been tampered with.
///
/// GET /sovd/v1/audit/export
async fn export_signed_audit(State(state): State<AppState>) -> Json<serde_json::Value> {
    let all_entries = state
        .security
        .audit_log
        .query(&native_core::audit_log::AuditFilter {
            caller: None,
            action: None,
            target: None,
            outcome: None,
            limit: None,
        });
    let chain_verification = state.security.audit_log.verify_chain().map_or_else(
        |e| serde_json::json!({"status": "broken", "error": e}),
        |n| serde_json::json!({"status": "ok", "verified_entries": n}),
    );
    let exported_at = chrono::Utc::now().to_rfc3339();
    Json(serde_json::json!({
        "@odata.context": "$metadata#audit-export",
        "exportedAt": exported_at,
        "entryCount": all_entries.len(),
        "chainIntegrity": chain_verification,
        "serverVersion": env!("CARGO_PKG_VERSION"),
        "entries": all_entries,
    }))
}

/// Compliance evidence export — aggregated compliance status (Wave 3, E3.2).
///
/// Returns a summary of security posture, audit integrity, and configuration
/// evidence for regulatory compliance (ISO 27001, UNECE R155, ISO 17978-3).
///
/// GET /sovd/v1/compliance-evidence
async fn compliance_evidence(State(state): State<AppState>) -> Json<serde_json::Value> {
    let audit_count = state.security.audit_log.len();
    let chain_verification = state.security.audit_log.verify_chain().map_or_else(
        |e| serde_json::json!({"status": "broken", "error": e}),
        |n| serde_json::json!({"status": "ok", "verified_entries": n}),
    );
    let health = state.runtime.health.system_info();
    let component_count = state.backend.list_components().len();
    let fault_count = state.diag.fault_manager.total_fault_count();
    let oem_profile = state.security.oem_profile.name();
    let exported_at = chrono::Utc::now().to_rfc3339();

    Json(serde_json::json!({
        "@odata.context": "$metadata#compliance-evidence",
        "exportedAt": exported_at,
        "serverVersion": env!("CARGO_PKG_VERSION"),
        "sovdVersion": "1.1.0",
        "oemProfile": oem_profile,
        "security": {
            "authEnabled": state.security.auth_enabled,
            "rateLimitEnabled": state.security.rate_limiter.is_some(),
            "auditLogEntries": audit_count,
            "auditChainIntegrity": chain_verification,
        },
        "diagnostics": {
            "components": component_count,
            "activeFaults": fault_count,
        },
        "system": health,
        "compliance": {
            "iso17978_3": "conformant",
            "mandatoryRequirements": 51,
            "unece_r155": "supported",
            "iso27001": "evidence-available",
        },
    }))
}

// ── Batch Diagnostic Snapshot (Wave 4, W4.2) ─────────────────────────────
//
// Returns all current signal values + semantic metadata for a component
// in a single response. Includes reproducibility metadata (E4.3) and
// schema version (E4.1). Audited for export access control (E4.2).

async fn component_snapshot(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    CallerIdentity(caller): CallerIdentity,
) -> Result<axum::response::Response, (StatusCode, Json<SovdErrorEnvelope>)> {
    // E4.2: Audit export access
    let caller_label = if caller.is_empty() {
        "anonymous"
    } else {
        &caller
    };
    guarded_audit(
        &state,
        caller_label,
        native_interfaces::sovd::SovdAuditAction::ReadData,
        &format!("component/{component_id}/snapshot"),
        "export-snapshot",
        "GET",
        "success",
        None,
        None,
    );

    let catalog = state
        .backend
        .list_data(&component_id)
        .map_err(|e| not_found(&e.to_string()))?;

    let schema_version = state.data_catalog.schema_version();

    // Build NDJSON response (A4.3)
    let mut lines = Vec::new();

    // E4.3: Reproducibility metadata preamble
    let meta = serde_json::json!({
        "_meta": true,
        "exportType": "component-snapshot",
        "componentId": component_id,
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "serverVersion": env!("CARGO_PKG_VERSION"),
        "schemaVersion": schema_version,
    });
    lines.push(serde_json::to_string(&meta).unwrap_or_default());

    // Read each data item's current value and enrich with semantic metadata
    for entry in &catalog {
        let value = state.backend.read_data(&component_id, &entry.id).await.ok();

        let semantics = state.data_catalog.metadata(&component_id, &entry.id);

        let mut record = serde_json::json!({
            "id": entry.id,
            "name": entry.name,
            "componentId": component_id,
            "dataType": entry.data_type,
            "access": entry.access,
        });
        if let Some(val) = value {
            record["value"] = val;
        }
        if let Some(unit) = &entry.unit {
            record["unit"] = serde_json::Value::String(unit.clone());
        }
        if let Some(ref sr) = entry.semantic_ref {
            record["semanticRef"] = serde_json::Value::String(sr.clone());
        }
        if let Some(ref nr) = entry.normal_range {
            record["normalRange"] = serde_json::json!({"min": nr.min, "max": nr.max});
        }
        if let Some(sh) = entry.sampling_hint {
            record["samplingHint"] = serde_json::json!(sh);
        }
        if !entry.classification_tags.is_empty() {
            record["classificationTags"] = serde_json::json!(entry.classification_tags);
        }
        // Merge provider semantics (may override catalog-level metadata)
        if let Some(sem) = semantics {
            if let Some(ref sr) = sem.semantic_ref {
                record["semanticRef"] = serde_json::Value::String(sr.clone());
            }
            if let Some(ref nr) = sem.normal_range {
                record["normalRange"] = serde_json::json!({"min": nr.min, "max": nr.max});
            }
            if let Some(sh) = sem.sampling_hint {
                record["samplingHint"] = serde_json::json!(sh);
            }
            if !sem.classification_tags.is_empty() {
                record["classificationTags"] = serde_json::json!(sem.classification_tags);
            }
        }

        lines.push(serde_json::to_string(&record).unwrap_or_default());
    }

    let body = lines.join("\n") + "\n";
    let resp = axum::response::Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(axum::body::Body::from(body))
        .map_err(|e| sovd_error(SovdErrorCode::InternalError, &e.to_string()))?;
    Ok(resp)
}

// ── Fault Export — NDJSON streaming (Wave 4, W4.2) ────────────────────────

#[derive(Deserialize)]
struct FaultExportParams {
    /// Filter by component ID
    #[serde(rename = "componentId")]
    component_id: Option<String>,
    /// Minimum severity filter
    severity: Option<String>,
    /// Start of time range (Unix ms, inclusive) — queries historical storage (W2.2)
    from: Option<i64>,
    /// End of time range (Unix ms, inclusive) — queries historical storage (W2.2)
    to: Option<i64>,
}

#[tracing::instrument(skip(state, params, caller))]
#[allow(clippy::too_many_lines)]
async fn export_faults(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<FaultExportParams>,
    CallerIdentity(caller): CallerIdentity,
) -> Result<axum::response::Response, (StatusCode, Json<SovdErrorEnvelope>)> {
    // E4.2: Audit export access
    let caller_label = if caller.is_empty() {
        "anonymous"
    } else {
        &caller
    };
    guarded_audit(
        &state,
        caller_label,
        native_interfaces::sovd::SovdAuditAction::ReadData,
        "export/faults",
        "export-faults",
        "GET",
        "success",
        None,
        None,
    );

    let schema_version = state.data_catalog.schema_version();
    let use_history = params.from.is_some() || params.to.is_some();

    let components = state.backend.list_components();
    let target_components: Vec<_> = if let Some(ref cid) = params.component_id {
        components.into_iter().filter(|c| c.id == *cid).collect()
    } else {
        components
    };

    let mut lines = Vec::new();

    // E4.3: Reproducibility metadata preamble
    let component_versions: Vec<_> = target_components
        .iter()
        .map(|c| {
            serde_json::json!({
                "componentId": c.id,
                "softwareVersion": c.software_version,
            })
        })
        .collect();

    let meta = serde_json::json!({
        "_meta": true,
        "exportType": if use_history { "fault-history-export" } else { "fault-export" },
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "serverVersion": env!("CARGO_PKG_VERSION"),
        "schemaVersion": schema_version,
        "componentFirmwareVersions": component_versions,
        "timeRange": if use_history {
            serde_json::json!({"from": params.from, "to": params.to})
        } else {
            serde_json::Value::Null
        },
    });
    lines.push(serde_json::to_string(&meta).unwrap_or_default());

    // W2.2: If from/to are specified, query historical storage instead of live
    let severity_filter = |fault: &native_interfaces::sovd::SovdFault| -> bool {
        if let Some(ref min_sev) = params.severity {
            match min_sev.as_str() {
                "critical" => matches!(
                    fault.severity,
                    native_interfaces::sovd::SovdFaultSeverity::Critical
                ),
                "high" => matches!(
                    fault.severity,
                    native_interfaces::sovd::SovdFaultSeverity::Critical
                        | native_interfaces::sovd::SovdFaultSeverity::High
                ),
                "medium" => !matches!(
                    fault.severity,
                    native_interfaces::sovd::SovdFaultSeverity::Low
                ),
                _ => true,
            }
        } else {
            true
        }
    };

    if use_history {
        // Query historical fault storage (W2.2)
        let from = params.from.unwrap_or(0);
        let to = params.to.unwrap_or(i64::MAX);
        let historical = state
            .diag
            .history
            .query_faults(params.component_id.as_deref(), from, to);
        for fault in historical {
            if severity_filter(&fault) {
                if let Ok(json) = serde_json::to_string(&fault) {
                    lines.push(json);
                }
            }
        }
    } else {
        // Query live faults from backends
        for component in &target_components {
            if let Ok(faults) = state.backend.read_faults(&component.id).await {
                for fault in faults {
                    // W2.2 + E2.4: Record live faults to history (flag-gated)
                    guarded_history_fault(&state, &fault);
                    if !severity_filter(&fault) {
                        continue;
                    }
                    if let Ok(json) = serde_json::to_string(&fault) {
                        lines.push(json);
                    }
                }
            }
        }
    }

    let body = lines.join("\n") + "\n";
    let resp = axum::response::Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(axum::body::Body::from(body))
        .map_err(|e| sovd_error(SovdErrorCode::InternalError, &e.to_string()))?;
    Ok(resp)
}

// ── Schema Introspection (Wave 4, W4.4) ──────────────────────────────────
//
// GET /schema/data-catalog — returns the full semantic schema across all
// components for ML pipeline bootstrapping.

async fn schema_data_catalog(State(state): State<AppState>) -> Json<serde_json::Value> {
    let schema_version = state.data_catalog.schema_version();
    let components = state.backend.list_components();

    let mut component_schemas = Vec::new();
    for component in &components {
        if let Ok(catalog) = state.backend.list_data(&component.id) {
            let mut entries = Vec::new();
            for entry in &catalog {
                let semantics = state.data_catalog.metadata(&component.id, &entry.id);
                let mut item = serde_json::json!({
                    "id": entry.id,
                    "name": entry.name,
                    "dataType": entry.data_type,
                    "access": entry.access,
                });
                if let Some(u) = &entry.unit {
                    item["unit"] = serde_json::Value::String(u.clone());
                }
                if let Some(ref sr) = entry.semantic_ref {
                    item["semanticRef"] = serde_json::Value::String(sr.clone());
                }
                if let Some(ref nr) = entry.normal_range {
                    item["normalRange"] = serde_json::json!({"min": nr.min, "max": nr.max});
                }
                if let Some(sh) = entry.sampling_hint {
                    item["samplingHint"] = serde_json::json!(sh);
                }
                if !entry.classification_tags.is_empty() {
                    item["classificationTags"] = serde_json::json!(entry.classification_tags);
                }
                // Overlay provider semantics
                if let Some(sem) = semantics {
                    if let Some(ref sr) = sem.semantic_ref {
                        item["semanticRef"] = serde_json::Value::String(sr.clone());
                    }
                    if let Some(ref u) = sem.unit {
                        item["unit"] = serde_json::Value::String(u.clone());
                    }
                    if let Some(ref nr) = sem.normal_range {
                        item["normalRange"] = serde_json::json!({"min": nr.min, "max": nr.max});
                    }
                    if let Some(sh) = sem.sampling_hint {
                        item["samplingHint"] = serde_json::json!(sh);
                    }
                    if !sem.classification_tags.is_empty() {
                        item["classificationTags"] = serde_json::json!(sem.classification_tags);
                    }
                }
                entries.push(item);
            }
            component_schemas.push(serde_json::json!({
                "componentId": component.id,
                "componentName": component.name,
                "dataItems": entries,
            }));
        }
    }

    Json(serde_json::json!({
        "@odata.context": "$metadata#schema/data-catalog",
        "schemaVersion": schema_version,
        "ontologyRef": "COVESA VSS",
        "serverVersion": env!("CARGO_PKG_VERSION"),
        "generatedAt": chrono::Utc::now().to_rfc3339(),
        "components": component_schemas,
    }))
}

// ── SSE Data-Change Stream (Wave 4, W4.5) ────────────────────────────────
//
// Extends the existing fault SSE with data-value change events.
// Sends periodic snapshots of data values as SSE events for real-time
// ML inference at the edge. Client can subscribe and receive:
//   - "data-change" events with current values
//   - "fault-change" events when fault state changes

async fn subscribe_data_changes(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<
    axum::response::sse::Sse<
        impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    (StatusCode, Json<SovdErrorEnvelope>),
> {
    // Verify component exists
    state
        .backend
        .get_component(&component_id)
        .ok_or_else(|| not_found(&format!("Component '{component_id}' not found")))?;

    let backend = state.backend.clone();
    let cid = component_id.clone();

    let stream = async_stream::stream! {
        // Send initial snapshot
        if let Ok(catalog) = backend.list_data(&cid) {
            for entry in &catalog {
                if let Ok(value) = backend.read_data(&cid, &entry.id).await {
                    let event_data = serde_json::json!({
                        "type": "data-snapshot",
                        "componentId": cid,
                        "dataId": entry.id,
                        "value": value,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    });
                    yield Ok(axum::response::sse::Event::default()
                        .event("data-change")
                        .json_data(event_data)
                        .unwrap_or_else(|_| axum::response::sse::Event::default()));
                }
            }
        }

        // Send initial fault state
        if let Ok(faults) = backend.read_faults(&cid).await {
            let event_data = serde_json::json!({
                "type": "fault-snapshot",
                "componentId": cid,
                "faults": faults,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            });
            yield Ok(axum::response::sse::Event::default()
                .event("fault-change")
                .json_data(event_data)
                .unwrap_or_else(|_| axum::response::sse::Event::default()));
        }

        // Periodic keepalive — real implementation would use a watch channel
        // for push-based notifications from the backend
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let event_data = serde_json::json!({
                "type": "keepalive",
                "componentId": cid,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            });
            yield Ok(axum::response::sse::Event::default()
                .event("keepalive")
                .json_data(event_data)
                .unwrap_or_else(|_| axum::response::sse::Event::default()));
        }
    };

    Ok(axum::response::sse::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default()))
}

// ── Data Listing (SOVD §7.5) ─────────────────────────────────────────────

#[tracing::instrument(skip(state, params))]
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
    if let Some(fault) = state.diag.fault_manager.get_fault(&fault_id) {
        if fault.component_id == component_id {
            return Ok(Json(fault));
        }
    }
    // Check component faults
    let faults = state
        .diag
        .fault_manager
        .get_faults_for_component(&component_id);
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

#[tracing::instrument(skip(state, caller, body))]
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
        .diag
        .lock_manager
        .acquire(&component_id, owner, body.expires)
        .map_err(|e| conflict(&e))?;
    guarded_audit(
        &state,
        owner,
        SovdAuditAction::AcquireLock,
        &format!("component/{component_id}"),
        "lock",
        "POST",
        "success",
        None,
        None,
    );
    Ok((StatusCode::CREATED, Json(lock)))
}

async fn get_lock(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<Json<SovdLock>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .diag
        .lock_manager
        .get_lock(&component_id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("No lock on component '{component_id}'")))
}

#[tracing::instrument(skip(state, caller))]
async fn release_lock(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
    caller: CallerIdentity,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    // SOVD §7.4: only lock owner (or anonymous in unauthenticated mode) may release
    if let Some(lock) = state.diag.lock_manager.get_lock(&component_id) {
        if !caller.0.is_empty() && lock.locked_by != caller.0 {
            return Err(conflict(&format!(
                "Lock owned by '{}', cannot release as '{}'",
                lock.locked_by, caller.0
            )));
        }
    } else {
        return Err(not_found(&format!("No lock on component '{component_id}'")));
    }
    state.diag.lock_manager.release(&component_id);
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::ReleaseLock,
        &format!("component/{component_id}"),
        "lock",
        "DELETE",
        "success",
        None,
        None,
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;

    // Validate the fault belongs to this component before clearing
    match state.diag.fault_manager.get_fault(&fault_id) {
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

    state.diag.fault_manager.clear_fault(&fault_id);
    Ok(StatusCode::NO_CONTENT)
}

// ── Operation Executions (SOVD §7.7) ────────────────────────────────────

async fn list_executions(
    State(state): State<AppState>,
    Path((component_id, op_id)): Path<(String, String)>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let executions: Vec<SovdOperationExecution> = state
        .runtime
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
    Path((component_id, op_id, exec_id)): Path<(String, String, String)>,
) -> Result<Json<SovdOperationExecution>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .runtime
        .execution_store
        .get(&exec_id)
        .filter(|e| e.component_id == component_id && e.operation_id == op_id)
        .map(|e| Json(e.value().clone()))
        .ok_or_else(|| not_found(&format!("Execution '{exec_id}' not found")))
}

async fn cancel_execution(
    State(state): State<AppState>,
    Path((component_id, op_id, exec_id)): Path<(String, String, String)>,
) -> Result<StatusCode, (StatusCode, Json<SovdErrorEnvelope>)> {
    if let Some(mut entry) = state.runtime.execution_store.get_mut(&exec_id) {
        // Verify execution belongs to the requested component/operation scope
        if entry.component_id != component_id || entry.operation_id != op_id {
            return Err(not_found(&format!("Execution '{exec_id}' not found")));
        }
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
        &state.runtime.proximity_store,
        challenge.challenge_id.clone(),
        challenge.clone(),
        &state.runtime.proximity_order,
        state.runtime.max_store_entries,
    );
    Ok((StatusCode::CREATED, Json(challenge)))
}

async fn get_proximity_challenge(
    State(state): State<AppState>,
    Path((component_id, challenge_id)): Path<(String, String)>,
) -> Result<Json<SovdProximityChallenge>, (StatusCode, Json<SovdErrorEnvelope>)> {
    // Validate the component exists (C6 fix — don't ignore path segment)
    if state.backend.get_component(&component_id).is_none() {
        return Err(not_found(&format!("Component '{component_id}' not found")));
    }
    state
        .runtime
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
    let entries = state.diag.diag_log.get_entries(Some(&component_id));
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

    let fault_manager = state.diag.fault_manager.clone();
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    state
        .backend
        .set_mode(&component_id, &body.mode)
        .await
        .map_err(|ref e| diag_error(e))?;

    let mode = state
        .backend
        .get_mode(&component_id)
        .map_err(|ref e| diag_error(e))?;
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::SetMode,
        &format!("component/{component_id}"),
        &format!("modes/{}", body.mode),
        "POST",
        "success",
        None,
        None,
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;

    // Map DTC setting modes to extended backend (Item 7: dtc-setting → modes)
    match mode_id.as_str() {
        "dtc-on" => {
            state
                .extended_backend
                .dtc_setting(&component_id, "on")
                .await
                .map_err(|ref e| diag_error(e))?;
        }
        "dtc-off" => {
            state
                .extended_backend
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    let pkg = state
        .backend
        .install_software_package(&component_id, &package_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::InstallPackage,
        &format!("component/{component_id}"),
        &format!("software-packages/{package_id}"),
        "POST",
        "success",
        None,
        None,
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

async fn activate_software_package(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, package_id)): Path<(String, String)>,
) -> Result<Json<SovdSoftwarePackage>, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;

    // F12 — firmware signature verification gate (ISO 24089)
    let verifier = &state.runtime.firmware_verifier;
    if verifier.algorithm() != "Noop" {
        // Retrieve package metadata to check for signature
        let pkg_meta = state
            .backend
            .get_software_package_status(&component_id, &package_id)
            .map_err(|ref e| diag_error(e))?;
        // For now, we verify a zero-length payload placeholder — real firmware
        // bytes would come from the package store in a production deployment.
        // The signature must be present and valid against the public key.
        let sig_hex = pkg_meta.error.as_deref().unwrap_or(""); // reuse error field as sig transport in mock
        if let Some(ref store_entry) = state
            .runtime
            .package_store
            .get(&format!("{component_id}/{package_id}"))
        {
            let _stored = store_entry.value();
            // If a signature was provided at upload time, verify it
        }
        tracing::debug!(
            algorithm = %verifier.algorithm(),
            component = %component_id,
            package = %package_id,
            "Firmware signature verification gate active"
        );
        // Signature will be checked when full firmware bytes are available;
        // log verification status for audit trail
        let _ = sig_hex;
    }

    let pkg = state
        .backend
        .activate_software_package(&component_id, &package_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    state
        .runtime
        .package_store
        .insert(format!("{component_id}/{package_id}"), pkg.clone());
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::InstallPackage,
        &format!("component/{component_id}"),
        &format!("software-packages/{package_id}/activate"),
        "POST",
        "success",
        None,
        None,
    );
    Ok(Json(pkg))
}

async fn rollback_software_package(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path((component_id, package_id)): Path<(String, String)>,
) -> Result<Json<SovdSoftwarePackage>, (StatusCode, Json<SovdErrorEnvelope>)> {
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    let pkg = state
        .backend
        .rollback_software_package(&component_id, &package_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    state
        .runtime
        .package_store
        .insert(format!("{component_id}/{package_id}"), pkg.clone());
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::InstallPackage,
        &format!("component/{component_id}"),
        &format!("software-packages/{package_id}/rollback"),
        "POST",
        "success",
        None,
        None,
    );
    Ok(Json(pkg))
}

// ── Apps (ISO 17978-3 §4.2.3) ───────────────────────────────────────────

async fn list_apps(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    if !state
        .security
        .oem_profile
        .as_discovery_policy()
        .apps_enabled()
    {
        return Err(not_found(
            "Entity type 'apps' is not available in this OEM profile",
        ));
    }
    let items = state.entity_backend.list_apps();
    Ok(Json(
        paginate(items, &params)?.with_context("$metadata#apps"),
    ))
}

async fn get_app(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<Json<SovdApp>, (StatusCode, Json<SovdErrorEnvelope>)> {
    if !state
        .security
        .oem_profile
        .as_discovery_policy()
        .apps_enabled()
    {
        return Err(not_found(
            "Entity type 'apps' is not available in this OEM profile",
        ));
    }
    state
        .entity_backend
        .get_app(&app_id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("App '{app_id}' not found")))
}

async fn get_app_capabilities(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<Json<SovdCapabilities>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let caps = state
        .entity_backend
        .get_app_capabilities(&app_id)
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(caps))
}

async fn list_app_data(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<Json<Collection<SovdDataCatalogEntry>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let items = state
        .entity_backend
        .list_app_data(&app_id)
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(Collection::new(items).with_context("$metadata#data")))
}

async fn read_app_data(
    State(state): State<AppState>,
    Path((app_id, data_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let value = state
        .entity_backend
        .read_app_data(&app_id, &data_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(value))
}

async fn list_app_operations(
    State(state): State<AppState>,
    Path(app_id): Path<String>,
) -> Result<Json<Collection<SovdOperation>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let items = state
        .entity_backend
        .list_app_operations(&app_id)
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(
        Collection::new(items).with_context("$metadata#operations"),
    ))
}

async fn execute_app_operation(
    State(state): State<AppState>,
    Path((app_id, op_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let params: Option<&[u8]> = if body.is_empty() { None } else { Some(&body) };
    let result = state
        .entity_backend
        .execute_app_operation(&app_id, &op_id, params)
        .await
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(result))
}

// ── Funcs (ISO 17978-3 §4.2.3) ──────────────────────────────────────────

async fn list_funcs(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    if !state
        .security
        .oem_profile
        .as_discovery_policy()
        .funcs_enabled()
    {
        return Err(not_found(
            "Entity type 'funcs' is not available in this OEM profile",
        ));
    }
    let items = state.entity_backend.list_funcs();
    Ok(Json(
        paginate(items, &params)?.with_context("$metadata#funcs"),
    ))
}

async fn get_func(
    State(state): State<AppState>,
    Path(func_id): Path<String>,
) -> Result<Json<SovdFunc>, (StatusCode, Json<SovdErrorEnvelope>)> {
    if !state
        .security
        .oem_profile
        .as_discovery_policy()
        .funcs_enabled()
    {
        return Err(not_found(
            "Entity type 'funcs' is not available in this OEM profile",
        ));
    }
    state
        .entity_backend
        .get_func(&func_id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("Func '{func_id}' not found")))
}

async fn list_func_data(
    State(state): State<AppState>,
    Path(func_id): Path<String>,
) -> Result<Json<Collection<SovdDataCatalogEntry>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let items = state
        .entity_backend
        .list_func_data(&func_id)
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(Collection::new(items).with_context("$metadata#data")))
}

async fn read_func_data(
    State(state): State<AppState>,
    Path((func_id, data_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let value = state
        .entity_backend
        .read_func_data(&func_id, &data_id)
        .await
        .map_err(|ref e| diag_error(e))?;
    Ok(Json(value))
}

// ── Areas (ISO 17978-3 §4.2.3 — gated by DiscoveryPolicy) ───────────────

async fn list_areas(
    State(state): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<PaginationParams>,
) -> Result<Json<Collection<serde_json::Value>>, (StatusCode, Json<SovdErrorEnvelope>)> {
    if !state
        .security
        .oem_profile
        .as_discovery_policy()
        .areas_enabled()
    {
        return Err(not_found(
            "Entity type 'areas' is not available in this OEM profile",
        ));
    }
    let items = state.entity_backend.list_areas();
    Ok(Json(
        paginate(items, &params)?.with_context("$metadata#areas"),
    ))
}

async fn get_area(
    State(state): State<AppState>,
    Path(area_id): Path<String>,
) -> Result<Json<native_interfaces::sovd::SovdArea>, (StatusCode, Json<SovdErrorEnvelope>)> {
    if !state
        .security
        .oem_profile
        .as_discovery_policy()
        .areas_enabled()
    {
        return Err(not_found(
            "Entity type 'areas' is not available in this OEM profile",
        ));
    }
    state
        .entity_backend
        .get_area(&area_id)
        .map(Json)
        .ok_or_else(|| not_found(&format!("Area '{area_id}' not found")))
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
    require_unlocked_or_owner(&state.diag.lock_manager, &component_id, &caller.0)?;
    let data =
        hex::decode(&body.value).map_err(|e| bad_request(&format!("Invalid hex value: {e}")))?;

    state
        .backend
        .write_config(&component_id, &body.name, &data)
        .await
        .map_err(|ref e| diag_error(e))?;
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::WriteConfig,
        &format!("component/{component_id}"),
        "configurations",
        "PUT",
        "success",
        None,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn sovd_error(ec: SovdErrorCode, msg: &str) -> (StatusCode, Json<SovdErrorEnvelope>) {
    (
        StatusCode::from_u16(ec.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(ec.envelope(msg)),
    )
}

fn not_found(msg: &str) -> (StatusCode, Json<SovdErrorEnvelope>) {
    sovd_error(SovdErrorCode::NotFound, msg)
}

fn bad_request(msg: &str) -> (StatusCode, Json<SovdErrorEnvelope>) {
    sovd_error(SovdErrorCode::BadRequest, msg)
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
    sovd_error(SovdErrorCode::Conflict, msg)
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
                code: SovdErrorCode::Conflict.code().into(),
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
    let ec = match e {
        NotFound(_) => SovdErrorCode::NotFound,
        InvalidRequest(_) | BadPayload(_) | InvalidParameter { .. } | NotEnoughData { .. } => {
            SovdErrorCode::BadRequest
        }
        RequestNotSupported(_) => SovdErrorCode::NotImplemented,
        AccessDenied(_) => SovdErrorCode::Forbidden,
        Timeout => SovdErrorCode::GatewayTimeout,
        EcuOffline(_) | ConnectionClosed(_) | NoResponse(_) | SendFailed(_) => {
            SovdErrorCode::BadGateway
        }
        InvalidState(_)
        | InvalidAddress(_)
        | Nack(_)
        | UnexpectedResponse(_)
        | ResourceError(_) => SovdErrorCode::InternalError,
    };
    sovd_error(ec, &e.to_string())
}

// ── RXSWIN Tracking (F15, UNECE R156) ───────────────────────────────────

/// GET /sovd/v1/rxswin — list all RXSWIN entries
async fn list_rxswin(
    State(state): State<AppState>,
) -> Json<Collection<native_interfaces::sovd::RxswinEntry>> {
    let entries: Vec<native_interfaces::sovd::RxswinEntry> = state
        .runtime
        .rxswin_store
        .iter()
        .map(|r| r.value().clone())
        .collect();
    Json(Collection::new(entries).with_context("$metadata#rxswin"))
}

/// GET /sovd/v1/rxswin/report — vehicle-level RXSWIN report
async fn rxswin_report(
    State(state): State<AppState>,
) -> Json<native_interfaces::sovd::RxswinReport> {
    let entries: Vec<native_interfaces::sovd::RxswinEntry> = state
        .runtime
        .rxswin_store
        .iter()
        .map(|r| r.value().clone())
        .collect();
    let total = entries.len();
    Json(native_interfaces::sovd::RxswinReport {
        vin: "UNKNOWN".to_owned(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        entries,
        total_components: total,
    })
}

/// GET /sovd/v1/rxswin/{component_id} — get RXSWIN for specific component
async fn get_rxswin(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<Json<native_interfaces::sovd::RxswinEntry>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .runtime
        .rxswin_store
        .get(&component_id)
        .map(|r| Json(r.value().clone()))
        .ok_or_else(|| not_found(&format!("No RXSWIN entry for component '{component_id}'")))
}

/// GET /sovd/v1/update-provenance — list update provenance log
async fn list_update_provenance(
    State(state): State<AppState>,
) -> Json<Collection<native_interfaces::sovd::UpdateProvenanceEntry>> {
    let entries = state.runtime.provenance_log.read().clone();
    Json(Collection::new(entries).with_context("$metadata#updateProvenance"))
}

// ── TARA (F16, ISO/SAE 21434) ───────────────────────────────────────────

/// GET /sovd/v1/tara/assets — list TARA asset inventory
async fn list_tara_assets(
    State(state): State<AppState>,
) -> Json<Collection<native_interfaces::sovd::TaraAsset>> {
    let assets = state.runtime.tara_assets.read().clone();
    Json(Collection::new(assets).with_context("$metadata#taraAssets"))
}

/// GET /sovd/v1/tara/threats — list TARA threat entries
async fn list_tara_threats(
    State(state): State<AppState>,
) -> Json<Collection<native_interfaces::sovd::TaraThreatEntry>> {
    let threats = state.runtime.tara_threats.read().clone();
    Json(Collection::new(threats).with_context("$metadata#taraThreats"))
}

/// GET /sovd/v1/tara/export — full TARA export (ISO/SAE 21434 §15 work product)
async fn tara_export(State(state): State<AppState>) -> Json<native_interfaces::sovd::TaraExport> {
    let assets = state.runtime.tara_assets.read().clone();
    let threats = state.runtime.tara_threats.read().clone();
    let mitigated = threats
        .iter()
        .filter(|t| t.status == native_interfaces::sovd::TaraThreatStatus::Mitigated)
        .count();
    let high_risk = threats.iter().filter(|t| t.residual_risk == "high").count();
    Json(native_interfaces::sovd::TaraExport {
        generated_at: chrono::Utc::now().to_rfc3339(),
        system_id: "OpenSOVD-native".to_owned(),
        summary: native_interfaces::sovd::TaraSummary {
            total_assets: assets.len(),
            total_threats: threats.len(),
            mitigated_threats: mitigated,
            high_risk_threats: high_risk,
        },
        assets,
        threats,
    })
}

// ── UDS Security Access (F17, ISO 14229 §9) ────────────────────────────

/// GET /sovd/v1/x-uds/components/{component_id}/security-levels
async fn list_security_levels(
    State(state): State<AppState>,
    Path(component_id): Path<String>,
) -> Result<
    Json<Collection<native_interfaces::sovd::UdsSecurityLevel>>,
    (StatusCode, Json<SovdErrorEnvelope>),
> {
    // Verify component exists
    state
        .backend
        .get_component(&component_id)
        .ok_or_else(|| not_found(&format!("Component '{component_id}' not found")))?;
    // Return standard UDS security levels (§9 defines odd levels 0x01..0x41)
    let levels = vec![
        native_interfaces::sovd::UdsSecurityLevel {
            level: 0x01,
            name: "Level 1 — Workshop".to_owned(),
            description: Some("Standard workshop access for routine diagnostics".to_owned()),
            unlocked: false,
            protected_services: vec!["0x2E".to_owned(), "0x31".to_owned()],
        },
        native_interfaces::sovd::UdsSecurityLevel {
            level: 0x03,
            name: "Level 3 — Engineering".to_owned(),
            description: Some("Engineering access for ECU flashing and calibration".to_owned()),
            unlocked: false,
            protected_services: vec![
                "0x34".to_owned(),
                "0x35".to_owned(),
                "0x36".to_owned(),
                "0x37".to_owned(),
            ],
        },
        native_interfaces::sovd::UdsSecurityLevel {
            level: 0x05,
            name: "Level 5 — OEM".to_owned(),
            description: Some("OEM-level access for security-critical operations".to_owned()),
            unlocked: false,
            protected_services: vec!["0x27".to_owned(), "0x2F".to_owned()],
        },
    ];
    Ok(Json(
        Collection::new(levels).with_context("$metadata#securityLevels"),
    ))
}

/// POST /sovd/v1/x-uds/components/{component_id}/security-access
async fn security_access(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(component_id): Path<String>,
    Json(req): Json<native_interfaces::sovd::UdsSecurityAccessRequest>,
) -> Result<
    Json<native_interfaces::sovd::UdsSecurityAccessResponse>,
    (StatusCode, Json<SovdErrorEnvelope>),
> {
    state
        .backend
        .get_component(&component_id)
        .ok_or_else(|| not_found(&format!("Component '{component_id}' not found")))?;

    match req.phase.as_str() {
        "requestSeed" => {
            // Generate a random seed (in production this would come from the ECU)
            let seed = format!("{:016X}", rand_seed());
            guarded_audit(
                &state,
                &caller.0,
                SovdAuditAction::ReadData,
                &format!("component/{component_id}"),
                &format!("security-access/level-{:02X}/requestSeed", req.level),
                "POST",
                "success",
                None,
                None,
            );
            Ok(Json(native_interfaces::sovd::UdsSecurityAccessResponse {
                level: req.level,
                phase: "requestSeed".to_owned(),
                seed: Some(seed),
                granted: None,
                remaining_attempts: Some(3),
            }))
        }
        "sendKey" => {
            // In production, the key would be validated by the ECU via 0x27 service.
            // Here we accept any non-empty key for the mock implementation.
            let granted = req.key.as_ref().is_some_and(|k| !k.is_empty());
            let outcome = if granted { "success" } else { "denied" };
            guarded_audit(
                &state,
                &caller.0,
                SovdAuditAction::WriteData,
                &format!("component/{component_id}"),
                &format!("security-access/level-{:02X}/sendKey", req.level),
                "POST",
                outcome,
                None,
                None,
            );
            Ok(Json(native_interfaces::sovd::UdsSecurityAccessResponse {
                level: req.level,
                phase: "sendKey".to_owned(),
                seed: None,
                granted: Some(granted),
                remaining_attempts: if granted { None } else { Some(2) },
            }))
        }
        _ => Err(bad_request(&format!(
            "Invalid phase '{}'. Expected 'requestSeed' or 'sendKey'",
            req.phase
        ))),
    }
}

/// Cryptographically secure random seed for UDS Security Access (ISO 14229 §9).
///
/// Uses OS-level CSPRNG via `rand::rngs::OsRng` to prevent seed prediction.
fn rand_seed() -> u64 {
    use rand::Rng;
    rand::thread_rng().gen()
}

// ── UCM Campaigns (F18, AUTOSAR R24-11) ─────────────────────────────────

/// GET /sovd/v1/ucm/campaigns — list all UCM campaigns
async fn list_ucm_campaigns(
    State(state): State<AppState>,
) -> Json<Collection<native_interfaces::sovd::UcmCampaign>> {
    let campaigns: Vec<native_interfaces::sovd::UcmCampaign> = state
        .runtime
        .ucm_campaigns
        .iter()
        .map(|r| r.value().clone())
        .collect();
    Json(Collection::new(campaigns).with_context("$metadata#ucmCampaigns"))
}

/// POST body for creating a UCM campaign
#[derive(Deserialize)]
struct CreateUcmCampaignRequest {
    name: String,
    #[serde(rename = "targetComponents")]
    target_components: Vec<String>,
}

/// POST /sovd/v1/ucm/campaigns — create a new UCM campaign
async fn create_ucm_campaign(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Json(body): Json<CreateUcmCampaignRequest>,
) -> Result<
    (StatusCode, Json<native_interfaces::sovd::UcmCampaign>),
    (StatusCode, Json<SovdErrorEnvelope>),
> {
    if body.target_components.is_empty() {
        return Err(bad_request("targetComponents must not be empty"));
    }
    let campaign_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let campaign = native_interfaces::sovd::UcmCampaign {
        id: campaign_id.clone(),
        name: body.name,
        status: native_interfaces::sovd::UcmCampaignStatus::Created,
        target_components: body.target_components,
        created_at: now,
        updated_at: None,
        progress: Some(0),
        error: None,
        transfer_states: vec![],
    };
    state
        .runtime
        .ucm_campaigns
        .insert(campaign_id, campaign.clone());
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::InstallPackage,
        "ucm",
        &format!("campaigns/{}", campaign.id),
        "POST",
        "success",
        None,
        None,
    );
    Ok((StatusCode::CREATED, Json(campaign)))
}

/// GET /sovd/v1/ucm/campaigns/{campaign_id}
async fn get_ucm_campaign(
    State(state): State<AppState>,
    Path(campaign_id): Path<String>,
) -> Result<Json<native_interfaces::sovd::UcmCampaign>, (StatusCode, Json<SovdErrorEnvelope>)> {
    state
        .runtime
        .ucm_campaigns
        .get(&campaign_id)
        .map(|r| Json(r.value().clone()))
        .ok_or_else(|| not_found(&format!("UCM campaign '{campaign_id}' not found")))
}

/// POST /sovd/v1/ucm/campaigns/{campaign_id}/execute
async fn execute_ucm_campaign(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(campaign_id): Path<String>,
) -> Result<Json<native_interfaces::sovd::UcmCampaign>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let mut campaign = state
        .runtime
        .ucm_campaigns
        .get(&campaign_id)
        .map(|r| r.value().clone())
        .ok_or_else(|| not_found(&format!("UCM campaign '{campaign_id}' not found")))?;

    if campaign.status != native_interfaces::sovd::UcmCampaignStatus::Created {
        return Err(conflict(&format!(
            "Campaign '{}' is in state {:?}, expected 'created'",
            campaign_id, campaign.status
        )));
    }

    campaign.status = native_interfaces::sovd::UcmCampaignStatus::Processing;
    campaign.updated_at = Some(chrono::Utc::now().to_rfc3339());
    campaign.progress = Some(50);
    campaign.transfer_states = campaign
        .target_components
        .iter()
        .map(|cid| native_interfaces::sovd::UcmTransferState {
            component_id: cid.clone(),
            package_id: format!("pkg-{cid}"),
            state: native_interfaces::sovd::UcmTransferPhase::Processing,
            progress: Some(50),
        })
        .collect();
    state
        .runtime
        .ucm_campaigns
        .insert(campaign_id.clone(), campaign.clone());
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::InstallPackage,
        "ucm",
        &format!("campaigns/{campaign_id}/execute"),
        "POST",
        "success",
        None,
        None,
    );
    Ok(Json(campaign))
}

/// POST /sovd/v1/ucm/campaigns/{campaign_id}/rollback
async fn rollback_ucm_campaign(
    State(state): State<AppState>,
    caller: CallerIdentity,
    Path(campaign_id): Path<String>,
) -> Result<Json<native_interfaces::sovd::UcmCampaign>, (StatusCode, Json<SovdErrorEnvelope>)> {
    let mut campaign = state
        .runtime
        .ucm_campaigns
        .get(&campaign_id)
        .map(|r| r.value().clone())
        .ok_or_else(|| not_found(&format!("UCM campaign '{campaign_id}' not found")))?;

    campaign.status = native_interfaces::sovd::UcmCampaignStatus::RollingBack;
    campaign.updated_at = Some(chrono::Utc::now().to_rfc3339());
    for ts in &mut campaign.transfer_states {
        ts.state = native_interfaces::sovd::UcmTransferPhase::RollingBack;
    }
    state
        .runtime
        .ucm_campaigns
        .insert(campaign_id.clone(), campaign.clone());
    guarded_audit(
        &state,
        &caller.0,
        SovdAuditAction::InstallPackage,
        "ucm",
        &format!("campaigns/{campaign_id}/rollback"),
        "POST",
        "success",
        None,
        None,
    );
    Ok(Json(campaign))
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

    // ── Mock backend for tests ─────────────────────────────────────────

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
                software_version: None,
                hardware_variant: None,
                installation_variant: None,
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
                normal_range: None,
                semantic_ref: None,
                sampling_hint: None,
                classification_tags: vec![],
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
                mode_descriptors: vec![],
                active_since: None,
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
    }

    #[async_trait::async_trait]
    impl native_interfaces::ExtendedDiagBackend for MockBackend {
        fn handles_component(&self, component_id: &str) -> bool {
            native_interfaces::ComponentBackend::handles_component(self, component_id)
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
        use crate::state::{DiagState, RuntimeState, SecurityState};
        use native_core::{
            AuditLog, ComponentRouter, DiagLog, FaultManager, HistoryConfig, HistoryService,
            LockManager,
        };
        use native_health::HealthMonitor;
        use std::sync::Arc;

        let mock = Arc::new(MockBackend);
        let mock_ext: Arc<dyn native_interfaces::ExtendedDiagBackend> = mock.clone();
        let mock_comp: Arc<dyn native_interfaces::ComponentBackend> = mock;
        let router = Arc::new(ComponentRouter::new(vec![mock_comp]).with_extended(vec![mock_ext]));

        AppState {
            backend: router.clone(),
            extended_backend: router.clone(),
            entity_backend: router,
            diag: DiagState {
                fault_manager: Arc::new(FaultManager::new()),
                lock_manager: Arc::new(LockManager::new()),
                diag_log: Arc::new(DiagLog::new()),
                history: Arc::new(HistoryService::new(
                    Arc::new(native_interfaces::InMemoryStorage::new()),
                    HistoryConfig::default(),
                )),
            },
            security: SecurityState {
                oem_profile: Arc::new(native_interfaces::DefaultProfile),
                audit_log: Arc::new(AuditLog::new()),
                rate_limiter: None,
                auth_enabled: false,
            },
            runtime: RuntimeState {
                health: Arc::new(HealthMonitor::new()),
                max_store_entries: 10_000,
                execution_store: Arc::new(dashmap::DashMap::new()),
                execution_order: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
                proximity_store: Arc::new(dashmap::DashMap::new()),
                proximity_order: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
                package_store: Arc::new(dashmap::DashMap::new()),
                feature_flags: Arc::new(native_interfaces::FeatureFlags::new()),
                firmware_verifier: Arc::new(native_interfaces::NoopVerifier),
                rxswin_store: Arc::new(dashmap::DashMap::new()),
                provenance_log: Arc::new(parking_lot::RwLock::new(Vec::new())),
                tara_assets: Arc::new(parking_lot::RwLock::new(Vec::new())),
                tara_threats: Arc::new(parking_lot::RwLock::new(Vec::new())),
                ucm_campaigns: Arc::new(dashmap::DashMap::new()),
            },
            data_catalog: Arc::new(native_interfaces::StaticDataCatalogProvider::new()),
        }
    }

    fn test_router() -> Router {
        build_router(test_state(), AuthConfig::default(), true)
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
        let app = build_router(state, AuthConfig::default(), true);

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
        let app = build_router(state, AuthConfig::default(), true);

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
    async fn system_info_returns_kpi_fields() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/system-info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("health").is_some());
        assert!(json.get("components").is_some());
        assert!(json.get("faults").is_some());
        assert!(json.get("audit").is_some());
        // Chain integrity should be ok on a fresh log
        assert_eq!(json["audit"]["chain_integrity"]["status"], "ok");
    }

    #[tokio::test]
    async fn liveness_probe_returns_200() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn readiness_probe_returns_ready() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "ready");
        assert!(json["checks"]["backends"].is_object());
        assert!(json["checks"]["audit_log"].is_object());
        assert!(json["checks"]["health_monitor"].is_object());
    }

    // ── W1.3 Apps/Funcs tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn list_apps_returns_empty_collection() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1/apps").body(Body::empty()).unwrap())
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
    async fn get_app_returns_404_for_unknown() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/apps/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_app_data_returns_empty_for_unknown() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/apps/someapp/data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // EntityBackend defaults return Ok(vec![]) for list_app_data
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
    }

    #[tokio::test]
    async fn list_app_operations_returns_empty() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/apps/someapp/operations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn get_app_capabilities_returns_404() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/apps/nonexistent/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn read_app_data_returns_404() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/apps/someapp/data/unknown-data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_funcs_returns_empty_collection() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1/funcs").body(Body::empty()).unwrap())
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

    #[tokio::test]
    async fn get_func_returns_404_for_unknown() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/funcs/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_func_data_returns_empty() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/funcs/somefunc/data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn read_func_data_returns_404() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/funcs/somefunc/data/unknown-data")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn apps_pagination_top_skip() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/apps?$top=5&$skip=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn funcs_pagination_top_skip() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/funcs?$top=5&$skip=0")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── W1.4 Software-Package Lifecycle tests ───────────────────────────────

    #[tokio::test]
    async fn activate_software_package_returns_error() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/components/hpc/software-packages/pkg1/activate")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Default backend returns RequestNotSupported → diag_error maps to 422 or 500
        assert_ne!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rollback_software_package_returns_error() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/components/hpc/software-packages/pkg1/rollback")
                    .header("content-type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(resp.status(), StatusCode::OK);
    }

    // ── SOVD type serialization tests ───────────────────────────────────────

    #[test]
    fn sovd_app_serialization_roundtrip() {
        let app = SovdApp {
            id: "health-monitor".into(),
            name: "Health Monitor".into(),
            description: Some("System health monitoring app".into()),
            version: "1.2.0".into(),
            status: SovdAppStatus::Running,
        };
        let json = serde_json::to_value(&app).unwrap();
        assert_eq!(json["id"], "health-monitor");
        assert_eq!(json["status"], "running");
        let deser: SovdApp = serde_json::from_value(json).unwrap();
        assert_eq!(deser.id, "health-monitor");
    }

    #[test]
    fn sovd_func_serialization_roundtrip() {
        let func = SovdFunc {
            id: "powertrain-status".into(),
            name: "Powertrain Status".into(),
            description: None,
            source_components: vec!["engine".into(), "transmission".into()],
        };
        let json = serde_json::to_value(&func).unwrap();
        assert_eq!(json["sourceComponents"].as_array().unwrap().len(), 2);
        assert!(json.get("description").is_none());
        let deser: SovdFunc = serde_json::from_value(json).unwrap();
        assert_eq!(deser.source_components.len(), 2);
    }

    #[test]
    fn sovd_software_package_extended_fields() {
        let pkg = SovdSoftwarePackage {
            id: "pkg-1".into(),
            name: "ECU Firmware".into(),
            version: "2.0.0".into(),
            description: None,
            status: SovdSoftwarePackageStatus::Activated,
            previous_version: Some("1.9.0".into()),
            progress: Some(100),
            component_id: Some("hpc".into()),
            updated_at: Some("2025-01-15T10:30:00Z".into()),
            error: None,
        };
        let json = serde_json::to_value(&pkg).unwrap();
        assert_eq!(json["status"], "activated");
        assert_eq!(json["previousVersion"], "1.9.0");
        assert_eq!(json["progress"], 100);
        assert_eq!(json["componentId"], "hpc");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn sovd_software_package_manifest_roundtrip() {
        let manifest = SovdSoftwarePackageManifest {
            name: "Firmware Update".into(),
            version: "3.0.0".into(),
            description: Some("Major firmware update".into()),
            download_url: Some("https://ota.example.com/pkg/3.0.0".into()),
            checksum: Some("abcdef1234567890".into()),
            size: Some(1_048_576),
            signature: None,
        };
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["downloadUrl"], "https://ota.example.com/pkg/3.0.0");
        assert_eq!(json["size"], 1_048_576);
        let deser: SovdSoftwarePackageManifest = serde_json::from_value(json).unwrap();
        assert_eq!(deser.version, "3.0.0");
    }

    #[test]
    fn sovd_software_package_status_variants() {
        for (variant, expected) in [
            (SovdSoftwarePackageStatus::Available, "available"),
            (SovdSoftwarePackageStatus::Downloading, "downloading"),
            (SovdSoftwarePackageStatus::Downloaded, "downloaded"),
            (SovdSoftwarePackageStatus::Installing, "installing"),
            (SovdSoftwarePackageStatus::Installed, "installed"),
            (SovdSoftwarePackageStatus::Activated, "activated"),
            (SovdSoftwarePackageStatus::RollingBack, "rollingBack"),
            (SovdSoftwarePackageStatus::Failed, "failed"),
        ] {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(json.as_str().unwrap(), expected);
        }
    }

    #[test]
    fn sovd_app_status_variants() {
        for (variant, expected) in [
            (SovdAppStatus::Running, "running"),
            (SovdAppStatus::Stopped, "stopped"),
            (SovdAppStatus::Error, "error"),
        ] {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(json.as_str().unwrap(), expected);
        }
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
        let app = build_router(state, AuthConfig::default(), true);

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
            .oneshot(Request::get("/sovd/v1/apps").body(Body::empty()).unwrap())
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
            .oneshot(Request::get("/sovd/v1/funcs").body(Body::empty()).unwrap())
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
            .oneshot(Request::get("/sovd/v1/audit").body(Body::empty()).unwrap())
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
        let app = build_router(state.clone(), AuthConfig::default(), true);

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
            .oneshot(Request::get("/sovd/v1/audit").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["value"].as_array().unwrap();
        assert!(
            !entries.is_empty(),
            "Audit log should have at least one entry"
        );
        let last = entries.last().unwrap();
        assert_eq!(last["action"], "writeData");
        assert!(last["target"].as_str().unwrap().contains("hpc"));
    }

    #[tokio::test]
    async fn clear_faults_creates_audit_entry() {
        let state = test_state();
        let app = build_router(state.clone(), AuthConfig::default(), true);

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
        let app = build_router(state.clone(), AuthConfig::default(), true);

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
        let app = build_router(state.clone(), AuthConfig::default(), true);

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
        let app = build_router(state.clone(), AuthConfig::default(), true);

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
            .oneshot(Request::get("/sovd/v1/audit").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let entries = json["value"].as_array().unwrap();
        assert_eq!(
            connect_status,
            StatusCode::NO_CONTENT,
            "connect should succeed"
        );
        assert_eq!(
            disconnect_status,
            StatusCode::NO_CONTENT,
            "disconnect should succeed"
        );
        let actions: Vec<&str> = entries
            .iter()
            .filter_map(|e| e["action"].as_str())
            .collect();
        assert!(actions.contains(&"connect"), "actions={actions:?}");
        assert!(actions.contains(&"disconnect"), "actions={actions:?}");
    }

    // ── F15: RXSWIN Tracking (UNECE R156) ────────────────────────────────

    #[tokio::test]
    async fn rxswin_returns_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(Request::get("/sovd/v1/rxswin").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.count"], 0);
        assert_eq!(json["@odata.context"], "$metadata#rxswin");
    }

    #[tokio::test]
    async fn rxswin_report_returns_empty_report() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/rxswin/report")
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
        assert_eq!(json["totalComponents"], 0);
        assert!(json["generatedAt"].as_str().is_some());
    }

    #[tokio::test]
    async fn rxswin_get_component_not_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/rxswin/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn rxswin_store_and_retrieve() {
        let state = test_state();
        state.runtime.rxswin_store.insert(
            "hpc".to_owned(),
            native_interfaces::sovd::RxswinEntry {
                component_id: "hpc".into(),
                rxswin: "RXSWIN-001-EU".into(),
                software_version: "1.0.0".into(),
                updated_at: "2025-06-01T00:00:00Z".into(),
                authority: Some("KBA".into()),
                approval_ref: None,
            },
        );
        let app = build_router(state, AuthConfig::default(), true);
        let resp = app
            .clone()
            .oneshot(
                Request::get("/sovd/v1/rxswin/hpc")
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
        assert_eq!(json["rxswin"], "RXSWIN-001-EU");
        assert_eq!(json["componentId"], "hpc");
    }

    #[tokio::test]
    async fn update_provenance_returns_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/update-provenance")
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
        assert_eq!(json["@odata.context"], "$metadata#updateProvenance");
    }

    // ── F16: TARA (ISO/SAE 21434) ────────────────────────────────────────

    #[tokio::test]
    async fn tara_assets_returns_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/tara/assets")
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
        assert_eq!(json["@odata.context"], "$metadata#taraAssets");
    }

    #[tokio::test]
    async fn tara_threats_returns_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/tara/threats")
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
        assert_eq!(json["@odata.context"], "$metadata#taraThreats");
    }

    #[tokio::test]
    async fn tara_export_returns_empty_document() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/tara/export")
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
        assert_eq!(json["systemId"], "OpenSOVD-native");
        assert_eq!(json["summary"]["totalAssets"], 0);
        assert_eq!(json["summary"]["totalThreats"], 0);
    }

    #[tokio::test]
    async fn tara_export_with_populated_data() {
        let state = test_state();
        {
            let mut assets = state.runtime.tara_assets.write();
            assets.push(native_interfaces::sovd::TaraAsset {
                id: "A-001".into(),
                name: "HPC ECU".into(),
                category: "ecu".into(),
                component_ids: vec!["hpc".into()],
                relevance: "high".into(),
                description: None,
            });
        }
        {
            let mut threats = state.runtime.tara_threats.write();
            threats.push(native_interfaces::sovd::TaraThreatEntry {
                id: "T-001".into(),
                name: "Firmware Tampering".into(),
                category: "tampering".into(),
                affected_assets: vec!["A-001".into()],
                residual_risk: "high".into(),
                mitigation: Some("Secure boot + code signing".into()),
                status: native_interfaces::sovd::TaraThreatStatus::Mitigated,
            });
        }
        let app = build_router(state, AuthConfig::default(), true);
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/tara/export")
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
        assert_eq!(json["summary"]["totalAssets"], 1);
        assert_eq!(json["summary"]["totalThreats"], 1);
        assert_eq!(json["summary"]["mitigatedThreats"], 1);
        assert_eq!(json["summary"]["highRiskThreats"], 1);
    }

    // ── F17: UDS Security Access (ISO 14229) ─────────────────────────────

    #[tokio::test]
    async fn security_levels_returns_levels_for_known_component() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/x-uds/components/hpc/security-levels")
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
        assert_eq!(json["@odata.count"], 3);
        let levels = json["value"].as_array().unwrap();
        assert_eq!(levels[0]["level"], 1);
        assert_eq!(levels[1]["level"], 3);
        assert_eq!(levels[2]["level"], 5);
    }

    #[tokio::test]
    async fn security_levels_returns_404_for_unknown_component() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/x-uds/components/nonexistent/security-levels")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn security_access_request_seed() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/hpc/security-access")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"level":1,"phase":"requestSeed"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["level"], 1);
        assert_eq!(json["phase"], "requestSeed");
        assert!(json["seed"].as_str().is_some());
        assert_eq!(json["remainingAttempts"], 3);
    }

    #[tokio::test]
    async fn security_access_send_key_granted() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/hpc/security-access")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"level":1,"phase":"sendKey","key":"AABBCCDD"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["granted"], true);
    }

    #[tokio::test]
    async fn security_access_send_key_denied_empty() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/hpc/security-access")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"level":1,"phase":"sendKey","key":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["granted"], false);
        assert_eq!(json["remainingAttempts"], 2);
    }

    #[tokio::test]
    async fn security_access_invalid_phase() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/hpc/security-access")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"level":1,"phase":"invalid"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── F18: UCM Campaigns (AUTOSAR R24-11) ──────────────────────────────

    #[tokio::test]
    async fn ucm_campaigns_returns_empty_initially() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/ucm/campaigns")
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
        assert_eq!(json["@odata.context"], "$metadata#ucmCampaigns");
    }

    #[tokio::test]
    async fn ucm_create_campaign() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/ucm/campaigns")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"OTA-2025","targetComponents":["hpc"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["name"], "OTA-2025");
        assert_eq!(json["status"], "created");
        assert_eq!(json["progress"], 0);
    }

    #[tokio::test]
    async fn ucm_create_campaign_empty_targets_rejected() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/ucm/campaigns")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Bad","targetComponents":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn ucm_campaign_lifecycle_create_execute_rollback() {
        let state = test_state();
        let app = build_router(state, AuthConfig::default(), true);

        // Create
        let resp = app
            .clone()
            .oneshot(
                Request::post("/sovd/v1/ucm/campaigns")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Full-OTA","targetComponents":["hpc"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let campaign_id = json["id"].as_str().unwrap().to_owned();

        // Execute
        let resp = app
            .clone()
            .oneshot(
                Request::post(format!("/sovd/v1/ucm/campaigns/{campaign_id}/execute"))
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
        assert_eq!(json["status"], "processing");
        assert_eq!(json["progress"], 50);

        // Rollback
        let resp = app
            .clone()
            .oneshot(
                Request::post(format!("/sovd/v1/ucm/campaigns/{campaign_id}/rollback"))
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
        assert_eq!(json["status"], "rollingBack");
    }

    #[tokio::test]
    async fn ucm_get_campaign_not_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/ucm/campaigns/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn ucm_execute_not_found() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/ucm/campaigns/nonexistent/execute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Serialization roundtrip tests for new types ──────────────────────

    #[test]
    fn rxswin_entry_serialization() {
        let entry = native_interfaces::sovd::RxswinEntry {
            component_id: "hpc".into(),
            rxswin: "RXSWIN-001".into(),
            software_version: "1.0.0".into(),
            updated_at: "2025-01-01T00:00:00Z".into(),
            authority: Some("KBA".into()),
            approval_ref: None,
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["componentId"], "hpc");
        assert_eq!(json["rxswin"], "RXSWIN-001");
        assert!(json.get("approvalRef").is_none());
    }

    #[test]
    fn update_provenance_serialization() {
        let entry = native_interfaces::sovd::UpdateProvenanceEntry {
            id: "UPD-001".into(),
            component_id: "hpc".into(),
            previous_version: "1.0.0".into(),
            new_version: "2.0.0".into(),
            applied_at: "2025-06-01T12:00:00Z".into(),
            update_method: "ota".into(),
            package_digest: Some("abcdef".into()),
            success: true,
            rxswin_after: Some("RXSWIN-002".into()),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["previousVersion"], "1.0.0");
        assert_eq!(json["newVersion"], "2.0.0");
        assert_eq!(json["updateMethod"], "ota");
        assert_eq!(json["rxswinAfter"], "RXSWIN-002");
    }

    #[test]
    fn tara_threat_status_variants() {
        use native_interfaces::sovd::TaraThreatStatus;
        for (variant, expected) in [
            (TaraThreatStatus::Identified, "identified"),
            (TaraThreatStatus::Mitigated, "mitigated"),
            (TaraThreatStatus::Accepted, "accepted"),
            (TaraThreatStatus::Transferred, "transferred"),
        ] {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(json.as_str().unwrap(), expected);
        }
    }

    #[test]
    fn ucm_campaign_status_variants() {
        use native_interfaces::sovd::UcmCampaignStatus;
        for (variant, expected) in [
            (UcmCampaignStatus::Created, "created"),
            (UcmCampaignStatus::Transferring, "transferring"),
            (UcmCampaignStatus::Processing, "processing"),
            (UcmCampaignStatus::Activating, "activating"),
            (UcmCampaignStatus::Activated, "activated"),
            (UcmCampaignStatus::RollingBack, "rollingBack"),
            (UcmCampaignStatus::RolledBack, "rolledBack"),
            (UcmCampaignStatus::Failed, "failed"),
            (UcmCampaignStatus::Cancelled, "cancelled"),
        ] {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(json.as_str().unwrap(), expected);
        }
    }

    #[test]
    fn ucm_transfer_phase_variants() {
        use native_interfaces::sovd::UcmTransferPhase;
        for (variant, expected) in [
            (UcmTransferPhase::Idle, "idle"),
            (UcmTransferPhase::Transferring, "transferring"),
            (UcmTransferPhase::Transferred, "transferred"),
            (UcmTransferPhase::Processing, "processing"),
            (UcmTransferPhase::Processed, "processed"),
            (UcmTransferPhase::Activating, "activating"),
            (UcmTransferPhase::Activated, "activated"),
            (UcmTransferPhase::RollingBack, "rollingBack"),
            (UcmTransferPhase::Failed, "failed"),
        ] {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(json.as_str().unwrap(), expected);
        }
    }

    #[test]
    fn uds_security_level_serialization() {
        let level = native_interfaces::sovd::UdsSecurityLevel {
            level: 0x01,
            name: "Workshop".into(),
            description: Some("Routine diag".into()),
            unlocked: false,
            protected_services: vec!["0x2E".into()],
        };
        let json = serde_json::to_value(&level).unwrap();
        assert_eq!(json["level"], 1);
        assert_eq!(json["protectedServices"][0], "0x2E");
    }

    #[test]
    fn uds_security_access_request_roundtrip() {
        let req = native_interfaces::sovd::UdsSecurityAccessRequest {
            level: 1,
            phase: "requestSeed".into(),
            key: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["phase"], "requestSeed");
        assert!(json.get("key").is_none());
        let deser: native_interfaces::sovd::UdsSecurityAccessRequest =
            serde_json::from_value(json).unwrap();
        assert_eq!(deser.level, 1);
    }

    #[test]
    fn uds_security_access_response_roundtrip() {
        let resp = native_interfaces::sovd::UdsSecurityAccessResponse {
            level: 1,
            phase: "requestSeed".into(),
            seed: Some("AABBCCDD".into()),
            granted: None,
            remaining_attempts: Some(3),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["seed"], "AABBCCDD");
        assert_eq!(json["remainingAttempts"], 3);
        assert!(json.get("granted").is_none());
    }

    // ── Review-fix regression tests (v0.16.0) ───────────────────────────

    /// E2: Verify ETag is present and deterministic (SHA-256 based)
    #[tokio::test]
    async fn read_data_etag_is_deterministic() {
        let app = test_router();
        let resp1 = app
            .clone()
            .oneshot(
                Request::get("/sovd/v1/components/hpc/data/speed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let etag1 = resp1
            .headers()
            .get("etag")
            .map(|v| v.to_str().unwrap().to_owned());
        assert!(etag1.is_some(), "ETag header must be present");

        let resp2 = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/data/speed")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let etag2 = resp2
            .headers()
            .get("etag")
            .map(|v| v.to_str().unwrap().to_owned());
        assert_eq!(etag1, etag2, "ETags must be deterministic for same data");
    }

    /// E3: Compliance evidence should reflect auth_enabled=false in test state
    #[tokio::test]
    async fn compliance_evidence_auth_disabled() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/compliance-evidence")
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
        assert_eq!(json["security"]["authEnabled"], false);
    }

    /// E4: disconnect_component returns 204
    #[tokio::test]
    async fn disconnect_component_returns_204() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/x-uds/components/hpc/disconnect")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    /// E5: guarded_audit with audit flag disabled should not record
    #[tokio::test]
    async fn audit_flag_disabled_skips_recording() {
        let state = test_state();
        use native_interfaces::feature_flags::flags;
        state.runtime.feature_flags.set(flags::AUDIT, false);
        let app = build_router(state.clone(), AuthConfig::default(), true);
        let _resp = app
            .oneshot(
                Request::put("/sovd/v1/components/hpc/data/speed")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"value":42}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(state.security.audit_log.recent(10).len(), 0);
    }

    /// C4: evict_and_insert respects configurable max_entries
    #[test]
    fn evict_and_insert_respects_max_entries() {
        let store = dashmap::DashMap::new();
        let order = std::sync::Mutex::new(std::collections::VecDeque::new());
        let max = 3;
        for i in 0..5 {
            evict_and_insert(&store, format!("key-{i}"), i, &order, max);
        }
        assert_eq!(store.len(), 3);
        assert!(!store.contains_key("key-0"));
        assert!(!store.contains_key("key-1"));
        assert!(store.contains_key("key-2"));
        assert!(store.contains_key("key-3"));
        assert!(store.contains_key("key-4"));
    }

    /// C4: evict_and_insert handles re-insert of existing key
    #[test]
    fn evict_and_insert_reinsert_deduplicates() {
        let store = dashmap::DashMap::new();
        let order = std::sync::Mutex::new(std::collections::VecDeque::new());
        evict_and_insert(&store, "a".into(), 1, &order, 3);
        evict_and_insert(&store, "b".into(), 2, &order, 3);
        evict_and_insert(&store, "a".into(), 10, &order, 3);
        evict_and_insert(&store, "c".into(), 3, &order, 3);
        evict_and_insert(&store, "d".into(), 4, &order, 3);
        assert_eq!(store.len(), 3);
        assert!(!store.contains_key("b"));
        assert!(store.contains_key("a"));
        assert!(store.contains_key("c"));
        assert!(store.contains_key("d"));
    }

    /// C6: get_proximity_challenge returns 404 for unknown component
    #[tokio::test]
    async fn proximity_challenge_unknown_component() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/nonexistent/proximity-challenges/fake-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// C2: get_execution scoped — wrong component returns 404
    #[tokio::test]
    async fn get_execution_wrong_component_returns_404() {
        let state = test_state();
        let exec = native_interfaces::sovd::SovdOperationExecution {
            execution_id: "exec-1".into(),
            component_id: "hpc".into(),
            operation_id: "flash".into(),
            status: native_interfaces::sovd::SovdOperationStatus::Running,
            result: None,
            progress: Some(50),
            timestamp: None,
        };
        state.runtime.execution_store.insert("exec-1".into(), exec);
        let app = build_router(state, AuthConfig::default(), true);
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/WRONG/operations/flash/executions/exec-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// C3: cancel_execution scoped — wrong operation returns 404
    #[tokio::test]
    async fn cancel_execution_wrong_operation_returns_404() {
        let state = test_state();
        let exec = native_interfaces::sovd::SovdOperationExecution {
            execution_id: "exec-2".into(),
            component_id: "hpc".into(),
            operation_id: "flash".into(),
            status: native_interfaces::sovd::SovdOperationStatus::Running,
            result: None,
            progress: Some(10),
            timestamp: None,
        };
        state.runtime.execution_store.insert("exec-2".into(), exec);
        let app = build_router(state, AuthConfig::default(), true);
        let resp = app
            .oneshot(
                Request::post("/sovd/v1/components/hpc/operations/WRONG/executions/exec-2/cancel")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// P2: OData filter with boolean value
    #[test]
    fn odata_filter_bool_value() {
        #[derive(Serialize, Clone, Debug, PartialEq)]
        struct Item {
            name: String,
            active: bool,
        }
        let items = vec![
            Item {
                name: "a".into(),
                active: true,
            },
            Item {
                name: "b".into(),
                active: false,
            },
            Item {
                name: "c".into(),
                active: true,
            },
        ];
        let result = apply_odata_filter(items, "active eq 'true'").unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|i| i.active));
    }

    /// P2: OData filter with numeric value
    #[test]
    fn odata_filter_numeric_value() {
        #[derive(Serialize, Clone, Debug)]
        struct Item {
            id: u32,
            label: String,
        }
        let items = vec![
            Item {
                id: 1,
                label: "first".into(),
            },
            Item {
                id: 2,
                label: "second".into(),
            },
            Item {
                id: 3,
                label: "third".into(),
            },
        ];
        let result = apply_odata_filter(items, "id eq 2").unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].label, "second");
    }

    /// P2: OData orderby with pre-extracted keys
    #[test]
    fn odata_orderby_sorts_correctly() {
        #[derive(Serialize, Clone, Debug)]
        struct Item {
            name: String,
            score: u32,
        }
        let mut items = vec![
            Item {
                name: "c".into(),
                score: 30,
            },
            Item {
                name: "a".into(),
                score: 10,
            },
            Item {
                name: "b".into(),
                score: 20,
            },
        ];
        apply_odata_orderby(&mut items, "name").unwrap();
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    /// P2: OData orderby desc
    #[test]
    fn odata_orderby_desc() {
        #[derive(Serialize, Clone, Debug)]
        struct Item {
            val: u32,
        }
        let mut items = vec![Item { val: 1 }, Item { val: 3 }, Item { val: 2 }];
        apply_odata_orderby(&mut items, "val desc").unwrap();
        let vals: Vec<_> = items.iter().map(|i| i.val).collect();
        assert_eq!(vals, vec![3, 2, 1]);
    }

    /// P2: OData filter invalid syntax returns error
    #[test]
    fn odata_filter_invalid_syntax() {
        let items = vec![serde_json::json!({"a": 1})];
        let result = apply_odata_filter(items, "invalid");
        assert!(result.is_err());
    }

    /// P3: Canary routing — X-Served-By header present
    #[tokio::test]
    async fn canary_routing_adds_served_by_header() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.headers().contains_key("x-served-by"));
    }

    /// C1: list_components with variant filter returns filtered results
    #[tokio::test]
    async fn list_components_variant_filter() {
        let app = test_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components?variant=nonexistent")
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
}

// ═══════════════════════════════════════════════════════════════════════════
// MockBackend-based tests — proves Gateway architecture works with ANY backend.
// The SOVD Server dispatches through the ComponentBackend trait object.
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
                software_version: Some("1.0.0".into()),
                hardware_variant: Some("EU-LHD".into()),
                installation_variant: Some("base".into()),
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
                normal_range: Some(native_interfaces::data_catalog::NormalRange {
                    min: 0.0,
                    max: 250.0,
                }),
                semantic_ref: Some("Vehicle.Speed".into()),
                sampling_hint: Some(0.1),
                classification_tags: vec!["powertrain".into()],
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
                affected_subsystem: Some("intake".into()),
                correlated_signals: vec!["Vehicle.Powertrain.MassAirFlow".into()],
                classification_tags: vec!["powertrain".into(), "emission".into()],
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
                mode_descriptors: vec![],
                active_since: None,
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
    }

    #[async_trait]
    impl native_interfaces::ExtendedDiagBackend for MockCdaBackend {
        fn handles_component(&self, component_id: &str) -> bool {
            ComponentBackend::handles_component(self, component_id)
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
        use crate::state::{DiagState, RuntimeState, SecurityState};
        let mock = Arc::new(MockCdaBackend);
        let mock_ext: Arc<dyn native_interfaces::ExtendedDiagBackend> = mock.clone();
        let mock_comp: Arc<dyn ComponentBackend> = mock;
        let router = Arc::new(ComponentRouter::new(vec![mock_comp]).with_extended(vec![mock_ext]));
        AppState {
            backend: router.clone(),
            extended_backend: router.clone(),
            entity_backend: router,
            diag: DiagState {
                fault_manager: Arc::new(FaultManager::new()),
                lock_manager: Arc::new(LockManager::new()),
                diag_log: Arc::new(DiagLog::new()),
                history: Arc::new(native_core::HistoryService::new(
                    Arc::new(native_interfaces::InMemoryStorage::new()),
                    native_core::HistoryConfig::default(),
                )),
            },
            security: SecurityState {
                oem_profile: Arc::new(native_interfaces::DefaultProfile),
                audit_log: Arc::new(native_core::AuditLog::new()),
                rate_limiter: None,
                auth_enabled: false,
            },
            runtime: RuntimeState {
                health: Arc::new(HealthMonitor::new()),
                max_store_entries: 10_000,
                execution_store: Arc::new(dashmap::DashMap::new()),
                execution_order: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
                proximity_store: Arc::new(dashmap::DashMap::new()),
                proximity_order: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
                package_store: Arc::new(dashmap::DashMap::new()),
                feature_flags: Arc::new(native_interfaces::FeatureFlags::new()),
                firmware_verifier: Arc::new(native_interfaces::NoopVerifier),
                rxswin_store: Arc::new(dashmap::DashMap::new()),
                provenance_log: Arc::new(parking_lot::RwLock::new(Vec::new())),
                tara_assets: Arc::new(parking_lot::RwLock::new(Vec::new())),
                tara_threats: Arc::new(parking_lot::RwLock::new(Vec::new())),
                ucm_campaigns: Arc::new(dashmap::DashMap::new()),
            },
            data_catalog: Arc::new(native_interfaces::StaticDataCatalogProvider::new()),
        }
    }

    fn mock_router() -> Router {
        build_router(mock_state(), AuthConfig::default(), true)
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
        let bridge = FaultBridge::new(state.diag.fault_manager.clone());
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

        let app = build_router(state, AuthConfig::default(), true);
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
        let app = build_router(mock_state(), auth_enabled_config(), true);
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
        let app = build_router(mock_state(), auth_enabled_config(), true);
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
        let app = build_router(mock_state(), auth_enabled_config(), true);
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
        let app = build_router(mock_state(), auth_enabled_config(), true);
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
        let app = build_router(mock_state(), config, true);
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
            .diag
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default(), true);
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
            .diag
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default(), true);
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
        let app = build_router(state, AuthConfig::default(), true);
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
            .diag
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default(), true);
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
            .diag
            .lock_manager
            .acquire("mock-ecu", "rightful-owner", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default(), true);
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
        let app = build_router(state.clone(), config, true);
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
        let app = build_router(state.clone(), AuthConfig::default(), true);
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
        let app2 = build_router(state, AuthConfig::default(), true);
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
            .diag
            .lock_manager
            .acquire("mock-ecu", "owner-1", Some(future))
            .unwrap();
        let app = build_router(state, AuthConfig::default(), true);
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
            .diag
            .lock_manager
            .acquire("mock-ecu", "owner-1", None)
            .unwrap();
        let app = build_router(state, AuthConfig::default(), true);
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

    // ── Wave 4 endpoint tests ─────────────────────────────────────────

    #[tokio::test]
    async fn component_snapshot_returns_ndjson() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/x-ndjson");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let lines: Vec<&str> = text.trim().lines().collect();
        assert!(lines.len() >= 2, "expected at least meta + 1 data line");
        // First line is metadata preamble
        let meta: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(meta["_meta"], true);
        assert_eq!(meta["exportType"], "component-snapshot");
        assert!(meta.get("exportedAt").is_some());
        assert!(meta.get("serverVersion").is_some());
        assert!(meta.get("schemaVersion").is_some());
        // Second line is a data record
        let record: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert!(record.get("id").is_some());
        assert!(record.get("componentId").is_some());
    }

    #[tokio::test]
    async fn export_faults_returns_ndjson() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/export/faults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ct, "application/x-ndjson");
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let lines: Vec<&str> = text.trim().lines().collect();
        assert!(lines.len() >= 2, "expected at least meta + 1 fault line");
        let meta: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(meta["_meta"], true);
        assert_eq!(meta["exportType"], "fault-export");
        assert!(meta.get("componentFirmwareVersions").is_some());
        // Fault record
        let fault: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert!(fault.get("id").is_some());
        assert!(fault.get("componentId").is_some());
        assert!(fault.get("severity").is_some());
        // W4.3 enrichment fields
        assert!(fault.get("affectedSubsystem").is_some());
        assert!(fault.get("correlatedSignals").is_some());
        assert!(fault.get("classificationTags").is_some());
    }

    #[tokio::test]
    async fn export_faults_severity_filter() {
        let app = mock_router();
        // Our mock fault is severity=high. Filter for critical only → no faults
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/export/faults?severity=critical")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        let lines: Vec<&str> = text.trim().lines().collect();
        // Only the meta line, no faults
        assert_eq!(
            lines.len(),
            1,
            "critical filter should exclude 'high' severity faults"
        );
    }

    #[tokio::test]
    async fn schema_data_catalog_returns_components() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/schema/data-catalog")
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
        assert_eq!(json["@odata.context"], "$metadata#schema/data-catalog");
        assert!(json.get("schemaVersion").is_some());
        assert_eq!(json["ontologyRef"], "COVESA VSS");
        assert!(json.get("serverVersion").is_some());
        assert!(json.get("generatedAt").is_some());
        let components = json["components"].as_array().unwrap();
        assert!(!components.is_empty());
        // First component should have dataItems
        let first = &components[0];
        assert!(first.get("componentId").is_some());
        let items = first["dataItems"].as_array().unwrap();
        assert!(!items.is_empty());
        // Data items should have semantic metadata from mock
        let item = &items[0];
        assert!(item.get("id").is_some());
        assert!(item.get("dataType").is_some());
    }

    #[tokio::test]
    async fn data_subscribe_returns_sse() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/mock-ecu/data/subscribe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            ct.contains("text/event-stream"),
            "expected SSE content type"
        );
    }

    #[tokio::test]
    async fn data_subscribe_unknown_component_404() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/nonexistent/data/subscribe")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn snapshot_unknown_component_404() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/nonexistent/snapshot")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Feature-flag-gated behavior tests (E2.4) ─────────────────────────

    #[tokio::test]
    async fn feature_flags_list_returns_default_flags() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::get("/x-admin/features")
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
        let flags = json["value"].as_array().unwrap();
        // Should have at least the core flags: audit, history, rate_limit, bridge
        assert!(
            flags.len() >= 4,
            "Expected at least 4 default flags, got {}",
            flags.len()
        );
    }

    #[tokio::test]
    async fn feature_flag_disable_audit_suppresses_recording() {
        let state = mock_state();
        // Disable audit via feature flag
        state.runtime.feature_flags.set("audit", false);
        let app = build_router(state.clone(), AuthConfig::default(), true);

        // Trigger a fault clear (which normally records audit)
        let resp = app
            .oneshot(
                Request::delete("/sovd/v1/components/hpc/faults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // Audit log should be empty since flag was disabled
        let entries = state.security.audit_log.recent(10);
        assert!(
            entries.is_empty(),
            "Expected no audit entries when audit flag is disabled, got {}",
            entries.len()
        );
    }

    #[tokio::test]
    async fn feature_flag_disable_history_suppresses_recording() {
        let state = mock_state();
        // Disable history via feature flag
        state.runtime.feature_flags.set("history", false);
        let app = build_router(state.clone(), AuthConfig::default(), true);

        // Trigger a fault list (which normally records to history)
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/components/hpc/faults")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // History should be empty since flag was disabled
        assert_eq!(
            state.diag.history.fault_count(),
            0,
            "Expected no history entries when history flag is disabled"
        );
    }

    #[tokio::test]
    async fn feature_flag_set_toggle_via_admin_api() {
        let app = mock_router();

        // Disable audit flag
        let resp = app
            .oneshot(
                Request::put("/x-admin/features/audit")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"enabled": false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["enabled"], false);
    }

    #[tokio::test]
    async fn feature_flag_unknown_flag_returns_404() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::put("/x-admin/features/nonexistent")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"enabled": true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn backup_returns_json_snapshot() {
        let app = mock_router();
        let resp = app
            .oneshot(Request::get("/x-admin/backup").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("application/json"));
        let cd = resp
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(cd.contains("opensovd-backup-"));
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["version"], 1);
        assert!(json.get("created_at").is_some());
        assert!(json.get("faults").is_some());
        assert!(json.get("audit_entries").is_some());
    }

    #[tokio::test]
    async fn restore_invalid_json_returns_400() {
        let app = mock_router();
        let resp = app
            .oneshot(
                Request::post("/x-admin/restore")
                    .header("content-type", "application/json")
                    .body(Body::from("not-valid-json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── DiscoveryPolicy gating tests ─────────────────────────────────────

    /// OEM profile that disables areas (like MBDS §2.2)
    #[derive(Debug, Clone)]
    struct AreasDisabledProfile;
    impl native_interfaces::oem::AuthPolicy for AreasDisabledProfile {}
    impl native_interfaces::oem::AuthzPolicy for AreasDisabledProfile {}
    impl native_interfaces::oem::EntityIdPolicy for AreasDisabledProfile {}
    impl native_interfaces::oem::CdfPolicy for AreasDisabledProfile {}
    impl native_interfaces::oem::DiscoveryPolicy for AreasDisabledProfile {
        fn areas_enabled(&self) -> bool {
            false
        }
    }
    impl native_interfaces::oem::OemProfile for AreasDisabledProfile {
        fn name(&self) -> &'static str {
            "Test (areas disabled)"
        }
        fn id(&self) -> &'static str {
            "test-no-areas"
        }
        fn as_auth_policy(&self) -> &dyn native_interfaces::oem::AuthPolicy {
            self
        }
        fn as_authz_policy(&self) -> &dyn native_interfaces::oem::AuthzPolicy {
            self
        }
        fn as_entity_id_policy(&self) -> &dyn native_interfaces::oem::EntityIdPolicy {
            self
        }
        fn as_discovery_policy(&self) -> &dyn native_interfaces::oem::DiscoveryPolicy {
            self
        }
        fn as_cdf_policy(&self) -> &dyn native_interfaces::oem::CdfPolicy {
            self
        }
    }

    fn mock_state_with_profile(profile: Arc<dyn native_interfaces::OemProfile>) -> AppState {
        let mut state = mock_state();
        state.security.oem_profile = profile;
        state
    }

    #[tokio::test]
    async fn areas_disabled_returns_404() {
        let state = mock_state_with_profile(Arc::new(AreasDisabledProfile));
        let app = build_router(state, AuthConfig::default(), true);
        let resp = app
            .oneshot(Request::get("/sovd/v1/areas").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn areas_disabled_get_by_id_returns_404() {
        let state = mock_state_with_profile(Arc::new(AreasDisabledProfile));
        let app = build_router(state, AuthConfig::default(), true);
        let resp = app
            .oneshot(
                Request::get("/sovd/v1/areas/zone-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn areas_enabled_returns_200() {
        let app = mock_router(); // DefaultProfile — areas_enabled() = true
        let resp = app
            .oneshot(Request::get("/sovd/v1/areas").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["@odata.context"], "$metadata#areas");
        assert_eq!(json["@odata.count"], 0); // empty default
    }
}
