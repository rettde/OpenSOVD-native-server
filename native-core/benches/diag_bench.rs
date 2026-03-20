// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Criterion Benchmarks (T2.1) — Diagnostic core hot-path performance
//
// Run:
//   cargo bench -p native-core
//
// Benchmarks:
//   - FaultManager: report, get, clear, list
//   - AuditLog: record, query, verify_chain
//   - HistoryService: record_fault, query_faults, compact
//   - Backup: create_snapshot, snapshot_to_json roundtrip
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use native_core::{
    AuditLog, FaultManager, HistoryConfig, HistoryService,
};
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

// ── FaultManager benchmarks ──────────────────────────────────────────────────

fn bench_fault_manager(c: &mut Criterion) {
    let mut group = c.benchmark_group("fault_manager");

    group.bench_function("report_fault", |b| {
        let fm = FaultManager::new();
        let mut i = 0u64;
        b.iter(|| {
            i += 1;
            fm.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
        });
    });

    group.bench_function("get_fault", |b| {
        let fm = FaultManager::new();
        for i in 0..1000 {
            fm.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
        }
        b.iter(|| {
            black_box(fm.get_fault("f500"));
        });
    });

    group.bench_function("get_faults_for_component_1000", |b| {
        let fm = FaultManager::new();
        for i in 0..1000 {
            fm.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
        }
        b.iter(|| {
            black_box(fm.get_faults_for_component("hpc"));
        });
    });

    group.bench_function("clear_fault", |b| {
        let fm = FaultManager::new();
        let mut i = 0u64;
        b.iter_custom(|iters| {
            // Pre-populate
            for _ in 0..iters {
                i += 1;
                fm.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
            }
            let start = std::time::Instant::now();
            for j in (i - iters + 1)..=i {
                fm.clear_fault(&format!("f{j}"));
            }
            start.elapsed()
        });
    });

    group.finish();
}

// ── AuditLog benchmarks ─────────────────────────────────────────────────────

fn bench_audit_log(c: &mut Criterion) {
    let mut group = c.benchmark_group("audit_log");

    group.bench_function("record", |b| {
        let al = AuditLog::new();
        b.iter(|| {
            al.record(
                "user",
                SovdAuditAction::ReadData,
                "component/hpc",
                "data",
                "GET",
                "success",
                None,
                None,
            );
        });
    });

    group.bench_function("query_100_of_1000", |b| {
        let al = AuditLog::new();
        for _ in 0..1000 {
            al.record("u", SovdAuditAction::ReadData, "c", "d", "GET", "ok", None, None);
        }
        let filter = native_core::audit_log::AuditFilter {
            caller: None,
            action: None,
            target: None,
            outcome: None,
            limit: Some(100),
        };
        b.iter(|| {
            black_box(al.query(&filter));
        });
    });

    group.bench_function("verify_chain_1000", |b| {
        let al = AuditLog::new();
        for _ in 0..1000 {
            al.record("u", SovdAuditAction::ReadData, "c", "d", "GET", "ok", None, None);
        }
        b.iter(|| {
            black_box(al.verify_chain());
        });
    });

    group.finish();
}

// ── HistoryService benchmarks ────────────────────────────────────────────────

fn bench_history(c: &mut Criterion) {
    let mut group = c.benchmark_group("history");

    group.bench_function("record_fault", |b| {
        let svc = HistoryService::new(
            Arc::new(InMemoryStorage::new()),
            HistoryConfig::default(),
        );
        let fault = make_fault("f1", "hpc", "P0100");
        b.iter(|| {
            svc.record_fault(black_box(&fault));
        });
    });

    group.bench_function("query_faults_100_of_1000", |b| {
        let store = Arc::new(InMemoryStorage::new());
        let svc = HistoryService::new(store, HistoryConfig::default());
        for i in 0..1000 {
            svc.record_fault(&make_fault(&format!("f{i}"), "hpc", "P0100"));
        }
        b.iter(|| {
            black_box(svc.query_faults(Some("hpc"), 0, i64::MAX));
        });
    });

    group.bench_function("compact_500_of_1000", |b| {
        b.iter_custom(|iters| {
            let mut total = std::time::Duration::ZERO;
            for _ in 0..iters {
                let store = Arc::new(InMemoryStorage::new());
                let svc = HistoryService::new(store, HistoryConfig::default());
                for i in 0..1000 {
                    svc.record_fault(&make_fault(&format!("f{i}"), "hpc", "P0100"));
                }
                let start = std::time::Instant::now();
                svc.compact(i64::MAX / 2);
                total += start.elapsed();
            }
            total
        });
    });

    group.finish();
}

// ── Backup benchmarks ────────────────────────────────────────────────────────

fn bench_backup(c: &mut Criterion) {
    let mut group = c.benchmark_group("backup");

    group.bench_function("create_snapshot_100_faults", |b| {
        let fm = FaultManager::new();
        for i in 0..100 {
            fm.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
        }
        let al = AuditLog::new();
        for _ in 0..100 {
            al.record("u", SovdAuditAction::ReadData, "c", "d", "GET", "ok", None, None);
        }
        b.iter(|| {
            black_box(native_core::create_snapshot(&fm, &al, 0, 0));
        });
    });

    group.bench_function("snapshot_json_roundtrip", |b| {
        let fm = FaultManager::new();
        for i in 0..50 {
            fm.report_fault(make_fault(&format!("f{i}"), "hpc", "P0100"));
        }
        let al = AuditLog::new();
        let snap = native_core::create_snapshot(&fm, &al, 0, 0);
        b.iter(|| {
            let json = native_core::snapshot_to_json(black_box(&snap)).unwrap();
            black_box(native_core::snapshot_from_json(&json).unwrap());
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_fault_manager,
    bench_audit_log,
    bench_history,
    bench_backup,
);
criterion_main!(benches);
