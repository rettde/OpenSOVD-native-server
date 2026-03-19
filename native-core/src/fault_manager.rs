// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ───────────────────────────────────────────────────────────────────────────────
// Fault Manager — central fault aggregation
// Follows the OpenSOVD design: "Diagnostic Fault Manager aggregates and
// manages diagnostic fault data from Fault libs across the system."
//
// Two backends (compile-time feature gate):
//   default  → DashMap (in-memory, no persistence)
//   persist  → sled (embedded key-value store, crash-safe)
// ───────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "persist"))]
use dashmap::DashMap;
use tracing::{debug, info};

use native_interfaces::sovd::*;

// ───────────────────────────────────────────────────────────────────────────────
// DashMap backend (default — in-memory)
// ───────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "persist"))]
pub struct FaultManager {
    faults: DashMap<String, SovdFault>,
}

#[cfg(not(feature = "persist"))]
impl FaultManager {
    pub fn new() -> Self {
        Self {
            faults: DashMap::new(),
        }
    }

    pub fn report_fault(&self, fault: SovdFault) {
        info!(id = %fault.id, code = %fault.code, severity = ?fault.severity, "Fault reported");
        self.faults.insert(fault.id.clone(), fault);
    }

    pub fn clear_fault(&self, fault_id: &str) -> bool {
        let removed = self.faults.remove(fault_id).is_some();
        if removed {
            info!(id = %fault_id, "Fault cleared");
        }
        removed
    }

    pub fn clear_faults_for_component(&self, component_id: &str) -> usize {
        let to_remove: Vec<String> = self
            .faults
            .iter()
            .filter(|entry| entry.value().component_id == component_id)
            .map(|entry| entry.key().clone())
            .collect();

        let count = to_remove.len();
        for id in &to_remove {
            self.faults.remove(id);
        }
        info!(component = %component_id, count, "Faults cleared for component");
        count
    }

    pub fn get_all_faults(&self) -> Vec<SovdFault> {
        self.faults.iter().map(|e| e.value().clone()).collect()
    }

    pub fn get_faults_for_component(&self, component_id: &str) -> Vec<SovdFault> {
        self.faults
            .iter()
            .filter(|e| e.value().component_id == component_id)
            .map(|e| e.value().clone())
            .collect()
    }

    pub fn get_fault(&self, fault_id: &str) -> Option<SovdFault> {
        self.faults.get(fault_id).map(|e| e.value().clone())
    }

    pub fn update_from_uds_scan(&self, component_id: &str, faults: Vec<SovdFault>) {
        self.clear_faults_for_component(component_id);

        let count = faults.len();
        for fault in faults {
            self.faults.insert(fault.id.clone(), fault);
        }
        debug!(component = %component_id, count, "Faults updated from UDS scan");
    }

    pub fn total_fault_count(&self) -> usize {
        self.faults.len()
    }
}

