// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// StorageBackend (A2.1) — pluggable key-value persistence abstraction
//
// Consumers (AuditLog, FaultManager, KPI store, etc.) depend on this trait
// instead of a concrete storage engine. Implementations:
//   - InMemoryStorage  (default, for tests and lightweight deployments)
//   - SledStorage      (crash-safe embedded KV, behind `persist` feature)
//   - Future: Redis, PostgreSQL, S3, etc.
//
// The trait is intentionally simple (get/put/delete/list) so it can be
// backed by anything from a HashMap to a cloud database.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::BTreeMap;
use std::sync::Mutex;

/// A pluggable key-value storage backend.
///
/// Keys and values are both byte slices. Higher-level serialization
/// (JSON, bincode, etc.) is the caller's responsibility.
///
/// All operations are synchronous and infallible in the trait contract.
/// Implementations that can fail (network, I/O) should log errors internally
/// and return empty/false as appropriate (fail-open for reads, best-effort for writes).
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
}

// ─────────────────────────────────────────────────────────────────────────────
// InMemoryStorage — default implementation (BTreeMap, sorted keys)
// ─────────────────────────────────────────────────────────────────────────────

/// In-memory storage backend backed by a `BTreeMap` (sorted key order).
///
/// Suitable for tests, development, and lightweight single-instance deployments
/// where persistence across restarts is not required.
pub struct InMemoryStorage {
    data: Mutex<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(BTreeMap::new()),
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
}
