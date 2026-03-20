// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// opensovd-native-server — Main binary
//
// Architecture:
//   Client → SOVD Server → ComponentRouter (Gateway)
//                               └── SovdHttpBackend → external CDA / SOVD backends
// ─────────────────────────────────────────────────────────────────────────────

mod tls_reload;

use std::sync::Arc;

use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::Deserialize;
use tracing::info;
use tracing_subscriber::{fmt, EnvFilter};

use dashmap::DashMap;
use native_core::{
    AuditLog, ComponentRouter, DiagLog, FaultBridge, FaultManager, HistoryConfig, HistoryService,
    LockManager, SovdHttpBackend, SovdHttpBackendConfig,
};
use native_health::HealthMonitor;
use native_interfaces::bridge::BridgeConfig;
use native_interfaces::tenant::MultiTenantConfig;
use native_interfaces::ComponentBackend;
use native_sovd::{
    build_router, AppState, AuthConfig, BridgeState, DltConfig, DltTextLayer,
    InMemoryBridgeTransport, MdnsConfig, MdnsHandle,
};

use native_comm_someip::{SomeIpConfig, SomeIpRuntime};

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AppConfig {
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    logging: LoggingConfig,
    #[serde(default)]
    auth: AuthConfig,
    #[serde(default)]
    someip: SomeIpConfig,
    #[serde(default)]
    mdns: MdnsConfig,
    #[serde(default)]
    dlt: DltConfig,
    #[serde(default)]
    rate_limit: native_sovd::RateLimitConfig,
    /// Cloud bridge configuration (Wave 3, A3.1)
    #[serde(default)]
    bridge: BridgeConfig,
    /// Multi-tenant configuration (Wave 3, A3.2)
    #[serde(default)]
    tenant: MultiTenantConfig,
    /// Persistent storage configuration (F1)
    #[serde(default)]
    storage: StorageConfig,
    /// Prometheus metrics endpoint configuration (F7)
    #[serde(default)]
    metrics: MetricsConfig,
    /// Secret provider configuration (F4)
    #[serde(default)]
    secrets: SecretsConfig,
    /// Firmware signature verification (F12, ISO 24089)
    #[serde(default)]
    firmware: FirmwareConfig,

    // ── Backend configuration ───────────────────────────────────────────
    /// External SOVD backends (CDA instances, native SOVD endpoints)
    #[serde(default)]
    backends: Vec<SovdHttpBackendConfig>,
}

#[derive(Debug, Deserialize)]
struct ServerConfig {
    host: String,
    port: u16,
    /// Path to TLS certificate file (PEM). When both `cert_path` and `key_path` are set, HTTPS is enabled.
    #[serde(default)]
    cert_path: Option<String>,
    /// Path to TLS private key file (PEM)
    #[serde(default)]
    key_path: Option<String>,
    /// Path to CA certificate file (PEM) for mutual TLS client verification.
    /// When set alongside cert_path/key_path, the server requires client certificates
    /// signed by this CA (MBDS S-SOVD §6.3).
    #[serde(default)]
    client_ca_path: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_owned(),
            port: 8080,
            cert_path: None,
            key_path: None,
            client_ca_path: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct LoggingConfig {
    level: String,
    /// Log output format: "text" (default, human-readable) or "json" (structured, SIEM-ready)
    #[serde(default = "LoggingConfig::default_format")]
    format: String,
    /// Optional OTLP endpoint for OpenTelemetry trace export (e.g. "http://localhost:4317").
    /// Requires the `otlp` feature flag. Ignored when the feature is not enabled.
    #[serde(default)]
    otlp_endpoint: Option<String>,
}

impl LoggingConfig {
    fn default_format() -> String {
        "text".to_owned()
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: Self::default_format(),
            otlp_endpoint: None,
        }
    }
}

