// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// OEM Profile — Plugin interface for vendor-specific SOVD customization
//
// Architecture inspired by CDA's SecurityPlugin trait hierarchy
// (cda-plugin-security/src/lib.rs):
//
//   CDA pattern:
//     SecurityPlugin: Any + SecurityApi + AuthApi
//       └→ SecurityPluginInitializer  (request-scoped init)
//       └→ SecurityPluginLoader       (combines init + auth + Default)
//
//   OpenSOVD pattern:
//     OemProfile: AuthPolicy + AuthzPolicy + EntityIdPolicy + DiscoveryPolicy + CdfPolicy
//       └→ DefaultProfile  (standard SOVD — permissive, no OEM restrictions)
//       └→ MbdsProfile     (Mercedes-Benz — DDAG IDs, VIN binding, scope ceiling)
//
// The profile is injected as Arc<dyn OemProfile> into AppState and shared
// across all axum handlers and middleware. Unlike CDA's per-request
// SecurityPlugin initialization, OemProfile is application-scoped (singleton)
// because OEM rules don't change per request.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

/// HTTP status code (re-exported to avoid axum/http dependency in interfaces crate)
pub type HttpStatusCode = u16;

// ── Sub-trait: Authentication & Authorization Policy ─────────────────────

/// OEM-specific authentication and authorization rules.
///
/// Analogous to CDA's `SecurityApi::validate_service()` but for SOVD-level
/// JWT claim enforcement rather than diagnostic service authorization.
pub trait AuthPolicy: Send + Sync {
    /// HTTP status code returned when a JWT token is structurally valid
    /// but fails OEM-specific validation (e.g. wrong VIN, exceeded scope).
    ///
    /// - Standard SOVD / RFC 9110: `401 Unauthorized`
    /// - MBDS S-SOVD §6.3:        `403 Forbidden`
    fn invalid_token_status(&self) -> HttpStatusCode {
        401
    }

    /// SOVD error code returned alongside `invalid_token_status`.
    fn invalid_token_error_code(&self) -> &'static str {
        "SOVD-ERR-401"
    }

    /// Validate OEM-specific JWT claims after standard JWT verification.
    ///
    /// Called with the deserialized claim map. Return `Ok(())` to accept,
    /// or `Err((http_status, error_code, message))` to reject.
    ///
    /// Default: accept all tokens (no OEM-specific claim rules).
    fn validate_claims(
        &self,
        _claims: &HashMap<String, serde_json::Value>,
        _request_path: &str,
    ) -> Result<(), (HttpStatusCode, String, String)> {
        Ok(())
    }

    /// Allowed OAuth2 scopes. Empty slice = no scope enforcement.
    ///
    /// MBDS §6.2 defines: `["After_Sales_BASIC", "After_Sales_ENHANCED"]`
    fn allowed_scopes(&self) -> &[&str] {
        &[]
    }
}

// ── Sub-trait: Entity ID Validation Policy ───────────────────────────────

/// OEM-specific entity identifier validation rules.
///
/// The standard SOVD spec does not define entity ID syntax.
/// MBDS §2.3 mandates DDAG naming rules (1-64 chars, alphanumeric + hyphen/underscore).
pub trait EntityIdPolicy: Send + Sync {
    /// Validate an entity identifier extracted from a URL path parameter.
    ///
    /// Return `Ok(())` to accept, `Err(reason)` to reject with 400 Bad Request.
    ///
    /// Default: accept any non-empty string (standard SOVD behavior).
    fn validate_entity_id(&self, id: &str) -> Result<(), String> {
        if id.is_empty() {
            return Err("Entity ID must not be empty".to_owned());
        }
        Ok(())
    }
}

// ── Sub-trait: Discovery Policy (allowed entity types) ───────────────────

/// Controls which SOVD entity types the server exposes.
///
/// ISO 17978-3 defines: SOVDServer, Component, App, Function, Area.
/// MBDS §2.2 **forbids** Area.
pub trait DiscoveryPolicy: Send + Sync {
    /// Whether the `/areas` entity collection is exposed.
    ///
    /// Default: `true` (standard SOVD allows Areas).
    /// MBDS override: `false`.
    fn areas_enabled(&self) -> bool {
        true
    }

