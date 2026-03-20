// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// AuditSink — Pluggable audit log forwarding (F11)
//
// Trait for forwarding audit entries to external systems (SIEM, syslog, Kafka).
// Implementations are registered on the AuditLog and called for every new entry.
// ─────────────────────────────────────────────────────────────────────────────

use crate::sovd::SovdAuditEntry;

/// Pluggable sink for forwarding audit entries to external systems.
///
/// Implementations must be thread-safe and non-blocking. If the sink
/// cannot deliver an entry, it should log the failure internally and
/// not block the caller.
///
/// # Built-in implementations
///
/// | Sink | Description |
/// |------|------------|
/// | `SyslogAuditSink` | RFC 5424 syslog over UDP/TCP (native-core) |
/// | `CallbackAuditSink` | Arbitrary closure (for testing / custom integrations) |
pub trait AuditSink: Send + Sync {
    /// Forward a single audit entry to the external system.
    ///
    /// Called synchronously in the `AuditLog::record()` path.
    /// Implementations should be fast and non-blocking.
    fn forward(&self, entry: &SovdAuditEntry);

    /// Human-readable sink name for logging.
    fn name(&self) -> &str;
}

/// Simple callback-based audit sink (useful for testing and custom integrations).
pub struct CallbackAuditSink<F: Fn(&SovdAuditEntry) + Send + Sync> {
    name: String,
    callback: F,
}

impl<F: Fn(&SovdAuditEntry) + Send + Sync> CallbackAuditSink<F> {
    pub fn new(name: impl Into<String>, callback: F) -> Self {
        Self {
            name: name.into(),
            callback,
        }
    }
}

impl<F: Fn(&SovdAuditEntry) + Send + Sync> AuditSink for CallbackAuditSink<F> {
    fn forward(&self, entry: &SovdAuditEntry) {
        (self.callback)(entry);
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Configuration for audit forwarding sinks.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct AuditForwardingConfig {
    /// Syslog target (e.g. "udp://localhost:514", "tcp://siem.corp:1514")
    #[serde(default)]
    pub syslog_target: Option<String>,

    /// Syslog facility (default: "local0")
    #[serde(default = "default_facility")]
    pub syslog_facility: String,

    /// Application name in syslog messages (default: "opensovd")
    #[serde(default = "default_app_name")]
    pub syslog_app_name: String,
}

fn default_facility() -> String {
    "local0".to_owned()
}

fn default_app_name() -> String {
    "opensovd".to_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::sovd::{SovdAuditAction, SovdAuditEntry};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    fn sample_entry() -> SovdAuditEntry {
        SovdAuditEntry {
            seq: 1,
            timestamp: "2026-03-20T15:00:00Z".to_owned(),
            caller: "test-user".to_owned(),
            action: SovdAuditAction::ReadData,
            target: "component/hpc".to_owned(),
            resource: "data".to_owned(),
            method: "GET".to_owned(),
            outcome: "success".to_owned(),
            detail: None,
            trace_id: None,
            prev_hash: None,
            hash: None,
        }
    }

    #[test]
    fn callback_sink_receives_entries() {
        let count = Arc::new(AtomicU32::new(0));
        let count_clone = count.clone();
        let sink = CallbackAuditSink::new("test", move |_entry: &SovdAuditEntry| {
            count_clone.fetch_add(1, Ordering::Relaxed);
        });

        assert_eq!(sink.name(), "test");
        sink.forward(&sample_entry());
        sink.forward(&sample_entry());
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }
}
