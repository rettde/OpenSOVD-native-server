// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Lock Manager — exclusive component access (SOVD Standard §7.4)
// Provides resource locking so only one client can access a component at a time.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use dashmap::DashMap;
use tracing::{debug, info, warn};

use native_interfaces::sovd::SovdLock;

/// Manages exclusive locks on SOVD components (SOVD §7.4)
pub struct LockManager {
    locks: DashMap<String, SovdLock>,
}

impl LockManager {
    pub fn new() -> Self {
        Self {
            locks: DashMap::new(),
        }
    }

    /// Start a background task that periodically reaps expired locks (SOVD §7.4).
    /// Returns a `JoinHandle` that runs until the runtime shuts down.
    pub fn start_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let reaped = mgr.reap_expired();
                if reaped > 0 {
                    info!(count = reaped, "Reaped expired locks");
                }
            }
        })
    }

    /// Remove all locks whose `expires` timestamp is in the past. Returns the count removed.
    pub fn reap_expired(&self) -> usize {
        let now = chrono::Utc::now();
        let mut reaped = 0usize;
        self.locks.retain(|comp_id, lock| {
            if let Some(ref exp) = lock.expires {
                if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(exp) {
                    if expiry < now {
                        debug!(component = %comp_id, expires = %exp, "Lock expired, reaping");
                        reaped += 1;
                        return false; // remove
                    }
                }
            }
            true // keep
        });
        reaped
    }

    /// Acquire a lock on a component. Returns the lock if successful.
    pub fn acquire(
        &self,
        component_id: &str,
        locked_by: &str,
        expires: Option<String>,
    ) -> Result<SovdLock, String> {
        if let Some(existing) = self.locks.get(component_id) {
            return Err(format!(
                "Component '{}' already locked by '{}'",
                component_id, existing.locked_by
            ));
        }

        let lock = SovdLock {
            component_id: component_id.to_owned(),
            locked_by: locked_by.to_owned(),
            locked_at: chrono::Utc::now().to_rfc3339(),
            expires,
        };
        self.locks.insert(component_id.to_owned(), lock.clone());
        info!(component = %component_id, by = %locked_by, "Component locked");
        Ok(lock)
    }

    /// Release a lock on a component
    pub fn release(&self, component_id: &str) -> bool {
        let removed = self.locks.remove(component_id).is_some();
        if removed {
            info!(component = %component_id, "Component unlocked");
        } else {
            warn!(component = %component_id, "Unlock requested but no lock exists");
        }
        removed
    }

    /// Get the current lock on a component (if any)
    pub fn get_lock(&self, component_id: &str) -> Option<SovdLock> {
        self.locks.get(component_id).map(|e| e.value().clone())
    }

    /// Check if a component is locked
    pub fn is_locked(&self, component_id: &str) -> bool {
        self.locks.contains_key(component_id)
    }

    /// Check if a component is locked by a specific client
    pub fn is_locked_by(&self, component_id: &str, client: &str) -> bool {
        self.locks
            .get(component_id)
            .map(|e| e.locked_by == client)
            .unwrap_or(false)
    }
}

impl Default for LockManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn acquire_and_release() {
        let lm = LockManager::new();
        let lock = lm.acquire("hpc", "client-1", None).unwrap();
        assert_eq!(lock.component_id, "hpc");
        assert_eq!(lock.locked_by, "client-1");
        assert!(lm.is_locked("hpc"));
        assert!(lm.release("hpc"));
        assert!(!lm.is_locked("hpc"));
    }

    #[test]
    fn double_lock_fails() {
        let lm = LockManager::new();
        lm.acquire("hpc", "client-1", None).unwrap();
        let err = lm.acquire("hpc", "client-2", None).unwrap_err();
        assert!(err.contains("already locked"));
    }

    #[test]
    fn release_nonexistent_returns_false() {
        let lm = LockManager::new();
        assert!(!lm.release("nope"));
    }

    #[test]
    fn get_lock_returns_none_when_unlocked() {
        let lm = LockManager::new();
        assert!(lm.get_lock("hpc").is_none());
    }

    #[test]
    fn is_locked_by_works() {
        let lm = LockManager::new();
        lm.acquire("hpc", "client-1", None).unwrap();
        assert!(lm.is_locked_by("hpc", "client-1"));
        assert!(!lm.is_locked_by("hpc", "client-2"));
    }

    #[test]
    fn reap_expired_removes_past_locks() {
        let lm = LockManager::new();
        // Lock with an expiry in the past
        let past = (chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc3339();
        lm.acquire("hpc", "client-1", Some(past)).unwrap();
        assert!(lm.is_locked("hpc"));
        let reaped = lm.reap_expired();
        assert_eq!(reaped, 1);
        assert!(!lm.is_locked("hpc"));
    }

    #[test]
    fn reap_expired_keeps_future_locks() {
        let lm = LockManager::new();
        let future = (chrono::Utc::now() + chrono::Duration::seconds(600)).to_rfc3339();
        lm.acquire("hpc", "client-1", Some(future)).unwrap();
        let reaped = lm.reap_expired();
        assert_eq!(reaped, 0);
        assert!(lm.is_locked("hpc"));
    }

    #[test]
    fn reap_expired_keeps_no_expiry_locks() {
        let lm = LockManager::new();
        lm.acquire("hpc", "client-1", None).unwrap();
        let reaped = lm.reap_expired();
        assert_eq!(reaped, 0);
        assert!(lm.is_locked("hpc"));
    }
}
