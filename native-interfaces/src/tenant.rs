// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// TenantContext — Multi-tenant isolation context (Wave 3, A3.3)
//
// Extracted from JWT claims or HTTP headers. Injected into request extensions
// by the tenant_context_middleware. Downstream handlers and storage operations
// use the tenant_id as a namespace prefix for data isolation.
//
// Isolation levels (ADR A3.2):
//   None      → single-tenant (backward compatible, default)
//   Namespace → shared state with tenant-prefixed keys
//   Strict    → separate state per tenant (future)
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

/// Isolation level for multi-tenant deployments (ADR A3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TenantIsolation {
    /// No isolation — single-tenant deployment (backward compatible)
    #[default]
    None,
    /// Namespace isolation — shared state with tenant-prefixed keys
    Namespace,
    /// Strict isolation — separate state instances per tenant (future)
    Strict,
}

/// Tenant context extracted from the authenticated request.
///
/// Injected into axum request extensions by `tenant_context_middleware`.
/// Handlers access it via the `TenantId` extractor or directly from extensions.
///
/// # Default
///
/// The default tenant context represents a single-tenant deployment:
/// `tenant_id = "default"`, `isolation = None`. This ensures backward
/// compatibility — existing single-tenant deployments work without any
/// tenant configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    /// Unique tenant identifier (from JWT `tenant_id` claim or `X-Tenant-ID` header)
    pub tenant_id: String,
    /// Human-readable tenant name (optional, for logging)
    pub tenant_name: Option<String>,
    /// Isolation level for this tenant
    pub isolation: TenantIsolation,
}

impl Default for TenantContext {
    fn default() -> Self {
        Self {
            tenant_id: "default".to_owned(),
            tenant_name: None,
            isolation: TenantIsolation::None,
        }
    }
}

impl TenantContext {
    /// Create a new tenant context with namespace isolation.
    pub fn new(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            tenant_name: None,
            isolation: TenantIsolation::Namespace,
        }
    }

    /// Create a tenant context with a display name.
    pub fn with_name(tenant_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            tenant_name: Some(name.into()),
            isolation: TenantIsolation::Namespace,
        }
    }

    /// Whether this is the default single-tenant context.
    pub fn is_default(&self) -> bool {
        self.tenant_id == "default" && self.isolation == TenantIsolation::None
    }

    /// Build a tenant-scoped storage key.
    ///
    /// For `TenantIsolation::None`, returns the key unchanged.
    /// For `Namespace` / `Strict`, prefixes with `{tenant_id}:`.
    ///
    /// # Examples
    /// ```
    /// use native_interfaces::tenant::{TenantContext, TenantIsolation};
    ///
    /// let default_ctx = TenantContext::default();
    /// assert_eq!(default_ctx.scoped_key("hpc-main:fault-1"), "hpc-main:fault-1");
    ///
    /// let tenant_ctx = TenantContext::new("workshop-a");
    /// assert_eq!(tenant_ctx.scoped_key("hpc-main:fault-1"), "workshop-a:hpc-main:fault-1");
    /// ```
    pub fn scoped_key(&self, key: &str) -> String {
        match self.isolation {
            TenantIsolation::None => key.to_owned(),
            TenantIsolation::Namespace | TenantIsolation::Strict => {
                format!("{}:{key}", self.tenant_id)
            }
        }
    }

    /// Display label for logging (tenant_id or tenant_name if available).
    pub fn display_name(&self) -> &str {
        self.tenant_name.as_deref().unwrap_or(&self.tenant_id)
    }
}

// ── Tenant configuration (for config file) ──────────────────────────────────

/// Per-tenant configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantConfig {
    /// Tenant ID (must match JWT `tenant_id` claim or `X-Tenant-ID` header)
    pub id: String,
    /// Human-readable name
    #[serde(default)]
    pub name: Option<String>,
    /// OEM profile to use for this tenant (e.g. "default", "mbds")
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Isolation level
    #[serde(default)]
    pub isolation: TenantIsolation,
}

fn default_profile() -> String {
    "default".to_owned()
}

/// Top-level multi-tenant configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTenantConfig {
    /// Enable multi-tenant mode (default: false = single-tenant)
    #[serde(default)]
    pub enabled: bool,
    /// Header name for tenant ID extraction (default: "X-Tenant-ID")
    #[serde(default = "default_tenant_header")]
    pub header_name: String,
    /// JWT claim name for tenant ID (default: "tenant_id")
    #[serde(default = "default_tenant_claim")]
    pub jwt_claim: String,
    /// Registered tenants (empty = accept any tenant ID from JWT/header)
    #[serde(default)]
    pub tenants: Vec<TenantConfig>,
}

impl Default for MultiTenantConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            header_name: default_tenant_header(),
            jwt_claim: default_tenant_claim(),
            tenants: Vec::new(),
        }
    }
}

fn default_tenant_header() -> String {
    "X-Tenant-ID".to_owned()
}

fn default_tenant_claim() -> String {
    "tenant_id".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tenant_is_single_tenant() {
        let ctx = TenantContext::default();
        assert_eq!(ctx.tenant_id, "default");
        assert!(ctx.is_default());
        assert_eq!(ctx.isolation, TenantIsolation::None);
    }

    #[test]
    fn scoped_key_no_isolation() {
        let ctx = TenantContext::default();
        assert_eq!(ctx.scoped_key("hpc-main:fault-1"), "hpc-main:fault-1");
    }

    #[test]
    fn scoped_key_namespace_isolation() {
        let ctx = TenantContext::new("workshop-a");
        assert_eq!(
            ctx.scoped_key("hpc-main:fault-1"),
            "workshop-a:hpc-main:fault-1"
        );
    }

    #[test]
    fn scoped_key_strict_isolation() {
        let ctx = TenantContext {
            tenant_id: "oem-x".to_owned(),
            tenant_name: Some("OEM X".to_owned()),
            isolation: TenantIsolation::Strict,
        };
        assert_eq!(ctx.scoped_key("lock-key"), "oem-x:lock-key");
    }

    #[test]
    fn display_name_prefers_tenant_name() {
        let ctx = TenantContext::with_name("ws-a", "Workshop Alpha");
        assert_eq!(ctx.display_name(), "Workshop Alpha");
    }

    #[test]
    fn display_name_falls_back_to_id() {
        let ctx = TenantContext::new("ws-a");
        assert_eq!(ctx.display_name(), "ws-a");
    }

    #[test]
    fn tenant_config_defaults() {
        let config = MultiTenantConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.header_name, "X-Tenant-ID");
        assert_eq!(config.jwt_claim, "tenant_id");
        assert!(config.tenants.is_empty());
    }

    #[test]
    fn tenant_isolation_serde_roundtrip() {
        let json = serde_json::to_string(&TenantIsolation::Namespace).expect("serialize");
        assert_eq!(json, "\"namespace\"");
        let parsed: TenantIsolation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, TenantIsolation::Namespace);
    }
}
