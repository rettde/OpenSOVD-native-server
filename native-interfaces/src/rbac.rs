// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// RBAC — Role-Based Access Control (F10)
//
// Configurable role-to-permission mapping that plugs into the OemProfile's
// AuthzPolicy. Three built-in roles (admin, operator, reader) with a
// permission matrix keyed by HTTP method × resource type.
//
// OEM profiles can either:
//   (a) use RbacPolicy directly by composing it into their AuthzPolicy::authorize()
//   (b) ignore it entirely and implement their own authz logic
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::{HashMap, HashSet};

use crate::oem::{AuthzContext, AuthzDecision};

/// A named role with a set of allowed actions.
///
/// An action is a `(method, resource_pattern)` pair where:
///   - `method` is "GET", "POST", "PUT", "DELETE", "PATCH", or "*" (any)
///   - `resource_pattern` is a resource name ("data", "faults", etc.) or "*" (any)
#[derive(Debug, Clone)]
pub struct RbacRole {
    pub name: String,
    pub permissions: HashSet<(String, String)>,
}

impl RbacRole {
    /// Check if this role allows the given method on the given resource.
    pub fn allows(&self, method: &str, resource: &str) -> bool {
        self.permissions.contains(&("*".to_owned(), "*".to_owned()))
            || self.permissions.contains(&(method.to_owned(), "*".to_owned()))
            || self.permissions.contains(&("*".to_owned(), resource.to_owned()))
            || self.permissions.contains(&(method.to_owned(), resource.to_owned()))
    }
}

/// Configurable RBAC policy with named roles and a permission matrix.
///
/// # Built-in roles (created by `RbacPolicy::default()`)
///
/// | Role | Permissions |
/// |------|------------|
/// | `admin` | `*` on `*` (full access) |
/// | `operator` | GET, POST, PUT on all resources |
/// | `reader` | GET on all resources |
#[derive(Debug, Clone)]
pub struct RbacPolicy {
    roles: HashMap<String, RbacRole>,
    /// Whether RBAC enforcement is enabled. When `false`, all requests are allowed.
    pub enabled: bool,
    /// Default role assigned when a caller has no roles in their token.
    /// Set to `None` to deny requests without roles.
    pub default_role: Option<String>,
}

impl Default for RbacPolicy {
    fn default() -> Self {
        let mut policy = Self {
            roles: HashMap::new(),
            enabled: true,
            default_role: Some("reader".to_owned()),
        };

        // admin: full access
        policy.add_role(RbacRole {
            name: "admin".to_owned(),
            permissions: [("*".to_owned(), "*".to_owned())].into_iter().collect(),
        });

        // operator: read + write + execute (no DELETE on audit, lock override)
        let operator_perms: HashSet<(String, String)> = [
            ("GET", "*"),
            ("POST", "*"),
            ("PUT", "*"),
            ("PATCH", "*"),
        ]
        .iter()
        .map(|(m, r)| ((*m).to_owned(), (*r).to_owned()))
        .collect();
        policy.add_role(RbacRole {
            name: "operator".to_owned(),
            permissions: operator_perms,
        });

        // reader: read-only
        policy.add_role(RbacRole {
            name: "reader".to_owned(),
            permissions: [("GET".to_owned(), "*".to_owned())].into_iter().collect(),
        });

        policy
    }
}

impl RbacPolicy {
    /// Create a new empty RBAC policy (no roles defined, enforcement enabled).
    pub fn new() -> Self {
        Self {
            roles: HashMap::new(),
            enabled: true,
            default_role: None,
        }
    }

    /// Add or replace a role definition.
    pub fn add_role(&mut self, role: RbacRole) {
        self.roles.insert(role.name.clone(), role);
    }

    /// Check whether the given context is authorized by any of the caller's roles.
    pub fn check(&self, ctx: &AuthzContext) -> AuthzDecision {
        if !self.enabled {
            return AuthzDecision::Allow;
        }

        // Collect effective roles: from token claims + optional default
        let mut effective_roles: Vec<&str> = ctx.roles.iter().map(String::as_str).collect();
        if effective_roles.is_empty() {
            if let Some(ref default) = self.default_role {
                effective_roles.push(default);
            }
        }

        if effective_roles.is_empty() {
            return AuthzDecision::Deny {
                status: 403,
                code: "SOVD-ERR-RBAC-001".to_owned(),
                message: "No roles assigned and no default role configured".to_owned(),
            };
        }

        // Check if any role permits this action
        for role_name in &effective_roles {
            if let Some(role) = self.roles.get(*role_name) {
                if role.allows(&ctx.method, &ctx.resource) {
                    return AuthzDecision::Allow;
                }
            }
            // Unknown roles are silently ignored (principle of least privilege)
        }

        AuthzDecision::Deny {
            status: 403,
            code: "SOVD-ERR-RBAC-002".to_owned(),
            message: format!(
                "None of the roles [{}] permit {} on '{}'",
                effective_roles.join(", "),
                ctx.method,
                ctx.resource,
            ),
        }
    }
}

