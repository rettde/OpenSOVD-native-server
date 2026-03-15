// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// native-core — Diagnostic core logic
//
// Architecture (OpenSOVD standard-conformant):
//   ComponentRouter (Gateway) → dispatches to backends:
//     ├── SovdHttpBackend     → external CDA via SOVD REST API (standard)
//     └── LocalUdsBackend     → embedded UDS/DoIP (standalone, feature-gated)
//
// FaultBridge connects fault-lib reporters to the Diagnostic Fault Manager.
// ─────────────────────────────────────────────────────────────────────────────
#![forbid(unsafe_code)]
#![deny(warnings)]
#![allow(
    clippy::redundant_closure_for_method_calls,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::single_match_else,
    clippy::wildcard_imports,
    clippy::if_not_else,
    clippy::items_after_statements,
    clippy::unnecessary_literal_bound,
    clippy::manual_let_else,
    clippy::map_unwrap_or
)]

// ── Always available ────────────────────────────────────────────────────────
pub mod diag_log;
pub mod fault_bridge;
pub mod fault_manager;
pub mod http_backend;
pub mod lock_manager;
pub mod router;

// ── Local UDS/DoIP backend (feature-gated) ──────────────────────────────────
#[cfg(feature = "local-uds")]
pub mod local_backend;
#[cfg(feature = "local-uds")]
pub mod ota;
#[cfg(feature = "local-uds")]
pub mod translation;

// ── Re-exports ──────────────────────────────────────────────────────────────
pub use diag_log::DiagLog;
pub use fault_bridge::{FaultBridge, FaultLifecycleStage, FaultRecord, FaultSeverity, FaultSink};
pub use fault_manager::FaultManager;
pub use http_backend::{SovdHttpBackend, SovdHttpBackendConfig};
pub use lock_manager::LockManager;
pub use router::ComponentRouter;

#[cfg(feature = "local-uds")]
pub use local_backend::LocalUdsBackend;
#[cfg(feature = "local-uds")]
pub use translation::SovdTranslator;
