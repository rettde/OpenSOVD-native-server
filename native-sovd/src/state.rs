// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Shared application state — injected into axum handlers via State extractor
//
// Uses ComponentBackend (trait object) for the OpenSOVD Gateway pattern.
// The server dispatches SOVD REST requests to one or more backends:
//   - SovdHttpBackend  → external CDA via SOVD REST API (standard-conformant)
//
// Sub-grouped into logical domains to keep the struct manageable as features
// are added (Wave 1–3). Each sub-state is Arc-wrapped and Clone-friendly.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use dashmap::DashMap;
use native_core::{AuditLog, DiagLog, FaultManager, HistoryService, LockManager};
use native_health::HealthMonitor;
use native_interfaces::data_catalog::DataCatalogProvider;
use native_interfaces::oem::OemProfile;
use native_interfaces::sovd::SovdSoftwarePackage;
use native_interfaces::sovd::{SovdOperationExecution, SovdProximityChallenge};
use native_interfaces::{ComponentBackend, EntityBackend, ExtendedDiagBackend};

// ── Sub-state: Diagnostics ──────────────────────────────────────────────────

/// Diagnostic-related state: fault management, locking, diagnostic logs.
#[derive(Clone)]
pub struct DiagState {
    /// Diagnostic Fault Manager (DFM) — aggregates faults from all sources
    pub fault_manager: Arc<FaultManager>,
    /// Component lock manager (SOVD §7.4)
    pub lock_manager: Arc<LockManager>,
    /// Diagnostic log ring buffer (SOVD §7.10)
    pub diag_log: Arc<DiagLog>,
    /// Historical diagnostic storage — time-range queries (W2.2)
    pub history: Arc<HistoryService>,
}

// ── Sub-state: Security ─────────────────────────────────────────────────────

/// Security-related state: OEM policy profile, audit trail, rate limiting.
#[derive(Clone)]
pub struct SecurityState {
    /// OEM profile — vendor-specific rules (auth, entity IDs, CDF, discovery)
    pub oem_profile: Arc<dyn OemProfile>,
    /// Audit trail — tamper-resistant log of security-relevant actions (Wave 1)
    pub audit_log: Arc<AuditLog>,
    /// Per-client rate limiter (A2.5) — None if disabled
    pub rate_limiter: Option<crate::rate_limit::RateLimiter>,
}

// ── Sub-state: Runtime ──────────────────────────────────────────────────────

/// Runtime operational state: health, execution tracking, proximity challenges.
#[derive(Clone)]
pub struct RuntimeState {
    /// System health monitor (CPU, memory, uptime)
    pub health: Arc<HealthMonitor>,
    /// Execution history: executionId → SovdOperationExecution
    pub execution_store: Arc<DashMap<String, SovdOperationExecution>>,
    /// Proximity challenge store: challengeId → SovdProximityChallenge
    pub proximity_store: Arc<DashMap<String, SovdProximityChallenge>>,
    /// Software package lifecycle store: "{component_id}/{package_id}" → SovdSoftwarePackage
    pub package_store: Arc<DashMap<String, SovdSoftwarePackage>>,
    /// Runtime feature flags (E2.4) — lock-free atomic toggles
    pub feature_flags: native_interfaces::SharedFeatureFlags,
}

// ── Top-level AppState ──────────────────────────────────────────────────────

/// Shared application state accessible by all axum route handlers.
///
/// Organized into logical sub-groups to keep the struct manageable as
/// Waves 1–3 add new capabilities (entity backends, software update
/// managers, KPI providers, tenant context, etc.).
#[derive(Clone)]
pub struct AppState {
    /// Gateway backend — dispatches to CDA (HTTP) or local UDS/DoIP
    pub backend: Arc<dyn ComponentBackend>,
    /// Extended diagnostics backend — UDS vendor extensions (x-uds routes)
    pub extended_backend: Arc<dyn ExtendedDiagBackend>,
    /// Entity backend — apps and funcs (ISO 17978-3 §4.2.3)
    pub entity_backend: Arc<dyn EntityBackend>,
    /// Diagnostic state: faults, locks, diagnostic logs
    pub diag: DiagState,
    /// Security state: OEM profile, audit trail
    pub security: SecurityState,
    /// Runtime state: health, execution tracking, proximity challenges
    pub runtime: RuntimeState,
    /// Semantic data catalog provider (Wave 4, A4.2)
    pub data_catalog: Arc<dyn DataCatalogProvider>,
}