    /// Whether the `/funcs` entity collection is exposed.
    fn funcs_enabled(&self) -> bool {
        true
    }

    /// Whether the `/apps` entity collection is exposed.
    fn apps_enabled(&self) -> bool {
        true
    }
}

// ── Sub-trait: Capability Description File (CDF) Policy ──────────────────

/// OEM-specific OpenAPI / CDF extension values.
///
/// Controls `x-sovd-*` extension fields in the OpenAPI spec.
/// Standard SOVD defines these extensions but doesn't mandate specific values.
/// MBDS erzwingt bestimmte Werte.
pub trait CdfPolicy: Send + Sync {
    /// `x-sovd-applicability` in the CDF info block.
    ///
    /// Default: `{"online": true, "offline": false}`
    fn applicability(&self) -> CdfApplicability {
        CdfApplicability {
            online: true,
            offline: false,
        }
    }

    /// Default `x-sovd-unit` for data resources without explicit unit.
    fn default_data_unit(&self) -> &'static str {
        "unspecified"
    }

    /// Whether operations require proximity proof by default.
    fn default_proximity_proof_required(&self) -> bool {
        false
    }
}

/// CDF applicability descriptor (online/offline capability)
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct CdfApplicability {
    pub online: bool,
    pub offline: bool,
}

// ── Sub-trait: Fine-Grained Authorization Policy (Wave 1) ─────────────

/// Context for fine-grained authorization decisions.
///
/// Built from the HTTP request after successful authentication.
/// Contains semantic fields parsed from the matched route template
/// so that `AuthzPolicy` implementations can make decisions based on
/// entity type, resource, and action — not just the raw URL path.
#[derive(Debug, Clone)]
pub struct AuthzContext {
    /// Authenticated caller identity (from JWT `sub` / API key label)
    pub caller: String,
    /// Parsed JWT roles (from `roles` claim), empty if API key auth
    pub roles: Vec<String>,
    /// OAuth2 scopes (from `scope` / `scp` claim), empty if not present
    pub scopes: Vec<String>,
    /// HTTP method: "GET", "POST", "PUT", "DELETE", "PATCH"
    pub method: String,
    /// Entity type being accessed: "component", "app", "func", "group", "server"
    pub entity_type: String,
    /// Entity ID (e.g. component_id, app_id), None for collection endpoints
    pub entity_id: Option<String>,
    /// Resource being accessed: "data", "faults", "operations", "configurations",
    ///   "software-packages", "lock", "mode", "logs", "proximity-challenge",
    ///   "capabilities", "discovery", "audit"
    pub resource: String,
    /// Sub-resource ID (e.g. data_id, fault_id, op_id), None for collections
    pub resource_id: Option<String>,
    /// Full original request path (for fallback / logging)
    pub path: String,
}

/// Result of an authorization decision.
#[derive(Debug, Clone)]
pub enum AuthzDecision {
    /// Allow the request to proceed.
    Allow,
    /// Deny the request with an HTTP status code and SOVD error.
    Deny {
        status: HttpStatusCode,
        code: String,
        message: String,
    },
}

/// OEM-specific fine-grained authorization rules (Wave 1).
///
/// Called **after** successful authentication (API key / JWT / OIDC) to decide
/// whether the authenticated caller is allowed to perform the specific action
/// on the specific resource.  This enables per-resource, per-operation, and
/// per-entity authorization policies beyond token-level validation.
///
/// Default: allow everything (standard SOVD — no restrictions).
pub trait AuthzPolicy: Send + Sync {
    /// Authorize a request after authentication has succeeded.
    ///
    /// Return `AuthzDecision::Allow` to proceed, or `AuthzDecision::Deny { .. }`
    /// to reject with the given HTTP status and SOVD error code.
    ///
    /// Default: allow all requests (permissive, standard SOVD behavior).
    fn authorize(&self, _ctx: &AuthzContext) -> AuthzDecision {
        AuthzDecision::Allow
    }
}

// ── Main trait: OEM Profile (combines all sub-traits) ────────────────────

