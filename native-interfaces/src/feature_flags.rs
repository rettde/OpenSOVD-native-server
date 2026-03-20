// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Feature Flags (E2.4) — Runtime feature toggle
//
// Thread-safe, lock-free feature flag registry that supports:
//   - Compile-time defaults
//   - Config-file overrides
//   - Runtime toggles via admin REST API
//
// Flags are stored as AtomicBool values keyed by &'static str names.
// All reads are lock-free (Ordering::Relaxed — eventual consistency is fine
// for feature flags, and the common path is a single atomic load).
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Well-known feature flag names.
///
/// Using constants avoids typos and enables IDE autocompletion.
pub mod flags {
    /// Enable rate limiting middleware
    pub const RATE_LIMIT: &str = "rate_limit";
    /// Enable authentication middleware
    pub const AUTH: &str = "auth";
    /// Enable audit trail recording
    pub const AUDIT: &str = "audit";
    /// Enable historical diagnostic storage (W2.2)
    pub const HISTORY: &str = "history";
    /// Enable cloud bridge endpoints (W3.1)
    pub const BRIDGE: &str = "bridge";
    /// Enable multi-tenant isolation (W3.2)
    pub const MULTI_TENANT: &str = "multi_tenant";
    /// Enable SSE data-change stream (W4.5)
    pub const SSE_STREAM: &str = "sse_stream";
    /// Enable mDNS discovery broadcast
    pub const MDNS: &str = "mdns";
    /// Enable Prometheus metrics endpoint
    pub const METRICS: &str = "metrics";
    /// Enable vendor extensions (x-uds routes)
    pub const VENDOR_EXTENSIONS: &str = "vendor_extensions";
}

/// A single feature flag with atomic state.
struct Flag {
    enabled: AtomicBool,
    description: &'static str,
}

/// Thread-safe feature flag registry.
///
/// All flag reads are lock-free atomic loads — zero overhead in the hot path.
pub struct FeatureFlags {
    registry: HashMap<&'static str, Flag>,
}

/// Serializable snapshot of all flags (for REST API responses).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagSnapshot {
    pub name: String,
    pub enabled: bool,
    pub description: String,
}

/// Configuration for feature flags (loaded from config file).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeatureFlagConfig {
    /// Override map: flag_name → enabled
    #[serde(default)]
    pub overrides: HashMap<String, bool>,
}

impl FeatureFlags {
    /// Create a new feature flag registry with sensible defaults.
    ///
    /// All flags default to `true` (enabled) — the toggle is for *disabling*
    /// features at runtime when needed (circuit-breaker pattern).
    pub fn new() -> Self {
        let mut registry = HashMap::new();

        macro_rules! register {
            ($name:expr, $default:expr, $desc:expr) => {
                registry.insert(
                    $name,
                    Flag {
                        enabled: AtomicBool::new($default),
                        description: $desc,
                    },
                );
            };
        }

        register!(flags::RATE_LIMIT, true, "Rate limiting middleware");
        register!(flags::AUTH, true, "Authentication middleware");
        register!(flags::AUDIT, true, "Audit trail recording");
        register!(flags::HISTORY, true, "Historical diagnostic storage (W2.2)");
        register!(flags::BRIDGE, true, "Cloud bridge endpoints (W3.1)");
        register!(flags::MULTI_TENANT, true, "Multi-tenant isolation (W3.2)");
        register!(flags::SSE_STREAM, true, "SSE data-change stream (W4.5)");
        register!(flags::MDNS, true, "mDNS discovery broadcast");
        register!(flags::METRICS, true, "Prometheus metrics endpoint");
        register!(
            flags::VENDOR_EXTENSIONS,
            true,
            "Vendor extensions (x-uds routes)"
        );

        Self { registry }
    }

    /// Create from configuration, applying overrides on top of defaults.
    pub fn from_config(config: &FeatureFlagConfig) -> Self {
        let flags = Self::new();
        for (name, enabled) in &config.overrides {
            if let Some(flag) = flags.registry.get(name.as_str()) {
                flag.enabled.store(*enabled, Ordering::Relaxed);
            }
        }
        flags
    }

    /// Check if a feature flag is enabled.
    ///
    /// Returns `true` if the flag is enabled or if the flag name is unknown
    /// (fail-open for unknown flags — safer than silently disabling features).
    #[inline]
    pub fn is_enabled(&self, name: &str) -> bool {
        self.registry
            .get(name)
            .is_none_or(|f| f.enabled.load(Ordering::Relaxed))
    }

