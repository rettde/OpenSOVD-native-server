// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// OEM Sample Profile — Template for vendor-specific SOVD customizations
//
// This file serves as a **reference implementation and template** for OEMs who
// want to adapt the OpenSOVD-native-server to their proprietary diagnostic
// standards. It demonstrates every extension point in the `OemProfile` trait
// hierarchy with extensive inline documentation.
//
// ┌──────────────────────────────────────────────────────────────────────────┐
// │ HOW TO CREATE YOUR OWN OEM PROFILE                                      │
// │                                                                          │
// │ 1. Copy this file to `oem_<your_oem>.rs` (e.g. `oem_acme.rs`)          │
// │ 2. Rename `SampleOemProfile` → `AcmeProfile`                           │
// │ 3. Implement each sub-trait to match your OEM diagnostic specification  │
// │ 4. Register the module in `lib.rs` (optionally behind a feature flag)   │
// │ 5. Inject your profile in `main.rs`:                                    │
// │       let profile: Arc<dyn OemProfile> = Arc::new(AcmeProfile::new()); │
// │ 6. Add `oem_acme.rs` to `.gitignore` if it is proprietary              │
// └──────────────────────────────────────────────────────────────────────────┘
//
// The OemProfile trait hierarchy (defined in native-interfaces/src/oem.rs):
//
//   OemProfile
//     ├── AuthPolicy         — Authentication & authorization rules
//     ├── EntityIdPolicy     — Entity identifier validation rules
//     ├── DiscoveryPolicy    — Which SOVD entity types are exposed
//     └── CdfPolicy          — Capability Description File (OpenAPI) extensions
//
// Each sub-trait has sensible defaults (standard SOVD behavior). You only need
// to override the methods that your OEM specification requires.
// ─────────────────────────────────────────────────────────────────────────────

use native_interfaces::oem::{
    AuthPolicy, CdfPolicy, DiscoveryPolicy, EntityIdPolicy, OemProfile,
};

// ═══════════════════════════════════════════════════════════════════════════════
// SAMPLE OEM PROFILE — demonstrates all extension points
// ═══════════════════════════════════════════════════════════════════════════════

/// Sample OEM profile that shows all available customization points.
///
/// This profile uses standard SOVD (ISO 17978-3) defaults everywhere.
/// In a real OEM profile, you would override specific methods to match
/// your diagnostic specification.
///
/// # Example: Creating a custom profile
///
/// ```rust
/// use native_interfaces::oem::*;
///
/// #[derive(Debug, Clone)]
/// pub struct AcmeProfile {
///     pub required_vin: Option<String>,
/// }
///
/// impl AuthPolicy for AcmeProfile {
///     fn invalid_token_status(&self) -> HttpStatusCode {
///         403 // Your OEM may require 403 instead of 401
///     }
/// }
/// // ... implement other sub-traits ...
/// ```
#[derive(Debug, Clone, Default)]
pub struct SampleOemProfile;

// ─────────────────────────────────────────────────────────────────────────────
// AuthPolicy — Authentication & Authorization
// ─────────────────────────────────────────────────────────────────────────────
//
// This sub-trait controls how the SOVD server handles JWT token validation
// and OEM-specific authorization rules.
//
// CUSTOMIZATION POINTS:
//
//   ┌─────────────────────────────┬────────────────────────────────────────────┐
//   │ Method                      │ What you can customize                     │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ invalid_token_status()      │ HTTP status for invalid/expired tokens.    │
//   │                             │ Standard: 401 Unauthorized (RFC 9110)      │
//   │                             │ Some OEMs require 403 Forbidden instead.   │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ invalid_token_error_code()  │ SOVD error code in the JSON error body.   │
//   │                             │ Standard: "SOVD-ERR-401"                  │
//   │                             │ Match to your OEM's error catalog.        │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ validate_claims()           │ Validate OEM-specific JWT claims AFTER     │
//   │                             │ standard JWT signature/expiry checks.      │
//   │                             │ Examples:                                  │
//   │                             │   • VIN binding (token must match vehicle) │
//   │                             │   • Scope ceiling (max permission level)   │
//   │                             │   • Region restrictions (geo-fencing)      │
//   │                             │   • Workshop ID validation                 │
//   │                             │   • Token-to-certificate binding           │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ allowed_scopes()            │ List of permitted OAuth2 scopes.           │
//   │                             │ Empty = no scope enforcement.              │
//   │                             │ Examples: "diagnostic_basic",              │
//   │                             │   "diagnostic_enhanced", "factory_mode"    │
//   └─────────────────────────────┴────────────────────────────────────────────┘