/// Complete OEM customization profile for the SOVD server.
///
/// Combines authentication policy, entity ID validation, discovery rules,
/// and CDF extensions into a single injectable unit.
///
/// # Architecture (inspired by CDA SecurityPlugin)
///
/// ```text
/// ┌───────────────────────────────────────────────────────────┐
/// │                       OemProfile                          │
/// │  ┌─────────────┐ ┌───────────────┐ ┌─────────────┐       │
/// │  │ AuthPolicy   │ │EntityIdPolicy │ │CdfPolicy    │       │
/// │  │ • 401 vs 403 │ │• DDAG rules   │ │• x-sovd-*   │       │
/// │  │ • VIN check  │ │• max length   │ │• offline    │       │
/// │  │ • scope ceil │ │               │ │• unit       │       │
/// │  └──────────────┘ └───────────────┘ └─────────────┘       │
/// │  ┌─────────────────────────────────────────────────┐       │
/// │  │            DiscoveryPolicy                      │       │
/// │  │  • areas_enabled  • funcs_enabled               │       │
/// │  └─────────────────────────────────────────────────┘       │
/// │  ┌─────────────────────────────────────────────────┐       │
/// │  │            AuthzPolicy (Wave 1)                 │       │
/// │  │  • authorize(ctx) → Allow / Deny                │       │
/// │  │  • per-resource, per-operation, per-entity      │       │
/// │  └─────────────────────────────────────────────────┘       │
/// └───────────────────────────────────────────────────────────┘
///           ▲                          ▲
///     DefaultProfile             MbdsProfile
///    (standard SOVD)        (Mercedes-Benz S-SOVD)
/// ```
///
/// Injected as `Arc<dyn OemProfile>` into `AppState`.
pub trait OemProfile: AuthPolicy + AuthzPolicy + EntityIdPolicy + DiscoveryPolicy + CdfPolicy + Send + Sync {
    /// Human-readable profile name for logging and diagnostics.
    fn name(&self) -> &'static str;

    /// Short identifier used in configuration files (e.g. "default", "mbds").
    fn id(&self) -> &'static str;

    /// Upcast helpers (analogous to CDA's `as_auth_plugin()` / `as_security_plugin()`)
    fn as_auth_policy(&self) -> &dyn AuthPolicy;
    fn as_authz_policy(&self) -> &dyn AuthzPolicy;
    fn as_entity_id_policy(&self) -> &dyn EntityIdPolicy;
    fn as_discovery_policy(&self) -> &dyn DiscoveryPolicy;
    fn as_cdf_policy(&self) -> &dyn CdfPolicy;
}

// ── Default Profile (standard SOVD — no OEM restrictions) ────────────────

/// Standard SOVD profile with no OEM-specific restrictions.
///
/// - Accepts all JWT tokens (no VIN/scope enforcement)
/// - Allows all requests (no fine-grained authorization)
/// - Allows all entity types (including Area)
/// - Permissive entity ID validation (any non-empty string)
/// - Standard CDF extensions (online-only, no unit, no proximity proof)
///
/// This is the baseline behavior when no OEM profile is configured.
#[derive(Debug, Clone, Default)]
pub struct DefaultProfile;

impl AuthPolicy for DefaultProfile {}
impl AuthzPolicy for DefaultProfile {}
impl EntityIdPolicy for DefaultProfile {}
impl DiscoveryPolicy for DefaultProfile {}
impl CdfPolicy for DefaultProfile {}

impl OemProfile for DefaultProfile {
    fn name(&self) -> &'static str {
        "Standard SOVD (ISO 17978-3)"
    }

    fn id(&self) -> &'static str {
        "default"
    }

    fn as_auth_policy(&self) -> &dyn AuthPolicy {
        self
    }
    fn as_authz_policy(&self) -> &dyn AuthzPolicy {
        self
    }
    fn as_entity_id_policy(&self) -> &dyn EntityIdPolicy {
        self
    }
    fn as_discovery_policy(&self) -> &dyn DiscoveryPolicy {
        self
    }
    fn as_cdf_policy(&self) -> &dyn CdfPolicy {
        self
    }
}