/// Serializable RBAC configuration (for config files / env vars).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RbacConfig {
    /// Enable RBAC enforcement (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Default role for callers without explicit roles (default: "reader")
    #[serde(default)]
    pub default_role: Option<String>,
    /// Custom role definitions. If empty, built-in roles (admin, operator, reader) are used.
    #[serde(default)]
    pub custom_roles: Vec<RbacRoleConfig>,
}

fn default_true() -> bool {
    true
}

impl Default for RbacConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_role: Some("reader".to_owned()),
            custom_roles: vec![],
        }
    }
}

/// Serializable role definition for configuration files.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RbacRoleConfig {
    pub name: String,
    /// Permissions as list of "METHOD:resource" strings, e.g. "GET:*", "POST:faults", "*:*"
    pub permissions: Vec<String>,
}

impl From<RbacConfig> for RbacPolicy {
    fn from(config: RbacConfig) -> Self {
        let mut policy = if config.custom_roles.is_empty() {
            Self::default()
        } else {
            let mut p = Self::new();
            for rc in &config.custom_roles {
                let perms: HashSet<(String, String)> = rc
                    .permissions
                    .iter()
                    .filter_map(|s| {
                        let parts: Vec<&str> = s.splitn(2, ':').collect();
                        if parts.len() == 2 {
                            Some((parts[0].to_owned(), parts[1].to_owned()))
                        } else {
                            None
                        }
                    })
                    .collect();
                p.add_role(RbacRole {
                    name: rc.name.clone(),
                    permissions: perms,
                });
            }
            p
        };
        policy.enabled = config.enabled;
        policy.default_role = config.default_role;
        policy
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn ctx(method: &str, resource: &str, roles: &[&str]) -> AuthzContext {
        AuthzContext {
            caller: "test-user".to_owned(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            scopes: vec![],
            method: method.to_owned(),
            entity_type: "component".to_owned(),
            entity_id: Some("hpc".to_owned()),
            resource: resource.to_owned(),
            resource_id: None,
            path: format!("/sovd/v1/components/hpc/{resource}"),
        }
    }

    #[test]
    fn admin_allows_everything() {
        let policy = RbacPolicy::default();
        assert!(matches!(
            policy.check(&ctx("GET", "data", &["admin"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("DELETE", "faults", &["admin"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("POST", "software-packages", &["admin"])),
            AuthzDecision::Allow
        ));
    }

    #[test]
    fn reader_allows_get_only() {
        let policy = RbacPolicy::default();
        assert!(matches!(
            policy.check(&ctx("GET", "data", &["reader"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("GET", "faults", &["reader"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("POST", "data", &["reader"])),
            AuthzDecision::Deny { .. }
        ));
        assert!(matches!(
            policy.check(&ctx("DELETE", "faults", &["reader"])),
            AuthzDecision::Deny { .. }
        ));
    }

    #[test]
    fn operator_allows_read_write_not_delete() {
        let policy = RbacPolicy::default();
        assert!(matches!(
            policy.check(&ctx("GET", "data", &["operator"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("POST", "operations", &["operator"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("PUT", "configurations", &["operator"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("DELETE", "faults", &["operator"])),
            AuthzDecision::Deny { .. }
        ));
    }

    #[test]
    fn no_roles_uses_default() {
        let policy = RbacPolicy::default(); // default_role = "reader"
        assert!(matches!(
            policy.check(&ctx("GET", "data", &[])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("POST", "data", &[])),
            AuthzDecision::Deny { .. }
        ));
    }

    #[test]
    fn no_roles_no_default_denies() {
        let mut policy = RbacPolicy::default();
        policy.default_role = None;
        assert!(matches!(
            policy.check(&ctx("GET", "data", &[])),
            AuthzDecision::Deny { .. }
        ));
    }

    #[test]
    fn disabled_allows_everything() {
        let mut policy = RbacPolicy::default();
        policy.enabled = false;
        assert!(matches!(
            policy.check(&ctx("DELETE", "faults", &[])),
            AuthzDecision::Allow
        ));
    }

    #[test]
    fn highest_role_wins() {
        let policy = RbacPolicy::default();
        // reader + admin → admin wins, DELETE allowed
        assert!(matches!(
            policy.check(&ctx("DELETE", "faults", &["reader", "admin"])),
            AuthzDecision::Allow
        ));
    }

    #[test]
    fn unknown_role_ignored() {
        let policy = RbacPolicy::default();
        assert!(matches!(
            policy.check(&ctx("POST", "data", &["unknown-role"])),
            AuthzDecision::Deny { .. }
        ));
    }

    #[test]
    fn config_round_trip() {
        let config = RbacConfig {
            enabled: true,
            default_role: None,
            custom_roles: vec![RbacRoleConfig {
                name: "custom".to_owned(),
                permissions: vec!["GET:data".to_owned(), "POST:operations".to_owned()],
            }],
        };
        let policy: RbacPolicy = config.into();
        assert!(matches!(
            policy.check(&ctx("GET", "data", &["custom"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("POST", "operations", &["custom"])),
            AuthzDecision::Allow
        ));
        assert!(matches!(
            policy.check(&ctx("DELETE", "data", &["custom"])),
            AuthzDecision::Deny { .. }
        ));
    }
}
