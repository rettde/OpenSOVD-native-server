// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// opensovd-native-server — Main binary
//
// Architecture:
//   Client → SOVD Server → ComponentRouter (Gateway)
//                               └── SovdHttpBackend → external CDA / SOVD backends
// ─────────────────────────────────────────────────────────────────────────────

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
    AuditLog, ComponentRouter, DiagLog, FaultBridge, FaultManager, LockManager, SovdHttpBackend,
    SovdHttpBackendConfig,
};
use native_health::HealthMonitor;
use native_interfaces::ComponentBackend;
use native_sovd::{
    build_router, AppState, AuthConfig, DltConfig, DltLayer, MdnsConfig, MdnsHandle,
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
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
        }
    }
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
                errors.push("server.cert_path is set but server.key_path is missing".into())
            }
            (None, Some(_)) => {
                errors.push("server.key_path is set but server.cert_path is missing".into())
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
    let config: AppConfig = Figment::new()
        .merge(Toml::file("opensovd-native-server.toml"))
        .merge(Toml::file("config/opensovd-native-server.toml"))
        .merge(Env::prefixed("SOVD_").split("__"))
        .extract()
        .unwrap_or_else(|e| {
            eprintln!("Config warning: {e} — using defaults");
            #[allow(clippy::unwrap_used)] // Infallible: empty JSON object always parses
            serde_json::from_str("{}").unwrap()
        });

    // Initialize tracing (with optional DLT layer)
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));
    let dlt_layer = DltLayer::new(&config.dlt);
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer())
            .with(dlt_layer)
            .init();
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

    info!("OpenSOVD-native-server starting");
    info!("Server: {}:{}", config.server.host, config.server.port);

    // ── Build backends (Gateway pattern) ────────────────────────────────
    let mut backends: Vec<Arc<dyn ComponentBackend>> = Vec::new();

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
        backends.push(Arc::new(http_backend));
    }

    if backends.is_empty() {
        tracing::warn!("No backends configured — server will have no components");
    }

    // ── Build ComponentRouter (Gateway) ─────────────────────────────────
    let router = Arc::new(ComponentRouter::new(backends));
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

    let state = AppState {
        backend: router.clone(),
        entity_backend: router,
        diag: native_sovd::DiagState {
            fault_manager,
            lock_manager,
            diag_log,
        },
        security: native_sovd::SecurityState {
            oem_profile,
            audit_log: audit_log.clone(),
        },
        runtime: native_sovd::RuntimeState {
            health,
            execution_store: Arc::new(DashMap::new()),
            proximity_store: Arc::new(DashMap::new()),
            package_store: Arc::new(DashMap::new()),
        },
    };
    let app = build_router(state, config.auth);

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
