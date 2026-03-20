// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// SyslogAuditSink — RFC 5424 syslog forwarding for audit entries (F11)
//
// Sends audit entries as structured syslog messages over UDP.
// Falls back to logging a warning if the target is unreachable.
// ─────────────────────────────────────────────────────────────────────────────

use std::net::UdpSocket;

use native_interfaces::audit_sink::AuditSink;
use native_interfaces::sovd::SovdAuditEntry;
use tracing::{debug, warn};

/// Syslog audit sink — sends JSON-serialized audit entries via UDP.
///
/// Message format: `<priority>1 timestamp hostname app_name - - - json`
/// where priority = facility * 8 + severity (6 = informational).
pub struct SyslogAuditSink {
    socket: UdpSocket,
    target: String,
    facility: u8,
    app_name: String,
    hostname: String,
}

impl SyslogAuditSink {
    /// Create a new syslog sink.
    ///
    /// `target` is a `host:port` string (e.g. "localhost:514").
    /// Returns `None` if the UDP socket cannot be created.
    pub fn new(target: &str, facility: &str, app_name: &str) -> Option<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.set_nonblocking(true).ok()?;

        let facility_num = match facility {
            "kern" => 0,
            "user" => 1,
            "mail" => 2,
            "daemon" => 3,
            "auth" => 4,
            "syslog" => 5,
            "local1" => 17,
            "local2" => 18,
            "local3" => 19,
            "local4" => 20,
            "local5" => 21,
            "local6" => 22,
            "local7" => 23,
            _ => 16, // default: local0
        };

        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "opensovd".to_owned());

        // Strip protocol prefix if present (e.g. "udp://localhost:514" → "localhost:514")
        let clean_target = target
            .strip_prefix("udp://")
            .or_else(|| target.strip_prefix("tcp://"))
            .unwrap_or(target);

        debug!(target = %clean_target, facility = %facility, "Syslog audit sink created");

        Some(Self {
            socket,
            target: clean_target.to_owned(),
            facility: facility_num,
            app_name: app_name.to_owned(),
            hostname,
        })
    }
}

impl AuditSink for SyslogAuditSink {
    fn forward(&self, entry: &SovdAuditEntry) {
        let severity = 6u8; // informational
        let priority = self.facility * 8 + severity;

        let json = match serde_json::to_string(entry) {
            Ok(j) => j,
            Err(e) => {
                warn!(error = %e, "Failed to serialize audit entry for syslog");
                return;
            }
        };

        // RFC 5424: <PRI>VERSION TIMESTAMP HOSTNAME APP-NAME PROCID MSGID SD MSG
        let msg = format!(
            "<{priority}>1 {ts} {host} {app} - - - {json}",
            priority = priority,
            ts = entry.timestamp,
            host = self.hostname,
            app = self.app_name,
            json = json,
        );

        if let Err(e) = self.socket.send_to(msg.as_bytes(), &self.target) {
            warn!(target = %self.target, error = %e, "Failed to send audit entry to syslog");
        }
    }

    fn name(&self) -> &str {
        "syslog"
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use native_interfaces::sovd::SovdAuditAction;
    use std::net::UdpSocket;

    fn sample_entry() -> SovdAuditEntry {
        SovdAuditEntry {
            seq: 42,
            timestamp: "2026-03-20T15:30:00Z".to_owned(),
            caller: "workshop-user".to_owned(),
            action: SovdAuditAction::WriteData,
            target: "component/hpc".to_owned(),
            resource: "data/rpm".to_owned(),
            method: "PUT".to_owned(),
            outcome: "success".to_owned(),
            detail: Some("rpm=3500".to_owned()),
            trace_id: None,
            prev_hash: None,
            hash: None,
        }
    }

    #[test]
    fn syslog_sink_sends_udp_message() {
        // Bind a local receiver
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let recv_addr = receiver.local_addr().unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();

        // Create sink pointing at receiver
        let sink = SyslogAuditSink::new(
            &format!("127.0.0.1:{}", recv_addr.port()),
            "local0",
            "opensovd-test",
        )
        .unwrap();

        // Forward an entry
        sink.forward(&sample_entry());

        // Receive and verify
        let mut buf = [0u8; 4096];
        let (len, _) = receiver.recv_from(&mut buf).unwrap();
        let msg = std::str::from_utf8(&buf[..len]).unwrap();

        // Check syslog format: <134> = local0 (16) * 8 + 6 (info) = 134
        assert!(msg.starts_with("<134>1 "));
        assert!(msg.contains("opensovd-test"));
        assert!(msg.contains("workshop-user"));
        assert!(msg.contains("writeData"));
        assert!(msg.contains("component/hpc"));
    }
}
