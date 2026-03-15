// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Fault Bridge — Connects the OpenSOVD fault-lib pattern to FaultManager
//
// The Eclipse OpenSOVD fault-lib (<https://github.com/eclipse-opensovd/fault-lib>)
// defines a `FaultSink` trait for non-blocking fault delivery to the Diagnostic
// Fault Manager (DFM). This module implements that pattern:
//
//   fault-lib Reporter → FaultSink → FaultBridge → FaultManager (DFM)
//
// The FaultBridge receives fault records and translates them into SOVD faults
// stored in the FaultManager. This follows the OpenSOVD design where the DFM
// aggregates faults from multiple sources (Fault Libraries across the system).
//
// Note: The actual fault-lib crate requires nightly Rust (edition 2024).
// This bridge defines a compatible interface that can be connected when
// the fault-lib stabilizes or when using nightly toolchains.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use tracing::{debug, info};

use crate::fault_manager::FaultManager;
use native_interfaces::sovd::{SovdFault, SovdFaultSeverity, SovdFaultStatus};

// ─────────────────────────────────────────────────────────────────────────────
// fault-lib compatible types (mirrors fault-lib::model without nightly dep)
// ─────────────────────────────────────────────────────────────────────────────

/// Severity levels aligned with `fault-lib::FaultSeverity`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultSeverity {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

/// Lifecycle stage aligned with `fault-lib::FaultLifecycleStage`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultLifecycleStage {
    NotTested,
    PreFailed,
    Failed,
    PrePassed,
    Passed,
}

/// Fault record aligned with `fault-lib::FaultRecord`
#[derive(Debug, Clone)]
pub struct FaultRecord {
    pub fault_id: String,
    pub source: String,
    pub severity: FaultSeverity,
    pub stage: FaultLifecycleStage,
    pub component_id: String,
    pub description: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// FaultSink trait — mirrors fault-lib::FaultSink
// ─────────────────────────────────────────────────────────────────────────────

/// Non-blocking fault delivery to the Diagnostic Fault Manager.
///
/// This mirrors the `fault-lib` `FaultSink` trait. When the `fault-lib` crate
/// stabilizes on stable Rust, this can be replaced with a direct adapter.
pub trait FaultSink: Send + Sync + 'static {
    /// Enqueue a record for delivery to the Diagnostic Fault Manager.
    ///
    /// # Errors
    /// Returns `FaultSinkError` if the record cannot be delivered.
    fn publish(&self, record: &FaultRecord) -> Result<(), FaultSinkError>;
}

#[derive(Debug)]
pub enum FaultSinkError {
    TransportDown,
    RateLimited,
    Other(String),
}

impl std::fmt::Display for FaultSinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TransportDown => write!(f, "transport unavailable"),
            Self::RateLimited => write!(f, "rate limited"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FaultBridge — FaultSink implementation that feeds into FaultManager
// ─────────────────────────────────────────────────────────────────────────────

/// Bridges `fault-lib` `FaultRecord`s into the SOVD `FaultManager` (DFM role).
///
/// Usage:
///   let bridge = FaultBridge::new(fault_manager.clone());
///   bridge.publish(&record)?;  // Non-blocking enqueue
pub struct FaultBridge {
    fault_manager: Arc<FaultManager>,
}

impl FaultBridge {
    #[must_use]
    pub fn new(fault_manager: Arc<FaultManager>) -> Self {
        info!("FaultBridge initialized — connecting fault-lib reporters to DFM");
        Self { fault_manager }
    }

    /// Convert a `fault-lib` `FaultRecord` to a SOVD fault
    fn to_sovd_fault(record: &FaultRecord) -> SovdFault {
        SovdFault {
            id: record.fault_id.clone(),
            component_id: record.component_id.clone(),
            code: record.fault_id.clone(),
            display_code: None,
            severity: match record.severity {
                FaultSeverity::Fatal | FaultSeverity::Error => SovdFaultSeverity::High,
                FaultSeverity::Warn => SovdFaultSeverity::Medium,
                _ => SovdFaultSeverity::Low,
            },
            status: match record.stage {
                FaultLifecycleStage::Failed => SovdFaultStatus::Active,
                FaultLifecycleStage::PreFailed => SovdFaultStatus::Pending,
                FaultLifecycleStage::Passed
                | FaultLifecycleStage::PrePassed
                | FaultLifecycleStage::NotTested => SovdFaultStatus::Passive,
            },
            name: format!("Fault {}", record.fault_id),
            description: record.description.clone(),
        }
    }
}

impl FaultSink for FaultBridge {
    fn publish(&self, record: &FaultRecord) -> Result<(), FaultSinkError> {
        let sovd_fault = Self::to_sovd_fault(record);

        debug!(
            fault_id = %record.fault_id,
            component = %record.component_id,
            stage = ?record.stage,
            "FaultBridge: delivering fault to DFM"
        );

        // For Passed stages, clear the fault; otherwise report it
        if record.stage == FaultLifecycleStage::Passed {
            self.fault_manager.clear_fault(&record.fault_id);
        } else {
            self.fault_manager.report_fault(sovd_fault);
        }

        Ok(())
    }
}

/// Log hook aligned with `fault-lib::LogHook`
pub trait FaultLogHook: Send + Sync + 'static {
    fn on_report(&self, record: &FaultRecord);
}

