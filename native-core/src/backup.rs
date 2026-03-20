// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Backup / Restore (E2.3) — Diagnostic state snapshot
//
// Serializes the complete diagnostic state (faults, audit entries, history
// counts) into a single JSON document that can be written to disk or sent
// via the admin API. Restore replays the snapshot back into memory.
//
// Snapshot format:
// {
//   "version": 1,
//   "created_at": "2026-03-20T09:00:00Z",
//   "server_version": "0.10.0",
//   "faults": [ ... ],
//   "audit_entries": [ ... ],
//   "history_fault_count": N,
//   "history_audit_count": N
// }
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};

use native_interfaces::sovd::{SovdAuditEntry, SovdFault};

use crate::audit_log::{AuditFilter, AuditLog};
use crate::fault_manager::FaultManager;

/// Snapshot format version — bump when the schema changes.
const SNAPSHOT_VERSION: u32 = 1;

/// A complete diagnostic state snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticSnapshot {
    /// Schema version for forward compatibility
    pub version: u32,
    /// ISO 8601 timestamp when the snapshot was created
    pub created_at: String,
    /// Server version that created the snapshot
    pub server_version: String,
    /// All active faults
    pub faults: Vec<SovdFault>,
    /// All audit log entries currently in the ring buffer
    pub audit_entries: Vec<SovdAuditEntry>,
    /// Number of historical fault records (informational, not restored)
    pub history_fault_count: usize,
    /// Number of historical audit records (informational, not restored)
    pub history_audit_count: usize,
}

/// Create a snapshot of the current diagnostic state.
pub fn create_snapshot(
    fault_manager: &FaultManager,
    audit_log: &AuditLog,
    history_fault_count: usize,
    history_audit_count: usize,
) -> DiagnosticSnapshot {
    let faults = fault_manager.get_all_faults();
    let audit_entries = audit_log.query(&AuditFilter {
        caller: None,
        action: None,
        target: None,
        outcome: None,
        limit: None,
    });

    DiagnosticSnapshot {
        version: SNAPSHOT_VERSION,
        created_at: chrono::Utc::now().to_rfc3339(),
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        faults,
        audit_entries,
        history_fault_count,
        history_audit_count,
    }
}

/// Restore diagnostic state from a snapshot.
///
/// - Faults are loaded into the FaultManager (existing faults are NOT cleared
///   first — the caller should clear if a full replace is desired).
/// - Audit entries are replayed into the AuditLog.
///
/// Returns the number of faults and audit entries restored.
pub fn restore_snapshot(
    snapshot: &DiagnosticSnapshot,
    fault_manager: &FaultManager,
    audit_log: &AuditLog,
) -> Result<RestoreResult, RestoreError> {
    if snapshot.version > SNAPSHOT_VERSION {
        return Err(RestoreError::UnsupportedVersion {
            snapshot: snapshot.version,
            supported: SNAPSHOT_VERSION,
        });
    }

    let mut faults_restored = 0;
    for fault in &snapshot.faults {
        fault_manager.report_fault(fault.clone());
        faults_restored += 1;
    }

    let mut audit_restored = 0;
    for entry in &snapshot.audit_entries {
        audit_log.record(
            &entry.caller,
            entry.action,
            &entry.target,
            &entry.resource,
            &entry.method,
            &entry.outcome,
            entry.detail.as_deref(),
            entry.trace_id.as_deref(),
        );
        audit_restored += 1;
    }

    Ok(RestoreResult {
        faults_restored,
        audit_restored,
    })
}

/// Result of a successful restore operation.
#[derive(Debug, Clone, Serialize)]
pub struct RestoreResult {
    pub faults_restored: usize,
    pub audit_restored: usize,
}

/// Errors that can occur during restore.
#[derive(Debug, Clone)]
pub enum RestoreError {
    /// Snapshot version is newer than what this server supports
    UnsupportedVersion { snapshot: u32, supported: u32 },
    /// JSON deserialization failed
    ParseError(String),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion {
                snapshot,
                supported,
            } => write!(
                f,
                "Snapshot version {snapshot} is newer than supported version {supported}"
            ),
            Self::ParseError(msg) => write!(f, "Snapshot parse error: {msg}"),
        }
    }
}

/// Serialize a snapshot to JSON bytes.
pub fn snapshot_to_json(snapshot: &DiagnosticSnapshot) -> Result<Vec<u8>, String> {
    serde_json::to_vec_pretty(snapshot).map_err(|e| format!("Snapshot serialization error: {e}"))
}

