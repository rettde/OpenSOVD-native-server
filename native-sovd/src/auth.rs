// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Auth middleware — tower layer for API key, JWT, and OIDC authentication
//
// Supports three modes (configurable, checked in order):
//   1. API-Key: static key in "X-API-Key" header
//   2. JWT Bearer (static secret): RS256/HS256 token via `jwt_secret`
//   3. OIDC: JWT validated against JWKS from `oidc_issuer_url`
//
// Health and discovery endpoints are excluded from auth by default.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use axum::{
    body::Body,
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use native_core::AuditLog;
use native_interfaces::oem::{AuthPolicy, AuthzContext, AuthzDecision, AuthzPolicy, OemProfile};
use native_interfaces::sovd::{SovdAuditAction, SovdErrorEnvelope};
use native_interfaces::tenant::TenantContext;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tracing::{debug, warn};

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Enable authentication (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// Static API key for X-API-Key header authentication
    #[serde(default)]
    pub api_key: Option<String>,
    /// JWT secret (for HS256) or public key path (for RS256)
    #[serde(default)]
    pub jwt_secret: Option<String>,
    /// JWT algorithm: "HS256" or "RS256" (default: "HS256")
    #[serde(default = "default_algorithm")]
    pub jwt_algorithm: String,
    /// JWT issuer to validate (optional)
    #[serde(default)]
    pub jwt_issuer: Option<String>,
    /// OIDC issuer URL for automatic JWKS discovery (e.g. "https://auth.example.com/realms/sovd")
    /// When set, the server fetches `{oidc_issuer_url}/.well-known/openid-configuration`
    /// to obtain the JWKS URI for RS256 key validation.
    #[serde(default)]
    pub oidc_issuer_url: Option<String>,
    /// Paths excluded from auth (default: ["/sovd/v1/health", "/sovd/v1/"])
    #[serde(default = "default_public_paths")]
    pub public_paths: Vec<String>,
    /// Allowed CORS origins. Empty = permissive (dev mode). Set for production.
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

fn default_algorithm() -> String {
    "HS256".to_owned()
}

fn default_public_paths() -> Vec<String> {
    vec![
        "/sovd/v1/".to_owned(),
        "/sovd/v1/health".to_owned(),
        "/healthz".to_owned(),
        "/readyz".to_owned(),
        "/sovd/v1/$metadata".to_owned(),
        "/openapi.json".to_owned(),
        "/metrics".to_owned(),
    ]
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            jwt_secret: None,
            jwt_algorithm: default_algorithm(),
            jwt_issuer: None,
            oidc_issuer_url: None,
            public_paths: default_public_paths(),
            cors_origins: vec![],
        }
    }
}

/// Combined middleware state: transport-level auth config + OEM profile.
///
/// Analogous to CDA's `SecurityPluginMiddleware` pattern where the plugin
/// is made available throughout the request lifecycle.
#[derive(Clone)]
pub struct AuthState {
    pub config: AuthConfig,
    pub oem_profile: Arc<dyn OemProfile>,
    pub audit_log: Arc<AuditLog>,
}

/// JWT claims structure
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user/service ID)
    pub sub: String,
    /// Expiration time (unix timestamp)
    pub exp: usize,
    /// Issued at (unix timestamp)
    #[serde(default)]
    pub iat: usize,
    /// Issuer
    #[serde(default)]
    pub iss: Option<String>,
    /// Roles/scopes
    #[serde(default)]
    pub roles: Vec<String>,
    /// Vehicle Identification Number (MBDS S-SOVD §6.2)
    #[serde(default)]
    pub vin: Option<String>,
    /// OAuth2 scope claim (MBDS S-SOVD §6.2)
    #[serde(default, alias = "scp")]
    pub scope: Option<String>,
    /// Tenant identifier for multi-tenant deployments (Wave 3, A3.3)
    #[serde(default)]
    pub tenant_id: Option<String>,
}

