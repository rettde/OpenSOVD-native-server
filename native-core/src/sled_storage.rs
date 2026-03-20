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

use native_interfaces::StorageBackend;
use std::path::Path;

/// Persistent key-value storage backed by sled (embedded B-tree).
///
/// All writes are immediately durable (sled flushes on commit).
/// Thread-safe: sled::Db is internally `Send + Sync`.
pub struct SledStorage {
    db: sled::Db,
}

impl SledStorage {
    /// Open (or create) a sled database at the given path.
    ///
    /// # Errors
    /// Returns `sled::Error` if the database cannot be opened (e.g. permissions, corruption).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, sled::Error> {
        let db = sled::open(path)?;
        Ok(Self { db })
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
}
