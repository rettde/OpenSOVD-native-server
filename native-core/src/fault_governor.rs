// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Fault Governor (W2.3) — DFM-side debounce layer
//
// Suppresses rapid-fire duplicate fault reports within a configurable debounce
// window. A fault is considered a duplicate if the same (fault_id, component_id)
// pair was reported within the window. Cleared faults reset the debounce state
// so re-occurrence is always reported.
//
// Thread-safe (DashMap), zero-alloc hot path after initial insert.
//
// Design alignment with eclipse-opensovd/fault-lib:
//   The fault-lib design doc (§"High Level Requirements") states:
//     "Debouncing should be in the fault lib to reduce the traffic on the IPC.
//      Debouncing needs to be also possible in the DFM if there is a
//      multi-fault aggregation."
//   This FaultGovernor implements the DFM-side debouncing. It operates on
//   already-delivered SovdFaults, complementing any client-side debouncing
//   that the fault-lib Reporter may apply before IPC delivery.
//   See: https://github.com/eclipse-opensovd/fault-lib/blob/main/docs/design/design.md
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::debug;

use crate::fault_manager::FaultManager;
use native_interfaces::sovd::SovdFault;

/// Configuration for the fault governor.
#[derive(Debug, Clone)]
pub struct FaultGovernorConfig {
    /// Debounce window — duplicate faults within this duration are suppressed
    pub debounce_window: Duration,
}

impl Default for FaultGovernorConfig {
    fn default() -> Self {
        Self {
            debounce_window: Duration::from_secs(5),
        }
    }
}

/// Debounce key: (fault_id, component_id)
type DebounceKey = (String, String);

/// Per-fault debounce state.
struct DebounceEntry {
    last_reported: Instant,
}

/// Fault governor — debounces fault reports before forwarding to FaultManager.
pub struct FaultGovernor {
    config: FaultGovernorConfig,
    debounce_map: DashMap<DebounceKey, DebounceEntry>,
    /// Total faults received (before debounce)
    total_received: AtomicU64,
    /// Total faults suppressed by debounce
    total_suppressed: AtomicU64,
}

impl FaultGovernor {
    /// Create a new fault governor with the given configuration.
    pub fn new(config: FaultGovernorConfig) -> Self {
        Self {
            config,
            debounce_map: DashMap::new(),
            total_received: AtomicU64::new(0),
            total_suppressed: AtomicU64::new(0),
        }
    }

    /// Report a fault through the governor. Returns `true` if the fault was
    /// forwarded to the FaultManager, `false` if it was debounced (suppressed).
    pub fn report(&self, fault_manager: &FaultManager, fault: SovdFault) -> bool {
        self.total_received.fetch_add(1, Ordering::Relaxed);

        let key = (fault.id.clone(), fault.component_id.clone());
        let now = Instant::now();

        // Check debounce map
        if let Some(entry) = self.debounce_map.get(&key) {
            if now.duration_since(entry.last_reported) < self.config.debounce_window {
                self.total_suppressed.fetch_add(1, Ordering::Relaxed);
                debug!(
                    fault_id = %fault.id,
                    component = %fault.component_id,
                    "Fault debounced (duplicate within window)"
                );
                return false;
            }
        }

        // Update debounce timestamp and forward
        self.debounce_map
            .insert(key, DebounceEntry { last_reported: now });
        fault_manager.report_fault(fault);
        true
    }

    /// Clear debounce state for a fault (called when a fault is cleared,
    /// so re-occurrence is always reported immediately).
    pub fn clear_debounce(&self, fault_id: &str, component_id: &str) {
        let key = (fault_id.to_owned(), component_id.to_owned());
        self.debounce_map.remove(&key);
    }

    /// Clear all debounce state for a component.
    pub fn clear_debounce_for_component(&self, component_id: &str) {
        self.debounce_map.retain(|key, _| key.1 != component_id);
    }

    /// Total faults received (before debounce filtering).
    pub fn total_received(&self) -> u64 {
        self.total_received.load(Ordering::Relaxed)
    }

    /// Total faults suppressed by debounce.
    pub fn total_suppressed(&self) -> u64 {
        self.total_suppressed.load(Ordering::Relaxed)
    }

    /// Number of tracked debounce entries.
    pub fn tracked_faults(&self) -> usize {
        self.debounce_map.len()
    }