/// Enforce OEM-specific claim rules via the active AuthPolicy.
///
/// Converts structured Claims into a generic HashMap so the policy
/// doesn't depend on our internal JWT struct. This keeps the OemProfile
/// trait in `native-interfaces` free of `jsonwebtoken` dependencies.
fn enforce_claims(
    claims: &Claims,
    auth_policy: &dyn AuthPolicy,
    path: &str,
) -> Result<(), Response> {
    let mut claim_map = std::collections::HashMap::new();
    claim_map.insert(
        "sub".to_owned(),
        serde_json::Value::String(claims.sub.clone()),
    );
    if let Some(ref vin) = claims.vin {
        claim_map.insert("vin".to_owned(), serde_json::Value::String(vin.clone()));
    }
    if let Some(ref scope) = claims.scope {
        claim_map.insert("scope".to_owned(), serde_json::Value::String(scope.clone()));
    }
    auth_policy
        .validate_claims(&claim_map, path)
        .map_err(|(status, code, message)| {
            auth_error(
                StatusCode::from_u16(status).unwrap_or(StatusCode::FORBIDDEN),
                &code,
                &message,
            )
        })
}

/// Build an OData-conformant JSON error response for auth failures (SOVD §5.4).
fn auth_error(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(SovdErrorEnvelope::new(code, message))).into_response()
}

// ── Fine-grained authorization (Wave 1) ──────────────────────────────────

/// Build an `AuthzContext` from the HTTP request path and authenticated claims.
///
/// Parses the SOVD path structure (`/sovd/v1/{entity_type}/{entity_id}/{resource}/{resource_id}`)
/// into semantic fields that `AuthzPolicy` implementations can use for decisions.
fn build_authz_context(
    method: &str,
    path: &str,
    caller: &str,
    roles: &[String],
    scopes: &[String],
) -> AuthzContext {
    // Strip "/sovd/v1/" (or "/sovd/v1/x-uds/") prefix
    let stripped = path
        .strip_prefix("/sovd/v1/x-uds/")
        .or_else(|| path.strip_prefix("/sovd/v1/"))
        .unwrap_or(path);

    let segments: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();

    // Parse entity_type / entity_id / resource / resource_id from segments
    let (entity_type, entity_id, resource, resource_id) = match segments.as_slice() {
        // /sovd/v1/ (server root)
        [] => ("server", None, "discovery".to_owned(), None),
        // /sovd/v1/health, /sovd/v1/version-info, /sovd/v1/$metadata, /sovd/v1/docs
        [single] => {
            let et = match *single {
                "components" => "component",
                "groups" => "group",
                "apps" => "app",
                "funcs" => "func",
                _ => "server",
            };
            let res = match *single {
                "components" | "groups" | "apps" | "funcs" => "collection".to_owned(),
                "audit" => "audit".to_owned(),
                other => other.to_owned(),
            };
            (et, None, res, None)
        }
        // /sovd/v1/components/{id} or /sovd/v1/groups/{id}
        [entity_collection, id] => {
            let et = entity_type_from_collection(entity_collection);
            (et, Some(id.to_string()), "detail".to_owned(), None)
        }
        // /sovd/v1/components/{id}/{resource}
        [entity_collection, id, res] => {
            let et = entity_type_from_collection(entity_collection);
            (et, Some(id.to_string()), res.to_string(), None)
        }
        // /sovd/v1/components/{id}/{resource}/{res_id} [or deeper]
        [entity_collection, id, res, res_id, ..] => {
            let et = entity_type_from_collection(entity_collection);
            (
                et,
                Some(id.to_string()),
                res.to_string(),
                Some(res_id.to_string()),
            )
        }
    };

    AuthzContext {
        caller: caller.to_owned(),
        roles: roles.to_vec(),
        scopes: scopes.to_vec(),
        method: method.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id,
        resource,
        resource_id,
        path: path.to_owned(),
    }
}

/// Map collection path segment to entity type name.
fn entity_type_from_collection(collection: &str) -> &'static str {
    match collection {
        "components" => "component",
        "groups" => "group",
        "apps" => "app",
        "funcs" => "func",
        _ => "unknown",
    }
}

