// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// DLT-compatible tracing layer (MBDS S-SOVD §8 / AUTOSAR DLT)
//
// Provides a `tracing_subscriber::Layer` that formats log events as
// DLT-style structured records.  When `dlt_daemon_socket` is configured,
// records are forwarded to a DLTDaemon over a Unix socket.  Otherwise
// they are written to stderr in DLT text format.
//
//  Format:  <timestamp> <ecu_id> <app_id> <ctx_id> <level> <message>
//
// Relationship to eclipse-opensovd/dlt-tracing-lib:
//   The shared `tracing-dlt` crate (https://github.com/eclipse-opensovd/dlt-tracing-lib)
//   provides a full DLT binary-protocol layer backed by libdlt FFI bindings.
//   It supports per-span DLT contexts, typed fields, and dynamic log-level
//   callbacks from the DLT daemon. However, it requires:
//     (a) nightly Rust (edition 2024)
//     (b) libdlt installed on the target system (Linux only)
//
//   This `DltTextLayer` is a lightweight, dependency-free fallback that emits
//   DLT-*text-format* records. It works on all platforms (macOS, CI, embedded)
//   without a native C dependency.
//
// TODO(dlt-tracing-lib): When tracing-dlt reaches stable Rust, add a feature
// flag `dlt-native` that pulls in the real `tracing-dlt` crate for Linux
// targets, keeping `DltTextLayer` as the fallback for other platforms.
// Track: https://github.com/eclipse-opensovd/dlt-tracing-lib
// ─────────────────────────────────────────────────────────────────────────────

use std::fmt::{self, Write as FmtWrite};
use std::io::Write;

use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

/// DLT tracing configuration
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DltConfig {
    /// Enable DLT-formatted output (default: false — use standard tracing)
    #[serde(default)]
    pub enabled: bool,
    /// ECU identifier emitted in every DLT record (max 4 chars, AUTOSAR DLT spec)
    #[serde(default = "default_ecu_id")]
    pub ecu_id: String,
    /// Application identifier (AUTOSAR DLT APID, max 4 chars)
    #[serde(default = "default_app_id")]
    pub app_id: String,
    /// Context identifier (AUTOSAR DLT CTID, max 4 chars)
    #[serde(default = "default_ctx_id")]
    pub ctx_id: String,
    /// Optional path to DLTDaemon Unix socket for forwarding
    /// (e.g. `/tmp/dlt` or `/var/run/dlt/dlt`).
    /// When None, DLT records are written to stderr.
    #[serde(default)]
    pub daemon_socket: Option<String>,
}

fn default_ecu_id() -> String {
    "SOVD".to_owned()
}
fn default_app_id() -> String {
    "SOVD".to_owned()
}
fn default_ctx_id() -> String {
    "MAIN".to_owned()
}

impl Default for DltConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ecu_id: default_ecu_id(),
            app_id: default_app_id(),
            ctx_id: default_ctx_id(),
            daemon_socket: None,
        }
    }
}

/// A `tracing_subscriber::Layer` that emits logs in DLT text format.
///
/// This is a lightweight fallback for environments without `libdlt`.
/// For production Linux deployments, prefer the full `tracing-dlt` crate
/// from <https://github.com/eclipse-opensovd/dlt-tracing-lib> which speaks
/// proper DLT binary protocol.
pub struct DltTextLayer {
    ecu_id: String,
    app_id: String,
    ctx_id: String,
    writer: DltTextWriter,
}

enum DltTextWriter {
    Stderr,
    #[cfg(unix)]
    UnixSocket(std::sync::Mutex<std::os::unix::net::UnixDatagram>),
}

impl DltTextLayer {
    /// Create a new DLT layer from config.  Returns `None` if DLT is disabled.
    pub fn new(config: &DltConfig) -> Option<Self> {
        if !config.enabled {
            return None;
        }

        let writer = match &config.daemon_socket {
            #[cfg(unix)]
            Some(path) => match std::os::unix::net::UnixDatagram::unbound() {
                Ok(sock) => {
                    if sock.connect(path).is_ok() {
                        tracing::info!(path = %path, "DLT: connected to DLTDaemon socket");
                        DltTextWriter::UnixSocket(std::sync::Mutex::new(sock))
                    } else {
                        tracing::warn!(path = %path, "DLT: failed to connect to daemon socket, falling back to stderr");
                        DltTextWriter::Stderr
                    }
                }
                Err(_) => DltTextWriter::Stderr,
            },
            #[cfg(not(unix))]
            Some(_) => {
                tracing::warn!("DLT: Unix sockets not available on this platform, using stderr");
                DltTextWriter::Stderr
            }
            None => DltTextWriter::Stderr,
        };

        Some(Self {
            ecu_id: config.ecu_id.chars().take(4).collect(),
            app_id: config.app_id.chars().take(4).collect(),
            ctx_id: config.ctx_id.chars().take(4).collect(),
            writer,
        })
    }

    fn write_record(&self, record: &str) {
        match &self.writer {
            DltTextWriter::Stderr => {
                let _ = writeln!(std::io::stderr(), "{record}");
            }
            #[cfg(unix)]
            DltTextWriter::UnixSocket(sock) => {
                if let Ok(s) = sock.lock() {
                    let _ = s.send(record.as_bytes());
                }
            }
        }
    }
}

/// Map tracing Level to DLT log level string
fn dlt_level(level: tracing::Level) -> &'static str {
    match level {
        tracing::Level::ERROR => "ERRO",
        tracing::Level::WARN => "WARN",
        tracing::Level::INFO => "INFO",
        tracing::Level::DEBUG => "DEBG",
        tracing::Level::TRACE => "VERB",
    }
}

impl<S> tracing_subscriber::Layer<S> for DltTextLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = dlt_level(*metadata.level());
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ");

        // Collect message fields
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let record = format!(
            "{now} {ecu} {app} {ctx} {level} [{target}] {msg}",
            ecu = self.ecu_id,
            app = self.app_id,
            ctx = self.ctx_id,
            target = metadata.target(),
            msg = visitor.message,
        );

        self.write_record(&record);
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else if !self.message.is_empty() {
            let _ = write!(self.message, " {}={:?}", field.name(), value);
        } else {
            self.message = format!("{}={:?}", field.name(), value);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            value.clone_into(&mut self.message);
        } else if !self.message.is_empty() {
            let _ = write!(self.message, " {}={}", field.name(), value);
        } else {
            self.message = format!("{}={}", field.name(), value);
        }
    }
}
