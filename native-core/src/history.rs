// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// HistoryService (W2.2) — Time-indexed diagnostic history
//
// Provides time-range queries over faults and audit entries using the
// pluggable StorageBackend trait (A2.1). Keys are encoded as:
//
//   {namespace}:{component_id}:{timestamp_millis_padded}:{id}
//
// BTreeMap-backed InMemoryStorage yields sorted keys, enabling efficient
// range scans via prefix + lexicographic comparison.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use native_interfaces::sovd::{SovdAuditEntry, SovdFault};
use native_interfaces::StorageBackend;
use tracing::debug;

/// Configuration for the history service.
#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// Whether historical recording is enabled
    pub enabled: bool,
    /// Maximum retention in days (0 = unlimited)
    pub retention_days: u32,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 90,
        }
    }
}

/// Time-indexed diagnostic history backed by a `StorageBackend`.
pub struct HistoryService {
    store: Arc<dyn StorageBackend>,
    config: HistoryConfig,
}

const FAULT_NS: &[u8] = b"hist:fault:";
const AUDIT_NS: &[u8] = b"hist:audit:";

impl HistoryService {
    /// Create a new history service with the given storage backend and config.
    pub fn new(store: Arc<dyn StorageBackend>, config: HistoryConfig) -> Self {
        Self { store, config }
    }

    /// Whether the history service is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    // ── Recording ────────────────────────────────────────────────────────

    /// Record a fault snapshot into history.
    pub fn record_fault(&self, fault: &SovdFault) {
        if !self.config.enabled {
            return;
        }
        let ts = chrono::Utc::now().timestamp_millis();
        let key = Self::fault_key(&fault.component_id, ts, &fault.id);
        if let Ok(value) = serde_json::to_vec(fault) {
            self.store.put(&key, &value);
            debug!(fault_id = %fault.id, component = %fault.component_id, "Fault recorded to history");
        }
    }

    /// Record an audit entry into history.
    pub fn record_audit(&self, entry: &SovdAuditEntry) {
        if !self.config.enabled {
            return;
        }
        let ts = Self::parse_timestamp(&entry.timestamp)
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        let key = Self::audit_key(ts, entry.seq);
        if let Ok(value) = serde_json::to_vec(entry) {
            self.store.put(&key, &value);
        }
    }

    // ── Querying ─────────────────────────────────────────────────────────

    /// Query historical faults within a time range.
    ///
    /// - `component_id`: optional filter (None = all components)
    /// - `from_ms` / `to_ms`: Unix timestamps in milliseconds (inclusive)
    pub fn query_faults(
        &self,
        component_id: Option<&str>,
        from_ms: i64,
        to_ms: i64,
    ) -> Vec<SovdFault> {
        let entries = self.store.list(Some(FAULT_NS));
        entries
            .into_iter()
            .filter(|(key, _)| {
                if let Some((cid, ts, _id)) = Self::parse_fault_key(key) {
                    let ts_match = ts >= from_ms && ts <= to_ms;
                    let cid_match = component_id.map_or(true, |c| c == cid);
                    ts_match && cid_match
                } else {
                    false
                }
            })
            .filter_map(|(_, value)| serde_json::from_slice::<SovdFault>(&value).ok())
            .collect()
    }

    /// Query historical audit entries within a time range.
    ///
    /// - `from_ms` / `to_ms`: Unix timestamps in milliseconds (inclusive)
    /// - `limit`: maximum number of entries to return (0 = unlimited)
    pub fn query_audit(&self, from_ms: i64, to_ms: i64, limit: usize) -> Vec<SovdAuditEntry> {
        let entries = self.store.list(Some(AUDIT_NS));
        let iter = entries.into_iter().filter(|(key, _)| {
            if let Some((_ts_parsed, _seq)) = Self::parse_audit_key(key) {
                _ts_parsed >= from_ms && _ts_parsed <= to_ms
            } else {
                false
            }
        });

        let mapped = iter.filter_map(|(_, value)| {
            serde_json::from_slice::<SovdAuditEntry>(&value).ok()
        });

        if limit > 0 {
            mapped.take(limit).collect()
        } else {
            mapped.collect()
        }
    }

    /// Count historical fault entries.
    pub fn fault_count(&self) -> usize {
        self.store.list_keys(Some(FAULT_NS)).len()
    }

    /// Count historical audit entries.
    pub fn audit_count(&self) -> usize {
        self.store.list_keys(Some(AUDIT_NS)).len()
    }

    // ── Maintenance ──────────────────────────────────────────────────────