    /// Remove stale debounce entries older than 2× the debounce window.
    pub fn reap_stale(&self) {
        let cutoff = Instant::now()
            .checked_sub(self.config.debounce_window * 2)
            .unwrap_or_else(Instant::now);
        self.debounce_map
            .retain(|_, entry| entry.last_reported > cutoff);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use native_interfaces::sovd::{SovdFaultSeverity, SovdFaultStatus};

    fn make_fault(id: &str, component_id: &str) -> SovdFault {
        SovdFault {
            id: id.into(),
            component_id: component_id.into(),
            code: format!("P0{id}"),
            display_code: None,
            severity: SovdFaultSeverity::High,
            status: SovdFaultStatus::Active,
            name: format!("Fault {id}"),
            description: None,
            scope: None,
        }
    }

    fn test_governor(debounce_ms: u64) -> FaultGovernor {
        FaultGovernor::new(FaultGovernorConfig {
            debounce_window: Duration::from_millis(debounce_ms),
        })
    }

    #[test]
    fn first_report_is_forwarded() {
        let fm = FaultManager::new();
        let gov = test_governor(1000);

        assert!(gov.report(&fm, make_fault("f1", "hpc")));
        assert_eq!(fm.total_fault_count(), 1);
        assert_eq!(gov.total_received(), 1);
        assert_eq!(gov.total_suppressed(), 0);
    }

    #[test]
    fn duplicate_within_window_is_suppressed() {
        let fm = FaultManager::new();
        let gov = test_governor(5000); // 5 second window

        assert!(gov.report(&fm, make_fault("f1", "hpc")));
        // Immediate duplicate — should be suppressed
        assert!(!gov.report(&fm, make_fault("f1", "hpc")));
        assert_eq!(fm.total_fault_count(), 1);
        assert_eq!(gov.total_received(), 2);
        assert_eq!(gov.total_suppressed(), 1);
    }

    #[test]
    fn different_faults_are_independent() {
        let fm = FaultManager::new();
        let gov = test_governor(5000);

        assert!(gov.report(&fm, make_fault("f1", "hpc")));
        assert!(gov.report(&fm, make_fault("f2", "hpc")));
        // Same fault_id but different component → distinct debounce key
        assert!(gov.report(&fm, make_fault("f1-brake", "brake")));
        assert_eq!(fm.total_fault_count(), 3);
        assert_eq!(gov.total_suppressed(), 0);
        assert_eq!(gov.tracked_faults(), 3);
    }

    #[test]
    fn after_window_expires_report_is_forwarded() {
        let fm = FaultManager::new();
        let gov = test_governor(1); // 1ms window

        assert!(gov.report(&fm, make_fault("f1", "hpc")));
        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(5));
        assert!(gov.report(&fm, make_fault("f1", "hpc")));
        assert_eq!(gov.total_suppressed(), 0);
    }

    #[test]
    fn clear_debounce_allows_immediate_re_report() {
        let fm = FaultManager::new();
        let gov = test_governor(5000);

        assert!(gov.report(&fm, make_fault("f1", "hpc")));
        assert!(!gov.report(&fm, make_fault("f1", "hpc"))); // suppressed

        gov.clear_debounce("f1", "hpc");
        assert!(gov.report(&fm, make_fault("f1", "hpc"))); // forwarded
        assert_eq!(gov.total_suppressed(), 1);
    }

    #[test]
    fn clear_debounce_for_component() {
        let fm = FaultManager::new();
        let gov = test_governor(5000);

        gov.report(&fm, make_fault("f1", "hpc"));
        gov.report(&fm, make_fault("f2", "hpc"));
        gov.report(&fm, make_fault("f3", "brake"));
        assert_eq!(gov.tracked_faults(), 3);

        gov.clear_debounce_for_component("hpc");
        assert_eq!(gov.tracked_faults(), 1); // only brake remains
    }

    #[test]
    fn reap_stale_removes_old_entries() {
        let gov = test_governor(1); // 1ms window
        let fm = FaultManager::new();

        gov.report(&fm, make_fault("f1", "hpc"));
        assert_eq!(gov.tracked_faults(), 1);

        // Wait for 2× window to expire
        std::thread::sleep(Duration::from_millis(5));
        gov.reap_stale();
        assert_eq!(gov.tracked_faults(), 0);
    }

    #[test]
    fn default_config_uses_5s_window() {
        let config = FaultGovernorConfig::default();
        assert_eq!(config.debounce_window, Duration::from_secs(5));
    }
}