/// Deserialize a snapshot from JSON bytes.
pub fn snapshot_from_json(data: &[u8]) -> Result<DiagnosticSnapshot, RestoreError> {
    serde_json::from_slice(data).map_err(|e| RestoreError::ParseError(e.to_string()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use native_interfaces::sovd::*;

    fn make_fault(id: &str, component_id: &str, code: &str) -> SovdFault {
        SovdFault {
            id: id.into(),
            component_id: component_id.into(),
            code: code.into(),
            display_code: None,
            severity: SovdFaultSeverity::High,
            status: SovdFaultStatus::Active,
            name: format!("Fault {id}"),
            description: None,
            scope: None,
            affected_subsystem: None,
            correlated_signals: vec![],
            classification_tags: vec![],
        }
    }

    fn test_manager() -> FaultManager {
        FaultManager::new()
    }

    #[test]
    fn create_snapshot_captures_state() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));
        fm.report_fault(make_fault("f2", "brake", "P0200"));

        let al = AuditLog::new();
        al.record(
            "alice",
            SovdAuditAction::ReadData,
            "c/hpc",
            "data",
            "GET",
            "success",
            None,
            None,
        );

        let snap = create_snapshot(&fm, &al, 10, 5);
        assert_eq!(snap.version, SNAPSHOT_VERSION);
        assert_eq!(snap.faults.len(), 2);
        assert_eq!(snap.audit_entries.len(), 1);
        assert_eq!(snap.history_fault_count, 10);
        assert_eq!(snap.history_audit_count, 5);
        assert!(!snap.server_version.is_empty());
    }

    #[test]
    fn snapshot_serialization_roundtrip() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));

        let al = AuditLog::new();
        al.record(
            "u",
            SovdAuditAction::ReadData,
            "c",
            "d",
            "GET",
            "ok",
            None,
            None,
        );

        let snap = create_snapshot(&fm, &al, 0, 0);
        let json = snapshot_to_json(&snap).unwrap();
        let restored = snapshot_from_json(&json).unwrap();

        assert_eq!(restored.version, snap.version);
        assert_eq!(restored.faults.len(), 1);
        assert_eq!(restored.audit_entries.len(), 1);
        assert_eq!(restored.faults[0].id, "f1");
    }

    #[test]
    fn restore_populates_managers() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));

        let al = AuditLog::new();
        al.record(
            "u",
            SovdAuditAction::ReadData,
            "c",
            "d",
            "GET",
            "ok",
            None,
            None,
        );

        let snap = create_snapshot(&fm, &al, 0, 0);

        // Create fresh managers to restore into
        let fm2 = test_manager();
        let al2 = AuditLog::new();
        assert_eq!(fm2.total_fault_count(), 0);
        assert!(al2.is_empty());

        let result = restore_snapshot(&snap, &fm2, &al2).unwrap();
        assert_eq!(result.faults_restored, 1);
        assert_eq!(result.audit_restored, 1);
        assert_eq!(fm2.total_fault_count(), 1);
        assert_eq!(fm2.get_fault("f1").unwrap().code, "P0100");
        assert_eq!(al2.len(), 1);
    }

    #[test]
    fn restore_rejects_future_version() {
        let snap = DiagnosticSnapshot {
            version: 999,
            created_at: String::new(),
            server_version: String::new(),
            faults: vec![],
            audit_entries: vec![],
            history_fault_count: 0,
            history_audit_count: 0,
        };

        let fm = test_manager();
        let al = AuditLog::new();
        let err = restore_snapshot(&snap, &fm, &al).unwrap_err();
        assert!(err.to_string().contains("999"));
    }

    #[test]
    fn restore_with_empty_snapshot_is_noop() {
        let snap = DiagnosticSnapshot {
            version: SNAPSHOT_VERSION,
            created_at: String::new(),
            server_version: String::new(),
            faults: vec![],
            audit_entries: vec![],
            history_fault_count: 0,
            history_audit_count: 0,
        };

        let fm = test_manager();
        let al = AuditLog::new();
        let result = restore_snapshot(&snap, &fm, &al).unwrap();
        assert_eq!(result.faults_restored, 0);
        assert_eq!(result.audit_restored, 0);
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let err = snapshot_from_json(b"not json").unwrap_err();
        assert!(err.to_string().contains("parse error"));
    }

    #[test]
    fn snapshot_includes_timestamp() {
        let fm = test_manager();
        let al = AuditLog::new();
        let snap = create_snapshot(&fm, &al, 0, 0);
        assert!(!snap.created_at.is_empty());
        // Should be valid RFC3339
        assert!(chrono::DateTime::parse_from_rfc3339(&snap.created_at).is_ok());
    }

    #[test]
    fn multiple_restore_accumulates_faults() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));
        let al = AuditLog::new();
        let snap = create_snapshot(&fm, &al, 0, 0);

        let fm2 = test_manager();
        fm2.report_fault(make_fault("f0", "brake", "P0099"));
        let al2 = AuditLog::new();

        restore_snapshot(&snap, &fm2, &al2).unwrap();
        // f0 was already there, f1 was restored
        assert_eq!(fm2.total_fault_count(), 2);
    }
}
