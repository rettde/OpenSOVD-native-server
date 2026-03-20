// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// StorageBackend (A2.1 + F9) — pluggable key-value persistence abstraction
//
// Consumers (AuditLog, FaultManager, KPI store, etc.) depend on this trait
// instead of a concrete storage engine. Implementations:
//   - InMemoryStorage  (default, for tests and lightweight deployments)
//   - SledStorage      (crash-safe embedded KV, behind `persist` feature)
//   - Future: Redis, PostgreSQL, S3, etc.
//
// F9 adds snapshot/rollback support following the AUTOSAR Adaptive Platform
// Persistency pattern (ara::per) and Eclipse S-CORE KvsBackend design:
//   - create_snapshot()   — capture current state
//   - list_snapshots()    — enumerate available snapshots
//   - restore_snapshot()  — rollback to a previous state
//   - delete_snapshot()   — remove a snapshot
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Unique identifier for a storage snapshot.
pub type SnapshotId = u64;

/// Internal type for snapshot storage: maps snapshot ID to (metadata, data).
type SnapshotStore = BTreeMap<SnapshotId, (SnapshotInfo, BTreeMap<Vec<u8>, Vec<u8>>)>;

/// Metadata for a stored snapshot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SnapshotInfo {
    /// Unique snapshot identifier.
    pub id: SnapshotId,
    /// ISO 8601 timestamp when the snapshot was created.
    pub created_at: String,
    /// Number of key-value entries captured in the snapshot.
    pub entry_count: usize,
    /// Optional human-readable label (e.g. "pre-update", "factory-reset").
    pub label: Option<String>,
}