/// Extract OAuth2 scopes from JWT claims into a Vec for authz context.
fn extract_scopes(claims: &Claims) -> Vec<String> {
    claims
        .scope
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Perform fine-grained authorization check via the OEM profile's AuthzPolicy.
fn perform_authz(
    authz_policy: &dyn AuthzPolicy,
    method: &str,
    path: &str,
    caller: &str,
    roles: &[String],
    scopes: &[String],
    audit_log: &AuditLog,
) -> Result<(), Response> {
    let ctx = build_authz_context(method, path, caller, roles, scopes);
    match authz_policy.authorize(&ctx) {
        AuthzDecision::Allow => Ok(()),
        AuthzDecision::Deny {
            status,
            code,
            message,
        } => {
            warn!(
                caller = %ctx.caller,
                method = %ctx.method,
                path = %ctx.path,
                entity_type = %ctx.entity_type,
                resource = %ctx.resource,
                "AuthZ denied: {message}"
            );
            audit_log.record(
                caller,
                SovdAuditAction::AuthzDenied,
                &ctx.entity_type,
                &ctx.resource,
                method,
                "denied",
                Some(&message),
                None,
            );
            Err(auth_error(
                StatusCode::from_u16(status).unwrap_or(StatusCode::FORBIDDEN),
                &code,
                &message,
            ))
        }
    }
}

/// Auth middleware function — used with axum::middleware::from_fn_with_state
#[allow(clippy::too_many_lines)]
pub async fn auth_middleware(
    axum::extract::State(auth_state): axum::extract::State<AuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, Response> {
    let config = &auth_state.config;

    // Skip auth if disabled
    if !config.enabled {
        return Ok(next.run(request).await);
    }

    let path = request.uri().path().to_owned();

    // Skip auth for public paths
    if config.public_paths.contains(&path) {
        return Ok(next.run(request).await);
    }

    // Try API key first
    if let Some(ref expected_key) = config.api_key {
        if let Some(provided_key) = request.headers().get("x-api-key") {
            if let Ok(key_str) = provided_key.to_str() {
                if key_str.as_bytes().ct_eq(expected_key.as_bytes()).into() {
                    debug!(path = %path, "Authenticated via API key");
                    // Fine-grained authorization check (Wave 1)
                    let req_method = request.method().to_string();
                    perform_authz(
                        auth_state.oem_profile.as_authz_policy(),
                        &req_method,
                        &path,
                        "api-key-client",
                        &[],
                        &[],
                        &auth_state.audit_log,
                    )?;
                    auth_state.audit_log.record(
                        "api-key-client",
                        SovdAuditAction::AuthSuccess,
                        "server",
                        &path,
                        &req_method,
                        "success",
                        Some("api-key"),
                        None,
                    );
                    // Inject client identity from API key (hash-based)
                    request
                        .extensions_mut()
                        .insert(AuthenticatedClient("api-key-client".to_owned()));
                    // Inject default tenant context for API key auth (Wave 3, A3.3)
                    request.extensions_mut().insert(TenantContext::default());
                    return Ok(next.run(request).await);
                }
            }
            warn!(path = %path, "Invalid API key");
            auth_state.audit_log.record(
                "unknown",
                SovdAuditAction::AuthFailure,
                "server",
                &path,
                request.method().as_str(),
                "denied",
                Some("Invalid API key"),
                None,
            );
            return Err(auth_error(
                StatusCode::UNAUTHORIZED,
                "SOVD-ERR-401",
                "Invalid API key",
            ));
        }
    }

    // Extract Bearer token (shared by jwt_secret and OIDC paths)
    if let Some(auth_header) = request.headers().get(header::AUTHORIZATION) {
        let auth_str = auth_header.to_str().map_err(|_| {
            warn!(path = %path, "Malformed Authorization header");
            auth_error(
                StatusCode::UNAUTHORIZED,
                "SOVD-ERR-401",
                "Malformed Authorization header",
            )
        })?;
        let token = auth_str.strip_prefix("Bearer ").ok_or_else(|| {
            warn!(path = %path, "Missing Bearer prefix in Authorization header");
            auth_error(
                StatusCode::UNAUTHORIZED,
                "SOVD-ERR-401",
                "Missing Bearer prefix",
            )
        })?;
        let token_owned = token.to_owned();

        // Try static JWT secret first
        if let Some(ref jwt_secret) = config.jwt_secret {
            return validate_jwt(&token_owned, jwt_secret, &auth_state, &path, next, request).await;
        }

        // Try OIDC issuer (fetch JWKS dynamically)
        if let Some(ref issuer_url) = config.oidc_issuer_url {
            return validate_oidc_jwt(&token_owned, issuer_url, &auth_state, &path, next, request)
                .await;
        }
    }

    // No valid credentials provided
    warn!(path = %path, "No authentication credentials provided");
    auth_state.audit_log.record(
        "unknown",
        SovdAuditAction::AuthFailure,
        "server",
        &path,
        request.method().as_str(),
        "denied",
        Some("No credentials"),
        None,
    );
    Err(auth_error(
        StatusCode::UNAUTHORIZED,
        "SOVD-ERR-401",
        "Authentication required",
    ))
}

/// Authenticated client identity — injected into request extensions by auth middleware.
/// Downstream handlers use this for lock ownership (SOVD §7.4) instead of a custom header.
#[derive(Debug, Clone)]
pub struct AuthenticatedClient(pub String);

async fn validate_jwt(
    token: &str,
    secret: &str,
    auth_state: &AuthState,
    path: &str,
    next: Next,
    mut request: Request<Body>,
) -> Result<Response, Response> {
    let config = &auth_state.config;
    let token_validator = auth_state.oem_profile.as_auth_policy();
    let access_checker = auth_state.oem_profile.as_authz_policy();

    let algorithm = match config.jwt_algorithm.as_str() {
        "RS256" => Algorithm::RS256,
        "HS384" => Algorithm::HS384,
        "HS512" => Algorithm::HS512,
        _ => Algorithm::HS256,
    };

    let decoding_key = match algorithm {
        Algorithm::RS256 => DecodingKey::from_rsa_pem(secret.as_bytes()).map_err(|e| {
            warn!("Invalid RSA public key: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SOVD-ERR-500",
                &format!("Invalid RSA key: {e}"),
            )
        })?,
        _ => DecodingKey::from_secret(secret.as_bytes()),
    };

    let mut validation = Validation::new(algorithm);
    if let Some(ref issuer) = config.jwt_issuer {
        validation.set_issuer(&[issuer]);
    }

    let token_status = StatusCode::from_u16(token_validator.invalid_token_status())
        .unwrap_or(StatusCode::UNAUTHORIZED);
    let token_error_code = token_validator.invalid_token_error_code();

    match decode::<Claims>(token, &decoding_key, &validation) {
        Ok(token_data) => {
            enforce_claims(&token_data.claims, token_validator, path)?;
            debug!(
                path = %path,
                sub = %token_data.claims.sub,
                "Authenticated via JWT"
            );
            // AuthZ check (Wave 1)
            let scopes = extract_scopes(&token_data.claims);
            perform_authz(
                access_checker,
                request.method().as_str(),
                path,
                &token_data.claims.sub,
                &token_data.claims.roles,
                &scopes,
                &auth_state.audit_log,
            )?;
            auth_state.audit_log.record(
                &token_data.claims.sub,
                SovdAuditAction::AuthSuccess,
                "server",
                path,
                request.method().as_str(),
                "success",
                Some("jwt"),
                None,
            );
            // Inject tenant context from JWT claims (Wave 3, A3.3)
            let tenant_ctx = token_data
                .claims
                .tenant_id
                .as_deref()
                .map_or_else(TenantContext::default, TenantContext::new);
            request.extensions_mut().insert(tenant_ctx);
            // Inject client identity from JWT sub claim (SOVD §7.4)
            request
                .extensions_mut()
                .insert(AuthenticatedClient(token_data.claims.sub));
            Ok(next.run(request).await)
        }
        Err(e) => {
            warn!(path = %path, error = %e, "JWT validation failed");
            auth_state.audit_log.record(
                "unknown",
                SovdAuditAction::AuthFailure,
                "server",
                path,
                request.method().as_str(),
                "denied",
                Some(&format!("JWT: {e}")),
                None,
            );
            Err(auth_error(token_status, token_error_code, "Invalid token"))
        }
    }
}

/// OIDC discovery document (partial — only what we need)
#[derive(Deserialize)]
struct OidcDiscovery {
    jwks_uri: String,
    #[serde(default)]
    issuer: Option<String>,
}

/// JWKS key set
#[derive(Deserialize, Clone)]
struct JwksKeySet {
    keys: Vec<JwksKey>,
}

/// Single JWK (RSA public key)
#[derive(Deserialize, Clone)]
struct JwksKey {
    #[serde(default)]
    kty: String,
    #[serde(default)]
    n: String,
    #[serde(default)]
    e: String,
    #[serde(default)]
    kid: Option<String>,
}

// ── JWKS TTL cache (process-global) ──────────────────────────────────────

/// JWKS cache TTL (5 minutes)
const JWKS_CACHE_TTL_SECS: u64 = 300;

struct JwksCacheEntry {
    keys: JwksKeySet,
    issuer: Option<String>,
    fetched_at: std::time::Instant,
}

static JWKS_CACHE: std::sync::OnceLock<tokio::sync::Mutex<Option<JwksCacheEntry>>> =
    std::sync::OnceLock::new();

fn jwks_cache() -> &'static tokio::sync::Mutex<Option<JwksCacheEntry>> {
    JWKS_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Fetch JWKS (with TTL cache). Returns cached keys if still fresh, otherwise re-fetches.
async fn fetch_jwks_cached(issuer_url: &str) -> Result<(JwksKeySet, Option<String>), Response> {
    let mut guard = jwks_cache().lock().await;

    // Return cached if fresh
    if let Some(ref entry) = *guard {
        if entry.fetched_at.elapsed().as_secs() < JWKS_CACHE_TTL_SECS {
            debug!(
                "JWKS cache hit (age={}s)",
                entry.fetched_at.elapsed().as_secs()
            );
            return Ok((entry.keys.clone(), entry.issuer.clone()));
        }
    }

    // Cache miss or stale — fetch
    debug!("JWKS cache miss, fetching from {issuer_url}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| {
            warn!("Failed to create HTTP client for OIDC: {e}");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SOVD-ERR-500",
                &format!("OIDC client error: {e}"),
            )
        })?;

    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    );
    let discovery: OidcDiscovery = client
        .get(&discovery_url)
        .send()
        .await
        .map_err(|e| {
            warn!(url = %discovery_url, error = %e, "OIDC discovery fetch failed");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SOVD-ERR-500",
                &format!("OIDC discovery failed: {e}"),
            )
        })?
        .json()
        .await
        .map_err(|e| {
            warn!(error = %e, "OIDC discovery parse failed");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SOVD-ERR-500",
                &format!("OIDC discovery parse failed: {e}"),
            )
        })?;

    let jwks: JwksKeySet = client
        .get(&discovery.jwks_uri)
        .send()
        .await
        .map_err(|e| {
            warn!(url = %discovery.jwks_uri, error = %e, "JWKS fetch failed");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SOVD-ERR-500",
                &format!("JWKS fetch failed: {e}"),
            )
        })?
        .json()
        .await
        .map_err(|e| {
            warn!(error = %e, "JWKS parse failed");
            auth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "SOVD-ERR-500",
                &format!("JWKS parse failed: {e}"),
            )
        })?;

    let issuer = discovery.issuer.clone();
    *guard = Some(JwksCacheEntry {
        keys: jwks.clone(),
        issuer: issuer.clone(),
        fetched_at: std::time::Instant::now(),
    });

    Ok((jwks, issuer))
}