#[cfg(not(feature = "persist"))]
impl Default for FaultManager {
    fn default() -> Self {
        Self::new()
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// sled backend (feature = "persist")
// ───────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "persist")]
pub struct FaultManager {
    #[allow(dead_code)] // kept alive to prevent sled from closing
    db: sled::Db,
    tree: sled::Tree,
}

#[cfg(feature = "persist")]
impl FaultManager {
    /// Create a new persistent FaultManager at the default path
    pub fn new() -> Self {
        Self::open("data/faults").expect("Failed to open sled database at data/faults")
    }

    /// Open with a custom database path
    pub fn open(path: &str) -> Result<Self, String> {
        let db = sled::open(path).map_err(|e| format!("sled open '{path}': {e}"))?;
        let tree = db
            .open_tree("faults")
            .map_err(|e| format!("sled tree: {e}"))?;
        info!(path = %path, "FaultManager sled database opened");
        Ok(Self { db, tree })
    }

    pub fn report_fault(&self, fault: SovdFault) {
        info!(id = %fault.id, code = %fault.code, severity = ?fault.severity, "Fault reported");
        let value = serde_json::to_vec(&fault).expect("SovdFault serialization");
        self.tree
            .insert(fault.id.as_bytes(), value)
            .expect("sled insert");
        let _ = self.tree.flush();
    }

    pub fn clear_fault(&self, fault_id: &str) -> bool {
        let removed = self
            .tree
            .remove(fault_id.as_bytes())
            .ok()
            .flatten()
            .is_some();
        if removed {
            info!(id = %fault_id, "Fault cleared");
            let _ = self.tree.flush();
        }
        removed
    }

    pub fn clear_faults_for_component(&self, component_id: &str) -> usize {
        let to_remove: Vec<Vec<u8>> = self
            .tree
            .iter()
            .filter_map(|entry| {
                let (key, val) = entry.ok()?;
                let fault: SovdFault = serde_json::from_slice(&val).ok()?;
                if fault.component_id == component_id {
                    Some(key.to_vec())
                } else {
                    None
                }
            })
            .collect();

        let count = to_remove.len();
        for key in &to_remove {
            let _ = self.tree.remove(key);
        }
        if count > 0 {
            let _ = self.tree.flush();
        }
        info!(component = %component_id, count, "Faults cleared for component");
        count
    }

    pub fn get_all_faults(&self) -> Vec<SovdFault> {
        self.tree
            .iter()
            .filter_map(|entry| {
                let (_key, val) = entry.ok()?;
                serde_json::from_slice(&val).ok()
            })
            .collect()
    }

    pub fn get_faults_for_component(&self, component_id: &str) -> Vec<SovdFault> {
        self.get_all_faults()
            .into_iter()
            .filter(|f| f.component_id == component_id)
            .collect()
    }

    pub fn get_fault(&self, fault_id: &str) -> Option<SovdFault> {
        let val = self.tree.get(fault_id.as_bytes()).ok()??;
        serde_json::from_slice(&val).ok()
    }

    pub fn update_from_uds_scan(&self, component_id: &str, faults: Vec<SovdFault>) {
        self.clear_faults_for_component(component_id);

        let count = faults.len();
        for fault in faults {
            let value = serde_json::to_vec(&fault).expect("SovdFault serialization");
            self.tree
                .insert(fault.id.as_bytes(), value)
                .expect("sled insert");
        }
        let _ = self.tree.flush();
        debug!(component = %component_id, count, "Faults updated from UDS scan");
    }

    pub fn total_fault_count(&self) -> usize {
        self.tree.len()
    }
}

#[cfg(feature = "persist")]
impl Default for FaultManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

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
        }
    }

    /// Create a FaultManager for testing
    /// - DashMap backend: just FaultManager::new()
    /// - sled backend: uses a temp directory to avoid test interference
    fn test_manager() -> FaultManager {
        #[cfg(not(feature = "persist"))]
        {
            FaultManager::new()
        }
        #[cfg(feature = "persist")]
        {
            let dir = tempfile::tempdir().expect("tempdir");
            FaultManager::open(dir.path().to_str().unwrap()).expect("sled open")
        }
    }

    #[test]
    fn new_manager_is_empty() {
        let fm = test_manager();
        assert_eq!(fm.total_fault_count(), 0);
        assert!(fm.get_all_faults().is_empty());
    }

    #[test]
    fn report_and_get_fault() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));
        assert_eq!(fm.total_fault_count(), 1);
        let f = fm.get_fault("f1").unwrap();
        assert_eq!(f.code, "P0100");
        assert_eq!(f.component_id, "hpc");
    }

    #[test]
    fn get_nonexistent_fault_returns_none() {
        let fm = test_manager();
        assert!(fm.get_fault("missing").is_none());
    }

    #[test]
    fn clear_specific_fault() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));
        fm.report_fault(make_fault("f2", "hpc", "P0200"));
        assert!(fm.clear_fault("f1"));
        assert_eq!(fm.total_fault_count(), 1);
        assert!(fm.get_fault("f1").is_none());
        assert!(fm.get_fault("f2").is_some());
    }

    #[test]
    fn clear_nonexistent_fault_returns_false() {
        let fm = test_manager();
        assert!(!fm.clear_fault("nope"));
    }

    #[test]
    fn clear_faults_for_component() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));
        fm.report_fault(make_fault("f2", "hpc", "P0200"));
        fm.report_fault(make_fault("f3", "brake", "P0300"));

        let cleared = fm.clear_faults_for_component("hpc");
        assert_eq!(cleared, 2);
        assert_eq!(fm.total_fault_count(), 1);
        assert!(fm.get_fault("f3").is_some());
    }

    #[test]
    fn get_faults_for_component_filters() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));
        fm.report_fault(make_fault("f2", "brake", "P0200"));
        fm.report_fault(make_fault("f3", "hpc", "P0300"));

        let hpc_faults = fm.get_faults_for_component("hpc");
        assert_eq!(hpc_faults.len(), 2);
        assert!(hpc_faults.iter().all(|f| f.component_id == "hpc"));

        let brake_faults = fm.get_faults_for_component("brake");
        assert_eq!(brake_faults.len(), 1);
    }

    #[test]
    fn update_from_uds_scan_replaces_faults() {
        let fm = test_manager();
        fm.report_fault(make_fault("old1", "hpc", "P0100"));
        fm.report_fault(make_fault("other", "brake", "P0999"));

        let new_faults = vec![
            make_fault("new1", "hpc", "P0200"),
            make_fault("new2", "hpc", "P0300"),
        ];
        fm.update_from_uds_scan("hpc", new_faults);

        assert!(fm.get_fault("old1").is_none());
        assert!(fm.get_fault("new1").is_some());
        assert!(fm.get_fault("new2").is_some());
        assert!(fm.get_fault("other").is_some());
        assert_eq!(fm.total_fault_count(), 3);
    }

    #[test]
    fn report_overwrites_existing_fault() {
        let fm = test_manager();
        fm.report_fault(make_fault("f1", "hpc", "P0100"));
        fm.report_fault(make_fault("f1", "hpc", "P0999"));
        assert_eq!(fm.total_fault_count(), 1);
        assert_eq!(fm.get_fault("f1").unwrap().code, "P0999");
    }

    #[test]
    fn default_creates_empty_manager() {
        let fm = FaultManager::default();
        assert_eq!(fm.total_fault_count(), 0);
    }
}