    /// Set a feature flag at runtime.
    ///
    /// Returns `true` if the flag was found and updated, `false` if unknown.
    pub fn set(&self, name: &str, enabled: bool) -> bool {
        if let Some(flag) = self.registry.get(name) {
            flag.enabled.store(enabled, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Toggle a feature flag (flip its current value).
    ///
    /// Returns the new value, or `None` if the flag is unknown.
    pub fn toggle(&self, name: &str) -> Option<bool> {
        self.registry.get(name).map(|flag| {
            let old = flag.enabled.load(Ordering::Relaxed);
            let new_val = !old;
            flag.enabled.store(new_val, Ordering::Relaxed);
            new_val
        })
    }

    /// Get a snapshot of all flags (for serialization / REST API).
    pub fn snapshot(&self) -> Vec<FlagSnapshot> {
        let mut flags: Vec<_> = self
            .registry
            .iter()
            .map(|(name, flag)| FlagSnapshot {
                name: (*name).to_owned(),
                enabled: flag.enabled.load(Ordering::Relaxed),
                description: flag.description.to_owned(),
            })
            .collect();
        flags.sort_by(|a, b| a.name.cmp(&b.name));
        flags
    }

    /// Get a single flag's snapshot, or `None` if unknown.
    pub fn get(&self, name: &str) -> Option<FlagSnapshot> {
        self.registry.get(name).map(|flag| FlagSnapshot {
            name: name.to_owned(),
            enabled: flag.enabled.load(Ordering::Relaxed),
            description: flag.description.to_owned(),
        })
    }

    /// List all known flag names.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<_> = self.registry.keys().copied().collect();
        names.sort_unstable();
        names
    }
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared feature flags handle (cheaply cloneable).
pub type SharedFeatureFlags = Arc<FeatureFlags>;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn default_flags_are_enabled() {
        let ff = FeatureFlags::new();
        assert!(ff.is_enabled(flags::RATE_LIMIT));
        assert!(ff.is_enabled(flags::AUTH));
        assert!(ff.is_enabled(flags::AUDIT));
        assert!(ff.is_enabled(flags::HISTORY));
        assert!(ff.is_enabled(flags::BRIDGE));
        assert!(ff.is_enabled(flags::MULTI_TENANT));
        assert!(ff.is_enabled(flags::SSE_STREAM));
        assert!(ff.is_enabled(flags::MDNS));
        assert!(ff.is_enabled(flags::METRICS));
        assert!(ff.is_enabled(flags::VENDOR_EXTENSIONS));
    }

    #[test]
    fn unknown_flag_returns_true() {
        let ff = FeatureFlags::new();
        assert!(ff.is_enabled("nonexistent_flag"));
    }

    #[test]
    fn set_flag() {
        let ff = FeatureFlags::new();
        assert!(ff.is_enabled(flags::RATE_LIMIT));
        assert!(ff.set(flags::RATE_LIMIT, false));
        assert!(!ff.is_enabled(flags::RATE_LIMIT));
        assert!(ff.set(flags::RATE_LIMIT, true));
        assert!(ff.is_enabled(flags::RATE_LIMIT));
    }

    #[test]
    fn set_unknown_flag_returns_false() {
        let ff = FeatureFlags::new();
        assert!(!ff.set("nonexistent", true));
    }

    #[test]
    fn toggle_flag() {
        let ff = FeatureFlags::new();
        assert!(ff.is_enabled(flags::AUTH));
        let new_val = ff.toggle(flags::AUTH);
        assert_eq!(new_val, Some(false));
        assert!(!ff.is_enabled(flags::AUTH));
        let new_val = ff.toggle(flags::AUTH);
        assert_eq!(new_val, Some(true));
        assert!(ff.is_enabled(flags::AUTH));
    }

    #[test]
    fn toggle_unknown_returns_none() {
        let ff = FeatureFlags::new();
        assert_eq!(ff.toggle("nonexistent"), None);
    }

    #[test]
    fn snapshot_contains_all_flags() {
        let ff = FeatureFlags::new();
        let snap = ff.snapshot();
        assert_eq!(snap.len(), 10);
        assert!(snap.iter().all(|f| f.enabled));
    }

    #[test]
    fn snapshot_reflects_changes() {
        let ff = FeatureFlags::new();
        ff.set(flags::AUDIT, false);
        let snap = ff.snapshot();
        let audit = snap.iter().find(|f| f.name == flags::AUDIT).unwrap();
        assert!(!audit.enabled);
    }

    #[test]
    fn get_single_flag() {
        let ff = FeatureFlags::new();
        let flag = ff.get(flags::METRICS).unwrap();
        assert!(flag.enabled);
        assert_eq!(flag.name, "metrics");
    }

    #[test]
    fn get_unknown_returns_none() {
        let ff = FeatureFlags::new();
        assert!(ff.get("nonexistent").is_none());
    }

    #[test]
    fn from_config_applies_overrides() {
        let config = FeatureFlagConfig {
            overrides: [
                ("rate_limit".to_owned(), false),
                ("auth".to_owned(), false),
            ]
            .into_iter()
            .collect(),
        };
        let ff = FeatureFlags::from_config(&config);
        assert!(!ff.is_enabled(flags::RATE_LIMIT));
        assert!(!ff.is_enabled(flags::AUTH));
        assert!(ff.is_enabled(flags::AUDIT)); // not overridden
    }

    #[test]
    fn from_config_ignores_unknown_overrides() {
        let config = FeatureFlagConfig {
            overrides: [("unknown_flag".to_owned(), false)].into_iter().collect(),
        };
        let ff = FeatureFlags::from_config(&config);
        // Should not panic, unknown flags ignored
        assert!(ff.is_enabled(flags::AUTH));
    }

    #[test]
    fn names_returns_sorted_list() {
        let ff = FeatureFlags::new();
        let names = ff.names();
        assert_eq!(names.len(), 10);
        // Verify sorted
        for i in 1..names.len() {
            assert!(names[i - 1] <= names[i]);
        }
    }

    #[test]
    fn concurrent_access_is_safe() {
        let ff = Arc::new(FeatureFlags::new());
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let ff = ff.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        if i % 2 == 0 {
                            ff.set(flags::RATE_LIMIT, false);
                        } else {
                            ff.set(flags::RATE_LIMIT, true);
                        }
                        let _ = ff.is_enabled(flags::RATE_LIMIT);
                        let _ = ff.snapshot();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // Just verifying no panics or data races
    }
}
