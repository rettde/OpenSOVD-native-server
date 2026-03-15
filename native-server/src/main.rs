// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// opensovd-native-server — Main binary (OpenSOVD standard-conformant architecture)
//
// Architecture:
//   Client → SOVD Server → ComponentRouter (Gateway)
//                               ├── SovdHttpBackend  → external CDA (standard)
//                               └── LocalUdsBackend  → embedded UDS/DoIP (standalone)
//
// The server dispatches to backends per component. Each backend can be:
//   - "http://<url>"  → SovdHttpBackend (forwards to external CDA)
//   - "local-uds"     → LocalUdsBackend (direct UDS/DoIP, feature-gated)
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
    ComponentRouter, DiagLog, FaultBridge, FaultManager, LockManager, SovdHttpBackend,
    SovdHttpBackendConfig,
};
use native_health::HealthMonitor;
use native_interfaces::ComponentBackend;
use native_sovd::{build_router, AppState, AuthConfig};

#[cfg(feature = "local-uds")]
use native_comm_doip::DoipConfig;
#[cfg(feature = "local-uds")]
use native_core::translation::{ComponentMapping, GroupDef, TranslationConfig};
#[cfg(feature = "local-uds")]
use native_core::{LocalUdsBackend, SovdTranslator};

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

    // ── Backend configuration ───────────────────────────────────────────
    /// External SOVD backends (CDA instances, native SOVD endpoints)
    #[serde(default)]
    backends: Vec<SovdHttpBackendConfig>,

    // ── Local UDS/DoIP backend (standalone mode, feature-gated) ─────────
    #[cfg(feature = "local-uds")]
    #[serde(default)]
    doip: DoipConfig,
    #[cfg(feature = "local-uds")]
    #[serde(default)]
    components: Vec<ComponentMapping>,
    #[cfg(feature = "local-uds")]
    #[serde(default)]
    groups: Vec<GroupDef>,
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
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_owned(),
            port: 8080,
            cert_path: None,
            key_path: None,
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

    // Initialize tracing
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logging.level));
    fmt().with_env_filter(filter).init();

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

    // 2. Local UDS/DoIP backend (standalone mode, feature-gated)
    #[cfg(feature = "local-uds")]
    {
        if !config.components.is_empty() {
            info!(
                count = config.components.len(),
                "Registering local UDS/DoIP backend (standalone mode)"
            );
            let translation_config = TranslationConfig {
                doip: config.doip.clone(),
                component_mappings: config.components,
                tester_present_interval_ms: 2000,
                groups: config.groups,
            };
            let translator = Arc::new(SovdTranslator::new(translation_config));
            let local_backend = Arc::new(LocalUdsBackend::new(translator));
            backends.push(local_backend);
        }
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
    let state = AppState {
        backend: router,
        fault_manager,
        lock_manager,
        diag_log,
        health,
        execution_store: Arc::new(DashMap::new()),
        proximity_store: Arc::new(DashMap::new()),
    };
    let app = build_router(state, config.auth);

    // ── Start server ────────────────────────────────────────────────────
    let bind_addr = format!("{}:{}", config.server.host, config.server.port);

    if let (Some(cert_path), Some(key_path)) = (&config.server.cert_path, &config.server.key_path) {
        // TLS mode (SOVD §5.3)
        use axum_server::tls_rustls::RustlsConfig;
        let tls_config = RustlsConfig::from_pem_file(cert_path, key_path)
            .await
            .map_err(|e| format!("TLS config error: {e}"))?;
        info!("SOVD API listening on https://{bind_addr}/sovd/v1 (TLS enabled)");
        axum_server::bind_rustls(bind_addr.parse()?, tls_config)
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

    // Graceful shutdown
    someip_runtime.stop().await;
    info!("OpenSOVD-native-server stopped");

    Ok(())
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