impl AuthPolicy for SampleOemProfile {
    // ── EXAMPLE: Override invalid token status ──────────────────────────
    //
    // fn invalid_token_status(&self) -> HttpStatusCode {
    //     // Return 403 Forbidden instead of 401 Unauthorized
    //     // when a JWT is structurally valid but fails OEM-specific checks.
    //     403
    // }

    // ── EXAMPLE: Override error code ────────────────────────────────────
    //
    // fn invalid_token_error_code(&self) -> &'static str {
    //     "SOVD-ERR-403"  // Must match your OEM's error catalog
    // }

    // ── EXAMPLE: Validate OEM-specific JWT claims ──────────────────────
    //
    // fn validate_claims(
    //     &self,
    //     claims: &std::collections::HashMap<String, serde_json::Value>,
    //     request_path: &str,
    // ) -> Result<(), (HttpStatusCode, String, String)> {
    //
    //     // ── VIN binding ────────────────────────────────────────────
    //     // Ensure the JWT was issued for THIS specific vehicle.
    //     // The "vin" claim must match the vehicle's VIN.
    //     //
    //     // if let Some(ref required_vin) = self.required_vin {
    //     //     let token_vin = claims.get("vin")
    //     //         .and_then(|v| v.as_str())
    //     //         .unwrap_or("");
    //     //     if token_vin != required_vin {
    //     //         return Err((403, "SOVD-ERR-403".into(),
    //     //             "Token VIN does not match this vehicle".into()));
    //     //     }
    //     // }
    //
    //     // ── Scope ceiling ─────────────────────────────────────────
    //     // Reject tokens that exceed the maximum allowed permission level.
    //     //
    //     // let scope_str = claims.get("scope")
    //     //     .and_then(|v| v.as_str())
    //     //     .unwrap_or("");
    //     // for s in scope_str.split_whitespace() {
    //     //     if !self.allowed_scopes.contains(&s) {
    //     //         return Err((403, "SOVD-ERR-403".into(),
    //     //             format!("Scope '{}' not permitted", s)));
    //     //     }
    //     // }
    //
    //     // ── Region restriction ────────────────────────────────────
    //     // Some OEMs restrict diagnostic access to specific regions.
    //     //
    //     // let region = claims.get("region")
    //     //     .and_then(|v| v.as_str())
    //     //     .unwrap_or("unknown");
    //     // if !self.allowed_regions.contains(&region) {
    //     //     return Err((403, "SOVD-ERR-403".into(),
    //     //         format!("Region '{}' not authorized", region)));
    //     // }
    //
    //     // ── Workshop ID ───────────────────────────────────────────
    //     // Require a valid workshop identifier in the token.
    //     //
    //     // if claims.get("workshop_id").is_none() {
    //     //     return Err((403, "SOVD-ERR-403".into(),
    //     //         "Missing workshop_id claim".into()));
    //     // }
    //
    //     Ok(())
    // }

    // ── EXAMPLE: Restrict OAuth2 scopes ─────────────────────────────────
    //
    // fn allowed_scopes(&self) -> &[&str] {
    //     &["diagnostic_basic", "diagnostic_enhanced"]
    // }
}

// ─────────────────────────────────────────────────────────────────────────────
// EntityIdPolicy — Entity Identifier Validation
// ─────────────────────────────────────────────────────────────────────────────
//
// Controls how entity IDs (component names, data IDs, operation IDs, etc.)
// are validated before being used in API operations.
//
// CUSTOMIZATION POINTS:
//
//   ┌─────────────────────────────┬────────────────────────────────────────────┐
//   │ Method                      │ What you can customize                     │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ validate_entity_id()        │ Validate entity ID format/characters.      │
//   │                             │ Standard: any non-empty string (permissive)│
//   │                             │ Examples:                                  │
//   │                             │   • Max length (e.g. 64 chars)             │
//   │                             │   • Allowed character set (alphanumeric)   │
//   │                             │   • No leading/trailing special chars      │
//   │                             │   • Regex pattern matching                 │
//   │                             │   • Hierarchical ID format (e.g. "a.b.c") │
//   └─────────────────────────────┴────────────────────────────────────────────┘

