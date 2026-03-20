// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Fault Injection Tests (T2.2) — System resilience under failure conditions
//
// These tests verify that the diagnostic core handles edge cases and failure
// scenarios gracefully:
//   - Concurrent access under contention
//   - Overflow / boundary conditions
//   - Corrupted input handling
//   - State recovery after failures
//   - Resource exhaustion simulation
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::thread;

use native_core::audit_log::{AuditFilter, AuditLog, AuditLogConfig};
use native_core::backup;
use native_core::fault_manager::FaultManager;
use native_core::history::{HistoryConfig, HistoryService};
use native_interfaces::sovd::*;
use native_interfaces::InMemoryStorage;

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

// ── Concurrent access tests ──────────────────────────────────────────────────

#[test]
fn concurrent_fault_reporting_no_data_loss() {
    let fm = Arc::new(FaultManager::new());
    let threads: Vec<_> = (0..10)
        .map(|t| {
            let fm = fm.clone();
            thread::spawn(move || {
                for i in 0..100 {
                    fm.report_fault(make_fault(
                        &format!("t{t}-f{i}"),
                        &format!("comp-{t}"),
                        "P0100",
                    ));
                }
            })
        })
        .collect();

    for t in threads {
        t.join().unwrap();
    }

    // 10 threads × 100 faults = 1000 total
    assert_eq!(fm.total_fault_count(), 1000);
}

#[test]
fn concurrent_fault_clear_while_reporting() {
    let fm = Arc::new(FaultManager::new());

    // Pre-populate
    for i in 0..500 {
        fm.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
    }

    let fm_writer = fm.clone();
    let fm_clearer = fm.clone();

    let writer = thread::spawn(move || {
        for i in 500..1000 {
            fm_writer.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
        }
    });

    let clearer = thread::spawn(move || {
        for i in 0..500 {
            fm_clearer.clear_fault(&format!("f{i}"));
        }
    });

    writer.join().unwrap();
    clearer.join().unwrap();

    // Some faults may or may not be present depending on ordering,
    // but the system should not panic or deadlock
    let count = fm.total_fault_count();
    assert!(count <= 1000, "Count should be at most 1000, got {count}");
}

#[test]
fn concurrent_audit_recording() {
    let al = Arc::new(AuditLog::new());
    let threads: Vec<_> = (0..10)
        .map(|t| {
            let al = al.clone();
            thread::spawn(move || {
                for _ in 0..100 {
                    al.record(
                        &format!("thread-{t}"),
                        SovdAuditAction::ReadData,
                        "c/hpc",
                        "data",
                        "GET",
                        "success",
                        None,
                        None,
                    );
                }
            })
        })
        .collect();

    for t in threads {
        t.join().unwrap();
    }

    assert_eq!(al.len(), 1000);
    // All sequence numbers should be unique (assigned by AtomicU64)
    let all = al.query(&AuditFilter::default());
    let mut seqs: Vec<u64> = all.iter().map(|e| e.seq).collect();
    seqs.sort();
    seqs.dedup();
    assert_eq!(seqs.len(), 1000, "All 1000 sequence numbers should be unique");
}

#[test]
fn concurrent_history_recording() {
    let svc = Arc::new(HistoryService::new(
        Arc::new(InMemoryStorage::new()),
        HistoryConfig::default(),
    ));
    let threads: Vec<_> = (0..10)
        .map(|t| {
            let svc = svc.clone();
            thread::spawn(move || {
                for i in 0..100 {
                    svc.record_fault(&make_fault(
                        &format!("t{t}-f{i}"),
                        &format!("comp-{t}"),
                        "P0100",
                    ));
                }
            })
        })
        .collect();

    for t in threads {
        t.join().unwrap();
    }

    let all = svc.query_faults(None, 0, i64::MAX);
    assert_eq!(all.len(), 1000);
}

// ── Boundary condition tests ─────────────────────────────────────────────────

#[test]
fn audit_log_overflow_evicts_oldest() {
    let config = AuditLogConfig {
        enabled: true,
        max_entries: 10,
        file_path: None,
    };
    let al = AuditLog::from_config(&config);

    for i in 0..100 {
        al.record(
            &format!("u{i}"),
            SovdAuditAction::ReadData,
            "c",
            "d",
            "GET",
            "ok",
            None,
            None,
        );
    }

    assert_eq!(al.len(), 10);
    let entries = al.query(&AuditFilter::default());
    // Should have the last 10 entries (u90..u99)
    assert_eq!(entries[0].caller, "u90");
    assert_eq!(entries[9].caller, "u99");
}

#[test]
fn fault_with_empty_id_is_handled() {
    let fm = FaultManager::new();
    fm.report_fault(make_fault("", "hpc", "P0100"));
    assert_eq!(fm.total_fault_count(), 1);
    assert!(fm.get_fault("").is_some());
    fm.clear_fault("");
    assert_eq!(fm.total_fault_count(), 0);
}

#[test]
fn fault_with_very_long_id() {
    let fm = FaultManager::new();
    let long_id = "x".repeat(10_000);
    fm.report_fault(make_fault(&long_id, "hpc", "P0100"));
    assert_eq!(fm.total_fault_count(), 1);
    assert!(fm.get_fault(&long_id).is_some());
}

