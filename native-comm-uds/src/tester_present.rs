// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// TesterPresent keepalive — periodically sends UDS 0x3E to keep
// diagnostic sessions alive
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use native_interfaces::TesterPresentType;

use super::UdsManager;

/// TesterPresent background task
pub struct TesterPresentTask {
    pub type_: TesterPresentType,
    pub task: JoinHandle<()>,
}

impl TesterPresentTask {
    /// Spawn a TesterPresent keepalive task for a specific ECU
    pub fn spawn_for_ecu(ecu_name: String, uds: Arc<UdsManager>, interval: Duration) -> Self {
        let type_ = TesterPresentType::Ecu(ecu_name.clone());
        let task = tokio::spawn(async move {
            info!(ecu = %ecu_name, interval_ms = interval.as_millis(), "TesterPresent task started");
            let mut ticker = tokio::time::interval(interval);

            loop {
                ticker.tick().await;
                match uds.tester_present(true).await {
                    Ok(()) => debug!(ecu = %ecu_name, "TesterPresent OK"),
                    Err(e) => warn!(ecu = %ecu_name, error = %e, "TesterPresent failed"),
                }
            }
        });

        Self { type_, task }
    }

    /// Stop the keepalive task
    pub fn stop(self) {
        self.task.abort();
        info!(type_ = ?self.type_, "TesterPresent task stopped");
    }
}
