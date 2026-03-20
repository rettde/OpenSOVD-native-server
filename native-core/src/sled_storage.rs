// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// SledStorage (F1) — crash-safe embedded key-value persistence
//
// Implements `StorageBackend` using sled (embedded B-tree engine).
// Feature-gated behind `persist` — when disabled, only `InMemoryStorage`
// is available.
//
// Usage:
//   let storage = SledStorage::open("/tmp/sovd-data")?;
//   storage.put(b"fault:123", b"{...}");
//   let val = storage.get(b"fault:123");
// ─────────────────────────────────────────────────────────────────────────────

use native_interfaces::storage::{SnapshotId, SnapshotInfo};
use native_interfaces::StorageBackend;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Persistent key-value storage backed by sled (embedded B-tree).
///
/// All writes are immediately durable (sled flushes on commit).
/// Thread-safe: sled::Db is internally `Send + Sync`.
///
/// Supports snapshots (F9): each snapshot is stored as a separate sled tree
/// named `snap:<id>`, with metadata in `snap_meta:<id>`.
pub struct SledStorage {
    db: sled::Db,
    next_snapshot_id: AtomicU64,
}

impl SledStorage {
    /// Evict oldest snapshots if over capacity.
    fn evict_oldest_snapshots(&self) {
        let mut snap_ids: Vec<u64> = self
            .db
            .tree_names()
            .iter()
            .filter_map(|name| {
                let s = std::str::from_utf8(name).ok()?;
                let id_str = s.strip_prefix("snap:")?;
                id_str.parse::<u64>().ok()
            })
            .collect();
        snap_ids.sort_unstable();
        while snap_ids.len() > Self::MAX_SNAPSHOTS {
            if let Some(oldest) = snap_ids.first().copied() {
                self.delete_snapshot(oldest);
                snap_ids.remove(0);
            }
        }
    }

    /// Open (or create) a sled database at the given path.
    ///
    /// # Errors
    /// Returns `sled::Error` if the database cannot be opened (e.g. permissions, corruption).
    /// Maximum number of snapshots retained on disk.
    const MAX_SNAPSHOTS: usize = 32;

    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, sled::Error> {
        let db = sled::open(path)?;
        // Determine next snapshot ID from existing snapshot trees
        let max_existing = db
            .tree_names()
            .iter()
            .filter_map(|name| {
                let s = std::str::from_utf8(name).ok()?;
                let id_str = s.strip_prefix("snap:")?;
                id_str.parse::<u64>().ok()
            })
            .max()
            .unwrap_or(0);
        Ok(Self {
            db,
            next_snapshot_id: AtomicU64::new(max_existing + 1),
        })
    }

    /// Return the on-disk size in bytes (approximate).
    #[must_use]
    pub fn disk_size(&self) -> u64 {
        self.db.size_on_disk().unwrap_or(0)
    }
}