/// A pluggable key-value storage backend.
///
/// Keys and values are both byte slices. Higher-level serialization
/// (JSON, bincode, etc.) is the caller's responsibility.
///
/// All operations are synchronous and infallible in the trait contract.
/// Implementations that can fail (network, I/O) should log errors internally
/// and return empty/false as appropriate (fail-open for reads, best-effort for writes).
///
/// ## Snapshot/Rollback (F9, AUTOSAR ara::per / S-CORE pattern)
///
/// Optional snapshot support enables point-in-time capture and rollback of the
/// entire key-value store. Default implementations return empty/false so that
/// backends without snapshot support still compile.
pub trait StorageBackend: Send + Sync + 'static {
    /// Retrieve the value for a key. Returns `None` if the key does not exist.
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;

    /// Insert or update a key-value pair.
    fn put(&self, key: &[u8], value: &[u8]);

    /// Delete a key. Returns `true` if the key existed.
    fn delete(&self, key: &[u8]) -> bool;

    /// List all keys (optionally filtered by a prefix).
    fn list_keys(&self, prefix: Option<&[u8]>) -> Vec<Vec<u8>>;

    /// List all key-value pairs (optionally filtered by a prefix).
    fn list(&self, prefix: Option<&[u8]>) -> Vec<(Vec<u8>, Vec<u8>)>;

    /// Return the total number of entries.
    fn count(&self) -> usize;

    /// Flush any buffered writes to durable storage (no-op for in-memory).
    fn flush(&self) {}

    // ── Snapshot/Rollback (F9) ───────────────────────────────────────────

    /// Capture the current state as a named snapshot.
    ///
    /// Returns the `SnapshotId` on success, or an error message on failure.
    /// Default: unsupported (returns error).
    fn create_snapshot(&self, _label: Option<&str>) -> Result<SnapshotId, String> {
        Err("Snapshots not supported by this backend".to_owned())
    }

    /// List all available snapshots, ordered by creation time (newest first).
    fn list_snapshots(&self) -> Vec<SnapshotInfo> {
        vec![]
    }

    /// Restore the store to the state captured in the given snapshot.
    ///
    /// The current state is **replaced** — all keys not in the snapshot are deleted,
    /// all keys in the snapshot are restored to their captured values.
    /// Returns the number of entries restored, or an error message.
    fn restore_snapshot(&self, _id: SnapshotId) -> Result<usize, String> {
        Err("Snapshots not supported by this backend".to_owned())
    }

    /// Delete a snapshot. Returns `true` if it existed.
    fn delete_snapshot(&self, _id: SnapshotId) -> bool {
        false
    }

    /// Number of snapshots currently stored.
    fn snapshot_count(&self) -> usize {
        0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// InMemoryStorage — default implementation (BTreeMap, sorted keys)
// ─────────────────────────────────────────────────────────────────────────────

/// In-memory storage backend backed by a `BTreeMap` (sorted key order).
///
/// Suitable for tests, development, and lightweight single-instance deployments
/// where persistence across restarts is not required.
///
/// Supports snapshots (F9): up to 16 point-in-time snapshots are kept in memory.
pub struct InMemoryStorage {
    data: Mutex<BTreeMap<Vec<u8>, Vec<u8>>>,
    snapshots: Mutex<SnapshotStore>,
    next_snapshot_id: AtomicU64,
}

impl InMemoryStorage {
    /// Maximum number of in-memory snapshots.
    const MAX_SNAPSHOTS: usize = 16;

    pub fn new() -> Self {
        Self {
            data: Mutex::new(BTreeMap::new()),
            snapshots: Mutex::new(BTreeMap::new()),
            next_snapshot_id: AtomicU64::new(1),
        }
    }
}

impl Default for InMemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageBackend for InMemoryStorage {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        data.get(key).cloned()
    }

    fn put(&self, key: &[u8], value: &[u8]) {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        data.insert(key.to_vec(), value.to_vec());
    }

    fn delete(&self, key: &[u8]) -> bool {
        let mut data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        data.remove(key).is_some()
    }

    fn list_keys(&self, prefix: Option<&[u8]>) -> Vec<Vec<u8>> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match prefix {
            Some(p) => data.keys().filter(|k| k.starts_with(p)).cloned().collect(),
            None => data.keys().cloned().collect(),
        }
    }

    fn list(&self, prefix: Option<&[u8]>) -> Vec<(Vec<u8>, Vec<u8>)> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match prefix {
            Some(p) => data
                .iter()
                .filter(|(k, _)| k.starts_with(p))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            None => data.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        }
    }

    fn count(&self) -> usize {
        self.data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn create_snapshot(&self, label: Option<&str>) -> Result<SnapshotId, String> {
        let data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let id = self.next_snapshot_id.fetch_add(1, Ordering::Relaxed);
        let info = SnapshotInfo {
            id,
            created_at: chrono::Utc::now().to_rfc3339(),
            entry_count: data.len(),
            label: label.map(ToOwned::to_owned),
        };
        let snapshot_data = data.clone();
        drop(data);

        let mut snaps = self
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Evict oldest if at capacity
        while snaps.len() >= Self::MAX_SNAPSHOTS {
            if let Some(&oldest_id) = snaps.keys().next() {
                snaps.remove(&oldest_id);
            }
        }
        snaps.insert(id, (info, snapshot_data));
        Ok(id)
    }

    fn list_snapshots(&self) -> Vec<SnapshotInfo> {
        let snaps = self
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        snaps.values().rev().map(|(info, _)| info.clone()).collect()
    }

    fn restore_snapshot(&self, id: SnapshotId) -> Result<usize, String> {
        let snaps = self
            .snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (_, snapshot_data) = snaps
            .get(&id)
            .ok_or_else(|| format!("Snapshot {id} not found"))?;
        let restored = snapshot_data.clone();
        let count = restored.len();
        drop(snaps);

        let mut data = self
            .data
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *data = restored;
        Ok(count)
    }

    fn delete_snapshot(&self, id: SnapshotId) -> bool {
        self.snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&id)
            .is_some()
    }

    fn snapshot_count(&self) -> usize {
        self.snapshots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn put_and_get() {
        let store = InMemoryStorage::new();
        store.put(b"key1", b"value1");
        assert_eq!(store.get(b"key1"), Some(b"value1".to_vec()));
    }

    #[test]
    fn get_missing_returns_none() {
        let store = InMemoryStorage::new();
        assert_eq!(store.get(b"missing"), None);
    }

    #[test]
    fn put_overwrites() {
        let store = InMemoryStorage::new();
        store.put(b"k", b"v1");
        store.put(b"k", b"v2");
        assert_eq!(store.get(b"k"), Some(b"v2".to_vec()));
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn delete_existing() {
        let store = InMemoryStorage::new();
        store.put(b"k", b"v");
        assert!(store.delete(b"k"));
        assert_eq!(store.get(b"k"), None);
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn delete_missing_returns_false() {
        let store = InMemoryStorage::new();
        assert!(!store.delete(b"nope"));
    }

    #[test]
    fn list_keys_all() {
        let store = InMemoryStorage::new();
        store.put(b"a", b"1");
        store.put(b"b", b"2");
        store.put(b"c", b"3");
        let keys = store.list_keys(None);
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn list_keys_with_prefix() {
        let store = InMemoryStorage::new();
        store.put(b"fault:f1", b"data1");
        store.put(b"fault:f2", b"data2");
        store.put(b"audit:a1", b"data3");

        let fault_keys = store.list_keys(Some(b"fault:"));
        assert_eq!(fault_keys.len(), 2);

        let audit_keys = store.list_keys(Some(b"audit:"));
        assert_eq!(audit_keys.len(), 1);
    }

    #[test]
    fn list_with_prefix() {
        let store = InMemoryStorage::new();
        store.put(b"ns:a", b"1");
        store.put(b"ns:b", b"2");
        store.put(b"other:c", b"3");

        let items = store.list(Some(b"ns:"));
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].1, b"1");
    }

    #[test]
    fn count_reflects_state() {
        let store = InMemoryStorage::new();
        assert_eq!(store.count(), 0);
        store.put(b"a", b"1");
        store.put(b"b", b"2");
        assert_eq!(store.count(), 2);
        store.delete(b"a");
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn flush_is_noop() {
        let store = InMemoryStorage::new();
        store.flush(); // Should not panic
    }

    #[test]
    fn default_creates_empty() {
        let store = InMemoryStorage::default();
        assert_eq!(store.count(), 0);
    }

    // ── Snapshot / Rollback tests (F9) ──────────────────────────────────

    #[test]
    fn create_snapshot_returns_id() {
        let store = InMemoryStorage::new();
        store.put(b"a", b"1");
        store.put(b"b", b"2");
        let id = store.create_snapshot(Some("test")).unwrap();
        assert!(id > 0);
        assert_eq!(store.snapshot_count(), 1);
    }

    #[test]
    fn list_snapshots_returns_newest_first() {
        let store = InMemoryStorage::new();
        store.put(b"x", b"1");
        let id1 = store.create_snapshot(Some("first")).unwrap();
        store.put(b"y", b"2");
        let id2 = store.create_snapshot(Some("second")).unwrap();

        let snaps = store.list_snapshots();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].id, id2); // newest first
        assert_eq!(snaps[1].id, id1);
        assert_eq!(snaps[0].label.as_deref(), Some("second"));
        assert_eq!(snaps[1].entry_count, 1); // first snapshot had 1 entry
        assert_eq!(snaps[0].entry_count, 2); // second snapshot had 2 entries
    }

    #[test]
    fn restore_snapshot_replaces_state() {
        let store = InMemoryStorage::new();
        store.put(b"original", b"value");
        let snap_id = store.create_snapshot(None).unwrap();

        // Mutate state after snapshot
        store.put(b"new_key", b"new_val");
        store.delete(b"original");
        assert_eq!(store.count(), 1);
        assert_eq!(store.get(b"original"), None);

        // Restore
        let restored = store.restore_snapshot(snap_id).unwrap();
        assert_eq!(restored, 1);
        assert_eq!(store.get(b"original"), Some(b"value".to_vec()));
        assert_eq!(store.get(b"new_key"), None); // new key is gone
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn restore_nonexistent_snapshot_fails() {
        let store = InMemoryStorage::new();
        assert!(store.restore_snapshot(999).is_err());
    }

    #[test]
    fn delete_snapshot_removes_it() {
        let store = InMemoryStorage::new();
        let id = store.create_snapshot(None).unwrap();
        assert_eq!(store.snapshot_count(), 1);
        assert!(store.delete_snapshot(id));
        assert_eq!(store.snapshot_count(), 0);
        assert!(!store.delete_snapshot(id)); // already gone
    }

    #[test]
    fn snapshot_evicts_oldest_at_capacity() {
        let store = InMemoryStorage::new();
        let mut first_id = 0;
        for i in 0..InMemoryStorage::MAX_SNAPSHOTS + 2 {
            store.put(format!("k{i}").as_bytes(), b"v");
            let id = store.create_snapshot(None).unwrap();
            if i == 0 {
                first_id = id;
            }
        }
        // Should be capped at MAX_SNAPSHOTS
        assert_eq!(store.snapshot_count(), InMemoryStorage::MAX_SNAPSHOTS);
        // First snapshot should have been evicted
        assert!(store.restore_snapshot(first_id).is_err());
    }

    #[test]
    fn snapshot_info_has_timestamp() {
        let store = InMemoryStorage::new();
        store.create_snapshot(Some("ts-test")).unwrap();
        let snaps = store.list_snapshots();
        assert!(!snaps[0].created_at.is_empty());
        // Should be a valid RFC 3339 timestamp
        assert!(snaps[0].created_at.contains('T'));
    }
}
