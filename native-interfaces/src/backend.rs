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
    SovdArea, SovdBulkDataCategory, SovdBulkDataItem, SovdBulkWriteItem, SovdCapabilities,
    SovdComponent, SovdComponentConfig, SovdDataCatalogEntry, SovdFault, SovdGroup, SovdMode,
    SovdOperation, SovdSoftwarePackage,
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

    /// Bulk read multiple data identifiers.
    /// `category` optionally filters by data category (MBDS §7.5.3): currentData, logs, trigger.
    async fn bulk_read(
        &self,
        component_id: &str,
        data_ids: &[String],
        category: Option<SovdBulkDataCategory>,
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

    // ── Software Packages (SOVD §5.5.10) ────────────────────────────────────

    /// List available software packages for a component
    fn list_software_packages(
        &self,
        _component_id: &str,
    ) -> Result<Vec<SovdSoftwarePackage>, DiagServiceError> {
        Ok(vec![])
    }

    /// Initiate installation of a software package
    async fn install_software_package(
        &self,
        _component_id: &str,
        _package_id: &str,
    ) -> Result<SovdSoftwarePackage, DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "software-packages not supported by this backend".into(),
        ))
    }

    /// Activate an installed software package (make it the running version)
    async fn activate_software_package(
        &self,
        _component_id: &str,
        _package_id: &str,
    ) -> Result<SovdSoftwarePackage, DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "activate not supported by this backend".into(),
        ))
    }

    /// Rollback a software package to its previous version
    async fn rollback_software_package(
        &self,
        _component_id: &str,
        _package_id: &str,
    ) -> Result<SovdSoftwarePackage, DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "rollback not supported by this backend".into(),
        ))
    }

    /// Get detailed status of a specific software package
    fn get_software_package_status(
        &self,
        _component_id: &str,
        _package_id: &str,
    ) -> Result<SovdSoftwarePackage, DiagServiceError> {
        Err(DiagServiceError::NotFound(Some("package not found".into())))
    }
}

// ── Extended Diagnostics (vendor extensions, x-prefixed) ─────────────────

/// Backend abstraction for UDS-specific vendor extension methods.
///
/// Extracted from `ComponentBackend` (A2.2 trait diet) so that non-UDS
/// backends (e.g. cloud adapters, KPI providers) don't need to stub out
/// 7 methods they'll never support.  All methods have default "not
/// supported" implementations.
///
/// Routes under `/sovd/v1/x-uds/…` use this trait.
#[async_trait]
pub trait ExtendedDiagBackend: Send + Sync {
    /// Whether this backend handles extended diagnostics for the given component.
    /// Used by the router to dispatch x-uds requests.
    fn handles_component(&self, _component_id: &str) -> bool {
        false
    }

    /// I/O control on a data identifier
    async fn io_control(
        &self,
        _component_id: &str,
        _data_id: &str,
        _control: &str,
        _value: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "io_control not supported by this backend".into(),
        ))
    }

    /// Communication control (enable/disable Rx/Tx)
    async fn communication_control(
        &self,
        _component_id: &str,
        _control_type: &str,
        _communication_type: u8,
    ) -> Result<(), DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "communication_control not supported by this backend".into(),
        ))
    }

    /// DTC setting control (on/off)
    async fn dtc_setting(
        &self,
        _component_id: &str,
        _setting: &str,
    ) -> Result<(), DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "dtc_setting not supported by this backend".into(),
        ))
    }

    /// Read raw memory from a component
    async fn read_memory(
        &self,
        _component_id: &str,
        _address: u32,
        _size: u32,
    ) -> Result<Vec<u8>, DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "read_memory not supported by this backend".into(),
        ))
    }

    /// Write raw memory to a component
    async fn write_memory(
        &self,
        _component_id: &str,
        _address: u32,
        _data: &[u8],
    ) -> Result<(), DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "write_memory not supported by this backend".into(),
        ))
    }

    /// Flash firmware to a component, returns result as JSON
    async fn flash(
        &self,
        _component_id: &str,
        _firmware: &[u8],
        _memory_address: u32,
    ) -> Result<serde_json::Value, DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "flash not supported by this backend".into(),
        ))
    }

    /// Get list of component IDs with active keepalive sessions
    fn active_keepalives(&self) -> Vec<String> {
        vec![]
    }
}

// ── Entity Backend (ISO 17978-3 §4.2.3: Apps + Funcs) ────────────────────

use crate::sovd::{SovdApp, SovdFunc};

/// Backend abstraction for non-component SOVD entities (apps, funcs).
///
/// Separate from `ComponentBackend` to keep the component trait focused.
/// Default implementations return empty collections / not-found — backends
/// only override what they support.
#[async_trait]
pub trait EntityBackend: Send + Sync {
    // ── Apps ───────────────────────────────────────────────────────────────

    /// List all diagnostic applications
    fn list_apps(&self) -> Vec<SovdApp> {
        vec![]
    }

    /// Get a single app by ID
    fn get_app(&self, _app_id: &str) -> Option<SovdApp> {
        None
    }

    /// List data catalog for an app
    fn list_app_data(&self, _app_id: &str) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
        Ok(vec![])
    }

    /// Read a data value from an app
    async fn read_app_data(
        &self,
        _app_id: &str,
        _data_id: &str,
    ) -> Result<serde_json::Value, DiagServiceError> {
        Err(DiagServiceError::NotFound(Some(
            "app data not found".into(),
        )))
    }

    /// List operations available on an app
    fn list_app_operations(&self, _app_id: &str) -> Result<Vec<SovdOperation>, DiagServiceError> {
        Ok(vec![])
    }

    /// Execute an operation on an app
    async fn execute_app_operation(
        &self,
        _app_id: &str,
        _op_id: &str,
        _params: Option<&[u8]>,
    ) -> Result<serde_json::Value, DiagServiceError> {
        Err(DiagServiceError::RequestNotSupported(
            "app operations not supported".into(),
        ))
    }

    /// Get capabilities for an app
    fn get_app_capabilities(&self, _app_id: &str) -> Result<SovdCapabilities, DiagServiceError> {
        Err(DiagServiceError::NotFound(Some("app not found".into())))
    }

    // ── Funcs ──────────────────────────────────────────────────────────────

    /// List all cross-component diagnostic functions
    fn list_funcs(&self) -> Vec<SovdFunc> {
        vec![]
    }

    /// Get a single func by ID
    fn get_func(&self, _func_id: &str) -> Option<SovdFunc> {
        None
    }

    /// List data catalog for a func
    fn list_func_data(
        &self,
        _func_id: &str,
    ) -> Result<Vec<SovdDataCatalogEntry>, DiagServiceError> {
        Ok(vec![])
    }

    /// Read a data value from a func
    async fn read_func_data(
        &self,
        _func_id: &str,
        _data_id: &str,
    ) -> Result<serde_json::Value, DiagServiceError> {
        Err(DiagServiceError::NotFound(Some(
            "func data not found".into(),
        )))
    }

    // ── Areas (gated by DiscoveryPolicy::areas_enabled) ───────────────────

    /// List all E/E architecture areas
    fn list_areas(&self) -> Vec<SovdArea> {
        vec![]
    }

    /// Get a single area by ID
    fn get_area(&self, _area_id: &str) -> Option<SovdArea> {
        None
    }
}
