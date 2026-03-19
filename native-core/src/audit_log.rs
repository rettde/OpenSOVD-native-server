// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Audit Log — Tamper-resistant trail of security-relevant actions (Wave 1)
//
// Records every mutating or security-sensitive action with caller identity,
// timestamp, action type, target, and outcome.
//
// Two backends:
//   1. In-memory ring buffer (bounded, for REST API queries)
//   2. Optional append-only JSONL file (for persistence / SIEM integration)
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use native_interfaces::sovd::{SovdAuditAction, SovdAuditEntry};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// Thread-safe audit log with bounded in-memory buffer and optional file sink.
pub struct AuditLog {
    entries: Mutex<VecDeque<SovdAuditEntry>>,
    max_entries: usize,
    seq_counter: AtomicU64,
    /// SHA-256 hash of the most recent entry (for hash-chain linking)
    prev_hash: Mutex<String>,
    /// Optional append-only JSONL file for tamper-resistant persistence
    file_sink: Option<Mutex<std::io::BufWriter<std::fs::File>>>,
    /// Whether the audit log is enabled
    enabled: bool,
}

/// Configuration for the audit log.
#[derive(Debug, Clone, Default)]
pub struct AuditLogConfig {
    /// Enable the audit log (default: true)
    pub enabled: bool,
    /// Maximum number of entries in the in-memory ring buffer
    pub max_entries: usize,
    /// Optional path for append-only JSONL file sink
    pub file_path: Option<String>,
}

/// Filter criteria for querying audit entries.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Filter by caller identity
    pub caller: Option<String>,
    /// Filter by action type
    pub action: Option<SovdAuditAction>,
    /// Filter by target (prefix match)
    pub target: Option<String>,
    /// Filter by outcome
    pub outcome: Option<String>,
    /// Maximum number of results
    pub limit: Option<usize>,
}

impl AuditLog {
    /// Create a new audit log with default settings (enabled, 10k entries, no file).
    /// Genesis hash — the `prev_hash` value for the very first entry.
    const GENESIS: &'static str = "genesis";

    pub fn new() -> Self {
        Self {
            entries: Mutex::new(VecDeque::with_capacity(DEFAULT_MAX_ENTRIES)),
            max_entries: DEFAULT_MAX_ENTRIES,
            seq_counter: AtomicU64::new(1),
            prev_hash: Mutex::new(Self::GENESIS.to_owned()),
            file_sink: None,
            enabled: true,
        }
    }