/// Default log hook that logs via tracing
pub struct TracingLogHook;

impl FaultLogHook for TracingLogHook {
    fn on_report(&self, record: &FaultRecord) {
        match record.severity {
            FaultSeverity::Fatal | FaultSeverity::Error => {
                tracing::error!(
                    fault_id = %record.fault_id,
                    component = %record.component_id,
                    stage = ?record.stage,
                    "Fault reported"
                );
            }
            FaultSeverity::Warn => {
                tracing::warn!(
                    fault_id = %record.fault_id,
                    component = %record.component_id,
                    stage = ?record.stage,
                    "Fault reported"
                );
            }
            _ => {
                tracing::info!(
                    fault_id = %record.fault_id,
                    component = %record.component_id,
                    stage = ?record.stage,
                    "Fault reported"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_publishes_failed_fault() {
        let fm = Arc::new(FaultManager::new());
        let bridge = FaultBridge::new(fm.clone());

        let record = FaultRecord {
            fault_id: "FAULT_001".into(),
            source: "sensor.temperature".into(),
            severity: FaultSeverity::Error,
            stage: FaultLifecycleStage::Failed,
            component_id: "hpc".into(),
            description: Some("Overtemperature".into()),
        };

        bridge.publish(&record).unwrap();

        assert_eq!(fm.total_fault_count(), 1);
        let fault = fm.get_fault("FAULT_001").unwrap();
        assert_eq!(fault.severity, SovdFaultSeverity::High);
        assert_eq!(fault.status, SovdFaultStatus::Active);
    }

    #[test]
    fn bridge_clears_passed_fault() {
        let fm = Arc::new(FaultManager::new());
        let bridge = FaultBridge::new(fm.clone());

        // First report failure
        bridge
            .publish(&FaultRecord {
                fault_id: "F1".into(),
                source: "s".into(),
                severity: FaultSeverity::Error,
                stage: FaultLifecycleStage::Failed,
                component_id: "hpc".into(),
                description: None,
            })
            .unwrap();
        assert_eq!(fm.total_fault_count(), 1);

        // Then report passed — should clear
        bridge
            .publish(&FaultRecord {
                fault_id: "F1".into(),
                source: "s".into(),
                severity: FaultSeverity::Info,
                stage: FaultLifecycleStage::Passed,
                component_id: "hpc".into(),
                description: None,
            })
            .unwrap();
        assert_eq!(fm.total_fault_count(), 0);
    }

    #[test]
    fn severity_mapping() {
        let record = FaultRecord {
            fault_id: "F".into(),
            source: "s".into(),
            severity: FaultSeverity::Warn,
            stage: FaultLifecycleStage::PreFailed,
            component_id: "x".into(),
            description: None,
        };
        let sovd = FaultBridge::to_sovd_fault(&record);
        assert_eq!(sovd.severity, SovdFaultSeverity::Medium);
        assert_eq!(sovd.status, SovdFaultStatus::Pending);
    }
}