impl StorageBackend for SledStorage {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        match self.db.get(key) {
            Ok(Some(ivec)) => Some(ivec.to_vec()),
            Ok(None) => None,
            Err(e) => {
                tracing::error!(error = %e, "sled get failed");
                None
            }
        }
    }

    fn put(&self, key: &[u8], value: &[u8]) {
        if let Err(e) = self.db.insert(key, value) {
            tracing::error!(error = %e, "sled put failed");
        }
    }

    fn delete(&self, key: &[u8]) -> bool {
        match self.db.remove(key) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(e) => {
                tracing::error!(error = %e, "sled delete failed");
                false
            }
        }
    }

    fn list_keys(&self, prefix: Option<&[u8]>) -> Vec<Vec<u8>> {
        let iter = match prefix {
            Some(p) => self.db.scan_prefix(p),
            None => self.db.iter(),
        };
        iter.filter_map(|r| match r {
            Ok((k, _)) => Some(k.to_vec()),
            Err(e) => {
                tracing::error!(error = %e, "sled scan failed");
                None
            }
        })
        .collect()
    }

    fn list(&self, prefix: Option<&[u8]>) -> Vec<(Vec<u8>, Vec<u8>)> {
        let iter = match prefix {
            Some(p) => self.db.scan_prefix(p),
            None => self.db.iter(),
        };
        iter.filter_map(|r| match r {
            Ok((k, v)) => Some((k.to_vec(), v.to_vec())),
            Err(e) => {
                tracing::error!(error = %e, "sled scan failed");
                None
            }
        })
        .collect()
    }

    fn count(&self) -> usize {
        self.db.len()
    }

    fn flush(&self) {
        if let Err(e) = self.db.flush() {
            tracing::error!(error = %e, "sled flush failed");
        }
    }

    fn create_snapshot(&self, label: Option<&str>) -> Result<SnapshotId, String> {
        let id = self.next_snapshot_id.fetch_add(1, Ordering::Relaxed);
        let tree_name = format!("snap:{id}");
        let snap_tree = self
            .db
            .open_tree(tree_name.as_bytes())
            .map_err(|e| format!("Failed to create snapshot tree: {e}"))?;

        // Copy all current data into the snapshot tree
        let mut entry_count = 0usize;
        for item in self.db.iter() {
            match item {
                Ok((k, v)) => {
                    snap_tree
                        .insert(&k, &*v)
                        .map_err(|e| format!("Snapshot write failed: {e}"))?;
                    entry_count += 1;
                }
                Err(e) => return Err(format!("Snapshot read failed: {e}")),
            }
        }

        // Store metadata
        let info = SnapshotInfo {
            id,
            created_at: chrono::Utc::now().to_rfc3339(),
            entry_count,
            label: label.map(ToOwned::to_owned),
        };
        let meta_key = format!("snap_meta:{id}");
        let meta_json =
            serde_json::to_vec(&info).map_err(|e| format!("Metadata serialize failed: {e}"))?;
        self.db
            .insert(meta_key.as_bytes(), meta_json)
            .map_err(|e| format!("Metadata write failed: {e}"))?;

        // Evict oldest snapshots if over capacity
        self.evict_oldest_snapshots();

        let _ = self.db.flush();
        Ok(id)
    }

    fn list_snapshots(&self) -> Vec<SnapshotInfo> {
        let mut infos: Vec<SnapshotInfo> = self
            .db
            .scan_prefix(b"snap_meta:")
            .filter_map(|r| {
                let (_, v) = r.ok()?;
                serde_json::from_slice(&v).ok()
            })
            .collect();
        // Newest first
        infos.sort_by(|a, b| b.id.cmp(&a.id));
        infos
    }

    fn restore_snapshot(&self, id: SnapshotId) -> Result<usize, String> {
        let tree_name = format!("snap:{id}");
        // Check metadata exists
        let meta_key = format!("snap_meta:{id}");
        if self.db.get(meta_key.as_bytes()).ok().flatten().is_none() {
            return Err(format!("Snapshot {id} not found"));
        }

        let snap_tree = self
            .db
            .open_tree(tree_name.as_bytes())
            .map_err(|e| format!("Failed to open snapshot tree: {e}"))?;

        // Clear current default tree
        self.db
            .clear()
            .map_err(|e| format!("Failed to clear current state: {e}"))?;

        // Copy snapshot data back into default tree
        let mut count = 0usize;
        for item in snap_tree.iter() {
            match item {
                Ok((k, v)) => {
                    self.db
                        .insert(&k, &*v)
                        .map_err(|e| format!("Restore write failed: {e}"))?;
                    count += 1;
                }
                Err(e) => return Err(format!("Restore read failed: {e}")),
            }
        }

        // Re-insert all snapshot metadata (they were cleared)
        for name in self.db.tree_names() {
            if let Ok(s) = std::str::from_utf8(&name) {
                if let Some(snap_id_str) = s.strip_prefix("snap:") {
                    if let Ok(snap_id) = snap_id_str.parse::<u64>() {
                        let st = self.db.open_tree(&name).ok();
                        if let Some(st) = st {
                            let ec = st.len();
                            // We don't have the original metadata anymore, rebuild it
                            let meta = SnapshotInfo {
                                id: snap_id,
                                created_at: String::new(),
                                entry_count: ec,
                                label: None,
                            };
                            let mk = format!("snap_meta:{snap_id}");
                            if let Ok(json) = serde_json::to_vec(&meta) {
                                let _ = self.db.insert(mk.as_bytes(), json);
                            }
                        }
                    }
                }
            }
        }

        let _ = self.db.flush();
        Ok(count)
    }

    fn delete_snapshot(&self, id: SnapshotId) -> bool {
        let tree_name = format!("snap:{id}");
        let meta_key = format!("snap_meta:{id}");
        let _ = self.db.remove(meta_key.as_bytes());
        self.db.drop_tree(tree_name.as_bytes()).unwrap_or(false)
    }

    fn snapshot_count(&self) -> usize {
        self.db
            .tree_names()
            .iter()
            .filter(|name| {
                std::str::from_utf8(name)
                    .ok()
                    .is_some_and(|s| s.starts_with("snap:"))
            })
            .count()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open_temp() -> (SledStorage, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let storage = SledStorage::open(dir.path().join("test.sled")).expect("open");
        (storage, dir)
    }

    #[test]
    fn put_get_roundtrip() {
        let (s, _dir) = open_temp();
        s.put(b"key1", b"value1");
        assert_eq!(s.get(b"key1"), Some(b"value1".to_vec()));
    }

    #[test]
    fn get_missing_returns_none() {
        let (s, _dir) = open_temp();
        assert_eq!(s.get(b"nonexistent"), None);
    }

    #[test]
    fn put_overwrite() {
        let (s, _dir) = open_temp();
        s.put(b"key1", b"v1");
        s.put(b"key1", b"v2");
        assert_eq!(s.get(b"key1"), Some(b"v2".to_vec()));
    }

    #[test]
    fn delete_existing_returns_true() {
        let (s, _dir) = open_temp();
        s.put(b"key1", b"v1");
        assert!(s.delete(b"key1"));
        assert_eq!(s.get(b"key1"), None);
    }

    #[test]
    fn delete_missing_returns_false() {
        let (s, _dir) = open_temp();
        assert!(!s.delete(b"nonexistent"));
    }

    #[test]
    fn count_tracks_entries() {
        let (s, _dir) = open_temp();
        assert_eq!(s.count(), 0);
        s.put(b"a", b"1");
        s.put(b"b", b"2");
        assert_eq!(s.count(), 2);
        s.delete(b"a");
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn list_keys_all() {
        let (s, _dir) = open_temp();
        s.put(b"alpha", b"1");
        s.put(b"beta", b"2");
        let keys = s.list_keys(None);
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn list_keys_with_prefix() {
        let (s, _dir) = open_temp();
        s.put(b"fault:001", b"f1");
        s.put(b"fault:002", b"f2");
        s.put(b"audit:001", b"a1");
        let keys = s.list_keys(Some(b"fault:"));
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k.starts_with(b"fault:")));
    }

    #[test]
    fn list_all_pairs() {
        let (s, _dir) = open_temp();
        s.put(b"x", b"1");
        s.put(b"y", b"2");
        let pairs = s.list(None);
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn list_pairs_with_prefix() {
        let (s, _dir) = open_temp();
        s.put(b"hist:a", b"1");
        s.put(b"hist:b", b"2");
        s.put(b"meta:c", b"3");
        let pairs = s.list(Some(b"hist:"));
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn flush_does_not_panic() {
        let (s, _dir) = open_temp();
        s.put(b"k", b"v");
        s.flush();
    }

    #[test]
    fn disk_size_nonzero_after_write() {
        let (s, _dir) = open_temp();
        s.put(b"data", b"payload");
        s.flush();
        assert!(s.disk_size() > 0);
    }

    #[test]
    fn reopen_persists_data() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("persist.sled");

        // Write and close
        {
            let s = SledStorage::open(&path).expect("open");
            s.put(b"persistent", b"value");
            s.flush();
        }

        // Reopen and verify
        {
            let s = SledStorage::open(&path).expect("reopen");
            assert_eq!(s.get(b"persistent"), Some(b"value".to_vec()));
        }
    }

    // ── Snapshot / Rollback tests (F9) ──────────────────────────────────

    #[test]
    fn sled_create_snapshot() {
        let (s, _dir) = open_temp();
        s.put(b"a", b"1");
        s.put(b"b", b"2");
        let id = s.create_snapshot(Some("test")).unwrap();
        assert!(id > 0);
        assert_eq!(s.snapshot_count(), 1);
    }

    #[test]
    fn sled_list_snapshots_newest_first() {
        let (s, _dir) = open_temp();
        s.put(b"x", b"1");
        let id1 = s.create_snapshot(Some("first")).unwrap();
        s.put(b"y", b"2");
        let id2 = s.create_snapshot(Some("second")).unwrap();

        let snaps = s.list_snapshots();
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].id, id2);
        assert_eq!(snaps[1].id, id1);
    }

    #[test]
    fn sled_restore_snapshot() {
        let (s, _dir) = open_temp();
        s.put(b"original", b"val");
        let snap_id = s.create_snapshot(None).unwrap();

        // Mutate after snapshot
        s.put(b"new_key", b"new_val");
        s.delete(b"original");

        // Restore
        let count = s.restore_snapshot(snap_id).unwrap();
        assert_eq!(count, 1);
        assert_eq!(s.get(b"original"), Some(b"val".to_vec()));
        assert_eq!(s.get(b"new_key"), None);
    }

    #[test]
    fn sled_restore_nonexistent_fails() {
        let (s, _dir) = open_temp();
        assert!(s.restore_snapshot(999).is_err());
    }

    #[test]
    fn sled_delete_snapshot() {
        let (s, _dir) = open_temp();
        let id = s.create_snapshot(None).unwrap();
        assert_eq!(s.snapshot_count(), 1);
        assert!(s.delete_snapshot(id));
        assert_eq!(s.snapshot_count(), 0);
    }

    #[test]
    fn sled_snapshot_persists_across_reopen() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("snap.sled");
        let snap_id;

        // Create data + snapshot
        {
            let s = SledStorage::open(&path).expect("open");
            s.put(b"key", b"value");
            snap_id = s.create_snapshot(Some("persist-test")).unwrap();
            s.flush();
        }

        // Reopen, mutate, then restore
        {
            let s = SledStorage::open(&path).expect("reopen");
            s.put(b"key", b"changed");
            s.put(b"extra", b"data");
            let count = s.restore_snapshot(snap_id).unwrap();
            assert_eq!(count, 1);
            assert_eq!(s.get(b"key"), Some(b"value".to_vec()));
            assert_eq!(s.get(b"extra"), None);
        }
    }
}
