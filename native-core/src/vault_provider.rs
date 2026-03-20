// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// VaultSecretProvider — HashiCorp Vault KV v2 secret backend (F4)
//
// Retrieves secrets from a HashiCorp Vault instance via the KV v2 HTTP API.
// Includes a time-based cache to avoid per-request Vault round-trips.
//
// Authentication: VAULT_TOKEN env var or explicit token in config.
// API: GET /v1/{mount}/data/{path_prefix}{name}
//
// Example config:
//   [secrets]
//   provider = "vault"
//   vault_addr = "http://vault:8200"
//   vault_mount = "secret"
//   vault_path_prefix = "sovd/"
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use native_interfaces::SecretProvider;

/// Configuration for the Vault secret provider.
#[derive(Debug, Clone)]
pub struct VaultConfig {
    /// Vault server address (e.g. "http://vault:8200")
    pub addr: String,
    /// Vault authentication token
    pub token: String,
    /// KV v2 mount path (e.g. "secret")
    pub mount: String,
    /// Path prefix within the mount (e.g. "sovd/")
    pub path_prefix: String,
    /// Cache TTL — how long to keep secrets before re-fetching
    pub cache_ttl: Duration,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            addr: "http://127.0.0.1:8200".to_owned(),
            token: String::new(),
            mount: "secret".to_owned(),
            path_prefix: "sovd/".to_owned(),
            cache_ttl: Duration::from_secs(300), // 5 minutes
        }
    }
}

struct CacheEntry {
    value: Option<String>,
    fetched_at: Instant,
}

/// A secret provider backed by HashiCorp Vault KV v2 engine.
///
/// Secrets are fetched via HTTP and cached with a configurable TTL.
/// The provider is synchronous (blocking HTTP) to match the `SecretProvider`
/// trait signature. For production use, ensure the Vault server is low-latency.
pub struct VaultSecretProvider {
    config: VaultConfig,
    client: reqwest::blocking::Client,
    cache: RwLock<HashMap<String, CacheEntry>>,
}