    /// Remove entries older than the given Unix timestamp (milliseconds).
    /// Returns the number of entries compacted.
    pub fn compact(&self, before_ms: i64) -> usize {
        let mut removed = 0;

        // Compact faults
        let fault_keys = self.store.list_keys(Some(FAULT_NS));
        for key in fault_keys {
            if let Some((_, ts, _)) = Self::parse_fault_key(&key) {
                if ts < before_ms {
                    self.store.delete(&key);
                    removed += 1;
                }
            }
        }

        // Compact audit entries
        let audit_keys = self.store.list_keys(Some(AUDIT_NS));
        for key in audit_keys {
            if let Some((ts, _)) = Self::parse_audit_key(&key) {
                if ts < before_ms {
                    self.store.delete(&key);
                    removed += 1;
                }
            }
        }

        if removed > 0 {
            debug!(removed, before_ms, "History compacted");
        }
        removed
    }

    /// Run retention-based compaction (deletes entries older than `retention_days`).
    /// No-op if retention_days is 0 (unlimited).
    pub fn compact_by_retention(&self) -> usize {
        if self.config.retention_days == 0 {
            return 0;
        }
        let cutoff = chrono::Utc::now().timestamp_millis()
            - i64::from(self.config.retention_days) * 86_400_000;
        self.compact(cutoff)
    }

    // ── Key encoding ─────────────────────────────────────────────────────
    //
    // Keys are designed for lexicographic sorting so BTreeMap range scans work:
    //   fault: "hist:fault:{component_id}:{ts_padded_20}:{fault_id}"
    //   audit: "hist:audit:{ts_padded_20}:{seq_padded_20}"

    fn fault_key(component_id: &str, timestamp_ms: i64, fault_id: &str) -> Vec<u8> {
        format!(
            "hist:fault:{}:{:020}:{}",
            component_id, timestamp_ms, fault_id
        )
        .into_bytes()
    }

    fn audit_key(timestamp_ms: i64, seq: u64) -> Vec<u8> {
        format!("hist:audit:{:020}:{:020}", timestamp_ms, seq).into_bytes()
    }

    fn parse_fault_key(key: &[u8]) -> Option<(String, i64, String)> {
        let s = std::str::from_utf8(key).ok()?;
        let stripped = s.strip_prefix("hist:fault:")?;
        let mut parts = stripped.splitn(3, ':');
        let component_id = parts.next()?.to_owned();
        let ts: i64 = parts.next()?.parse().ok()?;
        let fault_id = parts.next()?.to_owned();
        Some((component_id, ts, fault_id))
    }

    fn parse_audit_key(key: &[u8]) -> Option<(i64, u64)> {
        let s = std::str::from_utf8(key).ok()?;
        let stripped = s.strip_prefix("hist:audit:")?;
        let mut parts = stripped.splitn(2, ':');
        let ts: i64 = parts.next()?.parse().ok()?;
        let seq: u64 = parts.next()?.parse().ok()?;
        Some((ts, seq))
    }

    fn parse_timestamp(rfc3339: &str) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(rfc3339)
            .ok()
            .map(|dt| dt.timestamp_millis())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use native_interfaces::sovd::*;
    use native_interfaces::InMemoryStorage;

    fn make_service() -> HistoryService {
        HistoryService::new(
            Arc::new(InMemoryStorage::new()),
            HistoryConfig::default(),
        )
    }

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

    fn make_audit(seq: u64, caller: &str, action: SovdAuditAction) -> SovdAuditEntry {
        SovdAuditEntry {
            seq,
            timestamp: chrono::Utc::now().to_rfc3339(),
            caller: caller.into(),
            action,
            target: "c/hpc".into(),
            resource: "data".into(),
            method: "GET".into(),
            outcome: "success".into(),
            detail: None,
            trace_id: None,
            prev_hash: None,
            hash: None,
        }
    }

    #[test]
    fn record_and_query_faults() {
        let svc = make_service();
        svc.record_fault(&make_fault("f1", "hpc", "P0100"));
        svc.record_fault(&make_fault("f2", "brake", "P0200"));
        svc.record_fault(&make_fault("f3", "hpc", "P0300"));

        // Query all
        let all = svc.query_faults(None, 0, i64::MAX);
        assert_eq!(all.len(), 3);

        // Query by component
        let hpc = svc.query_faults(Some("hpc"), 0, i64::MAX);
        assert_eq!(hpc.len(), 2);
        assert!(hpc.iter().all(|f| f.component_id == "hpc"));
    }