    /// Create an audit log from configuration.
    pub fn from_config(config: &AuditLogConfig) -> Self {
        let max = if config.max_entries > 0 {
            config.max_entries
        } else {
            DEFAULT_MAX_ENTRIES
        };

        let file_sink = config.file_path.as_ref().and_then(|path| {
            // Ensure parent directory exists
            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.exists() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        warn!(path = %path, error = %e, "Failed to create audit log directory");
                        return None;
                    }
                }
            }
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => {
                    debug!(path = %path, "Audit log file sink opened");
                    Some(Mutex::new(std::io::BufWriter::new(file)))
                }
                Err(e) => {
                    warn!(path = %path, error = %e, "Failed to open audit log file");
                    None
                }
            }
        });

        Self {
            entries: Mutex::new(VecDeque::with_capacity(max)),
            max_entries: max,
            seq_counter: AtomicU64::new(1),
            prev_hash: Mutex::new(Self::GENESIS.to_owned()),
            file_sink,
            enabled: config.enabled,
        }
    }

    /// Record an audit event. This is the primary entry point.
    ///
    /// Caller provides action metadata; the audit log adds seq number and timestamp.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        caller: &str,
        action: SovdAuditAction,
        target: &str,
        resource: &str,
        method: &str,
        outcome: &str,
        detail: Option<&str>,
        trace_id: Option<&str>,
    ) {
        if !self.enabled {
            return;
        }

        // Build entry without hash fields first (used as hash input)
        let mut entry = SovdAuditEntry {
            seq: self.seq_counter.fetch_add(1, Ordering::Relaxed),
            timestamp: chrono::Utc::now().to_rfc3339(),
            caller: caller.to_owned(),
            action,
            target: target.to_owned(),
            resource: resource.to_owned(),
            method: method.to_owned(),
            outcome: outcome.to_owned(),
            detail: detail.map(ToOwned::to_owned),
            trace_id: trace_id.map(ToOwned::to_owned),
            prev_hash: None,
            hash: None,
        };

        // Hash chaining: prev_hash ← previous entry's hash, hash ← SHA-256(prev_hash + entry)
        let mut prev = self.prev_hash.lock().unwrap_or_else(|e| e.into_inner());
        entry.prev_hash = Some(prev.clone());
        let hash = Self::compute_hash(&prev, &entry);
        entry.hash = Some(hash.clone());
        *prev = hash;
        drop(prev);

        // Write to file sink first (before memory, so file is always ahead)
        if let Some(ref sink) = self.file_sink {
            if let Ok(mut writer) = sink.lock() {
                if let Ok(json) = serde_json::to_string(&entry) {
                    let _ = writeln!(writer, "{json}");
                    let _ = writer.flush();
                }
            }
        }

        // Append to in-memory ring buffer
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= self.max_entries {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// Query audit entries with optional filters.
    pub fn query(&self, filter: &AuditFilter) -> Vec<SovdAuditEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let limit = filter.limit.unwrap_or(usize::MAX);

        entries
            .iter()
            .filter(|e| {
                if let Some(ref c) = filter.caller {
                    if e.caller != *c {
                        return false;
                    }
                }
                if let Some(ref a) = filter.action {
                    if e.action != *a {
                        return false;
                    }
                }
                if let Some(ref t) = filter.target {
                    if !e.target.starts_with(t.as_str()) {
                        return false;
                    }
                }
                if let Some(ref o) = filter.outcome {
                    if e.outcome != *o {
                        return false;
                    }
                }
                true
            })
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Get the most recent N entries.
    pub fn recent(&self, count: usize) -> Vec<SovdAuditEntry> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .rev()
            .take(count)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Total number of entries currently in the in-memory buffer.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether the in-memory buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the audit log is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Verify the hash chain integrity of all in-memory entries.
    ///
    /// Returns `Ok(count)` with the number of verified entries, or
    /// `Err(msg)` describing the first broken link.
    pub fn verify_chain(&self) -> Result<usize, String> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut expected_prev = Self::GENESIS.to_owned();

        for (i, entry) in entries.iter().enumerate() {
            let prev = entry.prev_hash.as_deref().unwrap_or("<missing>");
            if prev != expected_prev {
                return Err(format!(
                    "Chain broken at seq {}: expected prev_hash '{}', got '{}'",
                    entry.seq, expected_prev, prev
                ));
            }
            let computed = Self::compute_hash(&expected_prev, entry);
            let stored = entry.hash.as_deref().unwrap_or("<missing>");
            if computed != stored {
                return Err(format!(
                    "Hash mismatch at seq {}: computed '{}', stored '{}'",
                    entry.seq, computed, stored
                ));
            }
            expected_prev = computed;
            let _ = i; // suppress unused warning in older compilers
        }
        Ok(entries.len())
    }

    /// Compute SHA-256 hash over `prev_hash` concatenated with entry's content fields.
    fn compute_hash(prev_hash: &str, entry: &SovdAuditEntry) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prev_hash.as_bytes());
        hasher.update(entry.seq.to_le_bytes());
        hasher.update(entry.timestamp.as_bytes());
        hasher.update(entry.caller.as_bytes());
        // Action as its JSON-serialized form for determinism
        if let Ok(action_str) = serde_json::to_string(&entry.action) {
            hasher.update(action_str.as_bytes());
        }
        hasher.update(entry.target.as_bytes());
        hasher.update(entry.resource.as_bytes());
        hasher.update(entry.method.as_bytes());
        hasher.update(entry.outcome.as_bytes());
        if let Some(ref d) = entry.detail {
            hasher.update(d.as_bytes());
        }
        if let Some(ref t) = entry.trace_id {
            hasher.update(t.as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    /// Flush the file sink (if any). Called during graceful shutdown to ensure
    /// all buffered audit entries are persisted before the process exits.
    pub fn flush(&self) {
        if let Some(ref sink) = self.file_sink {
            if let Ok(mut writer) = sink.lock() {
                let _ = writer.flush();
                debug!("Audit log file sink flushed");
            }
        }
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn make_log() -> AuditLog {
        AuditLog::new()
    }

    fn record_sample(log: &AuditLog, caller: &str, action: SovdAuditAction, target: &str) {
        log.record(caller, action, target, "data", "GET", "success", None, None);
    }

    #[test]
    fn record_and_retrieve() {
        let log = make_log();
        record_sample(&log, "user-1", SovdAuditAction::ReadData, "component/hpc");
        record_sample(&log, "user-2", SovdAuditAction::WriteData, "component/hpc");
        assert_eq!(log.len(), 2);

        let all = log.query(&AuditFilter::default());
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].caller, "user-1");
        assert_eq!(all[1].caller, "user-2");
    }

    #[test]
    fn sequence_numbers_are_monotonic() {
        let log = make_log();
        record_sample(&log, "u", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "u", SovdAuditAction::WriteData, "c/hpc");
        record_sample(&log, "u", SovdAuditAction::ClearFaults, "c/brake");

        let all = log.query(&AuditFilter::default());
        assert_eq!(all[0].seq, 1);
        assert_eq!(all[1].seq, 2);
        assert_eq!(all[2].seq, 3);
    }

    #[test]
    fn filter_by_caller() {
        let log = make_log();
        record_sample(&log, "alice", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "bob", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "alice", SovdAuditAction::WriteData, "c/hpc");

        let filter = AuditFilter {
            caller: Some("alice".into()),
            ..Default::default()
        };
        let results = log.query(&filter);
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.caller == "alice"));
    }

    #[test]
    fn filter_by_action() {
        let log = make_log();
        record_sample(&log, "u", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "u", SovdAuditAction::WriteData, "c/hpc");
        record_sample(&log, "u", SovdAuditAction::ReadData, "c/brake");

        let filter = AuditFilter {
            action: Some(SovdAuditAction::ReadData),
            ..Default::default()
        };
        let results = log.query(&filter);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn filter_by_target_prefix() {
        let log = make_log();
        record_sample(&log, "u", SovdAuditAction::ReadData, "component/hpc");
        record_sample(&log, "u", SovdAuditAction::ReadData, "component/brake");
        record_sample(&log, "u", SovdAuditAction::ReadData, "app/health");

        let filter = AuditFilter {
            target: Some("component/".into()),
            ..Default::default()
        };
        let results = log.query(&filter);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn filter_by_outcome() {
        let log = make_log();
        log.record(
            "u",
            SovdAuditAction::ReadData,
            "c/hpc",
            "data",
            "GET",
            "success",
            None,
            None,
        );
        log.record(
            "u",
            SovdAuditAction::WriteData,
            "c/hpc",
            "data",
            "PUT",
            "denied",
            None,
            None,
        );
        log.record(
            "u",
            SovdAuditAction::ReadData,
            "c/hpc",
            "data",
            "GET",
            "error",
            None,
            None,
        );

        let filter = AuditFilter {
            outcome: Some("success".into()),
            ..Default::default()
        };
        assert_eq!(log.query(&filter).len(), 1);
    }

    #[test]
    fn filter_with_limit() {
        let log = make_log();
        for i in 0..10 {
            record_sample(&log, &format!("u{i}"), SovdAuditAction::ReadData, "c/hpc");
        }

        let filter = AuditFilter {
            limit: Some(3),
            ..Default::default()
        };
        let results = log.query(&filter);
        assert_eq!(results.len(), 3);
        // Should return the last 3 (most recent)
        assert_eq!(results[0].caller, "u7");
        assert_eq!(results[2].caller, "u9");
    }

    #[test]
    fn evicts_oldest_when_full() {
        let config = AuditLogConfig {
            enabled: true,
            max_entries: 3,
            file_path: None,
        };
        let log = AuditLog::from_config(&config);
        record_sample(&log, "a", SovdAuditAction::ReadData, "c");
        record_sample(&log, "b", SovdAuditAction::ReadData, "c");
        record_sample(&log, "c", SovdAuditAction::ReadData, "c");
        record_sample(&log, "d", SovdAuditAction::ReadData, "c");
        assert_eq!(log.len(), 3);

        let all = log.query(&AuditFilter::default());
        assert_eq!(all[0].caller, "b");
        assert_eq!(all[2].caller, "d");
    }

    #[test]
    fn recent_returns_last_n() {
        let log = make_log();
        for i in 0..10 {
            record_sample(&log, &format!("u{i}"), SovdAuditAction::ReadData, "c");
        }
        let recent = log.recent(3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].caller, "u7");
        assert_eq!(recent[2].caller, "u9");
    }

    #[test]
    fn disabled_log_does_not_record() {
        let config = AuditLogConfig {
            enabled: false,
            max_entries: 100,
            file_path: None,
        };
        let log = AuditLog::from_config(&config);
        record_sample(&log, "u", SovdAuditAction::ReadData, "c");
        assert!(log.is_empty());
        assert!(!log.is_enabled());
    }

    #[test]
    fn detail_and_trace_id_are_recorded() {
        let log = make_log();
        log.record(
            "admin",
            SovdAuditAction::ClearFaults,
            "component/brake",
            "faults",
            "DELETE",
            "success",
            Some("Cleared 3 DTCs"),
            Some("abc123"),
        );
        let entry = &log.query(&AuditFilter::default())[0];
        assert_eq!(entry.detail.as_deref(), Some("Cleared 3 DTCs"));
        assert_eq!(entry.trace_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn entry_serializes_to_json() {
        let log = make_log();
        log.record(
            "tech",
            SovdAuditAction::InstallPackage,
            "component/hpc",
            "software-packages/pkg-1",
            "POST",
            "success",
            None,
            None,
        );
        let entry = &log.query(&AuditFilter::default())[0];
        let json = serde_json::to_string(entry).unwrap();
        assert!(json.contains("\"installPackage\""));
        assert!(json.contains("\"component/hpc\""));
        assert!(!json.contains("\"detail\"")); // None fields skipped
    }

    #[test]
    fn file_sink_writes_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let config = AuditLogConfig {
            enabled: true,
            max_entries: 100,
            file_path: Some(path.to_string_lossy().into_owned()),
        };
        let log = AuditLog::from_config(&config);
        record_sample(&log, "u1", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "u2", SovdAuditAction::WriteData, "c/hpc");

        // Read file and verify
        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line should be valid JSON with hash fields
        let entry: SovdAuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(entry.caller, "u1");
        assert!(entry.hash.is_some());
        assert!(entry.prev_hash.is_some());
    }

    // ── Hash chaining tests (E1.1) ──────────────────────────────────────

    #[test]
    fn entries_have_hash_and_prev_hash() {
        let log = make_log();
        record_sample(&log, "u1", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "u2", SovdAuditAction::WriteData, "c/hpc");

        let all = log.query(&AuditFilter::default());
        // First entry: prev_hash = "genesis"
        assert_eq!(all[0].prev_hash.as_deref(), Some("genesis"));
        assert!(all[0].hash.is_some());
        // Second entry: prev_hash = first entry's hash
        assert_eq!(all[1].prev_hash, all[0].hash);
        assert!(all[1].hash.is_some());
        // Hashes are different
        assert_ne!(all[0].hash, all[1].hash);
    }

    #[test]
    fn verify_chain_succeeds_on_valid_log() {
        let log = make_log();
        for i in 0..10 {
            record_sample(&log, &format!("u{i}"), SovdAuditAction::ReadData, "c");
        }
        let result = log.verify_chain();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 10);
    }

    #[test]
    fn verify_chain_detects_tampered_entry() {
        let log = make_log();
        record_sample(&log, "u1", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "u2", SovdAuditAction::WriteData, "c/hpc");

        // Tamper with an entry
        {
            let mut entries = log.entries.lock().unwrap();
            entries[1].caller = "TAMPERED".to_owned();
        }

        let result = log.verify_chain();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("Hash mismatch"), "Got: {msg}");
    }

    #[test]
    fn verify_chain_detects_broken_prev_hash() {
        let log = make_log();
        record_sample(&log, "u1", SovdAuditAction::ReadData, "c/hpc");
        record_sample(&log, "u2", SovdAuditAction::WriteData, "c/hpc");

        // Break the chain link
        {
            let mut entries = log.entries.lock().unwrap();
            entries[1].prev_hash = Some("bogus".to_owned());
        }

        let result = log.verify_chain();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("Chain broken"), "Got: {msg}");
    }

    #[test]
    fn verify_chain_empty_log_is_ok() {
        let log = make_log();
        assert_eq!(log.verify_chain().unwrap(), 0);
    }

    #[test]
    fn hash_is_deterministic() {
        // Same input → same hash
        let entry = SovdAuditEntry {
            seq: 1,
            timestamp: "2026-01-01T00:00:00Z".to_owned(),
            caller: "test".to_owned(),
            action: SovdAuditAction::ReadData,
            target: "c/hpc".to_owned(),
            resource: "data".to_owned(),
            method: "GET".to_owned(),
            outcome: "success".to_owned(),
            detail: None,
            trace_id: None,
            prev_hash: None,
            hash: None,
        };
        let h1 = AuditLog::compute_hash("genesis", &entry);
        let h2 = AuditLog::compute_hash("genesis", &entry);
        assert_eq!(h1, h2);
        // SHA-256 hex output is 64 chars
        assert_eq!(h1.len(), 64);
    }
}