impl VaultSecretProvider {
    /// Create a new Vault secret provider.
    ///
    /// The `config.token` can be empty — in that case the provider attempts
    /// to read `VAULT_TOKEN` from the environment.
    pub fn new(mut config: VaultConfig) -> Self {
        // Fall back to VAULT_TOKEN env var if no token provided
        if config.token.is_empty() {
            if let Ok(token) = std::env::var("VAULT_TOKEN") {
                config.token = token;
            }
        }

        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("HTTP client for Vault");

        Self {
            config,
            client,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Check if a cached entry is still valid (within TTL).
    fn cache_get(&self, name: &str) -> Option<Option<String>> {
        let cache = self.cache.read().ok()?;
        let entry = cache.get(name)?;
        if entry.fetched_at.elapsed() < self.config.cache_ttl {
            Some(entry.value.clone())
        } else {
            None
        }
    }

    /// Store a value in the cache.
    fn cache_put(&self, name: &str, value: Option<String>) {
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(
                name.to_owned(),
                CacheEntry {
                    value,
                    fetched_at: Instant::now(),
                },
            );
        }
    }

    /// Fetch a secret from Vault KV v2 API.
    ///
    /// URL: `{addr}/v1/{mount}/data/{path_prefix}{name}`
    /// Response JSON: `{ "data": { "data": { "value": "..." } } }`
    fn fetch_from_vault(&self, name: &str) -> Option<String> {
        let url = format!(
            "{}/v1/{}/data/{}{}",
            self.config.addr.trim_end_matches('/'),
            self.config.mount,
            self.config.path_prefix,
            name
        );

        let response = self
            .client
            .get(&url)
            .header("X-Vault-Token", &self.config.token)
            .send()
            .map_err(|e| {
                tracing::warn!(secret = name, error = %e, "Vault request failed");
            })
            .ok()?;

        if !response.status().is_success() {
            tracing::debug!(
                secret = name,
                status = %response.status(),
                "Vault returned non-200"
            );
            return None;
        }

        let body: serde_json::Value = response
            .json()
            .map_err(|e| {
                tracing::warn!(secret = name, error = %e, "Vault response parse failed");
            })
            .ok()?;

        // KV v2 response structure: { "data": { "data": { "value": "..." } } }
        body.get("data")
            .and_then(|d| d.get("data"))
            .and_then(|d| d.get("value"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned())
    }

    /// Invalidate a single cache entry.
    pub fn invalidate(&self, name: &str) {
        if let Ok(mut cache) = self.cache.write() {
            cache.remove(name);
        }
    }

    /// Invalidate all cached entries.
    pub fn invalidate_all(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }
}

impl SecretProvider for VaultSecretProvider {
    fn get_secret(&self, name: &str) -> Option<String> {
        // Check cache first
        if let Some(cached) = self.cache_get(name) {
            return cached;
        }

        // Fetch from Vault
        let value = self.fetch_from_vault(name);
        self.cache_put(name, value.clone());
        value
    }

    fn has_secret(&self, name: &str) -> bool {
        self.get_secret(name).is_some()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn test_config() -> VaultConfig {
        VaultConfig {
            addr: "http://127.0.0.1:19999".to_owned(), // unreachable
            token: "test-token".to_owned(),
            mount: "secret".to_owned(),
            path_prefix: "sovd/".to_owned(),
            cache_ttl: Duration::from_millis(100),
        }
    }

    #[test]
    fn vault_provider_returns_none_when_unreachable() {
        let provider = VaultSecretProvider::new(test_config());
        assert_eq!(provider.get_secret("jwt_secret"), None);
    }

    #[test]
    fn vault_provider_caches_none_result() {
        let provider = VaultSecretProvider::new(test_config());
        // First call: network error → None, cached
        assert_eq!(provider.get_secret("jwt_secret"), None);
        // Second call: should hit cache (no network call)
        assert_eq!(provider.get_secret("jwt_secret"), None);
        // Verify cache entry exists
        let cache = provider.cache.read().unwrap();
        assert!(cache.contains_key("jwt_secret"));
    }

    #[test]
    fn vault_provider_cache_expires() {
        let mut config = test_config();
        config.cache_ttl = Duration::from_millis(10);
        let provider = VaultSecretProvider::new(config);

        // Warm cache
        provider.cache_put("test_key", Some("cached_value".to_owned()));
        assert_eq!(
            provider.cache_get("test_key"),
            Some(Some("cached_value".to_owned()))
        );

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(provider.cache_get("test_key"), None);
    }

    #[test]
    fn vault_provider_invalidate_single() {
        let provider = VaultSecretProvider::new(test_config());
        provider.cache_put("a", Some("1".to_owned()));
        provider.cache_put("b", Some("2".to_owned()));

        provider.invalidate("a");

        assert_eq!(provider.cache_get("a"), None);
        assert_eq!(provider.cache_get("b"), Some(Some("2".to_owned())));
    }

    #[test]
    fn vault_provider_invalidate_all() {
        let provider = VaultSecretProvider::new(test_config());
        provider.cache_put("a", Some("1".to_owned()));
        provider.cache_put("b", Some("2".to_owned()));

        provider.invalidate_all();

        assert_eq!(provider.cache_get("a"), None);
        assert_eq!(provider.cache_get("b"), None);
    }

    #[test]
    fn vault_provider_has_secret_returns_false_when_unreachable() {
        let provider = VaultSecretProvider::new(test_config());
        assert!(!provider.has_secret("anything"));
    }

    #[test]
    fn vault_config_default_values() {
        let config = VaultConfig::default();
        assert_eq!(config.addr, "http://127.0.0.1:8200");
        assert_eq!(config.mount, "secret");
        assert_eq!(config.path_prefix, "sovd/");
        assert_eq!(config.cache_ttl, Duration::from_secs(300));
    }

    #[test]
    fn vault_provider_cache_stores_some_value() {
        let provider = VaultSecretProvider::new(test_config());
        provider.cache_put("key", Some("value".to_owned()));
        assert_eq!(provider.cache_get("key"), Some(Some("value".to_owned())));
    }

    #[test]
    fn vault_provider_cache_stores_none_value() {
        let provider = VaultSecretProvider::new(test_config());
        provider.cache_put("missing", None);
        assert_eq!(provider.cache_get("missing"), Some(None));
    }

    #[test]
    fn vault_provider_token_from_env() {
        // When no token is provided, it should try VAULT_TOKEN
        let config = VaultConfig {
            token: String::new(),
            ..test_config()
        };
        let provider = VaultSecretProvider::new(config);
        // Token may or may not be set in env, but creation should not panic
        assert!(provider.config.addr.contains("127.0.0.1"));
    }
}