impl EntityIdPolicy for SampleOemProfile {
    // ── EXAMPLE: Strict entity ID validation ────────────────────────────
    //
    // fn validate_entity_id(&self, id: &str) -> Result<(), String> {
    //     // Length check
    //     if id.is_empty() || id.len() > 64 {
    //         return Err(format!("Entity ID '{}' must be 1-64 characters", id));
    //     }
    //
    //     // Character set: alphanumeric + hyphens + underscores only
    //     if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
    //         return Err(format!(
    //             "Entity ID '{}' contains invalid characters (allowed: a-z, A-Z, 0-9, -, _)",
    //             id
    //         ));
    //     }
    //
    //     // No leading/trailing hyphens
    //     if id.starts_with('-') || id.ends_with('-') {
    //         return Err(format!(
    //             "Entity ID '{}' must not start or end with a hyphen", id
    //         ));
    //     }
    //
    //     // ── Hierarchical IDs ──────────────────────────────────────
    //     // Some OEMs use dot-separated hierarchical IDs:
    //     //   "body.lighting.headlamp_left"
    //     //
    //     // if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_') {
    //     //     return Err("Hierarchical ID must use alphanumeric + dots + underscores".into());
    //     // }
    //
    //     Ok(())
    // }
}

// ─────────────────────────────────────────────────────────────────────────────
// DiscoveryPolicy — Entity Type Exposure
// ─────────────────────────────────────────────────────────────────────────────
//
// Controls which SOVD entity types are exposed in the discovery API.
// This affects what endpoints are available.
//
// CUSTOMIZATION POINTS:
//
//   ┌─────────────────────────────┬────────────────────────────────────────────┐
//   │ Method                      │ What you can customize                     │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ areas_enabled()             │ Whether /areas endpoint is available.      │
//   │                             │ Standard: true                             │
//   │                             │ Some OEMs forbid the Area entity type.     │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ funcs_enabled()             │ Whether /funcs endpoint is available.      │
//   │                             │ Standard: true                             │
//   ├─────────────────────────────┼────────────────────────────────────────────┤
//   │ apps_enabled()              │ Whether /apps endpoint is available.       │
//   │                             │ Standard: true                             │
//   └─────────────────────────────┴────────────────────────────────────────────┘
//
//   ADDITIONAL IDEAS FOR OEM CUSTOMIZATION:
//
//   • Hide certain component categories based on user role
//   • Restrict discovery to only ECUs the workshop is authorized for
//   • Filter components based on vehicle variant/configuration
//   • Expose custom entity types specific to your OEM architecture

impl DiscoveryPolicy for SampleOemProfile {
    // ── EXAMPLE: Disable the /areas endpoint ────────────────────────────
    //
    // fn areas_enabled(&self) -> bool {
    //     false  // Your OEM may not support the Area entity type
    // }
}

// ─────────────────────────────────────────────────────────────────────────────
// CdfPolicy — Capability Description File (OpenAPI) Extensions
// ─────────────────────────────────────────────────────────────────────────────
//
// Controls OEM-specific extensions in the SOVD Capability Description File
// (CDF), which is the OpenAPI 3.1 spec served at /openapi.json.
//
// These extensions are defined in ASAM SOVD V1.1.0 §5 and allow OEMs to
// declare additional metadata about their diagnostic capabilities.
//
// CUSTOMIZATION POINTS:
//
//   ┌──────────────────────────────────┬────────────────────────────────────────┐
//   │ Method                           │ What you can customize                 │
//   ├──────────────────────────────────┼────────────────────────────────────────┤
//   │ applicability()                  │ x-sovd-applicability extension.        │
//   │                                  │ Declares online/offline capability.    │
//   │                                  │ Standard: {online: true, offline: false│
//   │                                  │ If your OEM supports offline diag:     │
//   │                                  │   {online: true, offline: true}        │
//   ├──────────────────────────────────┼────────────────────────────────────────┤
//   │ default_data_unit()              │ x-sovd-unit extension.                 │
//   │                                  │ Default unit for data resources.       │
//   │                                  │ Standard: "unspecified"                │
//   │                                  │ Examples: "raw", "SI", "imperial",     │
//   │                                  │   "engineering_unit"                   │
//   ├──────────────────────────────────┼────────────────────────────────────────┤
//   │ default_proximity_proof_required │ x-sovd-proximity-proof-required.       │
//   │                                  │ Whether operations need proof that the │
//   │                                  │ diagnostic tool is physically near.    │
//   │                                  │ Standard: false                        │
//   │                                  │ Security-critical OEMs may set true.   │
//   └──────────────────────────────────┴────────────────────────────────────────┘
//
//   ADDITIONAL IDEAS FOR CDF EXTENSIONS:
//
//   • Custom `x-oem-variant` extension for vehicle variant metadata
//   • Custom `x-oem-ecu-generation` for ECU hardware generation info
//   • Custom `x-oem-flash-protocol` for supported flashing protocols
//   • Custom `x-oem-min-tester-version` for minimum tool version requirements