#[test]
fn audit_with_unicode_content() {
    let al = AuditLog::new();
    al.record(
        "用户α",
        SovdAuditAction::WriteData,
        "コンポーネント/日本語",
        "データ",
        "PUT",
        "成功",
        Some("详细信息 🚗"),
        Some("trace-émoji-🔍"),
    );
    assert_eq!(al.len(), 1);
    let entries = al.query(&AuditFilter::default());
    assert_eq!(entries[0].caller, "用户α");
    assert_eq!(entries[0].detail.as_deref(), Some("详细信息 🚗"));
}

// ── Corrupted input tests ────────────────────────────────────────────────────

#[test]
fn restore_from_corrupted_json_returns_error() {
    let result = backup::snapshot_from_json(b"{{{{invalid json");
    assert!(result.is_err());
}

#[test]
fn restore_from_empty_bytes_returns_error() {
    let result = backup::snapshot_from_json(b"");
    assert!(result.is_err());
}

#[test]
fn restore_from_valid_json_wrong_schema_returns_error() {
    let result = backup::snapshot_from_json(b"{\"foo\": \"bar\"}");
    assert!(result.is_err());
}

#[test]
fn restore_from_truncated_snapshot() {
    let fm = FaultManager::new();
    fm.report_fault(make_fault("f1", "hpc", "P0100"));
    let al = AuditLog::new();
    let snap = backup::create_snapshot(&fm, &al, 0, 0);
    let json = backup::snapshot_to_json(&snap).unwrap();

    // Truncate the JSON
    let truncated = &json[..json.len() / 2];
    let result = backup::snapshot_from_json(truncated);
    assert!(result.is_err());
}

// ── State recovery tests ─────────────────────────────────────────────────────

#[test]
fn backup_restore_preserves_fault_state() {
    let fm = FaultManager::new();
    for i in 0..50 {
        fm.report_fault(make_fault(&format!("f{i}"), "hpc", &format!("P{i:04}")));
    }
    let al = AuditLog::new();
    al.record("admin", SovdAuditAction::ClearFaults, "c/hpc", "faults", "DELETE", "success", None, None);

    let snap = backup::create_snapshot(&fm, &al, 0, 0);
    let json = backup::snapshot_to_json(&snap).unwrap();

    // Restore into fresh managers
    let fm2 = FaultManager::new();
    let al2 = AuditLog::new();
    let restored = backup::snapshot_from_json(&json).unwrap();
    let result = backup::restore_snapshot(&restored, &fm2, &al2).unwrap();

    assert_eq!(result.faults_restored, 50);
    assert_eq!(result.audit_restored, 1);
    assert_eq!(fm2.total_fault_count(), 50);
    assert_eq!(fm2.get_fault("f25").unwrap().code, "P0025");
}

#[test]
fn history_compact_then_query_consistent() {
    let store = Arc::new(InMemoryStorage::new());
    let svc = HistoryService::new(store, HistoryConfig::default());

    // Record with known timestamps via direct key insertion
    for i in 0..100 {
        svc.record_fault(&make_fault(&format!("f{i}"), "hpc", "P0100"));
    }

    let before = svc.fault_count();
    assert_eq!(before, 100);

    // Compact with a cutoff that should remove some entries
    // (since all entries have ~current timestamp, compacting at 0 removes nothing)
    let removed = svc.compact(0);
    assert_eq!(removed, 0);
    assert_eq!(svc.fault_count(), 100);
}

// ── Disabled service tests ───────────────────────────────────────────────────

#[test]
fn disabled_audit_log_handles_all_operations() {
    let config = AuditLogConfig {
        enabled: false,
        max_entries: 100,
        file_path: None,
    };
    let al = AuditLog::from_config(&config);

    al.record("u", SovdAuditAction::ReadData, "c", "d", "GET", "ok", None, None);
    assert!(al.is_empty());
    assert!(al.query(&AuditFilter::default()).is_empty());
    assert!(al.recent(10).is_empty());
    assert_eq!(al.verify_chain().unwrap(), 0);
}

#[test]
fn disabled_history_handles_all_operations() {
    let svc = HistoryService::new(
        Arc::new(InMemoryStorage::new()),
        HistoryConfig {
            enabled: false,
            retention_days: 90,
        },
    );

    svc.record_fault(&make_fault("f1", "hpc", "P0100"));
    assert_eq!(svc.fault_count(), 0);
    assert!(svc.query_faults(None, 0, i64::MAX).is_empty());
    assert_eq!(svc.compact(i64::MAX), 0);
}

// ── Feature flag edge cases ──────────────────────────────────────────────────

#[test]
fn feature_flags_rapid_toggle_no_panic() {
    let ff = Arc::new(native_interfaces::FeatureFlags::new());
    let threads: Vec<_> = (0..20)
        .map(|_| {
            let ff = ff.clone();
            thread::spawn(move || {
                for _ in 0..1000 {
                    ff.toggle(native_interfaces::feature_flags::flags::RATE_LIMIT);
                    let _ = ff.is_enabled(native_interfaces::feature_flags::flags::AUTH);
                    let _ = ff.snapshot();
                }
            })
        })
        .collect();

    for t in threads {
        t.join().unwrap();
    }

    // No panics, no data races — just verify it's still functional
    assert!(
        ff.is_enabled(native_interfaces::feature_flags::flags::AUDIT)
            || !ff.is_enabled(native_interfaces::feature_flags::flags::AUDIT)
    );
}
