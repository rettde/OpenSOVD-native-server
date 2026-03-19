// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Secrets abstraction layer (A2.3)
//
// Abstracts how the server retrieves sensitive values (JWT signing keys,
// API keys, TLS passwords, bearer tokens). Implementations:
//   - EnvSecretProvider   (reads from environment variables — default)
//   - StaticSecretProvider (hardcoded values — for tests only)
//   - Future: HashiCorp Vault, AWS Secrets Manager, Azure Key Vault, etc.
//
// The trait is intentionally simple: get_secret(name) → Option<String>.
// Callers provide a logical name (e.g. "jwt_secret"); the provider maps
// it to the concrete source.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

/// A provider of secret values.
///
/// Implementations should never log or display secret values.
pub trait SecretProvider: Send + Sync + 'static {
    /// Retrieve a secret by logical name. Returns `None` if the secret
    /// is not configured or not available.
    fn get_secret(&self, name: &str) -> Option<String>;

    /// Check whether a secret exists without retrieving its value.
    fn has_secret(&self, name: &str) -> bool {
        self.get_secret(name).is_some()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// EnvSecretProvider — reads secrets from environment variables
// ─────────────────────────────────────────────────────────────────────────────

/// Reads secrets from environment variables with an optional prefix.
///
/// Example: with prefix `"SOVD_"`, `get_secret("jwt_secret")` reads
/// `SOVD_JWT_SECRET` from the environment.
pub struct EnvSecretProvider {
    /// Optional prefix prepended to the uppercased secret name
    prefix: String,
}

impl EnvSecretProvider {
    /// Create a new provider with the given prefix (e.g. `"SOVD_"`).
    pub fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.to_owned(),
        }
    }

    /// Create a provider with no prefix.
    pub fn no_prefix() -> Self {
        Self {
            prefix: String::new(),
        }
    }

    /// Build the environment variable name for a logical secret name.
    fn env_var_name(&self, name: &str) -> String {
        format!("{}{}", self.prefix, name.to_uppercase())
    }
}

impl Default for EnvSecretProvider {
    fn default() -> Self {
        Self::new("SOVD_")
    }
}

impl SecretProvider for EnvSecretProvider {
    fn get_secret(&self, name: &str) -> Option<String> {
        let var_name = self.env_var_name(name);
        std::env::var(&var_name).ok()
    }

    fn has_secret(&self, name: &str) -> bool {
        let var_name = self.env_var_name(name);
        std::env::var_os(&var_name).is_some()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// StaticSecretProvider — hardcoded values (tests / demos only)
// ─────────────────────────────────────────────────────────────────────────────

/// A secret provider backed by a fixed `HashMap`. For tests and demos only.
pub struct StaticSecretProvider {
    secrets: HashMap<String, String>,
}

impl StaticSecretProvider {
    /// Create a provider from a list of (name, value) pairs.
    pub fn new(secrets: Vec<(&str, &str)>) -> Self {
        Self {
            secrets: secrets
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
        }
    }

    /// Create an empty provider (no secrets available).
    pub fn empty() -> Self {
        Self {
            secrets: HashMap::new(),
        }
    }
}

impl SecretProvider for StaticSecretProvider {
    fn get_secret(&self, name: &str) -> Option<String> {
        self.secrets.get(name).cloned()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn static_provider_returns_configured_secrets() {
        let provider = StaticSecretProvider::new(vec![
            ("jwt_secret", "super-secret-key"),
            ("api_key", "key-12345"),
        ]);
        assert_eq!(
            provider.get_secret("jwt_secret"),
            Some("super-secret-key".to_owned())
        );
        assert_eq!(provider.get_secret("api_key"), Some("key-12345".to_owned()));
    }

    #[test]
    fn static_provider_returns_none_for_missing() {
        let provider = StaticSecretProvider::new(vec![("a", "1")]);
        assert_eq!(provider.get_secret("missing"), None);
    }

    #[test]
    fn static_provider_has_secret() {
        let provider = StaticSecretProvider::new(vec![("a", "1")]);
        assert!(provider.has_secret("a"));
        assert!(!provider.has_secret("b"));
    }

    #[test]
    fn empty_static_provider() {
        let provider = StaticSecretProvider::empty();
        assert_eq!(provider.get_secret("anything"), None);
    }

    #[test]
    fn env_provider_builds_correct_var_name() {
        let provider = EnvSecretProvider::new("SOVD_");
        assert_eq!(provider.env_var_name("jwt_secret"), "SOVD_JWT_SECRET");
        assert_eq!(provider.env_var_name("api_key"), "SOVD_API_KEY");
    }

    #[test]
    fn env_provider_no_prefix() {
        let provider = EnvSecretProvider::no_prefix();
        assert_eq!(provider.env_var_name("jwt_secret"), "JWT_SECRET");
    }

    #[test]
    fn env_provider_returns_none_for_unset_var() {
        let provider = EnvSecretProvider::new("SOVD_TEST_NONEXISTENT_");
        assert_eq!(provider.get_secret("some_key"), None);
        assert!(!provider.has_secret("some_key"));
    }

    #[test]
    fn default_env_provider_uses_sovd_prefix() {
        let provider = EnvSecretProvider::default();
        assert_eq!(provider.env_var_name("test"), "SOVD_TEST");
    }
}