    #[test]
    fn record_and_query_audit() {
        let svc = make_service();
        svc.record_audit(&make_audit(1, "alice", SovdAuditAction::ReadData));
        svc.record_audit(&make_audit(2, "bob", SovdAuditAction::WriteData));
        svc.record_audit(&make_audit(3, "alice", SovdAuditAction::ClearFaults));

        let all = svc.query_audit(0, i64::MAX, 0);
        assert_eq!(all.len(), 3);

        let limited = svc.query_audit(0, i64::MAX, 2);
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn fault_count() {
        let svc = make_service();
        assert_eq!(svc.fault_count(), 0);
        svc.record_fault(&make_fault("f1", "hpc", "P0100"));
        svc.record_fault(&make_fault("f2", "hpc", "P0200"));
        assert_eq!(svc.fault_count(), 2);
    }

    #[test]
    fn audit_count() {
        let svc = make_service();
        assert_eq!(svc.audit_count(), 0);
        svc.record_audit(&make_audit(1, "u", SovdAuditAction::ReadData));
        assert_eq!(svc.audit_count(), 1);
    }

    #[test]
    fn compact_removes_old_entries() {
        let store = Arc::new(InMemoryStorage::new());
        let svc = HistoryService::new(store, HistoryConfig::default());

        // Record faults with known timestamps
        let old_key = HistoryService::fault_key("hpc", 1000, "old-fault");
        let old_val = serde_json::to_vec(&make_fault("old-fault", "hpc", "P0001")).unwrap();
        svc.store.put(&old_key, &old_val);

        let new_key = HistoryService::fault_key("hpc", 999_999_999_999, "new-fault");
        let new_val = serde_json::to_vec(&make_fault("new-fault", "hpc", "P0002")).unwrap();
        svc.store.put(&new_key, &new_val);

        // Also add old audit entry
        let old_audit_key = HistoryService::audit_key(500, 1);
        let old_audit_val = serde_json::to_vec(&make_audit(1, "u", SovdAuditAction::ReadData)).unwrap();
        svc.store.put(&old_audit_key, &old_audit_val);

        assert_eq!(svc.fault_count(), 2);
        assert_eq!(svc.audit_count(), 1);

        // Compact entries older than ts=2000
        let removed = svc.compact(2000);
        assert_eq!(removed, 2); // old fault + old audit

        assert_eq!(svc.fault_count(), 1);
        assert_eq!(svc.audit_count(), 0);
    }

    #[test]
    fn time_range_filter_works() {
        let store = Arc::new(InMemoryStorage::new());
        let svc = HistoryService::new(store, HistoryConfig::default());

        // Manually insert faults with specific timestamps
        for (ts, id) in [(1000, "f1"), (2000, "f2"), (3000, "f3"), (4000, "f4")] {
            let key = HistoryService::fault_key("hpc", ts, id);
            let val = serde_json::to_vec(&make_fault(id, "hpc", "P0100")).unwrap();
            svc.store.put(&key, &val);
        }

        // Query range 2000..3000 (inclusive)
        let result = svc.query_faults(None, 2000, 3000);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn disabled_service_does_not_record() {
        let svc = HistoryService::new(
            Arc::new(InMemoryStorage::new()),
            HistoryConfig {
                enabled: false,
                retention_days: 90,
            },
        );
        svc.record_fault(&make_fault("f1", "hpc", "P0100"));
        svc.record_audit(&make_audit(1, "u", SovdAuditAction::ReadData));
        assert_eq!(svc.fault_count(), 0);
        assert_eq!(svc.audit_count(), 0);
        assert!(!svc.is_enabled());
    }

    #[test]
    fn empty_query_returns_empty() {
        let svc = make_service();
        assert!(svc.query_faults(None, 0, i64::MAX).is_empty());
        assert!(svc.query_audit(0, i64::MAX, 0).is_empty());
    }

    #[test]
    fn key_encoding_is_lexicographically_sorted() {
        // Smaller timestamp → smaller key (lexicographic)
        let k1 = HistoryService::fault_key("hpc", 1000, "f1");
        let k2 = HistoryService::fault_key("hpc", 2000, "f2");
        assert!(k1 < k2);
    }

    #[test]
    fn parse_fault_key_roundtrip() {
        let key = HistoryService::fault_key("hpc", 1710900000000, "P0123");
        let (cid, ts, fid) = HistoryService::parse_fault_key(&key).unwrap();
        assert_eq!(cid, "hpc");
        assert_eq!(ts, 1710900000000);
        assert_eq!(fid, "P0123");
    }

    #[test]
    fn parse_audit_key_roundtrip() {
        let key = HistoryService::audit_key(1710900000000, 42);
        let (ts, seq) = HistoryService::parse_audit_key(&key).unwrap();
        assert_eq!(ts, 1710900000000);
        assert_eq!(seq, 42);
    }

    #[test]
    fn compact_by_retention_respects_config() {
        let store = Arc::new(InMemoryStorage::new());
        let svc = HistoryService::new(
            store,
            HistoryConfig {
                enabled: true,
                retention_days: 0, // unlimited
            },
        );

        // Insert very old entry
        let key = HistoryService::fault_key("hpc", 1, "ancient");
        let val = serde_json::to_vec(&make_fault("ancient", "hpc", "P0001")).unwrap();
        svc.store.put(&key, &val);

        // unlimited retention → no compaction
        assert_eq!(svc.compact_by_retention(), 0);
        assert_eq!(svc.fault_count(), 1);
    }

    #[test]
    fn query_faults_nonexistent_component_returns_empty() {
        let svc = make_service();
        svc.record_fault(&make_fault("f1", "hpc", "P0100"));
        let result = svc.query_faults(Some("nonexistent"), 0, i64::MAX);
        assert!(result.is_empty());
    }
}