/// Persistent storage backend selection (F1).
///
/// ```toml
/// [storage]
/// backend = "sled"                    # "memory" (default) | "sled"
/// sled_path = "./data/sovd.sled"      # only used when backend = "sled"
/// ```
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields read behind `persist` feature gate
struct StorageConfig {
    /// Storage backend: "memory" (default, volatile) or "sled" (persistent, requires `persist` feature).
    #[serde(default = "StorageConfig::default_backend")]
    backend: String,
    /// Path to sled database directory (only used when backend = "sled").
    #[serde(default = "StorageConfig::default_sled_path")]
    sled_path: String,
}

impl StorageConfig {
    fn default_backend() -> String {
        "memory".to_owned()
    }
    fn default_sled_path() -> String {
        "./data/sovd.sled".to_owned()
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: Self::default_backend(),
            sled_path: Self::default_sled_path(),
        }
    }
}

/// Secret provider configuration (F4).
///
/// ```toml
/// [secrets]
/// provider = "vault"                     # "env" (default) | "vault" | "static"
/// vault_addr = "http://vault:8200"
/// vault_mount = "secret"
/// vault_path_prefix = "sovd/"
/// vault_cache_ttl_secs = 300
/// ```
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // Fields read behind `vault` feature gate
struct SecretsConfig {
    /// Secret provider: "env" (default), "vault" (requires `vault` feature), or "static".
    #[serde(default = "SecretsConfig::default_provider")]
    provider: String,
    /// Vault server address (only used when provider = "vault").
    #[serde(default)]
    vault_addr: Option<String>,
    /// Vault authentication token. Falls back to VAULT_TOKEN env var.
    #[serde(default)]
    vault_token: Option<String>,
    /// Vault KV v2 mount path (default: "secret").
    #[serde(default = "SecretsConfig::default_mount")]
    vault_mount: String,
    /// Vault path prefix within the mount (default: "sovd/").
    #[serde(default = "SecretsConfig::default_prefix")]
    vault_path_prefix: String,
    /// Cache TTL in seconds (default: 300 = 5 min).
    #[serde(default = "SecretsConfig::default_ttl")]
    vault_cache_ttl_secs: u64,
}

impl SecretsConfig {
    fn default_provider() -> String {
        "env".to_owned()
    }
    fn default_mount() -> String {
        "secret".to_owned()
    }
    fn default_prefix() -> String {
        "sovd/".to_owned()
    }
    fn default_ttl() -> u64 {
        300
    }
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self {
            provider: Self::default_provider(),
            vault_addr: None,
            vault_token: None,
            vault_mount: Self::default_mount(),
            vault_path_prefix: Self::default_prefix(),
            vault_cache_ttl_secs: Self::default_ttl(),
        }
    }
}

/// Prometheus metrics endpoint configuration (F7).
///
/// ```toml
/// [metrics]
/// enabled = true       # default: true
/// path = "/metrics"    # default
/// ```
#[derive(Debug, Deserialize)]
#[allow(dead_code)] // `path` field reserved for custom metrics endpoint path
struct MetricsConfig {
    /// Enable the Prometheus scrape endpoint. Default: true.
    #[serde(default = "MetricsConfig::default_enabled")]
    enabled: bool,
    /// HTTP path for the metrics endpoint. Default: "/metrics".
    #[serde(default = "MetricsConfig::default_path")]
    path: String,
}

impl MetricsConfig {
    fn default_enabled() -> bool {
        true
    }
    fn default_path() -> String {
        "/metrics".to_owned()
    }
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: Self::default_enabled(),
            path: Self::default_path(),
        }
    }
}

/// Firmware signature verification configuration (F12, ISO 24089).
///
/// ```toml
/// [firmware]
/// verify = true                                       # default: false (NoopVerifier)
/// public_key_hex = "aabbccdd..."                      # 32-byte Ed25519 public key (hex)
/// ```
#[derive(Debug, Deserialize)]
struct FirmwareConfig {
    /// Enable firmware signature verification before activation.
    #[serde(default)]
    verify: bool,
    /// Ed25519 public key (hex-encoded, 32 bytes) for signature verification.
    #[serde(default)]
    public_key_hex: Option<String>,
}

