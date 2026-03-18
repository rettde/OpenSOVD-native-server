// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// ComponentBackend — Abstraction over how the SOVD Server reaches components.
//
// Implementations:
//   SovdHttpBackend  — HTTP client forwarding to external CDA or SOVD server
//
// This follows the OpenSOVD architecture where the SOVD Server communicates
// with components via the SOVD Gateway, which dispatches to adapters (CDA)
// or native SOVD endpoints.
// ─────────────────────────────────────────────────────────────────────────────

use async_trait::async_trait;

use crate::sovd::{
    SovdBulkDataItem, SovdBulkWriteItem, SovdCapabilities, SovdComponent, SovdComponentConfig,
    SovdDataCatalogEntry, SovdFault, SovdGroup, SovdMode, SovdOperation,
};
use crate::DiagServiceError;

/// Backend abstraction for reaching diagnostic components.
///
/// Each backend manages a set of components and provides the full SOVD
/// capability surface for them. The `ComponentRouter` aggregates multiple
/// backends (e.g. one local UDS backend + one HTTP backend to an external CDA).
#[async_trait]
pub trait ComponentBackend: Send + Sync {
    /// Human-readable label for logging / diagnostics
    fn name(&self) -> &str;

    // ── Discovery ───────────────────────────────────────────────────────────

    /// List all components this backend manages
    fn list_components(&self) -> Vec<SovdComponent>;

    /// Get a single component by ID (returns None if not managed by this backend)
    fn get_component(&self, component_id: &str) -> Option<SovdComponent>;

    /// Whether this backend manages the given component
    fn handles_component(&self, component_id: &str) -> bool {
        self.get_component(component_id).is_some()
    }

    // ── Connection lifecycle ────────────────────────────────────────────────

    /// Connect to a component (establish communication channel)
    async fn connect(&self, component_id: &str) -> Result<(), DiagServiceError>;

    /// Disconnect from a component
    async fn disconnect(&self, component_id: &str) -> Result<(), DiagServiceError>;

    // ── Data (SOVD §7.5) ───────────────────────────────────────────────────

    /// List available data identifiers for a component.
    ///
    /// # Errors
    /// Returns `DiagServiceError::NotFound` if the component is unknown.
    fn list_data(&self, component_id: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError>;

    /// Read a data value from a component
    async fn read_data(
        &self,
        component_id: &str,
        data_id: &str,
    ) -> Result<serde_json::Value, DiagServiceError>;

    /// Write a data value to a component
    async fn write_data(
        &self,
        component_id: &str,
        data_id: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError>;

    // ── Faults (SOVD §7.5) ─────────────────────────────────────────────────

    /// Read faults from a component
    async fn read_faults(&self, component_id: &str) -> Result<Vec<SovdFault>, DiagServiceError>;

    /// Clear all faults on a component
    async fn clear_faults(&self, component_id: &str) -> Result<(), DiagServiceError>;

    // ── Operations (SOVD §7.7) ─────────────────────────────────────────────

    /// List available operations for a component.
    ///
    /// # Errors
    /// Returns `DiagServiceError::NotFound` if the component is unknown.
    fn list_operations(&self, component_id: &str) -> Result<Vec<SovdOperation>, DiagServiceError>;

    /// Execute an operation on a component, returns result as JSON
    async fn execute_operation(
        &self,
        component_id: &str,
        operation_id: &str,
        params: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError>;

    // ── Capabilities (SOVD §7.3) ───────────────────────────────────────────

    /// Get capabilities for a component.
    ///
    /// # Errors
    /// Returns `DiagServiceError::NotFound` if the component is unknown.
    fn get_capabilities(&self, component_id: &str) -> Result<SovdCapabilities, DiagServiceError>;

    // ── Mode / Session (SOVD §7.6) ─────────────────────────────────────────

    /// Get current diagnostic mode.
    ///
    /// # Errors
    /// Returns `DiagServiceError::NotFound` if the component is unknown.
    fn get_mode(&self, component_id: &str) -> Result<SovdMode, DiagServiceError>;

    /// Set diagnostic mode (e.g. "default", "extended", "programming")
    async fn set_mode(&self, component_id: &str, mode: &str) -> Result<(), DiagServiceError>;

    // ── Configuration (SOVD §7.8) ──────────────────────────────────────────

    /// Read component configuration
    async fn read_config(
        &self,
        component_id: &str,
    ) -> Result<SovdComponentConfig, DiagServiceError>;

    /// Write a configuration parameter
    async fn write_config(
        &self,
        component_id: &str,
        param_name: &str,
        value: &[u8],
    ) -> Result<(), DiagServiceError>;

    // ── Bulk Data (SOVD §7.5.3) ────────────────────────────────────────────

    /// Bulk read multiple data identifiers
    async fn bulk_read(
        &self,
        component_id: &str,
        data_ids: &[String],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError>;

    /// Bulk write multiple data identifiers
    async fn bulk_write(
        &self,
        component_id: &str,
        items: &[SovdBulkWriteItem],
    ) -> Result<Vec<SovdBulkDataItem>, DiagServiceError>;

    // ── Groups (SOVD §7.2) ─────────────────────────────────────────────────

    /// List all component groups this backend manages
    fn list_groups(&self) -> Vec<SovdGroup>;

    /// Get a single group by ID
    fn get_group(&self, group_id: &str) -> Option<SovdGroup>;

    // ── Extended Diagnostics (vendor extensions, x-prefixed) ────────────────

    /// I/O control on a data identifier
    async fn io_control(
        &self,
        component_id: &str,
        data_id: &str,
        control: &str,
        value: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError>;

    /// Communication control (enable/disable Rx/Tx)
    async fn communication_control(
        &self,
        component_id: &str,
        control_type: &str,
        communication_type: u8,
    ) -> Result<(), DiagServiceError>;

    /// DTC setting control (on/off)
    async fn dtc_setting(&self, component_id: &str, setting: &str) -> Result<(), DiagServiceError>;

    /// Read raw memory from a component
    async fn read_memory(
        &self,
        component_id: &str,
        address: u32,
        size: u32,
    ) -> Result<Vec<u8>, DiagServiceError>;

    /// Write raw memory to a component
    async fn write_memory(
        &self,
        component_id: &str,
        address: u32,
        data: &[u8],
    ) -> Result<(), DiagServiceError>;

    /// Flash firmware to a component, returns result as JSON
    async fn flash(
        &self,
        component_id: &str,
        firmware: &[u8],
        memory_address: u32,
    ) -> Result<serde_json::Value, DiagServiceError>;

    // ── Keepalive / status ──────────────────────────────────────────────────

    /// Get list of component IDs with active keepalive sessions
    fn active_keepalives(&self) -> Vec<String> {
        vec![]
    }
}
