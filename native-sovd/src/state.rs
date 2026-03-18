// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Shared application state — injected into axum handlers via State extractor
//
// Uses ComponentBackend (trait object) instead of SovdTranslator directly.
// This enables the OpenSOVD Gateway pattern where the server dispatches to:
//   - SovdHttpBackend  → external CDA via SOVD REST API (standard-conformant)
//   - LocalUdsBackend  → embedded UDS/DoIP (standalone mode)
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use dashmap::DashMap;
use native_core::{DiagLog, FaultManager, LockManager};
use native_health::HealthMonitor;
use native_interfaces::oem::OemProfile;
use native_interfaces::sovd::{SovdOperationExecution, SovdProximityChallenge};
use native_interfaces::ComponentBackend;

/// Shared application state accessible by all axum route handlers
#[derive(Clone)]
pub struct AppState {
    /// Gateway backend — dispatches to CDA (HTTP) or local UDS/DoIP
    pub backend: Arc<dyn ComponentBackend>,
    /// OEM profile — vendor-specific rules (auth, entity IDs, CDF, discovery)
    pub oem_profile: Arc<dyn OemProfile>,
    /// Diagnostic Fault Manager (DFM) — aggregates faults from all sources
    pub fault_manager: Arc<FaultManager>,
    pub lock_manager: Arc<LockManager>,
    pub diag_log: Arc<DiagLog>,
    pub health: Arc<HealthMonitor>,
    /// Execution history: executionId → SovdOperationExecution
    pub execution_store: Arc<DashMap<String, SovdOperationExecution>>,
    /// Proximity challenge store: challengeId → SovdProximityChallenge
    pub proximity_store: Arc<DashMap<String, SovdProximityChallenge>>,
}