impl Default for FirmwareConfig {
    fn default() -> Self {
        Self {
            verify: false,
            public_key_hex: None,
        }
    }
}

/// Build the firmware verifier from configuration.
fn build_firmware_verifier(config: &AppConfig) -> Arc<dyn native_interfaces::FirmwareVerifier> {
    if config.firmware.verify {
        if let Some(ref hex_key) = config.firmware.public_key_hex {
            match native_interfaces::Ed25519Verifier::from_hex(hex_key) {
                Ok(v) => {
                    info!("Firmware signature verification enabled (Ed25519)");
                    return Arc::new(v);
                }
                Err(e) => {
                    tracing::error!("Invalid firmware public key: {e} — falling back to NoopVerifier");
                }
            }
        } else {
            tracing::warn!("firmware.verify=true but no public_key_hex configured — using NoopVerifier");
        }
    }
    Arc::new(native_interfaces::NoopVerifier)
}

// ─────────────────────────────────────────────────────────────────────────────
// Config validation — fail-fast at startup
// ─────────────────────────────────────────────────────────────────────────────

impl AppConfig {
    /// Validate configuration at startup. Returns a list of errors; empty = valid.
    fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        // Server: port must be > 0 (type enforces u16)
        if self.server.port == 0 {
            errors.push("server.port must be > 0".into());
        }

        // TLS: cert and key must both be present or both absent
        match (&self.server.cert_path, &self.server.key_path) {
            (Some(_), None) => {
                errors.push("server.cert_path is set but server.key_path is missing".into());
            }
            (None, Some(_)) => {
                errors.push("server.key_path is set but server.cert_path is missing".into());
            }
            (Some(cert), Some(key)) => {
                if !std::path::Path::new(cert).exists() {
                    errors.push(format!("TLS cert file not found: {cert}"));
                }
                if !std::path::Path::new(key).exists() {
                    errors.push(format!("TLS key file not found: {key}"));
                }
            }
            (None, None) => {}
        }

        // mTLS: client_ca_path requires cert_path + key_path
        if let Some(ref ca) = self.server.client_ca_path {
            if self.server.cert_path.is_none() || self.server.key_path.is_none() {
                errors.push("server.client_ca_path requires cert_path and key_path".into());
            }
            if !std::path::Path::new(ca).exists() {
                errors.push(format!("Client CA file not found: {ca}"));
            }
        }

        // Auth: if enabled, at least one auth method must be configured
        if self.auth.enabled {
            let has_api_key = self.auth.api_key.is_some();
            let has_jwt = self.auth.jwt_secret.is_some();
            let has_oidc = self.auth.oidc_issuer_url.is_some();
            if !has_api_key && !has_jwt && !has_oidc {
                errors.push("auth.enabled is true but no auth method configured (api_key, jwt_secret, or oidc_issuer_url)".into());
            }
        }

        // Backends: validate URLs look reasonable
        for (i, backend) in self.backends.iter().enumerate() {
            if !backend.base_url.starts_with("http://") && !backend.base_url.starts_with("https://")
            {
                errors.push(format!(
                    "backends[{i}] '{}': base_url must start with http:// or https://, got '{}'",
                    backend.name, backend.base_url
                ));
            }
            if backend.component_ids.is_empty() {
                errors.push(format!(
                    "backends[{i}] '{}': component_ids is empty (backend owns no components)",
                    backend.name
                ));
            }
        }

        errors
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load configuration (figment: TOML file + env overrides)
    #[allow(unused_mut)] // mut needed when `vault` feature populates auth from Vault
    let mut config: AppConfig = Figment::new()
        .merge(Toml::file("opensovd-native-server.toml"))
        .merge(Toml::file("config/opensovd-native-server.toml"))
        .merge(Env::prefixed("SOVD_").split("__"))
        .extract()
        .unwrap_or_else(|e| {
            eprintln!("Config warning: {e} — using defaults");
            #[allow(clippy::unwrap_used)] // Infallible: empty JSON object always parses
            serde_json::from_str("{}").unwrap()
        });