/// Validate a JWT token using OIDC discovery (cached JWKS)
async fn validate_oidc_jwt(
    token: &str,
    issuer_url: &str,
    auth_state: &AuthState,
    path: &str,
    next: Next,
    mut request: Request<Body>,
) -> Result<Response, Response> {
    let config = &auth_state.config;
    let token_validator = auth_state.oem_profile.as_auth_policy();
    let access_checker = auth_state.oem_profile.as_authz_policy();
    let token_status = StatusCode::from_u16(token_validator.invalid_token_status())
        .unwrap_or(StatusCode::UNAUTHORIZED);
    let token_error_code = token_validator.invalid_token_error_code();

    // 1. Get JWKS (from cache or fetch)
    let (jwks, discovered_issuer) = fetch_jwks_cached(issuer_url).await?;

    // 2. Decode JWT header to find kid
    let jwt_header = jsonwebtoken::decode_header(token).map_err(|e| {
        warn!(path = %path, error = %e, "JWT header decode failed");
        auth_error(token_status, token_error_code, "JWT header decode failed")
    })?;

    // 3. Find matching key
    let key = jwks
        .keys
        .iter()
        .find(|k| k.kty == "RSA" && (jwt_header.kid.is_none() || k.kid == jwt_header.kid))
        .ok_or_else(|| {
            warn!(path = %path, "No matching RSA key found in JWKS");
            auth_error(
                token_status,
                token_error_code,
                "No matching RSA key in JWKS",
            )
        })?;

    // 4. Build decoding key from JWK components
    let decoding_key = DecodingKey::from_rsa_components(&key.n, &key.e).map_err(|e| {
        warn!(error = %e, "Failed to build RSA key from JWKS");
        auth_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "SOVD-ERR-500",
            &format!("JWKS key error: {e}"),
        )
    })?;

    // 5. Validate
    let mut validation = Validation::new(Algorithm::RS256);
    let issuer = config
        .jwt_issuer
        .as_deref()
        .or(discovered_issuer.as_deref());
    if let Some(iss) = issuer {
        validation.set_issuer(&[iss]);
    }

    match decode::<Claims>(token, &decoding_key, &validation) {
        Ok(token_data) => {
            enforce_claims(&token_data.claims, token_validator, path)?;
            debug!(
                path = %path,
                sub = %token_data.claims.sub,
                "Authenticated via OIDC JWT"
            );
            // AuthZ check (Wave 1)
            let scopes = extract_scopes(&token_data.claims);
            perform_authz(
                access_checker,
                request.method().as_str(),
                path,
                &token_data.claims.sub,
                &token_data.claims.roles,
                &scopes,
                &auth_state.audit_log,
            )?;
            auth_state.audit_log.record(
                &token_data.claims.sub,
                SovdAuditAction::AuthSuccess,
                "server",
                path,
                request.method().as_str(),
                "success",
                Some("oidc"),
                None,
            );
            // Inject tenant context from OIDC JWT claims (Wave 3, A3.3)
            let tenant_ctx = token_data
                .claims
                .tenant_id
                .as_deref()
                .map_or_else(TenantContext::default, TenantContext::new);
            request.extensions_mut().insert(tenant_ctx);
            request
                .extensions_mut()
                .insert(AuthenticatedClient(token_data.claims.sub));
            Ok(next.run(request).await)
        }
        Err(e) => {
            warn!(path = %path, error = %e, "OIDC JWT validation failed");
            auth_state.audit_log.record(
                "unknown",
                SovdAuditAction::AuthFailure,
                "server",
                path,
                request.method().as_str(),
                "denied",
                Some(&format!("OIDC: {e}")),
                None,
            );
            Err(auth_error(token_status, token_error_code, "Invalid token"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_interfaces::oem::AuthzDecision;

    // ── build_authz_context tests ─────────────────────────────────────────

    #[test]
    fn authz_context_server_root() {
        let ctx = build_authz_context("GET", "/sovd/v1/", "user1", &[], &[]);
        assert_eq!(ctx.entity_type, "server");
        assert_eq!(ctx.resource, "discovery");
        assert!(ctx.entity_id.is_none());
        assert!(ctx.resource_id.is_none());
    }

    #[test]
    fn authz_context_health() {
        let ctx = build_authz_context("GET", "/sovd/v1/health", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "server");
        assert_eq!(ctx.resource, "health");
    }

    #[test]
    fn authz_context_components_collection() {
        let ctx = build_authz_context("GET", "/sovd/v1/components", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "component");
        assert_eq!(ctx.resource, "collection");
        assert!(ctx.entity_id.is_none());
    }

    #[test]
    fn authz_context_single_component() {
        let ctx = build_authz_context("GET", "/sovd/v1/components/hpc", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "component");
        assert_eq!(ctx.entity_id.as_deref(), Some("hpc"));
        assert_eq!(ctx.resource, "detail");
    }

    #[test]
    fn authz_context_component_data() {
        let ctx = build_authz_context("GET", "/sovd/v1/components/hpc/data", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "component");
        assert_eq!(ctx.entity_id.as_deref(), Some("hpc"));
        assert_eq!(ctx.resource, "data");
        assert!(ctx.resource_id.is_none());
    }

    #[test]
    fn authz_context_component_data_item() {
        let ctx = build_authz_context("GET", "/sovd/v1/components/hpc/data/rpm", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "component");
        assert_eq!(ctx.entity_id.as_deref(), Some("hpc"));
        assert_eq!(ctx.resource, "data");
        assert_eq!(ctx.resource_id.as_deref(), Some("rpm"));
    }

    #[test]
    fn authz_context_faults_delete() {
        let ctx = build_authz_context(
            "DELETE",
            "/sovd/v1/components/brake/faults",
            "admin",
            &[],
            &[],
        );
        assert_eq!(ctx.method, "DELETE");
        assert_eq!(ctx.entity_type, "component");
        assert_eq!(ctx.entity_id.as_deref(), Some("brake"));
        assert_eq!(ctx.resource, "faults");
        assert_eq!(ctx.caller, "admin");
    }

    #[test]
    fn authz_context_operations_execute() {
        let ctx = build_authz_context(
            "POST",
            "/sovd/v1/components/hpc/operations/flash",
            "tech",
            &["admin".into()],
            &["diagnostic_enhanced".into()],
        );
        assert_eq!(ctx.method, "POST");
        assert_eq!(ctx.resource, "operations");
        assert_eq!(ctx.resource_id.as_deref(), Some("flash"));
        assert_eq!(ctx.roles, vec!["admin"]);
        assert_eq!(ctx.scopes, vec!["diagnostic_enhanced"]);
    }

    #[test]
    fn authz_context_software_packages() {
        let ctx = build_authz_context(
            "POST",
            "/sovd/v1/components/hpc/software-packages/pkg-1",
            "u",
            &[],
            &[],
        );
        assert_eq!(ctx.resource, "software-packages");
        assert_eq!(ctx.resource_id.as_deref(), Some("pkg-1"));
    }

    #[test]
    fn authz_context_groups() {
        let ctx = build_authz_context("GET", "/sovd/v1/groups/powertrain", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "group");
        assert_eq!(ctx.entity_id.as_deref(), Some("powertrain"));
    }

    #[test]
    fn authz_context_apps() {
        let ctx = build_authz_context("GET", "/sovd/v1/apps", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "app");
        assert_eq!(ctx.resource, "collection");
    }

    #[test]
    fn authz_context_funcs() {
        let ctx = build_authz_context("GET", "/sovd/v1/funcs", "u", &[], &[]);
        assert_eq!(ctx.entity_type, "func");
        assert_eq!(ctx.resource, "collection");
    }

    #[test]
    fn authz_context_x_uds_prefix() {
        let ctx = build_authz_context(
            "POST",
            "/sovd/v1/x-uds/components/hpc/connect",
            "u",
            &[],
            &[],
        );
        assert_eq!(ctx.entity_type, "component");
        assert_eq!(ctx.entity_id.as_deref(), Some("hpc"));
        assert_eq!(ctx.resource, "connect");
    }

    #[test]
    fn authz_context_lock() {
        let ctx = build_authz_context("POST", "/sovd/v1/components/hpc/lock", "u", &[], &[]);
        assert_eq!(ctx.resource, "lock");
        assert!(ctx.resource_id.is_none());
    }

    #[test]
    fn authz_context_deep_path() {
        let ctx = build_authz_context(
            "GET",
            "/sovd/v1/components/hpc/operations/flash/executions/exec-1",
            "u",
            &[],
            &[],
        );
        assert_eq!(ctx.resource, "operations");
        assert_eq!(ctx.resource_id.as_deref(), Some("flash"));
    }

    // ── perform_authz tests ───────────────────────────────────────────────

    /// A test policy that denies all non-GET requests for the "reader" role.
    struct ReadOnlyPolicy;
    impl AuthzPolicy for ReadOnlyPolicy {
        fn authorize(&self, ctx: &AuthzContext) -> AuthzDecision {
            if ctx.roles.contains(&"reader".to_owned()) && ctx.method != "GET" {
                return AuthzDecision::Deny {
                    status: 403,
                    code: "TEST-ERR-403".into(),
                    message: "Read-only role".into(),
                };
            }
            AuthzDecision::Allow
        }
    }

    fn test_audit_log() -> AuditLog {
        AuditLog::new()
    }

    #[test]
    fn perform_authz_allows_default_profile() {
        let profile = native_interfaces::oem::DefaultProfile;
        let log = test_audit_log();
        let result = perform_authz(
            &profile,
            "DELETE",
            "/sovd/v1/components/hpc/faults",
            "admin",
            &["admin".into()],
            &[],
            &log,
        );
        assert!(result.is_ok());
        assert!(log.is_empty()); // no denial → no audit entry
    }

    #[test]
    fn perform_authz_denies_read_only() {
        let policy = ReadOnlyPolicy;
        let log = test_audit_log();
        let result = perform_authz(
            &policy,
            "DELETE",
            "/sovd/v1/components/hpc/faults",
            "viewer",
            &["reader".into()],
            &[],
            &log,
        );
        assert!(result.is_err());
        assert_eq!(log.len(), 1); // denial recorded
        let entries = log.recent(1);
        assert_eq!(entries[0].action, SovdAuditAction::AuthzDenied);
    }

    #[test]
    fn perform_authz_allows_read_only_get() {
        let policy = ReadOnlyPolicy;
        let log = test_audit_log();
        let result = perform_authz(
            &policy,
            "GET",
            "/sovd/v1/components/hpc/data",
            "viewer",
            &["reader".into()],
            &[],
            &log,
        );
        assert!(result.is_ok());
    }

    // ── extract_scopes tests ──────────────────────────────────────────────

    #[test]
    fn extract_scopes_from_space_separated() {
        let claims = Claims {
            sub: "u".into(),
            exp: 0,
            iat: 0,
            iss: None,
            roles: vec![],
            vin: None,
            scope: Some("read write admin".into()),
            tenant_id: None,
        };
        assert_eq!(extract_scopes(&claims), vec!["read", "write", "admin"]);
    }

    #[test]
    fn extract_scopes_empty_when_none() {
        let claims = Claims {
            sub: "u".into(),
            exp: 0,
            iat: 0,
            iss: None,
            roles: vec![],
            vin: None,
            scope: None,
            tenant_id: None,
        };
        assert!(extract_scopes(&claims).is_empty());
    }

    // ── tenant context injection tests ──────────────────────────────────

    #[test]
    fn tenant_context_from_jwt_claim() {
        let claims = Claims {
            sub: "user1".into(),
            exp: 0,
            iat: 0,
            iss: None,
            roles: vec![],
            vin: None,
            scope: None,
            tenant_id: Some("workshop-a".into()),
        };
        let ctx = claims
            .tenant_id
            .as_deref()
            .map_or_else(TenantContext::default, TenantContext::new);
        assert_eq!(ctx.tenant_id, "workshop-a");
        assert!(!ctx.is_default());
    }

    #[test]
    fn tenant_context_default_when_no_claim() {
        let claims = Claims {
            sub: "user1".into(),
            exp: 0,
            iat: 0,
            iss: None,
            roles: vec![],
            vin: None,
            scope: None,
            tenant_id: None,
        };
        let ctx = claims
            .tenant_id
            .as_deref()
            .map_or_else(TenantContext::default, TenantContext::new);
        assert!(ctx.is_default());
    }
}