impl CdfPolicy for SampleOemProfile {
    // ── EXAMPLE: Declare offline diagnostic capability ──────────────────
    //
    // fn applicability(&self) -> CdfApplicability {
    //     CdfApplicability {
    //         online: true,
    //         offline: true,  // Server supports offline diagnostic sessions
    //     }
    // }

    // ── EXAMPLE: Set default data unit ──────────────────────────────────
    //
    // fn default_data_unit(&self) -> &'static str {
    //     "raw"  // All data resources use raw byte encoding by default
    // }

    // ── EXAMPLE: Require proximity proof for operations ─────────────────
    //
    // fn default_proximity_proof_required(&self) -> bool {
    //     true  // Security-critical: tool must prove physical proximity
    // }
}

// ─────────────────────────────────────────────────────────────────────────────
// OemProfile — Combine all sub-traits
// ─────────────────────────────────────────────────────────────────────────────
//
// This is the main trait that ties everything together. It is injected as
// `Arc<dyn OemProfile>` into the application's shared state (AppState).
//
// Every route handler, middleware, and the OpenAPI builder can access the
// active OEM profile through the state.
//
// INJECTION ARCHITECTURE:
//
//   main.rs:
//     let profile: Arc<dyn OemProfile> = Arc::new(SampleOemProfile);
//
//   AppState.oem_profile ──┬── entity_id_validation_middleware (EntityIdPolicy)
//                          ├── auth_middleware via AuthState    (AuthPolicy)
//                          ├── serve_docs / openapi builder    (CdfPolicy)
//                          └── route handlers                  (DiscoveryPolicy)

impl OemProfile for SampleOemProfile {
    fn name(&self) -> &'static str {
        "Sample OEM Profile (standard SOVD)"
    }

    fn id(&self) -> &'static str {
        "sample"
    }

    fn as_auth_policy(&self) -> &dyn AuthPolicy {
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

// ═══════════════════════════════════════════════════════════════════════════════
// TESTS — Verify the sample profile uses standard SOVD defaults
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use native_interfaces::oem::{AuthPolicy, CdfPolicy, DiscoveryPolicy, EntityIdPolicy};

    #[test]
    fn sample_profile_uses_standard_defaults() {
        let p = SampleOemProfile;

        // AuthPolicy defaults
        assert_eq!(p.invalid_token_status(), 401, "Standard: 401 Unauthorized");
        assert_eq!(p.invalid_token_error_code(), "SOVD-ERR-401");

        // EntityIdPolicy: permissive (accepts anything)
        assert!(p.validate_entity_id("any-id").is_ok());
        assert!(p.validate_entity_id("with spaces!@#").is_ok());

        // DiscoveryPolicy: all entity types enabled
        assert!(p.areas_enabled());
        assert!(p.funcs_enabled());
        assert!(p.apps_enabled());

        // CdfPolicy: online only, no unit, no proximity proof
        let app = p.applicability();
        assert!(app.online);
        assert!(!app.offline, "Standard: offline not declared");
        assert_eq!(p.default_data_unit(), "unspecified");
        assert!(!p.default_proximity_proof_required());
    }

    #[test]
    fn sample_profile_metadata() {
        let p = SampleOemProfile;
        assert_eq!(p.name(), "Sample OEM Profile (standard SOVD)");
        assert_eq!(p.id(), "sample");
    }
}