    // Initialize tracing (with optional DLT and OTLP layers)
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));
    let dlt_layer = DltTextLayer::new(&config.dlt);
    let use_json = config.logging.format.eq_ignore_ascii_case("json");

    // OTLP trace provider (A2.4) — created once, layer built per-branch for type inference
    #[cfg(feature = "otlp")]
    let otlp_provider = config.logging.otlp_endpoint.as_ref().map(|endpoint| {
        use opentelemetry_otlp::WithExportConfig;
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .expect("OTLP exporter init failed");
        opentelemetry_sdk::trace::TracerProvider::builder()
            .with_simple_exporter(exporter)
            .build()
    });

    #[cfg(not(feature = "otlp"))]
    if config.logging.otlp_endpoint.is_some() {
        eprintln!("Warning: otlp_endpoint configured but `otlp` feature not enabled — ignoring");
    }

    // Macro: build an OTLP layer with fresh type inference (avoids S unification across branches)
    #[cfg(feature = "otlp")]
    macro_rules! otlp_layer {
        () => {{
            use opentelemetry::trace::TracerProvider as _;
            otlp_provider
                .as_ref()
                .map(|p| tracing_opentelemetry::layer().with_tracer(p.tracer("opensovd-native")))
        }};
    }

    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        if use_json {
            // Structured JSON logging with trace correlation (E1.2)
            #[cfg(feature = "otlp")]
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().flatten_event(true).with_target(true))
                .with(dlt_layer)
                .with(otlp_layer!())
                .init();
            #[cfg(not(feature = "otlp"))]
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer().json().flatten_event(true).with_target(true))
                .with(dlt_layer)
                .init();
        } else {
            #[cfg(feature = "otlp")]
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer())
                .with(dlt_layer)
                .with(otlp_layer!())
                .init();
            #[cfg(not(feature = "otlp"))]
            tracing_subscriber::registry()
                .with(filter)
                .with(fmt::layer())
                .with(dlt_layer)
                .init();
        }
    }

    #[cfg(feature = "otlp")]
    if config.logging.otlp_endpoint.is_some() {
        // Log after tracing init so this actually gets emitted
        info!("OpenTelemetry OTLP export enabled");
    }

    // ── Config validation (fail-fast) ─────────────────────────────────
    let config_errors = config.validate();
    if !config_errors.is_empty() {
        for err in &config_errors {
            tracing::error!(target: "config", "{err}");
        }
        return Err(format!(
            "Configuration has {} error(s) — aborting startup",
            config_errors.len()
        )
        .into());
    }

    // ── Secret provider (F4) — populate auth config from Vault if configured ──
    if config.secrets.provider.eq_ignore_ascii_case("vault") {
        #[cfg(feature = "vault")]
        {
            let vault_addr = config
                .secrets
                .vault_addr
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:8200".to_owned());
            let vault_config = native_core::VaultConfig {
                addr: vault_addr,
                token: config.secrets.vault_token.clone().unwrap_or_default(),
                mount: config.secrets.vault_mount.clone(),
                path_prefix: config.secrets.vault_path_prefix.clone(),
                cache_ttl: std::time::Duration::from_secs(config.secrets.vault_cache_ttl_secs),
            };
            let vault = native_core::VaultSecretProvider::new(vault_config);
            use native_interfaces::SecretProvider as _;
            info!(
                "Secret provider: Vault ({})",
                config
                    .secrets
                    .vault_addr
                    .as_deref()
                    .unwrap_or("http://127.0.0.1:8200")
            );

            // Populate auth secrets from Vault (if not already set in config)
            if config.auth.jwt_secret.is_none() {
                if let Some(secret) = vault.get_secret("jwt_secret") {
                    info!("Loaded jwt_secret from Vault");
                    config.auth.jwt_secret = Some(secret);
                }
            }
            if config.auth.api_key.is_none() {
                if let Some(key) = vault.get_secret("api_key") {
                    info!("Loaded api_key from Vault");
                    config.auth.api_key = Some(key);
                }
            }
        }
        #[cfg(not(feature = "vault"))]
        {
            eprintln!("Warning: secrets.provider = \"vault\" but `vault` feature not enabled — using env provider");
        }
    } else {
        info!("Secret provider: {}", config.secrets.provider);
    }

    info!("OpenSOVD-native-server starting");
    info!("Server: {}:{}", config.server.host, config.server.port);

    // ── Build backends (Gateway pattern) ────────────────────────────────
    let mut backends: Vec<Arc<dyn ComponentBackend>> = Vec::new();
    let mut extended_backends: Vec<Arc<dyn native_interfaces::ExtendedDiagBackend>> = Vec::new();

    // 1. HTTP backends → external CDA / SOVD servers (standard-conformant)
    for backend_config in &config.backends {
        info!(
            name = %backend_config.name,
            url = %backend_config.base_url,
            components = ?backend_config.component_ids,
            "Registering HTTP backend"
        );
        let http_backend = SovdHttpBackend::new(backend_config.clone()).map_err(|e| {
            format!(
                "Failed to create HTTP backend '{}': {e}",
                backend_config.name
            )
        })?;

        // Discover components from external server
        if let Err(e) = http_backend.discover().await {
            tracing::warn!(
                name = %backend_config.name,
                error = %e,
                "Failed to discover components (will retry on connect)"
            );
        }
        let backend = Arc::new(http_backend);
        backends.push(backend.clone());
        extended_backends.push(backend);
    }

    if backends.is_empty() {
        tracing::warn!("No backends configured — server will have no components");
    }

    // ── Build ComponentRouter (Gateway) ─────────────────────────────────
    let router = Arc::new(ComponentRouter::new(backends).with_extended(extended_backends));
    info!(
        components = router.list_components().len(),
        "Gateway initialized"
    );

    // ── Initialize server-side services ─────────────────────────────────
    let fault_manager = Arc::new(FaultManager::new());
    let health = Arc::new(HealthMonitor::new());
    let lock_manager = Arc::new(LockManager::new());
    let diag_log = Arc::new(DiagLog::new());

    // Connect fault-lib bridge (DFM role)
    let _fault_bridge = FaultBridge::new(fault_manager.clone());

    // Start lock expiry reaper (SOVD §7.4)
    let _lock_reaper = lock_manager.start_reaper();

    // F12 — Build firmware verifier before config is partially moved
    let firmware_verifier = build_firmware_verifier(&config);

    // Initialize SOME/IP runtime (stub mode if vsomeip-ffi not enabled)
    let someip_runtime = SomeIpRuntime::new(config.someip);
    if let Err(e) = someip_runtime.init().await {
        tracing::warn!("SOME/IP init: {e}");
    }
    if let Err(e) = someip_runtime.start().await {
        tracing::warn!("SOME/IP start: {e}");
    }

    // ── Build axum app ──────────────────────────────────────────────────
    // OEM profile is auto-detected at compile time by native-sovd/build.rs:
    // If src/oem_mbds.rs exists → cfg(has_oem_mbds) is set → MbdsProfile used.
    // Otherwise → SampleOemProfile (standard SOVD) is the fallback.
    let oem_profile: Arc<dyn native_interfaces::oem::OemProfile> = {
        #[cfg(has_oem_mbds)]
        {
            Arc::new(native_sovd::MbdsProfile::default())
        }
        #[cfg(not(has_oem_mbds))]
        {
            Arc::new(native_sovd::SampleOemProfile)
        }
    };
    tracing::info!(profile = oem_profile.name(), "OEM profile loaded");

    let audit_log = Arc::new(AuditLog::new());
    info!("Audit log enabled (in-memory, {} max entries)", 10_000);

    // ── Historical diagnostic storage (W2.2 + F1) ─────────────────────
    let history_store: Arc<dyn native_interfaces::StorageBackend> = match config
        .storage
        .backend
        .as_str()
    {
        #[cfg(feature = "persist")]
        "sled" => {
            let store = native_core::SledStorage::open(&config.storage.sled_path).map_err(|e| {
                format!("Failed to open sled at '{}': {e}", config.storage.sled_path)
            })?;
            info!(path = %config.storage.sled_path, "Persistent storage: sled");
            Arc::new(store)
        }
        #[cfg(not(feature = "persist"))]
        "sled" => {
            return Err("storage.backend = \"sled\" requires the `persist` feature flag".into());
        }
        _ => {
            info!("Storage backend: in-memory (volatile)");
            Arc::new(native_interfaces::InMemoryStorage::new())
        }
    };
    let history = Arc::new(HistoryService::new(history_store, HistoryConfig::default()));

    // E2.4 + W2.2: Background compaction task — prunes expired history entries
    {
        let history_ref = history.clone();
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(6 * 3600); // every 6 hours
            loop {
                tokio::time::sleep(interval).await;
                let removed = history_ref.compact_by_retention();
                if removed > 0 {
                    tracing::info!(removed, "History compaction completed");
                }
            }
        });
        info!("History compaction task scheduled (every 6h)");
    }

    let state = AppState {
        backend: router.clone(),
        extended_backend: router.clone(),
        entity_backend: router,
        diag: native_sovd::DiagState {
            fault_manager,
            lock_manager,
            diag_log,
            history,
        },
        security: native_sovd::SecurityState {
            oem_profile,
            audit_log: audit_log.clone(),
            rate_limiter: if config.rate_limit.enabled {
                info!(
                    max_requests = config.rate_limit.max_requests,
                    window_secs = config.rate_limit.window_secs,
                    "Per-client rate limiting enabled"
                );
                Some(native_sovd::RateLimiter::new(&config.rate_limit))
            } else {
                None
            },
        },
        runtime: native_sovd::RuntimeState {
            health,
            execution_store: Arc::new(DashMap::new()),
            proximity_store: Arc::new(DashMap::new()),
            package_store: Arc::new(DashMap::new()),
            feature_flags: Arc::new(native_interfaces::FeatureFlags::new()),
            firmware_verifier,
        },
        data_catalog: Arc::new(native_interfaces::StaticDataCatalogProvider::new()),
    };
    // ── Bridge routes (Wave 3, W3.1 + F3 WebSocket) ───────────────────
    let app = if config.bridge.enabled {
        // Select bridge transport based on feature flags and config
        #[cfg(feature = "ws-bridge")]
        let bridge_transport: Arc<dyn native_interfaces::BridgeTransport> = {
            let ws = Arc::new(native_core::WsBridgeTransport::new(config.bridge.clone()));
            if config.bridge.listen_addr.is_some() {
                if let Err(e) = ws.start_accept_loop().await {
                    tracing::warn!(error = %e, "WebSocket bridge accept loop failed to start — falling back to in-memory");
                } else {
                    info!("WebSocket bridge transport active");
                }
            }
            ws
        };
        #[cfg(not(feature = "ws-bridge"))]
        let bridge_transport: Arc<dyn native_interfaces::BridgeTransport> =
            Arc::new(InMemoryBridgeTransport::new());

        let bridge_state = BridgeState {
            transport: bridge_transport,
            config: config.bridge.clone(),
        };
        info!("Cloud bridge mode enabled");
        let bridge_router = native_sovd::bridge::build_bridge_router(bridge_state);
        build_router(state, config.auth, config.metrics.enabled)
            .nest("/sovd/v1/x-bridge", bridge_router)
    } else {
        build_router(state, config.auth, config.metrics.enabled)
    };

    if config.tenant.enabled {
        info!(
            tenants = config.tenant.tenants.len(),
            "Multi-tenant mode enabled"
        );
    }

    // ── Start server ────────────────────────────────────────────────────
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);

    // ── mDNS/DNS-SD Discovery (MBDS §4.2) ────────────────────────────
    let _mdns_handle = MdnsHandle::register(&config.mdns, config.server.port);

    if let (Some(cert_path), Some(key_path)) = (&config.server.cert_path, &config.server.key_path) {
        // TLS mode (SOVD §5.3)
        use axum_server::tls_rustls::RustlsConfig;

        let tls_config = if let Some(ref ca_path) = config.server.client_ca_path {
            // mTLS mode — require client certificates (MBDS §6.3)
            info!("mTLS enabled — client CA: {ca_path}");
            build_mtls_config(cert_path, key_path, ca_path).await?
        } else {
            RustlsConfig::from_pem_file(cert_path, key_path)
                .await
                .map_err(|e| format!("TLS config error: {e}"))?
        };

        // E2.1: TLS certificate hot-reload — poll cert/key files for changes
        let _tls_reload_handle = tls_reload::spawn_tls_reloader(
            tls_config.clone(),
            tls_reload::reload_config_from_server(
                cert_path,
                key_path,
                config.server.client_ca_path.as_deref(),
            ),
        );
        info!("TLS certificate hot-reload enabled (30s poll interval)");

        // Graceful shutdown handle for axum_server (TLS path)
        let handle = axum_server::Handle::new();
        let shutdown_handle = handle.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            // 10-second grace period for in-flight requests
            shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
        });

        info!("SOVD API listening on https://{bind_addr}/sovd/v1 (TLS enabled)");
        axum_server::bind_rustls(bind_addr.parse()?, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await?;
    } else {
        // Plain TCP mode
        let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
        info!("SOVD API listening on http://{bind_addr}/sovd/v1");
        info!("Health endpoint: http://{bind_addr}/sovd/v1/health");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
    }

    // ── Post-shutdown cleanup ───────────────────────────────────────────
    info!("Server stopped — running cleanup");
    audit_log.flush();
    info!("Audit log flushed");
    someip_runtime.stop().await;
    info!("OpenSOVD-native-server shutdown complete");

    Ok(())
}

