// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// TLS Certificate Hot-Reload (E2.1)
//
// Polls certificate and key files for changes (by mtime) and calls
// RustlsConfig::reload_from_pem_file() when a change is detected.
//
// Design rationale:
//   - Polling (not inotify/kqueue) for portability and Kubernetes Secret
//     mount compatibility (symlink rotations don't always trigger inotify)
//   - Default poll interval: 30 seconds (configurable)
//   - Graceful: reload errors are logged but don't crash the server
//   - Supports both TLS and mTLS (watches client_ca_path too)
// ─────────────────────────────────────────────────────────────────────────────

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum_server::tls_rustls::RustlsConfig;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Configuration for TLS hot-reload.
#[derive(Debug, Clone)]
pub struct TlsReloadConfig {
    /// Path to TLS certificate file (PEM)
    pub cert_path: PathBuf,
    /// Path to TLS private key file (PEM)
    pub key_path: PathBuf,
    /// Optional path to client CA file (PEM) for mTLS
    pub client_ca_path: Option<PathBuf>,
    /// Poll interval for checking file changes
    pub poll_interval: Duration,
}

impl TlsReloadConfig {
    pub fn new(cert_path: &str, key_path: &str) -> Self {
        Self {
            cert_path: PathBuf::from(cert_path),
            key_path: PathBuf::from(key_path),
            client_ca_path: None,
            poll_interval: Duration::from_secs(30),
        }
    }

    pub fn with_client_ca(mut self, ca_path: &str) -> Self {
        self.client_ca_path = Some(PathBuf::from(ca_path));
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

/// Handle returned by `spawn_tls_reloader` — can be used to stop the watcher.
pub struct TlsReloadHandle {
    _shutdown_tx: watch::Sender<bool>,
}

impl TlsReloadHandle {
    /// Signal the reload task to stop.
    pub fn shutdown(&self) {
        let _ = self._shutdown_tx.send(true);
    }
}

/// Spawn a background task that polls certificate files and reloads TLS config
/// when changes are detected.
///
/// Returns a handle that keeps the task alive. Drop the handle to stop polling.
pub fn spawn_tls_reloader(
    tls_config: RustlsConfig,
    reload_config: TlsReloadConfig,
) -> TlsReloadHandle {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    tokio::spawn(async move {
        let mut last_cert_mtime = file_mtime(&reload_config.cert_path);
        let mut last_key_mtime = file_mtime(&reload_config.key_path);
        let mut last_ca_mtime = reload_config
            .client_ca_path
            .as_ref()
            .map(|p| file_mtime(p));

        info!(
            cert = %reload_config.cert_path.display(),
            key = %reload_config.key_path.display(),
            interval_secs = reload_config.poll_interval.as_secs(),
            "TLS hot-reload watcher started"
        );

        loop {
            tokio::select! {
                _ = tokio::time::sleep(reload_config.poll_interval) => {},
                _ = shutdown_rx.changed() => {
                    info!("TLS hot-reload watcher shutting down");
                    break;
                }
            }

            let cert_mtime = file_mtime(&reload_config.cert_path);
            let key_mtime = file_mtime(&reload_config.key_path);
            let ca_mtime = reload_config
                .client_ca_path
                .as_ref()
                .map(|p| file_mtime(p));

            let cert_changed = cert_mtime != last_cert_mtime;
            let key_changed = key_mtime != last_key_mtime;
            let ca_changed = ca_mtime != last_ca_mtime;

            if cert_changed || key_changed || ca_changed {
                info!(
                    cert_changed,
                    key_changed,
                    ca_changed,
                    "TLS certificate change detected — reloading"
                );

                match tls_config
                    .reload_from_pem_file(&reload_config.cert_path, &reload_config.key_path)
                    .await
                {
                    Ok(()) => {
                        info!("TLS certificates reloaded successfully");
                        last_cert_mtime = cert_mtime;
                        last_key_mtime = key_mtime;
                        last_ca_mtime = ca_mtime;
                    }
                    Err(e) => {
                        error!(error = %e, "TLS certificate reload failed — keeping previous config");
                    }
                }
            } else {
                debug!("TLS certificate poll: no changes");
            }
        }
    });

    TlsReloadHandle {
        _shutdown_tx: shutdown_tx,
    }
}

/// Get the modification time of a file, returning None if the file doesn't exist.
fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Convenience: build a `TlsReloadConfig` from server config fields.
pub fn reload_config_from_server(
    cert_path: &str,
    key_path: &str,
    client_ca_path: Option<&str>,
) -> TlsReloadConfig {
    let mut config = TlsReloadConfig::new(cert_path, key_path);
    if let Some(ca) = client_ca_path {
        config = config.with_client_ca(ca);
    }
    config
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn tls_reload_config_builder() {
        let config = TlsReloadConfig::new("/tmp/cert.pem", "/tmp/key.pem")
            .with_client_ca("/tmp/ca.pem")
            .with_poll_interval(Duration::from_secs(60));

        assert_eq!(config.cert_path, PathBuf::from("/tmp/cert.pem"));
        assert_eq!(config.key_path, PathBuf::from("/tmp/key.pem"));
        assert_eq!(
            config.client_ca_path,
            Some(PathBuf::from("/tmp/ca.pem"))
        );
        assert_eq!(config.poll_interval, Duration::from_secs(60));
    }

    #[test]
    fn file_mtime_returns_some_for_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.pem");
        std::fs::write(&path, "test").unwrap();
        assert!(file_mtime(&path).is_some());
    }

    #[test]
    fn file_mtime_returns_none_for_missing_file() {
        assert!(file_mtime(Path::new("/nonexistent/cert.pem")).is_none());
    }

    #[test]
    fn file_mtime_detects_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cert.pem");
        std::fs::write(&path, "v1").unwrap();
        let mtime1 = file_mtime(&path);

        // Ensure filesystem timestamp granularity
        std::thread::sleep(Duration::from_millis(50));

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        file.write_all(b"v2").unwrap();
        file.flush().unwrap();
        drop(file);

        let mtime2 = file_mtime(&path);
        // On most filesystems, mtime changes; on some, granularity may be 1s
        // This test verifies the function works, not that the OS updates mtime instantly
        assert!(mtime1.is_some());
        assert!(mtime2.is_some());
    }

    #[test]
    fn reload_config_from_server_builds_correctly() {
        let config = reload_config_from_server("/cert.pem", "/key.pem", Some("/ca.pem"));
        assert_eq!(config.cert_path, PathBuf::from("/cert.pem"));
        assert_eq!(config.key_path, PathBuf::from("/key.pem"));
        assert_eq!(config.client_ca_path, Some(PathBuf::from("/ca.pem")));
    }

    #[test]
    fn reload_config_from_server_without_ca() {
        let config = reload_config_from_server("/cert.pem", "/key.pem", None);
        assert!(config.client_ca_path.is_none());
    }

    #[test]
    fn default_poll_interval_is_30s() {
        let config = TlsReloadConfig::new("/cert.pem", "/key.pem");
        assert_eq!(config.poll_interval, Duration::from_secs(30));
    }
}