/// Build a TLS config with mutual TLS (client certificate verification).
async fn build_mtls_config(
    cert_path: &str,
    key_path: &str,
    client_ca_path: &str,
) -> Result<axum_server::tls_rustls::RustlsConfig, Box<dyn std::error::Error>> {
    use std::io::BufReader;

    // Read server cert chain
    let cert_pem = tokio::fs::read(cert_path).await?;
    let certs: Vec<_> =
        rustls_pemfile::certs(&mut BufReader::new(&cert_pem[..])).collect::<Result<Vec<_>, _>>()?;

    // Read server private key
    let key_pem = tokio::fs::read(key_path).await?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(&key_pem[..]))?
        .ok_or("No private key found in PEM file")?;

    // Read client CA for verification
    let ca_pem = tokio::fs::read(client_ca_path).await?;
    let mut root_store = rustls::RootCertStore::empty();
    for cert in rustls_pemfile::certs(&mut BufReader::new(&ca_pem[..])) {
        root_store.add(cert?)?;
    }

    let client_verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(root_store))
        .build()
        .map_err(|e| format!("Client verifier error: {e}"))?;

    let mut server_config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)
        .map_err(|e| format!("ServerConfig error: {e}"))?;

    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(axum_server::tls_rustls::RustlsConfig::from_config(
        Arc::new(server_config),
    ))
}

async fn shutdown_signal() {
    let ctrl_c = async {
        #[allow(clippy::expect_used)] // Signal handler install is unrecoverable
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        #[allow(clippy::expect_used)] // Signal handler install is unrecoverable
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => info!("Ctrl+C received, shutting down..."),
        () = terminate => info!("SIGTERM received, shutting down..."),
    }
}
